use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    pub llm_enabled: bool,
    pub api_key_env: String,
    pub base_url: String,
    pub model_name: String,
    pub max_requests_per_day: u32,
    pub max_words_per_prompt: usize,
    pub timeout_seconds: u64,
    pub max_retries: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            llm_enabled: false,
            api_key_env: "".to_string(), // Empty by default per feedback
            base_url: "".to_string(),
            model_name: "".to_string(), // Empty by default per feedback
            max_requests_per_day: 100,
            max_words_per_prompt: 1500,
            timeout_seconds: 30,
            max_retries: 3,
        }
    }
}

// Bounds applied to user-supplied numeric settings. `config.json` is hand-editable,
// so absurd values (e.g. `timeout_seconds: 0` → an *infinite* ureq timeout, or a
// huge `max_retries`/`max_words_per_prompt`) must not be able to hang the app or
// drive runaway token cost.
const MIN_TIMEOUT_SECONDS: u64 = 1;
const MAX_TIMEOUT_SECONDS: u64 = 300;
const MAX_RETRIES_CAP: u32 = 10;
const MAX_REQUESTS_PER_DAY_CAP: u32 = 100_000;
const MIN_WORDS_PER_PROMPT: usize = 1;
const MAX_WORDS_PER_PROMPT_CAP: usize = 50_000;

impl AppSettings {
    /// Clamps numeric fields into safe ranges. Applied whenever settings are
    /// loaded so neither a hand-edited config file nor in-app entry can push a
    /// value into a dangerous range.
    pub fn clamp(mut self) -> Self {
        self.timeout_seconds = self
            .timeout_seconds
            .clamp(MIN_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS);
        self.max_retries = self.max_retries.min(MAX_RETRIES_CAP);
        self.max_requests_per_day = self.max_requests_per_day.min(MAX_REQUESTS_PER_DAY_CAP);
        self.max_words_per_prompt = self
            .max_words_per_prompt
            .clamp(MIN_WORDS_PER_PROMPT, MAX_WORDS_PER_PROMPT_CAP);
        self
    }
}

/// Returns the configuration/data directory path `~/.myrss/`.
pub fn settings_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".myrss")
}

/// Loads the application settings from `~/.myrss/config.json`.
pub fn load_settings() -> AppSettings {
    let path = settings_dir().join("config.json");
    if path.exists()
        && let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(settings) = serde_json::from_str::<AppSettings>(&content)
    {
        return settings.clamp();
    }
    AppSettings::default()
}

/// Saves the application settings to `~/.myrss/config.json`.
pub fn save_settings(settings: &AppSettings) -> anyhow::Result<()> {
    let dir = settings_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config.json");
    let content = serde_json::to_string_pretty(settings)?;
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let s = AppSettings::default();
        assert!(!s.llm_enabled);
        assert_eq!(s.api_key_env, "");
        assert_eq!(s.model_name, "");
        assert_eq!(s.max_requests_per_day, 100);
        assert_eq!(s.max_words_per_prompt, 1500);
        assert_eq!(s.timeout_seconds, 30);
        assert_eq!(s.max_retries, 3);
    }

    #[test]
    fn test_settings_are_clamped() {
        let s = AppSettings {
            timeout_seconds: 0, // would be an *infinite* ureq timeout
            max_retries: 9_999,
            max_requests_per_day: 9_999_999,
            max_words_per_prompt: 10_000_000,
            ..AppSettings::default()
        }
        .clamp();
        assert_eq!(s.timeout_seconds, MIN_TIMEOUT_SECONDS);
        assert_eq!(s.max_retries, MAX_RETRIES_CAP);
        assert_eq!(s.max_requests_per_day, MAX_REQUESTS_PER_DAY_CAP);
        assert_eq!(s.max_words_per_prompt, MAX_WORDS_PER_PROMPT_CAP);

        // A timeout above the ceiling is reduced; in-range values are untouched.
        let s2 = AppSettings {
            timeout_seconds: 100_000,
            ..AppSettings::default()
        }
        .clamp();
        assert_eq!(s2.timeout_seconds, MAX_TIMEOUT_SECONDS);
        assert_eq!(AppSettings::default().clamp().timeout_seconds, 30);
    }
}
