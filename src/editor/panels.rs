use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiPrimaryContextPass, EguiContexts};
use bevy_egui::egui::{Ui, UiKind, WidgetText};
use egui_dock::{DockArea, DockState, TabViewer};
use strum_macros::Display;
use crate::constants::MAP_BLUEPRINT_EXTENSION;
use crate::editor::editable::{EditEvent, FeatureId, FeatureTimeline};
use crate::editor::multicam::MulticamState;
use crate::editor::save;
use crate::get;
use crate::tool::Tools;
use crate::tool::bakes::{BakePlugin, BakeCommands, LogECS};
use crate::tool::retarget::RetargetState;
use crate::tool::show::{ShowPlugin, GizmoVisibility};
use crate::tool::room::{CalculateRoomGeometry, ClearRoomGeometry};

enum DialogResult {
    SavePath(PathBuf),
    LoadPath(PathBuf),
}

#[derive(Resource, Clone)]
pub struct CurrentFilePath {
    pub path: Option<PathBuf>,
    dialog_result: Arc<Mutex<Option<DialogResult>>>,
    deferred_room_bake: u8,
}

impl Default for CurrentFilePath {
    fn default() -> Self {
        Self {
            path: None,
            dialog_result: Arc::new(Mutex::new(None)),
            deferred_room_bake: 0,
        }
    }
}

enum TabKinds {
    Empty(String),
    Tools,
    Bakes,
    Show,
    Timeline,
    History,
}

#[derive(Default)]
struct PendingEditEvents {
    events: Vec<EditEvent>,
}

struct TabViewerAndResources<'a> {
    current_tool: &'a State<Tools>,
    next_tool: &'a mut NextState<Tools>,
    editor_features: &'a mut FeatureTimeline,
    multicam_state: &'a mut MulticamState,
    bake_commands: &'a mut BakeCommands,
    gizmo_visibility: &'a mut GizmoVisibility,
    pending_edits: &'a mut PendingEditEvents,
    retarget_request: &'a mut Option<(FeatureId, String)>,
    gizmos: Gizmos<'a, 'a>,
}

impl<'a> TabViewer for TabViewerAndResources<'a> {
    type Tab = TabKinds;

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        match tab {
            TabKinds::Empty(name) => { name.as_str().into() }
            TabKinds::Tools => { get!("tools.title").into() }
            TabKinds::Bakes => { get!("bakes.title").into() }
            TabKinds::Show => { get!("show.title").into() }
            TabKinds::Timeline => { get!("editor.timeline.title").into() }
            TabKinds::History => { get!("editor.history.title").into() }
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        match tab {
            TabKinds::Empty(name) => {
                ui.label(format!("Empty: {}", name));
            }
            TabKinds::Tools => {
                Tools::ui(ui, self.current_tool, self.next_tool);
            }
            TabKinds::Bakes => {
                *self.bake_commands = BakePlugin::ui(ui);
            }
            TabKinds::Show => {
                ShowPlugin::ui(ui, self.multicam_state, self.gizmo_visibility);
            }
            TabKinds::Timeline => {
                FeatureTimeline::ui(ui, self.editor_features, &mut self.pending_edits.events, self.retarget_request)
            }
            TabKinds::History => {
                ui.label(get!("editor.history.title").to_string());
            }
        }
    }
}

pub struct EditorPanelPlugin;
impl Plugin for EditorPanelPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<EditorPanels>()
            .init_resource::<CurrentFilePath>()
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
    menu_bar_height: f32,
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
        let default_top_tabs = vec![TabKinds::Tools,];
        let default_left_tabs = vec![TabKinds::Timeline, TabKinds::Bakes,];
        let default_right_tabs = vec![TabKinds::Show, TabKinds::History,];
        let default_bottom_tabs = vec![TabKinds::Empty("Delta".to_owned()), TabKinds::Empty("Epsilon".to_owned())];
        
        Self {
            top_tabs: DockState::new(default_top_tabs),
            toolbar_height: 20.0,
            menu_bar_height: 0.0,
            top_height: 60.0,
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
        mut multicam_state: ResMut<MulticamState>,
        windows: Query<&Window, With<PrimaryWindow>>,

        current_tool: Res<State<Tools>>,
        mut gizmos: Gizmos,
        mut next_tool: ResMut<NextState<Tools>>,
        mut editor_features: ResMut<FeatureTimeline>,
        mut gizmo_visibility: ResMut<GizmoVisibility>,
        mut room_events: MessageWriter<CalculateRoomGeometry>,
        mut clear_room_events: MessageWriter<ClearRoomGeometry>,
        mut log_ecs_events: MessageWriter<LogECS>,
        mut edit_events: MessageWriter<EditEvent>,
        mut retarget_state: ResMut<RetargetState>,
        mut current_file: ResMut<CurrentFilePath>,
    ) -> Result {
        let ctx = contexts.ctx_mut();
        if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return Ok(()); }
        let ctx = ctx.unwrap();
        
        let mut bake_commands = BakeCommands::default();
        let mut pending_edits = PendingEditEvents::default();
        let mut retarget_request: Option<(FeatureId, String)> = None;
        let mut loaded_features: Option<FeatureTimeline> = None;

        enum FileOp { New, Save, SaveAs, Load }
        let mut pending_file_op: Option<FileOp> = None;
        
        let mut viewer = TabViewerAndResources  {
            current_tool: & *current_tool,
            gizmos,
            next_tool: &mut *next_tool,
            editor_features: &mut *editor_features,
            multicam_state: &mut *multicam_state,
            bake_commands: &mut bake_commands,
            gizmo_visibility: &mut *gizmo_visibility,
            pending_edits: &mut pending_edits,
            retarget_request: &mut retarget_request,
        };

        panels.menu_bar_height = egui::TopBottomPanel::top("menu_bar")
            .resizable(false)
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("New").clicked() {
                            ui.close_kind(UiKind::Menu);
                            pending_file_op = Some(FileOp::New);
                        }
                        if ui.button("Save").clicked() {
                            ui.close_kind(UiKind::Menu);
                            pending_file_op = Some(FileOp::Save);
                        }
                        if ui.button("Save As").clicked() {
                            ui.close_kind(UiKind::Menu);
                            pending_file_op = Some(FileOp::SaveAs);
                        }
                        if ui.button("Load").clicked() {
                            ui.close_kind(UiKind::Menu);
                            pending_file_op = Some(FileOp::Load);
                        }
                        ui.separator();
                        if ui.button("Quit").clicked() {
                            ui.close_kind(UiKind::Menu);
                        }
                    });
                });
            })
            .response
            .rect
            .height();

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

        drop(viewer);

        // Poll for completed async file dialog results
        let dialog_result = current_file.dialog_result.lock().unwrap().take();
        if let Some(result) = dialog_result {
            match result {
                DialogResult::SavePath(path) => {
                    match save::save(&path, &editor_features) {
                        Ok(()) => {
                            info!("Saved to {:?}", path);
                            current_file.path = Some(path);
                        }
                        Err(e) => error!("Save failed: {}", e),
                    }
                }
                DialogResult::LoadPath(path) => {
                    match save::load(&path) {
                        Ok(features) => {
                            info!("Loaded from {:?}", path);
                            loaded_features = Some(features);
                            current_file.path = Some(path);
                        }
                        Err(e) => error!("Load failed: {}", e),
                    }
                }
            }
        }

        // Spawn async file dialogs on background thread (non-blocking)
        if let Some(op) = pending_file_op {
            match op {
                FileOp::New => {
                    let template = PathBuf::from(format!(
                        "assets/default/blueprints/new.{}", MAP_BLUEPRINT_EXTENSION
                    ));
                    match save::load(&template) {
                        Ok(features) => {
                            info!("New from template {:?}", template);
                            loaded_features = Some(features);
                            current_file.path = None;
                        }
                        Err(e) => error!("New failed: {}", e),
                    }
                }
                FileOp::Save => {
                    if let Some(ref path) = current_file.path {
                        match save::save(path, &editor_features) {
                            Ok(()) => info!("Saved to {:?}", path),
                            Err(e) => error!("Save failed: {}", e),
                        }
                    } else {
                        let slot = current_file.dialog_result.clone();
                        std::thread::spawn(move || {
                            let handle = pollster::block_on(
                                rfd::AsyncFileDialog::new()
                                    .set_file_name(format!("Untitled.{}", MAP_BLUEPRINT_EXTENSION))
                                    .add_filter("Grackle Map Blueprint", &[MAP_BLUEPRINT_EXTENSION])
                                    .save_file()
                            );
                            if let Some(h) = handle {
                                *slot.lock().unwrap() = Some(DialogResult::SavePath(h.path().to_path_buf()));
                            }
                        });
                    }
                }
                FileOp::SaveAs => {
                    let slot = current_file.dialog_result.clone();
                    let existing = current_file.path.clone();
                    std::thread::spawn(move || {
                        let mut dialog = rfd::AsyncFileDialog::new()
                            .add_filter("Grackle Map Blueprint", &[MAP_BLUEPRINT_EXTENSION]);
                        if let Some(ref existing) = existing {
                            if let Some(dir) = existing.parent() {
                                dialog = dialog.set_directory(dir);
                            }
                            if let Some(name) = existing.file_name() {
                                dialog = dialog.set_file_name(name.to_string_lossy().to_string());
                            }
                        } else {
                            dialog = dialog.set_file_name(format!("Untitled.{}", MAP_BLUEPRINT_EXTENSION));
                        }
                        let handle = pollster::block_on(dialog.save_file());
                        if let Some(h) = handle {
                            *slot.lock().unwrap() = Some(DialogResult::SavePath(h.path().to_path_buf()));
                        }
                    });
                }
                FileOp::Load => {
                    let slot = current_file.dialog_result.clone();
                    std::thread::spawn(move || {
                        let handle = pollster::block_on(
                            rfd::AsyncFileDialog::new()
                                .add_filter("Grackle Map Blueprint", &[MAP_BLUEPRINT_EXTENSION])
                                .pick_file()
                        );
                        if let Some(h) = handle {
                            *slot.lock().unwrap() = Some(DialogResult::LoadPath(h.path().to_path_buf()));
                        }
                    });
                }
            }
        }

        // Handle load
        if let Some(new_features) = loaded_features {
            let old_entities: Vec<Entity> = editor_features.active_features()
                .filter_map(|(_, a)| a.object().entity())
                .collect();
            *editor_features = new_features;
            for entity in old_entities {
                editor_features.queue_despawn(entity);
            }
            editor_features.select(None);
            current_file.deferred_room_bake = 2;
        }

        // Fire deferred room bake after entities have been spawned and flushed
        if current_file.deferred_room_bake > 0 {
            current_file.deferred_room_bake -= 1;
            if current_file.deferred_room_bake == 0 {
                room_events.write(CalculateRoomGeometry);
            }
        }

        // Handle bake commands
        if bake_commands.calculate_room_geometry {
            room_events.write(CalculateRoomGeometry);
        }
        if bake_commands.clear_room_geometry {
            clear_room_events.write(ClearRoomGeometry);
        }
        if bake_commands.log_ecs {
            log_ecs_events.write(LogECS);
        }

        // Handle retarget request
        if let Some((feature_id, label)) = retarget_request {
            retarget_state.target_feature = Some(feature_id);
            retarget_state.target_point_ref_key = label;
            retarget_state.hovered_point = None;
            next_tool.set(Tools::Retarget);
        }

        // Flush edit events
        for event in pending_edits.events.drain(..) {
            edit_events.write(event);
        }

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
        let top_taken = (panels.toolbar_height + panels.menu_bar_height + panels.top_height) / window.height();
        // info!("[{} {}] -> [{}, {}]", left_taken, top_taken, 1.0 - right_taken, 1.0 - bottom_taken);

        multicam_state.start = Vec2::new(left_taken, top_taken);
        multicam_state.end = Vec2::new(1.0 - right_taken, 1.0 - bottom_taken);

        Ok(())
    }
}


