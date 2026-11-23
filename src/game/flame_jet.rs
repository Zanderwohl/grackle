//! The flamethrower's flames: a `bevy_hanabi` effect per body that is firing
//! one, drawn from its muzzle along its aim. Drawing only — the puff was
//! resolved on the tick by [`crate::game::flame`], whose traced cone is drawn
//! on `F6`.
//!
//! Each flame flies straight down the middle of the cone, spread a little at
//! random, for the first three quarters of the range (or until a quarter of
//! the range short of a wall), then brakes, rises and fades. The brake is
//! [`SPEED`] over a quarter of the range, so a flame stops drifting forward at
//! the cone's end. Flames are unlit and brighter than one, so they glow; they
//! light nothing else.
//!
//! A flame is a billboard cut from a generated sheet of shapes ([`flame_sheet`]).
//!
//! A body [`OnFire`] has its own emitter, the same flames rising from
//! inside its movement hull from the start, drawn in first person too. A
//! corpse or gib that [`DiedBurning`] keeps a fainter one over its bones.
//!
//! A jet's emitter is lit by [`ShotSeen`], so a remote body's flames come from the
//! relayed `Effect::Fired` and our own from the press, as recoil does. Every
//! machine makes its own; nothing is sent.

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy_hanabi::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::flame::{FlameSpec, OnFire};
use crate::common::hitbox::Hitboxes;
use crate::common::weapon::{Equipped, WeaponAction, WeaponCatalog};
use crate::game::collision::CollisionWorld;
use crate::game::held::{HeldModel, WeaponInHand};
use crate::game::hitscan::{aim, eye};
use crate::game::player::{LocalPlayer, PhysicsBody, Player, ViewMode, Watched};
use crate::game::ragdoll::{DiedBurning, Ragdoll};
use crate::game::recoil::{start_recoil, ShotSeen};
use crate::game::viewmodel::ApparentMuzzle;
use crate::game::warm_up::{self, WarmUp};
use crate::game::weapon::Button;

/// Meters per second down the cone.
const SPEED: f32 = 12.0;

/// Share of the range a flame flies straight.
const STRAIGHT_SHARE: f32 = 0.75;

/// Sideways speed at most, as a share of the cone's own spread, so flames stay
/// near its middle.
const SIDEWAYS_SHARE: f32 = 0.5;

/// Meters per second squared, once a flame stops flying straight.
const RISE: f32 = 14.0;

/// Seconds a flame rises and fades for.
const RISE_SECONDS: (f32, f32) = (0.3, 0.55);

/// Width in meters as it leaves the nozzle, and meters it grows per second.
const START_SIZE: f32 = 0.05;
const GROWTH: f32 = 0.8;

/// Per flame, a multiplier on [`START_SIZE`] and [`GROWTH`].
const SIZE_SHARE: (f32, f32) = (0.8, 1.3);

const RATE: f32 = 280.0;

/// Seconds a flame flies straight at most, so a long-range spec fits
/// [`CAPACITY`]; past 24 m its flames start rising sooner than three quarters.
const MOST_STRAIGHT: f32 = 1.5;

const CAPACITY: u32 = (RATE * (MOST_STRAIGHT + RISE_SECONDS.1) * 1.1) as u32 + 1;
const AFTERBURN_CAPACITY: u32 = (AFTERBURN_RATE * RISE_SECONDS.1 * 1.1) as u32 + 1;
const CORPSE_CAPACITY: u32 = (CORPSE_RATE * CORPSE_RISE_SECONDS.1 * 1.1) as u32 + 1;

/// Seconds an emitter stays lit after the last shot seen, in shot intervals,
/// and at least [`LIT_AFTER`], since a relayed shot can arrive late.
const LIT_INTERVALS: f32 = 2.0;
const LIT_AFTER: f32 = 0.15;

/// Seconds after an emitter goes out before it is removed, once its last flame
/// has faded.
const LAST_FLAME_GONE: f32 = 1.5;

/// Flames a second from a burning body.
const AFTERBURN_RATE: f32 = 120.0;

/// Meters per second a burning body's flames start rising at, and per second,
/// exponential, how fast that start fades.
const AFTERBURN_SPEED: f32 = 1.5;
const AFTERBURN_BRAKE: f32 = 3.0;

/// Shares of the movement hull's half-extents its flames start in: inside its
/// sides, and not much above its top.
const AFTERBURN_BOX: Vec3 = Vec3::new(0.6, 0.75, 0.6);

/// On [`START_SIZE`] and [`GROWTH`].
const AFTERBURN_SCALE: f32 = 1.4;

/// A corpse that died burning smolders: half the afterburn's flames, a third
/// the size, rising a sixth as fast, each lasting twice as long.
const CORPSE_RATE: f32 = AFTERBURN_RATE * 0.5;
const CORPSE_SCALE: f32 = AFTERBURN_SCALE / 3.0;
const CORPSE_SPEED: f32 = AFTERBURN_SPEED / 6.0;
const CORPSE_LIFT: f32 = RISE / 6.0;
const CORPSE_RISE_SECONDS: (f32, f32) = (RISE_SECONDS.0 * 2.0, RISE_SECONDS.1 * 2.0);

/// Share of the box round a corpse's bones its flames start in, and meters of
/// half-extent at least, so a lone hand still has room to burn.
const CORPSE_BOX_SHARE: f32 = 0.5;
const CORPSE_LEAST_BOX: f32 = 0.04;

/// Linear, multiplied into the sheet's colors. Above one, so the flames glow
/// once tonemapped.
const HOT: Vec3 = Vec3::new(2.6, 2.4, 2.1);
const RISING: Vec3 = Vec3::new(2.0, 1.4, 1.0);
const EMBER: Vec3 = Vec3::new(1.1, 0.45, 0.25);

/// Alpha before fading. Flames add, so many at the nozzle sum to white.
const OPACITY: f32 = 0.6;

/// A flame is drawn this many times longer, up the screen, than it is wide.
const ASPECT: f32 = 1.7;

/// Radians a flame leans from upright at most, and how far and how fast it
/// sways while it flies.
const LEAN: f32 = 0.35;
const SWAY: f32 = 0.18;
const SWAY_RATE: f32 = 9.0;

/// Share its length flickers by, and how fast.
const FLICKER: f32 = 0.18;
const FLICKER_RATE: f32 = 14.0;

/// Shapes a second a flame steps through its sheet.
const SHAPE_RATE: f32 = 10.0;

pub(crate) const FLAME_GRID: UVec2 = UVec2::new(4, 2);
pub(crate) const FLAME_TEXELS: u32 = 96;

/// The sheet of [`flame_sheet`] shapes, for anything else drawing flames.
#[derive(Resource)]
pub(crate) struct FlameSheet(pub Handle<Image>);

#[derive(Resource)]
struct FlameLook {
    effect: Handle<EffectAsset>,
    afterburn: Handle<EffectAsset>,
    corpse: Handle<EffectAsset>,
    sheet: Handle<Image>,
}

/// An emitter for `body`'s `button`. `idle` is seconds since it went out.
#[derive(Component)]
struct FlameJet {
    body: Entity,
    button: Button,
    lit_for: f32,
    idle: f32,
}

/// Seconds a flame flies straight, given the weapon's range and the meters to
/// the first wall along the aim.
fn straight_seconds(range: f32, reach: f32) -> f32 {
    ((range * STRAIGHT_SHARE).min(reach - range * (1.0 - STRAIGHT_SHARE)).max(0.0) / SPEED).min(MOST_STRAIGHT)
}

/// Per second, exponential: what stops a flame a quarter of the range after it
/// stops flying straight.
fn brake(range: f32) -> f32 {
    SPEED / (range * (1.0 - STRAIGHT_SHARE)).max(0.1)
}

/// A flame jet and afterburn are one effect at two rates; each emitter sets
/// its own properties, whose defaults are the jet's.
/// `rise` is the range of seconds a flame rises and fades for.
fn flame_effect(name: &'static str, rate: f32, capacity: u32, rise: (f32, f32)) -> EffectAsset {
    let writer = ExprWriter::new();
    let speed = writer.add_property("speed", SPEED.into());
    // Meters per second squared upward once a flame stops flying straight.
    let lift = writer.add_property("lift", RISE.into());
    // Half-extents, in meters, of the box about the emitter flames start in.
    let spawn_box = writer.add_property("spawn_box", Vec3::splat(0.02).into());
    let scale = writer.add_property("scale", 1.0.into());
    let direction = writer.add_property("direction", Vec3::NEG_Z.into());
    let carry = writer.add_property("carry", Vec3::ZERO.into());
    let sideways = writer.add_property("sideways", 0.0.into());
    let straight = writer.add_property("straight", 0.0.into());
    let braking = writer.add_property("brake", 0.0.into());

    let age = || writer.attr(Attribute::AGE);
    // Captured at birth, so a flame keeps the path it was fired on.
    let t_straight = || writer.attr(Attribute::F32_0);
    let t_brake = || writer.attr(Attribute::F32_1);
    let size_share = || writer.attr(Attribute::F32_2);
    // 0 to 1, per flame: its lean, the phase of its sway and flicker, and
    // where in the sheet it starts.
    let seed = || writer.attr(Attribute::F32_3);

    let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.0).expr());
    let anywhere = writer.rand(VectorType::VEC3F) * writer.lit(2.0) - writer.lit(1.0);
    let init_position = SetAttributeModifier::new(Attribute::POSITION, (anywhere * writer.prop(spawn_box)).expr());
    let random = writer.rand(VectorType::VEC3F) * writer.lit(2.0) - writer.lit(1.0);
    let velocity = writer.prop(direction) * writer.prop(speed) + random * writer.prop(sideways) + writer.prop(carry);
    let init_velocity = SetAttributeModifier::new(Attribute::VELOCITY, velocity.expr());
    let init_straight = SetAttributeModifier::new(Attribute::F32_0, writer.prop(straight).expr());
    let init_brake = SetAttributeModifier::new(Attribute::F32_1, writer.prop(braking).expr());
    let init_size_share =
        SetAttributeModifier::new(Attribute::F32_2, writer.lit(SIZE_SHARE.0).uniform(writer.lit(SIZE_SHARE.1)).expr());
    let rising = writer.lit(rise.0).uniform(writer.lit(rise.1));
    let init_lifetime = SetAttributeModifier::new(Attribute::LIFETIME, (writer.prop(straight) + rising).expr());
    let init_seed = SetAttributeModifier::new(Attribute::F32_3, writer.rand(ScalarType::Float).expr());

    // 0 while flying straight, 1 after.
    let after = || age().step(t_straight());
    // Not `LinearDragModifier`, whose `v *= 1 - k * dt` depends on frame rate.
    let braked = writer.attr(Attribute::VELOCITY) * (writer.lit(-1.0) * t_brake() * writer.delta_time() * after()).exp();
    let lifted = writer.lit(Vec3::Y) * writer.prop(lift) * writer.delta_time() * after();
    let update_velocity = SetAttributeModifier::new(Attribute::VELOCITY, (braked + lifted).expr());

    let phase = || seed() * writer.lit(std::f32::consts::TAU);
    let width = (writer.lit(START_SIZE) + writer.lit(GROWTH) * age()) * size_share() * writer.prop(scale);
    let flicker = writer.lit(1.0) + writer.lit(FLICKER) * (age() * writer.lit(FLICKER_RATE) + phase()).sin();
    let length = width.clone() * writer.lit(ASPECT) * flicker;
    let update_size = SetAttributeModifier::new(Attribute::SIZE2, length.vec2(width).expr());

    let shapes = (FLAME_GRID.x * FLAME_GRID.y) as f32;
    let shape = (seed() * writer.lit(shapes) + age() * writer.lit(SHAPE_RATE)).rem(writer.lit(shapes));
    let update_shape = SetAttributeModifier::new(
        Attribute::SPRITE_INDEX,
        // Short of `shapes`, so the floor never reaches past the last.
        shape.min(writer.lit(shapes - 0.001)).floor().cast(ScalarType::Int).expr(),
    );

    let to_rising = (age() / t_straight().max(writer.lit(1.0e-3))).saturate();
    let risen = ((age() - t_straight()) / (writer.attr(Attribute::LIFETIME) - t_straight()).max(writer.lit(1.0e-3))).saturate();
    let color = writer.lit(HOT).mix(writer.lit(RISING), to_rising).mix(writer.lit(EMBER), risen.clone());
    let alpha = (writer.lit(1.0) - risen) * writer.lit(OPACITY);
    let update_color = SetAttributeModifier::new(Attribute::HDR_COLOR, color.vec4_xyz_w(alpha).expr());

    // A quarter turn stands the quad's x, the flame's length, up the screen.
    // Oriented along its velocity instead, a flame flying away from the eye
    // is drawn as a sliver.
    let upright = writer.lit(std::f32::consts::FRAC_PI_2)
        + (seed() * writer.lit(2.0) - writer.lit(1.0)) * writer.lit(LEAN)
        + writer.lit(SWAY) * (age() * writer.lit(SWAY_RATE) + phase()).sin();
    let upright = upright.expr();

    let mut module = writer.finish();
    module.add_texture_slot("flames", SlotDimension::D2);

    EffectAsset::new(capacity, SpawnerSettings::rate(rate.into()), module)
        .with_name(name)
        .with_alpha_mode(bevy_hanabi::AlphaMode::Add)
        .init(init_position)
        .init(init_velocity)
        .init(init_age)
        .init(init_straight)
        .init(init_brake)
        .init(init_size_share)
        .init(init_lifetime)
        .init(init_seed)
        .update(update_velocity)
        .update(update_size)
        .update(update_shape)
        .update(update_color)
        .render(OrientModifier::new(OrientMode::FaceCameraPosition).with_rotation(upright))
        .render(FlipbookModifier { sprite_grid_size: FLAME_GRID })
        .render(ParticleTextureModifier { texture_slot: 0, sample_mapping: ImageSampleMapping::Modulate })
}

/// 0 to 1, the same on every machine.
pub(crate) fn noise(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

/// Smooth value noise on a lattice of `cells` per unit, 0 to 1.
pub(crate) fn value_noise(p: Vec2, cells: f32, seed: u32) -> f32 {
    let q = p * cells;
    let cell = q.floor();
    let f = q - cell;
    let corner = |dx: i32, dy: i32| {
        let (x, y) = (cell.x as i32 + dx, cell.y as i32 + dy);
        noise((x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663) ^ seed)
    };
    let s = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let bottom = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * s.x;
    let top = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * s.x;
    bottom + (top - bottom) * s.y
}

/// One tongue of a flame, in a cell's coordinates: `along` 0 to 1 from base
/// to tip, `across` -1 to 1.
struct Tongue {
    /// Where along the cell its base and tip are.
    base: f32,
    tip: f32,
    /// Half-width at its widest, and where across it starts.
    width: f32,
    offset: f32,
    /// How far its middle wanders across toward the tip, how many half-waves,
    /// and where in the wave it starts.
    wander: f32,
    waves: f32,
    phase: f32,
}

impl Tongue {
    /// 1 down its middle, 0 at its edge, below 0 outside.
    fn density(&self, along: f32, across: f32) -> f32 {
        let t = (along - self.base) / (self.tip - self.base);
        if !(0.0..=1.0).contains(&t) {
            return -1.0;
        }
        // Round at the base, widest a third of the way up, pointed at the tip.
        let half = self.width * (std::f32::consts::PI * t.powf(0.55)).sin().max(0.0).powf(0.7) * (1.0 - t).powf(0.6);
        let middle = self.offset + self.wander * t.powf(1.5) * (self.waves * std::f32::consts::PI * t + self.phase).sin();
        1.0 - (across - middle).abs() / half.max(1.0e-4)
    }
}

/// Linear color by heat, 0 at a flame's edge to 1 at its hottest.
fn heat_color(heat: f32) -> Vec3 {
    const STOPS: [(f32, Vec3); 4] = [
        (0.0, Vec3::new(0.55, 0.06, 0.01)),
        (0.35, Vec3::new(0.95, 0.32, 0.04)),
        (0.7, Vec3::new(1.0, 0.68, 0.22)),
        (1.0, Vec3::new(1.0, 0.95, 0.78)),
    ];
    let heat = heat.clamp(0.0, 1.0);
    let i = STOPS.iter().rposition(|(at, _)| *at <= heat).unwrap_or(0).min(STOPS.len() - 2);
    let ((a, from), (b, to)) = (STOPS[i], STOPS[i + 1]);
    from.lerp(to, (heat - a) / (b - a))
}

/// [`FLAME_GRID`] flames of `texels` square, as linear RGBA, rows top to
/// bottom. Each flame's base is at its cell's left and its tips at the right.
pub fn flame_sheet(texels: u32) -> Vec<u8> {
    let (width, height) = (FLAME_GRID.x * texels, FLAME_GRID.y * texels);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let texel = 1.0 / texels as f32;
    for flame in 0..FLAME_GRID.x * FLAME_GRID.y {
        let seed = flame.wrapping_mul(0x51ED_270B) + 7;
        let n = |k: u32| noise(seed.wrapping_add(k));
        let mut tongues = vec![Tongue {
            base: 0.04,
            tip: 0.85 + 0.1 * n(1),
            width: 0.42 + 0.1 * n(2),
            offset: (n(3) - 0.5) * 0.12,
            wander: 0.1 + 0.15 * n(4),
            waves: 1.0 + 1.5 * n(5),
            phase: n(6) * std::f32::consts::TAU,
        }];
        for lick in 0..1 + (n(7) * 2.99) as u32 {
            let k = 10 + lick * 10;
            let side = if n(k) < 0.5 { -1.0 } else { 1.0 };
            let base = 0.2 + 0.25 * n(k + 1);
            tongues.push(Tongue {
                base,
                tip: (base + 0.35 + 0.35 * n(k + 2)).min(0.96),
                width: 0.14 + 0.12 * n(k + 3),
                offset: side * (0.2 + 0.25 * n(k + 4)),
                wander: side * (0.1 + 0.2 * n(k + 5)),
                waves: 0.5 + 1.5 * n(k + 6),
                phase: n(k + 7) * std::f32::consts::TAU,
            });
        }
        let (column, row) = (flame % FLAME_GRID.x, flame / FLAME_GRID.x);
        for y in 0..texels {
            for x in 0..texels {
                let along = (x as f32 + 0.5) * texel;
                let across = ((y as f32 + 0.5) * texel) * 2.0 - 1.0;
                let p = Vec2::new(along, across);
                let fray = (0.6 * value_noise(p, 5.0, seed) + 0.4 * value_noise(p, 11.0, seed ^ 0xABCD) - 0.5) * (0.15 + 0.5 * along);
                let density = tongues.iter().map(|tongue| tongue.density(along, across)).fold(-1.0, f32::max) + fray;
                // About two texels of edge: sharp, not blurred.
                let alpha = (density / (4.0 * texel)).clamp(0.0, 1.0);
                let streaks = 0.75 + 0.5 * value_noise(Vec2::new(along * 2.0, across * 6.0), 1.5, seed ^ 0x5EED);
                let heat = density.max(0.0).powf(0.6) * (1.15 - 0.75 * along) * streaks;
                let color = heat_color(heat);
                let index = (((row * texels + y) * width + column * texels + x) * 4) as usize;
                let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                rgba[index..index + 4].copy_from_slice(&[byte(color.x), byte(color.y), byte(color.z), byte(alpha)]);
            }
        }
    }
    rgba
}

/// A generated sheet, written linear, so not decoded as sRGB.
pub(crate) fn sheet_image(rgba: Vec<u8>, width: u32, height: u32) -> Image {
    sheet_image_as(rgba, width, height, bevy::render::render_resource::TextureFormat::Rgba8Unorm)
}

pub(crate) fn sheet_image_as(rgba: Vec<u8>, width: u32, height: u32, format: bevy::render::render_resource::TextureFormat) -> Image {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::ImageSampler;
    use bevy::render::render_resource::{Extent3d, TextureDimension};

    let mut image = Image::new(
        Extent3d { width, height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        rgba,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

fn make_the_look(mut commands: Commands, mut effects: ResMut<Assets<EffectAsset>>, mut images: ResMut<Assets<Image>>) {
    let sheet = images.add(sheet_image(flame_sheet(FLAME_TEXELS), FLAME_GRID.x * FLAME_TEXELS, FLAME_GRID.y * FLAME_TEXELS));
    commands.insert_resource(FlameSheet(sheet.clone()));
    commands.insert_resource(FlameLook {
        effect: effects.add(flame_effect("flame_jet", RATE, CAPACITY, RISE_SECONDS)),
        afterburn: effects.add(flame_effect("afterburn", AFTERBURN_RATE, AFTERBURN_CAPACITY, RISE_SECONDS)),
        corpse: effects.add(flame_effect("corpse_fire", CORPSE_RATE, CORPSE_CAPACITY, CORPSE_RISE_SECONDS)),
        sheet,
    });
}

fn flame_of(catalog: &WeaponCatalog, equipped: &Equipped, button: Button) -> Option<FlameSpec> {
    match button.of(catalog.held_by(equipped)?).action {
        WeaponAction::Flame(spec) => Some(spec),
        _ => None,
    }
}

fn light_the_jets(
    mut commands: Commands,
    look: Res<FlameLook>,
    catalog: Res<WeaponCatalog>,
    mut seen: MessageReader<ShotSeen>,
    bodies: Query<&Equipped>,
    mut jets: Query<&mut FlameJet>,
) {
    for shot in seen.read() {
        let Ok(equipped) = bodies.get(shot.body) else { continue };
        let Some(weapon) = catalog.held_by(equipped) else { continue };
        let mounted = shot.button.of(weapon);
        if !matches!(mounted.action, WeaponAction::Flame(_)) {
            continue;
        }
        let lit_for = (mounted.cadence.interval() * LIT_INTERVALS).max(LIT_AFTER);
        match jets.iter_mut().find(|jet| jet.body == shot.body && jet.button == shot.button) {
            Some(mut jet) => {
                jet.lit_for = lit_for;
                jet.idle = 0.0;
            }
            None => {
                commands.spawn((
                    FlameJet { body: shot.body, button: shot.button, lit_for, idle: 0.0 },
                    ParticleEffect::new(look.effect.clone()),
                    EffectMaterial { images: vec![look.sheet.clone()] },
                    EffectProperties::default(),
                    // Placed by `aim_the_jets` before Hanabi first reads it.
                    Transform::default(),
                    Name::new("Flame jet"),
                ));
            }
        }
    }
}

/// Where a body's flames leave: for the body we view in first person, the
/// viewmodel's muzzle; otherwise the held model's; failing either, the eye.
pub(crate) fn nozzle(
    body: &Transform,
    eye: Vec3,
    held: Option<(&HeldModel, &WeaponInHand)>,
    first_person: bool,
    apparent: Option<Vec3>,
) -> Vec3 {
    if first_person {
        return apparent.unwrap_or(eye);
    }
    held.and_then(|(held, in_hand)| held.muzzle().map(|muzzle| body.transform_point(in_hand.0.transform_point(muzzle))))
        .unwrap_or(eye)
}

/// Where an effect fired by a body leaves: [`nozzle`], first person when the body is ours or
/// watched (`viewed`) and the view is inside it.
pub(crate) fn effect_origin(
    body: &Transform,
    player: &Player,
    stance: Stance,
    held: Option<(&HeldModel, &WeaponInHand)>,
    viewed: bool,
    view: Option<&ViewMode>,
    apparent: Option<&ApparentMuzzle>,
) -> Vec3 {
    let first_person = viewed && view.is_some_and(|view| !view.shows_own_body());
    let eye = eye(body.translation, stance, player.duck, player.eye);
    nozzle(body, eye, held, first_person, apparent.and_then(|apparent| apparent.0))
}

#[allow(clippy::type_complexity)]
fn aim_the_jets(
    mut commands: Commands,
    time: Res<Time>,
    particles: Res<Time<EffectSimulation>>,
    catalog: Res<WeaponCatalog>,
    world: Res<CollisionWorld>,
    view: Option<Res<ViewMode>>,
    apparent: Option<Res<ApparentMuzzle>>,
    bodies: Query<(
        &Transform,
        &Player,
        &Stance,
        &Equipped,
        Option<(&HeldModel, &WeaponInHand)>,
        Has<LocalPlayer>,
        Has<Watched>,
    )>,
    mut jets: Query<(Entity, &mut FlameJet, &mut Transform, &mut EffectProperties, Option<&mut EffectSpawner>), Without<Player>>,
) {
    let dt = time.delta_secs();
    // On the particles' clock, so a jet is not taken down while the kill cam holds its flames.
    let idle_dt = particles.delta_secs();
    for (entity, mut jet, mut transform, mut properties, spawner) in &mut jets {
        jet.lit_for -= dt;
        let found = bodies.get(jet.body).ok().and_then(|body| {
            let spec = flame_of(&catalog, body.3, jet.button)?;
            Some((body, spec))
        });
        let lit = jet.lit_for > 0.0 && found.is_some();
        jet.idle = if lit { 0.0 } else { jet.idle + idle_dt };
        if jet.idle >= LAST_FLAME_GONE {
            commands.entity(entity).despawn();
            continue;
        }
        // Added by Hanabi in `PostUpdate`, so absent on an emitter's first frame.
        if let Some(mut spawner) = spawner
            && spawner.active != lit
        {
            spawner.active = lit;
        }
        let Some(((body, player, stance, _, held, ours, watched), spec)) = found else { continue };
        if !lit {
            continue;
        }

        let from = effect_origin(body, player, *stance, held, ours || watched, view.as_deref(), apparent.as_deref());
        let direction = aim(player);
        // From where the flames start, which can be around a corner from the eye.
        let reach = world.ray_distance(&Ray3d::new(from, direction), spec.range_m).unwrap_or(spec.range_m);

        transform.translation = from;
        properties.set("direction", Vec3::from(direction).into());
        properties.set("carry", player.velocity.into());
        properties.set("sideways", (SPEED * spec.spread.tan() * SIDEWAYS_SHARE).into());
        properties.set("straight", straight_seconds(spec.range_m, reach).into());
        properties.set("brake", brake(spec.range_m).into());
    }
}

/// Flames rising off a body [`OnFire`]. `idle` is seconds since it went out.
#[derive(Component)]
struct Afterburn {
    body: Entity,
    idle: f32,
}

#[derive(Component)]
struct AfterburnBy;

fn light_the_burning(
    mut commands: Commands,
    look: Res<FlameLook>,
    alight: Query<Entity, (With<OnFire>, With<Hitboxes>, Without<AfterburnBy>)>,
) {
    for body in &alight {
        let mut properties = EffectProperties::default();
        properties.set("direction", Vec3::Y.into());
        properties.set("speed", AFTERBURN_SPEED.into());
        properties.set("brake", AFTERBURN_BRAKE.into());
        properties.set("scale", AFTERBURN_SCALE.into());
        commands.spawn((
            Afterburn { body, idle: 0.0 },
            ParticleEffect::new(look.afterburn.clone()),
            EffectMaterial { images: vec![look.sheet.clone()] },
            properties,
            // Placed by `follow_the_burning` before Hanabi first reads it.
            Transform::default(),
            Name::new("Afterburn"),
        ));
        commands.entity(body).insert(AfterburnBy);
    }
}

/// Where a body's flames start: its hull's middle, moved from the tick's
/// position to where the body is drawn, since the hull is boxed on the tick.
fn hull_middle(drawn: &Transform, boxes: &Hitboxes, tick: Option<&PhysicsBody>) -> Vec3 {
    match tick {
        Some(tick) => drawn.translation + (boxes.body.center - tick.current),
        None => boxes.body.center,
    }
}

/// Our own afterburn is drawn in first person too: the faint glow on the arms
/// is the only other sign of being on fire.
#[allow(clippy::type_complexity)]
fn follow_the_burning(
    mut commands: Commands,
    time: Res<Time<EffectSimulation>>,
    bodies: Query<(&Transform, &Hitboxes, Has<OnFire>, Option<&Player>, Option<&PhysicsBody>)>,
    mut afterburns: Query<
        (Entity, &mut Afterburn, &mut Transform, &mut EffectProperties, Option<&mut EffectSpawner>),
        Without<Hitboxes>,
    >,
) {
    let dt = time.delta_secs();
    for (entity, mut afterburn, mut transform, mut properties, spawner) in &mut afterburns {
        let body = bodies.get(afterburn.body).ok();
        let lit = body.is_some_and(|(_, _, burning, ..)| burning);
        afterburn.idle = if lit { 0.0 } else { afterburn.idle + dt };
        if afterburn.idle >= LAST_FLAME_GONE {
            commands.entity(entity).despawn();
            if body.is_some() {
                commands.entity(afterburn.body).remove::<AfterburnBy>();
            }
            continue;
        }
        // Added by Hanabi in `PostUpdate`, so absent on an emitter's first frame.
        if let Some(mut spawner) = spawner
            && spawner.active != lit
        {
            spawner.active = lit;
        }
        let Some((drawn, boxes, _, player, tick)) = body else { continue };
        if !lit {
            continue;
        }
        transform.translation = hull_middle(drawn, boxes, tick);
        properties.set("spawn_box", (boxes.body.half_extents * AFTERBURN_BOX).into());
        properties.set("carry", player.map_or(Vec3::ZERO, |player| player.velocity).into());
    }
}

/// Flames on a corpse that [`DiedBurning`], for as long as it lies there.
/// `idle` is seconds since the corpse went.
#[derive(Component)]
struct CorpseFire {
    corpse: Entity,
    idle: f32,
}

#[derive(Component)]
struct CorpseFireBy;

fn light_the_burnt_corpses(
    mut commands: Commands,
    look: Res<FlameLook>,
    corpses: Query<Entity, (With<DiedBurning>, With<Ragdoll>, Without<CorpseFireBy>)>,
) {
    for corpse in &corpses {
        let mut properties = EffectProperties::default();
        properties.set("direction", Vec3::Y.into());
        properties.set("speed", CORPSE_SPEED.into());
        properties.set("lift", CORPSE_LIFT.into());
        properties.set("brake", AFTERBURN_BRAKE.into());
        properties.set("scale", CORPSE_SCALE.into());
        commands.spawn((
            CorpseFire { corpse, idle: 0.0 },
            ParticleEffect::new(look.corpse.clone()),
            EffectMaterial { images: vec![look.sheet.clone()] },
            properties,
            // Placed by `follow_the_burnt_corpses` before Hanabi first reads it.
            Transform::default(),
            Name::new("Corpse fire"),
        ));
        commands.entity(corpse).insert(CorpseFireBy);
    }
}

/// The box a corpse's flames start in, centered on the bones it keeps, as a
/// center and half-extents.
fn corpse_fire_box(least: Vec3, most: Vec3) -> (Vec3, Vec3) {
    ((least + most) * 0.5, ((most - least) * 0.5 * CORPSE_BOX_SHARE).max(Vec3::splat(CORPSE_LEAST_BOX)))
}

fn follow_the_burnt_corpses(
    mut commands: Commands,
    time: Res<Time<EffectSimulation>>,
    corpses: Query<&Ragdoll>,
    mut fires: Query<(Entity, &mut CorpseFire, &mut Transform, &mut EffectProperties, Option<&mut EffectSpawner>)>,
) {
    let dt = time.delta_secs();
    for (entity, mut fire, mut transform, mut properties, spawner) in &mut fires {
        let bounds = corpses.get(fire.corpse).ok().and_then(Ragdoll::bounds);
        fire.idle = if bounds.is_some() { 0.0 } else { fire.idle + dt };
        if fire.idle >= CORPSE_RISE_SECONDS.1 {
            commands.entity(entity).despawn();
            continue;
        }
        // Added by Hanabi in `PostUpdate`, so absent on an emitter's first frame.
        if let Some(mut spawner) = spawner
            && spawner.active != bounds.is_some()
        {
            spawner.active = bounds.is_some();
        }
        let Some((least, most)) = bounds else { continue };
        let (center, half) = corpse_fire_box(least, most);
        transform.translation = center;
        properties.set("spawn_box", half.into());
    }
}

fn put_out_the_jets(
    mut commands: Commands,
    jets: Query<Entity, Or<(With<FlameJet>, With<Afterburn>, With<CorpseFire>)>>,
    bodies: Query<Entity, Or<(With<AfterburnBy>, With<CorpseFireBy>)>>,
) {
    for jet in &jets {
        commands.entity(jet).despawn();
    }
    for body in &bodies {
        commands.entity(body).remove::<(AfterburnBy, CorpseFireBy)>();
    }
}

/// See [`crate::game::warm_up`].
fn warm_up_the_flames(mut commands: Commands, look: Res<FlameLook>) {
    for effect in [&look.effect, &look.afterburn, &look.corpse] {
        let mut properties = EffectProperties::default();
        properties.set("straight", 0.2.into());
        properties.set("brake", brake(FlameSpec::FLAMETHROWER.range_m).into());
        commands.spawn((
            WarmUp,
            ParticleEffect::new(effect.clone()),
            EffectMaterial { images: vec![look.sheet.clone()] },
            properties,
            Transform::from_translation(Vec3::NEG_Z),
            RenderLayers::layer(crate::common::render_layers::WARM_UP),
            Name::new("Flame warm-up"),
        ));
    }
}

/// Added in `launch.rs` beside `GibBloodPlugin`: Hanabi needs a renderer, which
/// the game's tests do not have.
pub struct FlameJetPlugin;

impl Plugin for FlameJetPlugin {
    fn build(&self, app: &mut App) {
        warm_up::ensure(app);
        if !app.is_plugin_added::<HanabiPlugin>() {
            app.add_plugins(HanabiPlugin);
        }
        app.add_systems(Startup, (make_the_look, warm_up_the_flames).chain())
            .add_systems(
                Update,
                (light_the_jets.after(start_recoil), aim_the_jets)
                    .chain()
                    .run_if(crate::common::app_mode::simulating),
            )
            // Gated, so a client leaving Play lights nothing for bodies whose
            // despawns are still on the way.
            .add_systems(
                Update,
                (light_the_burning, follow_the_burning, light_the_burnt_corpses, follow_the_burnt_corpses)
                    .chain()
                    .run_if(crate::common::app_mode::simulating),
            )
            .add_systems(OnExit(AppMode::Play), put_out_the_jets)
            .add_systems(OnExit(AppMode::Weapon), put_out_the_jets);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if a flame in open air stops flying straight anywhere but three
    /// quarters down the cone, or stops drifting short of or past its end.
    #[test]
    fn a_flame_flies_straight_for_three_quarters_and_drifts_to_the_end() {
        let range = FlameSpec::FLAMETHROWER.range_m;
        let straight = straight_seconds(range, range) * SPEED;
        assert!((straight - range * 0.75).abs() < 1.0e-4, "straight for {straight} m");
        // Exponential braking from `SPEED` covers `SPEED / brake` meters.
        let drift = SPEED / brake(range);
        assert!((straight + drift - range).abs() < 1.0e-4, "comes to rest at {} m", straight + drift);
    }

    /// Fails if a flame aimed at a near wall flies straight into it rather than
    /// coming to rest at it.
    #[test]
    fn a_flame_at_a_wall_rises_short_of_it() {
        let range = FlameSpec::FLAMETHROWER.range_m;
        let wall = 3.0;
        let rests = straight_seconds(range, wall) * SPEED + SPEED / brake(range);
        assert!((rests - wall).abs() < 1.0e-4, "rests at {rests} m");
        assert_eq!(straight_seconds(range, 0.5), 0.0, "a wall at the nozzle");
    }

    /// Fails if a long-range flame outlives its emitter's capacity, or the
    /// afterburn starts at the tick's position rather than the drawn one.
    #[test]
    fn flames_fit_their_emitters_and_start_on_the_drawn_body() {
        let longest = straight_seconds(1000.0, 1000.0) + RISE_SECONDS.1;
        assert!(RATE * longest <= CAPACITY as f32, "{} flames, {CAPACITY} slots", RATE * longest);
        assert!(AFTERBURN_RATE * RISE_SECONDS.1 <= AFTERBURN_CAPACITY as f32);
        assert!(CORPSE_RATE * CORPSE_RISE_SECONDS.1 <= CORPSE_CAPACITY as f32);

        let boxes = Hitboxes {
            body: crate::common::hitbox::Box3 { center: Vec3::new(1.0, 0.9, 0.0), half_extents: Vec3::splat(0.4) },
            head: None,
        };
        let tick = PhysicsBody::at(Vec3::new(1.0, 0.9, 0.0));
        let drawn = Transform::from_xyz(0.9, 0.9, 0.0);
        assert_eq!(hull_middle(&drawn, &boxes, Some(&tick)), Vec3::new(0.9, 0.9, 0.0));
        assert_eq!(hull_middle(&drawn, &boxes, None), boxes.body.center);
    }

    /// Fails if a corpse's flames start off the middle of its bones, over half
    /// their spread, or with no room on a lone small piece.
    #[test]
    fn a_corpses_flames_start_round_its_bones() {
        let (center, half) = corpse_fire_box(Vec3::new(-0.9, 0.0, -0.1), Vec3::new(0.9, 0.2, 0.1));
        assert_eq!(center, Vec3::new(0.0, 0.1, 0.0));
        assert_eq!(half, Vec3::new(0.45, 0.05, 0.05));
        let (_, half) = corpse_fire_box(Vec3::ONE, Vec3::ONE);
        assert_eq!(half, Vec3::splat(CORPSE_LEAST_BOX));
    }

        /// Fails if a flame is not opaque and hot low in its middle, reaches the
    /// edge of its cell, is not hotter at the base than near the tip, or two
    /// flames are the same.
    #[test]
    fn every_flame_is_its_own_hot_at_the_base_and_stays_in_its_cell() {
        let n = FLAME_TEXELS as usize;
        let width = FLAME_GRID.x as usize * n;
        let rgba = flame_sheet(FLAME_TEXELS);
        assert_eq!(rgba.len(), width * FLAME_GRID.y as usize * n * 4);
        // Texels of one flame, `(x, y)` within its cell, as RGBA.
        let cell = |flame: usize| -> Vec<[u8; 4]> {
            let (column, row) = (flame % FLAME_GRID.x as usize, flame / FLAME_GRID.x as usize);
            (0..n * n)
                .map(|i| {
                    let at = ((row * n + i / n) * width + column * n + i % n) * 4;
                    [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
                })
                .collect()
        };
        let cells: Vec<Vec<[u8; 4]>> = (0..(FLAME_GRID.x * FLAME_GRID.y) as usize).map(cell).collect();
        let column_heat = |texels: &[[u8; 4]], x: usize| -> u32 {
            (0..n).map(|y| texels[y * n + x]).filter(|t| t[3] > 128).map(|t| t[1] as u32).max().unwrap_or(0)
        };
        for (flame, texels) in cells.iter().enumerate() {
            let low_middle = texels[n / 2 * n + n / 4];
            assert!(low_middle[3] == 255 && low_middle[1] > 150, "flame {flame} is thin or cold low in its middle: {low_middle:?}");
            let edge = (0..n).flat_map(|i| [i, (n - 1) * n + i, i * n, i * n + n - 1]);
            assert!(edge.into_iter().all(|i| texels[i][3] == 0), "flame {flame} reaches its edge");
            assert!(column_heat(texels, n / 4) > column_heat(texels, n * 3 / 4), "flame {flame} is as hot at the tip");
            let covered = texels.iter().filter(|t| t[3] > 0).count();
            let partial = texels.iter().filter(|t| t[3] > 0 && t[3] < 255).count();
            assert!(partial * 4 < covered, "flame {flame} is mostly blurred edge: {partial} of {covered}");
            for (other, theirs) in cells.iter().enumerate().skip(flame + 1) {
                assert_ne!(texels, theirs, "flames {flame} and {other} are the same");
            }
        }
    }

    /// Fails if the third-person nozzle ignores the body's turn or the weapon's
    /// place in its hands.
    #[test]
    fn the_nozzle_is_the_held_models_muzzle_in_the_world() {
        let body = Transform::from_xyz(1.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let held = HeldModel::with_muzzle(Vec3::new(0.0, 0.0, -0.5));
        let in_hand = WeaponInHand(Transform::from_xyz(0.2, 0.5, 0.0));
        let eye = Vec3::new(1.0, 0.7, 0.0);
        let at = nozzle(&body, eye, Some((&held, &in_hand)), false, None);
        // Turned a quarter left: -Z becomes -X, +X becomes -Z.
        assert!(at.abs_diff_eq(Vec3::new(0.5, 0.5, -0.2), 1.0e-5), "{at}");
        assert_eq!(nozzle(&body, eye, None, false, None), eye);
        assert_eq!(nozzle(&body, eye, Some((&held, &in_hand)), true, Some(Vec3::ONE)), Vec3::ONE);
    }
}

