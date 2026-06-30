//! How the UI is rendered, with the Ratatui library.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, LineGauge, List, ListItem, Paragraph, Wrap};
use std::rc::Rc;

use crate::app::AppImpl;
use crate::modes::{Mode, ReadMode, Selected};
use crate::rss::EntryMetadata;

const PINK: Color = Color::Rgb(255, 150, 167);
const SOFT_BLUE: Color = Color::Rgb(135, 178, 238);

pub fn predraw(f: &Frame, _mode: Mode) -> Rc<[Rect]> {
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());

    Layout::default()
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
        .direction(Direction::Horizontal)
        .split(main_layout[0])
}

pub fn draw(f: &mut Frame, chunks: Rc<[Rect]>, app: &mut AppImpl) {
    match app.mode {
        Mode::Settings | Mode::SettingsEditing(_) => {
            draw_settings(f, f.area(), app);
        }
        Mode::ViewLlmLog => {
            draw_llm_log(f, f.area(), app);
        }
        _ => {
            draw_info_column(f, chunks[0], app);

            match &app.selected {
                Selected::Feeds | Selected::Entries => {
                    draw_entries(f, chunks[1], app);
                }
                Selected::Entry(_entry_meta) => {
                    draw_entry(f, chunks[1], app);
                }
                Selected::None => draw_entries(f, chunks[1], app),
            }
        }
    }

    let bottom_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());

    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(15)])
        .split(bottom_layout[1]);

    if matches!(app.mode, Mode::Command) {
        let cmd_text = format!(":{}", app.command_input);
        let cmd_paragraph = Paragraph::new(cmd_text).style(Style::default().fg(Color::Yellow));
        f.render_widget(cmd_paragraph, status_chunks[0]);
    }

    let read_mode_str = match app.read_mode {
        crate::modes::ReadMode::ShowRead => "READ",
        crate::modes::ReadMode::ShowUnread => "UNREAD",
        crate::modes::ReadMode::All => "ALL",
    };
    let read_mode_paragraph = Paragraph::new(read_mode_str)
        .style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(ratatui::layout::Alignment::Right);
    f.render_widget(read_mode_paragraph, status_chunks[1]);

    if let Mode::Confirmation(action) = app.mode {
        let size = f.area();
        let popup_area = centered_rect(50, 15, size);
        let question = match action {
            crate::modes::ConfirmationAction::ClearCache => {
                " Are you sure you want to clear the LLM request cache? (y/n) "
            }
        };
        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            " Confirmation Required ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        let paragraph = Paragraph::new(question)
            .block(block)
            .wrap(Wrap { trim: true });

        f.render_widget(ratatui::widgets::Clear, popup_area);
        f.render_widget(paragraph, popup_area);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}

fn draw_info_column(f: &mut Frame, area: Rect, app: &mut AppImpl) {
    let constraints = match &app.mode {
        Mode::Normal
        | Mode::Command
        | Mode::Settings
        | Mode::SettingsEditing(_)
        | Mode::ViewLlmLog
        | Mode::Confirmation(_) => {
            if app.show_help {
                vec![
                    Constraint::Min(0),
                    Constraint::Percentage(25),
                    Constraint::Length(10),
                ]
            } else {
                vec![Constraint::Percentage(70), Constraint::Percentage(30)]
            }
        }
        Mode::Editing => {
            if app.show_help {
                vec![
                    Constraint::Min(0),
                    Constraint::Percentage(20),
                    Constraint::Length(3),
                    Constraint::Length(10),
                ]
            } else {
                vec![
                    Constraint::Min(0),
                    Constraint::Percentage(20),
                    Constraint::Length(3),
                ]
            }
        }
    };

    let chunks = Layout::default()
        .constraints(constraints)
        .direction(Direction::Vertical)
        .split(area);
    {
        // FEEDS
        draw_feeds(f, chunks[0], app);

        // INFO
        match &app.selected {
            Selected::Entry(entry) => draw_entry_info(f, chunks[1], entry),
            Selected::Entries => {
                if let Some(entry_meta) = &app.current_entry_meta {
                    draw_entry_info(f, chunks[1], entry_meta);
                } else {
                    draw_feed_info(f, chunks[1], app);
                }
            }
            Selected::None => draw_first_run_helper(f, chunks[1]),
            _ => {
                if app.current_feed.is_some() {
                    draw_feed_info(f, chunks[1], app);
                }
            }
        }

        match (app.mode, app.show_help) {
            (Mode::Editing, true) => {
                draw_new_feed_input(f, chunks[2], app);
                draw_help(f, chunks[3], app);
            }
            (Mode::Editing, false) => {
                draw_new_feed_input(f, chunks[2], app);
            }
            (_, true) => {
                draw_help(f, chunks[2], app);
            }
            _ => (),
        }
    }
}

fn draw_first_run_helper(f: &mut Frame, area: Rect) {
    let text = "Press 'i', then enter an RSS/Atom feed URL, then hit `Enter`!";

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        "TO SUBSCRIBE TO YOUR FIRST FEED",
        Style::default().fg(PINK).add_modifier(Modifier::BOLD),
    ));

    let paragraph = Paragraph::new(Text::from(text))
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn draw_entry_info(f: &mut Frame, area: Rect, entry_meta: &EntryMetadata) {
    let mut text = String::new();
    if let Some(item) = &entry_meta.title {
        text.push_str("Title: ");
        text.push_str(item.to_string().as_str());
        text.push('\n');
    };

    if let Some(item) = &entry_meta.link {
        text.push_str("Link: ");
        text.push_str(item);
        text.push('\n');
    }

    if let Some(pub_date) = &entry_meta.pub_date {
        text.push_str("Pub. date: ");
        text.push_str(pub_date.to_string().as_str());
    } else {
        // TODO this should probably pull the <updated> tag
        // and use that
        let inserted_at = entry_meta.inserted_at;
        text.push_str("Pulled date: ");
        text.push_str(inserted_at.to_string().as_str());
    }
    text.push('\n');

    if let Some(read_at) = &entry_meta.read_at {
        text.push_str("Read at: ");
        text.push_str(read_at.to_string().as_str());
        text.push('\n');
    }

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        "Info",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));

    let paragraph = Paragraph::new(Text::from(text.as_str()))
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn draw_feeds(f: &mut Frame, area: Rect, app: &mut AppImpl) {
    let feeds = app
        .feeds
        .items
        .iter()
        .flat_map(|feed| feed.title.as_ref())
        .map(Span::raw)
        .map(ListItem::new)
        .collect::<Vec<ListItem>>();

    let default_title = String::from("Feeds");
    let title = app.flash.as_ref().unwrap_or(&default_title);

    let feeds = List::new(feeds).block(
        Block::default().borders(Borders::ALL).title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
    );

    let feeds = match app.selected {
        Selected::Feeds => feeds
            .highlight_style(Style::default().fg(SOFT_BLUE).add_modifier(Modifier::BOLD))
            .highlight_symbol("> "),
        _ => feeds,
    };

    f.render_stateful_widget(feeds, area, &mut app.feeds.state);
}

fn draw_feed_info(f: &mut Frame, area: Rect, app: &mut AppImpl) {
    let mut text = String::new();
    if let Some(item) = app
        .current_feed
        .as_ref()
        .and_then(|feed| feed.title.as_ref())
    {
        text.push_str("Title: ");
        text.push_str(item);
        text.push('\n');
    }

    if let Some(item) = app
        .current_feed
        .as_ref()
        .and_then(|feed| feed.link.as_ref())
    {
        text.push_str("Link: ");
        text.push_str(item);
        text.push('\n');
    }

    if let Some(item) = app
        .current_feed
        .as_ref()
        .and_then(|feed| feed.feed_link.as_ref())
    {
        text.push_str("Feed link: ");
        text.push_str(item);
        text.push('\n');
    }

    if let Some(item) = app.entries.items.first()
        && let Some(pub_date) = &item.pub_date
    {
        text.push_str("Most recent entry at: ");
        text.push_str(pub_date.to_string().as_str());
        text.push('\n');
    }

    if let Some(item) = &app
        .current_feed
        .as_ref()
        .and_then(|feed| feed.refreshed_at)
        .map(|timestamp| timestamp.to_string())
        .or_else(|| Some("Never refreshed".to_string()))
    {
        text.push_str("Refreshed at: ");
        text.push_str(item.as_str());
        text.push('\n');
    }

    match app.read_mode {
        ReadMode::ShowUnread => text.push_str("Unread entries: "),
        ReadMode::ShowRead => text.push_str("Read entries: "),
        ReadMode::All => unreachable!("ReadMode::All should never be possible from the UI!"),
    }
    text.push_str(app.entries.items.len().to_string().as_str());
    text.push('\n');

    if let Some(feed_kind) = app.current_feed.as_ref().map(|feed| feed.feed_kind) {
        text.push_str("Feed kind: ");
        text.push_str(&feed_kind.to_string());
        text.push('\n');
    }

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        "Info",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));

    let paragraph = Paragraph::new(Text::from(text.as_str()))
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn draw_help(f: &mut Frame, area: Rect, app: &mut AppImpl) {
    let mut text = String::new();

    text.push_str("KEYS:\n");
    match app.selected {
        Selected::Feeds => {
            text.push_str("  gg/G: snap top/bottom | r: refresh | x: refresh all\n");
            text.push_str("  c: copy link | o: open browser | i: add feed | q: quit\n");
        }
        _ => {
            text.push_str("  gg/G: snap top/bottom | r: toggle read | a: read mode\n");
            text.push_str("  M: noteworthy | O: view text | q: quit\n");
        }
    }
    match app.mode {
        Mode::Normal => {}
        Mode::Editing => {
            text.push_str("  enter: fetch feed | del: delete feed | esc: exit edit\n");
        }
        Mode::Command => {
            text.push_str("  Type command and press Enter | esc: exit command\n");
        }
        Mode::Settings => {
            text.push_str("  j/k: navigate | enter: edit/toggle | esc: discard & exit\n");
        }
        Mode::SettingsEditing(_) => {
            text.push_str("  Type new value | enter: save | esc: cancel edit\n");
        }
        Mode::ViewLlmLog => {
            text.push_str("  j/k: scroll requests | esc/q: close log\n");
        }
        Mode::Confirmation(_) => {
            text.push_str("  y: confirm | n/esc: cancel\n");
        }
    }

    text.push_str("COMMANDS:\n");
    text.push_str("  :settings - configure | :view_llm_log - API logs\n");
    text.push_str("  :clear_cache - clear LLM cache | :summarize - AI summary\n");

    text.push_str("? - toggle help");

    let help_message =
        Paragraph::new(Text::from(text.as_str())).block(Block::default().borders(Borders::ALL));
    f.render_widget(help_message, area);
}

fn draw_new_feed_input(f: &mut Frame, area: Rect, app: &mut AppImpl) {
    let text = &app.feed_subscription_input;
    let text = Text::from(text.as_str());
    let input = Paragraph::new(text)
        .style(Style::default().fg(Color::Yellow))
        .block(
            Block::default().borders(Borders::ALL).title(Span::styled(
                "Add a feed",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
        );
    f.render_widget(input, area);
}

fn draw_entries(f: &mut Frame, area: Rect, app: &mut AppImpl) {
    let entries = app
        .entries
        .items
        .iter()
        .map(|entry| {
            let title = entry.title.as_ref().map_or("No title", |t| t.as_str());
            let mut style = if entry.newly_added {
                let is_bright = (app.tick_count / 2).is_multiple_of(2);
                let show_underline = app.tick_count.is_multiple_of(2);
                let color = if is_bright {
                    Color::LightMagenta
                } else {
                    Color::Magenta
                };
                let mut s = Style::default().fg(color);
                if show_underline {
                    s = s.add_modifier(Modifier::UNDERLINED);
                }
                s
            } else if entry.noteworthy {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            if entry.read_at.is_none() {
                style = style.add_modifier(Modifier::BOLD);
            }

            let styled_title = if entry.noteworthy {
                format!("★ {title}")
            } else {
                title.to_string()
            };

            ListItem::new(Span::styled(styled_title, style))
        })
        .collect::<Vec<ListItem>>();

    let default_title = "Entries".to_string();

    let title = app
        .current_feed
        .as_ref()
        .and_then(|feed| feed.title.as_ref())
        .unwrap_or(&default_title);

    let entries_titles = List::new(entries).block(
        Block::default().borders(Borders::ALL).title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
    );

    let entries_titles = match app.selected {
        Selected::Entries => entries_titles
            .highlight_style(Style::default().fg(SOFT_BLUE).add_modifier(Modifier::BOLD))
            .highlight_symbol("> "),
        _ => entries_titles,
    };

    if !&app.error_flash.is_empty() {
        let chunks = Layout::default()
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)].as_ref())
            .direction(Direction::Vertical)
            .split(area);
        {
            let error_text = error_text(&app.error_flash);

            let block = Block::default().borders(Borders::ALL).title(Span::styled(
                "Error - press 'q' to close",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));

            let error_widget = Paragraph::new(error_text)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((0, 0));

            f.render_stateful_widget(entries_titles, chunks[0], &mut app.entries.state);
            f.render_widget(error_widget, chunks[1]);
        }
    } else {
        f.render_stateful_widget(entries_titles, area, &mut app.entries.state);
    }
}

fn draw_entry(f: &mut Frame, area: Rect, app: &mut AppImpl) {
    let mut article_area = area;
    if let Some(summary) = &app.current_summary {
        let inner_width = area.width.saturating_sub(2).max(1) as usize;
        let box_height = summary_box_height(summary, inner_width, area.height);

        let summary_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(box_height),
                Constraint::Min(0),
            ])
            .split(area);

        let summary_block = Block::default().borders(Borders::ALL).title(Span::styled(
            " LLM Summary ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        let summary_paragraph = Paragraph::new(summary.as_str())
            .block(summary_block)
            .wrap(Wrap { trim: false });
        f.render_widget(summary_paragraph, summary_chunks[0]);
        article_area = summary_chunks[1];
    }

    let scroll = app.entry_scroll_position;
    let entry_meta = if let Selected::Entry(e) = &app.selected {
        e
    } else {
        panic!("draw_entry should only be called when app.selected was Selected::Entry")
    };

    let entry_title = entry_meta.title.as_deref().unwrap_or("No entry title");

    let mut entry_title_str = String::new();
    if entry_meta.noteworthy {
        entry_title_str.push_str("★ ");
    }
    entry_title_str.push_str(entry_title);

    let feed_title = app
        .current_feed
        .as_ref()
        .and_then(|feed| feed.title.as_deref())
        .unwrap_or("No feed title");

    let mut title = String::new();
    title.reserve_exact(entry_title_str.len() + feed_title.len() + 3);
    title.push_str(&entry_title_str);
    title.push_str(" - ");
    title.push_str(feed_title);

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        &title,
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    ));

    let mut lines = Vec::new();
    for line in app.current_entry_text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let heading = rest.to_uppercase();
            lines.push(Line::from(Span::styled(
                heading,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            let heading = rest.to_string();
            lines.push(Line::from(Span::styled(
                heading,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if let Some(rest) = trimmed.strip_prefix("### ") {
            let heading = rest.to_string();
            lines.push(Line::from(Span::styled(
                heading,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if trimmed.starts_with("=====") || trimmed.starts_with("-----") {
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(Color::DarkGray),
            )));
        } else if trimmed.starts_with("[Image")
            || trimmed.starts_with("[SVG")
            || (trimmed.starts_with("[") && trimmed.ends_with("]"))
        {
            lines.push(Line::from(Span::styled(
                line,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(line));
        }
    }
    let text = Text::from(lines);

    let paragraph = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    let entry_chunk_height = article_area.height - 2;

    let progress_gauge_chunk_percent = 3;

    let entry_percent = 100.0 - progress_gauge_chunk_percent as f32;

    let real_entry_chunk_height =
        (entry_chunk_height as f32 * (entry_percent / 100.0)).floor() as u16;

    app.entry_lines_rendered_len = real_entry_chunk_height;

    let percent = if app.entry_lines_len > 0 {
        let furthest_visible_position = app.entry_scroll_position + real_entry_chunk_height;
        let percent = ((furthest_visible_position as f32 / app.entry_lines_len as f32) * 100.0)
            .floor() as usize;

        if percent <= 100 { percent } else { 100 }
    } else {
        0
    };

    let label = format!("{percent}/100");
    let ratio = percent as f64 / 100.0;
    let gauge = LineGauge::default()
        .block(Block::default().borders(Borders::NONE))
        .filled_style(Style::default().fg(PINK))
        .ratio(ratio)
        .label(label);

    if !app.error_flash.is_empty() {
        let chunks = Layout::default()
            .constraints(
                [
                    Constraint::Percentage(57),
                    Constraint::Percentage(progress_gauge_chunk_percent),
                    Constraint::Percentage(40),
                ]
                .as_ref(),
            )
            .direction(Direction::Vertical)
            .split(article_area);
        {
            let error_text = error_text(&app.error_flash);
            let block = Block::default().borders(Borders::ALL).title(Span::styled(
                "Error - press 'q' to close",
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Cyan),
            ));

            let error_widget = Paragraph::new(error_text)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((0, 0));

            f.render_widget(paragraph, chunks[0]);
            f.render_widget(gauge, chunks[1]);
            f.render_widget(error_widget, chunks[2]);
        }
    } else {
        let chunks = Layout::default()
            .constraints(
                [
                    Constraint::Percentage(entry_percent.ceil() as u16),
                    Constraint::Percentage(progress_gauge_chunk_percent),
                ]
                .as_ref(),
            )
            .direction(Direction::Vertical)
            .split(article_area);

        f.render_widget(paragraph, chunks[0]);
        f.render_widget(gauge, chunks[1]);
    }
}

fn error_text(errors: &[anyhow::Error]) -> String {
    errors
        .iter()
        .flat_map(|e| {
            let mut s = format!("{e:?}")
                .split('\n')
                .map(|s| s.to_owned())
                .collect::<Vec<String>>();
            s.push("\n".to_string());
            s
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn draw_settings(f: &mut Frame, area: Rect, app: &mut AppImpl) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let fields_area = chunks[0];
    let models_area = chunks[1];

    let settings = &app.settings;
    let focus = match app.mode {
        Mode::Settings => Some(app.settings_cursor),
        Mode::SettingsEditing(idx) => Some(idx),
        _ => None,
    };
    let is_editing = matches!(app.mode, Mode::SettingsEditing(_));

    let mut list_items = Vec::new();

    let get_val = |idx: usize, val: &str| -> String {
        if focus == Some(idx) && is_editing {
            app.settings_buffer.clone()
        } else {
            val.to_string()
        }
    };

    let mut make_item = |idx: usize, label: &str, value: String| {
        let is_selected = focus == Some(idx);
        let prefix = if is_selected { "> " } else { "  " };

        let label_span = Span::styled(
            format!("{}{:<30}: ", prefix, label),
            Style::default().fg(Color::Cyan),
        );
        let val_style = if is_selected {
            if is_editing {
                Style::default()
                    .bg(Color::Green)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            }
        } else {
            Style::default().fg(Color::White)
        };
        let val_span = Span::styled(value, val_style);

        list_items.push(ListItem::new(Line::from(vec![label_span, val_span])));
    };

    make_item(
        0,
        "1. LLM Enabled (summarize)",
        if settings.llm_enabled {
            "Yes [Toggle with Enter]".to_string()
        } else {
            "No [Toggle with Enter]".to_string()
        },
    );
    make_item(
        1,
        "2. API Key Env Var Name",
        get_val(1, &settings.api_key_env),
    );
    make_item(2, "3. Custom Base URL", get_val(2, &settings.base_url));
    make_item(3, "4. LLM Model Name", get_val(3, &settings.model_name));
    make_item(
        4,
        "5. Max Requests Per Day",
        get_val(4, &settings.max_requests_per_day.to_string()),
    );
    make_item(
        5,
        "6. Max Words Per Prompt",
        get_val(5, &settings.max_words_per_prompt.to_string()),
    );
    make_item(
        6,
        "7. Timeout (seconds)",
        get_val(6, &settings.timeout_seconds.to_string()),
    );
    make_item(
        7,
        "8. Max Retries",
        get_val(7, &settings.max_retries.to_string()),
    );

    let btn_fetch = if focus == Some(8) {
        "> [ Fetch Models List ]"
    } else {
        "  [ Fetch Models List ]"
    };
    let btn_fetch_style = if focus == Some(8) {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    list_items.push(ListItem::new(Span::styled(btn_fetch, btn_fetch_style)));

    let btn_save = if focus == Some(9) {
        "> [ Save & Close Settings ]"
    } else {
        "  [ Save & Close Settings ]"
    };
    let btn_save_style = if focus == Some(9) {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    list_items.push(ListItem::new(Span::styled(btn_save, btn_save_style)));

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        " App Settings (Enter to Toggle/Edit; Esc to Discard) ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));

    let list = List::new(list_items).block(block);
    f.render_widget(list, fields_area);

    let mut model_items = Vec::new();
    for m in &app.available_models {
        model_items.push(ListItem::new(Span::raw(m)));
    }
    let models_block = Block::default().borders(Borders::ALL).title(Span::styled(
        " Available Models ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    let models_list = List::new(model_items).block(models_block);
    f.render_widget(models_list, models_area);
}

fn draw_llm_log(f: &mut Frame, area: Rect, app: &mut AppImpl) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);

    let list_area = chunks[0];
    let details_area = chunks[1];

    let logs = &app.request_logs;
    if logs.is_empty() {
        let empty_p = Paragraph::new("No requests logged yet.").block(
            Block::default()
                .borders(Borders::ALL)
                .title(" LLM Request Log (Esc/q to Close) "),
        );
        f.render_widget(empty_p, area);
        return;
    }

    let selected_idx = app.log_scroll_position;
    let list_items: Vec<ListItem> = logs
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let prefix = if idx == selected_idx { "> " } else { "  " };
            let datetime = chrono::DateTime::from_timestamp(entry.timestamp, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| "Unknown time".to_string());

            let line = format!(
                "{}ID: {:<4} | Time: {:<25} | Status: {:<4} | Finish: {}",
                prefix, entry.id, datetime, entry.status_code, entry.finish_reason
            );
            let style = if idx == selected_idx {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Span::styled(line, style))
        })
        .collect();

    let list_block = Block::default().borders(Borders::ALL).title(Span::styled(
        " LLM Request Log (j/k to Scroll; Esc/q to Close) ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    let list = List::new(list_items).block(list_block);
    f.render_widget(list, list_area);

    if let Some(entry) = logs.get(selected_idx) {
        let details_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(details_area);

        let prompt_block = Block::default()
            .borders(Borders::ALL)
            .title(" Prompt Payload ");
        let prompt_p = Paragraph::new(entry.prompt.as_str())
            .block(prompt_block)
            .wrap(Wrap { trim: false });
        f.render_widget(prompt_p, details_chunks[0]);

        let response_block = Block::default()
            .borders(Borders::ALL)
            .title(" Response Payload ");
        let response_p = Paragraph::new(entry.response.as_str())
            .block(response_block)
            .wrap(Wrap { trim: false });
        f.render_widget(response_p, details_chunks[1]);
    }
}

/// Compute the height (in terminal rows) of the LLM summary box, including its
/// top and bottom borders.  The result is clamped: at least 4 rows (so the box
/// is always visible) and at most half the available area height (so the article
/// body is never completely squeezed out).
pub(crate) fn summary_box_height(summary: &str, inner_width: usize, area_height: u16) -> u16 {
    let inner_width = inner_width.max(1);
    let content_lines: u16 = summary
        .lines()
        .map(|line| {
            let cols = line.chars().count();
            ((cols + inner_width - 1) / inner_width).max(1) as u16
        })
        .sum();
    // +2 for the top and bottom border rows
    (content_lines + 2).min(area_height / 2).max(4)
}

#[cfg(test)]
mod tests {
    use super::summary_box_height;

    // --- unit tests for the height-calculation logic ---

    #[test]
    fn short_summary_gets_minimum_height() {
        // One line that fits entirely in the inner width → 1 content line + 2 borders = 3,
        // but the minimum is 4.
        let h = summary_box_height("hello", 80, 40);
        assert_eq!(h, 4);
    }

    #[test]
    fn multi_line_summary_height_matches_line_count() {
        // Three explicit newlines → 3 content lines → 3 + 2 = 5 rows, well under half of 40.
        let summary = "line one\nline two\nline three";
        let h = summary_box_height(summary, 80, 40);
        assert_eq!(h, 5);
    }

    #[test]
    fn long_line_wraps_and_increases_height() {
        // A 160-character line on a 80-column inner width wraps to 2 rows.
        // 2 content rows + 2 borders = 4; area_height/2 = 20 → not capped.
        let long_line = "a".repeat(160);
        let h = summary_box_height(&long_line, 80, 40);
        assert_eq!(h, 4); // ceil(160/80)=2 lines + 2 borders = 4, also the minimum
    }

    #[test]
    fn very_long_summary_is_capped_at_half_area_height() {
        // 100 lines of text, each fitting on one row → 102 rows needed, but area is 40
        // so the cap is 40/2 = 20.
        let summary = "short\n".repeat(100);
        let h = summary_box_height(&summary, 80, 40);
        assert_eq!(h, 20);
    }

    #[test]
    fn summary_box_height_regression_fixed_7_row_truncation() {
        // The original bug: a 6-line (≥5 visible content rows) summary was silently
        // truncated because the box was hard-coded to 7 rows (5 content + 2 borders).
        // With the fix, a 6-line summary must produce a box taller than 7.
        let summary = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6";
        let h = summary_box_height(summary, 80, 40);
        // 6 content lines + 2 borders = 8, which is > 7 (the old cap) and ≤ 20 (half of 40)
        assert!(h > 7, "expected height > 7 (old hard-coded cap), got {h}");
        assert_eq!(h, 8);
    }
}
