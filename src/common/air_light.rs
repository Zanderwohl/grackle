//! The light in the air for things with no lightmap, baked with the lightmaps on a lattice fixed
//! in the world, so neighboring rooms share samples. The lattice sits half a step off the meter
//! grid rooms are laid out on, so no sample lands on a wall. See documentation/sun.md.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::common::sunlight::{Daylight, LitSurface, AIR_RAYS};

/// Meters between samples. Ambient light is smooth and a body is a meter wide.
pub const AIR_STEP: f32 = 2.0;

/// In lightmap units: the map's ambient plus sky and one bounce, as a texel is.
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub struct AirLight {
    samples: HashMap<IVec3, Vec3>,
}

impl AirLight {
    pub fn bake(rooms: &[(Vec3, Vec3)], surface: &LitSurface, light: &Daylight) -> AirLight {
        let mut bake = AirBake::plan(rooms);
        while !bake.is_done() {
            bake.step(surface, light);
        }
        bake.finish()
    }

    /// Trilinear over whichever of the eight samples round `point` exist; one inside a wall does not.
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
/// [`AirLight::bake`] a few samples at a time, its lattice counted before any ray is cast.
pub struct AirBake {
    /// Each once, in the order the rooms list them.
    cells: Vec<IVec3>,
    next: usize,
    samples: HashMap<IVec3, Vec3>,
}

impl AirBake {
    /// Samples per step: a few milliseconds.
    const STEP: usize = 16;

    pub fn plan(rooms: &[(Vec3, Vec3)]) -> Self {
        let mut seen = std::collections::HashSet::new();
        let mut cells = Vec::new();
        for &(min, max) in rooms {
            let (lo, hi) = (lattice(min).ceil().as_ivec3(), lattice(max).floor().as_ivec3());
            for x in lo.x..=hi.x {
                for y in lo.y..=hi.y {
                    for z in lo.z..=hi.z {
                        let cell = IVec3::new(x, y, z);
                        if seen.insert(cell) {
                            cells.push(cell);
                        }
                    }
                }
            }
        }
        AirBake { cells, next: 0, samples: HashMap::new() }
    }

    /// The rays the bake casts across spheres, counting those a sample outside the map skips.
    pub fn rays(&self) -> usize {
        self.cells.len() * AIR_RAYS
    }

    pub fn cast(&self) -> usize {
        self.next * AIR_RAYS
    }

    pub fn is_done(&self) -> bool {
        self.next >= self.cells.len()
    }

    pub fn step(&mut self, surface: &LitSurface, light: &Daylight) {
        let end = (self.next + Self::STEP).min(self.cells.len());
        for &cell in &self.cells[self.next..end] {
            let point = world(cell);
            if surface.encloses(point) {
                self.samples.insert(cell, Vec3::splat(light.base) + surface.ambient_in_air(point, light));
            }
        }
        self.next = end;
    }

    pub fn finish(self) -> AirLight {
        AirLight { samples: self.samples }
    }
}

fn lattice(point: Vec3) -> Vec3 {
    point / AIR_STEP - 0.5
}

fn world(cell: IVec3) -> Vec3 {
    (cell.as_vec3() + 0.5) * AIR_STEP
}


/// As a `MeshTag`: `ambient`, a multiple of the global ambient, as three 8-bit mantissas and a
/// 7-bit exponent biased by 64, and `sunlit` in the top bit. Never 0, which means no tag.
/// `air_lit.wesl` decodes it.
pub fn pack(ambient: Vec3, sunlit: bool) -> u32 {
    let sun = u32::from(sunlit) << 31;
    let ambient = ambient.max(Vec3::ZERO);
    let largest = ambient.max_element();
    if largest < 1e-18 {
        return sun | 1 << 24;
    }
    let exponent = largest.log2().floor() as i32 + 1;
    let mantissa = (ambient / 2f32.powi(exponent) * 256.0).min(Vec3::splat(255.0)).as_uvec3();
    let biased = (exponent + 64).clamp(1, 127) as u32;
    sun | mantissa.x | mantissa.y << 8 | mantissa.z << 16 | biased << 24
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::lightmap::{daylight, BASE_AMBIENT};
    use crate::common::sky::Brightness;
    use crate::common::sky::{SkyTime, SunPosition};
    use crate::tool::room::{Room, RoomSlot};

    /// What `air_lit.wesl` reads back.
    fn unpack(tag: u32) -> (Vec3, bool) {
        let exponent = ((tag >> 24) & 0x7f) as i32 - 72;
        let ambient = Vec3::new((tag & 0xff) as f32, ((tag >> 8) & 0xff) as f32, ((tag >> 16) & 0xff) as f32);
        (ambient * 2f32.powi(exponent), tag >> 31 == 1)
    }

    #[test]
    fn a_packed_light_comes_back_within_a_step() {
        for (ambient, sunlit) in [(Vec3::ONE, true), (Vec3::new(0.1, 3.7, 15.0), false), (Vec3::splat(0.003), true), (Vec3::ZERO, false)] {
            let (back, sun) = unpack(pack(ambient, sunlit));
            assert!((back - ambient).abs().max_element() <= ambient.max_element() / 128.0 + 1e-6, "{ambient} came back {back}");
            assert_eq!(sun, sunlit);
        }
        assert_ne!(pack(Vec3::ZERO, false), 0, "0 means no tag");
    }

    fn courtyard() -> [Room; 2] {
        [Room::new(Vec3::ZERO, Vec3::new(8.0, 4.0, 8.0)), Room::new(Vec3::new(8.0, 0.0, 0.0), Vec3::new(16.0, 4.0, 8.0))]
    }

    fn bake(rooms: &[Room], open: &[usize]) -> AirLight {
        let is_sky = |room: usize, slot: RoomSlot| open.contains(&room) && slot == RoomSlot::Ceiling;
        let surface = LitSurface::build(rooms, is_sky);
        let bounds: Vec<(Vec3, Vec3)> = rooms.iter().map(Room::bounds).collect();
        AirLight::bake(&bounds, &surface, &daylight(SunPosition { altitude: 60.0, azimuth: 90.0 }.toward(), SkyTime::Day, Brightness::default()))
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

    /// Fails if the light steps at a threshold more than along the rest of the walk.
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
