//! Choosing a prop by name: every pack's props, as the pictures [`thumbnail`](super::thumbnail)
//! takes of them. The widget is [`crate::common::picker`].

use std::path::PathBuf;

use bevy::ecs::system::SystemParam;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiTextureHandle};

use super::baked::{PropCache, SurfaceMaterials};
use super::archetype::Archetype;
use super::document::{load, prop_path, PROPS_DIR, PROP_EXTENSION};
use super::thumbnail::Thumbnails;
use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::common::picker::{every_in_packs, picker_button, Choice, PickerKind};
use crate::common::skeleton::clip::Clip;

const PROPS: PickerKind = PickerKind::new("prop");

/// `None` is no model.
pub fn prop_button(ui: &mut egui::Ui, current: Option<&str>, plain: &str) -> Option<Option<String>> {
    picker_button(ui, PROPS, current, Some(plain))
}

/// Pictures of props as egui textures: read off the thumbnail cache where it has a fresh one, and
/// baked so they are photographed where it does not.
#[derive(SystemParam)]
pub struct PropPreviews<'w, 's> {
    contexts: EguiContexts<'w, 's>,
    cache: ResMut<'w, PropCache>,
    packs: Res<'w, PackAssets>,
    surfaces: ResMut<'w, SurfaceMaterials>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    clips: ResMut<'w, Assets<Clip>>,
    thumbnails: Option<ResMut<'w, Thumbnails>>,
    /// The file each name is read from, `None` when no pack has one.
    sources: Local<'s, HashMap<String, Option<PathBuf>>>,
}

impl PropPreviews<'_, '_> {
    pub fn ctx(&mut self) -> Option<&mut egui::Context> {
        self.contexts.ctx_mut().ok()
    }

    /// Bakes at most one a frame, so opening onto a big pack does not stall.
    pub fn want_pictures<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        let Some(thumbnails) = self.thumbnails.as_mut() else { return };
        let mut to_bake = None;
        for name in names {
            if thumbnails.prop(name).is_some() || self.cache.knows(name) {
                continue;
            }
            let source = self.sources.entry(name.to_owned()).or_insert_with(|| source_of(name));
            let Some(source) = source.clone() else { continue };
            if !thumbnails.look_on_disk(name, source) {
                to_bake.get_or_insert(name);
            }
        }
        if let Some(name) = to_bake {
            self.cache.ensure(name, &self.packs, &mut self.meshes, &mut self.materials, &mut self.surfaces, &mut self.clips);
        }
    }

    /// A thumbnail's handle is kept across a retake, so registering it once is enough.
    pub fn picture(&mut self, name: &str) -> Option<egui::TextureId> {
        let handle = self.thumbnails.as_ref()?.prop(name)?.id();
        Some(self.contexts.add_image(EguiTextureHandle::Weak(handle)))
    }

    /// Changes whenever a picture arrives.
    pub fn pictures(&self) -> usize {
        self.thumbnails.as_ref().map_or(0, |thumbnails| thumbnails.len())
    }

    /// Parsed rather than baked: reading the file is cheap, evaluating it is not.
    fn archetype(&self, name: &str) -> Option<Archetype> {
        if let Some(baked) = self.cache.get(name) {
            return Some(baked.archetype);
        }
        let pack = default_packs().into_iter().rev().find(|pack| prop_path(pack, name).exists())?;
        load(&self.packs, &pack, name).ok().map(|doc| doc.archetype)
    }
}

/// Highest priority last, as [`PropCache`] reads them, so an overriding pack wins.
fn source_of(name: &str) -> Option<PathBuf> {
    default_packs().iter().rev().map(|pack| prop_path(pack, name)).find(|path| path.exists())
}

/// The directory is read again each time a picker opens, so a prop saved since is there.
pub(super) fn publish_prop_choices(
    mut previews: PropPreviews,
    mut props: Local<Option<Vec<(String, Option<String>)>>>,
) {
    let Some(ctx) = previews.ctx() else { return };
    if !PROPS.take_open(ctx) {
        *props = None;
        return;
    }
    let props = props.get_or_insert_with(|| {
        every_in_packs(PROPS_DIR, PROP_EXTENSION)
            .into_iter()
            .map(|name| {
                let archetype = previews.archetype(&name).map(Archetype::name);
                (name, archetype)
            })
            .collect()
    });
    previews.want_pictures(props.iter().map(|(name, _)| name.as_str()));

    let choices = props
        .iter()
        .map(|(name, archetype)| Choice {
            preview: previews.picture(name),
            badge: archetype.clone(),
            ..Choice::named(name.clone())
        })
        .collect();
    let Some(ctx) = previews.ctx() else { return };
    PROPS.publish(ctx, choices);
}
