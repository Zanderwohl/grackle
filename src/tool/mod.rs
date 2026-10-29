use bevy::app::App;
use bevy::prelude::*;
use bevy_egui::egui;
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter};
use crate::get;
use crate::tool::animation_display::AnimationDisplayPlugin;
use crate::tool::animation_grid::AnimationGridPlugin;
use crate::tool::bakes::BakePlugin;
use crate::tool::footprint::FootprintPlugin;
use crate::tool::footprint_drag::FootprintDragPlugin;
use crate::tool::mirror::MirrorPlugin;
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
pub mod mirror;
pub mod prop;
pub mod weapon_pickup;
pub mod rotate_drag;
pub mod retarget;
pub mod room;
pub mod footprint;
pub mod footprint_drag;
pub mod movement;
pub mod bakes;
pub mod sun_bake;
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
            .add_plugins(sun_bake::SunBakePlugin)
            .add_plugins(MovementPlugin)
            .add_plugins(SelectionPlugin)
            .add_plugins(PointPlugin)
            .add_plugins(SpawnPointPlugin)
            .add_plugins(PointLightPlugin)
            .add_plugins(PointDragPlugin)
            .add_plugins(RotateDragPlugin)
            .add_plugins(PropPlugin)
            .add_plugins(weapon_pickup::WeaponPickupPlugin)
            .add_plugins(MirrorPlugin)
            .add_plugins(AnimationDisplayPlugin)
            .add_plugins(AnimationGridPlugin)
            .add_plugins(RetargetPlugin)
            .add_plugins(RoomPlugin)
            .add_plugins(FootprintPlugin)
            .add_plugins(FootprintDragPlugin)
            .add_systems(Update, spectate_while_a_client);
    }
}

/// Every editing tool runs under a `Tools` state, so holding a client in
/// [`Tools::Spectate`] turns them all off at once.
fn spectate_while_a_client(
    role: Option<Res<crate::common::net::NetRole>>,
    tool: Res<State<Tools>>,
    mut next: ResMut<NextState<Tools>>,
) {
    let client = role.is_some_and(|role| !role.is_authority());
    match (client, *tool.get() == Tools::Spectate) {
        (true, false) => next.set(Tools::Spectate),
        (false, true) => next.set(Tools::Select),
        _ => {}
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
    WeaponPickup,
    AnimationDisplay,
    AnimationGrid,
    Room,
    Trigger,
    Block,
    Footprint,
    Mirror,
    Retarget,
    /// A client's: no tool runs under it.
    Spectate,
}

impl Tools {
    pub fn name(&self) -> String {
        match self {
            Self::Select => get!("tools.select"),
            Self::Point => get!("tools.point"),
            Self::PointLight => get!("tools.point_light"),
            Self::SpawnPoint => get!("tools.spawn_point"),
            Self::Prop => get!("tools.prop"),
            Self::WeaponPickup => get!("tools.weapon_pickup"),
            Self::AnimationDisplay => get!("tools.animation_display"),
            Self::AnimationGrid => get!("tools.animation_grid"),
            Self::Room => get!("tools.room"),
            Self::Trigger => get!("tools.trigger"),
            Self::Block => get!("tools.block"),
            Self::Footprint => get!("tools.footprint"),
            Self::Mirror => get!("tools.mirror"),
            Self::Retarget => "Retarget".into(),
            Self::Spectate => "Spectate".into(),
        }
    }

    fn is_hidden(&self) -> bool {
        matches!(self, Self::Retarget | Self::Spectate)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::net::NetRole;

    fn tool_with(role: NetRole, starting: Tools) -> Tools {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(starting)
            .insert_resource(role)
            .add_systems(Update, spectate_while_a_client);
        app.update();
        app.update();
        app.world().resource::<State<Tools>>().get().clone()
    }

    #[test]
    fn a_client_only_spectates_and_a_host_gets_its_tools_back() {
        let client = NetRole::Client { host: "example".into(), port: 1 };
        assert_eq!(tool_with(client, Tools::Room), Tools::Spectate);
        assert_eq!(tool_with(NetRole::Listen { port: 1 }, Tools::Room), Tools::Room);
        assert_eq!(tool_with(NetRole::Solo, Tools::Spectate), Tools::Select);
    }
}
