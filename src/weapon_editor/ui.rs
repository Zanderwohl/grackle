//! The weapon editor's window: the menu bar, the Weapons browser on the left and the selected
//! weapon on the right, each its own `DockState` as the prop editor's are, and the range's strip
//! under the middle, which [`super::range`]'s cameras draw into.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy_egui::egui::{self, Id, LayerId, Ui, UiBuilder};
use egui_dock::{DockArea, DockState, TabViewer};

use super::form::{edit_struct, unit_key, Form, FormMemory};
use super::preview::{damage_over_distance, pellet_pattern};
use super::range::RangeShow;
use super::{save_one, save_the_draft, KeyProblem, PublishedCatalog, WeaponEditor};
use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::assets::{default_packs, Assets};
use crate::common::class::Class;
use crate::common::edit_menu::{edit_menu, EditAction, EditAvailability};
use crate::common::file_menu::{dialog_directory, file_menu, DocumentKind, FileAction};
use crate::common::library::{pack_name, Library, NewDialog, NewOutcome};
use crate::common::list_row::{begin_rename, confirm_delete, renamable_row, Leading, RowEvent};
use crate::common::recent_files::RecentFiles;
use crate::common::net::NetRole;
use crate::common::shortcuts::chords;
use crate::common::weapon::{Mounted, Unit, WeaponAction, BUCKETS};
use crate::common::weapon_file::{read_pack_weapon, read_pack_weapons, weapon_file, weapon_path, WEAPONS_DIR, WEAPON_EXTENSION};
use crate::editor::editable::FeatureTimeline;
use crate::get;
use crate::prop::document::{load, PropDoc, PropEditor};
use crate::prop::picker::{prop_button, source_of, PropPreviews};

pub struct WeaponUiPlugin;

impl Plugin for WeaponUiPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        crate::common::recent_files::ensure(app);
        crate::common::library::ensure(app);
        app.init_resource::<WeaponPanels>()
            .init_resource::<WeaponFileDialog>()
            // A prop saved in another mode is read again.
            .add_systems(OnEnter(AppMode::Weapon), |mut panels: ResMut<WeaponPanels>| panels.browser.model = None)
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (panels, show_weapon_library.run_if(crate::common::library::showing(DocumentKind::Weapon)))
                    .chain()
                    .run_if(in_state(AppMode::Weapon)),
            )
            .add_systems(Update, weapon_shortcuts.run_if(in_state(AppMode::Weapon)));
    }
}

/// Collected on a later frame from the dialog's thread.
enum DialogResult {
    Open(PathBuf),
    /// The weapon at `pack` and `key` when Save As was asked, and where it goes.
    SaveAs { pack: PathBuf, key: String, path: PathBuf },
}

#[derive(Resource, Clone, Default)]
pub struct WeaponFileDialog {
    result: Arc<Mutex<Option<DialogResult>>>,
}

const THUMBNAIL: f32 = 24.0;

/// A held prop's clips, and whether the game falls back to a shared one where it has none.
const CLIPS: &[(&str, bool)] = &[
    ("fire", true),
    ("alt_fire", true),
    ("recoil", true),
    ("alt_recoil", true),
    ("reload", true),
    ("body_reload", true),
    ("deploy", false),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WeaponTab {
    Weapons,
    Weapon,
}

/// What the browser is in the middle of. View state: not saved, not undone.
#[derive(Default)]
struct Browser {
    new: Option<NewDialog>,
    rename_loadouts: bool,
    deleting: Option<(PathBuf, String)>,
    /// The selected weapon's model as its file held it, read when the name changes or the mode
    /// opens. Only for the summary: opening it reads the file again.
    model: Option<(String, Option<(PathBuf, PropDoc)>)>,
    /// What names the selected weapon, as of a generation of the draft.
    dependants: Option<(u64, usize, Vec<(PathBuf, String, crate::common::weapon::Slot)>)>,
}

#[derive(Resource)]
pub struct WeaponPanels {
    left: DockState<WeaponTab>,
    right: DockState<WeaponTab>,
    browser: Browser,
}

impl Default for WeaponPanels {
    fn default() -> Self {
        Self {
            left: DockState::new(vec![WeaponTab::Weapons]),
            right: DockState::new(vec![WeaponTab::Weapon]),
            browser: Browser { rename_loadouts: true, ..default() },
        }
    }
}

#[derive(Default)]
struct Requests {
    file: Option<FileAction>,
    edit: Option<EditAction>,
    mode: Option<AppMode>,
}

fn edit_availability(editor: &WeaponEditor) -> EditAvailability {
    EditAvailability { undo: editor.can_undo(), redo: editor.can_redo(), revert: editor.dirty() }
}

#[allow(clippy::too_many_arguments)]
fn panels(
    mut previews: PropPreviews,
    mut editor: ResMut<WeaponEditor>,
    mut panels: ResMut<WeaponPanels>,
    mut published: ResMut<PublishedCatalog>,
    dialog: Res<WeaponFileDialog>,
    mut memory: ResMut<FormMemory>,
    mut prop_editor: ResMut<PropEditor>,
    mut show: ResMut<RangeShow>,
    (assets, registry, timeline): (Res<Assets>, Res<AppTypeRegistry>, Res<FeatureTimeline>),
    mut next_mode: ResMut<NextState<AppMode>>,
    // Optional: a build with no `NetPlugin` is a closed game, its own authority.
    role: Option<Res<NetRole>>,
    mut exit: MessageWriter<AppExit>,
    mut recent: ResMut<RecentFiles>,
    mut library: ResMut<Library>,
) {
    let Some(ctx) = previews.ctx().cloned() else { return };
    let mut window_ui = Ui::new(
        ctx.clone(),
        Id::new("grackle_weapon_window"),
        UiBuilder::new().layer_id(LayerId::background()).max_rect(ctx.viewport_rect()),
    );
    previews.want_pictures(editor.weapons.iter().filter_map(|weapon| weapon.entry.model.as_deref()));

    let mut requests = Requests::default();
    egui::Panel::top("weapon_menu_bar").resizable(false).show(&mut window_ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            file_menu(ui, DocumentKind::Weapon, recent.of(DocumentKind::Weapon), &mut requests.file);
            edit_menu(ui, edit_availability(&editor), &mut requests.edit);
            // This system is gated on `AppMode::Weapon`.
            mode_menu(ui, AppMode::Weapon, role.as_deref(), &mut requests.mode);
            ui.separator();
            if let Some(weapon) = editor.selected.and_then(|index| editor.weapons.get(index)) {
                ui.label(format!("{}/{}", pack_name(&weapon.pack), weapon_file(&weapon.key)));
            }
            if editor.dirty() {
                ui.label("*");
            }
        });
    });

    let registry = registry.read();
    let packs = editor.packs();
    // Only asked while a rename or delete is open.
    let asking = panels.browser.deleting.is_some() || crate::common::list_row::any_rename(&ctx);
    let pickups: Vec<(String, String)> = match asking {
        true => timeline
            .features_map()
            .values()
            .filter_map(|feature| Some((feature.label(), feature.object().weapon_named()?.to_owned())))
            .collect(),
        false => Vec::new(),
    };
    let WeaponPanels { left, right, browser } = &mut *panels;
    let mut tabs = Tabs {
        editor: &mut editor,
        browser,
        previews: &mut previews,
        form: Form { registry: &registry, memory: &mut memory },
        packs: &packs,
        pickups: &pickups,
        assets: &assets,
        prop_editor: &mut prop_editor,
        open_mode: None,
        changed: false,
    };
    let dock_frame = crate::common::dock::dock_panel_frame(window_ui.style());
    egui::Panel::left("weapon_left").resizable(true).default_size(300.0).frame(dock_frame).show(&mut window_ui, |ui| {
        DockArea::new(left)
            .id(Id::new("egui_dock::DockArea::weapon_left"))
            .show_close_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_leaf_collapse_buttons(false)
            .draggable_tabs(false)
            .show_inside(ui, &mut tabs);
    });
    egui::Panel::right("weapon_right").resizable(true).default_size(420.0).frame(dock_frame).show(&mut window_ui, |ui| {
        DockArea::new(right)
            .id(Id::new("egui_dock::DockArea::weapon_right"))
            .show_close_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_leaf_collapse_buttons(false)
            .draggable_tabs(false)
            .show_inside(ui, &mut tabs);
    });
    let (changed, open_mode) = (tabs.changed, tabs.open_mode);
    egui::Panel::bottom("weapon_strip").resizable(true).default_size(150.0).show(&mut window_ui, |ui| strip(ui, &mut show));
    // What is left is the range's: the views are drawn there by their cameras, under egui.
    show.view = Some(window_ui.available_rect_before_wrap());
    if changed {
        editor.edited();
    }
    editor.checkpoint_when_idle(!ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input());

    handle_edit(requests.edit, &mut editor, &assets);
    // `mode_menu` never offers the current mode, so this is always a change.
    if let Some(mode) = requests.mode.or(open_mode) {
        next_mode.set(mode);
    }
    if requests.file == Some(FileAction::Library) {
        library.open(DocumentKind::Weapon);
    } else if requests.file.is_none() {
        requests.file = library.take(DocumentKind::Weapon);
    }
    if requests.file == Some(FileAction::Quit) {
        info!("Quitting from the File menu");
        exit.write(AppExit::Success);
    } else if requests.file.is_some() {
        handle_files(requests.file, &mut editor, &mut panels.browser, &mut published, &assets, &dialog, &mut recent);
    }
}

/// New, Open, Save and Save As, for the menu and the chords alike. Called every frame from
/// [`weapon_shortcuts`] too, to collect a dialog's answer.
fn handle_files(
    action: Option<FileAction>,
    editor: &mut WeaponEditor,
    browser: &mut Browser,
    published: &mut PublishedCatalog,
    assets: &Assets,
    dialog: &WeaponFileDialog,
    recent: &mut RecentFiles,
) {
    remember_what_was_written(editor, recent);
    if let Some(result) = dialog.result.lock().unwrap().take() {
        match result {
            DialogResult::Open(path) => open_file(editor, assets, &path),
            DialogResult::SaveAs { pack, key, path } => save_as(editor, published, &pack, &key, &path),
        }
    }
    let selected = editor.selected.and_then(|index| editor.weapons.get(index)).map(|weapon| (weapon.pack.clone(), weapon.key.clone()));
    // Only asked for a dialog: it reads the disk, and this runs every frame.
    let from = || selected.as_ref().and_then(|(pack, key)| dialog_directory(Some(&weapon_path(pack, key)), WEAPONS_DIR));
    let filter = get!("weapon_editor.file_filter");
    match action {
        // A weapon's key is its file's name, so a new one is asked for its key and pack.
        Some(FileAction::New) => browser.new = Some(NewDialog::new(DocumentKind::Weapon)),
        Some(FileAction::Create(item)) => {
            if let Err(e) = item.make_folder() {
                error!("Making a weapon: {e}");
            }
            editor.new_weapon(item.pack, &item.name);
        }
        Some(FileAction::OpenPath(path)) => open_file(editor, assets, &path),
        Some(FileAction::Demo(demo)) => open_file(editor, assets, Path::new(demo.path)),
        Some(FileAction::Save) => save_the_draft(editor, assets, published),
        Some(FileAction::SaveAs) => {
            let from = from();
            let Some((pack, key)) = selected else {
                warn!("Save As saves the selected weapon, and none is selected");
                return;
            };
            let slot = dialog.result.clone();
            std::thread::spawn(move || {
                let mut chooser = rfd::AsyncFileDialog::new()
                    .add_filter(filter, &[WEAPON_EXTENSION])
                    .set_file_name(format!("{key}.{WEAPON_EXTENSION}"));
                if let Some(from) = from {
                    chooser = chooser.set_directory(from);
                }
                if let Some(handle) = pollster::block_on(chooser.save_file()) {
                    *slot.lock().unwrap() = Some(DialogResult::SaveAs { pack, key, path: handle.path().to_path_buf() });
                }
            });
        }
        Some(FileAction::Open) => {
            let slot = dialog.result.clone();
            let from = from().or_else(|| dialog_directory(None, WEAPONS_DIR));
            std::thread::spawn(move || {
                let mut chooser = rfd::AsyncFileDialog::new().add_filter(filter, &[WEAPON_EXTENSION]);
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
    remember_what_was_written(editor, recent);
}

fn remember_what_was_written(editor: &mut WeaponEditor, recent: &mut RecentFiles) {
    for path in std::mem::take(&mut editor.written) {
        recent.saved(DocumentKind::Weapon, &path);
    }
}

/// The pack and key a weapon file at `path` is: `{pack}/weapons/{key}.toml`, the pack named as the
/// draft names it when it is one of `packs`.
fn locate(path: &Path, packs: &[PathBuf]) -> Result<(PathBuf, String), String> {
    let folder = path.parent().filter(|folder| folder.file_name().is_some_and(|name| name == WEAPONS_DIR));
    let (Some(folder), true) = (folder, path.extension().is_some_and(|extension| extension == WEAPON_EXTENSION)) else {
        return Err(format!("{} is not in a pack's {WEAPONS_DIR} folder as a .{WEAPON_EXTENSION} file", path.display()));
    };
    let key = path.file_stem().and_then(|stem| stem.to_str()).ok_or_else(|| format!("{} has no name", path.display()))?;
    let pack = folder.parent().unwrap_or(Path::new(""));
    let same = |of: &Path| match (std::fs::canonicalize(of), std::fs::canonicalize(pack)) {
        (Ok(a), Ok(b)) => a == b,
        _ => of == pack,
    };
    let pack = packs.iter().find(|of| same(of)).cloned().unwrap_or_else(|| pack.to_path_buf());
    Ok((pack, key.to_owned()))
}

fn open_file(editor: &mut WeaponEditor, assets: &Assets, path: &Path) {
    let (pack, key) = match locate(path, &editor.packs()) {
        Ok(found) => found,
        Err(e) => return error!("Opening a weapon: {e}"),
    };
    if let Some(weapon) = read_pack_weapon(assets, &pack, &key) {
        editor.adopt(weapon);
        info!("Opened {}", path.display());
    }
}

fn save_as(editor: &mut WeaponEditor, published: &mut PublishedCatalog, pack: &Path, key: &str, path: &Path) {
    let (to_pack, to_key) = match locate(path, &editor.packs()) {
        Ok(found) => found,
        Err(e) => return error!("Saving a weapon: {e}"),
    };
    if editor.key_problem(&to_key, None) == Some(KeyProblem::Malformed) {
        return warn!("Saving a weapon: {to_key} {}", get!("weapon_editor.key.malformed"));
    }
    let Some(index) = editor.weapons.iter().position(|weapon| weapon.pack == pack && weapon.key == key) else {
        return error!("Saving a weapon: {key} is no longer in the draft");
    };
    if editor.save_as(index, to_pack.clone(), &to_key).is_some() {
        save_one(editor, &to_pack, &to_key, published);
    }
}

struct Tabs<'a, 'w, 's> {
    editor: &'a mut WeaponEditor,
    browser: &'a mut Browser,
    previews: &'a mut PropPreviews<'w, 's>,
    form: Form<'a>,
    packs: &'a [PathBuf],
    /// Each placed weapon pickup in the open map, by label, with the key it names.
    pickups: &'a [(String, String)],
    assets: &'a Assets,
    prop_editor: &'a mut PropEditor,
    open_mode: Option<AppMode>,
    changed: bool,
}

impl TabViewer for Tabs<'_, '_, '_> {
    type Tab = WeaponTab;

    fn id(&mut self, tab: &mut WeaponTab) -> Id {
        Id::new(match tab {
            WeaponTab::Weapons => "weapon_weapons",
            WeaponTab::Weapon => "weapon_weapon",
        })
    }

    fn title(&mut self, tab: &mut WeaponTab) -> egui::WidgetText {
        match tab {
            WeaponTab::Weapons => get!("weapon_editor.tabs.weapons").into(),
            WeaponTab::Weapon => get!("weapon_editor.tabs.weapon").into(),
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut WeaponTab) {
        match tab {
            WeaponTab::Weapons => self.browser_ui(ui),
            WeaponTab::Weapon => {
                egui::ScrollArea::vertical().show(ui, |ui| self.weapon_ui(ui));
            }
        }
    }
}

/// The rule a weapon's key has beyond its own pack: unique across every pack, ids included.
fn taken_elsewhere(editor: &WeaponEditor, key: &str) -> Option<String> {
    (editor.key_problem(key, None) == Some(KeyProblem::Taken)).then(|| get!("weapon_editor.key.taken"))
}

/// The Library for weapons, pictured by their models' thumbnails.
fn show_weapon_library(
    mut previews: PropPreviews,
    mut library: ResMut<Library>,
    editor: Res<WeaponEditor>,
    assets: Res<Assets>,
    mut models: Local<(u64, std::collections::HashMap<PathBuf, Option<String>>)>,
) {
    if models.0 != library.opened {
        *models = (library.opened, default());
    }
    for entry in library.entries() {
        models.1.entry(entry.path.clone()).or_insert_with(|| {
            read_pack_weapon(&assets, &entry.pack, &entry.name).and_then(|weapon| weapon.entry.model)
        });
    }
    previews.want_pictures(models.1.values().flatten().map(String::as_str));
    let Some(ctx) = previews.ctx().cloned() else { return };
    let models = &models.1;
    library.show(
        &ctx,
        &mut |entry| models.get(&entry.path).cloned().flatten().and_then(|model| previews.picture(&model)),
        &|key| taken_elsewhere(&editor, key),
    );
}

impl Tabs<'_, '_, '_> {
    fn browser_ui(&mut self, ui: &mut Ui) {
        if ui.button(get!("library.new.button")).clicked() {
            self.browser.new = Some(NewDialog::new(DocumentKind::Weapon));
        }
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| self.list_ui(ui));

        let ctx = ui.ctx().clone();
        if let Some(dialog) = self.browser.new.as_mut() {
            let editor = &*self.editor;
            match dialog.show(&ctx, &|key| taken_elsewhere(editor, key)) {
                NewOutcome::Asking => {}
                NewOutcome::Canceled => self.browser.new = None,
                NewOutcome::Create(item) => {
                    // A new pack has no folder yet, and the draft's packs are the folders on disk.
                    if let Err(e) = item.make_folder() {
                        error!("Making a weapon: {e}");
                    }
                    self.editor.new_weapon(item.pack, &item.name);
                    self.browser.new = None;
                }
            }
        }
        let doomed = self.browser.deleting.clone();
        let found = doomed.and_then(|(pack, key)| self.editor.weapons.iter().position(|weapon| weapon.pack == pack && weapon.key == key));
        if self.browser.deleting.is_some() && found.is_none() {
            self.browser.deleting = None;
        }
        if let Some(index) = found {
            let key = self.editor.weapons[index].key.clone();
            let id = Id::new("weapon_confirm_delete");
            let more = |ui: &mut Ui| {
                self.dependants_ui(ui, index);
                ui.weak(get!("library.delete.to_trash_on_save"));
            };
            match confirm_delete(&ctx, id, &key, more) {
                Some(true) => {
                    self.editor.delete(index);
                    self.browser.deleting = None;
                }
                Some(false) => self.browser.deleting = None,
                None => {}
            }
        }
    }

    /// What names the weapon by key: every pack's loadouts, and the open map's pickups. Blueprints
    /// on disk are not read.
    fn dependants_ui(&mut self, ui: &mut Ui, index: usize) {
        let generation = self.editor.generation;
        if !matches!(&self.browser.dependants, Some((of, at, _)) if *of == generation && *at == index) {
            self.browser.dependants = Some((generation, index, self.editor.loadouts_naming(index)));
        }
        let key = &self.editor.weapons[index].key;
        let loadouts = self.browser.dependants.as_ref().map(|(_, _, found)| found.clone()).unwrap_or_default();
        let pickups: Vec<&String> =
            self.pickups.iter().filter(|(_, named)| named == key).map(|(label, _)| label).collect();
        if loadouts.is_empty() && pickups.is_empty() {
            ui.weak(get!("weapon_editor.named_by.nothing"));
            return;
        }
        for (pack, class, slot) in loadouts {
            let class = Class::named(&class).map(|class| class.name()).unwrap_or(class);
            ui.label(get!("weapon_editor.named_by.loadout", "class", class, "slot", slot.name(), "pack", pack_name(&pack)));
        }
        for label in pickups {
            ui.label(get!("weapon_editor.named_by.pickup", "name", label));
        }
    }

    /// By bucket, as the selector lays them out; a bucket no key selects last, so it can be fixed.
    fn list_ui(&mut self, ui: &mut Ui) {
        let rows: Vec<(usize, u8)> =
            self.editor.weapons.iter().enumerate().map(|(index, weapon)| (index, weapon.entry.bucket)).collect();
        for bucket in (1..=BUCKETS).map(Some).chain([None]) {
            let in_bucket: Vec<usize> = rows
                .iter()
                .filter(|(_, of)| match bucket {
                    Some(bucket) => *of == bucket,
                    None => !(1..=BUCKETS).contains(of),
                })
                .map(|(index, _)| *index)
                .collect();
            if in_bucket.is_empty() {
                continue;
            }
            if let Some(bucket) = bucket {
                ui.label(egui::RichText::new(bucket.to_string()).strong());
            }
            for index in in_bucket {
                let before = self.editor.weapons.len();
                self.row_ui(ui, index);
                // A duplicate or override inserted a row, so the indices after it moved; the rest
                // are drawn next frame.
                if self.editor.weapons.len() != before {
                    return;
                }
            }
        }
    }

    fn row_ui(&mut self, ui: &mut Ui, index: usize) {
        let weapon = &self.editor.weapons[index];
        let id = Id::new(("weapon_row", &weapon.pack, &weapon.key));
        let picture = weapon.entry.model.as_deref().and_then(|model| self.previews.picture(model));
        let broken = self.editor.problems.iter().any(|(at, _)| *at == index);
        let mut name =
            egui::RichText::new(format!("{}  ·  {}", get!(&weapon.entry.name_key), pack_name(&weapon.pack)));
        if broken {
            name = name.color(ui.visuals().warn_fg_color);
        }
        let selected = self.editor.selected == Some(index);
        let editor = &*self.editor;
        let problem = |key: &str| {
            editor.key_problem(key, Some(index)).map(|problem| match problem {
                KeyProblem::Malformed => get!("weapon_editor.key.malformed"),
                KeyProblem::Taken => get!("weapon_editor.key.taken"),
            })
        };
        let leading = Some(Leading { picture, size: THUMBNAIL });
        match renamable_row(ui, id, selected, name, leading, problem) {
            RowEvent::Row(response) => {
                if response.clicked() {
                    self.editor.selected = Some(index);
                }
                response.context_menu(|ui| self.row_menu(ui, index, id));
            }
            RowEvent::Renaming => {
                ui.indent(id, |ui| {
                    ui.checkbox(&mut self.browser.rename_loadouts, get!("weapon_editor.browser.rename_loadouts"));
                    self.dependants_ui(ui, index);
                });
            }
            RowEvent::Renamed(key) => {
                if key != self.editor.weapons[index].key {
                    self.editor.rename(index, &key, self.browser.rename_loadouts);
                }
            }
            RowEvent::PutBack => {}
        }
    }

    fn row_menu(&mut self, ui: &mut Ui, index: usize, id: Id) {
        let weapon = self.editor.weapons[index].clone();
        if ui.button(get!("weapon_editor.browser.rename")).clicked() {
            self.editor.selected = Some(index);
            begin_rename(ui.ctx(), id, &weapon.key);
            ui.close();
        }
        if ui.button(get!("weapon_editor.browser.duplicate")).clicked() {
            let key = (2..)
                .map(|n| if n == 2 { format!("{}_copy", weapon.key) } else { format!("{}_copy{n}", weapon.key) })
                .find(|key| self.editor.key_problem(key, None).is_none())
                .unwrap_or_default();
            let at = self.editor.duplicate(index, weapon.pack.clone(), &key);
            self.editor.selected = Some(at);
            begin_rename(ui.ctx(), Id::new(("weapon_row", &weapon.pack, &key)), &key);
            ui.close();
        }
        let rank = |pack: &PathBuf| self.packs.iter().position(|of| of == pack);
        let later: Vec<PathBuf> = self.packs.iter().filter(|pack| rank(pack) > rank(&weapon.pack)).cloned().collect();
        ui.add_enabled_ui(!later.is_empty(), |ui| {
            ui.menu_button(get!("weapon_editor.browser.override"), |ui| {
                for pack in later {
                    if ui.button(pack_name(&pack)).clicked() {
                        match self.editor.override_in(index, pack.clone()) {
                            Some(at) => self.editor.selected = Some(at),
                            None => warn!("{} cannot be overridden in {}", weapon.key, pack_name(&pack)),
                        }
                        ui.close();
                    }
                }
            });
        });
        ui.separator();
        if ui.button(get!("weapon_editor.browser.delete")).clicked() {
            self.browser.deleting = Some((weapon.pack.clone(), weapon.key.clone()));
            ui.close();
        }
    }

    fn weapon_ui(&mut self, ui: &mut Ui) {
        let Some(index) = self.editor.selected.filter(|index| *index < self.editor.weapons.len()) else { return };
        let problems: Vec<String> =
            self.editor.problems.iter().filter(|(at, _)| *at == index).map(|(_, problem)| problem.describe()).collect();
        let model = self.editor.weapons[index].entry.model.clone();
        self.read_the_model(model.as_deref());
        let mut changed = false;

        let weapon = &mut self.editor.weapons[index];
        ui.heading(get!(&weapon.entry.name_key));
        ui.label(format!("{}  ·  {}", weapon.key, weapon.pack.display()));
        for why in &problems {
            ui.colored_label(ui.visuals().error_fg_color, why);
        }
        if let Some(icon) = weapon.entry.icon.as_deref().filter(|icon| !crate::common::weapon_file::icon_exists(self.assets, icon)) {
            ui.colored_label(ui.visuals().warn_fg_color, get!("weapon_editor.problems.icon", "icon", icon));
        }
        ui.separator();

        egui::Grid::new("weapon_settings").num_columns(2).show(ui, |ui| {
            let entry = &mut weapon.entry;
            ui.label(get!("weapon_editor.fields.name_key"));
            changed |= ui.text_edit_singleline(&mut entry.name_key).changed();
            ui.end_row();

            ui.label(get!("weapon_editor.fields.model"));
            if let Some(pick) = prop_button(ui, entry.model.as_deref(), &get!("weapon_editor.model_none")) {
                changed |= pick != entry.model;
                entry.model = pick;
            }
            ui.end_row();

            ui.label(get!("weapon_editor.fields.icon"));
            let mut icon = entry.icon.clone().unwrap_or_default();
            if ui.text_edit_singleline(&mut icon).changed() {
                entry.icon = Some(icon).filter(|icon| !icon.is_empty());
                changed = true;
            }
            ui.end_row();

            ui.label(get!("weapon_editor.fields.bucket"));
            changed |= ui
                .add(egui::DragValue::new(&mut entry.bucket).range(1..=BUCKETS).clamp_existing_to_range(false))
                .changed();
            ui.end_row();

            ui.label(get!("weapon_editor.fields.deploy"));
            changed |= ui
                .add(
                    egui::DragValue::new(&mut entry.deploy)
                        .range(0.0..=f32::INFINITY)
                        .clamp_existing_to_range(false)
                        .speed(0.01)
                        .suffix(get!(unit_key(Unit::Seconds))),
                )
                .changed();
            ui.end_row();
        });
        self.model_ui(ui);

        let weapon = &mut self.editor.weapons[index];
        // Per weapon, so a kind tried on one is not what another switches back to.
        let root = format!("{}/{}", weapon.pack.display(), weapon.key);
        let form = &mut self.form;
        egui::CollapsingHeader::new(get!("weapon_editor.magazine")).default_open(true).show(ui, |ui| {
            changed |= edit_struct(ui, &mut weapon.entry.magazine, &format!("{root}.magazine"), form);
        });
        for (key, path, mounted) in [
            ("weapon_editor.primary", "primary", &mut weapon.entry.primary),
            ("weapon_editor.secondary", "secondary", &mut weapon.entry.secondary),
        ] {
            egui::CollapsingHeader::new(get!(key)).default_open(true).show(ui, |ui| {
                changed |= edit_struct(ui, mounted, &format!("{root}.{path}"), form);
                drawings(ui, mounted);
            });
        }
        let entry = &mut weapon.entry;
        egui::CollapsingHeader::new(get!("weapon_editor.scoped_primary")).default_open(false).show(ui, |ui| {
            let mut has = entry.scoped_primary.is_some();
            if ui.checkbox(&mut has, get!("weapon_editor.has_scoped_primary")).changed() {
                // Starts as the primary, so turning it on changes nothing until edited.
                entry.scoped_primary = has.then_some(entry.primary);
                changed = true;
            }
            if let Some(mounted) = entry.scoped_primary.as_mut() {
                changed |= edit_struct(ui, mounted, &format!("{root}.scoped_primary"), form);
                drawings(ui, mounted);
            }
        });
        self.changed |= changed;
    }

    fn read_the_model(&mut self, name: Option<&str>) {
        if self.browser.model.as_ref().map(|(read, _)| read.as_str()) == name {
            return;
        }
        self.browser.model = name.map(|name| (name.to_owned(), read_prop(self.assets, name)));
    }

    fn model_ui(&mut self, ui: &mut Ui) {
        let Some((name, read)) = &self.browser.model else { return };
        let name = name.clone();
        let Some((path, doc)) = read else {
            ui.colored_label(ui.visuals().warn_fg_color, get!("weapon_editor.model_missing"));
            return;
        };
        egui::CollapsingHeader::new(get!("weapon_editor.model")).default_open(false).show(ui, |ui| {
            ui.label(match doc.muzzle.is_some() {
                true => get!("weapon_editor.muzzle.yes"),
                false => get!("weapon_editor.muzzle.no"),
            });
            egui::Grid::new("weapon_clips").num_columns(2).show(ui, |ui| {
                for (clip, shared) in CLIPS {
                    ui.label(get!(format!("weapon_editor.clips.{clip}")));
                    ui.label(match (doc.animations.get(clip).is_some(), shared) {
                        (true, _) => get!("weapon_editor.clip_state.own"),
                        (false, true) => get!("weapon_editor.clip_state.shared"),
                        (false, false) => get!("weapon_editor.clip_state.none"),
                    });
                    ui.end_row();
                }
            });
            // Opening another prop would throw away the prop editor's unsaved edits.
            let blocked = self.prop_editor.dirty && self.prop_editor.path.as_deref() != Some(path.as_path());
            ui.horizontal(|ui| {
                for (key, mode) in
                    [("weapon_editor.open_in_prop", AppMode::Prop), ("weapon_editor.open_in_animation", AppMode::Animation)]
                {
                    let button = ui.add_enabled(!blocked, egui::Button::new(get!(key)));
                    if button.clicked() {
                        if self.prop_editor.path.as_deref() == Some(path.as_path()) {
                            self.open_mode = Some(mode);
                        } else if let Some((path, fresh)) = read_prop(self.assets, &name) {
                            // Read again: the summary's copy may predate a save in the prop editor.
                            self.prop_editor.open(fresh, Some(path));
                            self.open_mode = Some(mode);
                        }
                    }
                    button.on_disabled_hover_text(get!("weapon_editor.prop_unsaved"));
                }
            });
        });
    }
}

fn read_prop(assets: &Assets, name: &str) -> Option<(PathBuf, PropDoc)> {
    let path = source_of(name)?;
    let pack = path.parent()?.parent()?;
    match load(assets, pack, name) {
        Ok(doc) => Some((path, doc)),
        Err(e) => {
            warn!("Could not read the prop {name} for the weapon editor, {}/{e}", pack.display());
            None
        }
    }
}

const SPEEDS: [f32; 4] = [0.1, 0.25, 0.5, 1.0];

fn strip(ui: &mut Ui, show: &mut RangeShow) {
    ui.horizontal(|ui| {
        let play = match show.playing {
            true => get!("weapon_editor.range.pause"),
            false => get!("weapon_editor.range.play"),
        };
        if ui.button(play).clicked() {
            show.playing = !show.playing;
        }
        if ui.button(get!("weapon_editor.range.restart")).clicked() {
            show.restart = true;
            show.playing = true;
        }
        if ui.button(get!("weapon_editor.range.previous")).clicked() {
            show.step = -1;
            show.playing = true;
        }
        if ui.button(get!("weapon_editor.range.next")).clicked() {
            show.step = 1;
            show.playing = true;
        }
        ui.separator();
        ui.label(get!("weapon_editor.range.speed"));
        for speed in SPEEDS {
            let times = get!(unit_key(Unit::Times));
            if ui.selectable_label(show.speed == speed, format!("{speed}{times}")).clicked() {
                show.speed = speed;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        let current = show.current;
        let labels: Vec<String> = show.beats.iter().map(|beat| beat.label()).collect();
        for (index, label) in labels.into_iter().enumerate() {
            let mut on = !show.off.get(index).copied().unwrap_or(false);
            let text = match index == current {
                true => egui::RichText::new(label).strong(),
                false => egui::RichText::new(label),
            };
            if ui.checkbox(&mut on, text).changed()
                && let Some(off) = show.off.get_mut(index)
            {
                *off = !on;
            }
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
        for line in &show.log {
            ui.label(egui::RichText::new(line).monospace());
        }
    });
}

fn drawings(ui: &mut Ui, mounted: &Mounted) {
    damage_over_distance(ui, &mounted.action);
    if let WeaponAction::Hitscan(spec) = &mounted.action
        && let Some(grid) = &spec.pellets
    {
        pellet_pattern(ui, grid);
    }
}

/// Chords go through the menus' handlers, so a chord and its menu entry are one action.
fn weapon_shortcuts(
    actions: Res<crate::common::keybind::Actions>,
    mut editor: ResMut<WeaponEditor>,
    mut panels: ResMut<WeaponPanels>,
    mut published: ResMut<PublishedCatalog>,
    assets: Res<Assets>,
    dialog: Res<WeaponFileDialog>,
    mut recent: ResMut<RecentFiles>,
) {
    let chords = chords(&actions);
    handle_edit(
        match (chords.redo, chords.undo) {
            (true, _) => Some(EditAction::Redo),
            (false, true) => Some(EditAction::Undo),
            _ => None,
        },
        &mut editor,
        &assets,
    );
    handle_files(FileAction::from_chords(chords), &mut editor, &mut panels.browser, &mut published, &assets, &dialog, &mut recent);
}

fn handle_edit(action: Option<EditAction>, editor: &mut WeaponEditor, assets: &Assets) {
    match action {
        Some(EditAction::Undo) => editor.undo(),
        Some(EditAction::Redo) => editor.redo(),
        Some(EditAction::Revert) => {
            editor.revert_to(read_pack_weapons(assets, &default_packs()));
            info!("Reverted the weapons to what the packs hold");
        }
        None => {}
    }
}
