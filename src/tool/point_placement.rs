//! What a point-placing tool needs from its feature: how to make one at a
//! point, and what to draw where it would land. The picking is
//! [`crate::tool::map_wizard`]'s.

use bevy::prelude::*;

use crate::editor::editable::{FeatureTrait, PointRef};
use crate::tool::Tools;

pub trait PlaceablePoint: Send + Sync + 'static {
    const TOOL: Tools;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait>;

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait>;

    fn normal_color() -> Color;

    fn relative_color() -> Color;

    /// Draw the shape that would land at `cursor`, at its real size.
    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color);
}
