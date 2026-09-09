//! Places [`AnimationGrid`] features, and stands up the bodies they describe.
//!
//! The point tool again, with the grid at the end of it — the shared half
//! lives in [`crate::tool::point_placement`]. The bodies live here rather than
//! in the feature for the same reason a prop's mesh does: a feature only has
//! `Commands`, and re-applying it on every edit would spawn the roster again
//! each time the point moved.

use bevy::prelude::*;

use crate::common::damage::Damageable;
use crate::common::hitbox::Hitboxes;
use crate::common::skeleton::{
    default_humanoid, draw_skeleton, humanoid, AnimationPhase, DisplaySpeed, ForcedAnimation,
    Pose, SkeletonAnimator, SkeletonPalette,
};
use crate::common::skeleton::AnimationClock;
use crate::editor::animation_grid::{
    cells, showing, AnimationGrid, AnimationGridMarker, GridCell,
};
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

/// One of the bodies a grid stands up, waiting to be told what to do.
///
/// Requires [`Hitboxes`] and [`Damageable`]: the grid is the harness they are
/// checked against — stand in front of it, watch sixty heads bob inside head
/// boxes that hardly move, and shoot one to see the number that comes off.
///
/// Carries which cell of [`cells`] it is, so a grid can tell a body it is
/// missing from one it still has. Without it, a body shot out of the roster
/// leaves a hole nothing can name, and refilling one gap means rebuilding the
/// whole grid — taking the other fifty-nine bodies' phases and blends down
/// with it.
#[derive(Component, Debug)]
#[require(Hitboxes, Damageable)]
pub struct CarouselBody(pub usize);

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

/// Stand up the roster under every grid that is short of one.
///
/// Children, so the whole grid moves and turns with its point for free and is
/// despawned with it — including on undo, which despawns the entity outright.
///
/// Two ways a grid comes to be short, filled at different moments on purpose:
///
/// - **Nothing standing at all**: the grid has just been placed, or has just
///   had its point nudged — the marker is re-inserted on every edit. It gets
///   its roster now, because a mapper staring at an empty patch of floor
///   cannot tell a grid that is waiting from a tool that did not work.
/// - **A gap in it**: a body was shot out of the roster. That is filled at the
///   next shuffle, when the whole grid changes state anyway. A body appearing
///   out of nothing mid-pose is a thing you notice; one appearing as
///   everything else changes is not.
///
/// Per cell rather than by rebuilding the roster, so the bodies that survived
/// keep their phases and their blends. Refilling one gap must not visibly
/// restart the other fifty-nine.
///
/// Checking the cells present rather than `Added` alone is also what stops a
/// grid stacking rosters: the marker is re-inserted on every edit, and a grid
/// that spawned a fresh sixty each time its point moved would be sixty bodies
/// deep in itself within a drag.
pub fn sync_animation_grids(
    mut commands: Commands,
    clock: Res<AnimationClock>,
    grids: Query<(Entity, Option<&Children>), With<AnimationGridMarker>>,
    bodies: Query<&CarouselBody>,
    mut last_cycle: Local<Option<u64>>,
) {
    // The same boundary `drive_carousels` announces on, read from the same
    // clock rather than counted here: a refill that happened on its own
    // schedule would arrive in the middle of a pose.
    let cycle = showing(clock.seconds()).cycle;
    let shuffled = last_cycle.replace(cycle) != Some(cycle);

    let cells = cells();
    for (grid, children) in &grids {
        let mut standing = vec![false; cells.len()];
        for child in children.into_iter().flat_map(|children| children.iter()) {
            if let Ok(body) = bodies.get(child) {
                if let Some(cell) = standing.get_mut(body.0) {
                    *cell = true;
                }
            }
        }

        let missing: Vec<usize> = (0..cells.len()).filter(|cell| !standing[*cell]).collect();
        if missing.is_empty() {
            continue;
        }
        // Short but not empty: somebody shot one. It waits for the shuffle.
        if missing.len() < cells.len() && !shuffled {
            continue;
        }

        commands.entity(grid).with_children(|parent| {
            for cell in missing {
                parent.spawn(carousel_body(cell, &cells[cell]));
            }
        });
    }
}

/// One body of the roster, as the grid wants it.
fn carousel_body(cell: usize, placed: &GridCell) -> impl Bundle {
    (
        humanoid(placed.class.proportions()),
        Pose::rest(),
        SkeletonAnimator::default(),
        // From the cell's place in the grid, which every machine that loads
        // this map computes the same way. Sixty bodies bouncing in lockstep
        // would read as one machine, and sixty bodies with locally rolled
        // phases would put two viewers of the same map out of step with each
        // other.
        AnimationPhase::from_id(cell as u64),
        // The one difference from a player: pinned to this cell's state
        // instead of being told what is happening to it. Told what to do by
        // `drive_carousels`, every frame.
        CarouselBody(cell),
        ForcedAnimation::default(),
        DisplaySpeed::default(),
        Transform::from_translation(placed.offset),
        Name::new(placed.class.name()),
    )
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
        world.init_resource::<AnimationClock>();
        world.spawn((AnimationGridMarker, Transform::IDENTITY));
        world.run_system_once(sync_animation_grids).unwrap();
        world.flush();
        world
    }

    fn standing(world: &mut World) -> Vec<usize> {
        let mut cells: Vec<usize> = world
            .query::<&CarouselBody>()
            .iter(world)
            .map(|body| body.0)
            .collect();
        cells.sort_unstable();
        cells
    }

    /// A body shot out of the grid is not replaced on the spot — that would
    /// be a body appearing out of nothing while you watched — but it is back
    /// by the next shuffle.
    ///
    /// Registered rather than `run_system_once`d, because the system remembers
    /// which shuffle it last saw in a `Local` and a fresh system would think
    /// every call was a new one.
    #[test]
    fn a_body_shot_out_of_the_roster_comes_back_at_the_next_shuffle() {
        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        world.spawn((AnimationGridMarker, Transform::IDENTITY));
        let sync = world.register_system(sync_animation_grids);

        world.run_system(sync).unwrap();
        world.flush();
        assert_eq!(standing(&mut world).len(), cells().len());

        // Shoot one out of the middle of the line.
        let shot = world
            .query::<(Entity, &CarouselBody)>()
            .iter(&world)
            .find(|(_, body)| body.0 == 3)
            .map(|(entity, _)| entity)
            .expect("no third cell");
        world.despawn(shot);

        world.run_system(sync).unwrap();
        world.flush();
        assert_eq!(
            standing(&mut world).len(),
            cells().len() - 1,
            "the body came back mid-pose instead of waiting for the shuffle"
        );

        // Well past a whole rotation of the roster.
        world.resource_mut::<AnimationClock>().advance(1000.0);
        world.run_system(sync).unwrap();
        world.flush();

        let back = standing(&mut world);
        assert_eq!(back.len(), cells().len(), "the shuffle did not refill the grid");
        // The same cell, in its own place, rather than a sixtieth body on the
        // end of the line.
        assert!(back.contains(&3));
        assert_eq!(back, (0..cells().len()).collect::<Vec<_>>());
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
