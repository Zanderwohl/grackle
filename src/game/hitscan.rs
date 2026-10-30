//! A debug laser out of the player's eye, a ball where it lands, and hitscan
//! firing.
//!
//! The laser runs at frame rate from the interpolated body; the shot runs on
//! the tick from the fixed step's position, so it is reproducible across
//! machines. The two therefore start a fraction apart. Walls are handled by
//! shortening the range with [`CollisionWorld::ray_distance`] before
//! [`crate::common::hitscan::trace`] tests the bodies.

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::effects::Effect;
use crate::common::damage::{Damage, DamageSource, KillShove, PlayerId};
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::{trace, Hit, HitZone};
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::hitbox::update_hitboxes;
use crate::game::player::{step_player, Duck, LocalPlayer, PhysicsBody, Player};
use crate::game::viewmodel::ApparentMuzzle;
use crate::game::ragdoll::{Push, RagdollShove};
use crate::common::weapon::{HitscanSpec, WeaponAction};
use crate::game::weapon::WeaponActionFired;

/// How far the debug laser reaches, in meters.
pub const LASER_RANGE: f32 = 200.0;

/// Seconds. Short enough that a stream of tracers does not read as one beam.
const TRACER_LIFETIME: f32 = 0.12;

/// Radius of the ball drawn at a hit, in meters.
pub const HIT_BALL_RADIUS: f32 = 0.1;

pub struct HitscanPlugin;

impl Plugin for HitscanPlugin {
    fn build(&self, app: &mut App) {
        app
            .add_systems(
                FixedUpdate,
                fire_hitscan
                    .after(update_hitboxes)
                    .after(step_player)
                    .in_set(DamageSystems::Deal),
            )
            // Ungated: a client fires nothing but has to draw every shot.
            .add_systems(Update, (mark_tracers, fade_tracers))
            .add_systems(
                PostUpdate,
                (
                    // With the boxes it is aimed at, and off with them: it
                    // traces every player's aim against every body, every
                    // frame, which is a lot to pay for a line nobody asked for.
                    draw_debug_laser.run_if(|show: Res<crate::game::hitbox::ShowHitboxes>| show.0),
                    draw_tracers,
                )
                    .after(TransformSystems::Propagate)
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), clear_tracers)
        ;
    }
}

/// Which way the player is looking, in world space.
///
/// From yaw and pitch rather than the camera, because in third person the
/// camera is not at the eye.
pub(crate) fn aim(player: &Player) -> Dir3 {
    Dir3::new(looking(player) * Vec3::NEG_Z).unwrap_or(Dir3::NEG_Z)
}

fn looking(player: &Player) -> Quat {
    Quat::from_rotation_y(player.yaw) * Quat::from_rotation_x(player.pitch)
}

/// Where the eye is, given a body's center. Takes a center because the laser
/// passes the interpolated transform and the shot the fixed step's position.
pub(crate) fn eye(center: Vec3, stance: Stance, duck: Duck) -> Vec3 {
    center + Vec3::Y * duck.eye_from_center(stance)
}

/// The nearest body `ray` reaches other than `shooter`, whose own boxes
/// surround the eye the ray starts at.
pub(crate) fn trace_past(
    ray: &Ray3d,
    range: f32,
    shooter: Entity,
    targets: &Query<(Entity, &Hitboxes)>,
) -> Option<Hit<Entity>> {
    let others = targets
        .iter()
        .filter(|(entity, _)| *entity != shooter)
        .map(|(entity, boxes)| (entity, *boxes));
    trace(ray, range, others)
}

fn damage_for(zone: HitZone, spec: &HitscanSpec) -> u32 {
    match zone {
        HitZone::Body => spec.damage,
        HitZone::Head => spec.damage * spec.headshot_multiplier,
    }
}

/// One shot per [`WeaponActionFired`] carrying a hitscan action, of one ray
/// or of a [`PelletGrid`](crate::common::weapon::PelletGrid)'s.
pub fn fire_hitscan(
    mut fired: MessageReader<WeaponActionFired>,
    world: Res<CollisionWorld>,
    shooters: Query<(&PhysicsBody, &Player, &Stance, Option<&PlayerId>)>,
    targets: Query<(Entity, &Hitboxes)>,
    mut damage: MessageWriter<Damage>,
    mut shoves: MessageWriter<RagdollShove>,
    mut effects: MessageWriter<Effect>,
) {
    for shot in fired.read() {
        // The action comes from the message, not the shooter's loadout, so a
        // switch in the same tick cannot change this shot.
        let WeaponAction::Hitscan(spec) = shot.action else { continue };
        let Ok((body, player, stance, id)) = shooters.get(shot.shooter) else { continue };

        // The step's position, not `Transform`, which is interpolated at
        // frame rate and differs per machine.
        let origin = eye(body.current, *stance, player.duck);
        let looking = looking(player);
        let offsets = spec.pellets.map_or_else(|| vec![Vec2::ZERO], |grid| grid.offsets());
        let source = id.map_or(DamageSource::World, |id| DamageSource::Player(*id));
        // Along the aim rather than any one pellet, so a fatal blast throws a
        // body straight back whichever pellets landed.
        let kill_shove = spec.kill_shove.map(|shove| KillShove {
            velocity: looking * Vec3::NEG_Z * shove.speed + Vec3::Y * shove.lift,
            radius: shove.radius_m,
        });

        // One `Damage` per body per shot, not per pellet, so a blast at point
        // blank is one number and one flinch.
        let mut dealt: Vec<Damage> = Vec::new();

        for offset in offsets {
            let direction =
                Dir3::new(looking * Vec3::new(offset.x, offset.y, -1.0)).unwrap_or(Dir3::NEG_Z);
            let ray = Ray3d::new(origin, direction);
            let range = world.ray_distance(&ray, spec.range_m).unwrap_or(spec.range_m);

            let hit = trace_past(&ray, range, shot.shooter, &targets);

            // Before the `continue`: a miss still draws a tracer.
            effects.write(Effect::Tracer {
                from: ray.origin,
                to: hit.map_or(ray.origin + *ray.direction * range, |hit| hit.point),
                hit: hit.is_some(),
                by: id.copied(),
            });

            let Some(hit) = hit else { continue };

            let worth = damage_for(hit.zone, &spec);
            match dealt.iter_mut().find(|asked| asked.target == hit.target) {
                Some(asked) => asked.amount += worth,
                None => dealt.push(Damage {
                    target: hit.target,
                    source,
                    amount: worth,
                    point: hit.point,
                    from: Some(ray.origin),
                    kill_shove,
                }),
            }

            // Written for every hit, fatal or not. Scaled by what the pellet
            // was worth rather than what it took off, so a killing shot shoves
            // as hard as any other. The radius is tight so the hit bone moves
            // and the joints drag the rest.
            shoves.write(RagdollShove {
                at: hit.point,
                push: Push::Along(*ray.direction * (worth as f32 * spec.shove_per_damage)),
                radius: spec.shove_radius_m,
            });
        }

        damage.write_batch(dealt);
    }
}

/// An entity rather than a gizmo call in `fire_hitscan`, because firing is on
/// the tick and a gizmo written there would be drawn for zero or one frames.
#[derive(Component)]
pub struct Tracer {
    from: Vec3,
    to: Vec3,
    hit: bool,
    remaining: f32,
}

/// Our own shot starts at the barrel in first person rather than at the eye,
/// where it is a dot on the crosshair. For its whole life, unlike a
/// projectile's: it is gone before an ease would show.
fn mark_tracers(
    mut commands: Commands,
    mut effects: MessageReader<Effect>,
    muzzle: Option<Res<ApparentMuzzle>>,
    us: Query<&PlayerId, With<LocalPlayer>>,
) {
    let barrel = muzzle.and_then(|muzzle| muzzle.0);
    let us = us.single().ok().copied();
    for effect in effects.read() {
        let Effect::Tracer { from, to, hit, by } = *effect else { continue };
        let from = match barrel {
            Some(barrel) if by.is_some() && by == us => barrel,
            _ => from,
        };
        commands.spawn((
            Tracer { from, to, hit, remaining: TRACER_LIFETIME },
            Name::new("Tracer"),
        ));
    }
}

fn fade_tracers(mut commands: Commands, time: Res<Time>, mut tracers: Query<(Entity, &mut Tracer)>) {
    for (entity, mut tracer) in &mut tracers {
        tracer.remaining -= time.delta_secs();
        if tracer.remaining <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

fn draw_tracers(mut gizmos: Gizmos, tracers: Query<&Tracer>) {
    for tracer in &tracers {
        let fade = (tracer.remaining / TRACER_LIFETIME).clamp(0.0, 1.0);
        gizmos.line(
            tracer.from,
            tracer.to,
            Color::srgba(1.0, 0.85, 0.4, fade),
        );
        if tracer.hit {
            gizmos.sphere(
                Isometry3d::from_translation(tracer.to),
                HIT_BALL_RADIUS,
                Color::srgba(1.0, 0.4, 0.2, fade),
            );
        }
    }
}

fn clear_tracers(mut commands: Commands, tracers: Query<Entity, With<Tracer>>) {
    for tracer in &tracers {
        commands.entity(tracer).despawn();
    }
}

fn draw_debug_laser(
    mut gizmos: Gizmos,
    world: Res<CollisionWorld>,
    fixed: Res<Time<Fixed>>,
    shooters: Query<(Entity, &Transform, &Player, &Stance)>,
    targets: Query<(Entity, &Hitboxes)>,
) {
    let beam_color = Color::srgb(0.35, 0.8, 1.0);
    let body_color = Color::srgb(0.2, 0.95, 0.35);
    let head_color = Color::srgb(1.0, 0.15, 0.15);

    for (shooter, transform, player, stance) in &shooters {
        let ray = Ray3d::new(eye(transform.translation, *stance, player.drawn_duck(fixed.overstep_fraction())), aim(player));

        let range = world.ray_distance(&ray, LASER_RANGE).unwrap_or(LASER_RANGE);

        let hit = trace_past(&ray, range, shooter, &targets);

        let end = match hit {
            Some(hit) => hit.point,
            None => ray.origin + *ray.direction * range,
        };
        gizmos.line(ray.origin, end, beam_color);

        if let Some(hit) = hit {
            let color = match hit.zone {
                HitZone::Body => body_color,
                HitZone::Head => head_color,
            };
            gizmos.sphere(Isometry3d::from_translation(hit.point), HIT_BALL_RADIUS, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::class::{body_center_from_feet, TALLEST_CLASS_EYE_HEIGHT};
    use crate::common::hitbox::Box3;
    use crate::common::projectile::ProjectileSpec;

    /// A shooter at the origin looking down -Z and a hand-placed target box,
    /// so the test fails when firing changes rather than when a rig does.
    fn a_shooter_and_a_target(boxes: Hitboxes) -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<WeaponActionFired>>();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<RagdollShove>>();
        world.init_resource::<Messages<Effect>>();

        let center = body_center_from_feet(Vec3::ZERO);
        let shooter = world
            .spawn((
                Player::default(),
                PlayerId(7),
                Stance::Standing,
                PhysicsBody { previous: center, current: center },
                Hitboxes::default(),
            ))
            .id();
        let target = world.spawn(boxes).id();

        (world, shooter, target)
    }

    /// Boxes `away` meters down -Z at eye height, head on top.
    fn target_boxes(away: f32) -> Hitboxes {
        let eye = TALLEST_CLASS_EYE_HEIGHT;
        Hitboxes {
            body: Box3 {
                center: Vec3::new(0.0, eye, -away),
                half_extents: Vec3::new(0.4, 0.9, 0.4),
            },
            head: Some(Box3 {
                center: Vec3::new(0.0, eye + 0.7, -away),
                half_extents: Vec3::splat(0.15),
            }),
        }
    }

    const LASER: HitscanSpec = HitscanSpec {
        range_m: LASER_RANGE,
        damage: 30,
        headshot_multiplier: 3,
        shove_per_damage: 0.15,
        shove_radius_m: 0.4,
        pellets: None,
        kill_shove: None,
    };

    fn fire(world: &mut World, shooter: Entity) {
        fire_action(world, shooter, WeaponAction::Hitscan(LASER));
    }

    fn fire_action(world: &mut World, shooter: Entity, action: WeaponAction) {
        world.write_message(WeaponActionFired {
            shooter,
            button: crate::game::weapon::Button::Primary,
            action,
        });
        world.run_system_once(fire_hitscan).unwrap();
    }

    fn asked_for(world: &mut World) -> Vec<Damage> {
        let messages = world.resource::<Messages<Damage>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    fn effects_of(world: &mut World) -> Vec<Effect> {
        let messages = world.resource::<Messages<Effect>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    /// Breaks if the tracer is written after the miss's early return; firing
    /// into open space then shows nothing.
    #[test]
    fn a_shot_that_hits_nothing_still_leaves_a_tracer() {
        let (mut world, shooter, target) = a_shooter_and_a_target(target_boxes(5.0));
        world.entity_mut(target).despawn();
        fire(&mut world, shooter);

        assert!(asked_for(&mut world).is_empty(), "it hit something after all");

        let effects = effects_of(&mut world);
        assert_eq!(effects.len(), 1, "{effects:?}");
        assert!(matches!(effects[0], Effect::Tracer { .. }));
    }

    /// Breaks if our tracer starts at the eye (a dot on the crosshair in first
    /// person) or if somebody else's is moved to our barrel.
    #[test]
    fn our_tracer_leaves_the_barrel_and_nobody_elses_does() {
        let barrel = Vec3::new(0.3, -0.2, -0.6);
        let mut world = World::new();
        world.init_resource::<Messages<Effect>>();
        world.insert_resource(crate::game::viewmodel::ApparentMuzzle(Some(barrel)));
        world.spawn((LocalPlayer, PlayerId(1)));
        for by in [Some(PlayerId(1)), Some(PlayerId(2)), None] {
            world.write_message(Effect::Tracer { from: Vec3::ZERO, to: Vec3::NEG_Z, hit: false, by });
        }

        world.run_system_once(mark_tracers).unwrap();

        let mut starts: Vec<Vec3> =
            world.query::<&Tracer>().iter(&world).map(|tracer| tracer.from).collect();
        starts.sort_by(|a, b| a.x.total_cmp(&b.x));
        assert_eq!(starts, vec![Vec3::ZERO, Vec3::ZERO, barrel]);
    }

    #[test]
    fn a_tracer_stops_where_the_shot_did() {
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(5.0));
        fire(&mut world, shooter);

        let effects = effects_of(&mut world);
        let Some(Effect::Tracer { from, to, hit, .. }) = effects.first().copied() else {
            panic!("no tracer: {effects:?}");
        };
        assert!(hit, "the shot landed but the tracer says it did not");
        assert!((to - from).length() < LASER.range_m, "the tracer ran to full range");
        assert!((to.z - from.z).abs() > 1.0, "the tracer went nowhere");
    }

    #[test]
    fn a_shot_asks_for_damage_on_what_it_hit_and_says_who_fired() {
        let (mut world, shooter, target) = a_shooter_and_a_target(target_boxes(5.0));
        fire(&mut world, shooter);

        let asked = asked_for(&mut world);
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].target, target);
        assert_eq!(asked[0].source, DamageSource::Player(PlayerId(7)));
        assert_eq!(asked[0].amount, LASER.damage);
    }

    #[test]
    fn a_head_shot_is_worth_the_multiplier() {
        let mut boxes = target_boxes(5.0);
        // Aimed level, so only a head box at eye height is in the way.
        boxes.head.as_mut().unwrap().center.y = TALLEST_CLASS_EYE_HEIGHT;
        let (mut world, shooter, _) = a_shooter_and_a_target(boxes);

        fire(&mut world, shooter);
        assert_eq!(asked_for(&mut world)[0].amount, LASER.damage * LASER.headshot_multiplier);
    }

    /// Breaks if this system answers for another weapon's action.
    #[test]
    fn a_bullet_is_only_fired_for_a_hitscan_action() {
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(5.0));

        fire_action(
            &mut world,
            shooter,
            WeaponAction::Projectile(ProjectileSpec::ROCKET),
        );
        assert!(asked_for(&mut world).is_empty(), "the launcher fired a bullet");
    }

    /// Breaks if a shooter without a `PlayerId` (a turret) fires nothing.
    #[test]
    fn a_shot_fired_by_nobody_is_the_worlds() {
        let (mut world, shooter, target) = a_shooter_and_a_target(target_boxes(5.0));
        world.entity_mut(shooter).remove::<PlayerId>();

        fire(&mut world, shooter);

        let asked = asked_for(&mut world);
        assert_eq!(asked.len(), 1, "a shot with no shooter id vanished");
        assert_eq!(asked[0].target, target);
        assert_eq!(asked[0].source, DamageSource::World);
    }

    #[test]
    fn a_wall_between_you_and_a_body_stops_the_shot() {
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(20.0));
        // The room's far wall is between shooter and target.
        world.resource_mut::<CollisionWorld>().rebuild(&[crate::tool::room::Room::new(
            Vec3::new(-5.0, 0.0, -10.0),
            Vec3::new(5.0, 4.0, 5.0),
        )]);

        fire(&mut world, shooter);
        assert!(asked_for(&mut world).is_empty());
    }

    #[test]
    fn a_shot_never_lands_on_the_body_that_fired_it() {
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(5.0));
        let center = body_center_from_feet(Vec3::ZERO);
        world.entity_mut(shooter).insert(Hitboxes {
            body: Box3 { center, half_extents: Vec3::new(0.4, 0.9, 0.4) },
            head: Some(Box3 { center: center + Vec3::Y * 0.7, half_extents: Vec3::splat(0.15) }),
        });

        fire(&mut world, shooter);
        let asked = asked_for(&mut world);
        assert!(asked.iter().all(|hit| hit.target != shooter), "{asked:?}");
    }

    const SHOTGUN: HitscanSpec = HitscanSpec {
        range_m: 100.0,
        damage: 7,
        headshot_multiplier: 1,
        shove_per_damage: 0.15,
        shove_radius_m: 0.3,
        pellets: Some(crate::common::weapon::PelletGrid { across: 3, spread: 30.0 }),
        kill_shove: None,
    };

    /// Breaks if the pattern stops being a square of the stated width, or
    /// picks up any randomness.
    #[test]
    fn a_pellet_grid_is_a_square_as_wide_as_its_spread_says() {
        let grid = SHOTGUN.pellets.unwrap();
        let offsets = grid.offsets();
        assert_eq!(offsets.len(), 9);
        assert!(offsets.contains(&Vec2::ZERO), "no pellet down the aim: {offsets:?}");

        let width = |axis: fn(Vec2) -> f32| {
            let values = offsets.iter().map(|offset| axis(*offset));
            values.clone().fold(f32::MIN, f32::max) - values.fold(f32::MAX, f32::min)
        };
        assert!((width(|v| v.x) - 1.0 / 30.0).abs() < 1e-6);
        assert!((width(|v| v.y) - 1.0 / 30.0).abs() < 1e-6);
        assert_eq!(offsets, grid.offsets());
    }

    #[test]
    fn a_single_pellet_grid_is_one_bullet_down_the_aim() {
        let grid = crate::common::weapon::PelletGrid { across: 1, spread: 30.0 };
        assert_eq!(grid.offsets(), vec![Vec2::ZERO]);
    }

    /// Breaks if each pellet becomes its own `Damage`, which would put nine
    /// numbers and nine flinches on one body for one shot.
    #[test]
    fn every_pellet_that_lands_on_one_body_is_one_damage() {
        let (mut world, shooter, target) = a_shooter_and_a_target(target_boxes(5.0));
        fire_action(&mut world, shooter, WeaponAction::Hitscan(SHOTGUN));

        let asked = asked_for(&mut world);
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].target, target);
        // At five meters the square is 17 cm wide, inside the body box.
        assert_eq!(asked[0].amount, 9 * SHOTGUN.damage);

        let tracers = effects_of(&mut world)
            .into_iter()
            .filter(|effect| matches!(effect, Effect::Tracer { .. }))
            .count();
        assert_eq!(tracers, 9, "one tracer per pellet");
    }

    /// Breaks if the pellets all go down the aim.
    #[test]
    fn far_enough_away_some_pellets_miss() {
        // At sixty meters the square is two meters wide; the body box is 0.8.
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(60.0));
        fire_action(&mut world, shooter, WeaponAction::Hitscan(SHOTGUN));

        let asked = asked_for(&mut world);
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert!(asked[0].amount < 9 * SHOTGUN.damage, "every pellet landed: {asked:?}");
        assert!(asked[0].amount >= SHOTGUN.damage, "nothing landed: {asked:?}");
    }

    /// Breaks if the pitch sign is backwards.
    #[test]
    fn pitching_up_aims_up() {
        let level = aim(&Player { pitch: 0.0, ..default() });
        let up = aim(&Player { pitch: 0.5, ..default() });

        assert!(level.y.abs() < 1e-6);
        assert!(up.y > 0.0, "looking up aimed at {up:?}");
    }

    #[test]
    fn the_beam_goes_the_way_the_body_faces() {
        let player = Player { yaw: 1.1, pitch: 0.4, ..default() };
        let facing = Quat::from_rotation_y(player.yaw) * Vec3::NEG_Z;

        let aimed = aim(&player);
        assert!(
            (aimed.xz().normalize() - facing.xz().normalize()).length() < 1e-5,
            "aimed {aimed:?} while facing {facing:?}"
        );
    }

    #[test]
    fn the_beam_starts_at_eye_height() {
        use crate::common::class::body_center_from_feet;

        let feet = Vec3::new(2.0, 3.0, -1.0);
        let origin = eye(body_center_from_feet(feet), Stance::Standing, Duck::default());

        assert!((origin.y - feet.y - TALLEST_CLASS_EYE_HEIGHT).abs() < 1e-5, "at {origin:?}");
        assert!((origin.xz() - feet.xz()).length() < 1e-5);
    }
}
