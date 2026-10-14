//! Setting things on fire. `fire_flame` is the weapon; `burn` is afterburn,
//! a damage source in `DamageSystems::Deal` with no weapon behind it.
//!
//! A body caught by several of a puff's rays takes one puff, so the ray count
//! is a fidelity setting and not a damage multiplier. Afterburn is credited
//! to [`Burning::source`] because the shooter may be gone by the last tick.
//! A burning body's look is decided in [`crate::game::body_mesh`], not here.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::damage::{Damage, DamageSource, PlayerId};
use crate::common::flame::{cone_directions, Burning, FlameSpec};
use crate::common::hitbox::Hitboxes;
use crate::common::team::Allegiances;
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::hitscan::{aim, eye, trace_past};
use crate::game::player::{Player, PhysicsBody};
use crate::common::weapon::{Equipped, WeaponAction, WeaponCatalogue};
use crate::game::weapon::WeaponActionFired;

pub struct FlamePlugin;

impl Plugin for FlamePlugin {
    fn build(&self, app: &mut App) {
        app
            .add_systems(FixedUpdate, (fire_flame, burn).in_set(DamageSystems::Deal))
            .add_systems(PostUpdate, draw_flame_cone.run_if(in_state(AppMode::Play)))
            .add_systems(OnExit(AppMode::Play), put_out_fires)
        ;
    }
}

/// One puff per [`WeaponActionFired`] carrying a flame action.
pub fn fire_flame(
    mut commands: Commands,
    world: Res<CollisionWorld>,
    mut fired: MessageReader<WeaponActionFired>,
    shooters: Query<(&PhysicsBody, &Player, &Stance, Option<&PlayerId>)>,
    targets: Query<(Entity, &Hitboxes)>,
    mut alight: Query<&mut Burning>,
    allegiances: Allegiances,
    mut damage: MessageWriter<Damage>,
) {
    for shot in fired.read() {
        let WeaponAction::Flame(spec) = shot.action else { continue };
        let Ok((body, player, stance, id)) = shooters.get(shot.shooter) else { continue };

        let source = match id {
            Some(id) => DamageSource::Player(*id),
            None => DamageSource::World,
        };
        // The step's position, not the interpolated one, so every machine
        // agrees.
        let origin = eye(body.current, *stance);

        // One body, one puff, however many rays found it.
        let mut caught: Vec<Entity> = Vec::new();

        for direction in cone_directions(aim(player), spec.spread, spec.rays) {
            let ray = Ray3d::new(origin, direction);
            let reach = world.ray_distance(&ray, spec.range_m).unwrap_or(spec.range_m);

            let Some(hit) = trace_past(&ray, reach, shot.shooter, &targets) else { continue };
            if caught.contains(&hit.target) {
                continue;
            }
            caught.push(hit.target);

            // Lighting never passes through `apply_damage`'s team check, so
            // this weapon has to ask for itself.
            if !allegiances.may_hurt(source, hit.target) {
                continue;
            }

            // No headshot multiplier for fire.
            damage.write(Damage {
                target: hit.target,
                source,
                amount: spec.damage,
                point: hit.point,
            });

            match alight.get_mut(hit.target) {
                Ok(mut burning) => burning.relight(&spec, source),
                Err(_) => {
                    commands.entity(hit.target).insert(Burning::lit(&spec, source));
                }
            }
        }
    }
}

/// `Time<Fixed>`, so a match burns for the same number of ticks every run.
pub fn burn(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    mut bodies: Query<(Entity, &mut Burning, Option<&Hitboxes>, Option<&GlobalTransform>)>,
    mut damage: MessageWriter<Damage>,
) {
    let dt = time.delta_secs();
    for (entity, mut burning, boxes, placed) in &mut bodies {
        let due = burning.tick(dt);

        // The damage number goes over the burning body.
        let at = boxes
            .map(|boxes| boxes.body.centre)
            .or_else(|| placed.map(|placed| placed.translation()))
            .unwrap_or_default();

        for _ in 0..due {
            damage.write(Damage {
                target: entity,
                source: burning.source,
                amount: burning.damage,
                point: at,
            });
        }

        if burning.is_out() {
            // Removed rather than left at zero: presence means on fire.
            commands.entity(entity).remove::<Burning>();
        }
    }
}

/// The cone gizmo for a flame fired from `origin` along `direction`.
///
/// Bevy draws a [`Cone`] with its apex at local `+Y`, centred on the
/// isometry. So `+Y` maps onto `-direction` and the centre goes half a range
/// out, putting the apex on the eye and the base at full range. Separate so a
/// test can check the rotation.
fn flame_cone(origin: Vec3, direction: Dir3, spec: &FlameSpec) -> (Cone, Isometry3d) {
    let cone = Cone {
        radius: spec.range_m * spec.spread.tan(),
        height: spec.range_m,
    };
    let isometry = Isometry3d::new(
        origin + *direction * (spec.range_m * 0.5),
        Quat::from_rotation_arc(Vec3::Y, -*direction),
    );
    (cone, isometry)
}

/// Reads `Equipped` rather than what was fired, because it draws between
/// shots. `Equipped` is replicated, so this works on clients.
fn draw_flame_cone(
    mut gizmos: Gizmos,
    catalogue: Res<WeaponCatalogue>,
    shooters: Query<(&Transform, &Player, &Stance, &Equipped)>,
) {
    for (transform, player, stance, equipped) in &shooters {
        let held = catalogue.held_by(equipped).map(|weapon| weapon.primary.action);
        let Some(WeaponAction::Flame(spec)) = held else { continue };

        let (cone, isometry) = flame_cone(eye(transform.translation, *stance), aim(player), &spec);
        gizmos.primitive_3d(&cone, isometry, Color::srgba(1.0, 0.6, 0.1, 0.5));
    }
}

/// Called on leaving Play and again from
/// [`reset_for_play`](crate::game::reset::reset_for_play).
pub fn put_out_fires(mut commands: Commands, alight: Query<Entity, With<Burning>>) {
    for body in &alight {
        commands.entity(body).remove::<Burning>();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use crate::constants::one_tick;

    use super::*;
    use crate::common::class::body_centre_from_feet;
    use crate::common::damage::Damageable;
    use crate::common::hitbox::Box3;
    use crate::tool::room::Room;

    const SPEC: FlameSpec = FlameSpec::FLAMETHROWER;

    fn a_world() -> World {
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<WeaponActionFired>>();
        world.init_resource::<Messages<Damage>>();

        let mut fixed = Time::<Fixed>::default();
        fixed.advance_by(one_tick());
        world.insert_resource(fixed);
        world
    }

    /// A shooter at the origin, looking down -Z.
    fn a_shooter(world: &mut World) -> Entity {
        let centre = body_centre_from_feet(Vec3::ZERO);
        world
            .spawn((
                Player::default(),
                PlayerId(1),
                Stance::Standing,
                PhysicsBody { previous: centre, current: centre },
            ))
            .id()
    }

    /// A body `away` metres down -Z and `across` to the side, at eye height.
    fn a_body(world: &mut World, away: f32, across: f32) -> Entity {
        let eye = crate::common::class::TALLEST_CLASS_EYE_HEIGHT;
        let centre = Vec3::new(across, eye, -away);
        world
            .spawn((
                Hitboxes {
                    body: Box3 { centre, half_extents: Vec3::new(0.4, 0.9, 0.4) },
                    head: Box3 { centre: centre + Vec3::Y * 0.8, half_extents: Vec3::splat(0.15) },
                },
                Damageable::default(),
            ))
            .id()
    }

    fn puff(world: &mut World, shooter: Entity) {
        world.write_message(WeaponActionFired {
            shooter,
            button: crate::game::weapon::Button::Primary,
            action: WeaponAction::Flame(SPEC),
        });
        world.run_system_once(fire_flame).unwrap();
        world.flush();
    }

    fn asked_for(world: &mut World) -> Vec<Damage> {
        let messages = world.resource::<Messages<Damage>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    /// Breaks if lighting skips the team check, which `apply_damage` cannot
    /// do for it.
    #[test]
    fn a_teammate_in_the_cone_does_not_catch_fire() {
        use crate::common::team::Team;

        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        world.entity_mut(shooter).insert(Team::Green);
        let ally = a_body(&mut world, 2.0, 0.0);
        world.entity_mut(ally).insert((PlayerId(2), Team::Green));

        puff(&mut world, shooter);

        assert!(asked_for(&mut world).is_empty(), "asked to hurt a teammate");
        assert!(world.get::<Burning>(ally).is_none(), "set a teammate alight");
    }

    #[test]
    fn an_enemy_in_the_cone_burns() {
        use crate::common::team::Team;

        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        world.entity_mut(shooter).insert(Team::Green);
        let enemy = a_body(&mut world, 2.0, 0.0);
        world.entity_mut(enemy).insert((PlayerId(2), Team::Yellow));

        puff(&mut world, shooter);

        assert_eq!(asked_for(&mut world).len(), 1);
        assert!(world.get::<Burning>(enemy).is_some());
    }

    /// Breaks if the drawn cone's rotation is backwards.
    #[test]
    fn the_drawn_cone_points_where_the_flame_goes() {
        let spec = SPEC;
        let origin = Vec3::new(1.0, 2.0, 3.0);

        for direction in [
            Dir3::NEG_Z,
            Dir3::X,
            Dir3::Y,
            Dir3::new(Vec3::new(1.0, -0.4, 0.7)).unwrap(),
        ] {
            let (cone, isometry) = flame_cone(origin, direction, &spec);

            let apex = isometry * (Vec3::Y * cone.height * 0.5);
            let base = isometry * (Vec3::NEG_Y * cone.height * 0.5);

            assert!(
                (apex - origin).length() < 1e-4,
                "the apex is at {apex}, not on the eye at {origin}"
            );
            let far = origin + *direction * spec.range_m;
            assert!(
                (base - far).length() < 1e-4,
                "the base is at {base}, not down the barrel at {far}"
            );
        }
    }

    #[test]
    fn the_drawn_cone_is_as_wide_as_the_traced_one() {
        let (cone, _) = flame_cone(Vec3::ZERO, Dir3::NEG_Z, &SPEC);
        let edge = Vec3::NEG_Z * SPEC.range_m + Vec3::X * cone.radius;
        assert!(
            (edge.angle_between(Vec3::NEG_Z) - SPEC.spread).abs() < 1e-5,
            "the drawn rim is {} rad off the axis, not {}",
            edge.angle_between(Vec3::NEG_Z),
            SPEC.spread
        );
    }

    #[test]
    fn a_puff_burns_what_it_catches_and_lights_it() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        let target = a_body(&mut world, 3.0, 0.0);

        puff(&mut world, shooter);

        let asked = asked_for(&mut world);
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].target, target);
        assert_eq!(asked[0].amount, SPEC.damage);
        assert_eq!(asked[0].source, DamageSource::Player(PlayerId(1)));

        let burning = world.get::<Burning>(target).expect("it did not catch fire");
        assert_eq!(burning.remaining_s, SPEC.afterburn_duration_s);
        assert_eq!(burning.source, DamageSource::Player(PlayerId(1)));
    }

    /// Breaks if damage scales with the number of rays that hit.
    #[test]
    fn a_body_caught_by_several_rays_takes_one_puffs_worth() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        // Close, and wide enough that the whole cone is inside it.
        let eye = crate::common::class::TALLEST_CLASS_EYE_HEIGHT;
        let centre = Vec3::new(0.0, eye, -2.0);
        world.spawn((
            Hitboxes {
                body: Box3 { centre, half_extents: Vec3::new(3.0, 3.0, 0.4) },
                head: Box3 { centre, half_extents: Vec3::splat(0.05) },
            },
            Damageable::default(),
        ));

        puff(&mut world, shooter);

        assert_eq!(asked_for(&mut world).len(), 1, "the cone was worth its ray count");
    }

    #[test]
    fn a_body_outside_the_cone_is_not_caught() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        // Five metres out and four across is about 39 degrees off the axis.
        a_body(&mut world, 5.0, 4.0);

        puff(&mut world, shooter);
        assert!(asked_for(&mut world).is_empty(), "the cone reached round the side");
    }

    #[test]
    fn a_body_past_the_range_is_not_caught() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        a_body(&mut world, SPEC.range_m + 4.0, 0.0);

        puff(&mut world, shooter);
        assert!(asked_for(&mut world).is_empty(), "the flame reached past its range");
    }

    #[test]
    fn a_wall_between_you_and_a_body_stops_the_flame() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        a_body(&mut world, 5.0, 0.0);
        world.resource_mut::<CollisionWorld>().rebuild(&[Room::new(
            Vec3::new(-4.0, 0.0, -3.0),
            Vec3::new(4.0, 4.0, 4.0),
        )]);

        puff(&mut world, shooter);
        assert!(asked_for(&mut world).is_empty(), "fire went through the wall");
    }

    /// The cone starts inside the shooter's own hitbox.
    #[test]
    fn a_shooter_never_catches_its_own_flame() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        let centre = body_centre_from_feet(Vec3::ZERO);
        world.entity_mut(shooter).insert(Hitboxes {
            body: Box3 { centre, half_extents: Vec3::new(0.4, 0.9, 0.4) },
            head: Box3 { centre: centre + Vec3::Y * 0.7, half_extents: Vec3::splat(0.15) },
        });

        puff(&mut world, shooter);
        assert!(world.get::<Burning>(shooter).is_none(), "it set itself alight");
    }

    /// Re-lighting resets the timer rather than stacking it.
    #[test]
    fn a_second_puff_resets_the_burn_rather_than_stacking_it() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        let target = a_body(&mut world, 3.0, 0.0);

        puff(&mut world, shooter);
        for _ in 0..(64 * 3) {
            world.run_system_once(burn).unwrap();
            world.flush();
        }
        let half_spent = world.get::<Burning>(target).unwrap().remaining_s;
        assert!(half_spent < SPEC.afterburn_duration_s);

        puff(&mut world, shooter);
        assert_eq!(
            world.get::<Burning>(target).unwrap().remaining_s,
            SPEC.afterburn_duration_s,
            "the second puff did not reset the fire"
        );
    }

    #[test]
    fn afterburn_ticks_on_its_own_and_then_goes_out() {
        let mut world = a_world();
        let target = a_body(&mut world, 3.0, 0.0);
        world
            .entity_mut(target)
            .insert(Burning::lit(&SPEC, DamageSource::Player(PlayerId(2))));

        for _ in 0..(64 * 8) {
            world.run_system_once(burn).unwrap();
            world.flush();
        }

        let ticks = asked_for(&mut world);
        assert_eq!(
            ticks.len() as f32,
            SPEC.afterburn_duration_s / SPEC.afterburn_interval_s,
            "{} ticks",
            ticks.len()
        );
        assert!(ticks.iter().all(|tick| tick.amount == SPEC.afterburn_damage));
        assert!(
            ticks.iter().all(|tick| tick.source == DamageSource::Player(PlayerId(2))),
            "afterburn was not credited to whoever lit it"
        );
        assert!(world.get::<Burning>(target).is_none(), "the fire never went out");
    }

    #[test]
    fn an_afterburn_number_is_drawn_over_the_burning_body() {
        let mut world = a_world();
        let target = a_body(&mut world, 3.0, 0.0);
        let chest = world.get::<Hitboxes>(target).unwrap().body.centre;
        world
            .entity_mut(target)
            .insert(Burning::lit(&SPEC, DamageSource::World));

        for _ in 0..(64 + 1) {
            world.run_system_once(burn).unwrap();
            world.flush();
        }

        let ticks = asked_for(&mut world);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|tick| tick.point == chest), "{ticks:?}");
    }
}
