//! Setting things on fire, and what that costs them afterwards.
//!
//! Two systems, and they are two because fire is two things. `fire_flame` is a
//! weapon: a cone of traces, ten times a second, that hurts what it catches
//! and lights it. `burn` is a damage source with no weapon behind it at all —
//! it reads the [`Burning`] on a body and asks for a few points every half
//! second until the fire is out. Both sit in
//! [`DamageSystems::Deal`](crate::game::damage::DamageSystems) with everything
//! else that hurts something, which is the whole reason afterburn took no new
//! machinery: it writes a `Damage` like a bullet does.
//!
//! Three decisions worth knowing about:
//!
//! - **The cone is sampled, and the sampling is not worth anything.** A puff
//!   is [`FlameSpec::rays`] traces, and a body caught by four of them takes
//!   one puff's damage. How finely the cone is sampled is a fidelity knob; a
//!   weapon that hurt more because it was traced more carefully would be a
//!   weapon whose damage is a rendering setting.
//! - **A wall stops fire**, ray by ray, through the same `ray_distance` that
//!   shortens a bullet. Fire round a corner is fire through a wall.
//! - **Afterburn is credited to whoever lit it last**, which is what
//!   [`Burning::source`] is for. It is deliberately not the weapon's business:
//!   the shooter may be dead, or gone from the match, by the time the last
//!   tick lands.
//!
//! What a burning body looks like is [`crate::game::body_mesh`]'s business —
//! the tint is computed from `Burning` where every other body colour is
//! decided, rather than written onto the body from here. Two systems writing a
//! body's colour is a body that stays scorched after the fire is out.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::damage::{Damage, DamageSource, PlayerId};
use crate::common::flame::{cone_directions, Burning, FlameSpec};
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::trace;
use crate::common::team::Allegiances;
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::hitscan::{aim, eye};
use crate::game::player::{Player, PhysicsBody};
use crate::common::weapon::{Equipped, WeaponAction, WeaponCatalogue};
use crate::game::weapon::WeaponActionFired;

pub struct FlamePlugin;

impl Plugin for FlamePlugin {
    fn build(&self, app: &mut App) {
        app
            .add_systems(FixedUpdate, (fire_flame, burn).in_set(DamageSystems::Deal))
            .add_systems(PostUpdate, draw_flame_cone.run_if(in_state(AppMode::Play)))
            // Fire belongs to the match. A body left smouldering into the
            // editor would keep taking damage while somebody resizes a room.
            .add_systems(OnExit(AppMode::Play), put_out_fires)
        ;
    }
}

/// One puff of flame, for every shooter whose trigger came up holding one.
///
/// Cadence is not this system's business — a flamethrower gets a
/// [`WeaponActionFired`] ten times a second because the
/// [`Cadence`](crate::common::weapon::Cadence) on that button says so, and
/// this simply answers each one.
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
        // The step's own position, not the drawn one, for the same reason a
        // bullet is fired from it: a shot taken from an interpolated body
        // comes from a slightly different place on every machine.
        let origin = eye(body.current, *stance);

        // One body, one puff, however many rays found it.
        let mut caught: Vec<Entity> = Vec::new();

        for direction in cone_directions(aim(player), spec.spread, spec.rays) {
            let ray = Ray3d::new(origin, direction);
            let reach = world.ray_distance(&ray, spec.range).unwrap_or(spec.range);

            let others = targets
                .iter()
                .filter(|(entity, _)| *entity != shot.shooter)
                .map(|(entity, boxes)| (entity, *boxes));

            let Some(hit) = trace(&ray, reach, others) else { continue };
            if caught.contains(&hit.target) {
                continue;
            }
            caught.push(hit.target);

            // The one place a weapon has to ask about teams for itself.
            // Everything a flame *damages* is dropped by `apply_damage` like
            // any other request, but setting somebody alight is a second
            // effect that never goes through it — and a teammate walking away
            // on fire from a flame that did them no damage would be the
            // friendly fire rule with a hole straight through it.
            if !allegiances.may_hurt(source, hit.target) {
                continue;
            }

            // A head is not worth more to fire. The zone is where the flame
            // touched, not what it touched — and a flamethrower that rewarded
            // aiming at heads at four metres would be a sniper rifle.
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

/// Everything that is on fire, burning.
///
/// `Time<Fixed>` like the rest of the tick, so a body does not smoulder while
/// somebody is in the editor and two runs of the same match burn for the same
/// number of ticks.
pub fn burn(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    mut bodies: Query<(Entity, &mut Burning, Option<&Hitboxes>, Option<&GlobalTransform>)>,
    mut damage: MessageWriter<Damage>,
) {
    let dt = time.delta_secs();
    for (entity, mut burning, boxes, placed) in &mut bodies {
        let due = burning.tick(dt);

        // Over the body, like splash — see [`crate::game::explosion`]. A
        // number that appeared where the flame was fired from would be over
        // the shooter.
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
            // Removed rather than left at zero, so "is this body on fire" is
            // the presence of a component and nothing has to ask a second
            // question about it.
            commands.entity(entity).remove::<Burning>();
        }
    }
}

/// The cone gizmo for a flame fired from `origin` along `direction`.
///
/// Bevy draws a [`Cone`] with its **apex at local `+Y`** and its base at
/// `-Y`, centred on the isometry's translation. So aiming one means mapping
/// `+Y` onto the direction the shot travels *backwards* along, and placing it
/// half a range down the barrel — at which point the apex lands on the eye and
/// the base circle lands at the far end of the range.
///
/// A function of values rather than a lump inside the drawing, because the
/// rotation is the one thing here that can be silently backwards: a cone
/// pointing out of the back of your head is a picture nobody can check by
/// reading it.
fn flame_cone(origin: Vec3, direction: Dir3, spec: &FlameSpec) -> (Cone, Isometry3d) {
    let cone = Cone {
        radius: spec.range * spec.spread.tan(),
        height: spec.range,
    };
    let isometry = Isometry3d::new(
        origin + *direction * (spec.range * 0.5),
        Quat::from_rotation_arc(Vec3::Y, -*direction),
    );
    (cone, isometry)
}

/// The cone the flame is actually traced through.
///
/// Drawn from the interpolated transform rather than the step's position —
/// this is a picture, and a picture may be smooth. The shot it describes comes
/// from the fixed step's own position, so the two are a fraction apart, which
/// is the right way round.
/// Unlike the firing systems this reads what a body is *holding* rather than
/// what it fired, because it draws between shots. That is a read a client may
/// make: it decides what is on the screen, never what happened. Reading a
/// `Loadout` here was what made the cone invisible on every machine but the
/// server's, since only the authority ever had one.
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

/// Put every fire out.
///
/// Called on leaving Play, and again from
/// [`reset_for_play`](crate::game::reset::reset_for_play) — the same belt and
/// braces the corpses, the projectiles and the damage numbers get.
pub fn put_out_fires(mut commands: Commands, alight: Query<Entity, With<Burning>>) {
    for body in &alight {
        commands.entity(body).remove::<Burning>();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use std::time::Duration;

    use super::*;
    use crate::common::class::body_centre_from_feet;
    use crate::common::damage::Damageable;
    use crate::common::hitbox::Box3;
    use crate::tool::room::Room;

    const STEP: Duration = Duration::from_micros(15625);
    const SPEC: FlameSpec = FlameSpec::FLAMETHROWER;

    fn a_world() -> World {
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<WeaponActionFired>>();
        world.init_resource::<Messages<Damage>>();

        let mut fixed = Time::<Fixed>::default();
        fixed.advance_by(STEP);
        world.insert_resource(fixed);
        world
    }

    /// A shooter at the origin, looking down -Z, holding the flamethrower.
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

    /// A body `away` metres down -Z and `across` metres to the side, boxed at
    /// the shooter's eye height so a level shot meets it.
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

    /// A teammate caught in the cone is neither hurt nor lit. The damage half
    /// would be dropped by `apply_damage` anyway; the *lighting* is the half
    /// that never passes through it, and a teammate walking away on fire from
    /// a flame that did them no damage would be the rule with a hole in it.
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

    /// The other side of it, through the same harness: an enemy in the same
    /// place takes the puff and burns.
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

    /// The drawn cone has to describe the shot: apex on the eye, base at the
    /// far end of the range, and a base radius that matches the spread. The
    /// rotation is the piece that can be silently backwards — a cone out of
    /// the back of your head draws perfectly well.
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

            // The two points Bevy actually draws the cone between.
            let apex = isometry * (Vec3::Y * cone.height * 0.5);
            let base = isometry * (Vec3::NEG_Y * cone.height * 0.5);

            assert!(
                (apex - origin).length() < 1e-4,
                "the apex is at {apex}, not on the eye at {origin}"
            );
            let far = origin + *direction * spec.range;
            assert!(
                (base - far).length() < 1e-4,
                "the base is at {base}, not down the barrel at {far}"
            );
        }
    }

    /// And it is as wide as the cone the rays are actually traced through: a
    /// ray at the edge of the spread must land on the base circle's rim.
    #[test]
    fn the_drawn_cone_is_as_wide_as_the_traced_one() {
        let (cone, _) = flame_cone(Vec3::ZERO, Dir3::NEG_Z, &SPEC);
        let edge = Vec3::NEG_Z * SPEC.range + Vec3::X * cone.radius;
        assert!(
            (edge.angle_between(Vec3::NEG_Z) - SPEC.spread).abs() < 1e-5,
            "the drawn rim is {} rad off the axis, not {}",
            edge.angle_between(Vec3::NEG_Z),
            SPEC.spread
        );
    }

    /// A puff hurts what it catches and leaves it alight, credited to
    /// whoever pulled the trigger.
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
        assert_eq!(burning.remaining, SPEC.afterburn_duration);
        assert_eq!(burning.source, DamageSource::Player(PlayerId(1)));
    }

    /// How finely the cone is sampled must not be worth anything: a body
    /// standing in the middle of it is found by every ray and takes one puff.
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

    /// The cone is a cone: something off to the side, at a wider angle than
    /// the spread, is not caught.
    #[test]
    fn a_body_outside_the_cone_is_not_caught() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        // Five metres out and four across is about 39 degrees off the axis.
        a_body(&mut world, 5.0, 4.0);

        puff(&mut world, shooter);
        assert!(asked_for(&mut world).is_empty(), "the cone reached round the side");
    }

    /// And it is short. A body past the range is out of reach however
    /// squarely it is aimed at.
    #[test]
    fn a_body_past_the_range_is_not_caught() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        a_body(&mut world, SPEC.range + 4.0, 0.0);

        puff(&mut world, shooter);
        assert!(asked_for(&mut world).is_empty(), "the flame reached past its range");
    }

    /// A wall stops fire, which is the same rule a bullet obeys and comes
    /// from the same function.
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

    /// You cannot set fire to yourself with your own flamethrower — the cone
    /// starts inside your own hitbox.
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

    /// Being caught again resets the timer, whichever way that moves it.
    #[test]
    fn a_second_puff_resets_the_burn_rather_than_stacking_it() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        let target = a_body(&mut world, 3.0, 0.0);

        puff(&mut world, shooter);
        // Half the burn spent.
        for _ in 0..(64 * 3) {
            world.run_system_once(burn).unwrap();
            world.flush();
        }
        let half_spent = world.get::<Burning>(target).unwrap().remaining;
        assert!(half_spent < SPEC.afterburn_duration);

        puff(&mut world, shooter);
        assert_eq!(
            world.get::<Burning>(target).unwrap().remaining,
            SPEC.afterburn_duration,
            "the second puff did not reset the fire"
        );
    }

    /// Afterburn asks for damage on its own, with no weapon involved, and
    /// stops — and the fire goes out rather than sitting at zero.
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
            SPEC.afterburn_duration / SPEC.afterburn_interval,
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

    /// The number goes over the body that is burning, not wherever it was
    /// lit — it walks away, and the fire goes with it.
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
