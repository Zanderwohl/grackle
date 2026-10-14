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
use crate::common::damage::{Damage, DamageSource, PlayerId};
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::{trace, HitZone};
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::hitbox::update_hitboxes;
use crate::game::player::{step_player, PhysicsBody, Player};
use crate::game::ragdoll::{Push, RagdollShove};
use crate::common::weapon::{HitscanSpec, WeaponAction};
use crate::game::weapon::WeaponActionFired;

/// How far the debug laser reaches, in metres.
pub const LASER_RANGE: f32 = 200.0;

/// Seconds. Short enough that a stream of tracers does not read as one beam.
const TRACER_LIFETIME: f32 = 0.12;

/// Radius of the ball drawn at a hit, in metres.
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
                (draw_debug_laser, draw_tracers)
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
    let looking = Quat::from_rotation_y(player.yaw) * Quat::from_rotation_x(player.pitch);
    Dir3::new(looking * Vec3::NEG_Z).unwrap_or(Dir3::NEG_Z)
}

/// Where the eye is, given a body's centre. Takes a centre because the laser
/// passes the interpolated transform and the shot the fixed step's position.
pub(crate) fn eye(centre: Vec3, stance: Stance) -> Vec3 {
    centre + Vec3::Y * stance.eye_offset()
}

fn damage_for(zone: HitZone, spec: &HitscanSpec) -> u32 {
    match zone {
        HitZone::Body => spec.damage,
        HitZone::Head => spec.damage * spec.headshot_multiplier,
    }
}

/// One shot per [`WeaponActionFired`] carrying a hitscan action.
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
        let ray = Ray3d::new(eye(body.current, *stance), aim(player));
        let range = world.ray_distance(&ray, spec.range).unwrap_or(spec.range);

        let others = targets
            .iter()
            .filter(|(entity, _)| *entity != shot.shooter)
            .map(|(entity, boxes)| (entity, *boxes));

        let hit = trace(&ray, range, others);

        // Before the `continue`: a miss still draws a tracer.
        effects.write(Effect::Tracer {
            from: ray.origin,
            to: hit.map_or(ray.origin + *ray.direction * range, |hit| hit.point),
            hit: hit.is_some(),
        });

        let Some(hit) = hit else { continue };

        let worth = damage_for(hit.zone, &spec);
        damage.write(Damage {
            target: hit.target,
            source: id.map_or(DamageSource::World, |id| DamageSource::Player(*id)),
            amount: worth,
            point: hit.point,
        });

        // Written for every hit, fatal or not. Scaled by what the shot was
        // worth rather than what it took off, so a killing shot shoves as hard
        // as any other. The radius is tight so the hit bone moves and the
        // joints drag the rest.
        shoves.write(RagdollShove {
            at: hit.point,
            push: Push::Along(*ray.direction * (worth as f32 * spec.shove_per_damage)),
            radius: spec.shove_radius,
        });
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

fn mark_tracers(mut commands: Commands, mut effects: MessageReader<Effect>) {
    for effect in effects.read() {
        let Effect::Tracer { from, to, hit } = *effect else { continue };
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
    shooters: Query<(Entity, &Transform, &Player, &Stance)>,
    targets: Query<(Entity, &Hitboxes)>,
) {
    let beam_colour = Color::srgb(0.35, 0.8, 1.0);
    let body_colour = Color::srgb(0.2, 0.95, 0.35);
    let head_colour = Color::srgb(1.0, 0.15, 0.15);

    for (shooter, transform, player, stance) in &shooters {
        let ray = Ray3d::new(eye(transform.translation, *stance), aim(player));

        let range = world.ray_distance(&ray, LASER_RANGE).unwrap_or(LASER_RANGE);

        // Your own boxes surround your eye.
        let others = targets
            .iter()
            .filter(|(entity, _)| *entity != shooter)
            .map(|(entity, boxes)| (entity, *boxes));

        let hit = trace(&ray, range, others);

        let end = match hit {
            Some(hit) => hit.point,
            None => ray.origin + *ray.direction * range,
        };
        gizmos.line(ray.origin, end, beam_colour);

        if let Some(hit) = hit {
            let colour = match hit.zone {
                HitZone::Body => body_colour,
                HitZone::Head => head_colour,
            };
            gizmos.sphere(Isometry3d::from_translation(hit.point), HIT_BALL_RADIUS, colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::class::{body_centre_from_feet, TALLEST_CLASS_EYE_HEIGHT};
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

        let centre = body_centre_from_feet(Vec3::ZERO);
        let shooter = world
            .spawn((
                Player::default(),
                PlayerId(7),
                Stance::Standing,
                PhysicsBody { previous: centre, current: centre },
                Hitboxes::default(),
            ))
            .id();
        let target = world.spawn(boxes).id();

        (world, shooter, target)
    }

    /// Boxes `away` metres down -Z at eye height, head on top.
    fn target_boxes(away: f32) -> Hitboxes {
        let eye = TALLEST_CLASS_EYE_HEIGHT;
        Hitboxes {
            body: Box3 {
                centre: Vec3::new(0.0, eye, -away),
                half_extents: Vec3::new(0.4, 0.9, 0.4),
            },
            head: Box3 {
                centre: Vec3::new(0.0, eye + 0.7, -away),
                half_extents: Vec3::splat(0.15),
            },
        }
    }

    const LASER: HitscanSpec = HitscanSpec {
        range: LASER_RANGE,
        damage: 30,
        headshot_multiplier: 3,
        shove_per_damage: 0.15,
        shove_radius: 0.4,
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

    #[test]
    fn a_tracer_stops_where_the_shot_did() {
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(5.0));
        fire(&mut world, shooter);

        let effects = effects_of(&mut world);
        let Some(Effect::Tracer { from, to, hit }) = effects.first().copied() else {
            panic!("no tracer: {effects:?}");
        };
        assert!(hit, "the shot landed but the tracer says it did not");
        assert!((to - from).length() < LASER.range, "the tracer ran to full range");
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
        boxes.head.centre.y = TALLEST_CLASS_EYE_HEIGHT;
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
        let centre = body_centre_from_feet(Vec3::ZERO);
        world.entity_mut(shooter).insert(Hitboxes {
            body: Box3 { centre, half_extents: Vec3::new(0.4, 0.9, 0.4) },
            head: Box3 { centre: centre + Vec3::Y * 0.7, half_extents: Vec3::splat(0.15) },
        });

        fire(&mut world, shooter);
        let asked = asked_for(&mut world);
        assert!(asked.iter().all(|hit| hit.target != shooter), "{asked:?}");
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
        use crate::common::class::body_centre_from_feet;

        let feet = Vec3::new(2.0, 3.0, -1.0);
        let origin = eye(body_centre_from_feet(feet), Stance::Standing);

        assert!((origin.y - feet.y - TALLEST_CLASS_EYE_HEIGHT).abs() < 1e-5, "at {origin:?}");
        assert!((origin.xz() - feet.xz()).length() < 1e-5);
    }
}
