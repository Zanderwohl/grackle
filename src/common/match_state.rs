//! The server decides, for everybody, whether the match is being played or
//! edited, so everyone enters and leaves the round together. On a client F5
//! is answered with a log line rather than swallowed.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::net::NetRole;

/// Reliable and ordered: one mode change out of order would leave a client in
/// the editor for the rest of the round.
pub struct MatchChannel;

/// A statement of the mode, not a toggle: a missed toggle leaves a client
/// inverted, and a client joining mid-round needs to be told where things stand.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchMode {
    pub playing: bool,
}

impl From<AppMode> for MatchMode {
    fn from(mode: AppMode) -> Self {
        Self { playing: !mode.is_authoring() }
    }
}

/// Lossy on purpose: the server has no opinion on `AppMode::Prop`, so a client
/// modeling a prop when the round ends goes back to the map editor.
impl From<MatchMode> for AppMode {
    fn from(mode: MatchMode) -> Self {
        match mode.playing {
            true => AppMode::Play,
            false => AppMode::Editor,
        }
    }
}

pub struct MatchStatePlugin;

impl Plugin for MatchStatePlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<MatchChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ServerToClient);

        app.register_message::<MatchMode>()
            .add_direction(NetworkDirection::ServerToClient);

        app.add_systems(
            Update,
            (
                announce_mode_change.run_if(state_changed::<AppMode>),
                // A client connecting between mode changes still has to be
                // told, and there may be no change coming.
                tell_new_clients,
                follow_the_server,
            ),
        );
    }
}

/// Runs with or without clients; sending to an empty server is a no-op.
fn announce_mode_change(
    mode: Res<State<AppMode>>,
    role: Res<NetRole>,
    server: Option<Single<&Endpoint, With<Server>>>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    if !role.is_authority() {
        return Ok(());
    }
    let Some(server) = server else { return Ok(()) };
    sender.send::<_, MatchChannel>(
        &MatchMode::from(*mode.get()),
        &server,
        &NetworkTarget::All,
    )
}

fn tell_new_clients(
    mode: Res<State<AppMode>>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    for client in &joined {
        sender.send_to_entities::<_, MatchChannel>(
            &MatchMode::from(*mode.get()),
            core::iter::once(client),
        )?;
    }
    Ok(())
}

fn follow_the_server(
    mut receivers: Query<&mut MessageReceiver<MatchMode>>,
    current: Res<State<AppMode>>,
    mut next: ResMut<NextState<AppMode>>,
) {
    for mut receiver in &mut receivers {
        for message in receiver.receive() {
            let wanted = AppMode::from(message);
            // `set` runs the transition even for the same state, and
            // `OnEnter(Play)` rebuilds the match; a restatement must not
            // restart the round.
            if wanted == *current.get() {
                debug!("Server restated the mode: still {wanted:?}");
                continue;
            }
            info!("Server says the match is now {wanted:?}");
            next.set(wanted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mode_survives_the_round_trip() {
        for mode in [AppMode::Editor, AppMode::Play] {
            assert_eq!(AppMode::from(MatchMode::from(mode)), mode);
        }
    }

    #[test]
    fn the_message_is_a_statement_not_a_toggle() {
        assert_eq!(MatchMode::from(AppMode::Play), MatchMode { playing: true });
        assert_eq!(MatchMode::from(AppMode::Editor), MatchMode { playing: false });
    }
}
