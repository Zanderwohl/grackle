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
                give_bodies_to_whoever_needs_one.run_if(in_state(AppMode::Play)),
                claim_our_own_body,
            ),
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

/// Give a body to everybody who should have one and does not.
///
/// **One rule instead of four.** This used to be a system for a client
/// arriving mid-round, another for clients already connected when the round
/// started, a third to start replicating the host's body, and a spawn inside
/// `enter_play` for the host itself. Each one answered "when does somebody get
/// a body" for its own case, and respawning would have been a fifth.
///
/// Asking about *absence* answers all of them at once. A player with no body
/// gets one, whether they never had one, joined a minute ago, or died two
/// seconds ago. There is no queue to keep, no timer keyed to an entity that
/// has already been despawned, and nothing to get wrong when a case nobody
/// thought of turns up.
///
/// **Respawning is immediate**, which is a placeholder and not a decision.
/// When a body comes back — on a wave, after a delay, at your team's end of
/// the map — is a gamemode question, and this layer deliberately answers none
/// of those. What it guarantees is only that being alive is the resting state.
fn give_bodies_to_whoever_needs_one(
    mut commands: Commands,
    role: Res<NetRole>,
    links: Query<Entity, (With<ClientOf>, With<Connected>)>,
    owners: Query<&ControlledBy, With<Player>>,
    ours: Query<(), (With<Player>, With<LocalPlayer>)>,
    collision: Res<CollisionWorld>,
    spawns: Query<&Transform, With<SpawnPointMarker>>,
    rooms: Query<&Room>,
    mut ids: ResMut<NextPlayerId>,
) {
    if !role.is_authority() {
        return;
    }
    let hosting = matches!(*role, NetRole::Listen { .. });

    // Whoever is sitting at this machine, if anybody is. A dedicated server
    // would have nobody here and the loop below would be the whole of it.
    if ours.is_empty() {
        let spawn = choose_spawn(&collision, &spawns, &rooms);
        info!("Standing our own body up");
        // A fresh id each time rather than one kept across a death: the body
        // that comes back is a new body, and a kill feed that reused the id
        // would credit its damage to the one before it.
        let body = spawn_player(&mut commands, spawn, ids.allocate());
        commands.entity(body).insert(LocalPlayer);
        if hosting {
            commands.entity(body).insert(Replicate::to_clients(NetworkTarget::All));
        }
    }

    for link in &links {
        if owners.iter().any(|owned| owned.owner == link) {
            continue;
        }
        let spawn = choose_spawn(&collision, &spawns, &rooms);
        info!("Standing a client's body up");
        spawn_body_for(&mut commands, spawn, &mut ids, link);
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
