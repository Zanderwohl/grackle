//! What a cell is like to stand on, baked onto the nav graph: how dangerous it is, how much it sees
//! and is seen, and how far it is from each pickup. Stored in absolute units, never normalized to
//! the map. See "Fields, baked per cell" in documentation/bots.md.
//!
//! Sightlines and knockback are worked out per **viewpoint**, one cell standing for each square of
//! [`SIGHT_STRIDE`] columns at each floor height, and every cell reads its viewpoint's.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use bevy::prelude::*;

use crate::common::class::{body_center_from_feet, Stance, CROUCH_EYE_HEIGHT, TALLEST_CLASS_EYE_HEIGHT};
use crate::common::movement_rules::MovementRules;
use crate::common::trigger::{TriggerEffect, TriggerVolume};
use crate::game::collision::CollisionWorld;
use crate::game::condition::Condition;
use crate::game::explosion::push_speed;
use crate::game::nav::{cell_near, Cell, CellId, NavGraph, LIFT, MOST_TICKS, SPACING, TICK};
use crate::game::player::{step, Mover, PhysicsBody, Player, PlayerInput, PLAYER_HALF, STEP_HEIGHT};

/// Columns per side of the square one viewpoint stands for.
pub const SIGHT_STRIDE: i32 = 2;
/// Meters between feet; nothing further is looked at.
pub const SIGHT_RANGE: f32 = 40.0;
/// Heights above the feet sampled for how much of a body is seen: head, chest and knees.
const STANDING_BODY: [f32; 3] = [1.6, 1.1, 0.5];
const CROUCHED_BODY: [f32; 3] = [0.85, 0.6, 0.3];
/// Splash a rocket deals a body about a meter from it: the push every cell is tested against.
const KNOCKBACK_DAMAGE: u32 = 50;
/// Upward share of the push, since a blast at the feet throws up as well as out.
const KNOCKBACK_LIFT: f32 = 0.5;
const KNOCKBACK_HEADINGS: usize = 8;
/// Meters per second across the floor below which a thrown body has come to rest.
const AT_REST: f32 = 0.3;
/// Seconds of travel within which a cell's lowest exposure counts as its cover.
pub const COVER_SECONDS: f32 = 1.5;

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

#[derive(Clone, Debug, PartialEq)]
pub struct Pickup {
    pub label: String,
    pub feet: Vec3,
}

/// Which viewpoints see which, kept for the probe. Each row lists the viewpoints an eye sees a
/// body at, and how many of the three samples on it.
#[derive(Clone, Debug, Default)]
pub struct Sightlines {
    /// Per cell.
    pub viewpoint_of: Vec<u32>,
    pub viewpoints: Vec<CellId>,
    /// Standing eye to standing body.
    pub standing: Vec<Vec<(u32, u8)>>,
    /// Crouched eye to standing body.
    pub crouched_eye: Vec<Vec<(u32, u8)>>,
    /// Standing eye to crouched body.
    pub crouched_body: Vec<Vec<(u32, u8)>>,
}

/// Per cell. A field not baked yet is empty.
#[derive(Clone, Debug, Default)]
pub struct Fields {
    pub knockback: Vec<f32>,
    pub sight: Vec<f32>,
    pub exposure: Vec<f32>,
    pub crouched_sight: Vec<f32>,
    pub crouched_exposure: Vec<f32>,
    pub cover: Vec<f32>,
    pub height: Vec<f32>,
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
            Field::Cover => at(&self.cover),
            Field::Knockback => at(&self.knockback),
            Field::Height => at(&self.height),
            Field::Distance(pickup) => self.distances.get(pickup).and_then(at),
            Field::Hurt => None,
        }
    }
}

/// The first stage: knockback per viewpoint, a batch at a time, then the distance to each pickup.
pub struct FieldBake {
    viewpoints: Vec<CellId>,
    viewpoint_of: Vec<u32>,
    knockback: Vec<f32>,
    pickups: Vec<Pickup>,
}

impl FieldBake {
    pub fn new(graph: &NavGraph, pickups: Vec<Pickup>) -> Self {
        let (viewpoints, viewpoint_of) = viewpoints(graph);
        FieldBake { viewpoints, viewpoint_of, knockback: Vec::new(), pickups }
    }

    /// Items: the knockback batches, and the last pass.
    pub fn progress(&self) -> (usize, usize) {
        (self.knockback.len().div_ceil(KNOCKBACK_BATCH), self.viewpoints.len().div_ceil(KNOCKBACK_BATCH) + 1)
    }

    /// One batch. True once only [`FieldBake::finish`] is left.
    pub fn step(&mut self, world: &CollisionWorld, graph: &NavGraph, triggers: &[TriggerVolume], rules: &MovementRules) -> bool {
        let end = (self.knockback.len() + KNOCKBACK_BATCH).min(self.viewpoints.len());
        for viewpoint in self.knockback.len()..end {
            let feet = graph.cells()[self.viewpoints[viewpoint] as usize].feet;
            let thrown = (0..KNOCKBACK_HEADINGS).filter(|heading| !thrown_clear(world, graph, triggers, rules, feet, *heading)).count();
            self.knockback.push(thrown as f32 / KNOCKBACK_HEADINGS as f32);
        }
        self.knockback.len() == self.viewpoints.len()
    }

    pub fn finish(self, graph: &NavGraph) -> Fields {
        let knockback = self.viewpoint_of.iter().map(|viewpoint| self.knockback[*viewpoint as usize]).collect();
        let distances = self.pickups.iter().map(|pickup| distances_to(graph, pickup.feet)).collect();
        Fields { knockback, pickups: self.pickups, distances, ..default() }
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
    pub sightlines: Sightlines,
}

impl Fields {
    pub fn add_sight(&mut self, sight: Sight) {
        let Sight { sight, exposure, crouched_sight, crouched_exposure, cover, height, sightlines } = sight;
        (self.sight, self.exposure, self.crouched_sight, self.crouched_exposure) = (sight, exposure, crouched_sight, crouched_exposure);
        (self.cover, self.height, self.sightlines) = (cover, height, sightlines);
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
        let (viewpoints, viewpoint_of) = viewpoints(graph);
        let mut area = vec![0.0; viewpoints.len()];
        for &viewpoint in &viewpoint_of {
            area[viewpoint as usize] += SPACING * SPACING;
        }
        let rows = vec![Vec::new(); viewpoints.len()];
        SightBake {
            sightlines: Sightlines { viewpoint_of, viewpoints, standing: rows.clone(), crouched_eye: rows.clone(), crouched_body: rows },
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
    pub fn step(&mut self, world: &CollisionWorld, graph: &NavGraph) -> bool {
        let end = (self.rows_done + SIGHT_BATCH).min(self.count());
        for from in self.rows_done..end {
            self.sight_row(world, graph.cells(), from);
        }
        self.rows_done = end;
        self.rows_done == self.count()
    }

    /// Every pair of this viewpoint with a later one, both ways, since a segment is clear or not
    /// whichever end it is cast from.
    fn sight_row(&mut self, world: &CollisionWorld, cells: &[Cell], a: usize) {
        let lines = &mut self.sightlines;
        let feet = |viewpoint: usize| cells[lines.viewpoints[viewpoint] as usize].feet;
        let here = feet(a);
        let (eye, crouched_eye) = (here + Vec3::Y * TALLEST_CLASS_EYE_HEIGHT, here + Vec3::Y * CROUCH_EYE_HEIGHT);
        for b in a + 1..lines.viewpoints.len() {
            let there = feet(b);
            if here.distance_squared(there) > SIGHT_RANGE * SIGHT_RANGE {
                continue;
            }
            let (their_eye, their_crouched_eye) = (there + Vec3::Y * TALLEST_CLASS_EYE_HEIGHT, there + Vec3::Y * CROUCH_EYE_HEIGHT);
            let pairs = [
                (&mut lines.standing, seen(world, eye, there, &STANDING_BODY), seen(world, their_eye, here, &STANDING_BODY)),
                (&mut lines.crouched_eye, seen(world, crouched_eye, there, &STANDING_BODY), seen(world, their_crouched_eye, here, &STANDING_BODY)),
                (&mut lines.crouched_body, seen(world, eye, there, &CROUCHED_BODY), seen(world, their_eye, here, &CROUCHED_BODY)),
            ];
            for (rows, forward, back) in pairs {
                if forward > 0 {
                    rows[a].push((b as u32, forward));
                }
                if back > 0 {
                    rows[b].push((a as u32, back));
                }
            }
        }
    }

    pub fn finish(self, graph: &NavGraph) -> Sight {
        let cells = graph.cells();
        let count = self.count();
        let SightBake { sightlines, area, .. } = self;
        let share = |rows: &Vec<Vec<(u32, u8)>>| {
            let (mut sees, mut seen) = (vec![0.0; count], vec![0.0; count]);
            for (eye, row) in rows.iter().enumerate() {
                for &(body, samples) in row {
                    let fraction = samples as f32 / STANDING_BODY.len() as f32;
                    sees[eye] += area[body as usize] * fraction;
                    seen[body as usize] += area[eye] * fraction;
                }
            }
            (sees, seen)
        };
        let (sight, exposure) = share(&sightlines.standing);
        let (crouched_sight, _) = share(&sightlines.crouched_eye);
        let (_, crouched_exposure) = share(&sightlines.crouched_body);

        let feet = |viewpoint: usize| cells[sightlines.viewpoints[viewpoint] as usize].feet.y;
        let mut lowest_viewer: Vec<f32> = (0..count).map(feet).collect();
        for (eye, row) in sightlines.standing.iter().enumerate() {
            for &(body, _) in row {
                lowest_viewer[body as usize] = lowest_viewer[body as usize].min(feet(eye));
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
            sightlines,
        }
    }
}

/// Both stages at once, for tests and tools.
pub fn bake(world: &CollisionWorld, graph: &NavGraph, triggers: &[TriggerVolume], rules: &MovementRules, pickups: Vec<Pickup>) -> Fields {
    let mut first = FieldBake::new(graph, pickups);
    while !first.step(world, graph, triggers, rules) {}
    let mut fields = first.finish(graph);
    let mut second = SightBake::new(graph);
    while !second.step(world, graph) {}
    fields.add_sight(second.finish(graph));
    fields
}

/// The first cell of each square at each floor height stands for the rest of it.
fn viewpoints(graph: &NavGraph) -> (Vec<CellId>, Vec<u32>) {
    let mut squares: HashMap<IVec2, Vec<u32>> = HashMap::new();
    let mut viewpoints: Vec<CellId> = Vec::new();
    let cells = graph.cells();
    let viewpoint_of = cells
        .iter()
        .enumerate()
        .map(|(id, cell)| {
            let square = squares.entry(cell.column.div_euclid(IVec2::splat(SIGHT_STRIDE))).or_default();
            let near = square.iter().copied().find(|viewpoint| (cells[viewpoints[*viewpoint as usize] as usize].feet.y - cell.feet.y).abs() <= STEP_HEIGHT);
            near.unwrap_or_else(|| {
                viewpoints.push(id as CellId);
                square.push(viewpoints.len() as u32 - 1);
                viewpoints.len() as u32 - 1
            })
        })
        .collect();
    (viewpoints, viewpoint_of)
}

/// How many of the samples up a body standing at `feet` an eye sees.
fn seen(world: &CollisionWorld, eye: Vec3, feet: Vec3, body: &[f32; 3]) -> u8 {
    body.iter().filter(|height| clear(world, eye, feet + Vec3::Y * **height)).count() as u8
}

/// Against the walls hitscan is tested against, so a sightline is a shot the game allows.
fn clear(world: &CollisionWorld, from: Vec3, to: Vec3) -> bool {
    world.segment_clear(from, to)
}

/// Whether a rocket's push along `heading` (of [`KNOCKBACK_HEADINGS`]) leaves a body standing on
/// `feet` at rest on the graph, having touched no Kill trigger and come to rest in no Hurt one.
fn thrown_clear(world: &CollisionWorld, graph: &NavGraph, triggers: &[TriggerVolume], rules: &MovementRules, feet: Vec3, heading: usize) -> bool {
    let yaw = heading as f32 * std::f32::consts::TAU / KNOCKBACK_HEADINGS as f32;
    let out = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
    let kick = (out + Vec3::Y * KNOCKBACK_LIFT).normalize() * push_speed(KNOCKBACK_DAMAGE);
    let mut mover = Mover {
        player: Player { ground: Some(Vec3::Y), ..default() },
        body: PhysicsBody::at(body_center_from_feet(feet + Vec3::Y * LIFT)),
        stance: Stance::Standing,
    };
    let (condition, input) = (Condition::default(), PlayerInput::default());
    let touching = |center: Vec3, effect: fn(&TriggerEffect) -> bool| triggers.iter().any(|trigger| effect(&trigger.effect) && trigger.overlaps(center, PLAYER_HALF));
    for tick in 0..MOST_TICKS {
        step(&mut mover, if tick == 0 { kick } else { Vec3::ZERO }, &input, &condition, None, world, rules, TICK);
        let center = mover.body.current;
        if touching(center, |effect| *effect == TriggerEffect::Kill) {
            return false;
        }
        let velocity = mover.player.velocity;
        if tick > 0 && mover.player.on_ground() && Vec2::new(velocity.x, velocity.z).length() < AT_REST {
            let resting = center - Vec3::Y * PLAYER_HALF.y;
            return !touching(center, |effect| matches!(effect, TriggerEffect::Hurt { .. })) && cell_near(graph.cells(), graph.columns(), resting).is_some();
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

/// Seconds from every cell to the one under `feet`, over the links followed forwards.
fn distances_to(graph: &NavGraph, feet: Vec3) -> Vec<f32> {
    let cells = graph.cells();
    let mut seconds = vec![f32::INFINITY; cells.len()];
    let Some(target) = graph.cell_at(feet) else { return seconds };
    let mut into: Vec<Vec<(CellId, f32)>> = vec![Vec::new(); cells.len()];
    for from in 0..cells.len() as CellId {
        for link in graph.links(from) {
            into[link.to as usize].push((from, link.seconds));
        }
    }
    seconds[target as usize] = 0.0;
    let mut queue = BinaryHeap::from([Reverse((0u32, target))]);
    while let Some(Reverse((_, at))) = queue.pop() {
        let cost = seconds[at as usize];
        for &(from, time) in &into[at as usize] {
            let next = cost + time;
            if next < seconds[from as usize] {
                seconds[from as usize] = next;
                queue.push(Reverse(((next * 1000.0) as u32, from)));
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
        let fields = bake(&world, &graph, triggers, &rules, pickups);
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
            lines.standing[viewpoint(a) as usize].iter().any(|(seen, _)| *seen == viewpoint(b))
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
        let samples = |eye: usize, body: usize| lines.standing[eye].iter().find(|(seen, _)| *seen as usize == body).map_or(0, |(_, n)| *n);
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
        let pickup = Pickup { label: "here".into(), feet: Vec3::new(1.25, 0.0, 1.25) };
        let graph = baked(&[room(Vec3::ZERO, Vec3::new(12.0, 4.0, 4.0))], &[], vec![pickup]);
        let near = at(&graph, Vec3::new(1.25, 0.0, 1.25), Field::Distance(0));
        let far = at(&graph, Vec3::new(10.75, 0.0, 1.25), Field::Distance(0));
        assert_eq!(near, 0.0);
        assert!(far > 1.0 && far < 3.0, "9.5 m at running speed took {far} s");
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
