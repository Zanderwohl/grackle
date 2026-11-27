//! Running out of health. In order on the tick: `record_damage` logs each
//! hit, `announce_the_dead` writes [`Died`] and marks [`Dying`], and
//! `reap_the_dead` despawns a tick later.
//!
//! Recording must precede announcing within the tick, or a killing shot is
//! credited to nobody. Death is any [`Damageable`] at zero, player or not.
//! [`crate::game::ragdoll`] orders itself before `reap_the_dead`; reordering
//! these silently stops corpses appearing.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::damage::{Cause, Damage, DamageDealt, DamageLog, DamageSource, Damageable, Died, Explosion, HealthLeft, How, Kill, PlayerId};
use crate::common::skeleton::{AnimationClock, Pose, Skeleton};
use crate::game::skeleton::{skeleton_root, SkeletonRoot};
use crate::game::player::Inputs;

const SUICIDE_DAMAGE: u32 = 1000;

/// The weapon whose primary projectile's blast the explode key sets off.
const EXPLODES_AS: crate::common::weapon::WeaponId = crate::common::weapon::WeaponId::of("rocket_launcher");

/// The weapon the ignite key's afterburn is recorded as.
const IGNITES_AS: crate::common::weapon::WeaponId = crate::common::weapon::WeaponId::of("flamethrower");

/// Credited to yourself, which the team rule always lets through.
pub fn suicide(
    bodies: Query<(Entity, &Inputs, &PlayerId, &GlobalTransform), Without<Dying>>,
    mut damage: MessageWriter<Damage>,
) {
    for (body, input, id, placed) in &bodies {
        if input.suicide {
            damage.write(Damage {
                target: body,
                source: DamageSource::Player(*id),
                amount: SUICIDE_DAMAGE,
                point: placed.translation(),
                from: None,
                kill_shove: None,
                cause: Cause::of(None, How::Kill),
            });
        }
    }
}

/// Credited to yourself, which the team rule always lets through. Burning
/// already, the afterburn starts over, still credited to whoever lit it.
pub fn ignite_yourself(
    mut commands: Commands,
    mut bodies: Query<(Entity, &Inputs, &PlayerId, Option<&mut crate::common::flame::Burning>), Without<Dying>>,
) {
    use crate::common::flame::{Burning, FlameSpec};
    for (body, input, id, burning) in &mut bodies {
        if !input.ignite {
            continue;
        }
        let source = DamageSource::Player(*id);
        match burning {
            Some(mut burning) => {
                let lit_by = burning.source;
                burning.relight(&FlameSpec::FLAMETHROWER, lit_by);
            }
            None => {
                commands.entity(body).try_insert(Burning { weapon: Some(IGNITES_AS), ..Burning::lit(&FlameSpec::FLAMETHROWER, source) });
            }
        }
    }
}

/// A [`Kill`] rather than a [`Damage`], as [`explode_yourself`] is, so no
/// living body is left marked to lose its head.
pub fn behead_yourself(
    mut commands: Commands,
    bodies: Query<(Entity, &Inputs, &PlayerId, &GlobalTransform), Without<Dying>>,
    mut kills: MessageWriter<Kill>,
) {
    for (body, input, id, placed) in &bodies {
        if !input.behead {
            continue;
        }
        commands.entity(body).try_insert(crate::game::ragdoll::BlownApart::Head);
        kills.write(Kill { target: body, source: DamageSource::Player(*id), point: placed.translation(), how: How::Kill });
    }
}

/// A [`Kill`] rather than a [`Damage`], so neither armor nor the mode's
/// self-damage scaling can leave a living body marked to be blown apart. The
/// blast spares the body, so the kill's cause stays [`How::Exploded`]. Ordered
/// before `explode`, so its shove lands on the pieces `raise_ragdolls` makes
/// this tick.
pub fn explode_yourself(
    mut commands: Commands,
    catalog: Option<Res<crate::common::weapon::WeaponCatalog>>,
    bodies: Query<
        (Entity, &Inputs, &PlayerId, &GlobalTransform, Option<(&Skeleton, &Pose)>, Option<&SkeletonRoot>),
        Without<Dying>,
    >,
    mut kills: MessageWriter<Kill>,
    mut blasts: MessageWriter<Explosion>,
) {
    let rocket = catalog.as_deref().and_then(|catalog| match &catalog.get(EXPLODES_AS)?.primary.action {
        crate::common::weapon::WeaponAction::Projectile(spec) => Some(*spec),
        _ => None,
    });
    for (body, input, id, placed, rig, offset) in &bodies {
        if !input.explode {
            continue;
        }
        let source = DamageSource::Player(*id);
        let at = rig
            .and_then(|(skeleton, pose)| crate::game::ragdoll::torso_center(skeleton, pose, &skeleton_root(placed, offset)))
            .unwrap_or(placed.translation());
        commands.entity(body).try_insert(crate::game::ragdoll::BlownApart::Torso);
        kills.write(Kill { target: body, source, point: at, how: How::Exploded });
        if let Some(spec) = rocket {
            blasts.write(Explosion {
                at,
                radius: spec.explosion_radius,
                damage: spec.damage,
                at_edge: spec.splash_at_edge,
                scale: 1.0,
                knockback: spec.knockback,
                source,
                direct: None,
                weapon: None,
                reflected: false,
                critical: false,
                spare: Some(body),
            });
        }
    }
}

/// Put every hit into the victim's log. Game time, not wall time, so the
/// assist window does not run while somebody is in the editor.
pub fn record_damage(
    clock: Res<AnimationClock>,
    mut hits: MessageReader<DamageDealt>,
    mut logs: Query<&mut DamageLog>,
) {
    let now = clock.seconds();
    for hit in hits.read() {
        // Already reaped.
        let Ok(mut log) = logs.get_mut(hit.target) else { continue };
        // A hit on a body already emptied, later in the same tick, did not kill it.
        if hit.amount == 0 && hit.remaining == 0 {
            continue;
        }
        log.record(hit.source, now);
        log.caused(hit.cause);
    }
}

/// This body has died and is despawned next tick.
///
/// Replicated, so every machine learns of the death before the despawn on the
/// same ordered channel. `Died` goes on a separate channel with no ordering
/// against replication, so it is used only for the kill feed.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Reflect, Serialize, Deserialize)]
pub struct Dying;

/// Say who died, and mark the body for removal next tick.
pub fn announce_the_dead(
    mut commands: Commands,
    clock: Res<AnimationClock>,
    mut died: MessageWriter<Died>,
    bodies: Query<
        (
            Entity,
            &Damageable,
            &DamageLog,
            Option<&Name>,
            Option<&PlayerId>,
            Option<&GlobalTransform>,
        ),
        Without<Dying>,
    >,
    // Dying included: two who kill each other in one tick each see the other at nothing.
    people: Query<(&PlayerId, &Damageable, Has<Dying>), With<crate::game::player::Player>>,
) {
    let now = clock.seconds();
    for (entity, health, log, name, id, placed) in &bodies {
        if health.is_alive() {
            continue;
        }

        let killer = log.killer().unwrap_or(DamageSource::World);
        let killer_health = match killer {
            // A body given in the tick the last one died stands beside it; the living one is theirs.
            DamageSource::Player(killer) => people
                .iter()
                .filter(|(id, ..)| **id == killer)
                .min_by_key(|(.., dying)| *dying)
                .map(|(_, pool, _)| HealthLeft::of(pool)),
            _ => None,
        };
        died.write(Died {
            victim: entity,
            victim_name: name.map(|name| name.to_string()),
            victim_id: id.copied(),
            killer,
            assist: log.assist(now, id.copied()),
            at: placed.map(|placed| placed.translation()).unwrap_or_default(),
            cause: log.cause(),
            killer_health,
            rand: rand::random(),
        });

        commands.entity(entity).insert(Dying);
    }
}

/// Remove what died on an earlier tick. See [`Dying`].
pub fn reap_the_dead(mut commands: Commands, dying: Query<Entity, With<Dying>>) {
    for entity in &dying {
        commands.entity(entity).despawn();
    }
}

/// A client's record of each death, since it keeps no round: the authority logs its kills in
/// [`crate::game::telemetry`].
pub fn announce_deaths(mut deaths: MessageReader<Died>) {
    use crate::game::telemetry::{log_event, source_id, Event};
    for death in deaths.read() {
        log_event(&Event::Death {
            victim: death.victim_id.map(|id| id.0),
            victim_name: death.victim_name.clone(),
            killer: source_id(death.killer),
            assist: death.assist.and_then(source_id),
            at: death.at.to_array(),
        });
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::damage::ASSIST_WINDOW;

    const SHOOTER: DamageSource = DamageSource::Player(PlayerId(1));
    const HELPER: DamageSource = DamageSource::Player(PlayerId(2));

    fn a_world() -> World {
        let mut world = World::new();
        world.init_resource::<AnimationClock>();
        world.init_resource::<Messages<DamageDealt>>();
        world.init_resource::<Messages<Died>>();
        world
    }

    fn advance(world: &mut World, seconds: f32) {
        world.resource_mut::<AnimationClock>().advance(seconds);
    }

    /// Land one hit and let `record_damage` see it. Drained afterward, since
    /// each `run_system_once` has a fresh cursor that would re-read it.
    fn land(world: &mut World, target: Entity, source: DamageSource) {
        world.write_message(DamageDealt {
            target,
            source,
            amount: 1,
            remaining: 0,
            point: Vec3::ZERO,
            from: None,
            cause: Default::default(),
        });
        world.run_system_once(record_damage).unwrap();
        world.resource_mut::<Messages<DamageDealt>>().clear();
    }

    fn deaths(world: &mut World) -> Vec<Died> {
        let messages = world.resource::<Messages<Died>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).cloned().collect()
    }

    /// Announcing and removing, in schedule order.
    fn resolve(world: &mut World) {
        world.run_system_once(announce_the_dead).unwrap();
        world.flush();
        world.run_system_once(reap_the_dead).unwrap();
        world.flush();
    }

    /// The last hit gets the kill and the one before it the assist.
    #[test]
    fn a_kill_credits_the_last_hit_and_the_one_before_it() {
        let mut world = a_world();
        let victim = world
            .spawn((Damageable::with_health(100), DamageLog::default(), Name::new("Scout"), PlayerId(9)))
            .id();

        land(&mut world, victim, HELPER);

        advance(&mut world, 1.0);
        world.get_mut::<Damageable>(victim).unwrap().apply(100);
        land(&mut world, victim, SHOOTER);
        resolve(&mut world);

        let record = deaths(&mut world);
        assert_eq!(record.len(), 1);
        assert_eq!(record[0].killer, SHOOTER);
        assert_eq!(record[0].assist, Some(HELPER));
        assert_eq!(record[0].victim_name.as_deref(), Some("Scout"));
        assert_eq!(record[0].victim_id, Some(PlayerId(9)));
        assert!(world.get_entity(victim).is_err(), "the body was not removed");
    }

    /// Breaks if a death does not carry the killing hit's cause, or a hit landing on the emptied
    /// body afterward takes the kill or the cause.
    #[test]
    fn a_death_carries_the_killing_hit_and_not_one_after_it() {
        use crate::common::damage::{Cause, How};
        let mut world = a_world();
        let victim = world.spawn((Damageable::with_health(100), DamageLog::default(), PlayerId(9))).id();
        let rocket = Cause::of(Some(crate::common::weapon::WeaponId(7)), How::Splash);
        world.get_mut::<Damageable>(victim).unwrap().apply(100);
        for (source, amount, cause) in [(SHOOTER, 100, rocket), (HELPER, 0, Cause::of(None, How::Hitscan))] {
            world.write_message(DamageDealt { target: victim, source, amount, remaining: 0, point: Vec3::ZERO, from: None, cause });
        }
        world.run_system_once(record_damage).unwrap();
        resolve(&mut world);

        let record = deaths(&mut world);
        assert_eq!((record[0].killer, record[0].cause), (SHOOTER, rocket));
    }

    /// Breaks if a death does not carry its killer's health as it stood, or names health for a
    /// killer with no body.
    #[test]
    fn a_death_carries_the_killers_health() {
        let mut world = a_world();
        let mut killer = Damageable::with_health(150);
        killer.apply(40);
        world.spawn((crate::game::player::Player::default(), PlayerId(1), killer));
        let victim = world.spawn((Damageable::with_health(100), DamageLog::default(), PlayerId(9))).id();
        world.get_mut::<Damageable>(victim).unwrap().apply(100);
        land(&mut world, victim, SHOOTER);
        resolve(&mut world);
        assert_eq!(deaths(&mut world)[0].killer_health, Some(HealthLeft { health: 110, max: 150 }));

        let mut world = a_world();
        let victim = world.spawn((Damageable::with_health(100), DamageLog::default(), PlayerId(9))).id();
        world.get_mut::<Damageable>(victim).unwrap().apply(100);
        land(&mut world, victim, HELPER);
        resolve(&mut world);
        assert_eq!(deaths(&mut world)[0].killer_health, None);
    }

    #[test]
    fn a_stale_contributor_earns_nothing() {
        let mut world = a_world();
        let victim = world.spawn((Damageable::with_health(100), DamageLog::default())).id();

        land(&mut world, victim, HELPER);

        advance(&mut world, ASSIST_WINDOW + 1.0);
        world.get_mut::<Damageable>(victim).unwrap().apply(100);
        land(&mut world, victim, SHOOTER);
        resolve(&mut world);

        let record = deaths(&mut world);
        assert_eq!(record[0].killer, SHOOTER);
        assert_eq!(record[0].assist, None);
    }

    #[test]
    fn a_prop_dies_like_anything_else() {
        let mut world = a_world();
        let mut crate_health = Damageable::with_health(20);
        crate_health.apply(20);
        let crate_entity = world.spawn((crate_health, DamageLog::default(), Name::new("Crate"))).id();

        resolve(&mut world);

        let record = deaths(&mut world);
        assert_eq!(record[0].victim_name.as_deref(), Some("Crate"));
        assert_eq!(record[0].victim_id, None);
        assert_eq!(record[0].killer, DamageSource::World);
        assert!(world.get_entity(crate_entity).is_err());
    }

    /// Credited to yourself, so it passes the team rule.
    #[test]
    fn suicide_is_enough_damage_at_yourself() {
        let mut world = a_world();
        world.init_resource::<Messages<Damage>>();
        let us = world
            .spawn((
                lightyear::prelude::input::native::ActionState(crate::game::player::PlayerInput { suicide: true, ..default() }),
                PlayerId(3),
                GlobalTransform::default(),
            ))
            .id();
        world.spawn((Inputs::default(), PlayerId(4), GlobalTransform::default()));

        world.run_system_once(suicide).unwrap();

        let messages = world.resource::<Messages<Damage>>();
        let written: Vec<Damage> = messages.get_cursor().read(messages).copied().collect();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].target, us);
        assert_eq!(written[0].source, DamageSource::Player(PlayerId(3)));
        assert!(written[0].amount >= 1000);
    }

    /// Fails if the ignite key lights anybody but the body that pressed it, or
    /// credits the afterburn to anybody else.
    #[test]
    fn igniting_sets_only_yourself_alight() {
        let mut world = a_world();
        let us = world
            .spawn((
                lightyear::prelude::input::native::ActionState(crate::game::player::PlayerInput { ignite: true, ..default() }),
                PlayerId(3),
            ))
            .id();
        let them = world.spawn((Inputs::default(), PlayerId(4))).id();

        world.run_system_once(ignite_yourself).unwrap();

        let burning = world.get::<crate::common::flame::Burning>(us).expect("not alight");
        assert_eq!(burning.source, DamageSource::Player(PlayerId(3)));
        assert!(burning.remaining_s > 0.0 && burning.damage > 0);
        assert!(world.get::<crate::common::flame::Burning>(them).is_none());
    }

    #[test]
    fn exploding_kills_yourself_through_the_torso() {
        let mut world = a_world();
        world.init_resource::<Messages<Kill>>();
        world.init_resource::<Messages<Explosion>>();
        world.insert_resource(crate::common::weapon_file::default_catalog());
        let us = world
            .spawn((
                lightyear::prelude::input::native::ActionState(crate::game::player::PlayerInput { explode: true, ..default() }),
                PlayerId(3),
                GlobalTransform::default(),
            ))
            .id();
        let them = world.spawn((Inputs::default(), PlayerId(4), GlobalTransform::default())).id();

        world.run_system_once(explode_yourself).unwrap();

        let messages = world.resource::<Messages<Kill>>();
        let written: Vec<Kill> = messages.get_cursor().read(messages).copied().collect();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].target, us);
        assert_eq!(written[0].source, DamageSource::Player(PlayerId(3)));
        assert_eq!(world.get::<crate::game::ragdoll::BlownApart>(us), Some(&crate::game::ragdoll::BlownApart::Torso));
        assert!(world.get::<crate::game::ragdoll::BlownApart>(them).is_none());

        let messages = world.resource::<Messages<Explosion>>();
        let blasts: Vec<Explosion> = messages.get_cursor().read(messages).copied().collect();
        assert_eq!(blasts.len(), 1);
        assert_eq!(blasts[0].source, DamageSource::Player(PlayerId(3)));
        assert!(blasts[0].knockback > 0.0 && blasts[0].radius > 0.0);
        assert_eq!(blasts[0].spare, Some(us), "the blast would hurt the body it tears apart");
    }

    /// A body in its dying tick is not killed a second time.
    #[test]
    fn a_dying_body_cannot_suicide_again() {
        let mut world = a_world();
        world.init_resource::<Messages<Damage>>();
        world.spawn((
            lightyear::prelude::input::native::ActionState(crate::game::player::PlayerInput { suicide: true, ..default() }),
            PlayerId(3),
            GlobalTransform::default(),
            Dying,
        ));

        world.run_system_once(suicide).unwrap();

        assert!(world.resource::<Messages<Damage>>().is_empty());
    }

    #[test]
    fn a_living_body_is_not_reaped() {
        let mut world = a_world();
        let alive = world.spawn((Damageable::with_health(100), DamageLog::default())).id();

        resolve(&mut world);

        assert!(world.get_entity(alive).is_ok());
        assert!(deaths(&mut world).is_empty());
    }

    /// Breaks if the body is despawned in the tick it died, before `Dying`
    /// has been replicated.
    #[test]
    fn a_dead_body_survives_the_tick_it_died_on() {
        let mut world = a_world();
        let mut health = Damageable::with_health(10);
        health.apply(10);
        let body = world.spawn((health, DamageLog::default())).id();

        world.run_system_once(announce_the_dead).unwrap();
        world.flush();
        assert!(
            world.get_entity(body).is_ok(),
            "the body went in the same tick it died; nobody else will see the zero"
        );
        assert_eq!(deaths(&mut world).len(), 1, "the death was not announced");

        world.run_system_once(reap_the_dead).unwrap();
        world.flush();
        assert!(world.get_entity(body).is_err(), "the body was never removed");
    }

    #[test]
    fn a_body_only_dies_once() {
        let mut world = a_world();
        let mut health = Damageable::with_health(10);
        health.apply(10);
        world.spawn((health, DamageLog::default()));

        resolve(&mut world);
        resolve(&mut world);

        assert_eq!(deaths(&mut world).len(), 1);
    }
}
