//! A pack's `pack.toml`: who made it, and for a pack kept from a server, where it came from and
//! what each file was. See "Authors are per resource" and "Keeping foreign content" in
//! documentation/resource-streaming.md.
//!
//! **A kept pack is not in [`default_packs`](crate::common::assets::default_packs)**: its files
//! would otherwise join every local game's catalog and shadow local files by name. It is reached
//! only by opening one of its maps, whose lock mounts the files it names
//! ([`crate::common::resource::lock`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::assets::{AssetError, AssetSource, ASSET_ROOT, USER_PACK};

pub const PACK_FILE: &str = "pack.toml";

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct PackInfo {
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    /// By path inside the pack.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, FileOrigin>,
}

/// Where a kept pack came from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Origin {
    /// Always "kept" so far.
    pub kind: String,
    pub server: String,
    /// When it was kept, local time, RFC 3339.
    pub kept: String,
}

pub const KEPT: &str = "kept";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct FileOrigin {
    /// Its BLAKE3 hash in hex, as it was kept.
    pub hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
}

impl PackInfo {
    /// Empty when the pack has none; one that will not parse is warned of and read as empty.
    pub fn read(source: &dyn AssetSource, pack: &Path) -> PackInfo {
        match source.read(pack, PACK_FILE) {
            Ok(bytes) => match std::str::from_utf8(&bytes).map_err(|e| e.to_string()).and_then(|text| toml::from_str(text).map_err(|e| e.to_string())) {
                Ok(info) => info,
                Err(e) => {
                    warn!("{}/{PACK_FILE} will not read, so its pack is taken to credit nobody: {e}", pack.display());
                    PackInfo::default()
                }
            },
            Err(AssetError::NotFound(_)) => PackInfo::default(),
            Err(e) => {
                warn!("{e}");
                PackInfo::default()
            }
        }
    }

    pub fn is_kept(&self) -> bool {
        self.origin.as_ref().is_some_and(|origin| origin.kind == KEPT)
    }

    /// The file's own authors where the pack names them, else the pack's.
    pub fn authors_of(&self, path: &str) -> Vec<String> {
        match self.files.get(path) {
            Some(file) if !file.authors.is_empty() => file.authors.clone(),
            _ => self.authors.clone(),
        }
    }
}

/// Native only, as listing the packs is.
#[cfg(not(target_arch = "wasm32"))]
pub fn is_kept_pack(pack: &Path) -> bool {
    PackInfo::read(&crate::common::assets::NativeFiles, pack).is_kept()
}

#[cfg(target_arch = "wasm32")]
pub fn is_kept_pack(_: &Path) -> bool {
    false
}

/// Every kept pack under `assets/`, by name.
#[cfg(not(target_arch = "wasm32"))]
pub fn kept_packs() -> Vec<PathBuf> {
    let mut kept: Vec<PathBuf> = std::fs::read_dir(ASSET_ROOT)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && is_kept_pack(path))
        .collect();
    kept.sort();
    kept
}

#[cfg(target_arch = "wasm32")]
pub fn kept_packs() -> Vec<PathBuf> {
    Vec::new()
}

/// The pack a file is in, as `assets/{pack}`.
pub fn pack_of(file: &Path) -> Option<PathBuf> {
    let (pack, _) = crate::common::cache_path::in_pack(file)?;
    Some(Path::new(ASSET_ROOT).join(pack))
}

/// Whether `file` is in a kept pack, which the editors open read-only.
pub fn in_a_kept_pack(file: &Path) -> bool {
    pack_of(file).is_some_and(|pack| is_kept_pack(&pack))
}

/// Gives the user pack a `pack.toml` crediting the player, once they have a name. Never rewrites
/// one that is there.
#[cfg(not(target_arch = "wasm32"))]
pub fn credit_the_player(player: Option<Res<crate::common::player_settings::PlayerSettings>>) {
    let Some(player) = player else { return };
    let name = player.name.trim();
    if !player.is_changed() || name.is_empty() {
        return;
    }
    let file = Path::new(USER_PACK).join(PACK_FILE);
    if file.exists() {
        return;
    }
    let info = PackInfo { authors: vec![name.to_owned()], ..default() };
    let written = toml::to_string(&info).map_err(|e| e.to_string()).and_then(|text| std::fs::write(&file, text).map_err(|e| e.to_string()));
    match written {
        Ok(()) => info!("Credited {name} in {}", file.display()),
        Err(e) => warn!("Could not write {}: {e}", file.display()),
    }
}

#[cfg(target_arch = "wasm32")]
pub fn credit_the_player() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_own_authors_come_before_its_packs() {
        let info: PackInfo = toml::from_str(
            r#"
            authors = ["the pack"]
            [origin]
            kind = "kept"
            server = "Alice's"
            kept = "2026-11-23T10:00:00+00:00"
            [files."props/crate.gpp"]
            hash = "ab"
            authors = ["Bob"]
            [files."props/barrel.gpp"]
            hash = "cd"
            "#,
        )
        .unwrap();
        assert!(info.is_kept());
        assert_eq!(info.authors_of("props/crate.gpp"), ["Bob"]);
        assert_eq!(info.authors_of("props/barrel.gpp"), ["the pack"]);
        assert_eq!(toml::from_str::<PackInfo>(&toml::to_string(&info).unwrap()).unwrap(), info);
    }

    #[test]
    fn the_shipped_packs_are_not_kept() {
        assert!(!is_kept_pack(Path::new("assets/default")));
        assert_eq!(PackInfo::read(&crate::common::assets::NativeFiles, Path::new("assets/default")).authors, ["The Grackle contributors"]);
    }
}
