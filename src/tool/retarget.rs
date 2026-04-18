use bevy::prelude::*;
use crate::editor::editable::{AxisRef, EditEvent, EditorActionId, EditorActions};
use crate::editor::input::CurrentMouseInput;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

pub struct RetargetPlugin;

impl Plugin for RetargetPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<RetargetState>()
            .add_systems(Update, (
                Self::interface,
                Self::draw_gizmos,
            ).chain().run_if(in_state(Tools::Retarget)))
            .add_systems(OnExit(Tools::Retarget), Self::on_exit);
    }
}

#[derive(Resource, Default)]
pub struct RetargetState {
    pub target_action: Option<EditorActionId>,
    pub target_point_ref_key: String,
    pub hovered_point: Option<(EditorActionId, String, Vec3)>,
}

impl RetargetPlugin {
    fn prior_actions(actions: &EditorActions, target: EditorActionId) -> Vec<EditorActionId> {
        let order = actions.action_order();
        let cursor = actions.active_actions().count();
        let target_idx = order.iter().position(|id| *id == target).unwrap_or(0);
        let end = target_idx.min(cursor);
        order[..end].to_vec()
    }

    fn interface(
        mut state: ResMut<RetargetState>,
        mouse_input: Res<CurrentMouseInput>,
        keys: Res<ButtonInput<KeyCode>>,
        mut actions: ResMut<EditorActions>,
        mut next_tool: ResMut<NextState<Tools>>,
        mut commands: Commands,
        mut edit_events: MessageWriter<EditEvent>,
    ) {
        let Some(target_action_id) = state.target_action else {
            next_tool.set(Tools::Select);
            return;
        };

        if keys.just_pressed(KeyCode::Escape) {
            next_tool.set(Tools::Select);
            return;
        }

        let allowed = Self::prior_actions(&actions, target_action_id);

        state.hovered_point = mouse_input.world_pos
            .and_then(|ray| find_hovered_point_filtered(&ray, &actions, PICK_RADIUS, &allowed));

        if mouse_input.released == Some(MouseButton::Left) {
            if let Some((ref_action_id, ref_key, ref_pos)) = state.hovered_point.clone() {
                let point_ref_key = state.target_point_ref_key.clone();

                if let Some(mut action) = actions.actions_mut().remove(&target_action_id) {
                    if let Some(pr) = action.object_mut().get_point_ref_mut(&point_ref_key) {
                        let old_base = pr.resolved_reference.unwrap_or(bevy::math::Vec3::ZERO);
                        let current_resolved = bevy::math::Vec3::new(
                            pr.x.resolve_with_base(Some(old_base.x)).unwrap_or(0.0),
                            pr.y.resolve_with_base(Some(old_base.y)).unwrap_or(0.0),
                            pr.z.resolve_with_base(Some(old_base.z)).unwrap_or(0.0),
                        );

                        pr.reference = Some(ref_action_id);
                        pr.point_key = ref_key;
                        pr.resolved_reference = Some(ref_pos);

                        pr.x = AxisRef::Relative(current_resolved.x - ref_pos.x);
                        pr.y = AxisRef::Relative(current_resolved.y - ref_pos.y);
                        pr.z = AxisRef::Relative(current_resolved.z - ref_pos.z);
                    }

                    action.object_mut().resolve_references(actions.actions_map());
                    let parents = action.object().parent_ids();

                    if let Some(entity) = action.object().entity() {
                        action.object_mut().apply_to_entity(&mut commands, entity);
                        edit_events.write(EditEvent {
                            editor_id: target_action_id._id(),
                            action_id: target_action_id,
                            entity,
                        });
                    }

                    actions.actions_mut().insert(target_action_id, action);

                    if let Some(action) = actions.actions_mut().get_mut(&target_action_id) {
                        action.set_parents(parents);
                    }
                }

                actions.select(Some(target_action_id));
                next_tool.set(Tools::Select);
            }
        }
    }

    fn draw_gizmos(
        state: Res<RetargetState>,
        actions: Res<EditorActions>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        let Some(target_action_id) = state.target_action else { return; };
        let Some(ray) = mouse_input.world_pos else { return; };

        let allowed = Self::prior_actions(&actions, target_action_id);
        draw_picking_gizmos_filtered(&mut gizmos, &ray, &actions, &state.hovered_point, &allowed);
    }

    fn on_exit(mut state: ResMut<RetargetState>) {
        state.target_action = None;
        state.target_point_ref_key.clear();
        state.hovered_point = None;
    }
}
