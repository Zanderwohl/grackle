//! Which static shapes are near a box: a uniform grid over their bounds.
//!
//! Answers come back sorted by index, so "the first shape overlapped" means
//! the same thing it did when every query was a linear scan.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

/// Meters. About a body's width a few times over: a body's query touches a
/// handful of cells, and a 24 m floor slab sits in a few dozen.
pub const CELL: f32 = 4.0;

/// A query or a shape spanning more cells than this is answered by scanning
/// everything, which is never worse than having no grid.
const MOST_CELLS: i64 = 512;

#[derive(Default)]
pub struct Grid {
    cells: HashMap<IVec3, Vec<u32>>,
    /// Shapes too big to file, tested by every query.
    everywhere: Vec<u32>,
    len: u32,
}

impl Grid {
    pub fn build(bounds: impl Iterator<Item = (Vec3, Vec3)>) -> Grid {
        let mut grid = Grid::default();
        for (index, (min, max)) in bounds.enumerate() {
            let index = index as u32;
            grid.len = index + 1;
            match span(min, max) {
                Some((lo, hi)) => each_cell(lo, hi, |cell| grid.cells.entry(cell).or_default().push(index)),
                None => grid.everywhere.push(index),
            }
        }
        grid
    }

    /// Every shape whose bounds might touch `min..=max`, ascending.
    pub fn near(&self, min: Vec3, max: Vec3) -> Vec<u32> {
        let Some((lo, hi)) = span(min, max) else {
            return (0..self.len).collect();
        };
        let mut found = self.everywhere.clone();
        each_cell(lo, hi, |cell| {
            if let Some(shapes) = self.cells.get(&cell) {
                found.extend_from_slice(shapes);
            }
        });
        found.sort_unstable();
        found.dedup();
        found
    }

    /// Calls `hit` with the shapes in each cell the segment crosses, nearest cell first, and stops
    /// at the first that answers true. A shape in several cells is offered once per cell. Cheaper
    /// than [`Grid::near`] for a long segment, whose box covers cells the segment never enters.
    pub fn any_along(&self, from: Vec3, to: Vec3, mut hit: impl FnMut(u32) -> bool) -> bool {
        if self.everywhere.iter().any(|shape| hit(*shape)) {
            return true;
        }
        let (Some((start, _)), Some((end, _))) = (span(from, from), span(to, to)) else {
            return (0..self.len).any(hit);
        };
        let delta = to - from;
        let step = IVec3::select(delta.cmpne(Vec3::ZERO), delta.signum().as_ivec3(), IVec3::ZERO);
        // Fractions of the segment, per axis: to the next cell boundary, and across one cell.
        let moving = delta.cmpne(Vec3::ZERO);
        let across = Vec3::select(moving, CELL / delta.abs(), Vec3::INFINITY);
        let boundary = (start + step.max(IVec3::ZERO)).as_vec3() * CELL;
        let mut next = Vec3::select(moving, (boundary - from) / delta, Vec3::INFINITY);
        let mut cell = start;
        loop {
            if self.cells.get(&cell).is_some_and(|shapes| shapes.iter().any(|shape| hit(*shape))) {
                return true;
            }
            let axis = next.min_position();
            if cell == end || next[axis] > 1.0 {
                return false;
            }
            cell[axis] += step[axis];
            next[axis] += across[axis];
        }
    }
}

fn span(min: Vec3, max: Vec3) -> Option<(IVec3, IVec3)> {
    if !(min.is_finite() && max.is_finite()) {
        return None;
    }
    let lo = (min / CELL).floor();
    let hi = (max / CELL).floor();
    let limit = i32::MAX as f32 / 2.0;
    if lo.abs().max_element() > limit || hi.abs().max_element() > limit {
        return None;
    }
    let (lo, hi) = (lo.as_ivec3(), hi.as_ivec3());
    let size = (hi - lo + IVec3::ONE).max(IVec3::ZERO).as_i64vec3();
    (size.x * size.y * size.z <= MOST_CELLS).then_some((lo, hi))
}

fn each_cell(lo: IVec3, hi: IVec3, mut visit: impl FnMut(IVec3)) {
    for x in lo.x..=hi.x {
        for y in lo.y..=hi.y {
            for z in lo.z..=hi.z {
                visit(IVec3::new(x, y, z));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_finds_what_it_touches_and_not_what_is_far_away() {
        let grid = Grid::build([
            (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0)),
            (Vec3::new(50.0, 0.0, 0.0), Vec3::new(51.0, 1.0, 1.0)),
            (Vec3::new(-2.0, -2.0, -2.0), Vec3::new(0.0, 0.0, 0.0)),
        ].into_iter());
        assert_eq!(grid.near(Vec3::splat(-0.5), Vec3::splat(0.5)), vec![0, 2]);
    }

    /// Breaks if a segment skips a cell it crosses, or reports a shape it never reaches.
    #[test]
    fn a_segment_meets_only_what_lies_along_it() {
        let grid = Grid::build([
            (Vec3::new(10.0, 0.0, 10.0), Vec3::new(11.0, 1.0, 11.0)),
            (Vec3::new(10.0, 0.0, -10.0), Vec3::new(11.0, 1.0, -9.0)),
        ].into_iter());
        let met = |from: Vec3, to: Vec3| {
            let mut met = Vec::new();
            grid.any_along(from, to, |shape| {
                met.push(shape);
                false
            });
            met
        };
        assert_eq!(met(Vec3::new(0.5, 0.5, 0.5), Vec3::new(20.5, 0.5, 20.5)), vec![0]);
        assert_eq!(met(Vec3::new(20.5, 0.5, -20.5), Vec3::new(0.5, 0.5, 0.5)), vec![1]);
        assert!(met(Vec3::new(0.5, 0.5, 0.5), Vec3::new(0.5, 0.5, 20.5)).is_empty());
        assert!(met(Vec3::new(10.5, 0.5, 10.5), Vec3::new(10.5, 0.5, 10.5)) == vec![0]);
    }

    #[test]
    fn an_unbounded_query_sees_everything() {
        let grid = Grid::build([(Vec3::ZERO, Vec3::ONE), (Vec3::splat(100.0), Vec3::splat(101.0))].into_iter());
        assert_eq!(grid.near(Vec3::ZERO, Vec3::splat(f32::INFINITY)), vec![0, 1]);
    }
}
