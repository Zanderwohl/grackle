//! Putting the world back to the start of a match. State a match owns is reset
//! here, in one list.
//!
//! Left alone on purpose: [`NextPlayerId`](crate::common::damage::NextPlayerId)
//! (ids are unique for the process, or the kill feed credits the wrong
//! player), view and debug toggles, and the map.

use bevy::prelude::*;

use crate::common::damage::{Armor, DamageLog, Damageable};
use crate::common::flame::Burning;
use crate::common::skeleton::{AnimationClock, Gait, SkeletonAnimator};
use crate::game::damage::DamageNumber;
use crate::game::player::InputLatch;
use crate::game::projectile::{Projectile, ProjectileEmitter};
use crate::game::ragdoll::Ragdoll;

/// Everything a new match starts from scratch. Runs on entering Play, before
/// any body is spawned.
pub fn reset_for_play(
    mut commands: Commands,
    mut clock: ResMut<AnimationClock>,
    mut latch: ResMut<InputLatch>,
    role: Option<Res<crate::common::net::NetRole>>,
    mut bodies: Query<(&mut Gait, &mut SkeletonAnimator)>,
    mut health: Query<(&mut Damageable, &mut DamageLog, Option<&mut Armor>)>,
    numbers: Query<Entity, With<DamageNumber>>,
    corpses: Query<Entity, With<Ragdoll>>,
    in_flight: Query<Entity, Or<(With<Projectile>, With<ProjectileEmitter>)>>,
    alight: Query<Entity, With<Burning>>,
) {
    // So two runs of a map put every body at the same point in its cycle.
    *clock = AnimationClock::default();

    // A click made while editing must not fire on spawn. Aim is kept:
    // `aim_the_view_at_our_body` seeds it from the new body.
    latch.0.release_controls();

    for (mut gait, mut animator) in &mut bodies {
        gait.reset();
        *animator = SkeletonAnimator::default();
    }

    // Health is replicated, so only the authority restores it.
    let authority = role.is_none_or(|role| role.is_authority());
    if authority {
        for (mut body, mut log, armor) in &mut health {
            body.restore();
            log.clear();
            if let Some(mut armor) = armor {
                *armor = Armor::with_max(armor.max());
            }
        }
    }

    for number in &numbers {
        commands.entity(number).despawn();
    }

    // Corpses are replicated; a client's copies go with the server's despawn.
    if authority {
        for corpse in &corpses {
            commands.entity(corpse).despawn();
        }
    }

    for entity in &in_flight {
        commands.entity(entity).despawn();
    }

    for body in &alight {
        commands.entity(body).remove::<Burning>();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    #[test]
    fn entering_play_clears_the_last_match() {
        let mut world = World::new();

        let mut clock = AnimationClock::default();
        clock.advance(1.0 / 64.0);
        world.insert_resource(clock);
        world.insert_resource(InputLatch(crate::game::player::PlayerInput {
            attack: true,
            jump: true,
            ..default()
        }));

        let mut hurt = Damageable::with_health(100);
        hurt.apply(70);
        let mut log = DamageLog::default();
        log.record(crate::common::damage::DamageSource::World, 0.0);
        let body = world
            .spawn((hurt, log, Gait::default(), SkeletonAnimator::default()))
            .id();
        world.spawn(DamageNumber { amount: 70, at: Vec3::ZERO, remaining: 1.0 });

        world.run_system_once(reset_for_play).unwrap();

        assert_eq!(world.resource::<AnimationClock>().ticks(), 0);
        let latch = world.resource::<InputLatch>().0;
        assert!(!latch.attack, "a click made while editing fired on spawn");
        assert!(!latch.jump);
        assert_eq!(world.get::<Damageable>(body).unwrap().health(), 100);
        assert_eq!(world.get::<DamageLog>(body).unwrap().killer(), None, "last match's shooter carried over");
        assert_eq!(world.query::<&DamageNumber>().iter(&world).count(), 0);
    }
}
