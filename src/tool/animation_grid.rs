//! Places [`AnimationGrid`] features, and stands up the bodies they describe.
//!
//! The point tool again, with the grid at the end of it — the shared half
//! lives in [`crate::tool::point_placement`]. The bodies live here rather than
//! in the feature for the same reason a prop's mesh does: a feature only has
//! `Commands`, and re-applying it on every edit would spawn the roster again
//! each time the point moved.

use bevy::prelude::*;

use crate::common::skeleton::{
    default_humanoid, draw_skeleton, humanoid, AnimationPhase, DisplaySpeed, ForcedAnimation,
    Pose, SkeletonAnimator, SkeletonPalette,
};
use crate::editor::animation_grid::{cells, AnimationGrid, AnimationGridMarker};
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
            .add_systems(Update, sync_animation_grids)
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
                    ForcedAnimation(cell.state),
                    // The row's own speed: the upright gait changes shape as
                    // it speeds up, so a walk and a run are two rows of the
                    // same state rather than one.
                    DisplaySpeed(cell.speed),
                    Transform::from_translation(cell.offset),
                    Name::new(format!(
                        "{} \u{2014} {} ({:.1})",
                        cell.class.name(),
                        cell.state.name(),
                        cell.speed
                    )),
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

    /// One body per cell, each a real skeleton pinned to its cell's state.
    #[test]
    fn a_placed_grid_stands_up_the_whole_roster() {
        let mut world = grid_world();

        let mut query = world.query::<(&Skeleton, &ForcedAnimation, &Transform)>();
        let bodies: Vec<(AnimationState, Vec3)> = query
            .iter(&world)
            .map(|(_, forced, transform)| (forced.0, transform.translation))
            .collect();

        assert_eq!(bodies.len(), cells().len());
        for cell in cells() {
            assert!(
                bodies.iter().any(|(state, offset)| *state == cell.state && *offset == cell.offset),
                "no body for {:?} in {:?}",
                cell.class,
                cell.state
            );
        }
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
