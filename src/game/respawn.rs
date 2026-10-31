//! Who you are, and where you start. Bodies are stood up by
//! `give_bodies_to_whoever_needs_one` in [`crate::game::net_bodies`]; this
//! answers who is asking and where to put them. See "Coming back" in
//! CLAUDE.md.

use bevy::prelude::*;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};

use crate::common::damage::PlayerId;
use crate::common::player_color::PlayerColor;
use crate::common::team::{Side, Team};
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::player::{fallback_spawn, usable_spawns, Spawn};

/// Who somebody is, kept across deaths so a respawn is the same player on the
/// same team. Never on a body: in [`OurIdentity`] for this machine (dropped
/// by `enter_play`), or on a client's link entity (gone with the connection).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Identity {
    pub id: PlayerId,
    pub seat: Seat,
    /// Which hue on the wheel, for [`PlayerColor::nth`]. An index rather than
    /// the hue, so a new match's [`HueStart`] turns everybody's together.
    pub color: usize,
}

impl Identity {
    /// `None` while choosing, or in a mode without teams.
    pub fn team(&self) -> Option<Team> {
        self.seat.team()
    }

    pub fn color(&self, start: &HueStart) -> PlayerColor {
        PlayerColor::nth(start.0, self.color)
    }
}

/// Only a player who is playing is owed a body or has a standing; whoever
/// is choosing counts as dead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Seat {
    #[default]
    Choosing,
    /// `None` in a mode without teams.
    Playing(Option<Team>),
    /// Watching: on no team, and counted in no balance. See [`crate::game::spectate`].
    Spectating,
}

impl Seat {
    pub fn team(self) -> Option<Team> {
        match self {
            Seat::Choosing | Seat::Spectating => None,
            Seat::Playing(team) => team,
        }
    }

    pub fn is_playing(self) -> bool {
        matches!(self, Seat::Playing(_))
    }
}

/// Where this match's color wheel starts, in degrees. Random per match, so
/// the same player is not always the same red; dropped with [`OurIdentity`].
#[derive(Resource, Clone, Copy, Debug)]
pub struct HueStart(pub f32);

impl HueStart {
    pub fn random() -> Self {
        Self(rand::random_range(0.0..360.0))
    }
}

/// The identity of whoever is sitting at this machine. Absent on a client,
/// which learns who it is from the body the server hands it.
#[derive(Resource, Clone, Copy, Debug)]
pub struct OurIdentity(pub Identity);

/// For the log.
pub fn team_name(team: Option<Team>) -> String {
    team.map_or_else(|| "no team".to_string(), Team::name)
}

/// Where to put a body: a usable spawn point on its team's side uniformly at
/// random, else the largest room. With no team, any spawn. The one choice for
/// first spawns and respawns alike.
pub fn choose_spawn(placed: &[Spawn], team: Option<Team>, collision: &CollisionWorld) -> Spawn {
    // Sides are chosen before headroom, so a team whose own spawns are all
    // blocked falls back to the largest room rather than into the enemy base.
    let pool = preferred(placed, team);
    let usable = usable_spawns(&pool, collision);
    if usable.len() < pool.len() {
        warn!(
            "{} of {} spawn point(s) for {} have too little headroom to stand in",
            pool.len() - usable.len(),
            pool.len(),
            team_name(team)
        );
    }

    match usable.choose(&mut rand::rng()) {
        Some(spawn) => *spawn,
        None => {
            warn!("No usable spawn point for {}; falling back to the largest room", team_name(team));
            fallback_spawn(collision)
        }
    }
}

/// The team's own spawns if the map has any, else the ones nobody claimed,
/// else anybody's: a map with no spawn for a team still starts. Nobody's team
/// is anybody's spawn.
fn preferred(placed: &[Spawn], team: Option<Team>) -> Vec<Spawn> {
    let Some(team) = team else { return placed.to_vec() };
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
    fn a_team_spawns_on_its_own_side_when_it_has_one() {
        let at = |x: f32, side| Spawn { feet: Vec3::new(x, 0.0, 0.0), yaw: 0.0, side };
        let red = at(1.0, Some(Team::Red));
        let blue = at(2.0, Some(Team::Blue));
        let anybody = at(3.0, None);

        assert_eq!(preferred(&[red, blue, anybody], Some(Team::Blue)), vec![blue]);
        assert_eq!(preferred(&[red, anybody], Some(Team::Blue)), vec![anybody], "an unclaimed spawn before the enemy's");
        assert_eq!(preferred(&[red], Some(Team::Blue)), vec![red], "a map with no spawn for a team still starts");
        assert_eq!(preferred(&[red, blue, anybody], None), vec![red, blue, anybody], "a side kept somebody with no team out");
    }
}
