//! Keeping every body's hitboxes up to date, and drawing them.
//!
//! The work runs in `FixedUpdate`, with the pose sampled at the tick rather
//! than read off the body. That is the rule from
//! [`crate::common::hitbox`] made real: the pose on screen is written at frame
//! rate from the same shared clock, so reading it here would make where a
//! player can be hit depend on how fast the machine asking is drawing.
//!
//! Everything with a [`Skeleton`] gets boxes — players, mannequins, and the
//! sixty bodies in an animation grid. The grid is the harness: stand in front
//! of it and watch heads bob inside head boxes that hardly move.

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::common::hitbox::{hitboxes, Hitboxes};
use crate::common::skeleton::{
    finish_pose, AnimationClock, AnimationPhase, Skeleton, SkeletonAnimator,
};
use crate::game::player::{step_player, PhysicsBody, Player, ViewMode};
use crate::game::skeleton::{advance_animation_clock, skeleton_root, SkeletonRoot};

/// Whether hitboxes are drawn. `F4` toggles it.
///
/// On by default: they exist to be looked at, and nothing tests against them
/// yet, so a hidden hitbox layer would be a layer nobody ever checked.
#[derive(Resource, Debug)]
pub struct ShowHitboxes(pub bool);

impl Default for ShowHitboxes {
    fn default() -> Self {
        ShowHitboxes(true)
    }
}

pub struct HitboxPlugin;

impl Plugin for HitboxPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<ShowHitboxes>()
            .add_systems(Update, (give_bodies_hitboxes, toggle_hitboxes))
            // On the tick, with the physics that hit registration will one day
            // have to agree with.
            .add_systems(FixedUpdate, update_hitboxes
                .after(advance_animation_clock)
                // After the step, so a body is boxed where this tick left it
                // rather than where the last one did.
                .after(step_player))
            .add_systems(PostUpdate, draw_hitboxes.after(TransformSystems::Propagate))
        ;
    }
}

/// Anything with a rig can be shot at, so anything with a rig gets boxes.
///
/// One place rather than a line in each of the four things that spawn a body —
/// a body whose creator forgot would be one that silently could not be hit.
fn give_bodies_hitboxes(
    mut commands: Commands,
    bodies: Query<Entity, (With<Skeleton>, Without<Hitboxes>)>,
) {
    for body in &bodies {
        commands.entity(body).insert(Hitboxes::default());
    }
}

fn toggle_hitboxes(keys: Res<ButtonInput<KeyCode>>, mut show: ResMut<ShowHitboxes>) {
    if keys.just_pressed(KeyCode::F4) {
        show.0 = !show.0;
    }
}

/// Recompute every body's boxes from a pose sampled on this tick.
fn update_hitboxes(
    clock: Res<AnimationClock>,
    mut bodies: Query<(
        &Skeleton,
        &SkeletonAnimator,
        Option<&AnimationPhase>,
        &GlobalTransform,
        Option<&SkeletonRoot>,
        Option<&PhysicsBody>,
        &mut Hitboxes,
    )>,
) {
    for (skeleton, animator, phase, global, offset, physics, mut boxes) in &mut bodies {
        // A moving body's drawn transform is interpolated between ticks, which
        // is exactly the frame-rate-dependent quantity this must not use. The
        // fixed step's own position is the one two machines can agree on.
        let mut placed = *global;
        if let Some(physics) = physics {
            placed = GlobalTransform::from(
                global.compute_transform().with_translation(physics.current),
            );
        }
        let root = skeleton_root(&placed, offset);

        // The same pipeline the drawing goes through, corrections and all: a
        // hitbox worked out from an uncorrected pose would sit where the body
        // visibly is not.
        let seconds = clock.seconds() + phase.copied().unwrap_or_default().0;
        let pose = finish_pose(skeleton, animator.state(), seconds, &root);
        *boxes = hitboxes(skeleton, &pose, &root);
    }
}

fn draw_hitboxes(
    show: Res<ShowHitboxes>,
    view: Res<ViewMode>,
    mut gizmos: Gizmos,
    bodies: Query<(&Hitboxes, Option<&Player>)>,
) {
    if !show.0 {
        return;
    }

    // Red, and two shades of it: the head is the one worth aiming at, so it is
    // the one that has to be legible against the bones inside it.
    let body_colour = Color::srgb(0.55, 0.16, 0.16);
    let head_colour = Color::srgb(1.0, 0.25, 0.25);

    for (boxes, own_body) in &bodies {
        // Hidden along with the body it belongs to: in first person the camera
        // stands in the middle of its own body box, and a wireframe seen from
        // the inside is just lines across the view.
        if own_body.is_some() && !view.shows_own_body() {
            continue;
        }
        for (hitbox, colour) in [(boxes.body, body_colour), (boxes.head, head_colour)] {
            gizmos.cube(
                Transform::from_translation(hitbox.centre).with_scale(hitbox.half_extents * 2.0),
                colour,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::skeleton::{humanoid, Pose, Proportions};

    /// A body spawned by anything at all ends up with boxes, without its
    /// spawner having to remember.
    #[test]
    fn every_rigged_body_is_given_hitboxes() {
        let mut world = World::new();
        let body = world
            .spawn((humanoid(Proportions::DEFAULT), Pose::rest()))
            .id();

        world.run_system_once(give_bodies_hitboxes).unwrap();

        assert!(world.get::<Hitboxes>(body).is_some());
    }

    /// The boxes follow the body around the map, and the body box stands on
    /// its feet rather than being centred on them.
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
        assert!((boxes.body.centre.xz() - feet.xz()).length() < 1e-4);
        assert!(boxes.head.centre.y > boxes.body.centre.y, "the head is not above the middle of the body");
    }

    /// A body that physics has moved is boxed where the *step* left it, not
    /// where the interpolated transform is drawing it. Feeding a deliberately
    /// mismatched pair is the only way to see which one was used.
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
            (boxes.body.centre.x - stepped.x).abs() < 1e-4,
            "boxed at {}, which is the drawn position, not the stepped one",
            boxes.body.centre.x
        );
    }
}
