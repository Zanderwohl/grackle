//! The two typefaces: JetBrains Mono for every egui surface and the
//! scoreboard, and Alfa Slab One for the rest of what a player sees during a
//! round — the HUD, the pause menu and the damage numbers.
//!
//! Both are pack content, read once at `Startup` through [`Assets`] like the
//! weapon catalog, so a browser build fetches them the same way. A font that
//! is missing leaves the built-in one in its place rather than refusing to
//! start.
//!
//! Every attributed font in the packs ([`pack_fonts`]) is also given to egui
//! as a family of its own, named by its folder, for the Settings window's Fonts
//! page and its credits.

use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::assets::{default_packs, Assets};
use crate::common::attribution::scan_packs;
use crate::common::pack_fonts::{self, PackFont, FONTS_DIR};

const UI_FONT: &str = "fonts/JetBrains_Mono/static/JetBrainsMono-Regular.ttf";
const DISPLAY_FONT: &str = "fonts/Alfa_Slab_One/AlfaSlabOne-Regular.ttf";

/// The egui family the damage numbers are drawn in. Always defined — falling
/// back to the proportional fonts — because egui panics on a family it has
/// never been told about.
pub fn display_family() -> egui::FontFamily {
    egui::FontFamily::Name("display".into())
}

/// The Bevy UI font for play-mode text. The default handle is Bevy's built-in
/// font, which is what a test app with no fonts loaded draws in.
#[derive(Resource, Default, Clone)]
pub struct DisplayFont(pub Handle<Font>);

/// JetBrains Mono for Bevy UI, where columns of numbers have to line up: the
/// scoreboard.
#[derive(Resource, Default, Clone)]
pub struct MonoFont(pub Handle<Font>);

/// The packs' attributed fonts, sorted by name.
#[derive(Resource, Default, Clone)]
pub struct PackFonts {
    pub fonts: Vec<PackFont>,
}

/// Built at `Startup`, handed to egui on its first pass and then removed.
#[derive(Resource)]
struct PendingEguiFonts(egui::FontDefinitions);

pub struct FontsPlugin;

impl Plugin for FontsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Assets>()
            .init_resource::<DisplayFont>()
            .init_resource::<MonoFont>()
            .init_resource::<PackFonts>()
            .add_systems(Startup, load_fonts)
            .add_systems(
                EguiPrimaryContextPass,
                install_egui_fonts.run_if(resource_exists::<PendingEguiFonts>),
            );
    }
}

fn read_from_packs(assets: &Assets, relative: &str) -> Option<Vec<u8>> {
    let found = assets.read_from_packs(relative);
    if found.is_none() {
        warn!("No pack has {relative}; using the built-in font");
    }
    found
}

fn load_fonts(
    mut commands: Commands,
    assets: Res<Assets>,
    mut fonts: ResMut<bevy::asset::Assets<Font>>,
) {
    let ui = read_from_packs(&assets, UI_FONT);
    let display = read_from_packs(&assets, DISPLAY_FONT);
    let (found, warnings) = pack_fonts::discover(&scan_packs(&default_packs(), FONTS_DIR), &*assets);
    for warning in warnings {
        warn!("{warning}");
    }

    if let Some(bytes) = &display {
        commands.insert_resource(DisplayFont(fonts.add(Font::from_bytes(bytes.clone()))));
    }
    if let Some(bytes) = &ui {
        commands.insert_resource(MonoFont(fonts.add(Font::from_bytes(bytes.clone()))));
    }
    let mut definitions = egui_fonts(ui, display);
    for font in &found {
        let read = assets.read(&font.face_pack, &font.face_path()).map_err(|e| e.to_string());
        if let Err(problem) = read.and_then(|bytes| add_pack_font(&mut definitions, &font.folder, bytes)) {
            warn!("Could not read font {}: {problem}", font.face_path());
        }
    }
    commands.insert_resource(PackFonts { fonts: found });
    commands.insert_resource(PendingEguiFonts(definitions));
}

/// egui panics on a face it cannot parse, so the bytes are parsed here first,
/// as epaint will. egui's own fonts stay behind it for the glyphs it lacks.
fn add_pack_font(definitions: &mut egui::FontDefinitions, folder: &str, bytes: Vec<u8>) -> Result<(), String> {
    skrifa::FontRef::from_index(&bytes, 0).map_err(|e| e.to_string())?;
    let name = egui_name(folder);
    definitions.font_data.insert(name.clone(), Arc::new(egui::FontData::from_owned(bytes)));
    let mut stack = vec![name];
    stack.extend(egui::FontDefinitions::default().families[&egui::FontFamily::Proportional].iter().cloned());
    definitions.families.insert(family_of(folder), stack);
    Ok(())
}

/// Prefixed so it cannot replace one of egui's own fonts or ours.
fn egui_name(folder: &str) -> String {
    format!("pack:{folder}")
}

fn family_of(folder: &str) -> egui::FontFamily {
    egui::FontFamily::Name(egui_name(folder).into())
}

/// The font's family once egui has it. Drawing with a family egui lacks panics.
pub fn family(ctx: &egui::Context, font: &PackFont) -> Option<egui::FontFamily> {
    let family = family_of(&font.folder);
    ctx.fonts(|fonts| fonts.definitions().families.contains_key(&family)).then_some(family)
}

/// Ours go first in each family, egui's own stay behind them for the glyphs
/// ours lack (emoji, arrows).
fn egui_fonts(ui: Option<Vec<u8>>, display: Option<Vec<u8>>) -> egui::FontDefinitions {
    let mut definitions = egui::FontDefinitions::default();
    let proportional = definitions.families[&egui::FontFamily::Proportional].clone();

    if let Some(bytes) = ui {
        definitions.font_data.insert("ui".into(), Arc::new(egui::FontData::from_owned(bytes)));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            definitions.families.entry(family).or_default().insert(0, "ui".into());
        }
    }

    let mut display_stack = proportional;
    if let Some(bytes) = display {
        definitions
            .font_data
            .insert("display".into(), Arc::new(egui::FontData::from_owned(bytes)));
        display_stack.insert(0, "display".into());
    }
    definitions.families.insert(display_family(), display_stack);
    definitions
}

/// Retried until there is a context to give them to.
fn install_egui_fonts(
    mut commands: Commands,
    mut contexts: EguiContexts,
    pending: Res<PendingEguiFonts>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    ctx.set_fonts(pending.0.clone());
    commands.remove_resource::<PendingEguiFonts>();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A missing font must still leave the family defined, or the first
    /// damage number panics.
    #[test]
    fn the_display_family_exists_without_its_font() {
        let definitions = egui_fonts(None, None);
        assert!(!definitions.families[&display_family()].is_empty());
    }

    #[test]
    fn the_shipped_fonts_go_first() {
        let assets = Assets::default();
        let definitions = egui_fonts(
            read_from_packs(&assets, UI_FONT),
            read_from_packs(&assets, DISPLAY_FONT),
        );
        assert_eq!(definitions.families[&egui::FontFamily::Proportional][0], "ui");
        assert_eq!(definitions.families[&egui::FontFamily::Monospace][0], "ui");
        assert_eq!(definitions.families[&display_family()][0], "display");
    }

    /// Breaks if egui cannot read a shipped face, which would otherwise show
    /// only at runtime.
    #[test]
    fn egui_draws_every_shipped_pack_font() {
        let assets = Assets::default();
        let (found, warnings) = pack_fonts::discover(&scan_packs(&default_packs()[..1], FONTS_DIR), &assets);
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut definitions = egui::FontDefinitions::default();
        for font in &found {
            add_pack_font(&mut definitions, &font.folder, assets.read(&font.face_pack, &font.face_path()).unwrap()).unwrap();
        }
        assert!(add_pack_font(&mut definitions, "broken", b"<html>not a font</html>".to_vec()).is_err());
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for font in &found {
                let family = family(ui.ctx(), font).unwrap_or_else(|| panic!("{} was not installed", font.folder));
                ui.label(egui::RichText::new("Jackdaws love my big sphinx").font(egui::FontId::new(20.0, family)));
            }
        });
        output.textures_delta.clear();
    }
}
