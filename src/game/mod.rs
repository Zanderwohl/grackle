use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use rand::seq::IndexedRandom;

use crate::common::app_mode::{start_in_play, AppMode, StartInPlay};
use crate::common::damage::NextPlayerId;
use crate::common::net::NetRole;
use crate::common::skeleton::AnimationClock;
use crate::editor::multicam::Multicam;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::ragdoll::RagdollPlugin;
use crate::game::pause_menu::PauseMenuPlugin;
use crate::game::reset::reset_for_play;
use crate::game::player::{
    fallback_spawn, gather_input, interpolate_bodies, mouse_look, place_camera, spawn_player,
    give_the_local_body_a_camera, step_player, toggle_view, usable_spawns, write_client_inputs,
    InputLatch, LocalPlayer, Player, Spawn, ViewMode,
};
use crate::tool::bakes::BakeSystems;
use crate::tool::room::Room;

pub mod body_mesh;
pub mod collision;
pub mod damage;
pub mod death;
pub mod hitbox;
pub mod explosion;
pub mod flame;
pub mod hitscan;
pub mod projectile;
pub mod reset;
pub mod weapon;
pub mod player;
pub mod net_bodies;
pub mod pause_menu;
pub mod ragdoll;
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
            // What is left of a body once it stops holding itself up. Here
            // rather than beside the damage layer because it is physics: it
            // wants the same fixed steps and the same `CollisionWorld` the
            // player's own step does, and no renderer at all.
            .add_plugins(RagdollPlugin)
            // Escape, and the mouse comes back. Part of the game rather than
            // of the editor, so plain Bevy UI and no egui.
            .add_plugins(PauseMenuPlugin)
            .init_state::<AppMode>()
            .init_resource::<StartInPlay>()

            .init_resource::<CollisionWorld>()
            .init_resource::<NextPlayerId>()
            // Owned by `SkeletonPlugin`, initialised here as well because the
            // reset writes it and a game without the skeleton layer would
            // otherwise fail on the first F5 rather than at startup.
            .init_resource::<AnimationClock>()
            .init_resource::<InputLatch>()
            .init_resource::<ViewMode>()
            .add_systems(Update, (toggle_mode, start_in_play))
            .add_systems(Update, (
                toggle_view,
                // Not gated on being the one who spawned it: on a client the
                // body turns up from the server some time after the round
                // starts, and the camera has to follow it whenever it does.
                give_the_local_body_a_camera,
            ).run_if(in_state(AppMode::Play)))
            // Clear the table, then set it: a new match starts from a known
            // state rather than from whatever the last one left behind.
            // After `BakeSystems::All`, so the round is set up against a map
            // that has been rebuilt. The bake is what makes the map visible;
            // entering before it means a round played in a world you cannot
            // see, which is exactly what a client that has just been handed
            // the server's map would get.
            .add_systems(
                OnEnter(AppMode::Play),
                (reset_for_play, enter_play).chain().after(BakeSystems::All),
            )
            .add_systems(OnExit(AppMode::Play), leave_play)
            // Aim and input sampling stay at frame rate — the first because
            // 64 Hz aim is latency you can feel, the second so a press made on
            // this frame reaches the steps taken on this frame.
            .add_systems(RunFixedMainLoop, (
                // Not while the menu is up: a released cursor that still
                // walked the body would be worse than a captured one.
                gather_input.run_if(pause_menu::not_paused),
                // `mouse_look` is *not* gated — it has to drain the mouse
                // motion made while paused rather than save it up for the
                // frame the menu closes. It answers the question itself.
                mouse_look,
                // After `mouse_look`, which owns the rotation this reads.
                place_camera,
            ).chain().in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .run_if(in_state(AppMode::Play)))
            // Everything that decides where a body ends up runs at a fixed
            // rate, so the same inputs give the same trajectory whatever the
            // frame rate is doing. Prediction against a server needs that;
            // so does a browser tab whose refresh rate is anyone's guess.
            // The frame-rate latch becomes this tick's input here, in the set
            // Lightyear buffers and sends from. Everything downstream reads
            // that value and never consumes it, because rollback replays it.
            .add_systems(FixedPreUpdate, write_client_inputs
                .in_set(lightyear::prelude::client::input::InputSystems::WriteClientInputs)
                .run_if(in_state(AppMode::Play)))
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
///
/// **Only where this process holds authority.** A client is shown the match
/// the server is running: everyone drops into the editor between rounds
/// together and drops back into the round together, which only works if one
/// process owns the transition. The press is answered with a line in the log
/// rather than swallowed, because a key that silently does nothing reads as a
/// bug.
fn toggle_mode(
    keys: Res<ButtonInput<KeyCode>>,
    // Optional so that the game layer does not require the network layer: a
    // build or a test with no `NetPlugin` in it is a closed game, and a closed
    // game is its own authority.
    role: Option<Res<NetRole>>,
    mode: Res<State<AppMode>>,
    mut next: ResMut<NextState<AppMode>>,
) {
    if !keys.just_pressed(KeyCode::F5) {
        return;
    }
    if !role.is_none_or(|role| role.is_authority()) {
        info!("The server decides when the match is played; F5 does nothing here.");
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
    mut ids: ResMut<NextPlayerId>,
    role: Option<Res<NetRole>>,
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
    // Only where this process simulates. On a client the body is the
    // server's: it arrives replicated, and spawning one here would put a
    // second body in the world that nobody else can see and that the server
    // will never correct.
    if role.is_none_or(|role| role.is_authority()) {
        // A fresh id each time rather than one kept across F5: the body that
        // comes back is a new body, and a kill feed that reused the id would
        // credit its damage to the one before it.
        let body = spawn_player(&mut commands, spawn, ids.allocate());
        commands.entity(body).insert(LocalPlayer);
    }

    for mut camera in &mut editor_cameras {
        camera.is_active = false;
    }

    set_cursor_captured(&mut commands, &window, true);
    info!("Playing {} room(s); F5 to go back to editing", rooms.len());
}

fn leave_play(
    mut commands: Commands,
    role: Option<Res<NetRole>>,
    players: Query<Entity, With<Player>>,
    mut editor_cameras: Query<&mut Camera, With<Multicam>>,
    window: Query<Entity, With<PrimaryWindow>>,
) {
    // Only bodies this process owns. A client's bodies belong to the server,
    // which despawns them when the round ends and replicates that; despawning
    // them here as well would delete entities the receiver still has a record
    // of and expects to keep updating.
    if role.is_none_or(|role| role.is_authority()) {
        for player in &players {
            // Despawns the camera with it: it is a child of the body.
            commands.entity(player).despawn();
        }
    }

    for mut camera in &mut editor_cameras {
        camera.is_active = true;
    }

    set_cursor_captured(&mut commands, &window, false);
}

/// The window is created with `primary_cursor_options: None`, so it has no
/// `CursorOptions` component to reach for — insert one rather than expecting
/// to find it.
pub(crate) fn set_cursor_captured(
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
    use crate::common::class::Stance;
    use crate::game::player::{Inputs, LocalPlayer, PhysicsBody, PLAYER_HALF, SPAWN_YAW};

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
            // The drain first, as the real schedule has it: the latch becomes
            // this tick's input and only then is the step taken.
            app.world_mut().run_schedule(FixedPreUpdate);
            app.world_mut().run_schedule(FixedUpdate);
        }
    }

    /// The locally driven body's input, to poke at the way `gather_input`
    /// would. Filtered on `LocalPlayer` rather than `Player`, because a test
    /// may well have put another body in the world.
    fn input(app: &mut App) -> Mut<'_, Inputs> {
        let body = app
            .world_mut()
            .query_filtered::<Entity, With<LocalPlayer>>()
            .single(app.world())
            .expect("no local player");
        app.world_mut().get_mut::<Inputs>(body).expect("body has no input")
    }

    /// The simulated position, which is the one physics writes. `Transform` is
    /// the drawn position and lags it by up to a step.
    fn player_position(app: &mut App) -> Vec3 {
        app.world_mut()
            .query_filtered::<&PhysicsBody, With<LocalPlayer>>()
            .single(app.world())
            .expect("no local player")
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
            latch(&mut app).0.jump = true;
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

    /// The frame-rate latch, to poke the way `gather_input` would.
    fn latch(app: &mut App) -> Mut<'_, InputLatch> {
        app.world_mut().resource_mut::<InputLatch>()
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

        assert!(latch(&mut app).0.jump, "the press was dropped");

        tick(&mut app, 4);
        assert!(player_position(&mut app).y > rest + 0.05, "the latched press did not jump");
    }

    /// And it fires once, not once per step it survives into. The tick that
    /// takes the latch is what consumes it — not the step, which must be able
    /// to read the same input again when rollback replays it.
    #[test]
    fn the_tick_that_takes_the_latch_is_what_empties_it() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 8.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 60);

        latch(&mut app).0.jump = true;
        tick(&mut app, 1);

        assert!(!latch(&mut app).0.jump, "the latch survived the tick that took it");
        assert!(
            input(&mut app).jump,
            "the tick's own input was emptied; a rollback would replay a jump that never happened"
        );
    }

    /// The invariant the whole arrangement exists for: a tick's input can be
    /// read as many times as rollback needs to replay it, and says the same
    /// thing every time. A step that consumed what it read would replay as a
    /// step that was never asked to jump.
    #[test]
    fn replaying_a_tick_reads_the_same_input_again() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 20.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 60);
        let rest = player_position(&mut app).y;

        latch(&mut app).0.jump = true;
        tick(&mut app, 1);
        let once = player_position(&mut app).y;
        assert!(once > rest, "the jump did not happen the first time");

        // Replay the same tick from the same state, as a rollback would:
        // rewind the body and step again without touching the input.
        {
            let body = app
                .world_mut()
                .query_filtered::<Entity, With<LocalPlayer>>()
                .single(app.world())
                .unwrap();
            let mut physics = app.world_mut().get_mut::<PhysicsBody>(body).unwrap();
            *physics = PhysicsBody::at(Vec3::new(0.0, rest, 0.0));
            let mut player = app.world_mut().get_mut::<Player>(body).unwrap();
            player.velocity = Vec3::ZERO;
            player.on_ground = true;
        }
        app.world_mut().resource_mut::<Time>().advance_by(STEP);
        app.world_mut().run_schedule(FixedUpdate);

        assert!(
            (player_position(&mut app).y - once).abs() < 1e-5,
            "the replayed tick did not reach the same place"
        );
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
        latch(&mut app).0.jump = true;
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

    /// Aim travels as input and the step is what turns the body. `mouse_look`
    /// points the transform for the sake of the view, but a server replaying
    /// these inputs has only the yaw in them to go on — so the step has to be
    /// the thing that applies it.
    #[test]
    fn the_step_turns_the_body_to_the_aim_it_was_given() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 8.0, 5.0))]);
        enter(&mut app);

        let yaw = std::f32::consts::FRAC_PI_2;
        latch(&mut app).0.yaw = yaw;
        tick(&mut app, 1);

        let player = app
            .world_mut()
            .query::<&Player>()
            .single(app.world())
            .unwrap();
        assert!((player.yaw - yaw).abs() < 1e-5, "the body was not turned by its input");
    }

    /// And turning changes which way forward is, since movement is in the
    /// body's own frame. This is the half a server cannot guess: fed the
    /// movement without the aim it would walk the body the wrong way.
    #[test]
    fn aim_decides_which_way_forward_is() {
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        enter(&mut app);
        tick(&mut app, 60);
        let start = player_position(&mut app);

        // Facing a quarter turn to the left, walking forward.
        {
            let mut latch = latch(&mut app);
            latch.0.yaw = std::f32::consts::FRAC_PI_2;
            latch.0.movement = Vec2::new(0.0, 1.0);
        }
        tick(&mut app, 30);

        let moved = player_position(&mut app) - start;
        assert!(
            moved.x < -0.3 && moved.z.abs() < 0.1,
            "walked {moved} — forward did not follow the aim"
        );
    }

    /// A server spawns a body for everybody and drives none of them.
    ///
    /// This is the bug that produced a whole crop of unrelated-looking
    /// symptoms: `spawn_player` marked every body `LocalPlayer`, so on a host
    /// the second player to join made `gather_input` — a `Single` — match two
    /// entities and quietly stop running, froze the host in place, hid every
    /// body in the match, and gave each body its own camera. None of it
    /// errored.
    #[test]
    fn only_one_body_is_ever_the_local_one() {
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        enter(&mut app);
        spawn_a_second_body(&mut app);

        assert_eq!(
            app.world_mut().query::<&Player>().iter(app.world()).count(),
            2,
            "the second body was never spawned"
        );
        assert_eq!(
            app.world_mut().query::<&LocalPlayer>().iter(app.world()).count(),
            1,
            "more than one body is the local one; input silently stops working"
        );
    }

    /// And the one that is local is still driven. A `Single` that matches
    /// twice does not error, it skips — so this is the half that would go
    /// unnoticed.
    #[test]
    fn a_second_body_does_not_stop_the_first_being_driven() {
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        enter(&mut app);
        tick(&mut app, 60);
        spawn_a_second_body(&mut app);

        let start = player_position(&mut app);
        latch(&mut app).0.movement = Vec2::new(0.0, 1.0);
        tick(&mut app, 30);

        assert!(
            (player_position(&mut app) - start).length() > 0.3,
            "the local body stopped moving once a second body existed"
        );
    }

    /// Another player's body, as a server holding one would have it.
    fn spawn_a_second_body(app: &mut App) {
        let spawn = Spawn { feet: Vec3::new(5.0, 0.0, 0.0), yaw: 0.0 };
        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, app.world());
            spawn_player(&mut commands, spawn, crate::common::damage::PlayerId(99));
        }
        queue.apply(app.world_mut());
        app.update();
    }

    /// The reason the input moved onto the body at all: two bodies stepping in
    /// the same tick from different inputs. A single global input would walk
    /// both of them wherever the last writer said.
    #[test]
    fn two_bodies_step_from_their_own_inputs() {
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        enter(&mut app);

        // A second body beside the one the spawn made, as a server holding
        // somebody else's player would have.
        let other = app
            .world_mut()
            .spawn((
                Player::default(),
                Stance::default(),
                PhysicsBody::at(Vec3::new(5.0, PLAYER_HALF.y, 0.0)),
                Transform::from_xyz(5.0, PLAYER_HALF.y, 0.0),
            ))
            .id();
        tick(&mut app, 30);
        let other_start = app.world().get::<PhysicsBody>(other).unwrap().current;
        let own_start = player_position(&mut app);

        // Only the local body is asked to walk.
        latch(&mut app).0.movement = Vec2::new(0.0, 1.0);
        tick(&mut app, 30);

        let other_end = app.world().get::<PhysicsBody>(other).unwrap().current;
        assert!(
            (player_position(&mut app) - own_start).length() > 0.3,
            "the body that was asked to move did not"
        );
        assert!(
            (other_end - other_start).length() < 0.01,
            "the other body moved on somebody else's input"
        );
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
