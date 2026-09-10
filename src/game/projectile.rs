//! Things that are thrown rather than traced: rockets, pipe bombs, and
//! whatever else [`ProjectileSpec`] can be made to describe.
//!
//! One entity, one system, one spec. What the projectile *is* lives in
//! [`crate::common::projectile`] as numbers and arithmetic with no `World` in
//! them; what lives here is the part that has to ask the world questions —
//! where the walls are, whose body is in the way, and who to credit.
//!
//! **The move is swept, like the player's.** A rocket covers half a metre in a
//! tick and a wall slab is half a metre thick, so a projectile teleported to
//! its new position and then asked whether it overlaps anything would pass
//! through the first thin thing it met at speed. It is traced instead, along
//! the segment it would cover, against the walls and the hitboxes at once and
//! whichever is nearer wins. That reuses the two functions hit registration
//! already uses — [`trace`] for bodies, [`CollisionWorld::ray_hit`] for walls
//! — so "what can a rocket hit" and "what can a bullet hit" cannot drift
//! apart.
//!
//! **Bounces are resolved inside the step**, not one per tick: a pipe bomb
//! thrown into a corner meets two walls in the same 15 milliseconds, and
//! taking one contact per tick would sink it into the second. The loop is
//! bounded — a projectile wedged in a corner must not spin here forever — and
//! whatever motion is left when the bound is reached is dropped.
//!
//! **A projectile never hits the body that fired it.** Not a friendly-fire
//! rule — there are no teams yet, and the splash from your own rocket hurts
//! you like anyone else's — but a muzzle inside your own hitbox, which would
//! otherwise detonate every shot at zero metres.
//!
//! Everything a hit leads to leaves as an [`Explosion`], so this module knows
//! nothing about splash, falloff, health or corpses. See
//! [`crate::game::explosion`].
//!
//! Position goes through [`PhysicsBody`] rather than straight into
//! `Transform`, which buys interpolation for free — `interpolate_bodies` takes
//! every `PhysicsBody` there is — and matters more here than for a player: a
//! rocket at 30 m/s moves half a metre between ticks, and drawn at the tick it
//! would visibly stutter across the room. Rotation is this module's own, since
//! nothing else writes a projectile's.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::damage::{DamageSource, Explosion, PlayerId};
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::trace;
use crate::common::projectile::{bounce, fly, Ending, ProjectileSpec};
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::player::{PhysicsBody, Player, GRAVITY};
use crate::game::weapon::{Loadout, TriggerPulled, Weapon};

/// How many walls one projectile may meet in a single tick.
///
/// Four is a corner and then some. A projectile that has used them all keeps
/// whatever velocity the last bounce gave it and simply stops travelling for
/// the rest of the tick, which is a fraction of a millisecond of lost motion
/// rather than a projectile that tunnels.
const MAX_CONTACTS: usize = 4;

/// How far off a wall a bounce leaves the projectile, in metres.
///
/// The same trick [`CollisionWorld::depenetrate`] uses: land exactly on a
/// plane and the next trace of the same tick finds it at zero distance and
/// bounces off it again, burning every contact in the corner it is not in.
const SKIN: f32 = 0.002;

/// How far in front of the eye a projectile is born, in metres.
///
/// Far enough to be outside the shooter's own body, so that the first thing it
/// traces against is the room rather than the inside of a chest. The shooter
/// is excluded from its own direct hits as well; this is what stops it being
/// born inside a *wall* when you fire with your face against one.
const MUZZLE_OFFSET: f32 = 0.5;

/// Something in flight.
///
/// Everything mutable about it. The spec is copied onto it rather than looked
/// up from the weapon, because a rocket that is already in the air was fired
/// by the launcher as it was then: switching weapons, or an item that changes
/// a number, must not reach back and change a shot that has left.
#[derive(Component, Clone, Debug)]
pub struct Projectile {
    pub spec: ProjectileSpec,
    /// Who gets the kill. `None` for anything the map threw — see
    /// [`DamageSource::World`].
    pub owner: Option<PlayerId>,
    /// The body it came out of, which its own direct hits ignore.
    pub shooter: Option<Entity>,
    pub velocity: Vec3,
    /// How far it has tumbled, in radians. Cosmetic.
    pub spin: f32,
    /// Seconds of game time in the air.
    pub age: f32,
    pub bounces: u32,
}

impl Projectile {
    /// Who to credit for what this does.
    pub fn source(&self) -> DamageSource {
        match self.owner {
            Some(id) => DamageSource::Player(id),
            None => DamageSource::World,
        }
    }
}

/// A thing that fires a projectile every so often, at nobody in particular.
///
/// The test harness for all of this, and deliberately a component rather than
/// a hard-coded prop: planted in front of the animation grid it turns sixty
/// standing bodies into a firing range, and it is the only way to watch a
/// pipe bomb bounce the same way twice. Plant one with `G` and clear them
/// with `B` — see [`plant_emitters`].
#[derive(Component, Clone, Debug)]
pub struct ProjectileEmitter {
    pub spec: ProjectileSpec,
    /// Seconds between shots.
    pub interval: f32,
    /// Seconds since the last one. Starts at zero so a freshly planted
    /// emitter waits a full interval — long enough for its transform to have
    /// been propagated, and for whoever planted it to get out of the way.
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
            // In `Deal` with the other weapons: what these systems write is
            // `Explosion`, and `explode` turns that into `Damage` in the same
            // set, in the same tick.
            .add_systems(
                FixedUpdate,
                (fire_projectiles, run_emitters, step_projectiles)
                    .in_set(DamageSystems::Deal),
            )
            .add_systems(Update, plant_emitters.run_if(in_state(AppMode::Play)))
            .add_systems(Update, dress_projectiles)
            .init_resource::<ProjectileAssets>()
            // Everything in flight belongs to the match, like the corpses and
            // the damage numbers. A rocket left hanging in the editor would be
            // a prop nothing owns.
            .add_systems(OnExit(AppMode::Play), clear_projectiles)
        ;
    }
}

/// Put a projectile in the air.
///
/// A function rather than only a system, because three things launch one — a
/// player, an emitter, and a test — and the only thing they disagree about is
/// where it starts and who to blame.
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

/// Fire, for every shooter whose trigger was pulled and who is holding
/// something that travels.
///
/// The mirror of [`fire_hitscan`](crate::game::hitscan::fire_hitscan), down to
/// firing from the *step's* own position rather than the drawn one: a rocket
/// launched from an interpolated body would leave a different place on every
/// machine.
pub fn fire_projectiles(
    mut commands: Commands,
    mut pulled: MessageReader<TriggerPulled>,
    shooters: Query<(&PhysicsBody, &Player, &Stance, Option<&PlayerId>, &Loadout)>,
) {
    for shot in pulled.read() {
        let Ok((body, player, stance, id, loadout)) = shooters.get(shot.shooter) else { continue };
        let Some(Weapon::Projectile(spec)) = loadout.held() else { continue };

        let direction = crate::game::hitscan::aim(player);
        let muzzle = crate::game::hitscan::eye(body.current, *stance) + *direction * MUZZLE_OFFSET;
        launch(&mut commands, spec, muzzle, direction, id.copied(), Some(shot.shooter));
    }
}

/// Every emitter that is due, fired.
///
/// Game time, like everything else on the tick: an emitter does not fire while
/// somebody is in the editor, and two runs of the same map put the same number
/// of rockets in the air.
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
            // Nobody fired it. A kill it lands is the map's, which is exactly
            // what `DamageSource::World` is for.
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

    for (entity, mut projectile, mut body, mut transform) in &mut projectiles {
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

            // Your own body is around your own muzzle, and every shot would
            // land on it at zero metres.
            let others = bodies
                .iter()
                .filter(|(target, _)| Some(*target) != projectile.shooter)
                .map(|(target, boxes)| (target, *boxes));

            let body_hit = trace(&ray, distance, others);
            let wall = world.ray_hit(&ray, distance);

            // Nearest wins. A rocket that passes a wall to reach a body it
            // could not see would be a rocket that shoots through walls.
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
                // Nothing in the way: the whole step is travelled.
                position += travel;
                break;
            };

            let contact = ray.origin + *direction * reached;
            if projectile.bounces >= spec.max_bounces {
                // Stood off the wall by its own radius, so the blast goes off
                // in the room rather than inside the slab, where the sight
                // check would find the wall between it and everything.
                position = contact + normal * spec.radius;
                ending = Some((Ending::LastBounce, None));
                break;
            }

            projectile.bounces += 1;
            projectile.velocity = bounce(projectile.velocity, normal, spec.restitution);
            position = contact + normal * SKIN;
            // What is left of the step, along the new direction. The speed it
            // lost to the bounce shows up in the next tick rather than in the
            // remainder of this one — a tick's worth of a difference nobody
            // can see, against a much fiddlier sum.
            travel = projectile.velocity.normalize_or_zero() * (distance - reached).max(0.0);
        }

        // The fuse is checked after the move, so a pipe bomb that reaches its
        // time part way through a tick still travelled that part.
        if ending.is_none() && projectile.age >= spec.max_lifetime {
            ending = Some((Ending::Fuse, None));
        }

        body.previous = body.current;
        body.current = position;
        // Not the translation: `interpolate_bodies` owns that, and writing it
        // here would be overwritten and would skip the interpolation.
        transform.rotation = Quat::from_rotation_x(projectile.spin)
            * Quat::from_rotation_y(projectile.spin * 0.5);

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

        // Whether or not it went off, its flight is over. A rocket that does
        // not explode on contact still stops on the wall it hit.
        commands.entity(entity).despawn();
    }
}

/// The mesh and material every projectile is drawn with.
///
/// One of each, scaled per projectile by the transform: they differ by radius
/// and nothing else, and a handle per weapon would be a handle per number.
/// Filled on first use rather than at startup, so nothing here needs the
/// renderer to have been set up before the plugin was added.
#[derive(Resource, Default)]
struct ProjectileAssets {
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
}

/// Give anything in flight something to look at.
///
/// A sphere, and unashamedly a placeholder: a projectile's *shape* is art, and
/// what this layer owes is a thing you can see coming.
fn dress_projectiles(
    mut commands: Commands,
    mut assets: ResMut<ProjectileAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    undressed: Query<Entity, (With<Projectile>, Without<Mesh3d>)>,
) {
    if undressed.is_empty() {
        return;
    }

    // A unit sphere, because the transform carries the radius.
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

    for entity in &undressed {
        commands.entity(entity).insert((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone())));
    }
}

/// Plant an emitter where you are standing, or take them all away again.
///
/// A debug tool, and the one thing in this module that reads the keyboard
/// directly rather than going through [`PlayerInput`](crate::game::player::PlayerInput).
/// That is deliberate: the latch exists so that the *simulation* sees a press
/// exactly once, and planting a prop is not part of the simulation. Nothing
/// downstream of it has to be reproducible.
fn plant_emitters(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    players: Query<(&Transform, &Player, &Loadout)>,
    emitters: Query<Entity, With<ProjectileEmitter>>,
) {
    /// Seconds between an emitter's shots. Long enough to watch one flight
    /// end before the next begins.
    const EMITTER_INTERVAL: f32 = 2.0;

    if keys.just_pressed(KeyCode::KeyB) {
        for emitter in &emitters {
            commands.entity(emitter).despawn();
        }
    }

    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }

    for (transform, player, loadout) in &players {
        // Whatever you are holding, so choosing what an emitter fires is
        // choosing a weapon and pressing G.
        let Some(Weapon::Projectile(spec)) = loadout.held() else {
            info!("Hold a projectile weapon (2, 3 or 4) to plant an emitter");
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

/// Take everything in flight, and every emitter, away.
///
/// Called on leaving Play and again from
/// [`reset_for_play`](crate::game::reset::reset_for_play) — the same belt and
/// braces the corpses and the damage numbers get.
pub fn clear_projectiles(
    mut commands: Commands,
    projectiles: Query<Entity, With<Projectile>>,
    emitters: Query<Entity, With<ProjectileEmitter>>,
) {
    for entity in projectiles.iter().chain(&emitters) {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use std::time::Duration;

    use super::*;
    use crate::common::hitbox::Box3;
    use crate::tool::room::Room;

    /// One fixed step at Bevy's 64 Hz default.
    const STEP: Duration = Duration::from_micros(15625);

    fn a_world(rooms: &[Room]) -> World {
        let mut world = World::new();
        let mut collision = CollisionWorld::default();
        collision.rebuild(rooms);
        world.insert_resource(collision);
        world.init_resource::<Messages<Explosion>>();

        let mut fixed = Time::<Fixed>::default();
        fixed.advance_by(STEP);
        world.insert_resource(fixed);
        world
    }

    fn a_room() -> Room {
        Room::new(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 12.0, 20.0))
    }

    /// Take whole ticks of the step, the way the fixed loop would.
    fn tick(world: &mut World, steps: usize) {
        for _ in 0..steps {
            world.run_system_once(step_projectiles).unwrap();
            // `Commands` are queued, and a despawn that never landed would
            // leave a detonated projectile flying for the rest of the test.
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

    /// A rocket flies flat down the room and goes off on the wall at the end
    /// of it, crediting whoever fired it.
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

    /// A body in the way is a direct hit, worth the direct number, and the
    /// rocket stops there rather than at the wall behind it.
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

    /// The muzzle is inside the shooter's own hitbox, and a projectile that
    /// could hit its owner would detonate the instant it was fired.
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

    /// A wall stops a rocket before a body behind it: the same rule a bullet
    /// obeys, from the same two functions.
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

    /// A pipe bomb arcs, bounces off the floor rather than stopping on it, and
    /// is still in the air afterwards.
    #[test]
    fn a_pipe_bomb_bounces_off_the_floor_instead_of_going_off_on_it() {
        let mut world = a_world(&[a_room()]);
        let pipe = shoot(
            &mut world,
            ProjectileSpec::PIPE_BOMB,
            Vec3::new(0.0, 1.5, 10.0),
            Vec3::new(0.0, 0.1, -1.0),
        );

        // Long enough to come down, and well short of the fuse.
        tick(&mut world, 40);

        let flying = world.get::<Projectile>(pipe).expect("the pipe bomb was destroyed by the floor");
        assert!(flying.bounces >= 1, "it never bounced");
        assert!(blasts(&mut world).is_empty(), "it went off on the floor");
        assert!(
            world.get::<PhysicsBody>(pipe).unwrap().current.y > 0.0,
            "it fell through the floor"
        );
    }

    /// And it goes off on its fuse wherever it happens to be, which is the
    /// other half of what makes it a pipe bomb rather than a rocket.
    ///
    /// In an empty world, so that nothing it bounces off gets to end the
    /// flight first — a pipe bomb thrown around a room runs out of bounces
    /// well before it runs out of time, which is
    /// [`a_pipe_bomb_bounces_off_the_floor_instead_of_going_off_on_it`]'s
    /// business rather than this one's.
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

    /// A rocket that somehow flies for its whole lifetime without touching
    /// anything simply goes away — it has no fuse.
    #[test]
    fn a_rocket_that_runs_out_of_time_does_not_go_off() {
        let mut world = a_world(&[]);
        let rocket = shoot(&mut world, ProjectileSpec::ROCKET, Vec3::ZERO, Vec3::NEG_Z);

        tick(&mut world, (ProjectileSpec::ROCKET.max_lifetime * 64.0).ceil() as usize + 1);

        assert!(world.get::<Projectile>(rocket).is_none(), "it is still out there");
        assert!(blasts(&mut world).is_empty(), "a rocket grew a fuse");
    }

    /// The bound on contacts per tick: a projectile fired straight into a
    /// corner must not hang the step, however many walls it finds.
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

    /// An emitter is the harness: left alone, it puts one in the air every
    /// interval and no more often.
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
