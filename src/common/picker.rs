//! Choosing something by name from a pack: a button naming it, which opens a searchable grid of
//! previews ([`preview_grid`]). Materials, props and weapons are each a [`PickerKind`].
//!
//! A feature's `editor_ui` is handed egui and nothing else, so a system publishes each kind's
//! choices into the egui context's memory ([`PickerKind::publish`]) and [`picker_button`] reads
//! them from there. An open picker says so ([`PickerKind::take_open`]), so a publisher can leave
//! reading directories and baking previews until somebody is looking.

use std::collections::BTreeSet;

use bevy_egui::egui;

use crate::common::assets::default_packs;
use crate::common::preview_grid::{self, Face, Tile};
use crate::get;

const VISIBLE_ROWS: f32 = 4.5;
/// So the button keeps its width whatever it names.
const BUTTON_CHARS: f32 = 20.0;

#[derive(Clone, Copy)]
pub struct PickerKind {
    name: &'static str,
    columns: usize,
    /// Points, width then height.
    tile: [f32; 2],
    /// Each tile's label written under it.
    captioned: bool,
}

impl PickerKind {
    /// Four square tiles to a row.
    pub const fn new(name: &'static str) -> Self {
        Self { name, columns: 4, tile: [64.0, 64.0], captioned: false }
    }

    pub const fn captioned(name: &'static str, columns: usize, tile: [f32; 2]) -> Self {
        Self { name, columns, tile, captioned: true }
    }

    fn tile_size(self) -> egui::Vec2 {
        egui::vec2(self.tile[0], self.tile[1])
    }

    fn choices_id(self) -> egui::Id {
        egui::Id::new(("picker_choices", self.name))
    }

    fn open_id(self) -> egui::Id {
        egui::Id::new(("picker_open", self.name))
    }

    pub fn take_open(self, ctx: &egui::Context) -> bool {
        ctx.data_mut(|data| data.remove_temp::<bool>(self.open_id())).unwrap_or(false)
    }

    pub fn publish(self, ctx: &egui::Context, choices: Vec<Choice>) {
        ctx.data_mut(|data| data.insert_temp(self.choices_id(), Choices(choices)));
    }
}

#[derive(Clone, Default)]
struct Choices(Vec<Choice>);

#[derive(Clone)]
pub struct Choice {
    /// What is stored.
    pub name: String,
    /// What is shown, on the button and as a tile with no preview.
    pub label: String,
    pub preview: Option<egui::TextureId>,
    pub badge: Option<String>,
    /// `None` is the label.
    pub tooltip: Option<String>,
    /// Drawn white on gray when there is no preview; `None` is the "[No preview]" stand-in.
    pub placeholder: Option<String>,
}

impl Choice {
    pub fn named(name: impl Into<String>) -> Self {
        let name = name.into();
        Self { label: name.clone(), name, preview: None, badge: None, tooltip: None, placeholder: None }
    }
}

/// `plain` labels the choice of nothing, offered first and never filtered; without it nothing is
/// not a choice. `Some` when a choice was made, even of what `current` already was.
pub fn picker_button(
    ui: &mut egui::Ui,
    kind: PickerKind,
    current: Option<&str>,
    plain: Option<&str>,
) -> Option<Option<String>> {
    picker(ui, kind, current, plain, None)
}

/// [`picker_button`], labeling a `current` that names no choice `missing` rather than by its name,
/// for a kind whose names mean nothing to read.
pub fn picker_button_or_missing(
    ui: &mut egui::Ui,
    kind: PickerKind,
    current: Option<&str>,
    plain: Option<&str>,
    missing: &str,
) -> Option<Option<String>> {
    picker(ui, kind, current, plain, Some(missing))
}

fn picker(
    ui: &mut egui::Ui,
    kind: PickerKind,
    current: Option<&str>,
    plain: Option<&str>,
    missing: Option<&str>,
) -> Option<Option<String>> {
    let choices = ui.data(|data| data.get_temp::<Choices>(kind.choices_id())).unwrap_or_default();
    let font = egui::TextStyle::Button.resolve(ui.style());
    let width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'n')) * BUTTON_CHARS
        + 2.0 * ui.spacing().button_padding.x;
    let named = current.and_then(|name| choices.0.iter().find(|choice| choice.name == name));
    let label = match (current, named) {
        (_, Some(choice)) => &choice.label,
        (Some(name), None) => missing.unwrap_or(name),
        (None, None) => plain.unwrap_or_default(),
    };
    let mut response =
        ui.add_sized([width, ui.spacing().interact_size.y], egui::Button::new(label).truncate());
    if let Some(choice) = named {
        response = response.on_hover_text(choice.tooltip.as_ref().unwrap_or(&choice.label));
    }

    let search_id = response.id.with("search");
    if response.clicked() {
        ui.data_mut(|data| data.remove_temp::<String>(search_id));
    }
    let mut chosen = None;
    egui::Popup::from_toggle_button_response(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.data_mut(|data| data.insert_temp(kind.open_id(), true));
            let mut search = ui.data_mut(|data| data.get_temp::<String>(search_id)).unwrap_or_default();
            // Before the search field, which would otherwise stretch the popup to as wide as it may be.
            let width = kind.columns as f32 * (kind.tile[0] + ui.spacing().item_spacing.x) + ui.spacing().scroll.bar_width;
            ui.set_width(width);
            let field = ui.add(
                egui::TextEdit::singleline(&mut search)
                    .hint_text(get!("editor.picker.search"))
                    .desired_width(width),
            );
            if response.clicked() {
                field.request_focus();
            }
            ui.data_mut(|data| data.insert_temp(search_id, search.clone()));

            let needle = search.trim().to_lowercase();
            let shown = choices.0.iter().filter(|choice| {
                choice.name.to_lowercase().contains(&needle) || choice.label.to_lowercase().contains(&needle)
            });
            let tiles: Vec<Option<&Choice>> =
                plain.map(|_| None).into_iter().chain(shown.map(Some)).collect();
            let caption = if kind.captioned { ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y } else { 0.0 };
            egui::ScrollArea::vertical()
                .max_height(VISIBLE_ROWS * (kind.tile[1] + caption + ui.spacing().item_spacing.y))
                .show(ui, |ui| {
                    for row in tiles.chunks(kind.columns) {
                        ui.horizontal(|ui| {
                            for tile in row {
                                if let Some(pick) = tile_ui(ui, kind, *tile, current, plain.unwrap_or_default()) {
                                    chosen = Some(pick);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
        });
    chosen
}

/// `Some` when clicked: what should now be named.
fn tile_ui(ui: &mut egui::Ui, kind: PickerKind, choice: Option<&Choice>, current: Option<&str>, plain: &str) -> Option<Option<String>> {
    let label = choice.map_or(plain, |choice| choice.label.as_str());
    let face = match choice {
        None => Face::Text(plain),
        Some(Choice { preview: Some(texture), .. }) => Face::Picture(*texture),
        Some(Choice { placeholder: Some(text), .. }) => Face::Text(text),
        Some(_) => Face::NoPreview,
    };
    let tooltip = choice.map_or(plain, |choice| choice.tooltip.as_deref().unwrap_or(&choice.label));
    let tile = Tile {
        label,
        face,
        badge: choice.and_then(|choice| choice.badge.as_deref()),
        tooltip,
        selected: choice.map(|choice| choice.name.as_str()) == current,
    };
    preview_grid::tile(ui, kind.tile_size(), kind.captioned, &tile)
        .clicked()
        .then(|| choice.map(|choice| choice.name.clone()))
}

/// Authoring side only: read from disk, as a pack offers no listing of its own.
pub fn every_in_packs(folder: &str, extension: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for pack in default_packs() {
        let Ok(entries) = std::fs::read_dir(pack.join(folder)) else { continue };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|e| e.to_str()) == Some(extension)
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                names.insert(stem.to_owned());
            }
        }
    }
    names.into_iter().collect()
}
