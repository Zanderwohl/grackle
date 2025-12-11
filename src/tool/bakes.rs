use bevy::prelude::*;
use bevy_egui::egui;
use crate::get;
use crate::tool::room::CalculateRoomGeometry;

pub struct BakePlugin;

impl Plugin for BakePlugin {
    fn build(&self, app: &mut App) {
        app
            // UI moved to panels.rs Bakes tab
            // .add_systems(EguiPrimaryContextPass, Self::bake_ui)
            .add_message::<CalculateRoomGeometry>()
        ;
    }
}

impl BakePlugin {
    pub fn ui(ui: &mut egui::Ui) -> BakeCommands {
        let mut commands = BakeCommands::default();
        ui.vertical(|ui| {
            if ui.button(get!("bakes.room_geometry")).clicked() {
                commands.calculate_room_geometry = true;
            }
        });
        commands
    }
}

#[derive(Default)]
pub struct BakeCommands {
    pub calculate_room_geometry: bool,
}

/*
fn bake_ui(
    mut contexts: EguiContexts,
    mut room_events: MessageWriter<CalculateRoomGeometry>
) {
    let ctx = contexts.ctx_mut();
    if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return; }
    let ctx = ctx. unwrap();
    
    egui::Window::new(get!("bakes.title")).show(ctx, |ui| {
       ui.vertical(|ui| {
           if ui.button(get!("bakes.room_geometry")).clicked() {
               room_events.write(CalculateRoomGeometry);
           }
       });
    });
}
*/
