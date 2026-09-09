//! Running out of health, and what gets said about it.
//!
//! Three systems in a row on the tick, and the order is the whole design:
//!
//! 1. `record_damage` writes every hit into the victim's [`DamageLog`].
//! 2. `reap_the_dead` removes anything at zero and writes [`Died`].
//! 3. `announce_deaths` says so.
//!
//! The first has to happen before the second **within the same tick**, or a
//! body killed by the shot that was just fired is reaped before the log knows
//! who fired it, and every kill is credited to nobody.
//!
//! Death is a property of health, not of being a player: `reap_the_dead` asks
//! the world for [`Damageable`]s at zero and does not care what they are. A
//! crate shot to pieces takes the same path a person does, and the message
//! that comes out says so.
//!
//! **What happens after the entity goes is nobody's business here.** No
//! respawn, no ragdoll, no score. Those are gamemode questions, and answering
//! any of them at this layer would answer it for every gamemode at once.

use bevy::prelude::*;

use crate::common::damage::{DamageDealt, DamageLog, DamageSource, Damageable, Died, PlayerId};
use crate::common::skeleton::AnimationClock;

/// Put every hit into the victim's log, stamped with the tick it landed on.
///
/// Game time rather than wall time: it is the clock that stops with the match
/// and starts again from zero, so an assist window means the same eight
/// seconds on every machine and does not quietly expire while somebody is in
/// the editor.
pub fn record_damage(
    clock: Res<AnimationClock>,
    mut hits: MessageReader<DamageDealt>,
    mut logs: Query<&mut DamageLog>,
) {
    let now = clock.seconds();
    for hit in hits.read() {
        // A target that has already been reaped this tick. Its log went with
        // it, and there is nothing left to credit.
        let Ok(mut log) = logs.get_mut(hit.target) else { continue };
        log.record(hit.source, now);
    }
}

/// Remove everything that has run out of health, and say who did it.
///
/// The whole entity, children included — a player's camera hangs off its body,
/// which is a thing to know before anything can kill a player. Nothing can
/// today: you cannot shoot yourself and there is no second shooter.
pub fn reap_the_dead(
    mut commands: Commands,
    clock: Res<AnimationClock>,
    mut died: MessageWriter<Died>,
    bodies: Query<(
        Entity,
        &Damageable,
        &DamageLog,
        Option<&Name>,
        Option<&PlayerId>,
        Option<&GlobalTransform>,
    )>,
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
            // Nothing in the log means nothing hurt it — spawned at zero, or
            // killed by something that has not learned to sign its work.
            killer: log.killer().unwrap_or(DamageSource::World),
            assist: log.assist(now),
            at: placed.map(|placed| placed.translation()).unwrap_or_default(),
        });

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

    /// Advance game time by whole ticks, since that is the only way it moves.
    fn advance(world: &mut World, seconds: f32) {
        world.resource_mut::<AnimationClock>().advance(seconds);
    }

    /// Land one hit and let `record_damage` see it.
    ///
    /// The queue is drained afterwards because `run_system_once` builds a
    /// fresh system, and a fresh system has a fresh cursor that would read
    /// every message written so far all over again.
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

    /// The ordinary case, end to end: two people hurt a body, the second one
    /// finishes it, and the record says so.
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
        world.run_system_once(reap_the_dead).unwrap();
        world.flush();

        let record = deaths(&mut world);
        assert_eq!(record.len(), 1);
        assert_eq!(record[0].killer, SHOOTER);
        assert_eq!(record[0].assist, Some(HELPER));
        assert_eq!(record[0].victim_name.as_deref(), Some("Scout"));
        assert_eq!(record[0].victim_id, Some(PlayerId(9)));
        assert!(world.get_entity(victim).is_err(), "the body was not removed");
    }

    /// A hit from before the window is not an assist, and the kill still
    /// stands on its own.
    #[test]
    fn a_stale_contributor_earns_nothing() {
        let mut world = a_world();
        let victim = world.spawn((Damageable::with_health(100), DamageLog::default())).id();

        land(&mut world, victim, HELPER);

        advance(&mut world, ASSIST_WINDOW + 1.0);
        world.get_mut::<Damageable>(victim).unwrap().apply(100);
        land(&mut world, victim, SHOOTER);
        world.run_system_once(reap_the_dead).unwrap();

        let record = deaths(&mut world);
        assert_eq!(record[0].killer, SHOOTER);
        assert_eq!(record[0].assist, None);
    }

    /// Anything with health, not just a person. A crate is reaped the same
    /// way and produces the same message.
    #[test]
    fn a_prop_dies_like_anything_else() {
        let mut world = a_world();
        let mut crate_health = Damageable::with_health(20);
        crate_health.apply(20);
        let crate_entity = world.spawn((crate_health, DamageLog::default(), Name::new("Crate"))).id();

        world.run_system_once(reap_the_dead).unwrap();
        world.flush();

        let record = deaths(&mut world);
        assert_eq!(record[0].victim_name.as_deref(), Some("Crate"));
        assert_eq!(record[0].victim_id, None);
        // Nothing signed for it, so there is nobody to credit.
        assert_eq!(record[0].killer, DamageSource::World);
        assert!(world.get_entity(crate_entity).is_err());
    }

    /// A body with health left is left alone, which is most of them most of
    /// the time.
    #[test]
    fn a_living_body_is_not_reaped() {
        let mut world = a_world();
        let alive = world.spawn((Damageable::with_health(100), DamageLog::default())).id();

        world.run_system_once(reap_the_dead).unwrap();
        world.flush();

        assert!(world.get_entity(alive).is_ok());
        assert!(deaths(&mut world).is_empty());
    }

    /// A death is announced once. Reaping is what removes the body, so a
    /// second pass has nothing left to find — the check that a corpse does not
    /// keep dying every tick.
    #[test]
    fn a_body_only_dies_once() {
        let mut world = a_world();
        let mut health = Damageable::with_health(10);
        health.apply(10);
        world.spawn((health, DamageLog::default()));

        world.run_system_once(reap_the_dead).unwrap();
        world.flush();
        world.run_system_once(reap_the_dead).unwrap();
        world.flush();

        assert_eq!(deaths(&mut world).len(), 1);
    }
}
