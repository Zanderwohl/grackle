//! The Stairs tool's four points, picked by [`crate::tool::map_wizard`]:
//! start, end, a width point and a height point, each kept where it can make
//! stairs.

use bevy::prelude::*;

use crate::common::stairs::{StairsShape, StairsStyle};
use crate::editor::editable::{FeatureTimeline, FeatureTrait, PointRef};
use crate::editor::stairs::{draw_collider, EditorStairs, STAIRS_COLOR};
use crate::tool::map_wizard::MapPick;
use crate::tool::tool_helpers::dashed_line;
use crate::tool::Tools;

/// Meters. The height point is kept at least this far above the footprint while picking.
pub const LEAST_HEIGHT: f32 = 0.1;

/// Meters. A run or width shorter than this is refused.
const SHORTEST: f32 = 1.0e-3;

/// The footprint plane's normal through start, end and the width point, upward where it has an
/// upward side. `None` while the three are in a line.
fn footprint_up(start: Vec3, end: Vec3, width: Vec3) -> Option<Vec3> {
    let along = end - start;
    let run = along.length();
    if run < SHORTEST {
        return None;
    }
    let normal = (along / run).cross(width - start);
    if normal.length() < SHORTEST {
        return None;
    }
    let normal = normal.normalize();
    Some(if normal.y < 0.0 { -normal } else { normal })
}

/// `point`, moved along `up` until it is at least [`LEAST_HEIGHT`] above the plane through `start`.
fn lift(point: Vec3, start: Vec3, up: Vec3) -> Vec3 {
    let height = (point - start).dot(up);
    if height < LEAST_HEIGHT { point + up * (LEAST_HEIGHT - height) } else { point }
}

/// Where a pick at `cursor` lands, given the points before it: the height point lifted off the
/// footprint. `None` for an end on the start or a width point in line with the run.
pub fn placed(points: &[Vec3], cursor: Vec3) -> Option<Vec3> {
    match points {
        [start] => ((cursor - *start).length() >= SHORTEST).then_some(cursor),
        [start, end] => footprint_up(*start, *end, cursor).map(|_| cursor),
        [start, end, width] => Some(footprint_up(*start, *end, *width).map_or(cursor, |up| lift(cursor, *start, up))),
        _ => Some(cursor),
    }
}

pub fn build(_: &Tools, steps: &[Vec<MapPick>], _: &FeatureTimeline) -> Option<Box<dyn FeatureTrait>> {
    let points: Vec<PointRef> = steps
        .iter()
        .filter_map(|picks| match picks.first()? {
            MapPick::Point { point, .. } => Some(point.clone()),
            _ => None,
        })
        .collect();
    let [start, end, width, height] = <[PointRef; 4]>::try_from(points).ok()?;
    Some(Box::new(EditorStairs::new(start, end, width, height)))
}

pub fn preview(gizmos: &mut Gizmos, points: &[Vec3], at: Vec3) {
    match points {
        [start] => {
            gizmos.line(*start, at, STAIRS_COLOR);
        }
        [start, end] => {
            gizmos.line(*start, *end, STAIRS_COLOR);
            if let Some(along) = (*end - *start).try_normalize() {
                let foot = *start + along * (at - *start).dot(along);
                dashed_line(gizmos, foot, at, STAIRS_COLOR, 0.15, 0.1);
            }
        }
        [start, end, width] => {
            let shape = StairsShape { start: *start, end: *end, width: *width, height: at, style: StairsStyle::default() };
            draw_collider(gizmos, &shape, STAIRS_COLOR);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_height_point_is_kept_off_the_footprint() {
        let (start, end, width) = (Vec3::ZERO, Vec3::X * 4.0, Vec3::new(0.0, 0.0, -2.0));
        let up = footprint_up(start, end, width).unwrap();
        assert!(up.abs_diff_eq(Vec3::Y, 1e-5), "{up}");
        assert!(lift(Vec3::new(2.0, -1.0, 0.0), start, up).abs_diff_eq(Vec3::new(2.0, LEAST_HEIGHT, 0.0), 1e-5));
        assert_eq!(lift(Vec3::new(2.0, 3.0, 0.0), start, up), Vec3::new(2.0, 3.0, 0.0));
    }

    #[test]
    fn a_width_point_in_line_with_the_run_is_refused() {
        let picked = [Vec3::ZERO, Vec3::X * 4.0];
        assert!(placed(&picked, Vec3::X * 2.0).is_none());
        assert!(placed(&picked, Vec3::new(2.0, 0.0, 1.0)).is_some());
        assert!(placed(&picked[..1], Vec3::ZERO).is_none());
    }
}
