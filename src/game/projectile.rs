//! Things that are thrown rather than traced: rockets, pipe bombs, and
//! whatever else [`ProjectileSpec`] describes. The arithmetic is in
//! [`crate::common::projectile`]; this is the part that asks the world.
//!
//! The move is swept against walls and hitboxes at once, nearer wins: a
//! rocket covers half a meter a tick and a wall slab is half a meter thick, so
//! a teleport-then-overlap test would pass through it. It uses the same
//! [`trace`] and [`CollisionWorld::ray_hit`] as hit registration.
//!
//! Bounces are resolved inside the step, up to `MAX_CONTACTS`: a pipe bomb
//! thrown into a corner meets two walls in one 15 ms tick.
//!
//! A hit leaves as an [`Explosion`]; splash and damage are in
//! [`crate::game::explosion`]. Position goes through [`PhysicsBody`] for
//! interpolation: at 30 m/s a rocket drawn at the tick visibly stutters.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::damage::{DamageSource, Explosion, PlayerId};
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::trace;
use crate::common::movement_rules::MovementRules;
use crate::common::projectile::{bounce, fly, Ending, ProjectileSpec};
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::player::{interpolate_bodies, LocalPlayer, PhysicsBody, Player};
use crate::game::viewmodel::ApparentMuzzle;
use crate::common::weapon::WeaponAction;
use crate::game::weapon::WeaponActionFired;

/// Walls one projectile may meet in a single tick. Past it the rest of the
/// tick's motion is dropped rather than risking a tunnel.
const MAX_CONTACTS: usize = 4;

/// Meters a bounce leaves the projectile off the wall. Landing exactly on the
/// plane, the next trace in the same tick finds it at zero distance and
/// bounces again, burning every contact.
const SKIN: f32 = 0.002;

/// Meters in front of the eye a projectile is born: outside the shooter's
/// body, and not inside a wall when firing with your face against one.
const MUZZLE_OFFSET: f32 = 0.5;

/// Seconds one of our own shots takes to ease from the drawn barrel onto the
/// path it is really on.
pub const MUZZLE_EASE: f32 = 0.5;

/// Something in flight. The spec is copied on so that switching weapons
/// cannot change a shot already in the air.
#[derive(Component, Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct Projectile {
    pub spec: ProjectileSpec,
    /// Who gets the kill; `None` is [`DamageSource::World`].
    pub owner: Option<PlayerId>,
    /// The body it came out of, which its own direct hits ignore.
    ///
    /// Not sent: only the authority resolves hits, and skipping it keeps this
    /// component out of entity mapping.
    #[serde(skip)]
    #[reflect(ignore)]
    pub shooter: Option<Entity>,
    pub velocity: Vec3,
    /// Radians. Cosmetic.
    pub spin: f32,
    /// Seconds of game time in the air.
    pub age: f32,
    pub bounces: u32,
}

impl Projectile {
    pub fn source(&self) -> DamageSource {
        match self.owner {
            Some(id) => DamageSource::Player(id),
            None => DamageSource::World,
        }
    }
}

/// One of our own shots, drawn from the barrel rather than the eye it left,
/// easing onto its real path. Display only, and only on the shooter's
/// machine: everybody else sees it leave the shooter's eye, which is where it
/// is.
///
/// Eased over distance flown rather than time, and over no further than the
/// first thing in its way: a rocket that lands before the ease is over would
/// otherwise go off somewhere other than where it is drawn.
#[derive(Component, Debug)]
pub struct FromTheMuzzle {
    /// Meters, world space, from the real position to the barrel.
    offset: Vec3,
    /// Meters of flight over which the offset goes to nothing.
    span: f32,
    /// Meters flown since it was marked, summed per frame so a bounce counts.
    flown: f32,
    /// Where it was drawn last frame, before the offset.
    last: Vec3,
}

/// How much of the offset is still drawn: all of it at the start, none at
/// `span`, smoothly at both ends so it neither jumps off the barrel nor snaps
/// onto the path.
fn muzzle_weight(flown: f32, span: f32) -> f32 {
    if span <= 0.0 {
        return 0.0;
    }
    1.0 - EaseFunction::SmoothStep.sample_clamped(flown / span)
}

/// Fires a projectile every `interval` along its own forward, at nobody in
/// particular. Nothing places one yet; it is meant to become a map feature.
#[derive(Component, Clone, Debug)]
pub struct ProjectileEmitter {
    pub spec: ProjectileSpec,
    /// Seconds between shots.
    pub interval: f32,
    /// Seconds since the last one. Starts at zero so a fresh emitter waits a
    /// full interval, letting its transform propagate and its planter move.
    pub since: f32,
}

impl ProjectileEmitter {
    pub fn new(spec: ProjectileSpec, interval: f32) -> Self {
        Self { spec, interval, since: 0.0 }
    }
}

pub struct ProjectilePlugin;

impl Plugin for ProjectilePlugin {
    fn build(&self, app: &mut App) {
        app
            // In `Deal`: `explode` turns these `Explosion`s into `Damage` in the
            // same tick.
            // Chained, so a shot is launched, flies its first tick and goes off in the tick it is
            // fired: the order the nav bake's rocket jumps are worked out in.
            .add_systems(
                FixedUpdate,
                (fire_projectiles, run_emitters, step_projectiles)
                    .chain()
                    .in_set(DamageSystems::Deal),
            )
            // Drawing, so ungated: a client flies none of these and draws all.
            .add_systems(Update, (dress_projectiles, spin_projectiles))
            // After the interpolation it offsets, and in the same frame the
            // shot appears, or it is drawn once at the eye. Chained so the
            // mark is in place for the drawing.
            .add_systems(
                RunFixedMainLoop,
                (start_our_shots_at_the_muzzle, draw_our_shots_from_the_muzzle)
                    .chain()
                    .after(interpolate_bodies)
                    .after(crate::game::viewmodel::find_the_apparent_muzzle)
                    .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
            )
            .init_resource::<ProjectileAssets>()
            .add_systems(OnExit(AppMode::Play), clear_projectiles)
        ;
    }
}

/// A free function because a player, an emitter and a test all launch one.
pub fn launch(
    commands: &mut Commands,
    spec: ProjectileSpec,
    from: Vec3,
    direction: Dir3,
    owner: Option<PlayerId>,
    shooter: Option<Entity>,
) -> Entity {
    commands
        .spawn((
            fired(spec, direction, owner, shooter),
            PhysicsBody::at(from),
            Transform::from_translation(from).with_scale(Vec3::splat(spec.radius)),
            Visibility::default(),
            Name::new("Projectile"),
        ))
        .id()
}

/// Where a body's shot leaves from: ahead of its eye along its aim.
pub fn muzzle(center: Vec3, stance: Stance, player: &Player) -> Vec3 {
    crate::game::hitscan::eye(center, stance, player.duck) + *crate::game::hitscan::aim(player) * MUZZLE_OFFSET
}

/// A projectile just fired along `direction`.
pub fn fired(spec: ProjectileSpec, direction: Dir3, owner: Option<PlayerId>, shooter: Option<Entity>) -> Projectile {
    Projectile { spec, owner, shooter, velocity: *direction * spec.initial_speed, spin: 0.0, age: 0.0, bounces: 0 }
}

/// Fires from the step's position, not the drawn one: an interpolated muzzle
/// would differ on every machine.
pub fn fire_projectiles(
    mut commands: Commands,
    mut fired: MessageReader<WeaponActionFired>,
    shooters: Query<(&PhysicsBody, &Player, &Stance, Option<&PlayerId>)>,
) {
    for shot in fired.read() {
        // The spec comes from the message, so a switch in the same tick cannot
        // change it.
        let WeaponAction::Projectile(spec) = shot.action else { continue };
        let Ok((body, player, stance, id)) = shooters.get(shot.shooter) else { continue };

        let direction = crate::game::hitscan::aim(player);
        launch(&mut commands, spec, muzzle(body.current, *stance, player), direction, id.copied(), Some(shot.shooter));
    }
}

/// Game time, so an emitter does not fire while somebody is in the editor.
pub fn run_emitters(
    mut commands: Commands,
    time: Res<Time>,
    mut emitters: Query<(&mut ProjectileEmitter, &GlobalTransform)>,
) {
    let dt = time.delta_secs();
    for (mut emitter, placed) in &mut emitters {
        emitter.since += dt;
        if emitter.since < emitter.interval {
            continue;
        }
        emitter.since = 0.0;

        // Its own forward, so an emitter is aimed by being turned.
        let Ok(direction) = Dir3::new(placed.forward().as_vec3()) else { continue };
        launch(
            &mut commands,
            emitter.spec,
            placed.translation() + *direction * MUZZLE_OFFSET,
            direction,
            // Nobody fired it: `DamageSource::World`.
            None,
            None,
        );
    }
}

/// Fly everything one tick, and detonate whatever stops flying.
pub fn step_projectiles(
    mut commands: Commands,
    // In `FixedUpdate` the plain clock is the fixed one, as `step_player` reads it.
    time: Res<Time>,
    world: Res<CollisionWorld>,
    rules: Res<MovementRules>,
    mut projectiles: Query<(Entity, &mut Projectile, &mut PhysicsBody, &mut Transform)>,
    bodies: Query<(Entity, &Hitboxes)>,
    mut blasts: MessageWriter<Explosion>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (entity, mut projectile, mut body, _transform) in &mut projectiles {
        let shooter = projectile.shooter;
        // The muzzle is inside your own hitbox.
        let others = || bodies.iter().filter(move |(target, _)| Some(*target) != shooter).map(|(target, boxes)| (target, *boxes));
        let (position, ending) = advance(&mut projectile, body.current, &world, rules.fall(), dt, |ray, distance| trace(ray, distance, others()));

        body.previous = body.current;
        body.current = position;

        let Some((ending, direct)) = ending else { continue };
        let spec = projectile.spec;
        if spec.detonates_on(ending) {
            blasts.write(Explosion {
                at: position,
                radius: spec.explosion_radius,
                damage: spec.damage_splash,
                knockback: spec.knockback,
                source: projectile.source(),
                direct: direct.map(|target| (target, spec.damage_direct_hit)),
            });
        }

        // Its flight is over whether or not it went off.
        commands.entity(entity).despawn();
    }
}

/// One tick of a projectile's flight from `position`: where it ends up, and how its flight ended
/// if it did, with the body it hit. Swept against the walls and against whatever `hits` finds
/// along a ray, so the nav bake flies a rocket with nothing in the way the way a round does.
pub fn advance(
    projectile: &mut Projectile,
    mut position: Vec3,
    world: &CollisionWorld,
    gravity: f32,
    dt: f32,
    mut hits: impl FnMut(&Ray3d, f32) -> Option<crate::common::hitscan::Hit<Entity>>,
) -> (Vec3, Option<(Ending, Option<Entity>)>) {
    projectile.age += dt;
    projectile.spin += projectile.spec.spin_rate * dt;

    let spec = projectile.spec;
    let (velocity, step) = fly(projectile.velocity, &spec, gravity, dt);
    projectile.velocity = velocity;

    let mut travel = step;
    let mut ending = None;

    for _ in 0..MAX_CONTACTS {
        let distance = travel.length();
        let Ok(direction) = Dir3::new(travel) else { break };
        let ray = Ray3d::new(position, direction);

        let body_hit = hits(&ray, distance);
        let wall = world.ray_hit(&ray, distance);

        // Nearest wins, or rockets pass through walls to bodies behind.
        let body_first = match (&body_hit, &wall) {
            (Some(hit), Some((wall, _))) => hit.distance <= *wall,
            (Some(_), None) => true,
            _ => false,
        };

        if body_first {
            let hit = body_hit.expect("a body was nearer than the wall");
            position = hit.point;
            ending = Some((Ending::DirectHit, Some(hit.target)));
            break;
        }

        let Some((reached, normal)) = wall else {
            position += travel;
            break;
        };

        let contact = ray.origin + *direction * reached;
        if projectile.bounces >= spec.max_bounces {
            // Stood off by its radius, so the blast's sight check does not
            // find the wall between it and everything.
            position = contact + normal * spec.radius;
            ending = Some((Ending::LastBounce, None));
            break;
        }

        projectile.bounces += 1;
        projectile.velocity = bounce(projectile.velocity, normal, spec.restitution);
        position = contact + normal * SKIN;
        // The rest of the step, along the new direction. Speed lost to the
        // bounce applies from next tick; the error is invisible.
        travel = projectile.velocity.normalize_or_zero() * (distance - reached).max(0.0);
    }

    // Checked after the move, so a fuse ending mid-tick still traveled.
    if ending.is_none() && projectile.age >= spec.max_lifetime {
        ending = Some((Ending::Fuse, None));
    }
    (position, ending)
}

/// One mesh and material for every projectile, scaled per projectile by the
/// transform. Filled on first use so the plugin does not need the renderer.
#[derive(Resource, Default)]
struct ProjectileAssets {
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
}

/// `spin` is replicated and drawn here rather than in the step, which only
/// runs where the projectile is simulated. Rotation only:
/// `interpolate_bodies` owns the translation.
fn spin_projectiles(mut projectiles: Query<(&Projectile, &mut Transform)>) {
    for (projectile, mut transform) in &mut projectiles {
        transform.rotation = Quat::from_rotation_x(projectile.spin)
            * Quat::from_rotation_y(projectile.spin * 0.5);
    }
}

fn dress_projectiles(
    mut commands: Commands,
    mut assets: ResMut<ProjectileAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut undressed: Query<
        (Entity, &Projectile, Option<&PhysicsBody>, Option<&mut Transform>),
        Without<Mesh3d>,
    >,
) {
    if undressed.is_empty() {
        return;
    }

    let mesh = assets
        .mesh
        .get_or_insert_with(|| meshes.add(Sphere::new(1.0).mesh().ico(2).unwrap()))
        .clone();
    let material = assets
        .material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb(0.9, 0.35, 0.1),
                emissive: LinearRgba::rgb(3.0, 0.8, 0.1),
                ..default()
            })
        })
        .clone();

    for (entity, projectile, body, transform) in &mut undressed {
        // From the spec: a replicated projectile never went through `launch`,
        // so its transform starts at unit scale. Only the scale: a whole new
        // transform draws it at the origin for a frame.
        let scale = Vec3::splat(projectile.spec.radius);
        match transform {
            Some(mut transform) => transform.scale = scale,
            None => {
                let at = body.map_or(Vec3::ZERO, |body| body.current);
                commands.entity(entity).insert(Transform::from_translation(at).with_scale(scale));
            }
        }
        commands
            .entity(entity)
            .insert((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone())));
    }
}

/// Keyed on either component arriving: replication does not promise them in
/// one packet. A shot already past its ease, as one reaching a client late can
/// be, is drawn where it is.
fn start_our_shots_at_the_muzzle(
    mut commands: Commands,
    muzzle: Option<Res<ApparentMuzzle>>,
    world: Option<Res<CollisionWorld>>,
    us: Query<&PlayerId, With<LocalPlayer>>,
    bodies: Query<(Entity, &Hitboxes), Without<LocalPlayer>>,
    shots: Query<
        (Entity, &Projectile, &PhysicsBody),
        (Or<(Added<Projectile>, Added<PhysicsBody>)>, Without<FromTheMuzzle>),
    >,
) {
    let Some(barrel) = muzzle.and_then(|muzzle| muzzle.0) else { return };
    let Ok(us) = us.single() else { return };
    for (entity, projectile, body) in &shots {
        if projectile.owner != Some(*us) || projectile.age >= MUZZLE_EASE {
            continue;
        }
        let speed = projectile.velocity.length();
        let Ok(direction) = Dir3::new(projectile.velocity) else { continue };
        // What is left of half a second of flight, cut short by whatever it
        // will meet first. A straight line: an arc or a bounce is close enough
        // over this distance.
        let range = speed * (MUZZLE_EASE - projectile.age);
        let ray = Ray3d::new(body.current, direction);
        let wall = world.as_ref().and_then(|world| world.ray_distance(&ray, range));
        let body_hit = trace(&ray, range, bodies.iter().map(|(entity, boxes)| (entity, *boxes)));
        let span = [Some(range), wall, body_hit.map(|hit| hit.distance)]
            .into_iter()
            .flatten()
            .fold(f32::INFINITY, f32::min);

        commands.entity(entity).insert(FromTheMuzzle {
            offset: barrel - body.current,
            span,
            flown: 0.0,
            last: body.current,
        });
    }
}

/// Composes onto what `interpolate_bodies` wrote this frame, which it rewrites
/// every frame the two differ.
fn draw_our_shots_from_the_muzzle(
    mut commands: Commands,
    mut shots: Query<(Entity, &mut FromTheMuzzle, &mut Transform)>,
) {
    for (entity, mut shot, mut transform) in &mut shots {
        let drawn = transform.translation;
        shot.flown += drawn.distance(shot.last);
        shot.last = drawn;

        let weight = muzzle_weight(shot.flown, shot.span);
        if weight <= 0.0 {
            commands.entity(entity).remove::<FromTheMuzzle>();
            continue;
        }
        transform.translation += shot.offset * weight;
    }
}

/// Called on leaving Play and from
/// [`reset_for_play`](crate::game::reset::reset_for_play).
pub fn clear_projectiles(
    mut commands: Commands,
    role: Option<Res<crate::common::net::NetRole>>,
    projectiles: Query<Entity, With<Projectile>>,
    emitters: Query<Entity, With<ProjectileEmitter>>,
) {
    // Authority only: a client's projectiles are despawned by the server and
    // that replicates.
    if !role.is_none_or(|role| role.is_authority()) {
        return;
    }
    for entity in projectiles.iter().chain(&emitters) {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use crate::constants::one_tick;

    use crate::common::net::NetRole;

    /// A replicated projectile never went through `launch`; without sizing from
    /// the spec at dressing time a rocket is drawn a meter wide.
    #[test]
    fn a_projectile_is_sized_from_its_spec_rather_than_its_spawn() {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.init_resource::<ProjectileAssets>();

        // As replication delivers one: the component and nothing else.
        let spec = ProjectileSpec::ROCKET;
        let rocket = app
            .world_mut()
            .spawn(Projectile {
                spec,
                owner: None,
                shooter: None,
                velocity: Vec3::ZERO,
                spin: 0.0,
                age: 0.0,
                bounces: 0,
            })
            .id();

        app.world_mut().run_system_once(dress_projectiles).unwrap();

        let scale = app.world().get::<Transform>(rocket).unwrap().scale;
        assert!(
            scale.abs_diff_eq(Vec3::splat(spec.radius), 1e-6),
            "drawn at {scale} rather than its own radius"
        );
        assert!(app.world().get::<Mesh3d>(rocket).is_some(), "it was never dressed");
    }

    /// Spin is drawn, not stepped, so a machine that flies nothing still turns
    /// a replicated pipe bomb.
    #[test]
    fn a_projectile_turns_to_the_spin_it_carries() {
        let mut world = World::new();
        let pipe = world
            .spawn((
                Projectile {
                    spec: ProjectileSpec::PIPE_BOMB,
                    owner: None,
                    shooter: None,
                    velocity: Vec3::ZERO,
                    spin: 1.1,
                    age: 0.0,
                    bounces: 0,
                },
                Transform::default(),
            ))
            .id();

        world.run_system_once(spin_projectiles).unwrap();

        assert!(
            world.get::<Transform>(pipe).unwrap().rotation.angle_between(Quat::IDENTITY) > 0.1,
            "it did not turn"
        );
    }

    /// A client despawning the server's projectiles deletes entities
    /// replication still expects to update.
    #[test]
    fn a_client_does_not_clear_the_servers_projectiles() {
        let mut world = World::new();
        world.insert_resource(NetRole::Client { host: "elsewhere".into(), port: 27100 });
        let rocket = world.spawn(Projectile {
            spec: ProjectileSpec::ROCKET,
            owner: None,
            shooter: None,
            velocity: Vec3::ZERO,
            spin: 0.0,
            age: 0.0,
            bounces: 0,
        }).id();

        world.run_system_once(clear_projectiles).unwrap();
        assert!(world.get_entity(rocket).is_ok(), "the client despawned the server's rocket");

        world.insert_resource(NetRole::Listen { port: 27100 });
        world.run_system_once(clear_projectiles).unwrap();
        assert!(world.get_entity(rocket).is_err(), "the server left its own rocket in the air");
    }

    use super::*;
    use crate::common::hitbox::Box3;
    use crate::tool::room::Room;

    fn shot_by(world: &mut World, owner: u64, at: Vec3) -> Entity {
        world
            .spawn((
                Projectile {
                    spec: ProjectileSpec::ROCKET,
                    owner: Some(PlayerId(owner)),
                    shooter: None,
                    velocity: Vec3::NEG_Z * ProjectileSpec::ROCKET.initial_speed,
                    spin: 0.0,
                    age: 0.0,
                    bounces: 0,
                },
                PhysicsBody::at(at),
                Transform::from_translation(at),
            ))
            .id()
    }

    /// Breaks if our own shot is drawn at the eye on its first frame, or if
    /// somebody else's is pulled over to our barrel.
    #[test]
    fn our_shot_is_drawn_from_the_barrel_and_nobody_elses_is() {
        let barrel = Vec3::new(0.3, -0.2, -0.6);
        let eye = Vec3::new(0.0, 0.0, -0.5);

        let mut world = World::new();
        world.insert_resource(ApparentMuzzle(Some(barrel)));
        world.spawn((LocalPlayer, PlayerId(1)));
        let ours = shot_by(&mut world, 1, eye);
        let theirs = shot_by(&mut world, 2, eye);

        world.run_system_once(start_our_shots_at_the_muzzle).unwrap();
        world.run_system_once(draw_our_shots_from_the_muzzle).unwrap();

        let drawn = |world: &World, shot| world.get::<Transform>(shot).unwrap().translation;
        assert!(drawn(&world, ours).abs_diff_eq(barrel, 1e-6), "ours drawn at {}", drawn(&world, ours));
        assert_eq!(drawn(&world, theirs), eye, "somebody else's shot left our barrel");
    }

    #[test]
    fn a_shot_is_back_on_its_real_path_once_the_ease_is_over() {
        assert_eq!(muzzle_weight(0.0, 2.0), 1.0);
        assert_eq!(muzzle_weight(2.0, 2.0), 0.0);
        assert_eq!(muzzle_weight(0.0, 0.0), 0.0, "a shot with no room to ease still drew offset");
        let samples: Vec<f32> = (0..=10).map(|i| muzzle_weight(i as f32 / 5.0, 2.0)).collect();
        assert!(samples.windows(2).all(|pair| pair[1] <= pair[0]), "{samples:?}");
    }

    /// Breaks if a rocket fired at a near wall is still drawn off its path
    /// when it reaches the wall, which reads as the blast jumping sideways.
    #[test]
    fn a_shot_at_a_near_wall_is_on_its_path_by_the_time_it_gets_there() {
        let mut world = a_world(&[Room::new(Vec3::new(-5.0, -1.0, -2.0), Vec3::new(5.0, 4.0, 5.0))]);
        world.insert_resource(ApparentMuzzle(Some(Vec3::new(0.3, 0.8, -0.6))));
        world.spawn((LocalPlayer, PlayerId(1)));
        let ours = shot_by(&mut world, 1, Vec3::new(0.0, 1.0, -0.5));

        world.run_system_once(start_our_shots_at_the_muzzle).unwrap();

        let span = world.get::<FromTheMuzzle>(ours).unwrap().span;
        let rocket = ProjectileSpec::ROCKET.initial_speed * MUZZLE_EASE;
        assert!(span < rocket, "eased over {span} m, past the wall");
        assert!(span <= 1.6, "eased over {span} m with the wall 1.5 m away");
    }

    /// Third person, or a weapon with no muzzle: nothing is offset.
    #[test]
    fn with_no_barrel_on_screen_a_shot_is_drawn_where_it_is() {
        let mut world = World::new();
        world.insert_resource(ApparentMuzzle(None));
        world.spawn((LocalPlayer, PlayerId(1)));
        let ours = shot_by(&mut world, 1, Vec3::ZERO);

        world.run_system_once(start_our_shots_at_the_muzzle).unwrap();
        assert!(world.get::<FromTheMuzzle>(ours).is_none());
    }

    /// Bevy's default 64 Hz.

    fn a_world(rooms: &[Room]) -> World {
        let mut world = World::new();
        let mut collision = CollisionWorld::default();
        collision.rebuild(rooms);
        world.insert_resource(collision);
        world.init_resource::<MovementRules>();
        world.init_resource::<Messages<Explosion>>();

        // The clock `FixedUpdate` hands its systems: one tick a step.
        let mut fixed = Time::<()>::default();
        fixed.advance_by(one_tick());
        world.insert_resource(fixed);
        world
    }

    fn a_room() -> Room {
        Room::new(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 12.0, 20.0))
    }

    fn tick(world: &mut World, steps: usize) {
        for _ in 0..steps {
            world.run_system_once(step_projectiles).unwrap();
            // Flush so a detonated projectile's despawn lands.
            world.flush();
        }
    }

    fn blasts(world: &mut World) -> Vec<Explosion> {
        let messages = world.resource::<Messages<Explosion>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    fn shoot(world: &mut World, spec: ProjectileSpec, from: Vec3, direction: Vec3) -> Entity {
        let mut commands = world.commands();
        let entity = launch(
            &mut commands,
            spec,
            from,
            Dir3::new(direction).unwrap(),
            Some(PlayerId(4)),
            None,
        );
        world.flush();
        entity
    }

    fn body(world: &mut World, center: Vec3) -> Entity {
        world
            .spawn(Hitboxes {
                body: Box3 { center, half_extents: Vec3::new(0.4, 0.9, 0.4) },
                head: Some(Box3 { center: center + Vec3::Y * 0.8, half_extents: Vec3::splat(0.15) }),
            })
            .id()
    }

    /// A flat rocket goes off on the far wall, crediting its shooter.
    #[test]
    fn a_rocket_flies_until_it_meets_a_wall_and_goes_off_there() {
        let mut world = a_world(&[a_room()]);
        let rocket = shoot(&mut world, ProjectileSpec::ROCKET, Vec3::new(0.0, 2.0, 10.0), Vec3::NEG_Z);

        tick(&mut world, 64 * 2);

        assert!(world.get::<Projectile>(rocket).is_none(), "the rocket is still flying");
        let went_off = blasts(&mut world);
        assert_eq!(went_off.len(), 1, "{went_off:?}");
        assert!(went_off[0].at.z < -19.0, "it went off at {}", went_off[0].at);
        assert!((went_off[0].at.y - 2.0).abs() < 1e-3, "a flat rocket fell: {}", went_off[0].at);
        assert_eq!(went_off[0].source, DamageSource::Player(PlayerId(4)));
        assert_eq!(went_off[0].direct, None, "a wall is not a direct hit");
    }

    /// A body in the way takes the direct number and stops the rocket there.
    #[test]
    fn a_body_in_the_way_takes_a_direct_hit() {
        let mut world = a_world(&[a_room()]);
        let target = body(&mut world, Vec3::new(0.0, 2.0, 0.0));
        shoot(&mut world, ProjectileSpec::ROCKET, Vec3::new(0.0, 2.0, 10.0), Vec3::NEG_Z);

        tick(&mut world, 64);

        let went_off = blasts(&mut world);
        assert_eq!(went_off.len(), 1, "{went_off:?}");
        assert_eq!(
            went_off[0].direct,
            Some((target, ProjectileSpec::ROCKET.damage_direct_hit))
        );
        assert!(went_off[0].at.z > -1.0, "it flew past the body to {}", went_off[0].at);
    }

    /// The muzzle is inside the shooter's hitbox; a projectile that could hit
    /// its owner would detonate on firing.
    #[test]
    fn a_projectile_never_hits_the_body_that_fired_it() {
        let mut world = a_world(&[a_room()]);
        let shooter = body(&mut world, Vec3::new(0.0, 2.0, 10.0));
        let mut commands = world.commands();
        launch(
            &mut commands,
            ProjectileSpec::ROCKET,
            Vec3::new(0.0, 2.0, 10.0),
            Dir3::NEG_Z,
            Some(PlayerId(4)),
            Some(shooter),
        );
        world.flush();

        tick(&mut world, 4);

        assert!(blasts(&mut world).is_empty(), "the rocket went off in its own shooter");
    }

    /// A wall stops a rocket short of a body behind it.
    #[test]
    fn a_wall_stops_a_rocket_short_of_the_body_behind_it() {
        let mut world = a_world(&[Room::new(Vec3::new(-4.0, 0.0, -4.0), Vec3::new(4.0, 6.0, 4.0))]);
        let sheltered = body(&mut world, Vec3::new(0.0, 2.0, -10.0));
        shoot(&mut world, ProjectileSpec::ROCKET, Vec3::new(0.0, 2.0, 3.0), Vec3::NEG_Z);

        tick(&mut world, 64);

        let went_off = blasts(&mut world);
        assert_eq!(went_off.len(), 1);
        assert_eq!(went_off[0].direct, None, "the rocket hit {sheltered} through a wall");
        assert!(went_off[0].at.z > -5.0, "it went off past the wall at {}", went_off[0].at);
    }

    /// A pipe bomb bounces off the floor and keeps flying.
    #[test]
    fn a_pipe_bomb_bounces_off_the_floor_instead_of_going_off_on_it() {
        let mut world = a_world(&[a_room()]);
        let pipe = shoot(
            &mut world,
            ProjectileSpec::PIPE_BOMB,
            Vec3::new(0.0, 1.5, 10.0),
            Vec3::new(0.0, 0.1, -1.0),
        );

        // Long enough to come down, well short of the fuse.
        tick(&mut world, 40);

        let flying = world.get::<Projectile>(pipe).expect("the pipe bomb was destroyed by the floor");
        assert!(flying.bounces >= 1, "it never bounced");
        assert!(blasts(&mut world).is_empty(), "it went off on the floor");
        assert!(
            world.get::<PhysicsBody>(pipe).unwrap().current.y > 0.0,
            "it fell through the floor"
        );
    }

    /// A pipe bomb goes off on its fuse. In an empty world, since in a room it
    /// runs out of bounces before it runs out of time.
    #[test]
    fn a_pipe_bomb_goes_off_when_its_fuse_runs_out() {
        let mut world = a_world(&[]);
        shoot(
            &mut world,
            ProjectileSpec::PIPE_BOMB,
            Vec3::new(0.0, 1.5, 10.0),
            Vec3::new(0.0, 0.2, -1.0),
        );

        let fuse = (ProjectileSpec::PIPE_BOMB.max_lifetime * 64.0).ceil() as usize;
        tick(&mut world, fuse - 2);
        assert!(blasts(&mut world).is_empty(), "it went off early");

        tick(&mut world, 4);
        assert_eq!(blasts(&mut world).len(), 1, "the fuse never ran out");
    }

    /// A rocket that outlives its lifetime without touching anything vanishes;
    /// it has no fuse.
    #[test]
    fn a_rocket_that_runs_out_of_time_does_not_go_off() {
        let mut world = a_world(&[]);
        let rocket = shoot(&mut world, ProjectileSpec::ROCKET, Vec3::ZERO, Vec3::NEG_Z);

        tick(&mut world, (ProjectileSpec::ROCKET.max_lifetime * 64.0).ceil() as usize + 1);

        assert!(world.get::<Projectile>(rocket).is_none(), "it is still out there");
        assert!(blasts(&mut world).is_empty(), "a rocket grew a fuse");
    }

    /// A projectile fired into a corner must not hang the step.
    #[test]
    fn a_projectile_fired_into_a_corner_still_finishes_its_tick() {
        let mut world = a_world(&[Room::new(Vec3::splat(-2.0), Vec3::new(2.0, 4.0, 2.0))]);
        shoot(
            &mut world,
            ProjectileSpec { max_bounces: 1000, ..ProjectileSpec::PIPE_BOMB },
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
        );

        tick(&mut world, 64);
    }

    /// An emitter fires once per interval and no more often.
    #[test]
    fn an_emitter_fires_on_its_interval() {
        let mut world = a_world(&[a_room()]);
        world.spawn((
            ProjectileEmitter::new(ProjectileSpec::ROCKET, 2.0),
            Transform::from_xyz(0.0, 2.0, 10.0),
            GlobalTransform::from_xyz(0.0, 2.0, 10.0),
        ));

        let count = |world: &mut World| world.query::<&Projectile>().iter(world).count();

        for _ in 0..64 {
            world.run_system_once(run_emitters).unwrap();
            world.flush();
        }
        assert_eq!(count(&mut world), 0, "it fired inside its own interval");

        for _ in 0..70 {
            world.run_system_once(run_emitters).unwrap();
            world.flush();
        }
        assert_eq!(count(&mut world), 1, "the interval came and went");
    }
}
