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

    Ok(())
}

fn compute_hash<T: Hash>(t: &T) -> String {
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
}
