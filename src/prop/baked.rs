//! Props baked into mesh handles, by name. The only baker, shared by the prop
//! editor and the game so a weapon looks the same in both.
//!
//! Failures are cached too, or a missing prop is re-read every frame.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::prop::document::{self, PropDoc};
use crate::prop::feature::Evaluated;

use std::collections::BTreeMap;

use crate::common::skeleton::clip::{Clip, ClipSet};
use crate::prop::archetype::{Archetype, BreakableSpec, Collider, HatSpec, PickupSpec};
use crate::prop::hold::{HoldSpec, MuzzleSpec, ViewmodelSpec};
use crate::prop::surface::Surface;

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

    /// Everything that baked into something, leaving out the failures.
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
    ) {
        let previous = self.get(name).map(|baked| &baked.clips);
        let clips = publish_clips(&doc.animations, previous, clips);
        let mut baked = bake_doc(name, doc, meshes, materials, surfaces, clips);
        self.generation += 1;
        self.bakes += 1;
        baked.bake = self.bakes;
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
            Ok(doc) => found = Some(doc),
            Err(document::PropFileError::Asset(_)) => {}
            Err(e) => error!("{e}"),
        }
    }

    let doc = found?;
    let clips = publish_clips(&doc.animations, None, clips);
    Some(bake_doc(name, &doc, meshes, materials, surfaces, clips))
}

/// Put a prop's clips into `Assets<Clip>`, reusing the handle of any clip the
/// previous bake had under the same name.
///
/// Inserted only when the clip actually differs, so re-adopting an unchanged
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

    let parts = parts(&built)
        .into_iter()
        .map(|(surface, mesh)| (meshes.add(mesh), surfaces.get(surface, materials)))
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
    }
}
