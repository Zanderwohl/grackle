//! Reading what each class is built like from a pack's `classes.toml`, and the rig files under
//! `rigs/` those builds name.
//!
//! A bad entry is dropped with a line naming the file and the class, and that
//! class is then built as [`Proportions::DEFAULT`]: a body that looks wrong is
//! something you can see, where a panic on a listen server is not. A bad rig file
//! leaves the classes naming it built as the humanoid.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::Deserialize;
use strum::IntoEnumIterator;

use crate::common::assets::{AssetError, Assets};
use crate::common::class::{Class, ClassBuilds, TALLEST_CLASS_HEIGHT};
use crate::common::skeleton::rig::Build;
use crate::common::skeleton::rig_file::RigFile;
use crate::common::skeleton::Proportions;

const CLASSES_FILE: &str = "classes.toml";
pub const RIGS_DIR: &str = "rigs";

#[derive(Deserialize)]
struct ClassFile {
    #[serde(default)]
    builds: BTreeMap<String, BuildEntry>,
}

/// A build as written: a humanoid's nine numbers, or `rig`, `height`, `girth` and `scales`.
#[derive(Deserialize)]
struct BuildEntry {
    #[serde(default)]
    rig: Option<String>,
    #[serde(default)]
    scales: BTreeMap<String, f32>,
    #[serde(flatten)]
    proportions: Proportions,
}

/// Lowest priority first; a later pack replaces a class's whole build.
pub fn load_builds(assets: &Assets, packs: &[PathBuf]) -> ClassBuilds {
    let mut builds = ClassBuilds::default();
    for pack in packs {
        load_pack(assets, pack, &mut builds);
    }

    load_rigs(assets, packs, &mut builds);

    let missing: Vec<String> = Class::iter()
        .map(Class::key)
        .filter(|key| !builds.builds.contains_key(key))
        .collect();
    if !missing.is_empty() {
        warn!("No build for {}; drawn with the default one", missing.join(", "));
    }
    builds
}

fn load_pack(assets: &Assets, pack: &Path, builds: &mut ClassBuilds) {
    let named = format!("{}/{CLASSES_FILE}", pack.display());
    let text = match assets.read(pack, CLASSES_FILE) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => return error!("{named} is not valid UTF-8: {e}"),
        },
        Err(AssetError::NotFound(_)) => return,
        Err(e) => return error!("{e}"),
    };
    merge(&named, &text, builds);
}

fn merge(named: &str, text: &str, builds: &mut ClassBuilds) {
    let file: ClassFile = match toml::from_str(text) {
        Ok(file) => file,
        Err(e) => return error!("{named} will not parse: {e}"),
    };

    for (key, build) in file.builds {
        if Class::named(&key).is_none() {
            warn!("{named}: there is no class called `{key}`");
            continue;
        }
        let build = Build { rig: build.rig, proportions: build.proportions, scales: build.scales };
        if let Err(why) = check(&build) {
            warn!("{named}: the build for `{key}` is dropped: {why}");
            continue;
        }
        builds.builds.insert(key, build);
    }
}

/// Every rig a build names, from the highest-priority pack that has it. Named rather than listed,
/// since nothing behind a `fetch` can list a directory.
fn load_rigs(assets: &Assets, packs: &[PathBuf], builds: &mut ClassBuilds) {
    let wanted: std::collections::BTreeSet<String> =
        builds.builds.values().filter_map(|build| build.rig.clone()).collect();
    for name in wanted {
        match load_rig(assets, packs, &name) {
            Ok(rig) => {
                builds.rigs.insert(name, rig);
            }
            Err(why) => {
                warn!("The {name} rig is dropped, and the classes built on it are humanoids: {why}");
                for build in builds.builds.values_mut().filter(|build| build.rig.as_deref() == Some(&name)) {
                    build.rig = None;
                }
            }
        }
    }
}

pub fn load_rig(assets: &Assets, packs: &[PathBuf], name: &str) -> Result<RigFile, String> {
    let relative = format!("{RIGS_DIR}/{name}.toml");
    let mut found = None;
    for pack in packs {
        match assets.read(pack, &relative) {
            Ok(bytes) => found = Some((pack, bytes)),
            Err(AssetError::NotFound(_)) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    let (pack, bytes) = found.ok_or_else(|| format!("{relative} is in no pack"))?;
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    let rig: RigFile = toml::from_str(&text).map_err(|e| format!("{}/{relative}: {e}", pack.display()))?;
    rig.check(name).map_err(|why| format!("{}/{relative}: {why}", pack.display()))?;
    Ok(rig)
}

/// A build taller than spawns are checked for passes the editor and wedges a
/// head in the ceiling at runtime.
fn check(build: &Build) -> Result<(), String> {
    if build.scales.values().any(|scale| !scale.is_finite() || *scale <= 0.0) {
        return Err("every scale must be positive".into());
    }
    check_proportions(&build.proportions)
}

fn check_proportions(build: &Proportions) -> Result<(), String> {
    if build.height > TALLEST_CLASS_HEIGHT {
        return Err(format!(
            "{} m is taller than the {TALLEST_CLASS_HEIGHT} m a spawn point is checked for",
            build.height
        ));
    }
    let fields = [
        build.height,
        build.hip_height,
        build.torso_length,
        build.arm_length,
        build.head_length,
        build.shoulder_half_width,
        build.hip_half_width,
        build.girth,
        build.belly,
    ];
    if fields.iter().any(|value| !value.is_finite() || *value <= 0.0) {
        return Err("every number must be positive".into());
    }
    Ok(())
}

/// In every process: a client's is replaced by the server's on connect.
pub fn load_builds_at_startup(assets: Res<Assets>, mut builds: ResMut<ClassBuilds>) {
    let loaded = load_builds(&assets, &crate::common::assets::default_packs());
    info!("Loaded builds for {} classes and {} rigs", loaded.builds.len(), loaded.rigs.len());
    *builds = loaded;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn parse(text: &str) -> ClassBuilds {
        let mut builds = ClassBuilds::default();
        merge("test.toml", text, &mut builds);
        builds
    }

    #[test]
    fn the_default_pack_builds_every_class() {
        let builds = ClassBuilds::shipped();
        for class in Class::iter() {
            assert!(builds.builds.contains_key(&Class::key(class)), "{class:?} has no build");
        }
    }

    #[test]
    fn a_build_taller_than_the_clearance_is_dropped() {
        let builds = parse(
            "[builds.heavy]\nheight = 2.4\nhip_height = 0.47\ntorso_length = 0.34\n\
             arm_length = 0.32\nhead_length = 0.125\nshoulder_half_width = 0.155\n\
             hip_half_width = 0.075\ngirth = 1.75\nbelly = 1.0\n",
        );
        assert_eq!(builds.of(Class::Heavy), Proportions::DEFAULT);
    }

    #[test]
    fn an_unknown_class_is_dropped() {
        let builds = parse(
            "[builds.wizard]\nheight = 1.8\nhip_height = 0.5\ntorso_length = 0.3\n\
             arm_length = 0.3\nhead_length = 0.13\nshoulder_half_width = 0.1\n\
             hip_half_width = 0.05\ngirth = 1.0\nbelly = 1.0\n",
        );
        assert!(builds.builds.is_empty());
    }

    /// The Mercenary built on the default pack's strider.
    pub(crate) fn builds_with_a_strider() -> ClassBuilds {
        let mut builds = parse("[builds.mercenary]\nrig = \"strider\"\nheight = 1.5\n\n[builds.mercenary.scales]\nleg = 1.1\n");
        load_rigs(&Assets::default(), &crate::common::assets::default_packs(), &mut builds);
        builds
    }

    /// Fails if a class cannot name a rig file, or a missing one is not put back on the humanoid.
    #[test]
    fn a_class_may_be_built_on_a_rig_file() {
        let builds = builds_with_a_strider();
        let rig = builds.rig_for(Class::Mercenary);
        assert_eq!(rig.kind(), "strider");
        assert_eq!(rig.proportions().height, 1.5);
        assert_eq!(rig.build().scales.get("leg"), Some(&1.1));

        let mut missing = parse("[builds.mercenary]\nrig = \"nothing_here\"\nheight = 1.5\n");
        load_rigs(&Assets::default(), &crate::common::assets::default_packs(), &mut missing);
        assert_eq!(missing.build(Class::Mercenary).rig, None, "a missing rig was kept");
        assert_eq!(missing.rig_for(Class::Mercenary).kind(), crate::common::skeleton::rig::HUMANOID);
    }
}
