//! Reading how fast each animation layer fades, and what humiliation claims,
//! from a pack's `layers.toml`.
//!
//! A bad entry is dropped with a line saying why, and that layer snaps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::common::assets::{AssetError, Assets};
use serde::Deserialize;

use crate::common::skeleton::play::{Blend, Layer, LayerBlends, LayerClaims, Regions};

const LAYERS_FILE: &str = "layers.toml";

const CLAIMS_FROM_THE_FILE: [Layer; 1] = [Layer::Humiliation];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerEntry {
    #[serde(default)]
    blend_in: f32,
    #[serde(default)]
    blend_out: f32,
    #[serde(default)]
    claims: Option<Vec<String>>,
}

impl LayerEntry {
    fn blend(&self) -> Blend {
        Blend { blend_in: self.blend_in, blend_out: self.blend_out }
    }
}

/// Lowest priority first; a later pack replaces a layer's whole entry.
pub fn load_layers(assets: &Assets, packs: &[PathBuf]) -> (LayerBlends, LayerClaims) {
    let mut layers = (LayerBlends::default(), LayerClaims::default());
    for pack in packs {
        load_pack(assets, pack, &mut layers);
    }
    layers
}

pub fn load_blends(assets: &Assets, packs: &[PathBuf]) -> LayerBlends {
    load_layers(assets, packs).0
}

fn load_pack(assets: &Assets, pack: &Path, layers: &mut (LayerBlends, LayerClaims)) {
    let named = format!("{}/{LAYERS_FILE}", pack.display());
    let text = match assets.read(pack, LAYERS_FILE) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => return error!("{named} is not valid UTF-8: {e}"),
        },
        Err(AssetError::NotFound(_)) => return,
        Err(e) => return error!("{e}"),
    };
    merge(&named, &text, layers);
}

fn merge(named: &str, text: &str, (blends, claims): &mut (LayerBlends, LayerClaims)) {
    let file: BTreeMap<String, LayerEntry> = match toml::from_str(text) {
        Ok(file) => file,
        Err(e) => return error!("{named} will not parse: {e}"),
    };
    for (key, entry) in file {
        let Some(layer) = Layer::named(&key) else {
            warn!("{named}: there is no layer called `{key}`");
            continue;
        };
        match check(layer, &entry) {
            Ok(claimed) => {
                blends.0.insert(layer, entry.blend());
                match claimed {
                    Some(regions) => claims.0.insert(layer, regions),
                    None => claims.0.remove(&layer),
                };
            }
            Err(why) => warn!("{named}: `{key}` is dropped: {why}"),
        }
    }
}

fn check(layer: Layer, entry: &LayerEntry) -> Result<Option<Regions>, String> {
    let seconds = [entry.blend_in, entry.blend_out];
    if seconds.iter().any(|seconds| !seconds.is_finite() || *seconds < 0.0) {
        return Err("a blend is a number of seconds, zero or more".into());
    }
    let Some(names) = &entry.claims else { return Ok(None) };
    if !CLAIMS_FROM_THE_FILE.contains(&layer) {
        return Err(format!("the {} layer's claims are not a file's to state", layer.key()));
    }
    names.iter().try_fold(Regions::NONE, |claimed, name| {
        Regions::named(name)
            .map(|region| claimed.union(region))
            .ok_or_else(|| format!("there is no body part called `{name}`"))
    })
    .map(Some)
}

pub fn load_layers_at_startup(
    assets: Res<Assets>,
    mut blends: ResMut<LayerBlends>,
    mut claims: ResMut<LayerClaims>,
) {
    (*blends, *claims) = load_layers(&assets, &crate::common::assets::default_packs());
}

impl LayerClaims {
    pub fn shipped() -> LayerClaims {
        load_layers(&Assets::default(), &crate::common::assets::default_packs()).1
    }
}

impl LayerBlends {
    /// What the default pack ships, for tests and tools.
    pub fn shipped() -> LayerBlends {
        load_blends(&Assets::default(), &crate::common::assets::default_packs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> LayerBlends {
        parse_both(text).0
    }

    fn parse_both(text: &str) -> (LayerBlends, LayerClaims) {
        let mut layers = Default::default();
        merge("test.toml", text, &mut layers);
        layers
    }

    #[test]
    fn humiliation_claims_what_its_file_says_and_nothing_else_may() {
        assert_eq!(
            LayerClaims::shipped().of(Layer::Humiliation),
            Layer::Humiliation.claims(),
            "the shipped file and the built-in claims disagree",
        );
        let (_, claims) = parse_both("[humiliation]\nclaims = [\"arm.r\"]\n\n[weapon]\nclaims = [\"legs\"]\n");
        assert_eq!(claims.of(Layer::Humiliation), Regions::ARM_R);
        assert_eq!(claims.of(Layer::Weapon), Layer::Weapon.claims(), "the weapon's claims came from a file");
    }

    #[test]
    fn the_default_pack_fades_humiliation() {
        let blend = LayerBlends::shipped().of(Layer::Humiliation, None);
        assert!(blend.blend_in > 0.0 && blend.blend_out > 0.0, "{blend:?}");
    }

    #[test]
    fn an_unknown_layer_or_a_negative_blend_is_dropped() {
        let blends = parse("[wizardry]\nblend_in = 0.2\n\n[weapon]\nblend_in = -1\n");
        assert!(blends.0.is_empty(), "{blends:?}");
    }

    /// A clip's own number wins; a clip silent on one keeps the layer's.
    #[test]
    fn a_clip_overrides_only_what_it_states() {
        let blends = parse("[weapon]\nblend_in = 0.2\nblend_out = 0.3\n");
        let clip = crate::common::skeleton::clip::Clip { blend_in: Some(0.0), ..default() };
        assert_eq!(
            blends.of(Layer::Weapon, Some(&clip)),
            Blend { blend_in: 0.0, blend_out: 0.3 },
        );
    }
}
