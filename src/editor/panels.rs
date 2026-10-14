use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::edit_menu::{edit_menu, EditAction, EditAvailability};
use crate::common::file_menu::{file_menu, FileAction};
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiPrimaryContextPass, EguiContexts};
use bevy_egui::egui::{Id, LayerId, Ui, UiBuilder, WidgetText};
use egui_dock::{DockArea, DockState, TabViewer};
use strum::IntoEnumIterator;
use strum_macros::Display;
use crate::common::mode::GameMode;
use crate::common::shortcuts::chords;
use crate::common::net::{NetRequest, NetRole};
use crate::common::net_transport::NetStatus;
use crate::constants::MAP_BLUEPRINT_EXTENSION;
use crate::editor::editable::{EditEvent, FeatureId, FeatureTimeline};
use crate::editor::map_metadata::MapMetadata;
use crate::editor::multicam::MulticamState;
use crate::editor::net_menu::{network_menu, ConnectDialog};
use crate::editor::save::{self, LoadedBlueprint};
use crate::get;
use crate::tool::Tools;
use crate::tool::bakes::{BakePlugin, BakeCommands, LogECS};
use crate::tool::retarget::RetargetState;
use crate::tool::show::{ShowPlugin, GizmoVisibility};
use crate::tool::room::{CalculateRoomGeometry, ClearRoomGeometry};

enum DialogResult {
    SavePath(PathBuf),
    OpenPath(PathBuf),
}

/// Shared by Save As and by a Save with no path. The thread and `pollster` are the wasm problem
/// `CLAUDE.md` lists.
fn ask_where_to_save(current_file: &CurrentFilePath) {
    let slot = current_file.dialog_result.clone();
    let existing = current_file.path.clone();
    std::thread::spawn(move || {
        let mut dialog = rfd::AsyncFileDialog::new()
            .add_filter("Grackle Map Blueprint", &[MAP_BLUEPRINT_EXTENSION]);
        match existing.as_ref() {
            Some(existing) => {
                if let Some(directory) = existing.parent() {
                    dialog = dialog.set_directory(directory);
                }
                if let Some(name) = existing.file_name() {
                    dialog = dialog.set_file_name(name.to_string_lossy().to_string());
                }
            }
            None => {
                dialog = dialog.set_file_name(format!("Untitled.{MAP_BLUEPRINT_EXTENSION}"));
            }
        }
        if let Some(handle) = pollster::block_on(dialog.save_file()) {
            *slot.lock().unwrap() = Some(DialogResult::SavePath(handle.path().to_path_buf()));
        }
    });
}

/// The map editor's file and undo chords. Its own system because [`EditorPanels::ui`] is at Bevy's
/// parameter limit; the request goes through `CurrentFilePath` so the menu's code answers it.
fn file_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    egui: Res<bevy_egui::input::EguiWantsInput>,
    mut current_file: ResMut<CurrentFilePath>,
) {
    if let Some(action) = FileAction::from_chords(chords(&keys, egui.wants_keyboard_input())) {
        // Only on `Some`: a frame with nothing held must not clear a request the menu made this frame.
        current_file.requested_op = Some(action);
    }
}

#[derive(Resource, Clone)]
pub struct CurrentFilePath {
    pub path: Option<PathBuf>,
    dialog_result: Arc<Mutex<Option<DialogResult>>>,
    deferred_room_bake: u8,
    /// Set by the keyboard, taken by the panel.
    requested_op: Option<FileAction>,
}

impl Default for CurrentFilePath {
    fn default() -> Self {
        Self {
            path: None,
            dialog_result: Arc::new(Mutex::new(None)),
            deferred_room_bake: 0,
            requested_op: None,
        }
    }
}

/// A top panel sizes to its contents and a `DockArea` asks for almost nothing, so the default is
/// stated as the tab bar plus two rows of buttons.
const TAB_BAR_HEIGHT: f32 = 26.0;
const TOOLBAR_ROW_HEIGHT: f32 = Tools::BUTTON_HEIGHT + Tools::ROW_PADDING * 2.0;
const TOOLBAR_DEFAULT_HEIGHT: f32 = TAB_BAR_HEIGHT + TOOLBAR_ROW_HEIGHT * 2.0;
const TOOLBAR_MIN_HEIGHT: f32 = TAB_BAR_HEIGHT + TOOLBAR_ROW_HEIGHT;

enum TabKinds {
    Empty(String),
    Tools,
    Bakes,
    Show,
    Metadata,
    /// The feature list with its rollback bar, and the selected feature's editor beneath.
    History,
}

/// The panels' writes, bundled because `ui` hit Bevy's sixteen-parameter limit.
#[derive(SystemParam)]
struct PanelMessages<'w> {
    room: MessageWriter<'w, CalculateRoomGeometry>,
    clear_room: MessageWriter<'w, ClearRoomGeometry>,
    log_ecs: MessageWriter<'w, LogECS>,
    edits: MessageWriter<'w, EditEvent>,
    net: MessageWriter<'w, NetRequest>,
    exit: MessageWriter<'w, AppExit>,
    mode: ResMut<'w, NextState<AppMode>>,
}

#[derive(Default)]
struct PendingEditEvents {
    events: Vec<EditEvent>,
}

struct TabViewerAndResources<'a> {
    current_tool: &'a State<Tools>,
    next_tool: &'a mut NextState<Tools>,
    editor_features: &'a mut FeatureTimeline,
    map_metadata: &'a mut MapMetadata,
    multicam_state: &'a mut MulticamState,
    bake_commands: &'a mut BakeCommands,
    gizmo_visibility: &'a mut GizmoVisibility,
    pending_edits: &'a mut PendingEditEvents,
    retarget_request: &'a mut Option<(FeatureId, String)>,
}

impl<'a> TabViewer for TabViewerAndResources<'a> {
    type Tab = TabKinds;

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(match tab {
            TabKinds::Empty(name) => format!("empty:{}", name),
            TabKinds::Tools => "tools".to_owned(),
            TabKinds::Bakes => "bakes".to_owned(),
            TabKinds::Show => "show".to_owned(),
            TabKinds::Metadata => "metadata".to_owned(),
            TabKinds::History => "history".to_owned(),
        })
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        match tab {
            TabKinds::Empty(name) => { name.as_str().into() }
            TabKinds::Tools => { get!("tools.title").into() }
            TabKinds::Bakes => { get!("bakes.title").into() }
            TabKinds::Show => { get!("show.title").into() }
            TabKinds::Metadata => { get!("editor.metadata.title").into() }
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
            TabKinds::Metadata => {
                map_metadata_panel_ui(ui, self.map_metadata);
            }
            TabKinds::History => {
                FeatureTimeline::ui(ui, self.editor_features, &mut self.pending_edits.events, self.retarget_request)
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
            .add_systems(EguiPrimaryContextPass, EditorPanels::ui.run_if(in_state(AppMode::Editor)))
            // Before the panel, so a chord is answered this frame.
            .add_systems(Update, file_shortcuts.run_if(in_state(AppMode::Editor)))
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
        let default_left_tabs = vec![TabKinds::History, TabKinds::Bakes,];
        let default_right_tabs = vec![TabKinds::Show, TabKinds::Metadata];
        let default_bottom_tabs = vec![TabKinds::Empty("Delta".to_owned()), TabKinds::Empty("Epsilon".to_owned())];
        
        Self {
            top_tabs: DockState::new(default_top_tabs),
            toolbar_height: 20.0,
            menu_bar_height: 0.0,
            top_height: TOOLBAR_DEFAULT_HEIGHT,
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
        mut next_tool: ResMut<NextState<Tools>>,
        mut editor_features: ResMut<FeatureTimeline>,
        mut gizmo_visibility: ResMut<GizmoVisibility>,
        mut messages: PanelMessages,
        mut retarget_state: ResMut<RetargetState>,
        mut current_file: ResMut<CurrentFilePath>,
        mut map_metadata: ResMut<MapMetadata>,
        net_role: Res<NetRole>,
        net_status: Res<NetStatus>,
        mut connect_dialog: ResMut<ConnectDialog>,
    ) {
        let ctx = contexts.ctx_mut();
        if ctx.is_err() {
            warn!("{}", ctx.unwrap_err());
            return;
        }
        let ctx = ctx.unwrap();

        // egui 0.36 hangs top-level panels off a root `Ui` covering the viewport instead of off the
        // `Context` directly.
        let mut viewport_ui = Ui::new(
            ctx.clone(),
            Id::new("grackle_editor_viewport"),
            UiBuilder::new()
                .layer_id(LayerId::background())
                .max_rect(ctx.viewport_rect()),
        );

        map_metadata.sync_authors_ui_buffer_from_authors();
        
        let mut bake_commands = BakeCommands::default();
        let mut pending_edits = PendingEditEvents::default();
        let mut retarget_request: Option<(FeatureId, String)> = None;
        let mut loaded_blueprint: Option<LoadedBlueprint> = None;

        let mut pending_file_op: Option<FileAction> = current_file.requested_op.take();
        let mut wanted_mode: Option<AppMode> = None;
        let mut pending_edit: Option<EditAction> = None;
        // Read before the viewer borrows the timeline for the rest of the frame.
        let edit_available = EditAvailability {
            undo: editor_features.can_undo(),
            redo: editor_features.can_redo(),
            revert: current_file.path.is_some() && editor_features.is_dirty(),
        };
        let dirty = editor_features.is_dirty();
        
        let mut viewer = TabViewerAndResources  {
            current_tool: & *current_tool,
            next_tool: &mut *next_tool,
            editor_features: &mut *editor_features,
            map_metadata: &mut *map_metadata,
            multicam_state: &mut *multicam_state,
            bake_commands: &mut bake_commands,
            gizmo_visibility: &mut *gizmo_visibility,
            pending_edits: &mut pending_edits,
            retarget_request: &mut retarget_request,
        };

        panels.menu_bar_height = egui::Panel::top("menu_bar")
            .resizable(false)
            .show(&mut viewport_ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    file_menu(ui, &mut pending_file_op);
                    edit_menu(ui, edit_available, &mut pending_edit);
                    // `Editor` spelled out: this system is gated on it.
                    mode_menu(ui, AppMode::Editor, Some(&net_role), &mut wanted_mode);
                    network_menu(
                        ui,
                        &net_role,
                        &net_status,
                        &mut connect_dialog,
                        &mut messages.net,
                    );
                    if dirty {
                        ui.separator();
                        ui.label("*");
                    }
                });
            })
            .response
            .rect
            .height();

        let dock_frame = crate::common::dock::dock_panel_frame(viewport_ui.style());
        panels.top_height = egui::Panel::top("top_panel")
            .resizable(true)
            .frame(dock_frame)
            .default_size(TOOLBAR_DEFAULT_HEIGHT)
            .min_size(TOOLBAR_MIN_HEIGHT)
            .show(&mut viewport_ui, |ui| {
                DockArea::new(&mut panels.top_tabs)
                    .id(egui::Id::new("egui_dock::DockArea::top"))
                    .show_close_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .draggable_tabs(false)
                    .show_inside(ui, &mut viewer);
            })
            .response
            .rect
            .height();
        panels.left_width = egui::Panel::left("left_panel")
            .resizable(true)
            .frame(dock_frame)
            .show(&mut viewport_ui, |ui| {
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
        panels.right_width = egui::Panel::right("right_panel")
            .resizable(true)
            .frame(dock_frame)
            .show(&mut viewport_ui, |ui| {
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
        panels.bottom_height = egui::Panel::bottom("bottom_panel")
            .resizable(true)
            .frame(dock_frame)
            .show(&mut viewport_ui, |ui| {
                DockArea::new(&mut panels.bottom_tabs)
                    .id(egui::Id::new("egui_dock::DockArea::bottom"))
                    .show_close_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .draggable_tabs(false)
                    .show_inside(ui, &mut viewer);
            })
            .response
            .rect
            .height();

        drop(viewer);

        // `set`, not `set_if_neq`: `mode_menu` never offers the mode already in force.
        if let Some(mode) = wanted_mode {
            messages.mode.set(mode);
        }

        let dialog_result = current_file.dialog_result.lock().unwrap().take();
        if let Some(result) = dialog_result {
            match result {
                DialogResult::SavePath(path) => {
                    match save::save(&path, &editor_features, &map_metadata) {
                        Ok(()) => {
                            info!("Saved to {:?}", path);
                            current_file.path = Some(path);
                            editor_features.mark_saved();
                        }
                        Err(e) => error!("Save failed: {}", e),
                    }
                }
                DialogResult::OpenPath(path) => {
                    match save::load(&path) {
                        Ok(loaded) => {
                            info!("Loaded from {:?}", path);
                            loaded_blueprint = Some(loaded);
                            current_file.path = Some(path);
                        }
                        Err(e) => error!("Load failed: {}", e),
                    }
                }
            }
        }

        if let Some(op) = pending_file_op {
            match op {
                FileAction::New => {
                    let template = PathBuf::from(format!(
                        "assets/default/blueprints/new.{}", MAP_BLUEPRINT_EXTENSION
                    ));
                    match save::load(&template) {
                        Ok(loaded) => {
                            info!("New from template {:?}", template);
                            loaded_blueprint = Some(LoadedBlueprint {
                                timeline: loaded.timeline,
                                metadata: MapMetadata::default(),
                            });
                            current_file.path = None;
                        }
                        Err(e) => error!("New failed: {}", e),
                    }
                }
                FileAction::Save => match current_file.path.clone() {
                    Some(path) => match save::save(&path, &editor_features, &map_metadata) {
                        Ok(()) => {
                            info!("Saved to {:?}", path);
                            editor_features.mark_saved();
                        }
                        Err(e) => error!("Save failed: {}", e),
                    },
                    None => ask_where_to_save(&current_file),
                },
                FileAction::SaveAs => ask_where_to_save(&current_file),
                // No unsaved-changes prompt yet.
                FileAction::Quit => {
                    info!("Quitting, as asked");
                    messages.exit.write(AppExit::Success);
                }
                FileAction::Open => {
                    let slot = current_file.dialog_result.clone();
                    std::thread::spawn(move || {
                        let handle = pollster::block_on(
                            rfd::AsyncFileDialog::new()
                                .add_filter("Grackle Map Blueprint", &[MAP_BLUEPRINT_EXTENSION])
                                .pick_file()
                        );
                        if let Some(h) = handle {
                            *slot.lock().unwrap() = Some(DialogResult::OpenPath(h.path().to_path_buf()));
                        }
                    });
                }
            }
        }

        // Revert is one undo step: only the features that differ from the file are rebuilt.
        match pending_edit {
            Some(EditAction::Undo) => editor_features.undo(),
            Some(EditAction::Redo) => editor_features.redo(),
            Some(EditAction::Revert) => {
                if let Some(path) = current_file.path.clone() {
                    match save::load(&path) {
                        Ok(loaded) => {
                            info!("Reverted to {:?}", path);
                            editor_features.replace_as_edit(loaded.timeline);
                            *map_metadata = loaded.metadata;
                            current_file.deferred_room_bake = 2;
                        }
                        Err(e) => error!("Revert failed: {}", e),
                    }
                }
            }
            None => {}
        }

        if let Some(loaded) = loaded_blueprint {
            editor_features.adopt(loaded.timeline);
            *map_metadata = loaded.metadata;
            current_file.deferred_room_bake = 2;
        }

        // Deferred until the adopted features have entities.
        if current_file.deferred_room_bake > 0 {
            current_file.deferred_room_bake -= 1;
            if current_file.deferred_room_bake == 0 {
                messages.room.write(CalculateRoomGeometry);
            }
        }

        if bake_commands.calculate_room_geometry {
            messages.room.write(CalculateRoomGeometry);
        }
        if bake_commands.clear_room_geometry {
            messages.clear_room.write(ClearRoomGeometry);
        }
        if bake_commands.log_ecs {
            messages.log_ecs.write(LogECS);
        }

        if let Some((feature_id, label)) = retarget_request {
            retarget_state.target_feature = Some(feature_id);
            retarget_state.target_point_ref_key = label;
            retarget_state.hovered_point = None;
            // `set_if_neq`, not `set`: a same-state `set` still runs the transition, and
            // `OnExit(Tools::Retarget)` would clear the `RetargetState` just filled in.
            (&mut *next_tool).set_if_neq(Tools::Retarget);
        }

        for event in pending_edits.events.drain(..) {
            messages.edits.write(event);
        }

        Self::set_multicam_size(panels, multicam_state, windows);
    }

    fn set_multicam_size(
        panels: ResMut<Self>,
        mut multicam_state: ResMut<MulticamState>,
        windows: Query<&Window, With<PrimaryWindow>>,
    ) {
        let Ok(window) = windows.single() else {
            return;
        };

        multicam_state.reserve(
            crate::editor::multicam::Insets {
                left: panels.left_width,
                right: panels.right_width,
                top: panels.toolbar_height + panels.menu_bar_height + panels.top_height,
                bottom: panels.bottom_height,
            },
            window,
        );
    }
}

fn map_metadata_panel_ui(ui: &mut egui::Ui, meta: &mut MapMetadata) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("editor_map_metadata_grid")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(get!("editor.metadata.game_mode"));
                ui.vertical(|ui| {
                    egui::ComboBox::from_id_salt("editor_map_game_mode")
                        .selected_text(format!("{}", meta.game_mode))
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for mode in GameMode::iter() {
                                ui.selectable_value(&mut meta.game_mode, mode, format!("{}", mode));
                            }
                        });
                });
                ui.end_row();

                ui.label(get!("editor.metadata.authors"));
                ui.vertical(|ui| {
                    let r = ui.add(
                        egui::TextEdit::multiline(meta.authors_ui_text_mut())
                            .desired_width(ui.available_width())
                            .desired_rows(4)
                            .hint_text("anonymous"),
                    );
                    if r.changed() {
                        meta.apply_authors_ui_text_edit();
                    }
                    if r.lost_focus() {
                        meta.normalize_authors_ui_text();
                    }
                });
                ui.end_row();
            });
    });
}


