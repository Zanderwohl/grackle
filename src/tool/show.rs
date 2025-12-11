use bevy::prelude::*;
use crate::get;
use bevy_egui::egui;
use crate::editor::multicam::MulticamState;

pub struct ShowPlugin;

impl Plugin for ShowPlugin {
    fn build(&self, app: &mut App) {
        app
            // UI moved to panels.rs Show tab
            // .add_systems(EguiPrimaryContextPass, Self::ui)
        ;
    }
}

impl ShowPlugin {
    pub fn ui(
        ui: &mut egui::Ui,
        multicam_state: &mut MulticamState,
    ) {
        ui.heading(get!("show.cameras"));
        ui.checkbox(&mut multicam_state.draw_ortho_cameras, get!("show.ortho_cameras"));
        ui.checkbox(&mut multicam_state.draw_perspective_cameras, get!("show.perspective_cameras"));
    }
    
    /*
    fn ui(
        mut contexts: EguiContexts,
        mut multicam_state: ResMut<MulticamState>,
    ) {
        let ctx = contexts.ctx_mut();
        if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return; }
        let ctx = ctx.unwrap();
        
        egui::Window::new(get!("show.title")).show(ctx, |ui| {
            ui.heading(get!("show.cameras"));
            ui.checkbox(&mut multicam_state.draw_ortho_cameras, get!("show.ortho_cameras"));
            ui.checkbox(&mut multicam_state.draw_perspective_cameras, get!("show.perspective_cameras"));
        });
    }
    */
}
