//! The closed regions a sketch's lines enclose, found by arranging them into a
//! planar graph and walking its faces. See `documentation/sketches.md`.
//!
//! Crossing lines are not an error: they split what they cross, so two
//! overlapping squares are three regions and a bow tie is two. Construction
//! lines take no part. A region is named by the lines round its outside and
//! which side of each it is on — never by its index, which is an accident of
//! the arrangement — so moving a point keeps the name. Cutting a named region
//! in two does not lose it: [`Arrangement::resolve`] puts the pieces back
//! together.
//!
//! Pure: no `World`, nothing but 2D in the sketch's own plane.

use std::collections::BTreeSet;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prop::sketch::{Sketch, SketchId};

/// Metres. Points nearer than this are one point, and a point nearer than this
/// to a line is on it. Well below anything drawn on a prop, well above `f32`
/// rounding at its size.
pub(crate) const TOLERANCE: f32 = 1e-6;

/// Which side of a line, looking from its first point to its second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Bound {
    pub entity: SketchId,
    pub side: Side,
}

/// A region's name: every line round its outside and which side the region is
/// on, as a set. Holes are not part of it, so drawing an island inside a
/// region does not rename the region.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegionRef {
    pub bounds: Vec<Bound>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub name: RegionRef,
    /// Counter-clockwise.
    pub outer: Vec<Vec2>,
    /// The outsides of whatever is drawn inside it, in no promised winding.
    pub holes: Vec<Vec<Vec2>>,
}

impl Region {
    /// Square metres, holes taken away.
    pub fn area(&self) -> f32 {
        signed_area(&self.outer) - self.holes.iter().map(|hole| signed_area(hole).abs()).sum::<f32>()
    }

    pub fn contains(&self, point: Vec2) -> bool {
        inside(&self.outer, point) && !self.holes.iter().any(|hole| inside(hole, point))
    }

    /// A point well inside it: the middle of the widest span across it along
    /// any horizontal between two corners' heights. Not the corners' average,
    /// which for an L or a U lies outside.
    pub fn inside_point(&self) -> Vec2 {
        let loops: Vec<&Vec<Vec2>> = std::iter::once(&self.outer).chain(&self.holes).collect();
        let mut heights: Vec<f32> = loops.iter().flat_map(|l| l.iter().map(|p| p.y)).collect();
        heights.sort_by(f32::total_cmp);
        heights.dedup_by(|a, b| (*a - *b).abs() < TOLERANCE);
        let mut best: Option<(f32, Vec2)> = None;
        for pair in heights.windows(2) {
            let y = (pair[0] + pair[1]) / 2.0;
            let mut xs: Vec<f32> = Vec::new();
            for l in &loops {
                for (i, p) in l.iter().enumerate() {
                    let q = l[(i + 1) % l.len()];
                    if (p.y > y) != (q.y > y) {
                        xs.push(p.x + (y - p.y) * (q.x - p.x) / (q.y - p.y));
                    }
                }
            }
            xs.sort_by(f32::total_cmp);
            // Even-odd: every other gap is inside, holes included.
            for span in xs.chunks_exact(2) {
                let width = span[1] - span[0];
                if best.is_none_or(|(widest, _)| width > widest) {
                    best = Some((width, Vec2::new((span[0] + span[1]) / 2.0, y)));
                }
            }
        }
        best.map_or_else(|| centroid(&self.outer), |(_, at)| at)
    }
}

/// Something worth saying about a sketch that is not wrong enough to refuse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// A line that bounds nothing: part of an open chain, or a spur.
    Open(SketchId),
    /// A line whose two ends are one point.
    ZeroLength(SketchId),
    /// Two points in the same place, treated as one.
    Coincident(SketchId, SketchId),
    /// A line naming a point that is not there.
    MissingPoint(SketchId),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arrangement {
    pub regions: Vec<Region>,
    pub notes: Vec<Note>,
    graph: Graph,
}

/// The planar graph the regions were walked from. Half-edge `2e` runs along
/// edge `e`, `2e + 1` against it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Graph {
    vertices: Vec<Vec2>,
    edges: Vec<Edge>,
    /// The half-edge after each, walking its face with the face on the left.
    next: Vec<usize>,
    /// The region each half-edge bounds; `None` for the outside of a piece.
    face_of: Vec<Option<usize>>,
    /// Each region's own half-edges, round its outside.
    cycle_of: Vec<Vec<usize>>,
}

impl Graph {
    fn start(&self, h: usize) -> Vec2 {
        let edge = self.edges[h / 2];
        self.vertices[if h % 2 == 0 { edge.from } else { edge.to }]
    }

    fn bound(&self, h: usize) -> Bound {
        Bound {
            entity: self.edges[h / 2].entity,
            side: if h % 2 == 0 { Side::Left } else { Side::Right },
        }
    }
}

impl Arrangement {
    /// Exactly the region called `name`, as it was picked.
    pub fn find(&self, name: &RegionRef) -> Option<&Region> {
        self.regions.iter().find(|region| region.name == *name)
    }

    /// The region `name` means now, which [`Self::find`] is when it is still
    /// one face. When lines drawn across it have cut it into pieces, it is
    /// the pieces joined back up — the outline it was named by still encloses
    /// the same ground, so a sweep of it does not change because somebody
    /// drew on it. A piece that touches none of the outline is something drawn
    /// inside, and stays a hole as it would have before the cut.
    ///
    /// `hint` only chooses between candidates that answer to the same name,
    /// nearest first; it is never what a region is found by.
    ///
    /// Failing both, it is the face every one of those lines still bounds on
    /// the same side: the outline was partly erased and rerouted, as drawing a
    /// notch into or out of a shape does.
    ///
    /// `None` when a line it was named by is gone, which is what deleting a
    /// divider between two regions leaves.
    pub fn resolve(&self, name: &RegionRef, hint: Vec2) -> Option<Region> {
        // One the hint is in, else the one whose middle is nearest it.
        let miss = |region: &Region| match region.contains(hint) {
            true => 0.0,
            false => region.inside_point().distance(hint),
        };
        let nearest = |candidates: Vec<Region>| candidates.into_iter().min_by(|a, b| miss(a).total_cmp(&miss(b)));
        let exact: Vec<Region> = self.regions.iter().filter(|region| region.name == *name).cloned().collect();
        if !exact.is_empty() {
            return nearest(exact);
        }

        let named: BTreeSet<SketchId> = name.bounds.iter().map(|bound| bound.entity).collect();
        let touches = |region: &Region| region.name.bounds.iter().any(|bound| named.contains(&bound.entity));
        let graph = &self.graph;
        let mut piece_of: Vec<Option<usize>> = vec![None; self.regions.len()];
        let mut pieces: Vec<Vec<usize>> = Vec::new();
        for seed in 0..self.regions.len() {
            if piece_of[seed].is_some() || !touches(&self.regions[seed]) {
                continue;
            }
            let piece = pieces.len();
            let mut members = vec![seed];
            piece_of[seed] = Some(piece);
            let mut at = 0;
            while at < members.len() {
                let face = members[at];
                at += 1;
                for &h in &graph.cycle_of[face] {
                    let Some(other) = graph.face_of[h ^ 1] else { continue };
                    let divider = !named.contains(&graph.edges[h / 2].entity);
                    if divider && piece_of[other].is_none() && touches(&self.regions[other]) {
                        piece_of[other] = Some(piece);
                        members.push(other);
                    }
                }
            }
            pieces.push(members);
        }

        let joined: Vec<Region> = pieces
            .iter()
            .filter(|members| members.len() > 1)
            .filter_map(|members| self.join(members, &piece_of))
            .filter(|region| region.name == *name)
            .collect();
        if !joined.is_empty() {
            return nearest(joined);
        }

        // An outline partly erased and rerouted: every line it was named by
        // still bounds one face, on the same side, with more besides.
        let rerouted: Vec<Region> = self
            .regions
            .iter()
            .filter(|region| name.bounds.iter().all(|bound| region.name.bounds.contains(bound)))
            .cloned()
            .collect();
        nearest(rerouted)
    }

    /// One region covering `members`, which are connected faces: its outline
    /// is every half-edge of theirs whose other side is not one of them.
    fn join(&self, members: &[usize], piece_of: &[Option<usize>]) -> Option<Region> {
        let graph = &self.graph;
        let piece = piece_of[members[0]];
        let inside = |h: usize| graph.face_of[h].is_some_and(|face| piece_of[face] == piece);
        let edge_of_union = |h: usize| inside(h) && !inside(h ^ 1);

        let mut seen = vec![false; graph.next.len()];
        let mut outer: Option<(Vec<Vec2>, Vec<Bound>)> = None;
        let mut holes: Vec<Vec<Vec2>> = members.iter().flat_map(|&m| self.regions[m].holes.clone()).collect();
        for start in (0..graph.next.len()).filter(|&h| edge_of_union(h)) {
            if seen[start] {
                continue;
            }
            // A corner where a divider met one line of the outline was not a
            // corner of the region before the cut, and keeping it would
            // renumber everything named off the swept outline.
            let mut cycle: Vec<usize> = Vec::new();
            let mut divider_corners: Vec<usize> = Vec::new();
            let mut h = start;
            loop {
                seen[h] = true;
                cycle.push(h);
                // Round the far vertex until the walk is back on the outline.
                let mut n = graph.next[h];
                let mut crossed = false;
                while !edge_of_union(n) {
                    crossed = true;
                    n = graph.next[n ^ 1];
                }
                if crossed && graph.bound(n) == graph.bound(h) {
                    divider_corners.push(n);
                }
                if seen[n] {
                    break;
                }
                h = n;
            }
            let points: Vec<Vec2> =
                cycle.iter().filter(|h| !divider_corners.contains(h)).map(|&h| graph.start(h)).collect();
            let bounds: Vec<Bound> = cycle.iter().map(|&h| graph.bound(h)).collect();
            if signed_area(&points) > TOLERANCE * TOLERANCE {
                // A connected union has one outside.
                if outer.replace((points, bounds)).is_some() {
                    return None;
                }
            } else {
                holes.push(points);
            }
        }
        let (outer, bounds) = outer?;
        let bounds: BTreeSet<Bound> = bounds.into_iter().collect();
        Some(Region { name: RegionRef { bounds: bounds.into_iter().collect() }, outer, holes })
    }

    /// The region a click at `point` lands in. Faces of one arrangement do
    /// not overlap, so at most one can.
    pub fn region_at(&self, point: Vec2) -> Option<&Region> {
        self.regions.iter().find(|region| region.contains(point))
    }
}

/// Whether a region can be turned about the line through `a` and `b`: it lies
/// wholly on one side, touching allowed. One that straddles the axis would
/// sweep through itself.
pub fn axis_clear(region: &Region, a: Vec2, b: Vec2) -> bool {
    let along = b - a;
    if along.length() < TOLERANCE {
        return false;
    }
    let along = along.normalize();
    let sides = region.outer.iter().map(|p| along.perp_dot(*p - a));
    let (mut left, mut right) = (false, false);
    for side in sides {
        left |= side > TOLERANCE;
        right |= side < -TOLERANCE;
    }
    !(left && right)
}

/// Every closed region `sketch`'s lines enclose.
pub fn arrange(sketch: &Sketch) -> Arrangement {
    let mut notes = Vec::new();

    // Vertices: sketch points, with any two in one place made one.
    let mut vertices: Vec<Vec2> = Vec::new();
    let mut vertex_of: Vec<(SketchId, usize)> = Vec::new();
    for point in &sketch.points {
        let at = Vec2::from_array(point.at);
        match vertices.iter().position(|v| v.distance(at) < TOLERANCE) {
            Some(existing) => {
                let first = vertex_of.iter().find(|(_, v)| *v == existing).map(|(id, _)| *id);
                notes.extend(first.map(|first| Note::Coincident(first, point.id)));
                vertex_of.push((point.id, existing));
            }
            None => {
                vertex_of.push((point.id, vertices.len()));
                vertices.push(at);
            }
        }
    }
    let vertex = |id: SketchId| vertex_of.iter().find(|(point, _)| *point == id).map(|(_, v)| *v);

    // Segments: every line that can bound something.
    let mut segments: Vec<(SketchId, usize, usize)> = Vec::new();
    for (id, a, b, construction) in sketch.lines() {
        if construction {
            continue;
        }
        let (Some(va), Some(vb)) = (vertex(a), vertex(b)) else {
            notes.push(Note::MissingPoint(id));
            continue;
        };
        if va == vb {
            notes.push(Note::ZeroLength(id));
            continue;
        }
        segments.push((id, va, vb));
    }

    // Where each segment is cut: its ends, every vertex lying on it, and
    // every crossing with another.
    let mut cuts: Vec<Vec<(f32, usize)>> = segments.iter().map(|&(_, a, b)| vec![(0.0, a), (1.0, b)]).collect();
    for i in 0..segments.len() {
        for j in i + 1..segments.len() {
            let (_, a, b) = segments[i];
            let (_, c, d) = segments[j];
            let Some((t, u, at)) = crossing(vertices[a], vertices[b], vertices[c], vertices[d]) else { continue };
            let v = vertex_near(&mut vertices, at);
            cuts[i].push((t, v));
            cuts[j].push((u, v));
        }
    }
    // After the crossings, so a vertex a crossing made is found on a third
    // line through the same point.
    for (i, &(_, a, b)) in segments.iter().enumerate() {
        for (v, &p) in vertices.iter().enumerate() {
            if let Some(t) = interior_param(vertices[a], vertices[b], p) {
                cuts[i].push((t, v));
            }
        }
    }

    // Edges between consecutive cuts. One drawn twice — an overlap — is kept
    // once, under the line that came first.
    let mut edges: Vec<Edge> = Vec::new();
    for (i, segment_cuts) in cuts.iter_mut().enumerate() {
        segment_cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
        segment_cuts.dedup_by_key(|cut| cut.1);
        let entity = segments[i].0;
        for pair in segment_cuts.windows(2) {
            let (from, to) = (pair[0].1, pair[1].1);
            let duplicate = edges.iter().any(|e| (e.from == from && e.to == to) || (e.from == to && e.to == from));
            if from != to && !duplicate {
                edges.push(Edge { from, to, entity });
            }
        }
    }

    // Spurs and open chains bound nothing; peel them off until every vertex
    // left closes a loop.
    loop {
        let mut degree = vec![0usize; vertices.len()];
        for edge in &edges {
            degree[edge.from] += 1;
            degree[edge.to] += 1;
        }
        let before = edges.len();
        edges.retain(|edge| degree[edge.from] > 1 && degree[edge.to] > 1);
        if edges.len() == before {
            break;
        }
    }
    for &(entity, _, _) in &segments {
        if !edges.iter().any(|edge| edge.entity == entity) {
            notes.push(Note::Open(entity));
        }
    }

    let (regions, graph) = faces(vertices, edges);
    Arrangement { regions, notes, graph }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Edge {
    from: usize,
    to: usize,
    entity: SketchId,
}

/// Where two segments properly cross: the parameter along each and the
/// point. Ends touching, and parallel overlaps, are left to
/// [`interior_param`], which handles them by vertex.
pub(crate) fn crossing(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<(f32, f32, Vec2)> {
    let (r, s) = (b - a, d - c);
    let denominator = r.perp_dot(s);
    if denominator.abs() < f32::EPSILON * r.length() * s.length() {
        return None;
    }
    let t = (c - a).perp_dot(s) / denominator;
    let u = (c - a).perp_dot(r) / denominator;
    // Strictly inside both, by more than the tolerance in metres.
    let clear = |param: f32, length: f32| param * length > TOLERANCE && (1.0 - param) * length > TOLERANCE;
    (clear(t, r.length()) && clear(u, s.length())).then(|| (t, u, a + r * t))
}

/// How far along `a`–`b` the point `p` is, if it lies on its inside.
pub(crate) fn interior_param(a: Vec2, b: Vec2, p: Vec2) -> Option<f32> {
    let along = b - a;
    let length = along.length();
    let t = (p - a).dot(along) / (length * length);
    let off = (a + along * t).distance(p);
    (off < TOLERANCE && t * length > TOLERANCE && (1.0 - t) * length > TOLERANCE).then_some(t)
}

fn vertex_near(vertices: &mut Vec<Vec2>, at: Vec2) -> usize {
    match vertices.iter().position(|v| v.distance(at) < TOLERANCE) {
        Some(found) => found,
        None => {
            vertices.push(at);
            vertices.len() - 1
        }
    }
}

/// The bounded faces of a planar graph with no spurs.
///
/// Every edge is two half-edges, one each way. Leaving each vertex they are
/// sorted by angle, and a walk arriving along `u → v` leaves along the
/// half-edge just clockwise of `v → u`. That keeps the face on the walk's left,
/// so a bounded face comes out counter-clockwise (positive area) and the
/// outside of each connected piece clockwise. Each clockwise outside is a hole
/// in the smallest face of another piece that contains it.
fn faces(vertices: Vec<Vec2>, edges: Vec<Edge>) -> (Vec<Region>, Graph) {
    // Half-edge `2e` runs along edge `e`, `2e + 1` against it.
    let half = |h: usize| {
        let edge = edges[h / 2];
        match h % 2 {
            0 => (edge.from, edge.to),
            _ => (edge.to, edge.from),
        }
    };
    let mut leaving: Vec<Vec<usize>> = vec![vec![]; vertices.len()];
    for h in 0..edges.len() * 2 {
        leaving[half(h).0].push(h);
    }
    for (v, out) in leaving.iter_mut().enumerate() {
        let angle = |h: &usize| {
            let d = vertices[half(*h).1] - vertices[v];
            d.y.atan2(d.x)
        };
        out.sort_by(|a, b| angle(a).total_cmp(&angle(b)));
    }
    let next = |h: usize| {
        let (_, v) = half(h);
        let out = &leaving[v];
        let twin = h ^ 1;
        let at = out.iter().position(|&o| o == twin).expect("a half-edge leaves where its twin arrives");
        out[(at + out.len() - 1) % out.len()]
    };

    let mut seen = vec![false; edges.len() * 2];
    let mut cycles: Vec<Vec<usize>> = Vec::new();
    for start in 0..edges.len() * 2 {
        if seen[start] {
            continue;
        }
        let mut cycle = vec![];
        let mut h = start;
        while !seen[h] {
            seen[h] = true;
            cycle.push(h);
            h = next(h);
        }
        cycles.push(cycle);
    }

    // Which connected piece each vertex is in, so a piece's own outside is
    // never taken for a hole in one of its own faces.
    let mut piece: Vec<usize> = (0..vertices.len()).collect();
    fn root(piece: &mut [usize], v: usize) -> usize {
        let mut r = v;
        while piece[r] != r {
            r = piece[r];
        }
        piece[v] = r;
        r
    }
    for edge in &edges {
        let (a, b) = (root(&mut piece, edge.from), root(&mut piece, edge.to));
        piece[a] = b;
    }

    let points_of = |cycle: &[usize]| cycle.iter().map(|&h| vertices[half(h).0]).collect::<Vec<Vec2>>();
    let mut bounded: Vec<(Region, usize)> = Vec::new();
    let mut outsides: Vec<(Vec<Vec2>, usize)> = Vec::new();
    let mut face_of: Vec<Option<usize>> = vec![None; edges.len() * 2];
    let mut cycle_of: Vec<Vec<usize>> = Vec::new();
    for cycle in &cycles {
        let points = points_of(cycle);
        let owner = root(&mut piece, half(cycle[0]).0);
        if signed_area(&points) > TOLERANCE * TOLERANCE {
            for &h in cycle {
                face_of[h] = Some(bounded.len());
            }
            cycle_of.push(cycle.clone());
            let bounds: BTreeSet<Bound> = cycle
                .iter()
                .map(|&h| Bound {
                    entity: edges[h / 2].entity,
                    side: if h % 2 == 0 { Side::Left } else { Side::Right },
                })
                .collect();
            let name = RegionRef { bounds: bounds.into_iter().collect() };
            bounded.push((Region { name, outer: points, holes: vec![] }, owner));
        } else {
            outsides.push((points, owner));
        }
    }

    for (outside, owner) in outsides {
        let probe = outside[0];
        let container = bounded
            .iter()
            .enumerate()
            .filter(|(_, (region, other))| *other != owner && inside(&region.outer, probe))
            .min_by(|(_, a), (_, b)| signed_area(&a.0.outer).total_cmp(&signed_area(&b.0.outer)))
            .map(|(at, _)| at);
        if let Some(at) = container {
            bounded[at].0.holes.push(outside);
        }
    }
    let next = (0..edges.len() * 2).map(next).collect();
    let regions = bounded.into_iter().map(|(region, _)| region).collect();
    (regions, Graph { vertices, edges, next, face_of, cycle_of })
}

pub fn centroid(points: &[Vec2]) -> Vec2 {
    points.iter().copied().sum::<Vec2>() / points.len().max(1) as f32
}

/// The longest way across a loop's bounds.
pub fn reach_of(points: &[Vec2]) -> f32 {
    let (min, max) = points
        .iter()
        .fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(min, max), p| (min.min(*p), max.max(*p)));
    if min.x > max.x { 0.0 } else { (max - min).length() }
}

/// Positive when counter-clockwise.
fn signed_area(points: &[Vec2]) -> f32 {
    (0..points.len()).map(|i| points[i].perp_dot(points[(i + 1) % points.len()])).sum::<f32>() / 2.0
}

/// Even-odd, so it does not lean on the winding.
fn inside(polygon: &[Vec2], point: Vec2) -> bool {
    let mut inside = false;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
        if (a.y > point.y) != (b.y > point.y) && point.x < a.x + (point.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed loop through `corners`, each corner a new point.
    fn polygon(sketch: &mut Sketch, corners: &[[f32; 2]]) -> Vec<SketchId> {
        let points: Vec<SketchId> = corners.iter().map(|c| sketch.add_point(Vec2::from_array(*c))).collect();
        (0..points.len()).map(|i| sketch.add_line(points[i], points[(i + 1) % points.len()])).collect()
    }

    fn square(sketch: &mut Sketch, min: f32, max: f32) -> Vec<SketchId> {
        polygon(sketch, &[[min, min], [max, min], [max, max], [min, max]])
    }

    fn areas(arrangement: &Arrangement) -> Vec<f32> {
        let mut areas: Vec<f32> = arrangement.regions.iter().map(Region::area).collect();
        areas.sort_by(f32::total_cmp);
        areas
    }

    fn assert_areas(arrangement: &Arrangement, expected: &[f32]) {
        let found = areas(arrangement);
        assert_eq!(found.len(), expected.len(), "{found:?}");
        for (found, expected) in found.iter().zip(expected) {
            assert!((found - expected).abs() < 1e-5, "{found} vs {expected}");
        }
    }

    #[test]
    fn a_square_is_one_region() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 1.0);
        let arrangement = arrange(&sketch);
        assert_areas(&arrangement, &[1.0]);
        assert!(arrangement.notes.is_empty(), "{:?}", arrangement.notes);
        assert!(signed_area(&arrangement.regions[0].outer) > 0.0, "counter-clockwise");
    }

    #[test]
    fn a_square_drawn_clockwise_is_the_same_region() {
        let mut sketch = Sketch::default();
        polygon(&mut sketch, &[[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]);
        assert_areas(&arrange(&sketch), &[1.0]);
    }

    #[test]
    fn two_overlapping_squares_are_three_regions() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 2.0);
        square(&mut sketch, 1.0, 3.0);
        assert_areas(&arrange(&sketch), &[1.0, 3.0, 3.0]);
    }

    /// Self-intersection is not an error: a bow tie is two triangles.
    #[test]
    fn a_bow_tie_is_two_regions() {
        let mut sketch = Sketch::default();
        polygon(&mut sketch, &[[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]]);
        assert_areas(&arrange(&sketch), &[0.25, 0.25]);
    }

    #[test]
    fn a_square_inside_a_square_is_a_hole_and_an_island() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 4.0);
        square(&mut sketch, 1.0, 2.0);
        let arrangement = arrange(&sketch);
        assert_areas(&arrangement, &[1.0, 15.0]);
        let outer = arrangement.regions.iter().find(|r| r.area() > 2.0).unwrap();
        assert_eq!(outer.holes.len(), 1);
        assert!(!outer.contains(Vec2::splat(1.5)), "the hole is not the region");
        assert!(outer.contains(Vec2::splat(3.0)));
        assert!(arrangement.region_at(Vec2::splat(1.5)).is_some_and(|r| (r.area() - 1.0).abs() < 1e-5));
    }

    /// A line joining an island to its surround makes them one piece: the
    /// surround is walked round the island and back along the line, and comes
    /// out the same shape as with a hole.
    #[test]
    fn a_bridge_to_an_island_is_still_a_hole() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 4.0);
        square(&mut sketch, 1.0, 2.0);
        let (outer, inner) = (sketch.points[0].id, sketch.points[4].id);
        sketch.add_line(outer, inner);
        let arrangement = arrange(&sketch);
        assert_areas(&arrangement, &[1.0, 15.0]);
        let surround = arrangement.regions.iter().find(|r| r.area() > 2.0).unwrap();
        assert!(!surround.contains(Vec2::splat(1.5)));
        assert!(surround.contains(Vec2::new(3.0, 1.0)));
    }

    #[test]
    fn an_open_chain_is_no_region() {
        let mut sketch = Sketch::default();
        let points: Vec<SketchId> =
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].iter().map(|c| sketch.add_point(Vec2::from_array(*c))).collect();
        let lines: Vec<SketchId> = (0..3).map(|i| sketch.add_line(points[i], points[i + 1])).collect();
        let arrangement = arrange(&sketch);
        assert!(arrangement.regions.is_empty());
        assert_eq!(arrangement.notes, lines.into_iter().map(Note::Open).collect::<Vec<_>>());
    }

    /// Fails if a line ending on another's middle does not cut it: the
    /// halves would be one region with a line through it.
    #[test]
    fn a_line_ending_on_another_splits_what_it_crosses() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 2.0);
        let top = sketch.add_point(Vec2::new(1.0, 2.0));
        let bottom = sketch.add_point(Vec2::new(1.0, 0.0));
        sketch.add_line(top, bottom);
        assert_areas(&arrange(&sketch), &[2.0, 2.0]);
    }

    #[test]
    fn a_spur_or_a_construction_line_splits_nothing() {
        let mut sketch = Sketch::default();
        let sides = square(&mut sketch, 0.0, 2.0);
        let inner = sketch.add_point(Vec2::new(1.0, 1.0));
        let corner = sketch.points[0].id;
        let spur = sketch.add_line(corner, inner);
        let across = sketch.add_line(sketch.points[1].id, sketch.points[3].id);
        if let Some(crate::prop::sketch::SketchEntity::Line { construction, .. }) =
            sketch.entities.iter_mut().find(|e| e.id() == across)
        {
            *construction = true;
        }
        let arrangement = arrange(&sketch);
        assert_areas(&arrangement, &[4.0]);
        assert_eq!(arrangement.notes, vec![Note::Open(spur)]);
        assert!(arrangement.regions[0].name.bounds.iter().all(|b| sides.contains(&b.entity)));
    }

    /// Two points in one place close the loop, and are said to be one.
    #[test]
    fn coincident_points_close_a_loop() {
        let mut sketch = Sketch::default();
        let points: Vec<SketchId> = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]]
            .iter()
            .map(|c| sketch.add_point(Vec2::from_array(*c)))
            .collect();
        for i in 0..4 {
            sketch.add_line(points[i], points[i + 1]);
        }
        let arrangement = arrange(&sketch);
        assert_areas(&arrangement, &[1.0]);
        assert_eq!(arrangement.notes, vec![Note::Coincident(points[0], points[4])]);
    }

    /// Fails if regions are named by position or by index.
    #[test]
    fn a_region_keeps_its_name_when_a_point_moves() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 2.0);
        square(&mut sketch, 1.0, 3.0);
        let before = arrange(&sketch);
        let overlap = before.region_at(Vec2::splat(1.5)).unwrap().name.clone();

        sketch.points[6].at = [3.5, 3.2];
        let after = arrange(&sketch);
        let found = after.find(&overlap).expect("the overlap is still there");
        assert!(found.contains(Vec2::splat(1.5)));
        assert_eq!(after.region_at(Vec2::splat(1.5)).map(|r| &r.name), Some(&overlap));
    }

    #[test]
    fn every_region_has_a_different_name() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 2.0);
        square(&mut sketch, 1.0, 3.0);
        polygon(&mut sketch, &[[0.0, 0.0], [3.0, 3.0], [3.0, 0.0], [0.0, 3.0]]);
        let arrangement = arrange(&sketch);
        let names: BTreeSet<Vec<Bound>> = arrangement.regions.iter().map(|r| r.name.bounds.clone()).collect();
        assert_eq!(names.len(), arrangement.regions.len());
    }

    #[test]
    fn three_lines_through_one_point_meet_at_one_vertex() {
        let mut sketch = Sketch::default();
        square(&mut sketch, -1.0, 1.0);
        for (a, b) in [([-1.0, 0.0], [1.0, 0.0]), ([0.0, -1.0], [0.0, 1.0]), ([-1.0, -1.0], [1.0, 1.0])] {
            let (a, b) = (sketch.add_point(Vec2::from_array(a)), sketch.add_point(Vec2::from_array(b)));
            sketch.add_line(a, b);
        }
        let arrangement = arrange(&sketch);
        assert_eq!(arrangement.regions.len(), 6);
        assert!((areas(&arrangement).iter().sum::<f32>() - 4.0).abs() < 1e-5);
    }

    #[test]
    fn a_revolve_axis_must_not_cross_its_region() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 1.0, 2.0);
        let arrangement = arrange(&sketch);
        let region = &arrangement.regions[0];
        assert!(axis_clear(region, Vec2::ZERO, Vec2::Y));
        assert!(axis_clear(region, Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)), "touching is allowed");
        assert!(!axis_clear(region, Vec2::new(1.5, 0.0), Vec2::new(1.5, 1.0)));
        assert!(!axis_clear(region, Vec2::ZERO, Vec2::ZERO), "an axis needs a direction");
    }

    #[test]
    fn a_line_to_a_missing_point_or_to_itself_is_said() {
        let mut sketch = Sketch::default();
        let a = sketch.add_point(Vec2::ZERO);
        let zero = sketch.add_line(a, a);
        let missing = sketch.add_line(a, SketchId(99));
        let arrangement = arrange(&sketch);
        assert_eq!(arrangement.notes, vec![Note::ZeroLength(zero), Note::MissingPoint(missing)]);
    }

    fn line(sketch: &mut Sketch, a: [f32; 2], b: [f32; 2]) -> SketchId {
        let (a, b) = (sketch.add_point(Vec2::from_array(a)), sketch.add_point(Vec2::from_array(b)));
        sketch.add_line(a, b)
    }

    /// Fails if a line drawn across a region loses it.
    #[test]
    fn a_region_cut_in_two_is_still_found_whole() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 2.0);
        let name = arrange(&sketch).regions[0].name.clone();

        line(&mut sketch, [1.0, 0.0], [1.0, 2.0]);
        let after = arrange(&sketch);
        assert_eq!(after.regions.len(), 2);
        assert!(after.find(&name).is_none());
        let found = after.resolve(&name, Vec2::ONE).expect("the halves join back up");
        assert_eq!(found.name, name);
        assert!((found.area() - 4.0).abs() < 1e-5);
    }

    #[test]
    fn a_cut_running_past_the_outline_still_joins() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 2.0);
        let name = arrange(&sketch).regions[0].name.clone();

        line(&mut sketch, [1.0, -1.0], [1.0, 3.0]);
        line(&mut sketch, [-1.0, 1.0], [3.0, 1.0]);
        let found = arrange(&sketch).resolve(&name, Vec2::ONE).expect("four quarters join back up");
        assert!((found.area() - 4.0).abs() < 1e-5);
    }

    /// Fails if joining the pieces fills in a hole that was drawn inside.
    #[test]
    fn an_island_stays_a_hole_when_its_region_is_cut() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 4.0);
        square(&mut sketch, 0.5, 1.5);
        let name = arrange(&sketch).region_at(Vec2::new(3.0, 3.0)).unwrap().name.clone();

        line(&mut sketch, [3.0, 0.0], [3.0, 4.0]);
        let found = arrange(&sketch).resolve(&name, Vec2::splat(2.0)).unwrap();
        assert!((found.area() - 15.0).abs() < 1e-5, "{}", found.area());
    }

    /// A merge is not guessed at: the line the name was partly made of is gone.
    #[test]
    fn a_region_merged_with_its_neighbour_is_lost() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 2.0);
        let divider = line(&mut sketch, [1.0, 0.0], [1.0, 2.0]);
        let before = arrange(&sketch);
        let left = before.region_at(Vec2::new(0.5, 1.0)).unwrap().name.clone();

        sketch.remove(&[divider]);
        let after = arrange(&sketch);
        assert!(after.resolve(&left, Vec2::new(0.5, 1.0)).is_none());
        assert!(after.region_at(Vec2::new(0.5, 1.0)).is_some(), "the merged region is there to re-pick");
    }

    /// One half of a cut region is its own region, not the whole.
    #[test]
    fn a_half_is_found_as_a_half() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 2.0);
        line(&mut sketch, [1.0, 0.0], [1.0, 2.0]);
        let arrangement = arrange(&sketch);
        let half = arrangement.region_at(Vec2::new(0.5, 1.0)).unwrap();
        let found = arrangement.resolve(&half.name, Vec2::ZERO).unwrap();
        assert!((found.area() - 2.0).abs() < 1e-5);
    }


    /// The shape in `assets/user/not_enclosed_bug.gpp`: a rectangle whose right
    /// side was partly erased and rerouted round a notch. Every line it was
    /// named by still bounds it, on the same side, so it is the same region.
    #[test]
    fn a_rerouted_outline_keeps_its_region() {
        let mut sketch = Sketch::default();
        let rectangle = square(&mut sketch, 0.0, 2.0);
        let name = arrange(&sketch).regions[0].name.clone();

        let right = rectangle[1];
        let (corner, on_right) = (Vec2::new(2.0, 0.0), Vec2::new(2.0, 1.0));
        let corner_point = sketch.points.iter().find(|p| Vec2::from_array(p.at) == corner).unwrap().id;
        let out = sketch.add_point(Vec2::new(3.0, 0.0));
        let up = sketch.add_point(Vec2::new(3.0, 1.0));
        let back = sketch.add_point(on_right);
        sketch.add_line(corner_point, out);
        sketch.add_line(out, up);
        sketch.add_line(up, back);
        assert!(sketch.erase_piece(right, Vec2::new(2.0, 0.5)));

        let after = arrange(&sketch);
        assert_eq!(after.regions.len(), 1);
        let found = after.resolve(&name, Vec2::ONE).expect("the rerouted rectangle is still the region");
        assert!((found.area() - 5.0).abs() < 1e-5, "{}", found.area());
    }


    /// Fails if joining the pieces back up leaves corners where the cut met
    /// the outline, which would renumber what is named off the swept body.
    #[test]
    fn a_rejoined_region_has_the_outline_it_had() {
        let mut sketch = Sketch::default();
        square(&mut sketch, 0.0, 2.0);
        let before = arrange(&sketch).regions[0].clone();

        line(&mut sketch, [1.0, -1.0], [1.0, 3.0]);
        line(&mut sketch, [-1.0, 1.0], [3.0, 1.0]);
        let after = arrange(&sketch).resolve(&before.name, Vec2::ONE).unwrap();
        assert_eq!(after.outer, before.outer);
    }

    /// Fails if a region's inside point is the corners' average, which for a
    /// U is in the gap between its arms.
    #[test]
    fn a_u_has_its_inside_point_inside_it() {
        let mut sketch = Sketch::default();
        polygon(&mut sketch, &[[0.0, 0.0], [3.0, 0.0], [3.0, 3.0], [2.0, 3.0], [2.0, 1.0], [1.0, 1.0], [1.0, 3.0], [0.0, 3.0]]);
        let region = arrange(&sketch).regions[0].clone();
        assert!(!region.contains(centroid(&region.outer)), "the test shape no longer shows the problem");
        assert!(region.contains(region.inside_point()));
    }

}
