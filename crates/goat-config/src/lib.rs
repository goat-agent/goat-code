use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: ThemeChoice,
    pub computer_use_enabled: bool,
    pub browser_enabled: bool,
    pub mouse_capture_enabled: bool,
    pub remote: RemoteConfig,
    pub search: SearchConfig,
    pub web_fetch: WebFetchConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub default_target: Option<String>,
    pub accounts: Vec<SearchAccountConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebFetchConfig {
    pub readability: bool,
    pub render_enabled: bool,
    pub max_length: usize,
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self {
            readability: true,
            render_enabled: true,
            max_length: 48 * 1024,
        }
    }
}

pub use goat_search_provider::SearchAccountConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteConfig {
    pub bind: String,
    pub advertised: Vec<String>,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:4317".to_owned(),
            advertised: Vec::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            computer_use_enabled: false,
            browser_enabled: false,
            mouse_capture_enabled: true,
            remote: RemoteConfig::default(),
            search: SearchConfig::default(),
            web_fetch: WebFetchConfig::default(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        let Ok(config) = serde_json::from_str::<Self>(&raw) else {
            let _ = fs::rename(&path, path.with_extension("json.corrupt"));
            return Self::default();
        };
        config
    }

    #[cfg(test)]
    pub fn from_json(raw: &str) -> Result<Self, ConfigError> {
        Ok(serde_json::from_str(raw)?)
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let path = config_path().ok_or(ConfigError::NoHome)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config json failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not resolve home directory")]
    NoHome,
    #[error("config io failed: {0}")]
    Io(#[from] std::io::Error),
}

pub const HOME_NOT_FOUND: &str = "could not resolve ~/.goat/code";

fn shared_home() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".goat"))
}

fn app_home() -> Option<PathBuf> {
    shared_home().map(|home| home.join("code"))
}

pub fn config_path() -> Option<PathBuf> {
    app_home().map(|home| home.join("config.json"))
}

pub fn mcp_config_path() -> Option<PathBuf> {
    app_home().map(|home| home.join("mcp.json"))
}

pub fn db_path() -> Option<PathBuf> {
    app_home().map(|home| home.join("goat-code.db"))
}

pub fn auth_path() -> Option<PathBuf> {
    shared_home().map(|home| home.join("credentials.json"))
}

pub fn log_dir() -> Option<PathBuf> {
    app_home().map(|home| home.join("logs"))
}

pub fn skills_dir() -> Option<PathBuf> {
    app_home().map(|home| home.join("skills"))
}

pub fn browser_dir() -> Option<PathBuf> {
    app_home().map(|home| home.join("browser"))
}

pub fn browser_profile_dir() -> Option<PathBuf> {
    browser_dir().map(|dir| dir.join("profile"))
}

pub fn socket_path() -> Option<PathBuf> {
    app_home().map(|home| home.join("daemon.sock"))
}

pub fn remote_dir() -> Option<PathBuf> {
    app_home().map(|home| home.join("remote"))
}

pub fn update_dir() -> Option<PathBuf> {
    app_home().map(|home| home.join("update"))
}

pub fn bin_dir() -> Option<PathBuf> {
    app_home().map(|home| home.join("bin"))
}

pub const PROJECT_SKILLS_SUBDIR: &str = ".goat/skills";

pub fn agents_dir() -> Option<PathBuf> {
    app_home().map(|home| home.join("agents"))
}

pub const PROJECT_AGENTS_SUBDIR: &str = ".goat/agents";

pub const PROJECT_INSTRUCTIONS_FILE: &str = "AGENTS.md";
pub const PROJECT_INSTRUCTIONS_OVERRIDE_FILE: &str = "AGENTS.override.md";
pub const INSTRUCTIONS_MAX_BYTES: usize = 32 * 1024;

pub fn global_instructions_file() -> Option<PathBuf> {
    app_home().map(|home| home.join(PROJECT_INSTRUCTIONS_FILE))
}

pub fn rate_limits_path() -> Option<PathBuf> {
    app_home().map(|home| home.join("rate_limits.json"))
}

pub fn check_legacy_layout() -> Result<(), String> {
    let Some(home) = std::env::home_dir() else {
        return Ok(());
    };
    let legacy = home.join(".goat-code");
    if legacy.exists() {
        return Err(format!(
            "detected the old {} layout: move it to ~/.goat/code, move ~/.goat-code/auth.json \
             to ~/.goat/credentials.json, then remove ~/.goat-code",
            legacy.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Config, RemoteConfig, SearchConfig, ThemeChoice, WebFetchConfig};

    #[test]
    fn defaults_to_dark() {
        assert_eq!(Config::default().theme, ThemeChoice::Dark);
    }

    #[test]
    fn parses_minimal_json() {
        let cfg = Config::from_json(r#"{ "theme": "light" }"#).unwrap();
        assert_eq!(cfg.theme, ThemeChoice::Light);
    }

    #[test]
    fn empty_object_is_default() {
        assert_eq!(Config::from_json("{}").unwrap(), Config::default());
    }

    #[test]
    fn parses_search_config() {
        let cfg = Config::from_json(
            r#"{
                "search": {
                    "default_target": "searxng/home",
                    "accounts": [
                        { "provider": "searxng", "account": "home", "endpoint": "https://search.example.com" }
                    ]
                }
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.search.default_target.as_deref(), Some("searxng/home"));
        assert_eq!(cfg.search.accounts[0].target(), "searxng/home");
    }

    #[test]
    fn parses_browser_search_config() {
        let cfg = Config::from_json(
            r#"{
                "search": {
                    "default_target": "browser/duckduckgo",
                    "accounts": [
                        { "provider": "browser", "account": "duckduckgo", "engine": "duckduckgo" }
                    ]
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            cfg.search.default_target.as_deref(),
            Some("browser/duckduckgo")
        );
        assert_eq!(cfg.search.accounts[0].target(), "browser/duckduckgo");
    }

    #[test]
    fn round_trips_through_json() {
        let cfg = Config {
            theme: ThemeChoice::Light,
            computer_use_enabled: false,
            browser_enabled: true,
            mouse_capture_enabled: false,
            remote: RemoteConfig::default(),
            search: SearchConfig::default(),
            web_fetch: WebFetchConfig::default(),
        };
        let raw = serde_json::to_string(&cfg).unwrap();
        assert_eq!(Config::from_json(&raw).unwrap(), cfg);
    }
}
