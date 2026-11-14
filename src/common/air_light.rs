//! The light in the air, for things that carry no lightmap: what a point sees all round it, baked
//! with the lightmaps on a lattice fixed in the world, so neighboring rooms share samples and a
//! body crossing a threshold sees no step. See "Things that move" in documentation/sun.md.
//!
//! The lattice sits half a step off the meter grid rooms are laid out on, so no sample lands on a
//! wall.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::common::lightmap::BASE_AMBIENT;
use crate::common::sunlight::{Daylight, LitSurface};
use crate::common::world_surface::Face;

/// Meters between samples. Coarse on purpose: ambient light is smooth, and a body is a meter wide.
pub const AIR_STEP: f32 = 2.0;

/// In lightmap units: [`BASE_AMBIENT`] plus sky and one bounce, as a texel is.
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub struct AirLight {
    samples: HashMap<IVec3, Vec3>,
}

impl AirLight {
    /// Every lattice point inside some room's bounds and inside the map, gathered.
    pub fn bake(rooms: &[&[Face]], surface: &LitSurface, light: &Daylight) -> AirLight {
        let mut samples = HashMap::new();
        for faces in rooms {
            let Some((min, max)) = bounds(faces) else { continue };
            let (lo, hi) = (lattice(min).ceil().as_ivec3(), lattice(max).floor().as_ivec3());
            for x in lo.x..=hi.x {
                for y in lo.y..=hi.y {
                    for z in lo.z..=hi.z {
                        let cell = IVec3::new(x, y, z);
                        if samples.contains_key(&cell) {
                            continue;
                        }
                        let point = world(cell);
                        if surface.encloses(point) {
                            samples.insert(cell, Vec3::splat(BASE_AMBIENT) + surface.ambient_in_air(point, light));
                        }
                    }
                }
            }
        }
        AirLight { samples }
    }

    /// Trilinear between the samples round `point`, over whichever of the eight exist — one inside
    /// a wall does not — or `None` with none near.
    pub fn at(&self, point: Vec3) -> Option<Vec3> {
        let f = lattice(point);
        let base = f.floor();
        let t = f - base;
        let base = base.as_ivec3();
        let (mut sum, mut weight) = (Vec3::ZERO, 0.0);
        for corner in 0..8 {
            let step = IVec3::new(corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
            let Some(sample) = self.samples.get(&(base + step)) else { continue };
            let w = Vec3::select(step.cmpeq(IVec3::ONE), t, 1.0 - t);
            let w = w.x * w.y * w.z;
            sum += *sample * w;
            weight += w;
        }
        (weight > 1e-3).then(|| sum / weight)
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// In a fixed order, for the cache.
    pub fn to_list(&self) -> Vec<(IVec3, Vec3)> {
        let mut list: Vec<(IVec3, Vec3)> = self.samples.iter().map(|(cell, light)| (*cell, *light)).collect();
        list.sort_by_key(|(cell, _)| cell.to_array());
        list
    }

    pub fn from_list(list: Vec<(IVec3, Vec3)>) -> AirLight {
        AirLight { samples: list.into_iter().collect() }
    }
}

/// Lattice coordinates: sample `cell` is at `(cell + 0.5) * AIR_STEP`.
fn lattice(point: Vec3) -> Vec3 {
    point / AIR_STEP - 0.5
}

fn world(cell: IVec3) -> Vec3 {
    (cell.as_vec3() + 0.5) * AIR_STEP
}

fn bounds(faces: &[Face]) -> Option<(Vec3, Vec3)> {
    let mut vertices = faces.iter().flat_map(|face| face.vertices.iter().copied());
    let first = vertices.next()?;
    Some(vertices.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
}

/// `light` as a `MeshTag`: RGBE, three 8-bit mantissas and an exponent biased by 128. Never 0, so
/// a mesh with no tag can mean "unchanged". `air_lit.wesl` decodes it.
pub fn pack(light: Vec3) -> u32 {
    let light = light.max(Vec3::ZERO);
    let largest = light.max_element();
    if largest < 1e-20 {
        return 1 << 24;
    }
    let exponent = largest.log2().floor() as i32 + 1;
    let mantissa = (light / 2f32.powi(exponent) * 256.0).min(Vec3::splat(255.0)).as_uvec3();
    let biased = (exponent + 128).clamp(1, 255) as u32;
    mantissa.x | mantissa.y << 8 | mantissa.z << 16 | biased << 24
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::lightmap::daylight;
    use crate::common::sky::{SkyTime, SunPosition};
    use crate::common::world_surface::room_share;
    use crate::tool::room::{Room, RoomSlot};

    fn unpack(tag: u32) -> Vec3 {
        let exponent = ((tag >> 24) & 0xff) as i32 - 136;
        Vec3::new((tag & 0xff) as f32, ((tag >> 8) & 0xff) as f32, ((tag >> 16) & 0xff) as f32) * 2f32.powi(exponent)
    }

    #[test]
    fn a_packed_light_comes_back_within_a_step() {
        for light in [Vec3::ONE, Vec3::new(0.1, 3.7, 15.0), Vec3::splat(0.003), Vec3::ZERO] {
            let back = unpack(pack(light));
            assert!((back - light).abs().max_element() <= light.max_element() / 128.0 + 1e-6, "{light} came back {back}");
        }
        assert_ne!(pack(Vec3::ZERO), 0, "0 means no tag");
    }

    fn courtyard() -> [Room; 2] {
        [Room::new(Vec3::ZERO, Vec3::new(8.0, 4.0, 8.0)), Room::new(Vec3::new(8.0, 0.0, 0.0), Vec3::new(16.0, 4.0, 8.0))]
    }

    fn bake(rooms: &[Room], open: &[usize]) -> AirLight {
        let is_sky = |room: usize, slot: RoomSlot| open.contains(&room) && slot == RoomSlot::Ceiling;
        let surface = LitSurface::build(rooms, is_sky);
        let shares: Vec<Vec<Face>> = (0..rooms.len()).map(|index| room_share(rooms, index)).collect();
        let shares: Vec<&[Face]> = shares.iter().map(Vec::as_slice).collect();
        AirLight::bake(&shares, &surface, &daylight(SunPosition { altitude: 60.0, azimuth: 90.0 }.toward(), SkyTime::Day))
    }

    #[test]
    fn the_air_under_a_sky_is_brighter_than_the_air_in_a_closed_room() {
        let rooms = [Room::new(Vec3::ZERO, Vec3::new(8.0, 4.0, 8.0)), Room::new(Vec3::new(20.0, 0.0, 0.0), Vec3::new(28.0, 4.0, 8.0))];
        let air = bake(&rooms, &[0]);
        let open = air.at(Vec3::new(4.0, 1.2, 4.0)).expect("a sample near the middle of a room");
        let closed = air.at(Vec3::new(24.0, 1.2, 4.0)).expect("a sample near the middle of a room");
        assert_eq!(closed, Vec3::splat(BASE_AMBIENT), "light got into a closed room");
        assert!(open.x > BASE_AMBIENT + 50.0, "{open}");
    }

    /// Nothing samples outside the map, and a point there reads nothing.
    #[test]
    fn there_is_no_air_outside_the_map() {
        let air = bake(&courtyard(), &[0]);
        assert_eq!(air.at(Vec3::new(4.0, 30.0, 4.0)), None);
        assert!(!air.is_empty());
    }

    /// One sky over two rooms side by side: walking across the threshold, the light changes no
    /// more in the step across it than in the steps either side.
    #[test]
    fn the_air_runs_on_across_a_threshold() {
        let air = bake(&courtyard(), &[0, 1]);
        let along = |x: f32| air.at(Vec3::new(x, 1.2, 4.0)).unwrap().x;
        let steps: Vec<f32> = (0..9).map(|i| (along(6.0 + i as f32 * 0.5 + 0.5) - along(6.0 + i as f32 * 0.5)).abs()).collect();
        let largest = steps.iter().cloned().fold(0.0, f32::max);
        let mean = steps.iter().sum::<f32>() / steps.len() as f32;
        assert!(largest <= mean * 3.0 + 1.0, "a step at the threshold: {steps:?}");
    }
}
