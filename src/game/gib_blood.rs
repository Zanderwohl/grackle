//! Blood particles, in three `bevy_hanabi` effects:
//!
//! - a trail from each severed end of a gib ([`Ragdoll::severed_ends`]) while
//!   it flies faster than [`BLEED_SPEED`];
//! - a burst of [`goo_splat`]s from where a blast took bones
//!   ([`Ragdoll::torn_at`]);
//! - a spurt of [`blob_sheet`] lumps out of a hitscan wound, along the ray.
//!
//! Drops are slowed by [`DRAG`], collide with nothing, and fade linearly with
//! speed to nothing at [`STILL_SPEED`]; a spurt's drops fall, so they fade with
//! age instead. Hanabi simulates in compute shaders, so a browser needs WebGPU,
//! as texture-graph's previews already do. Every machine makes its own; nothing
//! is sent.

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy_hanabi::prelude::*;

use crate::common::damage::{DamageDealt, How};
use crate::common::movement_rules::MovementRules;
use crate::common::skeleton::Skeleton;
use crate::game::decal::HitBodies;
use crate::game::ragdoll::Ragdoll;
use crate::game::warm_up::{warm_up_camera, WarmUp, WarmUpPlugin};

/// Meters per second a severed end must move for it to bleed.
pub const BLEED_SPEED: f32 = 5.0;

/// Meters per second at which a drop is transparent and removed.
pub const STILL_SPEED: f32 = 0.1;

/// Per second, exponential: a drop at 10 m/s stops about 1.25 m on, 0.6 s later.
pub const DRAG: f32 = 8.0;

/// Drops per second from each bleeding end.
const RATE: f32 = 120.0;

/// Meters per second of random spread per axis, so a trail is a spray rather
/// than a line.
const JITTER: f32 = 0.5;

/// Diameter range, in meters.
const DROP_SIZE: (f32, f32) = (0.03, 0.1);

/// Seconds.
pub const BURST_SECONDS: f32 = 0.125;

const BURST_RATE: f32 = 480.0;

/// Meters: drops start anywhere in a ball this big.
const BURST_RADIUS: f32 = 0.25;

/// Meters per second.
const BURST_SPEED: (f32, f32) = (2.0, 8.0);

const BURST_DROP_SIZE: (f32, f32) = (DROP_SIZE.0 * 2.0, DROP_SIZE.1 * 2.0);

/// Seconds after an emitter stops before it goes: its fastest last drop has
/// faded by then.
const LAST_DROP_GONE: f32 = 1.0;

/// Seconds.
pub const SPURT_SECONDS: f32 = 1.0 / 16.0;

/// Meters per second along the ray, scaled per drop by [`SPURT_SPEED_SHARE`].
pub const SPURT_SPEED: f32 = 14.0;

const SPURT_SPEED_SHARE: (f32, f32) = (0.65, 1.35);

/// Meters per second, per axis.
const SPURT_JITTER: f32 = 2.5;

const SPURT_DROP_SIZE: (f32, f32) = (DROP_SIZE.0 * 2.0, DROP_SIZE.1 * 2.0);

/// With [`SPURT_PERIOD`], 240 drops a second on average.
const SPURT_CLUMP: (f32, f32) = (1.0, 7.0);

/// Seconds between clumps, 1/60 on average.
const SPURT_PERIOD: (f32, f32) = (0.004, 2.0 / 60.0 - 0.004);

/// Live drops per emitter: `RATE` times the longest life, from the fastest gib
/// worth planning for (about 60 m/s, so 0.8 s), with room to spare.
const CAPACITY: u32 = 128;

const SPLAT_TEXELS: u32 = 128;

const BLOB_GRID: UVec2 = UVec2::new(4, 3);

const BLOB_TEXELS: u32 = 32;

#[derive(Resource)]
struct BloodEffects {
    trail: Handle<EffectAsset>,
    burst: Handle<EffectAsset>,
    spurt: Handle<EffectAsset>,
    splat: Handle<Image>,
    blobs: Handle<Image>,
}

enum Look {
    Disc,
    /// Turned at random and growing to twice its size.
    Goo,
    /// Turned and picked at random.
    Blob,
}

/// A burst's or a spurt's emitter. Seconds.
#[derive(Component)]
struct Burst {
    age: f32,
    seconds: f32,
}

/// Added to each drop's starting velocity, in meters per second.
enum Spread {
    /// Each axis uniform within plus or minus this.
    Cube(f32),
    /// Every way, at a speed uniform in this range.
    Sphere(f32, f32),
}

/// The `end`-th of [`Ragdoll::severed_ends`]. `idle` is seconds since it last
/// emitted.
#[derive(Component)]
struct BleedsFrom {
    gib: Entity,
    end: usize,
    idle: f32,
}

#[derive(Component)]
struct Bleeding;

/// `radius` is in meters. A drop that `falls` never slows below `gravity / DRAG`,
/// so it fades with age over the life it would have had without gravity.
#[allow(clippy::too_many_arguments)]
fn blood_effect(
    name: &str,
    look: Look,
    size: (f32, f32),
    radius: f32,
    speed_share: (f32, f32),
    spread: Spread,
    falls: bool,
    spawner: SpawnerSettings,
) -> EffectAsset {
    let writer = ExprWriter::new();
    let velocity = writer.add_property("velocity", Vec3::ZERO.into());
    let gravity = falls.then(|| writer.add_property("gravity", Vec3::ZERO.into()));

    let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.0).expr());
    let init_position = SetPositionSphereModifier {
        center: writer.lit(Vec3::ZERO).expr(),
        radius: writer.lit(radius).expr(),
        dimension: ShapeDimension::Volume,
    };
    let random = writer.rand(VectorType::VEC3F) * writer.lit(2.0) - writer.lit(1.0);
    let spread = match spread {
        Spread::Cube(half) => random * writer.lit(half),
        Spread::Sphere(slowest, fastest) => random.normalized() * writer.lit(slowest).uniform(writer.lit(fastest)),
    };
    let share = writer.lit(speed_share.0).uniform(writer.lit(speed_share.1));
    let init_velocity = SetAttributeModifier::new(Attribute::VELOCITY, (writer.prop(velocity) * share + spread).expr());
    // The starting speed, which the fade is measured against.
    let init_speed = SetAttributeModifier::new(Attribute::F32_0, writer.attr(Attribute::VELOCITY).length().expr());
    // Drag takes speed down as `v0 * exp(-DRAG * t)`, so this is when it
    // reaches `STILL_SPEED`.
    let still = writer.lit(STILL_SPEED);
    let lifetime = (writer.attr(Attribute::F32_0).max(writer.lit(STILL_SPEED * 1.01)) / still.clone()).log()
        / writer.lit(DRAG);
    let init_lifetime = SetAttributeModifier::new(Attribute::LIFETIME, lifetime.expr());
    let init_start_size = SetAttributeModifier::new(Attribute::F32_1, writer.lit(size.0).uniform(writer.lit(size.1)).expr());
    let init_size = SetAttributeModifier::new(Attribute::SIZE, writer.attr(Attribute::F32_1).expr());
    let grown = writer.lit(1.0) + writer.attr(Attribute::AGE) / writer.attr(Attribute::LIFETIME).max(writer.lit(1.0e-3));
    let update_size = SetAttributeModifier::new(Attribute::SIZE, (writer.attr(Attribute::F32_1) * grown).expr());
    let init_turn = SetAttributeModifier::new(
        Attribute::F32_2,
        (writer.rand(ScalarType::Float) * writer.lit(std::f32::consts::TAU)).expr(),
    );
    let turn = writer.attr(Attribute::F32_2).expr();
    let lumps = (BLOB_GRID.x * BLOB_GRID.y) as f32;
    let init_lump = SetAttributeModifier::new(
        Attribute::SPRITE_INDEX,
        // Short of `lumps`, so the floor never reaches past the last.
        (writer.rand(ScalarType::Float) * writer.lit(lumps - 0.001)).floor().cast(ScalarType::Int).expr(),
    );

    let fade = if falls {
        (writer.lit(1.0) - writer.attr(Attribute::AGE) / writer.attr(Attribute::LIFETIME).max(writer.lit(1.0e-3))).saturate()
    } else {
        ((writer.attr(Attribute::VELOCITY).length() - still.clone())
            / (writer.attr(Attribute::F32_0) - still).max(writer.lit(1.0e-3)))
        .saturate()
    };
    // Not `LinearDragModifier`: its `v *= 1 - DRAG * dt` depends on frame rate
    // and zeroes every drop on a frame of 1 / DRAG.
    let drag = SetAttributeModifier::new(
        Attribute::VELOCITY,
        (writer.attr(Attribute::VELOCITY) * (writer.lit(-DRAG) * writer.delta_time()).exp()).expr(),
    );
    let pull = gravity.map(|gravity| AccelModifier::new(writer.prop(gravity).expr()));
    let blood = LinearRgba::from(Color::srgb(0.42, 0.02, 0.02));
    let color = writer.lit(Vec3::new(blood.red, blood.green, blood.blue)).vec4_xyz_w(fade);
    let update_color = SetAttributeModifier::new(Attribute::HDR_COLOR, color.expr());

    let mut module = writer.finish();
    let round = RoundModifier::constant(&mut module, 1.0);
    if matches!(look, Look::Goo | Look::Blob) {
        // Slot 0, the only one: bound by each emitter's `EffectMaterial`.
        module.add_texture_slot("splat", SlotDimension::D2);
    }

    let effect = EffectAsset::new(CAPACITY, spawner, module)
        .with_name(name)
        .with_alpha_mode(bevy_hanabi::AlphaMode::Blend)
        .init(init_position)
        .init(init_velocity)
        .init(init_speed)
        .init(init_age)
        .init(init_lifetime)
        .init(init_start_size)
        .init(init_size)
        .update(drag)
        .update(update_color);
    let effect = match pull {
        Some(pull) => effect.update(pull),
        None => effect,
    };
    match look {
        Look::Disc => effect
            .render(OrientModifier::new(OrientMode::FaceCameraPosition))
            .render(round),
        Look::Goo => effect
            .init(init_turn)
            .update(update_size)
            .render(OrientModifier::new(OrientMode::FaceCameraPosition).with_rotation(turn))
            .render(ParticleTextureModifier {
                texture_slot: 0,
                sample_mapping: ImageSampleMapping::ModulateOpacityFromR,
            }),
        Look::Blob => effect
            .init(init_turn)
            .init(init_lump)
            .render(OrientModifier::new(OrientMode::FaceCameraPosition).with_rotation(turn))
            .render(FlipbookModifier { sprite_grid_size: BLOB_GRID })
            .render(ParticleTextureModifier {
                texture_slot: 0,
                sample_mapping: ImageSampleMapping::ModulateOpacityFromR,
            }),
    }
}

/// 0 to 1, the same on every machine, so the textures are too.
fn noise(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

/// [`BLOB_GRID`] lumps of `texels` square in the red channel, rows top to bottom:
/// each a few circles run together, some with a droplet beside.
pub fn blob_sheet(texels: u32) -> Vec<u8> {
    // Polynomial smooth minimum: circles closer than `k` run together.
    let smooth_min = |a: f32, b: f32, k: f32| -> f32 {
        let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
        b + (a - b) * h - k * h * (1.0 - h)
    };

    let (width, height) = (BLOB_GRID.x * texels, BLOB_GRID.y * texels);
    let mut red = vec![0u8; (width * height) as usize];
    let texel = 2.0 / texels as f32;
    for lump in 0..BLOB_GRID.x * BLOB_GRID.y {
        let seed = lump * 97 + 13;
        // Cell coordinates run -1 to 1; everything stays inside 0.9, so
        // filtering never reaches a neighbor.
        let parts = 4 + (noise(seed) * 4.0) as u32;
        let circles: Vec<(Vec2, f32)> = (0..parts)
            .map(|part| {
                let n = seed + part * 5;
                let at = Vec2::from_angle(noise(n + 1) * std::f32::consts::TAU) * 0.45 * noise(n + 2).sqrt();
                (at, 0.11 + 0.19 * noise(n + 3))
            })
            .collect();
        let droplets: Vec<(Vec2, f32)> = (0..(noise(seed + 40) * 2.99) as u32)
            .map(|drop| {
                let n = seed + 50 + drop * 3;
                (Vec2::from_angle(noise(n) * std::f32::consts::TAU) * (0.65 + 0.15 * noise(n + 1)), 0.06 + 0.06 * noise(n + 2))
            })
            .collect();
        let (column, row) = (lump % BLOB_GRID.x, lump / BLOB_GRID.x);
        for y in 0..texels {
            for x in 0..texels {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * texel - Vec2::ONE;
                let mut nearest = f32::MAX;
                for &(center, radius) in &circles {
                    let d = p.distance(center) - radius;
                    nearest = if nearest == f32::MAX { d } else { smooth_min(nearest, d, 0.08) };
                }
                for &(center, radius) in &droplets {
                    nearest = nearest.min(p.distance(center) - radius);
                }
                let coverage = (0.5 - nearest / texel).clamp(0.0, 1.0);
                let index = (row * texels + y) * width + column * texels + x;
                red[index as usize] = (coverage * 255.0).round() as u8;
            }
        }
    }
    red
}

/// A blob with wandering, forking tendrils, `texels` square in the red channel.
pub fn goo_splat(texels: u32) -> Vec<u8> {
    /// Half-widths at each end.
    struct Strand {
        from: Vec2,
        to: Vec2,
        width: (f32, f32),
    }
    const ARMS: u32 = 7;
    const STEP: f32 = 0.07;
    let mut strands = Vec::new();
    let mut drops = Vec::new();
    // Tendrils still to grow: where, which way, half-width, and a seed.
    let mut growing: Vec<(Vec2, f32, f32, u32)> = (0..ARMS)
        .map(|arm| {
            let angle = (arm as f32 + (noise(arm) - 0.5) * 0.7) / ARMS as f32 * std::f32::consts::TAU;
            (Vec2::from_angle(angle) * 0.12, angle, 0.055 + 0.02 * noise(arm + 50), arm * 7919 + 1)
        })
        .collect();
    while let Some((mut at, mut heading, mut width, mut seed)) = growing.pop() {
        while width > 0.008 && at.length() < 0.92 {
            seed = seed.wrapping_add(1);
            heading += (noise(seed) - 0.5) * 0.6;
            let next = at + Vec2::from_angle(heading) * STEP;
            let thinner = width * (0.86 - 0.08 * noise(seed + 3));
            strands.push(Strand { from: at, to: next, width: (width, thinner) });
            if noise(seed + 11) < 0.28 && width > 0.016 {
                let side = if noise(seed + 17) < 0.5 { -1.0 } else { 1.0 };
                let fork = heading + side * (0.45 + 0.6 * noise(seed + 23));
                growing.push((next, fork, thinner * 0.75, seed.wrapping_mul(31) + 5));
            }
            at = next;
            width = thinner;
        }
        drops.push((at, width * 1.6 + 0.012));
    }
    let to_segment = |p: Vec2, a: Vec2, b: Vec2| -> (f32, f32) {
        let along = b - a;
        let t = ((p - a).dot(along) / along.length_squared().max(1.0e-9)).clamp(0.0, 1.0);
        (p.distance(a + along * t), t)
    };

    let texel = 2.0 / texels as f32;
    let mut red = Vec::with_capacity((texels * texels) as usize);
    for y in 0..texels {
        for x in 0..texels {
            // -1 to 1; the tendrils stop at 0.92.
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * texel - Vec2::ONE;
            let angle = p.y.atan2(p.x);
            let mut nearest = p.length() - (0.2 + 0.035 * (angle * 5.0).sin() + 0.025 * (angle * 3.0 + 1.0).sin());
            for strand in &strands {
                let (off, t) = to_segment(p, strand.from, strand.to);
                nearest = nearest.min(off - (strand.width.0 + (strand.width.1 - strand.width.0) * t));
            }
            for &(center, radius) in &drops {
                nearest = nearest.min(p.distance(center) - radius);
            }
            // One texel of antialiasing either side of the edge.
            let coverage = (0.5 - nearest / texel).clamp(0.0, 1.0);
            red.push((coverage * 255.0).round() as u8);
        }
    }
    red
}

/// Hanabi reads opacity from the red channel.
fn mask_image(red: Vec<u8>, width: u32, height: u32) -> Image {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::ImageSampler;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    let rgba: Vec<u8> = red.into_iter().flat_map(|red| [red, 0, 0, 255]).collect();
    let mut image = Image::new(
        Extent3d { width, height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        rgba,
        // Coverage, not color, so it is not decoded as sRGB.
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

fn make_the_effects(mut commands: Commands, mut effects: ResMut<Assets<EffectAsset>>, mut images: ResMut<Assets<Image>>) {
    let trail = blood_effect(
        "gib_blood",
        Look::Disc,
        DROP_SIZE,
        0.0,
        (1.0, 1.0),
        Spread::Cube(JITTER),
        false,
        SpawnerSettings::rate(RATE.into()).with_starts_active(false),
    );
    let burst = blood_effect(
        "gib_burst",
        Look::Goo,
        BURST_DROP_SIZE,
        BURST_RADIUS,
        (1.0, 1.0),
        Spread::Sphere(BURST_SPEED.0, BURST_SPEED.1),
        false,
        SpawnerSettings::rate(BURST_RATE.into()),
    );
    let spurt = blood_effect(
        "hit_spurt",
        Look::Blob,
        SPURT_DROP_SIZE,
        0.0,
        SPURT_SPEED_SHARE,
        Spread::Cube(SPURT_JITTER),
        true,
        SpawnerSettings::burst(CpuValue::Uniform(SPURT_CLUMP), CpuValue::Uniform(SPURT_PERIOD)),
    );
    commands.insert_resource(BloodEffects {
        trail: effects.add(trail),
        burst: effects.add(burst),
        spurt: effects.add(spurt),
        splat: images.add(mask_image(goo_splat(SPLAT_TEXELS), SPLAT_TEXELS, SPLAT_TEXELS)),
        blobs: images.add(mask_image(blob_sheet(BLOB_TEXELS), BLOB_GRID.x * BLOB_TEXELS, BLOB_GRID.y * BLOB_TEXELS)),
    });
}

/// Keyed on the absence of [`Bleeding`] rather than on the gib arriving, since
/// a client's gib has no `Skeleton` until it is dressed.
fn bleed_from_new_gibs(
    mut commands: Commands,
    effects: Res<BloodEffects>,
    gibs: Query<(Entity, &Ragdoll, &Skeleton), Without<Bleeding>>,
) {
    for (gib, ragdoll, skeleton) in &gibs {
        commands.entity(gib).insert(Bleeding);
        // An older gib reached a client joining late.
        if let Some(at) = ragdoll.torn_at()
            && ragdoll.age() <= BURST_SECONDS
        {
            commands.spawn((
                Burst { age: 0.0, seconds: BURST_SECONDS },
                ParticleEffect::new(effects.burst.clone()),
                EffectMaterial { images: vec![effects.splat.clone()] },
                EffectProperties::default(),
                Transform::from_translation(at),
                Name::new("Gib burst"),
            ));
        }
        for (end, (at, _)) in ragdoll.severed_ends(skeleton, 0.0).enumerate() {
            commands.spawn((
                BleedsFrom { gib, end, idle: 0.0 },
                ParticleEffect::new(effects.trail.clone()),
                EffectProperties::default(),
                Transform::from_translation(at),
                Name::new("Gib blood"),
            ));
        }
    }
}

/// An emitter goes once its gib is asleep and its last drop has faded. A
/// shove that wakes the gib later gets no trail.
fn follow_the_severed_ends(
    mut commands: Commands,
    fixed: Res<Time<Fixed>>,
    time: Res<Time>,
    gibs: Query<(&Ragdoll, &Skeleton)>,
    mut emitters: Query<(Entity, &mut BleedsFrom, &mut Transform, &mut EffectProperties, Option<&mut EffectSpawner>)>,
) {
    let dt = fixed.timestep().as_secs_f32();
    for (entity, mut from, mut transform, mut properties, spawner) in &mut emitters {
        let Ok((ragdoll, skeleton)) = gibs.get(from.gib) else {
            commands.entity(entity).despawn();
            continue;
        };
        let Some((at, velocity)) = ragdoll.severed_ends(skeleton, dt).nth(from.end) else {
            continue;
        };
        let bleeding = !ragdoll.is_grounded() && !ragdoll.is_asleep() && velocity.length() > BLEED_SPEED;
        from.idle = if bleeding { 0.0 } else { from.idle + time.delta_secs() };
        if ragdoll.is_asleep() && from.idle >= LAST_DROP_GONE {
            commands.entity(entity).despawn();
            continue;
        }
        // Added by Hanabi in `PostUpdate`, so absent on an emitter's first frame.
        if let Some(mut spawner) = spawner
            && spawner.active != bleeding
        {
            spawner.active = bleeding;
        }
        if bleeding {
            transform.translation = at;
            properties.set("velocity", velocity.into());
        }
    }
}

fn spurt_from_hits(
    mut commands: Commands,
    effects: Res<BloodEffects>,
    rules: Option<Res<MovementRules>>,
    mut dealt: MessageReader<DamageDealt>,
    mut hit_bodies: HitBodies,
) {
    hit_bodies.catch_up();
    for hit in dealt.read() {
        if hit.amount == 0 || hit.cause.how != How::Hitscan || !hit_bodies.is_a_body(hit.target) {
            continue;
        }
        let Some(direction) = hit.from.and_then(|from| (hit.point - from).try_normalize()) else { continue };
        let mut properties = EffectProperties::default();
        properties.set("velocity", (direction * SPURT_SPEED).into());
        let fall = rules.as_deref().map_or_else(|| MovementRules::default().fall(), MovementRules::fall);
        properties.set("gravity", (Vec3::Y * fall).into());
        commands.spawn((
            Burst { age: 0.0, seconds: SPURT_SECONDS },
            ParticleEffect::new(effects.spurt.clone()),
            EffectMaterial { images: vec![effects.blobs.clone()] },
            properties,
            Transform::from_translation(hit.point),
            Name::new("Hit spurt"),
        ));
    }
}

fn end_the_bursts(
    mut commands: Commands,
    time: Res<Time>,
    mut bursts: Query<(Entity, &mut Burst, Option<&mut EffectSpawner>)>,
) {
    for (entity, mut burst, spawner) in &mut bursts {
        burst.age += time.delta_secs();
        if burst.age >= burst.seconds + LAST_DROP_GONE {
            commands.entity(entity).despawn();
        } else if burst.age >= burst.seconds
            && let Some(mut spawner) = spawner
            && spawner.active
        {
            spawner.active = false;
        }
    }
}

/// See [`crate::game::warm_up`].
fn warm_up_the_blood(mut commands: Commands, effects: Res<BloodEffects>, mut images: ResMut<Assets<Image>>) {
    let layer = RenderLayers::layer(crate::common::render_layers::WARM_UP);
    commands.spawn(warm_up_camera(&mut images, "Blood warm-up camera"));
    let looks = [
        (&effects.trail, &effects.splat),
        (&effects.burst, &effects.splat),
        (&effects.spurt, &effects.blobs),
    ];
    for (effect, image) in looks {
        let mut properties = EffectProperties::default();
        properties.set("velocity", (Vec3::NEG_Z * BLEED_SPEED * 2.0).into());
        commands.spawn((
            WarmUp::default(),
            ParticleEffect::new(effect.clone()),
            EffectMaterial { images: vec![image.clone()] },
            properties,
            Transform::from_translation(Vec3::NEG_Z),
            layer.clone(),
            Name::new("Blood warm-up"),
        ));
    }
}

fn start_the_warm_up(mut spawners: Query<&mut EffectSpawner, (With<WarmUp>, Added<EffectSpawner>)>) {
    for mut spawner in &mut spawners {
        spawner.active = true;
    }
}

/// Added in `main.rs` beside `DecalPlugin`: Hanabi needs a renderer, which the
/// game's tests do not have.
pub struct GibBloodPlugin;

impl Plugin for GibBloodPlugin {
    fn build(&self, app: &mut App) {
        WarmUpPlugin::add_once(app);
        app.add_plugins(HanabiPlugin)
            .add_systems(Startup, (make_the_effects, warm_up_the_blood).chain())
            // Ungated: corpses arrive from the wire whenever.
            .add_systems(Update, (bleed_from_new_gibs, follow_the_severed_ends).chain())
            .add_systems(Update, (spurt_from_hits, end_the_bursts, start_the_warm_up));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if the splat is not solid in the middle and clear at the corners,
    /// or no tendril reaches most of the way out.
    #[test]
    fn the_splat_is_solid_in_the_middle_and_clear_at_the_corners() {
        let n = SPLAT_TEXELS as usize;
        let red = goo_splat(SPLAT_TEXELS);
        assert_eq!(red.len(), n * n);
        assert_eq!(red[n / 2 * n + n / 2], 255);
        for corner in [0, n - 1, n * (n - 1), n * n - 1] {
            assert_eq!(red[corner], 0, "corner {corner}");
        }
        let far = (0..n * n)
            .filter(|&i| red[i] > 128)
            .map(|i| Vec2::new((i % n) as f32, (i / n) as f32).distance(Vec2::splat(n as f32 / 2.0)))
            .fold(0.0, f32::max);
        assert!(far > n as f32 * 0.35, "reaches {far} texels of {}", n / 2);
    }

    /// Fails if a lump is empty, reaches the edge of its cell (where
    /// filtering would bleed into a neighbor), or two lumps are the same.
    #[test]
    fn every_lump_is_its_own_and_stays_in_its_cell() {
        let n = BLOB_TEXELS as usize;
        let width = BLOB_GRID.x as usize * n;
        let red = blob_sheet(BLOB_TEXELS);
        let cell = |lump: usize| -> Vec<u8> {
            let (column, row) = (lump % BLOB_GRID.x as usize, lump / BLOB_GRID.x as usize);
            (0..n * n).map(|i| red[(row * n + i / n) * width + column * n + i % n]).collect()
        };
        let cells: Vec<Vec<u8>> = (0..(BLOB_GRID.x * BLOB_GRID.y) as usize).map(cell).collect();
        for (lump, texels) in cells.iter().enumerate() {
            assert!(texels.iter().filter(|&&r| r > 128).count() > n * n / 10, "lump {lump} is nearly empty");
            let edge = (0..n).flat_map(|i| [i, (n - 1) * n + i, i * n, i * n + n - 1]);
            assert!(edge.into_iter().all(|i| texels[i] == 0), "lump {lump} reaches its edge");
            for (other, theirs) in cells.iter().enumerate().skip(lump + 1) {
                assert_ne!(texels, theirs, "lumps {lump} and {other} are the same");
            }
        }
    }
}
