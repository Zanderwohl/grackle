use bevy::app::App;
use bevy::prelude::*;
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::grackle_point_light::GracklePointLight;
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct PointLightPlugin;

impl Plugin for PointLightPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<GracklePointLight>(app);
    }
}

impl PlaceablePoint for GracklePointLight {
    const TOOL: Tools = Tools::PointLight;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Box::new(GracklePointLight::new(position.x, position.y, position.z))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(GracklePointLight::from_point_ref(point_ref))
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
