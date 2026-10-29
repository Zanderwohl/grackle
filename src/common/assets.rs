//! Where a pack's files come from.
//!
//! One trait with one method, deliberately not `list`: nothing behind a
//! `fetch` can enumerate a directory, and a manifest would be a second
//! description of the pack. A loader that needs several files names them.
//!
//! [`NativeFiles`] is the only `std::fs` in the runtime path besides the
//! native-only thumbnail cache; the browser build replaces it.
//! [`crate::common::lang`] does not use this yet: it reads through a
//! `LazyLock` before Bevy exists.

use std::path::{Path, PathBuf};

use bevy::prelude::*;

/// Both variants carry the path, so a pack author knows which file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetError {
    NotFound(String),
    Unreadable(String),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetError::NotFound(what) => write!(f, "no such asset: {what}"),
            AssetError::Unreadable(what) => write!(f, "could not read asset: {what}"),
        }
    }
}

/// Somewhere a pack's bytes can be got from.
pub trait AssetSource: Send + Sync + 'static {
    /// Missing is an `Err` rather than `Ok(None)` so every caller gets the
    /// message naming the file.
    fn read(&self, pack: &Path, relative: &str) -> Result<Vec<u8>, AssetError>;
}

/// `std::fs`, relative to the working directory, so binaries run from the
/// repo root.
pub struct NativeFiles;

impl AssetSource for NativeFiles {
    fn read(&self, pack: &Path, relative: &str) -> Result<Vec<u8>, AssetError> {
        let path = pack.join(relative);
        let named = path.display().to_string();
        match std::fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(AssetError::NotFound(named))
            }
            Err(e) => Err(AssetError::Unreadable(format!("{named}: {e}"))),
        }
    }
}

/// The source this process reads packs through. Boxed rather than a type
/// parameter on every loader, since the platform picks it once at startup.
#[derive(Resource)]
pub struct Assets(pub Box<dyn AssetSource>);

impl Default for Assets {
    fn default() -> Self {
        Self(Box::new(NativeFiles))
    }
}

impl Assets {
    pub fn read(&self, pack: &Path, relative: &str) -> Result<Vec<u8>, AssetError> {
        self.0.read(pack, relative)
    }

    /// The highest-priority pack's copy, so a pack overriding a file wins.
    pub fn read_from_packs(&self, relative: &str) -> Option<Vec<u8>> {
        let mut found = None;
        for pack in crate::common::lang::default_packs() {
            match self.read(&pack, relative) {
                Ok(bytes) => found = Some(bytes),
                Err(AssetError::NotFound(_)) => {}
                Err(e) => error!("{e}"),
            }
        }
        found
    }
}

/// Where somebody's own unsorted work goes: a pack like any other, last so it
/// overrides the shipped one, and gitignored.
pub const USER_PACK: &str = "assets/user";

/// Made up front so a save dialog for a new document has somewhere to open.
const USER_PACK_FOLDERS: [&str; 2] =
    [crate::constants::BLUEPRINTS_DIR, crate::prop::document::PROPS_DIR];

/// Authoring side only: the runtime never writes a pack.
pub fn create_the_user_pack() {
    for folder in USER_PACK_FOLDERS {
        let path = Path::new(USER_PACK).join(folder);
        if let Err(e) = std::fs::create_dir_all(&path) {
            warn!("Could not create {}: {e}", path.display());
        }
    }
}

/// The pack list until there is a real one; shared with
/// [`crate::common::lang::default_packs`].
pub fn default_packs() -> Vec<PathBuf> {
    crate::common::lang::default_packs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_names_itself() {
        let source = NativeFiles;
        let error = source
            .read(Path::new("assets/default"), "no-such-file.toml")
            .expect_err("a file that does not exist was read");

        match error {
            AssetError::NotFound(what) => {
                assert!(what.contains("no-such-file.toml"), "the error did not name the file: {what}");
                assert!(what.contains("assets/default"), "the error did not name the pack: {what}");
            }
            other => panic!("a missing file reported {other:?}"),
        }
    }

    /// A working directory that is not the repo root fails here rather than as
    /// an empty catalog.
    #[test]
    fn a_file_that_is_there_comes_back() {
        let source = NativeFiles;
        let bytes = source
            .read(Path::new("assets/default"), "lang/en-US.toml")
            .expect("the default pack's reference language is missing");
        assert!(!bytes.is_empty());
    }
}
