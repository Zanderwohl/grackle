//! Who decides whether the match is being played or edited.
//!
//! **The server does, for everybody.** A client does not enter Play on its own
//! and cannot leave it: it is shown the match the server is running. That is
//! the between-round loop the game is built around — everyone drops into the
//! editor together to change the map, and everyone drops back into the round
//! together — and it only works if one process owns the transition.
//!
//! `F5` therefore does nothing on a client, deliberately and visibly: the key
//! is gated on holding authority rather than swallowed, so a client pressing
//! it says so in the log instead of appearing to be broken.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::net::NetRole;

/// Everything about the match the server has to state outright, as against
/// what is inferred from replicated entities.
///
/// Reliable and ordered: mode changes are rare, and one arriving out of order
/// would put a client into the editor for the rest of the round.
pub struct MatchChannel;

/// "The match is now being played", or "the match is now being edited".
///
/// A whole statement of the mode rather than a toggle. A toggle that went
/// missing would leave a client inverted for the rest of the session, and a
/// client that joins halfway through needs to be told where things stand
/// rather than what just changed.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchMode {
    pub playing: bool,
}

impl From<AppMode> for MatchMode {
    fn from(mode: AppMode) -> Self {
        Self { playing: matches!(mode, AppMode::Play) }
    }
}

impl From<MatchMode> for AppMode {
    fn from(mode: MatchMode) -> Self {
        match mode.playing {
            true => AppMode::Play,
            false => AppMode::Editor,
        }
    }
}

/// Keeps every client's [`AppMode`] equal to the server's.
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
                // Separate from the broadcast: a client that connects
                // between two mode changes still has to be told where things
                // stand, and there may be no change coming.
                tell_new_clients,
                follow_the_server,
            ),
        );
    }
}

/// Tell every client the mode just changed.
///
/// Runs on the host whether or not anybody is connected — `send` to an empty
/// server is a no-op, and gating it on having clients would mean remembering
/// to send when the first one arrives.
fn announce_mode_change(
    mode: Res<State<AppMode>>,
    role: Res<NetRole>,
    server: Option<Single<&Server>>,
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

/// Tell one client the mode as it stands, the moment it connects.
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

/// Be in whatever mode the server says.
fn follow_the_server(
    mut receivers: Query<&mut MessageReceiver<MatchMode>>,
    current: Res<State<AppMode>>,
    mut next: ResMut<NextState<AppMode>>,
) {
    for mut receiver in &mut receivers {
        for message in receiver.receive() {
            let wanted = AppMode::from(message);
            // `set` runs the transition even for the same state, and
            // `OnEnter(Play)` tears the match down and rebuilds it. A repeated
            // statement of the mode we are already in must not restart the
            // round.
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

    /// The wire form says what the mode *is*, not that it changed. A client
    /// joining mid-round is told "playing", and one that misses a message is
    /// corrected by the next one rather than left inverted forever.
    #[test]
    fn the_message_is_a_statement_not_a_toggle() {
        assert_eq!(MatchMode::from(AppMode::Play), MatchMode { playing: true });
        assert_eq!(MatchMode::from(AppMode::Editor), MatchMode { playing: false });
    }
}
