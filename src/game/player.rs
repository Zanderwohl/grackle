use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::camera::Hdr;
use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use bevy::time::Fixed;
use lightyear::prelude::input::native::ActionState;

use crate::common::damage::{Damageable, PlayerId};
use crate::common::hitbox::Hitboxes;
use crate::common::class::{
    body_centre_from_feet, Stance, CLASS_HALF_EXTENTS, TALLEST_CLASS_EYE_HEIGHT,
    TALLEST_CLASS_HEIGHT,
};
use crate::game::collision::CollisionWorld;
use crate::game::weapon::{Loadout, Trigger};
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
pub const GRAVITY: f32 = -20.0;
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
///
/// Requires [`Hitboxes`] and [`Damageable`]: a player is what hitboxes exist
/// for, and one spawned without them fails silently as a body nobody can
/// shoot. Health is the same statement — it is not required by `Damageable`'s
/// own definition, because a crate has health and is not a player.
///
/// And a [`Loadout`], because a body with nothing to shoot with is a body the
/// trigger does nothing for — silently, since a missing component simply drops
/// it out of every weapon's query.
#[derive(Component, Debug, Clone, PartialEq, Reflect, Serialize, Deserialize)]
// `Transform` and `Visibility` as well, because a body does not always come
// into being through `spawn_player`: one arriving from the server is built by
// inserting the replicated components, and a body with children but no
// transform is a Bevy hierarchy warning per child per frame and a mesh drawn
// at the origin.
#[require(Hitboxes, Damageable, Loadout, Trigger, Inputs, Transform, Visibility)]
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

/// What one body is being asked to do this step.
///
/// **A component on the body, not a resource.** It was a resource while there
/// was only ever one body being driven, and that is exactly the assumption a
/// server breaks: a server steps every player from a different set of inputs
/// in the same tick, so the inputs have to hang off the bodies they belong to.
/// A global one would step every body from whoever's packet arrived last.
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
/// - It is the seam prediction needs. This struct is what travels: the same
///   `(state, input, fixed dt)` fed to the same stepping function has to
///   produce the same position on both ends, and that is only true if the
///   input is a value rather than a keyboard read.
///
/// Because it lives on the body, leaving Play takes it with the body. A click
/// made while editing cannot fire on the next spawn, because the thing that
/// would have remembered it no longer exists.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Reflect, Serialize, Deserialize)]
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
    /// A weapon slot asked for since the last step acted on it, if any.
    ///
    /// A slot number rather than a [`Weapon`](crate::game::weapon::Weapon):
    /// this struct is what will travel to a server, and what a slot holds is a
    /// property of the body at the other end rather than something the client
    /// gets to assert.
    pub select: Option<usize>,
    /// The trigger was pulled at some point since the last step consumed it.
    ///
    /// Latched like jump and unlike sprint, and for a sharper reason: a shot
    /// is an edge. Reading `just_pressed` from the fixed step would fire twice
    /// on a slow frame and not at all on a fast one, which is a weapon whose
    /// rate of fire is the frame rate.
    pub attack: bool,
    /// The trigger is being held *now*.
    ///
    /// Held rather than latched, like sprint: an automatic weapon asks whether
    /// the button is down, not whether it went down. Both halves are needed —
    /// see [`pull_trigger`](crate::game::weapon::pull_trigger) — because a
    /// click made and released between two fixed steps was never held on any
    /// step that ran.
    pub attack_held: bool,
    /// Where the body is being asked to look, about world Y.
    ///
    /// Aim is an input rather than something `mouse_look` writes straight onto
    /// the body, because the step reads it: which way you are facing decides
    /// which way "forward" is, and a server that had to guess a client's
    /// facing could not reproduce its movement. `mouse_look` accumulates here
    /// at frame rate and the step copies it onto the body.
    pub yaw: f32,
    /// Where the body is being asked to look, up and down. Tilts the head
    /// only — the run direction stays level — and is carried for the same
    /// reason as `yaw`: a shot goes where the pitch says.
    pub pitch: f32,
}

/// What this machine's keyboard and mouse have said since the last tick.
///
/// A resource, and correctly so: it is a fact about the peripherals attached
/// to *this* process, not about any body. It exists because the two clocks
/// disagree — `ButtonInput` is cleared once per frame while `FixedUpdate` runs
/// zero, one or several times — so a press has to be accumulated somewhere
/// that survives frames and is emptied once per tick.
///
/// [`gather_input`] fills it at frame rate; [`write_client_inputs`] empties it
/// into the body's [`Inputs`] once per tick, and that drain is the only place
/// an edge is consumed. **The step must not consume it**: a predicted tick is
/// replayed from the same buffered input during rollback, and a step that took
impl PlayerInput {
    /// Let go of every control without forgetting where you were looking.
    ///
    /// This struct holds two kinds of field and they behave differently when
    /// nobody is at the keyboard. Movement, sprint, crouch, jump, attack and a
    /// weapon switch are all things a key is *doing* — held or just pressed —
    /// and a key held as the pause menu goes up would stay held with nothing
    /// running to correct it, walking the body into a wall until the menu
    /// closes.
    ///
    /// **Aim is not one of those.** `yaw` and `pitch` are where the player has
    /// *put* the view, the way a mouse pointer has a position: nothing is
    /// holding them, and zeroing them on a pause spins you round to face north
    /// the moment you open the menu.
    ///
    /// A new field wants classifying deliberately, which is why this is a
    /// method that lists them rather than a `..default()` at each call site.
    pub fn release_controls(&mut self) {
        *self = PlayerInput {
            yaw: self.yaw,
            pitch: self.pitch,
            ..PlayerInput::default()
        };
    }
}

/// the jump out of it would replay as a step that never jumped.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct InputLatch(pub PlayerInput);

/// The input component a body actually carries.
///
/// `ActionState` is Lightyear's wrapper, and using it as the storage rather
/// than keeping a `PlayerInput` beside it is deliberate. The alternative — our
/// own component, copied to and from the networked one each tick — is a bridge
/// that has to be exactly right about *when* it copies, and rollback replays
/// ticks out of order. One value, written and read in place, has no such
/// window to get wrong.
///
/// It derefs to [`PlayerInput`], so everything downstream reads and writes the
/// fields as before.
pub type Inputs = ActionState<PlayerInput>;

/// The one body this machine is driving.
///
/// Everything that reads a keyboard or a mouse is looking for this, and
/// nothing else. In a closed game it is the only body there is; with a server
/// there are others in the world that must not be steered from local input,
/// and a system that queried `With<Player>` would drive all of them at once.
#[derive(Component, Default, Debug)]
pub struct LocalPlayer;

/// Where the body is at fixed-step boundaries, so rendering can draw between
/// them.
///
/// Physics writes here and never to `Transform`; [`interpolate_bodies`] writes
/// `Transform` and never to here. Without that split, a 64 Hz body drawn at
/// 144 Hz visibly steps — and keeping the previous position around is also
/// what reconciliation will want when it has to rewind.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect, Serialize, Deserialize)]
pub struct PhysicsBody {
    pub previous: Vec3,
    pub current: Vec3,
}

impl PhysicsBody {
    pub fn at(position: Vec3) -> Self {
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
///
/// **Does not mark the body as [`LocalPlayer`].** A server spawns a body for
/// every player in the match and drives none of them; whose body it is, is a
/// separate question answered by whoever knows — `enter_play` for a solo game
/// or a host, `claim_our_own_body` for a client. Marking it here made every
/// body on a server the local one, which reads as a pile of unrelated
/// symptoms: `gather_input` is a `Single`, so the host stopped being able to
/// move the moment somebody joined; `hide_own_body` hid every body in the
/// match; and every body was given its own camera.
///
/// It *does* mark the body [`Simulated`], because whoever spawns a body steps
/// it.
pub fn spawn_player(commands: &mut Commands, spawn: Spawn, id: PlayerId) -> Entity {
    let position = body_centre_from_feet(spawn.feet);
    commands
        .spawn((
            Player { yaw: spawn.yaw, ..default() },
            // Seeded rather than left at zero: the step copies aim off the
            // input, so a body spawned facing east would snap north on its
            // first step if the input still said zero.
            ActionState(PlayerInput { yaw: spawn.yaw, ..default() }),
            id,
            Stance::default(),
            PhysicsBody::at(position),
            Transform::from_translation(position).with_rotation(Quat::from_rotation_y(spawn.yaw)),
            Visibility::default(),
            Name::new("Player"),
        ))
        .id()
}

/// Hang a camera off whichever body this machine is looking out of.
///
/// Keyed on `LocalPlayer` rather than done in [`spawn_player`], because on a
/// client the body is not spawned here at all: it arrives from the server and
/// is marked local when it turns out to be ours. A camera is a fact about who
/// is watching, so it belongs with the mark and not with the body.
pub fn spawn_the_view(
    mut commands: Commands,
    bodies: Query<Entity, Added<LocalPlayer>>,
    existing: Query<(), With<PlayerCamera>>,
) {
    if bodies.is_empty() || !existing.is_empty() {
        return;
    }
    commands.spawn((
        PlayerCamera,
        // The window has five cameras in it — four editor viewports and this —
        // and Bevy UI picks one by ambiguity rules rather than by asking.
        bevy::ui::IsDefaultUiCamera,
        Camera3d::default(),
        Camera { order: PLAY_CAMERA_ORDER, ..default() },
        Hdr,
        Tonemapping::TonyMcMapface,
        Projection::Perspective(PerspectiveProjection {
            fov: FIELD_OF_VIEW,
            ..default()
        }),
        Transform::default(),
        Name::new("View"),
    ));
}

/// Point the view where our new body is already looking.
///
/// A body knows its facing before this machine does: on a host it comes from
/// the spawn point, and on a client it arrives from the server. The latch is
/// what the view is aimed from and what the next tick sends, so a latch left
/// at zero snaps the body round to face north on the first input it sends —
/// which is a spawn point's facing being quietly ignored.
///
/// On `Added<LocalPlayer>`, so one rule covers a fresh round, a respawn, and a
/// client being handed a body, without any of them knowing about the others.
pub fn aim_the_view_at_our_body(
    mut latch: ResMut<InputLatch>,
    ours: Query<&Player, Added<LocalPlayer>>,
) {
    for body in &ours {
        latch.0.yaw = body.yaw;
        latch.0.pitch = body.pitch;
    }
}

/// Take the view away with the round.
pub fn despawn_the_view(mut commands: Commands, cameras: Query<Entity, With<PlayerCamera>>) {
    for camera in &cameras {
        commands.entity(camera).despawn();
    }
}

/// Read the keyboard into [`InputLatch`], once per frame.
///
/// Runs before the fixed loop so a press is available to the steps taken in
/// the same frame it happened, rather than a frame late. Writes the latch
/// rather than the body, because the body's input belongs to a tick and this
/// runs on frames.
pub fn gather_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut latch: ResMut<InputLatch>,
) {
    let input = &mut latch.0;
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

    // The edge, not the state: one press is one shot, however many frames
    // pass before a fixed step gets round to it. Automatic fire is a weapon
    // property and will latch a held button instead.
    input.attack |= buttons.just_pressed(MouseButton::Left);
    input.attack_held = buttons.pressed(MouseButton::Left);

    // Latched like the trigger and for the same reason: a switch is an edge,
    // and a fixed step that ran twice this frame must not see it twice.
    for (key, slot) in [
        (KeyCode::Digit1, 0),
        (KeyCode::Digit2, 1),
        (KeyCode::Digit3, 2),
        (KeyCode::Digit4, 3),
        (KeyCode::Digit5, 4),
    ] {
        if keys.just_pressed(key) {
            input.select = Some(slot);
        }
    }

    input.sprint = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    input.crouch = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
}

/// Read the mouse into the latch, once per frame.
///
/// Aim is an input like any other and goes to exactly one place — nothing here
/// touches a body or a camera. What that buys is that a body's facing has one
/// writer anywhere in the system: the step, on the machine that owns it. There
/// is no version of "the view already turned but the body has not" to get
/// wrong.
///
/// Not gated on the pause menu, but it checks: a `MessageReader` has its own
/// cursor, so a frame this does not run is a frame of mouse motion still
/// waiting. Gated, closing the menu would apply every scrap of motion made
/// while it was up in one frame and spin the view.
pub fn gather_aim(
    mut motion: MessageReader<MouseMotion>,
    paused: Option<Res<crate::game::pause_menu::PauseMenu>>,
    mut latch: ResMut<InputLatch>,
) {
    // Drained first, whatever happens next: what is not read now is read
    // later, and later is the wrong frame.
    let delta: Vec2 = motion.read().map(|event| event.delta).sum();
    if paused.is_some_and(|menu| menu.open) || delta == Vec2::ZERO {
        return;
    }
    let input = &mut latch.0;
    input.yaw -= delta.x * MOUSE_SENSITIVITY;
    input.pitch = (input.pitch - delta.y * MOUSE_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
}

/// Empty the frame-rate latch into the local body's input for this tick.
///
/// The one place an edge is consumed. Level fields — movement, sprint, aim —
/// are carried across, since the peripherals are re-read every frame and the
/// latest reading is the right one. Edges are taken, so that one press is one
/// tick's worth of press however many frames it spanned.
///
/// Runs in `FixedPreUpdate`, in Lightyear's `WriteClientInputs` set, because
/// what is written here is what gets buffered and sent to the server.
pub fn write_client_inputs(
    mut latch: ResMut<InputLatch>,
    input: Option<Single<&mut Inputs, With<LocalPlayer>>>,
) {
    let Some(input) = input else { return };
    let mut input = input.into_inner();
    input.0 = latch.0;

    latch.0.jump = false;
    latch.0.attack = false;
    latch.0.select = None;
}

/// Turn every body to the yaw it is carrying.
///
/// **Every** body, our own included. `Player.yaw` is written by the step on
/// the machine that owns it and by replication everywhere else, so there is
/// one value and this is the one place it reaches a transform. The alternative
/// — the local body turned by the mouse and everybody else's by their yaw —
/// is two writers for one thing, and two writers is how the two ends came to
/// disagree about which way somebody was looking.
///
/// It is not only a drawing question. Hitboxes are built from the body's
/// `GlobalTransform`, so an unturned body has its head boxed where its head is
/// not, and a shot that visibly lands does not register.
pub fn face_bodies(mut bodies: Query<(&Player, &mut Transform)>) {
    for (player, mut transform) in &mut bodies {
        let facing = Quat::from_rotation_y(player.yaw);
        // Assigned only when it differs, so a body standing still does not
        // dirty its transform — and everything hanging off it — every frame.
        if transform.rotation != facing {
            transform.rotation = facing;
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

/// Put the view where the local body is, looking where the latch says.
///
/// The camera is **not a child of the body** and takes nothing from it but a
/// position. Its rotation comes straight from this machine's own aim, so
/// looking around is instant however far behind the server the body's facing
/// happens to be — which is the one thing that must never wait for a
/// round-trip. It is also the whole reason the camera is a separate entity: as
/// a child it would inherit the body's rotation and the view would lag.
pub fn place_camera(
    mode: Res<ViewMode>,
    world: Res<CollisionWorld>,
    latch: Res<InputLatch>,
    body: Option<Single<(&Transform, &Stance), (With<LocalPlayer>, Without<PlayerCamera>)>>,
    mut cameras: Query<&mut Transform, With<PlayerCamera>>,
) {
    let Some(body) = body else { return };
    let (body, stance) = body.into_inner();
    // Ducking lowers the eye, and it lowers what the third-person camera
    // orbits with it — otherwise crouching would swing the view around a point
    // above the body's own head.
    let eye = stance.eye_offset();
    let aim = Quat::from_rotation_y(latch.0.yaw) * Quat::from_rotation_x(latch.0.pitch);
    let pivot = body.translation + Vec3::Y * eye;

    for mut camera in &mut cameras {
        camera.rotation = aim;
        camera.translation = match *mode {
            ViewMode::FirstPerson => pivot,
            ViewMode::ThirdPerson => third_person_camera(pivot, aim, &world),
        };
    }
}

/// Where the third-person camera sits, in world space.
///
/// Behind the head along the direction the view is pointing, so pitching up
/// swings it down and vice versa — and pulled in short of anything solid, so
/// backing into a wall does not put the camera inside it and show the player
/// the world from the other side of the map.
///
/// World space throughout, now that the camera is its own entity: it used to
/// answer in the body's frame and be converted back, which was a conversion
/// that existed only because of the parenting.
pub fn third_person_camera(pivot: Vec3, aim: Quat, world: &CollisionWorld) -> Vec3 {
    let wanted = pivot + aim * Vec3::Z * THIRD_PERSON_DISTANCE;
    let (stopped, _) = world.move_and_slide(pivot, Vec3::splat(CAMERA_RADIUS), wanted - pivot);
    stopped
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
    world: Res<CollisionWorld>,
    mut players: Query<(&mut Player, &mut PhysicsBody, &mut Stance, &Inputs)>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (mut player, mut body, mut stance, input) in &mut players {
        // Read, never taken. The edge was already consumed by
        // `write_client_inputs` when it filled this tick's input; a step that
        // took it would empty the buffer the input layer sends from.
        let jump = input.jump;

        // Aim arrives as input and is written onto the body here. This is the
        // only place a body's facing is decided, on the only machine that
        // decides anything, and `face_bodies` draws every body from it.
        player.yaw = input.yaw;
        player.pitch = input.pitch;
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
    /// the way. `+Z` is behind a view that faces `-Z`.
    #[test]
    fn the_third_person_camera_sits_behind_the_head() {
        let eye = Stance::Standing.eye_offset();
        let pivot = Vec3::Y * eye;
        let camera = third_person_camera(pivot, Quat::IDENTITY, &open_room());

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
        let world = open_room();
        // Well clear of the floor: pitched up, the camera swings down, and one
        // that reached the floor would be pulled in by the sweep instead of
        // keeping its distance — which is the *next* test's business.
        let pivot = Vec3::new(0.0, 3.0, 0.0);

        let level = third_person_camera(pivot, Quat::IDENTITY, &world);
        let looking_up = third_person_camera(pivot, Quat::from_rotation_x(0.3), &world);

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
        let pivot = Vec3::new(0.0, Stance::Standing.eye_offset(), 8.0);
        let camera = third_person_camera(pivot, Quat::IDENTITY, &world);
        let behind = camera.z - pivot.z;

        assert!(behind < THIRD_PERSON_DISTANCE, "the camera kept its distance at {behind}");
        assert!(behind > 0.0, "the camera ended up in front of the body at {behind}");
        assert!(
            world.inside_map(camera),
            "the camera ended up outside the map"
        );
    }

    /// First person is the eye, and the eye is a class metric rather than a
    /// number this file made up.
    #[test]
    fn first_person_puts_the_camera_at_the_eye() {
        let mut app = App::new();
        app.init_resource::<ViewMode>();
        app.init_resource::<InputLatch>();
        app.insert_resource(open_room());
        let body = Vec3::new(0.0, 1.0, 0.0);
        app.world_mut().spawn((
            Player::default(),
            LocalPlayer,
            Stance::Standing,
            Transform::from_translation(body),
        ));
        let camera = app
            .world_mut()
            .spawn((PlayerCamera, Transform::default()))
            .id();

        app.world_mut().run_system_once(place_camera).unwrap();

        // World space, not the body's frame: the camera is its own entity and
        // inherits nothing.
        let placed = app.world().get::<Transform>(camera).unwrap().translation;
        let eye = Stance::Standing.eye_offset();
        assert_eq!(placed, body + Vec3::Y * eye);
        assert!(
            (eye + PLAYER_HALF.y - TALLEST_CLASS_EYE_HEIGHT).abs() < 1e-5,
            "the eye is not where the class says it is"
        );
    }

    /// The view is aimed from this machine's own latch and from nothing else.
    ///
    /// That is what lets looking around stay instant while the body it is
    /// standing in is a round-trip behind: the two are simply not connected.
    #[test]
    fn the_view_is_aimed_by_the_latch_not_by_the_body() {
        let mut app = App::new();
        app.init_resource::<ViewMode>();
        app.insert_resource(open_room());
        let yaw = 1.1;
        app.insert_resource(InputLatch(PlayerInput { yaw, ..default() }));
        app.world_mut().spawn((
            // The body says it is facing somewhere else entirely.
            Player { yaw: -2.0, ..default() },
            LocalPlayer,
            Stance::Standing,
            Transform::from_rotation(Quat::from_rotation_y(-2.0)),
        ));
        let camera = app.world_mut().spawn((PlayerCamera, Transform::default())).id();

        app.world_mut().run_system_once(place_camera).unwrap();

        let aimed = app.world().get::<Transform>(camera).unwrap().rotation;
        assert!(
            aimed.abs_diff_eq(Quat::from_rotation_y(yaw), 1e-5),
            "the view took its aim from the body rather than from the latch"
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
