//! The pack's animations as an editable document: every clip under `animations/` and the taunts in
//! `taunts.toml`, shaped like [`PropEditor`] (snapshot undo, `generation`, `dirty`, gestures). Taunts
//! share the clips' history because a taunt is edited together with its clip.
//!
//! `publish_the_body_clips` copies edits into the [`ClipLibrary`], [`NamedClips`] and [`Taunts`] the
//! game reads, so they show without a reload.
//!
//! [`PropEditor`]: crate::prop::document::PropEditor

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use strum::IntoEnumIterator;

use crate::common::assets::{default_packs, Assets};
use crate::common::class_file::{load_rig, RIGS_DIR};
use crate::common::clip_file::{read_clip, NamedClips, CLIPS_DIR, CLIP_EXTENSION};
use crate::common::skeleton::rig::HUMANOID;
use crate::common::skeleton::rig_file::RigFile;
use crate::common::taunt::{load_taunt_specs, taunt_specs_in, write_taunts, TauntSpec, Taunts, TAUNTS_FILE};
use crate::common::skeleton::clip::{Clip, StateAnimation};
use crate::common::skeleton::state::{AnimationState, ClipLibrary};

const UNDO_DEPTH: usize = 256;

#[derive(Debug, Default, Clone, PartialEq)]
struct PackDoc {
    /// Keyed by file name.
    clips: BTreeMap<String, StateAnimation>,
    /// In file order.
    taunts: Vec<(String, TauntSpec)>,
}

#[derive(Resource, Debug, Default)]
pub struct BodyClips {
    doc: PackDoc,
    /// Read on first use, so a process that never opens the animation editor does not pay. The game
    /// loads its own copy through `clip_file::load_body_clips_at_startup`.
    loaded: bool,
    undo: Vec<PackDoc>,
    redo: Vec<PackDoc>,
    pending: Option<PackDoc>,
    generation: u64,
    pub dirty: bool,
    /// Every rig file the packs hold, used or not, so another rig's clips can be edited before a
    /// class is built on it. Not part of the document: the editor reads rigs and never writes them.
    rigs: BTreeMap<String, RigFile>,
    /// The packs as last read or written, so a save leaves another pack's unchanged content alone.
    on_disk: PackDoc,
}

impl BodyClips {
    pub fn animations(&self) -> &BTreeMap<String, StateAnimation> {
        &self.doc.clips
    }

    pub fn taunts(&self) -> &[(String, TauntSpec)] {
        &self.doc.taunts
    }

    pub fn taunt(&self, key: &str) -> Option<&TauntSpec> {
        self.doc.taunts.iter().find(|(named, _)| named == key).map(|(_, spec)| spec)
    }

    /// Call inside a gesture, as with `doc_mut`.
    pub fn taunt_mut(&mut self, key: &str) -> Option<&mut TauntSpec> {
        self.taunt(key)?;
        self.changed();
        self.doc.taunts.iter_mut().find(|(named, _)| named == key).map(|(_, spec)| spec)
    }

    pub fn edit_taunts<T>(&mut self, change: impl FnOnce(&mut Vec<(String, TauntSpec)>) -> T) -> T {
        self.edit_doc(|doc| change(&mut doc.taunts))
    }

    fn edit_doc<T>(&mut self, change: impl FnOnce(&mut PackDoc) -> T) -> T {
        let before = self.doc.clone();
        let out = change(&mut self.doc);
        if before != self.doc {
            self.push_undo(before);
            self.changed();
        }
        out
    }

    /// A state's name for a plain clip, `state#index` for a blend variant. One string for both, so
    /// nothing that holds "which clip" has to know about blends.
    pub fn get(&self, name: &str) -> Option<&Clip> {
        let (state, variant) = address(name);
        self.doc.clips.get(state)?.clip(variant)
    }

    /// Call inside a gesture, as with `doc_mut`. A name that finds nothing is
    /// not an edit, so it bumps nothing.
    pub fn clip_mut(&mut self, name: &str) -> Option<&mut Clip> {
        let (state, variant) = address(name);
        self.doc.clips.get(state)?.clip(variant)?;
        self.changed();
        self.doc.clips.get_mut(state)?.clip_mut(variant)
    }

    /// One per plain state, one per variant of a blended one.
    pub fn names(&self) -> Vec<String> {
        self.doc
            .clips
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
        Self::rig_and_state(name).map(|(_, state)| state)
    }

    /// Another rig's state clips are filed as `<rig>/<state>`, as their files are.
    pub fn rig_and_state(name: &str) -> Option<(&str, AnimationState)> {
        let (rig, stem) = name.split_once('/').unwrap_or((HUMANOID, name));
        AnimationState::iter().find(|state| state.clip_name() == stem).map(|state| (rig, state))
    }

    pub fn rigs(&self) -> &BTreeMap<String, RigFile> {
        &self.rigs
    }

    pub fn ensure_loaded(&mut self, assets: &Assets) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.doc = read_the_packs(assets, &default_packs());
        self.rigs = read_the_rigs(assets, &default_packs());
        self.on_disk = self.doc.clone();
        // Not an edit, but bumped so whatever rebuilds on the counter sees the clips arrive.
        self.generation += 1;
    }

    /// Throw the edits away and take what is on disk: the File menu's Open, as one undo step through
    /// [`BodyClips::edit`].
    pub fn reload(&mut self, assets: &Assets) -> usize {
        let fresh = read_the_packs(assets, &default_packs());
        let count = fresh.clips.len();
        self.loaded = true;
        self.rigs = read_the_rigs(assets, &default_packs());
        self.on_disk = fresh.clone();
        self.edit_doc(|doc| *doc = fresh);
        // `edit` counts no change when the document was unedited, but `dirty` must come off either way.
        self.dirty = false;
        count
    }

    /// Every state back on its built-in pose: the File menu's New, and one undo step. Clips played
    /// by name and taunts have no built-in to fall back to, so they stay.
    pub fn clear(&mut self) {
        self.edit(|clips| clips.retain(|name, _| BodyClips::state_of(name).is_none()));
    }

    pub fn edit<T>(&mut self, change: impl FnOnce(&mut BTreeMap<String, StateAnimation>) -> T) -> T {
        self.edit_doc(|doc| change(&mut doc.clips))
    }

    pub fn begin_gesture(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(self.doc.clone());
        }
    }

    pub fn doc_mut(&mut self) -> &mut BTreeMap<String, StateAnimation> {
        self.changed();
        &mut self.doc.clips
    }

    pub fn end_gesture(&mut self) {
        let Some(before) = self.pending.take() else { return };
        if before != self.doc {
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
        self.redo.push(std::mem::replace(&mut self.doc, previous));
        self.changed();
    }

    pub fn redo(&mut self) {
        let Some(next) = self.redo.pop() else { return };
        self.undo.push(std::mem::replace(&mut self.doc, next));
        self.changed();
    }

    fn push_undo(&mut self, before: PackDoc) {
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
    /// browser. Writes what changed since the last read and whatever `pack` already holds.
    pub fn save(&mut self, pack: &Path) -> Result<usize, String> {
        let directory = pack.join(CLIPS_DIR);
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;

        let mut written = 0;
        for (name, animation) in &self.doc.clips {
            let path = directory.join(format!("{name}.{CLIP_EXTENSION}"));
            if self.on_disk.clips.get(name) == Some(animation) && !path.exists() {
                continue;
            }
            // Another rig's clips are in a directory of their own.
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            let text = animation.to_toml().map_err(|e| e.to_string())?;
            std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
            written += 1;
        }

        // Edited into the existing file so its comments survive.
        let path = pack.join(TAUNTS_FILE);
        let existing = std::fs::read_to_string(&path).ok();
        let already = existing.as_deref().map(taunt_specs_in).unwrap_or_default();
        let taunts: Vec<(String, TauntSpec)> = self
            .doc
            .taunts
            .iter()
            .filter(|(key, spec)| {
                already.iter().any(|(had, _)| had == key)
                    || !self.on_disk.taunts.iter().any(|(had, read)| had == key && read == spec)
            })
            .cloned()
            .collect();
        if existing.is_some() || !taunts.is_empty() {
            let text = write_taunts(existing.as_deref(), &taunts);
            std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
            written += 1;
        }
        self.on_disk = self.doc.clone();
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

/// Halfway to its neighbor above, or fifteen degrees past the last; a duplicate at the same heading
/// could never be blended toward. Returns the copy's index.
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
fn read_the_packs(assets: &Assets, packs: &[PathBuf]) -> PackDoc {
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
        for rig in rigs_on_disk(&[pack.clone()]) {
            let dir = crate::common::clip_file::rig_clips_dir(&rig);
            for state in AnimationState::iter() {
                let relative = format!("{dir}/{}.{CLIP_EXTENSION}", state.clip_name());
                match assets.read(pack, &relative) {
                    Ok(bytes) => {
                        let read = String::from_utf8(bytes)
                            .map_err(|e| e.to_string())
                            .and_then(|text| StateAnimation::from_toml(&text).map_err(|e| e.to_string()));
                        match read {
                            Ok(clip) => {
                                clips.insert(format!("{rig}/{}", state.clip_name()), clip);
                            }
                            Err(why) => warn!("{}/{relative}: {why}", pack.display()),
                        }
                    }
                    Err(crate::common::assets::AssetError::NotFound(_)) => {}
                    Err(why) => warn!("{}/{relative}: {why}", pack.display()),
                }
            }
        }
        for name in clips_played_by_name(pack) {
            let relative = format!("{CLIPS_DIR}/{name}.{CLIP_EXTENSION}");
            let read = assets
                .read(pack, &relative)
                .map_err(|e| e.to_string())
                .and_then(|bytes| String::from_utf8(bytes).map_err(|e| e.to_string()))
                .and_then(|text| StateAnimation::from_toml(&text).map_err(|e| e.to_string()));
            match read {
                Ok(clip) => {
                    clips.insert(name, clip);
                }
                Err(why) => warn!("{}/{relative}: {why}", pack.display()),
            }
        }
    }
    PackDoc { clips, taunts: load_taunt_specs(assets, packs) }
}

/// Every rig file in the packs, by name. Listed off the disk, as the clips are.
fn rigs_on_disk(packs: &[PathBuf]) -> std::collections::BTreeSet<String> {
    packs
        .iter()
        .filter_map(|pack| std::fs::read_dir(pack.join(RIGS_DIR)).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_string))
        .collect()
}

/// Each rig on disk that loads; one that does not is logged and left out.
fn read_the_rigs(assets: &Assets, packs: &[PathBuf]) -> BTreeMap<String, RigFile> {
    rigs_on_disk(packs)
        .into_iter()
        .filter_map(|name| match load_rig(assets, packs, &name) {
            Ok(rig) => Some((name, rig)),
            Err(why) => {
                warn!("The {name} rig: {why}");
                None
            }
        })
        .collect()
}

/// Every clip in a pack that no state names. Listing a directory is authoring-only; the game is
/// told each name by whatever plays it.
fn clips_played_by_name(pack: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(pack.join(CLIPS_DIR)) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == CLIP_EXTENSION))
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_string))
        .filter(|name| BodyClips::state_of(name).is_none())
        .collect();
    names.sort();
    names
}

/// On the generation counter rather than `Res::is_changed`, which fires every frame of a drag.
/// Publishes on edit, unlike a prop, since there is nothing to bake. The animation editor is not
/// offered while networked, so published taunts reach nobody else.
pub fn publish_the_body_clips(
    clips: Res<BodyClips>,
    mut library: ResMut<ClipLibrary>,
    mut named: ResMut<NamedClips>,
    mut assets: ResMut<bevy::asset::Assets<Clip>>,
    taunts: Option<ResMut<Taunts>>,
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

    // Every state of every rig, not every clip, so a deleted clip stops playing.
    for rig in std::iter::once(HUMANOID).chain(clips.rigs().keys().map(String::as_str)) {
        for state in AnimationState::iter() {
            let name = match rig {
                HUMANOID => state.clip_name().to_string(),
                _ => format!("{rig}/{}", state.clip_name()),
            };
            match clips.animations().get(&name) {
                Some(animation) => library.insert_for(rig, state, animation.clone()),
                None => library.remove_for(rig, state),
            }
        }
    }

    for (name, animation) in clips.animations() {
        if BodyClips::state_of(name).is_some() {
            continue;
        }
        if let Some(clip) = animation.clip(None) {
            named.bypass_change_detection().publish(&mut assets, name, clip);
        }
    }
    named.set_changed();

    if let Some(mut taunts) = taunts {
        let published = Taunts(resolve_taunts(&clips));
        if *taunts != published {
            *taunts = published;
        }
    }
}

/// What the game would load from these taunts, with `length = "clip"` read off the edited clip.
pub fn resolve_taunts(clips: &BodyClips) -> Vec<crate::common::taunt::Taunt> {
    clips
        .taunts()
        .iter()
        .filter_map(|(key, spec)| {
            spec.resolve(key, |name| clips.get(name).map(|clip| clip.length)).ok()
        })
        .collect()
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

    /// Fails if a save copies another pack's unchanged clip or taunt into the target.
    #[test]
    fn a_save_writes_what_changed_and_what_the_pack_holds() {
        let root = std::env::temp_dir().join(format!("grackle-pack-save-{}", std::process::id()));
        let (lower, target) = (root.join("lower"), root.join("target"));
        for pack in [&lower, &target] {
            std::fs::create_dir_all(pack.join(CLIPS_DIR)).unwrap();
        }
        let entry = |clip: &str| {
            format!("clip = \"{clip}\"\nclaims = []\nmovement = \"free\"\nturning = \"free\"\nweapon = \"shown\"\n")
        };
        std::fs::write(lower.join(TAUNTS_FILE), format!("[a]\n{}", entry("x"))).unwrap();
        std::fs::write(target.join(TAUNTS_FILE), format!("[b]\n{}", entry("y"))).unwrap();
        let clip = |length: f32| StateAnimation::from(Clip { length, ..Clip::default() }).to_toml().unwrap();
        std::fs::write(lower.join(CLIPS_DIR).join("low.gpa"), clip(1.0)).unwrap();
        // Another rig's state clip, beside its rig file.
        std::fs::create_dir_all(lower.join(RIGS_DIR)).unwrap();
        std::fs::copy("assets/default/rigs/strider.toml", lower.join(RIGS_DIR).join("strider.toml")).unwrap();
        std::fs::create_dir_all(lower.join(CLIPS_DIR).join("strider")).unwrap();
        std::fs::write(lower.join(CLIPS_DIR).join("strider").join("idle.gpa"), clip(3.0)).unwrap();

        let mut clips = BodyClips::default();
        clips.doc = read_the_packs(&Assets::default(), &[lower.clone(), target.clone()]);
        clips.on_disk = clips.doc.clone();
        assert_eq!(clips.taunts().len(), 2);
        assert!(clips.get("low").is_some(), "a clip played by name was not read");
        assert_eq!(clips.get("strider/idle").map(|clip| clip.length), Some(3.0), "another rig's clip was not read");
        assert_eq!(BodyClips::rig_and_state("strider/idle"), Some(("strider", AnimationState::Idle)));
        assert_eq!(BodyClips::rig_and_state("idle"), Some((HUMANOID, AnimationState::Idle)));

        clips.taunt_mut("b").unwrap().movement = crate::common::taunt::TauntMovement::Stopped;
        clips.save(&target).unwrap();
        let saved = taunt_specs_in(&std::fs::read_to_string(target.join(TAUNTS_FILE)).unwrap());
        let keys: Vec<&str> = saved.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["b"], "an unchanged taunt from another pack was copied in");
        assert!(!target.join(CLIPS_DIR).join("low.gpa").exists(), "an unchanged clip was copied in");

        clips.taunt_mut("a").unwrap().movement = crate::common::taunt::TauntMovement::Stopped;
        clips.clip_mut("low").unwrap().length = 2.0;
        clips.clip_mut("strider/idle").unwrap().length = 4.0;
        clips.save(&target).unwrap();
        let rig_clip = target.join(CLIPS_DIR).join("strider").join("idle.gpa");
        assert!(rig_clip.exists(), "an edited clip of another rig was not written in its own directory");
        let saved = taunt_specs_in(&std::fs::read_to_string(target.join(TAUNTS_FILE)).unwrap());
        assert_eq!(saved.len(), 2, "an edited taunt was not written");
        assert!(target.join(CLIPS_DIR).join("low.gpa").exists(), "an edited clip was not written");

        std::fs::remove_dir_all(&root).ok();
    }
}
