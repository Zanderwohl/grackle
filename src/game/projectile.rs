//! Things that are thrown rather than traced: rockets, pipe bombs, and
//! whatever else [`ProjectileSpec`] describes. The arithmetic is in
//! [`crate::common::projectile`]; this is the part that asks the world.
//!
//! The move is swept against walls and hitboxes at once, nearer wins: a
//! rocket covers half a metre a tick and a wall slab is half a metre thick, so
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
use crate::common::projectile::{bounce, fly, Ending, ProjectileSpec};
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::player::{PhysicsBody, Player, GRAVITY};
use crate::common::weapon::{Equipped, WeaponAction, WeaponCatalogue};
use crate::game::weapon::WeaponActionFired;

/// Walls one projectile may meet in a single tick. Past it the rest of the
/// tick's motion is dropped rather than risking a tunnel.
const MAX_CONTACTS: usize = 4;

/// Metres a bounce leaves the projectile off the wall. Landing exactly on the
/// plane, the next trace in the same tick finds it at zero distance and
/// bounces again, burning every contact.
const SKIN: f32 = 0.002;

/// Metres in front of the eye a projectile is born: outside the shooter's
/// body, and not inside a wall when firing with your face against one.
const MUZZLE_OFFSET: f32 = 0.5;

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

/// Fires a projectile every `interval` at nobody in particular. A test
/// harness: `G` plants one, `B` clears them (see [`plant_emitters`]).
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
            .add_systems(
                FixedUpdate,
                (fire_projectiles, run_emitters, step_projectiles)
                    .in_set(DamageSystems::Deal),
            )
            .add_systems(Update, plant_emitters.run_if(in_state(AppMode::Play)))
            // Drawing, so ungated: a client flies none of these and draws all.
            .add_systems(Update, (dress_projectiles, spin_projectiles))
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
            Projectile {
                spec,
                owner,
                shooter,
                velocity: *direction * spec.initial_speed,
                spin: 0.0,
                age: 0.0,
                bounces: 0,
            },
            PhysicsBody::at(from),
            Transform::from_translation(from).with_scale(Vec3::splat(spec.radius)),
            Visibility::default(),
            Name::new("Projectile"),
        ))
        .id()
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
        let muzzle = crate::game::hitscan::eye(body.current, *stance) + *direction * MUZZLE_OFFSET;
        launch(&mut commands, spec, muzzle, direction, id.copied(), Some(shot.shooter));
    }
}

/// Game time, so an emitter does not fire while somebody is in the editor.
pub fn run_emitters(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
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
    time: Res<Time<Fixed>>,
    world: Res<CollisionWorld>,
    mut projectiles: Query<(Entity, &mut Projectile, &mut PhysicsBody, &mut Transform)>,
    bodies: Query<(Entity, &Hitboxes)>,
    mut blasts: MessageWriter<Explosion>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (entity, mut projectile, mut body, _transform) in &mut projectiles {
        projectile.age += dt;
        projectile.spin += projectile.spec.spin_rate * dt;

        let spec = projectile.spec;
        let (velocity, step) = fly(projectile.velocity, &spec, GRAVITY, dt);
        projectile.velocity = velocity;

        let mut position = body.current;
        let mut travel = step;
        let mut ending = None;

        for _ in 0..MAX_CONTACTS {
            let distance = travel.length();
            let Ok(direction) = Dir3::new(travel) else { break };
            let ray = Ray3d::new(position, direction);

            // The muzzle is inside your own hitbox.
            let others = bodies
                .iter()
                .filter(|(target, _)| Some(*target) != projectile.shooter)
                .map(|(target, boxes)| (target, *boxes));

            let body_hit = trace(&ray, distance, others);
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

        // Checked after the move, so a fuse ending mid-tick still travelled.
        if ending.is_none() && projectile.age >= spec.max_lifetime {
            ending = Some((Ending::Fuse, None));
        }

        body.previous = body.current;
        body.current = position;

        let Some((ending, direct)) = ending else { continue };

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
    undressed: Query<(Entity, &Projectile), Without<Mesh3d>>,
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

    for (entity, projectile) in &undressed {
        // From the spec: a replicated projectile never went through `launch`,
        // so its transform starts at unit scale.
        commands.entity(entity).insert((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_scale(Vec3::splat(projectile.spec.radius)),
        ));
    }
}

/// `G` plants an emitter where you stand, `B` removes them all. Reads the
/// keyboard directly: planting a prop is not part of the simulation, so it
/// needs no latch.
fn plant_emitters(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    catalogue: Res<WeaponCatalogue>,
    players: Query<(&Transform, &Player, &Equipped)>,
    emitters: Query<Entity, With<ProjectileEmitter>>,
) {
    /// Seconds. Long enough to watch one flight end before the next.
    const EMITTER_INTERVAL: f32 = 2.0;

    if keys.just_pressed(KeyCode::KeyB) {
        for emitter in &emitters {
            commands.entity(emitter).despawn();
        }
    }

    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }

    for (transform, player, equipped) in &players {
        // Whatever you hold, primary only: an emitter has no right button.
        let held = catalogue.held_by(equipped).map(|weapon| weapon.primary.action);
        let Some(WeaponAction::Projectile(spec)) = held else {
            info!("Hold a projectile weapon to plant an emitter");
            continue;
        };
        let facing = Quat::from_rotation_y(player.yaw) * Quat::from_rotation_x(player.pitch);
        commands.spawn((
            ProjectileEmitter::new(spec, EMITTER_INTERVAL),
            Transform::from_translation(transform.translation).with_rotation(facing),
            Visibility::default(),
            Name::new("Projectile emitter"),
        ));
        info!("Planted an emitter firing every {EMITTER_INTERVAL} s; B clears them");
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
    /// the spec at dressing time a rocket is drawn a metre wide.
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

    /// Bevy's default 64 Hz.

    fn a_world(rooms: &[Room]) -> World {
        let mut world = World::new();
        let mut collision = CollisionWorld::default();
        collision.rebuild(rooms);
        world.insert_resource(collision);
        world.init_resource::<Messages<Explosion>>();

        let mut fixed = Time::<Fixed>::default();
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

    fn body(world: &mut World, centre: Vec3) -> Entity {
        world
            .spawn(Hitboxes {
                body: Box3 { centre, half_extents: Vec3::new(0.4, 0.9, 0.4) },
                head: Box3 { centre: centre + Vec3::Y * 0.8, half_extents: Vec3::splat(0.15) },
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
