//! The range: the selected weapon played by puppets on a small stage of its own, so an edit is on
//! screen a pass later. The puppets are real `Player` bodies whose `Inputs` a director writes, as
//! `steer_bots` does, so recoil, reloads, the viewmodel, damage and ragdolls are the match's own
//! systems (see `AppMode::simulates`). What a pass is made of comes from the weapon: each button's
//! action contributes its [`Beat`]s ([`beats_for`]), and the weapon adds a draw and a reload.
//!
//! The stage stands far below the map, so the map's triggers and pickups are nowhere near it, and
//! collides on its own `CollisionWorld`, swapped in on entry and back out on exit. The map is
//! hidden, not unloaded. Nothing here seats, scores or respawns anybody.

use bevy::camera::{ClearColorConfig, Viewport};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::camera::Hdr;
use bevy::camera::visibility::RenderLayers;
use bevy::window::PrimaryWindow;

use crate::common::app_mode::AppMode;
use crate::common::class::CLASS_HALF_EXTENTS;
use crate::common::damage::{DamageDealt, DamageSource, Died, PlayerId};
use crate::common::flame::Burning;
use crate::common::game_rules::ModeRules;
use crate::common::movement_rules::MovementRules;
use crate::common::player_color::PlayerColor;
use crate::common::render_layers;
use crate::common::team::Team;
use crate::common::weapon::{Ammo, Equipped, Mounted, WeaponAction, WeaponCatalog, WeaponId};
use crate::common::world_surface::Volume;
use crate::game::bot::aim_at;
use crate::game::bot_fight::aim_point;
use crate::game::collision::CollisionWorld;
use crate::game::player::{
    spawn_player, Inputs, PhysicsBody, Player, PlayerCamera, PlayerInput, Spawn, UneasedAim, ViewMode, ViewPreference,
    Watched, PLAY_CAMERA_ORDER,
};
use crate::game::projectile::Projectile;
use crate::game::ragdoll::Ragdoll;
use crate::game::respawn::{Identity, Seat};
use crate::game::viewmodel::ViewmodelCamera;
use crate::get;
use crate::tool::room::Room;

use super::WeaponEditor;

pub const RANGE_ORIGIN: Vec3 = Vec3::new(0.0, -2000.0, 0.0);
/// The stage, around the origin: the shooter at its feet, looking down `-Z`.
const STAGE_MIN: Vec3 = Vec3::new(-10.0, 0.0, -40.0);
const STAGE_MAX: Vec3 = Vec3::new(10.0, 8.0, 6.0);
/// Puppet ids count down from here, so none is ever a real player's.
const FIRST_PUPPET_ID: u64 = u64::MAX - 64;
const LOG_LINES: usize = 40;
/// Seconds before a beat's first press, so the reset is seen.
const LEAD_IN: f32 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Primary,
    Secondary,
}

impl Button {
    fn key(self) -> &'static str {
        match self {
            Button::Primary => "weapon_editor.primary",
            Button::Secondary => "weapon_editor.secondary",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BeatKind {
    /// At an enemy standing, or strafing across, this far ahead.
    Shoot { button: Button, distance: f32, strafing: bool },
    /// Flame an enemy, then stop and let the afterburn run out.
    Burn { button: Button, distance: f32 },
    Push { button: Button, distance: f32 },
    /// An enemy fires a rocket; turn it back.
    Reflect { button: Button },
    /// A burning teammate stands close; put them out.
    Extinguish { button: Button },
    /// Fire straight down on a jump from a standstill.
    RocketJump { button: Button },
    /// Raise the sights, fire the primary once they are up, and lower them.
    Scoped { button: Button, distance: f32, raise: f32 },
    /// The button does nothing yet; press it anyway.
    Stub { button: Button },
    /// Draw the weapon from another.
    Deploy,
    /// Empty the clip and watch it fill.
    Reload,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beat {
    pub kind: BeatKind,
    /// Seconds, the lead-in included.
    pub length: f32,
}

impl Beat {
    pub fn label(&self) -> String {
        let button = |button: Button| get!(button.key());
        match self.kind {
            BeatKind::Shoot { button: b, distance, strafing: false } => {
                get!("weapon_editor.beats.shoot", "button", button(b), "distance", format!("{distance:.0}"))
            }
            BeatKind::Shoot { button: b, distance, strafing: true } => {
                get!("weapon_editor.beats.shoot_strafing", "button", button(b), "distance", format!("{distance:.0}"))
            }
            BeatKind::Burn { button: b, .. } => get!("weapon_editor.beats.burn", "button", button(b)),
            BeatKind::Push { button: b, .. } => get!("weapon_editor.beats.push", "button", button(b)),
            BeatKind::Reflect { button: b } => get!("weapon_editor.beats.reflect", "button", button(b)),
            BeatKind::Extinguish { button: b } => get!("weapon_editor.beats.extinguish", "button", button(b)),
            BeatKind::RocketJump { button: b } => get!("weapon_editor.beats.rocket_jump", "button", button(b)),
            BeatKind::Scoped { button: b, distance, .. } => {
                get!("weapon_editor.beats.scoped", "button", button(b), "distance", format!("{distance:.0}"))
            }
            BeatKind::Stub { button: b } => get!("weapon_editor.beats.stub", "button", button(b)),
            BeatKind::Deploy => get!("weapon_editor.beats.deploy"),
            BeatKind::Reload => get!("weapon_editor.beats.reload"),
        }
    }
}

/// Seconds a [`BeatKind::Scoped`] beat holds the sights up.
const SCOPED_HOLD: f32 = 1.0;

pub fn beats_for(button: Button, mounted: &Mounted) -> Vec<Beat> {
    let shot = mounted.cadence.interval().max(0.1);
    let shoot = |distance: f32, strafing: bool| Beat {
        kind: BeatKind::Shoot { button, distance, strafing },
        length: LEAD_IN + (shot * 4.0).clamp(1.2, 4.0),
    };
    match &mounted.action {
        WeaponAction::Hitscan(spec) => {
            if spec.pellets.is_some() {
                return vec![shoot(10.0f32.min(spec.range_m), false)];
            }
            // The ends of its distance lines, where the damage numbers should change.
            let mut distances: Vec<f32> = [spec.ramp_up.map(|line| line.near.max(2.0)), spec.falloff.map(|line| line.far)]
                .into_iter()
                .flatten()
                .collect();
            if distances.is_empty() {
                distances = vec![5.0, 15.0];
            }
            distances.iter_mut().for_each(|distance| *distance = distance.min(spec.range_m).min(-STAGE_MIN.z - 2.0));
            distances.sort_by(f32::total_cmp);
            distances.dedup();
            distances.into_iter().map(|distance| shoot(distance, false)).collect()
        }
        WeaponAction::Projectile(spec) => {
            let mut beats = vec![shoot(12.0, false), shoot(12.0, true)];
            if spec.knockback > 0.0 {
                beats.push(Beat { kind: BeatKind::RocketJump { button }, length: LEAD_IN + 2.5 });
            }
            beats
        }
        WeaponAction::Flame(spec) => vec![Beat {
            kind: BeatKind::Burn { button, distance: (spec.range_m * 0.6).max(1.0) },
            length: LEAD_IN + 1.5 + spec.afterburn_duration_s + 0.5,
        }],
        WeaponAction::Airblast(spec) => {
            let mut beats = Vec::new();
            if spec.push > 0.0 || spec.lift > 0.0 {
                beats.push(Beat { kind: BeatKind::Push { button, distance: (spec.range * 0.6).max(1.0) }, length: LEAD_IN + 2.0 });
            }
            if spec.reflect {
                beats.push(Beat { kind: BeatKind::Reflect { button }, length: LEAD_IN + 3.0 });
            }
            if spec.extinguish {
                beats.push(Beat { kind: BeatKind::Extinguish { button }, length: LEAD_IN + 1.5 });
            }
            beats
        }
        WeaponAction::Scope(spec) => vec![Beat {
            kind: BeatKind::Scoped { button, distance: 15.0, raise: spec.raise },
            length: LEAD_IN + spec.raise + SCOPED_HOLD + spec.lower + 1.0,
        }],
        WeaponAction::HealBeam(_) | WeaponAction::Invulnerable(_) => {
            vec![Beat { kind: BeatKind::Stub { button }, length: LEAD_IN + 1.5 }]
        }
        WeaponAction::None => Vec::new(),
    }
}

/// The whole loop for a weapon: both buttons, then a draw from `other`, then a reload.
pub fn loop_for(weapon: &crate::common::weapon::Weapon, other: Option<WeaponId>) -> Vec<Beat> {
    let mut beats = beats_for(Button::Primary, &weapon.primary);
    beats.extend(beats_for(Button::Secondary, &weapon.secondary));
    if other.is_some() {
        beats.push(Beat { kind: BeatKind::Deploy, length: LEAD_IN + weapon.deploy + 1.0 });
    }
    if let Some(clip) = weapon.magazine.clip {
        let emptying = weapon.primary.cadence.interval().max(0.1) * clip as f32;
        let filling = match weapon.magazine.reload_one_at_a_time {
            true => weapon.magazine.first_reload.unwrap_or(weapon.magazine.reload) + weapon.magazine.reload * clip as f32,
            false => weapon.magazine.reload,
        };
        beats.push(Beat { kind: BeatKind::Reload, length: (LEAD_IN + emptying + filling + 1.0).min(20.0) });
    }
    beats
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Shooter,
    Enemy,
    Teammate,
}

#[derive(Component, Debug)]
pub struct RangePuppet(pub Role);

/// Everything the stage stands up but the puppets.
#[derive(Component)]
struct RangeStage;

#[derive(Component)]
pub struct RangeCamera;

/// What the strip shows and asks for. View state: not saved, not undone.
#[derive(Resource)]
pub struct RangeShow {
    pub weapon: Option<WeaponId>,
    pub beats: Vec<Beat>,
    /// Off beats are skipped; by index into `beats`, reset with them.
    pub off: Vec<bool>,
    pub current: usize,
    pub elapsed: f32,
    pub playing: bool,
    pub speed: f32,
    pub restart: bool,
    /// `+1` or `-1` asks for the next or previous beat.
    pub step: i32,
    pub log: Vec<String>,
    /// The middle of the window, in egui points, where the two views go.
    pub view: Option<bevy_egui::egui::Rect>,
    /// Stand the current beat up again, after an edit changed it.
    rerun: bool,
    /// Another weapon to draw from, and one to fire a rocket at the shooter with.
    helpers: (Option<WeaponId>, Option<WeaponId>),
    tally: Tally,
}

impl Default for RangeShow {
    fn default() -> Self {
        Self {
            weapon: None,
            beats: Vec::new(),
            off: Vec::new(),
            current: 0,
            elapsed: 0.0,
            playing: true,
            speed: 1.0,
            restart: true,
            step: 0,
            log: Vec::new(),
            view: None,
            rerun: false,
            helpers: (None, None),
            tally: Tally::default(),
        }
    }
}

/// What happened in the beat being played.
#[derive(Default, Debug)]
struct Tally {
    hits: Vec<(u32, f32)>,
    reflected: u32,
    killed_at: Option<f32>,
    fastest: f32,
}

#[derive(Resource, Default)]
struct HiddenByRange(Vec<(Entity, Visibility)>);

#[derive(Resource, Default)]
struct MapCollision(Option<CollisionWorld>);

/// The view the player chose, put back on exit: the range's top half is always first person.
#[derive(Resource, Default)]
struct ChosenView(Option<ViewPreference>);

fn enter_range(
    mut commands: Commands,
    mut hidden: ResMut<HiddenByRange>,
    mut map: ResMut<MapCollision>,
    mut world: ResMut<CollisionWorld>,
    mut show: ResMut<RangeShow>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut view: ResMut<ViewPreference>,
    mut chosen: ResMut<ChosenView>,
    mut roots: Query<(Entity, &mut Visibility, Option<&RenderLayers>), (Without<ChildOf>, Without<Node>, Without<Camera>)>,
) {
    chosen.0 = Some(*view);
    *view = ViewPreference(ViewMode::FirstPerson);
    // Hidden as they stand now, and put back as they were: something hidden on purpose stays so.
    // Only what the world's cameras draw, so a thumbnail being taken is left alone.
    let world_layer = RenderLayers::layer(render_layers::WORLD);
    for (entity, mut visibility, layers) in &mut roots {
        if layers.is_some_and(|layers| !layers.intersects(&world_layer)) {
            continue;
        }
        if *visibility != Visibility::Hidden {
            hidden.0.push((entity, *visibility));
            *visibility = Visibility::Hidden;
        }
    }
    let shape = Room::new(RANGE_ORIGIN + STAGE_MIN, RANGE_ORIGIN + STAGE_MAX);
    map.0 = Some(std::mem::take(&mut *world));
    world.rebuild_volumes(&[Volume::air(shape)]);
    *show = RangeShow::default();

    let size = STAGE_MAX - STAGE_MIN;
    let middle = RANGE_ORIGIN + (STAGE_MIN + STAGE_MAX) / 2.0;
    let floor = materials.add(StandardMaterial { base_color: Color::srgb(0.42, 0.42, 0.40), perceptual_roughness: 0.9, ..default() });
    let wall = materials.add(StandardMaterial { base_color: Color::srgb(0.55, 0.52, 0.48), perceptual_roughness: 0.9, ..default() });
    let thin = 0.1;
    let mut slab = |at: Vec3, extent: Vec3, material: &Handle<StandardMaterial>| {
        commands.spawn((
            RangeStage,
            Mesh3d(meshes.add(Cuboid::from_size(extent))),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(at),
        ));
    };
    slab(Vec3::new(middle.x, RANGE_ORIGIN.y - thin / 2.0, middle.z), Vec3::new(size.x, thin, size.z), &floor);
    slab(Vec3::new(RANGE_ORIGIN.x + STAGE_MIN.x - thin / 2.0, middle.y, middle.z), Vec3::new(thin, size.y, size.z), &wall);
    slab(Vec3::new(RANGE_ORIGIN.x + STAGE_MAX.x + thin / 2.0, middle.y, middle.z), Vec3::new(thin, size.y, size.z), &wall);
    slab(Vec3::new(middle.x, middle.y, RANGE_ORIGIN.z + STAGE_MIN.z - thin / 2.0), Vec3::new(size.x, size.y, thin), &wall);
    commands.spawn((
        RangeStage,
        DirectionalLight { illuminance: 8000.0, shadow_maps_enabled: false, ..default() },
        // The shooter's body is on `INSIDE_HEAD`, and a light lights only its own layers.
        RenderLayers::from_layers(&[render_layers::WORLD, render_layers::INSIDE_HEAD]),
        Transform::from_translation(RANGE_ORIGIN).looking_at(RANGE_ORIGIN + Vec3::new(0.4, -1.0, -0.6), Vec3::Y),
    ));
    // Above the play camera and its viewmodel: only the bottom camera on a window clears.
    commands.spawn((
        RangeStage,
        RangeCamera,
        Camera3d::default(),
        Camera { order: PLAY_CAMERA_ORDER + 10, clear_color: ClearColorConfig::None, ..default() },
        Hdr,
        Tonemapping::TonyMcMapface,
        // `INSIDE_HEAD`, so it shows the shooter the first-person view looks out of.
        RenderLayers::from_layers(&[render_layers::WORLD, render_layers::GIZMOS, render_layers::INSIDE_HEAD]),
        // Placed by `frame_the_stage` on its first frame.
        Transform::default(),
    ));
}

#[allow(clippy::type_complexity)]
fn leave_range(
    mut commands: Commands,
    mut hidden: ResMut<HiddenByRange>,
    mut map: ResMut<MapCollision>,
    mut world: ResMut<CollisionWorld>,
    mut time: ResMut<Time<Virtual>>,
    mut view: ResMut<ViewPreference>,
    mut chosen: ResMut<ChosenView>,
    mut visibilities: Query<&mut Visibility>,
    leftovers: Query<Entity, Or<(With<RangeStage>, With<RangePuppet>, With<Projectile>, With<Ragdoll>)>>,
) {
    for (entity, was) in hidden.0.drain(..) {
        if let Ok(mut visibility) = visibilities.get_mut(entity) {
            *visibility = was;
        }
    }
    if let Some(saved) = map.0.take() {
        *world = saved;
    }
    for entity in &leftovers {
        commands.entity(entity).despawn();
    }
    time.set_relative_speed(1.0);
    time.unpause();
    if let Some(was) = chosen.0.take() {
        *view = was;
    }
}

fn eye_of(center: Vec3, eye: f32) -> Vec3 {
    center + Vec3::Y * (eye - CLASS_HALF_EXTENTS.y)
}

fn helpers(catalog: &WeaponCatalog, weapon: WeaponId) -> (Option<WeaponId>, Option<WeaponId>) {
    let other = catalog.weapons.iter().find(|w| w.id != weapon && w.key == "pistol").or_else(|| catalog.weapons.iter().find(|w| w.id != weapon)).map(|w| w.id);
    let rocket = catalog
        .weapons
        .iter()
        .find(|w| matches!(w.primary.action, WeaponAction::Projectile(spec) if spec.gravity_factor == 0.0))
        .map(|w| w.id);
    (other, rocket)
}

/// Steps the loop and writes every puppet's input for this tick.
#[allow(clippy::too_many_arguments)]
fn direct(
    mut commands: Commands,
    time: Res<Time>,
    editor: Res<WeaponEditor>,
    catalog: Res<WeaponCatalog>,
    world: Res<CollisionWorld>,
    rules: Res<MovementRules>,
    mut show: ResMut<RangeShow>,
    mut puppets: Query<(Entity, &RangePuppet, &PhysicsBody, &Player, &mut Inputs, &Ammo)>,
    projectiles: Query<(Entity, &PhysicsBody), With<Projectile>>,
    corpses: Query<Entity, With<Ragdoll>>,
) {
    let selected = editor.selected.and_then(|index| editor.weapons.get(index)).map(|weapon| WeaponId::of(&weapon.key));
    let Some(weapon) = selected.and_then(|id| catalog.get(id)) else {
        if show.weapon.take().is_some() {
            clear(&mut commands, &puppets, &projectiles, &corpses);
        }
        return;
    };
    // Only when the catalog or the weapon changed: the loop is a function of the two.
    if show.weapon != Some(weapon.id) {
        show.weapon = Some(weapon.id);
        show.helpers = helpers(&catalog, weapon.id);
        show.beats = loop_for(weapon, show.helpers.0);
        show.off = vec![false; show.beats.len()];
        // The old weapon's place means nothing in this loop, which may be shorter.
        show.current = 0;
        show.restart = true;
    } else if catalog.is_changed() {
        show.helpers = helpers(&catalog, weapon.id);
        let beats = loop_for(weapon, show.helpers.0);
        if beats != show.beats {
            // An edit: keep the place and what is switched off, and play the beat again.
            if beats.len() != show.beats.len() {
                show.off = vec![false; beats.len()];
                show.current = show.current.min(beats.len().saturating_sub(1));
            }
            show.beats = beats;
            show.rerun = true;
        }
    }
    let (other, rocket) = show.helpers;
    if show.beats.is_empty() {
        return;
    }

    let step = std::mem::take(&mut show.step);
    let finished = show.elapsed >= show.beats[show.current].length;
    if show.restart || show.rerun || step != 0 || finished {
        if !show.restart && !show.rerun && finished {
            let line = summary(&show);
            show.log.push(line);
            if show.log.len() > LOG_LINES {
                show.log.remove(0);
            }
        }
        let next = match (show.rerun && step == 0 && !show.off[show.current], show.restart) {
            (true, _) => Some(show.current),
            (false, true) => next_on(&show.off, show.beats.len() - 1, true),
            (false, false) => next_on(&show.off, show.current, step >= 0),
        };
        show.restart = false;
        show.rerun = false;
        show.elapsed = 0.0;
        show.tally = Tally::default();
        clear(&mut commands, &puppets, &projectiles, &corpses);
        // Every beat switched off stands nothing up.
        if let Some(next) = next {
            show.current = next;
            stand_up(&mut commands, &catalog, show.beats[next].kind, weapon.id, other, rocket);
        }
        return;
    }
    show.elapsed += time.delta_secs();

    let t = show.elapsed - LEAD_IN;
    let kind = show.beats[show.current].kind;
    let shooter = puppets.iter().find(|(_, puppet, ..)| puppet.0 == Role::Shooter).map(|(entity, _, body, player, ..)| (entity, body.current, player.eye));
    let target = puppets.iter().find(|(_, puppet, ..)| puppet.0 != Role::Shooter).map(|(_, _, body, player, ..)| (body.current, player.velocity));
    let incoming = shooter.and_then(|(_, at, _)| {
        projectiles.iter().map(|(_, body)| body.current).min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)))
    });
    if let Some((_, velocity)) = target {
        show.tally.fastest = show.tally.fastest.max(velocity.length());
    }

    for (_, puppet, body, player, mut inputs, ammo) in &mut puppets {
        let eye = eye_of(body.current, player.eye);
        let mut input = PlayerInput { yaw: player.yaw, pitch: player.pitch, ..default() };
        match puppet.0 {
            Role::Shooter => {
                if let Some((center, velocity)) = target {
                    let feet = center - Vec3::Y * CLASS_HALF_EXTENTS.y;
                    let point = match kind {
                        BeatKind::Shoot { button, .. } => aim_point(&world, eye, center, feet, velocity, action(weapon, button), rules.gravity),
                        _ => center,
                    };
                    (input.yaw, input.pitch) = aim_at(eye, point);
                }
                shooter_presses(kind, t, &mut input, weapon, ammo, incoming.map(|at| at.distance(eye)));
            }
            Role::Enemy => {
                if let Some((_, at, its_eye)) = shooter {
                    (input.yaw, input.pitch) = aim_at(eye, eye_of(at, its_eye));
                }
                match kind {
                    BeatKind::Shoot { strafing: true, .. } => {
                        input.movement = Vec2::new(if (t * 1.2).sin() >= 0.0 { 1.0 } else { -1.0 }, 0.0);
                    }
                    BeatKind::Reflect { .. } if (0.0..0.05).contains(&t) => {
                        input.attack = true;
                        input.attack_held = true;
                    }
                    _ => {}
                }
            }
            Role::Teammate => {}
        }
        inputs.0 = input;
    }
}

fn action(weapon: &crate::common::weapon::Weapon, button: Button) -> &WeaponAction {
    match button {
        Button::Primary => &weapon.primary.action,
        Button::Secondary => &weapon.secondary.action,
    }
}

fn press(input: &mut PlayerInput, button: Button) {
    match button {
        Button::Primary => {
            input.attack = true;
            input.attack_held = true;
        }
        Button::Secondary => {
            input.alt_attack = true;
            input.alt_attack_held = true;
        }
    }
}

/// `t` is seconds since the lead-in ended; `incoming` how far the nearest projectile is.
fn shooter_presses(kind: BeatKind, t: f32, input: &mut PlayerInput, weapon: &crate::common::weapon::Weapon, ammo: &Ammo, incoming: Option<f32>) {
    if t < 0.0 {
        return;
    }
    match kind {
        BeatKind::Shoot { button, .. } if t < 2.5 => press(input, button),
        BeatKind::Burn { button, .. } if t < 1.5 => press(input, button),
        BeatKind::Push { button, .. } | BeatKind::Extinguish { button } if t < 0.1 => press(input, button),
        BeatKind::Reflect { button } => {
            let reach = match action(weapon, button) {
                WeaponAction::Airblast(spec) => spec.range,
                _ => 0.0,
            };
            if incoming.is_some_and(|distance| distance < reach * 0.8) {
                press(input, button);
            }
        }
        BeatKind::RocketJump { button } => {
            input.pitch = -std::f32::consts::FRAC_PI_2 + 0.01;
            if t < 0.05 {
                input.jump = true;
            }
            if (0.05..0.15).contains(&t) {
                press(input, button);
            }
        }
        BeatKind::Scoped { button, raise, .. } => {
            // A frame past the raise, so the shot is the scoped one.
            let up = raise + 0.1;
            let tick = crate::constants::one_tick().as_secs_f32();
            if t < tick || (up + SCOPED_HOLD..up + SCOPED_HOLD + tick).contains(&t) {
                press(input, button);
            } else if (up..up + 0.05).contains(&t) {
                press(input, Button::Primary);
            }
        }
        BeatKind::Stub { button } if t < 1.0 => press(input, button),
        BeatKind::Deploy if t < 0.05 => input.select = Some(weapon.id),
        BeatKind::Reload => {
            if ammo.weapons.iter().find(|(id, _)| *id == weapon.id).is_some_and(|(_, state)| state.clip > 0 && state.reloading <= 0.0) {
                press(input, Button::Primary);
            }
        }
        _ => {}
    }
}

/// The next beat that is on, from `from`, wrapping; `None` when all are off.
fn next_on(off: &[bool], from: usize, forward: bool) -> Option<usize> {
    let count = off.len();
    (1..=count)
        .map(|step| match forward {
            true => (from + step) % count,
            false => (from + count * 2 - step) % count,
        })
        .find(|index| !off[*index])
}

fn clear(
    commands: &mut Commands,
    puppets: &Query<(Entity, &RangePuppet, &PhysicsBody, &Player, &mut Inputs, &Ammo)>,
    projectiles: &Query<(Entity, &PhysicsBody), With<Projectile>>,
    corpses: &Query<Entity, With<Ragdoll>>,
) {
    for entity in puppets.iter().map(|(entity, ..)| entity).chain(projectiles.iter().map(|(entity, _)| entity)).chain(corpses.iter()) {
        commands.entity(entity).despawn();
    }
}

fn stand_up(
    commands: &mut Commands,
    catalog: &WeaponCatalog,
    kind: BeatKind,
    weapon: WeaponId,
    other: Option<WeaponId>,
    rocket: Option<WeaponId>,
) {
    let mut count = 0;
    let mut puppet = |commands: &mut Commands, role: Role, ahead: f32, carried: Vec<WeaponId>| {
        count += 1;
        let team = match role {
            Role::Enemy => Team::Blue,
            _ => Team::Red,
        };
        let who = Identity { id: PlayerId(FIRST_PUPPET_ID + count), seat: Seat::Playing(Some(team)), color: count as usize, class: None };
        // The shooter looks down -Z; the others face it.
        let yaw = if role == Role::Shooter { 0.0 } else { std::f32::consts::PI };
        let spawn = Spawn { feet: RANGE_ORIGIN + Vec3::new(0.0, 0.0, -ahead), yaw, side: Some(team) };
        let body = spawn_player(commands, spawn, &who, PlayerColor::nth(0.0, count as usize), crate::common::class::Class::Mercenary, &ModeRules::default(), catalog);
        let mut ammo = Ammo::default();
        for id in &carried {
            if let Some(weapon) = catalog.get(*id) {
                *ammo.at_mut(*id) = Ammo::full(&weapon.magazine);
            }
        }
        commands.entity(body).insert((RangePuppet(role), Equipped::carrying(carried), ammo));
        body
    };
    let carried = match (kind, other) {
        (BeatKind::Deploy, Some(other)) => vec![other, weapon],
        (_, Some(other)) => vec![weapon, other],
        (_, None) => vec![weapon],
    };
    let shooter = puppet(commands, Role::Shooter, 0.0, carried);
    commands.entity(shooter).insert((Watched, UneasedAim));
    match kind {
        BeatKind::Shoot { distance, .. }
        | BeatKind::Burn { distance, .. }
        | BeatKind::Push { distance, .. }
        | BeatKind::Scoped { distance, .. } => {
            puppet(commands, Role::Enemy, distance, vec![]);
        }
        BeatKind::Reflect { .. } => {
            puppet(commands, Role::Enemy, 15.0, rocket.into_iter().collect());
        }
        BeatKind::Extinguish { .. } => {
            let teammate = puppet(commands, Role::Teammate, 2.5, vec![]);
            commands.entity(teammate).insert(Burning {
                source: DamageSource::World,
                weapon: None,
                remaining_s: 10.0,
                damage: 1,
                interval_s: 0.5,
                until_next_s: 0.5,
            });
        }
        BeatKind::RocketJump { .. } => {}
        BeatKind::Stub { .. } | BeatKind::Deploy | BeatKind::Reload => {
            puppet(commands, Role::Enemy, 10.0, vec![]);
        }
    }
}

fn summary(show: &RangeShow) -> String {
    let beat = &show.beats[show.current];
    let tally = &show.tally;
    let total: u32 = tally.hits.iter().map(|(amount, _)| amount).sum();
    let mut line = get!("weapon_editor.outcome.hits", "beat", beat.label(), "hits", tally.hits.len(), "damage", total);
    if let Some(at) = tally.killed_at {
        line.push_str(&get!("weapon_editor.outcome.killed", "seconds", format!("{at:.2}")));
    }
    match beat.kind {
        BeatKind::Reflect { .. } => line.push_str(&match tally.reflected > 0 {
            true => get!("weapon_editor.outcome.reflected"),
            false => get!("weapon_editor.outcome.not_reflected"),
        }),
        BeatKind::Push { .. } | BeatKind::RocketJump { .. } => {
            line.push_str(&get!("weapon_editor.outcome.speed", "speed", format!("{:.1}", tally.fastest)))
        }
        _ => {}
    }
    line
}

/// The shooter's hits, and the first death, in the beat being played.
fn tally(
    mut show: ResMut<RangeShow>,
    mut dealt: MessageReader<DamageDealt>,
    mut died: MessageReader<Died>,
    puppets: Query<(&RangePuppet, &PhysicsBody, &Player)>,
) {
    let shooter = puppets.iter().find(|(puppet, ..)| puppet.0 == Role::Shooter).map(|(_, body, player)| eye_of(body.current, player.eye));
    let elapsed = show.elapsed - LEAD_IN;
    // The shooter is always the first puppet stood up.
    let ours = DamageSource::Player(PlayerId(FIRST_PUPPET_ID + 1));
    for hit in dealt.read() {
        let on_the_shooter = puppets.get(hit.target).is_ok_and(|(puppet, ..)| puppet.0 == Role::Shooter);
        if hit.source != ours || on_the_shooter {
            continue;
        }
        let distance = shooter.map_or(0.0, |eye| eye.distance(hit.point));
        show.tally.hits.push((hit.amount, distance));
        if hit.cause.reflected {
            show.tally.reflected += 1;
        }
    }
    for death in died.read() {
        if death.victim_id.is_some_and(|id| id.0 > FIRST_PUPPET_ID + 1) && show.tally.killed_at.is_none() {
            show.tally.killed_at = Some(elapsed.max(0.0));
        }
    }
}

/// The play camera and its viewmodel on the top half of the middle, the stage camera under it.
/// The middle is wider than it is tall, so side by side each view would be a tall strip.
#[allow(clippy::type_complexity)]
fn place_the_views(
    show: Res<RangeShow>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut first_person: Query<&mut Camera, Or<(With<PlayerCamera>, With<ViewmodelCamera>)>>,
    mut stage: Query<&mut Camera, (With<RangeCamera>, Without<PlayerCamera>, Without<ViewmodelCamera>)>,
) {
    let Some(rect) = show.view else { return };
    let scale = window.scale_factor();
    let to_viewport = |min: bevy_egui::egui::Pos2, max: bevy_egui::egui::Pos2| {
        let position = UVec2::new((min.x * scale).max(0.0) as u32, (min.y * scale).max(0.0) as u32);
        let size = UVec2::new(((max.x - min.x) * scale).max(1.0) as u32, ((max.y - min.y) * scale).max(1.0) as u32);
        let mut viewport = Viewport { physical_position: position, physical_size: size, ..default() };
        // The rect is last frame's, and the window may have shrunk since.
        viewport.clamp_to_size(window.physical_size());
        Some(viewport)
    };
    let half = rect.center().y;
    let top = to_viewport(rect.min, bevy_egui::egui::pos2(rect.max.x, half));
    let bottom = to_viewport(bevy_egui::egui::pos2(rect.min.x, half), rect.max);
    // Compared before writing, so a still window does not mark every camera changed each frame.
    let same = |a: &Option<Viewport>, b: &Option<Viewport>| match (a, b) {
        (Some(a), Some(b)) => a.physical_position == b.physical_position && a.physical_size == b.physical_size,
        (None, None) => true,
        _ => false,
    };
    for mut camera in &mut first_person {
        if !same(&camera.viewport, &top) {
            camera.viewport = top.clone();
        }
    }
    for mut camera in &mut stage {
        if !same(&camera.viewport, &bottom) {
            camera.viewport = bottom.clone();
        }
    }
}

/// From the shooter's right, a little behind and above, so a body is seen side on.
const STAGE_LOOK: Vec3 = Vec3::new(-1.0, -0.3, -0.5);
/// Meters ahead of the shooter a puppet may stand and still be framed: past the farthest target a
/// beat stands up (a reflect's rocketeer at 15 m), so the stage view shows both ends of the action.
const FRAME_AHEAD: f32 = 16.0;
/// Meters above a body's box kept in frame, for a hat.
const HAT_ROOM: f32 = 0.45;
/// Share of the frame's half-width left clear round what is framed.
const FRAME_PADDING: f32 = 0.08;
/// Per second, exponential, how fast the stage camera eases onto its framing.
const FRAME_EASE: f32 = 6.0;

/// What the stage camera keeps in frame, with the floor under the spawn and air ahead of it.
fn framed_points(puppets: impl IntoIterator<Item = Vec3>) -> Vec<Vec3> {
    let mut points = vec![RANGE_ORIGIN, RANGE_ORIGIN + Vec3::new(0.0, 1.0, -3.0)];
    for center in puppets {
        if RANGE_ORIGIN.z - center.z > FRAME_AHEAD {
            continue;
        }
        for corner in 0..8 {
            let sign = Vec3::new(
                if corner & 1 == 0 { -1.0 } else { 1.0 },
                if corner & 2 == 0 { -1.0 } else { 1.0 },
                if corner & 4 == 0 { -1.0 } else { 1.0 },
            );
            points.push(center + sign * CLASS_HALF_EXTENTS);
        }
        points.push(center + Vec3::Y * (CLASS_HALF_EXTENTS.y + HAT_ROOM));
    }
    points
}

/// On real time, so it settles while the range is paused or slowed.
fn frame_the_stage(
    time: Res<Time<Real>>,
    puppets: Query<&PhysicsBody, With<RangePuppet>>,
    mut camera: Single<(Entity, &Camera, &Projection, &mut Transform), With<RangeCamera>>,
    // A camera not framed before is put in place at once rather than eased.
    mut framed: Local<Option<Entity>>,
) {
    let (entity, camera, projection, transform) = &mut *camera;
    let Projection::Perspective(perspective) = projection else { return };
    let aspect = camera.viewport.as_ref().map_or(perspective.aspect_ratio, |viewport| {
        viewport.physical_size.x as f32 / viewport.physical_size.y.max(1) as f32
    });
    let rotation = Transform::IDENTITY.looking_to(STAGE_LOOK, Vec3::Y).rotation;
    let points = framed_points(puppets.iter().map(|body| body.current));
    let Some(framing) = crate::prop::thumbnail::frame_wide(points, rotation, perspective.fov, aspect, FRAME_PADDING) else {
        return;
    };
    let ease = match framed.replace(*entity) == Some(*entity) {
        true => 1.0 - (-FRAME_EASE * time.delta_secs()).exp(),
        false => 1.0,
    };
    transform.translation = transform.translation.lerp(framing.translation, ease);
    transform.rotation = rotation;
}

/// Pausing stops the clock rather than the director, so a body in the air stays there.
fn keep_time(show: Res<RangeShow>, mut time: ResMut<Time<Virtual>>) {
    if time.relative_speed() != show.speed {
        time.set_relative_speed(show.speed);
    }
    match (show.playing, time.is_paused()) {
        (true, true) => time.unpause(),
        (false, false) => time.pause(),
        _ => {}
    }
}

pub struct RangePlugin;

impl Plugin for RangePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RangeShow>()
            .init_resource::<HiddenByRange>()
            .init_resource::<MapCollision>()
            .init_resource::<ChosenView>()
            .add_systems(OnEnter(AppMode::Weapon), (enter_range, crate::game::player::spawn_the_view))
            .add_systems(OnExit(AppMode::Weapon), (leave_range, crate::game::player::despawn_the_view))
            // Hits are read before the director moves on, so each lands in the beat it was dealt in.
            .add_systems(FixedPreUpdate, (tally, direct).chain().run_if(in_state(AppMode::Weapon)))
            .add_systems(Update, (place_the_views, frame_the_stage, keep_time).chain().run_if(in_state(AppMode::Weapon)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon::{AirblastSpec, Cadence, Cost, HitscanSpec};
    use crate::common::weapon_file::default_catalog;

    fn mounted(action: WeaponAction) -> Mounted {
        Mounted { action, cadence: Cadence::Semi { interval: 0.5 }, cost: Cost::Free }
    }

    /// Fails when a new `WeaponAction` variant has nothing to show in the range.
    #[test]
    fn every_action_kind_contributes_a_beat() {
        use bevy::reflect::{TypeInfo, Typed};
        let TypeInfo::Enum(info) = WeaponAction::type_info() else { panic!("not an enum") };
        let registry = {
            let mut registry = bevy::reflect::TypeRegistry::default();
            registry.register::<Mounted>();
            registry
        };
        for name in info.variant_names() {
            let built = super::super::form::default_variant(&registry, info, name).expect("buildable");
            let mut action = WeaponAction::None;
            bevy::reflect::PartialReflect::try_apply(&mut action, built.as_ref()).expect("applies");
            let beats = beats_for(Button::Primary, &mounted(action));
            assert_eq!(beats.is_empty(), matches!(action, WeaponAction::None), "{name} contributes {beats:?}");
        }
    }

    #[test]
    fn an_airblast_shows_each_of_its_parts() {
        let beats = beats_for(Button::Secondary, &mounted(WeaponAction::Airblast(AirblastSpec::default())));
        let kinds: Vec<_> = beats.iter().map(|beat| std::mem::discriminant(&beat.kind)).collect();
        assert_eq!(kinds.len(), 3, "{beats:?}");
        let pushless = AirblastSpec { push: 0.0, lift: 0.0, reflect: false, ..AirblastSpec::default() };
        assert_eq!(beats_for(Button::Secondary, &mounted(WeaponAction::Airblast(pushless))).len(), 1);
    }

    #[test]
    fn every_shipped_weapon_has_a_loop_on_the_stage() {
        let catalog = default_catalog();
        for weapon in &catalog.weapons {
            let (other, _) = helpers(&catalog, weapon.id);
            let beats = loop_for(weapon, other);
            assert!(!beats.is_empty(), "{} shows nothing", weapon.key);
            for beat in &beats {
                if let BeatKind::Shoot { distance, .. } | BeatKind::Burn { distance, .. } | BeatKind::Push { distance, .. } = beat.kind {
                    assert!(distance > 0.0 && distance < -STAGE_MIN.z, "{}: {beat:?} is off the stage", weapon.key);
                }
            }
        }
    }

    #[test]
    fn a_hitscan_is_shot_at_the_ends_of_its_lines() {
        let spec = HitscanSpec {
            falloff: Some(crate::common::weapon::DistanceScale { near: 5.0, far: 10.0, scale: 0.5 }),
            ramp_up: Some(crate::common::weapon::DistanceScale { near: 0.0, far: 5.0, scale: 1.5 }),
            ..HitscanSpec::default()
        };
        let distances: Vec<f32> = beats_for(Button::Primary, &mounted(WeaponAction::Hitscan(spec)))
            .iter()
            .filter_map(|beat| match beat.kind {
                BeatKind::Shoot { distance, .. } => Some(distance),
                _ => None,
            })
            .collect();
        assert_eq!(distances, vec![2.0, 10.0]);
    }

    /// The range plays the round's systems on puppets, and nothing about it is a round: nobody is
    /// seated, nobody scores. Hits need rigs, which need the renderer, so this watches the shots.
    #[test]
    fn the_range_shoots_but_seats_nobody_and_scores_nothing() {
        use crate::game::score::Standing;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_plugins(bevy::input::InputPlugin)
            .add_plugins(bevy::time::TimePlugin)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 64.0)))
            .add_plugins(crate::game::GamePlugin)
            .add_plugins((crate::game::hitbox::HitboxPlugin, crate::game::weapon::WeaponPlugin))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<crate::common::air_lit::LitMaterial>>()
            .add_plugins(RangePlugin);
        let mut editor = WeaponEditor::default();
        editor.open(crate::common::weapon_file::read_pack_weapons(
            &crate::common::assets::Assets::default(),
            &crate::common::assets::default_packs(),
        ));
        editor.selected = editor.weapons.iter().position(|weapon| weapon.key == "rocket_launcher");
        assert!(editor.selected.is_some(), "the default pack ships a rocket launcher");
        app.insert_resource(editor).insert_resource(default_catalog());

        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Weapon);
        let (mut most, mut shots) = (0, 0);
        for _ in 0..(64 * 3) {
            app.update();
            let mut puppets = app.world_mut().query::<&RangePuppet>();
            most = most.max(puppets.iter(app.world()).count());
            let mut projectiles = app.world_mut().query_filtered::<(), With<Projectile>>();
            shots = shots.max(projectiles.iter(app.world()).count());
        }
        assert!(most >= 2, "the shooter and its target never stood up");
        assert!(shots > 0, "the shooter never fired");

        // An edit that changes the loop plays the same beat again, rather than starting over.
        app.world_mut().resource_mut::<RangeShow>().step = 1;
        for _ in 0..8 {
            app.update();
        }
        let before = app.world().resource::<RangeShow>().current;
        assert!(before > 0, "the step did not move on, so this tests nothing");
        let id = WeaponId::of("rocket_launcher");
        let mut catalog = app.world_mut().resource_mut::<WeaponCatalog>();
        let mut edited = catalog.get(id).expect("shipped").clone();
        edited.deploy += 0.5;
        catalog.insert(edited);
        app.update();
        app.update();
        let show = app.world().resource::<RangeShow>();
        assert_eq!(show.current, before, "the edit sent the loop back to the start");
        assert!(show.elapsed < 0.1, "the beat was not played again");

        // Choosing a weapon with a shorter loop, from a beat past its end.
        for _ in 0..3 {
            app.world_mut().resource_mut::<RangeShow>().step = 1;
            app.update();
        }
        let mut editor = app.world_mut().resource_mut::<WeaponEditor>();
        editor.selected = editor.weapons.iter().position(|weapon| weapon.key == "medi_wrench");
        app.update();
        app.update();
        let show = app.world().resource::<RangeShow>();
        assert!(show.current < show.beats.len());
        let mut standings = app.world_mut().query::<&Standing>();
        assert_eq!(standings.iter(app.world()).count(), 0, "a puppet was given a standing");
        assert!(app.world().get_resource::<crate::game::respawn::OurIdentity>().is_none(), "somebody was seated");

        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Editor);
        app.update();
        let mut puppets = app.world_mut().query::<&RangePuppet>();
        assert_eq!(puppets.iter(app.world()).count(), 0, "puppets outlived the range");
    }

    #[test]
    fn skipping_finds_the_next_beat_that_is_on() {
        let off = [false, true, true, false];
        assert_eq!(next_on(&off, 0, true), Some(3));
        assert_eq!(next_on(&off, 3, true), Some(0));
        assert_eq!(next_on(&off, 0, false), Some(3));
        assert_eq!(next_on(&[true, true], 0, true), None);
    }
}
