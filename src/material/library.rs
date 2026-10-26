//! Materials by name for the map, baked on Bevy's device.
//!
//! A name is looked for under every pack's `materials/` the first time a room asks for it, the
//! last pack holding it winning as for props, and its colour is baked once into a tile of
//! [`TILE_TEXELS`] that repeats across the wall ([`crate::tool::room::MATERIAL_TILE_METRES`] per
//! repeat). Only colour so far: roughness and metallic are constants, and a normal map needs
//! tangents on the room meshes.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::prelude::*;
use texture_graph_core::{EvalCtx, Graph};
use texture_graph_gpu::Baker;

use super::gpu::{BakedTextures, BevyDevice, Sampling};
use super::{FILE_EXTENSION, MATERIALS_DIR};
use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::tool::bakes::{RoomBake, RoomSlotMesh};
use crate::tool::room::RoomSurfaces;

pub const TILE_TEXELS: u32 = 512;

pub struct LibraryEntry {
    pub material: Handle<StandardMaterial>,
    colour: AssetId<Image>,
    /// The file it was baked from, the last pack's.
    pub file: PathBuf,
}

#[derive(Resource, Default)]
pub struct MaterialLibrary {
    /// Made on first use: the render device does not exist while plugins build.
    baker: Option<Baker>,
    /// `None`: looked for and not there, or would not bake.
    entries: HashMap<String, Option<LibraryEntry>>,
}

impl MaterialLibrary {
    pub fn get(&self, name: &str) -> Option<&LibraryEntry> {
        self.entries.get(name).and_then(Option::as_ref)
    }

    /// `None` once it is known not to exist. Also `None`, but not remembered, while there is no
    /// device to bake on yet.
    pub fn ensure(
        &mut self,
        name: &str,
        packs: &PackAssets,
        device: &BevyDevice,
        baked: &mut BakedTextures,
        standard: &mut Assets<StandardMaterial>,
    ) -> Option<&LibraryEntry> {
        if !self.entries.contains_key(name) {
            if self.baker.is_none() {
                self.baker = device.baker();
            }
            let baker = self.baker.as_mut()?;
            let entry = find(name, packs).and_then(|(graph, file)| {
                let out = baker
                    .bake_output(&graph, (TILE_TEXELS, TILE_TEXELS), &EvalCtx::default(), false)
                    .map_err(|e| error!("Baking material {name}: {e}"))
                    .ok()?;
                let colour = baked.insert(&out.color, Sampling::Repeat);
                let material = standard.add(StandardMaterial {
                    base_color_texture: Some(BakedTextures::handle(colour)),
                    perceptual_roughness: 0.9,
                    ..default()
                });
                info!("Baked material {name} from {}", file.display());
                Some(LibraryEntry { material, colour, file })
            });
            if entry.is_none() {
                warn!("No material named {name}; drawing its surfaces plain");
            }
            self.entries.insert(name.to_owned(), entry);
        }
        self.get(name)
    }

    /// So the next room to ask bakes it again, as after a save.
    pub fn forget(&mut self, name: &str, baked: &mut BakedTextures, standard: &mut Assets<StandardMaterial>) {
        if let Some(Some(entry)) = self.entries.remove(name) {
            baked.remove(entry.colour);
            standard.remove(&entry.material);
        }
    }
}

/// Highest-priority pack last, so an overriding pack wins.
fn find(name: &str, packs: &PackAssets) -> Option<(Graph, PathBuf)> {
    let relative = format!("{MATERIALS_DIR}/{name}.{FILE_EXTENSION}");
    let mut found = None;
    for pack in default_packs() {
        let Ok(bytes) = packs.read(&pack, &relative) else { continue };
        let parsed = String::from_utf8(bytes)
            .map_err(|e| e.to_string())
            .and_then(|text| texture_graph_core::load_from_str(&text).map_err(|e| e.to_string()));
        match parsed {
            Ok(file) => found = Some((file.graph, pack.join(&relative))),
            Err(e) => error!("{}: {e}", pack.join(&relative).display()),
        }
    }
    found
}

/// Every frame rather than on change: a slot's material can arrive late (a bake that waited
/// for the device) or go away (a name cleared), and there are three slots a room.
#[allow(clippy::too_many_arguments)]
pub fn paint_rooms(
    mut library: ResMut<MaterialLibrary>,
    mut baked: ResMut<BakedTextures>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    mut bake: ResMut<RoomBake>,
    packs: Res<PackAssets>,
    device: BevyDevice,
    rooms: Query<&RoomSurfaces>,
    mut parts: Query<(&RoomSlotMesh, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    for (part, mut drawn) in &mut parts {
        let named = rooms.get(part.room).ok().and_then(|surfaces| surfaces.of(part.slot));
        let wanted = match named
            .and_then(|name| library.ensure(name, &packs, &device, &mut baked, &mut standard))
        {
            Some(entry) => entry.material.clone(),
            None => bake.material(&mut standard),
        };
        if drawn.0 != wanted {
            drawn.0 = wanted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use texture_graph_core::{evaluate_material, Sample, FLAT_W};

    /// A material repeats across a wall, so its tile's opposite edges have to be the same colour.
    /// Checked on the CPU, which agrees with the GPU bake to within a step of sRGB.
    #[test]
    fn every_shipped_material_tiles() {
        let Ok(entries) = std::fs::read_dir(Path::new("assets/default").join(MATERIALS_DIR)) else {
            return;
        };
        let ctx = EvalCtx::default();
        let mut checked = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(FILE_EXTENSION) {
                continue;
            }
            let graph = super::super::load(&path).unwrap_or_else(|e| panic!("{e}"));
            for step in 0..=16 {
                let t = step as f32 / 16.0;
                let at = |u, v| evaluate_material(&graph, Sample { u, v, w: FLAT_W }, &ctx).color;
                let close = |a: texture_graph_core::Color, b: texture_graph_core::Color| {
                    (a.l - b.l).abs() < 1e-3 && (a.chroma - b.chroma).abs() < 1e-3
                };
                assert!(close(at(0.0, t), at(1.0, t)), "{} has a seam across u at v = {t}", path.display());
                assert!(close(at(t, 0.0), at(t, 1.0)), "{} has a seam across v at u = {t}", path.display());
            }
            checked += 1;
        }
        assert!(checked > 0, "the default pack ships no materials to check");
    }
}
