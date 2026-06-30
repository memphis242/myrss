//! Render tests for the chat modal (`Mode::Chat`), via `TestBackend`.
//!
//! Integration: render `ui::draw` in chat mode and assert the conversation,
//! input box, and status/title chrome appear in the terminal buffer; and that
//! scrolling changes which history content is visible.

use myrss::app::{AppImpl, ChatTurn};
use myrss::modes::Mode;
use myrss::rss::{EntryId, EntryMetadata, FeedId};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
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

fn entry_meta() -> EntryMetadata {
    EntryMetadata {
        id: EntryId::from(1),
        feed_id: FeedId::from(1),
        title: Some("Rust News".to_string()),
        pub_date: None,
        link: Some("http://example.com".to_string()),
        read_at: None,
        inserted_at: chrono::Utc::now(),
        noteworthy: false,
        newly_added: false,
    }
}

fn render_to_string(app: &mut AppImpl, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let chunks = myrss::ui::predraw(f, app.mode);
            myrss::ui::draw(f, chunks, app);
        })
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
        }
        out.push('\n');
    }
    out
}

#[test]
fn chat_modal_renders_history_input_and_chrome() {
    let mut app = make_app();
    app.current_entry_meta = Some(entry_meta());
    app.mode = Mode::Chat(EntryId::from(1));
    app.chat_messages = vec![
        ChatTurn {
            role: "user".to_string(),
            content: "what is this about".to_string(),
        },
        ChatTurn {
            role: "assistant".to_string(),
            content: "it is about rust".to_string(),
        },
    ];
    app.chat_input = "follow up question".to_string();

    let rendered = render_to_string(&mut app, 80, 24);

    // Title from the entry, role labels, message bodies, and the input draft.
    assert!(rendered.contains("Chat"), "missing chat title");
    assert!(rendered.contains("Rust News"), "missing article title");
    assert!(rendered.contains("You:"), "missing user label");
    assert!(
        rendered.contains("what is this about"),
        "missing user message"
    );
    assert!(rendered.contains("Assistant:"), "missing assistant label");
    assert!(
        rendered.contains("it is about rust"),
        "missing assistant message"
    );
    assert!(rendered.contains("Message"), "missing input box title");
    assert!(
        rendered.contains("follow up question"),
        "missing input draft"
    );
}

#[test]
fn chat_modal_empty_shows_placeholder_hint() {
    let mut app = make_app();
    app.current_entry_meta = Some(entry_meta());
    app.mode = Mode::Chat(EntryId::from(1));

    let rendered = render_to_string(&mut app, 80, 24);
    assert!(
        rendered.contains("Ask a question about this article"),
        "empty chat should show the placeholder hint"
    );
}

#[test]
fn chat_scroll_changes_visible_history() {
    let mut app = make_app();
    app.current_entry_meta = Some(entry_meta());
    app.mode = Mode::Chat(EntryId::from(1));
    // Enough turns to overflow the history viewport on a short terminal.
    app.chat_messages = (0..40)
        .map(|i| ChatTurn {
            role: "assistant".to_string(),
            content: format!("UNIQUELINE{i}"),
        })
        .collect();

    let top = render_to_string(&mut app, 80, 16);
    app.chat_scroll_position = 30;
    let scrolled = render_to_string(&mut app, 80, 16);

    // Unscrolled, the first turn is visible. After scrolling well past it, it is
    // no longer in the viewport and the rendered buffer differs.
    assert!(
        top.contains("UNIQUELINE0"),
        "first line should show unscrolled"
    );
    assert!(
        !scrolled.contains("UNIQUELINE0"),
        "scrolling should move the first line out of view"
    );
    assert_ne!(top, scrolled, "scrolling must change the rendered viewport");
}
