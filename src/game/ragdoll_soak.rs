//! Seeded soaks of corpses and gibs on the shipped maps: bodies killed at random places, many
//! against walls and on ledges, in random poses, carried and thrown by random blasts and shots.
//! A corpse still moving when its lifetime is up, or with a point left inside a wall, fails.

use bevy::prelude::*;
use rand::{rngs::StdRng, Rng, SeedableRng};

use crate::common::class::CLASS_HALF_EXTENTS;
use crate::common::skeleton::rig::{humanoid, Proportions};
use crate::common::skeleton::{Pose, Skeleton};
use crate::constants::MAP_BLUEPRINT_EXTENSION;
use crate::game::collision::CollisionWorld;
use crate::game::ragdoll::{BlownApart, Push, Ragdoll, RAGDOLL_LIFETIME};

const DT: f32 = 1.0 / 64.0;

/// Meters the furthest point of a corpse may move in its last second and still count as at rest.
const STILL: f32 = 0.3;
const FALL: f32 = -crate::common::movement_rules::DEFAULT_GRAVITY;

/// A shipped map's walls, stairs included, and the box round its floor.
pub(crate) fn shipped_map(name: &str) -> (CollisionWorld, Vec3, Vec3) {
    let path = std::path::PathBuf::from(format!("assets/default/blueprints/{name}.{MAP_BLUEPRINT_EXTENSION}"));
    let mut timeline = crate::editor::save::load(&path).unwrap().timeline;
    timeline.resolve_all();
    let mut volumes = Vec::new();
    let mut stairs = Vec::new();
    for (_, feature) in timeline.active_features() {
        let object = feature.object();
        volumes.extend(object.volumes());
        stairs.extend(object.stairs());
        stairs.extend(object.standing_copies().iter().filter_map(|copy| copy.object.stairs()));
    }
    let mut world = CollisionWorld::default();
    world.rebuild_map(&volumes, &stairs);
    let (mut least, mut most) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for volume in &volumes {
        let (min, max) = volume.shape.bounds();
        least = least.min(min);
        most = most.max(max);
    }
    (world, least, most)
}

/// What killed the body.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Death {
    /// A shot: one tight shove along a direction.
    Shot { speed: f32, radius: f32 },
    /// A blast near the body, which leaves it whole.
    Splash { knockback: f32, radius: f32 },
    /// A direct hit, which blows it apart.
    Gibbed { knockback: f32, radius: f32 },
}

/// One randomized death: where, in what pose, moving how, and how it died.
#[derive(Clone, Debug)]
pub(crate) struct Case {
    pub root: Transform,
    pub pose: Pose,
    pub carried: Vec3,
    pub death: Death,
    /// Where the shot or blast was, in world space.
    pub at: Vec3,
    pub direction: Vec3,
}

fn bent(skeleton: &Skeleton, rng: &mut StdRng) -> Pose {
    let mut pose = Pose::rest();
    for bone in skeleton.bones() {
        if rng.random_bool(0.5) {
            let axis = Vec3::new(rng.random_range(-1.0..1.0), rng.random_range(-1.0..1.0), rng.random_range(-1.0..1.0))
                .normalize_or(Vec3::X);
            pose = pose.with(bone.name, Quat::from_axis_angle(axis, rng.random_range(-1.2..1.2)));
        }
    }
    pose
}

/// Somewhere a body stands in the map: on a floor, and half the time pressed against a wall.
fn somewhere(world: &CollisionWorld, least: Vec3, most: Vec3, rng: &mut StdRng) -> Vec3 {
    let half = CLASS_HALF_EXTENTS;
    loop {
        let center = Vec3::new(
            rng.random_range(least.x..most.x),
            rng.random_range(least.y + half.y..most.y - half.y),
            rng.random_range(least.z..most.z),
        );
        if !world.fits(center, half) {
            continue;
        }
        let mut center = center;
        if rng.random_bool(0.6) {
            let yaw = rng.random_range(0.0..std::f32::consts::TAU);
            let (moved, _) = world.slide(center, half, Vec3::new(yaw.cos(), 0.0, yaw.sin()) * 20.0);
            center = moved;
        }
        let Some((ground, _)) = world.ground(center, half, 30.0, 0.7) else { continue };
        if world.fits(ground, half) {
            return ground - Vec3::Y * half.y;
        }
    }
}

impl Case {
    pub(crate) fn rolled(seed: u64, skeleton: &Skeleton, world: &CollisionWorld, least: Vec3, most: Vec3) -> Case {
        let mut rng = StdRng::seed_from_u64(seed);
        let feet = somewhere(world, least, most, &mut rng);
        let root = Transform::from_translation(feet)
            .with_rotation(Quat::from_rotation_y(rng.random_range(0.0..std::f32::consts::TAU)));
        let pose = bent(skeleton, &mut rng);
        // Per tick: up to a rocket jumper's 25 m/s.
        let carried = Vec3::new(rng.random_range(-1.0..1.0), rng.random_range(-0.5..1.0), rng.random_range(-1.0..1.0))
            * rng.random_range(0.0..25.0)
            * DT;
        let middle = feet + Vec3::Y * rng.random_range(0.2..1.8);
        let offset = Vec3::new(rng.random_range(-1.0..1.0), rng.random_range(-1.0..0.5), rng.random_range(-1.0..1.0));
        let direction = Vec3::new(rng.random_range(-1.0..1.0), rng.random_range(-0.3..0.6), rng.random_range(-1.0..1.0))
            .normalize_or(Vec3::X);
        let (death, at) = match rng.random_range(0..3) {
            0 => (Death::Shot { speed: rng.random_range(2.0..40.0), radius: rng.random_range(0.2..1.2) }, middle),
            1 => (
                Death::Splash { knockback: rng.random_range(5.0..30.0), radius: rng.random_range(1.5..4.0) },
                middle + offset * rng.random_range(0.0..1.5),
            ),
            _ => (
                Death::Gibbed { knockback: rng.random_range(5.0..30.0), radius: rng.random_range(1.5..4.0) },
                middle + offset * rng.random_range(0.0..0.4),
            ),
        };
        Case { root, pose, carried, death, at, direction }
    }

    /// As [`raise_ragdolls`](crate::game::ragdoll::raise_ragdolls) makes them, then shoved.
    pub(crate) fn corpses(&self, skeleton: &Skeleton, world: &CollisionWorld) -> Vec<Ragdoll> {
        let whole = Ragdoll::from_body(skeleton, &self.pose, &self.root, self.carried).seated(skeleton, world);
        let mut corpses = match self.death {
            Death::Gibbed { .. } => Ragdoll::tear(whole, skeleton, &self.pose, &self.root, BlownApart::Nearest(self.at)),
            _ => vec![whole],
        };
        for corpse in &mut corpses {
            match self.death {
                Death::Shot { speed, radius } => corpse.shove(self.at, Push::Along(self.direction * speed), radius, DT),
                Death::Splash { knockback, radius } | Death::Gibbed { knockback, radius } => {
                    corpse.shove(self.at, Push::Outward(knockback), radius, DT)
                }
            }
        }
        corpses
    }
}

/// How a corpse ended up after its lifetime.
#[derive(Debug)]
pub(crate) struct Outcome {
    pub asleep: bool,
    /// Points inside a wall or outside the map at the end.
    pub buried: usize,
    /// Meters its furthest point moved over the last second.
    pub last_second: f32,
}

pub(crate) fn soak(corpse: &mut Ragdoll, skeleton: &Skeleton, world: &CollisionWorld, seconds: f32) -> Outcome {
    let ticks = (seconds / DT) as usize;
    let mut a_second_ago: Vec<Vec3> = Vec::new();
    let kept: Vec<usize> = (0..skeleton.bones().len()).filter(|&index| corpse.keeps(index)).collect();
    let points = |corpse: &Ragdoll| -> Vec<Vec3> {
        kept.iter()
            .flat_map(|&index| {
                let (head, tail) = corpse.bone_points(index);
                [head, tail]
            })
            .collect()
    };
    for tick in 0..ticks {
        if tick + 64 == ticks {
            a_second_ago = points(corpse);
        }
        corpse.step(skeleton, world, FALL, DT);
    }
    let now = points(corpse);
    let last_second = now.iter().zip(&a_second_ago).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
    // A point's own box, shrunk by a millimeter so resting on a surface is not counted.
    let buried = kept
        .iter()
        .flat_map(|&index| {
            let (head, tail) = corpse.bone_points(index);
            [head, tail]
        })
        .filter(|&at| !world.fits(at, Vec3::splat(0.035)))
        .count();
    Outcome { asleep: corpse.is_asleep(), buried, last_second }
}

pub(crate) fn humanoid_rig() -> Skeleton {
    humanoid(Proportions::DEFAULT)
}

/// Every corpse and piece, by seed, that is still moving at the end of its lifetime or has a
/// point left in a wall. Seeds are spread over threads; the result is in seed order.
pub(crate) fn restless_in(
    world: &CollisionWorld,
    least: Vec3,
    most: Vec3,
    seeds: std::ops::Range<u64>,
    skeleton: &Skeleton,
) -> Vec<(u64, usize, Death, Outcome)> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()) as u64;
    let mut failing: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|thread| {
                let seeds = seeds.clone();
                scope.spawn(move || {
                    let mut failing = Vec::new();
                    for seed in seeds.filter(|seed| seed % threads == thread) {
                        let case = Case::rolled(seed, skeleton, world, least, most);
                        for (nth, mut corpse) in case.corpses(skeleton, world).into_iter().enumerate() {
                            let outcome = soak(&mut corpse, skeleton, world, RAGDOLL_LIFETIME);
                            if !outcome.asleep || outcome.buried > 0 {
                                failing.push((seed, nth, case.death, outcome));
                            }
                        }
                    }
                    failing
                })
            })
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().unwrap()).collect()
    });
    failing.sort_by_key(|(seed, nth, ..)| (*seed, *nth));
    failing
}

pub(crate) fn restless(map: &str, seeds: std::ops::Range<u64>, skeleton: &Skeleton) -> Vec<(u64, usize, Death, Outcome)> {
    let (world, least, most) = shipped_map(map);
    restless_in(&world, least, most, seeds, skeleton)
}

/// The seeds to soak: `RAGDOLL_SOAK_SEEDS` of them from `RAGDOLL_SOAK_SEED`, else `count` from 0.
fn seeds(count: u64) -> std::ops::Range<u64> {
    let read = |name: &str| std::env::var(name).ok().and_then(|value| value.parse::<u64>().ok());
    let first = read("RAGDOLL_SOAK_SEED").unwrap_or(0);
    first..first + read("RAGDOLL_SOAK_SEEDS").unwrap_or(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every corpse and gib, killed at random on each shipped map, has come to rest by the end
    /// of its lifetime with nothing inside a wall. Fixed seeds by default, so it fails the same
    /// way every run; set `RAGDOLL_SOAK_SEED` and `RAGDOLL_SOAK_SEEDS` to look elsewhere, and a
    /// failure names the seed to replay. A failure means something feeds energy into the solver
    /// or lets a point through a wall, not a constant to nudge.
    ///
    /// "At rest" is [`STILL`], not asleep: about one corpse in a thousand ends with a light
    /// point flickering by a few centimeters, which the survey lists.
    #[test]
    fn every_corpse_on_the_shipped_maps_comes_to_rest_clear_of_the_walls() {
        let skeleton = humanoid_rig();
        let mut failing = Vec::new();
        for map in ["arena", "cloister", "waterworks"] {
            for (seed, piece, death, outcome) in restless(map, seeds(32), &skeleton) {
                if outcome.buried > 0 || outcome.last_second > STILL {
                    failing.push(format!("{map} seed {seed} piece {piece}: {death:?} {outcome:?}"));
                }
            }
        }
        assert!(failing.is_empty(), "{} restless or buried:\n{}", failing.len(), failing.join("\n"));
    }

    /// Prints what fails over many seeds, by map. Run with `--ignored --nocapture`, and
    /// `RAGDOLL_SOAK_SEEDS=400` or so.
    #[test]
    #[ignore]
    fn survey_restless_corpses() {
        let skeleton = humanoid_rig();
        for map in ["arena", "cloister", "waterworks"] {
            let seeds = seeds(200);
            let count = seeds.end - seeds.start;
            let failing = restless(map, seeds, &skeleton);
            let fast = failing.iter().filter(|failure| failure.3.last_second > STILL).count();
            println!("{map}: {} failing of {count} seeds, {fast} still moving fast", failing.len());
            for (seed, piece, death, outcome) in &failing {
                println!("  seed {seed} piece {piece} {death:?} {outcome:?}");
            }
        }
    }
}
