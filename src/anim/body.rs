//! The body animations, as a document you can edit.
//!
//! A prop's clips live in its `.gpp` and go through [`PropEditor`]'s undo and
//! its save. A body's do not: they are pack content, one file per
//! [`AnimationState`], and they need a document of their own. This is it, and
//! it deliberately mirrors `PropEditor` rather than inventing a second shape —
//! whole-map snapshots for undo, a `generation` counter to rebuild on, a
//! `dirty` flag, and gestures so one drag is one step.
//!
//! **What is edited here is what the game plays.** `publish_the_body_clips`
//! copies into the [`ClipLibrary`] that `advance_animators` reads, on the same
//! generation counter, so retiming an idle is visible on every body in the map
//! without a reload. That is the same thing the prop editor does for a model
//! and the same argument the whole editor rests on.
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

/// How much history to keep, matching `PropEditor`'s.
const UNDO_DEPTH: usize = 256;

/// The body clips, and what has been done to them.
///
/// Keyed by [`AnimationState::clip_name`] rather than by the state itself, so
/// the key is the file name — which is what a save writes and what the panels
/// show. The state is recovered by asking the enum, and a key that matches no
/// state is one nothing will ever play, which [`BodyClips::state_of`] reports
/// rather than hides.
#[derive(Resource, Debug, Default)]
pub struct BodyClips {
    clips: BTreeMap<String, StateAnimation>,
    /// Whether the files have been read yet. Read on first use rather than at
    /// startup, because a process that never opens the animation editor should
    /// not pay for them — the *game* gets its copy through
    /// `clip_file::load_body_clips_at_startup`, which is a different path with
    /// a different lifetime.
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

    /// A clip, by the name the panels pick it by: a state's name for a plain
    /// clip, `state#index` for one variant of a blend.
    ///
    /// **One string for both**, so everything that holds "which clip" —
    /// the selection, the strip, the preview — goes on holding a name and
    /// none of it learns what a blend is.
    pub fn get(&self, name: &str) -> Option<&Clip> {
        let (state, variant) = address(name);
        self.clips.get(state)?.clip(variant)
    }

    /// The same, to write — inside a gesture, as `doc_mut` is.
    ///
    /// Only a clip that exists marks the document changed: a name that finds
    /// nothing is not an edit, and counting it would rebuild and dirty
    /// everything keyed on the generation for a write that never happened.
    pub fn clip_mut(&mut self, name: &str) -> Option<&mut Clip> {
        let (state, variant) = address(name);
        self.clips.get(state)?.clip(variant)?;
        self.changed();
        self.clips.get_mut(state)?.clip_mut(variant)
    }

    /// Every clip there is to pick, by name: one per plain state, one per
    /// variant of a blended one.
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

    /// Which state a clip is filed under, if any.
    ///
    /// `None` means a file nothing will play — a typo in a name, most likely.
    /// Worth reporting: the alternative is an animation somebody authored that
    /// silently never runs.
    pub fn state_of(name: &str) -> Option<AnimationState> {
        AnimationState::iter().find(|state| state.clip_name() == name)
    }

    /// Read every state's clip out of the packs, once.
    pub fn ensure_loaded(&mut self, assets: &Assets) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.clips = read_the_packs(assets, &default_packs());
        // Loading is not an edit. Bumped anyway so whatever rebuilds on this
        // counter notices the clips arriving.
        self.generation += 1;
    }

    /// Throw the edits away and take what is on disk.
    ///
    /// The File menu's *Open*, and the honest reading of it for a document
    /// that is a pack rather than a file: there is nothing to browse for,
    /// because which packs are in force is a property of the build. What it is
    /// *for* is the same as anywhere else — getting back to the last saved
    /// state — and going through [`BodyClips::edit`] is what makes it one
    /// `Ctrl+Z` rather than a cliff.
    pub fn reload(&mut self, assets: &Assets) -> usize {
        let fresh = read_the_packs(assets, &default_packs());
        let count = fresh.len();
        self.loaded = true;
        self.edit(|clips| *clips = fresh);
        // `edit` only counts a change, and reloading over an unedited document
        // is no change — but the caller still asked, and `dirty` has to come
        // off either way.
        self.dirty = false;
        count
    }

    /// Every state back on its built-in pose.
    ///
    /// The File menu's *New*: an empty set of body clips is exactly what a
    /// pack that ships none has, and that build animates perfectly well — it
    /// is where this one started. So this is a new document in the full sense
    /// rather than a destructive button wearing the word, and it is one undo
    /// step like any other edit.
    pub fn clear(&mut self) {
        self.edit(|clips| clips.clear());
    }

    /// One edit, recorded as one undo step if it changed anything.
    pub fn edit<T>(&mut self, change: impl FnOnce(&mut BTreeMap<String, StateAnimation>) -> T) -> T {
        let before = self.clips.clone();
        let out = change(&mut self.clips);
        if before != self.clips {
            self.push_undo(before);
            self.changed();
        }
        out
    }

    /// The first frame of a gesture — a drag, a held arrow key — snapshots; the
    /// rest do not.
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

    /// Write every clip back to the pack it came from.
    ///
    /// The **one** `std::fs` here, on the authoring side, exactly as
    /// `prop::document::save` is: reading goes through `Assets` so it reaches a
    /// browser, and writing does not have to.
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

    /// Where a save goes by default.
    pub fn default_pack() -> PathBuf {
        default_packs()
            .into_iter()
            .next()
            .unwrap_or_else(|| PathBuf::from("assets/default"))
    }
}

/// A clip's name taken apart: the state it is filed under, and which variant
/// of that state's blend, if it names one.
pub fn address(name: &str) -> (&str, Option<usize>) {
    match name.split_once('#') {
        Some((state, index)) => (state, index.parse().ok()),
        None => (name, None),
    }
}

/// The name one variant of a blend is picked by.
pub fn variant_name(state: &str, index: usize) -> String {
    format!("{state}#{index}")
}

/// Turn a plain clip into a blend of three copies of it, at the two diagonals
/// and straight ahead — the headings a keyboard can ask a gait for. The copies
/// are identical, so nothing moves until one of them is edited.
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

/// Copy a variant to a new heading beside it, and say where the copy went.
///
/// Halfway to its neighbour above, or fifteen degrees past it if it is the
/// last: a duplicate at the same heading would be a variant nothing could
/// ever blend towards.
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

/// Where a *Save As* landed, filled in on the dialog's own thread.
///
/// A resource of its own rather than a field on [`BodyClips`], which is the
/// document: where somebody is being asked to put it is not part of what is in
/// it, and an undo snapshot that carried a half-answered dialog would restore
/// one. The same shape `PropFileDialog` has, one directory instead of one file.
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
    /// Ask which pack to write into, on a background thread.
    ///
    /// A **directory**, not a file: the body clips are one file per state, so
    /// the question is which pack rather than which name — and `animations/`
    /// under it is added by the save, so what somebody picks is the pack they
    /// would recognise.
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

    /// Where the chooser opens: beside the packs, not wherever the process was
    /// started from.
    fn default_directory() -> PathBuf {
        let pack = BodyClips::default_pack();
        pack.parent().map(Path::to_path_buf).unwrap_or(pack)
    }
}

/// Read one set of clips out of a list of packs, lowest priority first.
///
/// Shared by the first load and by *Open*, so a revert cannot read the packs
/// differently from the way they were read to begin with.
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

/// Hand the edited clips to the game.
///
/// On the generation counter rather than `Res::is_changed`, for the reason
/// `publish_the_saved_prop` gives one layer along: every frame of a drag
/// changes the document, and rebuilding a library for nobody is work the
/// counter exists to avoid. Unlike a prop, this publishes on *edit* rather
/// than on save — a body animation has nothing to bake, so there is no cost to
/// showing it immediately and every reason to.
pub fn publish_the_body_clips(
    clips: Res<BodyClips>,
    mut library: ResMut<ClipLibrary>,
    mut seen: Local<u64>,
) {
    if clips.generation() == *seen {
        return;
    }
    // Before the files have been read, an empty document means "not loaded"
    // rather than "no clips" — and publishing it would take every state the
    // *game* loaded at startup back off its clip.
    if !clips.loaded() {
        return;
    }
    *seen = clips.generation();

    // Every state, not every clip: a clip that has been deleted, reverted
    // away or cleared has to *stop* playing, and a library that was only ever
    // inserted into would keep playing it for the rest of the process. The
    // document is the whole truth about which states are animated from data,
    // which is only safe to act on once it has been read — hence the guard
    // above.
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

    /// Undo is whole-document snapshots, the same as a prop's, and for the
    /// same reason: a handful of clips is a few kilobytes, so cloning per edit
    /// is free and correct by construction.
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

    /// An edit that changes nothing is not a step. Otherwise every panel that
    /// writes back what it read would fill the history with entries that undo
    /// to the same thing.
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

    /// A drag is one step however many frames it took.
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

    /// Asking for a clip that is not there writes nothing: no generation bump
    /// for everything keyed on it to rebuild over, and no dirty flag on a
    /// document nobody edited.
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

    /// A variant is picked by `state#index`, and a plain clip by its state.
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

    /// Blending a clip keeps it as the straight-ahead variant, and a duplicate
    /// lands at a heading of its own.
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

    /// Every state's file name maps back to the state it plays for.
    ///
    /// The round trip is what `publish_the_body_clips` relies on: a clip whose
    /// name matched no state would be one nobody ever sees play, silently.
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
