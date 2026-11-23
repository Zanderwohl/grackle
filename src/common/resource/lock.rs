//! Kept maps: a server's round map and what it needs copied into a pack of its own, and, while
//! one is open, its lock mounted so its files are found by hash. See stage 5 of
//! documentation/resource-streaming.md.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::common::assets::{AssetSource, Assets as PackAssets, NativeFiles, PacksChanged, ASSET_ROOT};
use crate::common::net::NetRole;
use crate::common::pack_info::{FileOrigin, Origin, PackInfo, KEPT, PACK_FILE};
use crate::common::resource::session::SessionPack;
use crate::common::resource::store::{ObjectStore, StoreError};
use crate::common::resource::sync::{DownloadsRoot, Fetch};
use crate::common::resource::{ContentHash, Manifest, ResourceKind};
use crate::editor::save::Required;

/// A kept pack's files, found by the hash its map's lock names, else in the downloads.
struct KeptObjects {
    pack: PathBuf,
    paths: HashMap<ContentHash, String>,
    downloads: Option<Box<dyn ObjectStore>>,
}

impl ObjectStore for KeptObjects {
    fn put(&self, _: ContentHash, _: &str, _: &[u8]) -> Result<(), StoreError> {
        Err(StoreError::Io("a kept pack is not written through its lock".into()))
    }

    fn get(&self, hash: ContentHash, extension: &str) -> Result<Vec<u8>, StoreError> {
        if let Some(path) = self.paths.get(&hash)
            && let Ok(bytes) = NativeFiles.read(&self.pack, path)
            && ContentHash::of(&bytes) == hash
        {
            return Ok(bytes);
        }
        match &self.downloads {
            Some(downloads) => downloads.get(hash, extension),
            None => Err(StoreError::Missing(hash)),
        }
    }

    fn has(&self, hash: ContentHash, extension: &str) -> bool {
        self.get(hash, extension).is_ok()
    }
}

#[derive(Resource, Default)]
pub struct LockedMap(pub Option<PathBuf>);

/// The lock as a manifest; entries that do not read are left out with a warning.
pub fn manifest_of(requires: &[Required]) -> Result<Manifest, String> {
    let entries = requires
        .iter()
        .filter_map(|required| {
            let entry = required.entry();
            if entry.is_none() {
                warn!("A map's lock names {} with a hash that does not read, so it is left out", required.path);
            }
            entry
        })
        .collect();
    Manifest::checked(entries)
}

/// Only in the map editor and Play: a mounted lock hides the local files it names, which the
/// other editors edit. A client mounts none, since its mounted pack is the server's. Ordered after
/// the client's own unmount, which a role change also makes.
#[allow(clippy::too_many_arguments)]
pub fn lock_kept_maps(
    role: Res<NetRole>,
    mode: Res<State<crate::common::app_mode::AppMode>>,
    file: Option<Res<crate::editor::panels::CurrentFilePath>>,
    root: Res<DownloadsRoot>,
    mut assets: ResMut<PackAssets>,
    mut locked: ResMut<LockedMap>,
    cache: Option<ResMut<crate::prop::baked::PropCache>>,
    mut changed: MessageWriter<PacksChanged>,
    // The panels hold the file path mutably every frame, so its change flag is always set; whether
    // the path is in a kept pack is read once per path.
    mut seen: Local<Option<(PathBuf, bool)>>,
) {
    // A role change unmounted whatever was mounted, so the lock is looked at again.
    if role.is_changed() {
        locked.0 = None;
    }
    let Some(file) = file else { return };
    if !role.is_authority() {
        return;
    }
    let kept = match (&file.path, seen.as_ref()) {
        (Some(path), Some((known, kept))) if known == path => *kept,
        (Some(path), _) => {
            let kept = crate::common::pack_info::in_a_kept_pack(path);
            *seen = Some((path.clone(), kept));
            kept
        }
        (None, _) => false,
    };
    let shown = matches!(mode.get(), crate::common::app_mode::AppMode::Editor | crate::common::app_mode::AppMode::Play);
    let opened = file.path.clone().filter(|_| kept && shown);
    if opened == locked.0 {
        return;
    }
    let unmounted = locked.0.take().is_some() && assets.unmount().is_some();
    if unmounted {
        info!("The kept map's lock is unmounted");
    }
    let mut mounted = false;
    if let Some(path) = &opened {
        match crate::editor::save::load(path).and_then(|loaded| manifest_of(&loaded.requires)) {
            Ok(manifest) => {
                let pack = crate::common::pack_info::pack_of(path).expect("a kept map is in a pack");
                let paths = manifest.entries.iter().map(|entry| (entry.key.hash, entry.key.path.to_string())).collect();
                let objects = KeptObjects { pack, paths, downloads: root.store() };
                info!("Mounted the lock of {}: {} files", path.display(), manifest.entries.len());
                assets.mount(SessionPack::new(format!("the kept map {}", path.display()), manifest, Box::new(objects)));
                locked.0 = opened.clone();
                mounted = true;
            }
            Err(e) => warn!("The kept map {} has no lock that reads, so its files are found by name: {e}", path.display()),
        }
    }
    if mounted || unmounted {
        if let Some(mut cache) = cache {
            cache.forget_all();
        }
        changed.write(PacksChanged::ALL);
    }
}

#[derive(Message, Clone, Copy, Debug, Default)]
pub struct KeepACopy;

/// A folder name from `name`: letters, digits, `-` and `_` only.
fn folder_name(name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    match safe.trim_matches('_') {
        "" => "server".to_owned(),
        trimmed => trimmed.to_owned(),
    }
}

/// The first `{root}/{base}`, `{root}/{base}-2`, ... not taken.
fn free_pack(root: &Path, base: &str) -> PathBuf {
    let mut pack = root.join(base);
    let mut n = 2;
    while pack.exists() {
        pack = root.join(format!("{base}-{n}"));
        n += 1;
    }
    pack
}

pub fn keep_a_copy(mut asked: MessageReader<KeepACopy>, role: Res<NetRole>, assets: Res<PackAssets>, fetch: Res<Fetch>) {
    if asked.read().count() == 0 {
        return;
    }
    let Some(session) = assets.session().filter(|_| matches!(*role, NetRole::Client { .. })) else {
        return warn!("Only a client keeps a copy of the server's map, once the server's files are mounted");
    };
    match keep(session, &fetch.server, Path::new(ASSET_ROOT)) {
        Ok(map) => info!("Kept the server's map as {}", map.display()),
        Err(e) => error!("Could not keep a copy of the server's map: {e}"),
    }
}

fn keep(session: &SessionPack, server: &str, root: &Path) -> Result<PathBuf, String> {
    let manifest = &session.manifest;
    let Some(map) = manifest.entries.iter().find(|entry| entry.key.kind() == ResourceKind::Blueprint) else {
        return Err("no round has started, so the server has sent no map to keep".into());
    };
    let now = chrono::Local::now();
    let pack = free_pack(root, &format!("{}-{}", folder_name(server), now.format("%Y-%m-%d_%H-%M")));
    let requires: Vec<Required> = manifest
        .entries
        .iter()
        .filter(|entry| !matches!(entry.key.kind(), ResourceKind::Blueprint | ResourceKind::Lightmap))
        .map(Required::of)
        .collect();
    let write = |relative: &str, bytes: &[u8]| -> Result<(), String> {
        let path = pack.join(relative);
        std::fs::create_dir_all(path.parent().expect("a file in a pack has a folder")).map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    };

    let mut files = std::collections::BTreeMap::new();
    for entry in manifest.entries.iter().filter(|entry| entry.key.kind() != ResourceKind::Lightmap) {
        let bytes = session.read(entry.key.path.as_str()).map_err(|e| e.to_string())?;
        let bytes = match entry.key == map.key {
            // The round's map carries no lock; the copy is written with one, so it opens as it played.
            true => {
                let text = std::str::from_utf8(&bytes).map_err(|e| format!("{}: {e}", entry.key.path))?;
                let loaded = crate::editor::save::parse(text).map_err(|e| format!("{}: {e}", entry.key.path))?;
                crate::editor::save::to_text_locked(&loaded.timeline, &loaded.metadata, &requires)?.into_bytes()
            }
            false => bytes,
        };
        write(entry.key.path.as_str(), &bytes)?;
        files.insert(entry.key.path.to_string(), FileOrigin { hash: entry.key.hash.to_string(), authors: entry.authors.clone() });
    }
    let info = PackInfo {
        authors: Vec::new(),
        origin: Some(Origin { kind: KEPT.to_owned(), server: server.to_owned(), kept: now.to_rfc3339() }),
        files,
    };
    write(PACK_FILE, toml::to_string(&info).map_err(|e| e.to_string())?.as_bytes())?;
    Ok(pack.join(map.key.path.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_name_makes_a_safe_folder() {
        assert_eq!(folder_name("Alice's server"), "Alice_s_server");
        assert_eq!(folder_name("../.."), "server");
        assert_eq!(folder_name("10.0.0.2:27015"), "10_0_0_2_27015");
    }

    #[test]
    fn a_lock_survives_being_written_and_read_back() {
        let loaded = crate::editor::save::parse(&std::fs::read_to_string("assets/default/blueprints/new.gmb").unwrap()).unwrap();
        let requires = crate::editor::save::lock_for(Path::new("assets/user/blueprints/new.gmb"), &crate::editor::save::to_text(&loaded.timeline, &loaded.metadata).unwrap());
        assert!(requires.iter().any(|required| required.path == "loadouts.toml"), "the lock names nothing a match needs: {requires:?}");
        let text = crate::editor::save::to_text_locked(&loaded.timeline, &loaded.metadata, &requires).unwrap();
        let back = crate::editor::save::parse(&text).unwrap();
        assert_eq!(back.requires, requires);
        let manifest = manifest_of(&back.requires).unwrap();
        assert_eq!(manifest.entries.len(), requires.len());
        let weapons = std::fs::read("assets/default/weapons/pistol.toml").unwrap();
        assert_eq!(manifest.get(&crate::common::resource::LogicalPath::new("weapons/pistol.toml").unwrap()).unwrap().key.hash, ContentHash::of(&weapons));
    }

    /// Fails if a local pack's file at the same path is read instead of the kept one.
    #[test]
    fn a_kept_lock_reads_the_kept_file_not_the_local_one() {
        let pack = std::env::temp_dir().join(format!("grackle-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&pack);
        std::fs::create_dir_all(pack.join("weapons")).unwrap();
        let kept = b"name_key = \"weapon.names.pistol\"\n# kept\n";
        std::fs::write(pack.join("weapons/pistol.toml"), kept).unwrap();
        let requires = vec![Required { path: "weapons/pistol.toml".into(), hash: ContentHash::of(kept).to_string(), size: kept.len() as u64 }];
        let manifest = manifest_of(&requires).unwrap();
        let paths = manifest.entries.iter().map(|entry| (entry.key.hash, entry.key.path.to_string())).collect();
        let mut assets = PackAssets::default();
        assets.mount(SessionPack::new("a kept map".into(), manifest, Box::new(KeptObjects { pack, paths, downloads: None })));
        assert_eq!(assets.read_from_packs("weapons/pistol.toml").unwrap(), kept);
    }

    /// Fails if a kept copy lacks a file, its map loses the lock, or its `pack.toml` does not say
    /// where it came from.
    #[test]
    fn a_kept_copy_holds_the_map_with_its_lock_and_every_file() {
        use crate::common::resource::store::tests::scratch_store;
        use crate::common::resource::{LogicalPath, ManifestEntry, ResourceKey};
        let (store, _) = scratch_store("kept_copy");
        let loaded = crate::editor::save::parse(&std::fs::read_to_string("assets/default/blueprints/new.gmb").unwrap()).unwrap();
        let map = crate::editor::save::to_text(&loaded.timeline, &loaded.metadata).unwrap();
        let pistol = std::fs::read_to_string("assets/default/weapons/pistol.toml").unwrap();
        let entries = [("blueprints/round.gmb", map.as_str(), vec!["Alice".to_owned()]), ("weapons/pistol.toml", pistol.as_str(), vec![])]
            .map(|(path, text, authors)| {
                let path = LogicalPath::new(path).unwrap();
                let hash = ContentHash::of(text.as_bytes());
                store.put(hash, path.extension().unwrap(), text.as_bytes()).unwrap();
                ManifestEntry { key: ResourceKey { path, hash }, size: text.len() as u64, pack: "user".into(), authors }
            })
            .to_vec();
        let session = SessionPack::new("a server".into(), Manifest::checked(entries).unwrap(), Box::new(store));
        let root = std::env::temp_dir().join(format!("grackle-kept-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let kept = keep(&session, "Alice's server", &root).unwrap();
        let pack = kept.parent().and_then(Path::parent).unwrap().to_path_buf();
        assert!(pack.file_name().unwrap().to_string_lossy().starts_with("Alice_s_server-"), "{}", pack.display());
        assert_eq!(std::fs::read_to_string(pack.join("weapons/pistol.toml")).unwrap(), pistol);
        let back = crate::editor::save::load(&kept).unwrap();
        assert_eq!(back.requires.len(), 1, "the kept map's lock should name the pistol: {:?}", back.requires);
        let info = PackInfo::read(&NativeFiles, &pack);
        assert!(info.is_kept());
        assert_eq!(info.origin.unwrap().server, "Alice's server");
        assert_eq!(info.files["blueprints/round.gmb"].authors, ["Alice"]);
        assert_eq!(info.files["weapons/pistol.toml"].hash, ContentHash::of(pistol.as_bytes()).to_string());
    }
}
