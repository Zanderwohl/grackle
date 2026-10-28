//! Lightmaps for the map's surface: a room's faces charted onto a lattice of [`TEXEL`] squares and
//! packed into one atlas per room. See documentation/sun.md.
//!
//! The lattice depends only on a face's plane, never on the face, so two rooms' floors meeting at
//! a threshold sample the same points and bake the same values on both sides of the seam. The room
//! bake writes `UV_1` from an [`Atlas`] and the sun bake fills one from the same faces; both call
//! [`Atlas::of`], which is deterministic, so neither has to hold the other's layout.

use bevy::prelude::*;

use crate::common::sunlight::LitSurface;
use crate::common::world_surface::Face;

/// Meters per lightmap texel.
pub const TEXEL: f32 = 0.25;

/// What a texel no light reaches is lit with, in `GlobalAmbientLight::brightness` units: a
/// lightmap replaces the ambient term on the meshes that carry one.
pub const BASE_AMBIENT: f32 = 100.0;

/// What the sun adds to a surface square on to it, in the same units.
pub const SUN_BRIGHTNESS: f32 = 1500.0;

/// Texels round a chart sampled at its face's edge, so bilinear filtering at the edge reads the
/// edge's own light rather than a neighboring chart's.
const PADDING: i32 = 1;

/// How far inside its face a texel past the edge is sampled, in lattice units: off the corner,
/// where a ray would start inside the wall it meets.
const INSET: f32 = 0.08;

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
    /// The lattice square at the chart's first texel.
    origin: IVec2,
    pub size: UVec2,
    /// Where the chart's first texel is in the atlas.
    at: UVec2,
}

impl Chart {
    fn lattice(&self, point: Vec3) -> Vec2 {
        Vec2::new(point.dot(self.u), point.dot(self.v)) / TEXEL
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Atlas {
    /// One per face, in the faces' order.
    pub charts: Vec<Chart>,
    pub size: UVec2,
}

impl Atlas {
    /// Shelf-packed, tallest first.
    pub fn of(faces: &[Face]) -> Atlas {
        let mut charts: Vec<Chart> = faces
            .iter()
            .map(|face| {
                let (u, v) = axes(face.plane.normal);
                let mut chart = Chart { u, v, origin: IVec2::ZERO, size: UVec2::ZERO, at: UVec2::ZERO };
                let (lo, hi) = face.vertices.iter().map(|p| chart.lattice(*p)).fold(
                    (Vec2::INFINITY, Vec2::NEG_INFINITY),
                    |(lo, hi), p| (lo.min(p), hi.max(p)),
                );
                chart.origin = lo.floor().as_ivec2() - PADDING;
                chart.size = (hi.ceil().as_ivec2() + PADDING - chart.origin).max(IVec2::ONE).as_uvec2();
                chart
            })
            .collect();

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
        Atlas { charts, size: UVec2::new(width, (y + shelf).max(1)) }
    }

    /// Where `point`, on face `index`, is in the atlas, 0 to 1.
    pub fn uv(&self, index: usize, point: Vec3) -> Vec2 {
        let chart = &self.charts[index];
        (chart.lattice(point) - chart.origin.as_vec2() + chart.at.as_vec2()) / self.size.as_vec2()
    }

    /// Light per texel, row by row from the top of the atlas. `sky` is per face: a sky face is not
    /// lightmapped, and its texels are left at the base.
    pub fn bake(&self, faces: &[Face], sky: &[bool], surface: &LitSurface, toward: Vec3) -> Vec<Vec3> {
        let mut texels = vec![Vec3::splat(BASE_AMBIENT); (self.size.x * self.size.y) as usize];
        for (index, (face, chart)) in faces.iter().zip(&self.charts).enumerate() {
            if sky[index] {
                continue;
            }
            let normal = face.plane.normal;
            let n_dot_l = normal.dot(toward).max(0.0);
            let outline: Vec<Vec2> = face.vertices.iter().map(|p| chart.lattice(*p)).collect();
            for j in 0..chart.size.y {
                for i in 0..chart.size.x {
                    let center = chart.origin.as_vec2() + Vec2::new(i as f32, j as f32) + 0.5;
                    let point = on_plane(face, chart, inside(&outline, center));
                    let sun = if n_dot_l > 0.0 { surface.sun_reaching(point, normal, toward) } else { 0.0 };
                    let at = (chart.at.y + j) * self.size.x + chart.at.x + i;
                    texels[at as usize] = Vec3::splat(BASE_AMBIENT + SUN_BRIGHTNESS * n_dot_l * sun);
                }
            }
        }
        texels
    }
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
            let chart = &atlas.charts[index];
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
            Atlas::of(&faces).charts[at].clone()
        };
        let (a, b) = (floor(0), floor(1));
        let world = |chart: &Chart, i: i32| (chart.origin.x + i) as f32 * TEXEL;
        // The texel holding x = 3.3 in each.
        let column = |chart: &Chart| (3.3 / TEXEL).floor() as i32 - chart.origin.x;
        assert_eq!(world(&a, column(&a)), world(&b, column(&b)));
    }

    #[test]
    fn a_floor_under_a_sky_is_brighter_than_one_under_a_roof() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)),
            Room::new(Vec3::new(10.0, 0.0, 0.0), Vec3::new(14.0, 3.0, 4.0)),
        ];
        let surface = LitSurface::build(&rooms, |index, slot| index == 0 && slot == RoomSlot::Ceiling);
        let toward = SunPosition { altitude: 70.0, azimuth: 10.0 }.toward();
        let floor_light = |index| {
            let faces = room_share(&rooms, index);
            let sky: Vec<bool> = faces.iter().map(|f| index == 0 && RoomSlot::of(f.plane.normal) == RoomSlot::Ceiling).collect();
            let atlas = Atlas::of(&faces);
            let texels = atlas.bake(&faces, &sky, &surface, toward);
            let at = faces.iter().position(|f| RoomSlot::of(f.plane.normal) == RoomSlot::Floor).unwrap();
            let chart = &atlas.charts[at];
            let middle = chart.at + chart.size / 2;
            texels[(middle.y * atlas.size.x + middle.x) as usize].x
        };
        assert!(floor_light(0) > BASE_AMBIENT + SUN_BRIGHTNESS * 0.9, "{}", floor_light(0));
        assert_eq!(floor_light(1), BASE_AMBIENT);
    }
}
