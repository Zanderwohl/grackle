use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::camera::Hdr;
use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use bevy::time::Fixed;
use lightyear::prelude::input::native::ActionState;

use crate::common::damage::{Armor, Damageable, Died, PlayerId, DEFAULT_HEALTH, DEFAULT_MAX_ARMOR};
use crate::game::death::Dying;
use crate::common::hitbox::Hitboxes;
use crate::common::movement_rules::MovementRules;
use crate::common::taunt::TauntTurning;
use crate::common::game_rules::ModeRules;
use crate::common::team::Team;
use crate::common::player_color::PlayerColor;
use crate::game::respawn::Identity;
use crate::common::class::{
    body_center_from_feet, Stance, CLASS_HALF_EXTENTS,
};
use crate::game::collision::{CollisionWorld, Touched};
use crate::common::weapon::{Ammo, Equipped, WeaponCatalog, WeaponId};
use crate::game::condition::Condition;
use crate::game::weapon::{AutoReload, Trigger};
use crate::common::world_surface::Fill;
use crate::tool::room::Room;

/// Half-extents of the body box: the tallest class, which spawn points are
/// checked against.
pub const PLAYER_HALF: Vec3 = CLASS_HALF_EXTENTS;
/// Meters per second. The gait reads speed off the ground covered, so this is
/// what makes it a walk.
const WALK_SPEED: f32 = 2.6;

/// Meters per second.
const SPRINT_SPEED: f32 = 7.0;

/// Meters per second. Wins over sprint.
const CROUCH_SPEED: f32 = 1.2;
const JUMP_SPEED: f32 = 7.0;

// Movement with momentum, as Quake and Source write it (`PM_Friction`,
// `PM_Accelerate`, `PM_AirAccelerate`), at Source's numbers — TF2's — turned
// from units into meters at 0.0254 m a unit.

/// Ground friction, per second: the fraction of speed lost each second above
/// [`STOP_SPEED`].
const FRICTION: f32 = 4.0;
/// Below this, meters per second, friction acts as if at this speed, so a
/// slow body stops in a moment rather than creeping on forever.
const STOP_SPEED: f32 = 2.54;
/// Per second, in multiples of the wished-for speed.
const GROUND_ACCELERATE: f32 = 10.0;
const AIR_ACCELERATE: f32 = 10.0;
/// How fast, meters per second, the keys alone push a body in the air along
/// any one direction. Small, so the air is mostly momentum — but measured
/// along the wish, so turning the view while strafing keeps adding speed
/// sideways to the motion: air strafing.
const AIR_WISH_SPEED: f32 = 0.76;
/// The steepest floor a body stands on, as the cosine of its tilt: about 45°,
/// Quake's number. Anything steeper is a wall that slopes, and is slid off.
pub const WALKABLE: f32 = 0.7;
/// The tallest ledge a body walks up rather than being stopped by: TF2's
/// eighteen units, a little over a stair.
pub const STEP_HEIGHT: f32 = 0.45;
/// An impulse carrying a body off its floor faster than this, meters per
/// second, launches it; slower, the floor keeps it. Quake's ten units.
const LAUNCH_SPEED: f32 = 0.25;
/// Seconds of no ground friction after a knockback.
const KNOCKBACK_SLIDE: f32 = 0.2;
/// How far below its feet a body looks for a floor after each step. Has to
/// clear the collision skin, which is where a step leaves a body standing.
const GROUND_PROBE: f32 = 0.02;
const MOUSE_SENSITIVITY: f32 = 0.0022;
/// Vertical field of view. Bevy measures FOV vertically, where Source's 90 is
/// horizontal at 4:3, so this is wider than Source's.
const FIELD_OF_VIEW: f32 = std::f32::consts::FRAC_PI_2;
/// Short of straight up and down, so the view never flips over.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

/// The body. Its `Transform` is the center of the collision box, not the feet
/// or the eye; [`PLAYER_HALF`] is measured from here.
///
/// The required components are ones whose absence fails silently: a body
/// nobody can shoot, or one every weapon query skips. `Damageable` does not
/// require `Player` itself, since a crate has health too.
#[derive(Component, Debug, Clone, PartialEq, Reflect, Serialize, Deserialize)]
// `Transform` and `Visibility` because a replicated body is built by
// inserting components rather than through `spawn_player`, and without them
// its children have nothing to inherit.
#[require(Hitboxes, Damageable, Armor, Equipped, Ammo, Trigger, AutoReload, Inputs, Condition, Impulse, Transform, Visibility)]
pub struct Player {
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// The normal of the floor under the body after the last step, if it is
    /// one it can stand on. Kept because the next step walks along it.
    pub ground: Option<Vec3>,
    /// The last step was stopped by something neither floor nor ceiling.
    /// A stopped move cannot be recovered from position alone; the animation
    /// layer reads this to tell running from running into a wall.
    pub against_wall: bool,
    /// A taunt held the body back from the aim, so it turns back at
    /// [`CATCH_UP_DEGREES_PER_SECOND`] rather than in one tick.
    pub catching_up: bool,
    /// Seconds left of no ground friction after a knockback, as Quake's
    /// knockback timer: a rocket at your feet would otherwise be mostly
    /// rubbed off before you left the floor.
    pub sliding: f32,
}

/// Velocity, meters per second, to add to a body on its next step, and what
/// put it there. Written by explosions; [`step_player`] is the only thing that
/// takes it, so velocity keeps one writer.
///
/// Authority-only and never replicated: only the authority sets things off,
/// and it never rolls back, so taking it in the step replays nothing. A client
/// learns of its own launch through the replicated `Player.velocity`.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct Impulse(pub Vec3);

pub const CATCH_UP_DEGREES_PER_SECOND: f32 = 720.0;

impl Player {
    pub fn on_ground(&self) -> bool {
        self.ground.is_some()
    }
}

impl Default for Player {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            ground: None,
            against_wall: false,
            catching_up: false,
            sliding: 0.0,
        }
    }
}

/// What one body is being asked to do this step.
///
/// A component, since a server steps every body from its own inputs in the
/// same tick. The step is a function of `(state, input, fixed dt)`, so a client
/// and a server fed the same values reach the same position. See "Three
/// clocks" in CLAUDE.md for how it is gathered and consumed.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Reflect, Serialize, Deserialize)]
pub struct PlayerInput {
    /// Movement in the body's own frame: `x` right, `y` forward.
    pub movement: Vec2,
    /// Jump was held or pressed since the last tick consumed it.
    pub jump: bool,
    /// Held, not latched.
    pub sprint: bool,
    /// Asking to duck. [`Stance`] can disagree: a body under a low ceiling
    /// cannot stand up.
    pub crouch: bool,
    /// A weapon asked for since the last tick consumed it. By id, since a
    /// pickup can shift its place in the list.
    pub select: Option<WeaponId>,
    /// The trigger was pulled since the last tick consumed it. Latched, since
    /// a shot is an edge.
    pub attack: bool,
    /// The trigger is held now. Both this and `attack` are needed: a click
    /// made and released between two fixed steps was never held on any step.
    /// See [`pull_trigger`](crate::game::weapon::pull_trigger).
    pub attack_held: bool,
    /// The other trigger was pulled since the last tick consumed it.
    pub alt_attack: bool,
    /// The other trigger is held now.
    pub alt_attack_held: bool,
    /// A reload was asked for since the last tick consumed it.
    pub reload: bool,
    /// A debug key until there are rounds to lose.
    pub humiliate: bool,
    pub taunt: bool,
    /// A debug key until there is a kill command.
    pub suicide: bool,
    /// Look direction about world Y, radians. An input because facing decides
    /// which way "forward" is, so a server needs it to reproduce movement.
    pub yaw: f32,
    /// Look direction up and down, radians, positive up. Tilts the head only;
    /// the run direction stays level.
    pub pitch: f32,
}

impl PlayerInput {
    /// Let go of every control without forgetting where you were looking.
    ///
    /// A key held as the pause menu opens would otherwise stay held. Aim is
    /// kept because it is a position, not a key; zeroing it turns the view to
    /// face north. Lists the kept fields so a new one is classified on purpose.
    pub fn release_controls(&mut self) {
        *self = PlayerInput {
            yaw: self.yaw,
            pitch: self.pitch,
            ..PlayerInput::default()
        };
    }
}

/// What this machine's keyboard and mouse have said since the last tick.
///
/// A resource: a fact about this process's peripherals, not about a body.
/// [`gather_input`] fills it at frame rate; [`write_client_inputs`] empties it
/// into the body's [`Inputs`] once per tick, the only place an edge is
/// consumed. The step must not consume it: rollback replays a tick from the
/// same buffered input.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct InputLatch(pub PlayerInput);

/// The input component a body carries.
///
/// Lightyear's `ActionState` used directly as storage. A separate component
/// copied to and from it would have to be right about when it copies, and
/// rollback replays ticks out of order.
pub type Inputs = ActionState<PlayerInput>;

/// A body this process steps forward itself.
///
/// Read only by `step_player`. Everything else that is not predicted asks
/// `has_authority`. Added by `spawn_player` (whoever spawns a body steps it)
/// and by `claim_our_own_body` on a client.
#[derive(Component, Default, Debug)]
pub struct Simulated;

/// The one body this machine is driving.
///
/// Everything reading a keyboard or mouse filters on this; `With<Player>`
/// would drive every body in the world.
#[derive(Component, Default, Debug)]
pub struct LocalPlayer;

/// Where the body is at fixed-step boundaries, so rendering can draw between
///
/// Physics writes here and never to `Transform`; [`interpolate_bodies`] writes
/// `Transform` and never here.
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

/// The play camera. Its own entity, not a child of the body: see
/// [`place_camera`].
#[derive(Component)]
pub struct PlayerCamera;

/// Above the editor's four viewport cameras.
pub const PLAY_CAMERA_ORDER: isize = 100;

/// Whether the player is watching from inside their own head or behind it.
///
/// A resource: a property of this view, not of a body.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    FirstPerson,
    ThirdPerson,
}

/// The view picked with `F`. [`ViewMode`] is what is drawn, so a condition
/// forcing third person never has to remember what to put back.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewPreference(pub ViewMode);

/// How far the camera has come out of the head: 0 at the eye, 1 where third
/// person puts it. [`ViewMode`] is third person whenever this is above zero,
/// so the body is drawn whenever the camera is not inside it.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraPull(pub f32);

/// Seconds for the camera to come all the way out of the head, or back in.
pub const CAMERA_PULL_SECONDS: f32 = 0.35;

impl CameraPull {
    /// Both views are worked out every frame, so a swing starts from where the
    /// unused one really is.
    pub fn place(self, first: Transform, third: Transform) -> Transform {
        let t = EaseFunction::SmoothStep.sample_clamped(self.0);
        Transform {
            translation: first.translation.lerp(third.translation, t),
            rotation: first.rotation.slerp(third.rotation, t),
            scale: Vec3::ONE,
        }
    }
}

impl ViewMode {
    /// Whether the player's own body is drawn. Other bodies always are.
    pub fn shows_own_body(&self) -> bool {
        matches!(self, ViewMode::ThirdPerson)
    }
}

/// How far behind the head the third-person camera sits.
const THIRD_PERSON_DISTANCE: f32 = 3.5;

/// The half-extent of the box the camera is swept as when looking for a wall.
/// Half-extent of the box the third-person camera is swept as, so it stops
/// in front of a wall.
const CAMERA_RADIUS: f32 = 0.15;

/// Which way a body faces when there is no spawn point to ask.
/// Only for [`fallback_spawn`]; a spawn point carries its own yaw.
pub const SPAWN_YAW: f32 = 0.0;

/// A place to stand and the way to face doing it, as the map gives it.
///
/// Kept together so the facing cannot be looked up from a different spawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    /// Where the feet go.
    pub feet: Vec3,
    /// Heading in radians about world Y.
    pub yaw: f32,
    /// The team the map put it on, through a Mirror. `None` is anybody's.
    pub side: Option<Team>,
}

/// The spawn points a body actually fits in, feet-first.
///
/// A spawn point under a low ceiling is dropped rather than resolved by
/// shoving the body somewhere unintended.
pub fn usable_spawns(spawns: &[Spawn], world: &CollisionWorld) -> Vec<Spawn> {
    spawns
        .iter()
        .copied()
        .filter(|spawn| world.fits(body_center_from_feet(spawn.feet), PLAYER_HALF))
        .collect()
}

/// For a map with no usable spawn point: somewhere a body fits in the largest
/// room, a crude guess at the main space. A block may stand on a room's
/// middle, so each room is searched outward from it, then the next largest.
pub fn fallback_spawn(world: &CollisionWorld) -> Spawn {
    /// Meters between the floor points tried.
    const STEP: f32 = 0.5;
    /// Bounds the search on a map where nothing fits, which runs per respawn.
    const MOST_ROOMS: usize = 4;
    const MOST_POINTS: usize = 4096;

    let mut rooms: Vec<&Room> = world.volumes().iter().filter(|volume| volume.fill == Fill::Air).map(|volume| &volume.shape).collect();
    rooms.sort_by(|a, b| b.volume().total_cmp(&a.volume()));

    let fits = |feet: &Vec3| world.fits(body_center_from_feet(*feet), PLAYER_HALF);
    let found = rooms.iter().take(MOST_ROOMS).find_map(|room| {
        let middle = room.floor_point();
        let (min, max) = room.bounds();
        let (columns, rows) = (((max.x - min.x) / STEP) as i32, ((max.z - min.z) / STEP) as i32);
        let mut points: Vec<Vec3> = (0..=columns)
            .flat_map(|i| (0..=rows).map(move |j| Vec3::new(min.x + i as f32 * STEP, room.floor, min.z + j as f32 * STEP)))
            .collect();
        points.sort_by(|a, b| a.distance_squared(middle).total_cmp(&b.distance_squared(middle)));
        std::iter::once(middle).chain(points).take(MOST_POINTS).find(fits)
    });

    let feet = match (found, rooms.first()) {
        (Some(feet), _) => feet,
        (None, Some(room)) => {
            warn!("Nowhere in any room to stand; spawning in the middle of the largest");
            room.floor_point()
        }
        (None, None) => {
            warn!("No rooms on the map; spawning at the origin");
            Vec3::ZERO
        }
    };
    Spawn { feet, yaw: SPAWN_YAW, side: None }
}

/// The spawn's `feet` is its own position; the body is centered above it.
///
/// Does not mark the body [`LocalPlayer`]: a server spawns every player's body
/// and drives none, and whose body it is is answered by `enter_play` or
/// `claim_our_own_body`. It does mark it [`Simulated`].
pub fn spawn_player(
    commands: &mut Commands,
    spawn: Spawn,
    who: &Identity,
    color: PlayerColor,
    rules: &ModeRules,
    catalog: &WeaponCatalog,
) -> Entity {
    let position = body_center_from_feet(spawn.feet);

    // This decides what the body carries; a corpse's build is still read off
    // the rig.
    let class = rules.class_to_play();
    let (equipped, ammo) = catalog.starting_equipment(class, rules.loadout.as_deref());
    let loadout = catalog.loadout(class);
    let health = loadout.and_then(|loadout| loadout.health).unwrap_or(DEFAULT_HEALTH);
    let armor = loadout.and_then(|loadout| loadout.armor).unwrap_or(DEFAULT_MAX_ARMOR);

    let mut body = commands.spawn((
        Player { yaw: spawn.yaw, ..default() },
        class,
        Damageable::with_health(health),
        Armor::with_max(armor),
        equipped,
        ammo,
        // Whoever stands a body up steps it.
        Simulated,
        // Seeded, since the step copies aim off the input and a zero would
        // snap the body north on its first step.
        ActionState(PlayerInput { yaw: spawn.yaw, ..default() }),
        who.id,
        color,
        Stance::default(),
        PhysicsBody::at(position),
        Transform::from_translation(position).with_rotation(Quat::from_rotation_y(spawn.yaw)),
        Visibility::default(),
        Name::new("Player"),
    ));
    if let Some(team) = who.team {
        body.insert(team);
    }
    body.id()
}

/// Hang a camera off whichever body this machine is looking out of.
///
/// Keyed on `LocalPlayer` rather than done in [`spawn_player`], because on a
/// client the body arrives from the server.
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
        children![crate::game::hud::hud_camera()],
    ));
}

/// Point the view where our new body is already looking.
///
/// The latch is what the view is aimed from and what the next tick sends, so a
/// latch left at zero would discard the spawn's facing. On `Added<LocalPlayer>`
/// to cover a new round, a respawn and a client being handed a body.
pub fn aim_the_view_at_our_body(
    mut latch: ResMut<InputLatch>,
    ours: Query<&Player, Added<LocalPlayer>>,
) {
    for body in &ours {
        latch.0.yaw = body.yaw;
        latch.0.pitch = body.pitch;
    }
}

pub fn despawn_the_view(mut commands: Commands, cameras: Query<Entity, With<PlayerCamera>>) {
    for camera in &cameras {
        commands.entity(camera).despawn();
    }
}

/// Read the keyboard into [`InputLatch`], once per frame.
///
/// Runs before the fixed loop so a press reaches the same frame's steps.
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

    // Accumulated, so a press between two fixed steps survives the frames
    // either side.
    input.jump |= keys.pressed(KeyCode::Space);

    // The edge, not the state: one press is one shot.
    input.attack |= buttons.just_pressed(MouseButton::Left);
    input.attack_held = buttons.pressed(MouseButton::Left);
    input.alt_attack |= buttons.just_pressed(MouseButton::Right);
    input.alt_attack_held = buttons.pressed(MouseButton::Right);

    input.reload |= keys.just_pressed(KeyCode::KeyR);
    input.humiliate |= keys.just_pressed(KeyCode::KeyH);
    input.taunt |= keys.just_pressed(KeyCode::KeyG);
    input.suicide |= keys.just_pressed(KeyCode::KeyO);

    input.sprint = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    input.crouch = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
}

/// Read the mouse into the latch, once per frame.
///
/// Touches neither a body nor a camera, so a body's facing has one writer:
/// the step.
///
/// Not gated on the pause menu: a `MessageReader` keeps its cursor, so
/// skipped frames would apply all the motion at once on closing it.
pub fn gather_aim(
    mut motion: MessageReader<MouseMotion>,
    paused: Option<Res<crate::game::pause_menu::PauseMenu>>,
    mut latch: ResMut<InputLatch>,
) {
    // Drained first, so motion during a pause is discarded.
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
/// The one place an edge is consumed. Level fields are carried across, since
/// they are re-read every frame. In `FixedPreUpdate`'s `WriteClientInputs` set
/// because what is written here is what Lightyear buffers and sends.
///
/// Edges are consumed with no body too, so a click made while dead does not
/// fire on respawning.
pub fn write_client_inputs(
    mut latch: ResMut<InputLatch>,
    input: Option<Single<&mut Inputs, With<LocalPlayer>>>,
) {
    if let Some(input) = input {
        input.into_inner().0 = latch.0;
    }

    latch.0.jump = false;
    latch.0.attack = false;
    latch.0.alt_attack = false;
    latch.0.reload = false;
    latch.0.humiliate = false;
    latch.0.taunt = false;
    latch.0.suicide = false;
    latch.0.select = None;
}

/// Turn every body to the yaw it is carrying.
///
/// Every body, our own included: the one writer of a body's rotation.
/// Hitboxes are built from `GlobalTransform`, so an unturned body is boxed
/// where it is not.
pub fn face_bodies(mut bodies: Query<(&Player, &mut Transform)>) {
    for (player, mut transform) in &mut bodies {
        let facing = Quat::from_rotation_y(player.yaw);
        // Only when it differs, so a still body does not dirty its transform
        // and its children every frame.
        if transform.rotation != facing {
            transform.rotation = facing;
        }
    }
}

/// `F` swaps between looking out of the body and looking at it.
pub fn toggle_view(keys: Res<ButtonInput<KeyCode>>, mut preference: ResMut<ViewPreference>) {
    if !keys.just_pressed(KeyCode::KeyF) {
        return;
    }
    preference.0 = match preference.0 {
        ViewMode::FirstPerson => ViewMode::ThirdPerson,
        ViewMode::ThirdPerson => ViewMode::FirstPerson,
    };
}

/// Put the view where the local body is, looking where the latch says.
///
/// Not a child of the body: its rotation comes from this machine's latch, so
/// looking around never waits on a round-trip.
///
/// Runs after `interpolate_bodies`; earlier, it sits a frame behind the body.
///
/// With no body, it orbits the [`DeathCam`] instead, still aimed by the latch.
pub fn place_camera(
    pull: Res<CameraPull>,
    world: Res<CollisionWorld>,
    latch: Res<InputLatch>,
    death_cam: Option<Res<DeathCam>>,
    body: Option<Single<(&Transform, &Stance), (With<LocalPlayer>, Without<PlayerCamera>)>>,
    mut cameras: Query<&mut Transform, With<PlayerCamera>>,
) {
    let pivot = match (body, death_cam) {
        // Ducking lowers the eye and the third-person pivot with it.
        (Some(body), _) => body.0.translation + Vec3::Y * body.1.eye_offset(),
        (None, Some(dead)) => dead.pivot,
        (None, None) => return,
    };
    let aim = Quat::from_rotation_y(latch.0.yaw) * Quat::from_rotation_x(latch.0.pitch);

    let first = Transform::from_translation(pivot).with_rotation(aim);
    let third = Transform::from_translation(third_person_camera(pivot, aim, &world)).with_rotation(aim);
    let placed = pull.place(first, third);
    for mut camera in &mut cameras {
        camera.translation = placed.translation;
        camera.rotation = placed.rotation;
    }
}

/// What the view orbits while we have no body. `decide_the_view` takes it
/// down when the next body arrives.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct DeathCam {
    /// Eased from the dying eye onto our corpse.
    pub pivot: Vec3,
    /// `None` leaves the pivot where the body died.
    pub whose: Option<PlayerId>,
}

/// Meters above the corpse's pelvis, so a body on the floor is looked down at.
const DEATH_CAM_LIFT: f32 = 0.6;

/// Per second. Also damps the ragdoll's tumbling, which shakes the camera.
const DEATH_CAM_FOLLOW_RATE: f32 = 6.0;

/// Removed on leaving Play, so a new round does not open on the last one's
/// death.
#[derive(Resource, Clone, Copy, Debug)]
pub struct OurLastBody {
    cam: DeathCam,
    /// A body gone without dying, at a disconnect or a round's end, is no death.
    died: bool,
}

/// Not on `Added<Dying>`: a frame longer than two ticks runs `Dying` and the
/// despawn before `Update` sees either. On a client `Died` and the despawn
/// arrive in either order.
pub fn start_the_death_cam(
    mut commands: Commands,
    mut deaths: MessageReader<Died>,
    last: Option<ResMut<OurLastBody>>,
    death_cam: Option<Res<DeathCam>>,
    ours: Option<Single<(&Transform, &Stance, Option<&PlayerId>, Has<Dying>), With<LocalPlayer>>>,
) {
    let seen = ours.map(|ours| {
        let (body, stance, id, dying) = ours.into_inner();
        let cam = DeathCam { pivot: body.translation + Vec3::Y * stance.eye_offset(), whose: id.copied() };
        (cam, dying)
    });
    let mut last = match (last, seen) {
        (Some(mut last), Some((cam, dying))) => {
            last.cam = cam;
            last.died |= dying;
            last
        }
        (Some(last), None) => last,
        (None, Some((cam, dying))) => {
            commands.insert_resource(OurLastBody { cam, died: dying });
            return;
        }
        (None, None) => return,
    };
    let whose = last.cam.whose;
    if deaths.read().any(|death| whose.is_some() && death.victim_id == whose) {
        last.died = true;
    }
    if seen.is_none() && last.died && death_cam.is_none() {
        commands.insert_resource(last.cam);
        commands.remove_resource::<OurLastBody>();
    }
}

/// With no corpse of ours, the pivot stays where it is.
pub fn follow_our_corpse(
    time: Res<Time>,
    death_cam: Option<ResMut<DeathCam>>,
    corpses: Query<(&crate::game::ragdoll::Ragdoll, &crate::game::ragdoll::CorpseOf)>,
) {
    let Some(mut death_cam) = death_cam else { return };
    let Some(whose) = death_cam.whose else { return };
    // Corpses outlive a respawn, so an earlier one of ours may still lie there.
    let ours = corpses.iter().filter(|(_, of)| of.0 == whose);
    let Some((corpse, _)) = ours.min_by(|(a, _), (b, _)| a.age().total_cmp(&b.age())) else { return };
    let target = corpse.pelvis() + Vec3::Y * DEATH_CAM_LIFT;
    let t = 1.0 - (-DEATH_CAM_FOLLOW_RATE * time.delta_secs()).exp();
    death_cam.pivot = death_cam.pivot.lerp(target, t);
}

/// Where the third-person camera sits, in world space.
///
/// Behind the head along the view direction, pulled in short of anything
/// solid.
pub fn third_person_camera(pivot: Vec3, aim: Quat, world: &CollisionWorld) -> Vec3 {
    let wanted = pivot + aim * Vec3::Z * THIRD_PERSON_DISTANCE;
    world.travel(pivot, Vec3::splat(CAMERA_RADIUS), wanted - pivot)
}

/// Walk, fall, jump, and stop at walls — one fixed step.
///
/// The same inputs and `dt` must give the same answer on client and server.
#[allow(clippy::type_complexity)]
pub fn step_player(
    time: Res<Time>,
    world: Res<CollisionWorld>,
    rules: Res<MovementRules>,
    // Absent in tests that build no game; the same table on both ends, since
    // the server sends it.
    taunts: Option<Res<crate::common::taunt::Taunts>>,
    mut players: Query<
        (&mut Player, &mut PhysicsBody, &mut Stance, &Inputs, &Condition, &mut Impulse),
        With<Simulated>,
    >,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (mut player_slot, mut body_slot, mut stance_slot, input, condition, mut impulse) in &mut players {
        // Stepped on copies and written back only if different: all three are
        // replicated, and a still body would otherwise be resent every tick.
        let mut player = player_slot.clone();
        let mut body = *body_slot;
        let mut stance = *stance_slot;

        player.sliding = (player.sliding - dt).max(0.0);
        if impulse.0 != Vec3::ZERO {
            let kick = std::mem::take(&mut impulse.0);
            player.velocity += kick;
            player.sliding = KNOCKBACK_SLIDE;
            // Off the floor here, or the walk below lays the kick flat along
            // it and the ground snap puts the body straight back.
            if player.ground.is_some_and(|floor| player.velocity.dot(floor) > LAUNCH_SPEED) {
                player.ground = None;
            }
        }

        // Read, never taken: `write_client_inputs` already consumed the edge,
        // and rollback replays this input.
        let jump = input.jump;

        // The view is aimed from the latch and never held back. Movement below
        // is from the body's yaw, so a held body walks the way it faces.
        let turning = condition.turning(taunts.as_deref());
        let aim = (input.yaw, input.pitch);
        (player.yaw, player.pitch) = match turning {
            TauntTurning::Free if player.catching_up => TauntTurning::AtMost(CATCH_UP_DEGREES_PER_SECOND),
            turning => turning,
        }
        .follow((player.yaw, player.pitch), aim, dt);
        player.catching_up = match turning {
            TauntTurning::Free => (player.yaw, player.pitch) != aim,
            _ => true,
        };
        let crouch = input.crouch && condition.may_crouch();
        change_stance(&mut stance, &mut body, player.on_ground(), crouch, &world);
        let half = stance.half_extents();
        let wish = Vec3::new(input.movement.x, 0.0, -input.movement.y).normalize_or_zero();
        let wish = Quat::from_rotation_y(player.yaw) * wish;
        let wishing = wish != Vec3::ZERO;

        // The animation measures ground covered; it is never told which speed.
        let speed = if rules.sprint.sprints(input.sprint) { SPRINT_SPEED } else { WALK_SPEED };
        let speed = if stance == Stance::Crouched { CROUCH_SPEED } else { speed };
        // A replicated, unpredicted `Condition`: the tick a taunt starts or
        // stops is a rollback, which is correct and rare.
        let speed = speed * condition.speed_factor(taunts.as_deref());
        let speed = if wishing { speed } else { 0.0 };

        // A held jump re-fires on landing, so chaining jumps needs no fresh
        // press. The tick that jumps feels no friction, so a hop chained on
        // landing keeps its speed: bunny hopping. On the ground velocity
        // follows the floor and gravity does nothing, so a walkable slope is
        // walked up and down and not slid down when standing still.
        let jumped = player.on_ground() && jump;
        player.velocity = match player.ground {
            Some(floor) => {
                let along_floor = (wish - floor * wish.dot(floor)).normalize_or_zero();
                let coasting = if jump || player.sliding > 0.0 { player.velocity } else { friction(player.velocity, dt) };
                let moving = accelerate(coasting, along_floor, speed, speed, GROUND_ACCELERATE, dt);
                if jump { Vec3::new(moving.x, JUMP_SPEED, moving.z) } else { onto_floor(moving, floor) }
            }
            None => {
                let moving = accelerate(player.velocity, wish, speed, speed.min(AIR_WISH_SPEED), AIR_ACCELERATE, dt);
                Vec3::new(moving.x, player.velocity.y + rules.fall() * dt, moving.z)
            }
        };

        // A room edited mid-match can enclose a body that crossed nothing,
        // which a sweep cannot see.
        let start = world.depenetrate(body.current, half);
        let delta = player.velocity * dt;
        let (mut position, mut touched) = world.slide(start, half, delta);

        // A stair is a wall too low to be one. Taken only if it gets further,
        // so pressing into a real wall does not hop on the spot.
        let walled = |touched: &Touched| touched.iter().any(|n| n.y.abs() < WALKABLE);
        if player.on_ground() && !jumped && walled(&touched) {
            if let Some((stepped, over)) = world.step_up(start, half, delta, STEP_HEIGHT, WALKABLE) {
                let level = |p: Vec3| Vec2::new(p.x - start.x, p.z - start.z).length();
                if level(stepped) > level(position) + 1.0e-4 {
                    (position, touched) = (stepped, over);
                }
            }
        }

        player.velocity = touched.clip(player.velocity);
        player.against_wall = walled(&touched);
        // Settled onto a floor found within the probe, so a body that ended a
        // fall just short of one does not stand on air.
        // Walking up a slope rises too; only a jump, or still going up after
        // one, leaves the ground.
        let rising = jumped || (player.ground.is_none() && player.velocity.y > 0.0);
        let ground = (!rising).then(|| world.ground(position, half, GROUND_PROBE, WALKABLE)).flatten();
        if let Some((resting, floor)) = ground {
            position = resting;
            // A landing settled by the probe touched nothing in the slide, so
            // the fall is taken off here; left in, the next tick would turn
            // it into speed along the floor.
            let into = player.velocity.dot(floor);
            if into < 0.0 {
                player.velocity -= floor * into;
            }
        }
        player.ground = ground.map(|(_, normal)| normal);

        body.previous = body.current;
        body.current = position;

        player_slot.set_if_neq(player);
        body_slot.set_if_neq(body);
        stance_slot.set_if_neq(stance);
    }
}

/// Slow down, by [`FRICTION`] scaled by horizontal speed — a slope's rise is
/// not speed to be rubbed off — applied to the whole of it.
fn friction(velocity: Vec3, dt: f32) -> Vec3 {
    let speed = Vec2::new(velocity.x, velocity.z).length();
    if speed < 1.0e-3 {
        return Vec3::ZERO;
    }
    let drop = speed.max(STOP_SPEED) * FRICTION * dt;
    velocity * ((speed - drop).max(0.0) / speed)
}

/// Push along `direction` until the speed *along it* reaches `cap`, at most
/// `rate × wish_speed` per second. Speed along other directions is neither
/// added to nor taken away, which is what lets momentum carry and what makes
/// air strafing: past `cap` in one direction, a direction at right angles to
/// the motion still has all of `cap` to give.
fn accelerate(velocity: Vec3, direction: Vec3, wish_speed: f32, cap: f32, rate: f32, dt: f32) -> Vec3 {
    let short = cap - velocity.dot(direction);
    if short <= 0.0 {
        return velocity;
    }
    velocity + direction * (rate * wish_speed * dt).min(short)
}

/// Along the floor at the same speed, as Quake does: a slope neither slows a
/// body walking up it nor speeds one walking down.
fn onto_floor(velocity: Vec3, floor: Vec3) -> Vec3 {
    let speed = velocity.length();
    (velocity - floor * velocity.dot(floor)).normalize_or_zero() * speed
}

/// Duck, or stand back up if there is room.
///
/// On the ground the feet stay and the head drops; in the air the head stays
/// and the feet come up, which is the crouch-jump. Standing up waits until
/// there is room, rather than standing into the ceiling and being depenetrated.
fn change_stance(
    stance: &mut Stance,
    body: &mut PhysicsBody,
    on_ground: bool,
    wants_crouch: bool,
    world: &CollisionWorld,
) {
    let shift = Stance::center_shift();
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

    // Both ends, so the change is not drawn as a slide through the floor.
    body.current = moved;
    body.previous += Vec3::Y * movement;
    *stance = wanted;
}

/// Draw the body between its last two fixed positions.
///
/// `overstep_fraction` is how far the clock has run past the latest step.
/// Rotation is [`face_bodies`]'s.
pub fn interpolate_bodies(
    fixed: Res<Time<Fixed>>,
    mut bodies: Query<(&PhysicsBody, &mut Transform)>,
) {
    let alpha = fixed.overstep_fraction();
    for (body, mut transform) in &mut bodies {
        let drawn = body.previous.lerp(body.current, alpha);
        // Assigned only when it differs, as in `face_bodies`: a body standing
        // still lerps between two equal points to the same place every frame,
        // and a written transform re-propagates every bone mesh hanging off it.
        if transform.translation != drawn {
            transform.translation = drawn;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use crate::common::class::TALLEST_CLASS_EYE_HEIGHT;

    use super::*;
    use crate::tool::room::Room;

    /// Breaks if the fallback stands a body in the room's middle whatever is
    /// there.
    #[test]
    fn the_fallback_spawn_steps_off_a_block_in_the_middle() {
        use crate::common::world_surface::Volume;
        let mut world = CollisionWorld::default();
        world.rebuild_volumes(&[
            Volume::air(Room::new(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 3.0, 2.0))),
            Volume::air(Room::new(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 3.0, 10.0))),
            Volume::solid(Room::new(Vec3::new(-4.0, 0.0, -4.0), Vec3::new(4.0, 3.0, 4.0))),
        ]);
        let spawn = fallback_spawn(&world);
        assert!(world.fits(body_center_from_feet(spawn.feet), PLAYER_HALF), "stood at {}", spawn.feet);
        assert!(spawn.feet.x.abs().max(spawn.feet.z.abs()) < 5.0, "went further than the block's edge: {}", spawn.feet);
    }

    fn a_death(id: PlayerId) -> crate::common::damage::Died {
        crate::common::damage::Died {
            victim: Entity::PLACEHOLDER,
            victim_name: None,
            victim_id: Some(id),
            killer: crate::common::damage::DamageSource::World,
            assist: None,
            at: Vec3::ZERO,
        }
    }

    const TICK: f32 = 1.0 / crate::constants::TICK_HZ as f32;

    fn run_on_the_ground(mut velocity: Vec3, direction: Vec3, ticks: usize) -> Vec3 {
        let speed = if direction == Vec3::ZERO { 0.0 } else { SPRINT_SPEED };
        for _ in 0..ticks {
            velocity = accelerate(friction(velocity, TICK), direction, speed, speed, GROUND_ACCELERATE, TICK);
        }
        velocity
    }

    #[test]
    fn a_run_builds_to_its_top_speed_and_no_further() {
        let early = run_on_the_ground(Vec3::ZERO, Vec3::NEG_Z, 3);
        assert!(early.length() < SPRINT_SPEED * 0.5, "at {early} after three ticks: no momentum");
        let later = run_on_the_ground(Vec3::ZERO, Vec3::NEG_Z, crate::constants::TICK_HZ as usize);
        assert!((later.length() - SPRINT_SPEED).abs() < 1e-3, "a second's run settled at {later}");
    }

    #[test]
    fn letting_go_coasts_and_then_stops() {
        let running = Vec3::NEG_Z * SPRINT_SPEED;
        let coasting = run_on_the_ground(running, Vec3::ZERO, 4);
        assert!(coasting.length() > SPRINT_SPEED * 0.5, "stopped dead at {coasting}");
        let stopped = run_on_the_ground(running, Vec3::ZERO, crate::constants::TICK_HZ as usize);
        assert_eq!(stopped, Vec3::ZERO, "still moving a second after letting go");
    }

    #[test]
    fn the_air_keeps_momentum_and_strafing_through_a_turn_adds_to_it() {
        let flying = Vec3::NEG_Z * SPRINT_SPEED;
        let air = |v: Vec3, wish: Vec3| accelerate(v, wish, SPRINT_SPEED, SPRINT_SPEED.min(AIR_WISH_SPEED), AIR_ACCELERATE, TICK);

        assert_eq!(air(flying, Vec3::ZERO), flying, "the air slowed a body with no keys down");
        let backpedal = air(flying, Vec3::Z);
        assert!(backpedal.length() < SPRINT_SPEED, "backpedaling in the air did nothing");

        // Holding a strafe key and turning with the motion: the wish stays at
        // right angles to where the body is going.
        let mut v = flying;
        for _ in 0..crate::constants::TICK_HZ as usize {
            let sideways = Vec3::Y.cross(v).normalize();
            v = air(v, sideways);
        }
        assert!(v.length() > SPRINT_SPEED * 1.2, "a second's air strafe reached only {}", v.length());
    }

    #[test]
    fn a_slope_neither_slows_nor_speeds_a_body_along_it() {
        let floor = Vec3::new(0.5, 1.0, 0.0).normalize();
        let along = onto_floor(Vec3::X * 5.0, floor);
        assert!((along.length() - 5.0).abs() < 1e-4);
        assert!(along.dot(floor).abs() < 1e-4, "{along} is not along the floor");
    }

    /// Breaks if the death cam waits to see `Dying`. `Died` comes after the
    /// despawn here, as it may on a client.
    #[test]
    fn the_death_cam_starts_when_our_body_is_gone_and_died() {
        let mut world = World::new();
        world.init_resource::<Messages<Died>>();
        let start = world.register_system(start_the_death_cam);
        let body = world
            .spawn((LocalPlayer, Transform::from_xyz(1.0, 1.0, 0.0), Stance::default(), PlayerId(5)))
            .id();

        world.run_system(start).unwrap();
        assert!(!world.contains_resource::<DeathCam>(), "a living body started the death cam");

        world.despawn(body);
        world.run_system(start).unwrap();
        assert!(!world.contains_resource::<DeathCam>(), "started before we were told of a death");

        world.write_message(a_death(PlayerId(5)));
        world.run_system(start).unwrap();
        let death_cam = *world.resource::<DeathCam>();
        assert_eq!(death_cam.whose, Some(PlayerId(5)));
        assert!((death_cam.pivot.y - (1.0 + Stance::default().eye_offset())).abs() < 1e-5);
    }

    /// Breaks if a body lost without dying, or somebody else's death, starts it.
    #[test]
    fn losing_a_body_without_dying_is_not_a_death() {
        let mut world = World::new();
        world.init_resource::<Messages<Died>>();
        let start = world.register_system(start_the_death_cam);
        let body = world.spawn((LocalPlayer, Transform::default(), Stance::default(), PlayerId(5))).id();
        world.run_system(start).unwrap();

        world.despawn(body);
        world.write_message(a_death(PlayerId(6)));
        world.run_system(start).unwrap();
        assert!(!world.contains_resource::<DeathCam>());
    }

    /// No body yet at the start of a round is not a death.
    #[test]
    fn having_had_no_body_is_not_a_death() {
        let mut world = World::new();
        world.init_resource::<Messages<Died>>();
        world.run_system_once(start_the_death_cam).unwrap();
        assert!(!world.contains_resource::<DeathCam>());
    }

    /// Eased toward our own corpse, not whichever corpse is nearest.
    #[test]
    fn the_death_cam_follows_our_own_corpse() {
        use crate::common::skeleton::{rig::{humanoid, Proportions}, Pose};
        use crate::game::ragdoll::{CorpseOf, Ragdoll};

        let skeleton = humanoid(Proportions::DEFAULT);
        let corpse_at = |x: f32| {
            Ragdoll::from_body(&skeleton, &Pose::rest(), &Transform::from_xyz(x, 0.0, 0.0), Vec3::ZERO)
        };
        let mut world = World::new();
        world.init_resource::<Time>();
        world.spawn((corpse_at(10.0), CorpseOf(PlayerId(1))));
        world.spawn((corpse_at(-10.0), CorpseOf(PlayerId(2))));
        world.insert_resource(DeathCam { pivot: Vec3::ZERO, whose: Some(PlayerId(1)) });

        for _ in 0..120 {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
            world.run_system_once(follow_our_corpse).unwrap();
        }

        let pivot = world.resource::<DeathCam>().pivot;
        assert!((pivot.x - 10.0).abs() < 0.1, "{pivot}");
        assert!(pivot.y > DEATH_CAM_LIFT, "not above the pelvis: {pivot}");
    }

    fn world(rooms: &[Room]) -> CollisionWorld {
        let mut world = CollisionWorld::default();
        world.rebuild(rooms);
        world
    }

    fn open_room() -> CollisionWorld {
        world(&[Room::new(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 6.0, 10.0))])
    }

    /// Behind the head at the asked distance; `+Z` is behind a view facing `-Z`.
    #[test]
    fn the_third_person_camera_sits_behind_the_head() {
        let eye = Stance::Standing.eye_offset();
        let pivot = Vec3::Y * eye;
        let camera = third_person_camera(pivot, Quat::IDENTITY, &open_room());

        assert!((camera.y - eye).abs() < 1e-3, "at height {}", camera.y);
        assert!((camera.z - THIRD_PERSON_DISTANCE).abs() < 1e-3, "at {} behind", camera.z);
        assert!(camera.x.abs() < 1e-3);
    }

    /// Looking up swings the camera down at the same distance.
    #[test]
    fn looking_up_swings_the_camera_down() {
        let world = open_room();
        // Clear of the floor, which would pull the camera in.
        let pivot = Vec3::new(0.0, 3.0, 0.0);

        let level = third_person_camera(pivot, Quat::IDENTITY, &world);
        let looking_up = third_person_camera(pivot, Quat::from_rotation_x(0.3), &world);

        assert!(looking_up.y < level.y, "the camera did not drop: {} then {}", level.y, looking_up.y);
        assert!(
            ((looking_up - pivot).length() - (level - pivot).length()).abs() < 1e-3,
            "the camera changed distance while pitching"
        );
    }

    /// A wall behind pulls the camera in rather than leaving it inside.
    #[test]
    fn a_wall_behind_pulls_the_camera_in() {
        let world = open_room();
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

    /// First person is at the eye height the class states.
    #[test]
    fn first_person_puts_the_camera_at_the_eye() {
        let mut app = App::new();
        app.init_resource::<CameraPull>();
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

        let placed = app.world().get::<Transform>(camera).unwrap().translation;
        let eye = Stance::Standing.eye_offset();
        assert_eq!(placed, body + Vec3::Y * eye);
        assert!(
            (eye + PLAYER_HALF.y - TALLEST_CLASS_EYE_HEIGHT).abs() < 1e-5,
            "the eye is not where the class says it is"
        );
    }

    /// The view is aimed by the latch, not by the body.
    #[test]
    fn the_view_is_aimed_by_the_latch_not_by_the_body() {
        let mut app = App::new();
        app.init_resource::<CameraPull>();
        app.insert_resource(open_room());
        let yaw = 1.1;
        app.insert_resource(InputLatch(PlayerInput { yaw, ..default() }));
        app.world_mut().spawn((
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

    /// Crouching on the ground keeps the feet and drops the head.
    #[test]
    fn crouching_on_the_ground_keeps_the_feet_and_drops_the_head() {
        let world = open_room();
        let mut stance = Stance::Standing;
        let mut body = PhysicsBody::at(body_center_from_feet(Vec3::ZERO));

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

    /// Crouching in the air keeps the head and lifts the feet.
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
            (lifted - 2.0 * Stance::center_shift()).abs() < 1e-5,
            "the feet came up by {lifted}"
        );
    }

    /// A body under a low ceiling stays crouched.
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

        let mut stance = Stance::Crouched;
        change_stance(&mut stance, &mut body, true, false, &open_room());
        assert_eq!(stance, Stance::Standing);
    }

    /// A stance change moves both ends of the interpolation, or a crouch draws
    /// as a slide into the floor.
    #[test]
    fn a_stance_change_moves_both_ends_of_the_interpolation() {
        let mut stance = Stance::Standing;
        let mut body = PhysicsBody::at(body_center_from_feet(Vec3::ZERO));

        change_stance(&mut stance, &mut body, true, true, &open_room());

        assert_eq!(body.current, body.previous);
    }

    /// `F` swaps, and swaps back.
    #[test]
    fn f_toggles_the_view() {
        let mut app = App::new();
        app.init_resource::<ViewPreference>();
        app.add_plugins(bevy::input::InputPlugin);

        let press = |app: &mut App| {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::KeyF);
            app.world_mut().run_system_once(toggle_view).unwrap();
            // Released too: a held key is not pressed again.
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::KeyF);
            keys.clear();
        };

        let view = |app: &App| app.world().resource::<ViewPreference>().0;
        assert_eq!(view(&app), ViewMode::FirstPerson);
        press(&mut app);
        assert_eq!(view(&app), ViewMode::ThirdPerson);
        press(&mut app);
        assert_eq!(view(&app), ViewMode::FirstPerson);
    }

    /// Your own body is hidden only in first person.
    #[test]
    fn only_third_person_shows_your_own_body() {
        assert!(!ViewMode::FirstPerson.shows_own_body());
        assert!(ViewMode::ThirdPerson.shows_own_body());
    }
}
