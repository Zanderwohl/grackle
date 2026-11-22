//! Deleted pack files go to `cache/trash/{kind}/` rather than away, and are emptied from there
//! once they have been in it [`KEPT_DAYS`]. Authoring side only: the runtime never deletes.
//!
//! A trashed file is named `{seconds since the epoch}_{its own name}`, the time it was trashed,
//! since a move keeps the file's own modified time.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bevy::prelude::*;

use crate::common::recent_files::DocumentKind;

pub const TRASH_DIR: &str = "cache/trash";
pub const KEPT_DAYS: u64 = 30;

impl DocumentKind {
    /// Its folder under [`TRASH_DIR`].
    pub fn trash_key(self) -> &'static str {
        match self {
            DocumentKind::Map => "map",
            DocumentKind::Prop => "prop",
            DocumentKind::Animation => "animation",
            DocumentKind::Material => "material",
            DocumentKind::Weapon => "weapon",
            DocumentKind::Lang => "lang",
        }
    }
}

pub fn trash(kind: DocumentKind, path: &Path) -> Result<PathBuf, String> {
    trash_into(&Path::new(TRASH_DIR).join(kind.trash_key()), path, SystemTime::now())
}

fn trash_into(folder: &Path, path: &Path, now: SystemTime) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let name = path.file_name().ok_or_else(|| format!("{} names no file", path.display()))?.to_string_lossy();
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let mut to = folder.join(format!("{seconds}_{name}"));
    // Two files of one name trashed in the same second.
    let mut again = 1;
    while to.exists() {
        again += 1;
        to = folder.join(format!("{seconds}_{again}_{name}"));
    }
    if std::fs::rename(path, &to).is_err() {
        // Across filesystems a rename fails.
        std::fs::copy(path, &to).map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    info!("Moved {} to the trash at {}", path.display(), to.display());
    Ok(to)
}

/// When `path` was trashed, read off its name; its modified time for a file named otherwise.
fn trashed_at(path: &Path) -> Option<SystemTime> {
    let name = path.file_name()?.to_str()?;
    match name.split_once('_').and_then(|(seconds, _)| seconds.parse::<u64>().ok()) {
        Some(seconds) => Some(UNIX_EPOCH + Duration::from_secs(seconds)),
        None => std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok(),
    }
}

/// Deletes everything under `root` trashed more than `kept` before `now`. Returns what it could
/// not delete.
fn empty(root: &Path, kept: Duration, now: SystemTime) -> Vec<String> {
    let Ok(kinds) = std::fs::read_dir(root) else { return Vec::new() };
    let mut problems = Vec::new();
    for folder in kinds.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_dir()) {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_file()) {
            let old = trashed_at(&path).is_some_and(|at| now.duration_since(at).is_ok_and(|age| age > kept));
            if old && let Err(e) = std::fs::remove_file(&path) {
                problems.push(format!("Could not empty {} from the trash: {e}", path.display()));
            }
        }
    }
    problems
}

/// At startup.
pub fn empty_old_trash() {
    let kept = Duration::from_secs(KEPT_DAYS * 24 * 60 * 60);
    for problem in empty(Path::new(TRASH_DIR), kept, SystemTime::now()) {
        warn!("{problem}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("grackle_trash_{name}_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_trashed_file_keeps_its_name_and_is_emptied_only_once_old() {
        let root = scratch("ages");
        let file = root.join("crate.gpp");
        std::fs::write(&file, "x").unwrap();
        let trash = root.join("trash");
        let day = Duration::from_secs(24 * 60 * 60);
        let then = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let moved = trash_into(&trash.join("prop"), &file, then).unwrap();
        assert!(!file.exists(), "the file was left where it was");
        assert!(moved.file_name().unwrap().to_str().unwrap().ends_with("_crate.gpp"));

        std::fs::write(&file, "y").unwrap();
        let second = trash_into(&trash.join("prop"), &file, then).unwrap();
        assert_ne!(moved, second, "a second file of one name overwrote the first");

        assert!(empty(&trash, day * 30, then + day * 29).is_empty());
        assert!(moved.exists(), "a file 29 days in the trash was emptied");
        empty(&trash, day * 30, then + day * 31);
        assert!(!moved.exists() && !second.exists(), "a file 31 days in the trash was kept");
        std::fs::remove_dir_all(&root).ok();
    }
}
