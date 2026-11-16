//! An airblast's smoke ring: one `bevy_hanabi` burst per gust, puffs started on a small circle
//! round the nozzle and flown down the aim, spreading outward with the cone, so the ring widens
//! as it goes and comes to rest at the cone's end. Drawing only — the gust was resolved on the
//! tick by [`crate::game::airblast`], whose cone is drawn on `F6`.
//!
//! Lit by [`ShotSeen`], as the flames are, so our own ring leaves on the press and a remote
//! body's on the relayed `Effect::Fired`. Every machine makes its own; nothing is sent.
//!
//! A puff is a billboard cut from a generated sheet of lumpy shapes ([`puff_sheet`]).

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy_hanabi::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::weapon::{AirblastSpec, Equipped, WeaponAction, WeaponCatalog};
use crate::game::flame_jet::{effect_origin, noise, sheet_image, value_noise};
use crate::game::replay::HeldBursts;
use crate::game::held::{HeldModel, WeaponInHand};
use crate::game::hitscan::aim;
use crate::game::player::{LocalPlayer, Player, ViewMode, Watched};
use crate::game::recoil::{start_recoil, ShotSeen};
use crate::game::viewmodel::ApparentMuzzle;
use crate::game::warm_up::{self, WarmUp};

const PUFFS: u32 = 72;

/// Rings an emitter holds at once: a gust may come before the last one's puffs fade.
const RINGS_AT_ONCE: u32 = 2;

/// Meters per second down the aim as a ring leaves the nozzle.
const SPEED: f32 = 22.0;

/// Per puff, a multiplier on its speed, so the ring is ragged.
const SPEED_SHARE: (f32, f32) = (0.85, 1.12);

/// Meters: the circle the puffs start on, round the nozzle.
const START_RADIUS: f32 = 0.12;

/// Share of the cone's edge the ring reaches: the puffs' middles stay inside it, their size
/// reaching the edge.
const REACH: f32 = 0.85;

/// Seconds a puff lasts.
const LIFETIME: (f32, f32) = (0.45, 0.7);

/// Meters wide as it leaves, and meters it grows per second.
const START_SIZE: f32 = 0.18;
const GROWTH: f32 = 1.3;

/// Per puff, a multiplier on its size.
const SIZE_SHARE: (f32, f32) = (0.75, 1.25);

/// Meters per second squared upward: smoke drifts up as it slows.
const RISE: f32 = 0.8;

/// Radians per second a puff turns at most.
const SPIN: f32 = 2.5;

/// Linear, multiplied into the sheet's grays.
const COLOR: Vec3 = Vec3::new(0.92, 0.94, 0.98);

/// Alpha at its densest, and seconds it takes to get there.
const OPACITY: f32 = 0.7;
const FADE_IN: f32 = 0.04;

/// Seconds after a gust before its emitter goes: its last puff has faded by then, and a body
/// gusting more often than this keeps one emitter.
const LAST_PUFF_GONE: f32 = LIFETIME.1 + 1.0;

const PUFF_GRID: UVec2 = UVec2::new(4, 2);
const PUFF_TEXELS: u32 = 64;

#[derive(Resource)]
struct RingLook {
    effect: Handle<EffectAsset>,
    sheet: Handle<Image>,
}

/// `body`'s emitter, reset for each gust; `idle` is seconds since the last.
#[derive(Component)]
#[require(crate::game::replay::Unreplayed)]
struct GustRing {
    body: Entity,
    idle: f32,
}

/// Per second, exponential: what brings a puff at [`SPEED`] to rest at `range`.
fn brake(range: f32) -> f32 {
    SPEED / range.max(0.1)
}

fn across(toward: Dir3) -> (Vec3, Vec3) {
    let right = toward.any_orthonormal_vector();
    (right, toward.cross(right))
}

fn ring_effect() -> EffectAsset {
    let writer = ExprWriter::new();
    let direction = writer.add_property("direction", Vec3::NEG_Z.into());
    let right = writer.add_property("right", Vec3::X.into());
    let up = writer.add_property("up", Vec3::Y.into());
    // tan of the cone's half-angle, times REACH.
    let widen = writer.add_property("widen", 0.5.into());
    let braking = writer.add_property("brake", brake(5.0).into());
    let carry = writer.add_property("carry", Vec3::ZERO.into());

    let age = || writer.attr(Attribute::AGE);
    // Round the ring, 0 to 1.
    let around = || writer.attr(Attribute::F32_0);
    // 0 to 1, per puff: its size, its turn and which shape it is.
    let seed = || writer.attr(Attribute::F32_1);
    let speed_share = || writer.attr(Attribute::F32_2);

    let init_around = SetAttributeModifier::new(Attribute::F32_0, writer.rand(ScalarType::Float).expr());
    let init_seed = SetAttributeModifier::new(Attribute::F32_1, writer.rand(ScalarType::Float).expr());
    let init_speed_share =
        SetAttributeModifier::new(Attribute::F32_2, writer.lit(SPEED_SHARE.0).uniform(writer.lit(SPEED_SHARE.1)).expr());
    let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.0).expr());
    let init_lifetime = SetAttributeModifier::new(Attribute::LIFETIME, writer.lit(LIFETIME.0).uniform(writer.lit(LIFETIME.1)).expr());

    let angle = || around() * writer.lit(std::f32::consts::TAU);
    let outward = || writer.prop(right) * angle().cos() + writer.prop(up) * angle().sin();
    let init_position = SetAttributeModifier::new(Attribute::POSITION, (outward() * writer.lit(START_RADIUS)).expr());
    let forward = writer.prop(direction) + outward() * writer.prop(widen);
    let velocity = forward * writer.lit(SPEED) * speed_share() + writer.prop(carry);
    let init_velocity = SetAttributeModifier::new(Attribute::VELOCITY, velocity.expr());

    // Not `LinearDragModifier`, whose `v *= 1 - k * dt` depends on frame rate.
    let braked = writer.attr(Attribute::VELOCITY) * (writer.lit(-1.0) * writer.prop(braking) * writer.delta_time()).exp();
    let lifted = writer.lit(Vec3::Y * RISE) * writer.delta_time();
    let update_velocity = SetAttributeModifier::new(Attribute::VELOCITY, (braked + lifted).expr());

    let size_share = writer.lit(SIZE_SHARE.0) + seed() * writer.lit(SIZE_SHARE.1 - SIZE_SHARE.0);
    let width = (writer.lit(START_SIZE) + writer.lit(GROWTH) * age()) * size_share;
    let update_size = SetAttributeModifier::new(Attribute::SIZE, width.expr());

    let life = (age() / writer.attr(Attribute::LIFETIME)).saturate();
    let fade_in = (age() / writer.lit(FADE_IN)).saturate();
    let fade_out = writer.lit(1.0) - life.clone() * life;
    let alpha = fade_in * fade_out * writer.lit(OPACITY);
    let update_color = SetAttributeModifier::new(Attribute::HDR_COLOR, writer.lit(COLOR).vec4_xyz_w(alpha).expr());

    let shapes = (PUFF_GRID.x * PUFF_GRID.y) as f32;
    let init_shape = SetAttributeModifier::new(
        Attribute::SPRITE_INDEX,
        (seed() * writer.lit(shapes - 0.001)).floor().cast(ScalarType::Int).expr(),
    );

    let turn = seed() * writer.lit(std::f32::consts::TAU) + (seed() * writer.lit(2.0) - writer.lit(1.0)) * writer.lit(SPIN) * age();
    let turn = turn.expr();

    let mut module = writer.finish();
    module.add_texture_slot("puffs", SlotDimension::D2);

    EffectAsset::new(PUFFS * RINGS_AT_ONCE, SpawnerSettings::once((PUFFS as f32).into()), module)
        .with_name("gust_ring")
        .with_alpha_mode(bevy_hanabi::AlphaMode::Blend)
        .init(init_around)
        .init(init_seed)
        .init(init_speed_share)
        .init(init_age)
        .init(init_lifetime)
        .init(init_position)
        .init(init_velocity)
        .init(init_shape)
        .update(update_velocity)
        .update(update_size)
        .update(update_color)
        .render(OrientModifier::new(OrientMode::FaceCameraPosition).with_rotation(turn))
        .render(FlipbookModifier { sprite_grid_size: PUFF_GRID })
        .render(ParticleTextureModifier { texture_slot: 0, sample_mapping: ImageSampleMapping::Modulate })
}

/// [`PUFF_GRID`] puffs of `texels` square, as linear RGBA, rows top to bottom.
pub fn puff_sheet(texels: u32) -> Vec<u8> {
    let (width, height) = (PUFF_GRID.x * texels, PUFF_GRID.y * texels);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let texel = 1.0 / texels as f32;
    for puff in 0..PUFF_GRID.x * PUFF_GRID.y {
        let seed = puff.wrapping_mul(0x2F6B_4C1D) + 11;
        let n = |k: u32| noise(seed.wrapping_add(k));
        // Blobs as (center, radius), in -1 to 1 across the cell.
        let mut blobs = vec![(Vec2::ZERO, 0.5 + 0.08 * n(1))];
        for lump in 0..4 + (n(2) * 2.99) as u32 {
            let k = 10 + lump * 4;
            let angle = n(k) * std::f32::consts::TAU;
            let away = 0.25 + 0.15 * n(k + 1);
            blobs.push((Vec2::from_angle(angle) * away, 0.24 + 0.14 * n(k + 2)));
        }
        let (column, row) = (puff % PUFF_GRID.x, puff / PUFF_GRID.x);
        for y in 0..texels {
            for x in 0..texels {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * texel * 2.0 - Vec2::ONE;
                // 1 at a blob's middle, 0 at its edge, below 0 outside.
                let density = blobs.iter().map(|(center, radius)| 1.0 - p.distance(*center) / radius).fold(-1.0, f32::max);
                let uv = (p + Vec2::ONE) * 0.5;
                let fray = 0.6 * value_noise(uv, 6.0, seed) + 0.4 * value_noise(uv, 13.0, seed ^ 0x77AA) - 0.5;
                let density = density + fray * 0.35;
                // About three texels of edge.
                let edge = (density / (6.0 * texel)).clamp(0.0, 1.0);
                let body = 0.55 + 0.45 * value_noise(uv, 4.0, seed ^ 0x1234);
                let alpha = edge * body;
                // Lit from the top left, so a puff reads as round.
                let shade = 0.72 + 0.28 * (0.5 - 0.35 * (p.x + p.y)).clamp(0.0, 1.0) + 0.1 * (value_noise(uv, 9.0, seed ^ 0xBEEF) - 0.5);
                let index = (((row * texels + y) * width + column * texels + x) * 4) as usize;
                let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                rgba[index..index + 4].copy_from_slice(&[byte(shade), byte(shade), byte(shade), byte(alpha)]);
            }
        }
    }
    rgba
}

fn make_the_look(mut commands: Commands, mut effects: ResMut<Assets<EffectAsset>>, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(RingLook {
        effect: effects.add(ring_effect()),
        sheet: images.add(sheet_image(puff_sheet(PUFF_TEXELS), PUFF_GRID.x * PUFF_TEXELS, PUFF_GRID.y * PUFF_TEXELS)),
    });
}

fn ring_properties(spec: &AirblastSpec, toward: Dir3, carry: Vec3) -> EffectProperties {
    let (right, up) = across(toward);
    let mut properties = EffectProperties::default();
    properties.set("direction", Vec3::from(toward).into());
    properties.set("right", right.into());
    properties.set("up", up.into());
    properties.set("widen", (spec.spread.tan() * REACH).into());
    properties.set("brake", brake(spec.range).into());
    properties.set("carry", carry.into());
    properties
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn blow_rings(
    mut commands: Commands,
    look: Res<RingLook>,
    catalog: Res<WeaponCatalog>,
    view: Option<Res<ViewMode>>,
    apparent: Option<Res<ApparentMuzzle>>,
    time: Res<Time>,
    particles: Res<Time<EffectSimulation>>,
    mut waiting: Local<HeldBursts<(Entity, Vec3, EffectProperties)>>,
    mut seen: MessageReader<ShotSeen>,
    bodies: Query<(&Transform, &Player, &Stance, &Equipped, Option<(&HeldModel, &WeaponInHand)>, Has<LocalPlayer>, Has<Watched>)>,
    mut rings: Query<(&mut GustRing, &mut Transform, &mut EffectProperties, Option<&mut EffectSpawner>), Without<Player>>,
) {
    for shot in seen.read() {
        let Ok((body, player, stance, equipped, held, ours, watched)) = bodies.get(shot.body) else { continue };
        let Some(weapon) = catalog.held_by(equipped) else { continue };
        let WeaponAction::Airblast(spec) = shot.button.of(weapon).action else { continue };
        let from = effect_origin(body, player, *stance, held, ours || watched, view.as_deref(), apparent.as_deref());
        waiting.hold(time.elapsed_secs_f64(), (shot.body, from, ring_properties(&spec, aim(player), player.velocity)));
    }
    for (body, from, properties) in waiting.due(particles.elapsed_secs_f64()) {
        // The simulation is in world space, so moving the emitter leaves the last ring's puffs
        // where they are.
        match rings.iter_mut().find(|(ring, ..)| ring.body == body) {
            Some((mut ring, mut transform, mut current, spawner)) => {
                ring.idle = 0.0;
                transform.translation = from;
                *current = properties;
                // Added by Hanabi in `PostUpdate`; until then the emitter has not fired its first.
                if let Some(mut spawner) = spawner {
                    spawner.reset();
                }
            }
            None => {
                commands.spawn((
                    GustRing { body, idle: 0.0 },
                    ParticleEffect::new(look.effect.clone()),
                    EffectMaterial { images: vec![look.sheet.clone()] },
                    properties,
                    Transform::from_translation(from),
                    Name::new("Gust ring"),
                ));
            }
        }
    }
}

fn end_the_rings(mut commands: Commands, time: Res<Time<EffectSimulation>>, mut rings: Query<(Entity, &mut GustRing)>) {
    for (entity, mut ring) in &mut rings {
        ring.idle += time.delta_secs();
        if ring.idle >= LAST_PUFF_GONE {
            commands.entity(entity).despawn();
        }
    }
}

fn clear_the_rings(mut commands: Commands, rings: Query<Entity, With<GustRing>>) {
    for ring in &rings {
        commands.entity(ring).despawn();
    }
}

/// See [`crate::game::warm_up`].
fn warm_up_the_ring(mut commands: Commands, look: Res<RingLook>) {
    commands.spawn((
        WarmUp,
        ParticleEffect::new(look.effect.clone()),
        EffectMaterial { images: vec![look.sheet.clone()] },
        EffectProperties::default(),
        Transform::from_translation(Vec3::NEG_Z),
        RenderLayers::layer(crate::common::render_layers::WARM_UP),
        Name::new("Gust ring warm-up"),
    ));
}

/// Added in `main.rs` beside `FlameJetPlugin`: Hanabi needs a renderer, which the game's tests do
/// not have.
pub struct GustRingPlugin;

impl Plugin for GustRingPlugin {
    fn build(&self, app: &mut App) {
        warm_up::ensure(app);
        if !app.is_plugin_added::<HanabiPlugin>() {
            app.add_plugins(HanabiPlugin);
        }
        app.add_systems(Startup, (make_the_look, warm_up_the_ring).chain())
            .add_systems(Update, (end_the_rings, blow_rings.after(start_recoil)).chain().run_if(crate::common::app_mode::simulating))
            .add_systems(OnExit(AppMode::Play), clear_the_rings)
            .add_systems(OnExit(AppMode::Weapon), clear_the_rings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if a ring stops short of or flies past the cone's end, or its puffs outlive the
    /// emitter.
    #[test]
    fn a_ring_comes_to_rest_at_the_cones_end() {
        let range = 5.0;
        // Exponential braking from `SPEED` covers `SPEED / brake` meters.
        assert!((SPEED / brake(range) - range).abs() < 1.0e-4);
        assert!(LAST_PUFF_GONE > LIFETIME.1);
    }

    /// Fails if the ring's circle is not square to the aim.
    #[test]
    fn the_ring_is_square_to_the_aim() {
        for toward in [Dir3::NEG_Z, Dir3::Y, Dir3::new(Vec3::new(0.3, -0.8, 0.5)).unwrap()] {
            let (right, up) = across(toward);
            assert!(right.dot(*toward).abs() < 1.0e-5 && up.dot(*toward).abs() < 1.0e-5 && right.dot(up).abs() < 1.0e-5);
            assert!((right.length() - 1.0).abs() < 1.0e-5 && (up.length() - 1.0).abs() < 1.0e-5);
        }
    }

    /// Fails if a puff is faint in its middle, reaches its cell's edge, is mostly blurred edge,
    /// or two puffs are the same.
    #[test]
    fn every_puff_is_its_own_solid_and_stays_in_its_cell() {
        let n = PUFF_TEXELS as usize;
        let width = PUFF_GRID.x as usize * n;
        let rgba = puff_sheet(PUFF_TEXELS);
        let cell = |puff: usize| -> Vec<[u8; 4]> {
            let (column, row) = (puff % PUFF_GRID.x as usize, puff / PUFF_GRID.x as usize);
            (0..n * n)
                .map(|i| {
                    let at = ((row * n + i / n) * width + column * n + i % n) * 4;
                    [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
                })
                .collect()
        };
        let cells: Vec<Vec<[u8; 4]>> = (0..(PUFF_GRID.x * PUFF_GRID.y) as usize).map(cell).collect();
        for (puff, texels) in cells.iter().enumerate() {
            let middle = texels[n / 2 * n + n / 2];
            assert!(middle[3] > 120, "puff {puff} is faint in its middle: {middle:?}");
            let edge = (0..n).flat_map(|i| [i, (n - 1) * n + i, i * n, i * n + n - 1]);
            assert!(edge.into_iter().all(|i| texels[i][3] == 0), "puff {puff} reaches its edge");
            let covered = texels.iter().filter(|t| t[3] > 0).count();
            let rim = (0..n * n)
                .filter(|&i| texels[i][3] > 0)
                .filter(|&i| {
                    let (x, y) = (i % n, i / n);
                    [(x.wrapping_sub(3), y), (x + 3, y), (x, y.wrapping_sub(3)), (x, y + 3)]
                        .into_iter()
                        .any(|(x, y)| x >= n || y >= n || texels[y * n + x][3] == 0)
                })
                .count();
            assert!(rim * 2 < covered, "puff {puff} is mostly rim: {rim} of {covered}");
            for (other, theirs) in cells.iter().enumerate().skip(puff + 1) {
                assert_ne!(texels, theirs, "puffs {puff} and {other} are the same");
            }
        }
    }
}
