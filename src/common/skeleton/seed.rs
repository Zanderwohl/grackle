//! Where the body clips came from: the Rust pose functions, sampled.
//!
//! The migration's first version had to be *faithful*, and nobody can hand-place
//! a cosine. So this samples [`AnimationState::pose`] and writes down what it
//! found. After that **the file is the source of truth and this is only how it
//! was seeded** — the same arrangement `new_map_template` has with the
//! blueprint a new map starts from.
//!
//! It lives in the library rather than inside `bake_body_clips` because two
//! things need it and only one of them writes a file:
//!
//! - the `bake_body_clips` binary, which seeds a pack that has no clips yet;
//! - [`clip_file`]'s fidelity test, which bakes a clip *in memory* and checks it
//!   still moves the way the function does.
//!
//! That split is what makes the clips editable. While the test read the shipped
//! files, every hand edit to an idle failed it — so the safety net and the
//! point of the migration were in direct opposition, and the net would have
//! been the one to go. Checking the generator instead keeps the guarantee that
//! matters (seeding a pack is faithful) without claiming the shipped files have
//! never been touched since, which is exactly what anybody is meant to do to
//! them.
//!
//! # What is lost in the sampling
//!
//! The functions carry named constants with doc comments arguing the number —
//! `IDLE_BOB` is `0.022` "in hip-heights" and says why. A clip has numbers and
//! no argument, so each seed carries a [`Seed::note`] saying where it came
//! from; the reasoning stays in the Rust this reads, which is why the functions
//! are not deleted.
//!
//! [`clip_file`]: crate::common::clip_file

use bevy::prelude::*;

use crate::common::rotation::euler_from_quat;
use crate::common::skeleton::clip::{
    Channel, Clip, Clock, Combine, Key, Space, StateAnimation, Track, Unit, Variant,
};
use crate::common::skeleton::state::{AnimationState, PoseInputs, IDLE_PERIOD};
use crate::common::skeleton::{default_humanoid, Interpolation, Pose};

/// How many keys to lay across a cycle.
///
/// Sixteen, which is enough that a cosine sampled at these points and eased
/// between them is within a fifth of a degree of the real thing everywhere —
/// `every_baked_clip_still_matches_its_function` is what actually holds that,
/// and this is the number to raise if it ever stops passing.
const KEYS_PER_CYCLE: usize = 16;

/// Smooth at both ends in the second derivative too, so a loop seam does not
/// show as a kink. An idle is a cosine and a cosine has no corners.
const EASE: Interpolation = Interpolation::SmootherStep;

/// How finely a stride is looked at before keys are chosen from it.
///
/// **A gait is not a cosine.** An arm's swing follows its foot, and a foot's
/// path has corners where it lands and lifts — the swing reverses in one
/// sample. Evenly spaced keys cannot land on a corner, so adding more of them
/// moves the error rather than removing it (sixty-four was *worse* than
/// thirty-two). So a stride is sampled this finely and each track keeps only
/// the keys it needs to stay within [`STRIDE_ANGLE`] or [`STRIDE_ROOT`] of the
/// function — which puts keys on the corners because that is where the error
/// is, and leaves a straight stretch with two.
const STRIDE_SAMPLES: usize = 960;

/// How far a stride's straight lines may stray from the function, at worst:
/// half the fidelity test's tolerance, so the test is not passing by a hair.
const STRIDE_ANGLE: f32 = 0.1 * std::f32::consts::PI / 180.0;
const STRIDE_ROOT: f32 = 0.0005;

/// Straight lines between keys. Every eased curve starts and ends a segment
/// at rest, which sampled densely is a staircase where the function was a
/// slope.
const STRIDE_EASE: Interpolation = Interpolation::Linear;

/// One state's seeding: what to sample, over how long, and why.
#[derive(Clone, Copy, Debug)]
pub struct Seed {
    pub state: AnimationState,
    pub length: f32,
    /// What drives it: the shared clock for a settle, the stride for anything
    /// that takes steps.
    pub clock: Clock,
    pub looping: bool,
    /// Headings to bake a variant at, in radians, making the state a blend.
    /// Empty for a plain clip.
    pub headings: &'static [f32],
    /// Prose for the clip's own `note`, since the file keeps no comments —
    /// saving one from the editor is `toml::to_string_pretty`, which strips
    /// them.
    pub note: &'static str,
}

/// The headings a forwards or backwards gait is blended across: straight, and
/// the two diagonals a keyboard can ask for. A strafe has none, because
/// pressing a diagonal is running, not strafing.
///
/// Upright gaits get the halfway headings too. Measured on a Mercenary, the
/// worst joint between headings: an upright gait at three headings is 4.5°
/// out, at five 1.35°; a crouched one is 1.5° at three. A crouch's swing is
/// small, so its in-betweens are nearer the average of their neighbours.
const DIAGONALS: &[f32] = &[-FRAC_PI_4, 0.0, FRAC_PI_4];
const FINE_DIAGONALS: &[f32] = &[-FRAC_PI_4, -FRAC_PI_8, 0.0, FRAC_PI_8, FRAC_PI_4];
use std::f32::consts::{FRAC_PI_4, FRAC_PI_8};

const GAIT_NOTE: &str = "Sampled from `gait_pose` at a walk, one stride per clip. Only the upper \
    body and the hips: the legs are the gait's own, planted from the stride. Each arm is \
    timed by the foot on its side, so it is right at any speed.";

/// Every state that has been migrated, and nothing else.
///
/// A state absent from here is one that is still a Rust function, which is how
/// the migration is thirteen independent decisions rather than one. Adding a
/// row is how the fourteenth is made — and the fidelity test picks it up by
/// walking this table, so a new row is checked the moment it exists.
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

/// Sample a state's own pose function and write down what it did: a plain
/// clip, or a variant per heading.
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
    // Still poses get one key, not sixteen identical ones: a track that says
    // the same thing sixteen times is one nobody can read, and the sampler
    // treats a single key as a constant anyway.
    let samples = if looping { KEYS_PER_CYCLE } else { 1 };
    let ease = EASE;
    let taken: Vec<(f32, Pose)> = (0..samples)
        .map(|step| {
            let at = length * step as f32 / samples as f32;
            let inputs = inputs_at(clock, at, length);
            (at, state.pose(&inputs))
        })
        .collect();

    // Every joint any sample had an opinion about, in the rig's own order so
    // the file is stable between runs — a `Pose`'s joints are a `HashMap`, and
    // writing them in its order would make every re-bake a diff.
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
                    // All three, because this is a faithful recording of what a
                    // function did — it had an opinion about every axis, so
                    // the clip says so. A key somebody authors afterwards may
                    // free whichever axes it has nothing to say about.
                    .map(|(at, pose)| {
                        Key::all(*at, euler_from_quat(pose.joint(bone)).to_array(), ease)
                    })
                    .collect(),
            },
        });
    }

    // And the root, if the state moves it at all.
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
                    .map(|(at, pose)| Key::all(*at, pose.root_offset.to_array(), ease))
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

/// Bake a gait at one heading: sample a stride finely, then keep per track
/// only the keys straight lines between need. See [`STRIDE_SAMPLES`].
///
/// **A bone on a side is timed by the foot on that side** — an arm swings
/// against its own leg — and its keys are laid at that foot's [`footfall`]
/// rather than at the stride, which is what makes one clip right at every
/// speed. Everything else is on the stride. The baking speed is a walk's, and
/// it only matters for where the stance boundary falls while sampling.
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

        // In the track's own time order, which for a foot is not the
        // stride's: the right foot's cycle starts half a stride in.
        let mut samples: Vec<(f32, Quat)> = taken
            .iter()
            .map(|(at, pose)| (time_on(timing, *at).rem_euclid(length), pose.joint(bone)))
            .collect();
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        // And round to the start again, so the segment across the seam is
        // checked like any other.
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

/// Which samples to keep as keys, of `count`, so that a straight line between
/// each kept pair stays close to every sample between them.
///
/// Ramer–Douglas–Peucker, with the distance left to the caller:
/// `apart(from, to, at)` is how far sample `at` is from the line between
/// `from` and `to`. The last sample is the loop coming round to the first, so
/// it bounds the last segment and is never itself a key.
///
/// **Split at the worst sample, not the first one that misses.** The first
/// miss on a long curved span is just past its start, so cutting there chops
/// every span into slivers — that version kept two hundred keys a track per
/// stride, which is a file nobody could edit.
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

/// Euler angles made continuous, so a straight line between two of them is
/// the short way round rather than the long one.
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

/// What a body would tell its state at `at` on a clip of this clock.
///
/// A stride clip's own time is strides — `Clip::time_from` reads it as
/// `stride * length` — so the stride is the time over the length. Speed and
/// travel are left at nothing, which is a body walking straight the way its
/// state says: the coarse direction `direction_of` gives a display standing on
/// the spot. Shared by the baking and by the test that checks it, so the two
/// cannot sample a gait differently.
pub fn inputs_at(clock: Clock, at: f32, length: f32) -> PoseInputs {
    match clock {
        Clock::Stride => PoseInputs {
            stride: (at / length.max(1.0)).rem_euclid(1.0),
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

    /// No state is seeded twice. Two rows for one state is two files written
    /// to one path, and the second one silently wins.
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

    /// A seed produces a clip with nothing to complain about — the same
    /// question the Problems panel asks. A generator that seeded a pack with a
    /// clip the editor flags would be one nobody could start from.
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
