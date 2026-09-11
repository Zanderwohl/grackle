//! Where a pack's files come from.
//!
//! One trait with one method, and the method is deliberately **not `list`**.
//! Nothing behind a `fetch` can enumerate a directory, so a wasm build would
//! need a manifest saying what a directory holds — and a manifest is a second
//! description of the pack, which drifts from the first the moment somebody
//! adds a file and forgets. A loader that needs several files names them.
//!
//! The native implementation is `std::fs` and is the only `std::fs` in the
//! runtime path; the browser one is a `wasm_bindgen` shim calling out to JS,
//! and the whole point of the trait is that there is exactly one place to
//! write it. See "Targeting wasm" in `CLAUDE.md`.
//!
//! [`crate::common::lang`] is the obvious second customer and is deliberately
//! **not** ported yet: it reads through a `LazyLock` before Bevy exists, so
//! moving it behind a resource is its own piece of work.

use std::path::{Path, PathBuf};

use bevy::prelude::*;

/// Why a file could not be read.
///
/// Both variants carry the path, because the one thing a pack author needs to
/// know is which file — an error that says only "not found" is an error you
/// have to reproduce to act on.
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
    /// Read `relative` beneath `pack`.
    ///
    /// Missing is an `Err` rather than an `Ok(None)`: every caller so far
    /// wants to say which file and why, and an `Option` would make each of
    /// them re-derive the message.
    fn read(&self, pack: &Path, relative: &str) -> Result<Vec<u8>, AssetError>;
}

/// `std::fs`, relative to the working directory — the same assumption every
/// other asset path in this repo makes, which is why binaries run from the
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

/// The source this process reads packs through.
///
/// A boxed trait object in a resource rather than a type parameter threaded
/// through every loader: the choice is made once, at startup, by the platform,
/// and a generic would put a parameter on every plugin that touches a file for
/// the benefit of one call site.
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
}

/// The pack list until Grackle grows a real one.
///
/// The same stand-in [`crate::common::lang::default_packs`] uses, and
/// deliberately the same shape: when there is a real pack list, both read it.
pub fn default_packs() -> Vec<PathBuf> {
    crate::common::lang::default_packs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one thing a pack author needs from a failure is which file, so the
    /// path is in the error rather than only in a log line somewhere.
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

    /// And a file that is there comes back, through the same call the loaders
    /// make — so a working directory that is not the repo root fails here
    /// rather than as an empty catalogue.
    #[test]
    fn a_file_that_is_there_comes_back() {
        let source = NativeFiles;
        let bytes = source
            .read(Path::new("assets/default"), "lang/en-US.toml")
            .expect("the default pack's reference language is missing");
        assert!(!bytes.is_empty());
    }
}
