//! Who you are, and where you start. Bodies are stood up by
//! `give_bodies_to_whoever_needs_one` in [`crate::game::net_bodies`]; this
//! answers who is asking and where to put them. See "Coming back" in
//! CLAUDE.md.

use bevy::prelude::*;
use rand::seq::IndexedRandom;

use crate::common::damage::PlayerId;
use crate::common::team::{Side, Team};
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

/// Where to put a body: a usable spawn point on its team's side uniformly at
/// random, else the largest room. The one choice for first spawns and respawns
/// alike.
pub fn choose_spawn(placed: &[Spawn], team: Team, collision: &CollisionWorld, rooms: &[Room]) -> Spawn {
    // Sides are chosen before headroom, so a team whose own spawns are all
    // blocked falls back to the largest room rather than into the enemy base.
    let pool = preferred(placed, team);
    let usable = usable_spawns(&pool, collision);
    if usable.len() < pool.len() {
        warn!(
            "{} of {} spawn point(s) for {} have too little headroom to stand in",
            pool.len() - usable.len(),
            pool.len(),
            team.name()
        );
    }

    match usable.choose(&mut rand::rng()) {
        Some(spawn) => *spawn,
        None => {
            warn!("No usable spawn point for {}; falling back to the largest room", team.name());
            fallback_spawn(rooms)
        }
    }
}

/// The team's own spawns if the map has any, else the ones nobody claimed,
/// else anybody's: a map with no spawn for a team still starts.
fn preferred(placed: &[Spawn], team: Team) -> Vec<Spawn> {
    for wanted in [Some(team), None] {
        let pool: Vec<Spawn> = placed.iter().copied().filter(|spawn| spawn.side == wanted).collect();
        if !pool.is_empty() {
            return pool;
        }
    }
    placed.to_vec()
}

/// Every spawn point on the map. The feature writes its facing into the
/// transform's rotation. `Transform` rather than `GlobalTransform`, which is a
/// frame late; a Mirror's copies hang off an entity at the identity.
pub fn placed_spawns(spawns: &Query<(&Transform, Option<&Side>), With<SpawnPointMarker>>) -> Vec<Spawn> {
    spawns
        .iter()
        .map(|(placed, side)| Spawn {
            feet: placed.translation,
            yaw: placed.rotation.to_euler(EulerRot::YXZ).0,
            side: side.map(|side| side.0),
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

    #[test]
    fn a_team_spawns_on_its_own_side_when_it_has_one() {
        let at = |x: f32, side| Spawn { feet: Vec3::new(x, 0.0, 0.0), yaw: 0.0, side };
        let red = at(1.0, Some(Team::Red));
        let blue = at(2.0, Some(Team::Blue));
        let anybody = at(3.0, None);

        assert_eq!(preferred(&[red, blue, anybody], Team::Blue), vec![blue]);
        assert_eq!(preferred(&[red, anybody], Team::Blue), vec![anybody], "an unclaimed spawn before the enemy's");
        assert_eq!(preferred(&[red], Team::Blue), vec![red], "a map with no spawn for a team still starts");
    }
}
