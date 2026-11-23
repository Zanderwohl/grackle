//! This machine's program settings, in `assets/user/settings.toml`, and the
//! Settings window that edits them, the [`Keymap`](crate::common::keybind::Keymap)
//! and the [`PlayerSettings`](crate::common::player_settings::PlayerSettings).
//! Of the three, only the player settings are sent to the server.

pub mod window;

use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub use window::{request_settings, OpenSettings, SettingsPlugin, SettingsWindow};

#[derive(Resource, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Settings {
    /// A lang file's code, such as `de-DE`; `None` is the fallback language.
    /// `--lang` overrides it for one run.
    pub language: Option<String>,
    /// Read once, when the log is set up; a change takes effect at the next launch.
    pub log_level: LogLevel,
    /// Files in `logs/` older than this are deleted at startup.
    pub log_days: u32,
    /// What servers sent and no session in this many days used is deleted at startup.
    pub download_days: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { language: None, log_level: LogLevel::default(), log_days: DEFAULT_LOG_DAYS, download_days: DEFAULT_DOWNLOAD_DAYS }
    }
}

pub const DEFAULT_LOG_DAYS: u32 = 7;
pub const DEFAULT_DOWNLOAD_DAYS: u32 = 14;
pub const LOG_DAYS_RANGE: std::ops::RangeInclusive<u32> = 1..=365;

/// The least severe level written to the console and to `logs/`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub const ALL: [LogLevel; 5] = [LogLevel::Error, LogLevel::Warn, LogLevel::Info, LogLevel::Debug, LogLevel::Trace];

    pub fn level(self) -> bevy::log::Level {
        use bevy::log::Level;
        match self {
            LogLevel::Error => Level::ERROR,
            LogLevel::Warn => Level::WARN,
            LogLevel::Info => Level::INFO,
            LogLevel::Debug => Level::DEBUG,
            LogLevel::Trace => Level::TRACE,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

pub fn path() -> PathBuf {
    std::path::Path::new(crate::common::assets::USER_PACK).join("settings.toml")
}

impl Settings {
    /// Read before the log exists, so problems come back as text.
    pub fn load() -> (Self, Option<String>) {
        let path = path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(settings) => (settings, None),
                Err(e) => (Self::default(), Some(format!("{} will not read, so the defaults stand: {e}", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (Self::default(), Some(format!("Could not read {}: {e}", path.display()))),
        }
    }

    /// Returns the failure, so the Settings window does not claim a save it did
    /// not make.
    pub fn save(&self) -> Result<(), String> {
        let path = path();
        let text = toml::to_string_pretty(self).map_err(|e| format!("Could not write the settings: {e}"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("Could not write {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let settings = Settings { language: Some("de-DE".to_owned()), log_level: LogLevel::Debug, log_days: 30, download_days: 3 };
        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(toml::from_str::<Settings>("").unwrap(), Settings::default());
    }
}
