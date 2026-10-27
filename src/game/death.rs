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

use crate::common::damage::{Damage, DamageDealt, DamageLog, DamageSource, Damageable, Died, PlayerId};
use crate::common::skeleton::AnimationClock;
use crate::game::player::Inputs;

const SUICIDE_DAMAGE: u32 = 1000;

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
        log.record(hit.source, now);
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
) {
    let now = clock.seconds();
    for (entity, health, log, name, id, placed) in &bodies {
        if health.is_alive() {
            continue;
        }

        died.write(Died {
            victim: entity,
            victim_name: name.map(|name| name.to_string()),
            victim_id: id.copied(),
            killer: log.killer().unwrap_or(DamageSource::World),
            assist: log.assist(now),
            at: placed.map(|placed| placed.translation()).unwrap_or_default(),
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

/// The kill feed, such as it is.
pub fn announce_deaths(mut deaths: MessageReader<Died>) {
    for death in deaths.read() {
        let victim = match (&death.victim_name, death.victim_id) {
            (Some(name), Some(id)) => format!("{name} ({id})"),
            (Some(name), None) => name.clone(),
            (None, Some(id)) => id.to_string(),
            (None, None) => format!("{}", death.victim),
        };
        match death.assist {
            Some(assist) => info!("{victim} killed by {} (assist {assist})", death.killer),
            None => info!("{victim} killed by {}", death.killer),
        }
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

    /// Land one hit and let `record_damage` see it. Drained afterwards, since
    /// each `run_system_once` has a fresh cursor that would re-read it.
    fn land(world: &mut World, target: Entity, source: DamageSource) {
        world.write_message(DamageDealt {
            target,
            source,
            amount: 1,
            remaining: 0,
            point: Vec3::ZERO,
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
