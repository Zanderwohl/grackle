use bevy::app::App;
use bevy::prelude::*;
use bevy_egui::egui;
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter};
use crate::get;
use crate::tool::bakes::BakePlugin;
use crate::tool::movement::MovementPlugin;
use crate::tool::room::RoomPlugin;
use crate::tool::selection::SelectionPlugin;
use crate::tool::show::ShowPlugin;

pub mod selection;
pub mod room;
pub mod movement;
pub mod bakes;
pub mod show;

pub struct ToolPlugin;

impl Plugin for ToolPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<ToolData>()
            .init_state::<Tools>()
            .add_plugins(ShowPlugin)
            .add_plugins(BakePlugin)
            .add_plugins(MovementPlugin)
            .add_plugins(SelectionPlugin)
            .add_plugins(RoomPlugin)
            // Toolbar moved to panels.rs Tools tab
            // .add_systems(EguiPrimaryContextPass, Self::toolbar)
        ;
    }
}

#[derive(Resource)]
pub struct ToolData {
}

impl Default for ToolData {
    fn default() -> Self {
        Self {

        }
    }
}

#[derive(EnumIter, States, Debug, Display, Clone, PartialEq, Eq, Hash, Default)]
pub enum Tools {
    #[default]
    Select,
    Room,
}

impl Tools {
    pub fn name(&self) -> String {
        match self {
            Self::Select => get!("tools.select"),
            Self::Room => get!("tools.room"),
        }
    }
    
    pub fn ui(
        ui: &mut egui::Ui,
        current_tool: &State<Self>,
        next_tool: &mut NextState<Self>,
    ) {
        egui::Grid::new("tools").show(ui, |ui| {
            for item in Self::iter() {
                if current_tool.eq(&item) {
                    ui.scope(|ui| {
                        ui.disable();
                        let _ = ui.button(item.name());
                    });
                } else {
                    if ui.button(item.name()).clicked() {
                        next_tool.set(item);
                    }
                }
            }
        });
    }
}

impl ToolPlugin {
    // Toolbar UI moved to panels.rs Tools tab
    /*
    fn toolbar(
        mut contexts: EguiContexts,
        current_tool: Res<State<Tools>>,
        mut next_tool: ResMut<NextState<Tools>>,
    ) {
        let ctx = contexts.ctx_mut();
        if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return; }
        let ctx = ctx.unwrap();

        egui::Window::new(get!("tools.title")).show(ctx, |ui| {
           egui::Grid::new("tools").show(ui, |ui| {
               for item in Tools::iter() {
                   if current_tool.eq(&item) {
                       ui.scope(|ui| {
                           ui.disable();
                           let _ = ui.button(item.name());
                       });
                   } else {
                       if ui.button(item.name()).clicked() {
                           next_tool.set(item);
                       }
                   }
               }
           })
        });
    }
    */
}
