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
use crate::common::skeleton::clip::{Channel, Clip, Clock, Combine, Key, Space, Track, Unit};
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

/// One state's seeding: what to sample, over how long, and why.
#[derive(Clone, Copy, Debug)]
pub struct Seed {
    pub state: AnimationState,
    pub length: f32,
    pub looping: bool,
    /// Prose for the clip's own `note`, since the file keeps no comments —
    /// saving one from the editor is `toml::to_string_pretty`, which strips
    /// them.
    pub note: &'static str,
}

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
        looping: true,
        note: "Sampled from `idle_pose`. A settle the body sinks into and comes back \
               out of, with the chest leaning into it and the arms trailing a quarter \
               cycle behind.",
    },
    Seed {
        state: AnimationState::Crouch,
        length: 1.0,
        looping: false,
        note: "Sampled from `crouch_posture`. Still, so one key says all of it — the \
               length is a formality.",
    },
    Seed {
        state: AnimationState::Airborne,
        length: 1.0,
        looping: false,
        note: "Sampled from `airborne_pose`. Still, for now: a real jump wants keys \
               through the arc, which is what having a timeline is for.",
    },
];

/// Sample a state's own pose function across a cycle and write down what it
/// did.
pub fn bake(seed: Seed) -> Clip {
    let Seed { state, length, looping, note } = seed;

    // Still poses get one key, not sixteen identical ones: a track that says
    // the same thing sixteen times is one nobody can read, and the sampler
    // treats a single key as a constant anyway.
    let samples = if looping { KEYS_PER_CYCLE } else { 1 };
    let taken: Vec<(f32, Pose)> = (0..samples)
        .map(|step| {
            let at = length * step as f32 / samples as f32;
            let inputs = PoseInputs {
                seconds: at,
                ..default()
            };
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
            channel: Channel::Rotation {
                bone: bone.to_string(),
                keys: taken
                    .iter()
                    // All three, because this is a faithful recording of what a
                    // function did — it had an opinion about every axis, so
                    // the clip says so. A key somebody authors afterwards may
                    // free whichever axes it has nothing to say about.
                    .map(|(at, pose)| {
                        Key::all(*at, euler_from_quat(pose.joint(bone)).to_array(), EASE)
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
            channel: Channel::Root {
                keys: taken
                    .iter()
                    .map(|(at, pose)| Key::all(*at, pose.root_offset.to_array(), EASE))
                    .collect(),
            },
        });
    }

    Clip {
        name_key: format!("animation.states.{}", state.clip_name()),
        length,
        clock: Clock::Shared,
        looping,
        note: note.to_string(),
        tracks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No state is seeded twice. Two rows for one state is two files written
    /// to one path, and the second one silently wins.
    #[test]
    fn each_state_is_seeded_at_most_once() {
        for (index, seed) in SEEDS.iter().enumerate() {
            assert!(
                !SEEDS[..index].iter().any(|earlier| earlier.state == seed.state),
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
