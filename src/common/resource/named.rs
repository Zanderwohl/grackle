//! What a pack file names, read by the loader that owns its format, so the formats and this list
//! cannot drift apart.

use crate::common::resource::{LogicalPath, ResourceKind};

/// A name as a file writes it, before it is a path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Named {
    /// A file stem under `props/`.
    Prop(String),
    /// A weapon key: a file stem under `weapons/`.
    Weapon(String),
    /// A material, not a role: roles are resolved by whoever reads the palette.
    Material(String),
    /// A file name under `decals/`, extension included.
    Decal(String),
    /// A file stem under `icons/`.
    Icon(String),
    /// A rig file's stem, which also names its own clip folder.
    Rig(String),
    /// A clip's path under `animations/` without its extension: `taunt_wave`, `raptor/taunt_wag`.
    Clip(String),
    /// A track's path inside a pack.
    Music(String),
}

/// A place a closure goes looking.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reference {
    /// Missing is reported unless `optional`.
    File { path: LogicalPath, optional: bool },
    /// Every file with the extension directly in the folder, in every pack.
    Folder { folder: String, extension: &'static str },
}

impl Named {
    /// `Err` with the name for one that makes no valid path, such as `../x` from a hostile file.
    pub fn references(&self) -> Result<Vec<Reference>, String> {
        let file = |path: String| -> Result<Reference, String> {
            LogicalPath::new(&path).map(|path| Reference::File { path, optional: false }).map_err(|e| format!("{path}: {e}"))
        };
        Ok(match self {
            Named::Prop(stem) => vec![file(format!("{}/{stem}.gpp", ResourceKind::Prop.folder()))?],
            Named::Weapon(key) => vec![file(crate::common::weapon_file::weapon_file(key))?],
            // No file stands behind these; they are drawn by code.
            Named::Material(name) if name == crate::common::sky::SKYBOX || name == crate::material::pit::PIT => Vec::new(),
            Named::Material(name) => {
                vec![file(format!("{}/{name}.{}", ResourceKind::Material.folder(), crate::material::FILE_EXTENSION))?]
            }
            Named::Decal(name) => vec![file(format!("{}/{name}", ResourceKind::Decal.folder()))?],
            Named::Icon(stem) => vec![file(format!("{}/{stem}.svg", ResourceKind::Icon.folder()))?],
            Named::Rig(stem) => {
                let rig = file(format!("{}/{stem}.toml", ResourceKind::Rig.folder()))?;
                let clips = format!("{}/{stem}", ResourceKind::Clip.folder());
                LogicalPath::new(&format!("{clips}/x.gpa")).map_err(|e| format!("{clips}: {e}"))?;
                vec![rig, Reference::Folder { folder: clips, extension: "gpa" }]
            }
            Named::Clip(path) => vec![file(format!("{}/{path}.gpa", ResourceKind::Clip.folder()))?],
            Named::Music(path) => {
                let mut references = vec![file(path.clone())?];
                // A track in a folder of its own is credited by that folder's file.
                if let Some((folder, _)) = path.rsplit_once('/').filter(|(folder, _)| folder.contains('/')) {
                    if let Ok(path) = LogicalPath::new(&format!("{folder}/attribution.txt")) {
                        references.push(Reference::File { path, optional: true });
                    }
                }
                references
            }
        })
    }
}

/// What a file names and who wrote it, where its format says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Described {
    pub named: Vec<Named>,
    pub authors: Vec<String>,
}

/// `Err` for a file its loader will not read.
pub fn describe(path: &LogicalPath, bytes: &[u8]) -> Result<Described, String> {
    let text = || std::str::from_utf8(bytes).map_err(|e| format!("not UTF-8: {e}"));
    match path.kind() {
        ResourceKind::Blueprint => crate::editor::save::resources_named_in(text()?),
        ResourceKind::Weapon => {
            let entry = crate::common::weapon_file::parse_weapon(text()?)?;
            let named = entry.model.map(Named::Prop).into_iter().chain(entry.icon.map(Named::Icon)).collect();
            Ok(Described { named, authors: Vec::new() })
        }
        ResourceKind::Table => describe_table(path, text()?),
        ResourceKind::Rig
        | ResourceKind::Clip
        | ResourceKind::Material
        | ResourceKind::Decal
        | ResourceKind::Icon
        | ResourceKind::Music
        | ResourceKind::Attribution => Ok(Described::default()),
        ResourceKind::Prop => {
            crate::prop::document::parse(text()?).map_err(|e| e.to_string())?;
            Ok(Described::default())
        }
    }
}

fn describe_table(path: &LogicalPath, text: &str) -> Result<Described, String> {
    let named = match path.as_str() {
        "loadouts.toml" => crate::common::weapon_file::resources_named_in_loadouts(text)?,
        "classes.toml" => crate::common::class_file::rigs_named_in(text)?.into_iter().map(Named::Rig).collect(),
        "taunts.toml" => crate::common::taunt::taunt_specs_in(text)
            .into_iter()
            .flat_map(|(_, spec)| {
                let clips = std::iter::once(spec.clip).chain(spec.rigs.into_values()).map(Named::Clip);
                clips.chain(spec.icon.map(Named::Icon)).collect::<Vec<_>>()
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(Described { named, authors: Vec::new() })
}
