//! The body animations as an editable document: pack content, one file per [`AnimationState`], with
//! the same shape as [`PropEditor`] (snapshot undo, a `generation` counter, `dirty`, gestures).
//!
//! `publish_the_body_clips` copies edits into the [`ClipLibrary`] the game plays, so a retimed clip
//! shows on every body without a reload.
//!
//! [`PropEditor`]: crate::prop::document::PropEditor

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use strum::IntoEnumIterator;

use crate::common::assets::{default_packs, Assets};
use crate::common::clip_file::{read_clip, CLIPS_DIR, CLIP_EXTENSION};
use crate::common::skeleton::clip::{Clip, StateAnimation};
use crate::common::skeleton::state::{AnimationState, ClipLibrary};

const UNDO_DEPTH: usize = 256;

/// Keyed by [`AnimationState::clip_name`], which is the file name. A key matching no state is
/// reported by [`BodyClips::state_of`] rather than hidden.
#[derive(Resource, Debug, Default)]
pub struct BodyClips {
    clips: BTreeMap<String, StateAnimation>,
    /// Read on first use, so a process that never opens the animation editor does not pay. The game
    /// loads its own copy through `clip_file::load_body_clips_at_startup`.
    loaded: bool,
    undo: Vec<BTreeMap<String, StateAnimation>>,
    redo: Vec<BTreeMap<String, StateAnimation>>,
    pending: Option<BTreeMap<String, StateAnimation>>,
    generation: u64,
    pub dirty: bool,
}

impl BodyClips {
    pub fn animations(&self) -> &BTreeMap<String, StateAnimation> {
        &self.clips
    }

    /// A state's name for a plain clip, `state#index` for a blend variant. One string for both, so
    /// nothing that holds "which clip" has to know about blends.
    pub fn get(&self, name: &str) -> Option<&Clip> {
        let (state, variant) = address(name);
        self.clips.get(state)?.clip(variant)
    }

    /// Call inside a gesture, as with `doc_mut`. A name that finds nothing is
    /// not an edit, so it bumps nothing.
    pub fn clip_mut(&mut self, name: &str) -> Option<&mut Clip> {
        let (state, variant) = address(name);
        self.clips.get(state)?.clip(variant)?;
        self.changed();
        self.clips.get_mut(state)?.clip_mut(variant)
    }

    /// One per plain state, one per variant of a blended one.
    pub fn names(&self) -> Vec<String> {
        self.clips
            .iter()
            .flat_map(|(state, animation)| match animation {
                StateAnimation::Single(_) => vec![state.clone()],
                StateAnimation::Blend(variants) => {
                    (0..variants.len()).map(|index| variant_name(state, index)).collect()
                }
            })
            .collect()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// `None` means a file nothing will play, most likely a typo.
    pub fn state_of(name: &str) -> Option<AnimationState> {
        AnimationState::iter().find(|state| state.clip_name() == name)
    }

    pub fn ensure_loaded(&mut self, assets: &Assets) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.clips = read_the_packs(assets, &default_packs());
        // Not an edit, but bumped so whatever rebuilds on the counter sees the clips arrive.
        self.generation += 1;
    }

    /// Throw the edits away and take what is on disk: the File menu's Open, as one undo step through
    /// [`BodyClips::edit`].
    pub fn reload(&mut self, assets: &Assets) -> usize {
        let fresh = read_the_packs(assets, &default_packs());
        let count = fresh.len();
        self.loaded = true;
        self.edit(|clips| *clips = fresh);
        // `edit` counts no change when the document was unedited, but `dirty` must come off either way.
        self.dirty = false;
        count
    }

    /// Every state back on its built-in pose: the File menu's New, and one undo step.
    pub fn clear(&mut self) {
        self.edit(|clips| clips.clear());
    }

    pub fn edit<T>(&mut self, change: impl FnOnce(&mut BTreeMap<String, StateAnimation>) -> T) -> T {
        let before = self.clips.clone();
        let out = change(&mut self.clips);
        if before != self.clips {
            self.push_undo(before);
            self.changed();
        }
        out
    }

    pub fn begin_gesture(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(self.clips.clone());
        }
    }

    pub fn doc_mut(&mut self) -> &mut BTreeMap<String, StateAnimation> {
        self.changed();
        &mut self.clips
    }

    pub fn end_gesture(&mut self) {
        let Some(before) = self.pending.take() else { return };
        if before != self.clips {
            self.push_undo(before);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        let Some(previous) = self.undo.pop() else { return };
        self.redo.push(std::mem::replace(&mut self.clips, previous));
        self.changed();
    }

    pub fn redo(&mut self) {
        let Some(next) = self.redo.pop() else { return };
        self.undo.push(std::mem::replace(&mut self.clips, next));
        self.changed();
    }

    fn push_undo(&mut self, before: BTreeMap<String, StateAnimation>) {
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn changed(&mut self) {
        self.generation += 1;
        self.dirty = true;
    }

    /// The one `std::fs` here, on the authoring side; reading goes through `Assets` so it reaches a
    /// browser.
    pub fn save(&mut self, pack: &Path) -> Result<usize, String> {
        let directory = pack.join(CLIPS_DIR);
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;

        let mut written = 0;
        for (name, animation) in &self.clips {
            let path = directory.join(format!("{name}.{CLIP_EXTENSION}"));
            let text = animation.to_toml().map_err(|e| e.to_string())?;
            std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
            written += 1;
        }
        self.dirty = false;
        Ok(written)
    }

    pub fn default_pack() -> PathBuf {
        default_packs()
            .into_iter()
            .next()
            .unwrap_or_else(|| PathBuf::from("assets/default"))
    }
}

/// The state a clip is filed under, and which blend variant if it names one.
pub fn address(name: &str) -> (&str, Option<usize>) {
    match name.split_once('#') {
        Some((state, index)) => (state, index.parse().ok()),
        None => (name, None),
    }
}

pub fn variant_name(state: &str, index: usize) -> String {
    format!("{state}#{index}")
}

/// Three copies at the two diagonals and straight ahead, the headings a keyboard can ask a gait for.
pub fn blend_by_heading(animation: &mut StateAnimation) {
    use crate::common::skeleton::clip::Variant;
    let StateAnimation::Single(clip) = animation else { return };
    let quarter = std::f32::consts::FRAC_PI_4;
    *animation = StateAnimation::Blend(
        [-quarter, 0.0, quarter]
            .into_iter()
            .map(|heading| Variant { heading, clip: clip.clone() })
            .collect(),
    );
}

/// Halfway to its neighbour above, or fifteen degrees past the last; a duplicate at the same heading
/// could never be blended towards. Returns the copy's index.
pub fn duplicate_variant(variants: &mut Vec<crate::common::skeleton::clip::Variant>, index: usize) -> usize {
    let Some(original) = variants.get(index).cloned() else { return index };
    let above = variants
        .iter()
        .map(|variant| variant.heading)
        .filter(|heading| *heading > original.heading)
        .min_by(|a, b| a.total_cmp(b));
    let heading = match above {
        Some(above) => (original.heading + above) / 2.0,
        None => original.heading + 15f32.to_radians(),
    };
    variants.insert(index + 1, crate::common::skeleton::clip::Variant { heading, ..original });
    index + 1
}

/// Where a Save As landed, filled in on the dialog's thread. Not on [`BodyClips`], so an undo
/// snapshot cannot carry a half-answered dialog.
#[derive(Resource, Clone)]
pub struct BodyClipDialog {
    result: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
}

impl Default for BodyClipDialog {
    fn default() -> Self {
        Self { result: std::sync::Arc::new(std::sync::Mutex::new(None)) }
    }
}

impl BodyClipDialog {
    /// A directory, since the clips are one file per state; `animations/` is added by the save.
    pub fn ask_where_to_save(&self) {
        let slot = self.result.clone();
        let from = Self::default_directory();
        std::thread::spawn(move || {
            let mut chooser = rfd::AsyncFileDialog::new();
            if from.is_dir() {
                chooser = chooser.set_directory(&from);
            }
            if let Some(handle) = pollster::block_on(chooser.pick_folder()) {
                *slot.lock().unwrap() = Some(handle.path().to_path_buf());
            }
        });
    }

    pub fn take(&self) -> Option<PathBuf> {
        self.result.lock().unwrap().take()
    }

    fn default_directory() -> PathBuf {
        let pack = BodyClips::default_pack();
        pack.parent().map(Path::to_path_buf).unwrap_or(pack)
    }
}

/// Lowest priority first. Shared by the first load and Open, so a revert reads the packs the same
/// way.
fn read_the_packs(assets: &Assets, packs: &[PathBuf]) -> BTreeMap<String, StateAnimation> {
    let mut clips = BTreeMap::new();
    for pack in packs {
        for state in AnimationState::iter() {
            match read_clip(assets, pack, state) {
                Ok(Some(clip)) => {
                    clips.insert(state.clip_name().to_string(), clip);
                }
                Ok(None) => {}
                Err(why) => warn!("{}: {why}", state.clip_name()),
            }
        }
    }
    clips
}

/// On the generation counter rather than `Res::is_changed`, which fires every frame of a drag.
/// Publishes on edit, unlike a prop, since there is nothing to bake.
pub fn publish_the_body_clips(
    clips: Res<BodyClips>,
    mut library: ResMut<ClipLibrary>,
    mut seen: Local<u64>,
) {
    if clips.generation() == *seen {
        return;
    }
    // Before the files are read, empty means "not loaded"; publishing it would strip every clip the
    // game loaded at startup.
    if !clips.loaded() {
        return;
    }
    *seen = clips.generation();

    // Every state, not every clip, so a deleted clip stops playing.
    for state in AnimationState::iter() {
        match clips.animations().get(state.clip_name()) {
            Some(animation) => library.insert(state, animation.clone()),
            None => library.remove(state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_clip(length: f32) -> Clip {
        Clip {
            length,
            ..Clip::default()
        }
    }

    #[test]
    fn an_edit_is_one_undo_step() {
        let mut clips = BodyClips::default();
        clips.edit(|clips| {
            clips.insert("idle".into(), a_clip(1.0).into());
        });
        assert!(clips.can_undo());

        clips.edit(|clips| {
            clips.get_mut("idle").unwrap().clip_mut(None).unwrap().length = 2.0;
        });
        clips.undo();
        assert_eq!(clips.get("idle").map(|clip| clip.length), Some(1.0));
        clips.undo();
        assert!(clips.get("idle").is_none());
    }

    /// Otherwise every panel that writes back what it read would fill the history.
    #[test]
    fn writing_back_what_was_there_is_not_a_step() {
        let mut clips = BodyClips::default();
        clips.edit(|clips| {
            clips.insert("idle".into(), a_clip(1.0).into());
        });
        let steps = clips.undo.len();

        clips.edit(|clips| {
            clips.insert("idle".into(), a_clip(1.0).into());
        });
        assert_eq!(clips.undo.len(), steps, "a no-op edit became an undo step");
    }

    #[test]
    fn a_gesture_is_one_step() {
        let mut clips = BodyClips::default();
        clips.edit(|clips| {
            clips.insert("idle".into(), a_clip(1.0).into());
        });
        let steps = clips.undo.len();

        clips.begin_gesture();
        for frame in 1..=20 {
            clips.clip_mut("idle").unwrap().length = 1.0 + frame as f32 * 0.01;
        }
        clips.end_gesture();

        assert_eq!(clips.undo.len(), steps + 1, "a drag was not one step");
        clips.undo();
        assert_eq!(clips.get("idle").map(|clip| clip.length), Some(1.0));
    }

    /// Breaks if asking for a missing clip bumps the generation or dirties.
    #[test]
    fn a_missing_clip_is_not_an_edit() {
        let mut clips = BodyClips::default();
        clips.edit(|clips| {
            clips.insert("idle".into(), a_clip(1.0).into());
        });
        clips.dirty = false;
        let generation = clips.generation();

        assert!(clips.clip_mut("nothing").is_none());
        assert!(clips.clip_mut("idle#3").is_none());
        assert_eq!(clips.generation(), generation, "a miss bumped the generation");
        assert!(!clips.dirty, "a miss dirtied the document");

        assert!(clips.clip_mut("idle").is_some());
        assert!(clips.generation() > generation, "a hit did not count as a write");
    }

    #[test]
    fn a_variant_is_addressed_by_its_state_and_index() {
        use crate::common::skeleton::clip::Variant;
        let mut clips = BodyClips::default();
        clips.edit(|clips| {
            clips.insert("idle".into(), a_clip(1.0).into());
            clips.insert(
                "run_forward".into(),
                StateAnimation::Blend(vec![
                    Variant { heading: -0.5, clip: a_clip(2.0) },
                    Variant { heading: 0.5, clip: a_clip(3.0) },
                ]),
            );
        });
        assert_eq!(
            clips.names(),
            vec!["idle".to_string(), "run_forward#0".into(), "run_forward#1".into()],
        );
        assert_eq!(clips.get("run_forward#1").map(|clip| clip.length), Some(3.0));
        assert_eq!(clips.get("idle").map(|clip| clip.length), Some(1.0));
        assert!(clips.get("run_forward").is_none(), "a blend is not one clip");
        assert!(clips.get("idle#0").is_none(), "a plain clip has no variants");
    }

    #[test]
    fn a_clip_blends_into_three_and_duplicates_beside_itself() {
        let mut animation: StateAnimation = a_clip(1.0).into();
        blend_by_heading(&mut animation);
        let StateAnimation::Blend(variants) = &mut animation else {
            panic!("blending made no blend");
        };
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[1].heading, 0.0);
        assert!(variants.iter().all(|variant| variant.clip == a_clip(1.0)));

        let landed = duplicate_variant(variants, 1);
        assert_eq!(landed, 2);
        assert!((variants[2].heading - std::f32::consts::FRAC_PI_8).abs() < 1e-5);
        let last = duplicate_variant(variants, 3);
        assert!(variants[last].heading > variants[3].heading);
    }

    /// `publish_the_body_clips` relies on this round trip; a name matching no state never plays.
    #[test]
    fn every_state_name_finds_its_way_back() {
        for state in AnimationState::iter() {
            assert_eq!(
                BodyClips::state_of(state.clip_name()),
                Some(state),
                "{state:?} does not answer to its own clip name",
            );
        }
        assert_eq!(BodyClips::state_of("not_a_state"), None);
    }
}
