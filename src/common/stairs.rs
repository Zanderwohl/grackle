//! A flight of stairs or a ramp, as geometry: what is drawn and what is collided with. The feature
//! that places one is [`crate::editor::stairs::EditorStairs`].
//!
//! Four points define it. `start` and `end` give the run; `width` lies off that line, and the
//! footprint is the rectangle from the line out to it; `height` lies off the footprint's plane,
//! and its distance from that plane is the rise. "Up" is toward the height point, so a footprint
//! on a level floor with the height point above it is an ordinary flight.
//!
//! What collides is the ramp from the start edge on the base toward the end edge at full height.
//! It passes through the middle of every tread, so each step is half above it and half below,
//! and a body walks the plane and does not catch on a step.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::csg::Polygon;
use crate::common::world_surface::Face;
use crate::game::collision::Hull;
use crate::game::player::WALKABLE;

/// Meters. A shorter run, width or height is degenerate.
const DEGENERATE: f32 = 1.0e-3;

/// Caps the faces one flight makes.
pub const MOST_STEPS: usize = 256;

/// The thinnest a thin collider is made, so a sweep still has a slab to stop on.
const THINNEST_COLLIDER: f32 = 0.02;

/// Flags and numbers rather than enums, so switching a choice off and back keeps its number.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct StairsStyle {
    pub steps: bool,
    /// Meters. The height each step is asked to be; the flight rounds it so a whole number of
    /// steps makes the rise.
    pub rise: f32,
    /// Solid down to the base, rather than floating slabs.
    pub thick: bool,
    /// Meters, for a thin flight.
    pub thickness: f32,
}

impl Default for StairsStyle {
    fn default() -> Self {
        Self { steps: true, rise: 0.25, thick: true, thickness: 0.1 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Degenerate {
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

    /// What is drawn, each face pointing out into the air.
    pub fn faces(&self) -> Vec<Face> {
        let Ok(frame) = self.frame() else { return Vec::new() };
        let mut faces = Faces { frame, faces: Vec::new() };
        match (self.style.steps, self.style.thick) {
            (false, true) => faces.thick_ramp(),
            (false, false) => faces.thin_ramp(self.style.thickness),
            (true, thick) => {
                let steps = Steps::of(&frame, self.style.rise);
                // Tread k's middle is on the ramp, so the first riser and the last, onto the floor
                // past the end edge, are half a step.
                for k in 0..steps.count {
                    let (near, far) = (k as f32 * steps.depth, (k + 1) as f32 * steps.depth);
                    let top = (k as f32 + 0.5) * steps.rise;
                    if thick {
                        faces.thick_step(near, far, top, if k == 0 { top } else { steps.rise });
                    } else {
                        // Never below the base: the first tread's top is only half a step up.
                        faces.block(near, far, (top - self.style.thickness).max(0.0), top);
                    }
                }
                if thick {
                    faces.back(frame.height - steps.rise / 2.0);
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

    /// Where the collider's slope ends, along, and how high its top is. For steps it stops at the
    /// last tread's height, halfway along it, so nothing solid stands above what is drawn there.
    pub fn collider_top(&self, frame: &Frame) -> (f32, f32) {
        if !self.style.steps {
            return (frame.run, frame.height);
        }
        let steps = Steps::of(frame, self.style.rise);
        (frame.run - steps.depth / 2.0, frame.height - steps.rise / 2.0)
    }

    /// What is collided with: thick, the solid under the collider's top faces, so nothing gets
    /// under it from any side; thin, a slab under each top face, as deep as what is drawn there.
    pub fn hulls(&self) -> Vec<Hull> {
        let Ok(frame) = self.frame() else { return Vec::new() };
        let (knee, top) = self.collider_top(&frame);
        if self.style.thick {
            let (r, w) = (frame.run, frame.width);
            let corners = [
                frame.at(0.0, 0.0, 0.0), frame.at(0.0, w, 0.0), frame.at(r, 0.0, 0.0), frame.at(r, w, 0.0),
                frame.at(knee, 0.0, top), frame.at(knee, w, top), frame.at(r, 0.0, top), frame.at(r, w, top),
            ];
            let faces = [frame.ramp_normal(), frame.up, -frame.up, frame.along, frame.across, -frame.across];
            let edges = [frame.along, frame.across, frame.up, frame.along * frame.run + frame.up * frame.height];
            vec![Hull::new(&corners, &faces, &edges)]
        } else {
            // Drawn thickness is straight down, so across a sloped face it is thinner.
            self.collider_faces()
                .into_iter()
                .map(|face| {
                    let depth = (self.style.thickness * face.plane.normal.dot(frame.up)).max(THINNEST_COLLIDER);
                    Hull::slab(&face.vertices, face.plane.normal, depth)
                })
                .collect()
        }
    }

    /// The collider's top, for what samples floor from faces (the nav graph's cells): the ramp up
    /// to [`Self::collider_top`]'s knee, then flat to the end edge.
    pub fn collider_faces(&self) -> Vec<Face> {
        let Ok(frame) = self.frame() else { return Vec::new() };
        let (knee, top) = self.collider_top(&frame);
        let (r, w) = (frame.run, frame.width);
        let mut faces: Vec<Face> = face(vec![frame.at(0.0, 0.0, 0.0), frame.at(0.0, w, 0.0), frame.at(knee, w, top), frame.at(knee, 0.0, top)], frame.ramp_normal()).into_iter().collect();
        if r - knee > DEGENERATE {
            faces.extend(face(vec![frame.at(knee, 0.0, top), frame.at(knee, w, top), frame.at(r, w, top), frame.at(r, 0.0, top)], frame.up));
        }
        faces
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

    fn back(&mut self, top: f32) {
        let f = self.frame;
        self.across((f.run, 0.0), (f.run, top), f.along);
    }

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
                for k in 0..4 {
                    let (near, far) = (k as f32, k as f32 + 1.0);
                    faces.extend(face(vec![Vec3::new(near, 0.0, 0.0), Vec3::new(far, 0.0, 0.0), Vec3::new(far, 0.0, 2.0), Vec3::new(near, 0.0, 2.0)], Vec3::NEG_Y));
                }
            } else {
                faces.extend(face(vec![Vec3::ZERO, Vec3::X * 4.0, Vec3::new(4.0, 0.0, 2.0), Vec3::Z * 2.0], Vec3::NEG_Y));
            }
            // Steps: 0.25 + 0.75 + 1.25 + 1.75 high, 1 m deep, 2 m wide. Ramp: half of 4 × 2 × 2.
            let expected = 8.0;
            assert!((volume(&faces) - expected).abs() < 1e-3, "steps {steps}: {}", volume(&faces));
        }
    }

    #[test]
    fn a_thin_flight_is_closed() {
        for steps in [false, true] {
            let style = StairsStyle { steps, rise: 0.5, thick: false, thickness: 0.1 };
            // Ramp: 4 m by 2 m, 0.1 m deep straight down. Steps: four 1 × 2 × 0.1 slabs.
            let expected = 0.8;
            assert!((volume(&flight(style).faces()) - expected).abs() < 1e-3, "steps {steps}");
        }
    }

    /// Breaks if the steps drift off the ramp: each tread's middle line is on it.
    #[test]
    fn every_tread_is_halved_by_the_ramp() {
        for thick in [false, true] {
            let shape = flight(StairsStyle { steps: true, rise: 0.3, thick, thickness: 0.1 });
            let ramp = shape.ramp().unwrap();
            let treads: Vec<Face> = shape.faces().into_iter().filter(|face| face.plane.normal.y > 0.99).collect();
            assert_eq!(treads.len(), 7);
            for tread in treads {
                let middle = tread.vertices.iter().sum::<Vec3>() / tread.vertices.len() as f32;
                assert!((ramp.plane.normal.dot(middle) - ramp.plane.w).abs() < 1e-4, "{middle} is off the ramp");
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
        world.rebuild_map(&[Volume::air(Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(10.0, 5.0, 5.0)))], &[flight(StairsStyle::default())]);
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
        world.rebuild_map(&[Volume::air(Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(10.0, 5.0, 5.0)))], &[flight(StairsStyle { thick: false, ..default() })]);
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
            let faces = world.rebuild_map(&[room.clone(), platform.clone()], with);
            let graph = nav::graph(&world, &faces, &[], &MovementRules::default(), &[spawn]);
            graph.cells().iter().filter(|cell| cell.feet.x > 7.0 && (cell.feet.y - 2.0).abs() < 0.1).count()
        };
        assert_eq!(on_the_platform(&[]), 0, "the platform is in reach without the flight, so this proves nothing");
        assert!(on_the_platform(&[flight]) > 0, "the nav graph does not climb the flight");
    }

    /// Breaks if a flight of one step draws nothing or stands nothing up.
    #[test]
    fn a_single_step_is_drawn_and_collides() {
        let shape = flight(StairsStyle { rise: 3.0, ..default() });
        assert_eq!(shape.steps().unwrap().count, 1);
        assert!(!shape.faces().is_empty() && !shape.hulls().is_empty());
    }

    /// Breaks if the collider stands taller at the back than the steps drawn there.
    #[test]
    fn nothing_solid_stands_over_the_last_tread() {
        for thick in [false, true] {
            let mut world = CollisionWorld::default();
            world.rebuild_map(&[Volume::air(Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(10.0, 5.0, 5.0)))], &[flight(StairsStyle { rise: 0.5, thick, ..default() })]);
            // Four steps: the last tread is 1.75 m up from 3 m to the end edge at 4 m.
            let behind = Ray3d::new(Vec3::new(8.0, 1.85, 1.0), Dir3::NEG_X);
            assert!((world.ray_distance(&behind, 20.0).unwrap() - 13.0).abs() < 1e-3, "thick {thick}: solid over the last tread");
            let down = Ray3d::new(Vec3::new(3.75, 4.0, 1.0), Dir3::NEG_Y);
            assert!((world.ray_distance(&down, 10.0).unwrap() - 2.25).abs() < 1e-3, "thick {thick}: the last tread is not the top");
        }
    }
}
