//! The surfaces a move touched, and a motion clipped so it no longer goes
//! into any of them.

use bevy::prelude::*;

/// A move that touches more than this many distinct surfaces in one step has
/// nowhere left to go anyway.
pub const MOST_TOUCHED: usize = 4;

/// Normals within this of each other are one surface.
const SAME_SURFACE: f32 = 1.0e-4;

/// How far into a surface a clipped motion may still point, for rounding.
const INTO: f32 = -1.0e-6;

/// Outward normals of what a move touched, deduplicated. Fixed-size, because
/// the ragdoll keeps one per point per tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Touched {
    normals: [Vec3; MOST_TOUCHED],
    len: usize,
}

impl Touched {
    pub fn add(&mut self, normal: Vec3) {
        if self.len == MOST_TOUCHED || self.iter().any(|n| n.abs_diff_eq(normal, SAME_SURFACE)) {
            return;
        }
        self.normals[self.len] = normal;
        self.len += 1;
    }

    pub fn iter(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.normals[..self.len].iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `motion` with whatever goes into a touched surface taken out: slid
    /// along one, or along the crease where two meet, or stopped where no
    /// direction is clear of them all. Sliding along each surface in turn
    /// instead jitters in a V, each slide pushing into the other side.
    pub fn clip(&self, motion: Vec3) -> Vec3 {
        let clear = |m: Vec3| self.iter().all(|n| m.dot(n) >= INTO * m.length().max(1.0));
        if clear(motion) {
            return motion;
        }
        for n in self.iter() {
            let along = motion - n * motion.dot(n);
            if clear(along) {
                return along;
            }
        }
        for (i, a) in self.iter().enumerate() {
            for b in self.iter().skip(i + 1) {
                let Some(crease) = a.cross(b).try_normalize() else { continue };
                let along = crease * motion.dot(crease);
                if clear(along) {
                    return along;
                }
            }
        }
        Vec3::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_motion_into_one_wall_slides_along_it() {
        let mut touched = Touched::default();
        touched.add(Vec3::NEG_X);
        assert_eq!(touched.clip(Vec3::new(3.0, 0.0, 2.0)), Vec3::new(0.0, 0.0, 2.0));
        assert_eq!(touched.clip(Vec3::new(-3.0, 0.0, 2.0)), Vec3::new(-3.0, 0.0, 2.0), "moving away is not clipped");
    }

    #[test]
    fn a_motion_into_a_corner_runs_along_its_crease() {
        let mut touched = Touched::default();
        touched.add(Vec3::NEG_X);
        touched.add(Vec3::NEG_Z);
        let clipped = touched.clip(Vec3::new(1.0, 0.5, 1.0));
        assert!(clipped.abs_diff_eq(Vec3::new(0.0, 0.5, 0.0), 1e-6), "{clipped}");
    }

    #[test]
    fn falling_into_a_v_stops() {
        let mut touched = Touched::default();
        touched.add(Vec3::new(1.0, 1.0, 0.0).normalize());
        touched.add(Vec3::new(-1.0, 1.0, 0.0).normalize());
        assert!(touched.clip(Vec3::NEG_Y).abs_diff_eq(Vec3::ZERO, 1e-6));
    }

    #[test]
    fn the_same_surface_twice_is_one() {
        let mut touched = Touched::default();
        touched.add(Vec3::Y);
        touched.add(Vec3::Y);
        assert_eq!(touched.iter().count(), 1);
    }
}
