//! Places [`AnimationGrid`] features, and stands up the bodies they describe.
//!
//! The point tool again, with the grid at the end of it — the shared half
//! lives in [`crate::tool::point_placement`]. The bodies live here rather than
//! in the feature for the same reason a prop's mesh does: a feature only has
//! `Commands`, and re-applying it on every edit would spawn the roster again
//! each time the point moved.

use bevy::prelude::*;

use crate::common::hitbox::Hitboxes;
use crate::common::skeleton::{
    default_humanoid, draw_skeleton, humanoid, AnimationPhase, DisplaySpeed, ForcedAnimation,
    Pose, SkeletonAnimator, SkeletonPalette,
};
use crate::common::skeleton::AnimationClock;
use crate::editor::animation_grid::{cells, showing, AnimationGrid, AnimationGridMarker};
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct AnimationGridPlugin;

impl Plugin for AnimationGridPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<AnimationGrid>(app);
        app
            // Deliberately not gated on `AppMode::Editor`: the grid is map
            // content, and its whole use is judging an animation at the size
            // and distance a player sees it at, which means seeing it in play.
            .add_systems(Update, (sync_animation_grids, drive_carousels).chain())
        ;
    }
}

/// Stand up the roster under every grid that does not have one yet.
///
/// Children, so the whole grid moves and turns with its point for free and is
/// despawned with it — including on undo, which despawns the entity outright.
///
/// Guarded on the grid having no bodies rather than on `Added` alone: the
/// marker is re-inserted whenever the feature is edited, and a grid that
/// spawned a second roster every time its point was nudged would be sixty
/// bodies deep in itself within a drag.
/// One of the bodies a grid stands up, waiting to be told what to do.
///
/// Requires [`Hitboxes`]: the grid is the harness they are checked against —
/// stand in front of it and watch sixty heads bob inside head boxes that
/// hardly move.
#[derive(Component, Debug)]
#[require(Hitboxes)]
pub struct CarouselBody;

/// Put every carousel body into whatever the shared clock says the roster is
/// showing.
///
/// All of them at once, which is the point: ten builds doing the same thing at
/// the same moment is the comparison worth having, and it costs ten bodies
/// rather than one per class per state.
///
/// Written as a [`ForcedAnimation`] like any other display, so the change goes
/// through the state machine and blends rather than cutting.
pub fn drive_carousels(
    clock: Res<AnimationClock>,
    mut bodies: Query<(&mut ForcedAnimation, &mut DisplaySpeed), With<CarouselBody>>,
    mut announced: Local<Option<u64>>,
) {
    if bodies.is_empty() {
        return;
    }

    let showing = showing(clock.seconds());
    for (mut forced, mut speed) in &mut bodies {
        forced.0 = showing.row.state;
        speed.0 = showing.row.speed;
    }

    // Once per shuffle, since the order is the one thing you cannot read off
    // the bodies themselves.
    if *announced != Some(showing.cycle) {
        *announced = Some(showing.cycle);
        let order: Vec<String> = crate::editor::animation_grid::order(showing.cycle)
            .iter()
            .map(|row| {
                // With the speed, since the same state at three speeds is
                // three of the entries and they are the ones worth telling
                // apart.
                if row.state.uses_gait() {
                    format!("{} ({:.1})", row.state.name(), row.speed)
                } else {
                    row.state.name()
                }
            })
            .collect();
        info!("Animation carousel: {}", order.join(", "));
    }
}

pub fn sync_animation_grids(
    mut commands: Commands,
    grids: Query<(Entity, Option<&Children>), Added<AnimationGridMarker>>,
) {
    for (grid, children) in &grids {
        if children.is_some_and(|children| !children.is_empty()) {
            continue;
        }

        commands.entity(grid).with_children(|parent| {
            for (index, cell) in cells().into_iter().enumerate() {
                parent.spawn((
                    humanoid(cell.class.proportions()),
                    Pose::rest(),
                    SkeletonAnimator::default(),
                    // From the cell's place in the grid, which every machine
                    // that loads this map computes the same way. Sixty bodies
                    // bouncing in lockstep would read as one machine, and
                    // sixty bodies with locally rolled phases would put two
                    // viewers of the same map out of step with each other.
                    AnimationPhase::from_id(index as u64),
                    // The one difference from a player: pinned to this cell's
                    // state instead of being told what is happening to it.
                    // Told what to do by `drive_carousels`, every frame.
                    CarouselBody,
                    ForcedAnimation::default(),
                    DisplaySpeed::default(),
                    Transform::from_translation(cell.offset),
                    Name::new(cell.class.name()),
                ));
            }
        });
    }
}

impl PlaceablePoint for AnimationGrid {
    const TOOL: Tools = Tools::AnimationGrid;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Box::new(AnimationGrid::new(position.x, position.y, position.z))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(AnimationGrid::from_point_ref(point_ref))
    }

    fn normal_color() -> Color {
        Color::srgb_u8(150, 230, 150)
    }

    fn relative_color() -> Color {
        Color::srgb_u8(200, 250, 200)
    }

    /// The front row only. Sixty bodies redrawn under a moving cursor would be
    /// a preview that told a mapper less, not more — the row shows the width
    /// and the height, which is what has to fit.
    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        for cell in cells().iter().filter(|cell| cell.offset.z == 0.0) {
            draw_skeleton(
                gizmos,
                default_humanoid(),
                &Pose::rest(),
                &Transform::from_translation(cursor + cell.offset),
                &SkeletonPalette::flat(color),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::skeleton::{AnimationState, Skeleton};

    fn grid_world() -> World {
        let mut world = World::new();
        world.spawn((AnimationGridMarker, Transform::IDENTITY));
        world.run_system_once(sync_animation_grids).unwrap();
        world
    }

    /// One body per cell, each a real skeleton standing where the line puts
    /// it. What it is doing comes later, from the clock.
    #[test]
    fn a_placed_grid_stands_up_the_whole_roster() {
        let mut world = grid_world();

        let mut query = world.query::<(&Skeleton, &CarouselBody, &Transform)>();
        let bodies: Vec<Vec3> = query
            .iter(&world)
            .map(|(_, _, transform)| transform.translation)
            .collect();

        assert_eq!(bodies.len(), cells().len());
        for cell in cells() {
            assert!(
                bodies.contains(&cell.offset),
                "no body standing where {:?} goes",
                cell.class
            );
        }
    }

    /// Every body is put into the same animation on the same tick — which is
    /// the whole reason ten of them are worth more than a hundred and forty.
    #[test]
    fn the_whole_line_is_shown_the_same_animation() {
        use crate::common::skeleton::{AnimationClock, DisplaySpeed};

        let mut world = grid_world();
        let mut clock = AnimationClock::default();
        // Far enough in to be part way through a rotation rather than at its
        // start, and on a whole number of ticks like the real one.
        for _ in 0..(64 * 9) {
            clock.advance(1.0 / 64.0);
        }
        let expected = showing(clock.seconds()).row;
        world.insert_resource(clock);
        world.run_system_once(drive_carousels).unwrap();

        let mut query = world.query::<(&ForcedAnimation, &DisplaySpeed)>();
        let shown: Vec<(AnimationState, f32)> = query
            .iter(&world)
            .map(|(forced, speed)| (forced.0, speed.0))
            .collect();

        assert_eq!(shown.len(), cells().len());
        assert!(
            shown.iter().all(|(state, speed)| *state == expected.state && *speed == expected.speed),
            "the line is not in step: {shown:?}"
        );
    }

    /// The marker is re-inserted on every edit of the feature, so the system
    /// has to be safe to run again on a grid that already has its bodies.
    /// Without the guard, dragging the point would bury the map in skeletons.
    #[test]
    fn syncing_a_grid_twice_does_not_double_it() {
        let mut world = grid_world();
        world.run_system_once(sync_animation_grids).unwrap();

        let mut query = world.query::<&ForcedAnimation>();
        assert_eq!(query.iter(&world).count(), cells().len());
    }

    /// The bodies are children, so the grid moves, turns and is deleted as one
    /// thing — undo despawns the feature's entity and the roster has to go
    /// with it.
    #[test]
    fn the_bodies_belong_to_the_grid() {
        let mut world = grid_world();

        let grid = {
            let mut query = world.query_filtered::<Entity, With<AnimationGridMarker>>();
            query.single(&world).unwrap()
        };
        assert_eq!(world.get::<Children>(grid).map(|c| c.len()), Some(cells().len()));

        world.entity_mut(grid).despawn();
        let mut query = world.query::<&ForcedAnimation>();
        assert_eq!(query.iter(&world).count(), 0, "the roster outlived its grid");
    }
}
