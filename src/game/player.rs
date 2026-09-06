use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::camera::Hdr;
use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use bevy::time::Fixed;

use crate::common::class::{
    body_centre_from_feet, CLASS_HALF_EXTENTS, TALLEST_CLASS_EYE_HEIGHT, TALLEST_CLASS_HEIGHT,
};
use crate::game::collision::CollisionWorld;
use crate::tool::room::Room;

/// Half-extents of the body box. The tallest class, since there is only one
/// body so far and a spawn point is checked against the tallest.
pub const PLAYER_HALF: Vec3 = CLASS_HALF_EXTENTS;
/// Where the camera sits relative to the body's *centre* — the class metric is
/// measured from the feet, so half the height comes back off.
const EYE_OFFSET: f32 = TALLEST_CLASS_EYE_HEIGHT - TALLEST_CLASS_HEIGHT * 0.5;

const WALK_SPEED: f32 = 7.0;
const GRAVITY: f32 = -20.0;
const JUMP_SPEED: f32 = 7.0;
const MOUSE_SENSITIVITY: f32 = 0.0022;
/// Vertical field of view.
///
/// Bevy measures FOV vertically, where Source measures 90 across the 4:3
/// horizontal — so this is the wider, faster-feeling reading of "90 degrees",
/// not Source's. Worth revisiting alongside a real FOV setting.
const FIELD_OF_VIEW: f32 = std::f32::consts::FRAC_PI_2;
/// Pitch stops just short of straight up and straight down, so the view never
/// flips over.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

/// The body. Its `Transform` is the centre of the collision box, not the feet
/// and not the eye — [`PLAYER_HALF`] is measured from here, and the camera
/// hangs off it as a child.
#[derive(Component, Debug)]
pub struct Player {
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }
    }
}

/// What the body is being asked to do this step.
///
/// Gathered once per frame and consumed by the fixed step, rather than each
/// system reading the keyboard for itself. Two reasons, and the second is the
/// important one:
///
/// - `FixedUpdate` runs zero, one or several times per frame while
///   `ButtonInput` is cleared once per frame, so reading edges like
///   `just_pressed` from inside the fixed step double-counts a press on a slow
///   frame and drops it entirely on a fast one. Latching here is what makes a
///   tap survive to the next step.
/// - It is the seam prediction will need. Once there is a server, this struct
///   is what travels: the same `(state, input, fixed dt)` fed to the same
///   stepping function has to produce the same position on both ends, and that
///   is only true if the input is a value rather than a keyboard read.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct PlayerInput {
    /// Movement in the body's own frame: `x` right, `y` forward.
    pub movement: Vec2,
    /// Jump was held or pressed at some point since the last step consumed it.
    pub jump: bool,
}

/// Where the body is at fixed-step boundaries, so rendering can draw between
/// them.
///
/// Physics writes here and never to `Transform`; [`interpolate_bodies`] writes
/// `Transform` and never to here. Without that split, a 64 Hz body drawn at
/// 144 Hz visibly steps — and keeping the previous position around is also
/// what reconciliation will want when it has to rewind.
#[derive(Component, Debug, Clone, Copy)]
pub struct PhysicsBody {
    pub previous: Vec3,
    pub current: Vec3,
}

impl PhysicsBody {
    fn at(position: Vec3) -> Self {
        Self { previous: position, current: position }
    }
}

/// The first-person camera, a child of the body so that walking moves it for
/// free and only pitch has to be written here.
#[derive(Component)]
pub struct PlayerCamera;

/// Drawn above the editor's four viewport cameras, which are switched off for
/// the duration anyway.
const PLAY_CAMERA_ORDER: isize = 100;

/// Which way a body faces when there is no spawn point to ask.
///
/// A spawn point carries its own yaw and that is what a body put on one uses.
/// This is only for [`fallback_spawn`], where there is no mapper intent to
/// read: facing down -Z from the middle of the largest room is as good a guess
/// as any other.
pub const SPAWN_YAW: f32 = 0.0;

/// A place to stand and the way to face doing it, as the map gives it.
///
/// The pair travels together from here on: choosing a spawn point and then
/// looking its facing back up would be two chances to pick different ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    /// Where the feet go.
    pub feet: Vec3,
    /// Heading in radians about world Y.
    pub yaw: f32,
}

/// The spawn points a body actually fits in, feet-first.
///
/// A spawn point under a low ceiling is dropped rather than used and then
/// resolved by shoving the body somewhere the mapper did not intend.
pub fn usable_spawns(spawns: &[Spawn], world: &CollisionWorld) -> Vec<Spawn> {
    spawns
        .iter()
        .copied()
        .filter(|spawn| world.fits(body_centre_from_feet(spawn.feet), PLAYER_HALF))
        .collect()
}

/// Stand the player up in the largest room on the map.
///
/// The fallback for a map with no usable spawn point — an unfinished map, or
/// one whose spawns are all walled in. Largest by volume is a crude guess at
/// "the main space", and a crude guess beats refusing to start.
pub fn fallback_spawn(rooms: &[Room]) -> Spawn {
    let largest = rooms.iter().max_by(|a, b| {
        let volume = |r: &Room| {
            let size = (r.max - r.min).abs();
            size.x * size.y * size.z
        };
        volume(a).total_cmp(&volume(b))
    });

    let feet = match largest {
        Some(room) => {
            let min = room.min.min(room.max);
            let max = room.min.max(room.max);
            let centre = (min + max) / 2.0;
            Vec3::new(centre.x, min.y, centre.z)
        }
        None => {
            warn!("No rooms on the map; spawning at the origin");
            Vec3::ZERO
        }
    };
    Spawn { feet, yaw: SPAWN_YAW }
}

/// The spawn's `feet` is its own position; the body is centred above it.
///
/// The yaw goes into `Player` as well as `Transform` because `mouse_look` owns
/// the rotation from the next frame on and reads the body's yaw to do it —
/// setting only the transform would be undone on the first mouse movement.
pub fn spawn_player(commands: &mut Commands, spawn: Spawn) {
    let position = body_centre_from_feet(spawn.feet);
    commands
        .spawn((
            Player { yaw: spawn.yaw, ..default() },
            PhysicsBody::at(position),
            Transform::from_translation(position).with_rotation(Quat::from_rotation_y(spawn.yaw)),
            Visibility::default(),
            Name::new("Player"),
        ))
        .with_children(|body| {
            body.spawn((
                PlayerCamera,
                Camera3d::default(),
                Camera {
                    order: PLAY_CAMERA_ORDER,
                    ..default()
                },
                Hdr,
                Tonemapping::TonyMcMapface,
                Projection::Perspective(PerspectiveProjection {
                    fov: FIELD_OF_VIEW,
                    ..default()
                }),
                Transform::from_xyz(0.0, EYE_OFFSET, 0.0),
            ));
        });
}

/// Read the keyboard into [`PlayerInput`], once per frame.
///
/// Runs before the fixed loop so a press is available to the steps taken in
/// the same frame it happened, rather than a frame late.
pub fn gather_input(keys: Res<ButtonInput<KeyCode>>, mut input: ResMut<PlayerInput>) {
    let mut movement = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        movement.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        movement.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        movement.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        movement.x += 1.0;
    }
    input.movement = movement;

    // Accumulated, not assigned: a press between two fixed steps must not be
    // erased by the frames either side of it that saw nothing.
    input.jump |= keys.pressed(KeyCode::Space);
}

/// Turn mouse movement into yaw on the body and pitch on the camera.
///
/// Stays at frame rate rather than moving into the fixed step: sampling aim at
/// 64 Hz is latency you can feel, and looking around changes nothing the
/// physics has to agree with a server about.
pub fn mouse_look(
    mut motion: MessageReader<MouseMotion>,
    mut players: Query<(&mut Player, &mut Transform)>,
    mut cameras: Query<&mut Transform, (With<PlayerCamera>, Without<Player>)>,
) {
    let delta: Vec2 = motion.read().map(|event| event.delta).sum();
    if delta == Vec2::ZERO {
        return;
    }

    let mut pitch = None;
    for (mut player, mut transform) in &mut players {
        player.yaw -= delta.x * MOUSE_SENSITIVITY;
        player.pitch = (player.pitch - delta.y * MOUSE_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        transform.rotation = Quat::from_rotation_y(player.yaw);
        pitch = Some(player.pitch);
    }

    // Yaw turns the body, pitch tilts only the head — so the run direction
    // stays level however far up or down you are looking.
    if let Some(pitch) = pitch {
        for mut camera in &mut cameras {
            camera.rotation = Quat::from_rotation_x(pitch);
        }
    }
}

/// Walk, fall, jump, and stop at walls — one fixed step.
///
/// Everything that decides where a body ends up lives here, at a fixed rate,
/// reading input from a value rather than from the keyboard. That is what
/// makes the step reproducible: prediction on a client and simulation on a
/// server have to agree, and they can only agree if the same inputs and the
/// same `dt` give the same answer on both.
pub fn step_player(
    time: Res<Time>,
    mut input: ResMut<PlayerInput>,
    world: Res<CollisionWorld>,
    mut players: Query<(&mut Player, &mut PhysicsBody)>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    // Taken, not read: the latch is per step, so a press cannot fire twice.
    let jump = std::mem::take(&mut input.jump);

    for (mut player, mut body) in &mut players {
        // Movement is in the body's frame, so turning turns the run direction.
        let wish = Vec3::new(input.movement.x, 0.0, -input.movement.y).normalize_or_zero();
        let wish = Quat::from_rotation_y(player.yaw) * wish;

        player.velocity.x = wish.x * WALK_SPEED;
        player.velocity.z = wish.z * WALK_SPEED;

        // A held jump re-fires the step a landing is detected, rather than
        // eating the input and asking for a fresh press. Chaining jumps is a
        // skill to reward, not a timing test.
        if player.on_ground && jump {
            player.velocity.y = JUMP_SPEED;
        } else {
            player.velocity.y += GRAVITY * dt;
        }

        // Before moving, get out of anything that has been built around us.
        // A room edited mid-match can enclose a body that crossed nothing, and
        // `move_and_slide` only ever answers questions about crossings.
        let start = world.depenetrate(body.current, PLAYER_HALF);

        let delta = player.velocity * dt;
        let (position, blocked) = world.move_and_slide(start, PLAYER_HALF, delta);

        body.previous = body.current;
        body.current = position;

        // Standing on something means the *downward* move was the blocked one;
        // clouting your head on a ceiling also blocks Y and must not count.
        player.on_ground = blocked.y && delta.y <= 0.0;
        if blocked.y {
            player.velocity.y = 0.0;
        }
    }
}

/// Draw the body between its last two fixed positions.
///
/// `overstep_fraction` is how far the clock has run past the most recent step,
/// so this is where the body *would* be if physics ran continuously. Runs
/// after the fixed loop and before transform propagation; rotation is left
/// alone, since [`mouse_look`] already writes it at frame rate.
pub fn interpolate_bodies(
    fixed: Res<Time<Fixed>>,
    mut bodies: Query<(&PhysicsBody, &mut Transform)>,
) {
    let alpha = fixed.overstep_fraction();
    for (body, mut transform) in &mut bodies {
        transform.translation = body.previous.lerp(body.current, alpha);
    }
}
