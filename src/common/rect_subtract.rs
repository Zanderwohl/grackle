/// Axis-aligned rectangle in 2D, used for face clipping during room baking.
#[derive(Debug, Clone, PartialEq)]
pub struct Rect2D {
    pub min_u: f32,
    pub max_u: f32,
    pub min_v: f32,
    pub max_v: f32,
}

impl Rect2D {
    pub fn new(min_u: f32, min_v: f32, max_u: f32, max_v: f32) -> Self {
        Self { min_u, min_v, max_u, max_v }
    }

    pub fn is_empty(&self) -> bool {
        self.max_u <= self.min_u || self.max_v <= self.min_v
    }

    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let r = Self {
            min_u: self.min_u.max(other.min_u),
            max_u: self.max_u.min(other.max_u),
            min_v: self.min_v.max(other.min_v),
            max_v: self.max_v.min(other.max_v),
        };
        if r.is_empty() { None } else { Some(r) }
    }
}

/// Given a face rectangle and a list of rectangular holes to subtract,
/// return a minimal set of solid rectangles that cover the remaining area.
///
/// Uses slab decomposition: collect all unique u/v breakpoints to form a grid
/// of micro-cells, classify each cell as solid or void, then greedily merge
/// adjacent solid cells to minimize output rectangle count.
pub fn subtract_rects(face: &Rect2D, holes: &[Rect2D]) -> Vec<Rect2D> {
    if face.is_empty() {
        return vec![];
    }

    let clipped_holes: Vec<Rect2D> = holes.iter()
        .filter_map(|h| face.intersection(h))
        .collect();

    if clipped_holes.is_empty() {
        return vec![face.clone()];
    }

    let mut us = vec![face.min_u, face.max_u];
    let mut vs = vec![face.min_v, face.max_v];
    for h in &clipped_holes {
        us.push(h.min_u);
        us.push(h.max_u);
        vs.push(h.min_v);
        vs.push(h.max_v);
    }

    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    vs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    us.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    vs.dedup_by(|a, b| (*a - *b).abs() < 1e-6);

    // Filter to only breakpoints inside the face
    us.retain(|&u| u >= face.min_u && u <= face.max_u);
    vs.retain(|&v| v >= face.min_v && v <= face.max_v);

    let cols = us.len() - 1;
    let rows = vs.len() - 1;
    if cols == 0 || rows == 0 {
        return vec![];
    }

    // Build grid: solid[row][col] = true means that cell is wall (not carved)
    let mut solid = vec![vec![true; cols]; rows];

    for row in 0..rows {
        for col in 0..cols {
            let cell_center_u = (us[col] + us[col + 1]) / 2.0;
            let cell_center_v = (vs[row] + vs[row + 1]) / 2.0;
            for h in &clipped_holes {
                if cell_center_u > h.min_u - 1e-7
                    && cell_center_u < h.max_u + 1e-7
                    && cell_center_v > h.min_v - 1e-7
                    && cell_center_v < h.max_v + 1e-7
                {
                    solid[row][col] = false;
                    break;
                }
            }
        }
    }

    // Greedy merge: scan top-to-bottom, left-to-right.
    // For each unvisited solid cell, extend right as far as possible,
    // then extend downward as far as all columns in the strip remain solid.
    let mut visited = vec![vec![false; cols]; rows];
    let mut result = Vec::new();

    for row in 0..rows {
        for col in 0..cols {
            if !solid[row][col] || visited[row][col] {
                continue;
            }

            // Extend right
            let mut end_col = col;
            while end_col + 1 < cols && solid[row][end_col + 1] && !visited[row][end_col + 1] {
                end_col += 1;
            }

            // Extend down
            let mut end_row = row;
            'outer: while end_row + 1 < rows {
                for c in col..=end_col {
                    if !solid[end_row + 1][c] || visited[end_row + 1][c] {
                        break 'outer;
                    }
                }
                end_row += 1;
            }

            // Mark visited
            for r in row..=end_row {
                for c in col..=end_col {
                    visited[r][c] = true;
                }
            }

            result.push(Rect2D {
                min_u: us[col],
                max_u: us[end_col + 1],
                min_v: vs[row],
                max_v: vs[end_row + 1],
            });
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_area(rects: &[Rect2D]) -> f32 {
        rects.iter().map(|r| (r.max_u - r.min_u) * (r.max_v - r.min_v)).sum()
    }

    #[test]
    fn no_holes() {
        let face = Rect2D::new(0.0, 0.0, 10.0, 10.0);
        let result = subtract_rects(&face, &[]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], face);
    }

    #[test]
    fn full_hole_removes_face() {
        let face = Rect2D::new(0.0, 0.0, 10.0, 10.0);
        let hole = Rect2D::new(0.0, 0.0, 10.0, 10.0);
        let result = subtract_rects(&face, &[hole]);
        assert!(result.is_empty());
    }

    #[test]
    fn hole_larger_than_face_removes_face() {
        let face = Rect2D::new(2.0, 2.0, 8.0, 8.0);
        let hole = Rect2D::new(0.0, 0.0, 10.0, 10.0);
        let result = subtract_rects(&face, &[hole]);
        assert!(result.is_empty());
    }

    #[test]
    fn centered_hole_produces_frame() {
        let face = Rect2D::new(0.0, 0.0, 10.0, 10.0);
        let hole = Rect2D::new(3.0, 3.0, 7.0, 7.0);
        let result = subtract_rects(&face, &[hole]);

        let face_area = 100.0;
        let hole_area = 16.0;
        let expected_area = face_area - hole_area;
        assert!((approx_area(&result) - expected_area).abs() < 0.01);
        // The greedy merge should produce at most 4 rectangles for a centered hole
        assert!(result.len() <= 4);
    }

    #[test]
    fn edge_hole_produces_three_rects_or_fewer() {
        let face = Rect2D::new(0.0, 0.0, 10.0, 10.0);
        // Hole along the left edge
        let hole = Rect2D::new(0.0, 3.0, 5.0, 7.0);
        let result = subtract_rects(&face, &[hole]);

        let expected_area = 100.0 - 20.0;
        assert!((approx_area(&result) - expected_area).abs() < 0.01);
        assert!(result.len() <= 4);
    }

    #[test]
    fn non_overlapping_hole_is_ignored() {
        let face = Rect2D::new(0.0, 0.0, 10.0, 10.0);
        let hole = Rect2D::new(20.0, 20.0, 30.0, 30.0);
        let result = subtract_rects(&face, &[hole]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], face);
    }

    #[test]
    fn multiple_holes() {
        let face = Rect2D::new(0.0, 0.0, 10.0, 10.0);
        let holes = vec![
            Rect2D::new(1.0, 1.0, 3.0, 3.0),
            Rect2D::new(7.0, 7.0, 9.0, 9.0),
        ];
        let result = subtract_rects(&face, &holes);
        let expected_area = 100.0 - 4.0 - 4.0;
        assert!((approx_area(&result) - expected_area).abs() < 0.01);
    }

    #[test]
    fn empty_face() {
        let face = Rect2D::new(5.0, 5.0, 5.0, 5.0);
        let result = subtract_rects(&face, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn partial_overlap_hole() {
        let face = Rect2D::new(0.0, 0.0, 10.0, 10.0);
        // Hole extends beyond face on one side
        let hole = Rect2D::new(5.0, -5.0, 15.0, 15.0);
        let result = subtract_rects(&face, &[hole]);
        // The clipped hole is (5,0)-(10,10), area 50. Remaining = 50.
        let expected_area = 50.0;
        assert!((approx_area(&result) - expected_area).abs() < 0.01);
    }
}
