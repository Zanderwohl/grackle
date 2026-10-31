//! What a cell is like to stand on, baked onto the nav graph: how dangerous it is, how much it sees
//! and is seen, and how far it is from each pickup. Stored in absolute units, never normalized to
//! the map. See "Fields, baked per cell" in documentation/bots.md.
//!
//! Sightlines and knockback are worked out per **viewpoint**, one for each square of
//! [`SIGHT_STRIDE`] columns at each floor height, and every cell reads its viewpoint's.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::class::{Stance, CROUCH_EYE_HEIGHT, TALLEST_CLASS_EYE_HEIGHT};
use crate::common::movement_rules::MovementRules;
use crate::common::trigger::{TriggerEffect, TriggerVolume};
use crate::game::collision::CollisionWorld;
use crate::game::condition::Condition;
use crate::game::explosion::push_speed;
use crate::game::world_prop;
use crate::game::nav::{cell_near, center_on, Reach, CellId, NavGraph, MOST_TICKS, SAME_FLOOR, SPACING, TICK};
use crate::game::player::{step, Duck, Mover, PhysicsBody, Player, PlayerInput, STEP_HEIGHT};

/// Columns per side of the square one viewpoint stands for.
pub const SIGHT_STRIDE: i32 = 2;
/// Meters between feet; nothing further is looked at.
pub const SIGHT_RANGE: f32 = 40.0;
/// Heights above the feet sampled for how much of a body is seen: head, chest and knees.
const STANDING_BODY: [f32; 3] = [1.6, 1.1, 0.5];
const CROUCHED_BODY: [f32; 3] = [1.2, 0.85, 0.4];
/// Height above the feet, in meters, of the floor a rocket is put at for splash.
const FEET: f32 = 0.1;
/// Splash a rocket deals a body about a meter from it: the push every cell is tested against.
const KNOCKBACK_DAMAGE: u32 = 50;
/// Upward share of the push, since a blast at the feet throws up as well as out.
const KNOCKBACK_LIFT: f32 = 0.5;
const KNOCKBACK_HEADINGS: usize = 8;
/// Meters per second across the floor below which a thrown body has come to rest.
const AT_REST: f32 = 0.3;
/// Seconds of travel within which a cell's lowest exposure counts as its cover.
pub const COVER_SECONDS: f32 = 1.5;
/// Seconds over the shortest route within which a cell counts as on it.
pub const ROUTE_SLACK: f32 = 0.25;

const KNOCKBACK_BATCH: usize = 32;
const SIGHT_BATCH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    /// Square meters of floor whose standing bodies this cell's standing eye sees, weighted by how
    /// much of each.
    Sight,
    /// Square meters of floor from which a standing body here is seen, weighted the same way.
    Exposure,
    /// Sight less exposure, in square meters.
    Advantage,
    CrouchedSight,
    CrouchedExposure,
    CrouchedAdvantage,
    /// Square meters of floor where this standing eye sees the floor at a standing body's feet:
    /// where it can put a rocket.
    FeetSight,
    /// Square meters of floor from which the floor at these feet is seen.
    FeetExposure,
    FeetAdvantage,
    /// The share of routes between spawns and pickups that pass within [`ROUTE_SLACK`] of here.
    Traffic,
    /// Square meters of floor this standing eye sees, each weighted by its traffic.
    Watch,
    /// The lowest exposure within [`COVER_SECONDS`] of travel, in square meters.
    Cover,
    /// The fraction of headings a rocket's push throws a body into a Kill or Hurt trigger or off
    /// the graph.
    Knockback,
    /// Damage per second.
    Hurt,
    /// Meters above the lowest cell that sees this one.
    Height,
    /// Seconds to the pickup at this index in [`Fields::pickups`].
    Distance(usize),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pickup {
    pub label: String,
    pub feet: Vec3,
    /// What it is taken through.
    pub reach: Reach,
}

impl Pickup {
    /// A weapon, taken through the sphere it hovers in.
    pub fn weapon(label: String, feet: Vec3) -> Self {
        let reach = Reach { center: feet + Vec3::Y * world_prop::WEAPON_HOVER, radius: world_prop::WEAPON_PICKUP_RADIUS };
        Pickup { label, feet, reach }
    }

    /// A placed prop. Taken through its colliders, which the bake does not read; the sphere a prop
    /// without them is taken through stands in.
    pub fn prop(label: String, at: Vec3) -> Self {
        Pickup { label, feet: at, reach: Reach { center: at, radius: world_prop::DEFAULT_PICKUP_RADIUS } }
    }
}

/// One viewpoint in another's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub viewpoint: u32,
    /// How many of the three body samples.
    pub samples: u8,
    /// The floor at the feet. Only in [`Sightlines::standing`].
    pub feet: bool,
}

/// Which viewpoints see which, kept for the probe. Each row lists what one eye sees.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sightlines {
    /// Per cell.
    pub viewpoint_of: Vec<u32>,
    /// Where each viewpoint's feet are.
    pub viewpoints: Vec<Vec3>,
    /// Per viewpoint, whether only a crouched body fits there, so its eye and body are crouched in
    /// every row.
    pub low: Vec<bool>,
    /// Standing eye to standing body.
    pub standing: Vec<Vec<Seen>>,
    /// Crouched eye to standing body.
    pub crouched_eye: Vec<Vec<Seen>>,
    /// Standing eye to crouched body.
    pub crouched_body: Vec<Vec<Seen>>,
}

/// Per cell. A field not baked yet is empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fields {
    pub knockback: Vec<f32>,
    pub sight: Vec<f32>,
    pub exposure: Vec<f32>,
    pub crouched_sight: Vec<f32>,
    pub crouched_exposure: Vec<f32>,
    pub cover: Vec<f32>,
    pub height: Vec<f32>,
    pub feet_sight: Vec<f32>,
    pub feet_exposure: Vec<f32>,
    pub traffic: Vec<f32>,
    pub watch: Vec<f32>,
    pub pickups: Vec<Pickup>,
    /// Per pickup, then per cell; infinite where the pickup cannot be reached.
    pub distances: Vec<Vec<f32>>,
    pub sightlines: Sightlines,
}

impl Fields {
    /// Everything but [`Field::Hurt`], which is the cell's own.
    pub fn get(&self, id: CellId, field: Field) -> Option<f32> {
        let at = |values: &Vec<f32>| values.get(id as usize).copied();
        match field {
            Field::Sight => at(&self.sight),
            Field::Exposure => at(&self.exposure),
            Field::Advantage => Some(at(&self.sight)? - at(&self.exposure)?),
            Field::CrouchedSight => at(&self.crouched_sight),
            Field::CrouchedExposure => at(&self.crouched_exposure),
            Field::CrouchedAdvantage => Some(at(&self.crouched_sight)? - at(&self.crouched_exposure)?),
            Field::FeetSight => at(&self.feet_sight),
            Field::FeetExposure => at(&self.feet_exposure),
            Field::FeetAdvantage => Some(at(&self.feet_sight)? - at(&self.feet_exposure)?),
            Field::Traffic => at(&self.traffic),
            Field::Watch => at(&self.watch),
            Field::Cover => at(&self.cover),
            Field::Knockback => at(&self.knockback),
            Field::Height => at(&self.height),
            Field::Distance(pickup) => self.distances.get(pickup).and_then(at),
            Field::Hurt => None,
        }
    }
}

/// The first stage: knockback per viewpoint, a batch at a time, then the distance to each pickup
/// and the traffic between them and the spawns, a search or a set of routes at a time.
pub struct FieldBake {
    viewpoints: Vec<Vec3>,
    low: Vec<bool>,
    viewpoint_of: Vec<u32>,
    knockback: Vec<f32>,
    pickups: Vec<Pickup>,
    /// The cells routes run between: spawns and pickups, once each.
    ends: Vec<CellId>,
    /// Out of each cell and into it, as `(cell, seconds)`, built once for every search.
    links: Option<(Vec<Vec<(CellId, f32)>>, Vec<Vec<(CellId, f32)>>)>,
    distances: Vec<Vec<f32>>,
    /// Per end, seconds from it and to it.
    from: Vec<Vec<f32>>,
    to: Vec<Vec<f32>>,
    /// Per cell, the routes that pass within [`ROUTE_SLACK`] of it.
    on: Vec<u32>,
    routes: u32,
    /// Ends whose routes are counted.
    routed: usize,
}

impl FieldBake {
    pub fn new(graph: &NavGraph, pickups: Vec<Pickup>, spawns: Vec<Vec3>) -> Self {
        let (viewpoints, low, viewpoint_of) = viewpoints(graph);
        let mut ends: Vec<CellId> = spawns.iter().chain(pickups.iter().map(|pickup| &pickup.feet)).filter_map(|feet| graph.cell_at(*feet)).collect();
        ends.sort_unstable();
        ends.dedup();
        FieldBake {
            viewpoints,
            low,
            viewpoint_of,
            knockback: Vec::new(),
            pickups,
            ends,
            links: None,
            distances: Vec::new(),
            from: Vec::new(),
            to: Vec::new(),
            on: vec![0; graph.cells().len()],
            routes: 0,
            routed: 0,
        }
    }

    /// Items: the knockback batches, the links, a search per pickup, two per end, the routes from
    /// each end, and the last pass.
    pub fn progress(&self) -> (usize, usize) {
        let completed = self.knockback.len().div_ceil(KNOCKBACK_BATCH)
            + usize::from(self.links.is_some())
            + self.distances.len()
            + self.from.len()
            + self.routed;
        let scheduled = self.viewpoints.len().div_ceil(KNOCKBACK_BATCH) + 1 + self.pickups.len() + 2 * self.ends.len() + 1;
        (completed, scheduled)
    }

    /// One item. True once only [`FieldBake::finish`] is left.
    pub fn step(&mut self, world: &CollisionWorld, graph: &NavGraph, triggers: &[TriggerVolume], rules: &MovementRules) -> bool {
        if self.knockback.len() < self.viewpoints.len() {
            let end = (self.knockback.len() + KNOCKBACK_BATCH).min(self.viewpoints.len());
            for viewpoint in self.knockback.len()..end {
                let (feet, stance) = (self.viewpoints[viewpoint], if self.low[viewpoint] { Stance::Crouched } else { Stance::Standing });
                let thrown = (0..KNOCKBACK_HEADINGS).filter(|heading| !thrown_clear(world, graph, triggers, rules, feet, stance, *heading)).count();
                self.knockback.push(thrown as f32 / KNOCKBACK_HEADINGS as f32);
            }
        } else if self.links.is_none() {
            self.links = Some((links_out(graph), links_into(graph)));
        } else if let Some((out, into)) = &self.links {
            if self.distances.len() < self.pickups.len() {
                // Taken standing over it, or in flight by a leap from wherever one starts.
                let pickup = self.distances.len();
                let over = graph.cell_at(self.pickups[pickup].feet).map(|cell| (cell, 0.0));
                let leaps = (0..graph.cells().len() as CellId)
                    .flat_map(|from| graph.links(from).iter().filter(|link| link.grabs == Some(pickup as u16)).map(move |link| (from, link.seconds)));
                let seeds: Vec<(CellId, f32)> = over.into_iter().chain(leaps).collect();
                self.distances.push(travel_from(into, &seeds));
            } else if self.from.len() < self.ends.len() {
                let end = self.ends[self.from.len()];
                self.from.push(travel(out, end));
                self.to.push(travel(into, end));
            } else if self.routed < self.ends.len() {
                self.count_routes_from(self.routed);
                self.routed += 1;
            }
        }
        self.knockback.len() == self.viewpoints.len() && self.links.is_some() && self.distances.len() == self.pickups.len() && self.routed == self.ends.len()
    }

    /// The routes from end `a` to every other: a band rather than one shortest path, which on a
    /// grid is one of many equally short and would draw a line wherever ties happened to break.
    fn count_routes_from(&mut self, a: usize) {
        let from = &self.from[a];
        for (b, to) in self.to.iter().enumerate() {
            let shortest = from[self.ends[b] as usize];
            if a == b || !shortest.is_finite() {
                continue;
            }
            self.routes += 1;
            for (cell, on) in self.on.iter_mut().enumerate() {
                if from[cell] + to[cell] <= shortest + ROUTE_SLACK {
                    *on += 1;
                }
            }
        }
    }

    pub fn finish(self) -> Fields {
        let knockback = self.viewpoint_of.iter().map(|viewpoint| self.knockback[*viewpoint as usize]).collect();
        let routes = self.routes;
        let traffic = self.on.into_iter().map(|on| if routes == 0 { 0.0 } else { on as f32 / routes as f32 }).collect();
        Fields { knockback, traffic, pickups: self.pickups, distances: self.distances, ..default() }
    }
}

/// What the second stage adds to [`Fields`].
#[derive(Clone, Debug, Default)]
pub struct Sight {
    pub sight: Vec<f32>,
    pub exposure: Vec<f32>,
    pub crouched_sight: Vec<f32>,
    pub crouched_exposure: Vec<f32>,
    pub cover: Vec<f32>,
    pub height: Vec<f32>,
    pub feet_sight: Vec<f32>,
    pub feet_exposure: Vec<f32>,
    pub watch: Vec<f32>,
    pub sightlines: Sightlines,
}

impl Fields {
    pub fn add_sight(&mut self, sight: Sight) {
        let Sight { sight, exposure, crouched_sight, crouched_exposure, cover, height, feet_sight, feet_exposure, watch, sightlines } = sight;
        (self.sight, self.exposure, self.crouched_sight, self.crouched_exposure) = (sight, exposure, crouched_sight, crouched_exposure);
        (self.cover, self.height, self.sightlines) = (cover, height, sightlines);
        (self.feet_sight, self.feet_exposure, self.watch) = (feet_sight, feet_exposure, watch);
    }

    pub fn has_sight(&self) -> bool {
        !self.sight.is_empty()
    }
}

/// The second stage, kept apart because it is most of the bake's time: a few rows of sightlines
/// per step, then everything derived from them at once.
pub struct SightBake {
    sightlines: Sightlines,
    area: Vec<f32>,
    rows_done: usize,
}

impl SightBake {
    pub fn new(graph: &NavGraph) -> Self {
        let (viewpoints, low, viewpoint_of) = viewpoints(graph);
        let mut area = vec![0.0; viewpoints.len()];
        for &viewpoint in &viewpoint_of {
            area[viewpoint as usize] += SPACING * SPACING;
        }
        let rows = vec![Vec::new(); viewpoints.len()];
        SightBake {
            sightlines: Sightlines { viewpoint_of, viewpoints, low, standing: rows.clone(), crouched_eye: rows.clone(), crouched_body: rows },
            area,
            rows_done: 0,
        }
    }

    fn count(&self) -> usize {
        self.sightlines.viewpoints.len()
    }

    /// Items: the batches of rows, and the last pass.
    pub fn progress(&self) -> (usize, usize) {
        (self.rows_done.div_ceil(SIGHT_BATCH), self.count().div_ceil(SIGHT_BATCH) + 1)
    }

    /// One batch. True once only [`SightBake::finish`] is left.
    pub fn step(&mut self, world: &CollisionWorld) -> bool {
        let end = (self.rows_done + SIGHT_BATCH).min(self.count());
        for from in self.rows_done..end {
            self.sight_row(world, from);
        }
        self.rows_done = end;
        self.rows_done == self.count()
    }

    /// Every pair of this viewpoint with a later one, both ways, since a segment is clear or not
    /// whichever end it is cast from.
    fn sight_row(&mut self, world: &CollisionWorld, a: usize) {
        let lines = &mut self.sightlines;
        let low = &lines.low;
        // A viewpoint only a crouched body fits is crouched however the row asks for it.
        let eye = |viewpoint: usize, crouching: bool| {
            lines.viewpoints[viewpoint] + Vec3::Y * if crouching || low[viewpoint] { CROUCH_EYE_HEIGHT } else { TALLEST_CLASS_EYE_HEIGHT }
        };
        let body = |viewpoint: usize, crouching: bool| if crouching || low[viewpoint] { &CROUCHED_BODY } else { &STANDING_BODY };
        let here = lines.viewpoints[a];
        let mut found: [Vec<(usize, usize, u8, bool)>; 3] = Default::default();
        for b in a + 1..lines.viewpoints.len() {
            let there = lines.viewpoints[b];
            if here.distance_squared(there) > SIGHT_RANGE * SIGHT_RANGE {
                continue;
            }
            let feet = |eye: Vec3, feet: Vec3| clear(world, eye, feet + Vec3::Y * FEET);
            // Standing eye to standing body, crouched eye to standing body, standing eye to crouched body.
            for (row, (eye_crouched, body_crouched)) in [(false, false), (true, false), (false, true)].into_iter().enumerate() {
                let with_feet = row == 0;
                for (from, to) in [(a, b), (b, a)] {
                    let samples = seen(world, eye(from, eye_crouched), lines.viewpoints[to], body(to, body_crouched));
                    let feet = with_feet && feet(eye(from, eye_crouched), lines.viewpoints[to]);
                    if samples > 0 || feet {
                        found[row].push((from, to, samples, feet));
                    }
                }
            }
        }
        for (rows, found) in [&mut lines.standing, &mut lines.crouched_eye, &mut lines.crouched_body].into_iter().zip(found) {
            for (from, to, samples, feet) in found {
                rows[from].push(Seen { viewpoint: to as u32, samples, feet });
            }
        }
    }

    pub fn finish(self, graph: &NavGraph) -> Sight {
        let count = self.count();
        let SightBake { sightlines, area, .. } = self;
        let share = |rows: &Vec<Vec<Seen>>, weight: &dyn Fn(&Seen) -> f32| {
            let (mut sees, mut seen) = (vec![0.0; count], vec![0.0; count]);
            for (eye, row) in rows.iter().enumerate() {
                for glimpse in row {
                    let (body, weight) = (glimpse.viewpoint as usize, weight(glimpse));
                    sees[eye] += area[body] * weight;
                    seen[body] += area[eye] * weight;
                }
            }
            (sees, seen)
        };
        let samples = |glimpse: &Seen| glimpse.samples as f32 / STANDING_BODY.len() as f32;
        let (sight, exposure) = share(&sightlines.standing, &samples);
        let (crouched_sight, _) = share(&sightlines.crouched_eye, &samples);
        let (_, crouched_exposure) = share(&sightlines.crouched_body, &samples);
        let (feet_sight, feet_exposure) = share(&sightlines.standing, &|glimpse| f32::from(u8::from(glimpse.feet)));

        // Each viewpoint's traffic is its cells' mean, so a square half off the route counts half.
        let mut traffic = vec![(0.0, 0usize); count];
        for (cell, viewpoint) in sightlines.viewpoint_of.iter().enumerate() {
            let entry = &mut traffic[*viewpoint as usize];
            *entry = (entry.0 + graph.fields().traffic.get(cell).copied().unwrap_or(0.0), entry.1 + 1);
        }
        let traffic: Vec<f32> = traffic.into_iter().map(|(sum, cells)| sum / cells.max(1) as f32).collect();
        let (watch, _) = share(&sightlines.standing, &|glimpse| samples(glimpse) * traffic[glimpse.viewpoint as usize]);

        let feet = |viewpoint: usize| sightlines.viewpoints[viewpoint].y;
        let mut lowest_viewer: Vec<f32> = (0..count).map(feet).collect();
        for (eye, row) in sightlines.standing.iter().enumerate() {
            for glimpse in row.iter().filter(|glimpse| glimpse.samples > 0) {
                let body = glimpse.viewpoint as usize;
                lowest_viewer[body] = lowest_viewer[body].min(feet(eye));
            }
        }
        let height: Vec<f32> = (0..count).map(|viewpoint| feet(viewpoint) - lowest_viewer[viewpoint]).collect();

        let per_cell = |values: &[f32]| sightlines.viewpoint_of.iter().map(|viewpoint| values[*viewpoint as usize]).collect::<Vec<f32>>();
        let exposure = per_cell(&exposure);
        let cover = cover(graph, &exposure);
        Sight {
            sight: per_cell(&sight),
            exposure,
            crouched_sight: per_cell(&crouched_sight),
            crouched_exposure: per_cell(&crouched_exposure),
            cover,
            height: per_cell(&height),
            feet_sight: per_cell(&feet_sight),
            feet_exposure: per_cell(&feet_exposure),
            watch: per_cell(&watch),
            sightlines,
        }
    }
}

/// Both stages at once, for tests and tools.
pub fn bake(world: &CollisionWorld, graph: &NavGraph, triggers: &[TriggerVolume], rules: &MovementRules, pickups: Vec<Pickup>, spawns: Vec<Vec3>) -> Fields {
    let mut first = FieldBake::new(graph, pickups, spawns);
    while !first.step(world, graph, triggers, rules) {}
    let mut graph = graph.clone();
    graph.set_fields(first.finish());
    let mut second = SightBake::new(&graph);
    while !second.step(world) {}
    let mut fields = graph.fields().clone();
    fields.add_sight(second.finish(&graph));
    fields
}

/// One viewpoint per square at each floor height, standing at the middle of the cells it holds, so
/// a map turned or mirrored on the meter grid is sampled the same way turned. Three cells in an L,
/// two on a diagonal, or cells across a step stand on the cell nearest that middle instead, since
/// the middle may be in whatever wall took the rest, or inside the step.
fn viewpoints(graph: &NavGraph) -> (Vec<Vec3>, Vec<bool>, Vec<u32>) {
    let mut squares: HashMap<IVec2, Vec<u32>> = HashMap::new();
    let mut members: Vec<Vec<CellId>> = Vec::new();
    let cells = graph.cells();
    let viewpoint_of = cells
        .iter()
        .enumerate()
        .map(|(id, cell)| {
            let square = squares.entry(cell.column.div_euclid(IVec2::splat(SIGHT_STRIDE))).or_default();
            // Crouched and standing cells are apart, since the two see from different heights.
            let near = square.iter().copied().find(|viewpoint| {
                let first = &cells[members[*viewpoint as usize][0] as usize];
                first.crouched == cell.crouched && (first.feet.y - cell.feet.y).abs() <= STEP_HEIGHT
            });
            let viewpoint = near.unwrap_or_else(|| {
                members.push(Vec::new());
                square.push(members.len() as u32 - 1);
                members.len() as u32 - 1
            });
            members[viewpoint as usize].push(id as CellId);
            viewpoint
        })
        .collect();
    let viewpoints = members
        .iter()
        .map(|held| {
            let feet = held.iter().map(|id| cells[*id as usize].feet);
            let middle = feet.clone().sum::<Vec3>() / held.len() as f32;
            let spread = held.iter().map(|id| cells[*id as usize].column).fold(IVec2::ZERO, |most, column| most.max((column - cells[held[0] as usize].column).abs()));
            let level = held.iter().all(|id| (cells[*id as usize].feet.y - cells[held[0] as usize].feet.y).abs() <= SAME_FLOOR);
            let whole = level && (held.len() == 4 || held.len() == 1 || spread.min_element() == 0);
            if whole { middle } else { feet.min_by(|a, b| a.distance_squared(middle).total_cmp(&b.distance_squared(middle))).expect("never empty") }
        })
        .collect();
    let low = members.iter().map(|held| cells[held[0] as usize].crouched).collect();
    (viewpoints, low, viewpoint_of)
}

/// How many of the samples up a body standing at `feet` an eye sees.
fn seen(world: &CollisionWorld, eye: Vec3, feet: Vec3, body: &[f32; 3]) -> u8 {
    body.iter().filter(|height| clear(world, eye, feet + Vec3::Y * **height)).count() as u8
}

/// Against the walls hitscan is tested against, so a sightline is a shot the game allows.
fn clear(world: &CollisionWorld, from: Vec3, to: Vec3) -> bool {
    world.segment_clear(from, to)
}

/// Whether a rocket's push along `heading` (of [`KNOCKBACK_HEADINGS`]) leaves a body of `stance` on
/// `feet` at rest on the graph, having touched no Kill trigger and come to rest in no Hurt one.
#[allow(clippy::too_many_arguments)]
fn thrown_clear(world: &CollisionWorld, graph: &NavGraph, triggers: &[TriggerVolume], rules: &MovementRules, feet: Vec3, stance: Stance, heading: usize) -> bool {
    let yaw = heading as f32 * std::f32::consts::TAU / KNOCKBACK_HEADINGS as f32;
    let out = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
    let kick = (out + Vec3::Y * KNOCKBACK_LIFT).normalize() * push_speed(KNOCKBACK_DAMAGE);
    let crouched = stance == Stance::Crouched;
    let duck = Duck { amount: if crouched { 1.0 } else { 0.0 }, feet: 0.0 };
    let mut mover = Mover {
        player: Player { ground: Some(Vec3::Y), duck, duck_before: duck, ..default() },
        body: PhysicsBody::at(center_on(feet, stance)),
        stance,
    };
    // Held, or a crouched body thrown out from under its ceiling would stand up as it lands.
    let (condition, input) = (Condition::default(), PlayerInput { crouch: crouched, ..default() });
    let touching = |center: Vec3, half: Vec3, effect: fn(&TriggerEffect) -> bool| triggers.iter().any(|trigger| effect(&trigger.effect) && trigger.overlaps(center, half));
    for tick in 0..MOST_TICKS {
        step(&mut mover, if tick == 0 { kick } else { Vec3::ZERO }, &input, &condition, None, world, rules, TICK);
        let (center, half) = (mover.body.current, mover.stance.half_extents());
        if touching(center, half, |effect| *effect == TriggerEffect::Kill) {
            return false;
        }
        let velocity = mover.player.velocity;
        if tick > 0 && mover.player.on_ground() && Vec2::new(velocity.x, velocity.z).length() < AT_REST {
            let resting = center - Vec3::Y * half.y;
            return !touching(center, half, |effect| matches!(effect, TriggerEffect::Hurt { .. })) && cell_near(graph.cells(), graph.columns(), resting).is_some();
        }
    }
    false
}

/// The lowest exposure among the cells the links reach within [`COVER_SECONDS`] of each cell, its
/// own included. One search per cell, so the buffers are reused rather than allocated per search.
fn cover(graph: &NavGraph, exposure: &[f32]) -> Vec<f32> {
    let count = graph.cells().len();
    let mut seconds = vec![f32::INFINITY; count];
    let mut reached: Vec<CellId> = Vec::new();
    let mut queue = BinaryHeap::new();
    (0..count as CellId)
        .map(|from| {
            for id in reached.drain(..) {
                seconds[id as usize] = f32::INFINITY;
            }
            seconds[from as usize] = 0.0;
            reached.push(from);
            queue.push(Reverse((0u32, from)));
            let mut least = exposure[from as usize];
            while let Some(Reverse((_, at))) = queue.pop() {
                let cost = seconds[at as usize];
                for link in graph.links(at) {
                    let next = cost + link.seconds;
                    if next <= COVER_SECONDS && next < seconds[link.to as usize] {
                        if seconds[link.to as usize].is_infinite() {
                            reached.push(link.to);
                            least = least.min(exposure[link.to as usize]);
                        }
                        seconds[link.to as usize] = next;
                        queue.push(Reverse(((next * 1000.0) as u32, link.to)));
                    }
                }
            }
            least
        })
        .collect()
}

/// Per cell, the links that lead into it, as `(from, seconds)`.
fn links_into(graph: &NavGraph) -> Vec<Vec<(CellId, f32)>> {
    let mut into = vec![Vec::new(); graph.cells().len()];
    for from in 0..graph.cells().len() as CellId {
        for link in graph.links(from) {
            into[link.to as usize].push((from, link.seconds));
        }
    }
    into
}

/// Per cell, the links that lead out of it, as `(to, seconds)`.
fn links_out(graph: &NavGraph) -> Vec<Vec<(CellId, f32)>> {
    (0..graph.cells().len() as CellId).map(|from| graph.links(from).iter().map(|link| (link.to, link.seconds)).collect()).collect()
}

/// Seconds between `start` and every cell along `links`: from it over [`links_out`], to it over
/// [`links_into`].
fn travel(links: &[Vec<(CellId, f32)>], start: CellId) -> Vec<f32> {
    travel_from(links, &[(start, 0.0)])
}

/// [`travel`] from several starts at once, each with the seconds it already costs.
fn travel_from(links: &[Vec<(CellId, f32)>], starts: &[(CellId, f32)]) -> Vec<f32> {
    let mut seconds = vec![f32::INFINITY; links.len()];
    let mut queue = BinaryHeap::new();
    for &(start, already) in starts {
        if already < seconds[start as usize] {
            seconds[start as usize] = already;
            queue.push(Reverse(((already * 1000.0) as u32, start)));
        }
    }
    while let Some(Reverse((_, at))) = queue.pop() {
        let cost = seconds[at as usize];
        for &(next_cell, time) in &links[at as usize] {
            let next = cost + time;
            if next < seconds[next_cell as usize] {
                seconds[next_cell as usize] = next;
                queue.push(Reverse(((next * 1000.0) as u32, next_cell)));
            }
        }
    }
    seconds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::world_surface::Volume;
    use crate::game::nav;
    use crate::tool::room::Room;

    fn room(min: Vec3, max: Vec3) -> Volume {
        Volume::air(Room::new(min, max))
    }

    fn baked(volumes: &[Volume], triggers: &[TriggerVolume], pickups: Vec<Pickup>) -> NavGraph {
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(volumes);
        let rules = MovementRules::default();
        let mut graph = nav::graph(&world, &faces, triggers, &rules, &[]);
        let fields = bake(&world, &graph, triggers, &rules, pickups, Vec::new());
        graph.set_fields(fields);
        graph
    }

    fn at(graph: &NavGraph, feet: Vec3, field: Field) -> f32 {
        let id = graph.cell_at(feet).expect("no cell there");
        graph.field(id, field).expect("not baked")
    }

    /// Breaks if sight passes through walls: a pillar between two cells hides each from the other.
    #[test]
    fn a_pillar_between_two_cells_hides_them_from_each_other() {
        let open = [room(Vec3::ZERO, Vec3::new(12.0, 4.0, 6.0))];
        let pillar = [open[0].clone(), Volume::solid(Room::new(Vec3::new(5.0, 0.0, 2.0), Vec3::new(7.0, 4.0, 4.0)))];
        let (a, b) = (Vec3::new(1.25, 0.0, 3.25), Vec3::new(10.75, 0.0, 3.25));
        let sees = |graph: &NavGraph| {
            let lines = &graph.fields().sightlines;
            let viewpoint = |feet| lines.viewpoint_of[graph.cell_at(feet).unwrap() as usize];
            lines.standing[viewpoint(a) as usize].iter().any(|seen| seen.viewpoint == viewpoint(b))
        };
        assert!(sees(&baked(&open, &[], Vec::new())), "two cells in an open room cannot see each other");
        assert!(!sees(&baked(&pillar, &[], Vec::new())), "two cells see each other through a pillar");
    }

    /// Breaks if a body is sampled at one height. A 3 m ledge, the high body 7 m back from its edge
    /// and the low one 13 m out: the edge hides the high body's knees, and not the low body's.
    #[test]
    fn a_ledge_sees_more_of_the_floor_below_than_the_floor_sees_of_it() {
        let volumes = [room(Vec3::ZERO, Vec3::new(20.0, 8.0, 4.0)), room(Vec3::new(20.0, 3.0, 0.0), Vec3::new(32.0, 11.0, 4.0))];
        let graph = baked(&volumes, &[], Vec::new());
        let lines = &graph.fields().sightlines;
        let (high_feet, low_feet) = (Vec3::new(27.25, 3.0, 2.25), Vec3::new(7.25, 0.0, 2.25));
        let viewpoint = |feet| lines.viewpoint_of[graph.cell_at(feet).unwrap() as usize] as usize;
        let (high, low) = (viewpoint(high_feet), viewpoint(low_feet));
        let samples = |eye: usize, body: usize| lines.standing[eye].iter().find(|seen| seen.viewpoint as usize == body).map_or(0, |seen| seen.samples);
        assert!(samples(high, low) > samples(low, high), "the ledge sees {} of the low body, and is seen {}", samples(high, low), samples(low, high));
        assert!(at(&graph, high_feet, Field::Height) > 2.0);
    }

    /// Breaks if a push is never simulated: the edge of a pit is dangerous and the middle of a
    /// room is not.
    #[test]
    fn a_cell_beside_a_pit_has_knockback_danger_and_one_in_the_middle_has_none() {
        let volumes = [room(Vec3::ZERO, Vec3::new(24.0, 4.0, 4.0)), room(Vec3::new(24.0, -6.0, 0.0), Vec3::new(30.0, 4.0, 4.0))];
        let kill = TriggerVolume { min: Vec3::new(24.0, -6.0, 0.0), max: Vec3::new(30.0, -5.0, 4.0), effect: TriggerEffect::Kill };
        let graph = baked(&volumes, &[kill], Vec::new());
        assert!(at(&graph, Vec3::new(23.25, 0.0, 2.25), Field::Knockback) > 0.0, "the edge of a pit is safe");
        assert_eq!(at(&graph, Vec3::new(10.25, 0.0, 2.25), Field::Knockback), 0.0, "the middle of a room is dangerous");
    }

    /// Breaks if distance is measured over links backwards, or is not travel time.
    #[test]
    fn distance_to_a_pickup_grows_away_from_it() {
        let pickup = Pickup::weapon("here".into(), Vec3::new(1.25, 0.0, 1.25));
        let graph = baked(&[room(Vec3::ZERO, Vec3::new(12.0, 4.0, 4.0))], &[], vec![pickup]);
        let near = at(&graph, Vec3::new(1.25, 0.0, 1.25), Field::Distance(0));
        let far = at(&graph, Vec3::new(10.75, 0.0, 1.25), Field::Distance(0));
        assert_eq!(near, 0.0);
        assert!(far > 1.0 && far < 3.0, "9.5 m at running speed took {far} s");
    }

    /// Breaks if a crouch-only viewpoint looks out from where a standing eye would be, inside the
    /// vent's ceiling: crouched in a 1.7 m vent, it sees down the vent into the room beyond.
    #[test]
    fn a_crouched_viewpoint_sees_from_under_its_ceiling() {
        let volumes = [room(Vec3::ZERO, Vec3::new(6.0, 1.7, 2.0)), room(Vec3::new(6.0, 0.0, -1.0), Vec3::new(12.0, 3.0, 3.0))];
        let graph = baked(&volumes, &[], Vec::new());
        let lines = &graph.fields().sightlines;
        let viewpoint = |feet| lines.viewpoint_of[graph.cell_at(feet).unwrap() as usize] as usize;
        let (vent, room) = (viewpoint(Vec3::new(1.25, 0.0, 1.25)), viewpoint(Vec3::new(10.25, 0.0, 1.25)));
        assert!(lines.low[vent], "the vent's viewpoint is not crouched");
        assert!(lines.standing[vent].iter().any(|seen| seen.viewpoint as usize == room), "the vent cannot see out of its own end");
        assert!(lines.standing[room].iter().any(|seen| seen.viewpoint as usize == vent), "the room cannot see into the vent");
    }

    /// Breaks if a viewpoint stands on whichever cell of its square came first: the same map turned
    /// a quarter is then sampled from other corners, and agrees with itself only roughly.
    #[test]
    fn a_map_turned_a_quarter_bakes_the_same_turned() {
        let boxes = [(Vec3::new(-8.0, 0.0, -3.0), Vec3::new(8.0, 4.0, 3.0)), (Vec3::new(3.0, 0.0, 3.0), Vec3::new(7.0, 4.0, 9.0))];
        let pillar = (Vec3::new(-2.0, 0.0, -1.0), Vec3::new(-1.0, 4.0, 1.0));
        let turn = |(min, max): (Vec3, Vec3)| {
            let (a, b) = (Vec3::new(min.z, min.y, -min.x), Vec3::new(max.z, max.y, -max.x));
            (a.min(b), a.max(b))
        };
        let volumes = |turned: bool| {
            let place = |corners| if turned { turn(corners) } else { corners };
            let mut volumes: Vec<Volume> = boxes.iter().map(|corners| { let (min, max) = place(*corners); room(min, max) }).collect();
            let (min, max) = place(pillar);
            volumes.push(Volume::solid(Room::new(min, max)));
            volumes
        };
        let (plain, turned) = (baked(&volumes(false), &[], Vec::new()), baked(&volumes(true), &[], Vec::new()));
        // A sightline grazing a corner exactly can fall either way in floating point: one sample
        // of one square.
        let one_sample = (SIGHT_STRIDE as f32 * SPACING).powi(2) / STANDING_BODY.len() as f32 + 1e-3;
        for (id, cell) in plain.cells().iter().enumerate() {
            let feet = cell.feet;
            let other = turned.cell_at(Vec3::new(feet.z, feet.y, -feet.x)).expect("the turned map has no cell there");
            for field in [Field::Exposure, Field::Sight, Field::FeetSight, Field::Knockback] {
                let (a, b) = (plain.field(id as CellId, field).unwrap(), turned.field(other, field).unwrap());
                assert!((a - b).abs() <= one_sample, "{field:?} at {feet}: {a} turned is {b}");
            }
        }
    }

    /// Breaks if feet are sampled as part of the body: a waist-high wall hides the floor at the
    /// feet behind it and not the head.
    #[test]
    fn a_waist_high_wall_hides_the_feet_and_not_the_head() {
        let volumes = [room(Vec3::ZERO, Vec3::new(12.0, 4.0, 4.0)), Volume::solid(Room::new(Vec3::new(8.0, 0.0, 0.0), Vec3::new(8.5, 1.0, 4.0)))];
        let graph = baked(&volumes, &[], Vec::new());
        let lines = &graph.fields().sightlines;
        let viewpoint = |feet| lines.viewpoint_of[graph.cell_at(feet).unwrap() as usize];
        let (eye, behind) = (viewpoint(Vec3::new(1.25, 0.0, 2.25)), viewpoint(Vec3::new(9.25, 0.0, 2.25)));
        let seen = lines.standing[eye as usize].iter().find(|seen| seen.viewpoint == behind).expect("the head behind the wall is not seen");
        assert!(!seen.feet, "the feet behind a 1 m wall are seen");
        assert!(at(&graph, Vec3::new(1.25, 0.0, 2.25), Field::FeetSight) < at(&graph, Vec3::new(1.25, 0.0, 2.25), Field::Sight));
    }

    /// Breaks if traffic is not the routes between ends: a corridor between two pickups carries all
    /// of it and a room off to the side none.
    #[test]
    fn a_corridor_between_pickups_is_busy_and_a_side_room_is_not() {
        let volumes = [room(Vec3::ZERO, Vec3::new(20.0, 3.0, 2.0)), room(Vec3::new(8.0, 0.0, 2.0), Vec3::new(12.0, 3.0, 8.0))];
        let pickups = vec![
            Pickup::weapon("west".into(), Vec3::new(0.75, 0.0, 0.75)),
            Pickup::weapon("east".into(), Vec3::new(19.25, 0.0, 0.75)),
        ];
        let graph = baked(&volumes, &[], pickups);
        assert!(at(&graph, Vec3::new(5.25, 0.0, 0.75), Field::Traffic) > 0.99);
        assert_eq!(at(&graph, Vec3::new(10.25, 0.0, 7.25), Field::Traffic), 0.0);
    }

    /// Breaks if cover is not the lowest exposure nearby: behind a pillar is cover for the cells
    /// just beside it.
    #[test]
    fn cover_is_the_least_exposure_a_step_away() {
        let volumes = [room(Vec3::ZERO, Vec3::new(12.0, 4.0, 6.0)), Volume::solid(Room::new(Vec3::new(5.0, 0.0, 2.0), Vec3::new(7.0, 4.0, 4.0)))];
        let graph = baked(&volumes, &[], Vec::new());
        for id in 0..graph.cells().len() as CellId {
            assert!(graph.field(id, Field::Cover).unwrap() <= graph.field(id, Field::Exposure).unwrap());
        }
    }
}
