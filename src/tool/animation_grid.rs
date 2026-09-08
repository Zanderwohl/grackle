//! Places [`AnimationGrid`] features, and stands up the bodies they describe.
//!
//! The point tool again, with the grid at the end of it. The bodies live here
//! rather than in the feature for the same reason a prop's mesh does: a
//! feature only has `Commands`, and re-applying it on every edit would spawn
//! the roster again each time the point moved.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::skeleton::{
    default_humanoid, draw_skeleton, humanoid, ForcedAnimation, Pose, SkeletonAnimator,
    SkeletonPalette,
};
use crate::editor::animation_grid::{cells, AnimationGrid, AnimationGridMarker};
use crate::editor::editable::{FeatureId, FeatureTimeline, PointRef};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::tool::room::Room;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const DEFAULT_SNAP_GRANULARITY: f32 = 0.1;

#[derive(PartialEq, Eq, Clone, Copy)]
enum AnimationGridToolMode {
    Normal,
    Picking,
    RelativeSelected,
}

#[derive(Resource)]
struct AnimationGridTool {
    mode: AnimationGridToolMode,
    last_position: Vec3,
    cursor: Option<Vec3>,
    reference_feature: Option<FeatureId>,
    reference_key: String,
    reference_resolved: Option<Vec3>,
    hovered_point: Option<(FeatureId, String, Vec3)>,
    snap: bool,
    snap_granularity: f32,
}

impl Default for AnimationGridTool {
    fn default() -> Self {
        Self {
            mode: AnimationGridToolMode::Normal,
            last_position: Vec3::ZERO,
            cursor: None,
            reference_feature: None,
            reference_key: String::new(),
            reference_resolved: None,
            hovered_point: None,
            snap: true,
            snap_granularity: DEFAULT_SNAP_GRANULARITY,
        }
    }
}

pub struct AnimationGridPlugin;

impl Plugin for AnimationGridPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<AnimationGridTool>()
            .add_systems(Update, (
                AnimationGridTool::interface,
                AnimationGridTool::draw_gizmos,
            ).chain().run_if(in_state(Tools::AnimationGrid)).run_if(in_state(AppMode::Editor)))
            // Deliberately not gated on `AppMode::Editor`: the grid is map
            // content, and its whole use is judging an animation at the size
            // and distance a player sees it at, which means seeing it in play.
            .add_systems(Update, sync_animation_grids)
            .add_systems(OnExit(Tools::AnimationGrid), AnimationGridTool::on_exit)
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
            for cell in cells() {
                parent.spawn((
                    humanoid(cell.class.proportions()),
                    Pose::rest(),
                    SkeletonAnimator::default(),
                    // The one difference from a player: pinned to this cell's
                    // state instead of being told what is happening to it.
                    ForcedAnimation(cell.state),
                    Transform::from_translation(cell.offset),
                    Name::new(format!("{} \u{2014} {}", cell.class.name(), cell.state.name())),
                ));
            }
        });
    }
}

impl AnimationGridTool {
    fn on_exit(mut tool: ResMut<Self>) {
        tool.mode = AnimationGridToolMode::Normal;
        tool.cursor = None;
        tool.hovered_point = None;
        tool.reference_feature = None;
        tool.reference_key.clear();
        tool.reference_resolved = None;
    }

    fn interface(
        mut tool: ResMut<Self>,
        cameras: Query<(Entity, &Multicam)>,
        mouse_input: Res<CurrentMouseInput>,
        keys: Res<ButtonInput<KeyCode>>,
        mut features: ResMut<FeatureTimeline>,
        rooms: Query<&Room>,
        mut next_tool: ResMut<NextState<Tools>>,
    ) {
        let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let shift_just_pressed = keys.just_pressed(KeyCode::ShiftLeft) || keys.just_pressed(KeyCode::ShiftRight);

        tool.cursor = compute_cursor(
            &mouse_input, &cameras, tool.last_position,
            tool.snap, tool.snap_granularity, &rooms,
        );

        match tool.mode {
            AnimationGridToolMode::Normal => {
                if shift_held {
                    tool.mode = AnimationGridToolMode::Picking;
                    tool.hovered_point = None;
                } else if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        let grid = AnimationGrid::new(cursor.x, cursor.y, cursor.z);
                        let id = features.apply_feature(Box::new(grid));
                        features.select(Some(id));
                        tool.last_position = cursor;
                        next_tool.set(Tools::Select);
                    }
                }
            }
            AnimationGridToolMode::Picking => {
                if !shift_held {
                    tool.mode = AnimationGridToolMode::Normal;
                    tool.hovered_point = None;
                    return;
                }

                tool.hovered_point = mouse_input.world_pos
                    .and_then(|ray| find_hovered_point(&ray, &features, PICK_RADIUS));

                if mouse_input.released == Some(MouseButton::Left) {
                    if let Some((feature_id, key, resolved)) = tool.hovered_point.take() {
                        tool.reference_feature = Some(feature_id);
                        tool.reference_key = key;
                        tool.reference_resolved = Some(resolved);
                        tool.mode = AnimationGridToolMode::RelativeSelected;
                    }
                }
            }
            AnimationGridToolMode::RelativeSelected => {
                if shift_just_pressed {
                    tool.mode = AnimationGridToolMode::Normal;
                    tool.reference_feature = None;
                    tool.reference_key.clear();
                    tool.reference_resolved = None;
                    return;
                }

                if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        if let (Some(ref_feature), Some(ref_resolved)) = (tool.reference_feature, tool.reference_resolved) {
                            let d = cursor - ref_resolved;
                            let mut pr = PointRef::reference_with_offset(ref_feature, d.x, d.y, d.z);
                            if !tool.reference_key.is_empty() {
                                pr.point_key = tool.reference_key.clone();
                            }
                            let grid = AnimationGrid::from_point_ref(pr);
                            let id = features.apply_feature(Box::new(grid));
                            features.select(Some(id));
                            tool.last_position = cursor;
                            next_tool.set(Tools::Select);
                        }
                    }
                }
            }
        }
    }

    fn draw_gizmos(
        tool: Res<AnimationGridTool>,
        features: Res<FeatureTimeline>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        if let Some(cursor) = tool.cursor {
            let colour = match tool.mode {
                AnimationGridToolMode::RelativeSelected => Color::srgb_u8(200, 250, 200),
                _ => Color::srgb_u8(150, 230, 150),
            };

            // The front row only. Sixty bodies redrawn under a moving cursor
            // would be a preview that told a mapper less, not more — the row
            // shows the width and the height, which is what has to fit.
            for cell in cells().iter().filter(|cell| cell.offset.z == 0.0) {
                draw_skeleton(
                    &mut gizmos,
                    default_humanoid(),
                    &Pose::rest(),
                    &Transform::from_translation(cursor + cell.offset),
                    &SkeletonPalette::flat(colour),
                );
            }

            if tool.mode == AnimationGridToolMode::RelativeSelected {
                if let Some(base) = tool.reference_resolved {
                    draw_taxicab_path(&mut gizmos, base, cursor);
                }
            }
        }

        if tool.mode == AnimationGridToolMode::Picking {
            if let Some(ray) = mouse_input.world_pos {
                draw_picking_gizmos(&mut gizmos, &ray, &features, &tool.hovered_point);
            }
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
