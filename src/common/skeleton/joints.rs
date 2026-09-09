//! How far a joint is allowed to bend.
//!
//! The rig itself has no opinion about this: a [`Pose`](super::Pose) is a set
//! of rotations and nothing checks them, because an animation is authored by
//! something that already knows a knee only goes one way. A ragdoll is the
//! first thing that *does not* know — it is handed positions by a solver and
//! will happily fold an elbow through a shoulder — so the knowledge has to be
//! written down somewhere, and here beside the rig is the only place it can be
//! stated once for every body. Limits are angles, so like a pose they are
//! valid on every build: a heavy's elbow and a scout's stop at the same place.
//!
//! Every limit is expressed **in the bone's own rest frame**, which is the
//! frame in which `+Y` is the bone and rest is the identity. That is what
//! makes one table cover both sides of the body without a mirror step: the rig
//! builds left and right bones with mirrored rest rotations, so "bends
//! forward" is the same local rotation on both arms. See
//! `rig::bone_rotation` for why those frames come
//! out mirrored rather than arbitrary.
//!
//! Two shapes of joint, and the difference is not decoration:
//!
//! - A **cone** is a shoulder or a neck: free to swing in any direction, up to
//!   some angle away from rest. Twist is not limited at all, because nothing
//!   here has a twist to limit — a solver that works in positions cannot tell
//!   a rolled forearm from an unrolled one.
//! - A **hinge** is an elbow or a knee: one axis, one direction, hard stops.
//!   Off-axis motion is removed outright rather than merely limited, which is
//!   also what stops a knee from being pulled sideways into the other leg.
//!
//! Numbers are a human's rough range of motion in radians, rounded. They are
//! not measured, and a class with a different build does not get different
//! ones — that is a per-class table when there is a class that needs it.

use bevy::prelude::*;

use super::rig::bone;

/// Which way a hinge bends, and how far, in radians about the bone's local
/// `+X`.
///
/// **Positive is forwards**, in the direction the body faces. The rig points
/// every bone's local `+Z` along world forward wherever that is meaningful, so
/// a positive rotation about local `+X` swings the bone's tail towards `+Z`
/// and therefore towards the front of the body — on both sides, on every
/// build.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hinge {
    pub min: f32,
    pub max: f32,
}

impl Hinge {
    /// The nearest angle inside the stops, going the short way round.
    ///
    /// Not `f32::clamp`, and the difference is the whole of a knee: an angle
    /// read back out of a direction comes from `atan2` and so lives in
    /// `-π..=π`, while a knee driven hard past its stop is at more than a
    /// half-turn from rest and wraps to the far side of that range. Clamped
    /// naively it lands against the *opposite* stop, and the leg snaps
    /// through straight and out the front.
    fn clamp(&self, angle: f32) -> f32 {
        if angle >= self.min && angle <= self.max {
            return angle;
        }
        // Whichever stop is the shorter turn away, measured around the circle
        // rather than along the number line.
        if turn_to(angle, self.min).abs() <= turn_to(angle, self.max).abs() {
            self.min
        } else {
            self.max
        }
    }
}

/// The signed short way round from `from` to `to`, in `-π..=π`.
fn turn_to(from: f32, to: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (to - from + PI).rem_euclid(TAU) - PI
}

/// What a joint will let its bone do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JointLimit {
    /// Anywhere within `radians` of rest, in any direction.
    Cone { radians: f32 },
    /// One axis only, between two stops.
    Hinge(Hinge),
    /// Held at rest. The pelvis, which has no parent to bend against.
    Fixed,
}

impl JointLimit {
    /// The nearest direction this joint will accept, given one it has been
    /// handed.
    ///
    /// `direction` and the answer are both in the bone's rest frame, where
    /// `+Y` is the bone at rest. A direction already inside the limit comes
    /// back unchanged, which is what lets a caller treat this as a projection
    /// and iterate it.
    pub fn nearest(&self, direction: Vec3) -> Vec3 {
        let Some(direction) = direction.try_normalize() else {
            // Nothing to point along — a zero-length bone, or a solve that has
            // collapsed one. Rest is the only answer available.
            return Vec3::Y;
        };

        match self {
            JointLimit::Fixed => Vec3::Y,
            JointLimit::Cone { radians } => {
                let angle = direction.y.clamp(-1.0, 1.0).acos();
                if angle <= *radians {
                    return direction;
                }
                // Swing back towards rest about the axis the two share. A
                // direction pointing straight down the bone's own negative Y
                // has no such axis; any perpendicular will do, and `+Z` is
                // forward, so a limb folded exactly backwards comes forward.
                let axis = Vec3::Y
                    .cross(direction)
                    .try_normalize()
                    .unwrap_or(Vec3::Z);
                Quat::from_axis_angle(axis, *radians) * Vec3::Y
            }
            JointLimit::Hinge(hinge) => {
                // Flatten onto the hinge plane first: an elbow has no sideways
                // freedom at all, so the off-axis component is removed rather
                // than clamped to something small.
                let flat = Vec2::new(direction.y, direction.z);
                // Pulled exactly along the hinge axis, so there is no bend
                // angle to read. Rest is the nearest thing on the plane.
                let Some(flat) = flat.try_normalize() else { return Vec3::Y };

                // Rotating `+Y` about `+X` by `angle` gives `(0, cos, sin)`,
                // so this is that rotation read back off the direction.
                let angle = hinge.clamp(flat.y.atan2(flat.x));
                Vec3::new(0.0, angle.cos(), angle.sin())
            }
        }
    }
}

/// A shoulder: the widest joint on the body, and the reason the rest pose is
/// an A-pose rather than a T — half of this range is spent getting to a T.
const SHOULDER: JointLimit = JointLimit::Cone { radians: 1.9 };

/// A hip. Narrower than a shoulder, and narrow enough that a corpse's legs
/// stay under it rather than folding up over the chest.
const HIP: JointLimit = JointLimit::Cone { radians: 1.3 };

/// An elbow bends forwards and stops just short of straight, so an arm at rest
/// sits against its stop rather than a hair inside it.
const ELBOW: JointLimit = JointLimit::Hinge(Hinge { min: -0.05, max: 2.6 });

/// A knee is an elbow the other way round.
const KNEE: JointLimit = JointLimit::Hinge(Hinge { min: -2.4, max: 0.05 });

/// What a named bone's joint will do.
///
/// A bone the table says nothing about gets a narrow cone rather than free
/// rotation: an unlisted joint is one nobody has thought about, and the
/// failure that hides a missing entry is a limb that bends anywhere.
pub fn limit_of(bone_name: &str) -> JointLimit {
    match bone_name {
        bone::PELVIS => JointLimit::Fixed,
        // The spine break is low and real: a chest folds forward over the hips
        // a long way and sideways much less, which a cone cannot say. The
        // wider of the two, since a body doubled over is the pose that reads
        // as dead.
        bone::CHEST => JointLimit::Cone { radians: 0.9 },
        bone::NECK => JointLimit::Cone { radians: 0.7 },
        // The head is on the end of the neck and does most of its moving
        // through it. Enough to loll, not enough to look backwards.
        bone::HEAD => JointLimit::Cone { radians: 0.5 },
        // A shrug, and nothing else. The clavicle is what absorbs a wide
        // chest's shoulder placement, so letting it swing would move the whole
        // arm off the body.
        bone::CLAVICLE_L | bone::CLAVICLE_R => JointLimit::Cone { radians: 0.35 },
        bone::UPPER_ARM_L | bone::UPPER_ARM_R => SHOULDER,
        bone::FOREARM_L | bone::FOREARM_R => ELBOW,
        // A wrist is a small cone rather than a hinge: it bends two ways, and
        // at a hand's size the difference does not read.
        bone::HAND_L | bone::HAND_R => JointLimit::Cone { radians: 0.8 },
        bone::THIGH_L | bone::THIGH_R => HIP,
        bone::SHIN_L | bone::SHIN_R => KNEE,
        bone::FOOT_L | bone::FOOT_R => JointLimit::Cone { radians: 0.6 },
        _ => JointLimit::Cone { radians: 0.3 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::rig::{humanoid, Proportions};

    /// A direction already inside a cone is not touched. Without this the
    /// limits would drag every joint towards rest a little on every solve,
    /// which reads as a corpse slowly standing back up.
    #[test]
    fn a_cone_leaves_a_direction_it_allows_alone() {
        let limit = JointLimit::Cone { radians: 1.0 };
        let inside = Quat::from_rotation_z(0.5) * Vec3::Y;
        assert!(limit.nearest(inside).abs_diff_eq(inside, 1e-5));
    }

    /// And one outside comes back at exactly the limit, in the same plane it
    /// went out in — swung back, not snapped to rest.
    #[test]
    fn a_cone_pulls_a_direction_back_to_its_edge() {
        let limit = JointLimit::Cone { radians: 1.0 };
        let outside = Quat::from_rotation_z(2.0) * Vec3::Y;
        let nearest = limit.nearest(outside);

        assert!((nearest.angle_between(Vec3::Y) - 1.0).abs() < 1e-4, "{nearest}");
        // Same plane: the sideways component keeps its sign.
        assert!(nearest.x.signum() == outside.x.signum(), "swung out of plane: {nearest}");
    }

    /// A hinge has no sideways freedom at all. A knee pulled towards its own
    /// other leg has to come back onto the plane, or the legs cross.
    #[test]
    fn a_hinge_removes_sideways_motion_rather_than_limiting_it() {
        let limit = JointLimit::Hinge(Hinge { min: -1.0, max: 1.0 });
        let sideways = Vec3::new(0.6, 0.8, 0.0).normalize();
        assert!(limit.nearest(sideways).x.abs() < 1e-5);
    }

    /// The whole point of a hinge: the wrong way is not a smaller bend, it is
    /// no bend.
    #[test]
    fn a_knee_does_not_bend_forwards() {
        // Tail swung well forward — local `+Z` is the front of the body.
        let forwards = Vec3::new(0.0, 0.5, 0.87);
        let nearest = KNEE.nearest(forwards);

        assert!(nearest.z <= 0.05, "the knee bent forwards: {nearest}");
    }

    /// And the right way is allowed all the way to a heel against a backside.
    #[test]
    fn a_knee_bends_backwards_as_far_as_it_is_told_to() {
        let JointLimit::Hinge(hinge) = KNEE else { panic!("a knee is a hinge") };

        let folded = Quat::from_rotation_x(hinge.min * 0.9) * Vec3::Y;
        assert!(KNEE.nearest(folded).abs_diff_eq(folded, 1e-5), "the bend was refused");

        // Past the stop, and held at it.
        let past = Quat::from_rotation_x(hinge.min - 1.0) * Vec3::Y;
        let stopped = Quat::from_rotation_x(hinge.min) * Vec3::Y;
        assert!(KNEE.nearest(past).abs_diff_eq(stopped, 1e-4));
    }

    /// An elbow is a knee the other way round, and this is the assertion that
    /// catches the two being swapped — which is invisible in a still frame and
    /// unmistakable the moment a body falls over.
    #[test]
    fn an_elbow_bends_the_opposite_way_to_a_knee() {
        let forwards = Vec3::new(0.0, 0.5, 0.87);
        let backwards = Vec3::new(0.0, 0.5, -0.87);

        assert!(ELBOW.nearest(forwards).z > 0.5, "an elbow must bend forwards");
        assert!(ELBOW.nearest(backwards).z <= 0.05, "an elbow bent backwards");
        assert!(KNEE.nearest(backwards).z < -0.5, "a knee must bend backwards");
    }

    /// Rest is inside every limit. A joint that rejected its own rest pose
    /// would make a body twitch the instant it was handed to the solver, and
    /// the hinges are the ones at risk: their stops sit right against rest.
    #[test]
    fn every_joint_accepts_the_pose_it_starts_in() {
        for bone in humanoid(Proportions::DEFAULT).bones() {
            let nearest = limit_of(bone.name).nearest(Vec3::Y);
            assert!(
                nearest.abs_diff_eq(Vec3::Y, 1e-5),
                "{} moves at rest, to {nearest}",
                bone.name
            );
        }
    }

    /// The mirror check, and the reason the table has one entry per pair
    /// rather than one per bone: a limit is stated in the bone's own frame, so
    /// both sides get the same entry and still bend the same way in the world.
    #[test]
    fn the_two_sides_of_the_body_are_limited_alike() {
        let skeleton = humanoid(Proportions::DEFAULT);
        for bone in skeleton.bones() {
            let Some(other) = bone.name.strip_suffix(".l") else { continue };
            let mirrored = format!("{other}.r");
            assert_eq!(
                limit_of(bone.name),
                limit_of(&mirrored),
                "{} and {mirrored} are limited differently",
                bone.name
            );
        }
    }

    /// A bone nobody has written an entry for is pinned, not free. The failure
    /// this guards is a new bone silently becoming the loosest joint on the
    /// body.
    #[test]
    fn an_unlisted_bone_is_not_left_free() {
        let JointLimit::Cone { radians } = limit_of("tail") else {
            panic!("the fallback should be a cone")
        };
        assert!(radians < 0.5, "the fallback cone is wide enough to hide a missing entry");
    }
}
