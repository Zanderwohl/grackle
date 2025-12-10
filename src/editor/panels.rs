use bevy_egui::egui::Context;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContext, EguiPrimaryContextPass, EguiContexts};
use bevy_egui::egui::{Ui, WidgetText};
use egui_dock::{DockArea, DockState, TabViewer};
use strum_macros::Display;
use crate::editor::editable::EditorActions;
use crate::editor::multicam::MulticamState;
use crate::tool::Tools;

enum TabKinds {
    Empty(String),
    Tools,
    Timeline,
}

struct TabViewerAndResources<'a> {
    current_tool: &'a State<Tools>,
    next_tool: &'a mut NextState<Tools>,
    editor_actions: &'a mut EditorActions,
    gizmos: Gizmos<'a, 'a>,
}

impl<'a> TabViewer for TabViewerAndResources<'a> {
    type Tab = TabKinds;

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        match tab {
            TabKinds::Empty(name) => { name.as_str().into() }
            TabKinds::Tools => { "Tools".into() }
            TabKinds::Timeline => { "Timeline".into() }
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        match tab {
            TabKinds::Empty(name) => {
                ui.label(format!("Empty: {}", name));
            }
            TabKinds::Tools => {
                ui.label("Tools.");
            }
            TabKinds::Timeline => {
                ui.label("Timeline.");
                EditorActions::ui(ui, self.editor_actions)
            }
        }
    }
}

pub struct EditorPanelPlugin;
impl Plugin for EditorPanelPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<EditorPanels>()
            .add_systems(Startup, EditorPanels::set_multicam_size)
            .add_systems(EguiPrimaryContextPass, EditorPanels::ui)
        ;
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Copy, Display)]
pub enum EditorPanelLocation {
    Left,
    Right,
    Bottom,
    Top,
}

#[derive(Resource)]
pub struct EditorPanels {
    top_tabs: DockState<TabKinds>,
    toolbar_height: f32,
    top_height: f32,
    bottom_tabs: DockState<TabKinds>,
    bottom_height: f32,
    left_tabs: DockState<TabKinds>,
    left_width: f32,
    right_tabs: DockState<TabKinds>,
    right_width: f32,
}

pub enum PanelError {
    PanelWithKeyAlreadyExists(String),
    SectionDoesNotExist(String),
    PanelDoesNotExist(String),
}

impl Default for EditorPanels {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorPanels {
    pub fn new() -> Self {
        let default_top_tabs = vec![TabKinds::Tools];
        let default_left_tabs = vec![TabKinds::Timeline, TabKinds::Empty("Alpha".to_owned())];
        let default_right_tabs = vec![TabKinds::Empty("Beta".to_owned()), TabKinds::Empty("Gamma".to_owned())];
        let default_bottom_tabs = vec![TabKinds::Empty("Delta".to_owned()), TabKinds::Empty("Epsilon".to_owned())];
        
        Self {
            top_tabs: DockState::new(default_top_tabs),
            toolbar_height: 20.0,
            top_height: 40.0,
            bottom_tabs: DockState::new(default_bottom_tabs),
            bottom_height: 30.0,
            left_tabs: DockState::new(default_left_tabs),
            left_width: 40.0,
            right_tabs: DockState::new(default_right_tabs),
            right_width: 40.0,
        }
    }

    fn ui(
        mut panels: ResMut<Self>,
        mut contexts: EguiContexts,
        multicam_state: ResMut<MulticamState>,
        windows: Query<&Window, With<PrimaryWindow>>,
        
        current_tool: Res<State<Tools>>,
        mut gizmos: Gizmos,
        mut next_tool: ResMut<NextState<Tools>>,
        mut editor_actions: ResMut<EditorActions>,
    ) -> Result {
        let ctx = contexts.ctx_mut();
        if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return Ok(()); }
        let ctx = ctx.unwrap();
        
        let mut viewer = TabViewerAndResources  {
            current_tool: & *current_tool,
            gizmos,
            next_tool: &mut *next_tool,
            editor_actions: &mut *editor_actions,
        };

        panels.top_height = egui::TopBottomPanel::top("top_panel")
            .resizable(true)
            .show(ctx, |ui| {
                DockArea::new(&mut panels.top_tabs)
                    .id(egui::Id::new("egui_dock::DockArea::top"))
                    .show_close_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .draggable_tabs(false)
                    .show_inside(ui, &mut viewer);
                //ui.allocate_rect(ui.available_rect_before_wrap(), egui::Sense::hover());
            })
            .response
            .rect
            .height();
        panels.left_width = egui::SidePanel::left("left_panel")
            .resizable(true)
            .show(ctx, |ui| {
                DockArea::new(&mut panels.left_tabs)
                    .id(egui::Id::new("egui_dock::DockArea::left"))
                    .show_close_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .draggable_tabs(false)
                    .show_inside(ui, &mut viewer);
                ui.allocate_rect(ui.available_rect_before_wrap(), egui::Sense::hover());
            })
            .response
            .rect
            .width();
        panels.right_width = egui::SidePanel::right("right_panel")
            .resizable(true)
            .show(ctx, |ui| {
                DockArea::new(&mut panels.right_tabs)
                    .id(egui::Id::new("egui_dock::DockArea::right"))
                    .show_close_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .draggable_tabs(false)
                    .show_inside(ui, &mut viewer);
                ui.allocate_rect(ui.available_rect_before_wrap(), egui::Sense::hover());
            })
            .response
            .rect
            .width();
        panels.bottom_height = egui::TopBottomPanel::bottom("bottom_panel")
            .resizable(true)
            .show(ctx, |ui| {
                DockArea::new(&mut panels.bottom_tabs)
                    .id(egui::Id::new("egui_dock::DockArea::bottom"))
                    .show_close_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .draggable_tabs(false)
                    .show_inside(ui, &mut viewer);
                //ui.allocate_rect(ui.available_rect_before_wrap(), egui::Sense::hover());
            })
            .response
            .rect
            .height();

        Self::set_multicam_size(panels, multicam_state, windows)
    }

    fn ui_for_panel(ui: &mut Ui) {
        ui.label("Panel is empty.");
    }

    fn set_multicam_size(panels: ResMut<Self>, mut multicam_state: ResMut<MulticamState>, windows: Query<&Window, With<PrimaryWindow>>,) -> Result {
        let window = windows.single()?;

        let left_taken = panels.left_width / window.width();
        let right_taken = panels.right_width / window.width();
        let bottom_taken = panels.bottom_height / window.height();
        let top_taken = (panels.toolbar_height + panels.top_height) / window.height();
        // info!("[{} {}] -> [{}, {}]", left_taken, top_taken, 1.0 - right_taken, 1.0 - bottom_taken);

        multicam_state.start = Vec2::new(left_taken, top_taken);
        multicam_state.end = Vec2::new(1.0 - right_taken, 1.0 - bottom_taken);

        Ok(())
    }
}


