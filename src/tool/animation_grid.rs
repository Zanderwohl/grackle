//! Places [`AnimationGrid`] features, and stands up the bodies they describe.
//! The shared placement half is [`crate::tool::point_placement`]. The bodies
//! live here because a feature only has `Commands` and is re-applied on every
//! edit.

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
            // Not authority-gated: a client receives these bodies and has to
            // drop them when the grid goes.
            .add_systems(Update, clear_orphaned_carousels)
            .add_systems(
                Update,
                // Not mode-gated: the grid is map content, used in Play.
                // Authority-gated: the bodies have health, so they must come
                // from the authority and replicate, or each machine shoots its
                // own copy.
                (sync_animation_grids, team_carousels, follow_moved_grids, drive_carousels)
                    .chain()
                    .run_if(crate::common::net::has_authority),
            )
        ;
    }
}

/// One of the bodies a grid stands up. Carries its cell of [`cells`], so a
/// gap can be refilled without rebuilding the rest of the roster.
#[derive(Component, Debug)]
#[require(Hitboxes, Damageable)]
pub struct CarouselBody(pub usize);

/// Which grid a body belongs to, and where in it. Not `ChildOf`: as a child
/// its transform would be relative to a grid entity viewers do not have.
#[derive(Component, Debug, Clone, Copy)]
pub struct OfGrid {
    pub grid: Entity,
    pub cell: usize,
}

/// Put every carousel body into whatever the shared clock says the roster is
/// showing, as a [`ForcedAnimation`] so the change blends.
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

    // Logged once per shuffle: the order cannot be read off the bodies.
    if *announced != Some(showing.cycle) {
        *announced = Some(showing.cycle);
        let order: Vec<String> = crate::editor::animation_grid::order(showing.cycle)
            .iter()
            .map(|row| {
                // The same state appears at several speeds.
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

/// Stand up the missing cells of every grid's roster.
///
/// An empty grid is filled at once. A gap (a body shot) waits for the next
/// shuffle, when every body changes state anyway. Filling per cell keeps the
/// survivors' phases and blends. Checking which cells are present, not
/// `Added`, is what stops a roster stacking: the marker is re-inserted on
/// every edit.
pub fn sync_animation_grids(
    mut commands: Commands,
    clock: Res<AnimationClock>,
    grids: Query<(Entity, &Transform, Option<&RosterTeam>), With<AnimationGridMarker>>,
    bodies: Query<&OfGrid>,
    mut last_cycle: Local<Option<u64>>,
) {
    // The same boundary `drive_carousels` uses, so a refill lands on a shuffle.
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
        // Short but not empty: waits for the shuffle.
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
/// Compares before writing. `RosterTeam` reads as changed on every frame of a
/// drag, and a written `Team` makes `build_body_meshes` rebuild the body.
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

/// One body of the roster. Positioned by `PhysicsBody` in the world, so a
/// viewer receives the same facts a player's body is drawn from.
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
        // From the cell, so phases vary but every machine agrees.
        AnimationPhase::from_id(cell as u64),
        CarouselBody(cell),
        OfGrid { grid, cell },
        placed.class,
        // Per cell when striped; see `RosterTeam`.
        teams.team_for(cell),
        ForcedAnimation::default(),
        DisplaySpeed::default(),
        PhysicsBody::at(world),
        Transform::from_translation(world),
        Name::new(placed.class.name()),
    )
}

/// Keep a grid's bodies with it when it is moved.
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

/// Take a grid's bodies with it when the grid goes. `RemovedComponents`, so
/// a deleted grid, a replaced map and a round ending are one case.
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

    /// The front row only, which shows the width and height.
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

    /// Breaks if `team_carousels` writes unconditionally, rebuilding every
    /// body's meshes each frame of a drag.
    #[test]
    fn re_teaming_an_unchanged_roster_touches_nothing() {
        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        world.spawn((AnimationGridMarker, Transform::IDENTITY, RosterTeam::Striped));
        world.run_system_once(sync_animation_grids).unwrap();
        world.flush();

        // Change ticks, not a `Changed` filter: a fresh system's filter
        // matches everything.
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

    /// A gap is refilled at the next shuffle, not at once. Registered rather
    /// than `run_system_once`d, because the shuffle is remembered in a `Local`.
    #[test]
    fn a_body_shot_out_of_the_roster_comes_back_at_the_next_shuffle() {
        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        world.spawn((AnimationGridMarker, Transform::IDENTITY));
        let sync = world.register_system(sync_animation_grids);

        world.run_system(sync).unwrap();
        world.flush();
        assert_eq!(standing(&mut world).len(), cells().len());

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

        world.resource_mut::<AnimationClock>().advance(1000.0);
        world.run_system(sync).unwrap();
        world.flush();

        let back = standing(&mut world);
        assert_eq!(back.len(), cells().len(), "the shuffle did not refill the grid");
        assert!(back.contains(&3));
        assert_eq!(back, (0..cells().len()).collect::<Vec<_>>());
    }

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

    #[test]
    fn the_whole_line_is_shown_the_same_animation() {
        use crate::common::skeleton::{AnimationClock, DisplaySpeed};

        let mut world = grid_world();
        let mut clock = AnimationClock::default();
        // Part way through a rotation, on whole ticks.
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

    /// Breaks if re-inserting the marker on an edit stacks a second roster.
    #[test]
    fn syncing_a_grid_twice_does_not_double_it() {
        let mut world = grid_world();
        world.run_system_once(sync_animation_grids).unwrap();

        let mut query = world.query::<&ForcedAnimation>();
        assert_eq!(query.iter(&world).count(), cells().len());
    }

    /// Breaks if a client builds its own copies, which the server's shots
    /// never reach.
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

    /// The bodies are not children of the grid, and still go when it does.
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
