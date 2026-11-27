//! Where the body clips came from: [`AnimationState::pose`], sampled, and for the raptor
//! [`super::raptor_seed`]'s poses. Once seeded, the clip file is the source of truth and this
//! is only the generator, as `new_map_template` is for the new-map blueprint.
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
use crate::common::skeleton::state::{AnimationState, PoseInputs, AIRBORNE_PERIOD, IDLE_PERIOD};
use crate::common::skeleton::humanoid_seed::{SHADOWBOX_LENGTH, TAUNT_SHADOWBOX, TAUNT_WAVE, WAVE_LENGTH};
use crate::common::skeleton::raptor_seed::{IDLE_LENGTH, LUNGE_LENGTH, TAUNT_LENGTH, TAUNT_LUNGE, TAUNT_WAG};
use crate::common::skeleton::rig::{Skeleton, HUMANOID};
use crate::common::skeleton::{default_humanoid, Interpolation, Pose};

/// How finely a loop is sampled before keys are chosen from it.
///
/// A foot-timed track bends where its foot lands and lifts, which evenly spaced keys cannot
/// hit. So each track keeps only the samples a spline through them needs to stay within
/// [`STRIDE_ANGLE`] or [`STRIDE_ROOT`].
const STRIDE_SAMPLES: usize = 960;
/// A clip played by name is sampled more finely: a taunt's fast moves turn a limb several degrees
/// between strides' worth of samples, and the spline can stray between them.
const NAMED_SAMPLES: usize = 3840;

/// Worst-case error of a stride's keys: half the fidelity test's tolerance, for margin.
const STRIDE_ANGLE: f32 = 0.1 * std::f32::consts::PI / 180.0;
const STRIDE_ROOT: f32 = 0.0005;

/// A spline passes through the keys at one speed, so a smooth motion takes a few keys and a
/// hand edit to one stays smooth. Eased curves rest at every key.
const STRIDE_EASE: Interpolation = Interpolation::Spline;

/// Keys a track starts from before [`fit`] adds where the spline misses.
const FIRST_KEYS: usize = 4;

/// One state's seeding: what to sample, over how long, and why.
#[derive(Clone, Copy, Debug)]
pub struct Seed {
    /// The rig the clip is for, as [`Skeleton::kind`] names it.
    pub rig: &'static str,
    /// A clip played by name, as a taunt is, rather than a state's: its file name, and what is
    /// sampled. `state` is then only what the clip is played over.
    pub name: Option<&'static str>,
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

const GAIT_NOTE: &str = "Sampled from `gait_pose` at a run, one stride per clip. Only the upper \
    body and the hips: the legs are the gait's own, planted from the stride. Each arm is \
    timed by the foot on its side, so it lands with its foot at any speed.";

/// The speed a gait is sampled at, in leg-lengths per second: a run, since a body in a match
/// sprints. A clip has one speed; a walk plays the run's upper body.
pub const SEED_SPEED: f32 = super::gait::RUN_SPEED;

/// Every state that is a clip. One absent is still a Rust function. The
/// fidelity test walks this table.
pub const SEEDS: &[Seed] = &[
    Seed {
        rig: HUMANOID,
        name: None,
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
        rig: HUMANOID,
        name: None,
        state: AnimationState::Crouch,
        length: 1.0,
        clock: Clock::Shared,
        looping: false,
        headings: &[],
        note: "Sampled from `crouch_posture`. Still, so one key says all of it — the \
               length is a formality.",
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::Airborne,
        length: AIRBORNE_PERIOD,
        clock: Clock::Shared,
        looping: true,
        headings: &[],
        note: "Sampled from `airborne_pose`: a slow pedal, each foot drawing a loop as the \
               knee straightens behind the hip, the legs half a turn apart and the arms \
               countering them.",
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::CrouchAirborne,
        length: AIRBORNE_PERIOD,
        clock: Clock::Shared,
        looping: true,
        headings: &[],
        note: "Sampled from `crouch_posture` with `airborne_legs`: folded like a crouch, \
               pedaling tucked. `duck_under` folds it under the short hull afterward and \
               `tuck_into_the_box` lifts the feet above its bottom.",
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::RunForward,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: FINE_DIAGONALS,
        note: GAIT_NOTE,
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::RunBackward,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: FINE_DIAGONALS,
        note: GAIT_NOTE,
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::StrafeLeft,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: &[],
        note: GAIT_NOTE,
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::StrafeRight,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: &[],
        note: GAIT_NOTE,
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::PushingWall,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: FINE_DIAGONALS,
        note: GAIT_NOTE,
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::CrouchWalk,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: DIAGONALS,
        note: GAIT_NOTE,
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::CrouchWalkBackward,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: DIAGONALS,
        note: GAIT_NOTE,
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::CrouchStrafeLeft,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: &[],
        note: GAIT_NOTE,
    },
    Seed {
        rig: HUMANOID,
        name: None,
        state: AnimationState::CrouchStrafeRight,
        length: 1.0,
        clock: Clock::Stride,
        looping: true,
        headings: &[],
        note: GAIT_NOTE,
    },
    Seed {
        rig: HUMANOID,
        name: Some(TAUNT_SHADOWBOX),
        state: AnimationState::Idle,
        length: SHADOWBOX_LENGTH,
        clock: Clock::Seconds,
        looping: false,
        headings: &[],
        note: "Sampled from `humanoid_seed::shadowbox` on the reference build: a hop into a boxer's \
               stance, left foot leading and the body side-on, the hands coming up to guard; a \
               left, a right and two quick lefts, the hips, chest and shoulder driving in behind \
               each and the rear heel pivoting on the right; and a hop back out. The feet are solved onto \
               the floor and the fists onto a point ahead of the chin, so retune it there.",
    },
    Seed {
        rig: HUMANOID,
        name: Some(TAUNT_WAVE),
        state: AnimationState::Idle,
        length: WAVE_LENGTH,
        clock: Clock::Seconds,
        looping: false,
        headings: &[],
        note: "Sampled from `humanoid_seed::wave` on the reference build: the right hand swings up \
               the side to beside the head, overshooting a little, with the body leaning away from \
               it; three sweeps out and in, the hand trailing the forearm, the hips sliding under a \
               chest that sways after the hand and the head tilting after that; then the arm comes \
               down and the body settles. The feet are solved onto the floor and the wrist onto \
               points from between the shoulders, so retune it there.",
    },
    raptor(AnimationState::Idle, IDLE_LENGTH, Clock::Shared, RAPTOR_IDLE_NOTE),
    raptor(AnimationState::Crouch, IDLE_LENGTH, Clock::Shared, RAPTOR_CROUCH_NOTE),
    raptor(AnimationState::Airborne, AIRBORNE_PERIOD, Clock::Shared, RAPTOR_AIRBORNE_NOTE),
    raptor(AnimationState::CrouchAirborne, AIRBORNE_PERIOD, Clock::Shared, RAPTOR_AIRBORNE_NOTE),
    raptor(AnimationState::RunForward, 1.0, Clock::Stride, RAPTOR_GAIT_NOTE),
    raptor(AnimationState::RunBackward, 1.0, Clock::Stride, RAPTOR_GAIT_NOTE),
    raptor(AnimationState::PushingWall, 1.0, Clock::Stride, RAPTOR_GAIT_NOTE),
    raptor(AnimationState::CrouchWalk, 1.0, Clock::Stride, RAPTOR_GAIT_NOTE),
    raptor(AnimationState::CrouchWalkBackward, 1.0, Clock::Stride, RAPTOR_GAIT_NOTE),
    Seed {
        rig: RAPTOR,
        name: Some(TAUNT_WAG),
        state: AnimationState::Idle,
        length: TAUNT_LENGTH,
        clock: Clock::Seconds,
        looping: false,
        headings: &[],
        note: "Sampled from `raptor_seed::taunt_wag`: the raptor's wave. The tail curls up to \
               point at the sky and waggles four times, the head lifts, and all of it settles \
               back. As long as the wave, since a taunt's length is read off its humanoid clip.",
    },
    Seed {
        rig: RAPTOR,
        name: Some(TAUNT_LUNGE),
        state: AnimationState::Idle,
        length: LUNGE_LENGTH,
        clock: Clock::Seconds,
        looping: false,
        headings: &[],
        note: "Sampled from `raptor_seed::taunt_lunge`: the raptor sinks back into a crouch with \
               the neck coiled and the tail lashing, draws back further, and lunges forward over \
               its planted feet with the neck thrown out and the claws reaching, snapping twice \
               and shaking its head before it settles. The feet are solved onto the floor on the \
               reference rig. As long as the humanoid's shadow-box, since a taunt's length is read \
               off its humanoid clip.",
    },
];

/// Humanoid clips generated key by key rather than sampled from a pose, by file name.
pub const WRITTEN: &[(&str, fn() -> Clip)] = &[(super::pistol_twirl::TAUNT_TWIRL, super::pistol_twirl::clip)];

/// The raptor's clips written key by key, from the raptor rig `packs` build (the last pack wins).
pub fn raptor_written(packs: &[std::path::PathBuf]) -> Vec<(&'static str, Clip)> {
    let builds = crate::common::class_file::load_builds(&crate::common::assets::Assets::default(), packs);
    let skeleton = builds.rig_for(crate::common::class::Class::Raptor);
    super::raptor_seed::FLINCHES.iter().map(|name| (*name, super::raptor_seed::flinch(name, &skeleton))).collect()
}

const RAPTOR: &str = "raptor";

/// A raptor state's seed, looping, from [`super::raptor_seed::pose`].
const fn raptor(state: AnimationState, length: f32, clock: Clock, note: &'static str) -> Seed {
    Seed { rig: RAPTOR, name: None, state, length, clock, looping: true, headings: &[], note }
}

const RAPTOR_IDLE_NOTE: &str = "Sampled from `raptor_seed::idle`: a slow breath the body tips \
    into, the neck taking the tip back, and the tail swaying twice in it, each bone a little \
    behind the one before.";
const RAPTOR_CROUCH_NOTE: &str = "Sampled from `raptor_seed::idle` and `crouched`: a stalk, \
    the front raised and the neck reaching forward and down with the head kept level. \
    `duck_under` drops the hips until the crown is at the rig's `crouched_crown`. Breathing as \
    standing does.";
const RAPTOR_AIRBORNE_NOTE: &str = "Sampled from `raptor_seed::airborne`: each ankle pedals a \
    loop half a turn behind the other, solved on the reference rig, the toes hanging and the tail \
    lifted and swinging against the legs. Crouched, the pedal is tucked and `tuck_into_the_box` \
    lifts what still hangs below the short box.";
const RAPTOR_GAIT_NOTE: &str = "Sampled from `raptor_seed::gait` at a run, one stride per clip. \
    Only the body and tail: the legs are the gait's own, planted from the stride. The hips rise, \
    turn, drop and sway as the humanoid's do, the spine turns against them, the neck holds the \
    head steady, and the tail swings against the legs, whipping down its length.";

/// The rig a seed is sampled on: the humanoid's reference build, or the first shipped class
/// built on that rig.
pub fn rig_of(seed: &Seed) -> Skeleton {
    if seed.rig == HUMANOID {
        return default_humanoid().clone();
    }
    use strum::IntoEnumIterator;
    let builds = crate::common::class::ClassBuilds::shipped();
    crate::common::class::Class::iter()
        .map(|class| builds.rig_for(class))
        .find(|rig| rig.kind() == seed.rig)
        .unwrap_or_else(|| panic!("no shipped class is built on the {} rig", seed.rig))
}

/// The pose a seed's clip is sampled from.
pub fn sample(seed: &Seed, skeleton: &Skeleton, inputs: &PoseInputs) -> Pose {
    match (seed.rig, seed.name) {
        (HUMANOID, Some(name)) => super::humanoid_seed::named(name, inputs),
        (_, Some(name)) => super::raptor_seed::named(name, inputs, skeleton),
        (HUMANOID, None) => seed.state.pose(inputs),
        (_, None) => super::raptor_seed::pose(seed.state, inputs, skeleton),
    }
}

impl Seed {
    /// The clip's file name, without its extension.
    pub fn file_name(&self) -> &'static str {
        self.name.unwrap_or_else(|| self.state.clip_name())
    }
}

pub fn bake(seed: Seed) -> StateAnimation {
    let skeleton = rig_of(&seed);
    if seed.headings.is_empty() {
        return StateAnimation::Single(bake_clip(seed, &skeleton, 0.0));
    }
    StateAnimation::Blend(
        seed.headings
            .iter()
            .map(|heading| Variant {
                heading: *heading,
                clip: bake_clip(seed, &skeleton, *heading),
            })
            .collect(),
    )
}

/// One clip of a seed, at one heading.
fn bake_clip(seed: Seed, skeleton: &Skeleton, heading: f32) -> Clip {
    let Seed { length, clock, looping, note, .. } = seed;

    // A state's one-shot is a held pose; a clip played by name plays through.
    if looping || seed.name.is_some() {
        return bake_a_loop(seed, skeleton, heading);
    }
    // A still pose gets one key; the sampler reads one key as a constant.
    let taken = [(0.0, sample(&seed, skeleton, &inputs_at(clock, 0.0, length)))];

    // Rig order, not the `Pose`'s `HashMap` order, so re-baking is stable.
    let mut tracks = Vec::new();
    for bone in skeleton.bones().iter().map(|bone| bone.name) {
        if !taken
            .iter()
            .any(|(_, pose)| pose.posed_joints().any(|(name, _)| name == bone))
        {
            continue;
        }
        tracks.push(Track {
            combine: Combine::Replace,
            space: Space::Body,
            unit: Unit::Meters,
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
        name_key: name_key(&seed),
        length,
        clock,
        looping,
        blend_in: None,
        blend_out: None,
        note: note.to_string(),
        tracks,
    }
}

/// A state's clip is named for its state; one played by name has no name of its own.
fn name_key(seed: &Seed) -> String {
    match seed.name {
        Some(_) => String::new(),
        None => format!("animation.states.{}", seed.state.clip_name()),
    }
}

/// Bake a loop, a gait at one heading, or a clip played by name through; see [`STRIDE_SAMPLES`].
///
/// On a stride, a bone on a side is keyed at that side's foot's [`footfall`] rather than on
/// the stride, which keeps it landing with its foot at every speed.
///
/// [`footfall`]: crate::common::skeleton::gait::footfall
fn bake_a_loop(seed: Seed, skeleton: &Skeleton, heading: f32) -> Clip {
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

    // A one-shot's last sample is its end, not the loop's return to its start.
    let samples = if seed.name.is_some() { NAMED_SAMPLES } else { STRIDE_SAMPLES };
    let last = if looping { samples - 1 } else { samples };
    let taken: Vec<(f32, Pose)> = (0..=last)
        .map(|step| {
            let at = length * step as f32 / samples as f32;
            (at, sample(&seed, skeleton, &inputs(at)))
        })
        .collect();

    // Where a sample lands on a track of this timing, in clip time.
    let time_on = |timing: Timing, at: f32| -> f32 {
        let whole = at.floor();
        match timing {
            Timing::Clip => at,
            Timing::LeftFoot => whole + footfall(at, stance),
            Timing::RightFoot => whole + footfall(at + 0.5, stance),
            // Only the first two feet are keyed by side.
            Timing::Foot(_) => at,
        }
    };

    let mut tracks = Vec::new();
    for bone in skeleton.bones().iter().map(|bone| bone.name) {
        if !taken
            .iter()
            .any(|(_, pose)| pose.posed_joints().any(|(name, _)| name == bone))
        {
            continue;
        }
        let timing = match bone {
            name if clock == Clock::Stride && name.ends_with(".l") => Timing::LeftFoot,
            name if clock == Clock::Stride && name.ends_with(".r") => Timing::RightFoot,
            _ => Timing::Clip,
        };

        // Track time order; the right foot's cycle starts half a stride in.
        let mut samples: Vec<(f32, Quat)> = taken
            .iter()
            .map(|(at, pose)| {
                let at = time_on(timing, *at);
                (if looping { at.rem_euclid(length) } else { at }, pose.joint(bone))
            })
            .collect();
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Close the loop so the seam segment is checked too.
        if looping {
            samples.push((samples[0].0 + length, samples[0].1));
        }

        let times: Vec<f32> = samples.iter().map(|(at, _)| *at).collect();
        let truth: Vec<Quat> = samples.iter().map(|(_, turn)| *turn).collect();
        let angles = unwrapped(truth.iter().map(|turn| euler_from_quat(*turn)).collect());
        let kept = fit(&times, &angles, length, looping, STRIDE_ANGLE, |sampled, at| {
            quat_from_euler(sampled).angle_between(truth[at])
        });
        tracks.push(Track {
            combine: Combine::Replace,
            space: Space::Body,
            unit: Unit::Meters,
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
        if looping {
            roots.push((length, roots[0].1));
        }
        let times: Vec<f32> = roots.iter().map(|(at, _)| *at).collect();
        let offsets: Vec<Vec3> = roots.iter().map(|(_, offset)| *offset).collect();
        let kept = fit(&times, &offsets, length, looping, STRIDE_ROOT, |sampled, at| sampled.distance(offsets[at]));
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
        name_key: name_key(&seed),
        length,
        clock,
        looping,
        blend_in: None,
        blend_out: None,
        note: note.to_string(),
        tracks,
    }
}

/// Which of a track's samples to key, so a spline through them, played as a match plays it,
/// stays within `tolerance` by `apart(sampled, at)` of sample `at`. `times` are in order. On a
/// loop the last sample is the return to the first and is never itself a key; on a one-shot it
/// is the end, and always one.
///
/// Starts from [`FIRST_KEYS`] even keys and keys the worst sample of every segment that
/// misses until none does. Each new key bends the spline on the segments beside it too,
/// which is why every sample is checked again each round.
pub(super) fn fit(times: &[f32], values: &[Vec3], length: f32, looping: bool, tolerance: f32, apart: impl Fn(Vec3, usize) -> f32) -> Vec<usize> {
    use super::play::sample_key;
    let count = times.len() - 1;
    let mut kept: Vec<usize> = (0..FIRST_KEYS.min(count)).map(|key| key * count / FIRST_KEYS.min(count)).collect();
    if !looping {
        kept.push(count);
    }
    kept.dedup();
    loop {
        let keys: Vec<Key> = kept.iter().map(|index| Key::all(times[*index], values[*index].to_array(), STRIDE_EASE)).collect();
        let mut added = Vec::new();
        for (segment, from) in kept.iter().enumerate() {
            let to = kept.get(segment + 1).copied().unwrap_or(count);
            let worst = (from + 1..to)
                .map(|at| {
                    let sampled = match sample_key(&keys, times[at], length, looping) {
                        Some(axes) => Vec3::from_array(axes.map(|axis| axis.value().unwrap_or(0.0))),
                        None => Vec3::ZERO,
                    };
                    (at, apart(sampled, at))
                })
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((at, _)) = worst.filter(|(_, by)| *by > tolerance) {
                added.push(at);
            }
        }
        if added.is_empty() {
            return kept;
        }
        kept.extend(added);
        kept.sort_unstable();
    }
}

/// A value through `keys` of (seconds, value) at `seconds`, held before the first and after the
/// last. Monotone cubic (Fritsch–Butland): the speed carries through each key without overshooting
/// it, so a key that is a turning point is where the motion turns, and a run of equal keys holds
/// still. The first and last keys are left at rest, as a one-shot starts and ends.
pub(super) fn keyed(keys: &[(f32, f32)], seconds: f32) -> f32 {
    let last = keys.len() - 1;
    if seconds <= keys[0].0 {
        return keys[0].1;
    }
    if seconds >= keys[last].0 {
        return keys[last].1;
    }
    let slope = |at: usize| (keys[at + 1].1 - keys[at].1) / (keys[at + 1].0 - keys[at].0);
    let tangent = |at: usize| {
        if at == 0 || at == last {
            return 0.0;
        }
        let (before, after) = (slope(at - 1), slope(at));
        if before * after <= 0.0 {
            return 0.0;
        }
        let (gap_before, gap_after) = (keys[at].0 - keys[at - 1].0, keys[at + 1].0 - keys[at].0);
        3.0 * (gap_before + gap_after)
            / ((2.0 * gap_after + gap_before) / before + (gap_after + 2.0 * gap_before) / after)
    };
    let at = keys.windows(2).position(|pair| seconds < pair[1].0).unwrap_or(last - 1);
    let ((from, start), (to, end)) = (keys[at], keys[at + 1]);
    let gap = to - from;
    let s = (seconds - from) / gap;
    let (s2, s3) = (s * s, s * s * s);
    (2.0 * s3 - 3.0 * s2 + 1.0) * start
        + (s3 - 2.0 * s2 + s) * tangent(at) * gap
        + (3.0 * s2 - 2.0 * s3) * end
        + (s3 - s2) * tangent(at + 1) * gap
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

/// What a body would tell its state at `at` on a clip of this clock. A stride is at
/// [`SEED_SPEED`]; travel is zero, so a gait goes the way `direction_of` says. Shared with
/// the fidelity test so the two sample alike.
pub fn inputs_at(clock: Clock, at: f32, length: f32) -> PoseInputs {
    match clock {
        Clock::Stride => PoseInputs {
            stride: (at / super::clip::strides_spanned(length)).rem_euclid(1.0),
            speed: SEED_SPEED,
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
                !all[..index].iter().any(|earlier| earlier.file_name() == seed.file_name() && earlier.rig == seed.rig),
                "{:?} is seeded twice",
                seed.state,
            );
        }
    }

    /// A seeded clip must not show in the Problems panel.
    /// Breaks if a shipped raptor flinch differs from what `raptor_seed::flinch` writes now.
    #[test]
    fn every_shipped_raptor_clip_is_what_is_written() {
        let packs = [std::path::PathBuf::from("assets/default")];
        for (name, mut fresh) in raptor_written(&packs) {
            let shipped = crate::common::clip_file::load_named_clip(&Default::default(), &packs, &format!("raptor/{name}"))
                .unwrap_or_else(|| panic!("no raptor/{name}; run `cargo run --bin bake_body_clips raptor/{name}`"));
            fresh.normalize();
            assert!(shipped == fresh, "raptor/{name} differs from a fresh bake; run `cargo run --bin bake_body_clips raptor/{name}`");
        }
    }

    #[test]
    fn every_seed_bakes_a_clip_with_no_problems() {
        for seed in SEEDS {
            let rig = rig_of(seed);
            let known = |name: &str| rig.bones().iter().any(|bone| bone.name == name);
            let clip = bake(*seed);
            let problems = clip.problems(known);
            assert!(
                problems.is_empty(),
                "{} {:?} bakes with problems: {problems:?}",
                seed.rig,
                seed.state,
            );
        }
    }
}
