//! Props baked into mesh handles, by name. The only baker, shared by the prop
//! editor and the game so a weapon looks the same in both.
//!
//! Failures are cached too, or a missing prop is re-read every frame.

use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::prop::document::{self, PropDoc};
use crate::prop::feature::Evaluated;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::common::skeleton::clip::{Clip, ClipSet};
use crate::prop::archetype::{Archetype, BreakableSpec, Collider, HatSpec, PickupSpec};
use crate::prop::hold::{HoldSpec, MuzzleSpec, ViewmodelSpec};
use crate::prop::surface::{Style, Surface};

/// Materials cached per surface for the life of the process, since a drag
/// rebuilds the prop every frame.
#[derive(Resource, Default)]
pub struct SurfaceMaterials(Vec<(Surface, Handle<StandardMaterial>)>);

impl SurfaceMaterials {
    pub fn get(
        &mut self,
        surface: Surface,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        match self.0.iter().find(|(known, _)| *known == surface) {
            Some((_, handle)) => handle.clone(),
            None => {
                let handle = materials.add(surface.material());
                self.0.push((surface, handle.clone()));
                handle
            }
        }
    }

    pub fn surface_of(&self, material: AssetId<StandardMaterial>) -> Option<Surface> {
        self.0.iter().find(|(_, handle)| handle.id() == material).map(|(surface, _)| *surface)
    }
}

/// One mesh per body per distinct surface. Not merged across bodies, because
/// the body is the unit the editor outlines and picks.
pub fn parts(evaluated: &Evaluated) -> Vec<(Surface, Mesh)> {
    evaluated
        .bodies
        .iter()
        .flat_map(|(_, solid)| solid.meshes())
        .collect()
}

/// Spreads the square view once across each lens as the viewmodel's eye sees
/// it, after `facing`: down `-Z`, `+X` right, `+Y` up. The kernel's UVs are in
/// meters, which would tile the view a few millimeters wide.
fn fit_the_view_to_the_lens(mesh: &mut Mesh, facing: Quat) {
    let Some(VertexAttributeValues::Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        return;
    };
    let Some(Indices::U32(indices)) = mesh.indices() else { return };
    let uvs = lens_uvs(positions, indices, facing);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
}

/// Each lens is a connected piece of the mesh, so two eyepieces on one body
/// each get the whole view.
fn lens_uvs(positions: &[[f32; 3]], indices: &[u32], facing: Quat) -> Vec<[f32; 2]> {
    // The kernel gives every polygon its own vertices, so pieces are found by
    // position. A tenth of a millimeter is well under any prop's detail.
    let weld = |p: &[f32; 3]| p.map(|c| (c * 1e4).round() as i64);
    let mut welded: HashMap<[i64; 3], usize> = HashMap::default();
    let ids: Vec<usize> = positions
        .iter()
        .map(|p| {
            let next = welded.len();
            *welded.entry(weld(p)).or_insert(next)
        })
        .collect();

    let mut parent: Vec<usize> = (0..welded.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for triangle in indices.chunks_exact(3) {
        let a = root(&mut parent, ids[triangle[0] as usize]);
        for &corner in &triangle[1..] {
            let b = root(&mut parent, ids[corner as usize]);
            parent[b] = a;
        }
    }

    let seen: Vec<Vec2> = positions.iter().map(|p| (facing * Vec3::from_array(*p)).truncate()).collect();
    let mut bounds: HashMap<usize, (Vec2, Vec2)> = HashMap::default();
    let pieces: Vec<usize> = ids.iter().map(|&id| root(&mut parent, id)).collect();
    for (piece, p) in pieces.iter().zip(&seen) {
        let (min, max) = bounds.entry(*piece).or_insert((Vec2::MAX, Vec2::MIN));
        *min = min.min(*p);
        *max = max.max(*p);
    }
    pieces
        .iter()
        .zip(&seen)
        .map(|(piece, p)| {
            let (min, max) = bounds[piece];
            let center = (min + max) / 2.0;
            // The larger side: a lens that is not round is cropped, not stretched.
            let side = (max - min).max_element().max(f32::EPSILON);
            [0.5 + (p.x - center.x) / side, 0.5 - (p.y - center.y) / side]
        })
        .collect()
}

pub struct BakedProp {
    pub parts: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    pub hold: HoldSpec,
    pub viewmodel: ViewmodelSpec,
    pub muzzle: Option<MuzzleSpec>,
    /// Handles, as `parts` are: a re-bake of the same name refills the same
    /// handle, so a body mid-deploy plays the saved clip on its next frame.
    pub clips: BTreeMap<String, Handle<Clip>>,
    pub archetype: Archetype,
    pub colliders: Vec<Collider>,
    pub breakable: BreakableSpec,
    pub pickup: PickupSpec,
    pub hat: HatSpec,
    /// [`PropCache::bakes`] as it stood after this bake, so a replaced entry
    /// can be told from the one it replaced.
    pub bake: u64,
    /// The file it was read from; `None` for a model handed over in memory,
    /// such as one the server sent.
    pub file: Option<PathBuf>,
}

#[derive(Resource, Default)]
pub struct PropCache {
    /// `None`: looked for and not there.
    baked: HashMap<String, Option<BakedProp>>,
    /// Bumped whenever an entry is replaced, so anything drawn from this
    /// looks again — including a client that cached a failure before the
    /// server's model arrived.
    generation: u64,
    /// Bumped on every bake, first or replacing, so something that wants to
    /// know a prop arrived need not walk the cache every frame.
    bakes: u64,
}

impl PropCache {
    /// `None` once it is known not to exist.
    pub fn ensure(
        &mut self,
        name: &str,
        packs: &PackAssets,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        surfaces: &mut SurfaceMaterials,
        clips: &mut Assets<Clip>,
    ) -> Option<&BakedProp> {
        if !self.baked.contains_key(name) {
            let mut baked = bake(name, packs, meshes, materials, surfaces, clips);
            self.bakes += 1;
            if let Some(baked) = &mut baked {
                baked.bake = self.bakes;
            }
            self.baked.insert(name.to_owned(), baked);
        }
        self.baked.get(name).and_then(Option::as_ref)
    }

    /// What is baked under this name, without baking it if it is not.
    pub fn get(&self, name: &str) -> Option<&BakedProp> {
        self.baked.get(name).and_then(Option::as_ref)
    }

    pub fn knows(&self, name: &str) -> bool {
        self.baked.contains_key(name)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn bakes(&self) -> u64 {
        self.bakes
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &BakedProp)> {
        self.baked.iter().filter_map(|(name, baked)| Some((name.as_str(), baked.as_ref()?)))
    }

    /// Replaces whatever was cached, which is often a failure from looking in
    /// the local pack before the server's document arrived. A clip the previous
    /// bake also had keeps its handle, so a mid-round save reaches its players.
    pub fn adopt(
        &mut self,
        name: &str,
        doc: &PropDoc,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        surfaces: &mut SurfaceMaterials,
        clips: &mut Assets<Clip>,
        file: Option<&Path>,
    ) {
        let previous = self.get(name).map(|baked| &baked.clips);
        let clips = publish_clips(&doc.animations, previous, clips);
        let mut baked = bake_doc(name, doc, meshes, materials, surfaces, clips);
        self.generation += 1;
        self.bakes += 1;
        baked.bake = self.bakes;
        baked.file = file.map(Path::to_path_buf);
        self.baked.insert(name.to_owned(), Some(baked));
    }
}

fn bake(
    name: &str,
    packs: &PackAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    surfaces: &mut SurfaceMaterials,
    clips: &mut Assets<Clip>,
) -> Option<BakedProp> {
    // Highest priority last, so an overriding pack wins.
    let mut found = None;
    for pack in default_packs().iter() {
        match document::load(packs, pack, name) {
            Ok(doc) => found = Some((doc, document::prop_path(pack, name))),
            Err(document::PropFileError::Asset(_)) => {}
            Err(e) => error!("{e}"),
        }
    }

    let (doc, file) = found?;
    let clips = publish_clips(&doc.animations, None, clips);
    let mut baked = bake_doc(name, &doc, meshes, materials, surfaces, clips);
    baked.file = Some(file);
    Some(baked)
}

/// Reuses the handle of any clip the previous bake had under the same name,
/// and refills it only when the clip differs, so re-adopting an unchanged
/// model is not a `Modified` event for every clip in it.
fn publish_clips(
    set: &ClipSet,
    previous: Option<&BTreeMap<String, Handle<Clip>>>,
    clips: &mut Assets<Clip>,
) -> BTreeMap<String, Handle<Clip>> {
    set.clips
        .iter()
        .map(|(name, clip)| {
            let handle = match previous.and_then(|previous| previous.get(name)) {
                Some(handle) => {
                    crate::common::skeleton::play::refill(clips, handle, clip);
                    handle.clone()
                }
                None => clips.add(clip.clone()),
            };
            (name.clone(), handle)
        })
        .collect()
}

fn bake_doc(
    name: &str,
    doc: &PropDoc,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    surfaces: &mut SurfaceMaterials,
    clips: BTreeMap<String, Handle<Clip>>,
) -> BakedProp {
    let built = doc.evaluate();
    for problem in &built.problems {
        warn!("prop {name}: {}", problem.message);
    }

    let facing = doc.viewmodel.rotation();
    let parts = parts(&built)
        .into_iter()
        .map(|(surface, mut mesh)| {
            if surface.style == Style::ForwardCamera {
                fit_the_view_to_the_lens(&mut mesh, facing);
            }
            (meshes.add(mesh), surfaces.get(surface, materials))
        })
        .collect();
    info!("Baked prop {name}");
    BakedProp {
        parts,
        hold: doc.hold.clone(),
        viewmodel: doc.viewmodel,
        muzzle: doc.muzzle,
        clips,
        archetype: doc.archetype,
        colliders: doc.colliders.clone(),
        breakable: doc.breakable,
        pickup: doc.pickup,
        hat: doc.hat.clone(),
        bake: 0,
        file: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    fn assert_spans_the_view(uvs: &[[f32; 2]]) {
        let (min, max) = uvs.iter().fold((Vec2::MAX, Vec2::MIN), |(min, max), uv| {
            (min.min(Vec2::from_array(*uv)), max.max(Vec2::from_array(*uv)))
        });
        assert!(min.abs_diff_eq(Vec2::ZERO, 1e-5) && max.abs_diff_eq(Vec2::ONE, 1e-5), "{min} to {max}");
    }

    /// A square seen face on, centered at `at`, normal `+Z`.
    fn square(at: Vec3, positions: &mut Vec<[f32; 3]>, indices: &mut Vec<u32>) {
        let base = positions.len() as u32;
        for corner in [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] {
            positions.push((at + Vec3::new(corner[0], corner[1], 0.0) * 0.02).to_array());
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    #[test]
    fn the_shipped_scope_glass_shows_the_whole_view() {
        let doc = document::parse(include_str!("../../assets/default/props/sniper_rifle.gpp")).unwrap();
        let built = doc.evaluate();
        let mut lenses = parts(&built).into_iter().filter(|(surface, _)| surface.style == Style::ForwardCamera);
        let (_, mut mesh) = lenses.next().expect("the rifle has no scope glass");
        fit_the_view_to_the_lens(&mut mesh, doc.viewmodel.rotation());
        let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0) else {
            panic!("no UVs");
        };
        assert_spans_the_view(uvs);
    }

    #[test]
    fn two_eyepieces_each_get_the_whole_view() {
        let (mut positions, mut indices) = (Vec::new(), Vec::new());
        square(Vec3::new(-0.05, 0.0, 0.0), &mut positions, &mut indices);
        square(Vec3::new(0.05, 0.0, 0.0), &mut positions, &mut indices);
        let uvs = lens_uvs(&positions, &indices, Quat::IDENTITY);
        assert_spans_the_view(&uvs[..4]);
        assert_spans_the_view(&uvs[4..]);
    }

    #[test]
    fn the_view_is_upright_from_behind() {
        let (mut positions, mut indices) = (Vec::new(), Vec::new());
        square(Vec3::ZERO, &mut positions, &mut indices);
        let uvs = lens_uvs(&positions, &indices, Quat::IDENTITY);
        // The first corner is bottom left: image rows run downward.
        assert_eq!(uvs[0], [0.0, 1.0]);
    }

    #[test]
    fn a_lens_is_fitted_as_the_eye_sees_it() {
        // Facing +X in the prop, turned to face the eye.
        let (mut positions, mut indices) = (Vec::new(), Vec::new());
        square(Vec3::ZERO, &mut positions, &mut indices);
        let turn = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        for p in &mut positions {
            *p = (turn * Vec3::from_array(*p)).to_array();
        }
        let uvs = lens_uvs(&positions, &indices, turn.inverse());
        assert_spans_the_view(&uvs);
    }
}
