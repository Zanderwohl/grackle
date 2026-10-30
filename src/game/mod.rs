use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::common::app_mode::{start_in_play, AppMode, StartInPlay};
use crate::common::damage::NextPlayerId;
use crate::common::movement_rules::MovementRules;
use crate::common::net::NetRole;
use crate::common::skeleton::AnimationClock;
use crate::editor::multicam::Multicam;
use crate::game::collision::CollisionWorld;
use crate::game::ragdoll::RagdollPlugin;
use crate::game::pause_menu::PauseMenuPlugin;
use crate::game::reset::reset_for_play;
use crate::game::respawn::OurIdentity;
use crate::game::player::{
    aim_the_view_at_our_body, despawn_the_view, face_bodies, follow_our_corpse, gather_aim,
    gather_input, interpolate_bodies, place_camera, spawn_the_view, start_the_death_cam, step_player, toggle_view, write_client_inputs, InputLatch, Player, ViewMode,
    ViewPreference,
};
use crate::tool::bakes::BakeSystems;
use crate::tool::room::{Block, MapVolumes, Room, VolumeOrder};

pub mod body_mesh;
pub mod collision;
pub mod condition;
pub mod damage;
pub mod damage_direction;
pub mod death;
pub mod hat;
pub mod held;
pub mod hitbox;
pub mod explosion;
pub mod flame;
pub mod flinch;
pub mod hitscan;
pub mod projectile;
pub mod recoil;
pub mod reloading;
pub mod reset;
pub mod respawn;
pub mod weapon;
pub mod weapon_select;
pub mod weapon_select_hud;
pub mod player;
pub mod net_bodies;
pub mod pause_menu;
pub mod hud;
pub mod pickup_beam;
pub mod rounds;
pub mod score;
pub mod tab_board;
pub mod team_choice;
pub mod team_picker;
pub mod trigger;
pub mod ragdoll;
pub mod skeleton;
pub mod viewmodel;
pub mod world_prop;

/// Playing the map that is currently open in the editor.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app
            // Here rather than beside the damage layer: it is physics, on the
            // same fixed steps and `CollisionWorld` as the player step.
            .add_plugins(RagdollPlugin)
            .add_plugins(PauseMenuPlugin)
            .add_plugins(crate::game::hud::HudPlugin)
            .add_plugins(crate::game::score::ScorePlugin)
            .add_plugins(crate::game::rounds::RoundHistoryPlugin)
            .add_plugins(crate::game::tab_board::TabBoardPlugin)
            .add_plugins((crate::game::team_choice::TeamChoicePlugin, crate::game::team_picker::TeamPickerPlugin))
            .add_plugins(crate::game::damage_direction::DamageDirectionPlugin)
            .add_plugins(crate::game::trigger::TriggerPlugin)
            .init_state::<AppMode>()
            .init_resource::<StartInPlay>()

            // A browser build replaces this line and nothing else.
            .init_resource::<crate::common::assets::Assets>()
            // Here rather than in `WeaponPlugin` because `spawn_player` reads
            // it. On a client it is replaced by the server's on connect.
            .init_resource::<crate::common::weapon::WeaponCatalog>()
            .add_systems(
                Startup,
                crate::common::weapon_file::load_catalog_at_startup,
            )
            // Bodies are rigged from it; replaced by the server's on connect.
            .init_resource::<crate::common::class::ClassBuilds>()
            .add_systems(Startup, crate::common::class_file::load_builds_at_startup)
            .init_resource::<crate::common::taunt::Taunts>()
            .add_systems(Startup, crate::common::taunt::load_taunts_at_startup)
            .init_resource::<crate::common::game_rules::Modes>()
            .add_systems(Startup, crate::common::game_rules::load_modes_at_startup)
            .init_resource::<crate::common::game_rules::GameRules>()
            // A pack that ships no clips leaves bodies on the built-in poses.
            .add_systems(
                Startup,
                crate::common::clip_file::load_body_clips_at_startup,
            )

            .init_resource::<CollisionWorld>()
            .init_resource::<MovementRules>()
            .init_resource::<NextPlayerId>()
            // Owned by `SkeletonPlugin`, initialized here too because the
            // reset writes it: without that plugin the first F5 would fail.
            .init_resource::<AnimationClock>()
            .init_resource::<InputLatch>()
            .init_resource::<crate::game::weapon_select::WeaponSelection>()
            // Also `DamagePlugin`'s; here too because the death cam reads it.
            .add_message::<crate::common::damage::Died>()
            .init_resource::<ViewMode>()
            .init_resource::<ViewPreference>()
            .init_resource::<crate::game::player::CameraPull>()
            .add_plugins(crate::game::condition::ConditionPlugin)
            .add_systems(Update, (toggle_mode, start_in_play))
            .add_systems(Update, (
                toggle_view,
                // Before any body: the team picker is drawn on the view.
                spawn_the_view,
                start_the_death_cam,
            ).run_if(in_state(AppMode::Play)))
            // After `BakeSystems::All`: a round entered before the bake is
            // played in a map nobody can see, which is what a client just
            // handed the server's map would get.
            .add_systems(
                OnEnter(AppMode::Play),
                (reset_for_play, enter_play).chain().after(BakeSystems::All),
            )
            .add_systems(OnExit(AppMode::Play), (leave_play, despawn_the_view))
            // `PreUpdate` is the only place early enough: the body's mark
            // lands at the end of `Update`, and `write_client_inputs` runs in
            // the next frame's fixed loop before `Update` does. Any later and
            // the latch's zero overwrites the spawn point's facing.
            .add_systems(PreUpdate, aim_the_view_at_our_body.run_if(in_state(AppMode::Play)))
            .add_systems(RunFixedMainLoop, (
                // A released cursor must not walk the body.
                gather_input.run_if(pause_menu::not_paused),
                // Ungated: it drains the wheel, like `gather_aim`.
                crate::game::weapon_select::choose_weapons,
                // Ungated: it drains motion made while paused rather than
                // applying all of it on the frame the menu closes.
                gather_aim,
            ).chain().in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .run_if(in_state(AppMode::Play)))
            // Rollback replays this tick's input, so nothing downstream may
            // consume it.
            .add_systems(FixedPreUpdate, write_client_inputs
                .in_set(lightyear::prelude::client::input::InputSystems::WriteClientInputs)
                .run_if(in_state(AppMode::Play)))
            // Not authority-gated: a client predicts its own body
            // (`step_player` picks bodies by `Simulated`) and walks into its
            // own copy of the walls. Lightyear re-runs `FixedUpdate` to replay
            // a rollback, so nothing here may consume what it reads.
            .add_systems(FixedUpdate, (
                rebuild_collision_when_rooms_change,
                step_player,
            ).chain().run_if(in_state(AppMode::Play)))
            // `interpolate_bodies` is not gated on Play: a replicated display
            // body in an editor animation grid would otherwise sit at the
            // origin until a round starts.
            .add_systems(RunFixedMainLoop, interpolate_bodies
                .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop))
            .add_systems(RunFixedMainLoop, (face_bodies, follow_our_corpse, place_camera)
                .chain()
                .after(interpolate_bodies)
                .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop)
                .run_if(in_state(AppMode::Play)))
        ;
    }
}

/// `F5` swaps the map editor and Play, only where this process holds
/// authority. A client's press is logged rather than swallowed, because a key
/// that silently does nothing reads as a bug.
fn toggle_mode(
    keys: Res<ButtonInput<KeyCode>>,
    // Optional: a build or test without `NetPlugin` is a closed game, which is
    // its own authority.
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
        // No wildcard: a new mode should have to decide what F5 means to it.
        mode @ (AppMode::Prop | AppMode::Animation | AppMode::Material) => {
            info!(
                "F5 swaps the map editor and Play; use the Mode menu to leave {}.",
                mode.name(),
            );
            return;
        }
    });
}

fn enter_play(
    mut commands: Commands,
    mut collision: ResMut<CollisionWorld>,
    volumes: MapVolumes,
    mut editor_cameras: Query<&mut Camera, With<Multicam>>,
    window: Query<Entity, With<PrimaryWindow>>,
) {
    // First: `give_bodies_to_whoever_needs_one` asks it where a body can stand
    // on the next Update.
    let volumes = volumes.volumes();
    collision.rebuild_volumes(&volumes);

    // A new match is a new player: dropping the identity makes the next body
    // allocate a fresh id. Dying inside a match keeps it. A new match turns
    // the color wheel too.
    commands.remove_resource::<OurIdentity>();
    commands.remove_resource::<crate::game::respawn::HueStart>();


    for mut camera in &mut editor_cameras {
        camera.is_active = false;
    }

    set_cursor_captured(&mut commands, &window, true);
    info!("Playing {} volume(s); F5 to go back to editing", volumes.len());
}

fn leave_play(
    mut commands: Commands,
    role: Option<Res<NetRole>>,
    players: Query<Entity, With<Player>>,
    mut editor_cameras: Query<&mut Camera, With<Multicam>>,
    window: Query<Entity, With<PrimaryWindow>>,
) {
    // Only on the authority: a client's bodies are the server's to despawn,
    // and that despawn replicates.
    if role.is_none_or(|role| role.is_authority()) {
        for player in &players {
            commands.entity(player).despawn();
        }
    }
    // Kept, the next match's first body would inherit it.
    commands.remove_resource::<OurIdentity>();
    commands.remove_resource::<crate::game::respawn::HueStart>();
    commands.remove_resource::<crate::game::player::DeathCam>();
    commands.remove_resource::<crate::game::player::OurLastBody>();

    for mut camera in &mut editor_cameras {
        camera.is_active = true;
    }

    set_cursor_captured(&mut commands, &window, false);
}

/// The window is created with `primary_cursor_options: None`, so there is no
/// `CursorOptions` to find; insert one.
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

pub(crate) fn rebuild_collision_when_rooms_change(
    mut collision: ResMut<CollisionWorld>,
    changed: Query<(), Or<(Changed<Room>, Changed<Block>, Changed<VolumeOrder>)>>,
    mut removed_rooms: RemovedComponents<Room>,
    mut removed_blocks: RemovedComponents<Block>,
    volumes: MapVolumes,
) {
    // Drained either way, or a removal seen on a frame with a change is seen
    // again next frame.
    let removed = removed_rooms.read().count() + removed_blocks.read().count() > 0;
    if changed.is_empty() && !removed {
        return;
    }
    collision.rebuild_volumes(&volumes.volumes());
}

#[cfg(test)]
mod tests {
    use crate::constants::one_tick;

    use crate::common::team::Team;

    use crate::editor::spawn_point::SpawnPointMarker;
    use crate::game::player::{spawn_player, Spawn};

    use bevy::ecs::system::RunSystemOnce;
    use bevy::time::Virtual;

    use super::*;
    use crate::common::class::Stance;
    use crate::game::player::{Inputs, LocalPlayer, PhysicsBody, PLAYER_HALF, SPAWN_YAW};

/// Bevy's default 64 Hz.

    /// No window or renderer. Virtual time is paused so `FixedUpdate` runs
    /// only when a test drives it.
    fn headless(rooms: &[Room]) -> App {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::input::InputPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.add_plugins(GamePlugin);
        // Bodies are handed out by `NetBodiesPlugin`.
        app.insert_resource(crate::common::net::NetRole::Solo);
        app.add_plugins(crate::game::net_bodies::NetBodiesPlugin);
        app.world_mut().resource_mut::<Time<Virtual>>().pause();

        for room in rooms {
            app.world_mut().spawn(room.clone());
        }
        app.update();
        app
    }

    /// Into Play and onto a team, as the picker would: the first frame meets
    /// us, the second seats us and stands the body up.
    fn enter(app: &mut App) {
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();
        app.world_mut().write_message(crate::game::team_choice::ChooseTeam {
            choice: crate::game::team_choice::TeamChoice::Random,
            from: crate::game::respawn::Seat::Choosing,
        });
        app.update();
    }

    fn tick(app: &mut App, steps: usize) {
        for _ in 0..steps {
            app.world_mut().resource_mut::<Time>().advance_by(one_tick());
            // Inputs are written before the step, as in the real schedule.
            app.world_mut().run_schedule(FixedPreUpdate);
            app.world_mut().run_schedule(FixedUpdate);
        }
    }

    /// Filtered on `LocalPlayer`, since a test may add other bodies.
    fn input(app: &mut App) -> Mut<'_, Inputs> {
        let body = app
            .world_mut()
            .query_filtered::<Entity, With<LocalPlayer>>()
            .single(app.world())
            .expect("no local player");
        app.world_mut().get_mut::<Inputs>(body).expect("body has no input")
    }

    /// `PhysicsBody`, not `Transform`, which lags it by up to a step.
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

    /// F5 swaps the map editor and Play, and does nothing from the prop editor.
    #[test]
    fn f5_toggles_the_map_editor_and_play_and_nothing_else() {
        fn press(app: &mut App) {
            app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::F5);
            app.update();
            // Released, not only cleared: a held key produces no second edge.
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::F5);
            keys.clear();
            app.update();
        }
        fn mode(app: &App) -> AppMode {
            *app.world().resource::<State<AppMode>>().get()
        }

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_state::<AppMode>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Update, toggle_mode);

        press(&mut app);
        assert_eq!(mode(&app), AppMode::Play, "F5 did not start a round");
        press(&mut app);
        assert_eq!(mode(&app), AppMode::Editor, "F5 did not end the round");

        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Prop);
        app.update();
        press(&mut app);
        assert_eq!(mode(&app), AppMode::Prop, "F5 started a round from the prop editor");
    }

    #[test]
    fn the_editor_is_what_starts() {
        let app = headless(&[]);
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

    /// The swap happens every round, so it must leave nothing behind.
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

    /// A plane rising toward `+X` from the line `x = 0` on the floor, `degrees`
    /// steep, as two triangles; mirrored, it rises toward `-X` instead.
    fn slope(degrees: f32, mirrored: bool) -> [crate::game::collision::Hull; 2] {
        use crate::game::collision::Hull;
        let s = if mirrored { -1.0 } else { 1.0 };
        let rise = 5.0 * degrees.to_radians().tan();
        let (a, b) = (Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, 5.0));
        let (c, d) = (Vec3::new(5.0 * s, rise, 5.0), Vec3::new(5.0 * s, rise, -5.0));
        [Hull::triangle(a, b, c), Hull::triangle(a, c, d)]
    }

    /// A body dropped at `drop` onto whatever `hulls` add to a plain room, and
    /// left to come to rest.
    fn dropped_onto(hulls: impl IntoIterator<Item = crate::game::collision::Hull>, drop: Vec3) -> App {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 6.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 1);
        app.world_mut().resource_mut::<CollisionWorld>().add_hulls(hulls);
        let body = app.world_mut().query_filtered::<Entity, With<LocalPlayer>>().single(app.world()).unwrap();
        *app.world_mut().get_mut::<PhysicsBody>(body).unwrap() = PhysicsBody::at(drop);
        *app.world_mut().get_mut::<Player>(body).unwrap() = Player::default();
        tick(&mut app, 60);
        app
    }

    fn on_ground(app: &mut App) -> bool {
        app.world_mut().query_filtered::<&Player, With<LocalPlayer>>().single(app.world()).unwrap().on_ground()
    }

    #[test]
    fn a_body_on_a_slope_it_can_walk_stands_still_on_it() {
        let mut app = dropped_onto(slope(30.0, false), Vec3::new(2.0, 3.5, 0.0));
        assert!(on_ground(&mut app), "a 30° slope was not ground");
        let settled = player_position(&mut app);
        assert!(settled.y > PLAYER_HALF.y + 0.5, "fell through the slope to {settled}");

        tick(&mut app, 120);
        let later = player_position(&mut app);
        assert!((later - settled).length() < 1e-3, "slid from {settled} to {later}");
    }

    #[test]
    fn a_body_walks_up_a_slope_it_can_walk() {
        let mut app = dropped_onto(slope(30.0, false), Vec3::new(1.0, 2.5, 0.0));
        let start = player_position(&mut app);
        // Facing `-Z`; walking right is toward `+X`, up the slope.
        app.world_mut().resource_mut::<InputLatch>().0.movement = Vec2::new(1.0, 0.0);
        tick(&mut app, 30);
        let end = player_position(&mut app);
        assert!(end.x > start.x + 0.5 && end.y > start.y + 0.2, "from {start} to {end}");
        assert!(on_ground(&mut app), "walking up the slope left the ground");
    }

    /// All the way down to the floor at its foot, within the second
    /// `dropped_onto` waits: what slides off a wall keeps its speed.
    #[test]
    fn a_body_on_a_slope_too_steep_to_stand_on_slides_off_it() {
        let mut app = dropped_onto(slope(60.0, false), Vec3::new(2.0, 5.0, 0.0));
        let after = player_position(&mut app);
        assert!(after.x < 1.0 && after.y < PLAYER_HALF.y + 0.3, "stuck on a 60° slope at {after}");
        assert!(on_ground(&mut app), "never reached the floor under the slope");
    }

    /// Breaks if each surface is slid along in turn: every slide down one
    /// side of the V pushes into the other.
    #[test]
    fn a_body_in_a_v_comes_to_rest_rather_than_jittering() {
        let hulls = slope(60.0, false).into_iter().chain(slope(60.0, true));
        let mut app = dropped_onto(hulls, Vec3::new(0.3, 4.0, 0.0));
        let settled = player_position(&mut app);
        assert!(settled.x.abs() < 0.01, "not in the bottom of the V: {settled}");

        tick(&mut app, 60);
        let later = player_position(&mut app);
        assert!((later - settled).length() < 1e-4, "jittered from {settled} to {later}");
    }

    /// A body standing at `x = 3` in a room with its floor at zero, walking
    /// toward `+X` into whatever else `rooms` adds, for two seconds.
    fn walked_towards(rooms: &[Room]) -> App {
        let mut all = vec![room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0))];
        all.extend_from_slice(rooms);
        let mut app = headless(&all);
        enter(&mut app);
        tick(&mut app, 1);
        let body = app.world_mut().query_filtered::<Entity, With<LocalPlayer>>().single(app.world()).unwrap();
        *app.world_mut().get_mut::<PhysicsBody>(body).unwrap() = PhysicsBody::at(Vec3::new(3.0, PLAYER_HALF.y + 0.01, 0.0));
        *app.world_mut().get_mut::<Player>(body).unwrap() = Player::default();
        tick(&mut app, 10);
        app.world_mut().resource_mut::<InputLatch>().0.movement = Vec2::new(1.0, 0.0);
        tick(&mut app, 128);
        app
    }

    #[test]
    fn a_body_walks_up_a_flight_of_stairs() {
        let mut app = walked_towards(&[
            room(Vec3::new(5.0, 0.3, -5.0), Vec3::new(6.0, 4.0, 5.0)),
            room(Vec3::new(6.0, 0.6, -5.0), Vec3::new(7.0, 4.0, 5.0)),
            room(Vec3::new(7.0, 0.9, -5.0), Vec3::new(12.0, 4.0, 5.0)),
        ]);
        let end = player_position(&mut app);
        assert!(end.x > 7.5, "stopped at the stairs: {end}");
        assert!((end.y - (0.9 + PLAYER_HALF.y)).abs() < 0.01, "not standing on the top step: {end}");
        assert!(on_ground(&mut app));
    }

    #[test]
    fn a_ledge_taller_than_a_step_stops_a_body() {
        let mut app = walked_towards(&[room(Vec3::new(5.0, 0.6, -5.0), Vec3::new(10.0, 4.0, 5.0))]);
        let end = player_position(&mut app);
        assert!(end.x < 5.0, "walked up a 0.6 m ledge: {end}");
        assert!((end.y - PLAYER_HALF.y).abs() < 0.01, "{end}");
    }

    /// Breaks if a failed step is taken anyway: the body would hop on the spot.
    #[test]
    fn pressing_into_a_wall_neither_climbs_nor_hops() {
        let mut app = walked_towards(&[]);
        let end = player_position(&mut app);
        assert!((end.x - (5.0 - PLAYER_HALF.x)).abs() < 0.01 && (end.y - PLAYER_HALF.y).abs() < 0.01, "{end}");
        tick(&mut app, 10);
        assert!((player_position(&mut app) - end).length() < 1e-4, "moved while pressed to the wall");
        let body = app.world_mut().query_filtered::<&Player, With<LocalPlayer>>().single(app.world()).unwrap();
        assert!(body.against_wall);
    }

    /// Catches a floor slab facing the wrong way, which would pass the landing
    /// test only because the body never moved.
    #[test]
    fn a_body_with_no_map_under_it_falls() {
        let mut app = headless(&[]);
        enter(&mut app);
        let start = player_position(&mut app);
        tick(&mut app, 60);

        assert!(player_position(&mut app).y < start.y - 1.0, "did not fall");
    }

    /// Jump is read as held, so landing with it down takes off again.
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

    fn latch(app: &mut App) -> Mut<'_, InputLatch> {
        app.world_mut().resource_mut::<InputLatch>()
    }

    /// `FixedUpdate` runs zero times on a fast frame; a press must survive to
    /// the next step.
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

    /// The tick that takes the latch empties it; the step must not, so a
    /// rollback can replay it.
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

    /// A step that consumed its input would replay as one that never jumped.
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

        // Rewind the body and step again without touching the input.
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
            player.ground = Some(Vec3::Y);
        }
        app.world_mut().resource_mut::<Time>().advance_by(one_tick());
        app.world_mut().run_schedule(FixedUpdate);

        assert!(
            (player_position(&mut app).y - once).abs() < 1e-5,
            "the replayed tick did not reach the same place"
        );
    }

    /// `G` starts the shipped wave, which stops a walking body and its gun;
    /// `G` again gives both back, and so does waiting it out.
    #[test]
    fn the_wave_stops_you_walking_and_g_again_lets_you_go() {
        use crate::game::condition::Condition;

        let mut app = headless(&[room(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 4.0, 10.0))]);
        enter(&mut app);
        tick(&mut app, 30);
        let condition = |app: &mut App| {
            *app.world_mut()
                .query_filtered::<&Condition, With<LocalPlayer>>()
                .single(app.world())
                .unwrap()
        };
        // Coasting to a stop afterward, so a walk does not carry into the next.
        let walked = |app: &mut App| {
            latch(app).0.movement = Vec2::Y;
            let from = player_position(app);
            tick(app, 16);
            latch(app).0.movement = Vec2::ZERO;
            // Half a second stops a sprint; short enough to stay inside the
            // wave's 1.2 s with the turn below.
            tick(app, 40);
            (player_position(app) - from).with_y(0.0).length()
        };
        let toggle = |app: &mut App| {
            latch(app).0.taunt = true;
            tick(app, 1);
        };

        assert!(walked(&mut app) > 0.5, "the body would not walk before taunting");

        toggle(&mut app);
        let taunting = condition(&mut app);
        assert!(taunting.taunt.is_some(), "G started no taunt");
        assert!(!taunting.may_fire());
        assert!(walked(&mut app) < 1e-4, "the wave did not stop the body");

        // The aim turns and the body does not; the view follows the aim.
        let body_yaw = |app: &mut App| {
            app.world_mut()
                .query_filtered::<&Player, With<LocalPlayer>>()
                .single(app.world())
                .unwrap()
                .yaw
        };
        let facing = body_yaw(&mut app);
        latch(&mut app).0.yaw += 1.0;
        tick(&mut app, 4);
        assert_eq!(body_yaw(&mut app), facing, "the wave let the body turn");

        toggle(&mut app);
        assert_eq!(condition(&mut app).taunt, None, "G again did not end the taunt");
        assert_ne!(body_yaw(&mut app), latch(&mut app).0.yaw, "the body snapped to the aim in one tick");
        tick(&mut app, 8);
        assert_eq!(body_yaw(&mut app), latch(&mut app).0.yaw, "the body did not turn back to the aim");
        assert!(walked(&mut app) > 0.5, "the body stayed stopped after the taunt");

        // Left alone, it ends by itself after its length.
        toggle(&mut app);
        assert!(condition(&mut app).taunt.is_some());
        tick(&mut app, 2 * crate::constants::TICK_HZ as usize);
        assert_eq!(condition(&mut app).taunt, None, "the wave never ended by itself");
    }

    /// Rises until it stops, and returns how far above `from` it peaked.
    fn peak_above(app: &mut App, from: f32) -> f32 {
        let mut peak = from;
        for _ in 0..(2 * crate::constants::TICK_HZ as usize) {
            tick(app, 1);
            peak = peak.max(player_position(app).y);
        }
        peak - from
    }

    fn our_id(app: &mut App) -> crate::common::damage::PlayerId {
        *app.world_mut()
            .query_filtered::<&crate::common::damage::PlayerId, With<LocalPlayer>>()
            .single(app.world())
            .unwrap()
    }

    /// A headless game that can set things off. The body is boxed by hand:
    /// its boxes come from a rig, which a headless game does not build.
    fn armed(rooms: &[Room]) -> App {
        let mut app = headless(rooms);
        // `explode` alone: the plugin also draws, and a headless game has no gizmos.
        app.add_plugins(crate::game::damage::DamagePlugin)
            .add_message::<crate::common::damage::Explosion>()
            .add_systems(
                FixedUpdate,
                crate::game::explosion::explode.in_set(crate::game::damage::DamageSystems::Deal),
            );
        app
    }

    fn rocket_at_our_feet(app: &mut App) {
        use crate::common::damage::{DamageSource, Explosion};
        use crate::common::hitbox::{Box3, Hitboxes};
        let center = player_position(app);
        let body = app.world_mut().query_filtered::<Entity, With<LocalPlayer>>().single(app.world()).unwrap();
        app.world_mut().entity_mut(body).insert(Hitboxes {
            body: Box3 { center, half_extents: PLAYER_HALF },
            head: None,
        });
        let feet = center - Vec3::Y * (PLAYER_HALF.y - 0.1);
        let source = DamageSource::Player(our_id(app));
        app.world_mut().resource_mut::<Messages<Explosion>>().write(Explosion {
            at: feet,
            // The shipped rocket launcher's.
            radius: 3.0,
            damage: 90,
            knockback: 9.0,
            source,
            direct: None,
        });
    }

    fn health(app: &mut App) -> u32 {
        app.world_mut()
            .query_filtered::<&crate::common::damage::Damageable, With<LocalPlayer>>()
            .single(app.world())
            .unwrap()
            .health()
    }

    #[test]
    fn a_rocket_at_your_feet_lifts_you_higher_than_a_jump() {
        let open = [room(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 30.0, 10.0))];

        let mut app = headless(&open);
        enter(&mut app);
        tick(&mut app, 30);
        let standing = player_position(&mut app).y;
        latch(&mut app).0.jump = true;
        tick(&mut app, 1);
        latch(&mut app).0.jump = false;
        let jumped = peak_above(&mut app, standing);

        let mut app = armed(&open);
        enter(&mut app);
        tick(&mut app, 30);
        let standing = player_position(&mut app).y;
        rocket_at_our_feet(&mut app);
        let launched = peak_above(&mut app, standing);

        assert!(jumped > 0.5, "the jump reached only {jumped}");
        assert!(launched > jumped, "a rocket at the feet reached {launched}, a jump {jumped}");
    }

    #[test]
    fn hurting_yourself_costs_what_the_mode_says() {
        let open = [room(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 30.0, 10.0))];
        let cost = |self_damage: f32| {
            let mut app = armed(&open);
            app.insert_resource(crate::common::game_rules::GameRules(crate::common::game_rules::ModeRules {
                self_damage,
                ..default()
            }));
            enter(&mut app);
            tick(&mut app, 30);
            let before = health(&mut app);
            rocket_at_our_feet(&mut app);
            tick(&mut app, 1);
            before - health(&mut app)
        };
        let full = cost(1.0);
        assert!(full > 0, "a rocket at our feet did no damage");
        assert_eq!(cost(0.5), (full as f32 * 0.5).round() as u32);
    }

    /// Simulated at 64 Hz, drawn between the last two steps.
    #[test]
    fn the_drawn_position_sits_between_the_last_two_steps() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 20.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 10);

        // Mid-jump, so the two step positions differ; at rest any
        // interpolation would pass.
        latch(&mut app).0.jump = true;
        tick(&mut app, 3);

        // One and a half steps, through the real schedule: `Time<Fixed>` only
        // accumulates from inside it.
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(one_tick() + one_tick() / 2);
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

    /// Breaks if a body standing still is written with what it already holds:
    /// a resend to every client and a propagation through every bone mesh.
    #[test]
    fn a_body_standing_still_is_not_marked_changed() {
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 20.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 60);
        let body = app
            .world_mut()
            .query_filtered::<Entity, With<LocalPlayer>>()
            .single(app.world())
            .unwrap();

        // A full frame, so the drawn position settles too.
        let frame = |app: &mut App| {
            app.world_mut().resource_mut::<Time<Virtual>>().advance_by(one_tick());
            app.world_mut().run_schedule(RunFixedMainLoop);
            app.world_mut().run_schedule(Update);
        };
        frame(&mut app);

        fn stamps(app: &App, body: Entity) -> Vec<(&'static str, bevy::ecs::change_detection::Tick)> {
            let entity = app.world().entity(body);
            let mut stamps = vec![
                ("PhysicsBody", entity.get_ref::<PhysicsBody>().unwrap().last_changed()),
                ("Player", entity.get_ref::<Player>().unwrap().last_changed()),
                ("Stance", entity.get_ref::<Stance>().unwrap().last_changed()),
                ("Transform", entity.get_ref::<Transform>().unwrap().last_changed()),
            ];
            if let Some(requests) = entity.get_ref::<crate::common::skeleton::BodyRequests>() {
                stamps.push(("BodyRequests", requests.last_changed()));
            }
            stamps
        }
        let before = stamps(&app, body);
        tick(&mut app, 5);
        frame(&mut app);
        assert_eq!(stamps(&app, body), before, "an idle body was written with what it already held");

        latch(&mut app).0.movement = Vec2::Y;
        tick(&mut app, 1);
        let after = stamps(&app, body);
        assert_ne!(after[0], before[0], "a moving body's position was not written");
    }

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

        // Feet where the spawn point is; the body stands centered above it.
        let position = player_position(&mut app);
        assert!((position.x - 7.0).abs() < 0.01, "spawned at {position}");
        assert!((position.z + 3.0).abs() < 0.01, "spawned at {position}");
        assert!((position.y - PLAYER_HALF.y).abs() < 0.01, "feet are not on the point: {position}");
    }

    /// A spawn point the body does not fit is skipped, not used and then
    /// depenetrated somewhere the mapper did not choose.
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

    /// A spawn point's facing reaches both `Player::yaw` and the transform.
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

    /// Without a facing, the body is turned to `SPAWN_YAW`, not only given it.
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

    /// A server replaying inputs has only the yaw in them, so the step must
    /// apply it.
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

    /// Movement is in the body's frame, so a server fed movement without aim
    /// walks the body the wrong way.
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

    /// Hitboxes are built from the body's transform, so an unturned remote body
    /// is boxed facing the wrong way and visible hits do not register.
    #[test]
    fn a_body_we_do_not_aim_for_is_turned_to_where_it_is_looking() {
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        enter(&mut app);
        spawn_a_second_body(&mut app);

        let yaw = std::f32::consts::FRAC_PI_2;
        let theirs = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, (With<Player>, Without<LocalPlayer>)>();
            query.single(world).expect("no second body")
        };
        // As replication would have delivered it.
        app.world_mut().get_mut::<Player>(theirs).unwrap().yaw = yaw;
        app.update();

        assert!(
            app.world()
                .get::<Transform>(theirs)
                .unwrap()
                .rotation
                .abs_diff_eq(Quat::from_rotation_y(yaw), 1e-5),
            "the body was not turned to the yaw it reported"
        );
    }

    /// `face_bodies` is the only writer of every body's rotation, ours included.
    #[test]
    fn our_own_body_is_turned_by_its_yaw_like_any_other() {
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        enter(&mut app);

        let mine = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<LocalPlayer>>();
            query.single(world).expect("no local body")
        };
        let yaw = 1.2;
        app.world_mut().get_mut::<Player>(mine).unwrap().yaw = yaw;
        app.world_mut().run_system_once(face_bodies).unwrap();

        assert!(
            app.world()
                .get::<Transform>(mine)
                .unwrap()
                .rotation
                .abs_diff_eq(Quat::from_rotation_y(yaw), 1e-5),
            "our own body was left facing somewhere else"
        );
    }

    /// `step_player` must pick bodies by `Simulated`; stepping all of them, a
    /// client disagrees with the server about every body and rolls back for ever.
    #[test]
    fn a_body_we_do_not_simulate_is_left_where_the_server_put_it() {
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        enter(&mut app);

        // As a replicated body arrives: no `Simulated`.
        let placed = Vec3::new(5.0, 4.0, 0.0);
        let theirs = app
            .world_mut()
            .spawn((Player::default(), PhysicsBody::at(placed)))
            .id();
        tick(&mut app, 60);

        assert_eq!(
            app.world().get::<PhysicsBody>(theirs).unwrap().current,
            placed,
            "we stepped somebody else's body, and gravity took it"
        );
    }

    /// The latch is seeded from the body; left at zero, the first tick's input
    /// snaps the body back round.
    #[test]
    fn a_spawn_points_facing_is_what_we_start_looking_along() {
        let yaw = std::f32::consts::FRAC_PI_2;
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        app.world_mut().spawn((
            Transform::from_translation(Vec3::ZERO).with_rotation(Quat::from_rotation_y(yaw)),
            SpawnPointMarker,
        ));
        enter(&mut app);
        // The body's mark lands at the end of the frame that spawned it.
        app.update();

        assert!(
            (app.world().resource::<InputLatch>().0.yaw - yaw).abs() < 1e-5,
            "the view is looking along {} rather than {yaw}",
            app.world().resource::<InputLatch>().0.yaw
        );
    }

    /// Bodies go to whoever lacks one, so dying needs no respawn system.
    #[test]
    fn a_player_whose_body_is_gone_gets_another_one() {
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        enter(&mut app);

        let first = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<LocalPlayer>>();
            query.single(world).expect("no body to begin with")
        };

        // However it went — reaped, disconnected, despawned by a gamemode.
        app.world_mut().entity_mut(first).despawn();
        app.update();

        let second = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<LocalPlayer>>();
            query.single(world).expect("the player was left with no body")
        };
        assert_ne!(second, first, "the old body came back rather than a new one");
    }

    /// A new body keeps the same `PlayerId` and team. Only F5 reallocates.
    #[test]
    fn the_person_who_comes_back_is_the_same_person() {
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        enter(&mut app);

        let who = |app: &mut App| {
            let world = app.world_mut();
            let mut query = world
                .query_filtered::<(&crate::common::damage::PlayerId, &Team), With<LocalPlayer>>();
            let (id, team) = query.single(world).expect("no body");
            (*id, *team)
        };
        let before = who(&mut app);

        let body = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<LocalPlayer>>();
            query.single(world).unwrap()
        };
        app.world_mut().entity_mut(body).despawn();
        app.update();

        assert_eq!(who(&mut app), before, "a life cost the player their identity");
    }

    /// Two `LocalPlayer` bodies on a host make `gather_input`'s `Single`
    /// silently stop running.
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

    /// A `Single` matching twice skips rather than errors, so check the local
    /// body still moves.
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
        let spawn = Spawn { feet: Vec3::new(5.0, 0.0, 0.0), yaw: 0.0, side: None };
        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, app.world());
            let who = crate::game::respawn::Identity {
                id: crate::common::damage::PlayerId(99),
                seat: crate::game::respawn::Seat::Playing(Some(Team::Red)),
                color: 1,
            };
            spawn_player(
                &mut commands,
                spawn,
                &who,
                who.color(&crate::game::respawn::HueStart(0.0)),
                &crate::common::game_rules::ModeRules::default(),
                &crate::common::weapon::WeaponCatalog::default(),
            );
        }
        queue.apply(app.world_mut());
        app.update();
    }

    /// Two bodies in one tick step from their own inputs, not the last writer's.
    #[test]
    fn two_bodies_step_from_their_own_inputs() {
        let mut app = headless(&[room(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 8.0, 20.0))]);
        enter(&mut app);

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

    /// Raising the floor mid-round moves the ground under the body.
    #[test]
    fn editing_a_room_while_playing_moves_the_ground() {
        // Tall enough that the raised floor still leaves standing room.
        let mut app = headless(&[room(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 8.0, 5.0))]);
        enter(&mut app);
        tick(&mut app, 60);
        assert!((player_position(&mut app).y - PLAYER_HALF.y).abs() < 0.05);

        // Raise the floor by two meters, the way a drag handle would.
        let mut rooms = app.world_mut().query::<&mut Room>();
        for mut room in rooms.iter_mut(app.world_mut()) {
            room.floor = 2.0;
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
