//! The one sweep: loops placed at a run of frames and skinned between them.
//! Extrude, Revolve and every primitive are ways of producing [`Section`]s;
//! this builds the solid and names what it made, so every sweep names its
//! corners, edges and faces by one rule. See `documentation/prop-modeling.md`.
//!
//! Naming, in [`Outline`] order:
//!
//! - **Points**: section by section, each loop's points as authored.
//! - **Edges**: round each loop at every section where a face ends — the
//!   first and last of an open run, and wherever the path's tag changes —
//!   then along each span from every corner where the loop's tag changes.
//!   An edge between two facets of one face is no edge, so is not listed.
//! - **Faces**: the start and end caps of an open run, then one face per run
//!   of spans sharing a path tag, per loop, per segment tag, holding every
//!   facet that tag swept. A revolved segment is one face, not a ring of them.

use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::common::csg;
use crate::prop::profile::triangulate;
use crate::prop::reference::{newell, Outline, OutlineFace, OutlineFacet};
use crate::prop::solid::{Face, Polygon, Solid, NO_FACE};
use crate::prop::surface::Surface;

/// A closed loop in a section's plane, each segment tagged with what made it:
/// segment `i` runs from point `i` to point `i + 1`. Either winding.
#[derive(Clone, Debug, PartialEq)]
pub struct TaggedLoop {
    pub points: Vec<Vec2>,
    pub tags: Vec<u32>,
}

impl TaggedLoop {
    /// Each segment its own tag, as every loop has until sketches have curves.
    pub fn numbered(points: Vec<Vec2>) -> TaggedLoop {
        let tags = (0..points.len() as u32).collect();
        TaggedLoop { points, tags }
    }

    fn ccw(&self) -> bool {
        area_x2(&self.points) >= 0.0
    }

    fn wound_ccw(&self) -> Vec<Vec2> {
        let mut points = self.points.clone();
        if !self.ccw() {
            points.reverse();
        }
        points
    }
}

fn area_x2(points: &[Vec2]) -> f32 {
    (0..points.len()).map(|i| points[i].perp_dot(points[(i + 1) % points.len()])).sum()
}

/// The loops at one place along the sweep: the first is the outside, the
/// rest holes. `frame` takes the loop's plane (`z = 0`) into the prop.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub loops: Vec<TaggedLoop>,
    pub frame: Affine3A,
}

/// What a run of sections is skinned as.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub sections: Vec<Section>,
    /// A tag per span, naming the piece of path it lies on; one per section
    /// when `closed`, one fewer otherwise.
    pub spans: Vec<u32>,
    /// The last section joins the first, and there are no caps.
    pub closed: bool,
}

#[derive(Debug)]
pub struct Swept {
    pub solid: Solid,
    pub outline: Outline,
    /// Per side face, in outline order after the caps: the span tag and the
    /// segment tag it was swept from.
    pub sides: Vec<(u32, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkinProblem {
    /// Sections whose loops differ in number or in point count.
    Mismatch,
    /// A frame mirrored against the one before it.
    Flips,
    /// A point travels backward through its section's plane while others go
    /// forward, so the skin passes through itself.
    Folds,
}

impl SkinProblem {
    pub fn message(self) -> String {
        match self {
            SkinProblem::Mismatch => crate::get!("prop.problems.skin_mismatch"),
            SkinProblem::Flips => crate::get!("prop.problems.skin_flips"),
            SkinProblem::Folds => crate::get!("prop.problems.skin_folds"),
        }
    }
}

/// How far past its caps a hole's cutter runs, as a fraction of the step
/// beside it, so no cap of the cutter is coplanar with one of the body's —
/// coplanar faces are where a BSP boolean is least sure which side it is on.
const CUTTER_OVERRUN: f32 = 0.01;
/// Meters the cutter overruns at least, however short the step.
const CUTTER_MIN_OVERRUN: f32 = 1e-4;
/// Meters a point may move between sections and still be standing still.
const STILL: f32 = 1e-6;

/// A loop with fewer than three points sweeps nothing and is no problem: the
/// solid comes back empty.
pub fn skin(run: &Run, surface: Surface) -> Result<Swept, SkinProblem> {
    check(run)?;
    let Some(first) = run.sections.first() else { return Ok(empty()) };
    if first.loops.first().is_none_or(|outer| outer.points.len() < 3) {
        return Ok(empty());
    }
    let frames: Vec<Affine3A> = run.sections.iter().map(|section| section.frame).collect();
    let loops = |loop_index: usize| -> Vec<Vec<Vec2>> {
        run.sections.iter().map(|section| section.loops[loop_index].wound_ccw()).collect()
    };
    let rings = |loops: &[Vec<Vec2>], frames: &[Affine3A]| -> Vec<Vec<Vec3>> {
        loops.iter().zip(frames).map(|(points, frame)| place(points, frame)).collect()
    };

    let numbers = FaceNumbers::of(run);
    let span_count = spans(run);
    let outer = loops(0);
    let side = |k: usize, i: usize| numbers.side(k, 0, wound_to_authored(&first.loops[0], i));
    let raw = shell(&rings(&outer, &frames), &outer, &frames, run.closed, surface, &side, (numbers.start, numbers.end));
    let outward = csg::signed_volume_x6(&raw) >= 0.0;
    let mut solid = Solid::from_polygons(raw);
    for hole in 1..first.loops.len() {
        let mut hole_loops = loops(hole);
        let cutter_frames = match run.closed {
            true => frames.clone(),
            false => {
                // The first and last loops again, at the frames past each end.
                let (head, tail) = (hole_loops[0].clone(), hole_loops[hole_loops.len() - 1].clone());
                hole_loops.insert(0, head);
                hole_loops.push(tail);
                overrun(&frames, &first.loops[hole])
            }
        };
        // An open cutter has a span more at each end, beyond the body.
        let shift = usize::from(!run.closed);
        let side = |k: usize, i: usize| {
            let k = k.saturating_sub(shift).min(span_count - 1);
            numbers.side(k, hole, wound_to_authored(&first.loops[hole], i))
        };
        let ends = (NO_FACE, NO_FACE);
        let cutter = shell(&rings(&hole_loops, &cutter_frames), &hole_loops, &cutter_frames, run.closed, surface, &side, ends);
        solid = solid.subtract(&Solid::from_polygons(cutter));
    }
    let (outline, sides) = outline(run, outward);
    Ok(Swept { solid, outline, sides })
}

/// Which outline face each polygon is part of: the numbering
/// [`outline`] lists faces in.
struct FaceNumbers {
    start: u32,
    end: u32,
    /// The path tag group of each span.
    group_of_span: Vec<usize>,
    /// Per group, per loop, per segment as authored.
    sides: Vec<Vec<Vec<u32>>>,
}

impl FaceNumbers {
    fn of(run: &Run) -> FaceNumbers {
        let loops = &run.sections[0].loops;
        let mut next = 0u32;
        let (start, end) = match run.closed {
            true => (NO_FACE, NO_FACE),
            false => {
                next = 2;
                (0, 1)
            }
        };
        let mut groups: Vec<u32> = Vec::new();
        let group_of_span = run
            .spans
            .iter()
            .take(spans(run))
            .map(|tag| match groups.iter().position(|t| t == tag) {
                Some(at) => at,
                None => {
                    groups.push(*tag);
                    groups.len() - 1
                }
            })
            .collect();
        let sides = groups
            .iter()
            .map(|_| {
                loops
                    .iter()
                    .map(|tagged| {
                        let mut seen: Vec<(u32, u32)> = Vec::new();
                        tagged
                            .tags
                            .iter()
                            .map(|tag| match seen.iter().find(|(t, _)| t == tag) {
                                Some((_, face)) => *face,
                                None => {
                                    seen.push((*tag, next));
                                    next += 1;
                                    next - 1
                                }
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect();
        FaceNumbers { start, end, group_of_span, sides }
    }

    fn side(&self, span: usize, loop_index: usize, segment: usize) -> u32 {
        self.sides[self.group_of_span[span]][loop_index][segment]
    }
}

/// The authored segment that segment `i` of the loop wound counter-clockwise
/// is: itself, or, for a loop authored clockwise and so reversed, the one
/// running the other way between the same two points.
fn wound_to_authored(tagged: &TaggedLoop, i: usize) -> usize {
    let n = tagged.points.len();
    match tagged.ccw() {
        true => i,
        false => (2 * n - 2 - i) % n,
    }
}

fn empty() -> Swept {
    Swept { solid: Solid::default(), outline: Outline::default(), sides: vec![] }
}

fn place(points: &[Vec2], frame: &Affine3A) -> Vec<Vec3> {
    points.iter().map(|p| frame.transform_point3(Vec3::new(p.x, p.y, 0.0))).collect()
}

fn check(run: &Run) -> Result<(), SkinProblem> {
    let Some(first) = run.sections.first() else { return Ok(()) };
    let counts = |section: &Section| section.loops.iter().map(|l| l.points.len()).collect::<Vec<_>>();
    if run.sections.iter().any(|section| counts(section) != counts(first)) {
        return Err(SkinProblem::Mismatch);
    }
    let sign = |frame: &Affine3A| frame.matrix3.determinant().signum();
    if run.sections.windows(2).any(|pair| sign(&pair[0].frame) != sign(&pair[1].frame)) {
        return Err(SkinProblem::Flips);
    }
    // Each point's travel to the next section, against that section's
    // normal: all one sign, or the skin turns back through itself.
    let Some(outer) = first.loops.first() else { return Ok(()) };
    let mut travel = 0.0f32;
    for k in 0..spans(run) {
        let (here, next) = (&run.sections[k], &run.sections[(k + 1) % run.sections.len()]);
        let normal = here.frame.matrix3.z_axis;
        let starts = place(&here.loops[0].points, &here.frame);
        let ends = place(&next.loops[0].points, &next.frame);
        for (a, b) in starts.iter().zip(&ends).take(outer.points.len()) {
            // A point on a revolve's axis stays put, give or take rounding
            // that would otherwise read as a direction.
            if a.distance(*b) < STILL {
                continue;
            }
            let along = (*b - *a).dot(normal.into());
            if travel != 0.0 && along.signum() != travel.signum() {
                return Err(SkinProblem::Folds);
            }
            travel = along;
        }
    }
    Ok(())
}

fn spans(run: &Run) -> usize {
    match run.closed {
        true => run.sections.len(),
        false => run.sections.len().saturating_sub(1),
    }
}

/// A step that only moves the loop, so the quad between two sections is a
/// parallelogram and flat by construction.
fn translates(a: &Affine3A, b: &Affine3A) -> bool {
    a.matrix3 == b.matrix3
}

/// The polygons, wound consistently and not yet oriented.
///
/// The order is the one the two sweeps this replaced used, caps first when
/// every step translates and sides first otherwise, because the BSP picks its
/// splitting planes in polygon order and a prop should build to the same
/// triangles it always did.
/// `side` is the face a span's wound segment sweeps, and `(start, end)` the
/// caps'.
fn shell(
    rings: &[Vec<Vec3>],
    caps: &[Vec<Vec2>],
    frames: &[Affine3A],
    closed: bool,
    surface: Surface,
    side: &dyn Fn(usize, usize) -> u32,
    (start_face, end_face): (u32, u32),
) -> Vec<Polygon> {
    let made = |tag: u32| Face { surface, tag };
    let s = rings.len();
    let n = rings[0].len();
    let spans = if closed { s } else { s - 1 };
    // A quad is flat only between two copies of one loop moved apart; a
    // loft's loops differ.
    let one_loop = caps.windows(2).all(|pair| pair[0] == pair[1]);
    let parallel = |k: usize, next: usize| one_loop && translates(&frames[k], &frames[next]);
    let mut sides = Vec::new();
    for k in 0..spans {
        // The last ring of a closed run equals the first only to within
        // rounding; reusing ring 0 closes the seam.
        let (next, head) = ((k + 1) % s, &rings[k]);
        let tail = &rings[next];
        for i in 0..n {
            let j = (i + 1) % n;
            let face = made(side(k, i));
            if parallel(k, next) {
                sides.extend(Polygon::new(vec![head[i], head[j], tail[j], tail[i]], face));
            } else {
                // A quad with a corner on a revolve's axis has two corners in
                // one place, so triangles.
                sides.extend(Polygon::new(vec![head[i], head[j], tail[j]], face));
                sides.extend(Polygon::new(vec![head[i], tail[j], tail[i]], face));
            }
        }
    }
    let mut ends = Vec::new();
    if !closed {
        // Each cap is wound against the sides it shares edges with. The
        // volume cannot see a cap the wrong way round when it lies in a plane
        // through a revolve's axis, but a boolean can.
        let (first, last) = (&rings[0], &rings[s - 1]);
        let start = triangulate(&caps[0]);
        let end = triangulate(&caps[s - 1]);
        for (&[a, b, c], &[d, e, f]) in start.iter().zip(&end) {
            ends.extend(Polygon::new(vec![last[d], last[e], last[f]], made(end_face)));
            ends.extend(Polygon::new(vec![first[c], first[b], first[a]], made(start_face)));
        }
    }
    let all_translate = (0..spans).all(|k| parallel(k, (k + 1) % s));
    match all_translate {
        true => [ends, sides].concat(),
        false => {
            // Sides, then the start and end caps triangle by triangle.
            let swapped = ends.chunks(2).flat_map(|pair| pair.iter().rev().cloned());
            sides.into_iter().chain(swapped).collect()
        }
    }
}

/// `frames` with one more at each end, past the first and last, for a hole's
/// cutter: a short way past a step that only moves the loop, and a whole step
/// past one that turns it, as the revolve this replaced did. A sliver of a
/// turned span beside the cap left the BSP keeping part of the hole.
fn overrun(frames: &[Affine3A], hole: &TaggedLoop) -> Vec<Affine3A> {
    let past = |end: Affine3A, inner: Affine3A| {
        let beyond = end * inner.inverse() * end;
        if !translates(&end, &inner) {
            return beyond;
        }
        let reach = place(&hole.points, &end)
            .iter()
            .zip(place(&hole.points, &beyond))
            .map(|(a, b)| a.distance(b))
            .fold(0.0, f32::max);
        let t = (CUTTER_OVERRUN + CUTTER_MIN_OVERRUN / reach.max(f32::EPSILON)).min(1.0);
        between(end, beyond, t)
    };
    let n = frames.len();
    let mut out = Vec::with_capacity(n + 2);
    out.push(past(frames[0], frames[1]));
    out.extend_from_slice(frames);
    out.push(past(frames[n - 1], frames[n - 2]));
    out
}

fn between(a: Affine3A, b: Affine3A, t: f32) -> Affine3A {
    let (sa, ra, ta) = a.to_scale_rotation_translation();
    let (sb, rb, tb) = b.to_scale_rotation_translation();
    Affine3A::from_scale_rotation_translation(sa.lerp(sb, t), ra.slerp(rb, t), ta.lerp(tb, t))
}

/// The names; see the module docs. `outward` is whether the raw polygons
/// enclosed positive volume, so whether a facet wound as they were faces out.
fn outline(run: &Run, outward: bool) -> (Outline, Vec<(u32, u32)>) {
    let sections = &run.sections;
    let s = sections.len();
    let loops = &sections[0].loops;
    let offsets: Vec<u32> = loops
        .iter()
        .scan(0u32, |at, l| {
            let here = *at;
            *at += l.points.len() as u32;
            Some(here)
        })
        .collect();
    let per_section: u32 = loops.iter().map(|l| l.points.len() as u32).sum();
    let index = |section: usize, loop_index: usize, point: usize| {
        let n = loops[loop_index].points.len();
        section as u32 * per_section + offsets[loop_index] + (point % n) as u32
    };

    let points: Vec<Vec3> =
        sections.iter().flat_map(|section| section.loops.iter().flat_map(|l| place(&l.points, &section.frame))).collect();

    let span_count = spans(run);
    let ends_a_face = |section: usize| match (run.closed, section) {
        (false, 0) => true,
        (false, at) if at == s - 1 => true,
        (_, at) => {
            let before = (at + span_count - 1) % span_count.max(1);
            at < span_count && run.spans.get(before) != run.spans.get(at)
        }
    };
    let mut edges = Vec::new();
    for section in (0..s).filter(|&at| ends_a_face(at)) {
        for (l, tagged) in loops.iter().enumerate() {
            for i in 0..tagged.points.len() {
                edges.push([index(section, l, i), index(section, l, i + 1)]);
            }
        }
    }
    for k in 0..span_count {
        for (l, tagged) in loops.iter().enumerate() {
            let n = tagged.points.len();
            for i in 0..n {
                if tagged.tags[(i + n - 1) % n] != tagged.tags[i] {
                    edges.push([index(k, l, i), index((k + 1) % s, l, i)]);
                }
            }
        }
    }

    // A facet listed in authored order is wound as the polygons were when the
    // loop was authored counter-clockwise.
    let facet = |points_of: Vec<u32>, sign: f32| {
        let at: Vec<Vec3> = points_of.iter().map(|&p| points[p as usize]).collect();
        OutlineFacet { outward: newell(&at).normalize_or_zero() * sign, points: points_of }
    };
    let base = if outward { 1.0 } else { -1.0 };
    let wound = |l: usize| if loops[l].ccw() { base } else { -base };
    let mut faces = Vec::new();
    if !run.closed {
        let n = loops[0].points.len();
        // The start cap is wound backward, as its polygons are.
        faces.push(OutlineFace { facets: vec![facet((0..n).map(|i| index(0, 0, i)).collect(), -wound(0))] });
        faces.push(OutlineFace { facets: vec![facet((0..n).map(|i| index(s - 1, 0, i)).collect(), wound(0))] });
    }
    let mut groups: Vec<(u32, Vec<usize>)> = Vec::new();
    for (k, tag) in run.spans.iter().enumerate().take(span_count) {
        match groups.iter_mut().find(|(t, _)| t == tag) {
            Some((_, spans)) => spans.push(k),
            None => groups.push((*tag, vec![k])),
        }
    }
    let mut sides = Vec::new();
    for (span_tag, group) in &groups {
        for (l, tagged) in loops.iter().enumerate() {
            // A hole's walls face into the hole, which is out of the body.
            let sign = if l == 0 { wound(l) } else { -wound(l) };
            let n = tagged.points.len();
            let mut seen: Vec<u32> = Vec::new();
            for &tag in &tagged.tags {
                if seen.contains(&tag) {
                    continue;
                }
                seen.push(tag);
                let facets = group
                    .iter()
                    .flat_map(|&k| {
                        (0..n).filter(move |&i| tagged.tags[i] == tag).map(move |i| {
                            let next = (k + 1) % s;
                            vec![index(k, l, i), index(k, l, i + 1), index(next, l, i + 1), index(next, l, i)]
                        })
                    })
                    .map(|points_of| facet(points_of, sign))
                    .collect();
                faces.push(OutlineFace { facets });
                sides.push((*span_tag, tag));
            }
        }
    }
    (Outline { points, edges, faces }, sides)
}

/// Sections for a push along local `+Z`, `depth` meters; `midplane` goes half
/// each way.
pub fn extrusion(loops: Vec<TaggedLoop>, depth: f32, midplane: bool) -> Run {
    if depth.abs() < f32::EPSILON {
        return Run { sections: vec![], spans: vec![], closed: false };
    }
    let (near, far) = if midplane { (-depth / 2.0, depth / 2.0) } else { (0.0, depth) };
    let at = |z: f32| Section { loops: loops.clone(), frame: Affine3A::from_translation(Vec3::Z * z) };
    Run { sections: vec![at(near), at(far)], spans: vec![0], closed: false }
}

/// Sections for an extrude from `from` to `to` meters along `along`, a unit
/// direction in the loops' own coordinates. With a taper the loops shrink
/// about the outside loop's centroid as they leave `z = 0`, by `taper`
/// degrees of lean at the outside loop's equivalent radius, so a section is
/// added at `z = 0` when the run crosses it.
pub fn pushed(loops: Vec<TaggedLoop>, from: f32, to: f32, along: Vec3, taper: f32) -> Run {
    if (to - from).abs() < f32::EPSILON || loops.is_empty() {
        return Run { sections: vec![], spans: vec![], closed: false };
    }
    let (center, radius) = reach(&loops[0].points);
    let lean = taper.to_radians().tan();
    let mut stops = vec![from];
    if lean != 0.0 && from < 0.0 && to > 0.0 {
        stops.push(0.0);
    }
    stops.push(to);
    let at = |s: f32| {
        let k = 1.0 - (s * along.z).abs() * lean / radius.max(f32::EPSILON);
        let shrink = Affine3A::from_translation(center.extend(0.0))
            * Affine3A::from_scale(Vec3::new(k, k, 1.0))
            * Affine3A::from_translation(-center.extend(0.0));
        let frame = match lean == 0.0 {
            true => Affine3A::from_translation(along * s),
            false => Affine3A::from_translation(along * s) * shrink,
        };
        Section { loops: loops.clone(), frame }
    };
    let spans = (0..stops.len() as u32 - 1).collect();
    Run { sections: stops.into_iter().map(at).collect(), spans, closed: false }
}

/// Whether a loop tapered by `taper` degrees still has some size `rise`
/// meters from its plane: a tenth of its own, at least.
pub fn tapers_within(points: &[Vec2], taper: f32, rise: f32) -> bool {
    let (_, radius) = reach(points);
    1.0 - rise * taper.to_radians().tan() / radius.max(f32::EPSILON) > 0.1
}

/// A loop's area centroid, and the radius of the circle of its area.
fn reach(points: &[Vec2]) -> (Vec2, f32) {
    let twice = area_x2(points);
    if twice.abs() < f32::EPSILON {
        let middle = points.iter().copied().sum::<Vec2>() / points.len().max(1) as f32;
        return (middle, 0.0);
    }
    let n = points.len();
    let center = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            (a + b) * a.perp_dot(b)
        })
        .sum::<Vec2>()
        / (3.0 * twice);
    (center, (twice.abs() / 2.0 / std::f32::consts::PI).sqrt())
}

/// Sections for a turn about local `+Y` in `steps` straight steps; a whole
/// turn closes.
pub fn revolution(loops: Vec<TaggedLoop>, degrees: f32, steps: u32) -> Run {
    let full = crate::prop::solid::is_full_turn(degrees);
    let sweep = if full { std::f32::consts::TAU } else { degrees.to_radians() };
    let count = if full { steps } else { steps + 1 };
    let sections = (0..count)
        .map(|k| {
            // Columns written out, not a quaternion, so a point lands exactly
            // where `(x cos, y, -x sin)` puts it.
            let (sin, cos) = (sweep * k as f32 / steps as f32).sin_cos();
            let turn = Mat3::from_cols(Vec3::new(cos, 0.0, -sin), Vec3::Y, Vec3::new(sin, 0.0, cos));
            Section { loops: loops.clone(), frame: Affine3A::from_mat3(turn) }
        })
        .collect();
    Run { sections, spans: vec![0; steps as usize], closed: full }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::reference::{Element, Located};

    fn square() -> TaggedLoop {
        TaggedLoop::numbered(vec![Vec2::new(0.02, 0.0), Vec2::new(0.06, 0.0), Vec2::new(0.06, 0.1), Vec2::new(0.02, 0.1)])
    }

    /// Fails if a polygon is tagged with a face it does not lie on.
    #[test]
    fn every_polygon_lies_on_the_face_it_is_tagged_with() {
        let clockwise = TaggedLoop::numbered(vec![Vec2::new(0.02, 0.0), Vec2::new(0.02, 0.1), Vec2::new(0.06, 0.1), Vec2::new(0.06, 0.0)]);
        let outer = TaggedLoop::numbered(vec![Vec2::new(0.01, -0.1), Vec2::new(0.09, -0.1), Vec2::new(0.09, 0.2), Vec2::new(0.01, 0.2)]);
        let runs = [
            extrusion(vec![square()], 0.2, false),
            extrusion(vec![clockwise.clone()], -0.2, true),
            revolution(vec![square()], 360.0, 8),
            revolution(vec![clockwise.clone()], 120.0, 4),
            extrusion(vec![outer.clone(), square()], 0.1, false),
            revolution(vec![outer, clockwise], 90.0, 3),
        ];
        for (n, run) in runs.iter().enumerate() {
            let swept = skin(run, Surface::default()).unwrap();
            for polygon in swept.solid.polygons() {
                let tag = polygon.surface.tag;
                assert_ne!(tag, NO_FACE, "run {n}");
                let Some(face @ Located::Face { .. }) = swept.outline.locate(Element::Face(tag)) else { panic!("run {n}: no face {tag}") };
                let Located::Face { facets, .. } = &face else { unreachable!() };
                let center = polygon.vertices.iter().sum::<Vec3>() / polygon.vertices.len() as f32;
                let on = facets.iter().any(|points| {
                    let normal = newell(points).normalize_or_zero();
                    normal != Vec3::ZERO && (center - points[0]).dot(normal).abs() < 1e-4
                });
                assert!(on, "run {n}: a polygon tagged {tag} is off that face");
            }
        }
    }

    #[test]
    fn a_revolved_segment_is_one_face() {
        let swept = skin(&revolution(vec![square()], 360.0, 8), Surface::default()).unwrap();
        assert_eq!(swept.outline.faces.len(), 4, "a whole turn has no caps, and one face per side of the square");
        assert!(swept.outline.faces.iter().all(|face| face.facets.len() == 8));
        // No ring edges inside a face; the four corners each trace a circle.
        assert_eq!(swept.outline.edges.len(), 4 * 8);
    }

    #[test]
    fn a_revolved_flat_ring_is_a_flat_face() {
        let swept = skin(&revolution(vec![square()], 360.0, 8), Surface::default()).unwrap();
        // Segment 0 runs along the bottom, so sweeps a flat annulus facing -Y.
        let Some(face @ Located::Face { normal, .. }) = swept.outline.locate(Element::Face(0)) else { panic!() };
        assert!(face.is_flat());
        assert!(normal.distance(Vec3::NEG_Y) < 1e-5, "{normal}");
        // Segment 1, the outside, is a cylinder.
        assert!(!swept.outline.locate(Element::Face(1)).unwrap().is_flat());
    }

    #[test]
    fn an_extrusion_names_caps_then_sides() {
        let swept = skin(&extrusion(vec![square()], 0.2, false), Surface::default()).unwrap();
        let normal = |i| match swept.outline.locate(Element::Face(i)) {
            Some(Located::Face { normal, .. }) => normal,
            _ => panic!(),
        };
        assert!(normal(0).distance(Vec3::NEG_Z) < 1e-5);
        assert!(normal(1).distance(Vec3::Z) < 1e-5);
        assert!(normal(2).distance(Vec3::NEG_Y) < 1e-5, "the side swept by segment 0");
        assert_eq!(swept.outline.edges.len(), 12);
    }

    #[test]
    fn a_hole_is_cut_and_its_walls_face_into_it() {
        let outer = TaggedLoop::numbered(vec![Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0)]);
        // Clockwise, as a hole may be.
        let hole = TaggedLoop::numbered(vec![Vec2::new(-0.5, -0.5), Vec2::new(-0.5, 0.5), Vec2::new(0.5, 0.5), Vec2::new(0.5, -0.5)]);
        let swept = skin(&extrusion(vec![outer, hole], 1.0, false), Surface::default()).unwrap();
        assert!((swept.solid.volume() - 3.0).abs() < 1e-4, "{}", swept.solid.volume());
        // The hole's first wall runs from (-0.5,-0.5) up to (-0.5,0.5): its
        // face points +X, into the hole.
        let Some(Located::Face { normal, .. }) = swept.outline.locate(Element::Face(6)) else { panic!() };
        assert!(normal.distance(Vec3::X) < 1e-5, "{normal}");
    }

    #[test]
    fn a_loop_straddling_the_axis_folds() {
        let across = TaggedLoop::numbered(vec![Vec2::new(-0.1, 0.0), Vec2::new(0.1, 0.0), Vec2::new(0.1, 0.1)]);
        assert_eq!(skin(&revolution(vec![across], 90.0, 4), Surface::default()).unwrap_err(), SkinProblem::Folds);
    }

    #[test]
    fn sections_that_do_not_match_are_a_problem() {
        let mut run = extrusion(vec![square()], 0.1, false);
        run.sections[1].loops[0].points.pop();
        run.sections[1].loops[0].tags.pop();
        assert_eq!(skin(&run, Surface::default()).unwrap_err(), SkinProblem::Mismatch);
    }
}
