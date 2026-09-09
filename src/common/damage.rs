//! Health, and the record of who took it off.
//!
//! There is no damage *model* here yet — no falloff, no resistances, no death.
//! What there is, is the two things that are expensive to retrofit and cheap
//! to put in now:
//!
//! - **[`Damageable`] is not a player component.** A prop, a door, a payload
//!   cart and a person are all things a shot can take health off, and the
//!   moment health lives on `Player` the first breakable crate needs a second,
//!   parallel notion of it.
//! - **Every hit records its source.** [`sketch.md`](../../documentation/sketch.md)
//!   makes the same argument about items: provenance is two fields at the
//!   point of the grant and a data migration afterwards. Kills are the same
//!   shape — a scoreboard, a stat-tracking weapon and a kill feed all want to
//!   know *who*, and a `DamageDealt` that only said how much would be a record
//!   with the interesting column missing.
//!
//! What deliberately is *not* recorded is which hitbox was hit. The zone chose
//! the number; the record is the number. Damage arrives from falls, crushings
//! and explosions too, and a field only hitscan can fill would be `None` on
//! most of them.

use bevy::prelude::*;

/// Health a body starts with when nothing says otherwise.
///
/// One number for every class, which is a placeholder and not a design: per-
/// class health belongs on [`crate::common::class::Class`] beside the
/// proportions, once classes differ by more than a name.
pub const DEFAULT_HEALTH: u32 = 100;

/// Who a body is, for as long as a match lasts.
///
/// **Not the account id.** Accounts hands the *play server* an account id and
/// nothing else knows it (see the sketch); this is the match-local handle that
/// travels in a damage record, a kill feed and, eventually, over the wire.
/// Keeping them apart is what lets a kill feed exist before there is an
/// accounts server, and what stops an account id leaking to every client that
/// can see a hit.
///
/// Allocated by [`NextPlayerId`] rather than randomly: two clients replaying
/// the same match have to agree on who did what, and an id drawn from an RNG
/// is the one thing in a step that cannot be reconciled.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlayerId(pub u64);

/// The counter [`PlayerId`]s come from.
///
/// Monotonic and never reused inside a session: an id that came back after its
/// body was gone would credit a kill to whoever inherited it.
#[derive(Resource, Debug, Default)]
pub struct NextPlayerId(u64);

impl NextPlayerId {
    pub fn allocate(&mut self) -> PlayerId {
        let id = PlayerId(self.0);
        self.0 += 1;
        id
    }
}

/// What did the damage.
///
/// An enum rather than a bare [`PlayerId`] because the map is going to be one
/// of the answers: this is a game about editing the level mid-match, and being
/// crushed by a floor somebody raised has no player behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageSource {
    Player(PlayerId),
    /// The map, gravity, or anything else with no-one to credit.
    World,
}

/// Something with health.
///
/// Requires nothing and is required by the markers that mean a real body, so
/// putting it on a crate is inserting one component.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Damageable {
    health: u32,
    max: u32,
}

impl Default for Damageable {
    fn default() -> Self {
        Self::with_health(DEFAULT_HEALTH)
    }
}

impl Damageable {
    pub fn with_health(max: u32) -> Self {
        Self { health: max, max }
    }

    pub fn health(&self) -> u32 {
        self.health
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    pub fn is_alive(&self) -> bool {
        self.health > 0
    }

    /// Back to full.
    ///
    /// What a fresh match does to every body on the map — see
    /// [`crate::game::reset`]. Not a heal: there is no healing, and when there
    /// is it will want a source and a cap of its own.
    pub fn restore(&mut self) {
        self.health = self.max;
    }

    /// Take `amount` off, and answer with how much actually came off.
    ///
    /// Clamped to what is left, so a 90-damage shot into 20 remaining health
    /// is recorded as 20. The number that gets shown, scored and sent is the
    /// damage *done*, not the damage attempted — a scoreboard adding up
    /// attempts would have everyone dealing more than the health on the map.
    ///
    /// Reaching zero does nothing on purpose. Death is a gamemode question —
    /// respawn timers, who gets the credit, whether the body ragdolls — and
    /// answering it here would answer it for every gamemode at once.
    pub fn apply(&mut self, amount: u32) -> u32 {
        let dealt = amount.min(self.health);
        self.health -= dealt;
        dealt
    }
}

/// A hit that landed, after the fact.
///
/// Written by whatever did the damage and read by anything that cares — the
/// floating numbers today, a kill feed and a scoreboard later. A message
/// rather than a method call so that the list of things watching can grow
/// without the thing shooting knowing about any of them.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct DamageDealt {
    /// The body that took it.
    pub target: Entity,
    pub source: DamageSource,
    /// What actually came off, already clamped to what was left.
    pub amount: u32,
    /// Health remaining afterwards. Zero is dead in every sense except that
    /// nothing has happened about it yet.
    pub remaining: u32,
    /// Where in the world it landed, so feedback can be drawn there.
    pub point: Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hit_takes_what_it_asks_for() {
        let mut target = Damageable::with_health(100);
        assert_eq!(target.apply(30), 30);
        assert_eq!(target.health(), 70);
        assert_eq!(target.max(), 100);
    }

    /// The clamp: overkill is recorded as what was there, not as what was
    /// swung. A scoreboard adding up attempts would have a match dealing more
    /// damage than there was health in it.
    #[test]
    fn overkill_is_recorded_as_what_was_left() {
        let mut target = Damageable::with_health(100);
        target.apply(80);
        assert_eq!(target.apply(90), 20);
        assert_eq!(target.health(), 0);
        assert!(!target.is_alive());
    }

    /// And a body at zero stays there rather than going negative — nothing
    /// happens at zero yet, and "how dead" is not a quantity.
    #[test]
    fn a_dead_body_takes_nothing_further() {
        let mut target = Damageable::with_health(30);
        target.apply(30);
        assert_eq!(target.apply(30), 0);
        assert_eq!(target.health(), 0);
    }

    /// A body put back to full is back to full, however far down it was.
    #[test]
    fn restoring_a_body_puts_it_back_to_its_maximum() {
        let mut target = Damageable::with_health(150);
        target.apply(140);
        target.restore();
        assert_eq!(target.health(), 150);
        assert!(target.is_alive());
    }

    /// Ids are handed out once each. A reused id credits a kill to whoever
    /// inherited it.
    #[test]
    fn no_two_players_get_the_same_id() {
        let mut ids = NextPlayerId::default();
        let handed: Vec<PlayerId> = (0..4).map(|_| ids.allocate()).collect();

        let mut unique = handed.clone();
        unique.dedup();
        assert_eq!(handed, unique);
    }
}
