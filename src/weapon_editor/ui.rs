//! The weapon editor's window: the menu bar, the weapons down the left and the selected weapon's
//! settings on the right. The generic reflected form and the range are later stages of
//! [documentation/weapon-editor.md](../../documentation/weapon-editor.md).

use bevy::prelude::*;
use bevy_egui::egui::{self, Id, LayerId, Ui, UiBuilder, UiKind};
use bevy_egui::EguiContexts;

use super::{save_the_draft, PublishedCatalog, WeaponEditor};
use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::assets::{default_packs, Assets};
use crate::common::edit_menu::{edit_menu, EditAction, EditAvailability};
use crate::common::file_menu::FileAction;
use crate::common::net::NetRole;
use crate::common::shortcuts::chords;
use crate::common::weapon::{Magazine, Mounted, WeaponAction, BUCKETS};
use crate::common::weapon_file::{problems, read_pack_weapons, WeaponEntry};
use crate::get;

pub struct WeaponUiPlugin;

impl Plugin for WeaponUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(bevy_egui::EguiPrimaryContextPass, panels.run_if(in_state(AppMode::Weapon)))
            .add_systems(Update, weapon_shortcuts.run_if(in_state(AppMode::Weapon)));
    }
}

/// The catalog is every pack's file at once, so there is no New, Open or Save As: a save writes
/// each weapon back to the file it came from.
const FILE_ACTIONS: &[FileAction] = &[FileAction::Save, FileAction::Quit];

#[derive(Default)]
struct Requests {
    file: Option<FileAction>,
    edit: Option<EditAction>,
    mode: Option<AppMode>,
}

fn edit_availability(editor: &WeaponEditor) -> EditAvailability {
    EditAvailability { undo: editor.can_undo(), redo: editor.can_redo(), revert: editor.dirty() }
}

fn panels(
    mut contexts: EguiContexts,
    mut editor: ResMut<WeaponEditor>,
    mut published: ResMut<PublishedCatalog>,
    assets: Res<Assets>,
    mut next_mode: ResMut<NextState<AppMode>>,
    // Optional: a build with no `NetPlugin` is a closed game, its own authority.
    role: Option<Res<NetRole>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();
    let mut window_ui = Ui::new(
        ctx.clone(),
        Id::new("grackle_weapon_window"),
        UiBuilder::new().layer_id(LayerId::background()).max_rect(ctx.viewport_rect()),
    );

    let mut requests = Requests::default();
    egui::Panel::top("weapon_menu_bar").resizable(false).show(&mut window_ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(get!("file.menu.title"), |ui| {
                for action in FILE_ACTIONS {
                    let mut button = egui::Button::new(get!(action.name_key()));
                    if let Some(chord) = action.chord() {
                        button = button.shortcut_text(chord.hint());
                    }
                    if ui.add(button).clicked() {
                        ui.close_kind(UiKind::Menu);
                        requests.file = Some(*action);
                    }
                }
            });
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

    let mut changed = false;
    egui::Panel::left("weapon_list").resizable(true).show(&mut window_ui, |ui| {
        weapon_list(ui, &mut editor);
    });
    egui::CentralPanel::default().show(&mut window_ui, |ui| {
        changed = settings(ui, &mut editor);
    });
    if changed {
        editor.edited();
    }
    editor.checkpoint_when_idle(!ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input());

    handle_edit(requests.edit, &mut editor, &assets);
    // `mode_menu` never offers the current mode, so this is always a change.
    if let Some(mode) = requests.mode {
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

/// By bucket, as the selector lays them out.
fn weapon_list(ui: &mut Ui, editor: &mut WeaponEditor) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        for bucket in 1..=BUCKETS {
            let mut in_bucket = editor
                .weapons
                .iter()
                .enumerate()
                .filter(|(_, weapon)| weapon.entry.bucket == bucket)
                .peekable();
            if in_bucket.peek().is_none() {
                continue;
            }
            ui.label(egui::RichText::new(bucket.to_string()).strong());
            let rows: Vec<(usize, String, String)> = in_bucket
                .map(|(index, weapon)| (index, get!(&weapon.entry.name_key), pack_name(&weapon.pack)))
                .collect();
            for (index, name, pack) in rows {
                let selected = editor.selected == Some(index);
                let row = ui.selectable_label(selected, format!("{name}  ·  {pack}"));
                if row.clicked() {
                    editor.selected = Some(index);
                }
            }
        }
        // A bucket no key selects still has to be reachable to be fixed.
        let stray: Vec<(usize, String)> = editor
            .weapons
            .iter()
            .enumerate()
            .filter(|(_, weapon)| !(1..=BUCKETS).contains(&weapon.entry.bucket))
            .map(|(index, weapon)| (index, weapon.key.clone()))
            .collect();
        for (index, key) in stray {
            if ui.selectable_label(editor.selected == Some(index), key).clicked() {
                editor.selected = Some(index);
            }
        }
    });
}

fn pack_name(pack: &std::path::Path) -> String {
    pack.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Whether anything changed.
fn settings(ui: &mut Ui, editor: &mut WeaponEditor) -> bool {
    let problems = problems(&editor.weapons);
    let Some(index) = editor.selected.filter(|index| *index < editor.weapons.len()) else {
        return false;
    };
    let weapon = &mut editor.weapons[index];
    let mut changed = false;

    ui.heading(get!(&weapon.entry.name_key));
    ui.label(format!("{}  ·  {}", weapon.key, weapon.pack.display()));
    ui.separator();

    egui::Grid::new("weapon_settings").num_columns(2).show(ui, |ui| {
        changed |= entry_rows(ui, &mut weapon.entry);
    });

    ui.separator();
    for (label, mounted) in [
        (get!("weapon_editor.primary"), &weapon.entry.primary),
        (get!("weapon_editor.secondary"), &weapon.entry.secondary),
    ] {
        ui.label(format!("{label}: {}", action_kind(mounted)));
    }

    for (_, why) in problems.iter().filter(|(at, _)| *at == index) {
        ui.colored_label(ui.visuals().error_fg_color, why);
    }
    changed
}

fn entry_rows(ui: &mut Ui, entry: &mut WeaponEntry) -> bool {
    let mut changed = false;

    ui.label(get!("weapon_editor.fields.name_key"));
    changed |= ui.text_edit_singleline(&mut entry.name_key).changed();
    ui.end_row();

    ui.label(get!("weapon_editor.fields.model"));
    let mut model = entry.model.clone().unwrap_or_default();
    if ui.text_edit_singleline(&mut model).changed() {
        entry.model = (!model.is_empty()).then_some(model);
        changed = true;
    }
    ui.end_row();

    ui.label(get!("weapon_editor.fields.bucket"));
    changed |= ui.add(egui::DragValue::new(&mut entry.bucket).range(1..=BUCKETS)).changed();
    ui.end_row();

    ui.label(get!("weapon_editor.fields.deploy"));
    changed |= ui.add(egui::DragValue::new(&mut entry.deploy).range(0.0..=10.0).speed(0.01).suffix(" s")).changed();
    ui.end_row();

    changed |= magazine_rows(ui, &mut entry.magazine);
    changed
}

fn magazine_rows(ui: &mut Ui, magazine: &mut Magazine) -> bool {
    let mut changed = false;

    ui.label(get!("weapon_editor.fields.clip"));
    ui.horizontal(|ui| {
        let mut has_clip = magazine.clip.is_some();
        if ui.checkbox(&mut has_clip, "").changed() {
            magazine.clip = has_clip.then_some(magazine.clip.unwrap_or(1));
            changed = true;
        }
        if let Some(clip) = &mut magazine.clip {
            changed |= ui.add(egui::DragValue::new(clip).range(1..=999)).changed();
        }
    });
    ui.end_row();

    ui.label(get!("weapon_editor.fields.reserve"));
    changed |= ui.add(egui::DragValue::new(&mut magazine.reserve).range(0..=9999)).changed();
    ui.end_row();

    ui.label(get!("weapon_editor.fields.reload"));
    changed |= ui.add(egui::DragValue::new(&mut magazine.reload).range(0.0..=10.0).speed(0.01).suffix(" s")).changed();
    ui.end_row();

    changed
}

/// The serde tag, which is what the file says.
fn action_kind(mounted: &Mounted) -> &'static str {
    match mounted.action {
        WeaponAction::Hitscan(_) => "hitscan",
        WeaponAction::Projectile(_) => "projectile",
        WeaponAction::Flame(_) => "flame",
        WeaponAction::Airblast(_) => "airblast",
        WeaponAction::HealBeam(_) => "heal_beam",
        WeaponAction::Invulnerable(_) => "invulnerable",
        WeaponAction::Scope(_) => "scope",
        WeaponAction::None => "none",
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
