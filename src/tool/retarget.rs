use bevy::prelude::*;
use crate::editor::editable::{AxisRef, EditEvent, FeatureId, FeatureHistory};
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
    pub target_feature: Option<FeatureId>,
    pub target_point_ref_key: String,
    pub hovered_point: Option<(FeatureId, String, Vec3)>,
}

impl RetargetPlugin {
    fn prior_features(features: &FeatureHistory, target: FeatureId) -> Vec<FeatureId> {
        let order = features.feature_order();
        let cursor = features.active_features().count();
        let target_idx = order.iter().position(|id| *id == target).unwrap_or(0);
        let end = target_idx.min(cursor);
        order[..end].to_vec()
    }

    fn interface(
        mut state: ResMut<RetargetState>,
        mouse_input: Res<CurrentMouseInput>,
        keys: Res<ButtonInput<KeyCode>>,
        mut features: ResMut<FeatureHistory>,
        mut next_tool: ResMut<NextState<Tools>>,
        mut commands: Commands,
        mut edit_events: MessageWriter<EditEvent>,
    ) {
        let Some(target_feature_id) = state.target_feature else {
            next_tool.set(Tools::Select);
            return;
        };

        if keys.just_pressed(KeyCode::Escape) {
            next_tool.set(Tools::Select);
            return;
        }

        let allowed = Self::prior_features(&features, target_feature_id);

        state.hovered_point = mouse_input.world_pos
            .and_then(|ray| find_hovered_point_filtered(&ray, &features, PICK_RADIUS, &allowed));

        if mouse_input.released == Some(MouseButton::Left) {
            if let Some((ref_feature_id, ref_key, ref_pos)) = state.hovered_point.clone() {
                let point_ref_key = state.target_point_ref_key.clone();

                if let Some(mut feature) = features.features_mut().remove(&target_feature_id) {
                    if let Some(pr) = feature.object_mut().get_point_ref_mut(&point_ref_key) {
                        let old_base = pr.resolved_reference.unwrap_or(bevy::math::Vec3::ZERO);
                        let current_resolved = bevy::math::Vec3::new(
                            pr.x.resolve_with_base(Some(old_base.x)).unwrap_or(0.0),
                            pr.y.resolve_with_base(Some(old_base.y)).unwrap_or(0.0),
                            pr.z.resolve_with_base(Some(old_base.z)).unwrap_or(0.0),
                        );

                        pr.reference = Some(ref_feature_id);
                        pr.point_key = ref_key;
                        pr.resolved_reference = Some(ref_pos);

                        pr.x = AxisRef::Relative(current_resolved.x - ref_pos.x);
                        pr.y = AxisRef::Relative(current_resolved.y - ref_pos.y);
                        pr.z = AxisRef::Relative(current_resolved.z - ref_pos.z);
                    }

                    feature.object_mut().resolve_references(features.features_map());
                    let parents = feature.object().parent_ids();

                    if let Some(entity) = feature.object().entity() {
                        feature.object_mut().apply_to_entity(&mut commands, entity);
                        edit_events.write(EditEvent {
                            editor_id: target_feature_id._id(),
                            feature_id: target_feature_id,
                            entity,
                        });
                    }

                    features.features_mut().insert(target_feature_id, feature);

                    if let Some(feat) = features.features_mut().get_mut(&target_feature_id) {
                        feat.set_parents(parents);
                    }
                }

                features.select(Some(target_feature_id));
                next_tool.set(Tools::Select);
            }
        }
    }

    fn draw_gizmos(
        state: Res<RetargetState>,
        features: Res<FeatureHistory>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        let Some(target_feature_id) = state.target_feature else { return; };
        let Some(ray) = mouse_input.world_pos else { return; };

        let allowed = Self::prior_features(&features, target_feature_id);
        draw_picking_gizmos_filtered(&mut gizmos, &ray, &features, &state.hovered_point, &allowed);
    }

    fn on_exit(mut state: ResMut<RetargetState>) {
        state.target_feature = None;
        state.target_point_ref_key.clear();
        state.hovered_point = None;
    }
}
