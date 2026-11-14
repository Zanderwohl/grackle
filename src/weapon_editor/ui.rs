//! The weapon editor's window: the menu bar, the Weapons browser on the left and the selected
//! weapon on the right, each its own `DockState` as the prop editor's are, and the range's strip
//! under the middle, which [`super::range`]'s cameras draw into.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy_egui::egui::{self, Id, LayerId, Ui, UiBuilder};
use egui_dock::{DockArea, DockState, TabViewer};

use super::form::{edit_struct, unit_key, Form, FormMemory};
use super::preview::{damage_over_distance, pellet_pattern};
use super::range::RangeShow;
use super::{save_the_draft, KeyProblem, PublishedCatalog, WeaponEditor};
use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::assets::{default_packs, Assets};
use crate::common::class::Class;
use crate::common::edit_menu::{edit_menu, EditAction, EditAvailability};
use crate::common::file_menu::{file_menu, FileAction};
use crate::common::net::NetRole;
use crate::common::shortcuts::chords;
use crate::common::weapon::{Mounted, Unit, WeaponAction, BUCKETS};
use crate::common::weapon_file::read_pack_weapons;
use crate::editor::editable::FeatureTimeline;
use crate::get;
use crate::prop::document::{load, PropDoc, PropEditor};
use crate::prop::picker::{prop_button, source_of, PropPreviews};

pub struct WeaponUiPlugin;

impl Plugin for WeaponUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WeaponPanels>()
            // A prop saved in another mode is read again.
            .add_systems(OnEnter(AppMode::Weapon), |mut panels: ResMut<WeaponPanels>| panels.browser.model = None)
            .add_systems(bevy_egui::EguiPrimaryContextPass, panels.run_if(in_state(AppMode::Weapon)))
            .add_systems(Update, weapon_shortcuts.run_if(in_state(AppMode::Weapon)));
    }
}

/// The catalog is every pack's file at once, so there is no New, Open or Save As: a save writes
/// each weapon back to the file it came from.
const FILE_ACTIONS: &[FileAction] = &[FileAction::Save, FileAction::Quit];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Naming {
    New,
    Duplicate,
    Rename,
}

/// What the browser is in the middle of. View state: not saved, not undone.
#[derive(Default)]
struct Browser {
    /// Index into [`WeaponEditor::packs`]; `None` is the last, the user pack.
    pack: Option<usize>,
    naming: Option<(Naming, String)>,
    rename_loadouts: bool,
    deleting: bool,
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
    mut memory: ResMut<FormMemory>,
    mut prop_editor: ResMut<PropEditor>,
    mut show: ResMut<RangeShow>,
    (assets, registry, timeline): (Res<Assets>, Res<AppTypeRegistry>, Res<FeatureTimeline>),
    mut next_mode: ResMut<NextState<AppMode>>,
    // Optional: a build with no `NetPlugin` is a closed game, its own authority.
    role: Option<Res<NetRole>>,
    mut exit: MessageWriter<AppExit>,
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
            file_menu(ui, FILE_ACTIONS, &mut requests.file);
            edit_menu(ui, edit_availability(&editor), &mut requests.edit);
            // This system is gated on `AppMode::Weapon`.
            mode_menu(ui, AppMode::Weapon, role.as_deref(), &mut requests.mode);
            ui.separator();
            ui.label(crate::common::weapon_file::WEAPONS_FILE);
            if editor.dirty() {
                ui.label("*");
            }
        });
    });

    let registry = registry.read();
    let packs = editor.packs();
    // Only asked while a rename or delete is open.
    let asking = panels.browser.deleting || matches!(panels.browser.naming, Some((Naming::Rename, _)));
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
    match requests.file {
        Some(FileAction::Save) => save_the_draft(&mut editor, &assets, &mut published),
        Some(FileAction::Quit) => {
            info!("Quitting, as asked");
            exit.write(AppExit::Success);
        }
        _ => {}
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

fn pack_name(pack: &Path) -> String {
    pack.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Tabs<'_, '_, '_> {
    fn target_pack(&self) -> PathBuf {
        let last = self.packs.len().saturating_sub(1);
        self.packs[self.browser.pack.unwrap_or(last).min(last)].clone()
    }

    fn browser_ui(&mut self, ui: &mut Ui) {
        let selected = self.editor.selected.filter(|index| *index < self.editor.weapons.len());
        ui.horizontal_wrapped(|ui| {
            if ui.button(get!("weapon_editor.browser.new")).clicked() {
                self.browser.naming = Some((Naming::New, String::new()));
                self.browser.deleting = false;
            }
            ui.add_enabled_ui(selected.is_some(), |ui| {
                if ui.button(get!("weapon_editor.browser.duplicate")).clicked() {
                    self.browser.naming = Some((Naming::Duplicate, String::new()));
                    self.browser.deleting = false;
                }
                if ui.button(get!("weapon_editor.browser.override")).clicked()
                    && let Some(index) = selected
                    && self.editor.override_in(index, self.target_pack()).is_none()
                {
                    warn!(
                        "{} cannot be overridden in {}",
                        self.editor.weapons[index].key,
                        pack_name(&self.target_pack())
                    );
                }
                if ui.button(get!("weapon_editor.browser.rename")).clicked()
                    && let Some(index) = selected
                {
                    self.browser.naming = Some((Naming::Rename, self.editor.weapons[index].key.clone()));
                    self.browser.deleting = false;
                }
                if ui.button(get!("weapon_editor.browser.delete")).clicked() {
                    self.browser.deleting = true;
                    self.browser.naming = None;
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label(get!("weapon_editor.browser.pack"));
            let target = pack_name(&self.target_pack());
            egui::ComboBox::from_id_salt("weapon_target_pack").selected_text(target).show_ui(ui, |ui| {
                for (index, pack) in self.packs.iter().enumerate() {
                    let chosen = self.browser.pack.unwrap_or(self.packs.len() - 1) == index;
                    if ui.selectable_label(chosen, pack_name(pack)).clicked() {
                        self.browser.pack = Some(index);
                    }
                }
            });
        });

        if let Some(naming) = self.browser.naming.clone() {
            self.naming_ui(ui, naming, selected);
        } else if self.browser.deleting
            && let Some(index) = selected
        {
            self.delete_ui(ui, index);
        }
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| self.list_ui(ui));
    }

    fn naming_ui(&mut self, ui: &mut Ui, (naming, mut text): (Naming, String), selected: Option<usize>) {
        let except = if naming == Naming::Rename { selected } else { None };
        let problem = self.editor.key_problem(&text, except);
        let mut done = false;
        let mut cancelled = false;
        ui.horizontal(|ui| {
            ui.label(get!("weapon_editor.fields.key"));
            let field = ui.text_edit_singleline(&mut text);
            done = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            done |= ui.add_enabled(problem.is_none(), egui::Button::new(get!("weapon_editor.browser.ok"))).clicked();
            cancelled = ui.button(get!("weapon_editor.browser.cancel")).clicked();
        });
        if let Some(problem) = problem.filter(|_| !text.is_empty()) {
            let why = match problem {
                KeyProblem::Malformed => get!("weapon_editor.key.malformed"),
                KeyProblem::Taken => get!("weapon_editor.key.taken"),
            };
            ui.colored_label(ui.visuals().error_fg_color, why);
        }
        if naming == Naming::Rename
            && let Some(index) = selected
        {
            ui.checkbox(&mut self.browser.rename_loadouts, get!("weapon_editor.browser.rename_loadouts"));
            self.dependants_ui(ui, index);
        }
        if cancelled {
            self.browser.naming = None;
            return;
        }
        if done && problem.is_none() {
            let pack = self.target_pack();
            match (naming, selected) {
                (Naming::New, _) => {
                    self.editor.new_weapon(pack, &text);
                }
                (Naming::Duplicate, Some(index)) => {
                    self.editor.duplicate(index, pack, &text);
                }
                (Naming::Rename, Some(index)) => self.editor.rename(index, &text, self.browser.rename_loadouts),
                _ => {}
            }
            self.browser.naming = None;
            return;
        }
        self.browser.naming = Some((naming, text));
    }

    fn delete_ui(&mut self, ui: &mut Ui, index: usize) {
        self.dependants_ui(ui, index);
        ui.horizontal(|ui| {
            let key = self.editor.weapons[index].key.clone();
            if ui.button(get!("weapon_editor.browser.confirm_delete", "key", key)).clicked() {
                self.editor.delete(index);
                self.browser.deleting = false;
            }
            if ui.button(get!("weapon_editor.browser.cancel")).clicked() {
                self.browser.deleting = false;
            }
        });
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
                self.row_ui(ui, index);
            }
        }
    }

    fn row_ui(&mut self, ui: &mut Ui, index: usize) {
        let weapon = &self.editor.weapons[index];
        let picture = weapon.entry.model.as_deref().and_then(|model| self.previews.picture(model));
        let broken = self.editor.problems.iter().any(|(at, _)| *at == index);
        let mut name =
            egui::RichText::new(format!("{}  ·  {}", get!(&weapon.entry.name_key), pack_name(&weapon.pack)));
        if broken {
            name = name.color(ui.visuals().warn_fg_color);
        }
        let selected = self.editor.selected == Some(index);
        let clicked = ui
            .horizontal(|ui| {
                match picture {
                    Some(texture) => {
                        ui.image((texture, egui::vec2(THUMBNAIL, THUMBNAIL)));
                    }
                    None => {
                        ui.allocate_exact_size(egui::vec2(THUMBNAIL, THUMBNAIL), egui::Sense::hover());
                    }
                }
                ui.selectable_label(selected, name).clicked()
            })
            .inner;
        if clicked {
            self.editor.selected = Some(index);
            self.browser.deleting = false;
            if self.browser.naming.as_ref().is_some_and(|(naming, _)| *naming != Naming::New) {
                self.browser.naming = None;
            }
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
            error!("{e}");
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
    keys: Res<ButtonInput<KeyCode>>,
    egui: Res<bevy_egui::input::EguiWantsInput>,
    mut editor: ResMut<WeaponEditor>,
    mut published: ResMut<PublishedCatalog>,
    assets: Res<Assets>,
) {
    let chords = chords(&keys, egui.wants_keyboard_input());
    handle_edit(
        match (chords.redo, chords.undo) {
            (true, _) => Some(EditAction::Redo),
            (false, true) => Some(EditAction::Undo),
            _ => None,
        },
        &mut editor,
        &assets,
    );
    // Save As has nowhere else to save to, so it is a save too.
    if chords.save || chords.save_as {
        save_the_draft(&mut editor, &assets, &mut published);
    }
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
