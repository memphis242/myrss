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
        && let Ok(settings) = serde_json::from_str(&content)
    {
        return settings;
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
}
