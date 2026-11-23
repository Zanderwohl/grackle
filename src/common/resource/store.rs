//! Foreign files, kept by hash alone. A logical path from a server never becomes a filesystem path
//! here, so a hostile one cannot reach outside the store.

use std::fmt;
use std::path::PathBuf;

use crate::common::resource::ContentHash;

/// Gitignored, beside `cache/` and `logs/`. Not a pack and not a cache: nothing in it overrides a
/// local file outside a session, and a server that has gone cannot send it again.
pub const DOWNLOADS_ROOT: &str = "downloads";
pub const OBJECTS_DIR: &str = "downloads/objects";

/// The Bevy asset source that reads [`OBJECTS_DIR`], for files the `AssetServer` loads (music).
pub const OBJECT_SOURCE: &str = "object";

#[derive(Debug)]
pub enum StoreError {
    /// The bytes do not hash to the name they were offered under.
    Mismatch { expected: ContentHash, got: ContentHash },
    Missing(ContentHash),
    Io(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Mismatch { expected, got } => {
                write!(f, "the bytes hash to {} rather than {}", got.short(), expected.short())
            }
            StoreError::Missing(hash) => write!(f, "no object {} is stored", hash.short()),
            StoreError::Io(why) => f.write_str(why),
        }
    }
}

/// An object is named by its hash and the extension of the path it was sent under, so a file the
/// `AssetServer` loads keeps the extension its loader is chosen by.
pub trait ObjectStore: Send + Sync + 'static {
    /// Refuses bytes that do not hash to `hash`.
    fn put(&self, hash: ContentHash, extension: &str, bytes: &[u8]) -> Result<(), StoreError>;
    /// Checks the bytes again, so a file damaged on disk reads as missing rather than as content.
    fn get(&self, hash: ContentHash, extension: &str) -> Result<Vec<u8>, StoreError>;
    fn has(&self, hash: ContentHash, extension: &str) -> bool;
}

/// `ab/cdef….ext`, split on the first byte so no folder holds every object.
pub fn object_name(hash: ContentHash, extension: &str) -> String {
    let hex = hash.to_string();
    format!("{}/{}.{extension}", &hex[..2], &hex[2..])
}

/// The `AssetServer` path of a stored object.
pub fn object_asset_path(hash: ContentHash, extension: &str) -> String {
    format!("{OBJECT_SOURCE}://{}", object_name(hash, extension))
}

/// [`OBJECTS_DIR`] on disk. Native only; a browser keeps the same layout in IndexedDB.
#[cfg(not(target_arch = "wasm32"))]
pub struct DiskObjects {
    root: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl DiskObjects {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, hash: ContentHash, extension: &str) -> PathBuf {
        self.root.join(object_name(hash, extension))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for DiskObjects {
    fn default() -> Self {
        Self::new(OBJECTS_DIR)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ObjectStore for DiskObjects {
    fn put(&self, hash: ContentHash, extension: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let got = ContentHash::of(bytes);
        if got != hash {
            return Err(StoreError::Mismatch { expected: hash, got });
        }
        let path = self.path(hash, extension);
        if path.is_file() {
            return Ok(());
        }
        let io = |e: std::io::Error| StoreError::Io(format!("{}: {e}", path.display()));
        std::fs::create_dir_all(path.parent().expect("an object has a folder")).map_err(io)?;
        // Written beside and renamed into place, so a reader never sees half a file. Named per
        // call, since two paths with the same bytes may be stored at once.
        static PUTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let put = PUTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let partial = path.with_extension(format!("{extension}.part-{}-{put}", std::process::id()));
        std::fs::write(&partial, bytes).map_err(io)?;
        match std::fs::rename(&partial, &path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&partial);
                match path.is_file() {
                    true => Ok(()),
                    false => Err(io(e)),
                }
            }
        }
    }

    fn get(&self, hash: ContentHash, extension: &str) -> Result<Vec<u8>, StoreError> {
        let path = self.path(hash, extension);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(StoreError::Missing(hash)),
            Err(e) => return Err(StoreError::Io(format!("{}: {e}", path.display()))),
        };
        let got = ContentHash::of(&bytes);
        if got != hash {
            bevy::log::warn!("{} is damaged, so it is deleted and will be fetched again", path.display());
            let _ = std::fs::remove_file(&path);
            return Err(StoreError::Missing(hash));
        }
        Ok(bytes)
    }

    fn has(&self, hash: ContentHash, extension: &str) -> bool {
        self.path(hash, extension).is_file()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) mod tests {
    use super::*;

    pub fn scratch_store(name: &str) -> (DiskObjects, PathBuf) {
        let root = std::env::temp_dir().join(format!("grackle-objects-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (DiskObjects::new(&root), root)
    }

    #[test]
    fn an_object_is_kept_by_its_hash_and_checked_both_ways() {
        let (store, root) = scratch_store("both_ways");
        let hash = ContentHash::of(b"crate");
        assert!(matches!(store.put(hash, "gpp", b"barrel"), Err(StoreError::Mismatch { .. })));
        assert!(!store.has(hash, "gpp"));

        store.put(hash, "gpp", b"crate").unwrap();
        assert_eq!(store.get(hash, "gpp").unwrap(), b"crate");
        let on_disk = root.join(object_name(hash, "gpp"));
        assert!(on_disk.is_file(), "{} was not written", on_disk.display());

        std::fs::write(&on_disk, b"tampered").unwrap();
        assert!(matches!(store.get(hash, "gpp"), Err(StoreError::Missing(_))));
        assert!(!on_disk.exists(), "a damaged object was kept");
    }

    #[test]
    fn an_object_path_keeps_its_extension() {
        let hash = ContentHash::of(b"");
        assert_eq!(object_asset_path(hash, "mp3"), format!("object://af/{}.mp3", &hash.to_string()[2..]));
    }
}
