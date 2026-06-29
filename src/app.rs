//! The main application state is managed here, in `App`.

use crate::modes::{Mode, ReadMode, Selected};
use crate::util;
use anyhow::Result;
use copypasta::{ClipboardContext, ClipboardProvider};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::sync::{Arc, Mutex};

macro_rules! delegate_to_locked_inner {
    ($(($fn_name:ident, $t:ty)),* $(,)? ) => {
        $(
            pub fn $fn_name(&self) -> $t {
                let inner = self.inner.lock().unwrap();
                inner.$fn_name()
            }
        )*
    };
}

macro_rules! delegate_to_locked_mut_inner {
    ($(($fn_name:ident, $t:ty)),* $(,)?) => {
        $(
            pub fn $fn_name(&self) -> $t {
                let mut inner = self.inner.lock().unwrap();
                inner.$fn_name()
            }
        )*
    };
}

#[derive(Clone, Debug)]
pub struct App {
    inner: Arc<Mutex<AppImpl>>,
}

impl App {
    delegate_to_locked_inner![
        (error_flash_is_empty, bool),
        (feed_ids, Result<Vec<crate::rss::FeedId>>),
        (force_redraw, Result<()>),
        (http_client, ureq::Agent),
        (mode, Mode),
        (selected, Selected),
        (open_link_in_browser, Result<()>),
        (should_quit, bool),
        (refresh_feed, Result<()>),
        (subscribe_to_feed, Result<()>),
        (feed_subscription_input_is_empty, bool)
    ];

    delegate_to_locked_mut_inner![
        (clear_error_flash, ()),
        (clear_flash, ()),
        (on_down, Result<()>),
        (on_left, Result<()>),
        (on_right, Result<()>),
        (on_up, Result<()>),
        (page_up, ()),
        (page_down, ()),
        (pop_feed_subscription_input, ()),
        (put_current_link_in_clipboard, Result<()>),
        (reset_feed_subscription_input, ()),
        (select_feeds, ()),
        (delete_feed, Result<()>),
        (toggle_help, Result<()>),
        (toggle_read, Result<()>),
        (toggle_read_mode, Result<()>),
        (toggle_noteworthy, Result<()>),
        (update_current_feed_and_entries, Result<()>),
        (select_and_show_current_entry, Result<()>),
        (on_snap_to_top, Result<()>),
        (on_snap_to_bottom, Result<()>)
    ];

    pub fn new(
        options: crate::ReadOptions,
        event_tx: std::sync::mpsc::Sender<crate::Event<crossterm::event::KeyEvent>>,
        io_tx: std::sync::mpsc::Sender<crate::io::Action>,
    ) -> Result<App> {
        Ok(App {
            inner: Arc::new(Mutex::new(AppImpl::new(options, event_tx, io_tx)?)),
        })
    }

    pub fn handle_g_keypress(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if let Some(instant) = inner.g_pressed_at {
            if instant.elapsed() < std::time::Duration::from_millis(1000) {
                inner.g_pressed_at = None;
                return true;
            }
        }
        inner.g_pressed_at = Some(std::time::Instant::now());
        false
    }

    pub fn clear_g_keypress(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.g_pressed_at = None;
    }

    pub fn push_command_char(&self, c: char) {
        let mut inner = self.inner.lock().unwrap();
        inner.command_input.push(c);
    }

    pub fn pop_command_char(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.command_input.pop();
    }

    pub fn command_input(&self) -> String {
        let inner = self.inner.lock().unwrap();
        inner.command_input.clone()
    }

    pub fn reset_command_input(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.command_input.clear();
    }

    pub fn current_summary(&self) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        inner.current_summary.clone()
    }

    pub fn set_current_summary(&self, summary: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        inner.current_summary = summary;
    }

    pub fn set_entry_ascii_content(&self, text: String, entry_meta: crate::rss::EntryMetadata) {
        let mut inner = self.inner.lock().unwrap();
        inner.current_entry_text = text;
        inner.entry_lines_len = inner.current_entry_text.matches('\n').count();
        inner.entry_scroll_position = 0;
        inner.selected = Selected::Entry(entry_meta);
        inner.flash = None;
    }

    pub(crate) fn open_current_article_with_ascii(&self) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        if let Some(entry_meta) = &inner.current_entry_meta {
            let entry_id = entry_meta.id;
            let width = inner.entry_column_width;
            inner.io_tx.send(crate::io::Action::RenderAsciiArticle(entry_id, width as u32))?;
        }
        Ok(())
    }

    pub(crate) fn summarize_current_entry(&self) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        if let Some(entry_meta) = &inner.current_entry_meta {
            let entry_id = entry_meta.id;
            inner.io_tx.send(crate::io::Action::SummarizeArticle(entry_id))?;
        }
        Ok(())
    }

    pub fn settings(&self) -> crate::settings::AppSettings {
        let inner = self.inner.lock().unwrap();
        inner.settings.clone()
    }

    pub fn settings_cursor(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.settings_cursor
    }

    pub fn set_settings_cursor(&self, idx: usize) {
        let mut inner = self.inner.lock().unwrap();
        inner.settings_cursor = idx;
    }

    pub fn settings_buffer(&self) -> String {
        let inner = self.inner.lock().unwrap();
        inner.settings_buffer.clone()
    }

    pub fn set_settings_buffer(&self, s: String) {
        let mut inner = self.inner.lock().unwrap();
        inner.settings_buffer = s;
    }

    pub fn push_settings_buffer_char(&self, c: char) {
        let mut inner = self.inner.lock().unwrap();
        inner.settings_buffer.push(c);
    }

    pub fn pop_settings_buffer_char(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.settings_buffer.pop();
    }

    pub fn set_available_models(&self, models: Vec<String>) {
        let mut inner = self.inner.lock().unwrap();
        inner.available_models = models;
    }

    pub fn request_logs(&self) -> Vec<crate::cache::RequestLogEntry> {
        let inner = self.inner.lock().unwrap();
        inner.request_logs.clone()
    }

    pub fn log_scroll_position(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.log_scroll_position
    }

    pub fn set_log_scroll_position(&self, idx: usize) {
        let mut inner = self.inner.lock().unwrap();
        inner.log_scroll_position = idx;
    }

    pub fn load_request_logs(&self) {
        let mut inner = self.inner.lock().unwrap();
        if let Ok(logs) = crate::cache::get_request_logs() {
            inner.request_logs = logs;
            inner.log_scroll_position = 0;
        }
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut crate::settings::AppSettings)) {
        let mut inner = self.inner.lock().unwrap();
        f(&mut inner.settings);
    }

    pub fn save_settings(&self) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        crate::settings::save_settings(&inner.settings)?;
        Ok(())
    }

    pub fn fetch_models_background(&self) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        inner.io_tx.send(crate::io::Action::FetchModels)?;
        Ok(())
    }

    pub fn tick(&self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.tick_count = inner.tick_count.wrapping_add(1);
        Ok(())
    }


    pub fn draw(&self, terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();

        terminal.draw(|f| {
            let chunks = crate::ui::predraw(f, inner.mode);

            assert!(
                chunks.len() >= 2,
                "There must be at least two chunks in order to draw two columns"
            );

            let new_width = chunks[1].width;

            if inner.entry_column_width != new_width {
                inner.entry_column_width = new_width;
                inner.select_and_show_current_entry().unwrap_or_else(|e| {
                    inner.error_flash = vec![e];
                })
            }

            inner.entry_column_width = chunks[1].width;

            crate::ui::draw(f, chunks, &mut inner);
        })?;

        Ok(())
    }

    pub fn set_should_quit(&mut self, should_quit: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.should_quit = should_quit
    }

    pub fn set_flash(&self, flash: String) {
        let mut inner = self.inner.lock().unwrap();
        inner.flash = Some(flash)
    }

    pub fn push_error_flash(&self, e: anyhow::Error) {
        let mut inner = self.inner.lock().unwrap();
        inner.error_flash.push(e);
    }

    pub fn set_mode(&self, mode: Mode) {
        let mut inner = self.inner.lock().unwrap();
        inner.mode = mode;
    }

    pub fn push_feed_subscription_input(&self, input: char) {
        let mut inner = self.inner.lock().unwrap();
        inner.feed_subscription_input.push(input);
    }

    pub fn set_feeds(&self, feeds: Vec<crate::rss::Feed>) {
        let mut inner = self.inner.lock().unwrap();
        let feeds = feeds.into();
        inner.feeds = feeds;
    }

    pub(crate) fn refresh_feeds(&self) -> Result<()> {
        let feed_ids = self.feed_ids()?;
        let inner = self.inner.lock().unwrap();
        inner
            .io_tx
            .send(crate::io::Action::RefreshFeeds(feed_ids))?;
        Ok(())
    }

    pub(crate) fn break_io_thread(&self) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        inner.io_tx.send(crate::io::Action::Break)?;
        Ok(())
    }

    pub(crate) fn has_entries(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        !inner.entries.items.is_empty()
    }

    pub(crate) fn has_current_entry(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.current_entry_meta.is_some()
    }
}

#[derive(Debug)]
pub struct AppImpl {
    // database stuff
    pub conn: rusqlite::Connection,
    // network stuff
    pub http_client: ureq::Agent,
    // feed stuff
    pub current_feed: Option<crate::rss::Feed>,
    pub feeds: util::StatefulList<crate::rss::Feed>,
    // entry stuff
    pub current_entry_meta: Option<crate::rss::EntryMetadata>,
    pub entries: util::StatefulList<crate::rss::EntryMetadata>,
    pub entry_selection_position: usize,
    pub current_entry_text: String,
    pub entry_scroll_position: u16,
    pub entry_lines_len: usize,
    pub entry_lines_rendered_len: u16,
    pub entry_column_width: u16,
    // modes
    pub should_quit: bool,
    pub selected: Selected,
    pub mode: Mode,
    pub read_mode: ReadMode,
    pub show_help: bool,
    // misc
    pub error_flash: Vec<anyhow::Error>,
    pub feed_subscription_input: String,
    pub flash: Option<String>,
    event_tx: std::sync::mpsc::Sender<crate::Event<crossterm::event::KeyEvent>>,
    io_tx: std::sync::mpsc::Sender<crate::io::Action>,
    pub is_wsl: bool,
    pub g_pressed_at: Option<std::time::Instant>,
    pub command_input: String,
    pub current_summary: Option<String>,
    pub tick_count: u32,
    pub settings: crate::settings::AppSettings,
    pub settings_cursor: usize,
    pub settings_buffer: String,
    pub available_models: Vec<String>,
    pub request_logs: Vec<crate::cache::RequestLogEntry>,
    pub log_scroll_position: usize,
}

impl AppImpl {
    pub fn new(
        options: crate::ReadOptions,
        event_tx: std::sync::mpsc::Sender<crate::Event<crossterm::event::KeyEvent>>,
        io_tx: std::sync::mpsc::Sender<crate::io::Action>,
    ) -> Result<AppImpl> {
        let mut conn = rusqlite::Connection::open(&options.database_path)?;

        let http_client = ureq::AgentBuilder::new()
            .timeout_read(options.network_timeout)
            .user_agent("myrss/0.5.0")
            .build();

        crate::rss::initialize_db(&mut conn)?;
        conn.execute("UPDATE entries SET newly_added = 0", [])?;
        let feeds: util::StatefulList<crate::rss::Feed> = vec![].into();
        let entries: util::StatefulList<crate::rss::EntryMetadata> = vec![].into();
        // default to having nothing selected,
        // as it's possible we are starting for the first time,
        // with an empty feeds db
        let selected = Selected::None;
        let initial_current_feed = None;

        let is_wsl = wsl::is_wsl();

        let mut app = AppImpl {
            conn,
            http_client,
            should_quit: false,
            error_flash: vec![],
            feeds,
            entries,
            selected,
            entry_scroll_position: 0,
            entry_lines_len: 0,
            entry_lines_rendered_len: 0,
            entry_column_width: 0,
            current_entry_meta: None,
            current_entry_text: String::new(),
            current_feed: initial_current_feed,
            feed_subscription_input: String::new(),
            mode: Mode::Normal,
            read_mode: ReadMode::ShowUnread,
            show_help: true,
            entry_selection_position: 0,
            flash: None,
            event_tx,
            is_wsl,
            io_tx,
            g_pressed_at: None,
            command_input: String::new(),
            current_summary: None,
            tick_count: 0,
            settings: crate::settings::load_settings(),
            settings_cursor: 0,
            settings_buffer: String::new(),
            available_models: Vec::new(),
            request_logs: Vec::new(),
            log_scroll_position: 0,
        };

        app.update_feeds()?;
        app.update_current_feed_and_entries()?;

        // we default to having Selected::None,
        // so if there are actually feeds, select them
        if !app.feeds.items.is_empty() {
            app.select_feeds()
        }

        Ok(app)
    }

    pub fn delete_feed(&mut self) -> Result<()> {
        if matches!(self.selected, Selected::Feeds) && matches!(self.mode(), Mode::Editing) {
            let feed_id = self.selected_feed_id();
            crate::rss::delete_feed(&mut self.conn, feed_id)?;

            // Remove the feed in app state
            let feeds_len = self.feeds.items.len();

            for i in 0..feeds_len {
                if self.feeds.items[i].id == feed_id {
                    self.feeds.items.remove(i);

                    if i == feeds_len - 1 {
                        self.feeds.previous();
                    }

                    break;
                }
            }

            // Remove the entries from the feed in app state
            self.entries.items.retain(|entry| entry.feed_id != feed_id);

            // Update
            self.update_current_feed_and_entries()?;
        }

        Ok(())
    }

    pub fn update_feeds(&mut self) -> Result<()> {
        let feeds = crate::rss::get_feeds(&self.conn)?.into();
        self.feeds = feeds;
        Ok(())
    }

    pub fn update_current_feed_and_entries(&mut self) -> Result<()> {
        self.update_current_feed()?;
        self.update_current_entries()?;
        Ok(())
    }

    fn update_current_feed(&mut self) -> Result<()> {
        let prev_feed_id = self.current_feed.as_ref().map(|f| f.id);

        self.current_feed = if self.feeds.items.is_empty() {
            self.selected = Selected::None;
            None
        } else {
            let selected_idx = match self.feeds.state.selected() {
                Some(idx) => idx,
                None => {
                    self.feeds.reset();
                    0
                }
            };
            let feed_id = self.feeds.items[selected_idx].id;
            Some(crate::rss::get_feed(&self.conn, feed_id)?)
        };

        let new_feed_id = self.current_feed.as_ref().map(|f| f.id);
        if prev_feed_id != new_feed_id {
            if let Some(prev_id) = prev_feed_id {
                crate::rss::clear_newly_added_for_feed(&self.conn, prev_id)?;
            }
        }

        Ok(())
    }

    fn update_current_entries(&mut self) -> Result<()> {
        let entries = if let Some(feed) = &self.current_feed {
            crate::rss::get_entries_metas(&self.conn, &self.read_mode, feed.id)?
                .into_iter()
                .collect::<Vec<_>>()
                .into()
        } else {
            vec![].into()
        };

        self.entries = entries;

        if self.entry_selection_position < self.entries.items.len() {
            self.entries
                .state
                .select(Some(self.entry_selection_position))
        } else {
            match self.entries.items.len().checked_sub(1) {
                Some(n) => self.entries.state.select(Some(n)),
                None => self.entries.reset(),
            }
        }
        Ok(())
    }

    fn update_entry_selection_position(&mut self) {
        if self.entries.items.is_empty() {
            self.entry_selection_position = 0
        } else if self.entry_selection_position > self.entries.items.len() - 1 {
            self.entry_selection_position = self.entries.items.len() - 1
        };
    }

    fn get_selected_entry_content(&self) -> Option<Result<crate::rss::EntryContent>> {
        self.entries.state.selected().and_then(|selected_idx| {
            self.entries
                .items
                .get(selected_idx)
                .map(|item| item.id)
                .map(|entry_id| crate::rss::get_entry_content(&self.conn, entry_id))
        })
    }

    fn get_selected_entry_meta(&self) -> Option<Result<crate::rss::EntryMetadata>> {
        self.entries.state.selected().and_then(|selected_idx| {
            self.entries
                .items
                .get(selected_idx)
                .map(|item| item.id)
                .map(|entry_id| crate::rss::get_entry_meta(&self.conn, entry_id))
        })
    }

    fn update_current_entry_meta(&mut self) -> Result<()> {
        if let Some(entry_meta) = self.get_selected_entry_meta() {
            let entry_meta = entry_meta?;
            self.current_entry_meta = Some(entry_meta);
        }
        Ok(())
    }

    fn page_up(&mut self) {
        if matches!(self.selected, Selected::Entry(_)) {
            self.entry_scroll_position = self
                .entry_scroll_position
                .checked_sub(self.entry_lines_rendered_len)
                .unwrap_or_default()
        };
    }

    fn page_down(&mut self) {
        if matches!(self.selected, Selected::Entry(_)) {
            self.entry_scroll_position = if self.entry_scroll_position
                + self.entry_lines_rendered_len
                >= self.entry_lines_len as u16
            {
                self.entry_lines_len as u16
            } else {
                self.entry_scroll_position + self.entry_lines_rendered_len
            };
        }
    }

    pub(crate) fn select_and_show_current_entry(&mut self) -> Result<()> {
        if let Some(entry_meta) = &self.current_entry_meta {
            let entry_meta = entry_meta.clone();

            if let Some(entry) = self.get_selected_entry_content() {
                let entry = entry?;
                let empty_string = String::from("No content or description tag provided.");

                // try content tag first,
                // if there is not content tag,
                // go to description tag,
                // if no description tag,
                // use empty string.
                // TODO figure out what to actually do if there are neither
                let entry_html = entry
                    .content
                    .as_ref()
                    .or(entry.description.as_ref())
                    .or(Some(&empty_string));

                // minimum is 1
                let line_length = if self.entry_column_width >= 5 {
                    self.entry_column_width - 2
                } else {
                    1
                };

                if let Some(html) = entry_html {
                    let text = html2text::from_read(html.as_bytes(), line_length.into())?;
                    self.entry_lines_len = text.matches('\n').count();
                    self.current_entry_text = text;
                } else {
                    self.current_entry_text = String::new();
                }
            }

            self.selected = Selected::Entry(entry_meta);
        }

        Ok(())
    }

    pub(crate) fn refresh_feed(&self) -> Result<()> {
        let feed_id = self.selected_feed_id();
        self.io_tx.send(crate::io::Action::RefreshFeed(feed_id))?;
        Ok(())
    }

    pub(crate) fn subscribe_to_feed(&self) -> Result<()> {
        let feed_subscription_input = self.feed_subscription_input();
        self.io_tx
            .send(crate::io::Action::SubscribeToFeed(feed_subscription_input))?;
        Ok(())
    }

    pub fn toggle_help(&mut self) -> Result<()> {
        self.show_help = !self.show_help;
        Ok(())
    }

    pub fn clear_error_flash(&mut self) {
        self.error_flash = vec![];
    }

    pub fn reset_feed_subscription_input(&mut self) {
        self.feed_subscription_input.clear();
    }

    pub fn pop_feed_subscription_input(&mut self) {
        self.feed_subscription_input.pop();
    }

    pub fn feed_subscription_input_is_empty(&self) -> bool {
        self.feed_subscription_input.is_empty()
    }

    pub fn feed_subscription_input(&self) -> String {
        self.feed_subscription_input.clone()
    }

    pub fn error_flash_is_empty(&self) -> bool {
        self.error_flash.is_empty()
    }

    pub fn clear_flash(&mut self) {
        self.flash = None
    }

    pub fn select_feeds(&mut self) {
        self.selected = Selected::Feeds;
    }

    pub fn selected(&self) -> Selected {
        self.selected.clone()
    }

    pub fn selected_feed_id(&self) -> crate::rss::FeedId {
        let selected_idx = self.feeds.state.selected().unwrap();
        self.feeds.items[selected_idx].id
    }

    pub fn feed_ids(&self) -> Result<Vec<crate::rss::FeedId>> {
        let ids = crate::rss::get_feed_ids(&self.conn)?;
        Ok(ids)
    }

    pub fn toggle_noteworthy(&mut self) -> Result<()> {
        match &self.selected {
            Selected::Entry(entry) => {
                entry.toggle_noteworthy(&self.conn)?;
                self.selected = Selected::Entries;
                self.update_current_entries()?;
                self.update_current_entry_meta()?;
                self.entry_scroll_position = 0;
            }
            Selected::Entries => {
                if let Some(entry_meta) = &self.current_entry_meta {
                    entry_meta.toggle_noteworthy(&self.conn)?;
                    self.update_current_entries()?;
                    self.update_current_entry_meta()?;
                    self.update_entry_selection_position();
                }
            }
            Selected::Feeds => (),
            Selected::None => (),
        }

        Ok(())
    }

    pub fn toggle_read(&mut self) -> Result<()> {
        match &self.selected {
            Selected::Entry(entry) => {
                entry.toggle_read(&self.conn)?;
                self.selected = Selected::Entries;
                self.update_current_entries()?;
                self.update_current_entry_meta()?;
                self.entry_scroll_position = 0;
            }
            Selected::Entries => {
                if let Some(entry_meta) = &self.current_entry_meta {
                    entry_meta.toggle_read(&self.conn)?;
                    self.update_current_entries()?;
                    self.update_current_entry_meta()?;
                    self.update_entry_selection_position();
                }
            }
            Selected::Feeds => (),
            Selected::None => (),
        }

        Ok(())
    }

    pub fn http_client(&self) -> ureq::Agent {
        // this is cheap because it only clones a struct containing two Arcs
        self.http_client.clone()
    }

    pub fn toggle_read_mode(&mut self) -> Result<()> {
        match (&self.read_mode, &self.selected) {
            (ReadMode::ShowRead, Selected::Feeds) | (ReadMode::ShowRead, Selected::Entries) => {
                self.entry_selection_position = 0;
                self.read_mode = ReadMode::ShowUnread
            }
            (ReadMode::ShowUnread, Selected::Feeds) | (ReadMode::ShowUnread, Selected::Entries) => {
                self.entry_selection_position = 0;
                self.read_mode = ReadMode::ShowRead
            }
            _ => (),
        }
        self.update_current_entries()?;

        if !self.entries.items.is_empty() {
            self.entries.reset();
        } else {
            self.entries.unselect();
        }

        self.update_current_entry_meta()?;

        Ok(())
    }

    fn get_current_link(&self) -> Option<&str> {
        match &self.selected {
            Selected::Feeds => self
                .current_feed
                .as_ref()
                .and_then(|feed| feed.link.as_deref().or(feed.feed_link.as_deref())),
            Selected::Entries => self
                .entries
                .items
                .get(self.entry_selection_position)
                .and_then(|entry| entry.link.as_deref()),
            Selected::Entry(e) => e.link.as_deref(),
            Selected::None => None,
        }
    }

    fn put_current_link_in_clipboard(&mut self) -> Result<()> {
        let current_link = self.get_current_link();

        if self.is_wsl {
            #[cfg(target_os = "linux")]
            {
                if let Some(current_link) = current_link {
                    util::set_wsl_clipboard_contents(current_link)
                } else {
                    Ok(())
                }
            }

            #[cfg(not(target_os = "linux"))]
            {
                unreachable!("This should never happen. This code should only be reachable if the target OS is WSL.")
            }
        } else if let Some(current_link) = current_link {
            let mut ctx = ClipboardContext::new().map_err(|e| anyhow::anyhow!(e))?;
            ctx.set_contents(current_link.to_owned())
                .map_err(|e| anyhow::anyhow!(e))
        } else {
            Ok(())
        }
    }

    fn open_link_in_browser(&self) -> Result<()> {
        if let Some(current_link) = self.get_current_link() {
            webbrowser::open(current_link).map_err(|e| anyhow::anyhow!(e))
        } else {
            Ok(())
        }
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn on_left(&mut self) -> Result<()> {
        match self.selected {
            Selected::Feeds => (),
            Selected::Entries => {
                self.entry_selection_position = 0;
                self.selected = Selected::Feeds
            }
            Selected::Entry(_) => {
                self.entry_scroll_position = 0;
                self.selected = {
                    self.current_entry_text = String::new();
                    Selected::Entries
                }
            }
            Selected::None => (),
        }

        Ok(())
    }

    pub fn on_up(&mut self) -> Result<()> {
        match self.selected {
            Selected::Feeds => {
                self.feeds.previous();
                self.update_current_feed_and_entries()?;
            }
            Selected::Entries => {
                if !self.entries.items.is_empty() {
                    self.entries.previous();
                    self.entry_selection_position = self.entries.state.selected().unwrap();
                    self.update_current_entry_meta()?;
                }
            }
            Selected::Entry(_) => {
                if let Some(n) = self.entry_scroll_position.checked_sub(1) {
                    self.entry_scroll_position = n
                };
            }
            Selected::None => (),
        }

        Ok(())
    }

    pub fn on_right(&mut self) -> Result<()> {
        match self.selected {
            Selected::Feeds => {
                if !self.entries.items.is_empty() {
                    self.selected = Selected::Entries;
                    self.entries.reset();
                    self.update_current_entry_meta()?;
                }
                Ok(())
            }
            Selected::Entries => self.select_and_show_current_entry(),
            Selected::Entry(_) => Ok(()),
            Selected::None => Ok(()),
        }
    }

    pub fn on_down(&mut self) -> Result<()> {
        match self.selected {
            Selected::Feeds => {
                self.feeds.next();
                self.update_current_feed_and_entries()?;
            }
            Selected::Entries => {
                if !self.entries.items.is_empty() {
                    self.entries.next();
                    self.entry_selection_position = self.entries.state.selected().unwrap();
                    self.update_current_entry_meta()?;
                }
            }
            Selected::Entry(_) => {
                if let Some(n) = self.entry_scroll_position.checked_add(1) {
                    self.entry_scroll_position = n
                };
            }
            Selected::None => (),
        }

        Ok(())
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn force_redraw(&self) -> Result<()> {
        self.event_tx.send(crate::Event::Tick).map_err(|e| e.into())
    }

    pub fn on_snap_to_top(&mut self) -> Result<()> {
        match self.selected {
            Selected::Feeds => {
                self.feeds.snap_to_top();
                self.update_current_feed_and_entries()?;
            }
            Selected::Entries => {
                if !self.entries.items.is_empty() {
                    self.entries.snap_to_top();
                    self.entry_selection_position = self.entries.state.selected().unwrap();
                    self.update_current_entry_meta()?;
                }
            }
            Selected::Entry(_) => {
                self.entry_scroll_position = 0;
            }
            Selected::None => (),
        }
        Ok(())
    }

    pub fn on_snap_to_bottom(&mut self) -> Result<()> {
        match self.selected {
            Selected::Feeds => {
                self.feeds.snap_to_bottom();
                self.update_current_feed_and_entries()?;
            }
            Selected::Entries => {
                if !self.entries.items.is_empty() {
                    self.entries.snap_to_bottom();
                    self.entry_selection_position = self.entries.state.selected().unwrap();
                    self.update_current_entry_meta()?;
                }
            }
            Selected::Entry(_) => {
                self.entry_scroll_position = self.entry_lines_len as u16;
            }
            Selected::None => (),
        }
        Ok(())
    }
}
