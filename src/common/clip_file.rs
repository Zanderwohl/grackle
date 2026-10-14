//! Reading the body animations a pack ships.
//!
//! One file per [`AnimationState`], named by the state, so nothing has to list
//! a directory (see `common::assets`). A missing file is the ordinary case:
//! that state stays on its Rust function. A file that is present and will not
//! parse is logged and the state also stays on its function, so a bad file on
//! a listen server does not panic.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use strum::IntoEnumIterator;

use crate::common::assets::{default_packs, AssetError, Assets};
use crate::common::skeleton::clip::StateAnimation;
use crate::common::skeleton::state::{AnimationState, ClipLibrary};

pub const CLIPS_DIR: &str = "animations";
pub const CLIP_EXTENSION: &str = "gpa";

pub fn read_clip(
    assets: &Assets,
    pack: &Path,
    state: AnimationState,
) -> Result<Option<StateAnimation>, String> {
    let relative = format!("{CLIPS_DIR}/{}.{CLIP_EXTENSION}", state.clip_name());
    let bytes = match assets.read(pack, &relative) {
        Ok(bytes) => bytes,
        Err(AssetError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };

    let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    // `from_toml` sorts keys here so the sampler, which runs in a match, need not.
    let animation = StateAnimation::from_toml(&text).map_err(|error| error.to_string())?;
    Ok(Some(animation))
}

/// Every body clip the packs provide, lowest priority first. A later pack
/// replaces a state's clip outright rather than merging tracks: half of two
/// authors' idles is a motion neither authored.
pub fn load_body_clips(assets: &Assets, packs: &[PathBuf]) -> ClipLibrary {
    let mut library = ClipLibrary::default();

    for pack in packs {
        for state in AnimationState::iter() {
            match read_clip(assets, pack, state) {
                Ok(Some(clip)) => library.insert(state, clip),
                Ok(None) => {}
                Err(why) => warn!(
                    "{}/{CLIPS_DIR}/{}.{CLIP_EXTENSION}: {why}; {state:?} keeps its built-in pose",
                    pack.display(),
                    state.clip_name(),
                ),
            }
        }
    }

    library
}

pub fn load_body_clips_at_startup(mut commands: Commands, assets: Res<Assets>) {
    let library = load_body_clips(&assets, &default_packs());
    if library.is_empty() {
        info!("No body animations on disk; every state is a built-in pose");
    } else {
        info!("Loaded {} body animation(s)", library.len());
    }
    commands.insert_resource(library);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::rig::humanoid;
    use crate::common::skeleton::state::PoseInputs;
    use crate::common::class::Class;
    use crate::common::skeleton::seed;

    /// A fifth of a degree: a baked clip is a sampling of a function, so it
    /// cannot be exact.
    const TOLERANCE: f32 = 0.2_f32.to_radians();

    fn shipped(state: AnimationState) -> Option<StateAnimation> {
        let path = Path::new("assets/default")
            .join(CLIPS_DIR)
            .join(format!("{}.{CLIP_EXTENSION}", state.clip_name()));
        let text = std::fs::read_to_string(path).ok()?;
        Some(StateAnimation::from_toml(&text).expect("a shipped clip should parse"))
    }

    /// Tolerance between a blend's headings. Looser than [`TOLERANCE`]: a blend
    /// lerps two poses and a diagonal is not quite their average. Measured worst:
    /// 1.35° upright at five headings, 1.51° crouched at three.
    const BETWEEN: f32 = 2.0_f32.to_radians();

    /// A body travelling at `heading` from the way its state faces.
    fn travelling(state: AnimationState, heading: f32) -> Vec2 {
        let facing = crate::common::skeleton::gait::direction_of(state);
        let (sin, cos) = heading.sin_cos();
        Vec2::new(facing.x * cos - facing.y * sin, facing.x * sin + facing.y * cos)
    }

    /// Seeded clips must move as the functions they replace, sampled across the
    /// whole cycle through `animation_pose`. Bakes in memory rather than reading
    /// `assets/default`, because the shipped clips are meant to be hand-edited.
    #[test]
    fn every_baked_clip_still_matches_its_function() {
        assert!(
            !seed::SEEDS.is_empty(),
            "no states are seeded — if the table moved, this test is passing \
             by not looking at anything",
        );

        // Through `animation_pose`, so foot timing and blends are read as a
        // match reads them.
        let rig = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Mercenary));
        let root = Transform::IDENTITY;

        for entry in seed::SEEDS {
            let state = entry.state;
            let animation = seed::bake(*entry);
            let clip = animation.clips()[0].clone();
            let gait = state.uses_gait();

            let speeds: &[f32] = if gait { &[0.0, 2.8, 3.6, 4.5, 7.5] } else { &[0.0] };
            let headings: Vec<(f32, f32)> = match entry.headings {
                [] => vec![(0.0, TOLERANCE)],
                authored => {
                    let (low, high) = (authored[0], authored[authored.len() - 1]);
                    (0..=12)
                        .map(|step| {
                            let heading = low + (high - low) * step as f32 / 12.0;
                            let on_a_variant =
                                authored.iter().any(|at| (at - heading).abs() < 1e-4);
                            (heading, if on_a_variant { TOLERANCE } else { BETWEEN })
                        })
                        .collect()
                }
            };

            for &speed in speeds {
                for &(heading, tolerance) in &headings {
                    for step in 0..=200 {
                        let at = clip.length * step as f32 / 200.0;
                        let inputs = PoseInputs {
                            speed,
                            travel: if gait { travelling(state, heading) } else { Vec2::ZERO },
                            ..seed::inputs_at(clip.clock, at, clip.length)
                        };

                        let was = state.pose(&inputs);
                        let now = crate::common::skeleton::ik::animation_pose(
                            &animation, state, &inputs, &rig, &root,
                        );

                        assert!(
                            (now.root_offset - was.root_offset).length() < 0.001,
                            "{state:?} at {at:.3}: the root moved to {:?}, not {:?}",
                            now.root_offset,
                            was.root_offset,
                        );
                        for (bone, wanted) in was.posed_joints() {
                            let apart = now.joint(bone).angle_between(wanted);
                            assert!(
                                apart < tolerance,
                                "{state:?} at {at:.3}, speed {speed}, heading {:.1}°: \
                                 '{bone}' is {:.3}° from where the function put it",
                                heading.to_degrees(),
                                apart.to_degrees(),
                            );
                        }
                    }
                }
            }
        }
    }

    /// A shipped clip with a problem (say, a track naming a bone the rig lacks)
    /// silently does nothing at runtime.
    #[test]
    fn every_shipped_body_clip_is_one_the_editor_would_not_flag() {
        let rig = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Mercenary));
        let known = |name: &str| rig.bones().iter().any(|bone| bone.name == name);

        let mut found = 0;
        for state in AnimationState::iter() {
            let Some(clip) = shipped(state) else { continue };
            let problems = clip.problems(known);
            assert!(
                problems.is_empty(),
                "{}.{CLIP_EXTENSION}: {problems:?}",
                state.clip_name(),
            );
            found += 1;
        }
        assert!(found > 0, "no shipped body clips were found");
    }

    /// Keyframes can jump at a segment boundary, and at the loop seam if the
    /// first and last keys disagree; the walk wraps round to include the seam.
    #[test]
    fn no_migrated_clip_lurches_anywhere_including_its_seam() {
        use crate::common::skeleton::rig::Proportions;

        const STEPS: usize = 400;
        // Same budget as `a_solved_limb_never_lurches`.
        const MOST: f32 = 0.2;

        for proportions in [Proportions::DEFAULT, crate::common::class::ClassBuilds::shipped().of(Class::Heavy)] {
            let rig = humanoid(proportions);
            let root = Transform::IDENTITY;

            for state in AnimationState::iter() {
                let Some(animation) = shipped(state) else { continue };
                let clip = animation.clips()[0].clone();
                if !clip.looping {
                    continue;
                }

                // At a sprint: the most a foot-timed track covers in one step.
                let at = |step: usize| {
                    let time = clip.length * step as f32 / STEPS as f32;
                    let inputs = PoseInputs {
                        speed: 7.5,
                        ..seed::inputs_at(clip.clock, time, clip.length)
                    };
                    crate::common::skeleton::ik::animation_pose(&animation, state, &inputs, &rig, &root)
                };

                // `..=STEPS`: the last step is the seam.
                let mut previous = at(0);
                for step in 1..=STEPS {
                    let pose = at(step);
                    for bone in rig.bones() {
                        let turned = previous
                            .joint(bone.name)
                            .angle_between(pose.joint(bone.name));
                        assert!(
                            turned < MOST,
                            "{state:?}: '{}' turns {turned:.3} rad in a four-hundredth \
                             of a cycle at step {step} on {proportions:?}",
                            bone.name,
                        );
                    }
                    let moved = (pose.root_offset - previous.root_offset).length();
                    assert!(
                        moved < MOST,
                        "{state:?}: the root moves {moved:.3} hip-heights in a \
                         four-hundredth of a cycle at step {step}",
                    );
                    previous = pose;
                }
            }
        }
    }

    /// A pack with no animations leaves every state on its function.
    #[test]
    fn a_pack_with_no_animations_says_nothing() {
        let library = load_body_clips(&Assets::default(), &[PathBuf::from("assets/nonexistent")]);
        assert!(library.is_empty());
    }
}
