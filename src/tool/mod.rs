use bevy::app::App;
use bevy::prelude::*;
use bevy_egui::egui;
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter};
use crate::get;
use crate::tool::animation_display::AnimationDisplayPlugin;
use crate::tool::animation_grid::AnimationGridPlugin;
use crate::tool::bakes::BakePlugin;
use crate::tool::movement::MovementPlugin;
use crate::tool::point::PointPlugin;
use crate::tool::spawn_point::SpawnPointPlugin;
use crate::tool::point_drag::PointDragPlugin;
use crate::tool::prop::PropPlugin;
use crate::tool::rotate_drag::RotateDragPlugin;
use crate::tool::point_light::PointLightPlugin;
use crate::tool::retarget::RetargetPlugin;
use crate::tool::room::RoomPlugin;
use crate::tool::selection::SelectionPlugin;
use crate::tool::show::ShowPlugin;

pub mod selection;
pub mod point;
pub mod spawn_point;
pub mod point_light;
pub mod point_drag;
pub mod animation_display;
pub mod animation_grid;
pub mod point_placement;
pub mod prop;
pub mod rotate_drag;
pub mod retarget;
pub mod room;
pub mod movement;
pub mod bakes;
pub mod show;
pub mod tool_helpers;

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
            .add_plugins(PointPlugin)
            .add_plugins(SpawnPointPlugin)
            .add_plugins(PointLightPlugin)
            .add_plugins(PointDragPlugin)
            .add_plugins(RotateDragPlugin)
            .add_plugins(PropPlugin)
            .add_plugins(AnimationDisplayPlugin)
            .add_plugins(AnimationGridPlugin)
            .add_plugins(RetargetPlugin)
            .add_plugins(RoomPlugin);
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
    Point,
    PointLight,
    SpawnPoint,
    Prop,
    AnimationDisplay,
    AnimationGrid,
    Room,
    Retarget,
}

impl Tools {
    pub fn name(&self) -> String {
        match self {
            Self::Select => get!("tools.select"),
            Self::Point => get!("tools.point"),
            Self::PointLight => get!("tools.point_light"),
            Self::SpawnPoint => get!("tools.spawn_point"),
            Self::Prop => get!("tools.prop"),
            Self::AnimationDisplay => get!("tools.animation_display"),
            Self::AnimationGrid => get!("tools.animation_grid"),
            Self::Room => get!("tools.room"),
            Self::Retarget => "Retarget".into(),
        }
    }

    fn is_hidden(&self) -> bool {
        matches!(self, Self::Retarget)
    }
    
    /// `TOOLBAR_HEIGHT` in [`crate::editor::panels`] is derived from these.
    pub const BUTTON_HEIGHT: f32 = 28.0;
    pub const BUTTON_MIN_WIDTH: f32 = 72.0;
    pub const ROW_PADDING: f32 = 6.0;

    pub fn ui(
        ui: &mut egui::Ui,
        current_tool: &State<Self>,
        next_tool: &mut NextState<Self>,
    ) {
        ui.add_space(Self::ROW_PADDING);
        // Wrapped, not a `Grid`, so a narrow panel does not hide buttons.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for item in Self::iter() {
                if item.is_hidden() { continue; }
                let size = egui::vec2(Self::BUTTON_MIN_WIDTH, Self::BUTTON_HEIGHT);
                if current_tool.eq(&item) {
                    ui.scope(|ui| {
                        ui.disable();
                        let _ = ui.add(egui::Button::new(item.name()).min_size(size));
                    });
                } else {
                    if ui.add(egui::Button::new(item.name()).min_size(size)).clicked() {
                        next_tool.set(item);
                    }
                }
            }
        });
        ui.add_space(Self::ROW_PADDING);
    }
}
