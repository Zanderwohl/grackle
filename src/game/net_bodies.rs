//! Who gets a body, and which process is believed about where it is.
//!
//! The server spawns every body, including the host's own. A client never
//! spawns one: its body arrives replicated, marked as predicted, and the
//! client steps its own copy forward from its own inputs so that moving feels
//! immediate — then is corrected whenever the server disagrees.
//!
//! Three kinds of body end up in a client's world and they are not
//! interchangeable:
//!
//! | Body | What it is | Who steps it |
//! | --- | --- | --- |
//! | Predicted | our own, run ahead of the server | us, and replayed on rollback |
//! | Interpolated | somebody else's, drawn slightly in the past | nobody — Lightyear blends it |
//! | Confirmed | the server's last word, kept for comparison | nobody |
//!
//! Stepping an interpolated body is the mistake this module exists to prevent:
//! it does not error, it just makes everybody else's body twitch between where
//! the simulation put it and where the server said it was.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::*;
use rand::seq::IndexedRandom;

use crate::common::app_mode::AppMode;
use crate::common::damage::NextPlayerId;
use crate::common::net::NetRole;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::player::{fallback_spawn, spawn_player, usable_spawns, LocalPlayer, Player, Spawn};
use crate::tool::room::Room;

/// Spawns bodies for connected clients and marks our own when it arrives.
pub struct NetBodiesPlugin;

impl Plugin for NetBodiesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                give_arriving_clients_a_body.run_if(in_state(AppMode::Play)),
                replicate_the_hosts_body.run_if(in_state(AppMode::Play)),
                claim_our_own_body,
            ),
        )
        // Everybody already connected when the round starts needs one too:
        // joining before the round and joining during it must both work, and
        // only one of them goes through the arrival path.
        .add_systems(
            OnEnter(AppMode::Play),
            give_waiting_clients_a_body.after(crate::game::reset::reset_for_play),
        );
    }
}

/// Stand a body up for one client and start replicating it.
///
/// One entity, replicated to everybody, and `ControlledBy` says whose it is.
///
/// No prediction and no interpolation target. Both make a *second* copy of a
/// body on the receiving end, and a second copy is a second writer — which is
/// the whole class of bug this arrangement exists to make impossible. The
/// server steps the body, everybody is told where it ended up, and
/// `interpolate_bodies` smooths between the last two positions it was told.
fn spawn_body_for(
    commands: &mut Commands,
    spawn: Spawn,
    ids: &mut NextPlayerId,
    link: Entity,
) {
    let body = spawn_player(commands, spawn, ids.allocate());
    commands.entity(body).insert((
        Replicate::to_clients(NetworkTarget::All),
        // Ties the body's life to the connection's: somebody who disconnects
        // does not leave a body standing in the map for the rest of the round.
        ControlledBy { owner: link, lifetime: Lifetime::default() },
    ));
}

/// Where a body should be put, asked of the map rather than of a client.
///
/// The same choice `enter_play` makes for the host, and deliberately the same
/// code: a spawn point that is usable for one body is usable for another, and
/// two answers to "where do people start" is how the host ends up somewhere
/// nobody else can be.
fn choose_spawn(
    collision: &CollisionWorld,
    spawns: &Query<&Transform, With<SpawnPointMarker>>,
    rooms: &Query<&Room>,
) -> Spawn {
    let placed: Vec<Spawn> = spawns
        .iter()
        .map(|t| Spawn {
            feet: t.translation,
            yaw: t.rotation.to_euler(EulerRot::YXZ).0,
        })
        .collect();
    let usable = usable_spawns(&placed, collision);
    match usable.choose(&mut rand::rng()) {
        Some(spawn) => *spawn,
        None => {
            warn!("No usable spawn point; falling back to the largest room");
            fallback_spawn(&rooms.iter().cloned().collect::<Vec<Room>>())
        }
    }
}

/// A client turned up mid-round: give it a body now.
fn give_arriving_clients_a_body(
    mut commands: Commands,
    role: Res<NetRole>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    collision: Res<CollisionWorld>,
    spawns: Query<&Transform, With<SpawnPointMarker>>,
    rooms: Query<&Room>,
    mut ids: ResMut<NextPlayerId>,
) {
    if !role.is_authority() {
        return;
    }
    for link in &joined {
        let spawn = choose_spawn(&collision, &spawns, &rooms);
        info!("Giving a new client a body");
        spawn_body_for(&mut commands, spawn, &mut ids, link);
    }
}

/// The round has just started: give everyone already here a body.
fn give_waiting_clients_a_body(
    mut commands: Commands,
    role: Res<NetRole>,
    waiting: Query<Entity, (With<ClientOf>, With<Connected>)>,
    collision: Res<CollisionWorld>,
    spawns: Query<&Transform, With<SpawnPointMarker>>,
    rooms: Query<&Room>,
    mut ids: ResMut<NextPlayerId>,
) {
    if !role.is_authority() {
        return;
    }
    for link in &waiting {
        let spawn = choose_spawn(&collision, &spawns, &rooms);
        info!("Giving a waiting client a body for the new round");
        spawn_body_for(&mut commands, spawn, &mut ids, link);
    }
}

/// Let clients see the host's own body.
///
/// The host's body is spawned by `enter_play` like a solo game's, because a
/// listen server is a solo game that also has company — so the replication has
/// to be added afterwards rather than at the spawn, which knows nothing about
/// the network. Interpolated to everybody: nobody else is driving it, so
/// nobody else should be predicting it.
fn replicate_the_hosts_body(
    mut commands: Commands,
    role: Res<NetRole>,
    ours: Query<Entity, (Added<LocalPlayer>, With<Player>, Without<Replicate>)>,
) {
    if !matches!(*role, NetRole::Listen { .. }) {
        return;
    }
    for body in &ours {
        commands.entity(body).insert(Replicate::to_clients(NetworkTarget::All));
    }
}

/// Recognise the body the server says is ours.
///
/// `Controlled` is the receiver-side half of the `ControlledBy` the server put
/// on it, so it arrives on exactly one body and only on the machine that
/// drives it. That makes it the natural answer to "which of these is mine",
/// and it does not require a second copy of the body to exist the way asking
/// `Predicted` did.
///
/// Marking it `LocalPlayer` is what aims the view at it; `InputMarker` is what
/// makes Lightyear send this machine's inputs for it. It is **not** marked as
/// something we simulate, because we simulate nothing.
fn claim_our_own_body(
    mut commands: Commands,
    ours: Query<Entity, (Added<Controlled>, With<Player>, Without<LocalPlayer>)>,
) {
    for body in &ours {
        info!("The server has given us a body");
        commands.entity(body).insert((
            LocalPlayer,
            InputMarker::<crate::game::player::PlayerInput>::default(),
        ));
    }
}
