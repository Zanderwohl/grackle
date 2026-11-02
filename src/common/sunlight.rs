//! Which of the map's surface the sun reaches: a point is sunlit when the first face toward the
//! sun is sky. The rooms' union is closed, so sky is the only way out, and rays are cast against
//! the whole map so light crosses thresholds with nothing to stitch. See documentation/sun.md.

use bevy::prelude::*;

use crate::common::grid::{Grid, CELL};
use crate::common::sky::SkyTime;
use crate::common::world_surface::{self, Face};
use crate::tool::room::{Room, RoomSlot};

/// Off the surface before casting, so a ray does not meet the face it started on. Well above the
/// kernel's tolerance and well below anything a mapper builds.
const LIFT: f32 = 0.01;

/// The sun's angular radius, in radians. Wider than the real sun's quarter degree, so a shadow's
/// edge is soft at the scale of a lightmap texel rather than a hard step.
const SUN_RADIUS: f32 = 0.02;

/// Rays per point, across the sun's disc.
const SUN_RAYS: usize = 8;

/// Rays per point across the hemisphere, for sky light and bounce.
const GATHER_RAYS: usize = 64;

/// Every constant a texel's light depends on, for keying the lightmap cache.
pub fn tuning() -> [u32; 4] {
    [LIFT.to_bits(), SUN_RADIUS.to_bits(), SUN_RAYS as u32, GATHER_RAYS as u32]
}

/// What lights the map from outside, in lightmap units: a lightmap texel is the irradiance over π,
/// so that a value is what `GlobalAmbientLight::brightness` would be for the same look.
#[derive(Clone, Copy, Debug)]
pub struct Daylight {
    /// Unit, toward the sun.
    pub toward: Vec3,
    /// On a surface square on to the sun.
    pub sun: f32,
    pub sky: SkyTime,
    /// What a sky color of 1 is worth.
    pub sky_brightness: f32,
    /// Of every surface, for the one bounce: nothing reads a material's own color.
    pub albedo: f32,
}

/// The map's surface as faces that know whether they are sky, indexed for rays.
pub struct LitSurface {
    faces: Vec<Face>,
    sky: Vec<bool>,
    grid: Grid,
    /// Longer than any ray inside the map can travel.
    reach: f32,
}

impl LitSurface {
    /// `is_sky` answers for a room's slot, by index into `rooms`.
    pub fn build(rooms: &[Room], is_sky: impl Fn(usize, RoomSlot) -> bool) -> Self {
        let mut faces = Vec::new();
        let mut sky = Vec::new();
        for index in 0..rooms.len() {
            for face in world_surface::room_share(rooms, index) {
                sky.push(is_sky(index, RoomSlot::of(face.plane.normal)));
                faces.push(face);
            }
        }
        Self::from_faces(faces, sky)
    }

    /// Faces already cut, as the room bake leaves them, and whether each is sky.
    pub fn from_faces(faces: Vec<Face>, sky: Vec<bool>) -> Self {
        let bounds: Vec<(Vec3, Vec3)> = faces.iter().map(face_bounds).collect();
        let (min, max) = bounds.iter().fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), (a, b)| (lo.min(*a), hi.max(*b)));
        let reach = if faces.is_empty() { 0.0 } else { (max - min).length() + 1.0 };
        Self { grid: Grid::build(bounds.into_iter()), faces, sky, reach }
    }

    /// The first face along the ray, and how far.
    ///
    /// Walked a cell at a time, so a ray across a big map tests the faces near it rather than all
    /// of them. A hit inside a step's length is inside that step's box, so the first step to find
    /// one has found the nearest.
    pub fn first_hit(&self, origin: Vec3, direction: Vec3) -> Option<(usize, f32)> {
        let mut start = 0.0;
        while start < self.reach {
            let end = (start + CELL).min(self.reach);
            let (a, b) = (origin + direction * start, origin + direction * end);
            let hit = self
                .grid
                .near(a.min(b), a.max(b))
                .into_iter()
                .filter_map(|index| {
                    let t = world_surface::meets(&self.faces[index as usize], origin, direction)?;
                    (start..=end).contains(&t).then_some((index as usize, t))
                })
                .min_by(|x, y| x.1.total_cmp(&y.1));
            if hit.is_some() {
                return hit;
            }
            start = end;
        }
        None
    }

    /// How much of the sun's disc a point on the surface sees, 0 to 1. `normal` points into the
    /// air and `toward` at the sun. Not `N·L`-weighted: that is lighting, and this is visibility.
    pub fn sun_reaching(&self, point: Vec3, normal: Vec3, toward: Vec3) -> f32 {
        if normal.dot(toward) <= 0.0 {
            return 0.0;
        }
        let origin = point + normal * LIFT;
        let (u, v) = toward.any_orthonormal_pair();
        let lit = (0..SUN_RAYS)
            .filter(|&i| {
                let (x, y) = sunflower(i, SUN_RAYS);
                let direction = (toward + (u * x + v * y) * SUN_RADIUS).normalize();
                // Nothing at all only happens through a crack in the kernel's output, which is
                // not a hole in the roof.
                self.first_hit(origin, direction).is_some_and(|(face, _)| self.sky[face])
            })
            .count();
        lit as f32 / SUN_RAYS as f32
    }

    /// Sky light and one bounce of sunlight onto a point: the average, over cosine-weighted rays
    /// across its hemisphere, of the sky's color where a ray escapes and of the sunlit surface
    /// where it does not. The surface a ray hits is lit by the sun alone, through one ray at the
    /// sun's center: that is the one bounce, and it stops there.
    pub fn ambient(&self, point: Vec3, normal: Vec3, light: &Daylight) -> Vec3 {
        let origin = point + normal * LIFT;
        let (u, v) = normal.any_orthonormal_pair();
        let total: Vec3 = (0..GATHER_RAYS)
            .map(|i| {
                // A disc lifted onto the hemisphere is cosine-weighted, so the plain average is
                // irradiance over π.
                let (x, y) = sunflower(i, GATHER_RAYS);
                let direction = (u * x + v * y + normal * (1.0 - x * x - y * y).max(0.0).sqrt()).normalize();
                let Some((face, t)) = self.first_hit(origin, direction) else { return Vec3::ZERO };
                if self.sky[face] {
                    return light.sky.color_at(direction.y).to_vec3() * light.sky_brightness;
                }
                let bounced = &self.faces[face];
                let facing = bounced.plane.normal.dot(light.toward);
                if facing <= 0.0 {
                    return Vec3::ZERO;
                }
                let at = origin + direction * t + bounced.plane.normal * LIFT;
                let sunlit = self.first_hit(at, light.toward).is_some_and(|(face, _)| self.sky[face]);
                if sunlit { Vec3::splat(light.sun * facing * light.albedo) } else { Vec3::ZERO }
            })
            .sum();
        total / GATHER_RAYS as f32
    }
}

/// The `i`th of `n` points spread evenly over the unit disc, the same every time so a bake is
/// repeatable.
fn sunflower(i: usize, n: usize) -> (f32, f32) {
    const GOLDEN_ANGLE: f32 = 2.399_963;
    let radius = ((i as f32 + 0.5) / n as f32).sqrt();
    let angle = i as f32 * GOLDEN_ANGLE;
    (radius * angle.cos(), radius * angle.sin())
}

fn face_bounds(face: &Face) -> (Vec3, Vec3) {
    face.vertices.iter().fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::sky::SunPosition;

    fn sun(altitude: f32, azimuth: f32) -> Vec3 {
        SunPosition { altitude, azimuth }.toward()
    }

    fn sky_ceiling_on(rooms: &[usize]) -> impl Fn(usize, RoomSlot) -> bool + '_ {
        move |index, slot| slot == RoomSlot::Ceiling && rooms.contains(&index)
    }

    #[test]
    fn a_closed_room_is_dark() {
        let surface = LitSurface::build(&[Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))], |_, _| false);
        assert_eq!(surface.sun_reaching(Vec3::new(2.0, 0.0, 2.0), Vec3::Y, sun(80.0, 0.0)), 0.0);
    }

    #[test]
    fn a_sky_ceiling_lights_the_floor_under_it() {
        let surface = LitSurface::build(&[Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))], sky_ceiling_on(&[0]));
        let toward = sun(80.0, 30.0);
        assert_eq!(surface.sun_reaching(Vec3::new(2.0, 0.0, 2.0), Vec3::Y, toward), 1.0);
        // A wall facing away from the sun is not lit, whatever is overhead.
        assert_eq!(surface.sun_reaching(Vec3::new(2.0, 1.5, 0.0), Vec3::Z, toward), 0.0);
    }

    /// A courtyard open to the sky with a low room off its east side, the sun low in the west:
    /// light comes through the opening onto the near part of the room's floor and not the far
    /// part, with a soft edge between.
    #[test]
    fn the_sun_comes_through_an_opening_and_stops_at_its_edge() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)),
            Room::new(Vec3::new(4.0, 0.0, 0.0), Vec3::new(8.0, 2.0, 4.0)),
        ];
        let surface = LitSurface::build(&rooms, sky_ceiling_on(&[0]));
        let toward = sun(45.0, 270.0);
        let floor = |x| surface.sun_reaching(Vec3::new(x, 0.0, 2.0), Vec3::Y, toward);

        assert_eq!(floor(5.0), 1.0, "through the opening and out of the courtyard's sky");
        assert_eq!(floor(7.0), 0.0, "under the low room's own ceiling");
        // The ray from here grazes the top of the opening.
        let edge = floor(6.0);
        assert!(edge > 0.0 && edge < 1.0, "the shadow's edge is {edge}");
    }

    /// Only the sky lets light in: the same opening into a courtyard with a solid roof is dark.
    #[test]
    fn an_opening_into_a_roofed_room_lets_nothing_in() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)),
            Room::new(Vec3::new(4.0, 0.0, 0.0), Vec3::new(8.0, 2.0, 4.0)),
        ];
        let surface = LitSurface::build(&rooms, |_, _| false);
        assert_eq!(surface.sun_reaching(Vec3::new(5.0, 0.0, 2.0), Vec3::Y, sun(45.0, 270.0)), 0.0);
    }

    /// Walking a ray a cell at a time finds what testing every face finds.
    #[test]
    fn a_ray_across_a_long_map_meets_the_far_wall() {
        let surface = LitSurface::build(&[Room::new(Vec3::ZERO, Vec3::new(60.0, 3.0, 4.0))], |_, _| false);
        let (face, t) = surface.first_hit(Vec3::new(1.0, 1.5, 2.0), Vec3::X).expect("the room is closed");
        assert!((t - 59.0).abs() < 1e-4, "{t}");
        assert!(surface.faces[face].plane.normal.abs_diff_eq(Vec3::NEG_X, 1e-5));
    }
}
