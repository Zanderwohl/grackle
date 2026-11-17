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
        mut cache: Local<Option<PickCache>>,
        mut last: Local<Option<LastPick>>,
    ) {
        state.hovered = None;

        if let Some(ray) = mouse_input.world_pos {
            let key = PickCache::key(features.last_changed(), &features);
            let hovered = match &*last {
                Some(seen) if seen.ray == ray && seen.key == key && seen.visibility == *visibility => seen.hovered,
                _ => {
                    let cache = PickCache::refresh(&mut cache, &features, key);
                    let hovered = find_nearest_feature_hit_in(&ray, &features, &visibility, cache);
                    *last = Some(LastPick { ray, key, visibility: visibility.clone(), hovered });
                    hovered
                }
            };
            state.hovered = hovered;

            let any_drag = point_drag.is_dragging()
                || room_drag.is_dragging()
                || footprint_drag.is_dragging()
                || rotate_drag.is_dragging();
            if mouse_input.released == Some(MouseButton::Left) && !any_drag {
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
        let Some((feature_id, hit_pos)) = state.hovered else { return; };
        let Some(feature) = features.get_feature(&feature_id) else { return; };

        let highlight = Color::srgb_u8(0, 230, 0);

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
}
