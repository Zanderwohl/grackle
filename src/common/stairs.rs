//! A flight of stairs or a ramp, as geometry: what is drawn and what is collided with. The feature
//! that places one is [`crate::editor::stairs::EditorStairs`].
//!
//! Four points define it. `start` and `end` give the run; `width` lies off that line, and the
//! footprint is the rectangle from the line out to it; `height` lies off the footprint's plane,
//! and its distance from that plane is the rise. "Up" is toward the height point, so a footprint
//! on a level floor with the height point above it is an ordinary flight.
//!
//! What is collided with is the ramp: a plane from the start edge on the base to the end edge at
//! full height. Steps are drawn entirely under it, their nosings on it, so a body walks the
//! plane and never catches on a step.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::csg::Polygon;
use crate::common::world_surface::Face;
use crate::game::collision::Hull;
use crate::game::player::WALKABLE;

/// Under this, in meters, a run, width or height is no length at all.
const DEGENERATE: f32 = 1.0e-3;

/// More steps than this is a ramp drawn the hard way.
pub const MOST_STEPS: usize = 256;

/// The thinnest a thin collider is made, so a sweep still has a slab to stop on.
const THINNEST_COLLIDER: f32 = 0.02;

/// How a flight is drawn and how solid it is. Each choice keeps its number when switched off, so
/// switching back loses nothing.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct StairsStyle {
    /// Steps rather than one sloped surface.
    pub steps: bool,
    /// Meters. The height each step is asked to be; the flight rounds it so a whole number of
    /// steps makes the rise.
    pub rise: f32,
    /// Solid down to the base, with walls round it, rather than a floating slab per step.
    pub thick: bool,
    /// Meters. How thick a thin ramp or each thin step is.
    pub thickness: f32,
}

impl Default for StairsStyle {
    fn default() -> Self {
        Self { steps: true, rise: 0.25, thick: true, thickness: 0.1 }
    }
}

/// Why four points make no flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Degenerate {
    /// `start` and `end` are the same point.
    NoRun,
    /// `width` lies on the line through `start` and `end`.
    NoWidth,
    /// `height` lies in the footprint's plane.
    NoHeight,
}

/// The flight's own axes: `along` from start to end, `across` toward the width point, `up` toward
/// the height point. Unit vectors, at right angles; either handedness, since a mirrored flight
/// keeps its points' sides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: Vec3,
    pub along: Vec3,
    pub across: Vec3,
    pub up: Vec3,
    /// Meters.
    pub run: f32,
    pub width: f32,
    pub height: f32,
}

impl Frame {
    /// `u` along, `w` across, `h` up, in meters from the start.
    pub fn at(&self, u: f32, w: f32, h: f32) -> Vec3 {
        self.origin + self.along * u + self.across * w + self.up * h
    }

    /// The ramp's normal, pointing out of the solid under it.
    pub fn ramp_normal(&self) -> Vec3 {
        (self.up * self.run - self.along * self.height).normalize()
    }

    /// Radians from the base's plane.
    pub fn slope(&self) -> f32 {
        self.height.atan2(self.run)
    }

    /// A box round the whole flight, base to top: center, a rotation taking the box's X, Y and Z
    /// onto `along`, `up` and `across`, and half extents.
    pub fn oriented_box(&self) -> (Vec3, Quat, Vec3) {
        let center = self.at(self.run / 2.0, self.width / 2.0, self.height / 2.0);
        // A rotation is right-handed; the box is symmetric, so flipping one axis changes nothing.
        let z = if self.along.cross(self.up).dot(self.across) > 0.0 { self.across } else { -self.across };
        let rotation = Quat::from_mat3(&Mat3::from_cols(self.along, self.up, z));
        (center, rotation, Vec3::new(self.run, self.height, self.width) / 2.0)
    }
}

/// How many steps a flight of `height` takes at `rise` each, and so each step's real rise and
/// depth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Steps {
    pub count: usize,
    /// Meters.
    pub rise: f32,
    pub depth: f32,
}

impl Steps {
    pub fn of(frame: &Frame, rise: f32) -> Steps {
        let count = (frame.height / rise.max(DEGENERATE)).round().clamp(1.0, MOST_STEPS as f32) as usize;
        Steps { count, rise: frame.height / count as f32, depth: frame.run / count as f32 }
    }
}

/// A flight as its feature resolved it: the four points and the style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StairsShape {
    pub start: Vec3,
    pub end: Vec3,
    pub width: Vec3,
    pub height: Vec3,
    pub style: StairsStyle,
}

/// On the feature's entity, and on each Mirror copy's.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Stairs(pub StairsShape);

impl StairsShape {
    pub fn frame(&self) -> Result<Frame, Degenerate> {
        let along = self.end - self.start;
        let run = along.length();
        if run < DEGENERATE {
            return Err(Degenerate::NoRun);
        }
        let along = along / run;
        let off = self.width - self.start;
        let across = off - along * off.dot(along);
        let width = across.length();
        if width < DEGENERATE {
            return Err(Degenerate::NoWidth);
        }
        let across = across / width;
        let normal = along.cross(across);
        let signed = (self.height - self.start).dot(normal);
        if signed.abs() < DEGENERATE {
            return Err(Degenerate::NoHeight);
        }
        Ok(Frame { origin: self.start, along, across, up: normal * signed.signum(), run, width, height: signed.abs() })
    }

    /// Whether a body can walk up the ramp: every class shares one hull and one steepest floor
    /// ([`WALKABLE`]), so what the slowest can climb, all can.
    pub fn walkable(&self) -> bool {
        self.frame().is_ok_and(|frame| frame.ramp_normal().y >= WALKABLE)
    }

    pub fn steps(&self) -> Option<Steps> {
        let frame = self.frame().ok()?;
        self.style.steps.then(|| Steps::of(&frame, self.style.rise))
    }

    /// What is drawn, each face pointing out into the air. None for a degenerate flight.
    pub fn faces(&self) -> Vec<Face> {
        let Ok(frame) = self.frame() else { return Vec::new() };
        let mut faces = Faces { frame, faces: Vec::new() };
        match (self.style.steps, self.style.thick) {
            (false, true) => faces.thick_ramp(),
            (false, false) => faces.thin_ramp(self.style.thickness),
            (true, thick) => {
                let steps = Steps::of(&frame, self.style.rise);
                // Step k (from 1) sits between risers k and k + 1, its nosing on the ramp. The last
                // riser is the end edge, up onto whatever floor the flight leads to.
                for k in 1..steps.count {
                    let (near, far, top) = (k as f32 * steps.depth, (k + 1) as f32 * steps.depth, k as f32 * steps.rise);
                    if thick {
                        faces.thick_step(near, far, top, steps.rise);
                    } else {
                        faces.block(near, far, top - self.style.thickness, top);
                    }
                }
                if thick && steps.count > 1 {
                    faces.back(steps.rise * (steps.count - 1) as f32);
                }
            }
        }
        faces.faces
    }

    /// The ramp's surface, pointing out of what is under it.
    pub fn ramp(&self) -> Option<Face> {
        let frame = self.frame().ok()?;
        let corners = ramp_corners(&frame);
        face(corners.to_vec(), frame.ramp_normal())
    }

    /// What is collided with: thick, the wedge under the ramp, so nothing gets under it from any
    /// side; thin, the ramp alone, as deep as the flight is thin.
    pub fn hulls(&self) -> Vec<Hull> {
        let Ok(frame) = self.frame() else { return Vec::new() };
        if self.style.thick {
            let (h, r, w) = (frame.height, frame.run, frame.width);
            let corners = [frame.at(0.0, 0.0, 0.0), frame.at(0.0, w, 0.0), frame.at(r, 0.0, 0.0), frame.at(r, w, 0.0), frame.at(r, 0.0, h), frame.at(r, w, h)];
            let faces = [frame.ramp_normal(), -frame.up, frame.along, frame.across, -frame.across];
            let edges = [frame.along, frame.across, frame.up, frame.along * r + frame.up * h];
            vec![Hull::new(&corners, &faces, &edges)]
        } else {
            let Some(ramp) = self.ramp() else { return Vec::new() };
            vec![Hull::slab(&ramp.vertices, ramp.plane.normal, self.style.thickness.max(THINNEST_COLLIDER))]
        }
    }

    /// The collider's faces, for what samples floor from faces (the nav graph's cells). Only the
    /// ramp is floor; a thick flight's walls are walls whichever faces list them.
    pub fn collider_faces(&self) -> Vec<Face> {
        self.ramp().into_iter().collect()
    }
}

/// Adds every flight's colliders to `world`, and their faces to `faces` beside the surface's.
pub fn add_to_world<'a>(world: &mut crate::game::collision::CollisionWorld, faces: &mut Vec<Face>, stairs: impl IntoIterator<Item = &'a StairsShape>) {
    let mut hulls = Vec::new();
    for flight in stairs {
        hulls.extend(flight.hulls());
        faces.extend(flight.collider_faces());
    }
    if !hulls.is_empty() {
        world.add_hulls(hulls);
    }
}

/// Start near, start far, end far, end near — the ramp's corners, going round it.
fn ramp_corners(frame: &Frame) -> [Vec3; 4] {
    let (r, w, h) = (frame.run, frame.width, frame.height);
    [frame.at(0.0, 0.0, 0.0), frame.at(0.0, w, 0.0), frame.at(r, w, h), frame.at(r, 0.0, h)]
}

/// A convex face through `corners`, wound to point along `outward` whichever way they go round.
fn face(mut corners: Vec<Vec3>, outward: Vec3) -> Option<Face> {
    let mut polygon = Polygon::new(corners.clone(), ())?;
    if polygon.plane.normal.dot(outward) < 0.0 {
        corners.reverse();
        polygon = Polygon::new(corners, ())?;
    }
    Some(polygon)
}

struct Faces {
    frame: Frame,
    faces: Vec<Face>,
}

impl Faces {
    fn add(&mut self, corners: Vec<Vec3>, outward: Vec3) {
        self.faces.extend(face(corners, outward));
    }

    /// A rectangle across the flight's width, from `(u0, h0)` to `(u1, h1)` in its side view.
    fn across(&mut self, (u0, h0): (f32, f32), (u1, h1): (f32, f32), outward: Vec3) {
        let (f, w) = (self.frame, self.frame.width);
        self.add(vec![f.at(u0, 0.0, h0), f.at(u1, 0.0, h1), f.at(u1, w, h1), f.at(u0, w, h0)], outward);
    }

    /// The same outline in the side view on both sides, each pointing out of its own side.
    fn sides(&mut self, outline: &[(f32, f32)]) {
        let (f, w) = (self.frame, self.frame.width);
        self.add(outline.iter().map(|&(u, h)| f.at(u, 0.0, h)).collect(), -f.across);
        self.add(outline.iter().map(|&(u, h)| f.at(u, w, h)).collect(), f.across);
    }

    fn thick_ramp(&mut self) {
        let (f, r, h) = (self.frame, self.frame.run, self.frame.height);
        self.across((0.0, 0.0), (r, h), f.ramp_normal());
        self.back(h);
        self.sides(&[(0.0, 0.0), (r, 0.0), (r, h)]);
    }

    fn thin_ramp(&mut self, thickness: f32) {
        let (f, r, h, t) = (self.frame, self.frame.run, self.frame.height, thickness);
        let normal = f.ramp_normal();
        self.across((0.0, 0.0), (r, h), normal);
        self.across((0.0, -t), (r, h - t), -normal);
        self.across((0.0, -t), (0.0, 0.0), -f.along);
        self.across((r, h - t), (r, h), f.along);
        self.sides(&[(0.0, -t), (r, h - t), (r, h), (0.0, 0.0)]);
    }

    /// A column of solid from the base to `top`, between `near` and `far` along, with its tread
    /// and the riser in front of it down to the step below.
    fn thick_step(&mut self, near: f32, far: f32, top: f32, rise: f32) {
        let f = self.frame;
        self.across((near, top), (far, top), f.up);
        self.across((near, top - rise), (near, top), -f.along);
        self.sides(&[(near, 0.0), (far, 0.0), (far, top), (near, top)]);
    }

    /// The thick flight's back, at the end edge, up to `top`.
    fn back(&mut self, top: f32) {
        let f = self.frame;
        self.across((f.run, 0.0), (f.run, top), f.along);
    }

    /// A floating slab between `near` and `far` along and `bottom` and `top` up.
    fn block(&mut self, near: f32, far: f32, bottom: f32, top: f32) {
        let f = self.frame;
        self.across((near, top), (far, top), f.up);
        self.across((near, bottom), (far, bottom), -f.up);
        self.across((near, bottom), (near, top), -f.along);
        self.across((far, bottom), (far, top), f.along);
        self.sides(&[(near, bottom), (far, bottom), (far, top), (near, top)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::collision::CollisionWorld;
    use crate::tool::room::Room;
    use crate::common::world_surface::Volume;

    /// 4 m along +X, 2 m wide along +Z, 2 m up.
    fn flight(style: StairsStyle) -> StairsShape {
        StairsShape { start: Vec3::ZERO, end: Vec3::new(4.0, 0.0, 0.0), width: Vec3::new(1.0, 0.0, 2.0), height: Vec3::new(3.0, 2.0, 1.0), style }
    }

    fn volume(faces: &[Face]) -> f32 {
        faces.iter().map(|face| {
            let a = face.vertices[0];
            (1..face.vertices.len() - 1).map(|i| a.dot(face.vertices[i].cross(face.vertices[i + 1]))).sum::<f32>()
        }).sum::<f32>() / 6.0
    }

    #[test]
    fn the_frame_reads_the_four_points() {
        let frame = flight(StairsStyle::default()).frame().unwrap();
        assert!((frame.run - 4.0).abs() < 1e-5 && (frame.width - 2.0).abs() < 1e-5 && (frame.height - 2.0).abs() < 1e-5, "{frame:?}");
        assert!(frame.up.abs_diff_eq(Vec3::Y, 1e-5));
        assert!(frame.across.abs_diff_eq(Vec3::Z, 1e-5));
    }

    #[test]
    fn points_in_a_line_or_a_plane_make_no_flight() {
        let mut shape = flight(StairsStyle::default());
        shape.width = Vec3::new(2.0, 0.0, 0.0);
        assert_eq!(shape.frame(), Err(Degenerate::NoWidth));
        let mut shape = flight(StairsStyle::default());
        shape.height = Vec3::new(3.0, 0.0, 1.5);
        assert_eq!(shape.frame(), Err(Degenerate::NoHeight));
        assert!(shape.faces().is_empty() && shape.hulls().is_empty());
        let mut shape = flight(StairsStyle::default());
        shape.end = shape.start;
        assert_eq!(shape.frame(), Err(Degenerate::NoRun));
    }

    /// Breaks if a riser or a side is wound inward: the solid's volume comes out short or negative.
    #[test]
    fn a_thick_flight_is_closed_but_for_its_base_and_measures_its_volume() {
        for steps in [false, true] {
            let style = StairsStyle { steps, rise: 0.5, thick: true, thickness: 0.1 };
            let mut faces = flight(style).faces();
            // The base, so the volume is of a closed solid.
            if steps {
                for k in 1..4 {
                    let (near, far) = (k as f32, k as f32 + 1.0);
                    faces.extend(face(vec![Vec3::new(near, 0.0, 0.0), Vec3::new(far, 0.0, 0.0), Vec3::new(far, 0.0, 2.0), Vec3::new(near, 0.0, 2.0)], Vec3::NEG_Y));
                }
            } else {
                faces.extend(face(vec![Vec3::ZERO, Vec3::X * 4.0, Vec3::new(4.0, 0.0, 2.0), Vec3::Z * 2.0], Vec3::NEG_Y));
            }
            // Steps: 0.5 + 1.0 + 1.5 high, 1 m deep, 2 m wide. Ramp: half of 4 × 2 × 2.
            let expected = if steps { 6.0 } else { 8.0 };
            assert!((volume(&faces) - expected).abs() < 1e-3, "steps {steps}: {}", volume(&faces));
        }
    }

    #[test]
    fn a_thin_flight_is_closed() {
        for steps in [false, true] {
            let style = StairsStyle { steps, rise: 0.5, thick: false, thickness: 0.1 };
            // Ramp: 4 m by 2 m, 0.1 m deep straight down. Steps: three 1 × 2 × 0.1 slabs.
            let expected = if steps { 0.6 } else { 0.8 };
            assert!((volume(&flight(style).faces()) - expected).abs() < 1e-3, "steps {steps}");
        }
    }

    /// Breaks if a step stands proud of the ramp, where a body would catch on its nosing.
    #[test]
    fn every_step_is_under_the_ramp() {
        for thick in [false, true] {
            let shape = flight(StairsStyle { steps: true, rise: 0.3, thick, thickness: 0.1 });
            let ramp = shape.ramp().unwrap();
            for face in shape.faces() {
                for vertex in face.vertices {
                    assert!(ramp.plane.normal.dot(vertex) <= ramp.plane.w + 1e-4, "{vertex} is above the ramp");
                }
            }
        }
    }

    #[test]
    fn steps_round_the_rise_to_fit_the_height() {
        let frame = flight(StairsStyle::default()).frame().unwrap();
        let steps = Steps::of(&frame, 0.3);
        assert_eq!(steps.count, 7);
        assert!((steps.rise * 7.0 - 2.0).abs() < 1e-5 && (steps.depth * 7.0 - 4.0).abs() < 1e-5);
    }

    #[test]
    fn a_steep_flight_is_not_walkable() {
        assert!(flight(StairsStyle::default()).walkable());
        let mut steep = flight(StairsStyle::default());
        steep.end = Vec3::new(1.5, 0.0, 0.0);
        assert!(!steep.walkable(), "{} degrees", steep.frame().unwrap().slope().to_degrees());
    }

    /// Breaks if the thick collider leaves a way under the ramp.
    #[test]
    fn nothing_gets_under_a_thick_flight() {
        let mut world = CollisionWorld::default();
        let mut faces = world.rebuild_volumes(&[Volume::air(Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(10.0, 5.0, 5.0)))]);
        let shape = flight(StairsStyle::default());
        add_to_world(&mut world, &mut faces, [&shape]);
        let side = Ray3d::new(Vec3::new(3.0, 0.5, -3.0), Dir3::Z);
        assert!((world.ray_distance(&side, 10.0).unwrap() - 3.0).abs() < 1e-3);
        let back = Ray3d::new(Vec3::new(8.0, 0.5, 1.0), Dir3::NEG_X);
        assert!((world.ray_distance(&back, 10.0).unwrap() - 4.0).abs() < 1e-3);
        let down = Ray3d::new(Vec3::new(2.0, 4.0, 1.0), Dir3::NEG_Y);
        assert!((world.ray_distance(&down, 10.0).unwrap() - 3.0).abs() < 1e-3, "the ramp is not at half height halfway up");
    }

    #[test]
    fn a_thin_flight_lets_things_under_its_high_end() {
        let mut world = CollisionWorld::default();
        let mut faces = world.rebuild_volumes(&[Volume::air(Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(10.0, 5.0, 5.0)))]);
        let shape = flight(StairsStyle { thick: false, ..default() });
        add_to_world(&mut world, &mut faces, [&shape]);
        let under = Ray3d::new(Vec3::new(3.5, 0.5, -3.0), Dir3::Z);
        assert!((world.ray_distance(&under, 20.0).unwrap() - 8.0).abs() < 1e-3, "the thin ramp has walls");
    }

    /// Breaks if the box's rotation does not carry its axes onto the flight's, whichever way the
    /// width point lies.
    #[test]
    fn the_box_lies_along_the_flight_either_side() {
        for side in [2.0, -2.0] {
            let mut shape = flight(StairsStyle::default());
            shape.width = Vec3::new(1.0, 0.0, side);
            let frame = shape.frame().unwrap();
            let (center, rotation, half) = frame.oriented_box();
            assert!(center.abs_diff_eq(Vec3::new(2.0, 1.0, side / 2.0), 1e-5));
            assert!((rotation * Vec3::X).abs_diff_eq(Vec3::X, 1e-5) && (rotation * Vec3::Y).abs_diff_eq(Vec3::Y, 1e-5));
            assert!(half.abs_diff_eq(Vec3::new(2.0, 1.0, 1.0), 1e-5));
        }
    }

    /// Breaks if the nav graph finds no floor on a ramp: a 2 m platform is out of reach without it.
    #[test]
    fn the_nav_graph_climbs_a_flight_onto_a_platform() {
        use crate::common::movement_rules::MovementRules;
        use crate::game::nav;
        let room = Volume::air(Room::new(Vec3::new(-6.0, 0.0, -4.0), Vec3::new(12.0, 6.0, 6.0)));
        let platform = Volume::solid(Room::new(Vec3::new(6.0, 0.0, -4.0), Vec3::new(12.0, 2.0, 6.0)));
        let flight = StairsShape {
            start: Vec3::new(1.0, 0.0, 0.0),
            end: Vec3::new(6.0, 0.0, 0.0),
            width: Vec3::new(1.0, 0.0, 2.0),
            height: Vec3::new(1.0, 2.0, 0.0),
            style: StairsStyle::default(),
        };
        assert!(flight.walkable());
        let spawn = Vec3::new(-4.0, 0.0, 1.0);
        let on_the_platform = |with: &[StairsShape]| {
            let mut world = CollisionWorld::default();
            let mut faces = world.rebuild_volumes(&[room.clone(), platform.clone()]);
            add_to_world(&mut world, &mut faces, with);
            let graph = nav::graph(&world, &faces, &[], &MovementRules::default(), &[spawn]);
            graph.cells().iter().filter(|cell| cell.feet.x > 7.0 && (cell.feet.y - 2.0).abs() < 0.1).count()
        };
        assert_eq!(on_the_platform(&[]), 0, "the platform is in reach without the flight, so this proves nothing");
        assert!(on_the_platform(&[flight]) > 0, "the nav graph does not climb the flight");
    }
}
