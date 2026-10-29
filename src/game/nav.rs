//! The nav graph: points on the map's floors where a body fits, and the moves between them. See
//! documentation/bots.md.
//!
//! Every upward face of the map's surface is a floor, since every face bounds air. Cells are
//! sampled on a lattice in `x` and `z` and kept where [`CollisionWorld::fits`] holds, so a cell and
//! a spawn point agree about where a body fits. Links are found by moving the body box with the
//! same collision calls the step uses.

use std::collections::{HashMap, VecDeque};

use bevy::prelude::*;

use crate::common::class::body_center_from_feet;
use crate::common::trigger::{TriggerEffect, TriggerVolume};
use crate::common::world_surface::Face;
use crate::game::collision::CollisionWorld;
use crate::game::player::{PLAYER_HALF, STEP_HEIGHT, WALKABLE};

/// Meters between cells along `x` and `z`.
pub const SPACING: f32 = 0.5;

/// Feet are tested this far above the floor, so the body box is clear of the floor's own slab.
const LIFT: f32 = 0.02;

/// Two samples in one column nearer than this, in meters, are one floor split by the kernel.
const SAME_FLOOR: f32 = 0.05;

/// How far a move may end from the cell it was aimed at, in meters, and still reach it.
const ARRIVED: f32 = 0.05;

pub type CellId = u32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub feet: Vec3,
    /// Which lattice square, in units of [`SPACING`].
    pub column: IVec2,
    /// Damage per second from Hurt triggers the body would stand in.
    pub hurt: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Walk,
    Step,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub to: CellId,
    pub kind: LinkKind,
    /// At running speed.
    pub seconds: f32,
}

#[derive(Clone, Debug, Default)]
pub struct NavGraph {
    cells: Vec<Cell>,
    links: Vec<Vec<Link>>,
    columns: HashMap<IVec2, Vec<CellId>>,
}

fn column_of(x: f32, z: f32) -> IVec2 {
    IVec2::new((x / SPACING).floor() as i32, (z / SPACING).floor() as i32)
}

fn column_center(column: IVec2) -> Vec2 {
    (column.as_vec2() + 0.5) * SPACING
}

/// Inclusive of the edges, either winding: a sample on a seam between two faces is kept by both
/// and merged afterwards.
fn inside_convex(outline: &[Vec2], point: Vec2) -> bool {
    const ON_THE_EDGE: f32 = 1.0e-5;
    let (mut left, mut right) = (false, false);
    for (i, &a) in outline.iter().enumerate() {
        let b = outline[(i + 1) % outline.len()];
        let side = (b - a).perp_dot(point - a);
        left |= side > ON_THE_EDGE;
        right |= side < -ON_THE_EDGE;
    }
    !(left && right)
}

/// Every lattice point on a floor where a standing body fits, outside every Kill trigger.
pub fn floor_cells(world: &CollisionWorld, faces: &[Face], triggers: &[TriggerVolume]) -> Vec<Cell> {
    let mut samples: HashMap<IVec2, Vec<f32>> = HashMap::new();
    for face in faces.iter().filter(|face| face.plane.normal.y >= WALKABLE) {
        let normal = face.plane.normal;
        let outline: Vec<Vec2> = face.vertices.iter().map(|v| Vec2::new(v.x, v.z)).collect();
        let (min, max) = outline.iter().fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let (low, high) = (column_of(min.x, min.y), column_of(max.x, max.y));
        for x in low.x..=high.x {
            for z in low.y..=high.y {
                let column = IVec2::new(x, z);
                let at = column_center(column);
                if !inside_convex(&outline, at) {
                    continue;
                }
                let y = (face.plane.w - normal.x * at.x - normal.z * at.y) / normal.y;
                let heights = samples.entry(column).or_default();
                if heights.iter().all(|h| (h - y).abs() > SAME_FLOOR) {
                    heights.push(y);
                }
            }
        }
    }

    let mut cells: Vec<Cell> = samples
        .into_iter()
        .flat_map(|(column, heights)| heights.into_iter().map(move |y| (column, y)))
        .filter_map(|(column, y)| {
            let at = column_center(column);
            let feet = Vec3::new(at.x, y, at.y);
            let center = body_center_from_feet(feet + Vec3::Y * LIFT);
            if !world.fits(center, PLAYER_HALF) {
                return None;
            }
            let standing_in = triggers.iter().filter(|trigger| trigger.overlaps(center, PLAYER_HALF));
            let mut hurt = 0;
            for trigger in standing_in {
                match trigger.effect {
                    TriggerEffect::Kill => return None,
                    TriggerEffect::Hurt { per_second } => hurt += per_second,
                }
            }
            Some(Cell { feet, column, hurt })
        })
        .collect();
    // By place, so the same map numbers its cells the same way every time.
    cells.sort_by(|a, b| (a.column.x, a.column.y).cmp(&(b.column.x, b.column.y)).then(a.feet.y.total_cmp(&b.feet.y)));
    cells
}

/// The cells' columns, for finding neighbors.
pub fn columns(cells: &[Cell]) -> HashMap<IVec2, Vec<CellId>> {
    let mut columns: HashMap<IVec2, Vec<CellId>> = HashMap::new();
    for (id, cell) in cells.iter().enumerate() {
        columns.entry(cell.column).or_default().push(id as CellId);
    }
    columns
}

/// Whether a body standing on `from` gets to `to` by walking straight at it, stepping up or down
/// at most [`STEP_HEIGHT`].
fn walks(world: &CollisionWorld, from: &Cell, to: &Cell) -> Option<LinkKind> {
    let rise = to.feet.y - from.feet.y;
    if rise.abs() > STEP_HEIGHT + SAME_FLOOR {
        return None;
    }
    let start = body_center_from_feet(from.feet + Vec3::Y * LIFT);
    let across = Vec3::new(to.feet.x - from.feet.x, 0.0, to.feet.z - from.feet.z);
    let level = |p: Vec3| Vec2::new(p.x - to.feet.x, p.z - to.feet.z).length();
    let (mut end, _) = world.slide(start, PLAYER_HALF, across);
    if level(end) > ARRIVED {
        end = world.step_up(start, PLAYER_HALF, across, STEP_HEIGHT, WALKABLE)?.0;
        if level(end) > ARRIVED {
            return None;
        }
    }
    let (resting, _) = world.ground(end, PLAYER_HALF, STEP_HEIGHT + LIFT + SAME_FLOOR, WALKABLE)?;
    let landed = resting.y - PLAYER_HALF.y;
    if (landed - to.feet.y).abs() > SAME_FLOOR + LIFT {
        return None;
    }
    Some(if rise.abs() > SAME_FLOOR { LinkKind::Step } else { LinkKind::Walk })
}

/// Columns either way a walk link may reach. Two, not one: a body is wider than [`SPACING`], so a
/// lip or a doorway's frame leaves an empty column between the last cells either side of it.
const REACH: i32 = 2;

/// The walking moves out of `id` to cells within [`REACH`] columns.
pub fn walk_links(world: &CollisionWorld, cells: &[Cell], columns: &HashMap<IVec2, Vec<CellId>>, id: CellId, speed: f32) -> Vec<Link> {
    let from = &cells[id as usize];
    let mut links = Vec::new();
    for dx in -REACH..=REACH {
        for dz in -REACH..=REACH {
            if dx == 0 && dz == 0 {
                continue;
            }
            let Some(candidates) = columns.get(&(from.column + IVec2::new(dx, dz))) else { continue };
            for &to in candidates {
                let target = &cells[to as usize];
                if let Some(kind) = walks(world, from, target) {
                    let meters = Vec2::new(target.feet.x - from.feet.x, target.feet.z - from.feet.z).length();
                    links.push(Link { to, kind, seconds: meters / speed });
                }
            }
        }
    }
    links
}

impl NavGraph {
    /// Keeps only what can be reached from a cell under one of `starts`; with no starts, keeps
    /// everything. Cell ids are renumbered.
    pub fn reachable(cells: Vec<Cell>, links: Vec<Vec<Link>>, starts: &[Vec3]) -> NavGraph {
        let everything = NavGraph { columns: columns(&cells), cells, links };
        if starts.is_empty() {
            return everything;
        }
        let mut seen = vec![false; everything.cells.len()];
        let mut queue: VecDeque<CellId> = starts.iter().filter_map(|feet| everything.cell_at(*feet)).collect();
        for &start in &queue {
            seen[start as usize] = true;
        }
        while let Some(at) = queue.pop_front() {
            for link in &everything.links[at as usize] {
                if !seen[link.to as usize] {
                    seen[link.to as usize] = true;
                    queue.push_back(link.to);
                }
            }
        }

        let mut renumbered = vec![None; seen.len()];
        let mut next = 0;
        for (old, kept) in seen.iter().enumerate() {
            if *kept {
                renumbered[old] = Some(next);
                next += 1;
            }
        }
        let cells: Vec<Cell> = everything.cells.iter().zip(&seen).filter(|(_, kept)| **kept).map(|(cell, _)| *cell).collect();
        let links = everything
            .links
            .into_iter()
            .zip(&seen)
            .filter(|(_, kept)| **kept)
            .map(|(links, _)| links.into_iter().filter_map(|link| Some(Link { to: renumbered[link.to as usize]?, ..link })).collect())
            .collect();
        NavGraph { columns: columns(&cells), cells, links }
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn links(&self, id: CellId) -> &[Link] {
        &self.links[id as usize]
    }

    /// The cell whose floor is nearest under `feet`, in its own column or a neighbor's, within a
    /// step's height.
    pub fn cell_at(&self, feet: Vec3) -> Option<CellId> {
        let column = column_of(feet.x, feet.z);
        let nearby = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| column + IVec2::new(dx, dz)));
        nearby
            .filter_map(|column| self.columns.get(&column))
            .flatten()
            .copied()
            .filter(|id| (self.cells[*id as usize].feet.y - feet.y).abs() <= STEP_HEIGHT)
            .min_by(|a, b| {
                let distance = |id: &CellId| self.cells[*id as usize].feet.distance_squared(feet);
                distance(a).total_cmp(&distance(b))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::world_surface::Volume;
    use crate::tool::room::Room;

    const SPEED: f32 = 7.0;

    fn bake(volumes: &[Volume], triggers: &[TriggerVolume], starts: &[Vec3]) -> NavGraph {
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(volumes);
        let cells = floor_cells(&world, &faces, triggers);
        let columns = columns(&cells);
        let links = (0..cells.len() as CellId).map(|id| walk_links(&world, &cells, &columns, id, SPEED)).collect();
        NavGraph::reachable(cells, links, starts)
    }

    fn room(min: Vec3, max: Vec3) -> Volume {
        Volume::air(Room::new(min, max))
    }

    fn reached_from(graph: &NavGraph, start: CellId) -> usize {
        let mut seen = vec![false; graph.cells().len()];
        let mut stack = vec![start];
        seen[start as usize] = true;
        while let Some(at) = stack.pop() {
            for link in graph.links(at) {
                if !std::mem::replace(&mut seen[link.to as usize], true) {
                    stack.push(link.to);
                }
            }
        }
        seen.iter().filter(|s| **s).count()
    }

    #[test]
    fn a_lone_room_is_one_connected_floor() {
        let graph = bake(&[room(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))], &[], &[]);
        // A body 0.7 m wide fits with its center 0.35 m in from each wall: 3.3 m of 4 m.
        assert!(!graph.cells().is_empty());
        assert!(graph.cells().iter().all(|cell| cell.feet.y.abs() < 1.0e-4));
        assert_eq!(reached_from(&graph, 0), graph.cells().len(), "part of one floor was cut off from the rest");
        let corner = graph.cell_at(Vec3::new(0.1, 0.0, 0.1));
        assert!(corner.is_some_and(|id| graph.cells()[id as usize].feet.x > 0.35), "a cell with its body in the wall");
    }

    fn ledge(height: f32) -> NavGraph {
        bake(&[room(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)), room(Vec3::new(4.0, height, 0.0), Vec3::new(8.0, 3.0 + height, 4.0))], &[], &[])
    }

    fn crosses(graph: &NavGraph) -> bool {
        (0..graph.cells().len() as CellId).any(|id| {
            let low = graph.cells()[id as usize].feet.y < 0.1;
            graph.links(id).iter().any(|link| low && graph.cells()[link.to as usize].feet.y > 0.1)
        })
    }

    #[test]
    fn a_lip_lower_than_a_step_is_walked_up_and_a_meter_is_not() {
        let low = ledge(0.4);
        assert!(crosses(&low), "a 0.4 m lip had no step up it");
        assert!(low.cells().iter().any(|c| c.feet.y > 0.3), "the raised floor has no cells");
        for id in 0..low.cells().len() as CellId {
            let y = low.cells()[id as usize].feet.y;
            for link in low.links(id) {
                if (low.cells()[link.to as usize].feet.y - y).abs() > 0.1 {
                    assert_eq!(link.kind, LinkKind::Step);
                }
            }
        }
        assert!(!crosses(&ledge(1.0)), "a 1 m ledge was walked up");
    }

    #[test]
    fn a_kill_trigger_has_no_cells_and_a_hurt_one_is_marked() {
        let volumes = [room(Vec3::ZERO, Vec3::new(8.0, 3.0, 4.0))];
        let kill = TriggerVolume { min: Vec3::new(0.0, 0.0, 0.0), max: Vec3::new(2.0, 0.1, 4.0), effect: TriggerEffect::Kill };
        let hurt = TriggerVolume { min: Vec3::new(6.0, 0.0, 0.0), max: Vec3::new(8.0, 0.1, 4.0), effect: TriggerEffect::Hurt { per_second: 10 } };
        let graph = bake(&volumes, &[kill, hurt], &[]);
        assert!(graph.cells().iter().all(|c| c.feet.x > 2.0 - PLAYER_HALF.x), "a cell stands in a kill trigger");
        assert!(graph.cells().iter().any(|c| c.hurt == 10));
        assert!(graph.cells().iter().any(|c| c.hurt == 0));
    }

    #[test]
    fn a_floor_under_a_low_ceiling_has_no_cells() {
        let graph = bake(&[room(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)), room(Vec3::new(4.0, 0.0, 0.0), Vec3::new(8.0, 1.2, 4.0))], &[], &[]);
        assert!(graph.cells().iter().all(|c| c.feet.x < 4.0 + PLAYER_HALF.x), "a body fits under 1.2 m");
    }

    #[test]
    fn floors_across_a_threshold_link_and_a_sealed_room_is_pruned() {
        let volumes = [
            room(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)),
            room(Vec3::new(4.0, 0.0, 0.0), Vec3::new(8.0, 3.0, 4.0)),
            room(Vec3::new(20.0, 0.0, 0.0), Vec3::new(24.0, 3.0, 4.0)),
        ];
        let everything = bake(&volumes, &[], &[]);
        let reached = bake(&volumes, &[], &[Vec3::new(1.0, 0.0, 1.0)]);
        assert!(reached.cells().iter().any(|c| c.feet.x > 4.5), "the doorway between two rooms was not walked through");
        assert!(reached.cells().iter().all(|c| c.feet.x < 20.0), "a sealed room was kept");
        assert!(everything.cells().iter().any(|c| c.feet.x > 20.0));
    }
}
