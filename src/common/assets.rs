//! Where a pack's files come from.
//!
//! One trait with two methods: `read` a file, and `list` a folder's files.
//! Nothing behind a `fetch` can enumerate a directory, so a browser build
//! answers `list` from a manifest written with the pack; loaders that know
//! their file names (`loadouts.toml`) still name them.
//!
//! [`NativeFiles`] is the only `std::fs` in the runtime path besides the
//! native-only thumbnail cache; the browser build replaces it.
//! [`crate::common::lang`] does not use this yet: it reads through a
//! `LazyLock` before Bevy exists.

use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::common::resource::session::{is_session_pack, SessionPack, SESSION_PACK};

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

    /// The names of the files directly in `pack/folder`, in any order; empty when there is no
    /// such folder.
    fn list(&self, pack: &Path, folder: &str) -> Vec<String>;

    /// The file's size in bytes, where it can be had without reading the file.
    fn size(&self, _pack: &Path, _relative: &str) -> Option<u64> {
        None
    }
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

    fn list(&self, pack: &Path, folder: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(pack.join(folder)) else { return Vec::new() };
        entries
            .filter_map(Result::ok)
            // `path().is_file()` rather than the entry's type, which does not follow a symlink.
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect()
    }

    fn size(&self, pack: &Path, relative: &str) -> Option<u64> {
        std::fs::metadata(pack.join(relative)).ok().filter(|metadata| metadata.is_file()).map(|metadata| metadata.len())
    }
}

/// Where this process reads packs from: the local packs through a source the platform picks once
/// at startup, and, while connected, the server's files as one more pack above them
/// ([`SessionPack`]). Content the game plays from is read over [`Assets::packs`]; the editors list
/// and write [`default_packs`], which never includes the server's.
#[derive(Resource)]
pub struct Assets {
    source: Box<dyn AssetSource>,
    session: Option<SessionPack>,
}

impl Default for Assets {
    fn default() -> Self {
        Self::new(Box::new(NativeFiles))
    }
}

impl AssetSource for Assets {
    /// A path the server's manifest lists is the server's alone: no local pack answers for it, so
    /// an object not yet stored reads as missing rather than as a local file of another version.
    fn read(&self, pack: &Path, relative: &str) -> Result<Vec<u8>, AssetError> {
        match (is_session_pack(pack), &self.session) {
            (true, Some(session)) => session.read(relative),
            (true, None) => Err(AssetError::NotFound(format!("{relative}: no server's files are mounted"))),
            (false, Some(session)) if session.lists(relative) => {
                Err(AssetError::NotFound(format!("{}/{relative}: the server's copy is used instead", pack.display())))
            }
            (false, _) => self.source.read(pack, relative),
        }
    }

    fn size(&self, pack: &Path, relative: &str) -> Option<u64> {
        match (is_session_pack(pack), &self.session) {
            (true, Some(session)) => session.size(relative),
            (true, None) => None,
            (false, Some(session)) if session.lists(relative) => None,
            (false, _) => self.source.size(pack, relative),
        }
    }

    fn list(&self, pack: &Path, folder: &str) -> Vec<String> {
        match (is_session_pack(pack), &self.session) {
            (true, Some(session)) => session.list(folder),
            (true, None) => Vec::new(),
            (false, _) => self.source.list(pack, folder),
        }
    }
}

impl Assets {
    pub fn new(source: Box<dyn AssetSource>) -> Self {
        Self { source, session: None }
    }

    pub fn read(&self, pack: &Path, relative: &str) -> Result<Vec<u8>, AssetError> {
        AssetSource::read(self, pack, relative)
    }

    pub fn list(&self, pack: &Path, folder: &str) -> Vec<String> {
        AssetSource::list(self, pack, folder)
    }

    pub fn size(&self, pack: &Path, relative: &str) -> Option<u64> {
        AssetSource::size(self, pack, relative)
    }

    /// Lowest priority first: the local packs, then the server's while one is mounted.
    pub fn packs(&self) -> Vec<PathBuf> {
        let mut packs = default_packs();
        if self.session.is_some() {
            packs.push(PathBuf::from(SESSION_PACK));
        }
        packs
    }

    /// What the gameplay tables (weapons, loadouts, classes, taunts, modes, layers, body clips) are
    /// built from: the server's files alone while one is mounted, so a file only this machine has
    /// is not in the match; the local packs otherwise.
    pub fn match_packs(&self) -> Vec<PathBuf> {
        match self.session {
            Some(_) => vec![PathBuf::from(SESSION_PACK)],
            None => default_packs(),
        }
    }

    /// The server's files take precedence over every local pack until [`Assets::unmount`].
    pub fn mount(&mut self, session: SessionPack) {
        self.session = Some(session);
    }

    pub fn unmount(&mut self) -> Option<SessionPack> {
        self.session.take()
    }

    pub fn session(&self) -> Option<&SessionPack> {
        self.session.as_ref()
    }

    /// `relative` in `pack` as the `AssetServer` names it: from [`ASSET_ROOT`] for a local pack,
    /// from the object store for the server's. `None` for a pack outside both.
    pub fn asset_path(&self, pack: &Path, relative: &str) -> Option<String> {
        match is_session_pack(pack) {
            true => self.session.as_ref()?.asset_path(relative),
            false => asset_path(pack, relative),
        }
    }

    pub fn stems(&self, pack: &Path, folder: &str, extension: &str) -> Vec<String> {
        let suffix = format!(".{extension}");
        let mut stems: Vec<String> = self
            .list(pack, folder)
            .into_iter()
            .filter_map(|name| name.strip_suffix(&suffix).filter(|stem| !stem.is_empty()).map(str::to_owned))
            .collect();
        stems.sort();
        stems
    }

    /// The highest-priority pack's copy, so a pack overriding a file wins.
    pub fn read_from_packs(&self, relative: &str) -> Option<Vec<u8>> {
        let mut found = None;
        for pack in self.packs() {
            match self.read(&pack, relative) {
                Ok(bytes) => found = Some(bytes),
                Err(AssetError::NotFound(_)) => {}
                Err(e) => warn!("{e}; using a lower pack's copy, if any"),
            }
        }
        found
    }
}

/// Where Bevy's `AssetServer` reads from, which every pack sits under.
pub const ASSET_ROOT: &str = "assets";
/// Where somebody's own unsorted work goes: a pack like any other, last so it
/// overrides the shipped one, and gitignored.
pub const USER_PACK: &str = "assets/user";

/// Made up front so a save dialog for a new document has somewhere to open, and so somebody adding
/// pictures can see where they go.
const USER_PACK_FOLDERS: [&str; 4] = [
    crate::constants::BLUEPRINTS_DIR,
    crate::prop::document::PROPS_DIR,
    crate::common::weapon_file::WEAPONS_DIR,
    crate::constants::TITLE_BACKGROUNDS_DIR,
];

/// `relative`, a file in `pack`, as the `AssetServer` names it from [`ASSET_ROOT`]; `None` for a
/// pack outside it.
pub fn asset_path(pack: &Path, relative: &str) -> Option<String> {
    let pack = pack.strip_prefix(ASSET_ROOT).ok()?;
    let pack = pack.to_str()?.replace('\\', "/");
    Some(match pack.is_empty() {
        true => relative.to_owned(),
        false => format!("{pack}/{relative}"),
    })
}

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
