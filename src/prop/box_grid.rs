//! Boxes filed in a uniform grid, so asking what lies near a point or a box
//! looks at a few cells rather than every box. A pattern multiplies both what
//! is asked about and what it is asked of, so a scan of everything is
//! quadratic in the copies.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

pub struct BoxGrid {
    min: Vec3,
    cell: f32,
    cells: HashMap<IVec3, Vec<u32>>,
    /// Boxes spanning too many cells to file, returned by every query.
    everywhere: Vec<u32>,
}

impl BoxGrid {
    /// A box spanning more cells than this goes in `everywhere`.
    const MOST_CELLS: i32 = 64;

    /// Each box as its corners, already widened by any tolerance.
    pub fn new(boxes: &[(Vec3, Vec3)]) -> BoxGrid {
        let (min, max) = boxes.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), (a, b)| (lo.min(*a), hi.max(*b)));
        // About one box per cell along each axis of a cube of them.
        let across = (boxes.len() as f32).cbrt().ceil().max(1.0);
        let cell = match boxes.is_empty() {
            true => 1.0,
            false => ((max - min).max_element() / across).max(1e-3),
        };
        let mut grid = BoxGrid { min: if boxes.is_empty() { Vec3::ZERO } else { min }, cell, cells: HashMap::default(), everywhere: Vec::new() };
        for (i, (low, high)) in boxes.iter().enumerate() {
            let (low, high) = (grid.cell_of(*low), grid.cell_of(*high));
            let span = high - low + IVec3::ONE;
            if span.x * span.y * span.z > Self::MOST_CELLS {
                grid.everywhere.push(i as u32);
                continue;
            }
            for x in low.x..=high.x {
                for y in low.y..=high.y {
                    for z in low.z..=high.z {
                        grid.cells.entry(IVec3::new(x, y, z)).or_default().push(i as u32);
                    }
                }
            }
        }
        grid
    }

    fn cell_of(&self, point: Vec3) -> IVec3 {
        ((point - self.min) / self.cell).floor().as_ivec3()
    }

    /// Every box whose cells hold `point`, and every unfiled box. May return
    /// boxes that do not hold it, and a box only once.
    pub fn near_point(&self, point: Vec3) -> impl Iterator<Item = usize> + '_ {
        let near = self.cells.get(&self.cell_of(point)).map(Vec::as_slice).unwrap_or(&[]);
        near.iter().chain(&self.everywhere).map(|&i| i as usize)
    }

    /// Every box sharing a cell with `(low, high)`, and every unfiled box,
    /// in order and each once. May return boxes that do not meet it.
    pub fn near_box(&self, low: Vec3, high: Vec3) -> Vec<usize> {
        let (low, high) = (self.cell_of(low), self.cell_of(high));
        let span = high - low + IVec3::ONE;
        let mut found: Vec<usize> = self.everywhere.iter().map(|&i| i as usize).collect();
        if span.x as i64 * span.y as i64 * span.z as i64 > self.cells.len() as i64 {
            found.extend(self.cells.values().flatten().map(|&i| i as usize));
        } else {
            for x in low.x..=high.x {
                for y in low.y..=high.y {
                    for z in low.z..=high.z {
                        if let Some(members) = self.cells.get(&IVec3::new(x, y, z)) {
                            found.extend(members.iter().map(|&i| i as usize));
                        }
                    }
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_finds_every_box_that_meets_it() {
        let boxes: Vec<(Vec3, Vec3)> = (0..200)
            .map(|i| {
                let at = Vec3::new((i % 7) as f32, (i % 11) as f32 * 0.3, (i / 13) as f32 * 0.7);
                (at, at + Vec3::splat(0.2 + (i % 5) as f32 * 0.4))
            })
            .chain([(Vec3::splat(-10.0), Vec3::splat(10.0))])
            .collect();
        let grid = BoxGrid::new(&boxes);
        for i in 0..60 {
            let p = Vec3::new(i as f32 * 0.13, (i % 9) as f32 * 0.4, (i % 4) as f32 * 2.1);
            let near: Vec<usize> = grid.near_point(p).collect();
            for (j, (low, high)) in boxes.iter().enumerate() {
                if low.cmple(p).all() && p.cmple(*high).all() {
                    assert!(near.contains(&j), "point {p} misses box {j}");
                }
            }
            let (low, high) = (p, p + Vec3::new(0.5, 1.0, 0.3));
            let near = grid.near_box(low, high);
            for (j, (a, b)) in boxes.iter().enumerate() {
                if a.cmple(high).all() && low.cmple(*b).all() {
                    assert!(near.contains(&j), "box at {p} misses box {j}");
                }
            }
        }
    }
}
