//! Who you are, and where you start. Bodies are stood up by
//! `give_bodies_to_whoever_needs_one` in [`crate::game::net_bodies`]; this
//! answers who is asking and where to put them. See "Coming back" in
//! CLAUDE.md.

use bevy::prelude::*;
use rand::seq::IndexedRandom;

use crate::common::damage::PlayerId;
use crate::common::team::Team;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::player::{fallback_spawn, usable_spawns, Spawn};
use crate::tool::room::Room;

/// Who somebody is, kept across deaths so a respawn is the same player on the
/// same team. Never on a body: in [`OurIdentity`] for this machine (dropped
/// by `enter_play`), or on a client's link entity (gone with the connection).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Identity {
    pub id: PlayerId,
    pub team: Team,
}

/// The identity of whoever is sitting at this machine. Absent on a client,
/// which learns who it is from the body the server hands it.
#[derive(Resource, Clone, Copy, Debug)]
pub struct OurIdentity(pub Identity);

/// Alternates, a placeholder, so two people on one map are on different
/// sides and friendly fire can be checked by hand.
pub fn next_team(already_here: usize) -> Team {
    match already_here % 2 {
        0 => Team::Red,
        _ => Team::Blue,
    }
}

/// Where to put a body: a usable spawn point uniformly at random, else the
/// largest room. The one choice for first spawns and respawns alike.
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

/// Every spawn point on the map. The feature writes its facing into the
/// transform's rotation.
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

    #[test]
    fn the_second_player_is_put_on_the_other_side() {
        assert_eq!(next_team(0), Team::Red);
        assert_eq!(next_team(1), Team::Blue);
        assert_ne!(next_team(0), next_team(1));
    }
}
