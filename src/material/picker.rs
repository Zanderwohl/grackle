//! Choosing a room slot's material: a button naming it, which opens a searchable grid of every
//! pack's materials as previews, and the palette's roles beside them.
//!
//! A feature's `editor_ui` is handed egui and nothing else, so [`publish_choices`] leaves the
//! choices in the egui context's memory and [`material_button`] reads them from there. Choices
//! are only gathered and baked while a picker is open, which the button says the same way.

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiTextureHandle};

use super::gpu::{BakedTextures, BevyDevice};
use super::library::MaterialLibrary;
use super::sky::Sky;
use crate::common::sky::SKYBOX;
use super::{FILE_EXTENSION, MATERIALS_DIR};
use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::common::palette::Palette;
use crate::get;

const COLUMNS: usize = 4;
const TILE: f32 = 64.0;
const VISIBLE_ROWS: f32 = 4.5;
/// So the button keeps its width whatever it names.
const BUTTON_CHARS: f32 = 20.0;

#[derive(Clone, Default)]
struct Choices(Vec<Choice>);

#[derive(Clone)]
struct Choice {
    name: String,
    preview: Option<egui::TextureId>,
    role: bool,
}

fn choices_id() -> egui::Id {
    egui::Id::new("material_picker_choices")
}

fn open_id() -> egui::Id {
    egui::Id::new("material_picker_open")
}

/// A button naming `current`, `None` being the default material. True when a choice was made. A role resolves
/// only to a material, so the palette's own pickers leave roles out.
pub fn material_button(ui: &mut egui::Ui, current: &mut Option<String>, offer_roles: bool) -> bool {
    let plain = get!("editor.material_picker.plain");
    let font = egui::TextStyle::Button.resolve(ui.style());
    let width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'n')) * BUTTON_CHARS
        + 2.0 * ui.spacing().button_padding.x;
    let label = current.as_deref().unwrap_or(&plain);
    let mut response =
        ui.add_sized([width, ui.spacing().interact_size.y], egui::Button::new(label).truncate());
    if let Some(name) = current.as_deref() {
        response = response.on_hover_text(name);
    }

    let search_id = response.id.with("search");
    if response.clicked() {
        ui.data_mut(|data| data.remove_temp::<String>(search_id));
    }
    let mut chosen = None;
    egui::Popup::from_toggle_button_response(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.data_mut(|data| data.insert_temp(open_id(), true));
            let choices = ui.data(|data| data.get_temp::<Choices>(choices_id())).unwrap_or_default();
            let mut search = ui.data_mut(|data| data.get_temp::<String>(search_id)).unwrap_or_default();
            // Before the search field, which would otherwise stretch the popup to as wide as it may be.
            let width = COLUMNS as f32 * (TILE + ui.spacing().item_spacing.x) + ui.spacing().scroll.bar_width;
            ui.set_width(width);
            let field = ui.add(
                egui::TextEdit::singleline(&mut search)
                    .hint_text(get!("editor.material_picker.search"))
                    .desired_width(width),
            );
            if response.clicked() {
                field.request_focus();
            }
            ui.data_mut(|data| data.insert_temp(search_id, search.clone()));

            let needle = search.trim().to_lowercase();
            let shown: Vec<&Choice> =
                choices.0.iter()
                    .filter(|choice| offer_roles || !choice.role)
                    .filter(|choice| choice.name.to_lowercase().contains(&needle))
                    .collect();
            egui::ScrollArea::vertical()
                .max_height(VISIBLE_ROWS * (TILE + ui.spacing().item_spacing.y))
                .show(ui, |ui| {
                    // Plain first and never filtered: it is always a choice.
                    let tiles = std::iter::once(None).chain(shown.iter().map(|choice| Some(*choice)));
                    let tiles: Vec<Option<&Choice>> = tiles.collect();
                    for row in tiles.chunks(COLUMNS) {
                        ui.horizontal(|ui| {
                            for tile in row {
                                if let Some(pick) = tile_ui(ui, *tile, current.as_deref(), &plain) {
                                    chosen = Some(pick);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
        });

    let Some(pick) = chosen else { return false };
    let changed = *current != pick;
    *current = pick;
    changed
}

/// `Some` when clicked: what the slot should now name.
fn tile_ui(ui: &mut egui::Ui, choice: Option<&Choice>, current: Option<&str>, plain: &str) -> Option<Option<String>> {
    let size = egui::Vec2::splat(TILE);
    let selected = choice.map(|choice| choice.name.as_str()) == current;
    let button = match choice.and_then(|choice| choice.preview) {
        Some(texture) => egui::Button::image(egui::Image::new((texture, size - ui.spacing().button_padding * 2.0))),
        None => egui::Button::new(choice.map_or(plain, |choice| choice.name.as_str())).truncate(),
    };
    let response = ui.add_sized(size, button.selected(selected));
    if choice.is_some_and(|choice| choice.role) {
        let badge = get!("editor.material_picker.role");
        let painter = ui.painter_at(response.rect);
        let text = painter.layout_no_wrap(badge, egui::FontId::proportional(10.0), egui::Color32::WHITE);
        let rect = egui::Rect::from_min_size(response.rect.left_bottom() - egui::vec2(0.0, text.size().y + 2.0), text.size() + egui::vec2(4.0, 2.0));
        painter.rect_filled(rect, 2.0, egui::Color32::from_black_alpha(180));
        painter.galley(rect.min + egui::vec2(2.0, 1.0), text, egui::Color32::WHITE);
    }
    let tooltip = match choice {
        Some(choice) if choice.role => get!("editor.material_picker.role_named", "name", &choice.name),
        Some(choice) => choice.name.clone(),
        None => plain.to_owned(),
    };
    response.on_hover_text(tooltip).clicked().then(|| choice.map(|choice| choice.name.clone()))
}

/// Every pack's materials by file stem. Read from disk on the authoring side, as a pack offers no
/// listing of its own.
fn every_material() -> Vec<String> {
    let mut names = BTreeSet::new();
    for pack in default_packs() {
        let Ok(entries) = std::fs::read_dir(pack.join(MATERIALS_DIR)) else { continue };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|e| e.to_str()) == Some(FILE_EXTENSION)
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                names.insert(stem.to_owned());
            }
        }
    }
    names.into_iter().collect()
}

#[derive(Default)]
pub(super) struct PickerState {
    open: bool,
    materials: Vec<String>,
    /// So a material baked again after a save frees the preview it had.
    registered: HashMap<String, AssetId<Image>>,
}

/// The directory is read again each time a picker opens, so a material saved since is there.
/// Bakes at most one material a frame, so opening onto a big pack does not stall.
#[allow(clippy::too_many_arguments)]
pub(super) fn publish_choices(
    mut contexts: EguiContexts,
    mut state: Local<PickerState>,
    mut library: ResMut<MaterialLibrary>,
    mut baked: ResMut<BakedTextures>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    packs: Res<PackAssets>,
    device: BevyDevice,
    palette: Res<Palette>,
    sky: Res<Sky>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let open = ctx.data_mut(|data| data.remove_temp::<bool>(open_id())).unwrap_or(false);
    if !open {
        state.open = false;
        return;
    }
    if !state.open {
        state.open = true;
        state.materials = every_material();
    }

    if let Some(name) = state.materials.iter().find(|name| !library.has_looked_for(name)) {
        library.ensure(name, &packs, &device, &mut baked, &mut standard);
    }

    let state = &mut *state;
    let mut previews = HashMap::new();
    for name in &state.materials {
        let Some(entry) = library.get(name) else { continue };
        let asset = entry.color_map();
        if let Some(old) = state.registered.insert(name.clone(), asset)
            && old != asset
        {
            contexts.remove_image(old);
        }
        previews.insert(name.as_str(), contexts.add_image(EguiTextureHandle::Weak(asset)));
    }

    let mut choices: Vec<Choice> = palette
        .every_role()
        .map(|(name, paint)| Choice {
            name: name.clone(),
            preview: paint
                .unsided
                .iter()
                .chain(paint.sides.values())
                .find_map(|material| previews.get(material.as_str()).copied()),
            role: true,
        })
        .collect();
    choices.push(Choice {
        name: SKYBOX.to_owned(),
        preview: Some(contexts.add_image(EguiTextureHandle::Weak(sky.preview.id()))),
        role: false,
    });
    choices.extend(state.materials.iter().filter(|name| *name != SKYBOX).map(|name| Choice {
        name: name.clone(),
        preview: previews.get(name.as_str()).copied(),
        role: false,
    }));

    let Ok(ctx) = contexts.ctx_mut() else { return };
    ctx.data_mut(|data| data.insert_temp(choices_id(), Choices(choices)));
}
