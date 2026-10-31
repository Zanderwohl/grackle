//! The nav graph: points on the map's floors where a body fits, and the moves between them. See
//! documentation/bots.md.
//!
//! Every upward face of the map's surface is a floor, since every face bounds air. Cells are
//! sampled on a lattice in `x` and `z` and kept where [`CollisionWorld::fits`] holds for a standing
//! body, or else for a crouched one, so a cell and a spawn point agree about where a body fits. Walks are found by moving the body box with the
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
use crate::common::hitbox::Box3;
use crate::common::projectile::ProjectileSpec;
use crate::common::weapon::WeaponId;
use crate::game::collision::CollisionWorld;
use crate::game::explosion::own_blast;
use crate::game::hitscan::aim;
use crate::game::projectile::{advance, fired, muzzle, Projectile};
use crate::game::condition::Condition;
use crate::game::nav_field::{Field, Fields};
use crate::game::player::{
    running_speed, step, Mover, PhysicsBody, Player, PlayerInput, CROUCH_SPEED, STEP_HEIGHT, WALKABLE,
};

/// Meters between cells along `x` and `z`.
pub const SPACING: f32 = 0.5;

/// Feet are tested this far above the floor, so the body box is clear of the floor's own slab.
pub(crate) const LIFT: f32 = 0.02;

/// Two samples in one column nearer than this, in meters, are one floor split by the kernel.
pub(crate) const SAME_FLOOR: f32 = 0.05;

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
    /// Only a crouched body fits here: a vent, or under a low ceiling.
    pub crouched: bool,
}

impl Cell {
    pub fn stance(&self) -> Stance {
        if self.crouched { Stance::Crouched } else { Stance::Standing }
    }
}

/// Where a body of `stance` standing on `feet` has its center, lifted clear of the floor.
pub(crate) fn center_on(feet: Vec3, stance: Stance) -> Vec3 {
    feet + Vec3::Y * (LIFT + stance.half_extents().y)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkKind {
    Walk,
    Step,
    /// Walked crouched: into or out of a crouch-only cell, or under a lintel too low to walk under.
    Crouch,
    /// Running off an edge. One way.
    Drop,
    Jump,
    /// A jump on one's own rocket.
    RocketJump,
}

/// The input that makes a drop or a jump, for a bot to replay rather than re-derive: run along
/// `yaw` with sprint held for `ticks`, having arrived at `run_up`, and jump on tick `run_ticks` if
/// `jump`. A rocket jump starts still, so what it needs of the bot is exactly reproducible, runs
/// `run_ticks` to pick how far it goes, then jumps, crouches and fires `rocket` straight down.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Leap {
    pub yaw: f32,
    /// Meters per second along `yaw` at the start.
    pub run_up: f32,
    pub jump: bool,
    pub ticks: u32,
    /// Ticks run before jumping.
    pub run_ticks: u32,
    /// The weapon fired at the feet, which must be held, deployed and loaded.
    pub rocket: Option<WeaponId>,
    /// What it costs in health, before a mode scales self-damage.
    pub damage: u32,
}

/// Pitch a rocket jump fires at: straight down, short of the pole so yaw still means something.
const STRAIGHT_DOWN: f32 = -std::f32::consts::FRAC_PI_2 + 0.01;

impl Leap {
    pub fn input(&self, tick: u32) -> PlayerInput {
        let rocket = self.rocket.is_some();
        let fire = rocket && tick == self.run_ticks;
        PlayerInput {
            movement: Vec2::Y,
            sprint: true,
            jump: self.jump && tick == self.run_ticks,
            crouch: rocket && tick >= self.run_ticks,
            attack: fire,
            attack_held: fire,
            yaw: self.yaw,
            pitch: if rocket { STRAIGHT_DOWN } else { 0.0 },
            ..default()
        }
    }
}

/// A sphere a pickup is taken through, for a leap to pass through in flight.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reach {
    pub center: Vec3,
    pub radius: f32,
}

impl Reach {
    /// Whether a body's box, `center` and `half`, reaches into the sphere.
    pub fn touches(&self, center: Vec3, half: Vec3) -> bool {
        self.center.clamp(center - half, center + half).distance(self.center) <= self.radius
    }
}

/// A weapon whose rocket a body can jump on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Launcher {
    pub weapon: WeaponId,
    pub rocket: ProjectileSpec,
}

/// What finding leaps reads beyond the walls.
#[derive(Clone, Copy, Debug, Default)]
pub struct Leaping<'a> {
    pub triggers: &'a [TriggerVolume],
    /// Per pickup, in the order [`crate::game::nav_field::Fields::pickups`] lists them.
    pub reaches: &'a [Reach],
    pub launcher: Option<Launcher>,
}

impl Leaping<'_> {
    /// Per pickup, whether a body standing on some cell takes it, so a leap that grabs it in
    /// flight is the only way to it rather than a detour.
    pub fn on_foot(&self, cells: &[Cell]) -> Vec<bool> {
        self.reaches.iter().map(|reach| cells.iter().any(|cell| reach.touches(center_on(cell.feet, cell.stance()), cell.stance().half_extents()))).collect()
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
    /// The pickup, by its place in [`crate::game::nav_field::Fields::pickups`], that the leap
    /// takes in flight.
    pub grabs: Option<u16>,
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
            let stance = [Stance::Standing, Stance::Crouched].into_iter().find(|stance| world.fits(center_on(feet, *stance), stance.half_extents()))?;
            let (center, half) = (center_on(feet, stance), stance.half_extents());
            let standing_in = triggers.iter().filter(|trigger| trigger.overlaps(center, half));
            let mut hurt = 0;
            for trigger in standing_in {
                match trigger.effect {
                    TriggerEffect::Kill => return None,
                    TriggerEffect::Hurt { per_second } => hurt += per_second,
                }
            }
            Some(Cell { feet, column, hurt, crouched: stance == Stance::Crouched })
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
    // Standing where both ends allow it, and crouched otherwise or where standing is stopped, as
    // under a lintel between two rooms a body stands in.
    let standing = !from.crouched && !to.crouched && walks_as(world, from, to, Stance::Standing);
    if standing {
        return Some(if rise.abs() > SAME_FLOOR { LinkKind::Step } else { LinkKind::Walk });
    }
    walks_as(world, from, to, Stance::Crouched).then_some(LinkKind::Crouch)
}

/// Whether a body of `stance` moved from `from` toward `to` ends on it.
fn walks_as(world: &CollisionWorld, from: &Cell, to: &Cell, stance: Stance) -> bool {
    let half = stance.half_extents();
    let start = center_on(from.feet, stance);
    let across = Vec3::new(to.feet.x - from.feet.x, 0.0, to.feet.z - from.feet.z);
    let level = |p: Vec3| Vec2::new(p.x - to.feet.x, p.z - to.feet.z).length();
    let (mut end, _) = world.slide(start, half, across);
    if level(end) > ARRIVED {
        let Some((stepped, _)) = world.step_up(start, half, across, STEP_HEIGHT, WALKABLE) else { return false };
        end = stepped;
        if level(end) > ARRIVED {
            return false;
        }
    }
    let Some((resting, _)) = world.ground(end, half, STEP_HEIGHT + LIFT + SAME_FLOOR, WALKABLE) else { return false };
    let landed = resting.y - half.y;
    (landed - to.feet.y).abs() <= SAME_FLOOR + LIFT
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
                    let speed = if kind == LinkKind::Crouch { CROUCH_SPEED } else { speed };
                    links.push(Link { to, kind, seconds: meters / speed, leap: None, grabs: None });
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
/// How long a rocket jump runs before it jumps, in ticks, each throwing it a different distance:
/// a running start of a fraction of a second covers less than the run itself, so a gap is not
/// overshot for want of a shorter one.
const ROCKET_RUN_TICKS: [u32; 3] = [0, 3, 6];
/// Meters ahead the floor must reach and must have given out by for a cell to rocket jump from:
/// a run-up from nearer the edge runs off it before firing.
const ROCKET_LOOKAHEAD: std::ops::Range<f32> = 1.0..2.0;

/// Whether `cell` is as far along `ahead` as any cell on its floor in its square of two columns
/// a side. Rocket jumps start only from these: a jump is long, its starts a band of cells deep,
/// and one start in each meter of edge finds what the rest would. Ties are all kept, since any
/// way of breaking one would not turn or mirror with the map.
fn foremost_in_its_square(cells: &[Cell], columns: &HashMap<IVec2, Vec<CellId>>, cell: &Cell, ahead: Vec3) -> bool {
    let square = cell.column.div_euclid(IVec2::splat(2)) * 2;
    let along = |feet: Vec3| feet.xz().dot(ahead.xz());
    [IVec2::ZERO, IVec2::X, IVec2::Y, IVec2::ONE]
        .into_iter()
        .filter_map(|offset| columns.get(&(square + offset)))
        .flatten()
        .map(|id| &cells[*id as usize])
        .filter(|other| (other.feet.y - cell.feet.y).abs() <= STEP_HEIGHT)
        .all(|other| along(other.feet) <= along(cell.feet) + SAME_FLOOR)
}

/// Seconds a leap must save over walking to the same cell to be worth a link; a running jump
/// across a flat floor is a walk.
const LEAP_SAVES: f32 = 0.5;

/// How a leap went: where it landed, after how many ticks, the first pickup it passed through,
/// and what its rocket cost.
struct Flight {
    landed: Vec3,
    ticks: u32,
    grabbed: Option<u16>,
    damage: u32,
}

/// Where a body standing on `feet` lands after `leap`'s input. `None` if it never left the floor,
/// never landed, or touched a Kill trigger. A rocket is fired and flown as a round does it, in the
/// same tick and order: the move, then the shot from [`muzzle`], its flight by [`advance`], and its
/// blast by [`own_blast`], which pushes the body on its next move.
fn fly(world: &CollisionWorld, leaping: &Leaping, rules: &MovementRules, feet: Vec3, leap: Leap) -> Option<Flight> {
    let heading = Quat::from_rotation_y(leap.yaw) * Vec3::NEG_Z;
    let mut mover = Mover {
        player: Player { velocity: heading * leap.run_up, yaw: leap.yaw, ground: Some(Vec3::Y), ..default() },
        body: PhysicsBody::at(body_center_from_feet(feet + Vec3::Y * LIFT)),
        stance: Stance::Standing,
    };
    let condition = Condition::default();
    let mut airborne = false;
    let mut grabbed = None;
    let mut rocket: Option<Shot> = None;
    let mut damage = 0;
    let mut kick = Vec3::ZERO;
    for tick in 0..MOST_TICKS {
        step(&mut mover, std::mem::take(&mut kick), &leap.input(tick), &condition, None, world, rules, TICK);
        let (center, half) = (mover.body.current, mover.stance.half_extents());
        let launcher = leap.launcher_of(leaping);
        if tick == leap.run_ticks
            && let Some(launcher) = launcher
        {
            rocket = Some(Shot::fire(launcher, &mover));
        }
        if let Some(shot) = rocket.as_mut()
            && let Some(blast) = shot.tick(world, rules.fall(), &mover)
        {
            (damage, kick) = blast.unwrap_or_default();
            // Thrown nothing, it is not a rocket jump; `leap_links` would drop it on landing.
            if damage == 0 {
                return None;
            }
            rocket = None;
        }
        let killed = leaping.triggers.iter().any(|trigger| trigger.effect == TriggerEffect::Kill && trigger.overlaps(center, half));
        if killed {
            return None;
        }
        if grabbed.is_none() {
            grabbed = leaping.reaches.iter().position(|reach| reach.touches(center, half)).map(|index| index as u16);
        }
        // Running up, or the rocket still in the air, the floor it stands on is not a landing.
        let pending = tick < leap.run_ticks || rocket.is_some() || kick != Vec3::ZERO;
        if !mover.player.on_ground() {
            airborne = true;
        } else if airborne && !pending {
            return Some(Flight { landed: center - Vec3::Y * half.y, ticks: tick + 1, grabbed, damage });
        } else if tick >= leap.run_ticks + RUN_TICKS && !pending {
            return None;
        }
    }
    None
}

/// A rocket fired at the feet, flown and set off as a round does it.
pub(crate) struct Shot {
    projectile: Projectile,
    at: Vec3,
}

impl Shot {
    /// From the muzzle of `mover` as this tick's move left it.
    pub(crate) fn fire(launcher: Launcher, mover: &Mover) -> Self {
        let at = muzzle(mover.body.current, mover.stance, &mover.player);
        Shot { projectile: fired(launcher.rocket, at, aim(&mover.player), None, None), at }
    }

    /// One tick of flight, after the move. `None` while it flies; once its flight ends, the damage
    /// and push its blast gives `mover`, if it went off.
    pub(crate) fn tick(&mut self, world: &CollisionWorld, gravity: f32, mover: &Mover) -> Option<Option<(u32, Vec3)>> {
        let (position, ending) = advance(&mut self.projectile, self.at, world, gravity, TICK, |_, _| None);
        self.at = position;
        let (ending, _) = ending?;
        let spec = self.projectile.spec;
        let body = Box3 { center: mover.body.current, half_extents: mover.stance.half_extents() };
        Some(spec.detonates_on(ending).then(|| {
            own_blast(world, position, spec.explosion_radius, spec.damage, &body, mover.stance == Stance::Crouched, mover.player.on_ground())
        }))
    }
}

impl Leap {
    fn launcher_of(&self, leaping: &Leaping) -> Option<Launcher> {
        leaping.launcher.filter(|launcher| self.rocket == Some(launcher.weapon))
    }
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
#[allow(clippy::too_many_arguments)]
pub fn leap_links(
    world: &CollisionWorld,
    cells: &[Cell],
    columns: &HashMap<IVec2, Vec<CellId>>,
    walking: &[Vec<Link>],
    leaping: &Leaping,
    on_foot: &[bool],
    rules: &MovementRules,
    id: CellId,
) -> Vec<Link> {
    let from = &cells[id as usize];
    // A leap is simulated standing; out of a vent there is nothing to run up or jump with.
    if from.crouched {
        return Vec::new();
    }
    let speed = running_speed(rules);
    let mut links: Vec<Link> = Vec::new();
    for heading in 0..8 {
        let yaw = heading as f32 * std::f32::consts::FRAC_PI_4;
        let ahead = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
        let neighbor = from.column + IVec2::new(ahead.x.round() as i32, ahead.z.round() as i32);
        let walked = walking[id as usize].iter().any(|link| cells[link.to as usize].column == neighbor);
        // A rocket jump needs floor under it while it runs up and fires, so it starts back from
        // the edge: floor a meter on, and none past it, a gap or a wall to go over.
        let floor_at = |meters: f32| cell_near(cells, columns, from.feet + ahead * meters).is_some();
        let open_ahead = leaping.launcher.is_some()
            && floor_at(ROCKET_LOOKAHEAD.start)
            && !floor_at(ROCKET_LOOKAHEAD.end)
            && foremost_in_its_square(cells, columns, from, ahead);
        if walked && !open_ahead {
            continue;
        }
        let plain = Leap { yaw, run_up: speed, jump: false, ticks: 0, run_ticks: 0, rocket: None, damage: 0 };
        let mut tries = Vec::new();
        if !walked {
            tries.extend([
                (LinkKind::Drop, plain),
                (LinkKind::Jump, Leap { run_up: 0.0, jump: true, ..plain }),
                (LinkKind::Jump, Leap { jump: true, ..plain }),
            ]);
        }
        if let Some(launcher) = leaping.launcher.filter(|_| open_ahead) {
            let rocket = Leap { run_up: 0.0, jump: true, rocket: Some(launcher.weapon), ..plain };
            tries.extend(ROCKET_RUN_TICKS.map(|run_ticks| (LinkKind::RocketJump, Leap { run_ticks, ..rocket })));
        }
        for (kind, leap) in tries {
            let Some(flight) = fly(world, leaping, rules, from.feet, leap) else { continue };
            let Some(to) = cell_near(cells, columns, flight.landed).filter(|to| !cells[*to as usize].crouched) else { continue };
            let seconds = flight.ticks as f32 * TICK;
            let leap = Leap { ticks: flight.ticks, damage: flight.damage, ..leap };
            // Taking a pickup no cell reaches is worth any leap, however it lands.
            let grabs = flight.grabbed.filter(|pickup| !on_foot.get(*pickup as usize).copied().unwrap_or(true));
            if let Some(pickup) = grabs {
                if !links.iter().any(|link| link.grabs == Some(pickup) && link.to == to) {
                    links.push(Link { to, kind, seconds, leap: Some(leap), grabs: Some(pickup) });
                }
                continue;
            }
            let known = walking[id as usize].iter().chain(&links).any(|link| link.to == to);
            if to == id || known || walks_within(walking, id, to, seconds + LEAP_SAVES) {
                continue;
            }
            links.push(Link { to, kind, seconds, leap: Some(leap), grabs: None });
        }
    }
    links
}

/// The whole graph at once, for tests and tools; the bake runs the same pieces in steps.
pub fn graph(world: &CollisionWorld, faces: &[Face], triggers: &[TriggerVolume], rules: &MovementRules, starts: &[Vec3]) -> NavGraph {
    graph_leaping(world, faces, &Leaping { triggers, ..default() }, rules, starts)
}

/// [`graph`], with pickups to grab in flight and a launcher to rocket jump on.
pub fn graph_leaping(world: &CollisionWorld, faces: &[Face], leaping: &Leaping, rules: &MovementRules, starts: &[Vec3]) -> NavGraph {
    let cells = floor_cells(world, faces, leaping.triggers);
    let columns = columns(&cells);
    let speed = running_speed(rules);
    let on_foot = leaping.on_foot(&cells);
    let walking: Vec<Vec<Link>> = (0..cells.len() as CellId).map(|id| walk_links(world, &cells, &columns, id, speed)).collect();
    let links = (0..cells.len() as CellId)
        .map(|id| {
            let mut links = walking[id as usize].clone();
            links.extend(leap_links(world, &cells, &columns, &walking, leaping, &on_foot, rules, id));
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

    /// The cheapest way from `from` to `to` along the links, or `None` if there is none. Each link
    /// costs its seconds plus whatever `extra` says entering it costs on top, in seconds and never
    /// negative (an infinite one is a link the caller cannot take), which is where a caller weighs
    /// exposure, danger, or anything a round has learned.
    ///
    /// Dijkstra, stopping at the end: a graph of a few thousand cells is searched in well under a
    /// millisecond, and a heuristic would have to know the fastest a leap can cover ground.
    pub fn path(&self, from: CellId, to: CellId, extra: impl Fn(CellId, &Link) -> f32) -> Option<Vec<Link>> {
        self.search(from, |at| at == to, None, extra)
    }

    /// The cheapest way from `from` to taking the pickup at `pickup` in
    /// [`Fields::pickups`](crate::game::nav_field::Fields::pickups): to the cell it stands over, or
    /// ending with a leap that takes it in flight.
    pub fn path_to_pickup(&self, from: CellId, pickup: u16, extra: impl Fn(CellId, &Link) -> f32) -> Option<Vec<Link>> {
        let over = self.fields.pickups.get(pickup as usize).and_then(|pickup| self.cell_at(pickup.feet));
        self.search(from, |at| Some(at) == over, Some(pickup), extra)
    }

    /// Dijkstra from `from` until the cheapest of: a cell `ends` holds for, or a link out of one
    /// reached that grabs `grabs`, which ends the path.
    fn search(&self, from: CellId, ends: impl Fn(CellId) -> bool, grabs: Option<u16>, extra: impl Fn(CellId, &Link) -> f32) -> Option<Vec<Link>> {
        let count = self.cells.len();
        if from as usize >= count {
            return None;
        }
        let mut seconds = vec![f32::INFINITY; count];
        let mut came_by: Vec<Option<(CellId, usize)>> = vec![None; count];
        // The cheapest end found, with the grabbing link it ends with, if one.
        let mut best: Option<(f32, CellId, Option<usize>)> = None;
        seconds[from as usize] = 0.0;
        let mut queue = BinaryHeap::from([Reverse((0u32, from))]);
        while let Some(Reverse((_, at))) = queue.pop() {
            let cost = seconds[at as usize];
            if best.is_some_and(|(least, ..)| least <= cost) {
                break;
            }
            if ends(at) {
                best = Some((cost, at, None));
                continue;
            }
            for (index, link) in self.links(at).iter().enumerate() {
                let next = cost + link.seconds + extra(link.to, link).max(0.0);
                if grabs.is_some() && link.grabs == grabs {
                    if next.is_finite() && best.is_none_or(|(least, ..)| next < least) {
                        best = Some((next, at, Some(index)));
                    }
                    continue;
                }
                if next < seconds[link.to as usize] {
                    seconds[link.to as usize] = next;
                    came_by[link.to as usize] = Some((at, index));
                    queue.push(Reverse(((next * 1000.0) as u32, link.to)));
                }
            }
        }
        let (_, end, grab) = best?;
        let mut path: Vec<Link> = grab.map(|index| self.links(end)[index]).into_iter().collect();
        let mut at = end;
        while let Some((before, index)) = came_by[at as usize] {
            path.push(self.links(before)[index]);
            at = before;
        }
        path.reverse();
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::player::PLAYER_HALF;
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

    /// The rocket launcher as `weapons.toml` ships it, for the leaps that need one.
    fn launcher() -> Launcher {
        Launcher { weapon: WeaponId::of("rocket_launcher"), rocket: ProjectileSpec::ROCKET }
    }

    /// Halls' BFG in small: a 4 m platform ringed by a 5.7 m gap over a kill floor, with a weapon
    /// over the platform's middle.
    fn an_island() -> (Vec<Volume>, TriggerVolume, Vec3) {
        let volumes = vec![
            room(Vec3::new(-10.0, -10.0, -10.0), Vec3::new(10.0, 6.0, 10.0)),
            Volume::solid(Room::new(Vec3::new(-10.0, -10.0, -10.0), Vec3::new(10.0, 0.0, -7.7))),
            Volume::solid(Room::new(Vec3::new(-2.0, -10.0, -2.0), Vec3::new(2.0, 0.0, 2.0))),
        ];
        let kill = TriggerVolume { min: Vec3::new(-10.0, -10.0, -10.0), max: Vec3::new(10.0, -9.0, 10.0), effect: TriggerEffect::Kill };
        (volumes, kill, Vec3::new(0.25, 0.0, 0.25))
    }

    /// Breaks if a rocket jump is not baked, or is baked for a gap a jump already crosses: from
    /// the ledge across 5.7 m to the platform, which no plain jump reaches, with what it costs.
    #[test]
    fn a_rocket_jump_crosses_a_gap_no_jump_can() {
        let (volumes, kill, platform) = an_island();
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(&volumes);
        let rules = MovementRules::default();
        let ledge = Vec3::new(0.25, 0.0, -8.25);
        let plain = graph_leaping(&world, &faces, &Leaping { triggers: &[kill], ..default() }, &rules, &[ledge]);
        assert!(plain.cell_at(platform).is_none(), "the platform was reached without a rocket");

        let rockets = graph_leaping(&world, &faces, &Leaping { triggers: &[kill], reaches: &[], launcher: Some(launcher()) }, &rules, &[ledge]);
        let island = rockets.cell_at(platform).expect("no rocket jump reaches the platform");
        let path = rockets.path(rockets.cell_at(ledge).unwrap(), island, |_, _| 0.0).unwrap();
        let jump = path.iter().find(|link| link.kind == LinkKind::RocketJump).expect("the way to the platform has no rocket jump in it");
        let leap = jump.leap.unwrap();
        assert_eq!(leap.rocket, Some(launcher().weapon));
        assert!(leap.damage > 0, "a rocket jump that costs nothing");
    }

    /// Breaks if rocket jumps start from cells chosen by something that does not turn with the
    /// map, as even columns did: the island turned a quarter has its rocket jumps from the same
    /// cells turned.
    #[test]
    fn rocket_jumps_start_from_the_same_cells_turned() {
        let (volumes, kill, _) = an_island();
        let turn = |p: Vec3| Vec3::new(p.z, p.y, -p.x);
        let turn_box = |min: Vec3, max: Vec3| (turn(min).min(turn(max)), turn(min).max(turn(max)));
        let turned: Vec<Volume> = volumes
            .iter()
            .map(|volume| {
                let (min, max) = volume.shape.bounds();
                let (min, max) = turn_box(min, max);
                Volume { shape: Room::new(min, max), fill: volume.fill }
            })
            .collect();
        let (min, max) = turn_box(kill.min, kill.max);
        let turned_kill = TriggerVolume { min, max, ..kill };
        let starts = |volumes: &[Volume], kill: TriggerVolume, ledge: Vec3| {
            let mut world = CollisionWorld::default();
            let faces = world.rebuild_volumes(volumes);
            let graph = graph_leaping(&world, &faces, &Leaping { triggers: &[kill], reaches: &[], launcher: Some(launcher()) }, &MovementRules::default(), &[ledge]);
            let mut starts: Vec<[i32; 3]> = (0..graph.cells().len() as CellId)
                .filter(|from| graph.links(*from).iter().any(|link| link.kind == LinkKind::RocketJump))
                .map(|from| (graph.cells()[from as usize].feet * 100.0).round().as_ivec3().to_array())
                .collect();
            starts.sort();
            starts
        };
        let ledge = Vec3::new(0.25, 0.0, -8.25);
        let plain = starts(&volumes, kill, ledge);
        assert!(!plain.is_empty(), "no rocket jumps to compare");
        let mut expected: Vec<[i32; 3]> = plain.iter().map(|feet| (turn(IVec3::from_array(*feet).as_vec3() / 100.0) * 100.0).round().as_ivec3().to_array()).collect();
        expected.sort();
        assert_eq!(starts(&turned, turned_kill, turn(ledge)), expected);
    }

    /// Breaks if a pickup no cell reaches cannot be planned for: hung over a 2 m gap, it is taken
    /// in flight by a jump across, kept as a link that grabs it.
    #[test]
    fn a_pickup_over_a_gap_is_taken_in_flight() {
        let volumes = [
            room(Vec3::new(0.0, -6.0, 0.0), Vec3::new(4.0, 4.0, 12.0)),
            Volume::solid(Room::new(Vec3::new(0.0, -6.0, 0.0), Vec3::new(4.0, 0.0, 4.0))),
            Volume::solid(Room::new(Vec3::new(0.0, -6.0, 6.0), Vec3::new(4.0, 0.0, 12.0))),
        ];
        let kill = TriggerVolume { min: Vec3::new(0.0, -6.0, 0.0), max: Vec3::new(4.0, -5.0, 12.0), effect: TriggerEffect::Kill };
        let reach = Reach { center: Vec3::new(2.0, 1.0, 5.0), radius: 0.6 };
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(&volumes);
        let leaping = Leaping { triggers: &[kill], reaches: &[reach], launcher: None };
        let graph = graph_leaping(&world, &faces, &leaping, &MovementRules::default(), &[Vec3::new(2.25, 0.0, 1.25)]);
        assert_eq!(leaping.on_foot(graph.cells()), vec![false], "a cell reaches a pickup hung over the gap");
        let grabs: Vec<&Link> = (0..graph.cells().len() as CellId).flat_map(|id| graph.links(id)).filter(|link| link.grabs == Some(0)).collect();
        assert!(!grabs.is_empty(), "no leap takes the pickup over the gap");
        assert!(grabs.iter().all(|link| link.leap.is_some()), "a walk takes a pickup in flight");
    }

    /// Breaks if a space only a crouched body fits is left off the graph or walked standing: a
    /// 1.7 m vent between two rooms has crouch-only cells, is entered by crouch links, and is the
    /// way from one room to the other.
    #[test]
    fn a_vent_is_crouched_through() {
        let graph = bake(
            &[room(Vec3::ZERO, Vec3::new(6.0, 3.0, 4.0)), room(Vec3::new(6.0, 0.0, 1.0), Vec3::new(12.0, 1.7, 3.0)), room(Vec3::new(12.0, 0.0, 0.0), Vec3::new(18.0, 3.0, 4.0))],
            &[],
            &[Vec3::new(1.25, 0.0, 2.25)],
        );
        let inside = graph.cell_at(Vec3::new(9.25, 0.0, 2.25)).expect("no cell in the vent");
        assert!(graph.cells()[inside as usize].crouched, "a cell under 1.7 m reads as standing room");
        assert!(graph.cells().iter().filter(|cell| cell.feet.x < 5.0 || cell.feet.x > 13.0).all(|cell| !cell.crouched), "a cell with headroom reads as crouch-only");
        let (from, to) = (graph.cell_at(Vec3::new(1.25, 0.0, 2.25)).unwrap(), graph.cell_at(Vec3::new(16.75, 0.0, 2.25)).unwrap());
        let path = graph.path(from, to, |_, _| 0.0).expect("no way through the vent");
        assert!(path.iter().any(|link| link.kind == LinkKind::Crouch), "the way through the vent has no crouch in it");
    }

    /// Breaks if a path does not join its ends link to link, or one is found where none exists.
    #[test]
    fn a_path_runs_link_to_link_from_one_end_to_the_other() {
        let graph = bake(&[room(Vec3::ZERO, Vec3::new(12.0, 3.0, 4.0)), room(Vec3::new(20.0, 0.0, 0.0), Vec3::new(24.0, 3.0, 4.0))], &[], &[]);
        let (from, to) = (graph.cell_at(Vec3::new(0.75, 0.0, 0.75)).unwrap(), graph.cell_at(Vec3::new(11.25, 0.0, 3.25)).unwrap());
        let path = graph.path(from, to, |_, _| 0.0).expect("no path across one room");
        let mut at = from;
        for link in &path {
            assert!(graph.links(at).contains(link), "a step of the path is not a link out of where it stands");
            at = link.to;
        }
        assert_eq!(at, to);
        let sealed = graph.cell_at(Vec3::new(21.25, 0.0, 1.25)).unwrap();
        assert!(graph.path(from, sealed, |_, _| 0.0).is_none(), "a path into a sealed room");
    }

    /// Breaks if the caller's cost is ignored: a pillar splits a room, and making one side dear
    /// sends the path round the other.
    #[test]
    fn a_dear_side_of_a_pillar_sends_the_path_round_the_other() {
        let graph = bake(&[room(Vec3::ZERO, Vec3::new(12.0, 3.0, 9.0)), Volume::solid(Room::new(Vec3::new(4.0, 0.0, 3.0), Vec3::new(8.0, 3.0, 6.0)))], &[], &[]);
        let (from, to) = (graph.cell_at(Vec3::new(0.75, 0.0, 4.25)).unwrap(), graph.cell_at(Vec3::new(11.25, 0.0, 4.25)).unwrap());
        let side = |path: &[Link]| path.iter().map(|link| graph.cells()[link.to as usize].feet.z).fold(4.5, |far: f32, z| if (z - 4.5).abs() > (far - 4.5).abs() { z } else { far });
        let north_dear = graph.path(from, to, |cell, _| if graph.cells()[cell as usize].feet.z < 3.0 { 10.0 } else { 0.0 }).unwrap();
        let south_dear = graph.path(from, to, |cell, _| if graph.cells()[cell as usize].feet.z > 6.0 { 10.0 } else { 0.0 }).unwrap();
        assert!(side(&north_dear) > 6.0, "with the low side dear the path still went low");
        assert!(side(&south_dear) < 3.0, "with the high side dear the path still went high");
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
