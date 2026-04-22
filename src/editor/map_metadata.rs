use bevy::prelude::Resource;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::common::mode::GameMode;
use crate::constants::SCHEMA_VERSION;

/// Strongly typed map metadata (backed by the `metadata` key/value table in the blueprint DB).
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
    /// File format version; matches [`crate::constants::SCHEMA_VERSION`].
    #[inline]
    pub fn map_schema_version() -> u64 {
        SCHEMA_VERSION
    }

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

    pub(crate) fn load_from_connection(conn: &Connection) -> rusqlite::Result<Self> {
        let authors_str: Option<String> = conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'authors'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let game_mode_str: Option<String> = conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'game_mode'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        let authors = authors_str
            .map(|s| parse_authors_csv(&s))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec!["anonymous".to_string()]);

        let game_mode = game_mode_str
            .as_deref()
            .and_then(|s| GameMode::try_from(s).ok())
            .unwrap_or(GameMode::Arena);

        let authors_ui_text = format_authors_for_ui(&authors);
        let authors_ui_synced_from = authors.clone();

        Ok(Self {
            authors,
            game_mode,
            authors_ui_text,
            authors_ui_synced_from,
        })
    }

    pub(crate) fn insert_rows(&self, tx: &Transaction<'_>) -> rusqlite::Result<()> {
        tx.execute(
            "INSERT INTO metadata (key, value) VALUES ('authors', ?1)",
            params![serialize_authors_csv(&self.authors)],
        )?;
        tx.execute(
            "INSERT INTO metadata (key, value) VALUES ('game_mode', ?1)",
            params![self.game_mode.prefix()],
        )?;
        Ok(())
    }
}

/// Serialize authors as comma-separated quoted fields; internal `"` doubled (RFC 4180).
pub fn serialize_authors_csv(authors: &[String]) -> String {
    if authors.is_empty() {
        return "\"anonymous\"".to_string();
    }
    authors
        .iter()
        .map(|a| {
            let inner = a.replace('"', "\"\"");
            format!("\"{inner}\"")
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_authors_csv(s: &str) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return vec!["anonymous".to_string()];
    }
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    while it.peek().is_some() {
        while matches!(it.peek(), Some(' ') | Some(',')) {
            it.next();
        }
        if it.peek().is_none() {
            break;
        }
        if it.peek() != Some(&'"') {
            let mut token = String::new();
            while let Some(&c) = it.peek() {
                if c == ',' {
                    break;
                }
                token.push(it.next().unwrap());
            }
            let t = token.trim().to_string();
            if !t.is_empty() {
                out.push(t);
            }
            if it.peek() == Some(&',') {
                it.next();
            }
            continue;
        }
        it.next(); // opening "
        let mut field = String::new();
        loop {
            match it.next() {
                None => {
                    out.push(field);
                    return finalize_authors(out);
                }
                Some('"') => {
                    if it.peek() == Some(&'"') {
                        it.next();
                        field.push('"');
                    } else {
                        out.push(field);
                        break;
                    }
                }
                Some(c) => field.push(c),
            }
        }
    }
    finalize_authors(out)
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
/// `anonymous`. (Author names containing commas cannot be represented in this field; the DB row
/// still uses quoted CSV via [`serialize_authors_csv`].)
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
