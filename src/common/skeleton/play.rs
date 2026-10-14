//! Turning a [`Clip`] into motion.
//!
//! - [`sample_at`] is pure and stateless (no cursor), so a backwards scrub, a
//!   body in a match and a rollback replay all make the same call.
//! - [`apply`] turns a sample into a pose. It is handed the clip's spaces in
//!   [`Frames`] rather than looking them up, so a clip samples in a unit test
//!   with no `World`.
//!
//! Every sample returns an `Option`, and `None` (or [`Axis::Free`] on one axis)
//! leaves the layer beneath alone, as absence does in a [`Pose`].

use std::sync::Arc;

use bevy::prelude::*;

use super::clip::{Axis, ChainRef, Channel, Clip, Combine, Key, Space, Unit};
use super::ik::{self, Chain, Reach};
use super::interpolation::Interpolation;
use super::rig::{Pose, Skeleton};
use crate::common::rotation::quat_from_euler;

/// The value a keyed channel holds at `time`, or `None` with no keys. `time`
/// must already be through [`Clip::wrap`].
///
/// Outside the keys a one-shot holds its end values, so a finished reload does
/// not snap back to the hold. A loop blends last to first across the seam,
/// over `(length - last.at) + first.at`. The outgoing key's curve governs each
/// segment, the seam's included.
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

    // The wrap-around segment's length; only meaningful while looping.
    let seam = (length - at(last)) + at(first);

    if time <= at(first) {
        if !looping {
            return Some(value(first));
        }
        // The tail of the seam, past `length` and back round to zero.
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

    // Linear scan: a track has tens of keys.
    for pair in keys.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        if time < at(from) || time > at(to) {
            continue;
        }
        let span = at(to) - at(from);
        // Two keys at one time is a legal snap; do not divide by zero.
        let along = if span > 0.0 { (time - at(from)) / span } else { 0.0 };
        return Some(blend(value(from), value(to), ease(from), along));
    }

    Some(value(last))
}

/// Sample a track, each axis eased independently, so a key like
/// `[0.2, "ground", "free"]` mixes kinds.
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
                    (Axis::Value(a), Axis::Value(b)) => Axis::Value(ease.f32(a, b, along)),
                    // Stays relative to the floor throughout a lift.
                    (Axis::Ground { clearance: a }, Axis::Ground { clearance: b }) => {
                        Axis::Ground { clearance: ease.f32(a, b, along) }
                    }
                    // A change of kind has no halfway; switch on arrival.
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

/// Euler angles for a `Replace` key, with `Free` axes read back from
/// `beneath`. Decomposition is in [`EULER_ORDER`]; near gimbal lock it is not
/// unique, so a key that needs certainty should set all three.
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

/// Euler angles for a `Compose` key. `Free` is zero here: reading the angle
/// beneath and multiplying it on would double it.
fn angles_onto(to: [Axis; 3]) -> Vec3 {
    let mut wanted = Vec3::ZERO;
    for axis in 0..3 {
        if let Axis::Value(at) = to[axis] {
            wanted[axis] = at;
        }
    }
    wanted
}

/// Insert a key at `at` holding the value the track already has there, so
/// keying never moves the pose; an empty track gets three free axes. Returns
/// the new key's index.
pub fn key_the_channel(channel: &mut Channel, at: f32, length: f32, looping: bool) -> usize {
    let at = at.clamp(0.0, length);
    let keys = channel.keys_mut();

    let to = sample_key(keys, at, length, looping).unwrap_or([Axis::Free; 3]);
    let ease = keys.last().map(|key| key.ease).unwrap_or_default();
    keys.push(Key { at, to, ease });
    keys.sort_by(|a, b| a.at.total_cmp(&b.at));
    keys.iter().position(|key| key.at == at).unwrap_or(0)
}

/// How many metres one of `unit` is on this body.
pub fn metres_per(unit: Unit, skeleton: &Skeleton) -> f32 {
    let proportions = skeleton.proportions();
    match unit {
        Unit::Metres => 1.0,
        Unit::LegLengths => ik::leg_length(skeleton),
        Unit::ArmLengths => proportions.arm_metres(),
        Unit::HipHeights => proportions.hip_metres(),
    }
}

/// What the spaces a clip names are, this frame, for this body. Filled by the
/// caller, since `prop::hold` and `game::collision` sit above this module.
pub struct Frames<'a> {
    /// The `root` [`ik::solve`] takes: feet on the floor, facing forward.
    pub body: Transform,
    /// The point between the shoulders, turned by the aim.
    pub carry: Transform,
    /// `None` for an empty hand; tracks in this space are then skipped.
    pub weapon: Option<Transform>,
    /// Eye position and look direction, for a viewmodel clip.
    pub eye: Option<Transform>,
    /// Floor height under a point. A closure so tests and the prop editor can
    /// answer with a plane and the game with a ray.
    pub ground: &'a dyn Fn(Vec3) -> f32,
    /// Each foot's phase in its own cycle, left first; see
    /// [`Timing`](super::clip::Timing). `None` puts the right foot half a
    /// stride after the left.
    pub footfalls: Option<[f32; 2]>,
}

impl Frames<'_> {
    /// Nothing in hand, no eye, no gait.
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
#[derive(Clone, Debug)]
pub struct Playing {
    pub clip: Arc<Clip>,
    /// On the clip's own clock.
    pub time: f32,
    /// `0..=1`.
    pub weight: f32,
}

/// Everything playing on this body, bottom layer first.
#[derive(Component, Clone, Debug, Default)]
pub struct ClipStack(pub Vec<Playing>);

/// Where the end of a chain currently is, in world space.
fn chain_tip(skeleton: &Skeleton, pose: &Pose, root: &Transform, chain: &Chain) -> Option<Vec3> {
    // `solve` puts the lower bone's tail on its target.
    let lower = skeleton.index_of(chain.lower)?;
    Some(skeleton.posed_bones(pose, root)[lower].tail)
}

/// Resolve a sampled target to a world point. `tip` is read back into the
/// track's space and unit so `Free` axes keep it.
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
                Combine::Compose => local[axis] + at,
            },
            // Set in world space below; a height through a turned frame is not
            // a height.
            Axis::Ground { .. } => local[axis],
        };
    }

    let mut target = frame.transform_point(wanted * scale);
    if let Axis::Ground { clearance } = to[1] {
        target.y = ground(target) + clearance * scale;
    }
    target
}

/// A track's time given its clip's. A foot-timed track keeps the whole strides
/// and swaps the fraction for its foot's phase.
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

/// Apply `clip` at `time` to `pose`, in place. `weapon` is moved by weapon
/// tracks; the arms are solved onto it later by `grip_the_weapon`, so no hand
/// is keyed. Returns each effector track's [`Reach`], in track order.
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
                // `Pose::set` needs the rig's `&'static str`. An unknown bone is
                // dropped here; `Clip::problems` reports it.
                let Some(index) = skeleton.index_of(bone) else {
                    continue;
                };
                let name = skeleton.bones()[index].name;
                let beneath = pose.joint(name);
                match track.combine {
                    Combine::Replace => {
                        pose.set(name, quat_from_euler(angles_for(to, beneath)))
                    }
                    Combine::Compose => {
                        pose.set(name, beneath * quat_from_euler(angles_onto(to)))
                    }
                }
            }

            Channel::Effector { chain, pole, keys } => {
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                // Skip rather than re-solve onto the current tip: `solve`
                // re-derives roll, so that is not the identity.
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
                // Authored and default poles alike are in the body's frame.
                let pole = frames.body.rotation
                    * pole.map(Vec3::from_array).unwrap_or_else(|| default_pole(*chain));

                // A short leg straightens and points; a short arm is left as
                // the animation had it. See `ik::reach_or_leave`.
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
                // Always hip-heights in the body frame; the track's space and
                // unit are ignored.
                for axis in 0..3 {
                    let was = pose.root_offset[axis];
                    pose.root_offset[axis] = match (to[axis], track.combine) {
                        (Axis::Free, _) => was,
                        (Axis::Value(at), Combine::Replace) => at,
                        (Axis::Value(at), Combine::Compose) => was + at,
                        // Meaningless here; `Clip::problems` says so.
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
                    // In the track's own space; free axes keep the hold's.
                    Combine::Replace => {
                        if let Some(frame) = frames.of(track.space) {
                            let beneath = frame.rotation.inverse() * placed.rotation;
                            placed.rotation =
                                frame.rotation * quat_from_euler(angles_for(to, beneath));
                        }
                    }
                    Combine::Compose => {
                        placed.rotation *= quat_from_euler(angles_onto(to));
                    }
                }
            }
        }
    }

    reaches
}

/// Which way a knee or an elbow points when a track does not say, in the
/// body's frame.
fn default_pole(chain: ChainRef) -> Vec3 {
    if chain.is_leg() {
        // Knees bend forwards, even in a sidestep.
        Vec3::NEG_Z
    } else {
        ik::elbow_pole(chain.side())
    }
}

/// Apply a whole stack, bottom layer first. A partial-weight layer is applied
/// to a copy and blended back with [`Pose::lerp`]; full weight skips the clone.
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
        // The weapon blends by weight too.
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

    /// A default from an empty track would overwrite the layer beneath.
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

    /// Extrapolating a one-shot puts a limb somewhere no key asked for.
    #[test]
    fn a_one_shot_holds_at_both_ends() {
        let keys = keys(&[0.25, 0.75]);
        assert_eq!(first_axis(sample_key(&keys, 0.0, 1.0, false)), 0.0);
        assert_eq!(first_axis(sample_key(&keys, 1.0, 1.0, false)), 1.0);
    }

    /// Rollback and backwards scrubbing ask for times out of order.
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

    /// A discontinuous seam makes a loop lurch.
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

    /// There is no blend between a `Ground` constraint and a value.
    #[test]
    fn a_change_of_kind_switches_rather_than_blending() {
        let keys = vec![
            Key { at: 0.0, to: [Axis::Free, Axis::ON_THE_GROUND, Axis::Free], ease: Interpolation::Linear },
            Key { at: 1.0, to: [Axis::Free, Axis::Value(0.5), Axis::Free], ease: Interpolation::Linear },
        ];
        assert_eq!(sample_key(&keys, 0.5, 1.0, false).unwrap()[1], Axis::ON_THE_GROUND);
        assert_eq!(sample_key(&keys, 1.0, 1.0, false).unwrap()[1], Axis::Value(0.5));
    }

    #[test]
    fn two_keys_at_one_time_do_not_divide_by_zero() {
        let keys = keys(&[0.5, 0.5]);
        let sampled = first_axis(sample_key(&keys, 0.5, 1.0, false));
        assert!(sampled.is_finite(), "a zero-length segment produced {sampled}");
    }

    #[test]
    fn a_unit_is_a_fraction_of_the_body_it_is_on() {
        let rig = default_humanoid();
        assert_eq!(metres_per(Unit::Metres, rig), 1.0);
        assert!((metres_per(Unit::LegLengths, rig) - ik::leg_length(rig)).abs() < 1e-6);
        assert!(metres_per(Unit::HipHeights, rig) > 0.0);
        assert!(metres_per(Unit::ArmLengths, rig) > 0.0);
    }

    /// One clip must land the foot on floors at different heights. The
    /// clearance is the ankle's resting height, since the effector is the
    /// ankle and a bare floor height is out of the leg's reach.
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

        // Out of reach: `Short`, not stretched.
        let ground = |_: Vec3| -0.5;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let mut pose = Pose::rest();
        let reaches = apply(&clip, 0.0, &rig, &mut pose, &frames, None);
        assert_eq!(reaches, vec![Reach::Short], "a floor out of reach was reached");
    }

    /// Re-solving onto an unchanged tip rewrites the roll, so a silent clip
    /// slowly rolls a limb over.
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

        // Exact: nothing should have run.
        assert_eq!(
            pose.joint(bone::UPPER_ARM_R),
            before.joint(bone::UPPER_ARM_R),
            "a wholly free track moved a joint"
        );
    }

    /// A `Replace` key turning one axis must not zero the other two.
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
        keys.push(Key {
            at: 0.0,
            to: [Axis::Value(0.3), Axis::Free, Axis::Free],
            ease: Interpolation::Linear,
        });

        let beneath = quat_from_euler(Vec3::new(-0.9, 0.4, -0.25));
        let mut pose = Pose::rest().with(bone::CHEST, beneath);
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        let after = crate::common::rotation::euler_from_quat(pose.joint(bone::CHEST));
        assert!((after.x - 0.3).abs() < 1e-4, "the keyed axis did not take: {after:?}");
        assert!((after.y - 0.4).abs() < 1e-4, "a free axis was overwritten: {after:?}");
        assert!((after.z + 0.25).abs() < 1e-4, "a free axis was overwritten: {after:?}");
    }

    /// Under `Compose`, keeping the angle beneath would double it.
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

    /// Keying a fresh track must not move a body.
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

    /// One mistyped bone must not blank the whole clip.
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
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);
        assert!(pose.posed_joints().next().is_none());
    }

    /// A key landing on a default would jump the body when added.
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
