use myrss::app::AppImpl;
use myrss::modes::Selected;
use std::sync::mpsc::channel;

#[test]
fn test_article_scrolling_bounds_regression() {
    let (event_tx, _) = channel();
    let (io_tx, _) = channel();
    let options = myrss::ReadOptions {
        database_path: std::path::PathBuf::from(":memory:"),
        tick_rate: 100,
        flash_display_duration_seconds: std::time::Duration::from_secs(1),
        network_timeout: std::time::Duration::from_secs(1),
    };
    let mut app = AppImpl::new(options, event_tx, io_tx).unwrap();

    // Configure a dummy entry selected state
    let dummy_metadata = myrss::rss::EntryMetadata {
        id: myrss::rss::EntryId::from(1),
        feed_id: myrss::rss::FeedId::from(1),
        title: Some("Dummy Title".to_string()),
        pub_date: None,
        link: Some("http://example.com".to_string()),
        read_at: None,
        inserted_at: chrono::Utc::now(),
        noteworthy: false,
        newly_added: false,
    };
    app.selected = Selected::Entry(dummy_metadata);
    app.entry_lines_len = 100; // total lines of article
    app.entry_lines_rendered_len = 25; // visible viewport size

    // Test initial state
    assert_eq!(app.entry_scroll_position, 0);

    // 1. Test snap to bottom ('G')
    app.on_snap_to_bottom().unwrap();
    // Capped position: 100 - 25 = 75
    assert_eq!(app.entry_scroll_position, 75);

    // 2. Test that pressing down ('j' or 'on_down') does not scroll past bottom cap
    app.on_down().unwrap();
    assert_eq!(app.entry_scroll_position, 75);

    // 3. Test page down capping bounds
    app.entry_scroll_position = 60; // reset to 60
    app.page_down();
    // 60 + 25 = 85 -> capped at 75
    assert_eq!(app.entry_scroll_position, 75);

    // 4. Test page down when far from bottom
    app.entry_scroll_position = 10;
    app.page_down();
    // 10 + 25 = 35 -> not capped
    assert_eq!(app.entry_scroll_position, 35);
}
