//! Materials by name for the map, baked on Bevy's device.
//!
//! A name is looked for under every pack's `materials/` the first time a room asks for it, the
//! last pack holding it winning as for props, and baked once into a tile of [`TILE_TEXELS`] that
//! repeats across the wall ([`crate::tool::room::MATERIAL_TILE_METRES`] per repeat): colour,
//! glTF-packed metallic-roughness and a normal map, each mipmapped by texture-graph's
//! `MaterialMapper`.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::prelude::*;
use texture_graph_core::{EvalCtx, Graph};
use texture_graph_gpu::{Baker, MaterialMapper};

use super::gpu::{BakedTextures, BevyDevice, Encoding, Sampling};
use super::{FILE_EXTENSION, MATERIALS_DIR};
use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::tool::bakes::{RoomBake, RoomSlotMesh};
use crate::common::palette::Palette;
use crate::common::team::Side;
use crate::tool::room::RoomSurfaces;

pub const TILE_TEXELS: u32 = 512;

/// texture-graph's normals are +Y down the image (`n.y = -dh/drow`, DirectX's way) and Bevy's
/// bitangent points up it, so they are flipped. Unflipped, every bump is lit from the wrong side;
/// `texture_graph_normals_face_uphill_on_a_wall` is what decides this.
const FLIP_NORMAL_MAP_Y: bool = true;

pub struct LibraryEntry {
    pub material: Handle<StandardMaterial>,
    maps: [AssetId<Image>; 3],
    /// The file it was baked from, the last pack's.
    pub file: PathBuf,
}

#[derive(Resource, Default)]
pub struct MaterialLibrary {
    /// Made on first use: the render device does not exist while plugins build.
    baker: Option<(Baker, MaterialMapper)>,
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
                self.baker = device.baker().map(|baker| {
                    let mapper = MaterialMapper::new(&baker.ctx().device);
                    (baker, mapper)
                });
            }
            let (baker, mapper) = self.baker.as_mut()?;
            let entry = find(name, packs).and_then(|(graph, file)| {
                let out = baker
                    .bake_output(&graph, (TILE_TEXELS, TILE_TEXELS), &EvalCtx::default(), false)
                    .map_err(|e| error!("Baking material {name}: {e}"))
                    .ok()?;
                let maps = mapper.maps(baker.ctx(), &out);
                let maps = [
                    baked.insert(&maps.color, Sampling::Repeat, Encoding::Colour),
                    baked.insert(&maps.metallic_roughness, Sampling::Repeat, Encoding::Data),
                    baked.insert(&maps.normal, Sampling::Repeat, Encoding::Data),
                ];
                let material = standard.add(StandardMaterial {
                    base_color_texture: Some(BakedTextures::handle(maps[0])),
                    // Both multiply the texture's value, so 1 lets the map decide.
                    perceptual_roughness: 1.0,
                    metallic: 1.0,
                    metallic_roughness_texture: Some(BakedTextures::handle(maps[1])),
                    normal_map_texture: Some(BakedTextures::handle(maps[2])),
                    flip_normal_map_y: FLIP_NORMAL_MAP_Y,
                    ..default()
                });
                info!("Baked material {name} from {}", file.display());
                Some(LibraryEntry { material, maps, file })
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
            for map in entry.maps {
                baked.remove(map);
            }
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
    palette: Res<Palette>,
    rooms: Query<(&RoomSurfaces, Option<&Side>)>,
    mut parts: Query<(&RoomSlotMesh, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    for (part, mut drawn) in &mut parts {
        let named = rooms.get(part.room).ok().and_then(|(surfaces, side)| {
            palette.resolve(surfaces.of(part.slot)?, side.copied())
        });
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
    /// Height rising toward the bottom of the image is, on a wall, height rising toward the floor
    /// (`v` runs down the world), so the slope faces up. texture-graph writes that as `n.y < 0`.
    /// Rebuilt the way Bevy's shader does, from this mesh's own tangents, the world normal has to
    /// point up; if it points down, every bump on every wall is lit from the wrong side, and
    /// `FLIP_NORMAL_MAP_Y` is wrong.
    #[test]
    fn texture_graph_normals_face_uphill_on_a_wall() {
        use crate::common::world_surface::{room_share, slot_meshes};
        use crate::tool::room::{Room, RoomSlot};
        let rooms = [Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))];
        let (_, walls) =
            slot_meshes(&room_share(&rooms, 0)).into_iter().find(|(slot, _)| *slot == RoomSlot::Walls).unwrap();
        let normals = walls.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap().as_float3().unwrap();
        let Some(bevy::mesh::VertexAttributeValues::Float32x4(tangents)) = walls.attribute(Mesh::ATTRIBUTE_TANGENT)
        else {
            panic!("room walls have no tangents")
        };

        let slope = Vec3::new(0.0, -0.5, 1.0).normalize();
        let flip = if FLIP_NORMAL_MAP_Y { -1.0 } else { 1.0 };
        for (normal, tangent) in normals.iter().zip(tangents) {
            let n = Vec3::from(*normal);
            let t = Vec3::new(tangent[0], tangent[1], tangent[2]);
            let b = tangent[3] * n.cross(t);
            let world = t * slope.x + b * slope.y * flip + n * slope.z;
            assert!(world.y > 0.1, "a slope rising toward the floor faces {world} on the wall facing {n}");
        }
    }

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
