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
    pub plan_shell_without_sandbox: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            computer_use_enabled: false,
            browser_enabled: false,
            mouse_capture_enabled: true,
            plan_shell_without_sandbox: false,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        config_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
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

pub const HOME_NOT_FOUND: &str = "could not resolve ~/.goat-code";

fn app_home() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".goat-code"))
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
    app_home().map(|home| home.join("auth.json"))
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

pub fn plans_dir() -> Option<PathBuf> {
    app_home().map(|home| home.join("plans"))
}

pub fn browser_profile_dir() -> Option<PathBuf> {
    browser_dir().map(|dir| dir.join("profile"))
}

pub fn update_dir() -> Option<PathBuf> {
    app_home().map(|home| home.join("update"))
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

#[cfg(test)]
mod tests {
    use super::{Config, ThemeChoice};

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
    fn round_trips_through_json() {
        let cfg = Config {
            theme: ThemeChoice::Light,
            computer_use_enabled: false,
            browser_enabled: true,
            mouse_capture_enabled: false,
            plan_shell_without_sandbox: true,
        };
        let raw = serde_json::to_string(&cfg).unwrap();
        assert_eq!(Config::from_json(&raw).unwrap(), cfg);
    }
}
