//! Lightmaps for the map's surface: a room's faces charted onto a lattice of [`TEXEL`] squares and
//! packed into one atlas per room. See documentation/sun.md.
//!
//! The lattice depends only on a face's plane, never on the face, so two rooms' floors meeting at
//! a threshold sample the same points and bake the same values on both sides of the seam. The room
//! bake writes `UV_1` from an [`Atlas`] and the sun bake fills one from the same faces; both call
//! [`Atlas::of`], which is deterministic, so neither has to hold the other's layout.

use bevy::prelude::*;

use crate::common::sky::{Brightness, SkyTime};
use crate::common::sunlight::{Daylight, LitSurface, GATHER_RAYS, SUN_RAYS};
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

/// The faces in one plane that touch, chained; each cluster ascending, the clusters by their first
/// face. Faces that are only near stay apart, so wall pieces either side of a block keep their own
/// padding. `outlines` are in lattice units.
fn clusters(faces: &[Face], outlines: &[Vec<Vec2>], bounds: &[(Vec2, Vec2)]) -> Vec<Vec<usize>> {
    /// Lattice units: a vertex this near another face's edge is on it.
    const TOUCHING: f32 = 0.01;
    let mut parent: Vec<usize> = (0..faces.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut planes: Vec<Vec<usize>> = Vec::new();
    for (index, face) in faces.iter().enumerate() {
        let coplanar = |other: &usize| {
            let plane = faces[*other].plane;
            plane.normal.dot(face.plane.normal) > 0.9999 && (plane.w - face.plane.w).abs() < 1e-3
        };
        match planes.iter_mut().find(|members| coplanar(&members[0])) {
            Some(members) => members.push(index),
            None => planes.push(vec![index]),
        }
    }
    for mut members in planes {
        members.sort_by(|a, b| bounds[*a].0.x.total_cmp(&bounds[*b].0.x));
        for (at, &a) in members.iter().enumerate() {
            for &b in &members[at + 1..] {
                if bounds[b].0.x > bounds[a].1.x + TOUCHING {
                    break;
                }
                let overlapping = (bounds[a].0 - TOUCHING).cmple(bounds[b].1).all() && (bounds[b].0 - TOUCHING).cmple(bounds[a].1).all();
                let touch = |from: usize, to: usize| outlines[from].iter().any(|v| distance_to(&outlines[to], *v) <= TOUCHING);
                if overlapping && (touch(a, b) || touch(b, a)) {
                    let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                    parent[ra.max(rb)] = ra.min(rb);
                }
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
    nearest_on_outline(outline, point).map_or(0.0, |nearest| nearest.distance(point))
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
    /// By face index.
    chart_of: Vec<usize>,
    pub size: UVec2,
}

impl Atlas {
    /// Faces in one plane that touch share a chart, so a floor cut into slivers by
    /// the kernel is one rectangle rather than a bounding box per sliver. Shelf-packed, tallest
    /// first.
    pub fn of(faces: &[Face]) -> Atlas {
        let outlines: Vec<Vec<Vec2>> = faces
            .iter()
            .map(|face| {
                let (u, v) = axes(face.plane.normal);
                face.vertices.iter().map(|p| Vec2::new(p.dot(u), p.dot(v)) / TEXEL).collect()
            })
            .collect();
        let bounds: Vec<(Vec2, Vec2)> = outlines
            .iter()
            .map(|outline| outline.iter().fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
            .collect();
        let mut charts: Vec<Chart> = clusters(faces, &outlines, &bounds)
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

    pub fn chart(&self, index: usize) -> &Chart {
        &self.charts[self.chart_of[index]]
    }

    /// Where `point`, on face `index`, is in the atlas, 0 to 1.
    pub fn uv(&self, index: usize, point: Vec3) -> Vec2 {
        let chart = self.chart(index);
        (chart.lattice(point) - chart.origin.as_vec2() + chart.at.as_vec2()) / self.size.as_vec2()
    }

    /// Light per texel, row by row from the top of the atlas.
    #[cfg(test)]
    pub fn bake(&self, faces: &[Face], sky: &[bool], surface: &LitSurface, light: &Daylight) -> Vec<Vec3> {
        let mut bake = LightmapBake::plan(self.clone(), faces, sky, light);
        while !bake.is_done() {
            bake.step(faces, surface, light);
        }
        bake.finish().1
    }
}

/// A room's lightmap, planned before any ray is cast and baked a row of a chart at a time, so a
/// bake can say how far through a room it is. `sky` is per face: a sky face is not lightmapped.
///
/// Only texels within [`REACH`] of a face are baked, each sampled on the nearest face of its
/// chart; the rest of a chart is never read and stays at the base. Gather samples are taken only
/// where a baked texel reads them.
pub struct LightmapBake {
    atlas: Atlas,
    charts: Vec<ChartPlan>,
    texels: Vec<Vec3>,
    /// The chart and row baked next.
    chart: usize,
    row: u32,
    /// The current chart's gather samples, as they are taken.
    gathered: Vec<Option<Vec3>>,
    rays: usize,
    cast: usize,
}

struct ChartPlan {
    /// Into the atlas's charts.
    chart: usize,
    /// Its faces that are not sky; `nearest` and `outlines` index this.
    lit: Vec<usize>,
    outlines: Vec<Vec<Vec2>>,
    /// Which face each texel is sampled on, if it is near enough one to be read.
    nearest: Vec<Option<u32>>,
    n_dot_l: f32,
    /// The gather lattice's first sample and width. Sample `k` sits at
    /// `k * AMBIENT_STEP + AMBIENT_STEP / 2`, enough of them that every texel center has one either
    /// side.
    k_lo: IVec2,
    columns: usize,
    samples: usize,
}

impl LightmapBake {
    pub fn plan(atlas: Atlas, faces: &[Face], sky: &[bool], light: &Daylight) -> Self {
        let mut rays = 0;
        let charts = atlas
            .charts
            .iter()
            .enumerate()
            .filter_map(|(index, chart)| {
                let lit: Vec<usize> = chart.faces.iter().copied().filter(|&face| !sky[face]).collect();
                if lit.is_empty() {
                    return None;
                }
                let outlines: Vec<Vec<Vec2>> =
                    lit.iter().map(|&face| faces[face].vertices.iter().map(|p| chart.lattice(*p)).collect()).collect();
                let width = chart.size.x as usize;
                let mut nearest: Vec<Option<(u32, f32)>> = vec![None; width * chart.size.y as usize];
                for (k, outline) in outlines.iter().enumerate() {
                    let (lo, hi) =
                        outline.iter().fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
                    let lo = ((lo - REACH).floor().as_ivec2() - chart.origin).max(IVec2::ZERO);
                    let hi = ((hi + REACH).ceil().as_ivec2() - chart.origin).min(chart.size.as_ivec2() - 1);
                    for j in lo.y..=hi.y {
                        for i in lo.x..=hi.x {
                            let center = (chart.origin + IVec2::new(i, j)).as_vec2() + 0.5;
                            let distance = distance_to(outline, center);
                            let slot = &mut nearest[j as usize * width + i as usize];
                            if distance <= REACH && slot.is_none_or(|(_, best)| distance < best) {
                                *slot = Some((k as u32, distance));
                            }
                        }
                    }
                }
                let nearest: Vec<Option<u32>> = nearest.into_iter().map(|slot| slot.map(|(k, _)| k)).collect();

                let half = AMBIENT_STEP / 2.0;
                let first = chart.origin.as_vec2() + 0.5;
                let last = first + (chart.size.as_vec2() - 1.0);
                let k_lo = ((first - half) / AMBIENT_STEP).floor().as_ivec2();
                let k_hi = ((last - half) / AMBIENT_STEP).floor().as_ivec2() + 1;
                let columns = (k_hi.x - k_lo.x + 1) as usize;
                let samples = columns * (k_hi.y - k_lo.y + 1) as usize;
                let n_dot_l = chart.normal.dot(light.toward).max(0.0);

                let mut wanted = vec![false; samples];
                let mut baked = 0;
                for (at, slot) in nearest.iter().enumerate() {
                    if slot.is_none() {
                        continue;
                    }
                    baked += 1;
                    let center = chart.origin.as_vec2() + Vec2::new((at % width) as f32, (at / width) as f32) + 0.5;
                    let (x, y) = gather_cell(center, k_lo);
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        wanted[(y + dy) * columns + x + dx] = true;
                    }
                }
                let sun_rays = if n_dot_l > 0.0 { baked * SUN_RAYS } else { 0 };
                rays += sun_rays + wanted.iter().filter(|w| **w).count() * GATHER_RAYS;
                Some(ChartPlan { chart: index, lit, outlines, nearest, n_dot_l, k_lo, columns, samples })
            })
            .collect::<Vec<_>>();
        let texels = vec![Vec3::splat(light.base); (atlas.size.x * atlas.size.y) as usize];
        let gathered = vec![None; charts.first().map_or(0, |plan: &ChartPlan| plan.samples)];
        LightmapBake { atlas, charts, texels, chart: 0, row: 0, gathered, rays, cast: 0 }
    }

    /// The rays the bake casts at the sun and across hemispheres, not counting the one ray from each
    /// surface a gather ray meets that asks whether the sun is on it.
    pub fn rays(&self) -> usize {
        self.rays
    }

    /// Of [`Self::rays`], those cast so far.
    pub fn cast(&self) -> usize {
        self.cast
    }

    pub fn is_done(&self) -> bool {
        self.chart >= self.charts.len()
    }

    /// Bakes the next row of a chart with anything on it to bake.
    pub fn step(&mut self, faces: &[Face], surface: &LitSurface, light: &Daylight) {
        while let Some(plan) = self.charts.get(self.chart) {
            let chart = &self.atlas.charts[plan.chart];
            if self.row >= chart.size.y {
                self.chart += 1;
                self.row = 0;
                self.gathered = vec![None; self.charts.get(self.chart).map_or(0, |plan| plan.samples)];
                continue;
            }
            let (j, width) = (self.row, chart.size.x as usize);
            self.row += 1;
            let row = &plan.nearest[j as usize * width..(j as usize + 1) * width];
            if row.iter().all(Option::is_none) {
                continue;
            }
            let sample_on = |k: usize, at: Vec2| on_plane(&faces[plan.lit[k]], chart, inside(&plan.outlines[k], at));
            let nearest_face = |at: Vec2| {
                (0..plan.lit.len())
                    .map(|k| (k, distance_to(&plan.outlines[k], at)))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map_or(0, |(k, _)| k)
            };
            let (gathered, cast) = (&mut self.gathered, &mut self.cast);
            let mut gather = |x: usize, y: usize| {
                *gathered[y * plan.columns + x].get_or_insert_with(|| {
                    *cast += GATHER_RAYS;
                    let at = (IVec2::new(x as i32, y as i32) + plan.k_lo).as_vec2() * AMBIENT_STEP + AMBIENT_STEP / 2.0;
                    surface.ambient(sample_on(nearest_face(at), at), chart.normal, light)
                })
            };
            let mut sun_rays = 0;
            for (i, slot) in row.iter().enumerate() {
                let Some(k) = slot else { continue };
                let center = chart.origin.as_vec2() + Vec2::new(i as f32, j as f32) + 0.5;
                let sun = if plan.n_dot_l > 0.0 {
                    sun_rays += SUN_RAYS;
                    surface.sun_reaching(sample_on(*k as usize, center), chart.normal, light.toward)
                } else {
                    0.0
                };
                let (x, y) = gather_cell(center, plan.k_lo);
                let t = gather_fraction(center, plan.k_lo);
                let low = gather(x, y).lerp(gather(x + 1, y), t.x);
                let high = gather(x, y + 1).lerp(gather(x + 1, y + 1), t.x);
                let at = (chart.at.y + j) * self.atlas.size.x + chart.at.x + i as u32;
                self.texels[at as usize] = light.base + light.sun * plan.n_dot_l * sun + low.lerp(high, t.y);
            }
            self.cast += sun_rays;
            return;
        }
    }

    /// Every texel left unbaked takes the nearest baked one in its chart: nothing samples them, but
    /// [`Atlas::carry_over`] copies whole charts onto a reshaped room.
    pub fn finish(mut self) -> (Atlas, Vec<Vec3>) {
        for plan in &self.charts {
            let chart = &self.atlas.charts[plan.chart];
            let (width, height) = (chart.size.x as usize, chart.size.y as usize);
            let mut from: Vec<Option<usize>> = plan.nearest.iter().enumerate().map(|(at, k)| k.map(|_| at)).collect();
            let mut queue: std::collections::VecDeque<usize> = (0..from.len()).filter(|&at| from[at].is_some()).collect();
            while let Some(at) = queue.pop_front() {
                let (i, j) = (at % width, at / width);
                let neighbors = [(i > 0).then(|| at - 1), (i + 1 < width).then(|| at + 1), (j > 0).then(|| at - width), (j + 1 < height).then(|| at + width)];
                for next in neighbors.into_iter().flatten() {
                    if from[next].is_none() {
                        from[next] = from[at];
                        queue.push_back(next);
                    }
                }
            }
            let in_atlas = |at: usize| ((chart.at.y as usize + at / width) * self.atlas.size.x as usize) + chart.at.x as usize + at % width;
            for (at, source) in from.iter().enumerate() {
                if let (None, Some(source)) = (plan.nearest[at], source) {
                    self.texels[in_atlas(at)] = self.texels[in_atlas(*source)];
                }
            }
        }
        (self.atlas, self.texels)
    }
}

/// The gather sample at or before `center` in each axis, from the lattice's first.
fn gather_cell(center: Vec2, k_lo: IVec2) -> (usize, usize) {
    let cell = ((center - AMBIENT_STEP / 2.0) / AMBIENT_STEP - k_lo.as_vec2()).floor();
    (cell.x as usize, cell.y as usize)
}

/// How far `center` is from its [`gather_cell`] to the next, 0 to 1 in each axis.
fn gather_fraction(center: Vec2, k_lo: IVec2) -> Vec2 {
    ((center - AMBIENT_STEP / 2.0) / AMBIENT_STEP - k_lo.as_vec2()).fract()
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
    let Some(nearest) = nearest_on_outline(outline, point) else { return point };
    let middle = outline.iter().copied().sum::<Vec2>() / outline.len() as f32;
    nearest + (middle - nearest).normalize_or_zero() * INSET
}

/// `None` if `point` is inside the convex `outline`, else the nearest point of its edge.
fn nearest_on_outline(outline: &[Vec2], point: Vec2) -> Option<Vec2> {
    let count = outline.len();
    let edge = |i: usize| (outline[i], outline[(i + 1) % count]);
    let winding: f32 = (0..count).map(|i| outline[i].perp_dot(outline[(i + 1) % count])).sum();
    if (0..count).all(|i| {
        let (a, b) = edge(i);
        (b - a).perp_dot(point - a) * winding >= 0.0
    }) {
        return None;
    }
    (0..count)
        .map(|i| {
            let (a, b) = edge(i);
            let t = ((point - a).dot(b - a) / (b - a).length_squared().max(f32::EPSILON)).clamp(0.0, 1.0);
            a + (b - a) * t
        })
        .min_by(|x, y| x.distance_squared(point).total_cmp(&y.distance_squared(point)))
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

    /// Casts exactly the rays its plan counted, over more steps than the room has faces.
    #[test]
    fn a_room_casts_the_rays_it_planned_a_row_at_a_time() {
        let rooms = courtyard();
        let surface = LitSurface::build(&rooms, |room, slot| room == 0 && slot == RoomSlot::Ceiling);
        let faces = room_share(&rooms, 0);
        let sky: Vec<bool> = faces.iter().map(|f| RoomSlot::of(f.plane.normal) == RoomSlot::Ceiling).collect();
        let light = day(50.0, 135.0);
        let mut bake = LightmapBake::plan(Atlas::of(&faces), &faces, &sky, &light);
        assert!(bake.rays() > 0);
        let (mut steps, mut last) = (0, 0);
        while !bake.is_done() {
            bake.step(&faces, &surface, &light);
            assert!(bake.cast() >= last && bake.cast() <= bake.rays(), "{} after {last}, of {}", bake.cast(), bake.rays());
            last = bake.cast();
            steps += 1;
        }
        assert_eq!(bake.cast(), bake.rays());
        assert!(steps > faces.len(), "{steps} steps for {} faces", faces.len());
    }

    /// The kernel cuts an L-shaped floor into pieces; they share one chart, or each sliver bakes its
    /// whole bounding box.
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
