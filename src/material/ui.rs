//! Grackle's menu bar over texture-graph's panels, and the chords and file dialogs around them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy_egui::egui::{self, Id, LayerId, Ui, UiBuilder};
use bevy_egui::EguiContexts;

use super::gpu::{BakedTextures, BevyEguiTextures};
use super::{load, save, MaterialEditor, FILE_EXTENSION, MATERIALS_DIR};
use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::edit_menu::{edit_menu, EditAction, EditAvailability};
use crate::common::file_menu::{dialog_directory, file_menu, FileAction};
use crate::common::net::NetRole;
use crate::common::shortcuts::chords;
use crate::get;

/// What a file dialog came back with. On a background thread, like the prop editor's, so not
/// wasm-ready.
enum DialogResult {
    Save(PathBuf),
    Open(PathBuf),
}

#[derive(Resource, Clone, Default)]
pub struct MaterialFileDialog {
    result: Arc<Mutex<Option<DialogResult>>>,
}

#[derive(Default)]
struct Requests {
    file: Option<FileAction>,
    edit: Option<EditAction>,
    mode: Option<AppMode>,
}

pub struct MaterialUiPlugin;

impl Plugin for MaterialUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MaterialFileDialog>()
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                panels.run_if(in_state(AppMode::Material)),
            )
            .add_systems(Update, material_shortcuts.run_if(in_state(AppMode::Material)));
    }
}

fn edit_availability(material: &MaterialEditor) -> EditAvailability {
    EditAvailability {
        undo: material.can_undo(),
        redo: material.can_redo(),
        revert: material.path.is_some() && material.dirty(),
    }
}

fn panels(
    mut contexts: EguiContexts,
    mut material: NonSendMut<MaterialEditor>,
    dialog: Res<MaterialFileDialog>,
    mut next_mode: ResMut<NextState<AppMode>>,
    // Optional: a build with no `NetPlugin` is a closed game, its own authority.
    role: Option<Res<NetRole>>,
    mut exit: MessageWriter<AppExit>,
    mut baked: ResMut<BakedTextures>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();
    let mut window_ui = Ui::new(
        ctx.clone(),
        Id::new("grackle_material_window"),
        UiBuilder::new().layer_id(LayerId::background()).max_rect(ctx.viewport_rect()),
    );

    let mut requests = Requests::default();
    egui::Panel::top("material_menu_bar").resizable(false).show(&mut window_ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            file_menu(ui, false, &mut requests.file);
            edit_menu(ui, edit_availability(&material), &mut requests.edit);
            // This system is gated on `AppMode::Material`.
            mode_menu(ui, AppMode::Material, role.as_deref(), &mut requests.mode);
            ui.separator();
            ui.label(match &material.path {
                Some(path) => path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                None => get!("material.untitled"),
            });
            if material.dirty() {
                ui.label("*");
            }
        });
    });

    let mut textures = BevyEguiTextures { egui: &mut contexts, baked: &mut baked };
    let changed = material.editor.show(&mut window_ui, Some(&mut textures));
    material.edited(changed);
    material.checkpoint_when_idle(!ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input());

    handle_edit(requests.edit, &mut material);
    // `mode_menu` never offers the current mode, so this is always a change.
    if let Some(mode) = requests.mode {
        next_mode.set(mode);
    }
    handle_files(requests.file, &mut material, &dialog);
    if requests.file == Some(FileAction::Quit) {
        info!("Quitting, as asked");
        exit.write(AppExit::Success);
    }
}

/// Chords go through the menus' handlers, so a chord and its menu entry are one action.
fn material_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    egui: Res<bevy_egui::input::EguiWantsInput>,
    dialog: Res<MaterialFileDialog>,
    mut material: NonSendMut<MaterialEditor>,
) {
    let chords = chords(&keys, egui.wants_keyboard_input());
    handle_edit(
        match (chords.redo, chords.undo) {
            (true, _) => Some(EditAction::Redo),
            (false, true) => Some(EditAction::Undo),
            _ => None,
        },
        &mut material,
    );
    if let Some(action) = FileAction::from_chords(chords) {
        handle_files(Some(action), &mut material, &dialog);
    }
}

fn handle_edit(action: Option<EditAction>, material: &mut MaterialEditor) {
    match action {
        Some(EditAction::Undo) => material.undo(),
        Some(EditAction::Redo) => material.redo(),
        Some(EditAction::Revert) => {
            let Some(path) = material.path.clone() else { return };
            match load(&path) {
                Ok(saved) => {
                    info!("Reverted the material to {path:?}");
                    material.revert_to(saved);
                }
                Err(e) => error!("Reverting the material: {e}"),
            }
        }
        None => {}
    }
}

/// New, open and save. Quit is the caller's. Called every frame, to collect a dialog's answer.
fn handle_files(action: Option<FileAction>, material: &mut MaterialEditor, dialog: &MaterialFileDialog) {
    if action == Some(FileAction::New) {
        material.open(texture_graph_core::Graph::new(), None);
    }

    if let Some(result) = dialog.result.lock().unwrap().take() {
        match result {
            DialogResult::Save(path) => save_to(material, path),
            DialogResult::Open(path) => match load(&path) {
                Ok(graph) => {
                    info!("Opened material {path:?}");
                    material.open(graph, Some(path));
                }
                Err(e) => error!("Opening the material: {e}"),
            },
        }
    }

    match action {
        Some(FileAction::Save) => match material.path.clone() {
            Some(path) => save_to(material, path),
            None => ask_where_to_save(material, dialog),
        },
        Some(FileAction::SaveAs) => ask_where_to_save(material, dialog),
        Some(FileAction::Open) => {
            let slot = dialog.result.clone();
            let from = dialog_directory(material.path.as_deref(), MATERIALS_DIR);
            std::thread::spawn(move || {
                let mut chooser =
                    rfd::AsyncFileDialog::new().add_filter("Texture Graph", &[FILE_EXTENSION]);
                if let Some(from) = from {
                    chooser = chooser.set_directory(from);
                }
                if let Some(handle) = pollster::block_on(chooser.pick_file()) {
                    *slot.lock().unwrap() = Some(DialogResult::Open(handle.path().to_path_buf()));
                }
            });
        }
        _ => {}
    }
}

fn save_to(material: &mut MaterialEditor, path: PathBuf) {
    match save(&path, material.graph()) {
        Ok(()) => {
            info!("Saved material to {path:?}");
            material.mark_saved(path);
        }
        Err(e) => error!("Saving the material: {e}"),
    }
}

fn ask_where_to_save(material: &MaterialEditor, dialog: &MaterialFileDialog) {
    let slot = dialog.result.clone();
    let from = dialog_directory(material.path.as_deref(), MATERIALS_DIR);
    std::thread::spawn(move || {
        let mut chooser = rfd::AsyncFileDialog::new()
            .add_filter("Texture Graph", &[FILE_EXTENSION])
            .set_file_name(format!("untitled.{FILE_EXTENSION}"));
        if let Some(from) = from {
            chooser = chooser.set_directory(from);
        }
        if let Some(handle) = pollster::block_on(chooser.save_file()) {
            *slot.lock().unwrap() = Some(DialogResult::Save(handle.path().to_path_buf()));
        }
    });
}
