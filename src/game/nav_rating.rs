//! What kind of place a cell is, read off its baked fields, and how much of a map's floor is each
//! kind. Derived rather than baked, so a threshold can be retuned without baking again. See "Map
//! statistics" in documentation/bots.md.

use crate::game::nav::{CellId, NavGraph, SPACING};
use crate::game::nav_field::Field;
use crate::get;

/// Square meters of sight and of exposure under which a place is cramped.
pub const CRAMPED: f32 = 60.0;
/// Square meters of exposure over which a place is open.
pub const OPEN: f32 = 200.0;
/// Share of knockback headings, two of eight, at which a place is precarious.
pub const PRECARIOUS: f32 = 0.25;
/// Share of routes at which a place is busy.
pub const BUSY: f32 = 0.2;
/// Square meters more floor whose feet a place sees than see its own, at which it is a vantage.
pub const VANTAGE: f32 = 20.0;
/// Square meters of exposure under which, and of traffic-weighted floor seen over which, a place
/// off the routes is an ambush.
pub const AMBUSH_EXPOSURE: f32 = 80.0;
pub const AMBUSH_WATCH: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rating {
    /// Sees little and is seen little.
    Cramped,
    /// Seen from a great deal of floor.
    Open,
    /// A rocket can throw a body somewhere it dies, or it stands in a Hurt trigger.
    Precarious,
    /// On many routes between spawns and pickups.
    Busy,
    /// Busy and open: a crossing under fire.
    Gauntlet,
    /// Sees the floor at more feet than see its own: where splash comes from.
    Vantage,
    /// Off the routes, little seen, and watching busy floor.
    Ambush,
}

impl Rating {
    pub const ALL: [Rating; 7] = [Rating::Cramped, Rating::Open, Rating::Precarious, Rating::Busy, Rating::Gauntlet, Rating::Vantage, Rating::Ambush];

    pub fn label(self) -> String {
        match self {
            Rating::Cramped => get!("nav.rating.cramped"),
            Rating::Open => get!("nav.rating.open"),
            Rating::Precarious => get!("nav.rating.precarious"),
            Rating::Busy => get!("nav.rating.busy"),
            Rating::Gauntlet => get!("nav.rating.gauntlet"),
            Rating::Vantage => get!("nav.rating.vantage"),
            Rating::Ambush => get!("nav.rating.ambush"),
        }
    }

    /// `None` until the fields it reads are baked.
    pub fn holds(self, graph: &NavGraph, id: CellId) -> Option<bool> {
        let field = |field| graph.field(id, field);
        Some(match self {
            Rating::Cramped => field(Field::Sight)? < CRAMPED && field(Field::Exposure)? < CRAMPED,
            Rating::Open => field(Field::Exposure)? > OPEN,
            Rating::Precarious => graph.cells()[id as usize].hurt > 0 || field(Field::Knockback)? >= PRECARIOUS,
            Rating::Busy => field(Field::Traffic)? >= BUSY,
            Rating::Gauntlet => Rating::Busy.holds(graph, id)? && Rating::Open.holds(graph, id)?,
            Rating::Vantage => field(Field::FeetAdvantage)? >= VANTAGE,
            Rating::Ambush => !Rating::Busy.holds(graph, id)? && field(Field::Exposure)? < AMBUSH_EXPOSURE && field(Field::Watch)? >= AMBUSH_WATCH,
        })
    }
}

/// How much of a map's floor is each kind of place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapStatistics {
    /// Square meters.
    pub floor: f32,
    /// Meters between the lowest floor and the highest.
    pub rise: f32,
    /// Per [`Rating::ALL`], the share of the floor; `None` until baked.
    pub shares: Vec<Option<f32>>,
}

impl MapStatistics {
    pub fn of(graph: &NavGraph) -> Self {
        let cells = graph.cells();
        let (low, high) = cells.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), cell| (low.min(cell.feet.y), high.max(cell.feet.y)));
        let shares = Rating::ALL
            .iter()
            .map(|rating| {
                let holding = (0..cells.len() as CellId).map(|id| rating.holds(graph, id)).collect::<Option<Vec<bool>>>()?;
                Some(holding.iter().filter(|holds| **holds).count() as f32 / cells.len().max(1) as f32)
            })
            .collect();
        MapStatistics { floor: cells.len() as f32 * SPACING * SPACING, rise: if cells.is_empty() { 0.0 } else { high - low }, shares }
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::*;
    use crate::common::movement_rules::MovementRules;
    use crate::common::trigger::{TriggerEffect, TriggerVolume};
    use crate::common::world_surface::Volume;
    use crate::game::collision::CollisionWorld;
    use crate::game::{nav, nav_field};
    use crate::tool::room::Room;

    fn baked(volumes: &[Volume], triggers: &[TriggerVolume]) -> NavGraph {
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(volumes);
        let rules = MovementRules::default();
        let mut graph = nav::graph(&world, &faces, triggers, &rules, &[]);
        graph.set_fields(nav_field::bake(&world, &graph, triggers, &rules, Vec::new(), Vec::new()));
        graph
    }

    fn holds(graph: &NavGraph, feet: Vec3, rating: Rating) -> bool {
        rating.holds(graph, graph.cell_at(feet).expect("no cell there")).expect("not baked")
    }

    /// Breaks if a closet and a hall are not told apart: a closet is cramped and a big hall open.
    #[test]
    fn a_closet_is_cramped_and_a_hall_is_open() {
        let closet = baked(&[Volume::air(Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)))], &[]);
        let hall = baked(&[Volume::air(Room::new(Vec3::ZERO, Vec3::new(24.0, 4.0, 24.0)))], &[]);
        let middle = |size: f32| Vec3::new(size / 2.0 + 0.25, 0.0, size / 2.0 + 0.25);
        assert!(holds(&closet, middle(4.0), Rating::Cramped));
        assert!(!holds(&closet, middle(4.0), Rating::Open));
        assert!(holds(&hall, middle(24.0), Rating::Open));
        assert!(!holds(&hall, middle(24.0), Rating::Cramped));
    }

    /// Breaks if the statistics count a rating the bake has not reached, or lose the floor area.
    #[test]
    fn statistics_share_out_the_floor() {
        let volumes = [Volume::air(Room::new(Vec3::ZERO, Vec3::new(24.0, 4.0, 4.0))), Volume::air(Room::new(Vec3::new(24.0, -6.0, 0.0), Vec3::new(30.0, 4.0, 4.0)))];
        let kill = TriggerVolume { min: Vec3::new(24.0, -6.0, 0.0), max: Vec3::new(30.0, -5.0, 4.0), effect: TriggerEffect::Kill };
        let graph = baked(&volumes, &[kill]);
        let statistics = MapStatistics::of(&graph);
        assert_eq!(statistics.floor, graph.cells().len() as f32 * SPACING * SPACING);
        let precarious = statistics.shares[Rating::ALL.iter().position(|rating| *rating == Rating::Precarious).unwrap()].unwrap();
        assert!(precarious > 0.0 && precarious < 0.5, "the edge of a pit is {precarious} of the floor");
        assert!(MapStatistics::of(&NavGraph::default()).shares.iter().all(|share| share.is_none() || *share == Some(0.0)));
    }
}
