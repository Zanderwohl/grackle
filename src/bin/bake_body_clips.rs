//! Writes the body animations the default pack ships, from the Rust functions
//! they replace.
//!
//! Run from the repo root:
//!
//! ```text
//! cargo run --bin bake_body_clips
//! ```
//!
//! # Why a generator rather than hand-authoring
//!
//! The point of migrating is that these become editable — but the *first*
//! version has to be faithful, and nobody can hand-place a cosine. So this
//! samples `AnimationState::pose` and writes down what it found, and after
//! that the file is the source of truth and this program is only how it was
//! seeded. The same arrangement `new_map_template` has with the blueprint a
//! new map starts from.
//!
//! Re-running it **overwrites** whatever has been authored since. That is the
//! price of keeping the seeding reviewable, and it is why it is a separate
//! binary rather than something that runs on startup.
//!
//! # What is lost
//!
//! The functions carry named constants with doc comments arguing the number —
//! `IDLE_BOB` is `0.022` "in hip-heights" and says why. A clip has numbers and
//! no argument, so each one gets a `note` saying where it came from; the
//! reasoning stays in the Rust the generator reads, which is why the functions
//! are not deleted.

use std::path::Path;

use bevy::prelude::*;

use grackle::common::rotation::euler_from_quat;
use grackle::common::skeleton::clip::{
    Axis, Channel, Clip, Clock, Combine, PointKey, RotationKey, Space, Track, Unit,
};
use grackle::common::skeleton::state::{AnimationState, PoseInputs};
use grackle::common::skeleton::{default_humanoid, Interpolation, Pose};

/// How many keys to lay across a cycle.
///
/// Sixteen, which is enough that a cosine sampled at these points and eased
/// between them is within a fifth of a degree of the real thing everywhere —
/// `every_migrated_clip_still_matches_its_function` is what actually holds
/// that, and it is the number to raise if it ever stops passing.
const KEYS_PER_CYCLE: usize = 16;

/// Smooth at both ends in the second derivative too, so a loop seam does not
/// show as a kink. An idle is a cosine and a cosine has no corners.
const EASE: Interpolation = Interpolation::SmootherStep;

fn main() {
    let directory = Path::new("assets/default/animations");
    std::fs::create_dir_all(directory).expect("assets/default/animations should be creatable");

    for (state, length, looping, note) in [
        (
            AnimationState::Idle,
            grackle::common::skeleton::state::IDLE_PERIOD,
            true,
            "Sampled from `idle_pose`. A settle the body sinks into and comes back \
             out of, with the chest leaning into it and the arms trailing a quarter \
             cycle behind.",
        ),
        (
            AnimationState::Crouch,
            1.0,
            false,
            "Sampled from `crouch_posture`. Still, so one key says all of it — the \
             length is a formality.",
        ),
        (
            AnimationState::Airborne,
            1.0,
            false,
            "Sampled from `airborne_pose`. Still, for now: a real jump wants keys \
             through the arc, which is what having a timeline is for.",
        ),
    ] {
        let clip = bake(state, length, looping, note);
        let path = directory.join(format!("{}.gpa", state.clip_name()));
        let text = toml::to_string_pretty(&clip).expect("a clip should serialise");
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        println!(
            "{} — {} track(s), {} key(s)",
            path.display(),
            clip.tracks.len(),
            clip.tracks
                .iter()
                .map(|track| track.channel.key_count())
                .sum::<usize>(),
        );
    }
}

/// Sample a state's own pose function across a cycle and write down what it
/// did.
fn bake(state: AnimationState, length: f32, looping: bool, note: &str) -> Clip {
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
                    .map(|(at, pose)| RotationKey {
                        at: *at,
                        turn: euler_from_quat(pose.joint(bone)).to_array(),
                        ease: EASE,
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
                    .map(|(at, pose)| PointKey {
                        at: *at,
                        to: [
                            Axis::Value(pose.root_offset.x),
                            Axis::Value(pose.root_offset.y),
                            Axis::Value(pose.root_offset.z),
                        ],
                        ease: EASE,
                    })
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
