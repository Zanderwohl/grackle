//! The weapon editor: every pack's weapons, one file each (`weapons/{key}.toml`), as one draft,
//! edited in memory and each written back to its own file in place. The plan, with what is left, is
//! [documentation/weapon-editor.md](../../documentation/weapon-editor.md).
//!
//! While the mode is open the game's [`WeaponCatalog`] is the draft, so everything that fires a
//! weapon fires the edit; leaving puts back the catalog as last published, which is the files as
//! they stood when the mode opened or as it last saved them.

pub mod form;
pub mod preview;
pub mod range;
pub mod save;
pub mod ui;

use std::path::PathBuf;

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::assets::default_packs;
use crate::common::weapon::{Mounted, WeaponCatalog, WeaponId};
use crate::common::weapon_file::{
    catalog_with, problems, read_loadouts, read_pack_weapons, LoadoutFiles, PackWeapon, WeaponEntry,
    WeaponProblem,
};
use crate::editor::multicam::Multicam;

const UNDO_DEPTH: usize = 256;

/// The weapons, and the renames since the last save.
type Snapshot = (Vec<PackWeapon>, Vec<Rename>);

/// A weapon's key changed from `from` to `to`, which on save moves its file in each pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub from: String,
    pub to: String,
    pub loadouts: bool,
}

fn loadout_renames(renames: &[Rename]) -> Vec<(String, String)> {
    renames.iter().filter(|rename| rename.loadouts).map(|rename| (rename.from.clone(), rename.to.clone())).collect()
}

fn renamed(key: &str, renames: &[Rename]) -> String {
    let mut key = key.to_owned();
    for rename in renames {
        if rename.from == key {
            key = rename.to.clone();
        }
    }
    key
}

/// One weapon file's change since it was read or saved.
#[derive(Debug, Clone, PartialEq)]
pub struct FileChange {
    pub pack: PathBuf,
    pub key: String,
    /// `None` deletes the file.
    pub now: Option<WeaponEntry>,
    /// What the file holds; `None` for one not written yet.
    pub was: Option<WeaponEntry>,
}

/// Every pack's weapons, lowest priority first, grouped by pack. A catalog is a few kilobytes, so
/// undo is whole-draft snapshots, as the other editors' is.
#[derive(Resource, Debug, Default)]
pub struct WeaponEditor {
    pub weapons: Vec<PackWeapon>,
    /// Since the last save, in order. In the draft, so undoing a rename undoes this too.
    pub renames: Vec<Rename>,
    pub selected: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    checkpoint: Snapshot,
    /// As last read or saved, which dirty is measured from.
    clean: Snapshot,
    /// Cached by [`WeaponEditor::compare_with_clean`], since the panels ask every frame.
    dirty: bool,
    unsettled: bool,
    /// Read from disk the first time the mode opens; an unsaved draft survives leaving it.
    loaded: bool,
    /// Bumped whenever the checkpoint moves, which is when the catalog is rebuilt.
    pub generation: u64,
    /// The generation the catalog and `problems` were last built from.
    played: Option<u64>,
    /// Read when the mode opens; nothing in this mode edits them.
    loadouts: LoadoutFiles,
    pub problems: Vec<(usize, WeaponProblem)>,
    /// Files written since the caller last took them, for the File menu's Recent list.
    pub written: Vec<PathBuf>,
}

impl WeaponEditor {
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    fn compare_with_clean(&mut self) {
        self.dirty = !self.changed_files().is_empty() || self.clean.1 != self.checkpoint.1;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
            || (self.unsettled && (self.weapons != self.checkpoint.0 || self.renames != self.checkpoint.1))
    }

    fn snapshot(&self) -> Snapshot {
        (self.weapons.clone(), self.renames.clone())
    }

    /// Keeps the same weapon selected, by pack and key: entries shift when one is added or removed.
    fn restore(&mut self, (weapons, renames): Snapshot) -> Snapshot {
        let was = self.snapshot();
        let selected = self.selected.and_then(|index| self.weapons.get(index)).map(|weapon| (weapon.pack.clone(), weapon.key.clone()));
        self.weapons = weapons;
        self.renames = renames;
        self.selected = selected.and_then(|(pack, key)| {
            self.weapons.iter().position(|weapon| weapon.pack == pack && weapon.key == key)
        });
        was
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn edited(&mut self) {
        self.unsettled = true;
    }

    /// One undo step per stretch of activity: a drag or a typed name is one step.
    pub fn checkpoint_when_idle(&mut self, idle: bool) {
        if !idle || !self.unsettled {
            return;
        }
        self.unsettled = false;
        let now = self.snapshot();
        if now == self.checkpoint {
            return;
        }
        let before = std::mem::replace(&mut self.checkpoint, now);
        self.compare_with_clean();
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.generation += 1;
    }

    pub fn undo(&mut self) {
        // An edit not yet checkpointed is the step being undone.
        self.checkpoint_when_idle(true);
        let Some(previous) = self.undo.pop() else { return };
        let was = self.restore(previous);
        self.redo.push(was);
        self.settle();
    }

    pub fn redo(&mut self) {
        let Some(next) = self.redo.pop() else { return };
        let was = self.restore(next);
        self.undo.push(was);
        self.settle();
    }

    fn settle(&mut self) {
        self.checkpoint = self.snapshot();
        self.compare_with_clean();
        self.unsettled = false;
        self.generation += 1;
        if self.selected.is_some_and(|index| index >= self.weapons.len()) {
            self.selected = self.weapons.len().checked_sub(1);
        }
    }

    /// A different document: history does not carry over.
    pub fn open(&mut self, weapons: Vec<PackWeapon>) {
        self.undo.clear();
        self.redo.clear();
        self.weapons = weapons;
        self.renames.clear();
        self.loaded = true;
        self.settle();
        self.clean = self.checkpoint.clone();
        self.compare_with_clean();
    }

    /// Back to what is on disk, as one undo step.
    pub fn revert_to(&mut self, saved: Vec<PackWeapon>) {
        self.checkpoint_when_idle(true);
        let was = self.restore((saved, Vec::new()));
        self.undo.push(was);
        self.redo.clear();
        self.settle();
        self.clean = self.checkpoint.clone();
        self.compare_with_clean();
    }

    /// Per file, so a save that wrote one file and failed on another is not dirty about the first.
    pub fn mark_saved(&mut self, pack: &std::path::Path, key: &str) {
        let at = |weapon: &PackWeapon| weapon.pack == pack && weapon.key == key;
        self.clean.0.retain(|weapon| !at(weapon));
        if let Some(weapon) = self.checkpoint.0.iter().find(|weapon| at(weapon)) {
            self.clean.0.push(weapon.clone());
        }
        self.compare_with_clean();
    }

    /// The renames are on disk now, so nothing should carry them again; a rename undone after this
    /// does not rename the loadouts back.
    fn forget_renames(&mut self) {
        self.renames.clear();
        self.checkpoint.1.clear();
        self.clean.1.clear();
        self.compare_with_clean();
        for snapshot in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            snapshot.1.clear();
        }
    }

    /// Why `key` cannot name a new weapon, or a weapon renamed (`except` is the one being renamed,
    /// whose overrides in other packs share its key and go with it).
    pub fn key_problem(&self, key: &str, except: Option<usize>) -> Option<KeyProblem> {
        if !crate::common::weapon_file::is_weapon_key(key) {
            return Some(KeyProblem::Malformed);
        }
        let id = WeaponId::of(key);
        let renaming = except.and_then(|index| self.weapons.get(index)).map(|weapon| weapon.key.as_str());
        let taken = self
            .weapons
            .iter()
            .any(|weapon| Some(weapon.key.as_str()) != renaming && (weapon.key == key || WeaponId::of(&weapon.key) == id));
        taken.then_some(KeyProblem::Taken)
    }

    /// At the end of its pack's group, so the draft stays in pack order.
    fn insert(&mut self, weapon: PackWeapon) -> usize {
        let order = self.packs();
        let rank = |pack: &PathBuf| order.iter().position(|of| of == pack).unwrap_or(order.len());
        let at = self.weapons.iter().position(|other| rank(&other.pack) > rank(&weapon.pack)).unwrap_or(self.weapons.len());
        self.weapons.insert(at, weapon);
        self.selected = Some(at);
        self.edited();
        at
    }

    pub fn new_weapon(&mut self, pack: PathBuf, key: &str) -> usize {
        let entry = WeaponEntry {
            name_key: format!("weapon.names.{key}"),
            model: None,
            icon: None,
            bucket: 1,
            deploy: 0.5,
            magazine: crate::common::weapon::Magazine {
                clip: Some(10),
                reserve: 40,
                reload: 1.0,
                reload_one_at_a_time: false,
                first_reload: None,
            },
            primary: Mounted {
                action: crate::common::weapon::WeaponAction::Hitscan(default()),
                cadence: crate::common::weapon::Cadence::Semi { interval: 0.25 },
                cost: crate::common::weapon::Cost::Ammo { rounds: 1 },
            },
            secondary: Mounted::EMPTY,
            scoped_primary: None,
        };
        self.insert(PackWeapon { pack, key: key.to_owned(), entry })
    }

    /// A copy under its own key and name key, so it is a weapon of its own rather than an override.
    pub fn duplicate(&mut self, index: usize, pack: PathBuf, key: &str) -> usize {
        let mut entry = self.weapons[index].entry.clone();
        entry.name_key = format!("weapon.names.{key}");
        self.insert(PackWeapon { pack, key: key.to_owned(), entry })
    }

    /// A copy under the same key and name key in a later pack, which the loader reads as the later
    /// pack retuning it.
    pub fn override_in(&mut self, index: usize, pack: PathBuf) -> Option<usize> {
        let source = self.weapons.get(index)?.clone();
        let order = self.packs();
        let rank = |pack: &PathBuf| order.iter().position(|of| of == pack);
        let later = rank(&pack) > rank(&source.pack);
        let already = self.weapons.iter().any(|weapon| weapon.pack == pack && weapon.key == source.key);
        (later && !already).then(|| self.insert(PackWeapon { pack, ..source }))
    }

    /// Every pack's entry under the old key, so an override stays an override of the weapon.
    pub fn rename(&mut self, index: usize, key: &str, in_loadouts: bool) {
        let Some(from) = self.weapons.get(index).map(|weapon| weapon.key.clone()) else { return };
        for weapon in self.weapons.iter_mut().filter(|weapon| weapon.key == from) {
            weapon.key = key.to_owned();
        }
        self.renames.push(Rename { from, to: key.to_owned(), loadouts: in_loadouts });
        self.edited();
    }

    pub fn delete(&mut self, index: usize) {
        if index < self.weapons.len() {
            self.weapons.remove(index);
            self.selected = None;
            self.edited();
        }
    }

    /// With the draft's renames applied.
    pub fn loadouts_naming(&self, index: usize) -> Vec<(PathBuf, String, crate::common::weapon::Slot)> {
        self.weapons
            .get(index)
            .map(|weapon| self.loadouts.renamed(&loadout_renames(&self.renames)).naming(&weapon.key))
            .unwrap_or_default()
    }

    /// Lowest priority first.
    pub fn packs(&self) -> Vec<PathBuf> {
        let mut packs = default_packs();
        for weapon in &self.weapons {
            if !packs.contains(&weapon.pack) {
                packs.push(weapon.pack.clone());
            }
        }
        packs
    }

    pub fn changed_files(&self) -> Vec<FileChange> {
        use std::collections::BTreeMap;
        fn by_file(weapons: &[PackWeapon]) -> BTreeMap<(&PathBuf, &str), &WeaponEntry> {
            weapons.iter().map(|weapon| ((&weapon.pack, weapon.key.as_str()), &weapon.entry)).collect()
        }
        let (now, was) = (by_file(&self.checkpoint.0), by_file(&self.clean.0));
        let mut changes = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for weapon in self.checkpoint.0.iter().chain(&self.clean.0) {
            let file = (&weapon.pack, weapon.key.as_str());
            if !seen.insert(file) {
                continue;
            }
            let (now, was) = (now.get(&file).copied(), was.get(&file).copied());
            if now != was {
                changes.push(FileChange { pack: weapon.pack.clone(), key: weapon.key.clone(), now: now.cloned(), was: was.cloned() });
            }
        }
        changes
    }

    /// The file a new `key` in `pack` was renamed from, what it holds, and whether this save
    /// deletes it; it stays when a new weapon has taken its key.
    fn renamed_from<'a>(&self, changes: &'a [FileChange], pack: &PathBuf, key: &str) -> Option<(&'a str, &'a WeaponEntry, bool)> {
        changes.iter().find_map(|change| match &change.was {
            Some(was) if &change.pack == pack && change.key != key && renamed(&change.key, &self.checkpoint.1) == key => {
                Some((change.key.as_str(), was, change.now.is_none()))
            }
            _ => None,
        })
    }

    /// The weapon at `index`, as edited, becomes `key` in `pack`; the original goes back to what its
    /// file holds. Clears the history: an undo would take away a weapon whose file is on disk, and
    /// the next save would delete it.
    pub fn save_as(&mut self, index: usize, pack: PathBuf, key: &str) -> Option<usize> {
        self.checkpoint_when_idle(true);
        let weapon = self.weapons.get(index)?.clone();
        if weapon.pack == pack && weapon.key == key {
            return Some(index);
        }
        let original = self.clean.0.iter().find(|clean| clean.pack == weapon.pack && clean.key == weapon.key).cloned();
        self.weapons.remove(index);
        if let Some(original) = original {
            self.weapons.insert(index, original);
        }
        // Overwritten.
        self.weapons.retain(|other| !(other.pack == pack && other.key == key));
        let at = self.insert(PackWeapon { pack, key: key.to_owned(), entry: weapon.entry });
        self.checkpoint_when_idle(true);
        self.undo.clear();
        self.redo.clear();
        Some(at)
    }

    /// A weapon file read from outside the draft, as saved; one the draft already has is selected.
    pub fn adopt(&mut self, weapon: PackWeapon) -> usize {
        if let Some(at) = self.weapons.iter().position(|other| other.pack == weapon.pack && other.key == weapon.key) {
            self.selected = Some(at);
            return at;
        }
        self.checkpoint_when_idle(true);
        let at = self.insert(weapon.clone());
        // In every snapshot rather than an undo step, so no undo takes away a file that is on disk.
        for snapshot in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            snapshot.0.push(weapon.clone());
        }
        self.checkpoint.0.push(weapon.clone());
        self.clean.0.push(weapon);
        self.unsettled = false;
        self.generation += 1;
        self.compare_with_clean();
        at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyProblem {
    /// Empty, or not lowercase letters, digits and underscores.
    Malformed,
    /// A weapon in some pack already has it, or a key that hashes to the same id.
    Taken,
}

/// The catalog as published, while the mode has the draft in its place.
#[derive(Resource, Debug, Default)]
pub struct PublishedCatalog(Option<WeaponCatalog>);

fn open_the_draft(
    mut editor: ResMut<WeaponEditor>,
    assets: Res<crate::common::assets::Assets>,
    mut catalog: ResMut<WeaponCatalog>,
    mut published: ResMut<PublishedCatalog>,
    mut cameras: Query<&mut Camera, With<Multicam>>,
) {
    // The panels cover the window; the next mode that shows the viewports switches them back on.
    for mut camera in &mut cameras {
        camera.is_active = false;
    }
    // An unsaved draft is kept; a clean one is read again, so edits made on disk meanwhile show.
    if !editor.loaded || !editor.dirty() {
        editor.open(read_pack_weapons(&assets, &default_packs()));
    }
    // So the range has something to play the moment the mode opens.
    if editor.selected.is_none() && !editor.weapons.is_empty() {
        editor.selected = Some(0);
    }
    editor.loadouts = read_loadouts(&assets, &editor.packs());
    published.0 = Some(catalog.clone());
    editor.played = None;
    play(&mut editor, &mut catalog);
}

fn play_the_draft(mut editor: ResMut<WeaponEditor>, mut catalog: ResMut<WeaponCatalog>) {
    if editor.played != Some(editor.generation) {
        play(&mut editor, &mut catalog);
    }
}

fn play(editor: &mut WeaponEditor, catalog: &mut WeaponCatalog) {
    editor.played = Some(editor.generation);
    *catalog = catalog_with(&editor.weapons, &editor.loadouts.renamed(&loadout_renames(&editor.renames)));
    editor.problems = problems(&editor.weapons);
}

fn put_back_the_published(mut catalog: ResMut<WeaponCatalog>, mut published: ResMut<PublishedCatalog>) {
    if let Some(saved) = published.0.take() {
        *catalog = saved;
    }
}

/// A file that fails to write stays dirty, so nothing reads as saved that is not.
pub fn save_the_draft(
    editor: &mut WeaponEditor,
    assets: &crate::common::assets::Assets,
    published: &mut PublishedCatalog,
) {
    editor.checkpoint_when_idle(true);
    let changes = editor.changed_files();
    let mut wrote_every_file = true;
    let mut kept: Vec<(PathBuf, String)> = Vec::new();
    // Writes before deletions, and new files before changed ones, so a rename starts from its old
    // file before a new weapon under the old key rewrites it.
    let writes = changes.iter().filter(|change| change.now.is_some() && change.was.is_none());
    let writes = writes.chain(changes.iter().filter(|change| change.now.is_some() && change.was.is_some()));
    for change in writes {
        if let Err(from) = save_file(editor, &changes, change, false) {
            wrote_every_file = false;
            // Its content is in no file yet, so the file it was renamed from stays.
            kept.extend(from.map(|key| (change.pack.clone(), key)));
        }
    }
    // Recomputed: a renamed weapon's old file was marked saved when it was moved.
    for change in editor.changed_files().into_iter().filter(|change| change.now.is_none()) {
        let Some(was) = &change.was else { continue };
        if kept.iter().any(|(pack, key)| *pack == change.pack && *key == change.key) {
            continue;
        }
        match save::delete_weapon(&change.pack, &change.key, was) {
            Ok(()) => {
                info!("Deleted {}", crate::common::weapon_file::weapon_path(&change.pack, &change.key).display());
                editor.mark_saved(&change.pack, &change.key);
            }
            Err(e) => {
                error!("Deleting a weapon: {e}");
                wrote_every_file = false;
            }
        }
    }
    // Only once the weapons they rename are on disk, or a loadout would name a key no file has.
    let renames = loadout_renames(&editor.checkpoint.1);
    if wrote_every_file {
        let mut renamed_everywhere = true;
        if !renames.is_empty() {
            for pack in editor.packs() {
                match save::rename_in_pack(&pack, &renames) {
                    Ok(true) => info!("Renamed weapons in {}'s loadouts", pack.display()),
                    Ok(false) => {}
                    Err(e) => {
                        error!("Renaming weapons in loadouts: {e}");
                        renamed_everywhere = false;
                    }
                }
            }
            editor.loadouts = read_loadouts(assets, &editor.packs());
        }
        if renamed_everywhere {
            editor.forget_renames();
        }
    }
    publish(editor, published);
}

/// On a failure, which is logged, the key of the file it was renamed from, if that file must stay.
/// `overwrite` is for a Save As whose dialog already asked.
fn save_file(
    editor: &mut WeaponEditor,
    changes: &[FileChange],
    change: &FileChange,
    overwrite: bool,
) -> Result<(), Option<String>> {
    let Some(now) = &change.now else { return Ok(()) };
    // A rename's old file is only its starting point when the new one is not there yet.
    let from = match change.was {
        None => editor.renamed_from(changes, &change.pack, &change.key),
        Some(_) => None,
    };
    let on_disk = overwrite.then(|| save::on_disk(&change.pack, &change.key));
    let expected = match &on_disk {
        Some(on_disk) => on_disk.as_ref(),
        None => change.was.as_ref(),
    };
    let path = crate::common::weapon_file::weapon_path(&change.pack, &change.key);
    match save::save_weapon(&change.pack, &change.key, now, expected, from) {
        Ok(deleted) => {
            info!("Saved {}", path.display());
            editor.written.push(path.clone());
            editor.mark_saved(&change.pack, &change.key);
            match (from, deleted) {
                (Some((old, _, true)), Ok(())) => {
                    let pack = change.pack.clone();
                    editor.mark_saved(&pack, old);
                    Ok(())
                }
                // The old file is left to the deletions, which try it again.
                (_, Err(e)) => {
                    error!("Saved {}, but could not remove the file it was renamed from: {e}", path.display());
                    Err(None)
                }
                _ => Ok(()),
            }
        }
        Err(e) => {
            error!("Could not save {}: {e}", path.display());
            Err(from.map(|(key, _, _)| key.to_owned()))
        }
    }
}

/// Writes only `key`'s file in `pack`, for Save As.
pub fn save_one(editor: &mut WeaponEditor, pack: &std::path::Path, key: &str, published: &mut PublishedCatalog) {
    editor.checkpoint_when_idle(true);
    let changes = editor.changed_files();
    if let Some(change) = changes.iter().find(|change| change.pack == pack && change.key == key) {
        let _ = save_file(editor, &[], change, true);
    }
    publish(editor, published);
}

/// What leaving the mode puts back: the files as they now stand.
fn publish(editor: &WeaponEditor, published: &mut PublishedCatalog) {
    published.0 = Some(catalog_with(&editor.clean.0, &editor.loadouts.renamed(&loadout_renames(&editor.clean.1))));
}

/// `--weapon`. A resource rather than an initial state, so entering goes through
/// `OnEnter(AppMode::Weapon)`.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StartInWeaponEditor(pub bool);

fn start_in_the_weapon_editor(start: Res<StartInWeaponEditor>, mut next: ResMut<NextState<AppMode>>, mut done: Local<bool>) {
    if !start.0 || *done {
        return;
    }
    info!("Opening the weapon editor, as the command line asked");
    next.set(AppMode::Weapon);
    *done = true;
}

pub struct WeaponEditorPlugin;

impl Plugin for WeaponEditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WeaponEditor>()
            .init_resource::<PublishedCatalog>()
            .init_resource::<form::FormMemory>()
            .init_resource::<StartInWeaponEditor>()
            // Ungated: it has to run in whatever mode the process started in.
            .add_systems(Update, start_in_the_weapon_editor)
            // Registered with their field types, so the form can build any variant from defaults.
            .register_type::<Mounted>()
            .register_type::<crate::common::weapon::Magazine>()
            .add_systems(OnEnter(AppMode::Weapon), open_the_draft)
            .add_systems(Update, play_the_draft.run_if(in_state(AppMode::Weapon)))
            .add_systems(OnExit(AppMode::Weapon), put_back_the_published)
            .add_plugins((ui::WeaponUiPlugin, range::RangePlugin));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon_file::default_catalog;

    fn opened() -> WeaponEditor {
        let mut editor = WeaponEditor::default();
        editor.open(read_pack_weapons(&crate::common::assets::Assets::default(), &default_packs()));
        editor
    }

    #[test]
    fn an_edit_is_one_undo_step_once_idle() {
        let mut editor = opened();
        let fresh = editor.weapons.clone();
        editor.weapons[0].entry.deploy += 1.0;
        editor.edited();
        editor.checkpoint_when_idle(false);
        assert!(!editor.dirty(), "checkpointed while busy");
        editor.checkpoint_when_idle(true);
        assert!(editor.dirty());

        editor.undo();
        assert_eq!(editor.weapons, fresh);
        assert!(!editor.dirty());
        editor.redo();
        assert_ne!(editor.weapons, fresh);
    }

    #[test]
    fn only_a_changed_file_is_written() {
        let mut editor = opened();
        assert!(editor.changed_files().is_empty());
        editor.weapons[0].entry.deploy += 1.0;
        editor.edited();
        editor.checkpoint_when_idle(true);
        let changed = editor.changed_files();
        assert_eq!(changed.len(), 1);
        assert_eq!((&changed[0].pack, &changed[0].key), (&editor.weapons[0].pack, &editor.weapons[0].key));
    }

    #[test]
    fn a_file_that_saved_is_clean_though_another_failed() {
        let mut editor = opened();
        let (pack, key) = (editor.weapons[0].pack.clone(), editor.weapons[0].key.clone());
        editor.weapons[0].entry.deploy += 1.0;
        editor.weapons[1].entry.deploy += 1.0;
        editor.edited();
        editor.checkpoint_when_idle(true);
        editor.mark_saved(&pack, &key);
        assert!(editor.dirty(), "a file nobody wrote reads as saved");
        let (pack, key) = (editor.weapons[1].pack.clone(), editor.weapons[1].key.clone());
        editor.mark_saved(&pack, &key);
        assert!(!editor.dirty());
    }

    #[test]
    fn a_rename_is_a_new_file_moved_from_the_old_one() {
        let mut editor = opened();
        let pistol = editor.weapons.iter().position(|weapon| weapon.key == "pistol").expect("a pistol ships");
        let pack = editor.weapons[pistol].pack.clone();
        editor.rename(pistol, "sidearm", false);
        editor.checkpoint_when_idle(true);
        let changes = editor.changed_files();
        assert_eq!(changes.len(), 2, "{changes:?}");
        let new = changes.iter().find(|change| change.key == "sidearm").expect("the new file");
        assert!(new.was.is_none());
        assert_eq!(editor.renamed_from(&changes, &pack, "sidearm").map(|(key, _, deleted)| (key, deleted)), Some(("pistol", true)));
        editor.new_weapon(pack.clone(), "pistol");
        editor.checkpoint_when_idle(true);
        let changes = editor.changed_files();
        assert_eq!(
            editor.renamed_from(&changes, &pack, "sidearm").map(|(key, _, deleted)| (key, deleted)),
            Some(("pistol", false)),
            "a rename lost its old file to a new weapon under the old key"
        );
    }

    #[test]
    fn save_as_leaves_the_original_as_its_file_holds_it() {
        let mut editor = opened();
        let pistol = editor.weapons.iter().position(|weapon| weapon.key == "pistol").expect("a pistol ships");
        let saved = editor.weapons[pistol].clone();
        editor.weapons[pistol].entry.deploy = 3.0;
        editor.edited();
        let at = editor.save_as(pistol, the_user_pack(), "sidearm").expect("saves as");
        assert_eq!(editor.weapons[at].key, "sidearm");
        assert_eq!(editor.weapons[at].entry.deploy, 3.0);
        assert_eq!(editor.selected, Some(at));
        assert!(editor.weapons.contains(&saved), "the original did not go back to its file");
        let changes = editor.changed_files();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].key, "sidearm");
    }

    #[test]
    fn an_adopted_file_is_not_dirty() {
        let mut editor = opened();
        let mut weapon = editor.weapons[0].clone();
        weapon.pack = PathBuf::from("elsewhere");
        let at = editor.adopt(weapon.clone());
        assert_eq!(editor.weapons[at], weapon);
        assert!(!editor.dirty());
        assert_eq!(editor.adopt(weapon.clone()), at, "adopted twice");
        editor.weapons[0].entry.deploy += 1.0;
        editor.edited();
        editor.undo();
        assert!(editor.weapons.contains(&weapon), "an undo took away the opened file");
        assert!(editor.changed_files().is_empty(), "a save would delete the opened file");
    }

    fn the_user_pack() -> PathBuf {
        default_packs().last().expect("a user pack").clone()
    }

    #[test]
    fn a_new_weapon_goes_after_its_packs_others_and_is_selected() {
        let mut editor = opened();
        let at = editor.new_weapon(the_user_pack(), "scattergun");
        assert_eq!(editor.selected, Some(at));
        assert_eq!(editor.weapons[at].pack, the_user_pack());
        let ranks: Vec<usize> = editor
            .weapons
            .iter()
            .map(|weapon| editor.packs().iter().position(|pack| *pack == weapon.pack).unwrap())
            .collect();
        assert!(ranks.windows(2).all(|pair| pair[0] <= pair[1]), "the draft is out of pack order");
    }

    #[test]
    fn a_taken_or_malformed_key_is_refused() {
        let editor = opened();
        let shipped = editor.weapons[0].key.clone();
        assert_eq!(editor.key_problem(&shipped, None), Some(KeyProblem::Taken));
        assert_eq!(editor.key_problem(&shipped, Some(0)), None, "a weapon may keep its own key");
        assert_eq!(editor.key_problem("Has Spaces", None), Some(KeyProblem::Malformed));
        assert_eq!(editor.key_problem("", None), Some(KeyProblem::Malformed));
        assert_eq!(editor.key_problem("scattergun", None), None);
    }

    #[test]
    fn an_override_goes_only_to_a_later_pack() {
        let mut editor = opened();
        let shipped = editor.weapons.iter().position(|weapon| weapon.pack != the_user_pack()).expect("a shipped weapon");
        let own_pack = editor.weapons[shipped].pack.clone();
        assert!(editor.override_in(shipped, own_pack).is_none(), "overrode into its own pack");
        let at = editor.override_in(shipped, the_user_pack()).expect("overrides into the user pack");
        assert_eq!(editor.weapons[at].key, editor.weapons[shipped].key);
        assert!(editor.override_in(shipped, the_user_pack()).is_none(), "overrode twice");
        editor.checkpoint_when_idle(true);
        assert!(editor.problems.is_empty() && crate::common::weapon_file::problems(&editor.weapons).is_empty());
    }

    #[test]
    fn a_rename_takes_the_weapons_overrides_with_it() {
        let mut editor = opened();
        let pistol = editor.weapons.iter().position(|weapon| weapon.key == "pistol").expect("a pistol ships");
        let twin = editor.override_in(pistol, the_user_pack()).expect("overrides");
        let base = editor.weapons.iter().position(|weapon| weapon.key == "pistol" && weapon.pack != the_user_pack()).unwrap();
        assert_eq!(editor.key_problem("sidearm", Some(base)), None);
        editor.rename(base, "sidearm", false);
        assert_eq!(editor.weapons[twin].key, "sidearm", "the override was left under the old key");
        assert!(crate::common::weapon_file::problems(&editor.weapons).is_empty());
    }

    #[test]
    fn undo_keeps_the_same_weapon_selected() {
        let mut editor = opened();
        let last = editor.weapons.len() - 1;
        let key = editor.weapons[last].key.clone();
        let first_pack = editor.weapons[0].pack.clone();
        editor.new_weapon(first_pack, "scattergun");
        editor.checkpoint_when_idle(true);
        editor.selected = editor.weapons.iter().position(|weapon| weapon.key == key);
        editor.undo();
        assert_eq!(editor.selected.map(|index| editor.weapons[index].key.clone()), Some(key));
    }

    #[test]
    fn undoing_a_rename_undoes_its_loadout_rename() {
        let mut editor = opened();
        editor.loadouts = read_loadouts(&crate::common::assets::Assets::default(), &default_packs());
        let pistol = editor.weapons.iter().position(|weapon| weapon.key == "pistol").expect("a pistol ships");
        let before = editor.loadouts_naming(pistol).len();
        assert!(before > 0, "nothing names the pistol, so this tests nothing");
        editor.rename(pistol, "sidearm", true);
        editor.checkpoint_when_idle(true);
        assert_eq!(editor.loadouts_naming(pistol).len(), before, "the loadouts did not follow the rename");
        editor.undo();
        assert_eq!(editor.weapons[pistol].key, "pistol");
        assert!(editor.renames.is_empty());
        assert!(!editor.dirty());
    }

    #[test]
    fn revert_is_one_undo_step() {
        let mut editor = opened();
        let fresh = editor.weapons.clone();
        editor.weapons[0].entry.bucket = 9;
        editor.edited();
        editor.revert_to(fresh.clone());
        assert_eq!(editor.weapons, fresh);
        assert!(!editor.dirty());
        editor.undo();
        assert_eq!(editor.weapons[0].entry.bucket, 9);
    }

    /// Fails if the draft and the loader disagree about what the files say.
    #[test]
    fn the_untouched_draft_is_the_shipped_catalog() {
        let editor = opened();
        let draft = crate::common::weapon_file::catalog_of(
            &crate::common::assets::Assets::default(),
            &editor.packs(),
            &editor.weapons,
        );
        assert_eq!(draft, default_catalog());
    }

    #[test]
    fn leaving_the_mode_puts_the_published_catalog_back() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_state::<AppMode>()
            .insert_resource(crate::common::assets::Assets::default())
            .insert_resource(default_catalog())
            .init_resource::<WeaponEditor>()
            .init_resource::<PublishedCatalog>()
            .add_systems(OnEnter(AppMode::Weapon), open_the_draft)
            .add_systems(Update, play_the_draft.run_if(in_state(AppMode::Weapon)))
            .add_systems(OnExit(AppMode::Weapon), put_back_the_published);
        let shipped = default_catalog();

        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Weapon);
        app.update();
        {
            let mut editor = app.world_mut().resource_mut::<WeaponEditor>();
            editor.weapons[0].entry.deploy = 9.0;
            editor.edited();
            editor.checkpoint_when_idle(true);
        }
        app.update();
        assert_ne!(*app.world().resource::<WeaponCatalog>(), shipped, "the range would not play the edit");

        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Editor);
        app.update();
        assert_eq!(*app.world().resource::<WeaponCatalog>(), shipped, "an unsaved edit outlived the mode");
    }
}
