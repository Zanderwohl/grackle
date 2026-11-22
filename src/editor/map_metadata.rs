use bevy::prelude::*;

use crate::common::game_rules::{GameRules, Modes};
use crate::common::mode::GameMode;
use crate::common::movement_rules::MovementRules;
use crate::common::palette::Palette;
use crate::common::sky::{Brightness, SkyTime, SunPosition};
use crate::editor::editable::FeatureId;

pub const POSTCARD_SLOTS: usize = 3;

/// A postcard the bake always takes.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltInPostcard {
    Top,
}

/// One of the postcards the bake takes, as a map names it. Only the choice is saved, never the
/// picture, which is in `cache/postcards/` ([`crate::tool::postcard_bake`]).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostcardChoice {
    BuiltIn(BuiltInPostcard),
    /// A spectator camera feature, or with `copy` the Mirror copy of it at that image.
    SpectatorCamera { feature: FeatureId, copy: Option<u32> },
}

impl PostcardChoice {
    /// What the picker stores and a picture is kept by in memory. Unlike [`Self::file_stem`] it
    /// does not change when a camera's order does.
    pub fn key(&self) -> String {
        match self {
            PostcardChoice::BuiltIn(BuiltInPostcard::Top) => "top".into(),
            PostcardChoice::SpectatorCamera { feature, copy: None } => format!("camera_{}", feature._id()),
            PostcardChoice::SpectatorCamera { feature, copy: Some(image) } => format!("camera_{}_copy_{image}", feature._id()),
        }
    }

    /// The inverse of [`Self::key`].
    pub fn from_key(key: &str) -> Option<Self> {
        if key == "top" {
            return Some(PostcardChoice::BuiltIn(BuiltInPostcard::Top));
        }
        let (feature, copy) = feature_and_copy(key.strip_prefix("camera_")?)?;
        Some(PostcardChoice::SpectatorCamera { feature, copy })
    }

    /// The picture's file name without `.png`: `top`, `camera_{order}_{feature}` or
    /// `camera_{order}_{feature}_copy_{image}`, so pictures sort as the spectator cameras do when
    /// the numbers are compared as numbers. `order` is ignored for a built-in.
    pub fn file_stem(&self, order: u32) -> String {
        match self {
            PostcardChoice::BuiltIn(_) => self.key(),
            PostcardChoice::SpectatorCamera { .. } => format!("camera_{order}_{}", &self.key()["camera_".len()..]),
        }
    }

    /// The inverse of [`Self::file_stem`], with the order; zero for a built-in.
    pub fn from_file_stem(stem: &str) -> Option<(Self, u32)> {
        if stem == "top" {
            return Some((PostcardChoice::BuiltIn(BuiltInPostcard::Top), 0));
        }
        let (order, rest) = stem.strip_prefix("camera_")?.split_once('_')?;
        let (feature, copy) = feature_and_copy(rest)?;
        Some((PostcardChoice::SpectatorCamera { feature, copy }, order.parse().ok()?))
    }

    /// Where the spectator cameras would put it: by order, feature, then a source before its
    /// copies. `None` for a built-in.
    pub fn camera_rank(&self, order: u32) -> Option<(u32, u64, u32)> {
        match self {
            PostcardChoice::BuiltIn(_) => None,
            PostcardChoice::SpectatorCamera { feature, copy } => Some((order, feature._id(), copy.map_or(0, |image| image + 1))),
        }
    }
}

/// `{feature}` or `{feature}_copy_{image}`.
fn feature_and_copy(text: &str) -> Option<(FeatureId, Option<u32>)> {
    let (feature, copy) = match text.split_once("_copy_") {
        Some((feature, image)) => (feature, Some(image.parse().ok()?)),
        None => (text, None),
    };
    Some((FeatureId::from_raw(feature.parse().ok()?), copy))
}

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
    pub brightness: Brightness,
    /// [`Track::path`](crate::common::music::Track::path)s, the same in every pack.
    pub music: Vec<String>,
    /// In display order. Empty slots show nothing.
    pub postcards: [Option<PostcardChoice>; POSTCARD_SLOTS],
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
            brightness: Brightness::default(),
            music: Vec::new(),
            postcards: [None; POSTCARD_SLOTS],
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
pub fn publish_game_rules(
    metadata: Res<MapMetadata>,
    modes: Res<Modes>,
    play_as: Option<Res<crate::common::game_rules::PlayAs>>,
    mut rules: ResMut<GameRules>,
) {
    let play_as = play_as.and_then(|play_as| play_as.0);
    if metadata.is_changed() || modes.is_changed() || rules.is_added() {
        let mut mode = modes.of(metadata.game_mode);
        if let Some(class) = play_as {
            mode.classes = vec![class];
        }
        rules.set_if_neq(GameRules(mode));
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
