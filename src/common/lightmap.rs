//! Lightmaps for the map's surface: a room's faces charted onto a lattice of [`TEXEL`] squares and
//! packed into one atlas per room. See documentation/sun.md.
//!
//! The lattice depends only on a face's plane, never on the face, so two rooms' floors meeting at
//! a threshold sample the same points and bake the same values on both sides of the seam. The room
//! bake writes `UV_1` from an [`Atlas`] and the sun bake fills one from the same faces; both call
//! [`Atlas::of`], which is deterministic, so neither has to hold the other's layout.

use bevy::prelude::*;

use crate::common::sky::SkyTime;
use crate::common::sunlight::{Daylight, LitSurface};
use crate::common::world_surface::Face;

/// Meters per lightmap texel.
pub const TEXEL: f32 = 0.25;

/// What a texel no light reaches is lit with, in `GlobalAmbientLight::brightness` units: a
/// lightmap replaces the ambient term on the meshes that carry one.
pub const BASE_AMBIENT: f32 = 100.0;

/// What the sun adds to a surface square on to it, in the same units.
pub const SUN_BRIGHTNESS: f32 = 1500.0;

/// What a sky color of 1 is worth, in the same units.
pub const SKY_BRIGHTNESS: f32 = 600.0;

/// Of every surface, for the bounce.
pub const ALBEDO: f32 = 0.4;

/// Sky light and bounce are smooth, so they are gathered every this many texels — a meter — and
/// interpolated between. On the same lattice, so neighbors across a threshold share samples too.
const AMBIENT_STEP: f32 = 4.0;

pub fn daylight(toward: Vec3, sky: SkyTime) -> Daylight {
    Daylight { toward, sun: SUN_BRIGHTNESS, sky, sky_brightness: SKY_BRIGHTNESS, albedo: ALBEDO }
}

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
    pub fn bake(&self, faces: &[Face], sky: &[bool], surface: &LitSurface, light: &Daylight) -> Vec<Vec3> {
        let mut texels = vec![Vec3::splat(BASE_AMBIENT); (self.size.x * self.size.y) as usize];
        for (index, (face, chart)) in faces.iter().zip(&self.charts).enumerate() {
            if sky[index] {
                continue;
            }
            let normal = face.plane.normal;
            let n_dot_l = normal.dot(light.toward).max(0.0);
            let outline: Vec<Vec2> = face.vertices.iter().map(|p| chart.lattice(*p)).collect();
            let sample_at = |at: Vec2| on_plane(face, chart, inside(&outline, at));

            // The gather grid: sample `k` sits at `k * AMBIENT_STEP + AMBIENT_STEP / 2`, enough
            // of them that every texel center has one either side.
            let half = AMBIENT_STEP / 2.0;
            let first = chart.origin.as_vec2() + 0.5;
            let last = first + (chart.size.as_vec2() - 1.0);
            let k_lo = ((first - half) / AMBIENT_STEP).floor().as_ivec2();
            let k_hi = ((last - half) / AMBIENT_STEP).floor().as_ivec2() + 1;
            let columns = (k_hi.x - k_lo.x + 1) as usize;
            let mut gathered = Vec::with_capacity(columns * (k_hi.y - k_lo.y + 1) as usize);
            for ky in k_lo.y..=k_hi.y {
                for kx in k_lo.x..=k_hi.x {
                    let at = IVec2::new(kx, ky).as_vec2() * AMBIENT_STEP + half;
                    gathered.push(surface.ambient(sample_at(at), normal, light));
                }
            }
            let ambient = |at: Vec2| {
                let f = (at - half) / AMBIENT_STEP - k_lo.as_vec2();
                let (cell, t) = (f.floor(), f.fract());
                let (x, y) = (cell.x as usize, cell.y as usize);
                let row = |y: usize| gathered[y * columns + x].lerp(gathered[y * columns + x + 1], t.x);
                row(y).lerp(row(y + 1), t.y)
            };

            for j in 0..chart.size.y {
                for i in 0..chart.size.x {
                    let center = chart.origin.as_vec2() + Vec2::new(i as f32, j as f32) + 0.5;
                    let sun = if n_dot_l > 0.0 { surface.sun_reaching(sample_at(center), normal, light.toward) } else { 0.0 };
                    let at = (chart.at.y + j) * self.size.x + chart.at.x + i;
                    texels[at as usize] = BASE_AMBIENT + light.sun * n_dot_l * sun + ambient(center);
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

    /// The light baked on room `index`'s floor at `(x, z)`, with the ceilings of `open` rooms sky.
    fn floor_light(rooms: &[Room], open: &[usize], light: &Daylight, index: usize, x: f32, z: f32) -> f32 {
        let is_sky = |room: usize, slot: RoomSlot| open.contains(&room) && slot == RoomSlot::Ceiling;
        let surface = LitSurface::build(rooms, is_sky);
        let faces = room_share(rooms, index);
        let sky: Vec<bool> = faces.iter().map(|f| is_sky(index, RoomSlot::of(f.plane.normal))).collect();
        let atlas = Atlas::of(&faces);
        let texels = atlas.bake(&faces, &sky, &surface, light);
        let at = faces.iter().position(|f| RoomSlot::of(f.plane.normal) == RoomSlot::Floor).unwrap();
        let chart = &atlas.charts[at];
        let texel = (Vec2::new(x, z) / TEXEL).floor().as_ivec2() - chart.origin;
        let texel = chart.at + texel.as_uvec2();
        texels[(texel.y * atlas.size.x + texel.x) as usize].x
    }

    fn day(altitude: f32, azimuth: f32) -> Daylight {
        daylight(SunPosition { altitude, azimuth }.toward(), SkyTime::Day)
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
        let at = |sky| floor_light(&courtyard(), &[0], &daylight(toward, sky), 0, 1.0, 0.3);
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
}
