//! Faces by tag, and the edges between them: a seam is wherever two named
//! faces meet on a body, found on the finished solid, so an edge a boolean
//! made has a name. See `documentation/prop-modeling.md`, stage 4.

use std::collections::VecDeque;

use bevy::platform::collections::{HashMap, HashSet};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prop::box_grid::BoxGrid;
use crate::prop::feature::PropFeatureId;
use crate::prop::reference::{CopyOf, Element, GeomRef, Located, Pickable, Source};
use crate::prop::solid::{Solid, NO_FACE};

/// The face every polygon tag names, interned per evaluation: a
/// [`GeomRef`] holds a list of copies and is not `Copy`, so a polygon
/// carries an index into this instead.
#[derive(Clone, Debug, Default)]
pub struct FaceTable {
    names: Vec<GeomRef>,
    index: HashMap<GeomRef, u32>,
}

impl FaceTable {
    pub fn intern(&mut self, name: GeomRef) -> u32 {
        if let Some(&tag) = self.index.get(&name) {
            return tag;
        }
        let tag = self.names.len() as u32;
        self.index.insert(name.clone(), tag);
        self.names.push(name);
        tag
    }

    pub fn name(&self, tag: u32) -> Option<&GeomRef> {
        self.names.get(tag as usize)
    }

    pub fn tag(&self, name: &GeomRef) -> Option<u32> {
        self.index.get(name).copied()
    }

    /// Face `face` of what `feature` made itself.
    pub fn of_generator(&mut self, feature: PropFeatureId, face: u32) -> u32 {
        self.intern(GeomRef { element: Element::Face(face), source: Source { feature, copies: vec![] } })
    }

    /// The tag of `tag`'s face as copy `index` made by `by` names it.
    pub fn copied(&mut self, tag: u32, by: PropFeatureId, index: u32) -> u32 {
        let Some(mut name) = self.name(tag).cloned() else { return tag };
        name.source.copies.push(CopyOf { by, index });
        self.intern(name)
    }
}

/// Where two named faces meet: the two faces, sorted so the name does not
/// depend on which polygon was seen first, and which of their meetings when
/// they meet more than once. `index` orders meetings by where their middles
/// are, so two that swap places as the prop is edited swap names; meeting
/// twice is uncommon. Scalars first: TOML needs every scalar before any
/// sub-table.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SeamRef {
    pub index: u32,
    pub faces: [GeomRef; 2],
}

/// Where three or more named faces meet, named by the three that sort
/// first.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JunctionRef {
    pub faces: [GeomRef; 3],
}

impl SeamRef {
    pub fn features(&self) -> Vec<PropFeatureId> {
        self.faces.iter().flat_map(|face| face.source.features()).collect()
    }
}

impl JunctionRef {
    pub fn features(&self) -> Vec<PropFeatureId> {
        self.faces.iter().flat_map(|face| face.source.features()).collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seam {
    pub body: PropFeatureId,
    pub name: SeamRef,
    /// In order along the seam; a closed one does not repeat its first point.
    pub points: Vec<Vec3>,
    pub closed: bool,
    /// Not lying on a generator edge that is offered already.
    pub offered: bool,
}

impl Seam {
    /// Its pieces, end to end, the closing one included.
    pub fn segments(&self) -> impl Iterator<Item = (Vec3, Vec3)> + '_ {
        let n = self.points.len();
        let count = if self.closed { n } else { n.saturating_sub(1) };
        (0..count).map(move |i| (self.points[i], self.points[(i + 1) % n]))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Junction {
    pub body: PropFeatureId,
    pub name: JunctionRef,
    pub at: Vec3,
    pub offered: bool,
}

/// Meters a segment's ends may sit off another's line and still share it.
const ON_LINE: f32 = 1e-5;
/// Meters; a boundary piece shorter than this is a BSP sliver, left out.
const SHORTEST: f32 = 1e-4;
/// Meters two pieces' ends may be apart and still join into one seam.
const JOIN: f32 = 2e-4;
/// The line-key cells: of a unit direction, and of meters for the point on
/// the line nearest the origin.
const DIRECTION_CELL: f32 = 0.02;
const OFFSET_CELL: f32 = 1e-3;
/// A key this close to a cell's side, as a fraction of the cell, is filed
/// under the neighbor too, so rounding that lands two copies of one line on
/// either side of a boundary still finds them together.
const MARGIN: f32 = 0.2;

struct Segment {
    a: Vec3,
    along: Vec3,
    length: f32,
    tag: u32,
}

/// The seams and junctions of one body. Polygons are matched edge to edge
/// by overlap along a shared line, never by shared corners: the BSP leaves
/// a corner of one polygon partway along another's edge.
pub fn find(body: PropFeatureId, solid: &Solid, faces: &FaceTable) -> (Vec<Seam>, Vec<Junction>) {
    let mut segments = Vec::new();
    for polygon in solid.polygons() {
        if polygon.surface.tag == NO_FACE {
            continue;
        }
        let v = &polygon.vertices;
        for i in 0..v.len() {
            let (a, b) = (v[i], v[(i + 1) % v.len()]);
            let length = a.distance(b);
            if length >= SHORTEST {
                segments.push(Segment { a, along: (b - a) / length, length, tag: polygon.surface.tag });
            }
        }
    }

    let mut buckets: HashMap<[i32; 6], Vec<usize>> = HashMap::default();
    let mut found = Vec::new();
    for (i, segment) in segments.iter().enumerate() {
        // Under both senses of the line, so no rule for which way it points
        // can file two copies of one line apart.
        for sense in [1.0, -1.0] {
            cells(&line_key(segment, sense), &mut found);
            for cell in &found {
                buckets.entry(*cell).or_default().push(i);
            }
        }
    }
    // Members of a bucket lie on one line, so only those whose spans along it
    // overlap can share an edge: sorted by where each starts, a member is
    // compared with those starting before it ends. A pattern along an axis
    // puts every copy's edges on the same few lines.
    let mut pairs: HashSet<(usize, usize)> = HashSet::default();
    let mut spans: Vec<(f32, f32, usize)> = Vec::new();
    for members in buckets.values_mut() {
        members.sort_unstable();
        members.dedup();
        let axis = segments[members[0]].along;
        spans.clear();
        spans.extend(members.iter().map(|&i| {
            let segment = &segments[i];
            let (u, w) = (segment.a.dot(axis), (segment.a + segment.along * segment.length).dot(axis));
            (u.min(w), u.max(w), i)
        }));
        spans.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        for (x, &(_, high, i)) in spans.iter().enumerate() {
            for &(low, _, j) in &spans[x + 1..] {
                if low > high + ON_LINE {
                    break;
                }
                if segments[i].tag != segments[j].tag {
                    pairs.insert((i.min(j), i.max(j)));
                }
            }
        }
    }

    let mut pieces: HashMap<(u32, u32), Vec<(Vec3, Vec3)>> = HashMap::default();
    for (i, j) in pairs {
        let (s, t) = (&segments[i], &segments[j]);
        // Faces wound outward walk a shared edge in opposite directions.
        if s.along.dot(t.along) > -(1.0 - 1e-4) {
            continue;
        }
        let off = |p: Vec3| {
            let to = p - s.a;
            (to - s.along * to.dot(s.along)).length()
        };
        let t_end = t.a + t.along * t.length;
        if off(t.a) > ON_LINE || off(t_end) > ON_LINE {
            continue;
        }
        let (u, w) = ((t.a - s.a).dot(s.along), (t_end - s.a).dot(s.along));
        let (low, high) = (u.min(w).max(0.0), u.max(w).min(s.length));
        if high - low > ON_LINE {
            let pair = (s.tag.min(t.tag), s.tag.max(t.tag));
            pieces.entry(pair).or_default().push((s.a + s.along * low, s.a + s.along * high));
        }
    }

    let mut seams = Vec::new();
    let mut ends: Vec<(Vec3, u32, u32)> = Vec::new();
    let mut by_pair: Vec<_> = pieces.into_iter().collect();
    by_pair.sort_by_key(|((a, b), _)| (*a, *b));
    for ((a, b), pieces) in by_pair {
        let (Some(name_a), Some(name_b)) = (faces.name(a), faces.name(b)) else { continue };
        let mut names = [name_a.clone(), name_b.clone()];
        names.sort();
        let mut chains = chain(pieces);
        chains.sort_by_key(|(points, _)| {
            let middle = points.iter().sum::<Vec3>() / points.len() as f32;
            let at = (middle / JOIN).round().as_ivec3();
            (at.x, at.y, at.z)
        });
        for (index, (points, closed)) in chains.into_iter().enumerate() {
            if !closed {
                ends.push((points[0], a, b));
                ends.push((points[points.len() - 1], a, b));
            }
            let name = SeamRef { index: index as u32, faces: names.clone() };
            seams.push(Seam { body, name, points, closed, offered: true });
        }
    }

    let mut junctions = Vec::new();
    let mut taken = vec![false; ends.len()];
    let cell_of = |point: Vec3| (point / JOIN).floor().as_ivec3();
    let mut grid: HashMap<IVec3, Vec<usize>> = HashMap::default();
    for (i, end) in ends.iter().enumerate() {
        grid.entry(cell_of(end.0)).or_default().push(i);
    }
    let mut near = Vec::new();
    for i in 0..ends.len() {
        if taken[i] {
            continue;
        }
        // In order, as a scan of every later end would take them.
        near.clear();
        let cell = cell_of(ends[i].0);
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    if let Some(members) = grid.get(&(cell + IVec3::new(x, y, z))) {
                        near.extend(members.iter().copied().filter(|&j| j >= i));
                    }
                }
            }
        }
        near.sort_unstable();
        let mut tags: Vec<u32> = Vec::new();
        let mut at = Vec3::ZERO;
        let mut count = 0.0;
        for &j in &near {
            if !taken[j] && ends[j].0.distance(ends[i].0) < JOIN {
                taken[j] = true;
                tags.extend([ends[j].1, ends[j].2]);
                at += ends[j].0;
                count += 1.0;
            }
        }
        let mut names: Vec<GeomRef> = tags.iter().filter_map(|&tag| faces.name(tag).cloned()).collect();
        names.sort();
        names.dedup();
        if let [first, second, third, ..] = &names[..] {
            let name = JunctionRef { faces: [first.clone(), second.clone(), third.clone()] };
            junctions.push(Junction { body, name, at: at / count, offered: true });
        }
    }
    (seams, junctions)
}

/// Marks which seams and junctions are offered, and returns the generator
/// edges a seam is offered in place of. A seam lying on one offered
/// generator edge is that edge, which keeps the name it had first; one lying
/// along several — a revolved corner's circle, two collinear edges of a
/// union — is offered as one curve in their place. A junction on an offered
/// corner is that corner.
pub fn offer(seams: &mut [Seam], junctions: &mut [Junction], pickables: &[Pickable]) -> Vec<GeomRef> {
    let reach = Vec3::splat(JOIN);
    let mut edges_of: HashMap<PropFeatureId, Vec<(&GeomRef, Vec3, Vec3)>> = HashMap::default();
    let mut corners_of: HashMap<PropFeatureId, Vec<Vec3>> = HashMap::default();
    for pickable in pickables {
        match pickable.at {
            Located::Edge(a, b) => edges_of.entry(pickable.body).or_default().push((&pickable.reference, a, b)),
            Located::Point(corner) => corners_of.entry(pickable.body).or_default().push(corner),
            _ => {}
        }
    }
    let grids: HashMap<PropFeatureId, BoxGrid> = edges_of
        .iter()
        .map(|(body, edges)| (*body, BoxGrid::new(&edges.iter().map(|(_, a, b)| (a.min(*b) - reach, a.max(*b) + reach)).collect::<Vec<_>>())))
        .collect();
    let mut replaced = Vec::new();
    let mut edges: Vec<(&GeomRef, Vec3, Vec3)> = Vec::new();
    for seam in seams.iter_mut() {
        // Only an edge whose box meets the seam's can cover or lie along it.
        edges.clear();
        if let (Some(all), Some(grid)) = (edges_of.get(&seam.body), grids.get(&seam.body)) {
            let (low, high) = seam.points.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
            edges.extend(grid.near_box(low - reach, high + reach).into_iter().map(|i| all[i]));
        }
        let covered = |point: Vec3| edges.iter().any(|(_, a, b)| distance_to_segment(point, *a, *b) < JOIN);
        let on_edges = seam.segments().all(|(a, b)| covered(a.lerp(b, 0.5)) && covered(a) && covered(b));
        let near_seam = |point: Vec3| seam.segments().any(|(a, b)| distance_to_segment(point, a, b) < JOIN);
        let along: Vec<&GeomRef> = edges
            .iter()
            .filter(|(_, a, b)| near_seam(*a) && near_seam(*b) && near_seam(a.lerp(*b, 0.5)))
            .map(|(reference, ..)| *reference)
            .collect();
        seam.offered = !on_edges || along.len() > 1;
        if on_edges && along.len() > 1 {
            replaced.extend(along.into_iter().cloned());
        }
    }
    for junction in junctions.iter_mut() {
        junction.offered = !corners_of
            .get(&junction.body)
            .is_some_and(|corners| corners.iter().any(|corner| corner.distance(junction.at) < JOIN));
    }
    replaced
}

fn distance_to_segment(point: Vec3, a: Vec3, b: Vec3) -> f32 {
    let along = b - a;
    let t = ((point - a).dot(along) / along.length_squared().max(f32::EPSILON)).clamp(0.0, 1.0);
    point.distance(a + along * t)
}

/// The direction, either way along, then the point on the line nearest the
/// origin, each in its cells.
fn line_key(segment: &Segment, sense: f32) -> [f32; 6] {
    let along = segment.along * sense;
    let nearest = segment.a - along * segment.a.dot(along);
    // Half a cell over, so an axis's components and round offsets — what a
    // prop is mostly made of — sit in the middle of a cell, not on its side.
    let d = along / DIRECTION_CELL + 0.5;
    let o = nearest / OFFSET_CELL + 0.5;
    [d.x, d.y, d.z, o.x, o.y, o.z]
}

/// The key's own cell, and each neighbor it lies within the margin of.
fn cells(key: &[f32; 6], into: &mut Vec<[i32; 6]>) {
    into.clear();
    let mut options = [[0i32; 2]; 6];
    let mut counts = [1usize; 6];
    for (dim, value) in key.iter().enumerate() {
        let floor = value.floor();
        let fraction = value - floor;
        options[dim][0] = floor as i32;
        // MARGIN is under a half, so a key is near one side at most.
        if fraction < MARGIN {
            options[dim][1] = floor as i32 - 1;
            counts[dim] = 2;
        } else if fraction > 1.0 - MARGIN {
            options[dim][1] = floor as i32 + 1;
            counts[dim] = 2;
        }
    }
    let total: usize = counts.iter().product();
    for mut n in 0..total {
        let mut cell = [0i32; 6];
        for dim in 0..6 {
            cell[dim] = options[dim][n % counts[dim]];
            n /= counts[dim];
        }
        into.push(cell);
    }
}

/// Pieces joined end to end into runs of points, each with whether it
/// closes, corners where a run only carries straight on taken out.
fn chain(mut pieces: Vec<(Vec3, Vec3)>) -> Vec<(Vec<Vec3>, bool)> {
    let mut chains = Vec::new();
    while let Some((a, b)) = pieces.pop() {
        let mut points: VecDeque<Vec3> = VecDeque::from([a, b]);
        loop {
            let (front, back) = (points[0], points[points.len() - 1]);
            let found = pieces.iter().position(|(p, q)| {
                p.distance(back) < JOIN || q.distance(back) < JOIN || p.distance(front) < JOIN || q.distance(front) < JOIN
            });
            let Some(at) = found else { break };
            let (p, q) = pieces.swap_remove(at);
            if p.distance(back) < JOIN {
                points.push_back(q);
            } else if q.distance(back) < JOIN {
                points.push_back(p);
            } else if q.distance(front) < JOIN {
                points.push_front(p);
            } else {
                points.push_front(q);
            }
        }
        let mut points: Vec<Vec3> = points.into();
        points.dedup_by(|a, b| a.distance(*b) < JOIN);
        let closed = points.len() > 2 && points[0].distance(points[points.len() - 1]) < JOIN;
        if closed {
            points.pop();
        }
        chains.push((straighten(points, closed), closed));
    }
    chains
}

/// Drops every point the run passes straight through.
fn straighten(points: Vec<Vec3>, closed: bool) -> Vec<Vec3> {
    let n = points.len();
    let keep = |i: usize| {
        if !closed && (i == 0 || i == n - 1) {
            return true;
        }
        let (before, here, after) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
        let (u, v) = (here - before, after - here);
        // About a hundredth of a degree: the BSP's rounding bends a straight
        // run by less, and a twenty-four-step turn by far more.
        u.cross(v).length() > 2e-4 * u.length() * v.length()
    };
    (0..n).filter(|&i| keep(i)).map(|i| points[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::feature::BodyRef;
    use crate::prop::feature::{evaluate, FeatureOp, PropFeature};
    use crate::prop::plane::{OriginPlane, PlaneRef};
    use crate::prop::profile::Placement;
    use crate::prop::refs::{CurveRef, Resolve};
    use crate::prop::skin::{extrusion, revolution, skin, TaggedLoop};
    use crate::prop::solid::Shape;
    use crate::prop::surface::Surface;

    fn feature(id: u32, op: FeatureOp) -> PropFeature {
        PropFeature::new(PropFeatureId(id), op)
    }

    fn cube(id: u32) -> PropFeature {
        let surface = Surface::default();
        feature(id, FeatureOp::Primitive { shape: Shape::Box { size: [0.2; 3] }, placement: Placement::default(), surface })
    }

    /// A box's edges are seams too, and none is offered: its own edges and
    /// corners are, under the box's names.
    #[test]
    fn a_boxs_own_edges_are_seams_offered_under_their_own_names() {
        let evaluated = evaluate(&[cube(0)]);
        assert_eq!(evaluated.seams().len(), 12);
        assert_eq!(evaluated.junctions().len(), 8);
        assert!(evaluated.seams().iter().all(|seam| !seam.offered && seam.points.len() == 2));
        assert!(evaluated.junctions().iter().all(|junction| !junction.offered));
    }

    /// A body bored through by a turned cylinder, both faces named as a
    /// feature would name them.
    fn bored(reach: f32) -> (Solid, FaceTable) {
        let mut faces = FaceTable::default();
        let square = |a: f32, b: f32, c: f32, d: f32| {
            TaggedLoop::numbered(vec![Vec2::new(a, c), Vec2::new(b, c), Vec2::new(b, d), Vec2::new(a, d)])
        };
        let body = skin(&extrusion(vec![square(-0.1, 0.1, -0.1, 0.1)], 0.2, true), Surface::default())
            .unwrap()
            .solid
            .retagged(|face| faces.of_generator(PropFeatureId(0), face));
        // Turned about Y, so it runs through the box's ±Y sides.
        let bore = skin(&revolution(vec![square(0.0, 0.04, -reach, reach)], 360.0, 12), Surface::default())
            .unwrap()
            .solid
            .retagged(|face| faces.of_generator(PropFeatureId(1), face));
        (body.subtract(&bore), faces)
    }

    #[test]
    fn a_bores_rims_are_seams() {
        let (solid, faces) = bored(0.2);
        let (seams, _) = find(PropFeatureId(2), &solid, &faces);
        let rims: Vec<&Seam> = seams.iter().filter(|seam| seam.closed).collect();
        assert_eq!(rims.len(), 2);
        for rim in rims {
            assert_eq!(rim.points.len(), 12, "a twelve-step turn's rim has twelve corners");
            assert!(rim.points.iter().all(|p| (p.y.abs() - 0.1).abs() < 1e-5), "on the box's ±Y sides");
            assert!(rim.name.faces.iter().any(|face| face.source.feature == PropFeatureId(1)));
        }
    }

    /// A turned body's corner circles are each one seam, offered in place of
    /// the per-step edges the sweep names.
    #[test]
    fn a_revolved_corner_is_one_seam_in_place_of_its_steps() {
        let mut faces = FaceTable::default();
        let ring = TaggedLoop::numbered(vec![Vec2::new(0.02, 0.0), Vec2::new(0.06, 0.0), Vec2::new(0.06, 0.1), Vec2::new(0.02, 0.1)]);
        let swept = skin(&revolution(vec![ring], 360.0, 12), Surface::default()).unwrap();
        let solid = swept.solid.retagged(|face| faces.of_generator(PropFeatureId(0), face));
        let wires = [crate::prop::reference::Wire::new(PropFeatureId(0), swept.outline)];
        let pickables = crate::prop::reference::pickables(PropFeatureId(0), &solid, &wires);
        let (mut seams, mut junctions) = find(PropFeatureId(0), &solid, &faces);
        let replaced = offer(&mut seams, &mut junctions, &pickables);
        assert_eq!(seams.len(), 4, "one circle per corner of the ring");
        assert!(seams.iter().all(|seam| seam.offered && seam.closed && seam.points.len() == 12));
        assert_eq!(replaced.len(), 4 * 12);
    }

    #[test]
    fn a_rim_keeps_its_name_when_the_bore_gets_longer() {
        let names = |reach| {
            let (solid, faces) = bored(reach);
            let mut names: Vec<SeamRef> =
                find(PropFeatureId(2), &solid, &faces).0.into_iter().filter(|seam| seam.closed).map(|seam| seam.name).collect();
            names.sort();
            names
        };
        assert_eq!(names(0.2), names(0.5));
    }

    /// A mirror's seams are the original's, carried across and named
    /// through the copy.
    #[test]
    fn a_mirrored_seam_is_the_reflection_of_the_original() {
        let surface = Surface::default();
        let rod = |id| {
            let shape = Shape::Prism { sides: 6, radius: 0.04, height: 0.4 };
            feature(id, FeatureOp::Primitive { shape, placement: Placement::at(0.05, 0.0, 0.0), surface })
        };
        let bored = feature(2, FeatureOp::Subtract { targets: vec![BodyRef::whole(PropFeatureId(0))], tools: vec![BodyRef::whole(PropFeatureId(1))] });
        let about = PlaneRef::Origin(OriginPlane::Yz);
        let mirror = feature(3, FeatureOp::Mirror { targets: vec![BodyRef::whole(PropFeatureId(2))], offset: 0.0, keep_original: false, about });
        let original = evaluate(&[cube(0), rod(1), bored.clone()]);
        let mirrored = evaluate(&[cube(0), rod(1), bored, mirror]);
        let offered: Vec<&Seam> = original.seams().iter().filter(|seam| seam.offered).collect();
        assert!(!offered.is_empty(), "the bore's rim pieces are new edges");
        let scope = mirrored.scope();
        for seam in offered {
            let mut faces = seam.name.faces.clone().map(|mut face| {
                face.source.copies.push(CopyOf { by: PropFeatureId(3), index: 1 });
                face
            });
            faces.sort();
            let name = SeamRef { index: seam.name.index, faces };
            let Some(curve) = (CurveRef::Seam { seam: name.clone() }).resolve(&scope) else { panic!("{name:?} is missing") };
            let reflect = |p: Vec3| Vec3::new(-p.x, p.y, p.z);
            let (start, end) = (reflect(seam.points[0]), reflect(seam.points[seam.points.len() - 1]));
            let ends = [curve.start(), curve.end()];
            assert!(ends.iter().any(|p| p.distance(start) < 1e-4) && ends.iter().any(|p| p.distance(end) < 1e-4));
        }
    }

    /// Every seam on every shipped prop has a name no other seam has, and
    /// resolves by it.
    #[test]
    fn every_shipped_seam_resolves_by_its_name() {
        let Ok(entries) = std::fs::read_dir("assets/default/props") else { return };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|e| e.to_str()) != Some("gpp") {
                continue;
            }
            let doc = crate::prop::document::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let evaluated = doc.evaluate();
            let mut names: Vec<&SeamRef> = evaluated.seams().iter().map(|seam| &seam.name).collect();
            names.sort();
            let count = names.len();
            names.dedup();
            assert_eq!(names.len(), count, "{}: two seams share a name", path.display());
            let scope = evaluated.scope();
            for seam in evaluated.seams() {
                assert!(seam.points.len() >= 2, "{}: {:?}", path.display(), seam.name);
                assert!(scope.seam(&seam.name).is_some());
            }
        }
    }
}
