//! This machine's program settings, in `assets/user/settings.toml`, and the
//! Settings window that edits them and the [`Keymap`](crate::common::keybind::Keymap).
//! Unlike [`PlayerSettings`](crate::common::player_settings), nothing here is
//! sent to the server.

pub mod window;

use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub use window::{request_settings, SettingsPlugin, SettingsWindow};

#[derive(Resource, Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Settings {
    /// A lang file's code, such as `de-DE`; `None` is the fallback language.
    /// `--lang` overrides it for one run.
    pub language: Option<String>,
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
        let settings = Settings { language: Some("de-DE".to_owned()) };
        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(toml::from_str::<Settings>("").unwrap(), Settings::default());
    }
}
