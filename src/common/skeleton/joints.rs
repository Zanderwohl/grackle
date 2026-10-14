//! How far a joint is allowed to bend.
//!
//! A [`Pose`](super::Pose) is unchecked rotations, since an animation's author
//! already knows a knee goes one way. A ragdoll solver is handed positions and
//! does not, so the limits live here. They are angles, so valid on every build.
//!
//! Every limit is in the bone's own rest frame (`+Y` along the bone, rest is
//! identity). The rig mirrors left and right rest rotations (see
//! `rig::bone_rotation`), so one entry covers both sides.
//!
//! - A cone (shoulder, neck) swings any direction up to an angle. Twist is not
//!   limited: a position solver cannot see it.
//! - A hinge (elbow, knee) has one axis and hard stops; off-axis motion is
//!   removed, which keeps a knee out of the other leg.
//!
//! Numbers are a human's rough range of motion in radians, not measured.

use bevy::prelude::*;

use super::rig::bone;

/// Stops in radians about the bone's local `+X`. Positive swings the tail
/// towards local `+Z`, which the rig points at the front of the body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hinge {
    pub min: f32,
    pub max: f32,
}

impl Hinge {
    /// The nearest stop measured around the circle, not `f32::clamp`: an
    /// `atan2` angle driven past a stop wraps across `±π`, and a plain clamp
    /// would then pick the opposite stop and snap the knee through straight.
    fn clamp(&self, angle: f32) -> f32 {
        if angle >= self.min && angle <= self.max {
            return angle;
        }
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
    /// Held at rest: the pelvis, which has no parent to bend against.
    Fixed,
}

impl JointLimit {
    /// The nearest direction this joint accepts, in the bone's rest frame. A
    /// direction already inside comes back unchanged, so this is a projection
    /// and safe to iterate.
    pub fn nearest(&self, direction: Vec3) -> Vec3 {
        let Some(direction) = direction.try_normalize() else {
            // A collapsed bone.
            return Vec3::Y;
        };

        match self {
            JointLimit::Fixed => Vec3::Y,
            JointLimit::Cone { radians } => {
                let angle = direction.y.clamp(-1.0, 1.0).acos();
                if angle <= *radians {
                    return direction;
                }
                // Straight down `-Y` has no swing axis; `+Z` brings the limb
                // forward.
                let axis = Vec3::Y
                    .cross(direction)
                    .try_normalize()
                    .unwrap_or(Vec3::Z);
                Quat::from_axis_angle(axis, *radians) * Vec3::Y
            }
            JointLimit::Hinge(hinge) => {
                let flat = Vec2::new(direction.y, direction.z);
                // Exactly along the hinge axis: no bend angle to read.
                let Some(flat) = flat.try_normalize() else { return Vec3::Y };

                // Rotating `+Y` about `+X` by `angle` gives `(0, cos, sin)`.
                let angle = hinge.clamp(flat.y.atan2(flat.x));
                Vec3::new(0.0, angle.cos(), angle.sin())
            }
        }
    }
}

const SHOULDER: JointLimit = JointLimit::Cone { radians: 1.9 };

/// Narrow enough that a corpse's legs do not fold up over the chest.
const HIP: JointLimit = JointLimit::Cone { radians: 1.3 };

/// The straight stop is just past rest, so rest sits inside the limit.
const ELBOW: JointLimit = JointLimit::Hinge(Hinge { min: -0.05, max: 2.6 });

const KNEE: JointLimit = JointLimit::Hinge(Hinge { min: -2.4, max: 0.05 });

/// What a named bone's joint will do. An unlisted bone gets a narrow cone, so
/// a missing entry is not a limb that bends anywhere.
pub fn limit_of(bone_name: &str) -> JointLimit {
    match bone_name {
        bone::PELVIS => JointLimit::Fixed,
        // Forward fold is wider than sideways, which a cone cannot say; this
        // takes the forward range, since doubled over reads as dead.
        bone::CHEST => JointLimit::Cone { radians: 0.9 },
        bone::NECK => JointLimit::Cone { radians: 0.7 },
        bone::HEAD => JointLimit::Cone { radians: 0.5 },
        // A shrug only; swinging the clavicle moves the whole arm off the body.
        bone::CLAVICLE_L | bone::CLAVICLE_R => JointLimit::Cone { radians: 0.35 },
        bone::UPPER_ARM_L | bone::UPPER_ARM_R => SHOULDER,
        bone::FOREARM_L | bone::FOREARM_R => ELBOW,
        // A wrist bends two ways, so a cone rather than a hinge.
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

    /// Otherwise every solve drags joints towards rest and a corpse stands up.
    #[test]
    fn a_cone_leaves_a_direction_it_allows_alone() {
        let limit = JointLimit::Cone { radians: 1.0 };
        let inside = Quat::from_rotation_z(0.5) * Vec3::Y;
        assert!(limit.nearest(inside).abs_diff_eq(inside, 1e-5));
    }

    /// Outside comes back to the edge in the same plane, not snapped to rest.
    #[test]
    fn a_cone_pulls_a_direction_back_to_its_edge() {
        let limit = JointLimit::Cone { radians: 1.0 };
        let outside = Quat::from_rotation_z(2.0) * Vec3::Y;
        let nearest = limit.nearest(outside);

        assert!((nearest.angle_between(Vec3::Y) - 1.0).abs() < 1e-4, "{nearest}");
        assert!(nearest.x.signum() == outside.x.signum(), "swung out of plane: {nearest}");
    }

    /// Otherwise the legs cross.
    #[test]
    fn a_hinge_removes_sideways_motion_rather_than_limiting_it() {
        let limit = JointLimit::Hinge(Hinge { min: -1.0, max: 1.0 });
        let sideways = Vec3::new(0.6, 0.8, 0.0).normalize();
        assert!(limit.nearest(sideways).x.abs() < 1e-5);
    }

    #[test]
    fn a_knee_does_not_bend_forwards() {
        let forwards = Vec3::new(0.0, 0.5, 0.87);
        let nearest = KNEE.nearest(forwards);

        assert!(nearest.z <= 0.05, "the knee bent forwards: {nearest}");
    }

    #[test]
    fn a_knee_bends_backwards_as_far_as_it_is_told_to() {
        let JointLimit::Hinge(hinge) = KNEE else { panic!("a knee is a hinge") };

        let folded = Quat::from_rotation_x(hinge.min * 0.9) * Vec3::Y;
        assert!(KNEE.nearest(folded).abs_diff_eq(folded, 1e-5), "the bend was refused");

        let past = Quat::from_rotation_x(hinge.min - 1.0) * Vec3::Y;
        let stopped = Quat::from_rotation_x(hinge.min) * Vec3::Y;
        assert!(KNEE.nearest(past).abs_diff_eq(stopped, 1e-4));
    }

    /// Catches elbow and knee being swapped.
    #[test]
    fn an_elbow_bends_the_opposite_way_to_a_knee() {
        let forwards = Vec3::new(0.0, 0.5, 0.87);
        let backwards = Vec3::new(0.0, 0.5, -0.87);

        assert!(ELBOW.nearest(forwards).z > 0.5, "an elbow must bend forwards");
        assert!(ELBOW.nearest(backwards).z <= 0.05, "an elbow bent backwards");
        assert!(KNEE.nearest(backwards).z < -0.5, "a knee must bend backwards");
    }

    /// A joint rejecting rest twitches the moment the solver takes the body.
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

    /// Limits are in the bone's own frame, so both sides share one entry.
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

    /// A new bone must not silently become the loosest joint on the body.
    #[test]
    fn an_unlisted_bone_is_not_left_free() {
        let JointLimit::Cone { radians } = limit_of("tail") else {
            panic!("the fallback should be a cone")
        };
        assert!(radians < 0.5, "the fallback cone is wide enough to hide a missing entry");
    }
}
