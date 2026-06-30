use crate::rss::EntryId;
use rusqlite::{Connection, params};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct RequestLogEntry {
    pub id: i32,
    pub timestamp: i64,
    pub prompt: String,
    pub response: String,
    pub status_code: i32,
    pub finish_reason: String,
}

/// A cached chunk and its embedding vector: `(chunk_text, embedding)`.
pub type ChunkEmbedding = (String, Vec<f32>);

/// One persisted chat turn. `role` is the textual role (`system`/`user`/
/// `assistant`/`tool`); `tool_calls` holds JSON-encoded tool calls for an
/// assistant turn that requested tools, and `tool_call_id` links a tool-result
/// turn back to the call it answers. Both are `None` for ordinary text turns.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredChatMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<String>,
    pub tool_call_id: Option<String>,
    pub created_at: i64,
}

/// Returns the cache database path `~/.myrss/cache.db`.
pub fn cache_db_path() -> PathBuf {
    crate::settings::settings_dir().join("cache.db")
}

/// Initializes the TUI cache and logging tables.
pub fn initialize_cache_db() -> rusqlite::Result<()> {
    let dir = crate::settings::settings_dir();
    let _ = std::fs::create_dir_all(&dir);
    let conn = Connection::open(cache_db_path())?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS llm_cache (
            cache_key TEXT PRIMARY KEY,
            text_hash TEXT,
            prompt_hash TEXT,
            model TEXT,
            response TEXT,
            created_at INTEGER
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS request_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER,
            prompt TEXT,
            response TEXT,
            status_code INTEGER,
            finish_reason TEXT
        )",
        [],
    )?;

    create_chat_tables(&conn)?;

    // Stamp a schema version on the cache DB so a future *destructive* change has
    // a migration hook. v1 = llm_cache/request_log + the chat tables above.
    conn.pragma_update(None, "user_version", 1)?;

    Ok(())
}

/// Creates the chat-session, chat-message, and paragraph-embedding tables.
/// Factored out (taking `&Connection`) so tests can build the same schema on an
/// in-memory database without touching `~/.myrss/cache.db`.
pub(crate) fn create_chat_tables(conn: &Connection) -> rusqlite::Result<()> {
    // One ongoing chat per article, keyed by the entry id.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS chat_sessions (
            entry_id   INTEGER PRIMARY KEY,
            model      TEXT,
            created_at INTEGER,
            updated_at INTEGER
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS chat_messages (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            entry_id     INTEGER NOT NULL,
            role         TEXT NOT NULL,
            content      TEXT NOT NULL,
            tool_calls   TEXT,
            tool_call_id TEXT,
            created_at   INTEGER NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_chat_messages_entry
         ON chat_messages (entry_id, id)",
        [],
    )?;
    // Cached RAG vectors. `content_hash` ties the cache to the exact article text
    // that produced it, so an edited/re-fetched article re-embeds rather than
    // reusing stale vectors. `embedding` is a little-endian f32 blob.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS paragraph_embeddings (
            entry_id     INTEGER NOT NULL,
            content_hash TEXT NOT NULL,
            chunk_index  INTEGER NOT NULL,
            chunk_text   TEXT NOT NULL,
            embedding    BLOB NOT NULL,
            model        TEXT NOT NULL,
            PRIMARY KEY (entry_id, content_hash, chunk_index)
        )",
        [],
    )?;
    Ok(())
}

pub(crate) fn compute_hash<T: Hash>(t: &T) -> String {
    let mut s = DefaultHasher::new();
    t.hash(&mut s);
    format!("{:x}", s.finish())
}

pub const PROMPT_VERSION: &str = "v1";

/// Checks the cache for an existing summary matching the article text, model, and system prompt.
pub fn get_cached_summary(
    text: &str,
    model: &str,
    system_prompt: &str,
) -> anyhow::Result<Option<String>> {
    let conn = Connection::open(cache_db_path())?;
    let cache_key = compute_hash(&(PROMPT_VERSION.to_string() + text + model + system_prompt));

    let mut stmt = conn.prepare("SELECT response FROM llm_cache WHERE cache_key = ?1")?;
    let mut rows = stmt.query([cache_key])?;
    if let Some(row) = rows.next()? {
        let response: String = row.get(0)?;
        Ok(Some(response))
    } else {
        Ok(None)
    }
}

/// Writes a summary response to the SQLite cache table.
pub fn insert_cached_summary(
    text: &str,
    model: &str,
    system_prompt: &str,
    response: &str,
) -> anyhow::Result<()> {
    let conn = Connection::open(cache_db_path())?;
    let text_hash = compute_hash(&text);
    let prompt_hash = compute_hash(&system_prompt);
    let cache_key = compute_hash(&(PROMPT_VERSION.to_string() + text + model + system_prompt));
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "INSERT OR REPLACE INTO llm_cache (cache_key, text_hash, prompt_hash, model, response, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![cache_key, text_hash, prompt_hash, model, response, now],
    )?;

    Ok(())
}

/// Checks if the daily rate limit (24 hours window) is exceeded.
pub fn check_daily_rate_limit(max_per_day: u32) -> anyhow::Result<bool> {
    let conn = Connection::open(cache_db_path())?;
    let now = chrono::Utc::now().timestamp();
    let one_day_ago = now - 86400;

    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM request_log WHERE timestamp > ?1",
        [one_day_ago],
        |r| r.get(0),
    )?;

    Ok(count < max_per_day)
}

/// Logs a request to the database.
pub fn log_request(
    prompt: &str,
    response: &str,
    status_code: i32,
    finish_reason: &str,
) -> anyhow::Result<()> {
    let conn = Connection::open(cache_db_path())?;
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "INSERT INTO request_log (timestamp, prompt, response, status_code, finish_reason)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![now, prompt, response, status_code, finish_reason],
    )?;

    Ok(())
}

/// Retrieves up to the 100 most recent LLM logs.
pub fn get_request_logs() -> anyhow::Result<Vec<RequestLogEntry>> {
    let conn = Connection::open(cache_db_path())?;
    let mut stmt = conn.prepare(
        "SELECT id, timestamp, prompt, response, status_code, finish_reason 
         FROM request_log 
         ORDER BY id DESC 
         LIMIT 100",
    )?;
    let entries = stmt
        .query_map([], |row| {
            Ok(RequestLogEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                prompt: row.get(2)?,
                response: row.get(3)?,
                status_code: row.get(4)?,
                finish_reason: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}

/// Deletes all cached summaries from the SQLite cache table.
pub fn clear_cache() -> anyhow::Result<()> {
    let conn = Connection::open(cache_db_path())?;
    conn.execute("DELETE FROM llm_cache", [])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Chat persistence
//
// Each public function opens a fresh connection (matching the rest of this
// module) and delegates to a `*_conn` helper that takes `&Connection`, so the
// SQL is unit-testable against an in-memory database.
// ---------------------------------------------------------------------------

/// Ensures a chat session row exists for `entry_id`; a no-op if one already does
/// (the existing `model`/timestamps are left untouched).
pub fn get_or_create_chat_session(entry_id: EntryId, model: &str) -> anyhow::Result<()> {
    let conn = Connection::open(cache_db_path())?;
    get_or_create_chat_session_conn(&conn, entry_id, model)
}

fn get_or_create_chat_session_conn(
    conn: &Connection,
    entry_id: EntryId,
    model: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO chat_sessions (entry_id, model, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(entry_id) DO NOTHING",
        params![entry_id, model, now],
    )?;
    Ok(())
}

/// Loads all messages for an article's chat, oldest first.
pub fn load_chat_messages(entry_id: EntryId) -> anyhow::Result<Vec<StoredChatMessage>> {
    let conn = Connection::open(cache_db_path())?;
    load_chat_messages_conn(&conn, entry_id)
}

fn load_chat_messages_conn(
    conn: &Connection,
    entry_id: EntryId,
) -> anyhow::Result<Vec<StoredChatMessage>> {
    let mut stmt = conn.prepare(
        "SELECT role, content, tool_calls, tool_call_id, created_at
         FROM chat_messages WHERE entry_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map([entry_id], |row| {
            Ok(StoredChatMessage {
                role: row.get(0)?,
                content: row.get(1)?,
                tool_calls: row.get(2)?,
                tool_call_id: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Appends a single message to an article's chat (crash-safe: each turn is
/// persisted as it completes).
pub fn append_chat_message(entry_id: EntryId, msg: &StoredChatMessage) -> anyhow::Result<()> {
    let conn = Connection::open(cache_db_path())?;
    append_chat_message_conn(&conn, entry_id, msg)
}

fn append_chat_message_conn(
    conn: &Connection,
    entry_id: EntryId,
    msg: &StoredChatMessage,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO chat_messages (entry_id, role, content, tool_calls, tool_call_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            entry_id,
            msg.role,
            msg.content,
            msg.tool_calls,
            msg.tool_call_id,
            msg.created_at
        ],
    )?;
    Ok(())
}

/// Bumps a chat session's `updated_at` to now.
pub fn touch_chat_session(entry_id: EntryId) -> anyhow::Result<()> {
    let conn = Connection::open(cache_db_path())?;
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE chat_sessions SET updated_at = ?2 WHERE entry_id = ?1",
        params![entry_id, now],
    )?;
    Ok(())
}

/// Returns cached `(chunk_text, embedding)` pairs for an article's exact text
/// (`content_hash`), in chunk order, or `None` if nothing is cached for that
/// pair (so the caller embeds and inserts).
pub fn get_paragraph_embeddings(
    entry_id: EntryId,
    content_hash: &str,
) -> anyhow::Result<Option<Vec<ChunkEmbedding>>> {
    let conn = Connection::open(cache_db_path())?;
    get_paragraph_embeddings_conn(&conn, entry_id, content_hash)
}

fn get_paragraph_embeddings_conn(
    conn: &Connection,
    entry_id: EntryId,
    content_hash: &str,
) -> anyhow::Result<Option<Vec<ChunkEmbedding>>> {
    let mut stmt = conn.prepare(
        "SELECT chunk_text, embedding FROM paragraph_embeddings
         WHERE entry_id = ?1 AND content_hash = ?2 ORDER BY chunk_index ASC",
    )?;
    let rows = stmt
        .query_map(params![entry_id, content_hash], |row| {
            let text: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((text, blob))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        rows.into_iter()
            .map(|(text, blob)| (text, le_bytes_to_f32s(&blob)))
            .collect(),
    ))
}

/// Caches the embedding vectors for an article's chunks, replacing any prior
/// rows for the same `(entry_id, content_hash)` so a recompute can't leave
/// stale duplicates.
pub fn insert_paragraph_embeddings(
    entry_id: EntryId,
    content_hash: &str,
    model: &str,
    chunks: &[ChunkEmbedding],
) -> anyhow::Result<()> {
    let mut conn = Connection::open(cache_db_path())?;
    insert_paragraph_embeddings_conn(&mut conn, entry_id, content_hash, model, chunks)
}

fn insert_paragraph_embeddings_conn(
    conn: &mut Connection,
    entry_id: EntryId,
    content_hash: &str,
    model: &str,
    chunks: &[ChunkEmbedding],
) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM paragraph_embeddings WHERE entry_id = ?1 AND content_hash = ?2",
        params![entry_id, content_hash],
    )?;
    for (index, (text, embedding)) in chunks.iter().enumerate() {
        let blob = f32s_to_le_bytes(embedding);
        tx.execute(
            "INSERT INTO paragraph_embeddings
             (entry_id, content_hash, chunk_index, chunk_text, embedding, model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![entry_id, content_hash, index as i64, text, blob, model],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Deletes ALL chat data (sessions, messages, cached embeddings). Backs the
/// `:clear_chat` command; deliberately separate from `clear_cache` so wiping
/// summaries never wipes chats and vice-versa.
pub fn clear_chat_history() -> anyhow::Result<()> {
    let conn = Connection::open(cache_db_path())?;
    clear_chat_history_conn(&conn)
}

fn clear_chat_history_conn(conn: &Connection) -> anyhow::Result<()> {
    conn.execute("DELETE FROM chat_messages", [])?;
    conn.execute("DELETE FROM chat_sessions", [])?;
    conn.execute("DELETE FROM paragraph_embeddings", [])?;
    Ok(())
}

/// Serializes f32 vectors to a little-endian byte blob for SQLite storage.
fn f32s_to_le_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for &v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Inverse of [`f32s_to_le_bytes`]. A trailing partial chunk (corrupt blob) is
/// dropped by `chunks_exact` rather than panicking.
fn le_bytes_to_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_and_logging() {
        // Setup temporary cache database for unit test
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE llm_cache (
                cache_key TEXT PRIMARY KEY,
                text_hash TEXT,
                prompt_hash TEXT,
                model TEXT,
                response TEXT,
                created_at INTEGER
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE request_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER,
                prompt TEXT,
                response TEXT,
                status_code INTEGER,
                finish_reason TEXT
            )",
            [],
        )
        .unwrap();

        // Check cache empty
        let cache_key = compute_hash(&("hello".to_string() + "model" + "prompt"));
        let count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_cache WHERE cache_key = ?1",
                [&cache_key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);

        // Check insert log
        conn.execute(
            "INSERT INTO request_log (timestamp, prompt, response, status_code, finish_reason)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![123456, "prompt", "response", 200, "stop"],
        )
        .unwrap();

        let logged: String = conn
            .query_row(
                "SELECT response FROM request_log WHERE timestamp = 123456",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(logged, "response");
    }

    fn msg(role: &str, content: &str, at: i64) -> StoredChatMessage {
        StoredChatMessage {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
            created_at: at,
        }
    }

    #[test]
    fn test_chat_session_and_message_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        create_chat_tables(&conn).unwrap();
        let entry = EntryId::from(42);

        // Creating the session twice is idempotent (no duplicate / no error).
        get_or_create_chat_session_conn(&conn, entry, "openai/gpt-4o-mini").unwrap();
        get_or_create_chat_session_conn(&conn, entry, "openai/gpt-4o-mini").unwrap();
        let session_count: u32 = conn
            .query_row("SELECT COUNT(*) FROM chat_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(session_count, 1);

        // A full mixed-role exchange, including tool_calls / tool_call_id, must
        // round-trip in insertion order.
        let user = msg("user", "What does the article claim?", 1);
        let assistant = StoredChatMessage {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(r#"[{"id":"call_1","name":"web_search"}]"#.to_string()),
            tool_call_id: None,
            created_at: 2,
        };
        let tool = StoredChatMessage {
            role: "tool".to_string(),
            content: "search results".to_string(),
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            created_at: 3,
        };
        let final_answer = msg("assistant", "Here is the answer.", 4);
        for m in [&user, &assistant, &tool, &final_answer] {
            append_chat_message_conn(&conn, entry, m).unwrap();
        }

        let loaded = load_chat_messages_conn(&conn, entry).unwrap();
        assert_eq!(loaded, vec![user, assistant, tool, final_answer]);

        // A different article has its own independent (empty) history.
        assert!(
            load_chat_messages_conn(&conn, EntryId::from(7))
                .unwrap()
                .is_empty()
        );

        // clear_chat_history wipes everything.
        clear_chat_history_conn(&conn).unwrap();
        assert!(load_chat_messages_conn(&conn, entry).unwrap().is_empty());
        let session_count: u32 = conn
            .query_row("SELECT COUNT(*) FROM chat_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(session_count, 0);
    }

    #[test]
    fn test_paragraph_embeddings_roundtrip() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_chat_tables(&conn).unwrap();
        let entry = EntryId::from(1);

        // Miss before anything is cached.
        assert!(
            get_paragraph_embeddings_conn(&conn, entry, "hashA")
                .unwrap()
                .is_none()
        );

        let chunks = vec![
            ("first paragraph".to_string(), vec![0.1f32, -0.2, 0.3]),
            ("second paragraph".to_string(), vec![1.0f32, 0.5]),
        ];
        insert_paragraph_embeddings_conn(&mut conn, entry, "hashA", "minilm", &chunks).unwrap();

        // f32 blobs round-trip bit-for-bit and preserve chunk order.
        let got = get_paragraph_embeddings_conn(&conn, entry, "hashA")
            .unwrap()
            .unwrap();
        assert_eq!(got, chunks);

        // A different content hash (edited article) is a cache miss.
        assert!(
            get_paragraph_embeddings_conn(&conn, entry, "hashB")
                .unwrap()
                .is_none()
        );

        // Re-inserting the same (entry, hash) replaces rather than duplicating.
        let replacement = vec![("merged paragraph".to_string(), vec![0.9f32])];
        insert_paragraph_embeddings_conn(&mut conn, entry, "hashA", "minilm", &replacement)
            .unwrap();
        assert_eq!(
            get_paragraph_embeddings_conn(&conn, entry, "hashA")
                .unwrap()
                .unwrap(),
            replacement
        );
    }

    #[test]
    fn test_f32_le_bytes_roundtrip() {
        let v = vec![0.0f32, 1.0, -1.5, f32::MIN, f32::MAX, 123.456];
        assert_eq!(le_bytes_to_f32s(&f32s_to_le_bytes(&v)), v);
        // A corrupt trailing partial f32 is dropped, not panicked on.
        let mut bytes = f32s_to_le_bytes(&[1.0, 2.0]);
        bytes.push(0xAB);
        assert_eq!(le_bytes_to_f32s(&bytes), vec![1.0, 2.0]);
    }
}
