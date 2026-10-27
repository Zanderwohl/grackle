//! Reading how fast each animation layer fades, from a pack's `layers.toml`.
//!
//! A bad entry is dropped with a line saying why, and that layer snaps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::common::assets::{AssetError, Assets};
use crate::common::skeleton::play::{Blend, Layer, LayerBlends};

const LAYERS_FILE: &str = "layers.toml";

/// Lowest priority first; a later pack replaces a layer's whole entry.
pub fn load_blends(assets: &Assets, packs: &[PathBuf]) -> LayerBlends {
    let mut blends = LayerBlends::default();
    for pack in packs {
        load_pack(assets, pack, &mut blends);
    }
    blends
}

fn load_pack(assets: &Assets, pack: &Path, blends: &mut LayerBlends) {
    let named = format!("{}/{LAYERS_FILE}", pack.display());
    let text = match assets.read(pack, LAYERS_FILE) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => return error!("{named} is not valid UTF-8: {e}"),
        },
        Err(AssetError::NotFound(_)) => return,
        Err(e) => return error!("{e}"),
    };
    merge(&named, &text, blends);
}

fn merge(named: &str, text: &str, blends: &mut LayerBlends) {
    let file: BTreeMap<String, Blend> = match toml::from_str(text) {
        Ok(file) => file,
        Err(e) => return error!("{named} will not parse: {e}"),
    };
    for (key, blend) in file {
        let Some(layer) = Layer::named(&key) else {
            warn!("{named}: there is no layer called `{key}`");
            continue;
        };
        let seconds = [blend.blend_in, blend.blend_out];
        if seconds.iter().any(|seconds| !seconds.is_finite() || *seconds < 0.0) {
            warn!("{named}: `{key}` is dropped: a blend is a number of seconds, zero or more");
            continue;
        }
        blends.0.insert(layer, blend);
    }
}

pub fn load_blends_at_startup(assets: Res<Assets>, mut blends: ResMut<LayerBlends>) {
    *blends = load_blends(&assets, &crate::common::assets::default_packs());
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
        let mut blends = LayerBlends::default();
        merge("test.toml", text, &mut blends);
        blends
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
