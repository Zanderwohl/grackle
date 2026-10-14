use bevy::prelude::Resource;

use crate::common::mode::GameMode;

/// Strongly typed map metadata, written at the top of a blueprint.
#[derive(Resource, Debug, Clone)]
pub struct MapMetadata {
    pub authors: Vec<String>,
    pub game_mode: GameMode,
    /// Comma-separated authors line in the metadata panel; kept in sync when `authors` is replaced
    /// from disk. Not a separate persisted column.
    authors_ui_text: String,
    authors_ui_synced_from: Vec<String>,
}

impl Default for MapMetadata {
    fn default() -> Self {
        let authors = vec!["anonymous".to_string()];
        let authors_ui_text = format_authors_for_ui(&authors);
        Self {
            authors,
            game_mode: GameMode::Arena,
            authors_ui_text,
            authors_ui_synced_from: vec!["anonymous".to_string()],
        }
    }
}

impl MapMetadata {
    /// Reset to defaults when starting a new map from the editor.
    pub fn reset_to_new_map_defaults(&mut self) {
        *self = Self::default();
    }

    /// Call each frame before drawing the metadata panel so a load/replace of `authors` refreshes
    /// the text field without clobbering in-progress edits.
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
fn finalize_authors(out: Vec<String>) -> Vec<String> {
    if out.is_empty() {
        vec!["anonymous".to_string()]
    } else {
        out
    }
}

/// Authors line for the metadata panel: plain comma-separated names (no quoting in the UI).
pub fn format_authors_for_ui(authors: &[String]) -> String {
    if authors.is_empty() {
        return String::new();
    }
    authors.join(", ")
}

/// Parse the metadata authors field: split on commas only, trim each segment. Empty input yields
/// `anonymous`. (Author names containing commas cannot be typed into this field; the blueprint
/// itself stores a list, so they survive a save and load if set some other way.)
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
