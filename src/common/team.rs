//! Whose side a body is on, and what that does to a shot.
//!
//! One component, [`Team`], and one rule, [`hostile`]. The rule is stated once
//! and read from the one system that touches a health pool
//! ([`apply_damage`](crate::game::damage::apply_damage)), for the same reason
//! damage itself is a message: a weapon that decided for itself whether its
//! target was a friend would be a weapon that could get it wrong on its own.
//!
//! Three cases, and the third is the one that is easy to lose:
//!
//! - **Two teams that differ** hurt each other, as you would expect.
//! - **A body with no team** hurts and is hurt by everybody. That is not an
//!   oversight to be tightened later — a breakable crate, a training dummy and
//!   the map itself have no side, and making them pick one would mean deciding
//!   which team a door belongs to.
//! - **You always hurt yourself.** Rocket jumping is the reason this exists,
//!   and it is stated as a *self* exception rather than as "splash ignores
//!   teams", so a rocket at your own feet costs you and costs the teammate
//!   beside you nothing.
//!
//! Teams are matched on a [`PlayerId`], not on an entity, because that is what
//! a [`DamageSource`] carries — the shooter may be a corpse, or gone from the
//! match, by the time an afterburn tick lands. [`Allegiances`] is the lookup,
//! and it is a `SystemParam` so that both the damage gate and the flamethrower
//! ask the same code the same question.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::common::damage::{DamageSource, PlayerId};
use crate::get;

/// Whose side a body is on.
///
/// Four, which is the shape of the game the sketch describes: teams edit the
/// map between rounds, and two teams is a tug of war where four is a scramble.
/// Nothing here says how many are *in play* — a gamemode picks that, and a
/// two-team mode simply never hands out the other two.
///
/// Serializable because a feature can name one — the animation grid's roster
/// does. What goes on disk is a *number* chosen by whatever stores it, not
/// this derive's names: see `RosterTeam::index`.
#[derive(
    Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, EnumIter,
)]
pub enum Team {
    Red,
    Blue,
    Yellow,
    Green,
}

impl Team {
    /// Every team, in a fixed order, so anything that cycles or deals them out
    /// does it the same way on every machine.
    pub const ALL: [Team; 4] = [Team::Red, Team::Blue, Team::Yellow, Team::Green];

    /// What a body on this team is drawn in.
    ///
    /// Desaturated on purpose: these are the base colour of a whole body under
    /// a map's lighting, not a UI swatch, and four fully saturated primaries
    /// read as toys. They also have to stay apart from each other at a
    /// distance and under a coloured light, which is what pulls yellow towards
    /// amber and green away from blue.
    pub fn colour(self) -> Color {
        match self {
            Team::Red => Color::srgb(0.72, 0.20, 0.18),
            Team::Blue => Color::srgb(0.20, 0.38, 0.72),
            Team::Yellow => Color::srgb(0.80, 0.66, 0.16),
            Team::Green => Color::srgb(0.24, 0.56, 0.26),
        }
    }

    pub fn name(self) -> String {
        match self {
            Team::Red => get!("team.names.red"),
            Team::Blue => get!("team.names.blue"),
            Team::Yellow => get!("team.names.yellow"),
            Team::Green => get!("team.names.green"),
        }
    }
}

/// May a body on `source`'s side hurt a body on `target`'s side?
///
/// The whole rule, as a function of two options so it can be reasoned about
/// without a `World`. `None` on either side means "no side", which is
/// hostile to everything — see the module docs. The self exception is not
/// here, because two teams cannot express it: it is applied by
/// [`Allegiances::may_hurt`], which knows who is who.
pub fn hostile(source: Option<Team>, target: Option<Team>) -> bool {
    match (source, target) {
        (Some(source), Some(target)) => source != target,
        _ => true,
    }
}

/// Who is on whose side, as one question anything can ask.
///
/// A `SystemParam` rather than a resource because a team is a component and
/// the answer has to stay true the moment somebody switches sides; a mirror
/// resource would be a second copy to forget to update. Both queries are
/// read-only, so this composes with a system that is writing health.
#[derive(SystemParam)]
pub struct Allegiances<'w, 's> {
    /// Everything, matched on its team and its match identity — both optional,
    /// because a crate has neither.
    bodies: Query<'w, 's, (Option<&'static Team>, Option<&'static PlayerId>)>,
    /// Only the things with a match identity, which is what a [`DamageSource`]
    /// names.
    players: Query<'w, 's, (&'static PlayerId, Option<&'static Team>)>,
}

impl Allegiances<'_, '_> {
    /// What side `id` is on, if it is still in the world and has one.
    pub fn team_of(&self, id: PlayerId) -> Option<Team> {
        self.players
            .iter()
            .find(|(player, _)| **player == id)
            .and_then(|(_, team)| team.copied())
    }

    /// What side whatever did the damage is on. The map has none, and neither
    /// does a shooter who has left.
    pub fn team_of_source(&self, source: DamageSource) -> Option<Team> {
        match source {
            DamageSource::Player(id) => self.team_of(id),
            DamageSource::World => None,
        }
    }

    /// **The gate.** May `source` hurt `target`?
    ///
    /// Yes for anything not in the world — a target that has been despawned
    /// between the shot and the tick is somebody else's problem, and
    /// [`apply_damage`](crate::game::damage::apply_damage) drops it for having
    /// no health rather than for having no team.
    pub fn may_hurt(&self, source: DamageSource, target: Entity) -> bool {
        let Ok((team, id)) = self.bodies.get(target) else { return true };

        // Your own splash is yours. Checked before the teams, because a
        // player is trivially on their own team and the team rule would
        // otherwise make rocket jumping free.
        if let (DamageSource::Player(shooter), Some(id)) = (source, id)
            && shooter == *id
        {
            return true;
        }

        hostile(self.team_of_source(source), team.copied())
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use strum::IntoEnumIterator;

    use super::*;
    use crate::common::damage::Damageable;

    /// The rule, without a world: different sides fight, and anything with no
    /// side fights everybody.
    #[test]
    fn only_a_shared_team_stops_a_hit() {
        assert!(!hostile(Some(Team::Red), Some(Team::Red)));
        assert!(hostile(Some(Team::Red), Some(Team::Blue)));
        assert!(hostile(None, Some(Team::Blue)), "a crate cannot hurt a player");
        assert!(hostile(Some(Team::Blue), None), "a player cannot hurt a crate");
        assert!(hostile(None, None));
    }

    /// Four teams, four colours, and no two of them the same — the colour is
    /// how you tell a friend from a target at a distance, so a duplicate is a
    /// team you cannot see.
    #[test]
    fn every_team_has_its_own_colour() {
        let mut seen: Vec<[u8; 4]> = Team::iter().map(|t| t.colour().to_srgba().to_u8_array()).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), Team::ALL.len());
    }

    /// `ALL` is the whole enum. It is hand-written, so it is worth checking it
    /// against the derive that knows.
    #[test]
    fn all_is_every_team() {
        assert_eq!(Team::iter().collect::<Vec<_>>(), Team::ALL.to_vec());
    }

    /// A world with one of each of the interesting bodies in it, and the
    /// question asked of every pairing.
    #[test]
    fn a_shot_reaches_everyone_but_a_teammate() {
        let mut world = World::new();
        let me = PlayerId(1);
        let mine = world.spawn((Damageable::default(), me, Team::Red)).id();
        let ally = world.spawn((Damageable::default(), PlayerId(2), Team::Red)).id();
        let enemy = world.spawn((Damageable::default(), PlayerId(3), Team::Blue)).id();
        let crate_ = world.spawn(Damageable::default()).id();

        let answers = world
            .run_system_once(move |allegiances: Allegiances| {
                let source = DamageSource::Player(me);
                [
                    allegiances.may_hurt(source, mine),
                    allegiances.may_hurt(source, ally),
                    allegiances.may_hurt(source, enemy),
                    allegiances.may_hurt(source, crate_),
                    allegiances.may_hurt(DamageSource::World, ally),
                ]
            })
            .unwrap();

        assert!(answers[0], "own splash has to hurt, or there is no rocket jumping");
        assert!(!answers[1], "shot a teammate");
        assert!(answers[2]);
        assert!(answers[3], "a crate has no side to be on");
        assert!(answers[4], "the map plays for nobody");
    }
}
