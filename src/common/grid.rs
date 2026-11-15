//! Which static shapes are near a box: a uniform grid over their bounds.
//!
//! [`Grid::near`] answers sorted by index. The other queries visit in no
//! particular order and allocate nothing.

use std::ops::ControlFlow;

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
    /// Each filed shape's cells, by index; unused for the shapes in `everywhere`.
    spans: Vec<(IVec3, IVec3)>,
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
                Some((lo, hi)) => {
                    grid.spans.push((lo, hi));
                    each_cell(lo, hi, |cell| grid.cells.entry(cell).or_default().push(index));
                }
                None => {
                    grid.spans.push((IVec3::ZERO, IVec3::ZERO));
                    grid.everywhere.push(index);
                }
            }
        }
        grid
    }

    /// Every shape whose bounds might touch `min..=max`, ascending.
    pub fn near(&self, min: Vec3, max: Vec3) -> Vec<u32> {
        let mut found = Vec::new();
        self.for_each_near(min, max, |shape| found.push(shape));
        found.sort_unstable();
        found
    }

    /// What [`Grid::near`] finds, each shape once, in no particular order.
    pub fn for_each_near(&self, min: Vec3, max: Vec3, mut visit: impl FnMut(u32)) {
        let _ = self.try_for_each_near(min, max, |shape| {
            visit(shape);
            ControlFlow::<()>::Continue(())
        });
    }

    /// [`Grid::for_each_near`] that stops at the first `Break`.
    pub fn try_for_each_near<B>(&self, min: Vec3, max: Vec3, mut visit: impl FnMut(u32) -> ControlFlow<B>) -> ControlFlow<B> {
        let Some((lo, hi)) = span(min, max) else {
            return (0..self.len).try_for_each(visit);
        };
        self.everywhere.iter().try_for_each(|shape| visit(*shape))?;
        for x in lo.x..=hi.x {
            for y in lo.y..=hi.y {
                for z in lo.z..=hi.z {
                    let cell = IVec3::new(x, y, z);
                    let Some(shapes) = self.cells.get(&cell) else { continue };
                    for &shape in shapes {
                        // Offered only in the first cell the query and the shape share, so once.
                        if self.spans[shape as usize].0.max(lo) == cell {
                            visit(shape)?;
                        }
                    }
                }
            }
        }
        ControlFlow::Continue(())
    }

    /// Calls `hit` with the shapes in each cell the segment crosses, nearest cell first, and stops
    /// at the first that answers true. A shape in several cells is offered once per cell. Cheaper
    /// than [`Grid::near`] for a long segment, whose box covers cells the segment never enters.
    pub fn any_along(&self, from: Vec3, to: Vec3, mut hit: impl FnMut(u32) -> bool) -> bool {
        let mut found = false;
        self.along(from, to, |_, shapes| {
            found = shapes.iter().any(|shape| hit(*shape));
            found
        });
        found
    }

    /// Calls `visit` with the shapes in each cell the segment crosses, nearest cell first, with
    /// the fraction of the segment at which it enters that cell, until `visit` answers true. The
    /// shapes too big to file come first, at zero; a segment that cannot be walked offers every
    /// shape at once, at zero.
    pub fn along(&self, from: Vec3, to: Vec3, mut visit: impl FnMut(f32, &[u32]) -> bool) {
        if visit(0.0, &self.everywhere) {
            return;
        }
        let (Some((start, _)), Some((end, _))) = (span(from, from), span(to, to)) else {
            let every: Vec<u32> = (0..self.len).collect();
            visit(0.0, &every);
            return;
        };
        let delta = to - from;
        let step = IVec3::select(delta.cmpne(Vec3::ZERO), delta.signum().as_ivec3(), IVec3::ZERO);
        // Fractions of the segment, per axis: to the next cell boundary, and across one cell.
        let moving = delta.cmpne(Vec3::ZERO);
        let across = Vec3::select(moving, CELL / delta.abs(), Vec3::INFINITY);
        let boundary = (start + step.max(IVec3::ZERO)).as_vec3() * CELL;
        let mut next = Vec3::select(moving, (boundary - from) / delta, Vec3::INFINITY);
        let mut cell = start;
        let mut entered = 0.0;
        loop {
            if let Some(shapes) = self.cells.get(&cell)
                && visit(entered, shapes)
            {
                return;
            }
            let axis = next.min_position();
            if cell == end || next[axis] > 1.0 {
                return;
            }
            entered = next[axis];
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

    /// A small deterministic generator, so the tests need no RNG crate.
    fn scatter(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed as f32 / u32::MAX as f32
    }

    /// Breaks if a shape is offered twice, or missed, by the query that does not sort.
    #[test]
    fn a_query_offers_each_shape_it_touches_once() {
        let mut seed = 0x9e37_79b9;
        let mut point = |scale: f32| Vec3::new(scatter(&mut seed), scatter(&mut seed), scatter(&mut seed)) * scale - scale * 0.5;
        let shapes: Vec<(Vec3, Vec3)> = (0..300)
            .map(|i| {
                let min = point(60.0);
                let size = if i % 50 == 0 { Vec3::splat(200.0) } else { point(12.0).abs() };
                (min, min + size)
            })
            .collect();
        let grid = Grid::build(shapes.iter().copied());
        for _ in 0..200 {
            let min = point(70.0);
            let max = min + point(20.0).abs();
            let mut expected = grid.everywhere.clone();
            if let Some((lo, hi)) = span(min, max) {
                each_cell(lo, hi, |cell| expected.extend(grid.cells.get(&cell).into_iter().flatten()));
            }
            expected.sort_unstable();
            expected.dedup();
            let mut offered = Vec::new();
            grid.for_each_near(min, max, |shape| offered.push(shape));
            offered.sort_unstable();
            assert_eq!(offered, expected);
        }
    }

    #[test]
    fn an_unbounded_query_sees_everything() {
        let grid = Grid::build([(Vec3::ZERO, Vec3::ONE), (Vec3::splat(100.0), Vec3::splat(101.0))].into_iter());
        assert_eq!(grid.near(Vec3::ZERO, Vec3::splat(f32::INFINITY)), vec![0, 1]);
    }
}
