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
use crate::common::team::Team;
use crate::game::player::PhysicsBody;
use crate::common::skeleton::{
    default_humanoid, draw_skeleton, humanoid, AnimationPhase, DisplaySpeed, ForcedAnimation,
    Pose, SkeletonAnimator, SkeletonPalette,
};
use crate::common::skeleton::AnimationClock;
use crate::editor::animation_grid::{
    cells, showing, AnimationGrid, AnimationGridMarker, GridCell, RosterTeam,
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
            //
            // Gated on authority, though, and that is not about editing. The
            // bodies a grid stands up have health and hitboxes, so something
            // has to be believed about whether each one is alive. Built on
            // every machine from the same feature they would be *different
            // entities with the same shape*: shoot one on the server and the
            // client's copy — which nobody told anything — goes on standing
            // there. Bodies come from the authority, like every other body.
            //
            // `team_carousels` after the sync, so a body stood up this frame
            // is teamed this frame rather than spending one drawn in the
            // colour of whichever side it was last on.
            // Not gated on authority: a client receives these bodies, so it has
            // to clean them up when the grid they belong to goes away.
            .add_systems(Update, clear_orphaned_carousels)
            .add_systems(
                Update,
                (sync_animation_grids, team_carousels, follow_moved_grids, drive_carousels)
                    .chain()
                    .run_if(crate::common::net::has_authority),
            )
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

/// Which grid a body belongs to, and where in it.
///
/// A component rather than `ChildOf`, because these bodies stand in the world
/// rather than inside the grid entity. That is what lets one be replicated on
/// its own terms: a viewer is handed a body at a place doing an animation, and
/// needs to know nothing about grids, cells or rosters to draw it. As a child
/// its transform would be relative to a parent the viewer does not have.
#[derive(Component, Debug, Clone, Copy)]
pub struct OfGrid {
    pub grid: Entity,
    pub cell: usize,
}

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
    grids: Query<(Entity, &Transform, Option<&RosterTeam>), With<AnimationGridMarker>>,
    bodies: Query<&OfGrid>,
    mut last_cycle: Local<Option<u64>>,
) {
    // The same boundary `drive_carousels` announces on, read from the same
    // clock rather than counted here: a refill that happened on its own
    // schedule would arrive in the middle of a pose.
    let cycle = showing(clock.seconds()).cycle;
    let shuffled = last_cycle.replace(cycle) != Some(cycle);

    let cells = cells();
    for (grid, placed_at, teams) in &grids {
        let mut standing = vec![false; cells.len()];
        for body in &bodies {
            if body.grid != grid {
                continue;
            }
            if let Some(cell) = standing.get_mut(body.cell) {
                *cell = true;
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

        let teams = teams.copied().unwrap_or_default();
        for cell in missing {
            commands.spawn(carousel_body(grid, cell, &cells[cell], placed_at, teams));
        }
    }
}

/// Keep every standing body on the side its grid says it is on.
///
/// Separate from the spawn because the choice can change under bodies that are
/// already up, and rebuilding the roster to recolour it would take all sixty
/// phases and blends down with it.
///
/// **It compares before it writes**, which is the load-bearing part. The
/// feature's `apply_to_entity` runs on every edit, so the grid's `RosterTeam`
/// reads as changed on every frame of a drag; a system that trusted change
/// detection would write sixty `Team`s a frame, and a written `Team` is a
/// changed one, which is `build_body_meshes` throwing sixty bodies away and
/// rebuilding them for as long as the point is moving.
///
/// Walks the bodies and asks each which grid it is on, rather than walking a
/// grid's children: these bodies stand in the world so that they can be
/// replicated on their own terms, and `OfGrid` is what replaced the parenting.
pub fn team_carousels(
    grids: Query<&RosterTeam, With<AnimationGridMarker>>,
    mut bodies: Query<(&OfGrid, &mut Team), With<CarouselBody>>,
) {
    for (body, mut team) in &mut bodies {
        let Ok(teams) = grids.get(body.grid) else { continue };
        let wanted = teams.team_for(body.cell);
        if *team != wanted {
            *team = wanted;
        }
    }
}

/// One body of the roster, as the grid wants it.
///
/// Placed in the world rather than inside the grid, and its position carried
/// by `PhysicsBody` like every other body's. That is the whole of what makes
/// it replicable: what a viewer receives is a body somewhere, of some build,
/// on some side, doing something — the same facts a player's body is drawn
/// from, and not one word about carousels.
fn carousel_body(
    grid: Entity,
    cell: usize,
    placed: &GridCell,
    at: &Transform,
    teams: RosterTeam,
) -> impl Bundle {
    let world = at.transform_point(placed.offset);
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
        OfGrid { grid, cell },
        // What it is built like, and what it is doing. The two things a viewer
        // needs beyond a position, and both of them travel.
        placed.class,
        // Whatever the grid was told to be, which for a striped one is a
        // different side per cell — see `RosterTeam`. Deterministic like the
        // phase above and for the same reason: teams rolled locally would put
        // two viewers of one map out of step about who may shoot whom. It
        // travels too, so a viewer draws the side rather than guessing it.
        teams.team_for(cell),
        ForcedAnimation::default(),
        DisplaySpeed::default(),
        PhysicsBody::at(world),
        Transform::from_translation(world),
        Name::new(placed.class.name()),
    )
}

/// Keep a grid's bodies with it when it is moved.
///
/// The price of standing them in the world instead of inside the grid: a child
/// followed its parent for free. Cheap to pay, and only on the frame a mapper
/// actually drags the thing.
pub fn follow_moved_grids(
    grids: Query<(Entity, &Transform), (With<AnimationGridMarker>, Changed<Transform>)>,
    mut bodies: Query<(&OfGrid, &mut PhysicsBody, &mut Transform), Without<AnimationGridMarker>>,
) {
    if grids.is_empty() {
        return;
    }
    let cells = cells();
    for (grid, placed_at) in &grids {
        for (body, mut physics, mut transform) in &mut bodies {
            if body.grid != grid {
                continue;
            }
            let Some(cell) = cells.get(body.cell) else { continue };
            let world = placed_at.transform_point(cell.offset);
            *physics = PhysicsBody::at(world);
            transform.translation = world;
        }
    }
}

/// Take a grid's bodies with it when the grid goes.
///
/// `ChildOf` did this for free too. `RemovedComponents` rather than watching
/// the timeline, so it covers a grid deleted, a map replaced, and a round
/// ending alike.
pub fn clear_orphaned_carousels(
    mut commands: Commands,
    mut gone: RemovedComponents<AnimationGridMarker>,
    bodies: Query<(Entity, &OfGrid)>,
) {
    let gone: Vec<Entity> = gone.read().collect();
    if gone.is_empty() {
        return;
    }
    for (entity, body) in &bodies {
        if gone.contains(&body.grid) {
            commands.entity(entity).despawn();
        }
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
    use bevy::ecs::change_detection::Tick;
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

    /// A grid stands its roster up on the side it was told to, and a striped
    /// one deals the four round in cell order.
    #[test]
    fn a_roster_stands_up_on_the_side_it_was_told_to() {
        for choice in RosterTeam::choices() {
            let mut world = World::new();
            world.init_resource::<AnimationClock>();
            world.spawn((AnimationGridMarker, Transform::IDENTITY, choice));
            world.run_system_once(sync_animation_grids).unwrap();
            world.flush();

            let mut wrong: Vec<usize> = world
                .query::<(&CarouselBody, &Team)>()
                .iter(&world)
                .filter(|(body, team)| **team != choice.team_for(body.0))
                .map(|(body, _)| body.0)
                .collect();
            wrong.sort_unstable();
            assert!(wrong.is_empty(), "{choice:?} put cells {wrong:?} on the wrong side");
        }
    }

    /// Changing the choice re-teams the bodies that are already standing,
    /// rather than waiting for them to be shot and replaced.
    #[test]
    fn changing_the_choice_re_teams_the_bodies_already_standing() {
        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        let grid = world.spawn((AnimationGridMarker, Transform::IDENTITY, RosterTeam::Striped)).id();
        world.run_system_once(sync_animation_grids).unwrap();
        world.flush();

        world.entity_mut(grid).insert(RosterTeam::One(Team::Yellow));
        world.run_system_once(team_carousels).unwrap();

        assert!(
            world.query::<(&CarouselBody, &Team)>().iter(&world).all(|(_, team)| *team == Team::Yellow),
            "a body kept the side it was standing on"
        );
    }

    /// And a re-run that changes nothing writes nothing. The feature
    /// re-inserts its `RosterTeam` on every edit, so a system that wrote
    /// unconditionally would mark sixty `Team`s changed on every frame of a
    /// drag — and a changed `Team` is `build_body_meshes` throwing sixty
    /// bodies away and rebuilding them.
    #[test]
    fn re_teaming_an_unchanged_roster_touches_nothing() {
        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        world.spawn((AnimationGridMarker, Transform::IDENTITY, RosterTeam::Striped));
        world.run_system_once(sync_animation_grids).unwrap();
        world.flush();

        // Read off the components themselves rather than through a `Changed`
        // filter: a freshly registered system has never run, so every filter
        // in it matches everything and would pass this whatever happened.
        let ticks = |world: &mut World| -> Vec<Tick> {
            world
                .query::<(&CarouselBody, Ref<Team>)>()
                .iter(world)
                .map(|(_, team)| team.last_changed())
                .collect()
        };

        let before = ticks(&mut world);
        world.increment_change_tick();
        world.run_system_once(team_carousels).unwrap();

        assert_eq!(before, ticks(&mut world), "bodies were rewritten for no reason");
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
    /// A client builds none of the grid's bodies.
    ///
    /// They have health and hitboxes, so something has to be believed about
    /// whether each one is alive. Built on both ends from the same feature
    /// they are *different entities with the same shape*: shoot one on the
    /// server and the client's copy, which nobody told anything, goes on
    /// standing there. That was the symptom; two authorities over one object
    /// was the cause.
    #[test]
    fn a_client_stands_up_none_of_the_grids_bodies() {
        use crate::common::net::NetRole;

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.init_state::<crate::common::app_mode::AppMode>();
        app.add_plugins(AnimationGridPlugin);
        app.insert_resource(NetRole::Client { host: "elsewhere".into(), port: 27100 });
        app.world_mut().spawn((
            AnimationGridMarker,
            Transform::default(),
            Visibility::default(),
        ));
        app.update();

        assert_eq!(
            app.world_mut().query::<&CarouselBody>().iter(app.world()).count(),
            0,
            "the client built its own set of bodies"
        );
    }

    /// The bodies stand in the world, not inside the grid, and still go when
    /// it does.
    ///
    /// They were children, which took care of both for free. Standing them in
    /// the world is what lets one be replicated on its own terms — a viewer is
    /// handed a body at a place, and a child's transform is relative to a
    /// parent the viewer has never heard of — and this is the bookkeeping that
    /// buys.
    #[test]
    fn the_bodies_belong_to_the_grid() {
        let mut world = grid_world();

        let grid = {
            let mut query = world.query_filtered::<Entity, With<AnimationGridMarker>>();
            query.single(&world).unwrap()
        };
        assert_eq!(
            world.query::<&OfGrid>().iter(&world).filter(|b| b.grid == grid).count(),
            cells().len(),
            "the roster is not the grid's"
        );
        assert!(
            world.get::<Children>(grid).is_none(),
            "the bodies are still inside the grid, where a viewer cannot place them"
        );

        world.entity_mut(grid).despawn();
        world.run_system_once(clear_orphaned_carousels).unwrap();
        world.flush();
        let mut query = world.query::<&ForcedAnimation>();
        assert_eq!(query.iter(&world).count(), 0, "the roster outlived its grid");
    }

    /// Moving the grid takes its bodies with it. A child did this for free.
    #[test]
    fn a_moved_grid_takes_its_bodies_with_it() {
        let mut world = grid_world();
        let before = {
            let mut query = world.query_filtered::<&Transform, With<CarouselBody>>();
            query.iter(&world).next().unwrap().translation
        };

        {
            let grid = {
                let mut query = world.query_filtered::<Entity, With<AnimationGridMarker>>();
                query.single(&world).unwrap()
            };
            world.get_mut::<Transform>(grid).unwrap().translation = Vec3::new(30.0, 0.0, -12.0);
        }
        world.run_system_once(follow_moved_grids).unwrap();

        let after = {
            let mut query = world.query_filtered::<&Transform, With<CarouselBody>>();
            query.iter(&world).next().unwrap().translation
        };
        assert!(
            (after - before - Vec3::new(30.0, 0.0, -12.0)).length() < 1e-4,
            "the body did not move with its grid: {before} then {after}"
        );
    }
}
