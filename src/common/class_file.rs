//! Reading what each class is built like from a pack's `classes.toml`.
//!
//! A bad entry is dropped with a line naming the file and the class, and that
//! class is then built as [`Proportions::DEFAULT`]: a body that looks wrong is
//! something you can see, where a panic on a listen server is not.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::Deserialize;
use strum::IntoEnumIterator;

use crate::common::assets::{AssetError, Assets};
use crate::common::class::{Class, ClassBuilds, TALLEST_CLASS_HEIGHT};
use crate::common::skeleton::Proportions;

const CLASSES_FILE: &str = "classes.toml";

#[derive(Deserialize)]
struct ClassFile {
    #[serde(default)]
    builds: BTreeMap<String, Proportions>,
}

/// Lowest priority first; a later pack replaces a class's whole build.
pub fn load_builds(assets: &Assets, packs: &[PathBuf]) -> ClassBuilds {
    let mut builds = ClassBuilds::default();
    for pack in packs {
        load_pack(assets, pack, &mut builds);
    }

    let missing: Vec<String> = Class::iter()
        .map(Class::key)
        .filter(|key| !builds.0.contains_key(key))
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
        if let Err(why) = check(&build) {
            warn!("{named}: the build for `{key}` is dropped: {why}");
            continue;
        }
        builds.0.insert(key, build);
    }
}

/// A build taller than spawns are checked for passes the editor and wedges a
/// head in the ceiling at runtime.
fn check(build: &Proportions) -> Result<(), String> {
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
    info!("Loaded builds for {} classes", loaded.0.len());
    *builds = loaded;
}

#[cfg(test)]
mod tests {
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
            assert!(builds.0.contains_key(&Class::key(class)), "{class:?} has no build");
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
        assert!(builds.0.is_empty());
    }
}
