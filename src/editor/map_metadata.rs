use bevy::prelude::*;

use crate::common::game_rules::{GameRules, Modes};
use crate::common::mode::GameMode;
use crate::common::movement_rules::MovementRules;
use crate::common::palette::Palette;
use crate::common::sky::{SkyTime, SunPosition};

/// Strongly typed map metadata, written at the top of a blueprint.
#[derive(Resource, Debug, Clone)]
pub struct MapMetadata {
    /// What the map is called to a player; `None` falls back to the file name.
    pub display_name: Option<String>,
    pub authors: Vec<String>,
    pub game_mode: GameMode,
    pub movement: MovementRules,
    pub palette: Palette,
    pub sky: SkyTime,
    pub sun: SunPosition,
    /// The panel's comma-separated authors text, refreshed when `authors` is replaced from disk.
    authors_ui_text: String,
    authors_ui_synced_from: Vec<String>,
}

impl Default for MapMetadata {
    fn default() -> Self {
        let authors = vec!["anonymous".to_string()];
        let authors_ui_text = format_authors_for_ui(&authors);
        Self {
            display_name: None,
            authors,
            game_mode: GameMode::Arena,
            movement: MovementRules::default(),
            palette: Palette::default(),
            sky: SkyTime::default(),
            sun: SunPosition::default(),
            authors_ui_text,
            authors_ui_synced_from: vec!["anonymous".to_string()],
        }
    }
}

impl MapMetadata {
    pub fn reset_to_new_map_defaults(&mut self) {
        *self = Self::default();
    }

    /// Call each frame before drawing the panel; refreshes the text only when `authors` changed, so an
    /// in-progress edit is not clobbered.
    pub fn sync_authors_ui_buffer_from_authors(&mut self) {
        if self.authors_ui_synced_from != self.authors {
            self.authors_ui_synced_from.clone_from(&self.authors);
            self.authors_ui_text = format_authors_for_ui(&self.authors);
        }
    }

    pub(crate) fn authors_ui_text_mut(&mut self) -> &mut String {
        &mut self.authors_ui_text
    }

    pub(crate) fn apply_authors_ui_text_edit(&mut self) {
        self.authors = parse_authors_from_ui(&self.authors_ui_text);
        self.authors_ui_synced_from.clone_from(&self.authors);
    }

    pub(crate) fn normalize_authors_ui_text(&mut self) {
        self.authors_ui_text = format_authors_for_ui(&self.authors);
        self.authors_ui_synced_from.clone_from(&self.authors);
    }

}
/// The only writer of [`MovementRules`]. Ungated, since a map's settings can change between
/// rounds and a client is handed them at any time.
pub fn publish_movement_rules(metadata: Res<MapMetadata>, mut rules: ResMut<MovementRules>) {
    if metadata.is_changed() {
        rules.set_if_neq(metadata.movement);
    }
}

/// The only writer of [`GameRules`]. Ungated, like [`publish_movement_rules`]; a client's
/// [`Modes`] may arrive after its map.
pub fn publish_game_rules(metadata: Res<MapMetadata>, modes: Res<Modes>, mut rules: ResMut<GameRules>) {
    if metadata.is_changed() || modes.is_changed() {
        rules.set_if_neq(GameRules(modes.of(metadata.game_mode)));
    }
}

fn finalize_authors(out: Vec<String>) -> Vec<String> {
    if out.is_empty() {
        vec!["anonymous".to_string()]
    } else {
        out
    }
}

pub fn format_authors_for_ui(authors: &[String]) -> String {
    if authors.is_empty() {
        return String::new();
    }
    authors.join(", ")
}

/// Splits on commas only and trims. Empty input yields `anonymous`. A name with a comma cannot be
/// typed here, but the blueprint stores a list, so one set elsewhere survives.
pub fn parse_authors_from_ui(s: &str) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return vec!["anonymous".to_string()];
    }
    let out: Vec<String> = s
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    finalize_authors(out)
}
