//! Props baked into mesh handles, by name. The only baker, shared by the prop
//! editor and the game so a weapon looks the same in both.
//!
//! Failures are cached too, or a missing prop is re-read every frame.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::prop::document::{self, PropDoc};
use crate::prop::feature::Evaluated;
use std::sync::Arc;

use crate::common::skeleton::clip::ClipSet;
use crate::prop::hold::{HoldSpec, ViewmodelSpec};
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
    /// Shared, since everybody holding one plays the same clips.
    pub clips: Arc<ClipSet>,
}

#[derive(Resource, Default)]
pub struct PropCache {
    /// `None`: looked for and not there.
    baked: HashMap<String, Option<BakedProp>>,
    /// Bumped whenever an entry is replaced, so anything drawn from this
    /// looks again — including a client that cached a failure before the
    /// server's model arrived.
    generation: u64,
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
    ) -> Option<&BakedProp> {
        if !self.baked.contains_key(name) {
            let baked = bake(name, packs, meshes, materials, surfaces);
            self.baked.insert(name.to_owned(), baked);
        }
        self.baked.get(name).and_then(Option::as_ref)
    }

    pub fn knows(&self, name: &str) -> bool {
        self.baked.contains_key(name)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Replaces whatever was cached, which is often a failure from looking in
    /// the local pack before the server's document arrived.
    pub fn adopt(
        &mut self,
        name: &str,
        doc: &PropDoc,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        surfaces: &mut SurfaceMaterials,
    ) {
        let baked = bake_doc(name, doc, meshes, materials, surfaces);
        self.baked.insert(name.to_owned(), Some(baked));
        self.generation += 1;
    }
}

fn bake(
    name: &str,
    packs: &PackAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    surfaces: &mut SurfaceMaterials,
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
    Some(bake_doc(name, &doc, meshes, materials, surfaces))
}

fn bake_doc(
    name: &str,
    doc: &PropDoc,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    surfaces: &mut SurfaceMaterials,
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
        clips: Arc::new(doc.animations.clone()),
    }
}
