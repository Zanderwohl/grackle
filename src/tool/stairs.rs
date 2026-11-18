//! Places [`EditorStairs`]: a click puts a flight's start there, and its other three points the
//! default size away from it, each naming whatever the start names.

use bevy::app::App;
use bevy::prelude::*;

use crate::editor::editable::{FeatureTrait, PointRef};
use crate::common::stairs::{StairsShape, StairsStyle};
use crate::editor::stairs::{draw_wedge, EditorStairs, DEFAULT_SIZE, STAIRS_COLOR};
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct StairsToolPlugin;

impl Plugin for StairsToolPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<EditorStairs>(app);
    }
}

impl PlaceablePoint for EditorStairs {
    const TOOL: Tools = Tools::Stairs;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Self::from_point_ref(PointRef::absolute(position.x, position.y, position.z))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(EditorStairs::from_start(point_ref))
    }

    fn normal_color() -> Color {
        STAIRS_COLOR
    }

    fn relative_color() -> Color {
        Color::srgb(0.75, 0.95, 0.65)
    }

    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        let preview = StairsShape {
            start: cursor,
            end: cursor + Vec3::X * DEFAULT_SIZE,
            width: cursor + Vec3::Z * DEFAULT_SIZE,
            height: cursor + Vec3::Y * DEFAULT_SIZE,
            style: StairsStyle::default(),
        };
        let Ok(frame) = preview.frame() else { return };
        draw_wedge(gizmos, &frame, color);
    }
}
