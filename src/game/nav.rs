//! The nav graph: points on the map's floors where a body fits, and the moves between them. See
//! documentation/bots.md.
//!
//! Every upward face of the map's surface is a floor, since every face bounds air. Cells are
//! sampled on a lattice in `x` and `z` and kept where [`CollisionWorld::fits`] holds, so a cell and
//! a spawn point agree about where a body fits. Walks are found by moving the body box with the
//! same collision calls the step uses; drops and jumps by running the step itself.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::class::{body_center_from_feet, Stance};
use crate::common::trigger::{TriggerEffect, TriggerVolume};
use crate::common::movement_rules::MovementRules;
use crate::common::world_surface::Face;
use crate::constants::TICK_HZ;
use crate::game::collision::CollisionWorld;
use crate::game::condition::Condition;
use crate::game::nav_field::{Field, Fields};
use crate::game::player::{
    running_speed, step, Mover, PhysicsBody, Player, PlayerInput, PLAYER_HALF, STEP_HEIGHT, WALKABLE,
};

/// Meters between cells along `x` and `z`.
pub const SPACING: f32 = 0.5;

/// Feet are tested this far above the floor, so the body box is clear of the floor's own slab.
pub(crate) const LIFT: f32 = 0.02;

/// Two samples in one column nearer than this, in meters, are one floor split by the kernel.
const SAME_FLOOR: f32 = 0.05;

/// How far a move may end from the cell it was aimed at, in meters, and still reach it.
const ARRIVED: f32 = 0.05;

pub type CellId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub feet: Vec3,
    /// Which lattice square, in units of [`SPACING`].
    pub column: IVec2,
    /// Damage per second from Hurt triggers the body would stand in.
    pub hurt: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkKind {
    Walk,
    Step,
    /// Running off an edge. One way.
    Drop,
    Jump,
}

/// The input that makes a drop or a jump, for a bot to replay rather than re-derive: run along
/// `yaw` with sprint held for `ticks`, jumping on the first if `jump`, having arrived at
/// `run_up`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Leap {
    pub yaw: f32,
    /// Meters per second along `yaw` at the start.
    pub run_up: f32,
    pub jump: bool,
    pub ticks: u32,
}

impl Leap {
    pub fn input(&self, tick: u32) -> PlayerInput {
        PlayerInput { movement: Vec2::Y, sprint: true, jump: self.jump && tick == 0, yaw: self.yaw, ..default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub to: CellId,
    pub kind: LinkKind,
    /// At running speed; simulated for a leap.
    pub seconds: f32,
    /// For a drop or a jump.
    pub leap: Option<Leap>,
}

/// Its serde leaves out the columns, which [`NavGraph::columns`] rebuilds from the cells.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(from = "Stored", into = "Stored")]
pub struct NavGraph {
    cells: Vec<Cell>,
    links: Vec<Vec<Link>>,
    columns: HashMap<IVec2, Vec<CellId>>,
    fields: Fields,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    cells: Vec<Cell>,
    links: Vec<Vec<Link>>,
    fields: Fields,
}

impl From<Stored> for NavGraph {
    fn from(Stored { cells, links, fields }: Stored) -> Self {
        NavGraph { columns: columns(&cells), cells, links, fields }
    }
}

impl From<NavGraph> for Stored {
    fn from(NavGraph { cells, links, fields, .. }: NavGraph) -> Self {
        Stored { cells, links, fields }
    }
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
                    links.push(Link { to, kind, seconds: meters / speed, leap: None });
                }
            }
        }
    }
    links
}

pub(crate) fn cell_near(cells: &[Cell], columns: &HashMap<IVec2, Vec<CellId>>, feet: Vec3) -> Option<CellId> {
    let column = column_of(feet.x, feet.z);
    let nearby = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| column + IVec2::new(dx, dz)));
    nearby
        .filter_map(|column| columns.get(&column))
        .flatten()
        .copied()
        .filter(|id| (cells[*id as usize].feet.y - feet.y).abs() <= STEP_HEIGHT)
        .min_by(|a, b| {
            let distance = |id: &CellId| cells[*id as usize].feet.distance_squared(feet);
            distance(a).total_cmp(&distance(b))
        })
}

pub(crate) const TICK: f32 = 1.0 / TICK_HZ as f32;
/// Ticks a leap may take before it is given up as a fall out of the map.
pub(crate) const MOST_TICKS: u32 = 3 * TICK_HZ as u32;
/// Ticks a drop may run along the floor before it is given up as a run into a wall.
const RUN_TICKS: u32 = TICK_HZ as u32 / 4;
/// Seconds a leap must save over walking to the same cell to be worth a link; a running jump
/// across a flat floor is a walk.
const LEAP_SAVES: f32 = 0.5;

/// Where a body standing on `feet` lands after `leap`'s input, and how many ticks it took. `None`
/// if it never left the floor, never landed, or touched a Kill trigger.
fn fly(world: &CollisionWorld, triggers: &[TriggerVolume], rules: &MovementRules, feet: Vec3, leap: Leap) -> Option<(Vec3, u32)> {
    let heading = Quat::from_rotation_y(leap.yaw) * Vec3::NEG_Z;
    let mut mover = Mover {
        player: Player { velocity: heading * leap.run_up, yaw: leap.yaw, ground: Some(Vec3::Y), ..default() },
        body: PhysicsBody::at(body_center_from_feet(feet + Vec3::Y * LIFT)),
        stance: Stance::Standing,
    };
    let condition = Condition::default();
    let mut airborne = false;
    for tick in 0..MOST_TICKS {
        step(&mut mover, Vec3::ZERO, &leap.input(tick), &condition, None, world, rules, TICK);
        let center = mover.body.current;
        let killed = triggers.iter().any(|trigger| trigger.effect == TriggerEffect::Kill && trigger.overlaps(center, PLAYER_HALF));
        if killed {
            return None;
        }
        if !mover.player.on_ground() {
            airborne = true;
        } else if airborne {
            return Some((center - Vec3::Y * PLAYER_HALF.y, tick + 1));
        } else if tick >= RUN_TICKS {
            return None;
        }
    }
    None
}

/// Whether walking links get from `from` to `to` within `budget` seconds.
fn walks_within(links: &[Vec<Link>], from: CellId, to: CellId, budget: f32) -> bool {
    let millis = |seconds: f32| (seconds * 1000.0) as u32;
    let budget = millis(budget);
    let mut best: HashMap<CellId, u32> = HashMap::from([(from, 0)]);
    let mut queue = BinaryHeap::from([Reverse((0, from))]);
    while let Some(Reverse((cost, at))) = queue.pop() {
        if at == to {
            return true;
        }
        if best.get(&at).is_some_and(|known| *known < cost) {
            continue;
        }
        for link in links[at as usize].iter().filter(|link| link.leap.is_none()) {
            let next = cost + millis(link.seconds);
            if next <= budget && best.get(&link.to).is_none_or(|known| next < *known) {
                best.insert(link.to, next);
                queue.push(Reverse((next, link.to)));
            }
        }
    }
    false
}

/// The drops and jumps out of `id`, tried along each of the eight headings in which it has no
/// walking neighbor. `walking` is every cell's walk links, which decide both the headings and
/// whether a leap is quicker than walking.
pub fn leap_links(
    world: &CollisionWorld,
    cells: &[Cell],
    columns: &HashMap<IVec2, Vec<CellId>>,
    walking: &[Vec<Link>],
    triggers: &[TriggerVolume],
    rules: &MovementRules,
    id: CellId,
) -> Vec<Link> {
    let from = &cells[id as usize];
    let speed = running_speed(rules);
    let mut links: Vec<Link> = Vec::new();
    for heading in 0..8 {
        let yaw = heading as f32 * std::f32::consts::FRAC_PI_4;
        let ahead = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
        let neighbor = from.column + IVec2::new(ahead.x.round() as i32, ahead.z.round() as i32);
        let walked = walking[id as usize].iter().any(|link| cells[link.to as usize].column == neighbor);
        if walked {
            continue;
        }
        let tries = [
            (LinkKind::Drop, Leap { yaw, run_up: speed, jump: false, ticks: 0 }),
            (LinkKind::Jump, Leap { yaw, run_up: 0.0, jump: true, ticks: 0 }),
            (LinkKind::Jump, Leap { yaw, run_up: speed, jump: true, ticks: 0 }),
        ];
        for (kind, leap) in tries {
            let Some((landed, ticks)) = fly(world, triggers, rules, from.feet, leap) else { continue };
            let Some(to) = cell_near(cells, columns, landed) else { continue };
            let seconds = ticks as f32 * TICK;
            let known = walking[id as usize].iter().chain(&links).any(|link| link.to == to);
            if to == id || known || walks_within(walking, id, to, seconds + LEAP_SAVES) {
                continue;
            }
            links.push(Link { to, kind, seconds, leap: Some(Leap { ticks, ..leap }) });
        }
    }
    links
}

/// The whole graph at once, for tests and tools; the bake runs the same pieces in steps.
pub fn graph(world: &CollisionWorld, faces: &[Face], triggers: &[TriggerVolume], rules: &MovementRules, starts: &[Vec3]) -> NavGraph {
    let cells = floor_cells(world, faces, triggers);
    let columns = columns(&cells);
    let speed = running_speed(rules);
    let walking: Vec<Vec<Link>> = (0..cells.len() as CellId).map(|id| walk_links(world, &cells, &columns, id, speed)).collect();
    let links = (0..cells.len() as CellId)
        .map(|id| {
            let mut links = walking[id as usize].clone();
            links.extend(leap_links(world, &cells, &columns, &walking, triggers, rules, id));
            links
        })
        .collect();
    NavGraph::reachable(cells, links, starts)
}

impl NavGraph {
    /// Keeps only what can be reached from a cell under one of `starts`; with no starts, keeps
    /// everything. Cell ids are renumbered.
    pub fn reachable(cells: Vec<Cell>, links: Vec<Vec<Link>>, starts: &[Vec3]) -> NavGraph {
        let everything = NavGraph { columns: columns(&cells), cells, links, fields: Fields::default() };
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
        NavGraph { columns: columns(&cells), cells, links, fields: Fields::default() }
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn links(&self, id: CellId) -> &[Link] {
        &self.links[id as usize]
    }

    pub fn columns(&self) -> &HashMap<IVec2, Vec<CellId>> {
        &self.columns
    }

    pub fn fields(&self) -> &Fields {
        &self.fields
    }

    pub fn set_fields(&mut self, fields: Fields) {
        self.fields = fields;
    }

    /// `None` until the field is baked.
    pub fn field(&self, id: CellId, field: Field) -> Option<f32> {
        match field {
            Field::Hurt => Some(self.cells.get(id as usize)?.hurt as f32),
            field => self.fields.get(id, field),
        }
    }

    /// The cell whose floor is nearest under `feet`, in its own column or a neighbor's, within a
    /// step's height.
    pub fn cell_at(&self, feet: Vec3) -> Option<CellId> {
        cell_near(&self.cells, &self.columns, feet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::world_surface::Volume;
    use crate::tool::room::Room;

    fn bake(volumes: &[Volume], triggers: &[TriggerVolume], starts: &[Vec3]) -> NavGraph {
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(volumes);
        graph(&world, &faces, triggers, &MovementRules::default(), starts)
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

    /// Four meters of headroom, so a jump up the ledge clears the lower room's ceiling.
    fn ledge(height: f32) -> NavGraph {
        bake(&[room(Vec3::ZERO, Vec3::new(4.0, 4.0, 4.0)), room(Vec3::new(4.0, height, 0.0), Vec3::new(8.0, 4.0 + height, 4.0))], &[], &[])
    }

    /// Kinds of link from a cell matching `from` to a cell matching `to`.
    fn kinds_between(graph: &NavGraph, from: impl Fn(&Cell) -> bool, to: impl Fn(&Cell) -> bool) -> Vec<LinkKind> {
        (0..graph.cells().len() as CellId)
            .filter(|id| from(&graph.cells()[*id as usize]))
            .flat_map(|id| graph.links(id).iter().filter(|link| to(&graph.cells()[link.to as usize])).map(|link| link.kind))
            .collect()
    }

    fn crosses(graph: &NavGraph) -> bool {
        kinds_between(graph, |c| c.feet.y < 0.1, |c| c.feet.y > 0.1).iter().any(|kind| matches!(kind, LinkKind::Walk | LinkKind::Step))
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
    fn a_meter_ledge_is_jumped_up_and_dropped_down() {
        let graph = ledge(1.0);
        let up = kinds_between(&graph, |c| c.feet.y < 0.1, |c| c.feet.y > 0.9);
        let down = kinds_between(&graph, |c| c.feet.y > 0.9, |c| c.feet.y < 0.1);
        assert!(up.contains(&LinkKind::Jump), "no jump up a 1 m ledge: {up:?}");
        assert!(down.contains(&LinkKind::Drop), "no drop off a 1 m ledge: {down:?}");
        for id in 0..graph.cells().len() as CellId {
            for link in graph.links(id) {
                assert_eq!(link.leap.is_some(), matches!(link.kind, LinkKind::Drop | LinkKind::Jump));
            }
        }
    }

    /// Two floors at 0 either side of a pit `gap` wide and 5 m deep.
    fn pit(gap: f32, triggers: &[TriggerVolume]) -> NavGraph {
        let volumes = [
            room(Vec3::ZERO, Vec3::new(4.0, 4.0, 4.0)),
            room(Vec3::new(4.0, -5.0, 0.0), Vec3::new(4.0 + gap, 4.0, 4.0)),
            room(Vec3::new(4.0 + gap, 0.0, 0.0), Vec3::new(8.0 + gap, 4.0, 4.0)),
        ];
        bake(&volumes, triggers, &[])
    }

    #[test]
    fn a_narrow_gap_is_jumped_and_a_wide_one_is_not() {
        let across = |gap: f32| kinds_between(&pit(gap, &[]), |c| c.feet.y > -0.1 && c.feet.x < 4.0, move |c| c.feet.y > -0.1 && c.feet.x > 4.0 + gap);
        assert!(across(2.0).contains(&LinkKind::Jump), "no jump across a 2 m gap");
        assert!(across(8.0).is_empty(), "a link across an 8 m gap");
        let into = kinds_between(&pit(8.0, &[]), |c| c.feet.y > -0.1, |c| c.feet.y < -4.0);
        assert!(into.contains(&LinkKind::Drop), "no drop into the pit");
    }

    #[test]
    fn a_drop_through_a_kill_trigger_has_no_link() {
        // In the air above the pit's floor, clear of a body standing on it.
        let kill = TriggerVolume { min: Vec3::new(4.0, -2.5, 0.0), max: Vec3::new(12.0, -1.5, 4.0), effect: TriggerEffect::Kill };
        let graph = pit(8.0, &[kill]);
        assert!(graph.cells().iter().any(|c| c.feet.y < -4.0), "the pit's floor lost its cells");
        assert!(kinds_between(&graph, |c| c.feet.y > -0.1, |c| c.feet.y < -4.0).is_empty(), "a drop through a kill trigger");
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
