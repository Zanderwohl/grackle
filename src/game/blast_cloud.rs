//! A blast's fireball: one `bevy_hanabi` burst per [`Effect::Blast`], clouds started in a small
//! sphere at the blast and flown outward in every direction, braking to rest inside the damage
//! radius as they burn from yellow-orange through red and black to a white that fades out.
//! Drawing only — the blast was resolved on the tick by [`crate::game::explosion`], whose sphere
//! is drawn on `F6`.
//!
//! Every machine makes its own from the `Effect` it already gets; nothing is sent.
//!
//! A cloud is a billboard cut from a sheet of the pack's `particles/svg/cloud_{n}.svg`, white
//! so the gradient's color is the color drawn. A pack without them draws the airblast's puffs.

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy_hanabi::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::effects::Effect;
use crate::common::icons::{demultiplied, render_svg};
use crate::game::flame_jet::{sheet_image, sheet_image_as};
use crate::game::gust_ring::puff_sheet;
use crate::game::replay::HeldBursts;
use crate::game::warm_up::{self, WarmUp};

const CLOUDS: u32 = 40;

/// Seconds a cloud lasts.
const LIFETIME: (f32, f32) = (0.22, 0.28);

/// Share of the damage radius the furthest cloud's middle comes to rest at: with the largest
/// cloud's half-width at its oldest, the smoke stays inside the radius.
const REACH: f32 = 0.6;

/// Share of the reach the clouds start inside.
const START_SHARE: f32 = 0.12;

/// Per second, exponential. Over [`LIFETIME`] it covers about 95% of a cloud's travel, so the
/// clouds are nearly still as they fade.
const BRAKE: f32 = 12.0;

/// Per cloud, a multiplier on its speed, so clouds come to rest throughout the sphere rather
/// than on a shell.
const SPEED_SHARE: (f32, f32) = (0.35, 1.0);

/// Width as it starts, and width it grows per second of age, as shares of the damage radius.
const START_SIZE: f32 = 0.22;
const GROWTH: f32 = 1.4;

const SIZE_SHARE: (f32, f32) = (0.75, 1.25);

/// Radians per second a cloud turns at most.
const SPIN: f32 = 1.5;

/// Linear RGB and alpha by share of a cloud's life. Above 1 glows.
const BURN: [(f32, Vec4); 7] = [
    (0.0, Vec4::new(5.0, 3.2, 0.8, 1.0)),
    (0.22, Vec4::new(3.6, 1.2, 0.15, 1.0)),
    (0.44, Vec4::new(1.4, 0.12, 0.03, 1.0)),
    (0.6, Vec4::new(0.3, 0.025, 0.015, 0.95)),
    (0.72, Vec4::new(0.025, 0.022, 0.02, 0.85)),
    (0.87, Vec4::new(0.3, 0.3, 0.3, 0.45)),
    (1.0, Vec4::new(0.9, 0.9, 0.9, 0.0)),
];

/// Seconds after a blast until its last cloud has faded and its emitter may be reused.
const LAST_CLOUD_GONE: f32 = LIFETIME.1 + 0.5;

/// Seconds an emitter waits unused for another blast before it goes.
const KEPT_IDLE: f32 = 5.0;

const CLOUD_GRID: UVec2 = UVec2::new(4, 2);
const CLOUD_TEXELS: u32 = 128;

#[derive(Resource)]
struct CloudLook {
    effect: Handle<EffectAsset>,
    sheet: Handle<Image>,
}

/// An emitter, reset for each blast it is given; `idle` is seconds since the last.
#[derive(Component)]
#[require(crate::game::replay::Unreplayed)]
struct BlastCloud {
    idle: f32,
}

fn cloud_effect() -> EffectAsset {
    let writer = ExprWriter::new();
    let radius = writer.add_property("radius", 3.0.into());

    let age = || writer.attr(Attribute::AGE);
    // 0 to 1, per cloud, each its own roll so shape, size and turn do not go together.
    let shape = || writer.attr(Attribute::F32_0);
    let size = || writer.attr(Attribute::F32_1);
    let spin = || writer.attr(Attribute::F32_2);

    let init_shape_seed = SetAttributeModifier::new(Attribute::F32_0, writer.rand(ScalarType::Float).expr());
    let init_size_seed = SetAttributeModifier::new(Attribute::F32_1, writer.rand(ScalarType::Float).expr());
    let init_spin_seed = SetAttributeModifier::new(Attribute::F32_2, writer.rand(ScalarType::Float).expr());
    let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.0).expr());
    let init_lifetime = SetAttributeModifier::new(Attribute::LIFETIME, writer.lit(LIFETIME.0).uniform(writer.lit(LIFETIME.1)).expr());

    let reach = || writer.prop(radius) * writer.lit(REACH);
    let init_position = SetPositionSphereModifier {
        center: writer.lit(Vec3::ZERO).expr(),
        radius: (reach() * writer.lit(START_SHARE)).expr(),
        dimension: ShapeDimension::Volume,
    };
    // A cloud at full share comes to rest at the reach: exponential braking from `v` covers
    // `v / BRAKE`.
    let speed = reach() * writer.lit(BRAKE) * writer.lit(SPEED_SHARE.0).uniform(writer.lit(SPEED_SHARE.1));
    let init_velocity = SetVelocitySphereModifier { center: writer.lit(Vec3::ZERO).expr(), speed: speed.expr() };

    // Not `LinearDragModifier`, whose `v *= 1 - k * dt` depends on frame rate.
    let braked = writer.attr(Attribute::VELOCITY) * (writer.lit(-BRAKE) * writer.delta_time()).exp();
    let update_velocity = SetAttributeModifier::new(Attribute::VELOCITY, braked.expr());

    let size_share = writer.lit(SIZE_SHARE.0) + size() * writer.lit(SIZE_SHARE.1 - SIZE_SHARE.0);
    let width = (writer.lit(START_SIZE) + writer.lit(GROWTH) * age()) * writer.prop(radius) * size_share;
    let update_size = SetAttributeModifier::new(Attribute::SIZE, width.expr());

    let shapes = (CLOUD_GRID.x * CLOUD_GRID.y) as f32;
    let init_shape = SetAttributeModifier::new(
        Attribute::SPRITE_INDEX,
        (shape() * writer.lit(shapes - 0.001)).floor().cast(ScalarType::Int).expr(),
    );

    let turn = shape() * writer.lit(std::f32::consts::TAU * 7.0) + (spin() * writer.lit(2.0) - writer.lit(1.0)) * writer.lit(SPIN) * age();
    let turn = turn.expr();

    let mut module = writer.finish();
    module.add_texture_slot("clouds", SlotDimension::D2);

    let mut burn = bevy_hanabi::Gradient::new();
    for (at, color) in BURN {
        burn.add_key(at, color);
    }

    EffectAsset::new(CLOUDS, SpawnerSettings::once((CLOUDS as f32).into()), module)
        .with_name("blast_cloud")
        .with_alpha_mode(bevy_hanabi::AlphaMode::Blend)
        .init(init_shape_seed)
        .init(init_size_seed)
        .init(init_spin_seed)
        .init(init_age)
        .init(init_lifetime)
        .init(init_position)
        .init(init_velocity)
        .init(init_shape)
        .update(update_velocity)
        .update(update_size)
        .render(ColorOverLifetimeModifier::new(burn))
        .render(OrientModifier::new(OrientMode::FaceCameraPosition).with_rotation(turn))
        .render(FlipbookModifier { sprite_grid_size: CLOUD_GRID })
        .render(ParticleTextureModifier { texture_slot: 0, sample_mapping: ImageSampleMapping::Modulate })
}

fn cloud_path(n: u32) -> String {
    format!("particles/svg/cloud_{n}.svg")
}

/// The [`CLOUD_GRID`] clouds rasterized at `texels` square, as sRGB RGBA with straight alpha,
/// rows top to bottom; `None` if any is missing or does not parse.
fn cloud_sheet(texels: u32, read: impl Fn(&str) -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    let width = CLOUD_GRID.x * texels;
    let mut rgba = vec![0u8; (width * CLOUD_GRID.y * texels * 4) as usize];
    for cloud in 0..CLOUD_GRID.x * CLOUD_GRID.y {
        let path = cloud_path(cloud + 1);
        let pixmap = render_svg(&read(&path)?, texels).inspect_err(|e| warn!("{path}: {e}")).ok()?;
        let (column, row) = (cloud % CLOUD_GRID.x, cloud / CLOUD_GRID.x);
        for (i, pixel) in demultiplied(&pixmap).enumerate() {
            let (x, y) = (i as u32 % texels, i as u32 / texels);
            let at = (((row * texels + y) * width + column * texels + x) * 4) as usize;
            rgba[at..at + 4].copy_from_slice(&pixel);
        }
    }
    Some(rgba)
}

fn make_the_look(
    mut commands: Commands,
    pack: Option<Res<crate::common::assets::Assets>>,
    mut effects: ResMut<Assets<EffectAsset>>,
    mut images: ResMut<Assets<Image>>,
) {
    let (width, height) = (CLOUD_GRID.x * CLOUD_TEXELS, CLOUD_GRID.y * CLOUD_TEXELS);
    let read = |relative: &str| pack.as_ref().and_then(|pack| pack.read_from_packs(relative));
    let sheet = match cloud_sheet(CLOUD_TEXELS, read) {
        Some(rgba) => sheet_image_as(rgba, width, height, TextureFormat::Rgba8UnormSrgb),
        None => {
            warn!("blast clouds: drawing the airblast's puffs, since particles/svg/cloud_*.svg did not all load");
            // The puff sheet has the same grid, at its own size.
            let texels = 64;
            sheet_image(puff_sheet(texels), CLOUD_GRID.x * texels, CLOUD_GRID.y * texels)
        }
    };
    commands.insert_resource(CloudLook { effect: effects.add(cloud_effect()), sheet: images.add(sheet) });
}

fn cloud_properties(radius: f32) -> EffectProperties {
    let mut properties = EffectProperties::default();
    properties.set("radius", radius.into());
    properties
}

/// Reads [`Effect`] rather than `Explosion`, which exists only on the authority. Drains it in
/// every mode, so a blast from before the round is not drawn when the next one starts.
#[allow(clippy::too_many_arguments)]
fn burst(
    mut commands: Commands,
    mode: Res<State<AppMode>>,
    time: Res<Time>,
    particles: Res<Time<EffectSimulation>>,
    look: Res<CloudLook>,
    mut effects: MessageReader<Effect>,
    mut held: Local<HeldBursts<(Vec3, f32)>>,
    mut clouds: Query<(&mut BlastCloud, &mut Transform, &mut EffectProperties, Option<&mut EffectSpawner>)>,
) {
    let simulating = mode.get().simulates();
    for effect in effects.read() {
        if let Effect::Blast { at, radius } = *effect
            && simulating
        {
            held.hold(time.elapsed_secs_f64(), (at, radius));
        }
    }
    if !simulating {
        held.clear();
        return;
    }
    for (at, radius) in held.due(particles.elapsed_secs_f64()) {
        // Only a faded emitter is reused, so a new radius changes no cloud still drawn. Hanabi
        // adds the spawner once an emitter has fired.
        let free = clouds.iter_mut().find(|(cloud, .., spawner)| cloud.idle >= LAST_CLOUD_GONE && spawner.is_some());
        match free {
            Some((mut cloud, mut transform, mut properties, Some(mut spawner))) => {
                cloud.idle = 0.0;
                transform.translation = at;
                *properties = cloud_properties(radius);
                spawner.reset();
            }
            _ => {
                commands.spawn((
                    BlastCloud { idle: 0.0 },
                    ParticleEffect::new(look.effect.clone()),
                    EffectMaterial { images: vec![look.sheet.clone()] },
                    cloud_properties(radius),
                    Transform::from_translation(at),
                    Name::new("Blast cloud"),
                ));
            }
        }
    }
}

fn end_the_clouds(mut commands: Commands, time: Res<Time<EffectSimulation>>, mut clouds: Query<(Entity, &mut BlastCloud)>) {
    for (entity, mut cloud) in &mut clouds {
        cloud.idle += time.delta_secs();
        if cloud.idle >= LAST_CLOUD_GONE + KEPT_IDLE {
            commands.entity(entity).despawn();
        }
    }
}

fn clear_the_clouds(mut commands: Commands, clouds: Query<Entity, With<BlastCloud>>) {
    for cloud in &clouds {
        commands.entity(cloud).despawn();
    }
}

/// See [`crate::game::warm_up`].
fn warm_up_the_cloud(mut commands: Commands, look: Res<CloudLook>) {
    commands.spawn((
        WarmUp,
        ParticleEffect::new(look.effect.clone()),
        EffectMaterial { images: vec![look.sheet.clone()] },
        cloud_properties(1.0),
        Transform::from_translation(Vec3::NEG_Z * 4.0),
        RenderLayers::layer(crate::common::render_layers::WARM_UP),
        Name::new("Blast cloud warm-up"),
    ));
}

/// Added in `main.rs` beside `GustRingPlugin`: Hanabi needs a renderer, which the game's tests do
/// not have.
pub struct BlastCloudPlugin;

impl Plugin for BlastCloudPlugin {
    fn build(&self, app: &mut App) {
        warm_up::ensure(app);
        if !app.is_plugin_added::<HanabiPlugin>() {
            app.add_plugins(HanabiPlugin);
        }
        app.add_systems(Startup, (make_the_look, warm_up_the_cloud).chain())
            .add_systems(Update, (end_the_clouds.run_if(crate::common::app_mode::simulating), burst).chain())
            .add_systems(OnExit(AppMode::Play), clear_the_clouds)
            .add_systems(OnExit(AppMode::Weapon), clear_the_clouds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped(relative: &str) -> Option<Vec<u8>> {
        std::fs::read(format!("assets/default/{relative}")).ok()
    }

    /// Fails if a cloud's emitter ages on `Time` rather than the particles' clock.
    #[test]
    fn a_cloud_is_kept_while_the_particles_clock_is_stopped() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(100)))
            // Nothing advances it, as while the kill cam holds.
            .init_resource::<Time<EffectSimulation>>()
            .add_systems(Update, end_the_clouds);
        let cloud = app.world_mut().spawn(BlastCloud { idle: 0.0 }).id();
        for _ in 0..100 {
            app.update();
        }
        assert!(app.world().get_entity(cloud).is_ok(), "a cloud was taken away while its particles were frozen");
    }

    /// Fails if the clouds stop well short of the reach, or still drift fast as they fade.
    #[test]
    fn clouds_come_to_rest_inside_the_blast() {
        let traveled = 1.0 - (-BRAKE * LIFETIME.0).exp();
        assert!(traveled > 0.9, "a cloud covers only {traveled} of its travel");
        let widest = (START_SIZE + GROWTH * LIFETIME.1) * SIZE_SHARE.1;
        assert!(REACH + widest / 2.0 <= 1.0, "the smoke reaches {} of the radius", REACH + widest / 2.0);
        assert!(LAST_CLOUD_GONE > LIFETIME.1);
    }

    /// Fails if the burn's keys are out of order, it does not end fully faded, or it starts dim.
    #[test]
    fn the_burn_runs_from_glowing_to_gone() {
        assert_eq!(BURN.first().unwrap().0, 0.0);
        assert_eq!(BURN.last().unwrap().0, 1.0);
        assert!(BURN.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(BURN.last().unwrap().1.w, 0.0);
        assert!(BURN[0].1.x > 1.0, "the fireball's start does not glow");
    }

    /// Fails if a shipped cloud is missing, faint in its middle, reaches its cell's edge, or two
    /// are the same.
    #[test]
    fn every_shipped_cloud_is_solid_and_stays_in_its_cell() {
        let n = 64usize;
        let rgba = cloud_sheet(n as u32, shipped).expect("every cloud SVG loads");
        let width = CLOUD_GRID.x as usize * n;
        let cell = |cloud: usize| -> Vec<[u8; 4]> {
            let (column, row) = (cloud % CLOUD_GRID.x as usize, cloud / CLOUD_GRID.x as usize);
            (0..n * n)
                .map(|i| {
                    let at = ((row * n + i / n) * width + column * n + i % n) * 4;
                    [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
                })
                .collect()
        };
        let cells: Vec<_> = (0..(CLOUD_GRID.x * CLOUD_GRID.y) as usize).map(cell).collect();
        for (cloud, texels) in cells.iter().enumerate() {
            let middle = texels[n / 2 * n + n / 2];
            assert!(middle[3] > 200, "cloud {} is faint in its middle: {middle:?}", cloud + 1);
            let edge = (0..n).flat_map(|i| [i, (n - 1) * n + i, i * n, i * n + n - 1]);
            assert!(edge.into_iter().all(|i| texels[i][3] < 8), "cloud {} reaches its edge", cloud + 1);
            for (other, theirs) in cells.iter().enumerate().skip(cloud + 1) {
                assert_ne!(texels, theirs, "clouds {} and {} are the same", cloud + 1, other + 1);
            }
        }
    }
}
