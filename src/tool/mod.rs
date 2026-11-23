use bevy::app::App;
use bevy::prelude::*;
use bevy_egui::egui;
use strum_macros::{Display, EnumIter};
use crate::common::icons;
use crate::common::toolbar::{self, Joins};
use crate::get;
use crate::tool::animation_grid::AnimationGridPlugin;
use crate::tool::bakes::BakePlugin;
use crate::tool::footprint_drag::FootprintDragPlugin;
use crate::tool::movement::MovementPlugin;
use crate::tool::spawn_point::SpawnPointPlugin;
use crate::tool::point_drag::PointDragPlugin;
use crate::tool::prop::PropPlugin;
use crate::tool::rotate_drag::RotateDragPlugin;
use crate::tool::room::RoomPlugin;
use crate::tool::selection::SelectionPlugin;
use crate::tool::show::ShowPlugin;

pub mod selection;
pub mod bake_progress;
pub mod point;
pub mod spawn_point;
pub mod stairs;
pub mod point_light;
pub mod point_drag;
pub mod animation_display;
pub mod animation_grid;
pub mod point_placement;
pub mod point_picker;
pub mod mirror;
pub mod prop;
pub mod weapon_pickup;
pub mod spectator_camera;
pub mod decal;
pub mod rotate_drag;
pub mod map_wizard;
pub mod map_products;
pub mod room;
pub mod footprint;
pub mod footprint_drag;
pub mod movement;
pub mod bakes;
pub mod bake_job;
pub mod sun_bake;
pub mod postcard_bake;
pub mod nav_bake;
pub mod bake_cache;
pub mod nav_cache;
pub mod round_prep;
pub mod nav_overlay;
pub mod show;
pub mod tool_helpers;

pub struct ToolPlugin;

impl Plugin for ToolPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app
            .init_resource::<ToolData>()
            .init_resource::<map_products::MapProducts>()
            .init_state::<Tools>()
            .add_plugins(ShowPlugin)
            .add_plugins(BakePlugin)
            .add_plugins(sun_bake::SunBakePlugin)
            .add_plugins(nav_bake::NavBakePlugin)
            .add_plugins(nav_overlay::NavOverlayPlugin)
            .add_plugins(MovementPlugin)
            .add_plugins(SelectionPlugin)
            .add_plugins(SpawnPointPlugin)
            .add_plugins(map_wizard::MapWizardPlugin)
            .add_plugins(PointDragPlugin)
            .add_plugins(RotateDragPlugin)
            .add_plugins(PropPlugin)
            .add_plugins(weapon_pickup::WeaponPickupPlugin)
            .add_plugins(AnimationGridPlugin)
            .add_plugins(RoomPlugin)
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
    SpectatorCamera,
    Decal,
    Prop,
    WeaponPickup,
    AnimationDisplay,
    AnimationGrid,
    Room,
    Block,
    Trigger,
    Footprint,
    FootprintBlock,
    Stairs,
    Mirror,
    Retarget,
    /// Re-picks a Mirror's sources, from its panel.
    MirrorSources,
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
            Self::SpectatorCamera => get!("tools.spectator_camera"),
            Self::Decal => get!("tools.decal"),
            Self::Prop => get!("tools.prop"),
            Self::WeaponPickup => get!("tools.weapon_pickup"),
            Self::AnimationDisplay => get!("tools.animation_display"),
            Self::AnimationGrid => get!("tools.animation_grid"),
            Self::Room => get!("tools.room"),
            Self::Block => get!("tools.block"),
            Self::Trigger => get!("tools.trigger"),
            Self::Footprint => get!("tools.footprint"),
            Self::FootprintBlock => get!("tools.footprint_block"),
            Self::Stairs => get!("tools.stairs"),
            Self::Mirror => get!("tools.mirror"),
            Self::Retarget => get!("tools.retarget"),
            Self::MirrorSources => get!("tools.mirror_sources"),
            Self::Spectate => "Spectate".into(),
        }
    }

    /// Named for the picture, not the tool: see `documentation/icons.md`.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Select => "cursor",
            Self::Point => "axes",
            Self::PointLight => "light_bulb",
            Self::SpawnPoint => "t_pose",
            Self::SpectatorCamera => "film_camera",
            Self::Decal => "ground_square",
            Self::Prop => "mug",
            Self::WeaponPickup => "rifle",
            Self::AnimationDisplay => "runner",
            Self::AnimationGrid => "two_runners",
            Self::Room => "cube_hollow",
            Self::Block => "cube_solid",
            Self::Trigger => "cube_exclamation",
            Self::Footprint => "double_pennant",
            Self::FootprintBlock => "double_pennant_solid",
            Self::Stairs => "stairs",
            Self::Mirror => "square_mirrored",
            Self::Retarget | Self::MirrorSources => icons::FALLBACK,
            Self::Spectate => icons::FALLBACK,
        }
    }

    #[cfg(test)]
    fn is_hidden(&self) -> bool {
        matches!(self, Self::Retarget | Self::MirrorSources | Self::Spectate)
    }

    pub fn ui(
        ui: &mut egui::Ui,
        current_tool: &State<Self>,
        next_tool: &mut NextState<Self>,
    ) {
        let current = current_tool.get();
        let mut chosen = None;
        toolbar::row(ui, |ui| {
            for group in TOOLBAR {
                match group {
                    ToolGroup::Run(members) => {
                        toolbar::cluster(ui, |ui| {
                            for (index, member) in members.iter().enumerate() {
                                if let Some(tool) = member.ui(ui, current, Joins::in_run(index, members.len())) {
                                    chosen = Some(tool);
                                }
                            }
                        });
                    }
                    group => {
                        if let Some(tool) = group.ui(ui, current, Joins::NONE) {
                            chosen = Some(tool);
                        }
                    }
                }
            }
        });
        if let Some(tool) = chosen.filter(|tool| tool != current) {
            next_tool.set(tool);
        }
    }
}

enum ToolGroup {
    One(Tools),
    /// The id keeps which one was last used.
    Split(&'static str, &'static [Tools]),
    /// Ones and splits sharing their edges, in a row that does not wrap.
    Run(&'static [ToolGroup]),
}

impl ToolGroup {
    /// Draws nothing for a run: its caller draws it, since only the caller knows each member's joins.
    fn ui(&self, ui: &mut egui::Ui, current: &Tools, joins: Joins) -> Option<Tools> {
        match self {
            Self::One(tool) => toolbar::button(ui, tool.icon(), tool.name(), current == tool, joins).clicked().then(|| tool.clone()),
            Self::Split(id, tools) => {
                let active = tools.iter().position(|tool| tool == current);
                let label = |index: usize| (tools[index].icon(), tools[index].name());
                toolbar::split_button(ui, id, tools.len(), label, active, joins).map(|index| tools[index].clone())
            }
            Self::Run(_) => None,
        }
    }

    #[cfg(test)]
    fn tools(&self) -> Vec<Tools> {
        match self {
            Self::One(tool) => vec![tool.clone()],
            Self::Split(_, tools) => tools.to_vec(),
            Self::Run(members) => members.iter().flat_map(Self::tools).collect(),
        }
    }
}

/// Every tool but the hidden ones, once each.
const TOOLBAR: &[ToolGroup] = &[
    ToolGroup::One(Tools::Select),
    ToolGroup::Split("points", &[Tools::Point, Tools::PointLight, Tools::SpawnPoint, Tools::SpectatorCamera, Tools::Decal]),
    ToolGroup::Split("props", &[Tools::Prop, Tools::WeaponPickup]),
    ToolGroup::Run(&[
        ToolGroup::Split("boxes", &[Tools::Room, Tools::Block, Tools::Trigger]),
        ToolGroup::Split("footprints", &[Tools::Footprint, Tools::FootprintBlock]),
        ToolGroup::One(Tools::Stairs),
    ]),
    ToolGroup::One(Tools::Mirror),
    ToolGroup::Split("animation", &[Tools::AnimationDisplay, Tools::AnimationGrid]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::net::NetRole;
    use strum::IntoEnumIterator;

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

    #[test]
    fn every_tool_is_on_the_toolbar_once() {
        let shown: Vec<Tools> = TOOLBAR.iter().flat_map(ToolGroup::tools).collect();
        for group in TOOLBAR {
            if let ToolGroup::Run(members) = group {
                assert!(!members.iter().any(|member| matches!(member, ToolGroup::Run(_))), "a run inside a run is not drawn");
            }
        }
        for tool in Tools::iter() {
            let count = shown.iter().filter(|shown| **shown == tool).count();
            assert_eq!(count, usize::from(!tool.is_hidden()), "{tool:?}");
        }
    }
}
