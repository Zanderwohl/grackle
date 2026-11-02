//! A bake on disk: `cache/{folder}/{hash}.bin`, as postcard. Keyed by a hash of everything the
//! bake reads, so an unchanged map opens with its bake rather than baking it again. Derived and
//! safe to delete; native only, like the thumbnails.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Cached<T> {
    hash: u64,
    value: T,
}

#[cfg(not(target_arch = "wasm32"))]
mod disk {
    use std::path::PathBuf;

    use bevy::prelude::*;
    use bevy::tasks::IoTaskPool;
    use serde::de::DeserializeOwned;

    use super::*;

    /// Tests keep out of the real cache, in a directory per process, emptied on first use in case
    /// an earlier process had the same id.
    fn dir(folder: &str) -> PathBuf {
        if cfg!(test) {
            static EMPTIED: std::sync::Once = std::sync::Once::new();
            let root = std::env::temp_dir().join(format!("grackle-cache-{}", std::process::id()));
            EMPTIED.call_once(|| {
                let _ = std::fs::remove_dir_all(&root);
            });
            root.join(folder)
        } else {
            PathBuf::from("cache").join(folder)
        }
    }

    fn path(folder: &str, hash: u64) -> PathBuf {
        dir(folder).join(format!("{hash:016x}.bin"))
    }

    /// `None` on a miss, or a file that does not decode: one left half-written, or from an older
    /// format.
    pub fn read<T: DeserializeOwned>(folder: &str, hash: u64) -> Option<T> {
        let bytes = std::fs::read(path(folder, hash)).ok()?;
        match postcard::from_bytes::<Cached<T>>(&bytes) {
            Ok(cached) if cached.hash == hash => Some(cached.value),
            Ok(_) => None,
            Err(e) => {
                warn!("Ignoring {}: {e}", path(folder, hash).display());
                None
            }
        }
    }

    pub fn write<T: Serialize + Send + 'static>(folder: &'static str, hash: u64, value: T) {
        IoTaskPool::get()
            .spawn(async move {
                if let Err(e) = store(folder, hash, value) {
                    warn!("Could not write {}: {e}", path(folder, hash).display());
                }
            })
            .detach();
    }

    /// Through a temporary file renamed into place, so a reader never sees half.
    pub fn store<T: Serialize>(folder: &str, hash: u64, value: T) -> Result<(), String> {
        let bytes = postcard::to_allocvec(&Cached { hash, value }).map_err(|e| e.to_string())?;
        let (path, partial) = (path(folder, hash), path(folder, hash).with_extension("partial"));
        std::fs::create_dir_all(dir(folder)).map_err(|e| e.to_string())?;
        std::fs::write(&partial, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&partial, &path).map_err(|e| e.to_string())
    }

    #[cfg(test)]
    pub fn spoil(folder: &str, hash: u64) {
        std::fs::write(path(folder, hash), b"not postcard").unwrap();
    }
}

#[cfg(target_arch = "wasm32")]
mod disk {
    use serde::de::DeserializeOwned;

    use super::*;

    pub fn read<T: DeserializeOwned>(_: &str, _: u64) -> Option<T> {
        None
    }

    pub fn write<T: Serialize + Send + 'static>(_: &'static str, _: u64, _: T) {}
}

pub use disk::{read, write};
#[cfg(all(test, not(target_arch = "wasm32")))]
pub use disk::{spoil, store};

#[cfg(test)]
mod tests {
    use super::*;

    /// Breaks if a stored value does not read back under its hash, or a spoiled file is not a miss.
    #[test]
    fn the_cache_reads_back_what_it_stored() {
        let (folder, hash) = ("bake-cache-test", 0x5eed);
        assert!(read::<Vec<u32>>(folder, hash).is_none());
        store(folder, hash, vec![1u32, 2, 3]).unwrap();
        assert_eq!(read::<Vec<u32>>(folder, hash), Some(vec![1, 2, 3]));
        assert!(read::<Vec<u32>>(folder, hash + 1).is_none(), "another hash read this file");
        spoil(folder, hash);
        assert!(read::<Vec<u32>>(folder, hash).is_none(), "a spoiled file read back");
    }
}
