//! The File menu's Recent submenu: files each authoring mode last saved, newest first, kept in
//! `assets/user/recent.toml`. A file joins the list when it is saved, never when it is opened.
//!
//! Its own file rather than a field of `settings.toml`, because the Settings window writes that
//! file from a draft and would put back a list older than the one in memory.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub const KEPT: usize = 10;

/// What an authoring mode edits, for its Recent and Demo submenus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Map,
    Prop,
    Animation,
    Material,
    Weapon,
    Lang,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentLists {
    pub map: Vec<PathBuf>,
    pub prop: Vec<PathBuf>,
    pub animation: Vec<PathBuf>,
    pub material: Vec<PathBuf>,
    pub weapon: Vec<PathBuf>,
    pub lang: Vec<PathBuf>,
}

impl RecentLists {
    pub fn of(&self, kind: DocumentKind) -> &[PathBuf] {
        match kind {
            DocumentKind::Map => &self.map,
            DocumentKind::Prop => &self.prop,
            DocumentKind::Animation => &self.animation,
            DocumentKind::Material => &self.material,
            DocumentKind::Weapon => &self.weapon,
            DocumentKind::Lang => &self.lang,
        }
    }

    fn of_mut(&mut self, kind: DocumentKind) -> &mut Vec<PathBuf> {
        match kind {
            DocumentKind::Map => &mut self.map,
            DocumentKind::Prop => &mut self.prop,
            DocumentKind::Animation => &mut self.animation,
            DocumentKind::Material => &mut self.material,
            DocumentKind::Weapon => &mut self.weapon,
            DocumentKind::Lang => &mut self.lang,
        }
    }

    /// Puts `path` first, dropping an earlier entry for the same file however it was spelled.
    pub fn push(&mut self, kind: DocumentKind, path: &Path) {
        let path = tidy(path);
        let list = self.of_mut(kind);
        list.retain(|other| !same_file(other, &path));
        list.insert(0, path);
        list.truncate(KEPT);
    }
}

#[derive(Resource, Debug, Clone, Default)]
pub struct RecentFiles {
    pub lists: RecentLists,
    /// Where the lists are written. `None` writes nothing, which is what tests get.
    file: Option<PathBuf>,
}

pub fn path() -> PathBuf {
    Path::new(crate::common::assets::USER_PACK).join("recent.toml")
}

impl RecentFiles {
    pub fn load() -> Self {
        let file = path();
        let lists = match std::fs::read_to_string(&file) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                warn!("{} will not read, so the recent files start empty: {e}", file.display());
                RecentLists::default()
            }),
            Err(_) => RecentLists::default(),
        };
        Self { lists, file: Some(file) }
    }

    pub fn of(&self, kind: DocumentKind) -> &[PathBuf] {
        self.lists.of(kind)
    }

    pub fn saved(&mut self, kind: DocumentKind, path: &Path) {
        self.lists.push(kind, path);
        self.write();
    }

    /// Keeps a renamed file in its place on the list. Asked after the rename, so `from` is gone
    /// and compared as spelled.
    pub fn renamed(&mut self, kind: DocumentKind, from: &Path, to: &Path) {
        let from = tidy(from);
        let Some(entry) = self.lists.of_mut(kind).iter_mut().find(|entry| **entry == from) else { return };
        *entry = tidy(to);
        self.write();
    }

    fn write(&self) {
        let Some(file) = &self.file else { return };
        let written = toml::to_string_pretty(&self.lists)
            .map_err(|e| e.to_string())
            .and_then(|text| std::fs::write(file, text).map_err(|e| e.to_string()));
        if let Err(e) = written {
            warn!("Could not write {}: {e}", file.display());
        }
    }
}

/// For plugins that stand alone in tests.
pub fn ensure(app: &mut App) {
    if !app.world().contains_resource::<RecentFiles>() {
        app.init_resource::<RecentFiles>();
    }
}

/// Relative to the working directory where it can be, as the packs' own paths are.
fn tidy(path: &Path) -> PathBuf {
    std::env::current_dir()
        .ok()
        .and_then(|here| path.strip_prefix(&here).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_goes_first_once_and_the_list_is_capped() {
        let mut lists = RecentLists::default();
        for n in 0..KEPT + 3 {
            lists.push(DocumentKind::Prop, Path::new(&format!("nowhere/{n}.gpp")));
        }
        lists.push(DocumentKind::Prop, Path::new("nowhere/5.gpp"));
        let props = lists.of(DocumentKind::Prop);
        assert_eq!(props.len(), KEPT);
        assert_eq!(props[0], Path::new("nowhere/5.gpp"));
        assert_eq!(props.iter().filter(|p| *p == Path::new("nowhere/5.gpp")).count(), 1);
        assert!(lists.of(DocumentKind::Map).is_empty(), "one mode's saves are not another's");
    }

    #[test]
    fn the_lists_survive_the_file() {
        let mut lists = RecentLists::default();
        lists.push(DocumentKind::Lang, Path::new("assets/user/lang/xx-XX.toml"));
        let text = toml::to_string_pretty(&lists).unwrap();
        assert_eq!(toml::from_str::<RecentLists>(&text).unwrap(), lists);
        assert_eq!(toml::from_str::<RecentLists>("").unwrap(), RecentLists::default());
    }
}
