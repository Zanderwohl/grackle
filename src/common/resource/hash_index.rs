//! The hash of each local pack file, kept by its size and modified time so a join need not hash
//! every file again. Derived and safe to delete, as everything in `cache/` is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::common::resource::ContentHash;

pub const DEFAULT_PATH: &str = "cache/hashes.bin";

/// Seconds. A file modified this close to when it was hashed may have been written again within
/// the same timestamp, so its hash is not trusted (git's "racy" entries).
const RACY_SECONDS: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Indexed {
    size: u64,
    /// Since the Unix epoch.
    modified: (u64, u32),
    hashed_at: u64,
    hash: ContentHash,
}

#[derive(Default, Serialize, Deserialize)]
struct OnDisk {
    files: BTreeMap<String, Indexed>,
}

pub struct HashIndex {
    file: PathBuf,
    entries: OnDisk,
    changed: bool,
}

fn since_epoch(time: SystemTime) -> (u64, u32) {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    (since.as_secs(), since.subsec_nanos())
}

impl HashIndex {
    /// Empty when `file` is missing or will not decode.
    pub fn open(file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        let entries = std::fs::read(&file)
            .ok()
            .and_then(|bytes| match postcard::from_bytes(&bytes) {
                Ok(entries) => Some(entries),
                Err(e) => {
                    bevy::log::warn!("Ignoring {}: {e}", file.display());
                    None
                }
            })
            .unwrap_or_default();
        Self { file, entries, changed: false }
    }

    /// Hashed again only when its size or modified time differs from what was indexed.
    pub fn hash_of(&mut self, path: &Path) -> std::io::Result<(ContentHash, u64)> {
        let metadata = std::fs::metadata(path)?;
        let size = metadata.len();
        let modified = since_epoch(metadata.modified()?);
        let key = path.to_string_lossy().into_owned();
        if let Some(known) = self.entries.files.get(&key)
            && known.size == size
            && known.modified == modified
            && modified.0 + RACY_SECONDS < known.hashed_at
        {
            return Ok((known.hash, size));
        }
        let hash = ContentHash::of(&std::fs::read(path)?);
        let hashed_at = since_epoch(SystemTime::now()).0;
        self.entries.files.insert(key, Indexed { size, modified, hashed_at, hash });
        self.changed = true;
        Ok((hash, size))
    }

    /// Drops files that are gone, then writes the index if anything changed.
    pub fn save(&mut self) -> std::io::Result<()> {
        let before = self.entries.files.len();
        self.entries.files.retain(|path, _| Path::new(path).is_file());
        if !self.changed && self.entries.files.len() == before {
            return Ok(());
        }
        if let Some(folder) = self.file.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let bytes = postcard::to_stdvec(&self.entries).map_err(std::io::Error::other)?;
        std::fs::write(&self.file, bytes)?;
        self.changed = false;
        Ok(())
    }

    #[cfg(test)]
    fn hashed_at(&self, path: &Path) -> Option<u64> {
        self.entries.files.get(&*path.to_string_lossy()).map(|known| known.hashed_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("grackle-hash-index-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn a_hash_survives_a_save_and_a_change_is_seen() {
        let folder = scratch("survives");
        let file = folder.join("crate.gpp");
        std::fs::write(&file, b"one").unwrap();

        let mut index = HashIndex::open(folder.join("hashes.bin"));
        assert_eq!(index.hash_of(&file).unwrap(), (ContentHash::of(b"one"), 3));
        index.save().unwrap();

        let mut index = HashIndex::open(folder.join("hashes.bin"));
        assert!(index.hashed_at(&file).is_some(), "the index did not come back from disk");
        std::fs::write(&file, b"three").unwrap();
        assert_eq!(index.hash_of(&file).unwrap(), (ContentHash::of(b"three"), 5));
    }

    /// The same size written within the same second as it was hashed is hashed again, not trusted.
    #[test]
    fn a_racy_entry_is_hashed_again() {
        let folder = scratch("racy");
        let file = folder.join("a.svg");
        std::fs::write(&file, b"aaa").unwrap();
        let mut index = HashIndex::open(folder.join("hashes.bin"));
        index.hash_of(&file).unwrap();
        std::fs::write(&file, b"bbb").unwrap();
        assert_eq!(index.hash_of(&file).unwrap().0, ContentHash::of(b"bbb"));
    }

    #[test]
    fn a_file_that_is_gone_leaves_the_index() {
        let folder = scratch("gone");
        let file = folder.join("b.svg");
        std::fs::write(&file, b"b").unwrap();
        let mut index = HashIndex::open(folder.join("hashes.bin"));
        index.hash_of(&file).unwrap();
        std::fs::remove_file(&file).unwrap();
        index.save().unwrap();
        assert!(HashIndex::open(folder.join("hashes.bin")).hashed_at(&file).is_none());
    }
}
