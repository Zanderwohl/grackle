//! A 2D drawing on a plane, which sweeps will turn into solids. See
//! `documentation/sketches.md`.
//!
//! A flat set, not a feature list: nothing in a drawing depends on the order
//! it was drawn in. Lines name their ends by id, so two lines sharing a corner
//! share a point, and whether a loop is closed is a question about ids rather
//! than about two coordinates agreeing. Undo is the prop's own; a sketch is
//! part of the document.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prop::plane::Plane;
use crate::prop::region::{centroid, Arrangement, Bound, Region, RegionRef};

/// Points and entities share one counter, and an id is never reused, so a
/// region or a later constraint naming one cannot be handed another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SketchId(pub u32);

impl std::fmt::Display for SketchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SketchPoint {
    pub id: SketchId,
    /// Metres along the plane's `x` and `y`.
    #[serde(with = "crate::prop::nice_f32::array")]
    pub at: [f32; 2],
}

/// Every field a scalar, so an entity is one flat TOML table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SketchEntity {
    Line {
        id: SketchId,
        a: SketchId,
        b: SketchId,
        /// Drawn and snapped to, never part of a region: a revolve's axis, a
        /// centre line.
        #[serde(default)]
        construction: bool,
    },
}

impl SketchEntity {
    pub fn id(&self) -> SketchId {
        match self {
            SketchEntity::Line { id, .. } => *id,
        }
    }

    pub fn construction(&self) -> bool {
        match self {
            SketchEntity::Line { construction, .. } => *construction,
        }
    }

    pub fn points(&self) -> Vec<SketchId> {
        match self {
            SketchEntity::Line { a, b, .. } => vec![*a, *b],
        }
    }
}

/// A region a sweep takes, kept on the sketch so that every sweep using it
/// names this rather than the lines round it — re-picking it here re-points
/// them all. See [`Arrangement::resolve`] for what it finds once the drawing
/// has changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SketchRegion {
    pub id: SketchId,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Where its middle was when picked, in the plane. Only chooses between
    /// regions answering to the same name.
    #[serde(with = "crate::prop::nice_f32::array")]
    pub hint: [f32; 2],
    pub bounds: Vec<Bound>,
}

impl SketchRegion {
    pub fn name(&self) -> RegionRef {
        RegionRef { bounds: self.bounds.clone() }
    }
}

/// `next_id` first: TOML needs every scalar before any sub-table.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sketch {
    pub next_id: u32,
    #[serde(default)]
    pub points: Vec<SketchPoint>,
    #[serde(default)]
    pub entities: Vec<SketchEntity>,
    #[serde(default)]
    pub regions: Vec<SketchRegion>,
}

impl Sketch {
    fn take_id(&mut self) -> SketchId {
        let id = SketchId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn add_point(&mut self, at: Vec2) -> SketchId {
        let id = self.take_id();
        self.points.push(SketchPoint { id, at: at.to_array() });
        id
    }

    pub fn add_line(&mut self, a: SketchId, b: SketchId) -> SketchId {
        let id = self.take_id();
        self.entities.push(SketchEntity::Line { id, a, b, construction: false });
        id
    }

    /// A line's two ends, if `id` is a line whose points are both there.
    pub fn line(&self, id: SketchId) -> Option<(Vec2, Vec2)> {
        let (_, a, b, _) = self.lines().find(|(line, ..)| *line == id)?;
        Some((self.point(a)?, self.point(b)?))
    }

    pub fn point(&self, id: SketchId) -> Option<Vec2> {
        self.points.iter().find(|point| point.id == id).map(|point| Vec2::from_array(point.at))
    }

    pub fn move_point(&mut self, id: SketchId, at: Vec2) {
        if let Some(point) = self.points.iter_mut().find(|point| point.id == id) {
            point.at = at.to_array();
        }
    }

    pub fn set_construction(&mut self, line: SketchId, to: bool) {
        for entity in &mut self.entities {
            match entity {
                SketchEntity::Line { id, construction, .. } if *id == line => *construction = to,
                SketchEntity::Line { .. } => {}
            }
        }
    }

    /// Takes the points and entities named, and every entity that ends on a
    /// point taken — a line with one end is no line.
    pub fn remove(&mut self, ids: &[SketchId]) {
        self.points.retain(|point| !ids.contains(&point.id));
        let points = &self.points;
        self.entities.retain(|entity| {
            !ids.contains(&entity.id()) && entity.points().iter().all(|end| points.iter().any(|p| p.id == *end))
        });
    }

    pub fn region(&self, id: SketchId) -> Option<&SketchRegion> {
        self.regions.iter().find(|region| region.id == id)
    }

    /// The entry for `region`: the one already naming it, so two sweeps of
    /// one region share it, or a new one.
    pub fn adopt_region(&mut self, region: &Region) -> SketchId {
        if let Some(existing) = self.regions.iter().find(|entry| entry.bounds == region.name.bounds) {
            return existing.id;
        }
        let id = self.take_id();
        self.regions.push(SketchRegion {
            id,
            label: String::new(),
            hint: centroid(&region.outer).to_array(),
            bounds: region.name.bounds.clone(),
        });
        id
    }

    /// Points entry `id` at `region` instead, for every sweep using it.
    pub fn repoint_region(&mut self, id: SketchId, region: &Region) {
        if let Some(entry) = self.regions.iter_mut().find(|entry| entry.id == id) {
            entry.bounds = region.name.bounds.clone();
            entry.hint = centroid(&region.outer).to_array();
        }
    }

    /// Drops entries nothing uses, unless somebody named them.
    pub fn prune_regions(&mut self, used: &[SketchId]) {
        self.regions.retain(|region| !region.label.is_empty() || used.contains(&region.id));
    }

    pub fn lines(&self) -> impl Iterator<Item = (SketchId, SketchId, SketchId, bool)> + '_ {
        self.entities.iter().map(|entity| match *entity {
            SketchEntity::Line { id, a, b, construction } => (id, a, b, construction),
        })
    }
}

/// A sketch as evaluated: where its plane was when it ran.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedSketch {
    pub id: crate::prop::feature::PropFeatureId,
    pub plane: Plane,
    pub sketch: Sketch,
    /// Its regions, found once per evaluation.
    pub arrangement: Arrangement,
}

impl PlacedSketch {
    pub fn world(&self, at: Vec2) -> Vec3 {
        self.plane.lift(at)
    }

    /// What region entry `id` means in the drawing as it now stands.
    pub fn resolve(&self, id: SketchId) -> Option<Region> {
        let entry = self.sketch.region(id)?;
        self.arrangement.resolve(&entry.name(), Vec2::from_array(entry.hint))
    }

    /// How a region entry is shown: its label, or its id.
    pub fn region_label(&self, id: SketchId) -> String {
        region_label(&self.sketch, id)
    }

    /// Each line's ends in the prop's space, and whether it is construction.
    /// A line naming a point that is not there is left out.
    pub fn world_lines(&self) -> Vec<(Vec3, Vec3, bool)> {
        self.sketch
            .lines()
            .filter_map(|(_, a, b, construction)| {
                Some((self.world(self.sketch.point(a)?), self.world(self.sketch.point(b)?), construction))
            })
            .collect()
    }
}

pub fn region_label(sketch: &Sketch, id: SketchId) -> String {
    match sketch.region(id) {
        Some(region) if !region.label.is_empty() => region.label.clone(),
        _ => crate::get!("prop.sketch.region_label", "id", id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::document::{parse, to_text, PropDoc};
    use crate::prop::feature::{evaluate, FeatureOp, PropFeatureId};
    use crate::prop::plane::{OriginPlane, PlaneRef};
    use crate::prop::profile::Placement;
    use crate::prop::reference::{Element, GeomRef, Source};
    use crate::prop::solid::Shape;
    use crate::prop::surface::Surface;

    fn triangle() -> Sketch {
        let mut sketch = Sketch::default();
        let a = sketch.add_point(Vec2::new(0.0, 0.0));
        let b = sketch.add_point(Vec2::new(0.05, 0.0));
        let c = sketch.add_point(Vec2::new(0.0, 0.05));
        sketch.add_line(a, b);
        sketch.add_line(b, c);
        sketch.add_line(c, a);
        sketch
    }

    /// A doc with a sketch on an origin plane and one on a plane feature.
    fn doc() -> (PropDoc, PropFeatureId) {
        let mut doc = PropDoc::new("test");
        let cube = doc.push(FeatureOp::Primitive {
            shape: Shape::Box { size: [0.2; 3] },
            placement: Placement::default(),
            surface: Surface::default(),
        });
        let top = doc.push(FeatureOp::Plane {
            // The far cap: the cube's +Z face.
            through: vec![GeomRef { element: Element::Face(1), source: Source { feature: cube, copies: vec![] } }],
        });
        doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xz), sketch: triangle() });
        doc.push(FeatureOp::Sketch { on: PlaneRef::Feature(top), sketch: triangle() });
        (doc, top)
    }

    #[test]
    fn ids_are_shared_and_never_reused() {
        let sketch = triangle();
        let mut ids: Vec<SketchId> =
            sketch.points.iter().map(|p| p.id).chain(sketch.entities.iter().map(SketchEntity::id)).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 6);
        assert_eq!(sketch.next_id, 6);
    }

    #[test]
    fn a_sketch_survives_the_file() {
        let (doc, _) = doc();
        let text = to_text(&doc).unwrap();
        assert_eq!(parse(&text).unwrap(), doc, "{text}");
    }

    #[test]
    fn a_sketch_is_drawn_on_its_plane_and_makes_no_body() {
        let (doc, _) = doc();
        let evaluated = evaluate(&doc.features);
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        assert_eq!(evaluated.bodies.len(), 1, "only the cube");
        assert_eq!(evaluated.sketches.len(), 2);

        // On XZ the sketch's x is world X and its y is world -Z.
        let floor = &evaluated.sketches[0];
        assert!(floor.world(Vec2::new(0.05, 0.0)).distance(Vec3::new(0.05, 0.0, 0.0)) < 1e-6);
        assert!(floor.world(Vec2::new(0.0, 0.05)).distance(Vec3::new(0.0, 0.0, -0.05)) < 1e-6);

        let on_top = &evaluated.sketches[1];
        for (a, b, _) in on_top.world_lines() {
            assert!((a.z - 0.1).abs() < 1e-6 && (b.z - 0.1).abs() < 1e-6, "{a} {b} are off the top face");
        }
    }

    #[test]
    fn a_sketch_on_a_missing_plane_is_a_problem() {
        let mut doc = PropDoc::new("test");
        doc.push(FeatureOp::Sketch { on: PlaneRef::Feature(PropFeatureId(42)), sketch: triangle() });
        let evaluated = evaluate(&doc.features);
        assert!(evaluated.sketches.is_empty());
        assert_eq!(evaluated.problems.len(), 1);
    }

    #[test]
    fn a_sketch_is_pinned_below_its_plane() {
        let (mut doc, top) = doc();
        let on_top = doc.features.last().unwrap().id;
        assert!(doc.dependants(top).contains(&on_top));
        // Clamped rather than refused: it stops just below the plane.
        doc.move_to(on_top, 0);
        assert_eq!(doc.index_of(on_top), doc.index_of(top).map(|at| at + 1));
    }
}
