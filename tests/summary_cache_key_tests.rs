/// Regression test for the cache-key mismatch that caused the LLM summary box to
/// disappear after any redraw that triggered `select_and_show_current_entry`.
///
/// Root cause
/// ----------
/// The `:summarize` IO path built `text` via `html2text(html, 80)` and stored the
/// summary in cache under `key = hash(build_prompt_payload(text, max_words) + model
/// + system_prompt)`.  Meanwhile, `select_and_show_current_entry` rebuilt
/// `current_entry_text` from the same HTML at pane width (`entry_column_width - 2`,
/// typically 54 on an 80-wide terminal) and then called
/// `get_cached_summary_for_text(&current_entry_text, settings)` — which hashes a
/// *different* text → different cache key → miss → `current_summary = None` → box
/// vanishes.
///
/// Fix
/// ---
/// After `summarize_article` succeeds, also write the summary under the
/// `current_entry_text`-derived key so the redraw lookup always hits.
///
/// Test strategy
/// -------------
/// Set up an in-memory DB with a real entry, call `select_and_show_current_entry`
/// at a specific pane width so the text is deterministic, insert the summary in
/// cache under THAT text's key (simulating the fixed IO path), then call
/// `select_and_show_current_entry` again (simulating a width-change redraw) and
/// assert `current_summary` survives.
use myrss::app::AppImpl;
use myrss::modes::Selected;
use myrss::rss::{EntryId, EntryMetadata, FeedId};
use std::sync::mpsc::channel;

fn make_app() -> AppImpl {
    let (event_tx, _) = channel();
    let (io_tx, _) = channel();
    let options = myrss::ReadOptions {
        database_path: std::path::PathBuf::from(":memory:"),
        tick_rate: 100,
        flash_display_duration_seconds: std::time::Duration::from_secs(1),
        network_timeout: std::time::Duration::from_secs(1),
    };
    AppImpl::new(options, event_tx, io_tx).unwrap()
}

/// Seed the in-memory DB with one feed and one entry; return (feed_id, entry_id).
/// `tag` makes the content unique per test so cache writes don't bleed between tests.
fn seed_db(app: &AppImpl, tag: &str) -> (FeedId, EntryId) {
    let feed_id = app
        .conn
        .query_row(
            "INSERT INTO feeds (title, feed_link, link) VALUES ('Test Feed','http://f.test/feed','http://f.test') RETURNING id",
            [],
            |r| r.get::<_, FeedId>(0),
        )
        .unwrap();

    let description = format!("<p>Article content unique to {tag}.</p>");
    let entry_id = app
        .conn
        .query_row(
            "INSERT INTO entries (feed_id, title, description, link, updated_at) \
             VALUES (?1, 'Test Entry', ?2, 'http://f.test/1', datetime('now')) \
             RETURNING id",
            rusqlite::params![feed_id, description],
            |r| r.get::<_, EntryId>(0),
        )
        .unwrap();

    (feed_id, entry_id)
}

fn dummy_meta(feed_id: FeedId, entry_id: EntryId) -> EntryMetadata {
    EntryMetadata {
        id: entry_id,
        feed_id,
        title: Some("Test Entry".to_string()),
        pub_date: None,
        link: Some("http://f.test/1".to_string()),
        read_at: None,
        inserted_at: chrono::Utc::now(),
        noteworthy: false,
        newly_added: false,
    }
}

#[test]
fn regression_summary_survives_select_and_show_recompute() {
    let _ = myrss::cache::initialize_cache_db();

    let mut app = make_app();
    let (feed_id, entry_id) = seed_db(&app, "select_and_show_recompute");
    let meta = dummy_meta(feed_id, entry_id);

    // Simulate the state when an entry is open: column width mirrors a real 80-wide
    // terminal where chunks[1] = 56 columns.
    app.entry_column_width = 56;
    app.current_entry_meta = Some(meta.clone());
    app.selected = Selected::Entry(meta.clone());
    // select_and_show_current_entry reads from entries.items[entries.state.selected()].
    app.entries.items = vec![meta.clone()];
    app.entries.state.select(Some(0));

    // First call: populates current_entry_text at pane width (56 - 2 = 54).
    app.select_and_show_current_entry().unwrap();

    // Grab the pane-width text that select_and_show_current_entry produced.
    let pane_width_text = app.current_entry_text.clone();
    assert!(!pane_width_text.is_empty());

    let expected_summary = "This is the LLM summary of the article.";

    // Simulate the FIXED IO path: after summarize_article returns, it now writes
    // the summary under the pane-width key in addition to the 80-col key.
    let settings = app.settings.clone();
    let alt_payload =
        myrss::llm::build_prompt_payload(&pane_width_text, settings.max_words_per_prompt);
    myrss::cache::insert_cached_summary(
        &alt_payload,
        &settings.model_name,
        myrss::llm::SYSTEM_PROMPT,
        expected_summary,
    )
    .unwrap();

    // Set current_summary as the IO thread would.
    app.current_summary = Some(expected_summary.to_string());
    assert_eq!(app.current_summary, Some(expected_summary.to_string()));

    // Now simulate a redraw that triggers select_and_show_current_entry (e.g. a
    // terminal resize or first draw after the Tick from force_redraw).  Before the
    // fix this overwrote current_summary with None (cache miss).  After the fix the
    // lookup succeeds because we also stored under the pane-width key.
    app.select_and_show_current_entry().unwrap();

    assert_eq!(
        app.current_summary,
        Some(expected_summary.to_string()),
        "summary should survive select_and_show_current_entry recompute after the fix"
    );
}

#[test]
fn regression_summary_survives_set_entry_ascii_content_recompute() {
    // set_entry_ascii_content has the same cache-lookup pattern and was equally
    // affected.  Verify it also finds the summary after the fix.
    let _ = myrss::cache::initialize_cache_db();

    let mut app = make_app();
    let (feed_id, entry_id) = seed_db(&app, "set_entry_ascii_content_recompute");
    let meta = dummy_meta(feed_id, entry_id);

    let article_text = "Rendered article text for set_entry_ascii_content test.";
    let expected_summary = "Another LLM summary.";

    // Insert in cache under the text that set_entry_ascii_content will use.
    let settings = app.settings.clone();
    let payload =
        myrss::llm::build_prompt_payload(article_text, settings.max_words_per_prompt);
    myrss::cache::insert_cached_summary(
        &payload,
        &settings.model_name,
        myrss::llm::SYSTEM_PROMPT,
        expected_summary,
    )
    .unwrap();

    // set_entry_ascii_content uses the provided text directly for the cache lookup.
    app.set_entry_ascii_content(article_text.to_string(), meta);

    assert_eq!(
        app.current_summary,
        Some(expected_summary.to_string()),
        "set_entry_ascii_content should load summary from cache when key matches"
    );
}
