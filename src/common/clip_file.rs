//! Reading the body animations a pack ships.
//!
//! One file per [`AnimationState`], named by the state — which is what lets
//! this load **without listing a directory**. The names come from the enum, so
//! the runtime asks for each by name and a pack that does not have one simply
//! does not answer; nothing behind a `fetch` can enumerate, and a manifest
//! saying what a pack holds would be a second description of the pack. The
//! same rule `common::assets` is built on.
//!
//! **A missing file is the ordinary case, not a failure.** A state with no
//! clip is a state that is still a Rust function, which is how the migration
//! is thirteen independent decisions rather than one. A file that is *present*
//! and will not parse is a different thing and says so in the log, then leaves
//! that state on its function — the same reading `weapon_file` takes of a bad
//! entry, and for the same reason: a bad file on a listen server with people
//! connected should read as "that animation is the old one", not as a panic.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use strum::IntoEnumIterator;

use crate::common::assets::{default_packs, AssetError, Assets};
use crate::common::skeleton::clip::StateAnimation;
use crate::common::skeleton::state::{AnimationState, ClipLibrary};

/// Where a pack keeps them, and what they are called.
pub const CLIPS_DIR: &str = "animations";
pub const CLIP_EXTENSION: &str = "gpa";

/// Read one state's animation out of one pack: a clip, or a blend of them.
pub fn read_clip(
    assets: &Assets,
    pack: &Path,
    state: AnimationState,
) -> Result<Option<StateAnimation>, String> {
    let relative = format!("{CLIPS_DIR}/{}.{CLIP_EXTENSION}", state.clip_name());
    let bytes = match assets.read(pack, &relative) {
        Ok(bytes) => bytes,
        // Nothing to say: this pack does not animate this state.
        Err(AssetError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };

    let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    // Sorted on the way in rather than at sample time, for the reason
    // `Channel::sort` gives: the sampler is the one thing here that runs in a
    // match. `from_toml` does it.
    let animation = StateAnimation::from_toml(&text).map_err(|error| error.to_string())?;
    Ok(Some(animation))
}

/// Every body clip the packs provide, lowest priority first.
///
/// A later pack replaces an earlier one's clip for the same state outright
/// rather than merging tracks into it: an animation is one description of a
/// motion, and half of somebody else's idle composed with half of yours is a
/// motion neither of you authored. The lang layer merges per *key* because a
/// translation genuinely is a bag of independent strings; this is not.
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

    /// How far a migrated clip may be from the function it replaced.
    ///
    /// A fifth of a degree at any joint, at any moment, on any build. The
    /// migration is a *sampling* — sixteen keys eased between, standing in for
    /// a cosine — so it cannot be exact, and saying how inexact is the whole
    /// point of having the number here rather than inline.
    const TOLERANCE: f32 = 0.2_f32.to_radians();

    fn shipped(state: AnimationState) -> Option<StateAnimation> {
        let path = Path::new("assets/default")
            .join(CLIPS_DIR)
            .join(format!("{}.{CLIP_EXTENSION}", state.clip_name()));
        let text = std::fs::read_to_string(path).ok()?;
        Some(StateAnimation::from_toml(&text).expect("a shipped clip should parse"))
    }

    /// **The migration safety net.**
    ///
    /// Seeding a pack has to produce clips that move the way the Rust
    /// functions they replace move. Without this, migrating is a change
    /// nothing can check: the bodies still animate, and whether they animate
    /// the way they used to is a matter of somebody's memory.
    ///
    /// It bakes in memory rather than reading `assets/default`, and that is
    /// the difference between a net and a muzzle. The shipped clips are *meant*
    /// to be edited — that is the whole point of having them as data — and
    /// while this test read them, the first retimed idle failed it. What has to
    /// stay faithful is the generator; what a file says after somebody has
    /// worked on it is their business, and `no_migrated_clip_lurches_anywhere_including_its_seam`
    /// below is what still holds the shipped files to something.
    ///
    /// Sampled across the whole cycle rather than at the keys, because the
    /// keys are the part the generator got right by construction — what is
    /// being tested is the easing between them.
    /// How far a blend may be from the function **between** its headings.
    ///
    /// Looser than [`TOLERANCE`], and not because the baking is careless: a
    /// blend is a lerp between two poses, and a diagonal is not quite the
    /// average of its neighbours. Measured worst: 1.35° for an upright gait
    /// at five headings, 1.51° for a crouched one at three.
    const BETWEEN: f32 = 2.0_f32.to_radians();

    /// A body travelling at `heading` from the way its state faces.
    fn travelling(state: AnimationState, heading: f32) -> Vec2 {
        let facing = crate::common::skeleton::gait::direction_of(state);
        let (sin, cos) = heading.sin_cos();
        Vec2::new(facing.x * cos - facing.y * sin, facing.x * sin + facing.y * cos)
    }

    #[test]
    fn every_baked_clip_still_matches_its_function() {
        assert!(
            !seed::SEEDS.is_empty(),
            "no states are seeded — if the table moved, this test is passing \
             by not looking at anything",
        );

        // **Through the path the game plays them by**, `animation_pose`, so a
        // foot-timed track is read at the footfalls a body at that speed
        // really has and a blend is blended the way a match blends it.
        let rig = humanoid(Class::Mercenary.proportions());
        let root = Transform::IDENTITY;

        for entry in seed::SEEDS {
            let state = entry.state;
            let animation = seed::bake(*entry);
            let clip = animation.clips()[0].clone();
            let gait = state.uses_gait();

            // Every speed a body really moves at, from standing to a sprint —
            // which is what foot timing is supposed to make one clip right at.
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

    /// Every clip the default pack ships parses and has nothing to complain
    /// about — the same question the Problems panel asks.
    ///
    /// This is what the fidelity test above used to cover incidentally, and the
    /// part of it that survives a hand edit: a retimed idle is fine, an idle
    /// with a track naming a bone this rig does not have is not, and the
    /// failure at runtime is a track that silently does nothing.
    #[test]
    fn every_shipped_body_clip_is_one_the_editor_would_not_flag() {
        let rig = humanoid(Class::Mercenary.proportions());
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

    /// **No migrated clip lurches**, including across its loop seam.
    ///
    /// The risk the whole migration was weighed against. `a_solved_limb_never_lurches`
    /// guards this for the parametric functions, which are structurally
    /// incapable of it — a sine has no corners. Keyframes are not: a clip can
    /// jump at every segment boundary, and *will* at the seam if the first and
    /// last keys disagree. So the same property is asserted of the clip path,
    /// and the seam is included by walking a whole cycle and wrapping round to
    /// where it started rather than stopping at the end.
    #[test]
    fn no_migrated_clip_lurches_anywhere_including_its_seam() {
        use crate::common::skeleton::rig::Proportions;

        const STEPS: usize = 400;
        // A four-hundredth of a cycle, the same budget the parametric guard
        // uses. Nothing here is solved, so this is the authored motion alone.
        const MOST: f32 = 0.2;

        for proportions in [Proportions::DEFAULT, Class::Heavy.proportions()] {
            let rig = humanoid(proportions);
            let root = Transform::IDENTITY;

            for state in AnimationState::iter() {
                let Some(animation) = shipped(state) else { continue };
                let clip = animation.clips()[0].clone();
                if !clip.looping {
                    // A still pose cannot lurch, and one that does not loop has
                    // no seam to come round to.
                    continue;
                }

                // Through the path the game plays by, at a sprint: the fastest
                // the footfalls move, so the most a foot-timed track is asked
                // to cover in one step.
                let at = |step: usize| {
                    let time = clip.length * step as f32 / STEPS as f32;
                    let inputs = PoseInputs {
                        speed: 7.5,
                        ..seed::inputs_at(clip.clock, time, clip.length)
                    };
                    crate::common::skeleton::ik::animation_pose(&animation, state, &inputs, &rig, &root)
                };

                // `..=STEPS` rather than `..STEPS`: the last step *is* the
                // seam, having wrapped back to the start, and it is the one
                // place a clip can jump that a function never could.
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

    /// A pack that ships no animations leaves every state on its function.
    ///
    /// The ordinary case for any pack but the default one, and the thing that
    /// makes a half-migrated build work rather than break.
    #[test]
    fn a_pack_with_no_animations_says_nothing() {
        let library = load_body_clips(&Assets::default(), &[PathBuf::from("assets/nonexistent")]);
        assert!(library.is_empty());
    }
}
