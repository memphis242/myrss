#![forbid(unsafe_code)]

use crate::modes::{Mode, Selected};
use anyhow::Result;
use app::App;
use clap::{Parser, Subcommand};
use crossterm::event::{self, KeyEvent, KeyEventKind};
use crossterm::event::{Event as CEvent, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc;
use std::{thread, time};

mod app;
mod ascii;
mod cache;
mod io;
mod llm;
mod modes;
mod opml;
mod rss;
mod settings;
mod ui;
mod util;

fn main() -> Result<()> {
    let options = Options::parse();

    let validated_options = match &options.subcommand {
        Some(sub) => sub.validate()?,
        None => {
            let database_path = get_database_path(&None)?;
            ValidatedOptions::Read(ReadOptions {
                database_path,
                tick_rate: 250,
                flash_display_duration_seconds: time::Duration::from_secs(4),
                network_timeout: time::Duration::from_secs(5),
            })
        }
    };

    match validated_options {
        ValidatedOptions::Import(options) => crate::opml::import(options),
        ValidatedOptions::Read(options) => run_reader(options),
    }
}

/// A TUI RSS reader with vim-like controls and a local-first, offline-first focus
#[derive(Debug, Parser)]
#[command(author, version, about, name = "myrss")]
struct Options {
    #[command(subcommand)]
    subcommand: Option<Command>,
}

/// Only used to take input at the boundary.
/// Turned into `ValidatedOptions` with `validate()`.
#[derive(Debug, Subcommand)]
enum Command {
    /// Read your feeds
    Read {
        /// Override where `russ` stores and reads feeds.
        /// By default, the feeds database on Linux this will be at `XDG_DATA_HOME/russ/feeds.db` or `$HOME/.local/share/russ/feeds.db`.
        /// On MacOS it will be at `$HOME/Library/Application Support/russ/feeds.db`.
        /// On Windows it will be at `{FOLDERID_LocalAppData}/russ/data/feeds.db`.
        #[arg(short, long)]
        database_path: Option<PathBuf>,
        /// time in ms between two ticks
        #[arg(short, long, default_value = "250")]
        tick_rate: u64,
        /// number of seconds to show the flash message before clearing it
        #[arg(short, long, default_value = "4", value_parser = parse_seconds)]
        flash_display_duration_seconds: time::Duration,
        /// RSS/Atom network request timeout in seconds
        #[arg(short, long, default_value = "5", value_parser = parse_seconds)]
        network_timeout: time::Duration,
    },
    /// Import feeds from an OPML document
    Import {
        /// Override where `russ` stores and reads feeds.
        /// By default, the feeds database on Linux this will be at `XDG_DATA_HOME/russ/feeds.db` or `$HOME/.local/share/russ/feeds.db`.
        /// On MacOS it will be at `$HOME/Library/Application Support/russ/feeds.db`.
        /// On Windows it will be at `{FOLDERID_LocalAppData}/russ/data/feeds.db`.
        #[arg(short, long)]
        database_path: Option<PathBuf>,
        #[arg(short, long)]
        opml_path: PathBuf,
        /// RSS/Atom network request timeout in seconds
        #[arg(short, long, default_value = "5", value_parser = parse_seconds)]
        network_timeout: time::Duration,
    },
}

impl Command {
    fn validate(&self) -> std::io::Result<ValidatedOptions> {
        match self {
            Command::Read {
                database_path,
                tick_rate,
                flash_display_duration_seconds,
                network_timeout,
            } => {
                let database_path = get_database_path(database_path)?;

                Ok(ValidatedOptions::Read(ReadOptions {
                    database_path,
                    tick_rate: *tick_rate,
                    flash_display_duration_seconds: *flash_display_duration_seconds,
                    network_timeout: *network_timeout,
                }))
            }
            Command::Import {
                database_path,
                opml_path,
                network_timeout,
            } => {
                let database_path = get_database_path(database_path)?;
                Ok(ValidatedOptions::Import(ImportOptions {
                    database_path,
                    opml_path: opml_path.to_owned(),
                    network_timeout: *network_timeout,
                }))
            }
        }
    }
}

fn parse_seconds(s: &str) -> Result<time::Duration, std::num::ParseIntError> {
    let as_u64 = s.parse::<u64>()?;
    Ok(time::Duration::from_secs(as_u64))
}

/// internal, validated options for the normal reader mode
#[derive(Debug)]
enum ValidatedOptions {
    Read(ReadOptions),
    Import(ImportOptions),
}

#[derive(Clone, Debug)]
struct ReadOptions {
    database_path: PathBuf,
    tick_rate: u64,
    flash_display_duration_seconds: time::Duration,
    network_timeout: time::Duration,
}

#[derive(Debug)]
struct ImportOptions {
    database_path: PathBuf,
    opml_path: PathBuf,
    network_timeout: time::Duration,
}

fn get_database_path(database_path: &Option<PathBuf>) -> std::io::Result<PathBuf> {
    let database_path = if let Some(database_path) = database_path {
        database_path.to_owned()
    } else {
        let mut database_path = directories::ProjectDirs::from("", "", "russ")
            .expect("unable to find home directory. if you like, you can provide a database path directly by passing the -d option.")
            .data_local_dir()
            .to_path_buf();

        std::fs::create_dir_all(&database_path)?;

        database_path.push("feeds.db");

        database_path
    };

    Ok(database_path)
}

pub enum Event<I> {
    Input(I),
    Tick,
}

fn run_reader(options: ReadOptions) -> Result<()> {
    let _ = crate::cache::initialize_cache_db();
    enable_raw_mode()?;

    let mut stdout = stdout();

    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);

    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    // Setup input handling
    let (event_tx, event_rx) = mpsc::channel();

    let event_tx_clone = event_tx.clone();

    let tick_rate = time::Duration::from_millis(options.tick_rate);

    thread::spawn(move || {
        let mut last_tick = time::Instant::now();
        loop {
            // poll for tick rate duration, if no events, sent tick event.
            if event::poll(tick_rate - last_tick.elapsed())
                .expect("Unable to poll for Crossterm event")
                && let CEvent::Key(key) = event::read().expect("Unable to read Crossterm event")
            {
                event_tx
                    .send(Event::Input(key))
                    .expect("Unable to send Crossterm Key input event");
            }
            if last_tick.elapsed() >= tick_rate {
                event_tx.send(Event::Tick).expect("Unable to send tick");
                last_tick = time::Instant::now();
            }
        }
    });

    let options_clone = options.clone();

    let (io_tx, io_rx) = mpsc::channel();

    let io_tx_clone = io_tx.clone();

    let mut app = App::new(options, event_tx_clone, io_tx)?;

    let cloned_app = app.clone();

    terminal.clear()?;

    // spawn this thread to handle receiving messages to performing blocking network and db IO
    let io_thread = thread::spawn(move || -> Result<()> {
        io::io_loop(cloned_app, io_tx_clone, io_rx, &options_clone)
    });

    // this is basically "the Elm Architecture".
    //
    // more or less:
    // ui <- current_state
    // action <- current_state + event
    // new_state <- current_state + action
    loop {
        app.draw(&mut terminal)?;

        let event = event_rx.recv()?;

        let action = get_action(&app, event);

        if let Some(action) = action {
            update(&mut app, action)?;
        }

        if app.should_quit() {
            app.break_io_thread()?;
            disable_raw_mode()?;
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            terminal.show_cursor()?;
            break;
        }
    }

    io_thread
        .join()
        .expect("Unable to join IO thread to main thread")?;

    Ok(())
}

enum Action {
    Quit,
    MoveLeft,
    MoveDown,
    MoveUp,
    MoveRight,
    PageUp,
    PageDown,
    RefreshAll,
    RefreshFeed,
    ToggleHelp,
    ToggleReadMode,
    EnterEditingMode,
    OpenLinkInBrowser,
    CopyLinkToClipboard,
    Tick,
    SubscribeToFeed,
    PushInputChar(char),
    DeleteInputChar,
    DeleteFeed,
    EnterNormalMode,
    ClearErrorFlash,
    SelectAndShowCurrentEntry,
    ToggleReadStatus,
    SnapToTop,
    SnapToBottom,
    ToggleNoteworthy,
    OpenArticleWithAscii,
    SummarizeArticle,
    FetchModels,
}

fn get_action(app: &App, event: Event<KeyEvent>) -> Option<Action> {
    match app.mode() {
        Mode::Normal => match event {
            Event::Input(key_event) if key_event.kind == KeyEventKind::Press => {
                match (key_event.code, key_event.modifiers) {
                    (KeyCode::Char('g'), _) => {
                        if app.handle_g_keypress() {
                            Some(Action::SnapToTop)
                        } else {
                            None
                        }
                    }
                    other => {
                        app.clear_g_keypress();
                        match other {
                            (KeyCode::Char('q'), _)
                            | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                            | (KeyCode::Esc, _) => {
                                if app.current_summary().is_some() {
                                    app.set_current_summary(None);
                                    None
                                } else if !app.error_flash_is_empty() {
                                    Some(Action::ClearErrorFlash)
                                } else {
                                    Some(Action::Quit)
                                }
                            }
                            (KeyCode::Char('r'), KeyModifiers::NONE) => match app.selected() {
                                Selected::Feeds => Some(Action::RefreshFeed),
                                _ => Some(Action::ToggleReadStatus),
                            },
                            (KeyCode::Char('x'), KeyModifiers::NONE) => Some(Action::RefreshAll),
                            (KeyCode::Left, _) | (KeyCode::Char('h'), _) => Some(Action::MoveLeft),
                            (KeyCode::Right, _) | (KeyCode::Char('l'), _) => Some(Action::MoveRight),
                            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => Some(Action::MoveDown),
                            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => Some(Action::MoveUp),
                            (KeyCode::PageUp, _) | (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                                Some(Action::PageUp)
                            }
                            (KeyCode::PageDown, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                Some(Action::PageDown)
                            }
                            (KeyCode::Enter, _) => match app.selected() {
                                Selected::Entries | Selected::Entry(_) => {
                                    if app.has_entries() && app.has_current_entry() {
                                        Some(Action::SelectAndShowCurrentEntry)
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            },
                            (KeyCode::Char('?'), _) => Some(Action::ToggleHelp),
                            (KeyCode::Char('a'), _) => Some(Action::ToggleReadMode),
                            (KeyCode::Char('e'), _) | (KeyCode::Char('i'), _) => {
                                Some(Action::EnterEditingMode)
                            }
                            (KeyCode::Char('c'), _) => Some(Action::CopyLinkToClipboard),
                            (KeyCode::Char('o'), _) => Some(Action::OpenLinkInBrowser),
                            (KeyCode::Char('O'), _) => Some(Action::OpenArticleWithAscii),
                            (KeyCode::Char('G'), _) => Some(Action::SnapToBottom),
                            (KeyCode::Char('M'), _) => Some(Action::ToggleNoteworthy),
                            (KeyCode::Char(':'), _) => {
                                app.reset_command_input();
                                app.set_mode(Mode::Command);
                                None
                            }
                            _ => None,
                        }
                    }
                }
            }
            Event::Input(_) => None,
            Event::Tick => Some(Action::Tick),
        },
        Mode::Editing => match event {
            Event::Input(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Enter => {
                        if !app.feed_subscription_input_is_empty() {
                            Some(Action::SubscribeToFeed)
                        } else {
                            None
                        }
                    }
                    KeyCode::Char(c) => Some(Action::PushInputChar(c)),
                    KeyCode::Backspace => Some(Action::DeleteInputChar),
                    KeyCode::Delete => Some(Action::DeleteFeed),
                    KeyCode::Esc => Some(Action::EnterNormalMode),
                    _ => None,
                }
            }
            Event::Input(_) => None,
            Event::Tick => Some(Action::Tick),
        },
        Mode::Command => match event {
            Event::Input(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Enter => {
                        let cmd = app.command_input();
                        app.reset_command_input();
                        app.set_mode(Mode::Normal);
                        if cmd == "summarize" {
                            let settings = app.settings();
                            if !settings.llm_enabled {
                                app.set_flash("LLM summarization is disabled. Run :settings to enable it.".to_string());
                                None
                            } else if settings.api_key_env.is_empty() || settings.model_name.is_empty() {
                                app.set_flash("LLM Summarization requires API key env and model to be configured in :settings.".to_string());
                                None
                            } else if app.has_entries() && app.has_current_entry() {
                                Some(Action::SummarizeArticle)
                            } else {
                                None
                            }
                        } else if cmd == "settings" {
                            app.set_settings_cursor(0);
                            app.set_mode(Mode::Settings);
                            None
                        } else if cmd == "view_llm_log" {
                            app.load_request_logs();
                            app.set_mode(Mode::ViewLlmLog);
                            None
                        } else if cmd == "clear_cache" {
                            app.set_mode(Mode::Confirmation(crate::modes::ConfirmationAction::ClearCache));
                            None
                        } else {
                            app.set_flash(format!("Unknown command: :{}", cmd));
                            None
                        }
                    }
                    KeyCode::Char(c) => {
                        app.push_command_char(c);
                        None
                    }
                    KeyCode::Backspace => {
                        app.pop_command_char();
                        None
                    }
                    KeyCode::Esc => {
                        app.reset_command_input();
                        app.set_mode(Mode::Normal);
                        None
                    }
                    _ => None,
                }
            }
            Event::Input(_) => None,
            Event::Tick => Some(Action::Tick),
        },
        Mode::Settings => match event {
            Event::Input(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        app.update_settings(|s| {
                            *s = crate::settings::load_settings();
                        });
                        app.set_mode(Mode::Normal);
                        None
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        let current = app.settings_cursor();
                        app.set_settings_cursor((current + 1) % 10);
                        None
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        let current = app.settings_cursor();
                        app.set_settings_cursor(if current == 0 { 9 } else { current - 1 });
                        None
                    }
                    KeyCode::Enter => {
                        let cursor = app.settings_cursor();
                        match cursor {
                            0 => {
                                app.update_settings(|s| s.llm_enabled = !s.llm_enabled);
                                None
                            }
                            idx if idx >= 1 && idx <= 7 => {
                                let settings = app.settings();
                                let val = match idx {
                                    1 => settings.api_key_env,
                                    2 => settings.base_url,
                                    3 => settings.model_name,
                                    4 => settings.max_requests_per_day.to_string(),
                                    5 => settings.max_words_per_prompt.to_string(),
                                    6 => settings.timeout_seconds.to_string(),
                                    7 => settings.max_retries.to_string(),
                                    _ => String::new(),
                                };
                                app.set_settings_buffer(val);
                                app.set_mode(Mode::SettingsEditing(idx));
                                None
                            }
                            8 => Some(Action::FetchModels),
                            9 => {
                                if let Err(e) = app.save_settings() {
                                    app.set_flash(format!("Save settings failed: {}", e));
                                    app.push_error_flash(e);
                                } else {
                                    app.set_flash("Settings saved!".to_string());
                                }
                                app.set_mode(Mode::Normal);
                                None
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                }
            }
            Event::Input(_) => None,
            Event::Tick => Some(Action::Tick),
        },
        Mode::SettingsEditing(idx) => match event {
            Event::Input(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Esc => {
                        app.set_mode(Mode::Settings);
                        None
                    }
                    KeyCode::Enter => {
                        let buffer = app.settings_buffer();
                        app.update_settings(|s| match idx {
                            1 => s.api_key_env = buffer,
                            2 => s.base_url = buffer,
                            3 => s.model_name = buffer,
                            4 => {
                                if let Ok(val) = buffer.parse::<u32>() {
                                    s.max_requests_per_day = val;
                                }
                            }
                            5 => {
                                if let Ok(val) = buffer.parse::<usize>() {
                                    s.max_words_per_prompt = val;
                                }
                            }
                            6 => {
                                if let Ok(val) = buffer.parse::<u64>() {
                                    s.timeout_seconds = val;
                                }
                            }
                            7 => {
                                if let Ok(val) = buffer.parse::<u32>() {
                                    s.max_retries = val;
                                }
                            }
                            _ => (),
                        });
                        app.set_mode(Mode::Settings);
                        None
                    }
                    KeyCode::Char(c) => {
                        app.push_settings_buffer_char(c);
                        None
                    }
                    KeyCode::Backspace => {
                        app.pop_settings_buffer_char();
                        None
                    }
                    _ => None,
                }
            }
            Event::Input(_) => None,
            Event::Tick => Some(Action::Tick),
        },
        Mode::ViewLlmLog => match event {
            Event::Input(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        app.set_mode(Mode::Normal);
                        None
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        let current = app.log_scroll_position();
                        let total = app.request_logs().len();
                        if total > 0 && current < total - 1 {
                            app.set_log_scroll_position(current + 1);
                        }
                        None
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        let current = app.log_scroll_position();
                        if current > 0 {
                            app.set_log_scroll_position(current - 1);
                        }
                        None
                    }
                    _ => None,
                }
            }
            Event::Input(_) => None,
            Event::Tick => Some(Action::Tick),
        },
        Mode::Confirmation(action) => match event {
            Event::Input(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        match action {
                            crate::modes::ConfirmationAction::ClearCache => {
                                match crate::cache::clear_cache() {
                                    Ok(_) => app.set_flash("LLM request cache cleared successfully!".to_string()),
                                    Err(e) => app.set_flash(format!("Failed to clear cache: {}", e)),
                                }
                            }
                        }
                        app.set_mode(Mode::Normal);
                        None
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        app.set_flash("Clear cache cancelled".to_string());
                        app.set_mode(Mode::Normal);
                        None
                    }
                    _ => None,
                }
            }
            Event::Input(_) => None,
            Event::Tick => Some(Action::Tick),
        },
    }
}

fn update(app: &mut App, action: Action) -> Result<()> {
    match action {
        Action::Tick => app.tick()?,
        Action::Quit => app.set_should_quit(true),
        Action::RefreshAll => app.refresh_feeds()?,
        Action::RefreshFeed => app.refresh_feed()?,
        Action::MoveLeft => app.on_left()?,
        Action::MoveDown => app.on_down()?,
        Action::MoveUp => app.on_up()?,
        Action::MoveRight => app.on_right()?,
        Action::PageUp => app.page_up(),
        Action::PageDown => app.page_down(),
        Action::ToggleHelp => app.toggle_help()?,
        Action::ToggleReadMode => app.toggle_read_mode()?,
        Action::ToggleReadStatus => app.toggle_read()?,
        Action::EnterEditingMode => app.set_mode(Mode::Editing),
        Action::CopyLinkToClipboard => app.put_current_link_in_clipboard()?,
        Action::OpenLinkInBrowser => app.open_link_in_browser()?,
        Action::SubscribeToFeed => app.subscribe_to_feed()?,
        Action::PushInputChar(c) => app.push_feed_subscription_input(c),
        Action::DeleteInputChar => app.pop_feed_subscription_input(),
        Action::DeleteFeed => app.delete_feed()?,
        Action::EnterNormalMode => app.set_mode(Mode::Normal),
        Action::ClearErrorFlash => app.clear_error_flash(),
        Action::SelectAndShowCurrentEntry => app.select_and_show_current_entry()?,
        Action::SnapToTop => app.on_snap_to_top()?,
        Action::SnapToBottom => app.on_snap_to_bottom()?,
        Action::ToggleNoteworthy => app.toggle_noteworthy()?,
        Action::OpenArticleWithAscii => {
            if app.has_entries() && app.has_current_entry() {
                app.open_current_article_with_ascii()?;
            }
        }
        Action::SummarizeArticle => {
            app.summarize_current_entry()?;
        }
        Action::FetchModels => {
            app.fetch_models_background()?;
        }
    };

    Ok(())
}
