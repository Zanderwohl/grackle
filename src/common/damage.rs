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

use std::fmt;

use bevy::prelude::*;

/// How long a hit still counts towards an assist, in seconds of game time.
///
/// Long enough that softening somebody up and letting a teammate finish is
/// credited, short enough that a hit landed at the other end of the round is
/// not. Eight seconds is a guess in the right order of magnitude and nothing
/// more; it is a gamemode's number in the end.
pub const ASSIST_WINDOW: f32 = 8.0;

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

impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "player {}", self.0)
    }
}

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

impl fmt::Display for DamageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DamageSource::Player(id) => write!(f, "{id}"),
            DamageSource::World => write!(f, "the map"),
        }
    }
}

/// Something with health.
///
/// Required by the markers that mean a real body, so putting it on a crate is
/// inserting one component. It in turn requires a [`DamageLog`]: anything that
/// can be killed has to be able to say who did it, and a health pool with no
/// log would be one that died anonymously.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[require(DamageLog)]
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

/// Who has hurt this body lately, and when.
///
/// One entry per source, oldest first, pruned to [`ASSIST_WINDOW`]. Per source
/// rather than per hit because what a kill credit asks is "who was involved",
/// and somebody who landed nine shots is no more involved than somebody who
/// landed one — they are just the more recent.
///
/// A `Vec` and not a fixed pair: two is what a kill feed shows, but the log is
/// what a scoreboard, a revenge system and an assist rule all read, and
/// throwing away the third contributor at the point of recording would be
/// deciding all of those here.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct DamageLog {
    contributors: Vec<(DamageSource, f32)>,
}

impl DamageLog {
    /// Note that `source` hurt this body at `at` seconds of game time.
    pub fn record(&mut self, source: DamageSource, at: f32) {
        // Drop this source's older entry along with anything stale: a
        // contributor is held at their *latest* hit, so a long fight does not
        // leave somebody credited for where they came in.
        self.contributors
            .retain(|(who, when)| *who != source && at - *when <= ASSIST_WINDOW);
        self.contributors.push((source, at));
    }

    /// Who landed the last hit. `None` for a body nothing has touched.
    pub fn killer(&self) -> Option<DamageSource> {
        self.contributors.last().map(|(who, _)| *who)
    }

    /// The most recent *other* contributor, if their hit is still inside the
    /// window at `now`.
    ///
    /// One, deliberately. "A second person also did damage recently" is the
    /// question a kill feed asks; who else was in the fight is a different one
    /// and the log still answers it.
    pub fn assist(&self, now: f32) -> Option<DamageSource> {
        self.contributors
            .iter()
            .rev()
            .nth(1)
            .filter(|(_, when)| now - *when <= ASSIST_WINDOW)
            .map(|(who, _)| *who)
    }

    pub fn clear(&mut self) {
        self.contributors.clear();
    }
}

/// A body that ran out of health, written as it is removed.
///
/// Carries the victim's *name* rather than only its entity, because by the
/// time anything reads this the entity is gone — a kill feed that looked the
/// victim up would find nothing to look up.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct Died {
    /// The entity that was despawned. Useful for matching a death to a hit
    /// already in flight; useless for asking the world anything about it.
    pub victim: Entity,
    pub victim_name: Option<String>,
    /// The victim's match identity, if it had one. A prop does not.
    pub victim_id: Option<PlayerId>,
    /// Whoever landed the last hit. [`DamageSource::World`] when there is
    /// no-one to credit.
    pub killer: DamageSource,
    pub assist: Option<DamageSource>,
    /// Where it died.
    pub at: Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: DamageSource = DamageSource::Player(PlayerId(1));
    const B: DamageSource = DamageSource::Player(PlayerId(2));
    const C: DamageSource = DamageSource::Player(PlayerId(3));

    /// The last hit is the kill and the one before it is the assist.
    #[test]
    fn the_last_two_contributors_are_the_kill_and_the_assist() {
        let mut log = DamageLog::default();
        log.record(A, 0.0);
        log.record(B, 1.0);

        assert_eq!(log.killer(), Some(B));
        assert_eq!(log.assist(1.0), Some(A));
    }

    /// A hit from long enough ago is not an assist. This is the whole reason
    /// the log carries times rather than just an order.
    #[test]
    fn an_old_hit_does_not_earn_an_assist() {
        let mut log = DamageLog::default();
        log.record(A, 0.0);
        log.record(B, ASSIST_WINDOW + 1.0);

        assert_eq!(log.killer(), Some(B));
        assert_eq!(log.assist(ASSIST_WINDOW + 1.0), None);
    }

    /// Shooting somebody twice is not an assist to yourself.
    #[test]
    fn one_person_alone_gets_no_assist() {
        let mut log = DamageLog::default();
        log.record(A, 0.0);
        log.record(A, 1.0);

        assert_eq!(log.killer(), Some(A));
        assert_eq!(log.assist(1.0), None);
    }

    /// A contributor is held at their latest hit, so coming back into a fight
    /// moves you up the order rather than leaving you where you came in.
    #[test]
    fn hitting_again_moves_a_contributor_to_the_front() {
        let mut log = DamageLog::default();
        log.record(A, 0.0);
        log.record(B, 1.0);
        log.record(A, 2.0);

        assert_eq!(log.killer(), Some(A));
        assert_eq!(log.assist(2.0), Some(B));
    }

    /// Three in a fight: the two most recent are the ones a kill feed shows,
    /// and the third is still in the log for whatever wants it.
    #[test]
    fn a_third_contributor_does_not_displace_the_credit() {
        let mut log = DamageLog::default();
        log.record(C, 0.0);
        log.record(A, 1.0);
        log.record(B, 2.0);

        assert_eq!(log.killer(), Some(B));
        assert_eq!(log.assist(2.0), Some(A));
    }

    /// Nothing has touched it, so there is nobody to credit.
    #[test]
    fn an_untouched_body_has_no_killer() {
        assert_eq!(DamageLog::default().killer(), None);
        assert_eq!(DamageLog::default().assist(0.0), None);
    }

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
