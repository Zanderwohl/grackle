use bevy::app::App;
use bevy::prelude::*;
use crate::editor::editable::{FeatureId, FeatureTimeline};
use crate::editor::input::CurrentMouseInput;
use crate::tool::point_drag::PointDragState;
use crate::tool::room::RoomDragState;
use crate::tool::show::GizmoVisibility;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

pub struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<SelectionState>()
            .add_systems(Update, (
                Self::select.run_if(in_state(Tools::Select)),
                Self::draw_hover,
            ).chain())
        ;
    }
}

impl SelectionPlugin {
    fn select(
        mut state: ResMut<SelectionState>,
        mouse_input: Res<CurrentMouseInput>,
        mut features: ResMut<FeatureTimeline>,
        visibility: Res<GizmoVisibility>,
        point_drag: Res<PointDragState>,
        room_drag: Res<RoomDragState>,
    ) {
        state.hovered = None;

        if let Some(ray) = mouse_input.world_pos {
            if let Some((feature_id, hit_pos)) = find_nearest_feature_hit(&ray, &features, &visibility) {
                state.hovered = Some((feature_id, hit_pos));
            }

            let any_drag = point_drag.is_dragging() || room_drag.is_dragging();
            if mouse_input.released == Some(MouseButton::Left) && !any_drag {
                let selection = state.hovered.map(|(id, _)| id);
                features.select(selection);
            }
        }
    }

    fn draw_hover(
        state: Res<SelectionState>,
        features: Res<FeatureTimeline>,
        mut gizmos: Gizmos,
    ) {
        let Some((feature_id, hit_pos)) = state.hovered else { return; };
        let Some(feature) = features.get_feature(&feature_id) else { return; };

        let highlight = Color::srgb_u8(0, 230, 0);

        match feature.object().type_key() {
            "editor_room" => {
                if let Some((min, max)) = feature.object().drag_handle_bounds() {
                    bounds_gizmo(&mut gizmos, min, max, highlight);
                }
            }
            _ => {
                gizmos.sphere(Isometry3d::from_translation(hit_pos), 0.15, highlight);
            }
        }
    }
}

#[derive(Component)]
pub struct EditorSelectable {
    pub id: String,
    pub bounding_box: Cuboid,
}

#[derive(Resource, Default)]
pub struct SelectionState {
    pub hovered: Option<(FeatureId, Vec3)>,
}
