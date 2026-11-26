use bevy::app::App;
use bevy::prelude::*;
use crate::common::app_mode::AppMode;
use crate::editor::editable::{FeatureId, FeatureTimeline};
use crate::editor::input::CurrentMouseInput;
use crate::tool::footprint_drag::FootprintDragState;
use crate::tool::point_drag::PointDragState;
use crate::tool::room::RoomDragState;
use crate::tool::rotate_drag::RotateDragState;
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
                Self::draw_hover.run_if(not(in_state(Tools::Spectate))),
            ).chain().run_if(in_state(AppMode::Editor)))
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
        footprint_drag: Res<FootprintDragState>,
        rotate_drag: Res<RotateDragState>,
        mut products: ResMut<crate::tool::map_products::MapProducts>,
        mut last: Local<Option<LastPick>>,
        actions: Res<crate::common::keybind::Actions>,
    ) {
        state.hovered = None;
        state.hovered_point = None;

        let any_drag = point_drag.is_dragging()
            || room_drag.is_dragging()
            || footprint_drag.is_dragging()
            || rotate_drag.is_dragging();
        // A drag edits the timeline every frame, so a fresh pick would rebuild every product and
        // cut room surfaces again each frame for a highlight nobody is choosing from.
        if any_drag {
            state.hovered = last.as_ref().and_then(|seen| seen.hovered);
            return;
        }

        if let Some(ray) = mouse_input.world_pos {
            let key = PickCache::key(features.last_changed(), &features);
            let hovered = match &*last {
                Some(seen) if seen.ray == ray && seen.key == key && seen.visibility == *visibility => seen.hovered,
                _ => {
                    products.refresh(&features, key);
                    let hovered = products.hit(&ray, &visibility);
                    *last = Some(LastPick { ray, key, visibility: visibility.clone(), hovered });
                    hovered
                }
            };
            state.hovered = hovered;
            // With Shift, a named point: what a tool started next measures from.
            if actions.pressed(crate::common::keybind::Action::Modify) {
                products.refresh(&features, key);
                state.hovered_point = products.point_under(&features, &ray, PICK_RADIUS);
                if mouse_input.released == Some(MouseButton::Left) {
                    state.point = match state.hovered_point.clone() {
                        Some((id, _)) if state.point.as_ref() == Some(&id) => None,
                        picked => picked.map(|(id, _)| id),
                    };
                }
                return;
            }
            if mouse_input.released == Some(MouseButton::Left) {
                state.point = None;
                let selection = state.hovered.map(|(id, _)| id);
                // Not an edit, so not a change to the undo checkpoint or `map_sync`.
                features.bypass_change_detection().select(selection);
            }
        }
    }

    fn draw_hover(
        state: Res<SelectionState>,
        features: Res<FeatureTimeline>,
        mut gizmos: Gizmos,
    ) {
        let highlight = Color::srgb_u8(0, 230, 0);
        // From the timeline, which an edit made from a panel has already moved.
        let selected_point = state.point.as_ref().and_then(|id| {
            let mut point = crate::editor::editable::PointRef::reference(id.feature);
            point.point_key = id.key.clone();
            point.resolve(features.features_map()).ok()
        });
        for at in selected_point.into_iter().chain(state.hovered_point.as_ref().map(|(_, at)| *at)) {
            gizmos.sphere(Isometry3d::from_translation(at), 0.12, highlight);
        }
        let Some((feature_id, hit_pos)) = state.hovered else { return; };
        let Some(feature) = features.get_feature(&feature_id) else { return; };

        let volumes = feature.object().volumes();
        if !volumes.is_empty() {
            for volume in &volumes {
                for (a, b) in volume.shape.edges() {
                    gizmos.line(a, b, highlight);
                }
            }
            return;
        }
        let pick_boxes = feature.object().pick_boxes();
        if pick_boxes.is_empty() {
            gizmos.sphere(Isometry3d::from_translation(hit_pos), 0.15, highlight);
        }
        for pick_box in pick_boxes {
            gizmos.cube(pick_box.transform(), highlight);
        }
    }
}

/// What the pointer was last over, so a still pointer over a still map is not picked again.
struct LastPick {
    ray: Ray3d,
    key: (bevy::ecs::change_detection::Tick, u64),
    visibility: GizmoVisibility,
    hovered: Option<(FeatureId, Vec3)>,
}

#[derive(Component)]
pub struct EditorSelectable {
    pub id: String,
    pub bounding_box: Cuboid,
}

#[derive(Resource, Default)]
pub struct SelectionState {
    pub hovered: Option<(FeatureId, Vec3)>,
    /// A named point picked with Shift, beside the feature selection; a plain
    /// click clears it.
    pub point: Option<crate::tool::map_products::PointId>,
    pub hovered_point: Option<(crate::tool::map_products::PointId, Vec3)>,
}
