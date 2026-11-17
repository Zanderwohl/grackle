//! Lightmaps for the map's surface: a room's faces charted onto a lattice of [`TEXEL`] squares and
//! packed into one atlas per room. See documentation/sun.md.
//!
//! The lattice depends only on a face's plane, never on the face, so two rooms' floors meeting at
//! a threshold sample the same points and bake the same values on both sides of the seam. The room
//! bake writes `UV_1` from an [`Atlas`] and the sun bake fills one from the same faces; both call
//! [`Atlas::of`], which is deterministic, so neither has to hold the other's layout.

use bevy::prelude::*;

use crate::common::sky::{Brightness, SkyTime};
use crate::common::sunlight::{Daylight, LitSurface};
use crate::common::world_surface::Face;

/// Meters per lightmap texel.
pub const TEXEL: f32 = 0.25;

/// The default [`Brightness`](crate::common::sky::Brightness) ambient, in
/// `GlobalAmbientLight::brightness` units: a lightmap replaces the ambient term on its meshes.
pub const BASE_AMBIENT: f32 = 100.0;

/// The default sun brightness, in the same units.
pub const SUN_BRIGHTNESS: f32 = 1500.0;

/// The default sky brightness, in the same units.
pub const SKY_BRIGHTNESS: f32 = 600.0;

/// Of every surface, for the bounce.
pub const ALBEDO: f32 = 0.4;

/// Sky light and bounce are smooth, so they are gathered every this many texels — a meter — and
/// interpolated between. On the same lattice, so neighbors across a threshold share samples too.
const AMBIENT_STEP: f32 = 4.0;

pub fn daylight(toward: Vec3, sky: SkyTime, brightness: Brightness) -> Daylight {
    Daylight { toward, base: brightness.ambient, sun: brightness.sun, sky, sky_brightness: brightness.sky, albedo: ALBEDO }
}

/// Texels round a chart sampled at its face's edge, so bilinear filtering at the edge reads the
/// edge's own light rather than a neighboring chart's.
const PADDING: i32 = 1;

/// How far inside its face a texel past the edge is sampled, in lattice units: off the corner,
/// where a ray would start inside the wall it meets.
const INSET: f32 = 0.08;

/// How far from a face a texel center may be and still be baked, in lattice units: bilinear
/// filtering at a point on a face reads texels up to a diagonal, √2, away.
const REACH: f32 = 1.5;

/// Every constant a bake's output depends on, for keying the lightmap cache.
pub fn tuning() -> [u32; 6] {
    [TEXEL, ALBEDO, AMBIENT_STEP, PADDING as f32, INSET, REACH].map(f32::to_bits)
}

/// The faces in one plane whose bounds, in lattice units, come within two texels of each other,
/// chained: each cluster ascending, the clusters by their first face.
fn clusters(faces: &[Face], bounds: &[(Vec2, Vec2)]) -> Vec<Vec<usize>> {
    const GAP: f32 = 2.0;
    let coplanar = |a: &Face, b: &Face| a.plane.normal.dot(b.plane.normal) > 0.9999 && (a.plane.w - b.plane.w).abs() < 1e-3;
    let near = |a: (Vec2, Vec2), b: (Vec2, Vec2)| (a.0 - GAP).cmple(b.1).all() && (b.0 - GAP).cmple(a.1).all();
    let mut parent: Vec<usize> = (0..faces.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for a in 0..faces.len() {
        for b in a + 1..faces.len() {
            if coplanar(&faces[a], &faces[b]) && near(bounds[a], bounds[b]) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    let mut slot = vec![usize::MAX; faces.len()];
    for index in 0..faces.len() {
        let r = root(&mut parent, index);
        if slot[r] == usize::MAX {
            slot[r] = clusters.len();
            clusters.push(Vec::new());
        }
        clusters[slot[r]].push(index);
    }
    clusters
}

/// Zero inside the convex `outline`, else the distance to its nearest edge.
fn distance_to(outline: &[Vec2], point: Vec2) -> f32 {
    let count = outline.len();
    let winding: f32 = (0..count).map(|i| outline[i].perp_dot(outline[(i + 1) % count])).sum();
    let edge = |i: usize| (outline[i], outline[(i + 1) % count]);
    if (0..count).all(|i| {
        let (a, b) = edge(i);
        (b - a).perp_dot(point - a) * winding >= 0.0
    }) {
        return 0.0;
    }
    (0..count)
        .map(|i| {
            let (a, b) = edge(i);
            let t = ((point - a).dot(b - a) / (b - a).length_squared().max(f32::EPSILON)).clamp(0.0, 1.0);
            (a + (b - a) * t).distance(point)
        })
        .fold(f32::INFINITY, f32::min)
}

/// The lattice's two in-plane axes for a surface facing along `normal`. Floors and ceilings are
/// ruled in X and Z; walls along their run and down.
fn axes(normal: Vec3) -> (Vec3, Vec3) {
    if normal.y.abs() > 0.5 {
        (Vec3::X, Vec3::Z)
    } else {
        (Vec3::Y.cross(normal).normalize(), Vec3::NEG_Y)
    }
}

/// One face's rectangle of texels.
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    u: Vec3,
    v: Vec3,
    /// The face's plane, which is what decides its lattice.
    normal: Vec3,
    w: f32,
    /// The lattice square at the chart's first texel.
    origin: IVec2,
    pub size: UVec2,
    /// Where the chart's first texel is in the atlas.
    at: UVec2,
    /// The faces on it, ascending.
    faces: Vec<usize>,
}

impl Chart {
    fn lattice(&self, point: Vec3) -> Vec2 {
        Vec2::new(point.dot(self.u), point.dot(self.v)) / TEXEL
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Atlas {
    /// One per cluster of touching faces in one plane.
    pub charts: Vec<Chart>,
    /// Which chart each face is on, in the faces' order.
    chart_of: Vec<usize>,
    pub size: UVec2,
}

impl Atlas {
    /// Faces in one plane whose padded bounds touch share a chart, so a floor cut into slivers by
    /// the kernel is one rectangle rather than a bounding box per sliver. Shelf-packed, tallest
    /// first.
    pub fn of(faces: &[Face]) -> Atlas {
        let bounds: Vec<(Vec2, Vec2)> = faces
            .iter()
            .map(|face| {
                let (u, v) = axes(face.plane.normal);
                face.vertices
                    .iter()
                    .map(|p| Vec2::new(p.dot(u), p.dot(v)) / TEXEL)
                    .fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), p| (lo.min(p), hi.max(p)))
            })
            .collect();
        let mut charts: Vec<Chart> = clusters(faces, &bounds)
            .into_iter()
            .map(|members| {
                let plane = faces[members[0]].plane;
                let (u, v) = axes(plane.normal);
                let (lo, hi) = members
                    .iter()
                    .map(|&index| bounds[index])
                    .fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), (a, b)| (lo.min(a), hi.max(b)));
                let origin = lo.floor().as_ivec2() - PADDING;
                let size = (hi.ceil().as_ivec2() + PADDING - origin).max(IVec2::ONE).as_uvec2();
                Chart { u, v, normal: plane.normal, w: plane.w, origin, size, at: UVec2::ZERO, faces: members }
            })
            .collect();
        let mut chart_of = vec![0; faces.len()];
        for (index, chart) in charts.iter().enumerate() {
            for &face in &chart.faces {
                chart_of[face] = index;
            }
        }

        let area: u32 = charts.iter().map(|chart| chart.size.x * chart.size.y).sum();
        let widest = charts.iter().map(|chart| chart.size.x).max().unwrap_or(1);
        let width = widest.max((area as f32).sqrt().ceil() as u32);
        let mut order: Vec<usize> = (0..charts.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse((charts[index].size.y, charts[index].size.x)));

        let (mut x, mut y, mut shelf) = (0, 0, 0);
        for index in order {
            let size = charts[index].size;
            if x + size.x > width {
                (x, y, shelf) = (0, y + shelf, 0);
            }
            charts[index].at = UVec2::new(x, y);
            x += size.x;
            shelf = shelf.max(size.y);
        }
        Atlas { charts, chart_of, size: UVec2::new(width, (y + shelf).max(1)) }
    }

    /// The chart face `index` is on.
    pub fn chart(&self, index: usize) -> &Chart {
        &self.charts[self.chart_of[index]]
    }

    /// Where `point`, on face `index`, is in the atlas, 0 to 1.
    pub fn uv(&self, index: usize, point: Vec3) -> Vec2 {
        let chart = self.chart(index);
        (chart.lattice(point) - chart.origin.as_vec2() + chart.at.as_vec2()) / self.size.as_vec2()
    }

    /// Light per texel, row by row from the top of the atlas. `sky` is per face: a sky face is not
    /// lightmapped, and its texels are left at the base.
    ///
    /// Only texels within [`REACH`] of a face are baked, each sampled on the nearest face of its
    /// chart; the rest of a chart is never read and stays at the base.
    pub fn bake(&self, faces: &[Face], sky: &[bool], surface: &LitSurface, light: &Daylight) -> Vec<Vec3> {
        let mut texels = vec![Vec3::splat(light.base); (self.size.x * self.size.y) as usize];
        for chart in &self.charts {
            let lit: Vec<usize> = chart.faces.iter().copied().filter(|&index| !sky[index]).collect();
            if lit.is_empty() {
                continue;
            }
            let normal = chart.normal;
            let n_dot_l = normal.dot(light.toward).max(0.0);
            let outlines: Vec<Vec<Vec2>> =
                lit.iter().map(|&index| faces[index].vertices.iter().map(|p| chart.lattice(*p)).collect()).collect();
            let sample_on = |k: usize, at: Vec2| on_plane(&faces[lit[k]], chart, inside(&outlines[k], at));
            let nearest_face = |at: Vec2| {
                (0..lit.len())
                    .map(|k| (k, distance_to(&outlines[k], at)))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map_or(0, |(k, _)| k)
            };

            // Which face each texel is sampled on, if it is near enough one to be read.
            let width = chart.size.x as usize;
            let mut nearest: Vec<Option<(usize, f32)>> = vec![None; width * chart.size.y as usize];
            for (k, outline) in outlines.iter().enumerate() {
                let (lo, hi) = outline.iter().fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
                let lo = ((lo - REACH).floor().as_ivec2() - chart.origin).max(IVec2::ZERO);
                let hi = ((hi + REACH).ceil().as_ivec2() - chart.origin).min(chart.size.as_ivec2() - 1);
                for j in lo.y..=hi.y {
                    for i in lo.x..=hi.x {
                        let center = (chart.origin + IVec2::new(i, j)).as_vec2() + 0.5;
                        let distance = distance_to(outline, center);
                        let slot = &mut nearest[j as usize * width + i as usize];
                        if distance <= REACH && slot.is_none_or(|(_, best)| distance < best) {
                            *slot = Some((k, distance));
                        }
                    }
                }
            }

            // The gather grid: sample `k` sits at `k * AMBIENT_STEP + AMBIENT_STEP / 2`, enough
            // of them that every texel center has one either side. Gathered only where a baked
            // texel reads it.
            let half = AMBIENT_STEP / 2.0;
            let first = chart.origin.as_vec2() + 0.5;
            let last = first + (chart.size.as_vec2() - 1.0);
            let k_lo = ((first - half) / AMBIENT_STEP).floor().as_ivec2();
            let k_hi = ((last - half) / AMBIENT_STEP).floor().as_ivec2() + 1;
            let columns = (k_hi.x - k_lo.x + 1) as usize;
            let mut gathered: Vec<Option<Vec3>> = vec![None; columns * (k_hi.y - k_lo.y + 1) as usize];
            let mut gather = |x: usize, y: usize| {
                *gathered[y * columns + x].get_or_insert_with(|| {
                    let at = (IVec2::new(x as i32, y as i32) + k_lo).as_vec2() * AMBIENT_STEP + half;
                    surface.ambient(sample_on(nearest_face(at), at), normal, light)
                })
            };

            for j in 0..chart.size.y {
                for i in 0..chart.size.x {
                    let Some((k, _)) = nearest[j as usize * width + i as usize] else { continue };
                    let center = chart.origin.as_vec2() + Vec2::new(i as f32, j as f32) + 0.5;
                    let sun = if n_dot_l > 0.0 { surface.sun_reaching(sample_on(k, center), normal, light.toward) } else { 0.0 };
                    let f = (center - half) / AMBIENT_STEP - k_lo.as_vec2();
                    let (cell, t) = (f.floor(), f.fract());
                    let (x, y) = (cell.x as usize, cell.y as usize);
                    let low = gather(x, y).lerp(gather(x + 1, y), t.x);
                    let high = gather(x, y + 1).lerp(gather(x + 1, y + 1), t.x);
                    let at = (chart.at.y + j) * self.size.x + chart.at.x + i;
                    texels[at as usize] = light.base + light.sun * n_dot_l * sun + low.lerp(high, t.y);
                }
            }
        }
        texels
    }
}

impl Atlas {
    /// `old`'s light, baked for an earlier shape of the room, laid onto this atlas without casting a
    /// ray: so an edit leaves the light it had until a bake replaces it. Charts in one plane share
    /// a lattice, so a texel is copied from the old chart in the same plane where there is one, else
    /// from the nearest parallel one — a dragged wall keeps its light — and clamped onto that
    /// chart's edge where it runs past it. Surface with nothing parallel before is the base.
    pub fn carry_over(&self, old: &Atlas, old_light: &[Vec3], base: f32) -> Vec<Vec3> {
        let mut light = vec![Vec3::splat(base); (self.size.x * self.size.y) as usize];
        for chart in &self.charts {
            let mut parallel: Vec<&Chart> =
                old.charts.iter().filter(|other| other.normal.dot(chart.normal) > 0.999).collect();
            if parallel.is_empty() {
                continue;
            }
            parallel.sort_by(|a, b| (a.w - chart.w).abs().total_cmp(&(b.w - chart.w).abs()));
            let nearest = (parallel[0].w - chart.w).abs();
            parallel.retain(|other| (other.w - chart.w).abs() <= nearest + 1e-3);
            for j in 0..chart.size.y {
                for i in 0..chart.size.x {
                    let at = chart.origin + IVec2::new(i as i32, j as i32);
                    let (from, offset) = parallel
                        .iter()
                        .map(|other| {
                            let offset = (at - other.origin).clamp(IVec2::ZERO, other.size.as_ivec2() - 1);
                            (*other, offset, (at - other.origin - offset).length_squared())
                        })
                        .min_by_key(|(_, _, off_by)| *off_by)
                        .map(|(other, offset, _)| (other, offset.as_uvec2()))
                        .expect("there is a parallel chart");
                    let source = (from.at.y + offset.y) * old.size.x + from.at.x + offset.x;
                    let target = (chart.at.y + j) * self.size.x + chart.at.x + i;
                    light[target as usize] = old_light[source as usize];
                }
            }
        }
        light
    }
}

/// What a room's faces are, for telling whether a layout or a bake is still theirs. Stable across
/// builds, since it keys the lightmap cache.
pub fn faces_hash(faces: &[Face]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = crate::common::stable_hash::Fnv::default();
    for face in faces {
        face.vertices.len().hash(&mut hasher);
        for vertex in &face.vertices {
            vertex.to_array().map(f32::to_bits).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// The point of `face`'s plane at lattice coordinates `at`. The lattice axes need not lie in the
/// plane (a sloped floor), so this solves along their normal.
fn on_plane(face: &Face, chart: &Chart, at: Vec2) -> Vec3 {
    let (a, b) = (at.x * TEXEL, at.y * TEXEL);
    let w = chart.u.cross(chart.v);
    let normal = face.plane.normal;
    let k = (face.plane.w - a * normal.dot(chart.u) - b * normal.dot(chart.v)) / normal.dot(w);
    chart.u * a + chart.v * b + w * k
}

/// `point` if it is inside the convex `outline`, else the nearest point of it, pulled [`INSET`]
/// toward the middle.
fn inside(outline: &[Vec2], point: Vec2) -> Vec2 {
    let count = outline.len();
    let turn = |a: Vec2, b: Vec2, p: Vec2| (b - a).perp_dot(p - a);
    let winding: f32 = (0..count).map(|i| outline[i].perp_dot(outline[(i + 1) % count])).sum();
    if (0..count).all(|i| turn(outline[i], outline[(i + 1) % count], point) * winding >= 0.0) {
        return point;
    }
    let nearest = (0..count)
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % count]);
            let t = ((point - a).dot(b - a) / (b - a).length_squared().max(f32::EPSILON)).clamp(0.0, 1.0);
            a + (b - a) * t
        })
        .min_by(|x, y| x.distance_squared(point).total_cmp(&y.distance_squared(point)))
        .unwrap_or(point);
    let middle = outline.iter().copied().sum::<Vec2>() / count as f32;
    nearest + (middle - nearest).normalize_or_zero() * INSET
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::sky::SunPosition;
    use crate::common::world_surface::room_share;
    use crate::tool::room::{Room, RoomSlot};

    #[test]
    fn no_two_charts_overlap_and_every_one_fits() {
        let rooms = [Room::new(Vec3::new(0.3, 0.0, -1.7), Vec3::new(9.1, 3.3, 5.2))];
        let atlas = Atlas::of(&room_share(&rooms, 0));
        let rect = |c: &Chart| (c.at, c.at + c.size);
        for (i, a) in atlas.charts.iter().enumerate() {
            let (lo, hi) = rect(a);
            assert!(hi.cmple(atlas.size).all(), "chart {i} runs off the atlas");
            for b in &atlas.charts[i + 1..] {
                let (blo, bhi) = rect(b);
                assert!(hi.x <= blo.x || bhi.x <= lo.x || hi.y <= blo.y || bhi.y <= lo.y, "charts overlap");
            }
        }
    }

    /// Every vertex lands inside its own chart, and not in its padding.
    #[test]
    fn a_faces_uvs_stay_in_its_chart() {
        let rooms = [Room::new(Vec3::ZERO, Vec3::new(5.0, 3.0, 2.5))];
        let faces = room_share(&rooms, 0);
        let atlas = Atlas::of(&faces);
        for (index, face) in faces.iter().enumerate() {
            let chart = atlas.chart(index);
            let lo = (chart.at.as_vec2() + PADDING as f32) / atlas.size.as_vec2();
            let hi = (chart.at + chart.size).as_vec2() / atlas.size.as_vec2() - PADDING as f32 / atlas.size.as_vec2();
            for vertex in &face.vertices {
                let uv = atlas.uv(index, *vertex);
                assert!(uv.cmpge(lo - 1e-5).all() && uv.cmple(hi + 1e-5).all(), "{uv} outside {lo}..{hi}");
            }
        }
    }

    /// Two rooms' floors meet at a threshold: the lattice is the plane's, so a texel either side
    /// of the seam is sampled at the same place it would be in one big floor.
    #[test]
    fn floors_across_a_threshold_share_a_lattice() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::new(3.3, 3.0, 4.0)),
            Room::new(Vec3::new(3.3, 0.0, 0.0), Vec3::new(7.0, 3.0, 4.0)),
        ];
        let floor = |index| {
            let faces = room_share(&rooms, index);
            let at = faces.iter().position(|f| RoomSlot::of(f.plane.normal) == RoomSlot::Floor).unwrap();
            Atlas::of(&faces).chart(at).clone()
        };
        let (a, b) = (floor(0), floor(1));
        let world = |chart: &Chart, i: i32| (chart.origin.x + i) as f32 * TEXEL;
        // The texel holding x = 3.3 in each.
        let column = |chart: &Chart| (3.3 / TEXEL).floor() as i32 - chart.origin.x;
        assert_eq!(world(&a, column(&a)), world(&b, column(&b)));
    }

    /// An L-shaped floor comes out of the kernel in several pieces, and they are one chart: a
    /// chart per piece would be a bounding box per sliver, each baked whole.
    #[test]
    fn a_floor_in_pieces_is_one_chart() {
        let footprint = vec![Vec2::ZERO, Vec2::new(6.0, 0.0), Vec2::new(6.0, 2.0), Vec2::new(2.0, 2.0), Vec2::new(2.0, 6.0), Vec2::new(0.0, 6.0)];
        let faces = room_share(&[Room::prism(footprint, 0.0, 3.0)], 0);
        let floors: Vec<usize> = (0..faces.len()).filter(|&i| RoomSlot::of(faces[i].plane.normal) == RoomSlot::Floor).collect();
        assert!(floors.len() > 1, "the floor came out whole, so this tests nothing");
        let atlas = Atlas::of(&faces);
        assert!(floors.iter().all(|&i| atlas.chart(i) == atlas.chart(floors[0])));
    }

    /// The light baked on room `index`'s floor at `(x, z)`, with the ceilings of `open` rooms sky.
    fn floor_light(rooms: &[Room], open: &[usize], light: &Daylight, index: usize, x: f32, z: f32) -> f32 {
        let is_sky = |room: usize, slot: RoomSlot| open.contains(&room) && slot == RoomSlot::Ceiling;
        let surface = LitSurface::build(rooms, is_sky);
        let faces = room_share(rooms, index);
        let sky: Vec<bool> = faces.iter().map(|f| is_sky(index, RoomSlot::of(f.plane.normal))).collect();
        let atlas = Atlas::of(&faces);
        let texels = atlas.bake(&faces, &sky, &surface, light);
        let at = faces.iter().position(|f| RoomSlot::of(f.plane.normal) == RoomSlot::Floor).unwrap();
        let chart = atlas.chart(at);
        let texel = (Vec2::new(x, z) / TEXEL).floor().as_ivec2() - chart.origin;
        let texel = chart.at + texel.as_uvec2();
        texels[(texel.y * atlas.size.x + texel.x) as usize].x
    }

    fn day(altitude: f32, azimuth: f32) -> Daylight {
        daylight(SunPosition { altitude, azimuth }.toward(), SkyTime::Day, Brightness::default())
    }

    /// A courtyard open to the sky with a low room off its east side, as in `sunlight`'s tests.
    fn courtyard() -> [Room; 2] {
        [Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)), Room::new(Vec3::new(4.0, 0.0, 0.0), Vec3::new(8.0, 2.0, 4.0))]
    }

    #[test]
    fn a_floor_under_a_sky_is_brighter_than_one_under_a_roof() {
        let rooms = [Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)), Room::new(Vec3::new(10.0, 0.0, 0.0), Vec3::new(14.0, 3.0, 4.0))];
        let light = day(70.0, 10.0);
        let open = floor_light(&rooms, &[0], &light, 0, 2.0, 2.0);
        assert!(open > BASE_AMBIENT + SUN_BRIGHTNESS * 0.9, "{open}");
        // Nothing gets into a closed room: no sun, no sky, nothing sunlit to bounce off.
        assert_eq!(floor_light(&rooms, &[0], &light, 1, 12.0, 2.0), BASE_AMBIENT);
    }

    /// The far end of the low room sees no sun, but sees out into the sunlit courtyard: a little
    /// from the far end, where the doorway is a small part of the view, and more near it.
    #[test]
    fn a_room_off_a_sunlit_courtyard_is_lit_by_it() {
        let light = day(45.0, 270.0);
        let deep = floor_light(&courtyard(), &[0], &light, 1, 7.5, 2.0);
        let nearer = floor_light(&courtyard(), &[0], &light, 1, 6.5, 2.0);
        assert!(deep > BASE_AMBIENT + 1.0, "{deep}");
        assert!(nearer > deep, "{nearer} nearer the doorway, {deep} further in");
        let roofed = floor_light(&courtyard(), &[], &light, 1, 7.5, 2.0);
        assert_eq!(roofed, BASE_AMBIENT);
    }

    #[test]
    fn a_night_sky_lights_less_than_a_day_one() {
        let toward = SunPosition { altitude: 45.0, azimuth: 270.0 }.toward();
        // Out of the sun, so only the sky differs.
        let at = |sky| floor_light(&courtyard(), &[0], &daylight(toward, sky, Brightness::default()), 0, 1.0, 0.3);
        assert!(at(SkyTime::Night) < at(SkyTime::Day), "{} {}", at(SkyTime::Night), at(SkyTime::Day));
    }

    /// One sky over two rooms whose floors meet: the light either side of the seam agrees, because
    /// the rays do not know which room they started in.
    #[test]
    fn light_runs_on_across_a_threshold() {
        let rooms = [Room::new(Vec3::ZERO, Vec3::new(3.3, 3.0, 4.0)), Room::new(Vec3::new(3.3, 0.0, 0.0), Vec3::new(7.0, 3.0, 4.0))];
        let light = day(60.0, 0.0);
        let left = floor_light(&rooms, &[0, 1], &light, 0, 3.2, 2.0);
        let right = floor_light(&rooms, &[0, 1], &light, 1, 3.4, 2.0);
        assert!((left - right).abs() / left < 0.02, "{left} against {right}");
    }

    /// A room is made a meter longer. Nothing is baked again: the floor keeps its light where it
    /// was, and the wall that moved keeps the light the old one had at the same height.
    #[test]
    fn a_reshaped_room_carries_its_light_over() {
        let light = day(40.0, 250.0);
        let bake = |room: Room| {
            let rooms = [room];
            let faces = room_share(&rooms, 0);
            let sky: Vec<bool> = faces.iter().map(|f| RoomSlot::of(f.plane.normal) == RoomSlot::Ceiling).collect();
            let surface = LitSurface::build(&rooms, |_, slot| slot == RoomSlot::Ceiling);
            let atlas = Atlas::of(&faces);
            let texels = atlas.bake(&faces, &sky, &surface, &light);
            (faces, atlas, texels)
        };
        let at = |faces: &[Face], atlas: &Atlas, texels: &[Vec3], normal: Vec3, point: Vec3| {
            let index = faces.iter().position(|f| f.plane.normal.abs_diff_eq(normal, 1e-5)).unwrap();
            let chart = atlas.chart(index);
            let texel = chart.at + (chart.lattice(point).floor().as_ivec2() - chart.origin).as_uvec2();
            texels[(texel.y * atlas.size.x + texel.x) as usize]
        };

        let (old_faces, old_atlas, old_light) = bake(Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)));
        let (faces, atlas, _) = bake(Room::new(Vec3::ZERO, Vec3::new(5.0, 3.0, 4.0)));
        let carried = atlas.carry_over(&old_atlas, &old_light, BASE_AMBIENT);

        let floor = Vec3::new(1.1, 0.0, 2.1);
        assert_eq!(at(&faces, &atlas, &carried, Vec3::Y, floor), at(&old_faces, &old_atlas, &old_light, Vec3::Y, floor));
        let (was, now) = (Vec3::new(4.0, 1.1, 2.1), Vec3::new(5.0, 1.1, 2.1));
        assert_eq!(
            at(&faces, &atlas, &carried, Vec3::NEG_X, now),
            at(&old_faces, &old_atlas, &old_light, Vec3::NEG_X, was),
            "the wall that moved lost its light"
        );
    }

    #[test]
    fn a_closed_room_is_the_maps_ambient() {
        let rooms = [Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))];
        let night = Brightness { ambient: 12.5, sun: 200.0, sky: 40.0 };
        let light = daylight(SunPosition { altitude: 70.0, azimuth: 10.0 }.toward(), SkyTime::Night, night);
        assert_eq!(floor_light(&rooms, &[], &light, 0, 2.0, 2.0), 12.5);
    }
}
