//! A projectile's trail ([`Trail::Cloud`]): the blast's clouds, left in the air where it passed,
//! with no gravity. Drawing only; every machine makes its own.
//!
//! The emitter is not a child, because a projectile's transform is scaled by its radius and turned
//! by its spin. It replays the projectile's drawn path [`GAP`] behind it, so our own shot trails
//! from the barrel. When the projectile goes, the emitter runs on to where it was going, since the
//! drawn position lags the simulated one by up to a tick. It cuts across a bounce, so a bouncing
//! projectile should have no trail.

use std::collections::VecDeque;

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy_hanabi::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::projectile::Trail;
use crate::constants::one_tick;
use crate::game::blast_cloud::{CloudLook, CLOUD_GRID};
use crate::game::collision::CollisionWorld;
use crate::game::player::PhysicsBody;
use crate::game::projectile::Projectile;
use crate::game::warm_up::{self, WarmUp};

/// Meters behind the projectile, along its path, that the emitter is.
const GAP: f32 = 0.1;

/// Clouds per meter of path.
const PER_METER: f32 = 12.0;

/// Seconds a cloud lasts.
const LIFETIME: (f32, f32) = (1.35, 1.8);

/// Linear RGB and alpha by share of a cloud's life. Above 1 glows.
const TRAIL_BURN: [(f32, Vec4); 8] = [
    (0.0, Vec4::new(5.0, 3.2, 0.8, 1.0)),
    (0.075, Vec4::new(3.6, 1.2, 0.15, 1.0)),
    (0.15, Vec4::new(1.4, 0.12, 0.03, 1.0)),
    (0.2, Vec4::new(0.3, 0.025, 0.015, 0.95)),
    (0.25, Vec4::new(0.04, 0.035, 0.03, 0.85)),
    (0.35, Vec4::new(0.55, 0.55, 0.55, 0.6)),
    (0.5, Vec4::new(1.0, 1.0, 1.0, 0.4)),
    (1.0, Vec4::new(1.0, 1.0, 1.0, 0.0)),
];

/// Particles one trail may have alive at once: past 45 m/s it thins.
const CAPACITY: u32 = 1024;

/// Meters a cloud starts off the path, at most, on each axis.
const JITTER: f32 = 0.03;

/// Meters per second a cloud drifts at most as it starts, braked like the blast's.
const DRIFT: f32 = 0.4;
const BRAKE: f32 = 4.0;

/// Meters wide as it starts, and meters it grows per second of age.
const START_SIZE: f32 = 0.14;
const GROWTH: f32 = 0.6;

const SIZE_SHARE: (f32, f32) = (0.75, 1.25);

/// Radians per second a cloud turns at most.
const SPIN: f32 = 1.5;

/// Seconds after its projectile goes that a trail's last cloud has faded.
const LAST_CLOUD_GONE: f32 = LIFETIME.1 + 0.2;

#[derive(Resource)]
struct TrailLook {
    effect: Handle<EffectAsset>,
}

/// On a projectile `start_trails` has seen, trailed or not.
#[derive(Component)]
struct Trailed;

/// Placed on the particles' clock, so already in step with the kill cam's view.
#[derive(Component)]
#[require(crate::game::replay::Unreplayed)]
struct RocketTrail {
    of: Entity,
    /// Seconds of `Time` since the trail started, and where its projectile was drawn then, oldest
    /// first.
    path: VecDeque<(f32, Vec3)>,
    clock: f32,
    /// Seconds of `Time<EffectSimulation>` since the trail started, less how far that clock
    /// trailed `Time` then: the moment on `path` the clouds are laid at. Behind `clock` while the
    /// kill cam replays.
    shown: f32,
    /// Seconds behind the projectile, along its path, that the emitter is.
    delay: f32,
    /// Includes the fraction carried to the next frame.
    owed: f32,
    /// Clouds to spawn this frame, handed to Hanabi after it ticks.
    spawn: u32,
    /// Seconds since its projectile went.
    ended: Option<f32>,
    /// Where the trail runs on to if its projectile goes this frame.
    end: Vec3,
}

impl RocketTrail {
    /// `lag` is seconds the particles' clock trails `Time`.
    fn of(projectile: Entity, at: Vec3, lag: f32) -> Self {
        Self { of: projectile, path: VecDeque::new(), clock: 0.0, shown: -lag, delay: 0.0, owed: 0.0, spawn: 0, ended: None, end: at }
    }

    fn move_to(&mut self, transform: &mut Transform, properties: &mut EffectProperties, at: Vec3) {
        let was = transform.translation;
        self.owed += was.distance(at) * PER_METER;
        let whole = self.owed.floor();
        self.owed -= whole;
        self.spawn = whole as u32;
        transform.translation = at;
        properties.set("behind", (was - at).into());
    }
}

/// Stopped at the first wall. Straight, since gravity bends a tick's flight by millimeters.
fn a_tick_on(from: Vec3, velocity: Vec3, world: Option<&CollisionWorld>) -> Vec3 {
    let Ok(direction) = Dir3::new(velocity) else { return from };
    let range = velocity.length() * one_tick().as_secs_f32();
    let wall = world.and_then(|world| world.ray_distance(&Ray3d::new(from, direction), range));
    from + direction * wall.unwrap_or(range)
}

/// Spawns a few clouds every frame so its pipelines compile.
#[derive(Component)]
struct WarmUpTrail;

fn trail_effect() -> EffectAsset {
    let writer = ExprWriter::new();
    let behind = writer.add_property("behind", Vec3::ZERO.into());

    let age = || writer.attr(Attribute::AGE);
    let shape = || writer.attr(Attribute::F32_0);
    let size = || writer.attr(Attribute::F32_1);
    let spin = || writer.attr(Attribute::F32_2);

    let init_shape_seed = SetAttributeModifier::new(Attribute::F32_0, writer.rand(ScalarType::Float).expr());
    let init_size_seed = SetAttributeModifier::new(Attribute::F32_1, writer.rand(ScalarType::Float).expr());
    let init_spin_seed = SetAttributeModifier::new(Attribute::F32_2, writer.rand(ScalarType::Float).expr());
    let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.0).expr());
    let init_lifetime = SetAttributeModifier::new(Attribute::LIFETIME, writer.lit(LIFETIME.0).uniform(writer.lit(LIFETIME.1)).expr());

    // Spread along the stretch the emitter covered this frame, so a fast trail is not a row of
    // lumps a frame apart.
    let jitter = (writer.rand(VectorType::VEC3F) * writer.lit(2.0) - writer.lit(1.0)) * writer.lit(JITTER);
    let along = writer.prop(behind) * writer.rand(ScalarType::Float);
    let init_position = SetAttributeModifier::new(Attribute::POSITION, (along + jitter).expr());
    let init_velocity = SetVelocitySphereModifier {
        center: writer.lit(Vec3::ZERO).expr(),
        speed: writer.lit(0.0).uniform(writer.lit(DRIFT)).expr(),
    };
    // Not `LinearDragModifier`, whose `v *= 1 - k * dt` depends on frame rate.
    let braked = writer.attr(Attribute::VELOCITY) * (writer.lit(-BRAKE) * writer.delta_time()).exp();
    let update_velocity = SetAttributeModifier::new(Attribute::VELOCITY, braked.expr());

    let size_share = writer.lit(SIZE_SHARE.0) + size() * writer.lit(SIZE_SHARE.1 - SIZE_SHARE.0);
    let width = (writer.lit(START_SIZE) + writer.lit(GROWTH) * age()) * size_share;
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
    for (at, color) in TRAIL_BURN {
        burn.add_key(at, color);
    }

    // A rate of nothing: `hand_over_the_clouds` sets each frame's count from the path covered.
    EffectAsset::new(CAPACITY, SpawnerSettings::rate(0.0.into()), module)
        .with_name("rocket_trail")
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

fn make_the_look(mut commands: Commands, mut effects: ResMut<Assets<EffectAsset>>) {
    commands.insert_resource(TrailLook { effect: effects.add(trail_effect()) });
}

fn behind_by(offset: Vec3) -> EffectProperties {
    let mut properties = EffectProperties::default();
    properties.set("behind", offset.into());
    properties
}

/// Keyed on the trail's absence rather than on the projectile arriving: replication does not
/// promise a projectile's components in one packet.
fn start_trails(
    mut commands: Commands,
    time: Res<Time>,
    particles: Res<Time<EffectSimulation>>,
    trail_look: Res<TrailLook>,
    cloud_look: Res<CloudLook>,
    projectiles: Query<(Entity, &Projectile, &Transform), Without<Trailed>>,
) {
    let lag = (time.elapsed_secs_f64() - particles.elapsed_secs_f64()).max(0.0) as f32;
    for (entity, projectile, transform) in &projectiles {
        commands.entity(entity).insert(Trailed);
        if projectile.spec.trail != Trail::Cloud {
            continue;
        }
        commands.spawn((
            RocketTrail::of(entity, transform.translation, lag),
            ParticleEffect::new(trail_look.effect.clone()),
            EffectMaterial { images: vec![cloud_look.sheet.clone()] },
            behind_by(Vec3::ZERO),
            Transform::from_translation(transform.translation),
            Name::new("Rocket trail"),
        ));
    }
}

/// Where the path was `delay` seconds before its newest point, `path` oldest first; the oldest
/// point when the path does not go back that far.
fn back_along(path: &VecDeque<(f32, Vec3)>, delay: f32) -> Option<Vec3> {
    let &(newest, _) = path.back()?;
    let at = newest - delay;
    let later = path.iter().position(|&(time, _)| time >= at)?;
    if later == 0 {
        return Some(path[0].1);
    }
    let (t0, p0) = path[later - 1];
    let (t1, p1) = path[later];
    let share = if t1 > t0 { (at - t0) / (t1 - t0) } else { 1.0 };
    Some(p0.lerp(p1, share))
}

/// Runs after the barrel offset is drawn. A trail Hanabi has not set up yet stays where it started
/// and catches up once it can spawn, so the clouds near the barrel are not lost.
fn follow_projectiles(
    mut commands: Commands,
    time: Res<Time>,
    particles: Res<Time<EffectSimulation>>,
    world: Option<Res<CollisionWorld>>,
    projectiles: Query<(&Projectile, &Transform, Option<&PhysicsBody>), Without<RocketTrail>>,
    mut trails: Query<(Entity, &mut RocketTrail, &mut Transform, &mut EffectProperties, Has<EffectSpawner>)>,
) {
    let dt = time.delta_secs();
    let particle_dt = particles.delta_secs();
    for (entity, mut trail, mut transform, mut properties, ready) in &mut trails {
        trail.spawn = 0;
        trail.shown += particle_dt;
        let gone = match projectiles.get(trail.of) {
            Ok((projectile, drawn, body)) if trail.ended.is_none() => {
                trail.clock += dt;
                let clock = trail.clock;
                trail.path.push_back((clock, drawn.translation));
                let simulated = body.map_or(drawn.translation, |body| body.current);
                trail.end = a_tick_on(simulated, projectile.velocity, world.as_deref());
                trail.delay = GAP / projectile.velocity.length().max(1.0);
                false
            }
            _ => true,
        };
        // Its projectile went at `clock`; the clouds run on to where it was going once the
        // particles reach that moment.
        if gone && (trail.shown >= trail.clock || !ready) {
            if trail.ended.is_none() && ready {
                let end = trail.end;
                trail.move_to(&mut transform, &mut properties, end);
            }
            let ended = trail.ended.map_or(0.0, |ended| ended + particle_dt);
            trail.ended = Some(ended);
            if ended >= LAST_CLOUD_GONE {
                commands.entity(entity).despawn();
            }
            continue;
        }
        if !ready {
            continue;
        }

        let (clock, shown, delay) = (trail.clock, trail.shown, trail.delay);
        let Some(at) = back_along(&trail.path, clock - shown + delay) else { continue };
        // Keep one point older than the moment shown, to interpolate from next frame.
        while trail.path.len() > 2 && trail.path[1].0 <= shown - delay {
            trail.path.pop_front();
        }
        trail.move_to(&mut transform, &mut properties, at);
    }
}

/// After Hanabi's own tick, which would overwrite the count.
fn hand_over_the_clouds(
    mut trails: Query<(&mut RocketTrail, &mut EffectSpawner)>,
    mut warm_ups: Query<&mut EffectSpawner, (With<WarmUpTrail>, Without<RocketTrail>)>,
) {
    for (mut trail, mut spawner) in &mut trails {
        spawner.spawn_count = std::mem::take(&mut trail.spawn);
    }
    for mut spawner in &mut warm_ups {
        spawner.spawn_count = 4;
    }
}

fn clear_the_trails(mut commands: Commands, trails: Query<Entity, With<RocketTrail>>) {
    for trail in &trails {
        commands.entity(trail).despawn();
    }
}

/// See [`crate::game::warm_up`]. In `PostStartup`, after the blast clouds' sheet is made.
fn warm_up_the_trail(mut commands: Commands, trail_look: Res<TrailLook>, cloud_look: Res<CloudLook>) {
    commands.spawn((
        WarmUp,
        WarmUpTrail,
        ParticleEffect::new(trail_look.effect.clone()),
        EffectMaterial { images: vec![cloud_look.sheet.clone()] },
        behind_by(Vec3::X * 0.2),
        Transform::from_translation(Vec3::NEG_Z * 4.0),
        RenderLayers::layer(crate::common::render_layers::WARM_UP),
        Name::new("Rocket trail warm-up"),
    ));
}

/// Added in `launch.rs` beside `BlastCloudPlugin`, whose cloud sheet it draws with: Hanabi needs a
/// renderer, which the game's tests do not have.
pub struct RocketTrailPlugin;

impl Plugin for RocketTrailPlugin {
    fn build(&self, app: &mut App) {
        warm_up::ensure(app);
        if !app.is_plugin_added::<HanabiPlugin>() {
            app.add_plugins(HanabiPlugin);
        }
        app.add_systems(Startup, make_the_look)
            .add_systems(PostStartup, warm_up_the_trail)
            .add_systems(
                RunFixedMainLoop,
                (start_trails, follow_projectiles)
                    .chain()
                    .after(crate::game::projectile::draw_our_shots_from_the_muzzle)
                    .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
            )
            .add_systems(PostUpdate, hand_over_the_clouds.after(EffectSystems::TickSpawners))
            .add_systems(OnExit(AppMode::Play), clear_the_trails)
            .add_systems(OnExit(AppMode::Weapon), clear_the_trails);
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn path(points: &[(f32, f32)]) -> VecDeque<(f32, Vec3)> {
        points.iter().map(|&(time, x)| (time, Vec3::X * x)).collect()
    }

    fn frame(world: &mut World, dt: f32) {
        frame_with_particles(world, dt, dt);
    }

    fn frame_with_particles(world: &mut World, dt: f32, particle_dt: f32) {
        world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(dt));
        world.resource_mut::<Time<EffectSimulation>>().advance_by(std::time::Duration::from_secs_f32(particle_dt));
        world.run_system_once(follow_projectiles).unwrap();
        world.flush();
    }

    /// A rocket at the origin flying along -Z, and its trail.
    fn rocket_and_trail(world: &mut World, speed: f32, ready: bool) -> (Entity, Entity) {
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Time::<EffectSimulation>::default());
        let spec = crate::common::projectile::ProjectileSpec { initial_speed: speed, ..default() };
        let projectile = crate::game::projectile::fired(spec, Vec3::ZERO, Dir3::NEG_Z, None, None);
        let rocket = world.spawn((projectile, PhysicsBody::at(Vec3::ZERO), Transform::default())).id();
        let mut trail = world.spawn((RocketTrail::of(rocket, Vec3::ZERO, 0.0), Transform::default(), EffectProperties::default()));
        if ready {
            trail.insert(EffectSpawner::new(&SpawnerSettings::rate(0.0.into())));
        }
        (rocket, trail.id())
    }

    /// Clouds the trail was handed over the frames.
    fn fly(world: &mut World, (rocket, trail): (Entity, Entity), speed: f32, frames: usize, dt: f32) -> u32 {
        let mut spawned = 0;
        for _ in 0..frames {
            let mut body = world.get_mut::<PhysicsBody>(rocket).unwrap();
            body.previous = body.current;
            body.current += Vec3::NEG_Z * speed * dt;
            let at = body.current;
            world.get_mut::<Transform>(rocket).unwrap().translation = at;
            frame(world, dt);
            spawned += std::mem::take(&mut world.get_mut::<RocketTrail>(trail).unwrap().spawn);
        }
        spawned
    }

    /// Fails if the cloud count is off from the path covered, or the trail stops short of where its
    /// rocket was going.
    #[test]
    fn a_trail_lays_clouds_to_where_its_rocket_went() {
        let mut world = World::new();
        let (speed, dt) = (20.0, 0.01);
        let (rocket, trail) = rocket_and_trail(&mut world, speed, true);
        let spawned = fly(&mut world, (rocket, trail), speed, 50, dt) as f32;
        let flown = speed * dt * 50.0;
        assert!((spawned - (flown - GAP) * PER_METER).abs() <= 1.0, "{spawned} clouds over {flown} m");

        world.despawn(rocket);
        frame(&mut world, dt);
        let going = flown + speed * one_tick().as_secs_f32();
        let at = world.get::<Transform>(trail).unwrap().translation;
        assert!((at.z + going).abs() < 1e-3, "the trail stopped at {at}, short of {going} m");
        let last = world.get::<RocketTrail>(trail).unwrap().spawn as f32;
        assert!((last - (going - flown + GAP) * PER_METER).abs() <= 1.0, "{last} clouds over the last stretch");
    }

    /// Fails if a trail emits after its rocket is gone, or is never despawned.
    #[test]
    fn a_trail_stops_with_its_rocket_and_goes_once_its_clouds_fade() {
        let mut world = World::new();
        let (rocket, trail) = rocket_and_trail(&mut world, 20.0, true);
        fly(&mut world, (rocket, trail), 20.0, 10, 0.01);
        world.despawn(rocket);
        frame(&mut world, 0.01);
        world.get_mut::<RocketTrail>(trail).unwrap().spawn = 0;
        frame(&mut world, 0.01);
        assert_eq!(world.get::<RocketTrail>(trail).unwrap().spawn, 0);
        frame(&mut world, LAST_CLOUD_GONE);
        assert!(world.get_entity(trail).is_err(), "the trail outlived its clouds");
    }

    /// Fails if a trail lays clouds while the particles' clock is stopped, or does not lay them
    /// along the same path once it runs again.
    #[test]
    fn a_trail_waits_for_the_particles_clock() {
        let mut world = World::new();
        let (speed, dt) = (20.0, 0.01);
        let (rocket, trail) = rocket_and_trail(&mut world, speed, true);
        fly(&mut world, (rocket, trail), speed, 10, dt);
        let held = world.get::<Transform>(trail).unwrap().translation;
        let mut frozen = 0;
        for _ in 0..10 {
            let mut body = world.get_mut::<PhysicsBody>(rocket).unwrap();
            body.current += Vec3::NEG_Z * speed * dt;
            let at = body.current;
            world.get_mut::<Transform>(rocket).unwrap().translation = at;
            frame_with_particles(&mut world, dt, 0.0);
            frozen += std::mem::take(&mut world.get_mut::<RocketTrail>(trail).unwrap().spawn);
        }
        assert_eq!(frozen, 0, "laid clouds while the particles were stopped");
        assert_eq!(world.get::<Transform>(trail).unwrap().translation, held);

        world.despawn(rocket);
        frame_with_particles(&mut world, 0.0, 0.05);
        let caught_up = world.get::<Transform>(trail).unwrap().translation;
        assert!((caught_up.z - (held.z - speed * 0.05)).abs() < 1e-3, "{caught_up} after catching up from {held}");
        assert!(world.get::<RocketTrail>(trail).unwrap().ended.is_none(), "ran on to the end before reaching it");
    }

    /// Fails if the clouds a trail owes while Hanabi sets it up are lost.
    #[test]
    fn a_trail_set_up_late_still_trails_from_the_start() {
        let mut world = World::new();
        let (speed, dt) = (20.0, 0.01);
        let (rocket, trail) = rocket_and_trail(&mut world, speed, false);
        let early = fly(&mut world, (rocket, trail), speed, 3, dt);
        assert_eq!(early, 0);
        world.entity_mut(trail).insert(EffectSpawner::new(&SpawnerSettings::rate(0.0.into())));
        let spawned = fly(&mut world, (rocket, trail), speed, 1, dt) as f32;
        let flown = speed * dt * 4.0;
        assert!((spawned - (flown - GAP) * PER_METER).abs() <= 1.0, "{spawned} clouds over {flown} m");
        let behind = world.get::<Transform>(trail).unwrap().translation;
        assert!((behind.z + flown - GAP).abs() < 1e-3, "{behind}");
    }

    /// Fails if the burn's keys are out of order, it starts dim, or it does not end faded white.
    #[test]
    fn the_trail_burns_from_glowing_to_white() {
        assert_eq!(TRAIL_BURN.first().unwrap().0, 0.0);
        assert_eq!(TRAIL_BURN.last().unwrap().0, 1.0);
        assert!(TRAIL_BURN.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(TRAIL_BURN[0].1.x > 1.0, "the trail's start does not glow");
        let last = TRAIL_BURN.last().unwrap().1;
        assert_eq!(last.w, 0.0);
        assert!(last.x == last.y && last.y == last.z && last.x >= 0.9, "the trail ends {last}, not white");
    }

    /// Fails if the emitter is not where the projectile was `delay` ago, between two frames.
    #[test]
    fn the_trail_is_where_the_projectile_was_a_moment_ago() {
        let flown = path(&[(0.0, 0.0), (0.01, 0.2), (0.02, 0.4)]);
        let at = back_along(&flown, 0.005).unwrap();
        assert!((at.x - 0.3).abs() < 1e-5, "{at}");
    }

    /// Fails if a trail younger than its delay is put anywhere but where it started.
    #[test]
    fn a_new_trail_waits_where_the_projectile_started() {
        let flown = path(&[(0.0, 1.0), (0.004, 1.1)]);
        assert_eq!(back_along(&flown, 0.01), Some(Vec3::X));
        assert_eq!(back_along(&VecDeque::new(), 0.01), None);
    }

    /// Fails if the gap stops being about 10 cm at a rocket's speed.
    #[test]
    fn the_gap_is_ten_centimeters_at_a_rockets_speed() {
        let speed = 20.0;
        let flown: VecDeque<_> = (0..20).map(|i| (i as f32 / 144.0, Vec3::NEG_Z * speed * i as f32 / 144.0)).collect();
        let at = back_along(&flown, GAP / speed).unwrap();
        let gap = flown.back().unwrap().1.distance(at);
        assert!((gap - GAP).abs() < 1e-4, "{gap}");
    }
}
