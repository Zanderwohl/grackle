//! Where the body clips came from: [`AnimationState::pose`], sampled. Once
//! seeded, the clip file is the source of truth and this is only the generator,
//! as `new_map_template` is for the new-map blueprint.
//!
//! Used by the `bake_body_clips` binary and by [`clip_file`]'s fidelity test,
//! which bakes in memory so the shipped clips stay free to be hand-edited.
//!
//! The pose functions are kept because their constants carry the reasoning a
//! clip cannot; each seed's [`Seed::note`] points back to them.
//!
//! [`clip_file`]: crate::common::clip_file

use bevy::prelude::*;

use crate::common::rotation::euler_from_quat;
use crate::common::skeleton::clip::{
    Channel, Clip, Clock, Combine, Key, Space, StateAnimation, Track, Unit, Variant,
};
use crate::common::skeleton::state::{AnimationState, PoseInputs, IDLE_PERIOD};
use crate::common::skeleton::{default_humanoid, Interpolation, Pose};

/// Keys across a looping non-stride cycle. Raise this if
/// `every_baked_clip_still_matches_its_function` (0.2° tolerance) fails.
const KEYS_PER_CYCLE: usize = 16;

/// How finely a stride is sampled before keys are chosen from it.
///
/// A gait has corners where a foot lands and lifts, which evenly spaced keys
/// cannot hit (64 even keys measured worse than 32). So each track keeps only
/// the samples needed to stay within [`STRIDE_ANGLE`] or [`STRIDE_ROOT`].
const STRIDE_SAMPLES: usize = 960;

/// Worst-case error of a stride's straight lines: half the fidelity test's
/// tolerance, for margin.
const STRIDE_ANGLE: f32 = 0.1 * std::f32::consts::PI / 180.0;
const STRIDE_ROOT: f32 = 0.0005;

/// Eased curves rest at every key, which over dense keys is a staircase.
const STRIDE_EASE: Interpolation = Interpolation::Linear;

/// One state's seeding: what to sample, over how long, and why.
#[derive(Clone, Copy, Debug)]
pub struct Seed {
    pub state: AnimationState,
    pub length: f32,
    pub clock: Clock,
    pub looping: bool,
    /// Headings in radians to bake a blend variant at. Empty for a plain clip.
    pub headings: &'static [f32],
    /// The clip's `note`, since saving from the editor strips TOML comments.
    pub note: &'static str,
}

/// Headings a forwards or backwards gait is blended across. A strafe has none:
/// a diagonal is running, not strafing.
///
/// Worst joint between headings, measured on a Mercenary: upright at three
/// headings 4.5°, at five 1.35°; crouched at three 1.5°. Hence the finer set
/// for upright gaits only.
const DIAGONALS: &[f32] = &[-FRAC_PI_4, 0.0, FRAC_PI_4];
const FINE_DIAGONALS: &[f32] = &[-FRAC_PI_4, -FRAC_PI_8, 0.0, FRAC_PI_8, FRAC_PI_4];
use std::f32::consts::{FRAC_PI_4, FRAC_PI_8};

const GAIT_NOTE: &str = "Sampled from `gait_pose` at a walk, one stride per clip. Only the upper \
    body and the hips: the legs are the gait's own, planted from the stride. Each arm is \
    timed by the foot on its side, so it is right at any speed.";

/// Every state that is a clip. One absent is still a Rust function. The
/// fidelity test walks this table.
pub const SEEDS: &[Seed] = &[
    Seed {
        state: AnimationState::Idle,
        length: IDLE_PERIOD,
        clock: Clock::Shared,
        looping: true,
        headings: &[],
        note: "Sampled from `idle_pose`. A settle the body sinks into and comes back \
               out of, with the chest leaning into it and the arms trailing a quarter \
               cycle behind.",
    },
    Seed {
        state: AnimationState::Crouch,
        length: 1.0,
        clock: Clock::Shared,
        looping: false,
        headings: &[],
        note: "Sampled from `crouch_posture`. Still, so one key says all of it — the \
               length is a formality.",
    },
    Seed {
        state: AnimationState::Airborne,
        length: 1.0,
        clock: Clock::Shared,
        looping: false,
        headings: &[],
        note: "Sampled from `airborne_pose`. Still, for now: a real jump wants keys \
               through the arc, which is what having a timeline is for.",
    },
    Seed {
        state: AnimationState::CrouchAirborne,
        length: 1.0,
        clock: Clock::Shared,
        looping: false,
        headings: &[],
        note: "Sampled from `crouch_posture` with `airborne_legs`. Still: folded like a \
               crouch, legs tucked like a jump. `duck_under` folds it under the short \
               hull afterwards, as it does for every crouched state.",
    },
    Seed {
        state: AnimationState::RunForward,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: FINE_DIAGONALS,
        note: GAIT_NOTE,
    },
    Seed {
        state: AnimationState::RunBackward,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: FINE_DIAGONALS,
        note: GAIT_NOTE,
    },
    Seed {
        state: AnimationState::StrafeLeft,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: &[],
        note: GAIT_NOTE,
    },
    Seed {
        state: AnimationState::StrafeRight,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: &[],
        note: GAIT_NOTE,
    },
    Seed {
        state: AnimationState::PushingWall,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: FINE_DIAGONALS,
        note: GAIT_NOTE,
    },
    Seed {
        state: AnimationState::CrouchWalk,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: DIAGONALS,
        note: GAIT_NOTE,
    },
    Seed {
        state: AnimationState::CrouchWalkBackward,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: DIAGONALS,
        note: GAIT_NOTE,
    },
    Seed {
        state: AnimationState::CrouchStrafeLeft,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: &[],
        note: GAIT_NOTE,
    },
    Seed {
        state: AnimationState::CrouchStrafeRight,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: &[],
        note: GAIT_NOTE,
    },
];

pub fn bake(seed: Seed) -> StateAnimation {
    if seed.headings.is_empty() {
        return StateAnimation::Single(bake_clip(seed, 0.0));
    }
    StateAnimation::Blend(
        seed.headings
            .iter()
            .map(|heading| Variant {
                heading: *heading,
                clip: bake_clip(seed, *heading),
            })
            .collect(),
    )
}

/// One clip of a seed, at one heading.
fn bake_clip(seed: Seed, heading: f32) -> Clip {
    let Seed { state, length, clock, looping, note, .. } = seed;

    if looping && clock == Clock::Stride {
        return bake_a_stride(seed, heading);
    }
    // A still pose gets one key; the sampler reads one key as a constant.
    let samples = if looping { KEYS_PER_CYCLE } else { 1 };
    let taken: Vec<(f32, Pose)> = (0..samples)
        .map(|step| {
            let at = length * step as f32 / samples as f32;
            let inputs = inputs_at(clock, at, length);
            (at, state.pose(&inputs))
        })
        .collect();

    // Rig order, not the `Pose`'s `HashMap` order, so re-baking is stable.
    let mut tracks = Vec::new();
    for bone in default_humanoid().bones().iter().map(|bone| bone.name) {
        if !taken
            .iter()
            .any(|(_, pose)| pose.posed_joints().any(|(name, _)| name == bone))
        {
            continue;
        }
        tracks.push(Track {
            combine: Combine::Replace,
            space: Space::Body,
            unit: Unit::Metres,
            enabled: true,
            timing: Default::default(),
            channel: Channel::Rotation {
                bone: bone.to_string(),
                keys: taken
                    .iter()
                    .map(|(at, pose)| {
                        Key::all(*at, euler_from_quat(pose.joint(bone)).to_array(), Interpolation::SmootherStep)
                    })
                    .collect(),
            },
        });
    }

    if taken
        .iter()
        .any(|(_, pose)| pose.root_offset.length_squared() > 0.0)
    {
        tracks.push(Track {
            combine: Combine::Replace,
            space: Space::Body,
            unit: Unit::HipHeights,
            enabled: true,
            timing: Default::default(),
            channel: Channel::Root {
                keys: taken
                    .iter()
                    .map(|(at, pose)| Key::all(*at, pose.root_offset.to_array(), Interpolation::SmootherStep))
                    .collect(),
            },
        });
    }

    Clip {
        name_key: format!("animation.states.{}", state.clip_name()),
        length,
        clock,
        looping,
        note: note.to_string(),
        tracks,
    }
}

/// Bake a gait at one heading; see [`STRIDE_SAMPLES`].
///
/// A bone on a side is keyed at that side's foot's [`footfall`] rather than on
/// the stride, which keeps one clip right at every speed.
///
/// [`footfall`]: crate::common::skeleton::gait::footfall
fn bake_a_stride(seed: Seed, heading: f32) -> Clip {
    use crate::common::rotation::quat_from_euler;
    use crate::common::skeleton::clip::Timing;
    use crate::common::skeleton::gait::{direction_of, footfall};
    let Seed { state, length, clock, looping, note, .. } = seed;

    let facing = direction_of(state);
    let (sin, cos) = heading.sin_cos();
    let travel = Vec2::new(facing.x * cos - facing.y * sin, facing.x * sin + facing.y * cos);
    let inputs = |at: f32| PoseInputs {
        travel,
        ..inputs_at(clock, at, length)
    };
    let stance = state.stance(&inputs(0.0)).unwrap_or(0.5);

    let taken: Vec<(f32, Pose)> = (0..STRIDE_SAMPLES)
        .map(|step| {
            let at = length * step as f32 / STRIDE_SAMPLES as f32;
            (at, state.pose(&inputs(at)))
        })
        .collect();

    // Where a sample lands on a track of this timing, in clip time.
    let time_on = |timing: Timing, at: f32| -> f32 {
        let whole = at.floor();
        match timing {
            Timing::Clip => at,
            Timing::LeftFoot => whole + footfall(at, stance),
            Timing::RightFoot => whole + footfall(at + 0.5, stance),
        }
    };

    let mut tracks = Vec::new();
    for bone in default_humanoid().bones().iter().map(|bone| bone.name) {
        if !taken
            .iter()
            .any(|(_, pose)| pose.posed_joints().any(|(name, _)| name == bone))
        {
            continue;
        }
        let timing = match bone {
            name if name.ends_with(".l") => Timing::LeftFoot,
            name if name.ends_with(".r") => Timing::RightFoot,
            _ => Timing::Clip,
        };

        // Track time order; the right foot's cycle starts half a stride in.
        let mut samples: Vec<(f32, Quat)> = taken
            .iter()
            .map(|(at, pose)| (time_on(timing, *at).rem_euclid(length), pose.joint(bone)))
            .collect();
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Close the loop so the seam segment is checked too.
        samples.push((samples[0].0 + length, samples[0].1));

        let times: Vec<f32> = samples.iter().map(|(at, _)| *at).collect();
        let truth: Vec<Quat> = samples.iter().map(|(_, turn)| *turn).collect();
        let angles = unwrapped(truth.iter().map(|turn| euler_from_quat(*turn)).collect());
        let kept = simplify(samples.len(), STRIDE_ANGLE, |from, to, at| {
            let t = (times[at] - times[from]) / (times[to] - times[from]);
            quat_from_euler(angles[from].lerp(angles[to], t)).angle_between(truth[at])
        });
        tracks.push(Track {
            combine: Combine::Replace,
            space: Space::Body,
            unit: Unit::Metres,
            enabled: true,
            timing,
            channel: Channel::Rotation {
                bone: bone.to_string(),
                keys: kept
                    .iter()
                    .map(|index| Key::all(times[*index], angles[*index].to_array(), STRIDE_EASE))
                    .collect(),
            },
        });
    }

    let mut roots: Vec<(f32, Vec3)> = taken.iter().map(|(at, pose)| (*at, pose.root_offset)).collect();
    if roots.iter().any(|(_, root)| root.length_squared() > 0.0) {
        roots.push((length, roots[0].1));
        let kept = simplify(roots.len(), STRIDE_ROOT, |from, to, at| {
            let t = (roots[at].0 - roots[from].0) / (roots[to].0 - roots[from].0);
            roots[from].1.lerp(roots[to].1, t).distance(roots[at].1)
        });
        tracks.push(Track {
            combine: Combine::Replace,
            space: Space::Body,
            unit: Unit::HipHeights,
            enabled: true,
            timing: Timing::Clip,
            channel: Channel::Root {
                keys: kept
                    .iter()
                    .map(|index| Key::all(roots[*index].0, roots[*index].1.to_array(), STRIDE_EASE))
                    .collect(),
            },
        });
    }

    Clip {
        name_key: format!("animation.states.{}", state.clip_name()),
        length,
        clock,
        looping,
        note: note.to_string(),
        tracks,
    }
}

/// Ramer–Douglas–Peucker over `count` samples: `apart(from, to, at)` is how far
/// sample `at` is from the line `from`–`to`. The last sample is the loop's
/// return to the first and is never itself a key.
///
/// Splits at the worst sample; splitting at the first miss kept ~200 keys a
/// track.
fn simplify(
    count: usize,
    tolerance: f32,
    apart: impl Fn(usize, usize, usize) -> f32,
) -> Vec<usize> {
    let mut kept = vec![0];
    let mut spans = vec![(0, count - 1)];
    while let Some((from, to)) = spans.pop() {
        let worst = (from + 1..to)
            .map(|at| (at, apart(from, to, at)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((at, by)) = worst {
            if by > tolerance {
                kept.push(at);
                spans.push((at, to));
                spans.push((from, at));
            }
        }
    }
    kept.sort_unstable();
    kept.dedup();
    kept
}

/// Euler angles made continuous, so a line between two goes the short way.
fn unwrapped(mut angles: Vec<Vec3>) -> Vec<Vec3> {
    use std::f32::consts::{PI, TAU};
    for index in 1..angles.len() {
        let before = angles[index - 1];
        let now = &mut angles[index];
        for axis in 0..3 {
            while now[axis] - before[axis] > PI {
                now[axis] -= TAU;
            }
            while now[axis] - before[axis] < -PI {
                now[axis] += TAU;
            }
        }
    }
    angles
}

/// What a body would tell its state at `at` on a clip of this clock. Speed and
/// travel are zero, so a gait walks the way `direction_of` says. Shared with
/// the fidelity test so the two sample alike.
pub fn inputs_at(clock: Clock, at: f32, length: f32) -> PoseInputs {
    match clock {
        Clock::Stride => PoseInputs {
            stride: (at / super::clip::strides_spanned(length)).rem_euclid(1.0),
            ..default()
        },
        Clock::Seconds | Clock::Shared => PoseInputs {
            seconds: at,
            ..default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two rows for one state write one path and the second silently wins.
    #[test]
    fn each_state_is_seeded_at_most_once() {
        let all: Vec<&Seed> = SEEDS.iter().collect();
        for (index, seed) in all.iter().enumerate() {
            assert!(
                !all[..index].iter().any(|earlier| earlier.state == seed.state),
                "{:?} is seeded twice",
                seed.state,
            );
        }
    }

    /// A seeded clip must not show in the Problems panel.
    #[test]
    fn every_seed_bakes_a_clip_with_no_problems() {
        let rig = default_humanoid();
        let known = |name: &str| rig.bones().iter().any(|bone| bone.name == name);
        for seed in SEEDS {
            let clip = bake(*seed);
            let problems = clip.problems(known);
            assert!(
                problems.is_empty(),
                "{:?} bakes with problems: {problems:?}",
                seed.state,
            );
        }
    }
}
