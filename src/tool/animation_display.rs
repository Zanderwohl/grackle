//! Places [`AnimationDisplay`] features.
//!
//! The point tool with a different feature at the end of it, the same way the
//! spawn point and prop tools are: placing a display *is* placing a point, so
//! relative placement and shift-picking work here without a mapper learning
//! anything new. Which state it holds is chosen afterwards, in the feature
//! panel, and which way it faces from the rotation rings in
//! [`crate::tool::rotate_drag`]. The shared half lives in
//! [`crate::tool::point_placement`].

use bevy::prelude::*;

use crate::common::skeleton::{default_humanoid, draw_skeleton, Pose, SkeletonPalette};
use crate::editor::animation_display::AnimationDisplay;
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct AnimationDisplayPlugin;

impl Plugin for AnimationDisplayPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<AnimationDisplay>(app);
    }
}

impl PlaceablePoint for AnimationDisplay {
    const TOOL: Tools = Tools::AnimationDisplay;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Box::new(AnimationDisplay::new(position.x, position.y, position.z))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(AnimationDisplay::from_point_ref(point_ref))
    }

    fn normal_color() -> Color {
        Color::srgb_u8(190, 140, 255)
    }

    fn relative_color() -> Color {
        Color::srgb_u8(220, 190, 255)
    }

    /// The body that would land here, at the size it would land at. A display
    /// is placed to be looked at from somewhere, so its footprint and its
    /// height matter before it is committed rather than after.
    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        draw_skeleton(
            gizmos,
            default_humanoid(),
            &Pose::rest(),
            &Transform::from_translation(cursor),
            &SkeletonPalette::flat(color),
        );
    }
}
