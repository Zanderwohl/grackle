//! What a body can be hit on. Three volumes, kept apart:
//!
//! - The movement hull ([`crate::common::class::CLASS_HALF_EXTENTS`]): one box
//!   per stance, never animated.
//! - Hitboxes: what a shot is tested against. Bigger than the bones and moving
//!   far less.
//! - Bones: move as the animation says.
//!
//! The head hitbox follows the head only by a damped, clamped amount
//! ([`HEAD_FOLLOW`], [`HEAD_MAX_OFFSET`]) relative to where the head settles
//! in the current state, so animation bob does not move the target but
//! ducking does.
//!
//! Hitboxes are computed from a pose sampled on the tick, never the drawn
//! pose, which is written at frame rate and would resolve differently on
//! machines drawing at different rates. See [`crate::game::hitbox`].

use bevy::prelude::*;

use crate::common::class::Stance;
use crate::common::skeleton::rig::{bone, Pose, Skeleton};

/// Fraction of the head's movement the head hitbox takes: enough that leaning
/// counts, little enough that an idle cycle does not move the target.
pub const HEAD_FOLLOW: f32 = 0.25;

/// Metres. Hard clamp behind [`HEAD_FOLLOW`].
pub const HEAD_MAX_OFFSET: f32 = 0.05;

/// Metres added on each side of the head bone, so a shot clipping an ear
/// counts.
pub const HEAD_MARGIN: f32 = 0.03;

/// Axis-aligned even for a tilted head: slightly generous, but every test is a
/// comparison rather than a transform.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Box3 {
    pub centre: Vec3,
    pub half_extents: Vec3,
}

impl Box3 {
    pub fn min(&self) -> Vec3 {
        self.centre - self.half_extents
    }

    pub fn max(&self) -> Vec3 {
        self.centre + self.half_extents
    }

    pub fn contains(&self, point: Vec3) -> bool {
        let d = (point - self.centre).abs();
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
///
/// `settled` is the same state with the cycle taken out, and anchors the head
/// box. Passing `pose` would make the box track the animation exactly; passing
/// the rest pose would leave it behind when the body ducks.
pub fn hitboxes(
    skeleton: &Skeleton,
    pose: &Pose,
    settled: &Pose,
    root: &Transform,
    stance: Stance,
) -> Hitboxes {
    Hitboxes {
        body: body_box(root, stance),
        head: Some(head_box(skeleton, pose, settled, root)),
    }
}

/// Equal to the collision hull today, but kept separate because it is a
/// different idea and should not change by accident with the hull.
fn body_box(root: &Transform, stance: Stance) -> Box3 {
    Box3 {
        centre: root.translation + Vec3::Y * (stance.height() * 0.5),
        half_extents: stance.half_extents(),
    }
}

/// Sized and placed from the rig, so each class is boxed around its own head.
fn head_box(skeleton: &Skeleton, pose: &Pose, settled: &Pose, root: &Transform) -> Box3 {
    let head = skeleton.index_of(bone::HEAD);
    let head_centre = |pose: &Pose| {
        head.map(|index| skeleton.posed_bone(pose, root, index))
            .map(|posed| (posed.prism().translation, posed.length, posed.thickness))
    };

    let Some((anchor, length, thickness)) = head_centre(settled) else {
        return Box3::default();
    };
    let posed = head_centre(pose).map(|(centre, _, _)| centre).unwrap_or(anchor);

    let travel = (posed - anchor) * HEAD_FOLLOW;
    let travel = travel.clamp_length_max(HEAD_MAX_OFFSET);

    Box3 {
        centre: anchor + travel,
        half_extents: Vec3::new(
            thickness.x * 0.5 + HEAD_MARGIN,
            length * 0.5 + HEAD_MARGIN,
            thickness.y * 0.5 + HEAD_MARGIN,
        ),
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
        let boxes = hitboxes(&skeleton, &Pose::rest(), &Pose::rest(), &root, Stance::Standing);

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

    #[test]
    fn an_idle_moves_the_head_far_more_than_its_hitbox() {
        let skeleton = rig();
        let root = Transform::IDENTITY;

        let head_of = |pose: &Pose| {
            skeleton
                .posed_bones(pose, &root)
                .into_iter()
                .find(|bone| bone.name == bone::HEAD)
                .unwrap()
                .prism()
                .translation
        };

        let rest = hitboxes(&skeleton, &Pose::rest(), &Pose::rest(), &root, Stance::Standing).head.unwrap().centre;
        let mut most_head = 0.0_f32;
        let mut most_box = 0.0_f32;
        for step in 0..64 {
            let pose = AnimationState::Idle.pose(&PoseInputs { seconds: step as f32 * 0.05, ..default() });
            most_head = most_head.max((head_of(&pose) - head_of(&Pose::rest())).length());
            most_box = most_box.max((hitboxes(&skeleton, &pose, &Pose::rest(), &root, Stance::Standing).head.unwrap().centre - rest).length());
        }

        assert!(most_head > 0.015, "the idle barely moves the head at all: {most_head} m");
        assert!(
            most_box <= most_head * HEAD_FOLLOW + 1e-4,
            "the hitbox took {most_box} m of the head's {most_head} m"
        );
        assert!(most_box <= HEAD_MAX_OFFSET + 1e-4);
    }

    /// An extreme pose on purpose; the clamp must still hold.
    #[test]
    fn no_animation_can_move_the_head_hitbox_far() {
        let skeleton = rig();
        let root = Transform::IDENTITY;
        let thrown = Pose::rest()
            .with(bone::CHEST, Quat::from_rotation_x(-1.4))
            .with(bone::NECK, Quat::from_rotation_x(-1.0));

        let rest = hitboxes(&skeleton, &Pose::rest(), &Pose::rest(), &root, Stance::Standing).head.unwrap().centre;
        let moved = hitboxes(&skeleton, &thrown, &Pose::rest(), &root, Stance::Standing).head.unwrap().centre;

        assert!(
            (moved - rest).length() <= HEAD_MAX_OFFSET + 1e-5,
            "a duck moved the head hitbox by {} m",
            (moved - rest).length()
        );
    }

    /// Fails if the damping absorbs a change of stance, leaving the head box
    /// above a ducked head.
    #[test]
    fn the_head_box_follows_a_body_that_ducks() {
        use crate::common::skeleton::finish_pose;

        let skeleton = rig();
        let root = Transform::from_xyz(1.0, 0.0, 2.0);
        let inputs = PoseInputs::default();

        let ducked = finish_pose(&skeleton, AnimationState::Crouch, &inputs, &root, None);
        let standing = finish_pose(&skeleton, AnimationState::Idle, &inputs, &root, None);

        let up = hitboxes(&skeleton, &standing, &standing, &root, Stance::Standing).head.unwrap();
        let down = hitboxes(&skeleton, &ducked, &ducked, &root, Stance::Crouched).head.unwrap();

        assert!(
            up.centre.y - down.centre.y > 0.5,
            "ducking moved the head box by {:.2} m",
            up.centre.y - down.centre.y
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
            let boxes = hitboxes(&skeleton, &Pose::rest(), &Pose::rest(), &root, Stance::Standing);

            let head = skeleton
                .posed_bones(&Pose::rest(), &root)
                .into_iter()
                .find(|posed| posed.name == bone::HEAD)
                .unwrap();
            assert!(
                boxes.head.unwrap().contains(head.head) && boxes.head.unwrap().contains(head.tail),
                "{class:?} is not boxed around its own head"
            );
            heights.push(boxes.head.unwrap().centre.y);
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

        let standing = hitboxes(&skeleton, &Pose::rest(), &Pose::rest(), &root, Stance::Standing).body;
        let idling = hitboxes(
            &skeleton,
            &AnimationState::Idle.pose(&PoseInputs { seconds: 1.7, ..default() }),
            &Pose::rest(),
            &root,
            Stance::Standing,
        )
        .body;

        assert_eq!(standing, idling);
        assert_eq!(standing.half_extents, CLASS_HALF_EXTENTS);
        assert!((standing.min().y - root.translation.y).abs() < 1e-5);
    }
}
