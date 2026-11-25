//! Where a match is in its cycle, and the messages that tell clients. The authority decides, in
//! `crate::game::match_cycle`. See [documentation/match-cycle.md](../../documentation/match-cycle.md).

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::match_state::MatchChannel;
use crate::common::net::NetRole;

/// Without the cycle's plugin it never exists, and a round is live throughout ([`live`]).
#[derive(SubStates, Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[source(AppMode = AppMode::Play)]
pub enum MatchPhase {
    #[default]
    PreGame,
    Live,
    PostGame,
    /// No bodies; map editing will go here.
    BetweenRounds,
}

/// The round counts. True with no cycle, as in the game's tests.
pub fn live(phase: Option<Res<State<MatchPhase>>>) -> bool {
    phase.is_none_or(|phase| *phase.get() == MatchPhase::Live)
}

pub fn phase_of(phase: &Option<Res<State<MatchPhase>>>) -> Option<MatchPhase> {
    phase.as_ref().map(|phase| *phase.get())
}

/// Counts matches since the process started. Entering `PreGame` makes a new one, so a restart
/// in place differs from the server restating the phase only by this.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchNumber(pub u32);

/// Seconds left in the pre-game or between rounds.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct Countdown {
    /// The phase counted down, so a client entering one does not show the last count of the one
    /// before. `None` before any count.
    pub phase: Option<MatchPhase>,
    /// `None` while the pre-game waits for players.
    pub left: Option<f32>,
}

impl Countdown {
    /// The count for `phase`, if one has been made since it began.
    pub fn of(&self, phase: MatchPhase) -> Option<Option<f32>> {
        (self.phase == Some(phase)).then_some(self.left)
    }
}

/// Real seconds since this machine entered the pre-game or the time between rounds, for the
/// camera cycle.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct ShotClock(pub f32);

pub fn shows_the_cameras(phase: Option<MatchPhase>) -> bool {
    matches!(phase, Some(MatchPhase::PreGame | MatchPhase::BetweenRounds))
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PhaseStatement {
    pub phase: MatchPhase,
    pub number: u32,
    /// The authority's pick for the post-game, so a client names the same winners; empty otherwise.
    pub winners: Vec<crate::common::damage::PlayerId>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct CountdownMessage {
    pub phase: MatchPhase,
    pub left: Option<f32>,
}

/// Sent every tick, so a lost one is replaced by the next; an old one is worth nothing.
pub struct CountdownChannel;

/// The last phase the server stated, which a client in Play follows.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerPhase(pub Option<PhaseStatement>);

/// The messages, on both ends; the state is `MatchCyclePlugin`'s.
pub struct MatchPhaseSyncPlugin;

impl Plugin for MatchPhaseSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<CountdownChannel>(ChannelSettings { mode: ChannelMode::SequencedUnreliable, ..default() })
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<PhaseStatement>().add_direction(NetworkDirection::ServerToClient);
        app.register_message::<CountdownMessage>().add_direction(NetworkDirection::ServerToClient);
        app.init_resource::<ServerPhase>()
            .init_resource::<Countdown>()
            .init_resource::<MatchNumber>()
            .add_systems(Update, (announce_the_phase, tell_new_clients_the_phase, hear_the_server))
            .add_systems(FixedUpdate, send_the_countdown);
    }
}

fn statement(phase: &Option<Res<State<MatchPhase>>>, number: &MatchNumber, winners: Option<&crate::game::match_cycle::Winners>) -> Option<PhaseStatement> {
    let winners = |phase| match phase {
        MatchPhase::PostGame => winners.map(|winners| winners.0.clone()).unwrap_or_default(),
        _ => Vec::new(),
    };
    phase_of(phase).map(|phase| PhaseStatement { phase, number: number.0, winners: winners(phase) })
}

/// On a change of phase or number; nothing outside Play, where the phase does not exist.
fn announce_the_phase(
    role: Res<NetRole>,
    phase: Option<Res<State<MatchPhase>>>,
    number: Res<MatchNumber>,
    winners: Option<Res<crate::game::match_cycle::Winners>>,
    server: Option<Single<&Endpoint, With<Server>>>,
    mut sent: Local<Option<PhaseStatement>>,
    mut sender: crate::common::net_transport::ToClients,
) -> Result {
    if !role.is_authority() {
        return Ok(());
    }
    let now = statement(&phase, &number, winners.as_deref());
    if now == *sent {
        return Ok(());
    }
    sent.clone_from(&now);
    let (Some(now), Some(server)) = (now, server) else { return Ok(()) };
    sender.send::<_, MatchChannel>(&now, &server, &NetworkTarget::All)
}

fn tell_new_clients_the_phase(
    role: Res<NetRole>,
    phase: Option<Res<State<MatchPhase>>>,
    number: Res<MatchNumber>,
    winners: Option<Res<crate::game::match_cycle::Winners>>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: crate::common::net_transport::ToClients,
) -> Result {
    if !role.is_authority() {
        return Ok(());
    }
    let Some(now) = statement(&phase, &number, winners.as_deref()) else { return Ok(()) };
    for client in &joined {
        sender.send_to_entities::<_, MatchChannel>(&now, core::iter::once(client))?;
    }
    Ok(())
}

fn send_the_countdown(
    role: Res<NetRole>,
    phase: Option<Res<State<MatchPhase>>>,
    countdown: Res<Countdown>,
    server: Option<Single<&Endpoint, With<Server>>>,
    mut sender: crate::common::net_transport::ToClients,
) -> Result {
    let Some(server) = server else { return Ok(()) };
    if !role.is_authority() || !shows_the_cameras(phase_of(&phase)) {
        return Ok(());
    }
    let Some(phase) = countdown.phase else { return Ok(()) };
    sender.send::<_, CountdownChannel>(&CountdownMessage { phase, left: countdown.left }, &server, &NetworkTarget::All)
}

fn hear_the_server(
    mut phases: Query<&mut MessageReceiver<PhaseStatement>>,
    mut countdowns: Query<&mut MessageReceiver<CountdownMessage>>,
    mut server_phase: ResMut<ServerPhase>,
    mut countdown: ResMut<Countdown>,
) {
    for mut receiver in &mut phases {
        for message in receiver.receive() {
            server_phase.0 = Some(message);
        }
    }
    for mut receiver in &mut countdowns {
        for message in receiver.receive() {
            countdown.set_if_neq(Countdown { phase: Some(message.phase), left: message.left });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Postcard refuses an internally tagged enum; the phase must survive the wire's encoding.
    #[test]
    fn a_statement_survives_the_wire_encoding() {
        let sent = PhaseStatement { phase: MatchPhase::PostGame, number: 7, winners: vec![crate::common::damage::PlayerId(3)] };
        let bytes = postcard::to_allocvec(&sent).unwrap();
        assert_eq!(postcard::from_bytes::<PhaseStatement>(&bytes).unwrap(), sent);
        let sent = CountdownMessage { phase: MatchPhase::BetweenRounds, left: Some(3.5) };
        let bytes = postcard::to_allocvec(&sent).unwrap();
        assert_eq!(postcard::from_bytes::<CountdownMessage>(&bytes).unwrap(), sent);
    }
}
