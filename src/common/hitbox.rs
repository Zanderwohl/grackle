//! What a body can be hit on. Three volumes, kept apart:
//!
//! - The movement hull ([`crate::common::class::CLASS_HALF_EXTENTS`]): one box
//!   per stance, never animated.
//! - Hitboxes: what a shot is tested against. Bigger than the bones and moving
//!   far less.
//! - Bones: move as the animation says.
//!
//! The head box is wherever the head is, as in TF2; the body box does not
//! move.
//!
//! Hitboxes are computed from a pose sampled on the tick, never the drawn
//! pose, which is written at frame rate and would resolve differently on
//! machines drawing at different rates. See [`crate::game::hitbox`].

use bevy::prelude::*;

use crate::common::class::Stance;
use crate::common::skeleton::rig::{bone, Pose, Skeleton};

/// Meters added on each side of the head bone, so a shot clipping an ear
/// counts.
pub const HEAD_MARGIN: f32 = 0.03;

/// Axis-aligned even for a tilted head: slightly generous, but every test is a
/// comparison rather than a transform.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Box3 {
    pub center: Vec3,
    pub half_extents: Vec3,
}

impl Box3 {
    pub fn min(&self) -> Vec3 {
        self.center - self.half_extents
    }

    pub fn max(&self) -> Vec3 {
        self.center + self.half_extents
    }

    pub fn contains(&self, point: Vec3) -> bool {
        let d = (point - self.center).abs();
        d.x <= self.half_extents.x && d.y <= self.half_extents.y && d.z <= self.half_extents.z
    }
}

/// World space. Head and body only; limbs would have to track the pose
/// closely.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct Hitboxes {
    /// The movement hull's box; not animated.
    pub body: Box3,
    /// `None` for something with no head to aim at: a crate.
    pub head: Option<Box3>,
}

/// `root` is the skeleton's transform, at the feet. `pose` must be sampled on
/// the tick, not the drawn one.
pub fn hitboxes(skeleton: &Skeleton, pose: &Pose, root: &Transform, stance: Stance) -> Hitboxes {
    Hitboxes {
        body: body_box(root, stance),
        head: Some(head_box(skeleton, pose, root)),
    }
}

/// Equal to the collision hull today, but kept separate because it is a
/// different idea and should not change by accident with the hull.
fn body_box(root: &Transform, stance: Stance) -> Box3 {
    Box3 {
        center: root.translation + Vec3::Y * (stance.height() * 0.5),
        half_extents: stance.half_extents(),
    }
}

/// From the rig, around the head as it is turned: the box is axis-aligned, so
/// it encloses the tilted head rather than an upright one that leaves the
/// crown out.
fn head_box(skeleton: &Skeleton, pose: &Pose, root: &Transform) -> Box3 {
    let Some(index) = skeleton.index_of(bone::HEAD) else {
        return Box3::default();
    };
    let posed = skeleton.posed_bone(pose, root, index);
    let half = Vec3::new(posed.thickness.x * 0.5, posed.length * 0.5, posed.thickness.y * 0.5);
    let turned = Mat3::from_quat(posed.rotation).abs() * half;
    Box3 {
        center: posed.prism().translation,
        half_extents: turned + Vec3::splat(HEAD_MARGIN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::class::CLASS_HALF_EXTENTS;
    use crate::common::skeleton::state::{AnimationState, PoseInputs};
    use crate::common::skeleton::{humanoid, Proportions};

    fn rig() -> Skeleton {
        humanoid(Proportions::DEFAULT)
    }

    #[test]
    fn the_head_box_contains_the_head() {
        let skeleton = rig();
        let root = Transform::from_translation(Vec3::new(2.0, 0.0, -1.0));
        let boxes = hitboxes(&skeleton, &Pose::rest(), &root, Stance::Standing);

        let head = skeleton
            .posed_bones(&Pose::rest(), &root)
            .into_iter()
            .find(|bone| bone.name == bone::HEAD)
            .unwrap();

        assert!(boxes.head.unwrap().contains(head.head), "the base of the skull is outside the box");
        assert!(boxes.head.unwrap().contains(head.tail), "the crown is outside the box");
        assert!(boxes.head.unwrap().half_extents.y > head.length * 0.5);
        assert!(boxes.head.unwrap().half_extents.x > head.thickness.x * 0.5);
    }

    /// The box goes where the head goes, however far.
    #[test]
    fn the_head_box_follows_the_head_wherever_it_goes() {
        let skeleton = rig();
        let root = Transform::IDENTITY;
        let thrown = Pose::rest()
            .with(bone::CHEST, Quat::from_rotation_x(-1.4))
            .with(bone::NECK, Quat::from_rotation_x(-1.0));
        let head = skeleton
            .posed_bones(&thrown, &root)
            .into_iter()
            .find(|posed| posed.name == bone::HEAD)
            .unwrap();
        let rest = hitboxes(&skeleton, &Pose::rest(), &root, Stance::Standing).head.unwrap();
        let moved = hitboxes(&skeleton, &thrown, &root, Stance::Standing).head.unwrap();

        assert!((moved.center - rest.center).length() > 0.2, "the box stayed behind");
        assert!(moved.contains(head.head) && moved.contains(head.tail), "the thrown head is outside its box");
        // Tilted, so wider than it is tall would be wrong the other way round.
        assert!(moved.half_extents.z > rest.half_extents.z, "the box did not widen for a tilted head");
    }

    /// Ducking takes the head box down with the head.
    #[test]
    fn the_head_box_follows_a_body_that_ducks() {
        use crate::common::skeleton::finish_pose;

        let skeleton = rig();
        let root = Transform::from_xyz(1.0, 0.0, 2.0);
        let inputs = PoseInputs::default();

        let ducked = finish_pose(&skeleton, AnimationState::Crouch, &inputs, &root, None);
        let standing = finish_pose(&skeleton, AnimationState::Idle, &inputs, &root, None);

        let up = hitboxes(&skeleton, &standing, &root, Stance::Standing).head.unwrap();
        let down = hitboxes(&skeleton, &ducked, &root, Stance::Crouched).head.unwrap();

        let drop = Stance::Standing.height() - Stance::Crouched.height();
        assert!(
            up.center.y - down.center.y > drop * 0.8,
            "ducking moved the head box by {:.2} m",
            up.center.y - down.center.y
        );

        let head = skeleton
            .posed_bones(&ducked, &root)
            .into_iter()
            .find(|posed| posed.name == bone::HEAD)
            .unwrap();
        assert!(down.contains(head.head) && down.contains(head.tail), "the ducked head is outside its own box");
    }

    #[test]
    fn every_class_is_boxed_around_its_own_head() {
        use crate::common::class::Class;
        use crate::common::skeleton::humanoid;
        use strum::IntoEnumIterator;

        let mut heights = Vec::new();
        for class in Class::iter() {
            let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
            let root = Transform::IDENTITY;
            let boxes = hitboxes(&skeleton, &Pose::rest(), &root, Stance::Standing);

            let head = skeleton
                .posed_bones(&Pose::rest(), &root)
                .into_iter()
                .find(|posed| posed.name == bone::HEAD)
                .unwrap();
            assert!(
                boxes.head.unwrap().contains(head.head) && boxes.head.unwrap().contains(head.tail),
                "{class:?} is not boxed around its own head"
            );
            heights.push(boxes.head.unwrap().center.y);
        }

        // Otherwise the check above could pass on one generous box.
        let low = heights.iter().copied().fold(f32::MAX, f32::min);
        let high = heights.iter().copied().fold(f32::MIN, f32::max);
        assert!(high - low > 0.15, "every class's head box is at the same height");
    }

    #[test]
    fn the_body_box_ignores_the_pose_entirely() {
        let skeleton = rig();
        let root = Transform::from_translation(Vec3::new(-4.0, 2.0, 0.5));

        let standing = hitboxes(&skeleton, &Pose::rest(), &root, Stance::Standing).body;
        let idling = hitboxes(
            &skeleton,
            &AnimationState::Idle.pose(&PoseInputs { seconds: 1.7, ..default() }),
            &root,
            Stance::Standing,
        )
        .body;

        assert_eq!(standing, idling);
        assert_eq!(standing.half_extents, CLASS_HALF_EXTENTS);
        assert!((standing.min().y - root.translation.y).abs() < 1e-5);
    }
}
