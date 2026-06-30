/// Regression tests for the LLM summary box truncation bug.
///
/// Bug: the summary box had a hard-coded height of 7 rows (5 visible content rows
/// + 2 border rows).  Any summary longer than 5 wrapped lines was silently cut off
/// — the text existed in memory but was never rendered to the terminal buffer.
///
/// Fix: `ui::summary_box_height` now computes the required height from the wrapped
/// line count and only caps at half the available area height.
///
/// Test levels
/// -----------
/// Integration – render `ui::draw` to a `TestBackend` and assert that every line
///   of the summary appears somewhere in the rendered buffer.
/// E2E         – simulate the exact terminal geometry that triggered the bug (80×24,
///   which gave only 5 visible content rows in the old 7-row box) and confirm that
///   a 6-line summary is now fully visible.
use myrss::app::AppImpl;
use myrss::modes::{Mode, Selected};
use myrss::rss::{EntryId, EntryMetadata, FeedId};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
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

fn dummy_entry_meta() -> EntryMetadata {
    EntryMetadata {
        id: EntryId::from(1),
        feed_id: FeedId::from(1),
        title: Some("Test Entry".to_string()),
        pub_date: None,
        link: Some("http://example.com".to_string()),
        read_at: None,
        inserted_at: chrono::Utc::now(),
        noteworthy: false,
        newly_added: false,
    }
}

/// Render `ui::draw` using `TestBackend` and return the concatenated string of
/// every cell in the buffer (one row per line).
fn render_to_string(app: &mut AppImpl, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let chunks = myrss::ui::predraw(f, app.mode.clone());
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

// ---------------------------------------------------------------------------
// Integration: a 6-line summary is fully rendered in the buffer
// ---------------------------------------------------------------------------

#[test]
fn integration_long_summary_all_lines_visible_in_buffer() {
    let mut app = make_app();
    app.mode = Mode::Normal;
    app.selected = Selected::Entry(dummy_entry_meta());
    app.current_entry_text = "Article body text.".to_string();

    let summary = "Line 1 of summary\nLine 2 of summary\nLine 3 of summary\nLine 4 of summary\nLine 5 of summary\nLine 6 of summary";
    app.current_summary = Some(summary.to_string());

    // 80×40 gives plenty of room — every line must appear.
    let rendered = render_to_string(&mut app, 80, 40);
    for line in summary.lines() {
        // Check a stable prefix to avoid failures from trailing cell padding.
        let prefix = &line[..line.len().min(12)];
        assert!(
            rendered.contains(prefix),
            "expected \"{prefix}\" in rendered buffer but it was absent (truncated)"
        );
    }
}

// ---------------------------------------------------------------------------
// Integration: a short summary (≤5 lines) is also fully rendered (no regression)
// ---------------------------------------------------------------------------

#[test]
fn integration_short_summary_still_renders_correctly() {
    let mut app = make_app();
    app.mode = Mode::Normal;
    app.selected = Selected::Entry(dummy_entry_meta());
    app.current_entry_text = "Article body text.".to_string();

    let summary = "Only one line";
    app.current_summary = Some(summary.to_string());

    let rendered = render_to_string(&mut app, 80, 40);
    assert!(
        rendered.contains("Only one"),
        "short summary should still appear in the rendered buffer"
    );
}

// ---------------------------------------------------------------------------
// E2E: reproduce the original bug geometry (80×24 terminal, 6-line summary)
// ---------------------------------------------------------------------------

#[test]
fn e2e_regression_6_line_summary_not_truncated_at_80x24() {
    // The old code used a fixed 7-row box (5 visible rows + 2 borders).  On an
    // 80×24 terminal the entry pane is ~23 rows tall; the box gave only 5 visible
    // content rows, so a 6-line summary had its last line silently cut off.
    let mut app = make_app();
    app.mode = Mode::Normal;
    app.selected = Selected::Entry(dummy_entry_meta());
    app.current_entry_text = "Article body.".to_string();

    // Craft a summary whose 6th line would have been invisible under the old code.
    let summary = "Summary line 1\nSummary line 2\nSummary line 3\nSummary line 4\nSummary line 5\nSummary line 6 - THIS WAS CUT OFF";
    app.current_summary = Some(summary.to_string());

    let rendered = render_to_string(&mut app, 80, 24);

    // The 6th line contains a distinctive marker that was invisible before the fix.
    assert!(
        rendered.contains("THIS WAS CUT"),
        "6th summary line should be visible on an 80×24 terminal; was it truncated?\n\
         Rendered output:\n{rendered}"
    );
}

// ---------------------------------------------------------------------------
// E2E: no summary → no summary box rendered, article body fills the pane
// ---------------------------------------------------------------------------

#[test]
fn e2e_no_summary_renders_article_body_only() {
    let mut app = make_app();
    app.mode = Mode::Normal;
    app.selected = Selected::Entry(dummy_entry_meta());
    app.current_entry_text = "The full article body text.".to_string();
    app.current_summary = None;

    let rendered = render_to_string(&mut app, 80, 24);

    // LLM Summary box title must not appear when there is no summary.
    assert!(
        !rendered.contains("LLM Summary"),
        "summary box should not render when current_summary is None"
    );
}
