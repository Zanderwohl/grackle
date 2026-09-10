//! Putting the world back to the start of a match.
//!
//! **One list, in one place, on purpose.** Entering Play resets several things
//! that live in several modules, and the failure mode of spreading that across
//! them is not a compile error — it is a body that comes back into a new match
//! still carrying the last one's damage, and nobody noticing for a month. If
//! you add state that a match owns, add it here.
//!
//! What is reset is *match* state. What is deliberately left alone:
//!
//! - **[`NextPlayerId`](crate::common::damage::NextPlayerId).** The one thing
//!   here that must never be reset. Ids are unique for the life of the
//!   process; handing the same id to the body after this one is what makes a
//!   kill feed credit the wrong player.
//! - **View and debug toggles** — `ViewMode`, `ShowHitboxes`, `ShowBones`, the
//!   perf overlay. They are how you are looking at the game, not part of it,
//!   and a reset that put you back in first person every time you pressed F5
//!   would be a reset nobody wanted.
//! - **The map.** Rooms, lights and features belong to the document, not to
//!   the match. Resetting the map on play would undo the edit you pressed F5
//!   to go and look at, which is the whole feature.

use bevy::prelude::*;

use crate::common::damage::{DamageLog, Damageable};
use crate::common::flame::Burning;
use crate::common::skeleton::{AnimationClock, Gait, SkeletonAnimator};
use crate::game::damage::DamageNumber;
use crate::game::player::InputLatch;
use crate::game::projectile::{Projectile, ProjectileEmitter};
use crate::game::ragdoll::Ragdoll;

/// Everything a new match starts from scratch.
///
/// Runs on entering Play, before the body is spawned. Ordering against the
/// spawn barely matters — a body created afterwards is created fresh — but
/// "clear the table, then set it" is the order that stays correct when
/// something is added to either half.
pub fn reset_for_play(
    mut commands: Commands,
    mut clock: ResMut<AnimationClock>,
    mut latch: ResMut<InputLatch>,
    mut bodies: Query<(&mut Gait, &mut SkeletonAnimator)>,
    mut health: Query<(&mut Damageable, &mut DamageLog)>,
    numbers: Query<Entity, With<DamageNumber>>,
    corpses: Query<Entity, With<Ragdoll>>,
    in_flight: Query<Entity, Or<(With<Projectile>, With<ProjectileEmitter>)>>,
    alight: Query<Entity, With<Burning>>,
) {
    // Back to zero, so two runs of the same map put every body at the same
    // point in its cycle. A match that started at whatever second the editor
    // happened to be left at could not be compared with the one before it.
    *clock = AnimationClock::default();

    // A click or a held key made while editing is not an order to shoot on
    // spawn. The bodies are new and carry nothing, but the latch is a fact
    // about the keyboard rather than about a body, so it outlives them both.
    *latch = InputLatch::default();

    for (mut gait, mut animator) in &mut bodies {
        // Standing still at the start of a stride, in a state rather than part
        // way between two.
        gait.reset();
        *animator = SkeletonAnimator::default();
    }

    for (mut body, mut log) in &mut health {
        body.restore();
        // Who hurt it last match is not who hurt it this match. A stale log
        // would credit the first kill of a new round to whoever was shooting
        // when the last one ended.
        log.clear();
    }

    // Feedback from the last match, hanging in the air over bodies that are
    // now at full health.
    for number in &numbers {
        commands.entity(number).despawn();
    }

    // And the bodies themselves. Everything that died last match is alive
    // again by the line above, so a corpse of it would be a second copy of
    // somebody standing a few feet away.
    for corpse in &corpses {
        commands.entity(corpse).despawn();
    }

    // And anything the last match left in the air, along with the emitters
    // that were putting it there. A rocket fired a moment before F5 would
    // otherwise arrive in the new match, from a shooter that no longer exists.
    for entity in &in_flight {
        commands.entity(entity).despawn();
    }

    // And put out anything still burning. The health is back by the loop
    // above, so a fire carried over would be a body at full health taking
    // damage for something that happened last match.
    for body in &alight {
        commands.entity(body).remove::<Burning>();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// The whole list at once: a world left in the state a match leaves it in,
    /// put back.
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
