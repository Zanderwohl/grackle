use bevy::app::App;
use bevy::prelude::*;
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::global_point::GlobalPoint;
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct PointPlugin;

impl Plugin for PointPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<GlobalPoint>(app);
    }
}

impl PlaceablePoint for GlobalPoint {
    const TOOL: Tools = Tools::Point;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Box::new(GlobalPoint::new(position.x, position.y, position.z))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(GlobalPoint::from_point_ref(point_ref))
    }

    fn normal_color() -> Color {
        Color::srgb_u8(60, 120, 255)
    }

    fn relative_color() -> Color {
        Color::srgb_u8(80, 140, 255)
    }

    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        gizmos.sphere(Isometry3d::from_translation(cursor), 0.15, color);
    }
}
