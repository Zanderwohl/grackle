use bevy::prelude::*;
use crate::common::app_mode::AppMode;
use crate::get;
use bevy_egui::egui;
use crate::editor::editable::FeatureTimeline;
use crate::editor::multicam::MulticamState;

#[derive(Resource)]
pub struct GizmoVisibility {
    pub points: bool,
    pub rooms: bool,
    pub point_lights: bool,
    pub spawn_points: bool,
    pub props: bool,
    pub animation_displays: bool,
    pub animation_grids: bool,
}

impl Default for GizmoVisibility {
    fn default() -> Self {
        Self {
            points: false,
            rooms: false,
            point_lights: false,
            spawn_points: false,
            props: false,
            animation_displays: false,
            animation_grids: false,
        }
    }
}

pub struct ShowPlugin;

impl Plugin for ShowPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<GizmoVisibility>()
            .add_systems(Update, Self::draw_visible_gizmos.run_if(in_state(AppMode::Editor)))
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
        ui.heading(get!("show.gizmos_title"));
        ui.checkbox(&mut gizmo_visibility.points, get!("show.gizmos_points"));
        ui.checkbox(&mut gizmo_visibility.rooms, get!("show.gizmos_rooms"));
        ui.checkbox(&mut gizmo_visibility.point_lights, get!("show.gizmos_point_lights"));
        ui.checkbox(&mut gizmo_visibility.spawn_points, get!("show.gizmos_spawn_points"));
        ui.checkbox(&mut gizmo_visibility.props, get!("show.gizmos_props"));
        ui.checkbox(&mut gizmo_visibility.animation_displays, get!("show.gizmos_animation_displays"));
        ui.checkbox(&mut gizmo_visibility.animation_grids, get!("show.gizmos_animation_grids"));
    }

    fn draw_visible_gizmos(
        visibility: Res<GizmoVisibility>,
        feature: Res<FeatureTimeline>,
        mut gizmos: Gizmos,
    ) {
        if !visibility.points && !visibility.rooms && !visibility.point_lights
            && !visibility.spawn_points && !visibility.props && !visibility.animation_displays
            && !visibility.animation_grids
        {
            return;
        }

        for (_id, feature) in feature.active_features() {
            let key = feature.object().type_key();
            let draw = match key {
                "global_point" => visibility.points,
                "editor_room" => visibility.rooms,
                "grackle_point_light" => visibility.point_lights,
                "spawn_point" => visibility.spawn_points,
                "prop" => visibility.props,
                "animation_display" => visibility.animation_displays,
                "animation_grid" => visibility.animation_grids,
                _ => false,
            };
            if draw {
                feature.object().debug_gizmos(&mut gizmos);
            }
        }
    }
}
