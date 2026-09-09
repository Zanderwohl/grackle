use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use rand::seq::IndexedRandom;

use crate::common::app_mode::AppMode;
use crate::editor::multicam::Multicam;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::player::{
    fallback_spawn, gather_input, interpolate_bodies, mouse_look, place_camera, spawn_player,
    step_player, toggle_view, usable_spawns, Player, PlayerInput, Spawn, ViewMode,
};
use crate::tool::room::Room;

pub mod body_mesh;
pub mod collision;
pub mod hitbox;
pub mod player;
pub mod skeleton;

/// Playing the map that is currently open in the editor.
///
/// Deliberately small: this is the swap between editing and playing, plus
/// enough of a body to walk around and find out whether a room is the right
/// size. It is not the game — there is no health, no weapon, no network.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app
            .init_state::<AppMode>()
            .init_resource::<CollisionWorld>()
            .init_resource::<PlayerInput>()
            .init_resource::<ViewMode>()
            .add_systems(Update, toggle_mode)
            .add_systems(Update, toggle_view.run_if(in_state(AppMode::Play)))
            .add_systems(OnEnter(AppMode::Play), enter_play)
            .add_systems(OnExit(AppMode::Play), leave_play)
            // Aim and input sampling stay at frame rate — the first because
            // 64 Hz aim is latency you can feel, the second so a press made on
            // this frame reaches the steps taken on this frame.
            .add_systems(RunFixedMainLoop, (
                gather_input,
                mouse_look,
                // After `mouse_look`, which owns the rotation this reads.
                place_camera,
            ).chain().in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .run_if(in_state(AppMode::Play)))
            // Everything that decides where a body ends up runs at a fixed
            // rate, so the same inputs give the same trajectory whatever the
            // frame rate is doing. Prediction against a server needs that;
            // so does a browser tab whose refresh rate is anyone's guess.
            .add_systems(FixedUpdate, (
                rebuild_collision_when_rooms_change,
                step_player,
            ).chain().run_if(in_state(AppMode::Play)))
            // Draws the body between fixed steps, so a 64 Hz simulation does
            // not step visibly on a 144 Hz display.
            .add_systems(RunFixedMainLoop, interpolate_bodies
                .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop)
                .run_if(in_state(AppMode::Play)))
        ;
    }
}

/// `F5` both ways, as in Hammer.
///
/// Deliberately the only way back to the editor: `Escape` is spoken for by the
/// pause menu, and a key that sometimes pauses and sometimes throws you into
/// the editor would be worse than either.
fn toggle_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<State<AppMode>>,
    mut next: ResMut<NextState<AppMode>>,
) {
    if !keys.just_pressed(KeyCode::F5) {
        return;
    }
    next.set(match mode.get() {
        AppMode::Editor => AppMode::Play,
        AppMode::Play => AppMode::Editor,
    });
}

fn enter_play(
    mut commands: Commands,
    mut collision: ResMut<CollisionWorld>,
    rooms: Query<&Room>,
    spawns: Query<&Transform, With<SpawnPointMarker>>,
    mut editor_cameras: Query<&mut Camera, With<Multicam>>,
    window: Query<Entity, With<PrimaryWindow>>,
) {
    let rooms: Vec<Room> = rooms.iter().cloned().collect();
    // Before choosing, because whether a spawn point is usable is a question
    // about the walls.
    collision.rebuild(&rooms);

    // The feature writes its facing into the transform's rotation, so the
    // marked entity carries both halves and neither has to be looked up twice.
    let placed: Vec<Spawn> = spawns
        .iter()
        .map(|t| Spawn {
            feet: t.translation,
            yaw: t.rotation.to_euler(EulerRot::YXZ).0,
        })
        .collect();
    let usable = usable_spawns(&placed, &collision);
    if usable.len() < placed.len() {
        warn!(
            "{} of {} spawn point(s) have too little headroom to stand in",
            placed.len() - usable.len(),
            placed.len()
        );
    }

    // Uniformly at random for now. Per-team spawns, and not dropping someone
    // on top of someone else, are gamemode questions this is deliberately not
    // trying to answer yet.
    let spawn = match usable.choose(&mut rand::rng()) {
        Some(spawn) => *spawn,
        None => {
            warn!("No usable spawn point on this map; falling back to the largest room");
            fallback_spawn(&rooms)
        }
    };
    spawn_player(&mut commands, spawn);

    for mut camera in &mut editor_cameras {
        camera.is_active = false;
    }

    set_cursor_captured(&mut commands, &window, true);
    info!("Playing {} room(s); F5 to go back to editing", rooms.len());
}

fn leave_play(
    mut commands: Commands,
    players: Query<Entity, With<Player>>,
    mut editor_cameras: Query<&mut Camera, With<Multicam>>,
    window: Query<Entity, With<PrimaryWindow>>,
) {
    for player in &players {
        // Despawns the camera with it: it is a child of the body.
        commands.entity(player).despawn();
    }

    for mut camera in &mut editor_cameras {
        camera.is_active = true;
    }

    set_cursor_captured(&mut commands, &window, false);
}

/// The window is created with `primary_cursor_options: None`, so it has no
/// `CursorOptions` component to reach for — insert one rather than expecting
/// to find it.
fn set_cursor_captured(
    commands: &mut Commands,
    window: &Query<Entity, With<PrimaryWindow>>,
    captured: bool,
) {
    let Ok(window) = window.single() else { return };
    commands.entity(window).insert(CursorOptions {
        visible: !captured,
        grab_mode: if captured { CursorGrabMode::Locked } else { CursorGrabMode::None },
        ..default()
    });
}

/// Keep the solid world in step with the rooms while playing.
///
/// This is the between-round editing feature in its smallest form: change a
/// room and the thing you are standing in changes under you, with no reload
/// and no re-bake. `Changed` also fires on the frame a room is added, and
/// rebuilding is cheap enough at prototype map sizes that a whole-world
/// rebuild is the right trade for now. Removing a room is not detected —
/// `RemovedComponents` would cover it, and will have to once rooms can be
/// deleted mid-match.
fn rebuild_collision_when_rooms_change(
    mut collision: ResMut<CollisionWorld>,
    changed: Query<(), Changed<Room>>,
    rooms: Query<&Room>,
) {
    if changed.is_empty() {
        return;
    }
    let rooms: Vec<Room> = rooms.iter().cloned().collect();
    collision.rebuild(&rooms);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;
    use bevy::time::Virtual;

    use super::*;
    use crate::game::player::{PhysicsBody, PLAYER_HALF, SPAWN_YAW};

    /// One fixed step, matching Bevy's 64 Hz default.
    const STEP: Duration = Duration::from_micros(15625);

    /// Enough of an app to drive the swap and a few steps of walking, with no
    /// window and no renderer.
    ///
    /// Virtual time is paused so the fixed loop never advances on its own:
    /// these tests drive `FixedUpdate` by hand, which is the whole point of
    /// having moved physics there. A test whose result depended on how long
    /// the machine took to get to it would be testing the machine.
    fn headless(rooms: &[Room]) -> App {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::input::InputPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.add_plugins(GamePlugin);
        app.world_mut().resource_mut::<Time<Virtual>>().pause();

        for room in rooms {
            app.world_mut().spawn(room.clone());
        }
        app.update();
        app
    }

    fn enter(app: &mut App) {
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();
    }

    /// Take whole fixed steps, the way the fixed loop would.
    fn tick(app: &mut App, steps: usize) {
        for _ in 0..steps {
            app.world_mut().resource_mut::<Time>().advance_by(STEP);
            app.world_mut().run_schedule(FixedUpdate);
        }
    }

    /// The simulated position, which is the one physics writes. `Transform` is
    /// the drawn position and lags it by up to a step.
    fn player_position(app: &mut App) -> Vec3 {
        app.world_mut()
            .query_filtered::<&PhysicsBody, With<Player>>()
            .single(app.world())
            .expect("no player")
            .current
    }

    fn room(min: Vec3, max: Vec3) -> Room {
        Room::new(min, max)
    }

    #[test]
    fn the_editor_is_what_starts() {
        let mut app = headless(&[]);
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::Editor);
    }

    #[test]
    fn entering_play_spawns_a_body_in_the_largest_room() {
        let mut app = headless(&[
            room(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 3.0, 1.0)),
            room(Vec3::new(10.0, 0.0, -10.0), Vec3::new(30.0, 8.0, 10.0)),
        ]);
        enter(&mut app);

        let position = player_position(&mut app);
        assert!((position.x - 20.0).abs() < 0.01, "spawned at {position}");
        assert!((position.z - 0.0).abs() < 0.01, "spawned at {position}");
    }

    /// The swap has to be reversible without leaving anything behind — it
    /// happens between every round, not once.
    #[test]
    fn leaving_play_takes_the_body_with_it() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0))]);
        enter(&mut app);
        assert_eq!(app.world_mut().query::<&Player>().iter(app.world()).count(), 1);

        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Editor);
        app.update();

        assert_eq!(app.world_mut().query::<&Player>().iter(app.world()).count(), 0);
    }

    #[test]
    fn a_body_dropped_in_a_room_lands_on_its_floor() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 60);

        let position = player_position(&mut app);
        assert!(
            (position.y - PLAYER_HALF.y).abs() < 0.05,
            "came to rest at y = {}, expected about {}",
            position.y,
            PLAYER_HALF.y
        );
    }

    /// Gravity with nothing under it. Guards against a floor slab that is
    /// present but facing the wrong way, which would read as "landed" only
    /// because the body never moved.
    #[test]
    fn a_body_with_no_map_under_it_falls() {
        let mut app = headless(&[]);
        enter(&mut app);
        let start = player_position(&mut app);
        tick(&mut app, 60);

        assert!(player_position(&mut app).y < start.y - 1.0, "did not fall");
    }

    /// Holding jump has to keep bouncing: the input is read as held rather
    /// than as an edge, so a landing with the key still down takes off again
    /// instead of swallowing it.
    #[test]
    fn holding_jump_bounces_without_releasing() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 8.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 60);

        let rest = player_position(&mut app).y;

        // Count landings: back down at rest height having been well above it.
        let mut landings = 0;
        let mut airborne = false;
        for _ in 0..240 {
            // Held: re-latched every frame, as `gather_input` would.
            app.world_mut().resource_mut::<PlayerInput>().jump = true;
            tick(&mut app, 1);
            let y = player_position(&mut app).y;
            if y > rest + 0.3 {
                airborne = true;
            } else if airborne && (y - rest).abs() < 0.05 {
                landings += 1;
                airborne = false;
            }
        }

        assert!(landings >= 2, "bounced {landings} time(s) while jump was held");
    }

    /// A tap has to survive to the next fixed step. `FixedUpdate` runs zero
    /// times on a fast frame, and a press read straight from the keyboard in
    /// there would simply be gone.
    #[test]
    fn a_press_between_fixed_steps_is_not_lost() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 8.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 60);
        let rest = player_position(&mut app).y;

        // A frame that gathers input but takes no fixed step at all.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Space);
        app.world_mut().run_system_once(gather_input).unwrap();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::Space);
        app.world_mut().run_system_once(gather_input).unwrap();

        assert!(app.world().resource::<PlayerInput>().jump, "the press was dropped");

        tick(&mut app, 4);
        assert!(player_position(&mut app).y > rest + 0.05, "the latched press did not jump");
    }

    /// And it fires once, not once per step it survives into.
    #[test]
    fn a_latched_press_is_consumed_by_the_step_that_uses_it() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 8.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 60);

        app.world_mut().resource_mut::<PlayerInput>().jump = true;
        tick(&mut app, 1);

        assert!(!app.world().resource::<PlayerInput>().jump, "the latch survived its step");
    }

    /// The body is simulated at 64 Hz but drawn whenever the frame lands, so
    /// what is drawn has to be the position in between.
    #[test]
    fn the_drawn_position_sits_between_the_last_two_steps() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 20.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 10);

        // Mid-jump, so the two step positions differ. A body at rest on the
        // floor interpolates between two identical points and would pass this
        // however the maths was written.
        app.world_mut().resource_mut::<PlayerInput>().jump = true;
        tick(&mut app, 3);

        // A frame worth one and a half steps: the loop takes one and keeps
        // half a step of overstep, which is the gap the renderer has to draw
        // across. Driven through the real schedule rather than by poking the
        // clock, because `Time<Fixed>` only accumulates from inside it — and
        // this way the ordering of the three schedules is under test too.
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(STEP + STEP / 2);
        app.world_mut().run_schedule(RunFixedMainLoop);

        let body = *app
            .world_mut()
            .query_filtered::<&PhysicsBody, With<Player>>()
            .single(app.world())
            .unwrap();
        assert!(body.previous.y < body.current.y, "not moving; nothing to interpolate");

        let drawn = app
            .world_mut()
            .query_filtered::<&Transform, With<Player>>()
            .single(app.world())
            .unwrap()
            .translation;

        let midpoint = body.previous.lerp(body.current, 0.5);
        assert!((drawn.y - midpoint.y).abs() < 0.001, "drew {} not {}", drawn.y, midpoint.y);
    }

    /// Spawn the app with some spawn points placed, as the editor's feature
    /// sync would have left them.
    fn with_spawns(rooms: &[Room], feet: &[Vec3]) -> App {
        let mut app = headless(rooms);
        for position in feet {
            app.world_mut()
                .spawn((Transform::from_translation(*position), SpawnPointMarker));
        }
        app
    }

    #[test]
    fn a_spawn_point_is_used_in_preference_to_the_fallback() {
        let mut app = with_spawns(
            &[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))],
            &[Vec3::new(7.0, 0.0, -3.0)],
        );
        enter(&mut app);

        // Feet where the spawn point is; the body stands centred above it.
        let position = player_position(&mut app);
        assert!((position.x - 7.0).abs() < 0.01, "spawned at {position}");
        assert!((position.z + 3.0).abs() < 0.01, "spawned at {position}");
        assert!((position.y - PLAYER_HALF.y).abs() < 0.01, "feet are not on the point: {position}");
    }

    /// A spawn point with a ceiling too close over it cannot be stood in, and
    /// has to be passed over rather than used and then resolved by shoving the
    /// body somewhere the mapper did not choose.
    #[test]
    fn a_spawn_point_without_headroom_is_skipped() {
        let mut app = with_spawns(
            &[
                // A crawlspace, and a hall tall enough to stand in.
                room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(-10.0, 1.5, -10.0)),
                room(Vec3::new(0.0, 0.0, 0.0), Vec3::new(20.0, 8.0, 20.0)),
            ],
            &[Vec3::new(-15.0, 0.0, -15.0), Vec3::new(10.0, 0.0, 10.0)],
        );
        enter(&mut app);

        let position = player_position(&mut app);
        assert!((position.x - 10.0).abs() < 0.01, "spawned in the crawlspace: {position}");
        assert!((position.z - 10.0).abs() < 0.01, "spawned in the crawlspace: {position}");
    }

    /// Every spawn point unusable is not a reason to refuse to start.
    #[test]
    fn a_map_whose_spawns_are_all_too_tight_still_starts() {
        let mut app = with_spawns(
            &[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))],
            // Head in the ceiling.
            &[Vec3::new(0.0, 7.5, 0.0)],
        );
        enter(&mut app);

        assert_eq!(app.world_mut().query::<&Player>().iter(app.world()).count(), 1);
        let position = player_position(&mut app);
        assert!(position.y < 7.0, "used the unusable spawn anyway: {position}");
    }

    /// A spawn point's facing has to survive the whole trip: the feature
    /// writes it into the marked entity's rotation, `enter_play` reads it back
    /// out, and the body is turned by it. Anywhere along there it could be
    /// dropped and every spawn would silently face north again.
    ///
    /// `Player::yaw` matters as much as the transform, because `mouse_look`
    /// owns the rotation from the next frame on and drives it from that field.
    #[test]
    fn a_spawn_point_turns_the_body_it_spawns() {
        let yaw = std::f32::consts::FRAC_PI_2;
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        app.world_mut().spawn((
            Transform::from_translation(Vec3::ZERO).with_rotation(Quat::from_rotation_y(yaw)),
            SpawnPointMarker,
        ));
        enter(&mut app);

        let (player_yaw, rotation) = {
            let world = app.world_mut();
            let mut query = world.query::<(&Player, &Transform)>();
            let (player, transform) = query.single(world).unwrap();
            (player.yaw, transform.rotation)
        };
        assert!((player_yaw - yaw).abs() < 1e-5, "the body's yaw is {player_yaw}, not {yaw}");
        assert!(
            rotation.abs_diff_eq(Quat::from_rotation_y(yaw), 1e-5),
            "the yaw was written but the body was never turned by it"
        );
    }

    /// With no spawn point to ask, facing falls back to a fixed direction, and
    /// the body has to actually be turned that way rather than the yaw being
    /// written and never applied.
    #[test]
    fn a_spawned_body_faces_the_fixed_direction() {
        let mut app = with_spawns(
            &[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))],
            &[Vec3::new(0.0, 0.0, 0.0)],
        );
        enter(&mut app);

        let (player, transform) = {
            let world = app.world_mut();
            let mut query = world.query::<(&Player, &Transform)>();
            let (player, transform) = query.single(world).unwrap();
            (player.yaw, transform.rotation)
        };
        assert_eq!(player, SPAWN_YAW);
        assert!(transform.angle_between(Quat::from_rotation_y(SPAWN_YAW)) < 1e-5);
    }

    /// With several to choose from, it must not always be the same one.
    #[test]
    fn the_spawn_point_is_chosen_at_random() {
        let feet: Vec<Vec3> = (0..8).map(|i| Vec3::new(i as f32 * 2.0, 0.0, 0.0)).collect();

        let mut seen = std::collections::HashSet::new();
        for _ in 0..40 {
            let mut app = with_spawns(
                &[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))],
                &feet,
            );
            enter(&mut app);
            seen.insert(player_position(&mut app).x.round() as i32);
        }

        assert!(seen.len() > 1, "always picked the same spawn point: {seen:?}");
    }

    /// The between-round feature in miniature: resize the room while the body
    /// is standing in it, and the solid world follows without a reload.
    #[test]
    fn editing_a_room_while_playing_moves_the_ground() {
        // Tall enough that raising the floor still leaves room to stand: a
        // body in a gap exactly its own height cannot be placed with
        // clearance either side, which is a different situation entirely.
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 8.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 60);
        assert!((player_position(&mut app).y - PLAYER_HALF.y).abs() < 0.05);

        // Raise the floor by two metres, the way a drag handle would.
        let mut rooms = app.world_mut().query::<&mut Room>();
        for mut room in rooms.iter_mut(app.world_mut()) {
            room.min.y = 2.0;
        }
        tick(&mut app, 60);

        let position = player_position(&mut app);
        assert!(
            (position.y - (2.0 + PLAYER_HALF.y)).abs() < 0.05,
            "the ground did not move: resting at y = {}",
            position.y
        );
    }
}
