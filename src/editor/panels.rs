use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::edit_menu::{edit_menu, EditAction, EditAvailability};
use crate::common::file_menu::{dialog_directory, file_menu, FileAction};
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiPrimaryContextPass, EguiContexts};
use bevy_egui::egui::{Id, LayerId, Ui, UiBuilder, WidgetText};
use egui_dock::{DockArea, DockState, TabViewer};
use strum::IntoEnumIterator;
use strum_macros::Display;
use crate::common::mode::GameMode;
use crate::common::movement_rules::SprintPolicy;
use crate::common::sky::SkyTime;
use crate::common::shortcuts::chords;
use crate::common::toolbar;
use crate::common::net::{NetRequest, NetRole};
use crate::common::net_transport::NetStatus;
use crate::constants::{BLUEPRINTS_DIR, MAP_BLUEPRINT_EXTENSION};
use crate::editor::editable::{EditEvent, FeatureId, FeatureTimeline};
use crate::editor::map_metadata::MapMetadata;
use crate::common::palette::{Palette, RolePaint};
use crate::common::team::Team;
use crate::material::picker::material_button;
use crate::editor::multicam::MulticamState;
use crate::editor::net_menu::{network_menu, ConnectDialog};
use crate::editor::save::{self, LoadedBlueprint};
use crate::common::player_settings::{PlayerSettings, NAME_LIMIT};
use crate::game::rounds::RoundHistory;
use crate::get;
use crate::tool::Tools;
use crate::tool::bake_job::BakeProgress;
use crate::common::round_settings::{BotSkill, RoundSettings, MOST_SLOTS};
use crate::tool::bake_progress::{self, Step};
use crate::tool::bakes::{BakePlugin, BakeCommands, LogECS, RoomBake};
use crate::tool::sun_bake::{BakeSunlight, ClearSunlight, SunBake};
use crate::tool::nav_overlay::{NavCommands, NavPanel};
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
        if let Some(directory) = dialog_directory(existing.as_deref(), BLUEPRINTS_DIR) {
            dialog = dialog.set_directory(directory);
        }
        dialog = match existing.as_ref().and_then(|existing| existing.file_name()) {
            Some(name) => dialog.set_file_name(name.to_string_lossy().to_string()),
            None => dialog.set_file_name(format!("Untitled.{MAP_BLUEPRINT_EXTENSION}")),
        };
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
const MENU_BAR_PROGRESS_WIDTH: f32 = 120.0;
const TOOLBAR_ROW_HEIGHT: f32 = toolbar::BUTTON_HEIGHT + toolbar::ROW_PADDING * 2.0;
const TOOLBAR_DEFAULT_HEIGHT: f32 = TAB_BAR_HEIGHT + TOOLBAR_ROW_HEIGHT * 2.0;
const TOOLBAR_MIN_HEIGHT: f32 = TAB_BAR_HEIGHT + TOOLBAR_ROW_HEIGHT;

enum TabKinds {
    Tools,
    Bakes,
    Show,
    Metadata,
    /// How the server runs a round: slots, and bots to fill them.
    Round,
    /// The feature list with its rollback bar, and the selected feature's editor beneath.
    Features,
    /// The last rounds' final standings.
    Rounds,
    /// This machine's player: name and preferences.
    Player,
}

/// The panels' writes, and one read, bundled because `ui` hit Bevy's
/// sixteen-parameter limit.
#[derive(SystemParam)]
struct PanelMessages<'w> {
    rounds: Res<'w, RoundHistory>,
    round: ResMut<'w, RoundSettings>,
    player: ResMut<'w, PlayerSettings>,
    room: MessageWriter<'w, CalculateRoomGeometry>,
    clear_room: MessageWriter<'w, ClearRoomGeometry>,
    log_ecs: MessageWriter<'w, LogECS>,
    room_bake: ResMut<'w, RoomBake>,
    sun_bake: ResMut<'w, SunBake>,
    bake_progress: Res<'w, BakeProgress>,
    nav: NavPanel<'w>,
    sunlight: MessageWriter<'w, BakeSunlight>,
    clear_sunlight: MessageWriter<'w, ClearSunlight>,
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
    bake_progress: &'a BakeProgress,
    nav_commands: &'a mut NavCommands,
    gizmo_visibility: &'a mut GizmoVisibility,
    pending_edits: &'a mut PendingEditEvents,
    retarget_request: &'a mut Option<(FeatureId, String)>,
    /// Whether the Features panel changed the timeline, which it holds with
    /// change detection bypassed.
    timeline_touched: bool,
    /// A client watches the server's map; only its own Player settings stay
    /// editable.
    spectating: bool,
    rounds: &'a RoundHistory,
    round: &'a mut RoundSettings,
    /// Change detection bypassed, as the timeline's is: held for the whole
    /// frame it would read as changed every frame, and every change is saved
    /// and sent.
    player: &'a mut PlayerSettings,
    player_touched: bool,
}

impl<'a> TabViewer for TabViewerAndResources<'a> {
    type Tab = TabKinds;

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(match tab {
            TabKinds::Tools => "tools".to_owned(),
            TabKinds::Bakes => "bakes".to_owned(),
            TabKinds::Show => "show".to_owned(),
            TabKinds::Metadata => "metadata".to_owned(),
            TabKinds::Round => "round".to_owned(),
            TabKinds::Features => "features".to_owned(),
            TabKinds::Rounds => "rounds".to_owned(),
            TabKinds::Player => "player".to_owned(),
        })
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        match tab {
            TabKinds::Tools => { get!("tools.title").into() }
            TabKinds::Bakes => { get!("bakes.title").into() }
            TabKinds::Show => { get!("show.title").into() }
            TabKinds::Metadata => { get!("editor.metadata.title").into() }
            TabKinds::Round => { get!("editor.round.title").into() }
            TabKinds::Features => { get!("editor.timeline.title").into() }
            TabKinds::Rounds => { get!("editor.rounds.title").into() }
            TabKinds::Player => { get!("editor.player.title").into() }
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        if self.spectating && !matches!(tab, TabKinds::Player) {
            ui.disable();
        }
        match tab {
            TabKinds::Tools => {
                Tools::ui(ui, self.current_tool, self.next_tool);
            }
            TabKinds::Bakes => {
                *self.bake_commands = BakePlugin::ui(ui, self.bake_commands.auto, self.bake_commands.auto_sun, self.nav_commands, self.bake_progress);
            }
            TabKinds::Show => {
                ShowPlugin::ui(ui, self.multicam_state, self.gizmo_visibility);
            }
            TabKinds::Metadata => {
                map_metadata_panel_ui(ui, self.map_metadata);
            }
            TabKinds::Round => {
                round_panel_ui(ui, self.round);
            }
            TabKinds::Features => {
                self.timeline_touched |= FeatureTimeline::ui(
                    ui,
                    self.editor_features,
                    &mut self.pending_edits.events,
                    self.retarget_request,
                );
            }
            TabKinds::Rounds => {
                rounds_panel_ui(ui, self.rounds);
            }
            TabKinds::Player => {
                self.player_touched |= player_panel_ui(ui, self.player);
            }
        }
    }
}

pub struct EditorPanelPlugin;
impl Plugin for EditorPanelPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<EditorPanels>()
            .init_resource::<RoundSettings>()
            .init_resource::<CurrentFilePath>()
            .add_systems(Startup, EditorPanels::set_multicam_size)
            .add_systems(Startup, crate::common::assets::create_the_user_pack)
            .add_systems(EguiPrimaryContextPass, EditorPanels::ui.run_if(in_state(AppMode::Editor)))
            // Before the panel, so a chord is answered this frame.
            .add_systems(Update, file_shortcuts.run_if(in_state(AppMode::Editor)).run_if(crate::common::net::has_authority))
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
        let default_left_tabs = vec![TabKinds::Features, TabKinds::Bakes,];
        let default_right_tabs = vec![TabKinds::Show, TabKinds::Metadata, TabKinds::Round];
        let default_bottom_tabs = vec![TabKinds::Rounds, TabKinds::Player];
        
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
        
        let mut bake_commands = BakeCommands { auto: messages.room_bake.auto(), auto_sun: messages.sun_bake.auto(), ..default() };
        let mut nav_commands = messages.nav.commands();
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
            // Bypassed, and marked below only if the Features panel said it
            // changed something. Borrowed mutably for the whole frame, it
            // read as changed every frame — so the undo checkpoint serialized
            // the map on every idle frame and `map_sync`'s pre-filter never
            // filtered anything.
            editor_features: editor_features.bypass_change_detection(),
            map_metadata: &mut *map_metadata,
            multicam_state: &mut *multicam_state,
            bake_commands: &mut bake_commands,
            bake_progress: &messages.bake_progress,
            nav_commands: &mut nav_commands,
            gizmo_visibility: &mut *gizmo_visibility,
            pending_edits: &mut pending_edits,
            retarget_request: &mut retarget_request,
            timeline_touched: false,
            spectating: !net_role.is_authority(),
            rounds: &messages.rounds,
            // Through `bypass_change_detection`, as `player` is: held every frame, it would read as
            // changed every frame.
            round: messages.round.bypass_change_detection(),
            player: messages.player.bypass_change_detection(),
            player_touched: false,
        };

        panels.menu_bar_height = egui::Panel::top("menu_bar")
            .resizable(false)
            .show(&mut viewport_ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    // The network menu stays live, so a client can leave.
                    ui.add_enabled_ui(net_role.is_authority(), |ui| {
                        file_menu(ui, true, &mut pending_file_op);
                        edit_menu(ui, edit_available, &mut pending_edit);
                        // `Editor` spelled out: this system is gated on it.
                        mode_menu(ui, AppMode::Editor, Some(&net_role), &mut wanted_mode);
                    });
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
                    if let Some(step) = Step::current(&messages.bake_progress) {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            bake_progress::bar(ui, step.within(), MENU_BAR_PROGRESS_WIDTH);
                            ui.add(egui::Label::new(step.line()).truncate());
                        });
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

        let timeline_touched = viewer.timeline_touched;
        let player_touched = viewer.player_touched;
        drop(viewer);
        if timeline_touched {
            editor_features.set_changed();
        }
        if player_touched {
            messages.player.set_changed();
        }

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
                            editor_features.mark_saved(&map_metadata);
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
                // Both are untitled, so a save asks where rather than writing over the shipped file.
                FileAction::New | FileAction::DemoScene => {
                    let name = if op == FileAction::New { "new" } else { "demo" };
                    let template = PathBuf::from(format!(
                        "assets/default/{BLUEPRINTS_DIR}/{name}.{MAP_BLUEPRINT_EXTENSION}"
                    ));
                    match save::load(&template) {
                        Ok(loaded) => {
                            info!("New from template {:?}", template);
                            loaded_blueprint = Some(loaded);
                            current_file.path = None;
                        }
                        Err(e) => error!("New failed: {}", e),
                    }
                }
                FileAction::Save => match current_file.path.clone() {
                    Some(path) => match save::save(&path, &editor_features, &map_metadata) {
                        Ok(()) => {
                            info!("Saved to {:?}", path);
                            editor_features.mark_saved(&map_metadata);
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
                    let from = dialog_directory(current_file.path.as_deref(), BLUEPRINTS_DIR);
                    std::thread::spawn(move || {
                        let mut dialog = rfd::AsyncFileDialog::new()
                            .add_filter("Grackle Map Blueprint", &[MAP_BLUEPRINT_EXTENSION]);
                        if let Some(from) = from {
                            dialog = dialog.set_directory(from);
                        }
                        let handle = pollster::block_on(dialog.pick_file());
                        if let Some(h) = handle {
                            *slot.lock().unwrap() = Some(DialogResult::OpenPath(h.path().to_path_buf()));
                        }
                    });
                }
            }
        }

        // Revert is one undo step: only the features that differ from the file are rebuilt.
        match pending_edit {
            Some(EditAction::Undo) => editor_features.undo(&mut map_metadata),
            Some(EditAction::Redo) => editor_features.redo(&mut map_metadata),
            Some(EditAction::Revert) => {
                if let Some(path) = current_file.path.clone() {
                    match save::load(&path) {
                        Ok(loaded) => {
                            info!("Reverted to {:?}", path);
                            editor_features.replace_as_edit(loaded.timeline, &loaded.metadata, &mut map_metadata);
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

        if bake_commands.auto != messages.room_bake.auto() {
            messages.room_bake.set_auto(bake_commands.auto);
        }
        if bake_commands.auto_sun != messages.sun_bake.auto() {
            messages.sun_bake.set_auto(bake_commands.auto_sun);
        }
        if bake_commands.bake_sunlight {
            messages.sunlight.write(BakeSunlight);
        }
        if bake_commands.clear_sunlight {
            messages.clear_sunlight.write(ClearSunlight);
        }
        if bake_commands.calculate_room_geometry {
            messages.room.write(CalculateRoomGeometry);
        }
        if bake_commands.clear_room_geometry {
            messages.clear_room.write(ClearRoomGeometry);
        }
        messages.nav.apply(&nav_commands);
        if bake_commands.log_ecs {
            messages.log_ecs.write(LogECS);
        }

        if let Some((feature_id, label)) = retarget_request {
            retarget_state.target_feature = Some(feature_id);
            retarget_state.target_point_ref_key = label;
            retarget_state.hovered_point = None;
            // `set_if_different`, not `set`: a same-state `set` still runs the transition, and
            // `OnExit(Tools::Retarget)` would clear the `RetargetState` just filled in.
            next_tool.set_if_different(Tools::Retarget);
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

/// Returns whether anything changed.
fn player_panel_ui(ui: &mut egui::Ui, settings: &mut PlayerSettings) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(get!("editor.player.name"));
        changed |= ui
            .add(egui::TextEdit::singleline(&mut settings.name).char_limit(NAME_LIMIT))
            .changed();
    });
    changed |= ui.checkbox(&mut settings.auto_reload, get!("editor.player.auto_reload")).changed();
    changed
}

/// What ticking Time limit starts at, in seconds.
const DEFAULT_TIME_LIMIT: f32 = 600.0;

fn round_panel_ui(ui: &mut egui::Ui, settings: &mut RoundSettings) {
    ui.heading(get!("editor.round.slots"));
    ui.add(egui::DragValue::new(&mut settings.slots).range(1..=MOST_SLOTS));

    ui.heading(get!("editor.round.bots"));
    ui.checkbox(&mut settings.fill_with_bots, get!("editor.round.fill_with_bots"));
    let skill_name = |skill: BotSkill| match skill {
        BotSkill::Easy => get!("editor.round.skill.easy"),
        BotSkill::Normal => get!("editor.round.skill.normal"),
        BotSkill::Hard => get!("editor.round.skill.hard"),
    };
    egui::ComboBox::from_label(get!("editor.round.skill.title")).selected_text(skill_name(settings.bot_skill)).show_ui(ui, |ui| {
        for skill in BotSkill::ALL {
            ui.selectable_value(&mut settings.bot_skill, skill, skill_name(skill));
        }
    });

    ui.heading(get!("editor.round.time"));
    ui.horizontal(|ui| {
        let mut limited = settings.time_limit.is_some();
        if ui.checkbox(&mut limited, get!("editor.round.time_limit")).changed() {
            settings.time_limit = limited.then_some(DEFAULT_TIME_LIMIT);
        }
        if let Some(limit) = settings.time_limit.as_mut() {
            ui.add(egui::DragValue::new(limit).range(1.0..=f32::MAX).speed(1.0).max_decimals(0).suffix(get!("editor.round.seconds_suffix")));
        }
    });
}

/// Newest first, the newest open.
fn rounds_panel_ui(ui: &mut egui::Ui, history: &RoundHistory) {
    let mut any = false;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (index, round) in history.rounds().enumerate() {
            any = true;
            let length = round.length_text();
            egui::CollapsingHeader::new(get!("editor.rounds.round", "number", round.number, "length", length))
                .id_salt(("round", round.number))
                .default_open(index == 0)
                .show(ui, |ui| {
                    egui::Grid::new(("round_standings", round.number)).striped(true).show(ui, |ui| {
                        ui.label("");
                        ui.strong(get!("editor.rounds.player"));
                        ui.strong(get!("editor.rounds.frags"));
                        ui.strong(get!("editor.rounds.deaths"));
                        ui.end_row();
                        for standing in &round.standings {
                            let color = Srgba::from(standing.swatch());
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgb(
                                (color.red * 255.0).round() as u8,
                                (color.green * 255.0).round() as u8,
                                (color.blue * 255.0).round() as u8,
                            ));
                            ui.label(standing.display_name());
                            ui.label(standing.frags.to_string());
                            ui.label(standing.deaths.to_string());
                            ui.end_row();
                        }
                    });
                });
        }
    });
    if !any {
        ui.label(get!("editor.rounds.none"));
    }
}

fn map_metadata_panel_ui(ui: &mut egui::Ui, meta: &mut MapMetadata) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("editor_map_metadata_grid")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(get!("editor.metadata.display_name"));
                let mut name = meta.display_name.clone().unwrap_or_default();
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
                // Trimmed only once the field is left, or typing "2 Fort" would eat the space.
                if r.lost_focus() {
                    name = name.trim().to_owned();
                }
                if r.changed() || r.lost_focus() {
                    let name = (!name.trim().is_empty()).then_some(name);
                    if meta.display_name != name {
                        meta.display_name = name;
                    }
                }
                ui.end_row();

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

                ui.label(get!("editor.metadata.sky"));
                egui::ComboBox::from_id_salt("editor_map_sky")
                    .selected_text(format!("{}", meta.sky))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for sky in SkyTime::iter() {
                            ui.selectable_value(&mut meta.sky, sky, format!("{}", sky));
                        }
                    });
                ui.end_row();

                ui.label(get!("editor.metadata.sun_altitude"));
                ui.add(egui::DragValue::new(&mut meta.sun.altitude).range(0.0..=90.0).speed(0.5).suffix("°"));
                ui.end_row();

                ui.label(get!("editor.metadata.sun_azimuth"));
                ui.add(egui::DragValue::new(&mut meta.sun.azimuth).range(0.0..=360.0).speed(1.0).suffix("°"));
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

        ui.add_space(8.0);
        ui.heading(get!("editor.metadata.movement"));
        egui::Grid::new("editor_map_movement_grid")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(get!("editor.metadata.sprint"));
                egui::ComboBox::from_id_salt("editor_map_sprint")
                    .selected_text(format!("{}", meta.movement.sprint))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for policy in SprintPolicy::iter() {
                            ui.selectable_value(&mut meta.movement.sprint, policy, format!("{}", policy));
                        }
                    });
                ui.end_row();

                ui.label(get!("editor.metadata.gravity"));
                ui.add(
                    egui::DragValue::new(&mut meta.movement.gravity)
                        .range(0.0..=100.0)
                        .speed(0.1)
                        .suffix(" m/s²"),
                );
                ui.end_row();
            });

        ui.add_space(8.0);
        ui.heading(get!("editor.metadata.palette.title"));
        palette_ui(ui, &mut meta.palette);
    });
}

/// A role's name is committed when its field is left: renamed per keystroke, the role would move
/// through the sorted list under the cursor and lose the field's focus.
fn palette_ui(ui: &mut egui::Ui, palette: &mut Palette) {
    let mut rename = None;
    let mut remove = None;
    for (name, paint) in palette.roles.iter_mut() {
        ui.push_id(name, |ui| {
            ui.separator();
            ui.horizontal(|ui| {
                let buffer = ui.id().with("name");
                let mut text = ui.data(|data| data.get_temp::<String>(buffer)).unwrap_or_else(|| name.clone());
                let field = ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width() - 30.0));
                if field.lost_focus() {
                    ui.data_mut(|data| data.remove_temp::<String>(buffer));
                    let text = text.trim();
                    if !text.is_empty() && text != name {
                        rename = Some((name.clone(), text.to_owned()));
                    }
                } else if field.has_focus() {
                    ui.data_mut(|data| data.insert_temp(buffer, text));
                }
                if ui.button("×").clicked() {
                    remove = Some(name.clone());
                }
            });
            egui::Grid::new("sides").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                for team in Team::ALL {
                    ui.label(team.name());
                    let mut material = paint.sides.get(&team).cloned();
                    if material_button(ui, &mut material, false) {
                        match material {
                            Some(material) => paint.sides.insert(team, material),
                            None => paint.sides.remove(&team),
                        };
                    }
                    ui.end_row();
                }
                ui.label(get!("editor.metadata.palette.unsided"));
                material_button(ui, &mut paint.unsided, false);
                ui.end_row();
            });
        });
    }
    if let Some(name) = remove {
        palette.roles.remove(&name);
    }
    if let Some((from, to)) = rename
        && !palette.roles.contains_key(&to)
        && let Some(paint) = palette.roles.remove(&from)
    {
        palette.roles.insert(to, paint);
    }
    ui.separator();
    if ui.button(get!("editor.metadata.palette.add")).clicked() {
        let base = get!("editor.metadata.palette.new_role");
        let name = std::iter::once(base.clone())
            .chain((2..).map(|n| format!("{base} {n}")))
            .find(|name| !palette.roles.contains_key(name))
            .expect("an unbounded sequence of names has a free one");
        palette.roles.insert(name, RolePaint::default());
    }
}


