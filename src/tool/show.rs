use bevy::prelude::*;
use crate::get;
use bevy_egui::egui;
use crate::editor::editable::EditorActions;
use crate::editor::multicam::MulticamState;

#[derive(Resource)]
pub struct GizmoVisibility {
    pub points: bool,
    pub rooms: bool,
    pub point_lights: bool,
}

impl Default for GizmoVisibility {
    fn default() -> Self {
        Self {
            points: false,
            rooms: false,
            point_lights: false,
        }
    }
}

pub struct ShowPlugin;

impl Plugin for ShowPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<GizmoVisibility>()
            .add_systems(Update, Self::draw_visible_gizmos)
        ;
    }
}

impl ShowPlugin {
    pub fn ui(
        ui: &mut egui::Ui,
        multicam_state: &mut MulticamState,
        gizmo_visibility: &mut GizmoVisibility,
    ) {
        ui.heading(get!("show.cameras"));
        ui.checkbox(&mut multicam_state.draw_ortho_cameras, get!("show.ortho_cameras"));
        ui.checkbox(&mut multicam_state.draw_perspective_cameras, get!("show.perspective_cameras"));

        ui.separator();
        ui.heading(get!("show.gizmos"));
        ui.checkbox(&mut gizmo_visibility.points, get!("show.gizmos.points"));
        ui.checkbox(&mut gizmo_visibility.rooms, get!("show.gizmos.rooms"));
        ui.checkbox(&mut gizmo_visibility.point_lights, get!("show.gizmos.point_lights"));
    }

    fn draw_visible_gizmos(
        visibility: Res<GizmoVisibility>,
        actions: Res<EditorActions>,
        mut gizmos: Gizmos,
    ) {
        if !visibility.points && !visibility.rooms && !visibility.point_lights {
            return;
        }

        for (_id, action) in actions.active_actions() {
            let key = action.object().type_key();
            let draw = match key {
                "global_point" => visibility.points,
                "editor_room" => visibility.rooms,
                "grackle_point_light" => visibility.point_lights,
                _ => false,
            };
            if draw {
                action.object().debug_gizmos(&mut gizmos);
            }
        }
    }
}
