//! Fields worked out from the graph and its sightlines rather than from more rays: how far a cell's
//! sightlines run, how soon a body there is out of the sight of everything that sees it, and how
//! soon each spawn reaches a weapon it did not start with. See
//! documentation/nav-derived-fields.md.
//!
//! A sightline here is a standing eye seeing at least one body sample; one that sees only the feet
//! is splash, not a duel.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use bevy::prelude::*;

use crate::common::game_rules::ModeRules;
use crate::common::weapon::{WeaponCatalog, WeaponId};
use crate::game::nav::{CellId, NavGraph, SPACING};
use crate::game::nav_field::{Field, Sightlines};

/// Seconds of walking past which an escape is not looked for, and counts as none.
pub const ESCAPE_MOST: f32 = 4.0;
/// Meters at the top of each engagement range band but the last, which is open above.
pub const RANGE_BANDS: [f32; 3] = [8.0, 15.0, 25.0];

/// Per viewpoint, the viewpoints it sees.
fn sees(lines: &Sightlines) -> Vec<Vec<u32>> {
    lines.standing.iter().map(|row| row.iter().filter(|seen| seen.samples > 0).map(|seen| seen.viewpoint).collect()).collect()
}

/// Per viewpoint, the viewpoints that see it.
fn watchers(sees: &[Vec<u32>]) -> Vec<Vec<u32>> {
    let mut watchers = vec![Vec::new(); sees.len()];
    for (eye, row) in sees.iter().enumerate() {
        for &body in row {
            watchers[body as usize].push(eye as u32);
        }
    }
    watchers
}

/// Per cell, in meters: the 25th, 50th and 75th percentiles of the distances to the viewpoints it
/// sees or is seen from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ranges {
    pub low: Vec<f32>,
    pub median: Vec<f32>,
    pub high: Vec<f32>,
}

/// Each other viewpoint counts once, weighted by the floor it stands for. All three are 0 for a
/// cell that sees and is seen by nothing.
pub fn ranges(lines: &Sightlines) -> Ranges {
    let count = lines.viewpoints.len();
    let sees = sees(lines);
    let watchers = watchers(&sees);
    let mut area = vec![0.0f32; count];
    for &viewpoint in &lines.viewpoint_of {
        area[viewpoint as usize] += SPACING * SPACING;
    }
    let mut stamp = vec![u32::MAX; count];
    let mut found: Vec<(f32, f32)> = Vec::new();
    let per_viewpoint: Vec<[f32; 3]> = (0..count)
        .map(|here| {
            found.clear();
            for &other in sees[here].iter().chain(&watchers[here]) {
                if std::mem::replace(&mut stamp[other as usize], here as u32) != here as u32 {
                    found.push((lines.viewpoints[here].distance(lines.viewpoints[other as usize]), area[other as usize]));
                }
            }
            percentiles(&mut found)
        })
        .collect();
    let per_cell = |which: usize| lines.viewpoint_of.iter().map(|viewpoint| per_viewpoint[*viewpoint as usize][which]).collect();
    Ranges { low: per_cell(0), median: per_cell(1), high: per_cell(2) }
}

/// The 25th, 50th and 75th weighted percentiles of `(value, weight)`.
fn percentiles(found: &mut [(f32, f32)]) -> [f32; 3] {
    if found.is_empty() {
        return [0.0; 3];
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f32 = found.iter().map(|(_, weight)| weight).sum();
    let at = |share: f32| {
        let mut so_far = 0.0;
        for &(value, weight) in found.iter() {
            so_far += weight;
            if so_far >= share * total {
                return value;
            }
        }
        found[found.len() - 1].0
    };
    [at(0.25), at(0.5), at(0.75)]
}

/// Per cell: seconds of walking to the nearest cell none of its watchers sees, and that cell.
/// Infinite and `None` where there is none within [`ESCAPE_MOST`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Escapes {
    pub seconds: Vec<f32>,
    pub to: Vec<Option<CellId>>,
}

/// A cell's watchers are the viewpoints that see its viewpoint. What they see together is a union
/// of their rows, kept as a bitset over viewpoints, with each watcher's own square in it; a cell
/// whose viewpoint is outside it is out of their sight. Each search stops at the first such cell.
pub fn escapes(graph: &NavGraph, lines: &Sightlines) -> Escapes {
    let cells = graph.cells().len();
    let mut escapes = Escapes { seconds: vec![f32::INFINITY; cells], to: vec![None; cells] };
    if lines.viewpoint_of.len() != cells {
        return escapes;
    }
    let count = lines.viewpoints.len();
    let words = count.div_ceil(64);
    let sees = sees(lines);
    let watchers = watchers(&sees);
    let set = |bits: &mut [u64], viewpoint: usize| bits[viewpoint / 64] |= 1 << (viewpoint % 64);
    let mut rows = vec![0u64; count * words];
    for (eye, row) in sees.iter().enumerate() {
        for &body in row {
            set(&mut rows[eye * words..(eye + 1) * words], body as usize);
        }
    }
    let mut members: Vec<Vec<CellId>> = vec![Vec::new(); count];
    for (cell, viewpoint) in lines.viewpoint_of.iter().enumerate() {
        members[*viewpoint as usize].push(cell as CellId);
    }
    let mut covered = vec![0u64; words];
    let mut search = Bounded::new(cells);
    for (here, held) in members.iter().enumerate() {
        if watchers[here].is_empty() {
            for &cell in held {
                (escapes.seconds[cell as usize], escapes.to[cell as usize]) = (0.0, Some(cell));
            }
            continue;
        }
        covered.fill(0);
        for &watcher in &watchers[here] {
            let watcher = watcher as usize;
            for (word, row) in covered.iter_mut().zip(&rows[watcher * words..(watcher + 1) * words]) {
                *word |= row;
            }
            set(&mut covered, watcher);
        }
        let hidden = |cell: CellId| {
            let viewpoint = lines.viewpoint_of[cell as usize] as usize;
            covered[viewpoint / 64] & (1 << (viewpoint % 64)) == 0
        };
        for &cell in held {
            if let Some((seconds, to)) = search.nearest(graph, cell, ESCAPE_MOST, hidden) {
                (escapes.seconds[cell as usize], escapes.to[cell as usize]) = (seconds, Some(to));
            }
        }
    }
    escapes
}

/// Dijkstra over the links with a limit, its buffers reused from one search to the next.
struct Bounded {
    seconds: Vec<f32>,
    reached: Vec<CellId>,
    queue: BinaryHeap<Reverse<(u32, CellId)>>,
}

impl Bounded {
    fn new(cells: usize) -> Self {
        Bounded { seconds: vec![f32::INFINITY; cells], reached: Vec::new(), queue: BinaryHeap::new() }
    }

    /// The nearest cell `wanted` holds for within `most` seconds of `from`, and the seconds to it.
    fn nearest(&mut self, graph: &NavGraph, from: CellId, most: f32, wanted: impl Fn(CellId) -> bool) -> Option<(f32, CellId)> {
        for id in self.reached.drain(..) {
            self.seconds[id as usize] = f32::INFINITY;
        }
        self.queue.clear();
        self.seconds[from as usize] = 0.0;
        self.reached.push(from);
        self.queue.push(Reverse((0, from)));
        while let Some(Reverse((_, at))) = self.queue.pop() {
            let cost = self.seconds[at as usize];
            if wanted(at) {
                return Some((cost, at));
            }
            for link in graph.links(at) {
                let next = cost + link.seconds;
                let to = link.to as usize;
                if next <= most && next < self.seconds[to] {
                    if self.seconds[to].is_infinite() {
                        self.reached.push(link.to);
                    }
                    self.seconds[to] = next;
                    self.queue.push(Reverse(((next * 1000.0) as u32, link.to)));
                }
            }
        }
        None
    }
}

/// What every class a bot may roll starts with, so a pickup any of them lacks counts as a first
/// weapon: the mode's loadout, else each class's defaults.
pub fn starting_weapons(catalog: &WeaponCatalog, rules: &ModeRules) -> Vec<WeaponId> {
    let carried = |class| catalog.starting_equipment(class, rules.loadout.as_deref()).0.carried().to_vec();
    let mut classes = rules.armed_classes(catalog).into_iter();
    let Some(first) = classes.next() else { return Vec::new() };
    classes.fold(carried(first), |mut shared, class| {
        let theirs = carried(class);
        shared.retain(|weapon| theirs.contains(weapon));
        shared
    })
}

/// From one spawn to the nearest weapon pickup it does not start with.
#[derive(Clone, Debug, PartialEq)]
pub struct FirstWeapon {
    pub spawn: Vec3,
    /// By its place in [`crate::game::nav_field::Fields::pickups`]; `None` if there is none to
    /// reach, or the spawn is off the graph.
    pub pickup: Option<usize>,
    pub seconds: f32,
    /// Square meters, each cell's exposure weighted by the seconds spent entering it.
    pub mean_exposure: f32,
    pub most_exposure: f32,
}

/// One per spawn the bake was given.
pub fn first_weapons(graph: &NavGraph, starting: &[WeaponId]) -> Vec<FirstWeapon> {
    let fields = graph.fields();
    let exposure = |cell: CellId| graph.field(cell, Field::Exposure).unwrap_or(0.0);
    fields
        .spawns
        .iter()
        .map(|&spawn| {
            let none = FirstWeapon { spawn, pickup: None, seconds: f32::INFINITY, mean_exposure: 0.0, most_exposure: 0.0 };
            let Some(from) = graph.cell_at(spawn) else { return none };
            let nearest = fields
                .pickups
                .iter()
                .enumerate()
                .filter(|(_, pickup)| pickup.weapon.is_some_and(|weapon| !starting.contains(&weapon)))
                .filter_map(|(index, _)| Some((index, *fields.distances.get(index)?.get(from as usize)?)))
                .filter(|(_, seconds)| seconds.is_finite())
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let Some((pickup, seconds)) = nearest else { return none };
            let route = graph.path_to_pickup(from, pickup as u16, |_, _| 0.0).unwrap_or_default();
            let (mut weighted, mut time, mut most) = (0.0, 0.0, exposure(from));
            for link in &route {
                let here = exposure(link.to);
                (weighted, time, most) = (weighted + here * link.seconds, time + link.seconds, most.max(here));
            }
            // Never over the most, which rounding over many equal cells can otherwise put it.
            let mean_exposure = if time > 0.0 { (weighted / time).min(most) } else { exposure(from) };
            FirstWeapon { spawn, pickup: Some(pickup), seconds, mean_exposure, most_exposure: most }
        })
        .collect()
}

/// The share of the floor in each engagement range band, by median; `None` until baked.
pub fn range_bands(graph: &NavGraph) -> Option<[f32; 4]> {
    let cells = graph.cells().len();
    if cells == 0 {
        return None;
    }
    let mut counts = [0usize; 4];
    for id in 0..cells as CellId {
        let median = graph.field(id, Field::RangeMedian)?;
        counts[RANGE_BANDS.iter().take_while(|top| median >= **top).count()] += 1;
    }
    Some(counts.map(|count| count as f32 / cells as f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::movement_rules::MovementRules;
    use crate::common::world_surface::Volume;
    use crate::game::collision::CollisionWorld;
    use crate::game::nav_field::Pickup;
    use crate::game::{nav, nav_field};
    use crate::tool::room::Room;

    fn room(min: Vec3, max: Vec3) -> Volume {
        Volume::air(Room::new(min, max))
    }

    fn baked(volumes: &[Volume], pickups: Vec<Pickup>, spawns: Vec<Vec3>) -> NavGraph {
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(volumes);
        let rules = MovementRules::default();
        let mut graph = nav::graph(&world, &faces, &[], &rules, &spawns);
        let fields = nav_field::bake(&world, &graph, &[], &rules, pickups, spawns);
        graph.set_fields(fields);
        graph
    }

    fn at(graph: &NavGraph, feet: Vec3, field: Field) -> f32 {
        graph.field(graph.cell_at(feet).expect("no cell there"), field).expect("not baked")
    }

    /// Breaks if range is not the length of the sightlines: the middle of a 30 m hall ranges
    /// further than the middle of a 4 m closet, and every cell's percentiles are in order.
    #[test]
    fn a_hall_ranges_further_than_a_closet() {
        let closet = baked(&[room(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))], Vec::new(), Vec::new());
        let hall = baked(&[room(Vec3::ZERO, Vec3::new(30.0, 4.0, 6.0))], Vec::new(), Vec::new());
        let (small, large) = (at(&closet, Vec3::new(2.25, 0.0, 2.25), Field::RangeMedian), at(&hall, Vec3::new(1.25, 0.0, 3.25), Field::RangeMedian));
        assert!(small < 3.0 && large > 10.0, "the closet ranges {small} m and the end of the hall {large} m");
        for id in 0..hall.cells().len() as CellId {
            let (low, median, high) = (hall.field(id, Field::RangeLow).unwrap(), hall.field(id, Field::RangeMedian).unwrap(), hall.field(id, Field::RangeHigh).unwrap());
            assert!(low <= median && median <= high, "{low} {median} {high}");
        }
        assert_eq!(range_bands(&closet).unwrap()[0], 1.0, "a closet is not all close range");
    }

    /// Breaks if escape is not walking time to somewhere out of sight, or the cell it names is
    /// seen by a watcher. A hall, a passage up from its east end, and a room off the passage's top:
    /// the room is out of sight of everything that sees the hall, and nearer its east end.
    #[test]
    fn around_two_corners_is_out_of_sight() {
        let volumes = [
            room(Vec3::ZERO, Vec3::new(10.0, 3.0, 4.0)),
            room(Vec3::new(10.0, 0.0, 0.0), Vec3::new(14.0, 3.0, 12.0)),
            room(Vec3::new(14.0, 0.0, 8.0), Vec3::new(24.0, 3.0, 12.0)),
        ];
        let graph = baked(&volumes, Vec::new(), Vec::new());
        let lines = &graph.fields().sightlines;
        let (west, east) = (at(&graph, Vec3::new(1.25, 0.0, 2.25), Field::Escape), at(&graph, Vec3::new(8.25, 0.0, 2.25), Field::Escape));
        assert!(east.is_finite() && east < west, "the hall's east end escapes in {east} s and its west end in {west} s");
        let sees = sees(lines);
        for id in 0..graph.cells().len() as CellId {
            let (Some(to), here) = (graph.fields().escape_to(id), lines.viewpoint_of[id as usize]) else { continue };
            let there = lines.viewpoint_of[to as usize];
            for (eye, row) in sees.iter().enumerate() {
                if row.contains(&here) {
                    assert!(eye as u32 != there && !row.contains(&there), "cell {id} escapes to {to}, which a watcher sees");
                }
            }
        }
    }

    /// Breaks if an open hall reads as anything but a death trap: no cell of it is out of sight.
    #[test]
    fn an_open_hall_has_no_escape() {
        let graph = baked(&[room(Vec3::ZERO, Vec3::new(20.0, 4.0, 20.0))], Vec::new(), Vec::new());
        assert!(at(&graph, Vec3::new(10.25, 0.0, 10.25), Field::Escape).is_infinite());
        assert_eq!(graph.fields().escape_to(graph.cell_at(Vec3::new(10.25, 0.0, 10.25)).unwrap()), None);
    }

    /// Breaks if a weapon the body starts with is counted, or the nearer of two is not the one
    /// picked: the pistol beside the spawn is skipped for the shotgun further off.
    #[test]
    fn the_first_weapon_is_the_nearest_one_not_started_with() {
        let (pistol, shotgun, rifle) = (WeaponId::of("pistol"), WeaponId::of("shotgun"), WeaponId::of("rifle"));
        let pickups = vec![
            Pickup::weapon("pistol".into(), Vec3::new(2.25, 0.0, 1.25)).giving(pistol),
            Pickup::weapon("shotgun".into(), Vec3::new(10.25, 0.0, 1.25)).giving(shotgun),
            Pickup::weapon("rifle".into(), Vec3::new(18.25, 0.0, 1.25)).giving(rifle),
        ];
        let spawn = Vec3::new(1.25, 0.0, 1.25);
        let graph = baked(&[room(Vec3::ZERO, Vec3::new(20.0, 3.0, 3.0))], pickups, vec![spawn]);
        let first = first_weapons(&graph, &[pistol]);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].pickup, Some(1), "went for {:?}", first[0].pickup);
        assert!(first[0].seconds > 0.5 && first[0].seconds < 3.0, "9 m took {} s", first[0].seconds);
        assert!(first[0].most_exposure >= first[0].mean_exposure);
        assert_eq!(first_weapons(&graph, &[pistol, shotgun, rifle])[0].pickup, None);
    }

    /// Breaks if exposure along the route is not read off the route: the same walk across an open
    /// hall is more exposed than down a walled corridor.
    #[test]
    fn a_route_across_a_hall_is_more_exposed_than_down_a_corridor() {
        let weapon = WeaponId::of("shotgun");
        let pickup = || vec![Pickup::weapon("shotgun".into(), Vec3::new(14.25, 0.0, 1.25)).giving(weapon)];
        let spawn = Vec3::new(1.25, 0.0, 1.25);
        let corridor = baked(&[room(Vec3::ZERO, Vec3::new(16.0, 3.0, 2.5))], pickup(), vec![spawn]);
        let hall = baked(&[room(Vec3::ZERO, Vec3::new(16.0, 3.0, 16.0))], pickup(), vec![spawn]);
        let (narrow, open) = (first_weapons(&corridor, &[])[0].mean_exposure, first_weapons(&hall, &[])[0].mean_exposure);
        assert!(open > narrow, "the hall's route is {open} m² exposed and the corridor's {narrow} m²");
    }
}
