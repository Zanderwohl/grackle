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
    pub mirrors: bool,
    pub triggers: bool,
    pub blocks: bool,
}

impl Default for GizmoVisibility {
    fn default() -> Self {
        Self {
            points: true,
            rooms: true,
            point_lights: true,
            spawn_points: true,
            props: true,
            animation_displays: true,
            animation_grids: true,
            mirrors: true,
            triggers: true,
            blocks: true,
        }
    }
}

impl GizmoVisibility {
    /// By `type_key`, so a Mirror's copy answers to its own kind's switch rather than the Mirror's.
    fn shows(&self, type_key: &str) -> bool {
        match type_key {
            "global_point" => self.points,
            "editor_room" | "footprint_room" => self.rooms,
            "grackle_point_light" => self.point_lights,
            "spawn_point" => self.spawn_points,
            "prop" | "weapon_pickup" => self.props,
            "animation_display" => self.animation_displays,
            "animation_grid" => self.animation_grids,
            "mirror" => self.mirrors,
            "trigger" => self.triggers,
            "block" => self.blocks,
            _ => false,
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
        ui.checkbox(&mut gizmo_visibility.mirrors, get!("show.gizmos_mirrors"));
        ui.checkbox(&mut gizmo_visibility.triggers, get!("show.gizmos_triggers"));
        ui.checkbox(&mut gizmo_visibility.blocks, get!("show.gizmos_blocks"));
    }

    fn draw_visible_gizmos(
        visibility: Res<GizmoVisibility>,
        feature: Res<FeatureTimeline>,
        mut gizmos: Gizmos,
    ) {
        if !visibility.points && !visibility.rooms && !visibility.point_lights
            && !visibility.spawn_points && !visibility.props && !visibility.animation_displays
            && !visibility.animation_grids && !visibility.mirrors && !visibility.triggers && !visibility.blocks
        {
            return;
        }

        for (_id, feature) in feature.active_features() {
            let object = feature.object();
            for drawn in std::iter::once(object).chain(object.standing_copies().into_iter().map(|copy| copy.object)) {
                if visibility.shows(drawn.type_key()) {
                    drawn.debug_gizmos(&mut gizmos);
                }
            }
        }
    }
}
