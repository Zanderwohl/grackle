//! What the person at this machine has chosen about how they play: a name and
//! whether to reload unasked. Edited in the map editor's Player tab, kept in
//! the user pack, and sent to the server, which is where it is acted on.
//!
//! The server keeps a client's as a [`ClientSettings`] on its link entity,
//! beside its `Identity`, and its own as the [`PlayerSettings`] resource.
//! [`settings_of`] is the one way to ask which applies to whom.

use std::path::Path;

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::assets::USER_PACK;

const SETTINGS_FILE: &str = "player.toml";

/// Characters; a longer name is cut, here and on the server.
pub const NAME_LIMIT: usize = 24;

#[derive(Resource, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerSettings {
    /// Anything, unchecked and not unique. Empty is shown as "Player {id}".
    pub name: String,
    /// Reload a short clip after a second without firing.
    pub auto_reload: bool,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self { name: String::new(), auto_reload: true }
    }
}

impl PlayerSettings {
    /// As the server holds it: trimmed and cut to [`NAME_LIMIT`].
    pub fn cleaned(mut self) -> Self {
        self.name = self.name.trim().chars().take(NAME_LIMIT).collect();
        self
    }
}

/// A client's settings, on the server, on that client's link entity. Gone with
/// the connection, like its `Identity`.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct ClientSettings(pub PlayerSettings);

/// Whose settings apply to a player: ours for the body this machine plays,
/// else the client's on `link`, else the defaults.
pub fn settings_of<'a>(
    ours: bool,
    link: Option<&'a ClientSettings>,
    local: Option<&'a PlayerSettings>,
    fallback: &'a PlayerSettings,
) -> &'a PlayerSettings {
    match ours {
        true => local.unwrap_or(fallback),
        false => link.map_or(fallback, |link| &link.0),
    }
}

/// Reliable and ordered, so the latest change is the one that stands.
pub struct SettingsChannel;

pub struct PlayerSettingsPlugin;

impl Plugin for PlayerSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load())
            .add_systems(Update, save_on_change.run_if(resource_changed::<PlayerSettings>));
    }
}

/// The wire half, beside the transport's other plugins.
pub struct PlayerSettingsSyncPlugin;

impl Plugin for PlayerSettingsSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<SettingsChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<PlayerSettings>().add_direction(NetworkDirection::ClientToServer);
        app.add_systems(Update, (send_our_settings, take_client_settings));
    }
}

fn path() -> std::path::PathBuf {
    Path::new(USER_PACK).join(SETTINGS_FILE)
}

/// Defaults if there is no file or it will not read, which is a first run
/// rather than an error worth stopping for.
fn load() -> PlayerSettings {
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(text) = std::fs::read_to_string(path()) {
        match toml::from_str::<PlayerSettings>(&text) {
            Ok(settings) => return settings.cleaned(),
            Err(e) => warn!("{} will not read, so the defaults stand: {e}", path().display()),
        }
    }
    PlayerSettings::default()
}

/// On every change: a name is typed a letter at a time, and the file is a few
/// bytes.
fn save_on_change(settings: Res<PlayerSettings>) {
    if settings.is_added() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let text = match toml::to_string(&*settings) {
            Ok(text) => text,
            Err(e) => return warn!("Could not write the player settings: {e}"),
        };
        if let Err(e) = std::fs::write(path(), text) {
            warn!("Could not save {}: {e}", path().display());
        }
    }
}

/// On connecting, and again on every change while connected.
fn send_our_settings(
    settings: Option<Res<PlayerSettings>>,
    mut clients: Query<(&mut MessageSender<PlayerSettings>, Ref<Connected>), With<Client>>,
) {
    let Some(settings) = settings else { return };
    for (mut sender, connected) in &mut clients {
        if connected.is_added() || settings.is_changed() {
            sender.send::<SettingsChannel>(settings.clone().cleaned());
        }
    }
}

/// Cleaned again here: a client is not trusted to have done it.
fn take_client_settings(
    mut commands: Commands,
    mut links: Query<(Entity, &mut MessageReceiver<PlayerSettings>), With<ClientOf>>,
) {
    for (link, mut receiver) in &mut links {
        if let Some(latest) = receiver.receive().last() {
            commands.entity(link).insert(ClientSettings(latest.cleaned()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_trimmed_and_cut() {
        let long = PlayerSettings { name: format!("  {}  ", "x".repeat(40)), auto_reload: true }.cleaned();
        assert_eq!(long.name, "x".repeat(NAME_LIMIT));
    }

    #[test]
    fn settings_come_from_wherever_the_player_is() {
        let fallback = PlayerSettings::default();
        let local = PlayerSettings { name: "host".into(), auto_reload: false };
        let client = ClientSettings(PlayerSettings { name: "guest".into(), auto_reload: true });
        assert_eq!(settings_of(true, Some(&client), Some(&local), &fallback).name, "host");
        assert_eq!(settings_of(false, Some(&client), Some(&local), &fallback).name, "guest");
        assert_eq!(*settings_of(false, None, Some(&local), &fallback), fallback);
    }

    /// The file is what a person edits by hand, so a field left out is a
    /// default, not a reason to throw the rest away.
    #[test]
    fn a_settings_file_missing_a_field_keeps_the_rest() {
        let read: PlayerSettings = toml::from_str("name = \"zandy\"\n").unwrap();
        assert_eq!(read, PlayerSettings { name: "zandy".into(), auto_reload: true });
    }
}
