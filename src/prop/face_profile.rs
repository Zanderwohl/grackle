//! A flat face as a sweep's profile, read off the body as it stands when the
//! sweep runs. Each boundary piece is tagged by the face across it, so side
//! faces are numbered by what they border. See
//! `documentation/prop-modeling.md`, "Faces as profiles".

use bevy::prelude::*;

use crate::get;
use crate::prop::plane::Plane;
use crate::prop::reference::{GeomRef, Located};
use crate::prop::refs::Scope;
use crate::prop::region::{Region, RegionRef};
use crate::prop::sketch::SketchId;
use crate::prop::solid::{Polygon, NO_FACE};

/// Meters off a plane or a line that still counts as on it, above the kernel's rounding.
const ON: f32 = 2e-5;
/// Meters; a boundary piece shorter than this is a BSP sliver.
const SHORTEST: f32 = 1e-4;
/// Meters two pieces' ends may be apart and still join.
const JOIN: f32 = 2e-4;

/// In the face's plane, with the face on its left and face `across` on its right.
#[derive(Clone, Copy, Debug)]
struct Piece {
    a: Vec2,
    b: Vec2,
    across: u32,
}

/// `x` follows the generator's first edge, so a later cut into the face does
/// not turn the plane. Entity ids are the ranks of the faces across.
pub fn face_profile(face: &GeomRef, scope: &Scope) -> Result<(Plane, Vec<Region>), String> {
    let missing = || get!("prop.problems.missing_profile_face");
    let Some(Located::Face { facets, normal }) = scope.locate(face) else { return Err(missing()) };
    let located = Located::Face { facets: facets.clone(), normal };
    if !located.is_flat() {
        return Err(get!("prop.problems.profile_face_curved"));
    }
    let (solid, tag, faces) = scope.body_with_face(face).ok_or_else(missing)?;
    let plane = plane_of(&facets, normal).ok_or_else(missing)?;

    let own: Vec<&Polygon> = solid.polygons().iter().filter(|p| p.surface.tag == tag).collect();
    if own.iter().flat_map(|p| &p.vertices).any(|v| plane.distance(*v).abs() > ON * 10.0) {
        return Err(get!("prop.problems.profile_face_curved"));
    }
    // Only a polygon with an edge in the face's plane can border it.
    let (low, high) = own
        .iter()
        .flat_map(|p| &p.vertices)
        .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(low, high), v| (low.min(*v), high.max(*v)));
    let (low, high) = (low - Vec3::splat(JOIN), high + Vec3::splat(JOIN));
    let near = |v: &Vec3| plane.distance(*v).abs() < ON * 10.0 && v.cmpge(low).all() && v.cmple(high).all();
    let others: Vec<&Polygon> = solid
        .polygons()
        .iter()
        .filter(|p| p.surface.tag != tag && p.vertices.iter().filter(|v| near(v)).count() >= 2)
        .collect();

    let pieces = boundary(&own, &others, &plane);
    let loops = chain(pieces).ok_or_else(|| get!("prop.problems.profile_face_open"))?;

    // Source first, so faces a later feature makes rank last.
    let mut names: Vec<(crate::prop::reference::Source, crate::prop::reference::Element, u32)> = loops
        .iter()
        .flatten()
        .filter_map(|piece| faces.name(piece.across).map(|name| (name.source.clone(), name.element, piece.across)))
        .collect();
    names.sort();
    names.dedup_by_key(|(_, _, tag)| *tag);
    let unnamed = names.len() as u32;
    let rank = |across: u32| {
        names.iter().position(|(_, _, tag)| *tag == across && across != NO_FACE).map_or(unnamed, |at| at as u32)
    };

    let mut outers: Vec<(Vec<Vec2>, Vec<SketchId>)> = Vec::new();
    let mut holes: Vec<(Vec<Vec2>, Vec<SketchId>)> = Vec::new();
    for pieces in loops {
        let points: Vec<Vec2> = pieces.iter().map(|piece| piece.a).collect();
        let entities = pieces.iter().map(|piece| SketchId(rank(piece.across))).collect();
        match signed_area(&points) > 0.0 {
            true => outers.push((points, entities)),
            false => holes.push((points, entities)),
        }
    }
    let tangled = || get!("prop.problems.profile_face_tangled");
    if outers.is_empty() {
        return Err(tangled());
    }
    let mut regions: Vec<Region> = outers
        .into_iter()
        .map(|(outer, outer_entities)| Region {
            name: RegionRef { bounds: vec![] },
            outer,
            holes: vec![],
            outer_entities,
            hole_entities: vec![],
        })
        .collect();
    for (hole, entities) in holes {
        // From beside the hole, since its corner may lie on the outside. The
        // smallest outside wins, for a hole in an island.
        let beside = beside_the_hole(&hole).ok_or_else(tangled)?;
        let around = regions
            .iter_mut()
            .filter(|region| inside(&region.outer, beside))
            .min_by(|a, b| signed_area(&a.outer).total_cmp(&signed_area(&b.outer)))
            .ok_or_else(tangled)?;
        around.holes.push(hole);
        around.hole_entities.push(entities);
    }
    Ok((plane, regions))
}

fn plane_of(facets: &[Vec<Vec3>], normal: Vec3) -> Option<Plane> {
    let normal = Dir3::new(normal).ok()?;
    let first = facets.first()?;
    let origin = *first.first()?;
    let x = first
        .iter()
        .skip(1)
        .map(|p| *p - origin)
        .map(|v| v - v.dot(*normal) * *normal)
        .find(|v| v.length() > SHORTEST)
        .unwrap_or_else(|| normal.any_orthonormal_vector());
    Some(Plane { origin, x: Dir3::new(x).ok()?, normal })
}

/// Edges no other piece of the face shares, split where the face across
/// changes. Polygons wind counter-clockwise from outside, so the face is on
/// each piece's left.
fn boundary(own: &[&Polygon], others: &[&Polygon], plane: &Plane) -> Vec<Piece> {
    let edges = |polygons: &[&Polygon]| -> Vec<(usize, Vec3, Vec3, u32)> {
        polygons
            .iter()
            .enumerate()
            .flat_map(|(at, p)| {
                let n = p.vertices.len();
                (0..n).map(move |i| (at, p.vertices[i], p.vertices[(i + 1) % n], p.surface.tag))
            })
            .collect()
    };
    let own_edges = edges(own);
    let other_edges = edges(others);
    let mut pieces = Vec::new();
    for &(polygon, a, b, _) in &own_edges {
        let along = b - a;
        let length_sq = along.length_squared();
        if length_sq < SHORTEST * SHORTEST {
            continue;
        }
        let span = |p: Vec3, q: Vec3| -> Option<(f32, f32)> {
            let off = |x: Vec3| (x - a).reject_from_normalized(along / length_sq.sqrt()).length();
            (off(p) < ON && off(q) < ON).then(|| {
                let (s, t) = ((p - a).dot(along) / length_sq, (q - a).dot(along) / length_sq);
                (s.min(t), s.max(t))
            })
        };
        let covered: Vec<(f32, f32)> =
            own_edges.iter().filter(|(other, ..)| *other != polygon).filter_map(|&(_, p, q, _)| span(p, q)).collect();
        let across: Vec<((f32, f32), u32)> = other_edges.iter().filter_map(|&(_, p, q, tag)| Some((span(p, q)?, tag))).collect();
        let tolerance = SHORTEST / length_sq.sqrt();
        for (s, t) in uncovered(&covered, tolerance) {
            let mut cuts: Vec<f32> = vec![s, t];
            cuts.extend(across.iter().flat_map(|((p, q), _)| [*p, *q]).filter(|c| *c > s + tolerance && *c < t - tolerance));
            cuts.sort_by(f32::total_cmp);
            for pair in cuts.windows(2) {
                let (from, to) = (pair[0], pair[1]);
                if to - from < tolerance {
                    continue;
                }
                let middle = (from + to) / 2.0;
                let tag = across
                    .iter()
                    .find(|((p, q), _)| *p <= middle && middle <= *q)
                    .map_or(NO_FACE, |(_, tag)| *tag);
                pieces.push(Piece { a: plane.flatten(a + along * from), b: plane.flatten(a + along * to), across: tag });
            }
        }
    }
    pieces
}

/// `[0, 1]` less every interval in `covered`.
fn uncovered(covered: &[(f32, f32)], tolerance: f32) -> Vec<(f32, f32)> {
    let mut intervals: Vec<(f32, f32)> = covered.iter().map(|(s, t)| (s.max(0.0), t.min(1.0))).filter(|(s, t)| t > s).collect();
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    let mut at = 0.0f32;
    for (s, t) in intervals {
        if s > at + tolerance {
            out.push((at, s));
        }
        at = at.max(t);
    }
    if at < 1.0 - tolerance {
        out.push((at, 1.0));
    }
    out
}

/// A walk through a pinch or a touching hole passes one point twice, so each
/// walk is split into simple loops. `None` if a loop does not close.
fn chain(mut pieces: Vec<Piece>) -> Option<Vec<Vec<Piece>>> {
    let mut loops = Vec::new();
    while let Some(first) = pieces.pop() {
        let mut walked = vec![first];
        loop {
            let last = *walked.last().unwrap();
            if last.b.distance(walked[0].a) < JOIN && walked.len() > 1 {
                break;
            }
            let heading = (last.b - last.a).normalize_or_zero();
            let next = pieces
                .iter()
                .enumerate()
                .filter(|(_, piece)| piece.a.distance(last.b) < JOIN)
                .max_by(|(_, p), (_, q)| {
                    // Leftmost first, which keeps to the face; straight back last.
                    let turn = |piece: &Piece| {
                        let out = (piece.b - piece.a).normalize_or_zero();
                        let angle = heading.perp_dot(out).atan2(heading.dot(out));
                        if angle > std::f32::consts::PI - 1e-3 { -std::f32::consts::PI } else { angle }
                    };
                    turn(p).total_cmp(&turn(q))
                })
                .map(|(at, _)| at)?;
            walked.push(pieces.swap_remove(next));
        }
        loops.extend(split_at_repeats(walked).into_iter().map(merged));
    }
    Some(loops.into_iter().filter(|pieces| pieces.len() >= 3).collect())
}

/// Cuts a loop off wherever a piece ends where an earlier piece on the stack starts.
fn split_at_repeats(pieces: Vec<Piece>) -> Vec<Vec<Piece>> {
    let mut loops = Vec::new();
    let mut stack: Vec<Piece> = Vec::new();
    for piece in pieces {
        stack.push(piece);
        if let Some(at) = stack.iter().position(|earlier| earlier.a.distance(piece.b) < JOIN) {
            loops.push(stack.split_off(at));
        }
    }
    if !stack.is_empty() {
        loops.push(stack);
    }
    loops
}

/// Consecutive pieces on one line with the same face across, as one.
fn merged(pieces: Vec<Piece>) -> Vec<Piece> {
    let straight = |p: &Piece, q: &Piece| {
        p.across == q.across && {
            let (u, v) = (p.b - p.a, q.b - q.a);
            u.perp_dot(v).abs() < ON * u.length().max(v.length()) && u.dot(v) > 0.0
        }
    };
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        match out.last_mut() {
            Some(last) if straight(last, &piece) => last.b = piece.b,
            _ => out.push(piece),
        }
    }
    while out.len() > 1 && straight(out.last().unwrap(), &out[0]) {
        let last = out.pop().unwrap();
        out[0].a = last.a;
    }
    out
}

/// Just off the middle of the hole's longest side, on its left, where the face is.
fn beside_the_hole(hole: &[Vec2]) -> Option<Vec2> {
    let n = hole.len();
    let (a, b) = (0..n).map(|i| (hole[i], hole[(i + 1) % n])).max_by(|(a, b), (c, d)| a.distance(*b).total_cmp(&c.distance(*d)))?;
    let along = (b - a).try_normalize()?;
    Some((a + b) / 2.0 + along.perp() * (SHORTEST / 2.0).min(a.distance(b) / 4.0))
}

fn signed_area(points: &[Vec2]) -> f32 {
    (0..points.len()).map(|i| points[i].perp_dot(points[(i + 1) % points.len()])).sum::<f32>() / 2.0
}

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
    use crate::prop::feature::{evaluate, BooleanOp, End, Evaluated, FeatureOp, ProfileSource, PropFeature, PropFeatureId, SweepOp};
    use crate::prop::profile::Placement;
    use crate::prop::reference::{Element, GeomRef, Located, Source};
    use crate::prop::refs::CurveRef;
    use crate::prop::solid::Shape;
    use crate::prop::surface::Surface;
    use bevy::prelude::*;

    fn block(id: u32, size: [f32; 3], at: Vec3) -> PropFeature {
        PropFeature::new(
            PropFeatureId(id),
            FeatureOp::Primitive { shape: Shape::Box { size }, placement: Placement::at(at.x, at.y, at.z), surface: Surface::default() },
        )
    }

    fn cut(id: u32, target: u32, tool: u32) -> PropFeature {
        PropFeature::new(
            PropFeatureId(id),
            FeatureOp::Boolean { op: BooleanOp::Subtract, target: PropFeatureId(target), tool: PropFeatureId(tool) },
        )
    }

    fn face(feature: u32, index: u32) -> GeomRef {
        GeomRef { element: Element::Face(index), source: Source { feature: PropFeatureId(feature), copies: vec![] } }
    }

    /// The face `feature` made whose normal is `normal`.
    fn facing(evaluated: &Evaluated, feature: u32, normal: Vec3) -> Option<u32> {
        (0..64).find(|&i| {
            matches!(evaluated.locate(&face(feature, i)), Some(Located::Face { normal: n, .. }) if n.dot(normal) > 0.999)
        })
    }

    fn extruded(id: u32, face: GeomRef, distance: f32, op: SweepOp, target: Option<u32>) -> PropFeature {
        PropFeature::new(
            PropFeatureId(id),
            FeatureOp::Extrude {
                taper: 0.0,
                op,
                target: target.map(PropFeatureId),
                end: End::Distance { distance },
                other: None,
                direction: None,
                profile: ProfileSource::Face { face },
                surface: Surface::default(),
            },
        )
    }

    fn volume_of(evaluated: &Evaluated, id: u32) -> f32 {
        evaluated.bodies.iter().find(|(body, _)| *body == PropFeatureId(id)).expect("the body").1.volume()
    }

    fn top_of_the_cube(features: &[PropFeature]) -> GeomRef {
        face(0, facing(&evaluate(features), 0, Vec3::Y).expect("a top face"))
    }

    #[test]
    fn a_face_extrudes_out_of_its_body() {
        let mut features = vec![block(0, [1.0; 3], Vec3::ZERO)];
        let top = top_of_the_cube(&features);
        features.push(extruded(9, top, 0.5, SweepOp::Join, Some(0)));
        let evaluated = evaluate(&features);
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        assert_eq!(evaluated.bodies.len(), 1);
        assert!((volume_of(&evaluated, 9) - 1.5).abs() < 1e-4);
    }

    /// Fails if the hole is filled, or the joined cap survives inside the body.
    #[test]
    fn a_face_with_a_hole_in_it_extrudes_with_the_hole() {
        let mut features = vec![
            block(0, [1.0; 3], Vec3::ZERO),
            block(1, [0.4, 2.0, 0.4], Vec3::ZERO),
            cut(2, 0, 1),
        ];
        let top = top_of_the_cube(&features);
        features.push(extruded(9, top, 1.0, SweepOp::Join, Some(2)));
        let evaluated = evaluate(&features);
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        assert!((volume_of(&evaluated, 9) - 2.0 * (1.0 - 0.16)).abs() < 1e-4, "{}", volume_of(&evaluated, 9));
    }

    /// Fails if a notch cut into the face renames its side faces.
    #[test]
    fn a_notch_in_the_face_renames_no_side_face() {
        let plain = vec![block(0, [1.0; 3], Vec3::ZERO)];
        let top = top_of_the_cube(&plain);
        let mut before = plain.clone();
        before.push(extruded(9, top.clone(), 1.0, SweepOp::NewBody, None));
        let before = evaluate(&before);
        let east = facing(&before, 9, Vec3::X).expect("an east side");

        let notched = vec![
            block(0, [1.0; 3], Vec3::ZERO),
            block(1, [0.4, 0.4, 0.4], Vec3::new(-0.5, 0.5, -0.5)),
            cut(2, 0, 1),
            extruded(9, top, 1.0, SweepOp::NewBody, None),
        ];
        let after = evaluate(&notched);
        assert!(after.problems.is_empty(), "{:?}", after.problems);
        assert!((volume_of(&after, 9) - 0.96).abs() < 1e-4, "{}", volume_of(&after, 9));
        let Some(Located::Face { normal, .. }) = after.locate(&face(9, east)) else { panic!("face {east} is gone") };
        assert!(normal.dot(Vec3::X) > 0.999, "face {east} now faces {normal}");
    }

    #[test]
    fn a_face_cut_away_entirely_is_a_problem() {
        let mut features = vec![
            block(0, [1.0; 3], Vec3::ZERO),
            block(1, [2.0, 1.0, 2.0], Vec3::new(0.0, 0.5, 0.0)),
            cut(2, 0, 1),
        ];
        let top = top_of_the_cube(&features);
        features.push(extruded(9, top, 1.0, SweepOp::NewBody, None));
        let evaluated = evaluate(&features);
        assert!(evaluated.problems.iter().any(|problem| problem.feature == PropFeatureId(9)));
        assert!(evaluated.bodies.iter().all(|(body, _)| *body != PropFeatureId(9)));
    }

    #[test]
    fn a_face_revolves_about_its_own_edge() {
        let mut features = vec![block(0, [1.0; 3], Vec3::ZERO)];
        let evaluated = evaluate(&features);
        let east = face(0, facing(&evaluated, 0, Vec3::X).expect("an east face"));
        let edge = (0..64)
            .map(|i| GeomRef { element: Element::Edge(i), source: Source { feature: PropFeatureId(0), copies: vec![] } })
            .find(|edge| {
                matches!(evaluated.locate(edge), Some(Located::Edge(a, b))
                    if [a, b].iter().all(|p| (p.x - 0.5).abs() < 1e-5 && (p.z - 0.5).abs() < 1e-5))
            })
            .expect("the edge up the east face's corner");
        features.push(PropFeature::new(
            PropFeatureId(9),
            FeatureOp::Revolve {
                degrees: 360.0,
                steps: 12,
                symmetric: false,
                op: SweepOp::NewBody,
                target: None,
                axis: CurveRef::Edge { edge },
                profile: ProfileSource::Face { face: east },
                surface: Surface::default(),
            },
        ));
        let evaluated = evaluate(&features);
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        // A 12-gon a meter to its corners, a meter tall.
        assert!((volume_of(&evaluated, 9) - 3.0).abs() < 1e-3, "{}", volume_of(&evaluated, 9));
    }

    /// Fails if a walk through a touching hole comes out as one loop.
    #[test]
    fn a_walk_through_a_touching_hole_is_split_into_simple_loops() {
        use super::{chain, signed_area, Piece};
        let ring = |corners: &[Vec2]| {
            let n = corners.len();
            (0..n).map(|i| Piece { a: corners[i], b: corners[(i + 1) % n], across: i as u32 }).collect::<Vec<_>>()
        };
        let mut pieces = ring(&[Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(2.0, 2.0), Vec2::new(0.0, 2.0)]);
        pieces[0].across = 7;
        pieces[1].across = 7;
        pieces.extend(ring(&[Vec2::new(1.0, 0.0), Vec2::new(0.5, 0.5), Vec2::new(1.0, 1.0), Vec2::new(1.5, 0.5)]));
        for _ in 0..pieces.len() {
            pieces.rotate_left(1);
            let loops = chain(pieces.clone()).expect("closed");
            assert_eq!(loops.len(), 2, "{loops:?}");
            let areas: Vec<f32> = loops.iter().map(|l| signed_area(&l.iter().map(|p| p.a).collect::<Vec<_>>())).collect();
            assert!(areas.iter().any(|a| (*a - 4.0).abs() < 1e-5) && areas.iter().any(|a| (*a + 0.5).abs() < 1e-5), "{areas:?}");
        }
    }

    #[test]
    fn a_face_pinched_at_a_corner_extrudes_as_two_pieces() {
        let mut features = vec![
            block(0, [1.0; 3], Vec3::ZERO),
            block(1, [0.5, 0.4, 0.5], Vec3::new(0.25, 0.5, -0.25)),
            cut(2, 0, 1),
            block(3, [0.5, 0.4, 0.5], Vec3::new(-0.25, 0.5, 0.25)),
            cut(4, 2, 3),
        ];
        let top = top_of_the_cube(&features);
        features.push(extruded(9, top, 1.0, SweepOp::NewBody, None));
        let evaluated = evaluate(&features);
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        assert!((volume_of(&evaluated, 9) - 0.5).abs() < 1e-4, "{}", volume_of(&evaluated, 9));
    }

    /// Fails if a hole meeting the outside at one corner is filled in.
    #[test]
    fn a_hole_touching_the_faces_edge_is_still_a_hole() {
        let half = 0.1 * std::f32::consts::SQRT_2;
        let mut diamond = block(1, [0.2, 0.4, 0.2], Vec3::new(0.5 - half, 0.5, 0.0));
        if let FeatureOp::Primitive { placement, .. } = &mut diamond.op {
            placement.rotation = [0.0, std::f32::consts::FRAC_PI_4, 0.0];
        }
        let mut features = vec![block(0, [1.0; 3], Vec3::ZERO), diamond, cut(2, 0, 1)];
        let top = top_of_the_cube(&features);
        features.push(extruded(9, top, 1.0, SweepOp::NewBody, None));
        let evaluated = evaluate(&features);
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        assert!((volume_of(&evaluated, 9) - 0.96).abs() < 1e-3, "{}", volume_of(&evaluated, 9));
    }

    /// Fails if the second region's polygons name the first region's faces.
    #[test]
    fn every_polygon_of_a_two_region_sweep_lies_on_the_face_it_names() {
        let mut features = vec![block(0, [1.0; 3], Vec3::ZERO), block(1, [0.2, 2.0, 2.0], Vec3::ZERO), cut(2, 0, 1)];
        let top = top_of_the_cube(&features);
        features.push(extruded(9, top, 1.0, SweepOp::NewBody, None));
        let evaluated = evaluate(&features);
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        let (_, solid) = evaluated.bodies.iter().find(|(body, _)| *body == PropFeatureId(9)).unwrap();
        for polygon in solid.polygons() {
            let name = evaluated.faces.name(polygon.surface.tag).expect("a named face").clone();
            let Some(Located::Face { facets, normal }) = evaluated.locate(&name) else { panic!("{name:?} does not resolve") };
            let middle = polygon.vertices.iter().sum::<Vec3>() / polygon.vertices.len() as f32;
            let on = facets.iter().flatten().next().unwrap();
            assert!((middle - *on).dot(normal).abs() < 1e-4, "a polygon at {middle} is named {name:?}");
        }
    }

    #[test]
    fn a_face_profile_survives_the_file() {
        let mut features = vec![block(0, [1.0; 3], Vec3::ZERO)];
        let top = top_of_the_cube(&features);
        features.push(extruded(9, top, 0.5, SweepOp::NewBody, None));
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Doc {
            features: Vec<PropFeature>,
        }
        let doc = Doc { features };
        let text = toml::to_string_pretty(&doc).unwrap();
        assert!(text.contains("from = \"face\""), "{text}");
        assert_eq!(toml::from_str::<Doc>(&text).unwrap(), doc);
    }
}
