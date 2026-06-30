pub mod app;
pub mod ascii;
pub mod cache;
pub mod io;
pub mod llm;
pub mod modes;
pub mod opml;
pub mod rss;
pub mod settings;
pub mod summarize;
pub mod ui;
pub mod util;

use std::path::PathBuf;
use std::time;

#[derive(Clone, Debug)]
pub struct ReadOptions {
    pub database_path: PathBuf,
    pub tick_rate: u64,
    pub flash_display_duration_seconds: time::Duration,
    pub network_timeout: time::Duration,
}

#[derive(Clone, Debug)]
pub struct ImportOptions {
    pub database_path: PathBuf,
    pub opml_path: PathBuf,
    pub network_timeout: time::Duration,
}

#[derive(Debug)]
pub enum Event<I> {
    Input(I),
    Tick,
}
