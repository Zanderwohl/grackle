//! The two typefaces: JetBrains Mono for every egui surface, and Alfa Slab One
//! for what a player sees during a round — the HUD, the pause menu and the
//! damage numbers.
//!
//! Both are pack content, read once at `Startup` through [`Assets`] like the
//! weapon catalogue, so a browser build fetches them the same way. A font that
//! is missing leaves the built-in one in its place rather than refusing to
//! start.

use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::assets::Assets;
use crate::common::lang::default_packs;

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

/// Built at `Startup`, handed to egui on its first pass and then removed.
#[derive(Resource)]
struct PendingEguiFonts(egui::FontDefinitions);

pub struct FontsPlugin;

impl Plugin for FontsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Assets>()
            .init_resource::<DisplayFont>()
            .add_systems(Startup, load_fonts)
            .add_systems(
                EguiPrimaryContextPass,
                install_egui_fonts.run_if(resource_exists::<PendingEguiFonts>),
            );
    }
}

/// Highest priority last, so a pack overriding a font wins.
fn read_from_packs(assets: &Assets, relative: &str) -> Option<Vec<u8>> {
    let mut found = None;
    for pack in default_packs() {
        match assets.read(&pack, relative) {
            Ok(bytes) => found = Some(bytes),
            Err(crate::common::assets::AssetError::NotFound(_)) => {}
            Err(e) => error!("{e}"),
        }
    }
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

    if let Some(bytes) = &display {
        commands.insert_resource(DisplayFont(fonts.add(Font::from_bytes(bytes.clone()))));
    }
    commands.insert_resource(PendingEguiFonts(egui_fonts(ui, display)));
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
}
