//! The server's files as a pack: its manifest, read out of the object store, mounted above every
//! local pack while connected.

use std::path::Path;

use crate::common::assets::AssetError;
use crate::common::resource::store::{object_asset_path, ObjectStore};
use crate::common::resource::{LogicalPath, Manifest};

/// The pack path the session pack is listed under. No folder has this name, and nothing writes to
/// it.
pub const SESSION_PACK: &str = "<server>";

pub fn is_session_pack(pack: &Path) -> bool {
    pack == Path::new(SESSION_PACK)
}

pub struct SessionPack {
    /// For messages.
    pub label: String,
    pub manifest: Manifest,
    store: Box<dyn ObjectStore>,
}

impl SessionPack {
    pub fn new(label: String, manifest: Manifest, store: Box<dyn ObjectStore>) -> Self {
        Self { label, manifest, store }
    }

    pub fn read(&self, relative: &str) -> Result<Vec<u8>, AssetError> {
        let named = || format!("{relative} from {}", self.label);
        let path = LogicalPath::new(relative).map_err(|_| AssetError::NotFound(named()))?;
        let entry = self.manifest.get(&path).ok_or_else(|| AssetError::NotFound(named()))?;
        self.store
            .get(entry.key.hash, path.extension().unwrap_or_default())
            .map_err(|e| AssetError::Unreadable(format!("{}: {e}", named())))
    }

    /// Whether the manifest lists `relative`, which makes it the server's alone.
    pub fn lists(&self, relative: &str) -> bool {
        LogicalPath::new(relative).is_ok_and(|path| self.manifest.get(&path).is_some())
    }

    pub fn size(&self, relative: &str) -> Option<u64> {
        Some(self.manifest.get(&LogicalPath::new(relative).ok()?)?.size)
    }

    pub fn list(&self, folder: &str) -> Vec<String> {
        let prefix = format!("{folder}/");
        self.manifest
            .entries
            .iter()
            .filter_map(|entry| entry.key.path.as_str().strip_prefix(&prefix))
            .filter(|name| !name.contains('/'))
            .map(str::to_owned)
            .collect()
    }

    /// Every file under `folder`, at any depth, as paths inside the pack.
    pub fn files_under(&self, folder: &str) -> Vec<String> {
        let prefix = format!("{folder}/");
        self.manifest
            .entries
            .iter()
            .map(|entry| entry.key.path.as_str())
            .filter(|path| path.starts_with(&prefix))
            .map(str::to_owned)
            .collect()
    }

    pub fn asset_path(&self, relative: &str) -> Option<String> {
        let path = LogicalPath::new(relative).ok()?;
        let entry = self.manifest.get(&path)?;
        Some(object_asset_path(entry.key.hash, path.extension()?))
    }

    pub fn lacking(&self) -> Vec<&LogicalPath> {
        self.manifest
            .entries
            .iter()
            .filter(|entry| !self.store.has(entry.key.hash, entry.key.path.extension().unwrap_or_default()))
            .map(|entry| &entry.key.path)
            .collect()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::common::assets::Assets;
    use crate::common::resource::store::tests::scratch_store;
    use crate::common::resource::store::ObjectStore;
    use crate::common::resource::{ContentHash, ManifestEntry, ResourceKey};

    /// The server's pistol: the shipped one with another deploy time.
    fn their_pistol() -> String {
        let ours = std::fs::read_to_string("assets/default/weapons/pistol.toml").unwrap();
        let line = ours.lines().find(|line| line.starts_with("deploy")).expect("the pistol states its deploy").to_owned();
        ours.replacen(&line, "deploy = 1.75", 1)
    }

    fn mounted(name: &str, files: &[(&str, &str)]) -> Assets {
        let (store, _) = scratch_store(name);
        let mut entries: Vec<ManifestEntry> = files
            .iter()
            .map(|(path, text)| {
                let path = LogicalPath::new(path).unwrap();
                let hash = ContentHash::of(text.as_bytes());
                store.put(hash, path.extension().unwrap(), text.as_bytes()).unwrap();
                ManifestEntry { key: ResourceKey { path, hash }, size: text.len() as u64, pack: "user".into(), authors: Vec::new() }
            })
            .collect();
        entries.sort_by(|a, b| a.key.path.cmp(&b.key.path));
        let mut assets = Assets::default();
        assets.mount(SessionPack::new("a server".into(), Manifest { entries }, Box::new(store)));
        assets
    }

    #[test]
    fn the_servers_file_wins_until_it_is_unmounted() {
        let pistol = their_pistol();
        let mut assets = mounted("wins", &[("weapons/pistol.toml", &pistol)]);
        assert_eq!(assets.packs().last().map(|pack| is_session_pack(pack)), Some(true));
        assert_eq!(assets.read_from_packs("weapons/pistol.toml").unwrap(), pistol.as_bytes());
        assert_eq!(assets.list(Path::new(SESSION_PACK), "weapons"), vec!["pistol.toml".to_owned()]);

        let catalog = crate::common::weapon_file::load_catalog(&assets, &assets.match_packs());
        let keys: Vec<&str> = catalog.weapons().iter().map(|weapon| weapon.key.as_str()).collect();
        assert_eq!(keys, ["pistol"], "the catalog held a weapon the server did not send");
        assert_eq!(catalog.weapons()[0].deploy, 1.75, "the catalog was not built from the server's pistol");

        assert!(assets.unmount().is_some());
        assert!(!assets.packs().iter().any(|pack| is_session_pack(pack)));
        assert_ne!(assets.read_from_packs("weapons/pistol.toml").unwrap(), pistol.as_bytes());
    }

    #[test]
    fn a_file_outside_the_manifest_comes_from_the_local_packs() {
        let assets = mounted("outside", &[("icons/kill_pistol.svg", "<svg/>")]);
        assert!(assets.read(Path::new(SESSION_PACK), "weapons/pistol.toml").is_err());
        assert!(assets.read_from_packs("weapons/pistol.toml").is_some());
        assert!(assets.read(Path::new(SESSION_PACK), "../../etc/passwd").is_err());
    }

    #[test]
    fn a_track_is_loaded_out_of_the_object_store() {
        let assets = mounted("track", &[("music/Theirs/theme.ogg", "not really ogg")]);
        let hash = ContentHash::of(b"not really ogg");
        let path = assets.asset_path(Path::new(SESSION_PACK), "music/Theirs/theme.ogg");
        assert_eq!(path, Some(crate::common::resource::store::object_asset_path(hash, "ogg")));
        let listings = crate::common::attribution::scan_assets(&assets, "music");
        let theirs = listings.last().expect("the server's pack is listed last");
        assert_eq!(theirs.files, vec!["music/Theirs/theme.ogg".to_owned()]);
    }

    /// An object not yet stored is missing, not replaced by the local file of the same path.
    #[test]
    fn a_listed_path_never_falls_back_to_a_local_file() {
        let (store, _) = scratch_store("never_falls_back");
        let path = LogicalPath::new("weapons/pistol.toml").unwrap();
        let entry = ManifestEntry { key: ResourceKey { path, hash: ContentHash::of(b"not stored") }, size: 10, pack: "user".into(), authors: Vec::new() };
        let mut assets = Assets::default();
        assets.mount(SessionPack::new("a server".into(), Manifest { entries: vec![entry] }, Box::new(store)));
        assert_eq!(assets.read_from_packs("weapons/pistol.toml"), None);
        assert!(assets.read(Path::new("assets/default"), "weapons/pistol.toml").is_err());
        assert!(assets.read(Path::new("assets/default"), "weapons/shotgun.toml").is_ok(), "an unlisted file is still local");
    }

    #[test]
    fn a_stored_object_is_not_lacking() {
        let assets = mounted("lacking", &[("icons/kill_pistol.svg", "<svg/>")]);
        assert!(assets.session().unwrap().lacking().is_empty());
    }
}
