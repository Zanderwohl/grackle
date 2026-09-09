//! What a body can be hit on, which is not the same thing as what it is drawn
//! as.
//!
//! Three volumes get confused with each other, and keeping them apart is the
//! whole of this module:
//!
//! - **The movement hull** ([`crate::common::class::CLASS_HALF_EXTENTS`]) is
//!   what stops a body walking through a wall. It is one box, the same for
//!   every class, and no animation touches it.
//! - **Hitboxes** are what a shot is tested against. They are related to the
//!   bones — the head one is attached to the head — but they are not the
//!   bones: bigger than what they cover, and moving far less.
//! - **Bones** move as much as the animation says, which is the point of them.
//!
//! Two rules follow, and both are about fairness rather than tidiness:
//!
//! - **A head hitbox barely moves — within a pose.** It follows the head
//!   through a fraction and a hard clamp ([`HEAD_FOLLOW`], [`HEAD_MAX_OFFSET`])
//!   *of where that body's head settles in what it is currently doing*, so an
//!   animator can give a body as much bob as it needs and a shot at where the
//!   head plainly is still lands. What the budget must never damp is the head
//!   moving because the body did: a ducked body's head is most of a metre
//!   lower, and a hitbox that stayed up where it used to be would make
//!   crouching a way of leaving your head behind.
//! - **A hitbox never reads the drawn pose.** The pose on screen is written at
//!   frame rate, and a hit test that read it would resolve differently on two
//!   machines drawing at different rates. Hitboxes are computed on the tick,
//!   from a pose sampled at the tick — see [`crate::game::hitbox`].
//!
//! What tests against them is [`crate::common::hitscan`], and the debug laser
//! in [`crate::game::hitscan`] is how you look at the answer. There is still no
//! weapon and no damage — a hit is a coloured ball and no further.

use bevy::prelude::*;

use crate::common::class::Stance;
use crate::common::skeleton::rig::{bone, Pose, Skeleton};

/// How much of the head's movement the head hitbox takes.
///
/// A quarter: enough that leaning out of the way is worth something, little
/// enough that an idle cycle does not move where you have to aim.
pub const HEAD_FOLLOW: f32 = 0.25;

/// The furthest the head hitbox will go from where it rests, in metres.
///
/// The backstop behind [`HEAD_FOLLOW`]: whatever an animation does, a head
/// hitbox stays within this of the head on a body standing still.
pub const HEAD_MAX_OFFSET: f32 = 0.05;

/// How much bigger than the head bone the head hitbox is, in metres on each
/// side.
///
/// "A bit bigger than the head" — a shot that clips an ear should count, and
/// the drawn skull is a rectangle standing in for one.
pub const HEAD_MARGIN: f32 = 0.03;

/// An axis-aligned box.
///
/// Axis-aligned including the head one, which a body's own head is not. That
/// is a simplification worth naming: it makes the box a little generous when a
/// head is tilted, and it makes every test against it a comparison rather than
/// a transform.
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

/// Where a body can be hit, in world space.
///
/// Head and body only. Limbs are real in the games this is modelled on, but
/// they are also the part that has to track the pose closely, and there is
/// nothing to shoot them with yet.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct Hitboxes {
    /// The bulk of the body: an upright box the size of the movement hull,
    /// which does not move with the animation at all.
    pub body: Box3,
    /// The head: attached to the head bone, bigger than it, and damped.
    pub head: Box3,
}

/// The boxes for a body whose feet are at `root`, posed as `pose`.
///
/// `root` is the skeleton's own transform — feet on the floor — and the pose
/// must be one sampled on the tick, not the one being drawn.
///
/// `settled` is the same body doing the same thing with the cycle taken out of
/// it: the pose it would hold standing at that moment. It is what the head
/// hitbox is anchored to, so that ducking moves the box and a step bob does
/// not. Passing `pose` itself would make the box track the animation exactly;
/// passing the rest pose would leave it behind whenever the body changed
/// shape.
pub fn hitboxes(
    skeleton: &Skeleton,
    pose: &Pose,
    settled: &Pose,
    root: &Transform,
    stance: Stance,
) -> Hitboxes {
    Hitboxes {
        body: body_box(root, stance),
        head: head_box(skeleton, pose, settled, root),
    }
}

/// The body box: the movement hull of whatever stance the body is in,
/// standing on its feet.
///
/// Ducking shrinks it, which is the point of ducking — a crouched body is a
/// smaller thing to hit as well as a shorter one to see over. It is the only
/// thing about this box that moves; the animation still does not touch it.
///
/// Deliberately the same box the body collides with, and deliberately written
/// as its own function anyway: the two are not the same idea, so when one of
/// them has to change the other does not follow by accident.
fn body_box(root: &Transform, stance: Stance) -> Box3 {
    Box3 {
        centre: root.translation + Vec3::Y * (stance.height() * 0.5),
        half_extents: stance.half_extents(),
    }
}

/// The head box: this body's own head bone, inflated, anchored where that head
/// settles and only fractionally where the animation has taken it.
///
/// Everything about it comes from the rig rather than from a constant, so a
/// class with a bigger head or a shorter body is boxed around its own head
/// rather than around the average of everybody's.
fn head_box(skeleton: &Skeleton, pose: &Pose, settled: &Pose, root: &Transform) -> Box3 {
    let head_centre = |pose: &Pose| {
        skeleton
            .posed_bones(pose, root)
            .into_iter()
            .find(|posed| posed.name == bone::HEAD)
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

    /// The head hitbox has to cover the head it is drawn around, or a shot
    /// that plainly hit somebody misses.
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

        assert!(boxes.head.contains(head.head), "the base of the skull is outside the box");
        assert!(boxes.head.contains(head.tail), "the crown is outside the box");
        // And bigger than it, on every axis.
        assert!(boxes.head.half_extents.y > head.length * 0.5);
        assert!(boxes.head.half_extents.x > head.thickness.x * 0.5);
    }

    /// The whole point of the layer: the head moves through a whole idle
    /// cycle and the hitbox barely does.
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

        let rest = hitboxes(&skeleton, &Pose::rest(), &Pose::rest(), &root, Stance::Standing).head.centre;
        let mut most_head = 0.0_f32;
        let mut most_box = 0.0_f32;
        for step in 0..64 {
            let pose = AnimationState::Idle.pose(&PoseInputs { seconds: step as f32 * 0.05, ..default() });
            most_head = most_head.max((head_of(&pose) - head_of(&Pose::rest())).length());
            most_box = most_box.max((hitboxes(&skeleton, &pose, &Pose::rest(), &root, Stance::Standing).head.centre - rest).length());
        }

        assert!(most_head > 0.015, "the idle barely moves the head at all: {most_head} m");
        assert!(
            most_box <= most_head * HEAD_FOLLOW + 1e-4,
            "the hitbox took {most_box} m of the head's {most_head} m"
        );
        assert!(most_box <= HEAD_MAX_OFFSET + 1e-4);
    }

    /// However wild the animation, the head hitbox stays near where a shooter
    /// would expect it. A pose no idle would ever produce, on purpose.
    #[test]
    fn no_animation_can_move_the_head_hitbox_far() {
        let skeleton = rig();
        let root = Transform::IDENTITY;
        let thrown = Pose::rest()
            .with(bone::CHEST, Quat::from_rotation_x(-1.4))
            .with(bone::NECK, Quat::from_rotation_x(-1.0));

        let rest = hitboxes(&skeleton, &Pose::rest(), &Pose::rest(), &root, Stance::Standing).head.centre;
        let moved = hitboxes(&skeleton, &thrown, &Pose::rest(), &root, Stance::Standing).head.centre;

        assert!(
            (moved - rest).length() <= HEAD_MAX_OFFSET + 1e-5,
            "a duck moved the head hitbox by {} m",
            (moved - rest).length()
        );
    }

    /// A ducked body's head is most of a metre lower, and the box has to go
    /// with it. The damping is there to absorb a step bob, not a change of
    /// stance — anchoring on the standing pose made crouching a way of leaving
    /// your head behind.
    #[test]
    fn the_head_box_follows_a_body_that_ducks() {
        use crate::common::skeleton::finish_pose;

        let skeleton = rig();
        let root = Transform::from_xyz(1.0, 0.0, 2.0);
        let inputs = PoseInputs::default();

        let ducked = finish_pose(&skeleton, AnimationState::Crouch, &inputs, &root);
        let standing = finish_pose(&skeleton, AnimationState::Idle, &inputs, &root);

        let up = hitboxes(&skeleton, &standing, &standing, &root, Stance::Standing).head;
        let down = hitboxes(&skeleton, &ducked, &ducked, &root, Stance::Crouched).head;

        assert!(
            up.centre.y - down.centre.y > 0.5,
            "ducking moved the head box by {:.2} m",
            up.centre.y - down.centre.y
        );

        // And it is around the ducked head, not merely lower than it was.
        let head = skeleton
            .posed_bones(&ducked, &root)
            .into_iter()
            .find(|posed| posed.name == bone::HEAD)
            .unwrap();
        assert!(down.contains(head.head) && down.contains(head.tail), "the ducked head is outside its own box");
    }

    /// Every class is boxed around its own head. A roster whose heads are at
    /// ten different heights cannot share one number, and the rig already
    /// knows where each of them is.
    #[test]
    fn every_class_is_boxed_around_its_own_head() {
        use crate::common::class::Class;
        use crate::common::skeleton::humanoid;
        use strum::IntoEnumIterator;

        let mut heights = Vec::new();
        for class in Class::iter() {
            let skeleton = humanoid(class.proportions());
            let root = Transform::IDENTITY;
            let boxes = hitboxes(&skeleton, &Pose::rest(), &Pose::rest(), &root, Stance::Standing);

            let head = skeleton
                .posed_bones(&Pose::rest(), &root)
                .into_iter()
                .find(|posed| posed.name == bone::HEAD)
                .unwrap();
            assert!(
                boxes.head.contains(head.head) && boxes.head.contains(head.tail),
                "{class:?} is not boxed around its own head"
            );
            heights.push(boxes.head.centre.y);
        }

        // And those heights genuinely differ, or the check above would pass on
        // a box that happened to be generous enough for everybody.
        let low = heights.iter().copied().fold(f32::MAX, f32::min);
        let high = heights.iter().copied().fold(f32::MIN, f32::max);
        assert!(high - low > 0.15, "every class's head box is at the same height");
    }

    /// The body box is the movement hull and does not animate. If it ever
    /// starts following the pose, a body that crouched visually would become
    /// unhittable where it plainly is.
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
        // Standing on its feet, not centred on them.
        assert!((standing.min().y - root.translation.y).abs() < 1e-5);
    }
}
