use myrss::app::AppImpl;
use std::sync::mpsc::channel;

#[test]
fn test_cached_summary_loading_and_retention() {
    let _ = myrss::cache::initialize_cache_db();
    let (event_tx, _) = channel();
    let (io_tx, _) = channel();

    let options = myrss::ReadOptions {
        database_path: std::path::PathBuf::from(":memory:"),
        tick_rate: 100,
        flash_display_duration_seconds: std::time::Duration::from_secs(1),
        network_timeout: std::time::Duration::from_secs(1),
    };
    let mut app = AppImpl::new(options, event_tx, io_tx).unwrap();

    // Configure settings model name
    app.update_settings(|s| {
        s.model_name = "test-summary-model".to_string();
        s.max_words_per_prompt = 100;
    });

    let article_text = "This is the content of the article that has a pre-computed summary.";
    let expected_summary = "Pre-computed summary of the article.";

    // Insert summary into cache
    let prompt_payload = format!("<article_text>\n{}\n</article_text>", article_text);
    myrss::cache::insert_cached_summary(
        &prompt_payload,
        "test-summary-model",
        myrss::llm::SYSTEM_PROMPT,
        expected_summary,
    )
    .unwrap();

    // 1. Verify get_cached_summary_for_text helper loads it
    let settings = app.settings.clone();
    let loaded = myrss::llm::get_cached_summary_for_text(article_text, &settings);
    assert_eq!(loaded, Some(expected_summary.to_string()));

    // 2. Verify set_entry_ascii_content automatically resolves the cached summary
    let dummy_metadata = myrss::rss::EntryMetadata {
        id: myrss::rss::EntryId::from(1),
        feed_id: myrss::rss::FeedId::from(1),
        title: Some("Title".to_string()),
        pub_date: None,
        link: Some("http://example.com".to_string()),
        read_at: None,
        inserted_at: chrono::Utc::now(),
        noteworthy: false,
        newly_added: false,
    };

    assert!(app.current_summary().is_none());
    app.set_entry_ascii_content(article_text.to_string(), dummy_metadata);
    assert_eq!(app.current_summary(), Some(expected_summary.to_string()));

    // 3. Verify on_left clears current_summary
    app.on_left().unwrap();
    assert!(app.current_summary().is_none());
}
