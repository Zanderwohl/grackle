//! Keeping every body's hitboxes up to date, and drawing them.
//!
//! The pose is sampled on the tick rather than read off the body: the drawn
//! pose is written at frame rate, so reading it here would make where a body
//! can be hit depend on the viewer's frame rate. See [`crate::common::hitbox`].
//!
//! Built in the drawn pose's order: the animator, claims standing, the look,
//! then the layer stack, whose clips are at most a frame behind the tick.

use bevy::prelude::*;

use crate::common::app_mode::showing_the_world;
use bevy::transform::TransformSystems;

use crate::common::class::Stance;
use crate::common::hitbox::{hitboxes, Hitboxes};
use crate::common::skeleton::state::ClipLibrary;
use crate::common::skeleton::{
    animator_pose, AnimationClock, AnimationPhase, Gait, PoseInputs, Skeleton, SkeletonAnimator,
};
use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::ik::finish_pose;
use crate::common::skeleton::play::{apply_stack, ClipStack, Frames, Layer};
use crate::common::skeleton::state::{AnimationState, BodyRequests};
use crate::game::player::{step_player, LocalPlayer, PhysicsBody, Player, ViewMode};
use crate::game::skeleton::{
    advance_animation_clock, advance_gaits, look, skeleton_root, stand_where_claimed, SkeletonRoot,
};
use crate::prop::hold::carry_frame;

/// Whether hitboxes, and the debug laser aimed at them, are drawn. `F4`
/// toggles both. Off by default: a debug view, and the laser traces every body.
#[derive(Resource, Debug, Default)]
pub struct ShowHitboxes(pub bool);

pub struct HitboxPlugin;

impl Plugin for HitboxPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<ShowHitboxes>()
            .add_systems(Update, toggle_hitboxes)
            .add_systems(FixedUpdate, update_hitboxes
                .after(advance_animation_clock)
                .after(advance_gaits)
                // So a body is boxed where this tick's step left it.
                .after(step_player))
            // A gizmo is not an entity, so the prop editor's hiding cannot
            // reach it. `update_hitboxes` keeps running regardless.
            .add_systems(
                PostUpdate,
                draw_hitboxes
                    .after(TransformSystems::Propagate)
                    .run_if(showing_the_world),
            )
        ;
    }
}

fn toggle_hitboxes(keys: Res<ButtonInput<KeyCode>>, mut show: ResMut<ShowHitboxes>) {
    if keys.just_pressed(KeyCode::F4) {
        show.0 = !show.0;
    }
}

/// Recompute every body's boxes from a pose sampled on this tick.
pub fn update_hitboxes(
    clock: Res<AnimationClock>,
    // Absent with no clips loaded and in tests; every state is then a function.
    library: Option<Res<ClipLibrary>>,
    layer_clips: Option<Res<Assets<Clip>>>,
    mut bodies: Query<(
        (&Skeleton, &SkeletonAnimator, Option<&AnimationPhase>, Option<&Gait>),
        Option<&Stance>,
        &GlobalTransform,
        Option<&SkeletonRoot>,
        Option<&PhysicsBody>,
        (Option<&Player>, Option<&ClipStack>, Option<&BodyRequests>),
        &mut Hitboxes,
    )>,
) {
    for ((skeleton, animator, phase, gait), stance, global, offset, physics, layers, mut boxes) in
        &mut bodies
    {
        let (player, stack, requests) = layers;
        // The step's position, not the interpolated transform, which depends
        // on frame rate.
        let mut placed = *global;
        if let Some(physics) = physics {
            placed = GlobalTransform::from(
                global.compute_transform().with_translation(physics.current),
            );
        }
        let root = skeleton_root(&placed, offset);

        let clips = library.as_deref();
        let gait = gait.copied().unwrap_or_default();
        let inputs = PoseInputs {
            seconds: clock.seconds() + phase.copied().unwrap_or_default().0,
            stride: gait.phase(),
            speed: gait.speed(),
            travel: gait.travel(),
        };
        let mut pose = animator_pose(skeleton, animator, &inputs, &root, clips);
        if let Some(stack) = stack {
            let still = AnimationState::from_requests(&BodyRequests {
                crouching: requests.is_some_and(|requests| requests.crouching),
                airborne: requests.is_some_and(|requests| requests.airborne),
                ..default()
            });
            stand_where_claimed(skeleton, &mut pose, stack, || {
                finish_pose(skeleton, still, &inputs, &root, clips)
            });
        }
        let pitch = player.map_or(0.0, |player| player.pitch);
        look(&mut pose, pitch);
        if let (Some(stack), Some(layer_clips)) = (stack, layer_clips.as_deref()) {
            let floor = root.translation.y;
            let ground = move |_: Vec3| floor;
            let aim = Quat::from_rotation_x(pitch);
            let frames = Frames::standing(
                root,
                carry_frame(skeleton, &pose, &root, aim).unwrap_or(root),
                &ground,
            );
            // Not the flinch: started by the last hit and run on frame time, it
            // would move the next shot's target by the server's frame rate.
            let held = ClipStack(stack.0.iter().filter(|playing| playing.layer != Layer::Flinch).cloned().collect());
            apply_stack(&held, layer_clips, skeleton, &mut pose, &frames, None);
        }
        // A body shown rather than simulated has no stance; infer it from the
        // animation.
        let stance = stance.copied().unwrap_or_else(|| {
            if animator.state().ducks() {
                Stance::Crouched
            } else {
                Stance::Standing
            }
        });

        *boxes = hitboxes(skeleton, &pose, &root, stance);
    }
}

fn draw_hitboxes(
    show: Res<ShowHitboxes>,
    view: Res<ViewMode>,
    mut gizmos: Gizmos,
    bodies: Query<(&Hitboxes, Option<&LocalPlayer>)>,
) {
    if !show.0 {
        return;
    }

    let body_color = Color::srgb(0.55, 0.16, 0.16);
    let head_color = Color::srgb(1.0, 0.25, 0.25);

    for (boxes, own_body) in &bodies {
        // `LocalPlayer`, not `Player`: replicated bodies carry `Player` too.
        if own_body.is_some() && !view.shows_own_body() {
            continue;
        }
        let head = boxes.head.map(|head| (head, head_color));
        for (hitbox, color) in [(boxes.body, body_color)].into_iter().chain(head) {
            gizmos.cube(
                Transform::from_translation(hitbox.center).with_scale(hitbox.half_extents * 2.0),
                color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::skeleton::{humanoid, Pose, Proportions};

    /// The boxes follow the body, and the body box stands on its feet.
    #[test]
    fn the_boxes_are_placed_where_the_body_is() {
        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        let feet = Vec3::new(3.0, 1.0, -2.0);
        let body = world
            .spawn((
                humanoid(Proportions::DEFAULT),
                Pose::rest(),
                SkeletonAnimator::default(),
                Hitboxes::default(),
                Transform::from_translation(feet),
                GlobalTransform::from_translation(feet),
            ))
            .id();

        world.run_system_once(update_hitboxes).unwrap();

        let boxes = world.get::<Hitboxes>(body).unwrap();
        assert!((boxes.body.min().y - feet.y).abs() < 1e-4, "the body box is not standing on the floor");
        assert!((boxes.body.center.xz() - feet.xz()).length() < 1e-4);
        assert!(boxes.head.unwrap().center.y > boxes.body.center.y, "the head is not above the middle of the body");
    }

    /// A simulated body's stance wins over its animation.
    #[test]
    fn a_simulated_bodys_stance_beats_its_animation() {
        use crate::common::class::TALLEST_CLASS_HEIGHT;
        use crate::common::skeleton::{AnimationState, ForcedAnimation};

        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        let body = world
            .spawn((
                humanoid(Proportions::DEFAULT),
                Pose::rest(),
                SkeletonAnimator::default(),
                // Deliberately at odds: ducking animation, standing stance.
                ForcedAnimation(AnimationState::Crouch),
                Stance::Standing,
                Hitboxes::default(),
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
            ))
            .id();

        world.run_system_once(update_hitboxes).unwrap();

        let height = world.get::<Hitboxes>(body).unwrap().body.half_extents.y * 2.0;
        assert!(
            (height - TALLEST_CLASS_HEIGHT).abs() < 1e-4,
            "the animation overrode the stance: {height:.2} m"
        );
    }

    /// A body with no stance is boxed as its animation stands. Every animation
    /// grid body relies on this.
    #[test]
    fn a_body_with_no_stance_is_boxed_as_its_animation_stands() {
        use crate::common::class::CROUCH_HEIGHT;
        use crate::common::skeleton::{AnimationState, ForcedAnimation};

        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        let body = world
            .spawn((
                humanoid(Proportions::DEFAULT),
                Pose::rest(),
                SkeletonAnimator::default(),
                ForcedAnimation(AnimationState::Crouch),
                Hitboxes::default(),
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
            ))
            .id();
        // Settle it into the crouch it is being shown in.
        world.resource_mut::<AnimationClock>().advance(1.0 / 64.0);
        {
            let mut animator = world.get_mut::<SkeletonAnimator>(body).unwrap();
            animator.force(AnimationState::Crouch, 1.0);
        }

        world.run_system_once(update_hitboxes).unwrap();

        let boxes = world.get::<Hitboxes>(body).unwrap();
        let height = boxes.body.half_extents.y * 2.0;
        assert!(
            (height - CROUCH_HEIGHT).abs() < 1e-4,
            "a ducked body is boxed {height:.2} m tall"
        );
    }

    /// Boxed at the stepped position, not the interpolated one.
    #[test]
    fn a_physics_body_is_boxed_at_its_stepped_position() {
        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        let stepped = Vec3::new(10.0, 0.0, 0.0);
        let drawn = Vec3::new(-10.0, 0.0, 0.0);
        let body = world
            .spawn((
                humanoid(Proportions::DEFAULT),
                Pose::rest(),
                SkeletonAnimator::default(),
                Hitboxes::default(),
                PhysicsBody { previous: stepped, current: stepped },
                Transform::from_translation(drawn),
                GlobalTransform::from_translation(drawn),
            ))
            .id();

        world.run_system_once(update_hitboxes).unwrap();

        let boxes = world.get::<Hitboxes>(body).unwrap();
        assert!(
            (boxes.body.center.x - stepped.x).abs() < 1e-4,
            "boxed at {}, which is the drawn position, not the stepped one",
            boxes.body.center.x
        );
    }

    /// The head box of a body at `pitch`, with `stack` playing over it.
    fn head_box_of(pitch: f32, stack: ClipStack, clips: Assets<Clip>) -> crate::common::hitbox::Box3 {
        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        world.insert_resource(clips);
        let body = world
            .spawn((
                humanoid(Proportions::DEFAULT),
                Pose::rest(),
                SkeletonAnimator::default(),
                Player { pitch, ..default() },
                stack,
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
            ))
            .id();
        world.run_system_once(update_hitboxes).unwrap();
        world.get::<Hitboxes>(body).unwrap().head.unwrap()
    }

    /// Looking at the floor takes the head box down and forward, as in TF2.
    #[test]
    fn the_head_box_looks_where_the_body_looks() {
        let level = head_box_of(0.0, ClipStack::default(), Assets::default());
        let down = head_box_of(-1.2, ClipStack::default(), Assets::default());
        assert!(down.center.y < level.center.y - 0.02, "looking down left the head box up");
        assert!(down.center.z < level.center.z - 0.02, "looking down left the head box back");
    }

    /// A layer that moves the head moves the box.
    #[test]
    fn the_head_box_follows_a_layer_that_moves_the_head() {
        use crate::common::skeleton::clip::{Channel, Key, Track};
        use crate::common::skeleton::play::{Layer, Playing, Regions};
        use crate::common::skeleton::rig::bone;
        use crate::common::skeleton::Interpolation;

        let mut clips = Assets::<Clip>::default();
        let mut bow = Track::rotation(bone::CHEST);
        let Channel::Rotation { keys, .. } = &mut bow.channel else { unreachable!() };
        keys.push(Key::all(0.0, [1.0, 0.0, 0.0], Interpolation::Linear));
        let bowing = clips.add(Clip { length: 1.0, tracks: vec![bow], ..default() });
        let stack = ClipStack(vec![Playing {
            claims: Regions::TORSO.union(Regions::HEAD),
            ..Playing::new(Layer::Taunt, bowing, 0.0)
        }]);

        let upright = head_box_of(0.0, ClipStack::default(), Assets::default());
        let bowed = head_box_of(0.0, stack, clips);
        assert!(
            (bowed.center - upright.center).length() > 0.2,
            "a bow moved the head box {} m",
            (bowed.center - upright.center).length(),
        );
    }
}

