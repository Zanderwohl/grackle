//! Who you are, and where you start.
//!
//! **Your identity outlives your body.** [`Identity`] holds a [`PlayerId`] and
//! a [`Team`] for as long as somebody is in the match, and every body they are
//! given carries the *same* one. A new match is a new player; a new life is
//! not. A scoreboard that handed out a fresh id per death would show one
//! player per life, and a team that came back different would put you on the
//! other side for dying.
//!
//! **Where a body goes is decided in one place**, [`choose_spawn`], shared by
//! the first body of a match and every one after it. Two copies of "pick a
//! usable spawn point, fall back to the largest room" is one copy that quietly
//! stops matching the other, and the failure is a body that respawns inside a
//! wall on maps the first spawn handles fine.
//!
//! **What is not here is the standing-up.** That is
//! `give_bodies_to_whoever_needs_one` in [`crate::game::net_bodies`], which
//! asks a single question — who should have a body and does not — and so
//! covers starting a round, joining one mid-match, and dying, without a queue,
//! a timer, or a system per case. This module answers what that one needs to
//! know: who is asking, and where to put them.
//!
//! Respawning is **immediate**, which is a placeholder and meant to look like
//! one. A respawn timer, wave respawns, spawn protection and whether you come
//! back at all are a gamemode's numbers, and none can be answered here without
//! answering them for every gamemode at once. What is not a placeholder is the
//! shape: being alive is the resting state.
//!
//! The corpse is left where it fell. `raise_ragdolls` stands one up before the
//! reaping, so what you see on coming back is the body you were in a moment
//! ago, lying there.

use bevy::prelude::*;
use rand::seq::IndexedRandom;

use crate::common::damage::PlayerId;
use crate::common::team::Team;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::player::{fallback_spawn, usable_spawns, Spawn};
use crate::tool::room::Room;

/// Who somebody is, independent of whatever body they are currently wearing.
///
/// Never stored on a body, for exactly the reason it exists: a body is the
/// thing that keeps being destroyed. It lives wherever the *player* lives, and
/// there are two of those:
///
/// | Whose | Where it lives | How long |
/// | --- | --- | --- |
/// | the person at this machine | [`OurIdentity`], a resource | the match |
/// | a connected client | a component on their link entity | the connection |
///
/// Two homes rather than one because those are genuinely two lifetimes, and
/// each is already exactly right: a round ends and `enter_play` drops the
/// resource, so the next match is a new player; somebody disconnects and their
/// link goes with them, taking the identity a returning player must not
/// inherit. Neither needs a roster to be kept in step with who is actually
/// here.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Identity {
    pub id: PlayerId,
    pub team: Team,
}

/// The identity of whoever is sitting at this machine.
///
/// Absent in the editor, and absent on a client — a client is *told* who it is
/// by the body the server hands it, and a second opinion held locally is the
/// two-writers mistake this branch is mostly about.
#[derive(Resource, Clone, Copy, Debug)]
pub struct OurIdentity(pub Identity);

/// The side the next player to need one is put on.
///
/// Alternating, which is a placeholder and meant to look like one — there is
/// no lobby, no team select and no balancing worth the name. What it buys is
/// that two people on one map are on *different* sides, so the rule in
/// [`crate::common::team`] is something you can walk up to and check rather
/// than something only the animation grid exercises. A gamemode picks this the
/// day there is one.
pub fn next_team(already_here: usize) -> Team {
    match already_here % 2 {
        0 => Team::Red,
        _ => Team::Blue,
    }
}

/// Where to put a body, given what the map offers.
///
/// Shared by the first spawn of a match and every one after it. Uniformly at
/// random for now: per-team spawns, and not dropping somebody on top of
/// somebody else, are gamemode questions this is deliberately not trying to
/// answer yet — and both of them get harder, not easier, if the two spawn
/// paths have drifted apart by the time they are asked.
///
/// Warns about spawn points it had to skip, and falls back to the largest room
/// rather than refusing to put anybody anywhere. A map with no usable spawn is
/// a map you can still walk around in.
pub fn choose_spawn(placed: &[Spawn], collision: &CollisionWorld, rooms: &[Room]) -> Spawn {
    let usable = usable_spawns(placed, collision);
    if usable.len() < placed.len() {
        warn!(
            "{} of {} spawn point(s) have too little headroom to stand in",
            placed.len() - usable.len(),
            placed.len()
        );
    }

    match usable.choose(&mut rand::rng()) {
        Some(spawn) => *spawn,
        None => {
            warn!("No usable spawn point on this map; falling back to the largest room");
            fallback_spawn(rooms)
        }
    }
}

/// Every spawn point on the map, as somewhere a body could stand.
///
/// The feature writes its facing into the transform's rotation, so the marked
/// entity carries both halves and neither has to be looked up twice.
pub fn placed_spawns(spawns: &Query<&Transform, With<SpawnPointMarker>>) -> Vec<Spawn> {
    spawns
        .iter()
        .map(|placed| Spawn {
            feet: placed.translation,
            yaw: placed.rotation.to_euler(EulerRot::YXZ).0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both sides are handed out, so two people on one map can shoot each
    /// other. With everybody on one team the rule in `common::team` is
    /// unreachable outside the animation grid.
    #[test]
    fn the_second_player_is_put_on_the_other_side() {
        assert_eq!(next_team(0), Team::Red);
        assert_eq!(next_team(1), Team::Blue);
        assert_ne!(next_team(0), next_team(1));
    }
}
