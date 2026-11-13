//! The weapon editor: every pack's `weapons.toml` as one document, edited in memory and written
//! back in place. The plan, with what is left, is
//! [documentation/weapon-editor.md](../../documentation/weapon-editor.md).
//!
//! While the mode is open the game's [`WeaponCatalog`] is the draft, so everything that fires a
//! weapon fires the edit; leaving puts back the catalog as last published, which is the files as
//! they stood when the mode opened or as it last saved them.

pub mod save;
pub mod ui;

use std::path::PathBuf;

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::assets::default_packs;
use crate::common::weapon::WeaponCatalog;
use crate::common::weapon_file::{catalog_of, read_pack_weapons, PackWeapon};
use crate::editor::multicam::Multicam;

const UNDO_DEPTH: usize = 256;

/// Every pack's weapons, lowest priority first, in key order within a pack. A catalog is a few
/// kilobytes, so undo is whole-draft snapshots, as the other editors' is.
#[derive(Resource, Debug, Default)]
pub struct WeaponEditor {
    pub weapons: Vec<PackWeapon>,
    pub selected: Option<usize>,
    undo: Vec<Vec<PackWeapon>>,
    redo: Vec<Vec<PackWeapon>>,
    checkpoint: Vec<PackWeapon>,
    /// As last read or saved, which dirty is measured from.
    clean: Vec<PackWeapon>,
    unsettled: bool,
    /// Read from disk the first time the mode opens; an unsaved draft survives leaving it.
    loaded: bool,
    /// Bumped whenever the checkpoint moves, which is when the catalog is rebuilt.
    pub generation: u64,
}

impl WeaponEditor {
    pub fn dirty(&self) -> bool {
        self.clean != self.checkpoint
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || (self.unsettled && self.weapons != self.checkpoint)
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
        if self.weapons == self.checkpoint {
            return;
        }
        let before = std::mem::replace(&mut self.checkpoint, self.weapons.clone());
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
        self.redo.push(std::mem::replace(&mut self.weapons, previous));
        self.settle();
    }

    pub fn redo(&mut self) {
        let Some(next) = self.redo.pop() else { return };
        self.undo.push(std::mem::replace(&mut self.weapons, next));
        self.settle();
    }

    fn settle(&mut self) {
        self.checkpoint = self.weapons.clone();
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
        self.clean = self.weapons.clone();
        self.loaded = true;
        self.settle();
    }

    /// Back to what is on disk, as one undo step.
    pub fn revert_to(&mut self, saved: Vec<PackWeapon>) {
        self.checkpoint_when_idle(true);
        self.undo.push(std::mem::replace(&mut self.weapons, saved));
        self.redo.clear();
        self.clean = self.weapons.clone();
        self.settle();
    }

    pub fn mark_saved(&mut self) {
        self.checkpoint_when_idle(true);
        self.clean = self.checkpoint.clone();
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
    /// now hold.
    pub fn changed_packs(&self) -> Vec<(PathBuf, Vec<(String, crate::common::weapon_file::WeaponEntry)>)> {
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
                let now = of(&self.checkpoint, &pack);
                (now != of(&self.clean, &pack)).then_some((pack, now))
            })
            .collect()
    }
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
    if !editor.loaded {
        editor.open(read_pack_weapons(&assets, &default_packs()));
    }
    published.0 = Some(catalog.clone());
    *catalog = catalog_of(&assets, &editor.packs(), &editor.weapons);
}

fn play_the_draft(
    editor: Res<WeaponEditor>,
    assets: Res<crate::common::assets::Assets>,
    mut catalog: ResMut<WeaponCatalog>,
    mut played: Local<Option<u64>>,
) {
    if *played == Some(editor.generation) {
        return;
    }
    *played = Some(editor.generation);
    *catalog = catalog_of(&assets, &editor.packs(), &editor.weapons);
}

fn put_back_the_published(mut catalog: ResMut<WeaponCatalog>, mut published: ResMut<PublishedCatalog>) {
    if let Some(saved) = published.0.take() {
        *catalog = saved;
    }
}

/// Writes every pack that changed, then publishes. A pack that fails is logged and the draft
/// stays dirty, so nothing reads as saved that is not on disk.
pub fn save_the_draft(
    editor: &mut WeaponEditor,
    assets: &crate::common::assets::Assets,
    published: &mut PublishedCatalog,
) {
    editor.checkpoint_when_idle(true);
    let mut failed = false;
    for (pack, weapons) in editor.changed_packs() {
        match save::save_pack(&pack, &weapons) {
            Ok(()) => info!("Saved {} weapons to {}", weapons.len(), pack.display()),
            Err(e) => {
                error!("Saving weapons: {e}");
                failed = true;
            }
        }
    }
    if failed {
        return;
    }
    editor.mark_saved();
    published.0 = Some(catalog_of(assets, &editor.packs(), &editor.weapons));
}

pub struct WeaponEditorPlugin;

impl Plugin for WeaponEditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WeaponEditor>()
            .init_resource::<PublishedCatalog>()
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
        let draft = catalog_of(&crate::common::assets::Assets::default(), &editor.packs(), &editor.weapons);
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
