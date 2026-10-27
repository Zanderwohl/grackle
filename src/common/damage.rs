//! Health, and the record of who took it off.
//!
//! [`Damageable`] is not a player component, so a crate or a door can have
//! health too. Every hit records its source, which is cheap now and a
//! migration later (see [`sketch.md`](../../documentation/sketch.md)).
//!
//! Which hitbox was hit is not recorded: falls, crushing and explosions have
//! none, and the zone has already chosen the number.

use std::fmt;

use bevy::ecs::entity::{EntityMapper, MapEntities};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Seconds of game time a hit still counts towards an assist. A guess at the
/// right order of magnitude; ultimately a gamemode's number.
pub const ASSIST_WINDOW: f32 = 8.0;

/// Placeholder: the same for every class until classes differ by more than a
/// name.
pub const DEFAULT_HEALTH: u32 = 100;

/// A match-local handle, not the account id, which only the play server knows
/// and which must not leak to every client that sees a hit.
///
/// Allocated by [`NextPlayerId`] rather than randomly, so replays agree.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Reflect, Serialize, Deserialize)]
pub struct PlayerId(pub u64);

impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "player {}", self.0)
    }
}

/// Never reuses an id within a session, or a kill would be credited to
/// whoever inherited it.
#[derive(Resource, Debug, Default)]
pub struct NextPlayerId(u64);

impl NextPlayerId {
    pub fn allocate(&mut self) -> PlayerId {
        let id = PlayerId(self.0);
        self.0 += 1;
        id
    }
}

/// An enum rather than a bare [`PlayerId`] because damage from the map (a
/// raised floor, a fall) has no player behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect, Serialize, Deserialize)]
pub enum DamageSource {
    Player(PlayerId),
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

/// Requires a [`DamageLog`], so anything that can die can say who killed it.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Reflect, Serialize, Deserialize)]
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

    /// For a fresh match, not a heal: healing will want a source and a cap.
    pub fn restore(&mut self) {
        self.health = self.max;
    }

    /// Returns what actually went on, capped at the maximum.
    pub fn heal(&mut self, amount: u32) -> u32 {
        let healed = amount.min(self.max - self.health);
        self.health += healed;
        healed
    }

    pub fn is_full(&self) -> bool {
        self.health >= self.max
    }

    /// Returns what actually came off, clamped to what was left, so scores add
    /// up to no more health than existed. Reaching zero does nothing here;
    /// death is decided elsewhere.
    pub fn apply(&mut self, amount: u32) -> u32 {
        let dealt = amount.min(self.health);
        self.health -= dealt;
        dealt
    }
}

/// Placeholder, like [`DEFAULT_HEALTH`]: per class once health is.
pub const DEFAULT_MAX_ARMOUR: u32 = 100;

/// The share of each hit armour takes while it lasts, as in Quake.
pub const ARMOUR_ABSORBS: f32 = 2.0 / 3.0;

/// A second pool in front of [`Damageable`]. Spent only by
/// [`apply_damage`](crate::game::damage::apply_damage). A body starts with
/// none.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Reflect, Serialize, Deserialize)]
pub struct Armour {
    points: u32,
    max: u32,
}

impl Default for Armour {
    fn default() -> Self {
        Self { points: 0, max: DEFAULT_MAX_ARMOUR }
    }
}

impl Armour {
    pub fn points(&self) -> u32 {
        self.points
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    pub fn is_full(&self) -> bool {
        self.points >= self.max
    }

    /// Returns what actually went on, capped at the maximum.
    pub fn add(&mut self, amount: u32) -> u32 {
        let added = amount.min(self.max - self.points.min(self.max));
        self.points += added;
        added
    }

    /// Takes [`ARMOUR_ABSORBS`] of a hit, rounded up so a 1-point hit is not
    /// free of armour for ever, and returns what is left for health.
    pub fn soak(&mut self, amount: u32) -> u32 {
        let wanted = (amount as f32 * ARMOUR_ABSORBS).ceil() as u32;
        let taken = wanted.min(self.points).min(amount);
        self.points -= taken;
        amount - taken
    }
}

/// Linear from 1 at the centre to 0 at `radius`. Shared by splash damage and
/// shove so a body is never thrown further than it was hurt. A radius `<= 0`
/// reaches nothing.
pub fn falloff(distance: f32, radius: f32) -> f32 {
    if radius <= 0.0 {
        return 0.0;
    }
    (1.0 - distance / radius).clamp(0.0, 1.0)
}

/// A request to hurt something. Only [`crate::game::damage::apply_damage`]
/// applies it, clamps it and writes the [`DamageDealt`] record.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct Damage {
    /// A target with no [`Damageable`] is skipped, not an error.
    pub target: Entity,
    pub source: DamageSource,
    /// Before the clamp.
    pub amount: u32,
    pub point: Vec3,
}

/// Anything that goes off writes one; only [`crate::game::explosion`] knows
/// what splash means. [`falloff`] applies to `damage` and `knockback` alike.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct Explosion {
    pub at: Vec3,
    pub radius: f32,
    /// At the centre.
    pub damage: u32,
    /// Metres per second of corpse shove at the centre.
    pub knockback: f32,
    pub source: DamageSource,
    /// The body hit squarely, which takes this amount instead of splash, not
    /// on top of it.
    pub direct: Option<(Entity, u32)>,
}

#[derive(Message, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DamageDealt {
    pub target: Entity,
    pub source: DamageSource,
    /// Already clamped.
    pub amount: u32,
    pub remaining: u32,
    pub point: Vec3,
}

/// The entity in a relayed record is the sender's and must be mapped, or a
/// damage number anchors to whatever local entity shares the index.
impl MapEntities for DamageDealt {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        self.target = mapper.get_mapped(self.target);
    }
}

impl MapEntities for Died {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        self.victim = mapper.get_mapped(self.victim);
    }
}

/// One entry per source, oldest first, pruned to [`ASSIST_WINDOW`]. Per
/// source because a kill credit asks who was involved, not how often. A `Vec`
/// rather than a pair, so later readers can see past the second contributor.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct DamageLog {
    contributors: Vec<(DamageSource, f32)>,
}

impl DamageLog {
    /// `at` is seconds of game time.
    pub fn record(&mut self, source: DamageSource, at: f32) {
        // A contributor is held at their latest hit.
        self.contributors
            .retain(|(who, when)| *who != source && at - *when <= ASSIST_WINDOW);
        self.contributors.push((source, at));
    }

    pub fn killer(&self) -> Option<DamageSource> {
        self.contributors.last().map(|(who, _)| *who)
    }

    /// The most recent other contributor, if still inside the window at `now`.
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

/// Carries the victim's name because the entity is gone by the time anything
/// reads this.
#[derive(Message, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Died {
    /// Already despawned; only useful for matching against hits in flight.
    pub victim: Entity,
    pub victim_name: Option<String>,
    pub victim_id: Option<PlayerId>,
    pub killer: DamageSource,
    pub assist: Option<DamageSource>,
    pub at: Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armour_takes_two_thirds_until_it_runs_out() {
        let mut armour = Armour::default();
        armour.add(10);
        assert_eq!(armour.soak(9), 3);
        assert_eq!(armour.points(), 4);
        assert_eq!(armour.soak(30), 26, "took more than it had");
        assert_eq!(armour.points(), 0);
        assert_eq!(armour.soak(30), 30);
    }

    #[test]
    fn armour_fills_to_its_maximum_and_no_further() {
        let mut armour = Armour::default();
        assert_eq!(armour.add(70), 70);
        assert_eq!(armour.add(70), DEFAULT_MAX_ARMOUR - 70);
        assert!(armour.is_full());
    }

    struct Shift;
    impl EntityMapper for Shift {
        fn get_mapped(&mut self, entity: Entity) -> Entity {
            Entity::from_raw_u32(entity.index().index() + 100).unwrap()
        }
        fn set_mapped(&mut self, _from: Entity, _to: Entity) {}
    }

    /// Fails if a relayed record's entity is not mapped to the receiver's.
    #[test]
    fn a_record_that_crosses_a_wire_has_its_entity_translated() {
        let target = Entity::from_raw_u32(7).unwrap();
        let mut hit = DamageDealt {
            target,
            source: A,
            amount: 30,
            remaining: 70,
            point: Vec3::new(1.0, 2.0, 3.0),
        };
        hit.map_entities(&mut Shift);
        assert_eq!(hit.target, Entity::from_raw_u32(107).unwrap());
        assert_eq!(hit.amount, 30);
        assert_eq!(hit.point, Vec3::new(1.0, 2.0, 3.0));

        let mut death = Died {
            victim: target,
            victim_name: Some("somebody".into()),
            victim_id: Some(PlayerId(4)),
            killer: A,
            assist: None,
            at: Vec3::ZERO,
        };
        death.map_entities(&mut Shift);
        assert_eq!(death.victim, Entity::from_raw_u32(107).unwrap());
        assert_eq!(death.victim_id, Some(PlayerId(4)), "the match identity is not an entity");
    }

    const A: DamageSource = DamageSource::Player(PlayerId(1));
    const B: DamageSource = DamageSource::Player(PlayerId(2));
    const C: DamageSource = DamageSource::Player(PlayerId(3));

    #[test]
    fn the_last_two_contributors_are_the_kill_and_the_assist() {
        let mut log = DamageLog::default();
        log.record(A, 0.0);
        log.record(B, 1.0);

        assert_eq!(log.killer(), Some(B));
        assert_eq!(log.assist(1.0), Some(A));
    }

    #[test]
    fn an_old_hit_does_not_earn_an_assist() {
        let mut log = DamageLog::default();
        log.record(A, 0.0);
        log.record(B, ASSIST_WINDOW + 1.0);

        assert_eq!(log.killer(), Some(B));
        assert_eq!(log.assist(ASSIST_WINDOW + 1.0), None);
    }

    #[test]
    fn one_person_alone_gets_no_assist() {
        let mut log = DamageLog::default();
        log.record(A, 0.0);
        log.record(A, 1.0);

        assert_eq!(log.killer(), Some(A));
        assert_eq!(log.assist(1.0), None);
    }

    #[test]
    fn hitting_again_moves_a_contributor_to_the_front() {
        let mut log = DamageLog::default();
        log.record(A, 0.0);
        log.record(B, 1.0);
        log.record(A, 2.0);

        assert_eq!(log.killer(), Some(A));
        assert_eq!(log.assist(2.0), Some(B));
    }

    #[test]
    fn a_third_contributor_does_not_displace_the_credit() {
        let mut log = DamageLog::default();
        log.record(C, 0.0);
        log.record(A, 1.0);
        log.record(B, 2.0);

        assert_eq!(log.killer(), Some(B));
        assert_eq!(log.assist(2.0), Some(A));
    }

    #[test]
    fn an_untouched_body_has_no_killer() {
        assert_eq!(DamageLog::default().killer(), None);
        assert_eq!(DamageLog::default().assist(0.0), None);
    }

    #[test]
    fn a_radial_effect_falls_off_to_nothing_at_its_edge() {
        assert_eq!(falloff(0.0, 4.0), 1.0);
        assert_eq!(falloff(2.0, 4.0), 0.5);
        assert_eq!(falloff(4.0, 4.0), 0.0);
        assert_eq!(falloff(9.0, 4.0), 0.0);
        assert_eq!(falloff(0.0, 0.0), 0.0);
    }

    #[test]
    fn a_hit_takes_what_it_asks_for() {
        let mut target = Damageable::with_health(100);
        assert_eq!(target.apply(30), 30);
        assert_eq!(target.health(), 70);
        assert_eq!(target.max(), 100);
    }

    #[test]
    fn overkill_is_recorded_as_what_was_left() {
        let mut target = Damageable::with_health(100);
        target.apply(80);
        assert_eq!(target.apply(90), 20);
        assert_eq!(target.health(), 0);
        assert!(!target.is_alive());
    }

    #[test]
    fn a_dead_body_takes_nothing_further() {
        let mut target = Damageable::with_health(30);
        target.apply(30);
        assert_eq!(target.apply(30), 0);
        assert_eq!(target.health(), 0);
    }

    #[test]
    fn restoring_a_body_puts_it_back_to_its_maximum() {
        let mut target = Damageable::with_health(150);
        target.apply(140);
        target.restore();
        assert_eq!(target.health(), 150);
        assert!(target.is_alive());
    }

    #[test]
    fn no_two_players_get_the_same_id() {
        let mut ids = NextPlayerId::default();
        let handed: Vec<PlayerId> = (0..4).map(|_| ids.allocate()).collect();

        let mut unique = handed.clone();
        unique.dedup();
        assert_eq!(handed, unique);
    }
}
