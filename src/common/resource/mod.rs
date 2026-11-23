//! A pack file named across machines: its kind, its path inside a pack and the BLAKE3 hash of its
//! bytes. Two files with one path and different hashes are different resources. See
//! documentation/resource-streaming.md.

pub mod closure;
#[cfg(not(target_arch = "wasm32"))]
pub mod hash_index;
mod named;
pub mod session;
pub mod store;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use named::{describe, Described, Named, Reference};

/// Registers [`store::OBJECT_SOURCE`], so the `AssetServer` can load a server's file (a track) out
/// of the object store by [`store::object_asset_path`]. Before `AssetPlugin`, which builds the
/// sources it is given and no later ones.
pub struct ObjectSourcePlugin;

impl bevy::app::Plugin for ObjectSourcePlugin {
    fn build(&self, app: &mut bevy::app::App) {
        use bevy::asset::io::{AssetSource, AssetSourceBuilder};
        use bevy::asset::AssetApp;
        // Absolute, as `AssetPlugin::file_path` is, since Bevy otherwise reads from the executable's folder.
        let root = std::env::current_dir()
            .map(|dir| dir.join(store::OBJECTS_DIR).to_string_lossy().into_owned())
            .unwrap_or_else(|_| store::OBJECTS_DIR.to_owned());
        app.register_asset_source(store::OBJECT_SOURCE, AssetSourceBuilder::new(AssetSource::get_default_reader(root)));
    }
}

/// BLAKE3-256 of a file's bytes as stored.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// The first 12 hex digits, for logs and the UI.
    pub fn short(&self) -> String {
        self.to_string()[..12].to_owned()
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.short())
    }
}

/// A path inside a pack, such as `props/crate.gpp`, that some [`ResourceKind`] allows. Checked on
/// construction, since it may come from a stranger's server and must never reach outside a pack.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct LogicalPath(String);

/// Bytes, so a path fits a datagram-sized field whatever its characters.
pub const MAX_PATH_BYTES: usize = 255;
const MAX_DEPTH: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathRefused {
    Empty,
    TooLong,
    TooDeep,
    /// A component that is empty, `.`, `..` or hidden, or holds a character no pack file name may.
    BadComponent(String),
    /// No kind takes a file there with that extension.
    NoKind,
}

impl fmt::Display for PathRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathRefused::Empty => write!(f, "the path is empty"),
            PathRefused::TooLong => write!(f, "the path is longer than {MAX_PATH_BYTES} bytes"),
            PathRefused::TooDeep => write!(f, "the path is more than {MAX_DEPTH} folders deep"),
            PathRefused::BadComponent(part) => write!(f, "the path has a bad part {part:?}"),
            PathRefused::NoKind => write!(f, "no kind of pack file lives at that path"),
        }
    }
}

impl LogicalPath {
    pub fn new(path: &str) -> Result<Self, PathRefused> {
        if path.is_empty() {
            return Err(PathRefused::Empty);
        }
        if path.len() > MAX_PATH_BYTES {
            return Err(PathRefused::TooLong);
        }
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() > MAX_DEPTH {
            return Err(PathRefused::TooDeep);
        }
        for part in &parts {
            let bad = part.is_empty()
                || part.starts_with('.')
                || part.chars().any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'));
            if bad {
                return Err(PathRefused::BadComponent((*part).to_owned()));
            }
        }
        let path = Self(path.to_owned());
        match ResourceKind::of(&path) {
            Some(_) => Ok(path),
            None => Err(PathRefused::NoKind),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn kind(&self) -> ResourceKind {
        ResourceKind::of(self).expect("a LogicalPath has a kind by construction")
    }

    pub fn extension(&self) -> Option<&str> {
        let name = self.0.rsplit('/').next()?;
        name.rsplit_once('.').map(|(_, extension)| extension)
    }

    fn folder(&self) -> &str {
        self.0.split('/').next().unwrap_or("")
    }

    fn depth(&self) -> usize {
        self.0.split('/').count()
    }
}

impl fmt::Display for LogicalPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for LogicalPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

/// Deserialized through [`LogicalPath::new`], so a path off the wire is checked like any other.
impl<'de> Deserialize<'de> for LogicalPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        LogicalPath::new(&text).map_err(serde::de::Error::custom)
    }
}

/// The pack files a match can depend on. Lang files, UI icons, fonts and settings are left out:
/// a client reads its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    Blueprint,
    Prop,
    Weapon,
    /// `loadouts.toml`, `classes.toml`, `taunts.toml`, `modes.toml`, `layers.toml`.
    Table,
    Rig,
    Clip,
    Material,
    Decal,
    /// Named by a weapon or a taunt.
    Icon,
    Music,
    /// A music folder's `attribution.txt`.
    Attribution,
}

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

pub const TABLES: [&str; 5] = ["loadouts.toml", "classes.toml", "taunts.toml", "modes.toml", "layers.toml"];

impl ResourceKind {
    pub const ALL: [ResourceKind; 11] = [
        ResourceKind::Blueprint,
        ResourceKind::Prop,
        ResourceKind::Weapon,
        ResourceKind::Table,
        ResourceKind::Rig,
        ResourceKind::Clip,
        ResourceKind::Material,
        ResourceKind::Decal,
        ResourceKind::Icon,
        ResourceKind::Music,
        ResourceKind::Attribution,
    ];

    /// The pack folder the kind lives under; empty for the tables at a pack's root.
    pub fn folder(self) -> &'static str {
        match self {
            ResourceKind::Blueprint => crate::constants::BLUEPRINTS_DIR,
            ResourceKind::Prop => crate::prop::document::PROPS_DIR,
            ResourceKind::Weapon => crate::common::weapon_file::WEAPONS_DIR,
            ResourceKind::Table => "",
            ResourceKind::Rig => crate::common::class_file::RIGS_DIR,
            ResourceKind::Clip => "animations",
            ResourceKind::Material => crate::material::MATERIALS_DIR,
            ResourceKind::Decal => "decals",
            ResourceKind::Icon => "icons",
            ResourceKind::Music | ResourceKind::Attribution => "music",
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            ResourceKind::Blueprint => &[crate::constants::MAP_BLUEPRINT_EXTENSION],
            ResourceKind::Prop => &["gpp"],
            ResourceKind::Weapon | ResourceKind::Table | ResourceKind::Rig => &["toml"],
            ResourceKind::Clip => &["gpa"],
            ResourceKind::Material => &[crate::material::FILE_EXTENSION],
            ResourceKind::Decal => &["png"],
            ResourceKind::Icon => &["svg"],
            ResourceKind::Music => &["mp3", "flac", "wav", "ogg"],
            ResourceKind::Attribution => &["txt"],
        }
    }

    /// The largest file of the kind a client will take.
    pub fn max_bytes(self) -> u64 {
        match self {
            ResourceKind::Blueprint => 16 * MIB,
            ResourceKind::Prop | ResourceKind::Clip | ResourceKind::Material => MIB,
            ResourceKind::Weapon | ResourceKind::Table | ResourceKind::Rig | ResourceKind::Icon => 256 * KIB,
            ResourceKind::Decal => 8 * MIB,
            ResourceKind::Music => 32 * MIB,
            ResourceKind::Attribution => 64 * KIB,
        }
    }

    /// What a path is, by its folder, depth and extension. Only a weapon, rig or table sits
    /// directly in its folder; clips are one folder further down for a rig's own, and a track
    /// may be in a folder of its own.
    pub fn of(path: &LogicalPath) -> Option<ResourceKind> {
        let extension = path.extension()?;
        let depth = path.depth();
        if depth == 1 {
            return TABLES.contains(&path.as_str()).then_some(ResourceKind::Table);
        }
        if path.folder() == "music" && path.as_str().ends_with("/attribution.txt") {
            return (depth == 3).then_some(ResourceKind::Attribution);
        }
        let kind = Self::ALL
            .into_iter()
            .filter(|kind| *kind != ResourceKind::Table && *kind != ResourceKind::Attribution)
            .find(|kind| kind.folder() == path.folder() && kind.extensions().contains(&extension))?;
        let deepest = match kind {
            ResourceKind::Clip | ResourceKind::Music => 3,
            _ => 2,
        };
        (depth <= deepest).then_some(kind)
    }
}

/// A file named across machines.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ResourceKey {
    pub path: LogicalPath,
    pub hash: ContentHash,
}

impl ResourceKey {
    pub fn kind(&self) -> ResourceKind {
        self.path.kind()
    }
}

/// One file a match needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub key: ResourceKey,
    /// Bytes. Not part of the hash: it weights a progress bar and caps a transfer.
    pub size: u64,
    /// The folder name of the pack it came from, for the UI only. It never names a folder on the
    /// machine that reads it.
    pub pack: String,
    pub authors: Vec<String>,
}

/// Every file a match needs, sorted by path, one entry per path. Deserialized through
/// [`Manifest::checked`], since one arrives from a server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ManifestIn")]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
}

#[derive(Deserialize)]
struct ManifestIn {
    entries: Vec<ManifestEntry>,
}

impl TryFrom<ManifestIn> for Manifest {
    type Error = String;

    fn try_from(manifest: ManifestIn) -> Result<Self, String> {
        Manifest::checked(manifest.entries)
    }
}

pub const MAX_MANIFEST_ENTRIES: usize = 4096;

impl Manifest {
    /// Sorted by path; refused whole for too many entries, a path listed twice, or a size over its
    /// kind's cap.
    pub fn checked(mut entries: Vec<ManifestEntry>) -> Result<Self, String> {
        if entries.len() > MAX_MANIFEST_ENTRIES {
            return Err(format!("{} entries, over the {MAX_MANIFEST_ENTRIES} a manifest may list", entries.len()));
        }
        if let Some(entry) = entries.iter().find(|entry| entry.size > entry.key.kind().max_bytes()) {
            return Err(format!("{} is {} bytes, over the {} its kind may be", entry.key.path, entry.size, entry.key.kind().max_bytes()));
        }
        entries.sort_by(|a, b| a.key.path.cmp(&b.key.path));
        if let Some(pair) = entries.windows(2).find(|pair| pair[0].key.path == pair[1].key.path) {
            return Err(format!("{} is listed twice", pair[0].key.path));
        }
        Ok(Self { entries })
    }

    pub fn get(&self, path: &LogicalPath) -> Option<&ManifestEntry> {
        self.entries.binary_search_by(|entry| entry.key.path.cmp(path)).ok().map(|at| &self.entries[at])
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> LogicalPath {
        LogicalPath::new(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn each_kind_is_found_where_it_lives() {
        assert_eq!(path("blueprints/arena.gmb").kind(), ResourceKind::Blueprint);
        assert_eq!(path("props/crate.gpp").kind(), ResourceKind::Prop);
        assert_eq!(path("weapons/pistol.toml").kind(), ResourceKind::Weapon);
        assert_eq!(path("loadouts.toml").kind(), ResourceKind::Table);
        assert_eq!(path("rigs/raptor.toml").kind(), ResourceKind::Rig);
        assert_eq!(path("animations/idle.gpa").kind(), ResourceKind::Clip);
        assert_eq!(path("animations/raptor/idle.gpa").kind(), ResourceKind::Clip);
        assert_eq!(path("materials/concrete.tgraph").kind(), ResourceKind::Material);
        assert_eq!(path("decals/blood_1.png").kind(), ResourceKind::Decal);
        assert_eq!(path("icons/kill_rocket.svg").kind(), ResourceKind::Icon);
        assert_eq!(path("music/Pump/Pump.mp3").kind(), ResourceKind::Music);
        assert_eq!(path("music/Pump/attribution.txt").kind(), ResourceKind::Attribution);
    }

    #[test]
    fn a_path_out_of_a_pack_is_refused() {
        for bad in [
            "",
            "../assets/user/props/crate.gpp",
            "props/../../crate.gpp",
            "/props/crate.gpp",
            "props//crate.gpp",
            "props/.crate.gpp",
            "props\\crate.gpp",
            "C:/props/crate.gpp",
            "props/crate.exe",
            "settings.toml",
            "lang/en-US.toml",
            "props/deeper/crate.gpp",
            "music/a/b/c.mp3",
        ] {
            assert!(LogicalPath::new(bad).is_err(), "{bad:?} was taken");
        }
        assert_eq!(LogicalPath::new(&"a".repeat(300)), Err(PathRefused::TooLong));
    }

    #[test]
    fn a_path_off_the_wire_is_checked() {
        let bytes = postcard::to_stdvec(&"../escape.gpp".to_owned()).unwrap();
        assert!(postcard::from_bytes::<LogicalPath>(&bytes).is_err());
        let bytes = postcard::to_stdvec(&path("props/crate.gpp")).unwrap();
        assert_eq!(postcard::from_bytes::<LogicalPath>(&bytes).unwrap(), path("props/crate.gpp"));
    }

    /// Breaks if the hash function changes, which would make every stored object a miss.
    #[test]
    fn the_hash_is_blake3() {
        assert_eq!(ContentHash::of(b"").short(), "af1349b9f5f9");
        assert_eq!(
            ContentHash::of(b"").to_string(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn a_manifest_off_the_wire_is_checked() {
        let entry = |text: &str, size: u64| ManifestEntry {
            key: ResourceKey { path: path(text), hash: ContentHash::of(text.as_bytes()) },
            size,
            pack: "user".into(),
            authors: Vec::new(),
        };
        // Written as the wire would carry it, bypassing `checked`.
        let send = |entries: Vec<ManifestEntry>| postcard::to_stdvec(&Manifest { entries }).unwrap();

        let unsorted = postcard::from_bytes::<Manifest>(&send(vec![entry("props/z.gpp", 1), entry("props/a.gpp", 1)])).unwrap();
        assert!(unsorted.get(&path("props/z.gpp")).is_some() && unsorted.get(&path("props/a.gpp")).is_some());
        assert!(postcard::from_bytes::<Manifest>(&send(vec![entry("props/a.gpp", 1), entry("props/a.gpp", 2)])).is_err());
        assert!(postcard::from_bytes::<Manifest>(&send(vec![entry("icons/a.svg", 10 * MIB)])).is_err());
        let many = (0..=MAX_MANIFEST_ENTRIES).map(|n| entry(&format!("props/p{n}.gpp"), 1)).collect();
        assert!(postcard::from_bytes::<Manifest>(&send(many)).is_err());
    }

    #[test]
    fn a_manifest_crosses_the_wire() {
        let manifest = Manifest {
            entries: vec![ManifestEntry {
                key: ResourceKey { path: path("props/crate.gpp"), hash: ContentHash::of(b"crate") },
                size: 5,
                pack: "user".into(),
                authors: vec!["somebody".into()],
            }],
        };
        let bytes = postcard::to_stdvec(&manifest).unwrap();
        assert_eq!(postcard::from_bytes::<Manifest>(&bytes).unwrap(), manifest);
    }
}
