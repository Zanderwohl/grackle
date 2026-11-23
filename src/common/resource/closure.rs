//! Everything a match needs, found by walking what each file names from a few roots, over the
//! packs as a lookup by name would see them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::common::assets::{AssetError, Assets};
use crate::common::pack_info::PackInfo;
use crate::common::resource::named::Reference;
use crate::common::resource::{describe, ContentHash, LogicalPath, Manifest, ManifestEntry, ResourceKey, ResourceKind, TABLES};

/// A reference nothing answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub path: LogicalPath,
    /// `None` for a root.
    pub named_by: Option<LogicalPath>,
}

#[derive(Clone, Debug, Default)]
pub struct Closure {
    pub manifest: Manifest,
    pub missing: Vec<Missing>,
    /// A file left out, and why: too big, unreadable, or naming a path that is refused.
    pub problems: Vec<String>,
}

/// What every match loads whatever its map: the tables, every weapon file (the catalog is all of
/// them) and the clips under `animations/` played by state or by name.
pub fn match_roots(map: LogicalPath) -> Vec<Reference> {
    let mut roots = vec![Reference::File { path: map, optional: false }];
    roots.extend(TABLES.iter().map(|table| Reference::File {
        path: LogicalPath::new(table).expect("a table's name is a valid path"),
        optional: true,
    }));
    roots.push(Reference::Folder { folder: ResourceKind::Weapon.folder().to_owned(), extension: "toml" });
    roots.push(Reference::Folder { folder: ResourceKind::Clip.folder().to_owned(), extension: "gpa" });
    roots
}

/// A file found in a pack, or one handed in by its bytes (a round's map, serialized as it stands).
struct Found {
    bytes: Vec<u8>,
    pack: String,
    /// `None` for one handed in.
    from: Option<PathBuf>,
}

/// `packs` lowest priority first, as [`crate::common::assets::default_packs`] gives them, so the
/// highest pack's copy of a path wins. `given` answers a path before any pack does.
pub fn closure(assets: &Assets, packs: &[PathBuf], given: &BTreeMap<LogicalPath, (Vec<u8>, String)>, roots: Vec<Reference>) -> Closure {
    let mut result = Closure::default();
    let mut seen: BTreeSet<LogicalPath> = BTreeSet::new();
    let mut entries: Vec<ManifestEntry> = Vec::new();
    let mut infos: std::collections::HashMap<PathBuf, PackInfo> = std::collections::HashMap::new();
    let mut queue: Vec<(Reference, Option<LogicalPath>)> = roots.into_iter().rev().map(|root| (root, None)).collect();

    while let Some((reference, named_by)) = queue.pop() {
        let (path, optional) = match reference {
            Reference::File { path, optional } => (path, optional),
            Reference::Folder { folder, extension } => {
                for path in in_folder(assets, packs, &folder, extension, &mut result.problems).into_iter().rev() {
                    queue.push((Reference::File { path, optional: false }, named_by.clone()));
                }
                continue;
            }
        };
        if !seen.insert(path.clone()) {
            continue;
        }

        let found = match given.get(&path) {
            Some((bytes, pack)) => Lookup::Found(Found { bytes: bytes.clone(), pack: pack.clone(), from: None }),
            None => find(assets, packs, &path, &mut result.problems),
        };
        let Found { bytes, pack, from } = match found {
            Lookup::Found(found) => found,
            Lookup::Missing if !optional => {
                result.missing.push(Missing { path, named_by });
                continue;
            }
            Lookup::Missing | Lookup::Refused => continue,
        };

        let kind = path.kind();
        let size = bytes.len() as u64;
        if size > kind.max_bytes() {
            result.problems.push(format!("{path} is {size} bytes, over the {} a {kind:?} may be", kind.max_bytes()));
            continue;
        }
        let described = match describe(&path, &bytes) {
            Ok(described) => described,
            Err(why) => {
                result.problems.push(format!("{path} from the {pack} pack will not read: {why}"));
                continue;
            }
        };
        for name in described.named {
            match name.references() {
                Ok(references) => {
                    for reference in references.into_iter().rev() {
                        queue.push((reference, Some(path.clone())));
                    }
                }
                Err(why) => result.problems.push(format!("{path} names a path that is refused: {why}")),
            }
        }
        // A file's own authors, else its pack's `pack.toml`.
        let authors = match (described.authors.is_empty(), &from) {
            (true, Some(from)) => infos.entry(from.clone()).or_insert_with(|| PackInfo::read(assets, from)).authors_of(path.as_str()),
            _ => described.authors,
        };
        entries.push(ManifestEntry { key: ResourceKey { path, hash: ContentHash::of(&bytes) }, size, pack, authors });
    }

    entries.sort_by(|a, b| a.key.path.cmp(&b.key.path));
    result.manifest = Manifest { entries };
    result
}

fn pack_label(pack: &std::path::Path) -> String {
    pack.file_name().map_or_else(|| pack.display().to_string(), |name| name.to_string_lossy().into_owned())
}

enum Lookup {
    Found(Found),
    Missing,
    /// Over its kind's cap, and so not read.
    Refused,
}

fn find(assets: &Assets, packs: &[PathBuf], path: &LogicalPath, problems: &mut Vec<String>) -> Lookup {
    let most = path.kind().max_bytes();
    for pack in packs.iter().rev() {
        if let Some(size) = assets.size(pack, path.as_str())
            && size > most
        {
            problems.push(format!("{}/{path} is {size} bytes, over the {most} a {:?} may be", pack.display(), path.kind()));
            return Lookup::Refused;
        }
        match assets.read(pack, path.as_str()) {
            Ok(bytes) => return Lookup::Found(Found { bytes, pack: pack_label(pack), from: Some(pack.clone()) }),
            Err(AssetError::NotFound(_)) => {}
            Err(e) => problems.push(format!("{e}; looking in a lower pack")),
        }
    }
    Lookup::Missing
}

/// Sorted, each path once however many packs have it.
fn in_folder(assets: &Assets, packs: &[PathBuf], folder: &str, extension: &str, problems: &mut Vec<String>) -> Vec<LogicalPath> {
    let suffix = format!(".{extension}");
    let mut paths = BTreeSet::new();
    for pack in packs {
        for name in assets.list(pack, folder) {
            if !name.ends_with(&suffix) {
                continue;
            }
            match LogicalPath::new(&format!("{folder}/{name}")) {
                Ok(path) => {
                    paths.insert(path);
                }
                Err(why) => problems.push(format!("{}/{folder}/{name} is left out: {why}", pack.display())),
            }
        }
    }
    paths.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> LogicalPath {
        LogicalPath::new(text).unwrap()
    }

    fn shipped(map: &str) -> Closure {
        let packs = vec![PathBuf::from("assets/default")];
        closure(&Assets::default(), &packs, &BTreeMap::new(), match_roots(path(&format!("blueprints/{map}.gmb"))))
    }

    fn has(closure: &Closure, text: &str) -> bool {
        closure.manifest.get(&path(text)).is_some()
    }

    /// Every shipped map's closure is whole: nothing it names is missing, and every file reads.
    #[test]
    fn every_shipped_map_needs_only_what_ships() {
        let packs = vec![PathBuf::from("assets/default")];
        let maps = Assets::default().stems(&packs[0], "blueprints", "gmb");
        assert!(maps.contains(&"arena".to_owned()), "the shipped maps were not found: {maps:?}");
        // `ctf_2fort.gmb` is a SQLite blueprint from before the text format, which nothing reads.
        for map in maps.into_iter().filter(|map| map != "ctf_2fort") {
            let closure = shipped(&map);
            assert!(closure.problems.is_empty(), "{map}: {:#?}", closure.problems);
            assert!(closure.missing.is_empty(), "{map}: {:#?}", closure.missing);
        }
    }

    #[test]
    fn the_arena_brings_its_weapons_their_models_and_the_rigs() {
        let closure = shipped("arena");
        for needed in [
            "blueprints/arena.gmb",
            "loadouts.toml",
            "classes.toml",
            "taunts.toml",
            "weapons/rocket_launcher.toml",
            "props/rocket_launcher.gpp",
            "props/top_hat.gpp",
            "icons/kill_rocket.svg",
            "rigs/raptor.toml",
            "animations/idle.gpa",
            "animations/raptor/idle.gpa",
            "materials/grid_gray.tgraph",
        ] {
            assert!(has(&closure, needed), "the arena's closure lacks {needed}");
        }
        assert!(!closure.manifest.entries.iter().any(|entry| entry.key.path.as_str().starts_with("lang/")), "a lang file is not a match's");
        let arena = closure.manifest.get(&path("blueprints/arena.gmb")).unwrap();
        assert_eq!(arena.pack, "default");
        assert_eq!(arena.authors, vec!["Claude".to_owned()], "a map is credited by its own authors");
        let launcher = closure.manifest.get(&path("props/rocket_launcher.gpp")).unwrap();
        assert_eq!(launcher.authors, vec!["The Grackle contributors".to_owned()], "a prop is credited by its pack");
    }

    /// A role names a material per side, and a map with that role may paint any of them.
    #[test]
    fn a_role_brings_every_side_of_it() {
        let closure = shipped("showcase");
        for side in ["grid_red", "grid_blue", "grid_yellow", "grid_green", "orange"] {
            assert!(has(&closure, &format!("materials/{side}.tgraph")), "the showcase lacks {side}");
        }
    }

    #[test]
    fn a_given_map_stands_in_for_the_file() {
        let packs = vec![PathBuf::from("assets/default")];
        let map = path("blueprints/played.gmb");
        let bytes = std::fs::read("assets/default/blueprints/new.gmb").unwrap();
        let given = BTreeMap::from([(map.clone(), (bytes.clone(), "user".to_owned()))]);
        let closure = closure(&Assets::default(), &packs, &given, match_roots(map.clone()));
        let entry = closure.manifest.get(&map).expect("the given map is in its own closure");
        assert_eq!(entry.key.hash, ContentHash::of(&bytes));
        assert_eq!(entry.pack, "user");
        assert!(closure.missing.is_empty(), "{:#?}", closure.missing);
    }

    #[test]
    fn a_name_that_climbs_out_of_the_pack_is_refused() {
        let packs = vec![PathBuf::from("assets/default")];
        let map = path("blueprints/hostile.gmb");
        let text = std::fs::read_to_string("assets/default/blueprints/new.gmb")
            .unwrap()
            .replacen("music: []", "music: [\"../../../etc/passwd\"]", 1);
        assert!(text.contains("passwd"), "the template's music list moved");
        let given = BTreeMap::from([(map.clone(), (text.into_bytes(), "user".to_owned()))]);
        let closure = closure(&Assets::default(), &packs, &given, match_roots(map));
        assert!(closure.problems.iter().any(|problem| problem.contains("passwd")), "{:#?}", closure.problems);
    }

    /// Two packs of files in memory.
    struct Memory(Vec<(&'static str, &'static str, &'static str)>);

    impl crate::common::assets::AssetSource for Memory {
        fn read(&self, pack: &std::path::Path, relative: &str) -> Result<Vec<u8>, AssetError> {
            self.0
                .iter()
                .find(|(in_pack, file, _)| std::path::Path::new(in_pack) == pack && *file == relative)
                .map(|(_, _, text)| text.as_bytes().to_vec())
                .ok_or_else(|| AssetError::NotFound(relative.to_owned()))
        }

        fn list(&self, pack: &std::path::Path, folder: &str) -> Vec<String> {
            self.0
                .iter()
                .filter(|(in_pack, _, _)| std::path::Path::new(in_pack) == pack)
                .filter_map(|(_, file, _)| file.strip_prefix(&format!("{folder}/")).map(str::to_owned))
                .collect()
        }
    }

    #[test]
    fn a_higher_pack_wins_and_a_folder_lists_each_path_once() {
        let assets = Assets::new(Box::new(Memory(vec![
            ("low", "icons/a.svg", "<svg low/>"),
            ("high", "icons/a.svg", "<svg high/>"),
            ("low", "icons/b.svg", "<svg/>"),
        ])));
        let packs = vec![PathBuf::from("low"), PathBuf::from("high")];
        let roots = vec![Reference::Folder { folder: "icons".into(), extension: "svg" }];
        let closure = closure(&assets, &packs, &BTreeMap::new(), roots);
        let a = closure.manifest.get(&path("icons/a.svg")).unwrap();
        assert_eq!((a.pack.as_str(), a.key.hash), ("high", ContentHash::of(b"<svg high/>")));
        assert_eq!(closure.manifest.get(&path("icons/b.svg")).unwrap().pack, "low");
        assert_eq!(closure.manifest.entries.len(), 2);
        assert_eq!(closure.manifest.total_bytes(), "<svg high/>".len() as u64 + "<svg/>".len() as u64);
    }
}
