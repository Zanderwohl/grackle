use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::camera::Hdr;
use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use bevy::time::Fixed;

use crate::common::class::{
    body_centre_from_feet, Stance, CLASS_HALF_EXTENTS, TALLEST_CLASS_EYE_HEIGHT,
    TALLEST_CLASS_HEIGHT,
};
use crate::game::collision::CollisionWorld;
use crate::tool::room::Room;

/// Half-extents of the body box. The tallest class, since there is only one
/// body so far and a spawn point is checked against the tallest.
pub const PLAYER_HALF: Vec3 = CLASS_HALF_EXTENTS;
/// Where the camera sits relative to the body's *centre* — the class metric is
/// measured from the feet, so half the height comes back off.
const EYE_OFFSET: f32 = TALLEST_CLASS_EYE_HEIGHT - TALLEST_CLASS_HEIGHT * 0.5;

/// How fast a body moves without being asked to hurry, in metres per second.
///
/// A walking pace, which is what makes the gait a walk: the animation reads
/// speed off the ground covered, so this number is what decides whether a body
/// has a foot down at all times. Sprinting is [`SPRINT_SPEED`].
const WALK_SPEED: f32 = 2.6;

/// How fast a body moves while sprinting.
///
/// The speed everything moved at before there was a distinction, so holding
/// shift is the movement this prototype has always had.
const SPRINT_SPEED: f32 = 7.0;

/// How fast a body moves while crouched.
///
/// Slow enough that ducking is a decision. Sprinting while crouched is not a
/// thing: the stance wins, which is what makes crouching cost something.
const CROUCH_SPEED: f32 = 1.2;
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
    /// Which axes the last step was stopped on.
    ///
    /// A fact about the step, kept because nothing else can recover it: the
    /// body's position after a blocked move looks like a body that simply did
    /// not go that far. The animation layer reads it to tell running from
    /// running into a wall — see [`crate::game::skeleton`].
    pub blocked: BVec3,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            blocked: BVec3::FALSE,
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
    /// Asking to go faster.
    ///
    /// Held rather than latched, like movement and unlike jump: it describes a
    /// state the body is in for as long as it is asked for, not an edge that
    /// has to survive to the next step.
    pub sprint: bool,
    /// Asking to duck. Whether the body actually is ducked is [`Stance`],
    /// which can disagree: a body under a low ceiling cannot stand up when
    /// this goes false.
    pub crouch: bool,
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

/// Whether the player is watching from inside their own head or behind it.
///
/// A resource rather than a component: it is a property of *this* view, not of
/// a body. A spectator watching someone else, or a second local view, would
/// each have their own, which is the same reason it is not on the player.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    FirstPerson,
    ThirdPerson,
}

impl ViewMode {
    /// Whether the player's own body is drawn.
    ///
    /// It is not, in first person: from inside its head the body is a set of
    /// bones across the lens, and its own hitbox is a wireframe box the camera
    /// is standing in the middle of. Other people's bodies are always drawn —
    /// this is about the one you are inside.
    pub fn shows_own_body(&self) -> bool {
        matches!(self, ViewMode::ThirdPerson)
    }
}

/// How far behind the head the third-person camera sits.
const THIRD_PERSON_DISTANCE: f32 = 3.5;

/// The half-extent of the box the camera is swept as when looking for a wall.
///
/// Small: it exists so the camera stops in front of a wall rather than inside
/// it, not so the camera has a body.
const CAMERA_RADIUS: f32 = 0.15;

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
            Stance::default(),
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

    input.sprint = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    input.crouch = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
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

/// `F` swaps between looking out of the body and looking at it.
///
/// Only while playing: in the editor the cameras belong to the viewports, and
/// there is no body to be inside.
pub fn toggle_view(keys: Res<ButtonInput<KeyCode>>, mut mode: ResMut<ViewMode>) {
    if !keys.just_pressed(KeyCode::KeyF) {
        return;
    }
    *mode = match *mode {
        ViewMode::FirstPerson => ViewMode::ThirdPerson,
        ViewMode::ThirdPerson => ViewMode::FirstPerson,
    };
}

/// Put the camera where the current [`ViewMode`] says.
///
/// Runs at frame rate beside [`mouse_look`], which owns the camera's rotation;
/// this owns its position. Splitting them is what lets the third-person camera
/// swing with the pitch that was just written without either system having to
/// know when the other ran.
pub fn place_camera(
    mode: Res<ViewMode>,
    world: Res<CollisionWorld>,
    players: Query<(&Transform, &Stance), (With<Player>, Without<PlayerCamera>)>,
    mut cameras: Query<&mut Transform, With<PlayerCamera>>,
) {
    let Ok((body, stance)) = players.single() else { return };
    // Ducking lowers the eye, and it lowers what the third-person camera
    // orbits with it — otherwise crouching would swing the view around a point
    // above the body's own head.
    let eye = stance.eye_offset();

    for mut camera in &mut cameras {
        camera.translation = match *mode {
            ViewMode::FirstPerson => Vec3::Y * eye,
            ViewMode::ThirdPerson => third_person_camera(body, camera.rotation, eye, &world),
        };
    }
}

/// Where the third-person camera sits, in the body's own frame.
///
/// Behind the head along the direction the view is pointing, so pitching up
/// swings it down and vice versa — and pulled in short of anything solid, so
/// backing into a wall does not put the camera inside it and show the player
/// the world from the other side of the map.
pub fn third_person_camera(
    body: &Transform,
    pitch: Quat,
    eye: f32,
    world: &CollisionWorld,
) -> Vec3 {
    let pivot = Vec3::Y * eye;
    let wanted = pivot + pitch * Vec3::Z * THIRD_PERSON_DISTANCE;

    // Swept in world space, because that is where the walls are.
    let pivot_world = body.transform_point(pivot);
    let wanted_world = body.transform_point(wanted);
    let (stopped, _) = world.move_and_slide(
        pivot_world,
        Vec3::splat(CAMERA_RADIUS),
        wanted_world - pivot_world,
    );

    body.to_matrix().inverse().transform_point3(stopped)
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
    mut players: Query<(&mut Player, &mut PhysicsBody, &mut Stance)>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    // Taken, not read: the latch is per step, so a press cannot fire twice.
    let jump = std::mem::take(&mut input.jump);

    for (mut player, mut body, mut stance) in &mut players {
        change_stance(&mut stance, &mut body, player.on_ground, input.crouch, &world);
        let half = stance.half_extents();
        // Movement is in the body's frame, so turning turns the run direction.
        let wish = Vec3::new(input.movement.x, 0.0, -input.movement.y).normalize_or_zero();
        let wish = Quat::from_rotation_y(player.yaw) * wish;

        // The animation is never told which of these it was: it measures the
        // ground covered and works out for itself whether that is a walk.
        let speed = if input.sprint { SPRINT_SPEED } else { WALK_SPEED };
        let speed = if *stance == Stance::Crouched { CROUCH_SPEED } else { speed };
        player.velocity.x = wish.x * speed;
        player.velocity.z = wish.z * speed;

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
        let start = world.depenetrate(body.current, half);

        let delta = player.velocity * dt;
        let (position, blocked) = world.move_and_slide(start, half, delta);

        body.previous = body.current;
        body.current = position;

        // Standing on something means the *downward* move was the blocked one;
        // clouting your head on a ceiling also blocks Y and must not count.
        player.blocked = blocked;
        player.on_ground = blocked.y && delta.y <= 0.0;
        if blocked.y {
            player.velocity.y = 0.0;
        }
    }
}

/// Duck, or stand back up if there is room.
///
/// Which end of the body stays put is the whole of the crouch-jump: on the
/// ground the feet stay and the head drops, and in the air the head stays and
/// the feet come up, which is what lets a body clear a ledge it could not walk
/// onto. Both are the same box moving half the difference in height, in
/// opposite directions.
///
/// Standing up is a request rather than a certainty. A body under a vent stays
/// ducked until it has somewhere to put its head, which is what stops it
/// standing into the ceiling and being shoved back out by depenetration.
fn change_stance(
    stance: &mut Stance,
    body: &mut PhysicsBody,
    on_ground: bool,
    wants_crouch: bool,
    world: &CollisionWorld,
) {
    let shift = Stance::centre_shift();
    let (wanted, movement) = match (wants_crouch, *stance) {
        (true, Stance::Standing) => (
            Stance::Crouched,
            if on_ground { -shift } else { shift },
        ),
        (false, Stance::Crouched) => (
            Stance::Standing,
            if on_ground { shift } else { -shift },
        ),
        _ => return,
    };

    let moved = body.current + Vec3::Y * movement;
    if wanted == Stance::Standing && !world.fits(moved, Stance::Standing.half_extents()) {
        return;
    }

    // Both ends of the interpolation, so the change does not draw as the body
    // sliding through the floor over the next frame.
    body.current = moved;
    body.previous += Vec3::Y * movement;
    *stance = wanted;
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

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::tool::room::Room;

    fn world(rooms: &[Room]) -> CollisionWorld {
        let mut world = CollisionWorld::default();
        world.rebuild(rooms);
        world
    }

    fn open_room() -> CollisionWorld {
        world(&[Room::new(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 6.0, 10.0))])
    }

    /// Behind the head, at the distance asked for, when there is nothing in
    /// the way. Local `+Z` is behind a body that faces `-Z`.
    #[test]
    fn the_third_person_camera_sits_behind_the_head() {
        let body = Transform::from_xyz(0.0, 1.0, 0.0);
        let eye = Stance::Standing.eye_offset();
        let camera = third_person_camera(&body, Quat::IDENTITY, eye, &open_room());

        assert!((camera.y - eye).abs() < 1e-3, "at height {}", camera.y);
        assert!((camera.z - THIRD_PERSON_DISTANCE).abs() < 1e-3, "at {} behind", camera.z);
        assert!(camera.x.abs() < 1e-3);
    }

    /// It orbits the head rather than sliding: looking up puts the camera
    /// lower, and it stays the same distance away.
    ///
    /// A gentle pitch, deliberately. Look up far enough and the camera would
    /// be under the floor, and the sweep pulls it in instead of keeping the
    /// distance — which is the behaviour the next test is about, not this one.
    #[test]
    fn looking_up_swings_the_camera_down() {
        let body = Transform::from_xyz(0.0, 1.0, 0.0);
        let world = open_room();
        let eye = Stance::Standing.eye_offset();
        let pivot = Vec3::Y * eye;

        let level = third_person_camera(&body, Quat::IDENTITY, eye, &world);
        let looking_up = third_person_camera(&body, Quat::from_rotation_x(0.3), eye, &world);

        assert!(looking_up.y < level.y, "the camera did not drop: {} then {}", level.y, looking_up.y);
        assert!(
            ((looking_up - pivot).length() - (level - pivot).length()).abs() < 1e-3,
            "the camera changed distance while pitching"
        );
    }

    /// Backing into a wall pulls the camera in rather than putting it inside
    /// the wall, where it would show the player the far side of the map.
    #[test]
    fn a_wall_behind_pulls_the_camera_in() {
        let world = open_room();
        // Close enough to the +Z wall that the camera cannot have its distance.
        let body = Transform::from_xyz(0.0, 1.0, 8.0);
        let camera = third_person_camera(&body, Quat::IDENTITY, Stance::Standing.eye_offset(), &world);

        assert!(camera.z < THIRD_PERSON_DISTANCE, "the camera kept its distance at {}", camera.z);
        assert!(camera.z > 0.0, "the camera ended up in front of the body at {}", camera.z);
        assert!(
            world.inside_map(body.transform_point(camera)),
            "the camera ended up outside the map"
        );
    }

    /// First person is the eye, and the eye is a class metric rather than a
    /// number this file made up.
    #[test]
    fn first_person_puts_the_camera_at_the_eye() {
        let mut app = App::new();
        app.init_resource::<ViewMode>();
        app.insert_resource(open_room());
        app.world_mut().spawn((
            Player::default(),
            Stance::Standing,
            Transform::from_xyz(0.0, 1.0, 0.0),
        ));
        let camera = app
            .world_mut()
            .spawn((PlayerCamera, Transform::default()))
            .id();

        app.world_mut().run_system_once(place_camera).unwrap();

        let placed = app.world().get::<Transform>(camera).unwrap().translation;
        assert_eq!(placed, Vec3::Y * Stance::Standing.eye_offset());
        assert!(
            (placed.y + PLAYER_HALF.y - TALLEST_CLASS_EYE_HEIGHT).abs() < 1e-5,
            "the eye is not where the class says it is"
        );
    }

    /// Ducking on the ground leaves the feet where they are and brings the
    /// head down — which is what makes a crouch a way through a low gap.
    #[test]
    fn crouching_on_the_ground_keeps_the_feet_and_drops_the_head() {
        let world = open_room();
        let mut stance = Stance::Standing;
        let mut body = PhysicsBody::at(body_centre_from_feet(Vec3::ZERO));

        let head = body.current.y + Stance::Standing.half_extents().y;
        change_stance(&mut stance, &mut body, true, true, &world);

        assert_eq!(stance, Stance::Crouched);
        let feet = body.current.y - Stance::Crouched.half_extents().y;
        assert!(feet.abs() < 1e-5, "the feet moved to {feet}");
        assert!(
            body.current.y + Stance::Crouched.half_extents().y < head - 0.5,
            "the head barely moved"
        );
    }

    /// In the air it is the other way round: the head stays and the feet come
    /// up. That is the crouch-jump — the body clears a ledge it could not have
    /// walked onto, by pulling its legs out of the way.
    #[test]
    fn crouching_in_the_air_keeps_the_head_and_lifts_the_feet() {
        let world = open_room();
        let mut stance = Stance::Standing;
        let mut body = PhysicsBody::at(Vec3::new(0.0, 3.0, 0.0));

        let head = body.current.y + Stance::Standing.half_extents().y;
        let feet = body.current.y - Stance::Standing.half_extents().y;
        change_stance(&mut stance, &mut body, false, true, &world);

        assert_eq!(stance, Stance::Crouched);
        assert!(
            (body.current.y + Stance::Crouched.half_extents().y - head).abs() < 1e-5,
            "the head moved"
        );
        let lifted = body.current.y - Stance::Crouched.half_extents().y - feet;
        assert!(
            (lifted - 2.0 * Stance::centre_shift()).abs() < 1e-5,
            "the feet came up by {lifted}"
        );
    }

    /// Standing up is a request, not a certainty. Under something low the body
    /// stays down rather than standing into the ceiling and being shoved out
    /// of it again.
    #[test]
    fn a_body_under_a_low_ceiling_cannot_stand_up() {
        let world = world(&[Room::new(
            Vec3::new(-5.0, 0.0, -5.0),
            Vec3::new(5.0, Stance::Crouched.height() + 0.2, 5.0),
        )]);
        let mut stance = Stance::Crouched;
        let mut body = PhysicsBody::at(Vec3::Y * Stance::Crouched.half_extents().y);
        let was = body.current;

        change_stance(&mut stance, &mut body, true, false, &world);

        assert_eq!(stance, Stance::Crouched, "stood up into the ceiling");
        assert_eq!(body.current, was);

        // And in a room it fits in, it stands straight back up.
        let mut stance = Stance::Crouched;
        change_stance(&mut stance, &mut body, true, false, &open_room());
        assert_eq!(stance, Stance::Standing);
    }

    /// The interpolated position moves with the stance too. Without it, a
    /// crouch would be drawn as the body sliding into the floor over the
    /// following frame.
    #[test]
    fn a_stance_change_moves_both_ends_of_the_interpolation() {
        let mut stance = Stance::Standing;
        let mut body = PhysicsBody::at(body_centre_from_feet(Vec3::ZERO));

        change_stance(&mut stance, &mut body, true, true, &open_room());

        assert_eq!(body.current, body.previous);
    }

    /// `F` swaps, and swaps back.
    #[test]
    fn f_toggles_the_view() {
        let mut app = App::new();
        app.init_resource::<ViewMode>();
        app.add_plugins(bevy::input::InputPlugin);

        let press = |app: &mut App| {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::KeyF);
            app.world_mut().run_system_once(toggle_view).unwrap();
            // Released as well as cleared: a key already held is not pressed
            // again, so without this the second press never happens.
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::KeyF);
            keys.clear();
        };

        assert_eq!(*app.world().resource::<ViewMode>(), ViewMode::FirstPerson);
        press(&mut app);
        assert_eq!(*app.world().resource::<ViewMode>(), ViewMode::ThirdPerson);
        press(&mut app);
        assert_eq!(*app.world().resource::<ViewMode>(), ViewMode::FirstPerson);
    }

    /// The rule the drawing systems read: your own body is hidden from inside
    /// its own head, and only there.
    #[test]
    fn only_third_person_shows_your_own_body() {
        assert!(!ViewMode::FirstPerson.shows_own_body());
        assert!(ViewMode::ThirdPerson.shows_own_body());
    }
}
