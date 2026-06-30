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

    // Chat / RAG / web-search settings. All carry `#[serde(default)]` so a
    // `config.json` written by an older build (which lacks these keys) still
    // deserializes instead of silently falling back to the whole default.
    //
    // `:chat` is gated on `llm_enabled` (it is an LLM feature like summarize), so
    // there is no separate chat toggle. Web search is opt-in: setting
    // `search_api_key_env` to the name of an env var holding a Tavily key enables
    // the `web_search` tool; leaving it empty simply never offers the tool.
    /// Env var holding the Tavily API key. Empty disables web search.
    #[serde(default)]
    pub search_api_key_env: String,
    /// Cosine-similarity floor for RAG chunk selection. Chunks at or above this
    /// are injected; if none qualify the full (truncated) article is sent.
    #[serde(default = "default_rag_similarity_threshold")]
    pub rag_similarity_threshold: f32,
    /// Hard cap on web-search rounds within a single chat turn, bounding latency
    /// and token cost of the agentic loop.
    #[serde(default = "default_max_search_iterations")]
    pub max_search_iterations: u32,
}

fn default_rag_similarity_threshold() -> f32 {
    0.5
}

fn default_max_search_iterations() -> u32 {
    3
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
            search_api_key_env: "".to_string(),
            rag_similarity_threshold: default_rag_similarity_threshold(),
            max_search_iterations: default_max_search_iterations(),
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
const MIN_SEARCH_ITERATIONS: u32 = 1;
const MAX_SEARCH_ITERATIONS: u32 = 5;

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
        // A cosine threshold only makes sense in [0, 1]; a NaN from a hand-edited
        // config would make every comparison false (→ always full-context), so
        // coerce it back to the default.
        if !self.rag_similarity_threshold.is_finite() {
            self.rag_similarity_threshold = default_rag_similarity_threshold();
        }
        self.rag_similarity_threshold = self.rag_similarity_threshold.clamp(0.0, 1.0);
        self.max_search_iterations = self
            .max_search_iterations
            .clamp(MIN_SEARCH_ITERATIONS, MAX_SEARCH_ITERATIONS);
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
        assert_eq!(s.search_api_key_env, "");
        assert_eq!(s.rag_similarity_threshold, 0.5);
        assert_eq!(s.max_search_iterations, 3);
    }

    #[test]
    fn test_legacy_config_without_new_fields_still_loads() {
        // A config.json written by a build that predates the chat feature has none
        // of the new keys. It must still deserialize (serde defaults fill them in),
        // not error out and silently discard the user's LLM settings.
        let legacy = r#"{
            "llm_enabled": true,
            "api_key_env": "OPENAI_API_KEY",
            "base_url": "",
            "model_name": "openai/gpt-4o-mini",
            "max_requests_per_day": 100,
            "max_words_per_prompt": 1500,
            "timeout_seconds": 30,
            "max_retries": 3
        }"#;
        let s: AppSettings = serde_json::from_str(legacy).expect("legacy config must parse");
        assert!(s.llm_enabled);
        assert_eq!(s.model_name, "openai/gpt-4o-mini");
        // New fields fall back to their declared defaults, not type-zero.
        assert_eq!(s.search_api_key_env, "");
        assert_eq!(s.rag_similarity_threshold, 0.5);
        assert_eq!(s.max_search_iterations, 3);
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

    #[test]
    fn test_chat_settings_are_clamped() {
        // Out-of-range threshold and iteration count are coerced into safe bounds.
        let s = AppSettings {
            rag_similarity_threshold: 5.0,
            max_search_iterations: 9_999,
            ..AppSettings::default()
        }
        .clamp();
        assert_eq!(s.rag_similarity_threshold, 1.0);
        assert_eq!(s.max_search_iterations, MAX_SEARCH_ITERATIONS);

        let s2 = AppSettings {
            rag_similarity_threshold: -1.0,
            max_search_iterations: 0,
            ..AppSettings::default()
        }
        .clamp();
        assert_eq!(s2.rag_similarity_threshold, 0.0);
        assert_eq!(s2.max_search_iterations, MIN_SEARCH_ITERATIONS);

        // A NaN threshold (possible via hand-edited config) resets to the default
        // rather than poisoning every similarity comparison.
        let s3 = AppSettings {
            rag_similarity_threshold: f32::NAN,
            ..AppSettings::default()
        }
        .clamp();
        assert_eq!(s3.rag_similarity_threshold, 0.5);
    }
}
