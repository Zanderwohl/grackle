//! The weapon editor: every pack's `weapons.toml` as one document, edited in memory and written
//! back in place. The plan, with what is left, is
//! [documentation/weapon-editor.md](../../documentation/weapon-editor.md).
//!
//! While the mode is open the game's [`WeaponCatalog`] is the draft, so everything that fires a
//! weapon fires the edit; leaving puts back the catalog as last published, which is the files as
//! they stood when the mode opened or as it last saved them.

pub mod form;
pub mod preview;
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

/// The weapons, and the renames to carry into the loadouts when the draft is saved.
type Snapshot = (Vec<PackWeapon>, Vec<(String, String)>);

/// Every pack's weapons, lowest priority first, grouped by pack. A catalog is a few kilobytes, so
/// undo is whole-draft snapshots, as the other editors' is.
#[derive(Resource, Debug, Default)]
pub struct WeaponEditor {
    pub weapons: Vec<PackWeapon>,
    /// Weapon renames the loadouts should follow, `(from, to)` in order. In the draft, so undoing a
    /// rename undoes this too; applied to the loadout files on save.
    pub renames: Vec<(String, String)>,
    pub selected: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    checkpoint: Snapshot,
    /// As last read or saved, which dirty is measured from.
    clean: Snapshot,
    unsettled: bool,
    /// Read from disk the first time the mode opens; an unsaved draft survives leaving it.
    loaded: bool,
    /// Bumped whenever the checkpoint moves, which is when the catalog is rebuilt.
    pub generation: u64,
    /// The generation the catalog and `problems` were last built from.
    played: Option<u64>,
    /// Read when the mode opens; nothing here edits them.
    loadouts: LoadoutFiles,
    pub problems: Vec<(usize, WeaponProblem)>,
}

impl WeaponEditor {
    pub fn dirty(&self) -> bool {
        self.clean != self.checkpoint
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || (self.unsettled && self.snapshot() != self.checkpoint)
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
    }

    /// Back to what is on disk, as one undo step.
    pub fn revert_to(&mut self, saved: Vec<PackWeapon>) {
        self.checkpoint_when_idle(true);
        let was = self.restore((saved, Vec::new()));
        self.undo.push(was);
        self.redo.clear();
        self.settle();
        self.clean = self.checkpoint.clone();
    }

    /// `pack` now holds what the checkpoint says it does. Per pack, so a save that wrote one pack
    /// and failed on another is not dirty about the one it wrote.
    pub fn mark_saved(&mut self, pack: &std::path::Path) {
        self.clean.0 = self
            .packs()
            .iter()
            .flat_map(|of| {
                let from = if of == pack { &self.checkpoint.0 } else { &self.clean.0 };
                from.iter().filter(move |weapon| &weapon.pack == of).cloned()
            })
            .collect();
    }

    /// The renames are on disk now, so nothing should carry them again; a rename undone after this
    /// does not rename the loadouts back.
    fn forget_renames(&mut self) {
        self.renames.clear();
        self.checkpoint.1.clear();
        self.clean.1.clear();
        for snapshot in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            snapshot.1.clear();
        }
    }

    /// Why `key` cannot name a new weapon, or a weapon renamed (`except` is the one being renamed,
    /// whose overrides in other packs share its key and go with it).
    pub fn key_problem(&self, key: &str, except: Option<usize>) -> Option<KeyProblem> {
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
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
        if in_loadouts {
            self.renames.push((from, key.to_owned()));
        }
        self.edited();
    }

    pub fn delete(&mut self, index: usize) {
        if index < self.weapons.len() {
            self.weapons.remove(index);
            self.selected = None;
            self.edited();
        }
    }

    /// Each class slot that names the weapon at `index`, with the draft's renames applied.
    pub fn loadouts_naming(&self, index: usize) -> Vec<(PathBuf, String, crate::common::weapon::Slot)> {
        self.weapons
            .get(index)
            .map(|weapon| self.loadouts.renamed(&self.renames).naming(&weapon.key))
            .unwrap_or_default()
    }

    /// Every pack a weapon is in or could go to, lowest priority first.
    pub fn packs(&self) -> Vec<PathBuf> {
        let mut packs = default_packs();
        for weapon in &self.weapons {
            if !packs.contains(&weapon.pack) {
                packs.push(weapon.pack.clone());
            }
        }
        packs
    }

    /// The packs whose weapons differ from what was last read or saved, each with what it should
    /// now hold and what it held.
    #[allow(clippy::type_complexity)]
    pub fn changed_packs(&self) -> Vec<(PathBuf, Vec<(String, WeaponEntry)>, Vec<(String, WeaponEntry)>)> {
        let of = |weapons: &[PackWeapon], pack: &PathBuf| -> Vec<(String, _)> {
            weapons
                .iter()
                .filter(|weapon| &weapon.pack == pack)
                .map(|weapon| (weapon.key.clone(), weapon.entry.clone()))
                .collect()
        };
        self.packs()
            .into_iter()
            .filter_map(|pack| {
                let (now, was) = (of(&self.checkpoint.0, &pack), of(&self.clean.0, &pack));
                (now != was).then_some((pack, now, was))
            })
            .collect()
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
    *catalog = catalog_with(&editor.weapons, &editor.loadouts.renamed(&editor.renames));
    editor.problems = problems(&editor.weapons);
}

fn put_back_the_published(mut catalog: ResMut<WeaponCatalog>, mut published: ResMut<PublishedCatalog>) {
    if let Some(saved) = published.0.take() {
        *catalog = saved;
    }
}

/// Writes every pack that changed, then publishes what is on disk. A pack that fails is logged
/// and stays dirty, so nothing reads as saved that is not.
pub fn save_the_draft(
    editor: &mut WeaponEditor,
    assets: &crate::common::assets::Assets,
    published: &mut PublishedCatalog,
) {
    editor.checkpoint_when_idle(true);
    let mut wrote_every_pack = true;
    for (pack, weapons, was) in editor.changed_packs() {
        match save::save_pack(&pack, &weapons, &was) {
            Ok(()) => {
                info!("Saved {} weapons to {}", weapons.len(), pack.display());
                editor.mark_saved(&pack);
            }
            Err(e) => {
                error!("Saving weapons: {e}");
                wrote_every_pack = false;
            }
        }
    }
    // Only once the weapons they rename are on disk, or a loadout would name a key no file has.
    let renames = editor.checkpoint.1.clone();
    if wrote_every_pack && !renames.is_empty() {
        let mut renamed_everywhere = true;
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
        if renamed_everywhere {
            editor.loadouts = read_loadouts(assets, &editor.packs());
            editor.forget_renames();
        }
    }
    published.0 = Some(catalog_with(&editor.clean.0, &editor.loadouts.renamed(&editor.clean.1)));
}

pub struct WeaponEditorPlugin;

impl Plugin for WeaponEditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WeaponEditor>()
            .init_resource::<PublishedCatalog>()
            .init_resource::<form::FormMemory>()
            // Registered with their field types, so the form can build any variant from defaults.
            .register_type::<Mounted>()
            .register_type::<crate::common::weapon::Magazine>()
            .add_systems(OnEnter(AppMode::Weapon), open_the_draft)
            .add_systems(Update, play_the_draft.run_if(in_state(AppMode::Weapon)))
            .add_systems(OnExit(AppMode::Weapon), put_back_the_published)
            .add_plugins(ui::WeaponUiPlugin);
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
    fn only_a_changed_pack_is_written() {
        let mut editor = opened();
        assert!(editor.changed_packs().is_empty());
        editor.weapons[0].entry.deploy += 1.0;
        editor.edited();
        editor.checkpoint_when_idle(true);
        let changed = editor.changed_packs();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, editor.weapons[0].pack);
    }

    #[test]
    fn a_pack_that_saved_is_clean_though_another_failed() {
        let mut editor = opened();
        let pack = editor.weapons[0].pack.clone();
        editor.weapons[0].entry.deploy += 1.0;
        editor.edited();
        editor.checkpoint_when_idle(true);
        editor.mark_saved(std::path::Path::new("assets/nowhere"));
        assert!(editor.dirty(), "a pack nobody wrote reads as saved");
        editor.mark_saved(&pack);
        assert!(!editor.dirty());
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
