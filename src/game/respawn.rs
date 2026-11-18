//! Who you are, and where you start. Bodies are stood up by
//! `give_bodies_to_whoever_needs_one` in [`crate::game::net_bodies`]; this
//! answers who is asking and where to put them. See "Coming back" in
//! CLAUDE.md.

use bevy::prelude::*;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};

use crate::common::class::{body_center_from_feet, Class};
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
    /// What their next body is. A living body keeps its own [`Class`], so a choice made while
    /// alive waits for the next spawn. Forgotten at each match's start.
    pub class: Option<Class>,
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

/// Meters within which another body makes a spawn point all but the last choice.
pub const CROWDED: f32 = 2.5;
/// Larger than the summed score of every other body, so a crowded spawn loses to every
/// uncrowded one, and two crowded ones are told apart by how many crowd them.
const CROWDED_PENALTY: f32 = 10_000.0;
/// Meters past which a body counts as far, so many distant bodies cannot outweigh one near.
const FAR: f32 = 15.0;
/// Meters inside which a body costs more than its distance, ramping up toward [`CROWDED`].
const NEAR: f32 = 8.0;
/// Extra score lost per meter inside [`NEAR`].
const NEAR_COST: f32 = 2.0;
/// Scores this close to the best are a tie, so mirrored spawns are picked at random.
const TIE: f32 = 0.5;

/// The usable spawn on the team's side farthest from `others` (body centers), else
/// the largest room; with no team, any spawn. For first spawns and respawns alike.
pub fn choose_spawn(placed: &[Spawn], team: Option<Team>, others: &[Vec3], collision: &CollisionWorld) -> Spawn {
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

    match farthest(&usable, others).choose(&mut rand::rng()) {
        Some(spawn) => *spawn,
        None => {
            warn!("No usable spawn point for {}; falling back to the largest room", team_name(team));
            fallback_spawn(collision)
        }
    }
}

/// Every spawn within [`TIE`] of the best [`spread`] score.
fn farthest(spawns: &[Spawn], others: &[Vec3]) -> Vec<Spawn> {
    let scores: Vec<f32> = spawns.iter().map(|spawn| spread(spawn, others)).collect();
    let best = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    spawns.iter().zip(scores).filter(|(_, score)| *score >= best - TIE).map(|(spawn, _)| *spawn).collect()
}

/// Higher is better. Straight-line, so a body behind a wall counts.
fn spread(spawn: &Spawn, others: &[Vec3]) -> f32 {
    let center = body_center_from_feet(spawn.feet);
    others
        .iter()
        .map(|other| {
            let distance = center.distance(*other);
            let crowded = if distance < CROWDED { CROWDED_PENALTY } else { 0.0 };
            distance.min(FAR) - NEAR_COST * (NEAR - distance).max(0.0) - crowded
        })
        .sum()
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

    fn at(x: f32) -> Spawn {
        Spawn { feet: Vec3::new(x, 0.0, 0.0), yaw: 0.0, side: None }
    }

    fn body_at(x: f32) -> Vec3 {
        body_center_from_feet(Vec3::new(x, 0.0, 0.0))
    }

    #[test]
    fn with_nobody_else_standing_every_spawn_ties() {
        let spawns = [at(0.0), at(10.0), at(20.0)];
        assert_eq!(farthest(&spawns, &[]), spawns.to_vec());
    }

    #[test]
    fn the_spawn_farthest_from_everybody_wins() {
        let spawns = [at(0.0), at(10.0), at(40.0)];
        assert_eq!(farthest(&spawns, &[body_at(5.0), body_at(12.0)]), vec![at(40.0)]);
    }

    #[test]
    fn one_near_body_outweighs_many_far_ones() {
        // 3 m from one and 60 m from six, against 20 m from all seven.
        let near = at(0.0);
        let clear = Spawn { feet: Vec3::new(0.0, 0.0, 200.0), ..at(0.0) };
        let mut others = vec![body_at(3.0)];
        others.extend((0..6).map(|_| body_at(-60.0)));
        let clear_others: Vec<Vec3> = (0..7).map(|_| body_center_from_feet(Vec3::new(0.0, 0.0, 220.0))).collect();
        assert!(spread(&clear, &clear_others) > spread(&near, &others));
    }

    #[test]
    fn mirrored_spawns_tie_despite_rounding() {
        let spawns = [at(-10.0), at(10.000_001)];
        assert_eq!(farthest(&spawns, &[body_at(0.0)]).len(), 2);
    }

    #[test]
    fn a_spawn_with_somebody_standing_on_it_comes_last() {
        // Far from the crowd but with one body on it, against near the crowd and clear.
        let spawns = [at(0.0), at(100.0)];
        let others = [body_at(3.0), body_at(3.0), body_at(3.0), body_at(3.0), body_at(3.0), body_at(3.0), body_at(101.0)];
        assert_eq!(farthest(&spawns, &others), vec![at(0.0)]);
    }

    #[test]
    fn more_bodies_than_spawns_still_share_them_out() {
        // As a round start places one body after another, each counted for the next.
        let spawns = [at(0.0), at(30.0), at(60.0)];
        let mut others = Vec::new();
        let mut used = Vec::new();
        for _ in 0..spawns.len() {
            let spawn = farthest(&spawns, &others)[0];
            others.push(body_center_from_feet(spawn.feet));
            used.push(spawn.feet.x);
        }
        used.sort_by(f32::total_cmp);
        assert_eq!(used, vec![0.0, 30.0, 60.0], "two bodies on one spawn while another stood empty");
        let fourth = farthest(&spawns, &others);
        assert!(!fourth.is_empty());
        for spawn in fourth {
            let on_it = others.iter().filter(|other| other.distance(body_center_from_feet(spawn.feet)) < CROWDED).count();
            assert_eq!(on_it, 1, "a fourth body doubled up on a spawn already holding {on_it}");
        }
    }
}
