//! Props baked into mesh handles, by name.
//!
//! The prop editor rebuilds one document over and over; the game wants a
//! named prop once and then every frame after. Both bake the same way, and
//! **this is the only place that does it** — two bakers for one format is the
//! failure this codebase keeps naming, and it would show up as a weapon that
//! looks one way in the editor it was modelled in and another in the hand.
//!
//! Failures are cached as well as successes. A weapon naming a prop that is
//! not there would otherwise re-read a missing file every frame for the rest
//! of the match.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::prop::document;
use crate::prop::feature::Evaluated;
use crate::prop::hold::HoldSpec;
use crate::prop::surface::Surface;

/// Materials cached per surface for the life of the process.
///
/// A drag rebuilds the prop every frame, and minting a material per rebuild
/// would fill `Assets<StandardMaterial>` with identical greys.
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

/// The meshes an evaluated prop draws as, one per body per distinct surface.
///
/// Not merged across bodies: a body is the unit the evaluator produced and the
/// unit the editor outlines, and merging would save an entity at the cost of
/// the two disagreeing about what a body is.
pub fn parts(evaluated: &Evaluated) -> Vec<(Surface, Mesh)> {
    evaluated
        .bodies
        .iter()
        .flat_map(|(_, solid)| solid.meshes())
        .collect()
}

/// One prop, ready to hang on an entity.
pub struct BakedProp {
    pub parts: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    pub hold: HoldSpec,
}

/// Props baked from their documents, by the name a weapon knows them by.
#[derive(Resource, Default)]
pub struct PropCache {
    /// `None` means "looked for it and it is not there", which is an answer
    /// worth keeping.
    baked: HashMap<String, Option<BakedProp>>,
}

impl PropCache {
    /// The prop under this name, loading and baking it if this is the first
    /// ask. `None` once it is known not to exist.
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

    /// Whether this name has been asked about, whatever the answer was.
    pub fn knows(&self, name: &str) -> bool {
        self.baked.contains_key(name)
    }
}

fn bake(
    name: &str,
    packs: &PackAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    surfaces: &mut SurfaceMaterials,
) -> Option<BakedProp> {
    // Highest priority last, the way the weapon catalogue is read, so a pack
    // overriding a model wins.
    let mut found = None;
    for pack in default_packs().iter() {
        match document::load(packs, pack, name) {
            Ok(doc) => found = Some(doc),
            Err(document::PropFileError::Asset(_)) => {}
            Err(e) => error!("{e}"),
        }
    }

    let doc = found?;
    let built = doc.evaluate();
    for problem in &built.problems {
        warn!("prop {name}: {}", problem.message);
    }

    let parts = parts(&built)
        .into_iter()
        .map(|(surface, mesh)| (meshes.add(mesh), surfaces.get(surface, materials)))
        .collect();
    info!("Baked prop {name}");
    Some(BakedProp { parts, hold: doc.hold })
}
