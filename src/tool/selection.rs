use bevy::app::App;
use bevy::prelude::*;
use crate::editor::editable::{EditorActionId, EditorActions};
use crate::editor::input::CurrentMouseInput;
use crate::tool::point_drag::PointDragState;
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
        mut actions: ResMut<EditorActions>,
        visibility: Res<GizmoVisibility>,
        drag_state: Res<PointDragState>,
    ) {
        state.hovered = None;

        if let Some(ray) = mouse_input.world_pos {
            if let Some((action_id, hit_pos)) = find_nearest_action_hit(&ray, &actions, &visibility) {
                state.hovered = Some((action_id, hit_pos));
            }

            if mouse_input.released == Some(MouseButton::Left) && !drag_state.is_dragging() {
                let selection = state.hovered.map(|(id, _)| id);
                actions.select(selection);
            }
        }
    }

    fn draw_hover(
        state: Res<SelectionState>,
        actions: Res<EditorActions>,
        mut gizmos: Gizmos,
    ) {
        let Some((action_id, hit_pos)) = state.hovered else { return; };
        let Some(action) = actions.get_action(&action_id) else { return; };

        let highlight = Color::srgb_u8(0, 230, 0);

        match action.object().type_key() {
            "editor_room" => {
                if let Some((min, max)) = action.object().drag_handle_bounds() {
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
    pub hovered: Option<(EditorActionId, Vec3)>,
}
