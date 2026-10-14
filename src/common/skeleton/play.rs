//! Turning a [`Clip`] into motion.
//!
//! Two halves, kept apart on purpose:
//!
//! - [`sample_at`] is **pure**. Given some keys and a time it produces a
//!   value, and it knows nothing about bodies, worlds or frames. Stateless —
//!   no cursor, no last-frame index — so a scrub that jumps backwards is the
//!   same call as one that steps forwards, which is what lets the editor's
//!   playhead and a body in a match go through one function rather than two
//!   that agree until they do not.
//! - [`apply`] is where a sampled target becomes a pose, and it needs to know
//!   what the clip's spaces *are*. It is handed them in [`Frames`] rather than
//!   looking them up, because `common::skeleton` must not reach for a
//!   `CollisionWorld` or a `HoldSpec` — a clip has to be samplable in a unit
//!   test with no App, which is the rule `src/prop/` already states as
//!   "nothing above `view` touches a `World`".
//!
//! # Absence composes
//!
//! Every sample returns an `Option` and a channel with nothing to say returns
//! `None`. A layer that wrote a default would be a layer that overwrote the
//! one beneath it, and "absence means rest" is already how a [`Pose`] is read.
//! [`Axis::Free`] is the same idea one axis down: a track that only cares
//! about height leaves the other two where the layer beneath put them.

use std::sync::Arc;

use bevy::prelude::*;

use super::clip::{Axis, ChainRef, Channel, Clip, Combine, Key, Space, Unit};
use super::ik::{self, Chain, Reach};
use super::interpolation::Interpolation;
use super::rig::{Pose, Skeleton};
use crate::common::rotation::quat_from_euler;

/// The value a keyed channel holds at `time`, or `None` if it has nothing to
/// say.
///
/// `time` is expected to have been through [`Clip::wrap`] already, so a
/// looping clip's time is inside `[0, length)`.
///
/// Five cases, and each is a decision:
///
/// - **No keys** — `None`. The case that makes layering work.
/// - **One key** — that key, everywhere. A track can be a constant.
/// - **Before the first key** — held, not extrapolated. On a looping clip
///   "before the first" is really *after the last*, coming round the seam, so
///   it blends from the last key instead.
/// - **After the last key, not looping** — held. A one-shot that has finished
///   keeps the pose it ended in until something takes it off the stack, which
///   is the right resting state: the alternative is a weapon that snaps back
///   to the hold the instant a reload's last key passes.
/// - **After the last key, looping** — blended round to the first, over
///   `(length - last.at) + first.at`. This is the seam, and it is the one
///   place a clip can visibly lurch.
///
/// **The outgoing key's curve governs a segment**, including the seam's. Said
/// once, here, because the other reading is equally defensible and two call
/// sites that disagree is a curve that inverts halfway along.
pub fn sample_at<K, T: Copy>(
    keys: &[K],
    time: f32,
    length: f32,
    looping: bool,
    at: impl Fn(&K) -> f32,
    value: impl Fn(&K) -> T,
    ease: impl Fn(&K) -> Interpolation,
    blend: impl Fn(T, T, Interpolation, f32) -> T,
) -> Option<T> {
    let first = keys.first()?;
    let last = keys.last()?;

    if keys.len() == 1 {
        return Some(value(first));
    }

    // How long the wrap-around segment lasts, and where in it `time` sits.
    // Only meaningful while looping; computed once so the two branches that
    // use it cannot disagree about which way round it goes.
    let seam = (length - at(last)) + at(first);

    if time <= at(first) {
        if !looping {
            return Some(value(first));
        }
        // Before the first key on a looping clip is the tail of the seam: the
        // clip has come round past `length` and back to zero.
        if seam <= 0.0 {
            return Some(value(first));
        }
        let along = (time + length - at(last)) / seam;
        return Some(blend(value(last), value(first), ease(last), along));
    }

    if time >= at(last) {
        if !looping || seam <= 0.0 {
            return Some(value(last));
        }
        let along = (time - at(last)) / seam;
        return Some(blend(value(last), value(first), ease(last), along));
    }

    // Somewhere in the middle. A linear scan rather than a binary search: a
    // track has tens of keys, not thousands, and the scan is what the cache
    // wants at that size.
    for pair in keys.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        if time < at(from) || time > at(to) {
            continue;
        }
        let span = at(to) - at(from);
        // Two keys stacked at one time is a legal thing to author — it is how
        // a snap is written without using `Step` — and dividing by it is not.
        let along = if span > 0.0 { (time - at(from)) / span } else { 0.0 };
        return Some(blend(value(from), value(to), ease(from), along));
    }

    Some(value(last))
}

/// Sample a track — any track.
///
/// Each axis is eased independently, which is what makes a mixed key mean
/// anything at all: the three axes are three little channels that happen to
/// share a time and a curve. `[0.2, "ground", "free"]` is a foot a fifth of a
/// leg forward, on the floor, and left alone sideways.
pub fn sample_key(keys: &[Key], time: f32, length: f32, looping: bool) -> Option<[Axis; 3]> {
    sample_at(
        keys,
        time,
        length,
        looping,
        |key| key.at,
        |key| key.to,
        |key| key.ease,
        |from, to, ease, along| {
            let mut blended = [Axis::Free; 3];
            for axis in 0..3 {
                blended[axis] = match (from[axis], to[axis]) {
                    // Two numbers ease into each other, which is the ordinary
                    // case and the only one with anything to interpolate.
                    (Axis::Value(a), Axis::Value(b)) => Axis::Value(ease.f32(a, b, along)),
                    // Two clearances ease too — a foot lifting off the floor
                    // is exactly this, and it stays pinned to the surface the
                    // whole way up rather than to the height the floor
                    // happened to be at when the key was authored.
                    (Axis::Ground { clearance: a }, Axis::Ground { clearance: b }) => {
                        Axis::Ground { clearance: ease.f32(a, b, along) }
                    }
                    // Anything else is a change of *kind* — an axis that was
                    // free and is now a number, or pinned to the floor and is
                    // now a height — and there is no halfway between a
                    // constraint and a value, nor between having an opinion
                    // and not having one. It switches on arrival, the way
                    // `Step` does, because the alternative is inventing a
                    // blend nobody could predict.
                    (a, b) => {
                        if ease.ease(along) >= 1.0 {
                            b
                        } else {
                            a
                        }
                    }
                };
            }
            blended
        },
    )
}

/// The angles a sampled rotation key asks for, against what is already there.
///
/// **`Free` means the layer beneath keeps that angle**, which is the whole
/// point of a key being three optional axes: a key can turn a bone about one
/// axis and say nothing about the other two.
///
/// Reading an angle back out of a quaternion is not unique — several Euler
/// triples name the same orientation — so what comes back is the
/// decomposition in [`EULER_ORDER`], which is the one the file is written in.
/// For the angles a rig actually uses that is the reading anybody would
/// expect; near a gimbal it is a choice rather than a fact, and a key that
/// wants certainty should say all three.
///
/// [`EULER_ORDER`]: crate::common::rotation::EULER_ORDER
fn angles_for(to: [Axis; 3], beneath: Quat) -> Vec3 {
    let already = crate::common::rotation::euler_from_quat(beneath);
    let mut wanted = already;
    for axis in 0..3 {
        wanted[axis] = match to[axis] {
            Axis::Free => already[axis],
            Axis::Value(at) => at,
            // No floor under an angle; `Clip::problems` says so.
            Axis::Ground { .. } => already[axis],
        };
    }
    wanted
}

/// The same, for a rotation composed onto whatever is beneath it.
///
/// `Free` contributes **nothing** here rather than reading the layer beneath,
/// because composing what is already there onto itself would double it. An
/// axis with no opinion turns the bone by zero, which is what "says nothing"
/// has to mean when the operation is a multiply.
fn angles_onto(to: [Axis; 3]) -> Vec3 {
    let mut wanted = Vec3::ZERO;
    for axis in 0..3 {
        if let Axis::Value(at) = to[axis] {
            wanted[axis] = at;
        }
    }
    wanted
}

/// Put a key on a channel at `at`, holding whatever the channel already said
/// there.
///
/// **Inserting a key does not change the pose.** The value is sampled from the
/// track as it stands, so adding a key is a way of *pinning* what is already
/// happening before you change it — which is what keying at the playhead means
/// everywhere else and what makes it safe to do while looking at something you
/// like. A key inserted at a default would jump the body the moment it landed.
///
/// A track with nothing on it yet gets a key that says nothing either: three
/// free axes, which changes no pose at all. A place to start rather than a
/// pose somebody has to undo.
///
/// Returns where it ended up, so a caller can select it.
pub fn key_the_channel(channel: &mut Channel, at: f32, length: f32, looping: bool) -> usize {
    let at = at.clamp(0.0, length);
    let keys = channel.keys_mut();

    let to = sample_key(keys, at, length, looping).unwrap_or([Axis::Free; 3]);
    let ease = keys.last().map(|key| key.ease).unwrap_or_default();
    keys.push(Key { at, to, ease });
    keys.sort_by(|a, b| a.at.total_cmp(&b.at));
    keys.iter().position(|key| key.at == at).unwrap_or(0)
}

/// How many metres one of `unit` is on this body./// How many metres one of `unit` is on this body.
///
/// One function so two call sites cannot disagree, and the reason the whole
/// format is worth having: a clip authored in leg-lengths fits every build,
/// and one authored in metres fits the build it was drawn against.
pub fn metres_per(unit: Unit, skeleton: &Skeleton) -> f32 {
    let proportions = skeleton.proportions();
    match unit {
        Unit::Metres => 1.0,
        Unit::LegLengths => ik::leg_length(skeleton),
        // Exactly what `weapon_transform` computes for `HoldSpec::carry`.
        Unit::ArmLengths => proportions.height * proportions.arm_length,
        Unit::HipHeights => proportions.hip_metres(),
    }
}

/// What the spaces a clip names actually are, this frame, for this body.
///
/// Filled by the caller because the answers live in three different layers —
/// `prop::hold` knows where the weapon is, `game::collision` knows where the
/// floor is, and neither belongs underneath `common::skeleton`.
pub struct Frames<'a> {
    /// Feet on the floor, facing the way the body faces. The same `root` that
    /// [`ik::plant_feet`] and [`ik::solve`] are already given.
    pub body: Transform,
    /// The point between the shoulders, turned by the aim — the pivot a
    /// weapon hangs from.
    pub carry: Transform,
    /// The weapon's own space, as placed. `None` for a body holding nothing,
    /// and tracks naming it are then skipped rather than falling back to
    /// somewhere arbitrary.
    pub weapon: Option<Transform>,
    /// Eye position and look direction, for a viewmodel clip.
    pub eye: Option<Transform>,
    /// How high the floor is under a point.
    ///
    /// A closure rather than a `&CollisionWorld`, so a test and the prop
    /// editor answer it with a plane and the game answers it with a ray.
    /// **`Ground` degrading to a plane where there is no world is a decision,
    /// not a fallback**: the reference figure stands in an empty viewport, and
    /// an axis that resolved to nothing there would author differently from
    /// how it plays — which is the failure the weapons plan already names
    /// about holds.
    pub ground: &'a dyn Fn(Vec3) -> f32,
    /// Where each foot is in its own cycle, left first — see
    /// [`Timing`](super::clip::Timing). Stated by whoever knows the gait, the
    /// way the floor is.
    ///
    /// `None` reads a foot-timed track as though the stance were exactly one
    /// half: the left foot at the stride, the right half a stride on. Right
    /// for nothing in particular, and the honest answer for a body with no gait.
    pub footfalls: Option<[f32; 2]>,
}

impl Frames<'_> {
    /// A body standing on a flat floor with nothing in its hands — what a
    /// test and the reference figure both want.
    pub fn standing(body: Transform, carry: Transform, ground: &dyn Fn(Vec3) -> f32) -> Frames<'_> {
        Frames {
            body,
            carry,
            weapon: None,
            eye: None,
            ground,
            footfalls: None,
        }
    }

    fn of(&self, space: Space) -> Option<Transform> {
        match space {
            Space::Body => Some(self.body),
            Space::Carry => Some(self.carry),
            Space::Weapon => self.weapon,
            Space::Eye => self.eye,
        }
    }
}

/// One clip playing on one body.
///
/// The clip is shared rather than cloned: a clip is a few kilobytes and sixty
/// bodies in an animation grid play the same one.
#[derive(Clone, Debug)]
pub struct Playing {
    pub clip: Arc<Clip>,
    /// Where this performance has got to, on the clip's own clock.
    pub time: f32,
    /// How much of it is showing, `0..=1`.
    pub weight: f32,
}

/// Everything playing on this body, bottom layer first.
#[derive(Component, Clone, Debug, Default)]
pub struct ClipStack(pub Vec<Playing>);

/// Where the end of a chain currently is, in world space.
fn chain_tip(skeleton: &Skeleton, pose: &Pose, root: &Transform, chain: &Chain) -> Option<Vec3> {
    // The `lower` bone's tail: the wrist or the ankle, which is the point
    // `solve` puts on its target.
    let lower = skeleton.index_of(chain.lower)?;
    Some(skeleton.posed_bones(pose, root)[lower].tail)
}

/// Resolve a sampled target to a world point.
///
/// `Free` keeps whatever the layer beneath put there, which is why the current
/// tip has to be read back and taken apart in the track's own space and unit —
/// there is no other way to say "leave this one axis alone".
fn resolve_point(
    to: [Axis; 3],
    tip: Vec3,
    frame: &Transform,
    scale: f32,
    combine: Combine,
    ground: &dyn Fn(Vec3) -> f32,
) -> Vec3 {
    let inverse = frame.to_matrix().inverse();
    let local = inverse.transform_point3(tip) / scale;

    let mut wanted = local;
    for axis in 0..3 {
        wanted[axis] = match to[axis] {
            Axis::Free => local[axis],
            Axis::Value(at) => match combine {
                Combine::Replace => at,
                // An offset from wherever the layer beneath left it — which is
                // what a recoil wants: "thirty millimetres back from wherever
                // the hands are", not "thirty millimetres from the shoulder".
                Combine::Compose => local[axis] + at,
            },
            // Left as-is here and overwritten in world space below: the floor
            // is a fact about the world, not about the track's frame, and
            // rotating a height through a turned frame is not a height.
            Axis::Ground { .. } => local[axis],
        };
    }

    let mut target = frame.transform_point(wanted * scale);
    // In world space, after the frame has had its say: a height above the
    // floor is a height above the floor whatever the track was measured in,
    // and the clearance is scaled because it is a distance like any other.
    if let Axis::Ground { clearance } = to[1] {
        target.y = ground(target) + clearance * scale;
    }
    target
}

/// Apply `clip` at `time` to `pose`, in place.
///
/// In place rather than producing a pose to merge, for the reason
/// `look_with_the_head` edits the pose the animator left: a merge needs a rule
/// for every joint neither side named, and "leave it alone" is that rule
/// written as code instead of as a policy.
///
/// `weapon` is moved by weapon tracks, and is what makes a reload a reload:
/// the clip dips the weapon and the arms are solved onto it *afterwards*, by
/// `grip_the_weapon`, so a recoil authored once is correct on every build and
/// no hand is ever keyed.
///
/// Returns what each effector track managed, in track order, so a caller can
/// tell an arm that did not reach from one that did — the same thing
/// `grip_with` hands back and for the same reason.
/// When a track is, given when its clip is.
///
/// A foot-timed track keeps the clip's whole strides and swaps the fraction
/// for its foot's own cycle, so a clip several strides long still walks
/// through its strides in order.
pub fn track_time(timing: super::clip::Timing, time: f32, footfalls: Option<[f32; 2]>) -> f32 {
    use super::clip::Timing;
    let foot = match timing {
        Timing::Clip => return time,
        Timing::LeftFoot => 0,
        Timing::RightFoot => 1,
    };
    let fraction = match footfalls {
        Some(feet) => feet[foot],
        None => (time + 0.5 * foot as f32).rem_euclid(1.0),
    };
    time.floor() + fraction
}

pub fn apply(
    clip: &Clip,
    time: f32,
    skeleton: &Skeleton,
    pose: &mut Pose,
    frames: &Frames,
    mut weapon: Option<&mut Transform>,
) -> Vec<Reach> {
    let time = clip.wrap(time);
    let length = clip.length;
    let looping = clip.looping;
    let mut reaches = Vec::new();

    for track in &clip.tracks {
        if !track.enabled {
            continue;
        }
        let time = track_time(track.timing, time, frames.footfalls);

        match &track.channel {
            Channel::Rotation { bone, keys } => {
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                // `Pose::set` takes a `&'static str` and a clip's names are
                // `String`s, so the name has to come back off the rig. A bone
                // this rig has not got is dropped — silently, the way a
                // misspelling in a `Pose` always has been, which is why
                // `Clip::problems` exists to say so in a panel and why the
                // shipped-clip test replays every file.
                let Some(index) = skeleton.index_of(bone) else {
                    continue;
                };
                let name = skeleton.bones()[index].name;
                let beneath = pose.joint(name);
                match track.combine {
                    // Whatever the key does not mention keeps the angle it
                    // already had, which is what lets a key turn one axis and
                    // leave the other two to the layer beneath.
                    Combine::Replace => {
                        pose.set(name, quat_from_euler(angles_for(to, beneath)))
                    }
                    // The composition `look_with_the_head` performs: in the
                    // joint's own frame, on top of whatever was there. An axis
                    // with no opinion turns it by nothing.
                    Combine::Compose => {
                        pose.set(name, beneath * quat_from_euler(angles_onto(to)))
                    }
                }
            }

            Channel::Effector { chain, pole, keys } => {
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                // Nothing to do, and *not* a solve onto where the tip already
                // is: `solve` re-derives the roll from the hinge, so re-solving
                // onto an unchanged target is a small silent rewrite of three
                // joints rather than the identity.
                if to.iter().all(|axis| matches!(axis, Axis::Free)) {
                    continue;
                }
                let Some(frame) = frames.of(track.space) else {
                    continue;
                };
                let solved = chain.chain();
                let Some(tip) = chain_tip(skeleton, pose, &frames.body, &solved) else {
                    continue;
                };

                let target = resolve_point(
                    to,
                    tip,
                    &frame,
                    metres_per(track.unit, skeleton),
                    track.combine,
                    frames.ground,
                );
                let pole = pole
                    .map(|pole| Vec3::from_array(pole))
                    .unwrap_or_else(|| default_pole(*chain, &frames.body));

                // A leg that cannot reach straightens and points, because a
                // body whose foot cannot find the floor should be falling. An
                // arm that cannot reach is put back where the animation had
                // it, because a limb stretched at something it cannot hold
                // reads as broken rather than as not holding. Two different
                // right answers, which is why there are two calls.
                reaches.push(if chain.is_leg() {
                    ik::solve(skeleton, pose, &frames.body, &solved, target, pole)
                } else {
                    ik::reach_or_leave(skeleton, pose, &frames.body, &solved, target, pole)
                });
            }

            Channel::Root { keys } => {
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                // In hip-heights and in the body's own frame, so neither the
                // track's space nor its unit is consulted — a root offset is a
                // displacement of the body from where it would otherwise
                // stand, and there is no other frame it could be read in.
                for axis in 0..3 {
                    let was = pose.root_offset[axis];
                    pose.root_offset[axis] = match (to[axis], track.combine) {
                        (Axis::Free, _) => was,
                        (Axis::Value(at), Combine::Replace) => at,
                        (Axis::Value(at), Combine::Compose) => was + at,
                        // There is no floor to find in hip-heights above a
                        // body's own feet; `Clip::problems` says so.
                        (Axis::Ground { .. }, _) => was,
                    };
                }
            }

            Channel::Weapon { keys } => {
                let Some(placed) = weapon.as_deref_mut() else {
                    continue;
                };
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                let Some(frame) = frames.of(track.space) else {
                    continue;
                };
                placed.translation = resolve_point(
                    to,
                    placed.translation,
                    &frame,
                    metres_per(track.unit, skeleton),
                    track.combine,
                    frames.ground,
                );
            }

            Channel::WeaponRotation { keys } => {
                let Some(placed) = weapon.as_deref_mut() else {
                    continue;
                };
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                match track.combine {
                    // Stated in the track's own space, so a clip can say "the
                    // muzzle points up the body" without knowing the aim — and
                    // an axis it says nothing about keeps whatever the hold
                    // had it at.
                    Combine::Replace => {
                        if let Some(frame) = frames.of(track.space) {
                            let beneath = frame.rotation.inverse() * placed.rotation;
                            placed.rotation =
                                frame.rotation * quat_from_euler(angles_for(to, beneath));
                        }
                    }
                    // On top of wherever the hold put it — a kick, which is
                    // what almost every weapon clip wants.
                    Combine::Compose => {
                        placed.rotation *= quat_from_euler(angles_onto(to));
                    }
                }
            }
        }
    }

    reaches
}

/// Which way a knee or an elbow points when a track does not say.
fn default_pole(chain: ChainRef, root: &Transform) -> Vec3 {
    if chain.is_leg() {
        // Knees bend forwards. The one thing a solver cannot work out and the
        // one thing a body is never in two minds about — a sidestep still has
        // its knees pointing the way its toes do.
        root.rotation * Vec3::NEG_Z
    } else {
        ik::elbow_pole(chain.side())
    }
}

/// Apply a whole stack, bottom layer first.
///
/// A layer at less than full weight is evaluated into a copy of the pose so
/// far and blended back with [`Pose::lerp`] — which is exactly what
/// `animator_pose` does with two finished poses, and well defined for the same
/// reason: a pose is sparse and named, so a joint one side has an opinion
/// about blends towards rest.
///
/// Full weight skips the clone, which is the common case and the one that runs
/// sixty times in an animation grid.
pub fn apply_stack(
    stack: &ClipStack,
    skeleton: &Skeleton,
    pose: &mut Pose,
    frames: &Frames,
    mut weapon: Option<&mut Transform>,
) {
    for playing in &stack.0 {
        if playing.weight <= 0.0 {
            continue;
        }

        if playing.weight >= 1.0 {
            apply(
                &playing.clip,
                playing.time,
                skeleton,
                pose,
                frames,
                weapon.as_deref_mut(),
            );
            continue;
        }

        let before = pose.clone();
        // The weapon is blended the same way and for the same reason: half a
        // recoil is half of the weapon's deviation, not all of it applied to
        // half the arms.
        let was = weapon.as_deref().copied();
        apply(
            &playing.clip,
            playing.time,
            skeleton,
            pose,
            frames,
            weapon.as_deref_mut(),
        );
        *pose = Pose::lerp(&before, pose, playing.weight);

        if let (Some(placed), Some(was)) = (weapon.as_deref_mut(), was) {
            placed.translation = was.translation.lerp(placed.translation, playing.weight);
            placed.rotation = was.rotation.slerp(placed.rotation, playing.weight);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::clip::{Clock, Track};
    use crate::common::skeleton::rig::{bone, default_humanoid};

    fn keys(times: &[f32]) -> Vec<Key> {
        times
            .iter()
            .enumerate()
            .map(|(index, at)| Key {
                at: *at,
                to: [Axis::Value(index as f32), Axis::Free, Axis::Free],
                ease: Interpolation::Linear,
            })
            .collect()
    }

    fn first_axis(sampled: Option<[Axis; 3]>) -> f32 {
        sampled.expect("expected a sample")[0]
            .value()
            .expect("expected a number")
    }

    /// A channel with nothing to say says nothing, rather than saying zero.
    /// The whole of layering rests on this: a layer that wrote a default would
    /// be a layer that overwrote the one beneath it.
    #[test]
    fn an_empty_track_has_nothing_to_say() {
        assert!(sample_key(&[], 0.5, 1.0, false).is_none());
        assert!(sample_key(&[], 0.5, 1.0, false).is_none());
    }

    /// One key is a constant, not a degenerate segment.
    #[test]
    fn one_key_is_a_constant() {
        let keys = keys(&[0.4]);
        for time in [0.0, 0.4, 0.9] {
            assert_eq!(first_axis(sample_key(&keys, time, 1.0, false)), 0.0);
        }
    }

    /// Outside the keyed range a one-shot holds rather than extrapolating.
    /// A clip that ran on past its last key would put a limb somewhere no key
    /// ever asked for, and the tell is one frame of a body inside out.
    #[test]
    fn a_one_shot_holds_at_both_ends() {
        let keys = keys(&[0.25, 0.75]);
        assert_eq!(first_axis(sample_key(&keys, 0.0, 1.0, false)), 0.0);
        assert_eq!(first_axis(sample_key(&keys, 1.0, 1.0, false)), 1.0);
    }

    /// The sampler has no cursor, so the order times are asked in cannot
    /// matter. This is what lets a scrub dragged backwards and a body in a
    /// match go through one function — and what rollback needs, since it
    /// replays ticks out of order.
    #[test]
    fn sampling_is_stateless() {
        let keys = keys(&[0.0, 0.5, 1.0]);
        let forwards: Vec<f32> = (0..=20)
            .map(|step| first_axis(sample_key(&keys, step as f32 / 20.0, 1.0, false)))
            .collect();
        let backwards: Vec<f32> = (0..=20)
            .rev()
            .map(|step| first_axis(sample_key(&keys, step as f32 / 20.0, 1.0, false)))
            .collect();

        let mut backwards = backwards;
        backwards.reverse();
        assert_eq!(forwards, backwards);
    }

    /// The seam is the one place a loop can visibly lurch, so it is sampled
    /// either side of the wrap and the two have to meet.
    #[test]
    fn a_looping_clip_is_continuous_across_the_seam() {
        let keys = keys(&[0.0, 0.6]);
        let length = 1.0;

        let before = first_axis(sample_key(&keys, length - 1e-4, length, true));
        let after = first_axis(sample_key(&keys, 0.0, length, true));

        assert!(
            (before - after).abs() < 0.01,
            "the seam jumped: {before} then {after}"
        );
    }

    /// A change of *kind* — a foot that was pinned to the floor and is now at
    /// a height — has no halfway, so it switches on arrival. Written down
    /// because interpolating towards `Ground` looks like something that ought
    /// to work and cannot: there is no blend between a constraint and a value.
    #[test]
    fn a_change_of_kind_switches_rather_than_blending() {
        let keys = vec![
            Key { at: 0.0, to: [Axis::Free, Axis::ON_THE_GROUND, Axis::Free], ease: Interpolation::Linear },
            Key { at: 1.0, to: [Axis::Free, Axis::Value(0.5), Axis::Free], ease: Interpolation::Linear },
        ];
        assert_eq!(sample_key(&keys, 0.5, 1.0, false).unwrap()[1], Axis::ON_THE_GROUND);
        assert_eq!(sample_key(&keys, 1.0, 1.0, false).unwrap()[1], Axis::Value(0.5));
    }

    /// Two keys stacked at one time is a legal way to write a snap. Dividing
    /// by the gap between them is not.
    #[test]
    fn two_keys_at_one_time_do_not_divide_by_zero() {
        let keys = keys(&[0.5, 0.5]);
        let sampled = first_axis(sample_key(&keys, 0.5, 1.0, false));
        assert!(sampled.is_finite(), "a zero-length segment produced {sampled}");
    }

    /// A clip authored in leg-lengths has to mean the same fraction of every
    /// build, which is the entire reason a key holds a target rather than a
    /// value. In metres it would mean the same distance and therefore a
    /// different pose on everybody.
    #[test]
    fn a_unit_is_a_fraction_of_the_body_it_is_on() {
        let rig = default_humanoid();
        assert_eq!(metres_per(Unit::Metres, rig), 1.0);
        assert!((metres_per(Unit::LegLengths, rig) - ik::leg_length(rig)).abs() < 1e-6);
        assert!(metres_per(Unit::HipHeights, rig) > 0.0);
        assert!(metres_per(Unit::ArmLengths, rig) > 0.0);
    }

    /// The feature, tested directly: the same clip over two floor heights puts
    /// the foot on each. This is what "don't hit a height, hit the floor"
    /// means, and the whole reason a key holds a target rather than a value.
    ///
    /// The clearance is the ankle's own resting height, because a leg chain's
    /// effector is the ankle and the ankle is never on the floor — the sole
    /// is. A bare floor height is not merely lower, it is out of the leg's
    /// reach, which is how this test found that `Ground` needed an offset.
    #[test]
    fn a_ground_axis_lands_on_the_floor_under_the_body() {
        let rig = default_humanoid().clone();
        let scale = metres_per(Unit::LegLengths, &rig);

        let resting = chain_tip(&rig, &Pose::rest(), &Transform::IDENTITY, &ik::RIGHT_LEG)
            .expect("the rig has a right leg");
        let clearance = resting.y / scale;

        let clip = Clip {
            length: 1.0,
            clock: Clock::Seconds,
            tracks: vec![Track {
                combine: Combine::Replace,
                space: Space::Body,
                unit: Unit::LegLengths,
                enabled: true,
                timing: Default::default(),
                channel: Channel::Effector {
                    chain: ChainRef::RightLeg,
                    pole: None,
                    keys: vec![Key {
                        at: 0.0,
                        to: [Axis::Free, Axis::Ground { clearance }, Axis::Free],
                        ease: Interpolation::Linear,
                    }],
                },
            }],
            ..Clip::default()
        };

        for floor in [0.0_f32, 0.1, 0.2] {
            let root = Transform::IDENTITY;
            let ground = move |_: Vec3| floor;
            let frames = Frames::standing(root, Transform::IDENTITY, &ground);

            let mut pose = Pose::rest();
            apply(&clip, 0.0, &rig, &mut pose, &frames, None);

            let ankle = chain_tip(&rig, &pose, &root, &ik::RIGHT_LEG).expect("the rig has a leg");
            let wanted = floor + clearance * scale;
            assert!(
                (ankle.y - wanted).abs() < 1e-3,
                "with the floor at {floor} the ankle wanted {wanted} and landed at {}",
                ankle.y
            );
        }

        // A floor further down than the leg is long is not a target to stretch
        // for: `solve` straightens and says `Short`, which is a body that
        // should be falling rather than one with a rubber leg. A clip cannot
        // ask for something the build cannot do, and this is what happens when
        // it tries.
        let ground = |_: Vec3| -0.5;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let mut pose = Pose::rest();
        let reaches = apply(&clip, 0.0, &rig, &mut pose, &frames, None);
        assert_eq!(reaches, vec![Reach::Short], "a floor out of reach was reached");
    }

    /// All three axes free is a skip, not a solve onto where the tip already
    /// is. `solve` re-derives the roll from its hinge, so re-solving an
    /// unchanged target silently rewrites three joints — a drift that shows up
    /// as a limb slowly rolling over while a clip that says nothing plays.
    #[test]
    fn a_wholly_free_track_leaves_the_pose_exactly_alone() {
        let rig = default_humanoid().clone();
        let clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Replace,
                space: Space::Body,
                unit: Unit::Metres,
                enabled: true,
                timing: Default::default(),
                channel: Channel::Effector {
                    chain: ChainRef::RightArm,
                    pole: None,
                    keys: vec![Key {
                        at: 0.0,
                        to: [Axis::Free; 3],
                        ease: Interpolation::Linear,
                    }],
                },
            }],
            ..Clip::default()
        };

        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);

        let before = Pose::rest().with(bone::UPPER_ARM_R, Quat::from_rotation_x(0.3));
        let mut pose = before.clone();
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        // Exactly, not nearly: nothing ran at all. A partially-free track is
        // the one that drifts, and it gets its own tolerance where it is used.
        assert_eq!(
            pose.joint(bone::UPPER_ARM_R),
            before.joint(bone::UPPER_ARM_R),
            "a wholly free track moved a joint"
        );
    }

    /// **A rotation key can say nothing about an axis**, and that axis keeps
    /// whatever the layer beneath put there.
    ///
    /// The After Effects reading, and the reason every key is three optional
    /// axes rather than three numbers: a key that turns a bone about one axis
    /// should not quietly zero the other two. Written against `Replace`,
    /// where "leave it" means reading the angle back out of what is already
    /// there.
    #[test]
    fn a_rotation_key_that_frees_an_axis_leaves_it_alone() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);

        let mut clip = Clip {
            length: 1.0,
            tracks: vec![Track::rotation(bone::CHEST)],
            ..Clip::default()
        };
        let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else { unreachable!() };
        // Pitch only; yaw and roll are nobody's business here.
        keys.push(Key {
            at: 0.0,
            to: [Axis::Value(0.3), Axis::Free, Axis::Free],
            ease: Interpolation::Linear,
        });

        // Something underneath with an opinion about all three.
        let beneath = quat_from_euler(Vec3::new(-0.9, 0.4, -0.25));
        let mut pose = Pose::rest().with(bone::CHEST, beneath);
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        let after = crate::common::rotation::euler_from_quat(pose.joint(bone::CHEST));
        assert!((after.x - 0.3).abs() < 1e-4, "the keyed axis did not take: {after:?}");
        assert!((after.y - 0.4).abs() < 1e-4, "a free axis was overwritten: {after:?}");
        assert!((after.z + 0.25).abs() < 1e-4, "a free axis was overwritten: {after:?}");
    }

    /// Composed, a free axis turns the bone by **nothing** rather than by what
    /// was already there.
    ///
    /// The two readings differ and both look reasonable until you write them
    /// down: under a multiply, "keep what is beneath" would compose the
    /// existing angle onto itself and double it. Saying nothing has to mean
    /// contributing nothing.
    #[test]
    fn a_composed_rotation_key_turns_a_free_axis_by_nothing() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);

        let mut clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Compose,
                ..Track::rotation(bone::CHEST)
            }],
            ..Clip::default()
        };
        let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else { unreachable!() };
        keys.push(Key {
            at: 0.0,
            to: [Axis::Value(0.2), Axis::Free, Axis::Free],
            ease: Interpolation::Linear,
        });

        let beneath = quat_from_euler(Vec3::new(0.0, 0.5, 0.0));
        let mut pose = Pose::rest().with(bone::CHEST, beneath);
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        let wanted = beneath * quat_from_euler(Vec3::new(0.2, 0.0, 0.0));
        assert!(
            pose.joint(bone::CHEST).abs_diff_eq(wanted, 1e-5),
            "a free axis contributed a turn of its own",
        );
    }

    /// A key on a track with nothing on it says nothing either, so keying at
    /// the playhead on a fresh track cannot move a body.
    #[test]
    fn keying_an_empty_track_changes_no_pose() {
        let mut channel = Channel::Rotation {
            bone: bone::CHEST.to_string(),
            keys: Vec::new(),
        };
        key_the_channel(&mut channel, 0.5, 1.0, false);

        let Channel::Rotation { keys, .. } = &channel else { unreachable!() };
        assert_eq!(keys[0].to, [Axis::Free; 3], "a fresh key had an opinion");
    }

    /// A disabled track is off. It is what anybody debugging a layered thing
    /// reaches for first, so it had better actually do nothing.
    #[test]
    fn a_disabled_track_does_nothing() {
        let rig = default_humanoid().clone();
        let mut clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                enabled: false,
                timing: Default::default(),
                ..Track::rotation(bone::CHEST)
            }],
            ..Clip::default()
        };
        let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else {
            unreachable!()
        };
        keys.push(Key::all(0.0, [0.5, 0.0, 0.0], Interpolation::Linear));

        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let mut pose = Pose::rest();
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        assert_eq!(pose.joint(bone::CHEST), Quat::IDENTITY);
    }

    /// `Compose` multiplies onto the layer beneath rather than replacing it,
    /// which is what lets a recoil run over whatever the body was doing.
    #[test]
    fn compose_builds_on_what_was_there_and_replace_does_not() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let beneath = Quat::from_rotation_x(0.2);
        let turn = [0.3_f32, 0.0, 0.0];

        let build = |combine| {
            let mut clip = Clip { length: 1.0, tracks: vec![Track { combine, ..Track::rotation(bone::CHEST) }], ..Clip::default() };
            let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else { unreachable!() };
            keys.push(Key::all(0.0, turn, Interpolation::Linear));
            clip
        };

        let mut replaced = Pose::rest().with(bone::CHEST, beneath);
        apply(&build(Combine::Replace), 0.0, &rig, &mut replaced, &frames, None);

        let mut composed = Pose::rest().with(bone::CHEST, beneath);
        apply(&build(Combine::Compose), 0.0, &rig, &mut composed, &frames, None);

        let authored = quat_from_euler(Vec3::from_array(turn));
        assert!(replaced.joint(bone::CHEST).abs_diff_eq(authored, 1e-5));
        assert!(composed.joint(bone::CHEST).abs_diff_eq(beneath * authored, 1e-5));
    }

    /// A bone the rig has not got is dropped and the rest of the clip plays.
    /// Nothing refuses — the rule `prop::feature`'s evaluator already keeps —
    /// because a clip that blanked a body over one mistyped name would be
    /// unusable while it was being written.
    #[test]
    fn a_clip_naming_an_unknown_bone_still_plays_its_other_tracks() {
        let rig = default_humanoid().clone();
        let mut clip = Clip {
            length: 1.0,
            tracks: vec![Track::rotation("elbow.q"), Track::rotation(bone::CHEST)],
            ..Clip::default()
        };
        for track in &mut clip.tracks {
            let Channel::Rotation { keys, .. } = &mut track.channel else { unreachable!() };
            keys.push(Key::all(0.0, [0.4, 0.0, 0.0], Interpolation::Linear));
        }

        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let mut pose = Pose::rest();
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        assert_ne!(pose.joint(bone::CHEST), Quat::IDENTITY, "the good track did not play");
    }

    /// A weapon track moves the weapon and nothing else. This is the whole
    /// arrangement a reload rests on: the arms are solved onto wherever the
    /// weapon ended up, afterwards, by somebody else.
    #[test]
    fn a_weapon_track_moves_the_weapon() {
        let rig = default_humanoid().clone();
        let clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Compose,
                space: Space::Carry,
                unit: Unit::Metres,
                enabled: true,
                timing: Default::default(),
                channel: Channel::Weapon {
                    keys: vec![Key {
                        at: 0.0,
                        to: [Axis::Free, Axis::Value(-0.2), Axis::Free],
                        ease: Interpolation::Linear,
                    }],
                },
            }],
            ..Clip::default()
        };

        let ground = |_: Vec3| 0.0;
        let frames = Frames {
            body: Transform::IDENTITY,
            carry: Transform::IDENTITY,
            weapon: Some(Transform::IDENTITY),
            eye: None,
            ground: &ground,
            footfalls: None,
        };

        let mut placed = Transform::IDENTITY;
        let mut pose = Pose::rest();
        apply(&clip, 0.0, &rig, &mut pose, &frames, Some(&mut placed));

        assert!((placed.translation.y + 0.2).abs() < 1e-5, "the weapon did not dip: {placed:?}");
    }

    /// A body with nothing in its hands skips weapon tracks rather than
    /// writing to a frame that is not there.
    #[test]
    fn a_weapon_track_on_an_empty_hand_does_nothing() {
        let rig = default_humanoid().clone();
        let clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Compose,
                space: Space::Weapon,
                unit: Unit::Metres,
                enabled: true,
                timing: Default::default(),
                channel: Channel::Weapon {
                    keys: vec![Key { at: 0.0, to: [Axis::Value(1.0); 3], ease: Interpolation::Linear }],
                },
            }],
            ..Clip::default()
        };

        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let mut pose = Pose::rest();
        // The point is that this does not panic and writes nothing.
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);
        assert!(pose.posed_joints().next().is_none());
    }

    /// Keying at the playhead pins what is already happening. A key that
    /// landed on a default would jump the body the moment it was added, which
    /// makes keying unusable for the thing it is for: fixing something you
    /// like before changing what comes after it.
    #[test]
    fn inserting_a_key_holds_what_the_track_already_said() {
        let mut channel = Channel::Weapon {
            keys: vec![
                Key { at: 0.0, to: [Axis::Value(0.0), Axis::Free, Axis::Free], ease: Interpolation::Linear },
                Key { at: 1.0, to: [Axis::Value(1.0), Axis::Free, Axis::Free], ease: Interpolation::Linear },
            ],
        };

        let before = sample_key(
            match &channel { Channel::Weapon { keys } => keys, _ => unreachable!() },
            0.25, 1.0, false,
        );

        let landed = key_the_channel(&mut channel, 0.25, 1.0, false);

        let Channel::Weapon { keys } = &channel else { unreachable!() };
        assert_eq!(keys.len(), 3, "the key was not added");
        assert_eq!(keys[landed].at, 0.25, "the key did not land where it was asked for");
        assert_eq!(
            Some(keys[landed].to), before,
            "adding a key changed what the track said at that moment",
        );
    }

    /// And the track still samples the same everywhere else, not just at the
    /// key. A key that pinned its own moment while bending the curve either
    /// side of it would be worse than one that jumped.
    #[test]
    fn inserting_a_key_does_not_bend_the_curve_around_it() {
        let keys = vec![
            Key { at: 0.0, to: [Axis::Value(0.0), Axis::Free, Axis::Free], ease: Interpolation::Linear },
            Key { at: 1.0, to: [Axis::Value(1.0), Axis::Free, Axis::Free], ease: Interpolation::Linear },
        ];
        let before: Vec<f32> = (0..=20)
            .map(|step| {
                sample_key(&keys, step as f32 / 20.0, 1.0, false).unwrap()[0].value().unwrap()
            })
            .collect();

        let mut channel = Channel::Weapon { keys };
        key_the_channel(&mut channel, 0.25, 1.0, false);
        let Channel::Weapon { keys } = &channel else { unreachable!() };

        for (step, was) in before.iter().enumerate() {
            let now = sample_key(keys, step as f32 / 20.0, 1.0, false).unwrap()[0]
                .value()
                .unwrap();
            assert!(
                (now - was).abs() < 1e-5,
                "adding a key moved the curve at {}: {was} became {now}",
                step as f32 / 20.0,
            );
        }
    }

    /// A layer at half weight is half of itself, and at zero it is not there.
    #[test]
    fn a_layer_shows_as_much_of_itself_as_its_weight() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);

        let mut clip = Clip { length: 1.0, tracks: vec![Track::rotation(bone::CHEST)], ..Clip::default() };
        let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else { unreachable!() };
        keys.push(Key::all(0.0, [0.6, 0.0, 0.0], Interpolation::Linear));
        let clip = Arc::new(clip);

        let full = {
            let mut pose = Pose::rest();
            apply_stack(
                &ClipStack(vec![Playing { clip: clip.clone(), time: 0.0, weight: 1.0 }]),
                &rig, &mut pose, &frames, None,
            );
            pose.joint(bone::CHEST)
        };

        let half = {
            let mut pose = Pose::rest();
            apply_stack(
                &ClipStack(vec![Playing { clip: clip.clone(), time: 0.0, weight: 0.5 }]),
                &rig, &mut pose, &frames, None,
            );
            pose.joint(bone::CHEST)
        };

        let none = {
            let mut pose = Pose::rest();
            apply_stack(
                &ClipStack(vec![Playing { clip, time: 0.0, weight: 0.0 }]),
                &rig, &mut pose, &frames, None,
            );
            pose.joint(bone::CHEST)
        };

        assert_eq!(none, Quat::IDENTITY, "a weightless layer showed");
        assert!(half.angle_between(Quat::IDENTITY) < full.angle_between(Quat::IDENTITY));
        assert!(half.angle_between(Quat::IDENTITY) > 1e-3, "a half-weight layer did not show at all");
    }
}
