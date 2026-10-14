//! Whose side a body is on, and what that does to a shot.
//!
//! The rule is read only by
//! [`apply_damage`](crate::game::damage::apply_damage) (and the flamethrower,
//! which lights bodies without a `Damage`), so no weapon decides for itself:
//!
//! - Two teams that differ hurt each other.
//! - A body with no team hurts and is hurt by everybody: a crate or the map
//!   has no side, deliberately.
//! - You always hurt yourself, as a self exception rather than "splash ignores
//!   teams", so rocket jumping costs you and not the teammate beside you.
//!
//! Teams are matched on a [`PlayerId`], not an entity, because the shooter in
//! a [`DamageSource`] may be a corpse or gone by the time afterburn lands.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::common::damage::{DamageSource, PlayerId};
use crate::get;

/// Whose side a body is on. How many are in play is a gamemode's choice.
///
/// On disk, a feature stores a number of its own choosing, not this derive's
/// names: see `RosterTeam::index`.
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
    /// Fixed order, so dealing teams out agrees on every machine.
    pub const ALL: [Team; 4] = [Team::Red, Team::Blue, Team::Yellow, Team::Green];

    /// Desaturated: the base colour of a whole body under map lighting. Yellow is
    /// pulled towards amber and green away from blue so the four stay apart at a
    /// distance and under coloured light.
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

/// `None` means no side, hostile to everything. The self exception needs
/// identities and lives in [`Allegiances::may_hurt`].
pub fn hostile(source: Option<Team>, target: Option<Team>) -> bool {
    match (source, target) {
        (Some(source), Some(target)) => source != target,
        _ => true,
    }
}

/// Who is on whose side. A `SystemParam` over the components rather than a
/// mirror resource that could go stale; read-only, so it composes with a
/// system writing health.
#[derive(SystemParam)]
pub struct Allegiances<'w, 's> {
    bodies: Query<'w, 's, (Option<&'static Team>, Option<&'static PlayerId>)>,
    players: Query<'w, 's, (&'static PlayerId, Option<&'static Team>)>,
}

impl Allegiances<'_, '_> {
    pub fn team_of(&self, id: PlayerId) -> Option<Team> {
        self.players
            .iter()
            .find(|(player, _)| **player == id)
            .and_then(|(_, team)| team.copied())
    }

    /// The map has no side, and neither does a shooter who has left.
    pub fn team_of_source(&self, source: DamageSource) -> Option<Team> {
        match source {
            DamageSource::Player(id) => self.team_of(id),
            DamageSource::World => None,
        }
    }

    /// The gate. True for a target not in the world: `apply_damage` drops it for
    /// having no health.
    pub fn may_hurt(&self, source: DamageSource, target: Entity) -> bool {
        let Ok((team, id)) = self.bodies.get(target) else { return true };

        // Before the teams: a player is on their own team, so the team rule
        // would make rocket jumping free.
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

    #[test]
    fn only_a_shared_team_stops_a_hit() {
        assert!(!hostile(Some(Team::Red), Some(Team::Red)));
        assert!(hostile(Some(Team::Red), Some(Team::Blue)));
        assert!(hostile(None, Some(Team::Blue)), "a crate cannot hurt a player");
        assert!(hostile(Some(Team::Blue), None), "a player cannot hurt a crate");
        assert!(hostile(None, None));
    }

    /// Colour is how a friend is told from a target; a duplicate is a team you
    /// cannot see.
    #[test]
    fn every_team_has_its_own_colour() {
        let mut seen: Vec<[u8; 4]> = Team::iter().map(|t| t.colour().to_srgba().to_u8_array()).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), Team::ALL.len());
    }

    /// `ALL` is hand-written; checked against the derive.
    #[test]
    fn all_is_every_team() {
        assert_eq!(Team::iter().collect::<Vec<_>>(), Team::ALL.to_vec());
    }

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
