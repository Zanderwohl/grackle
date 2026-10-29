//! Choosing a room slot's material: every pack's materials as previews, and the palette's roles
//! beside them. The widget is [`crate::common::picker`].

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiTextureHandle};

use super::gpu::{BakedTextures, BevyDevice};
use super::library::MaterialLibrary;
use super::pit::{Pit, PIT};
use super::sky::Sky;
use super::{FILE_EXTENSION, MATERIALS_DIR};
use crate::common::assets::Assets as PackAssets;
use crate::common::palette::Palette;
use crate::common::picker::{every_in_packs, picker_button, Choice, PickerKind};
use crate::common::sky::SKYBOX;
use crate::get;

/// Two kinds, because a role resolves only to a material: the palette's own pickers offer none.
const MATERIALS_AND_ROLES: PickerKind = PickerKind::new("material_or_role");
const MATERIALS: PickerKind = PickerKind::new("material");

/// `None` is the default material. True when a choice was made.
pub fn material_button(ui: &mut egui::Ui, current: &mut Option<String>, offer_roles: bool) -> bool {
    let plain = get!("editor.material_picker.plain");
    let kind = if offer_roles { MATERIALS_AND_ROLES } else { MATERIALS };
    let Some(pick) = picker_button(ui, kind, current.as_deref(), Some(&plain)) else { return false };
    let changed = *current != pick;
    *current = pick;
    changed
}

#[derive(Default)]
pub(super) struct PickerState {
    open: bool,
    materials: Vec<String>,
    /// So a material baked again after a save frees the preview it had.
    registered: HashMap<String, AssetId<Image>>,
}

/// The directory is read again each time a picker opens, so a material saved since is there.
/// Bakes at most one material a frame, so opening onto a big pack does not stall.
#[allow(clippy::too_many_arguments)]
pub(super) fn publish_choices(
    mut contexts: EguiContexts,
    mut state: Local<PickerState>,
    mut library: ResMut<MaterialLibrary>,
    mut baked: ResMut<BakedTextures>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    packs: Res<PackAssets>,
    device: BevyDevice,
    palette: Res<Palette>,
    sky: Res<Sky>,
    pit: Res<Pit>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    // Both taken every frame, so a flag left by one does not outlive its frame.
    let with_roles = MATERIALS_AND_ROLES.take_open(ctx);
    if !(MATERIALS.take_open(ctx) | with_roles) {
        state.open = false;
        return;
    }
    if !state.open {
        state.open = true;
        state.materials = every_in_packs(MATERIALS_DIR, FILE_EXTENSION);
    }

    if let Some(name) = state.materials.iter().find(|name| !library.has_looked_for(name)) {
        library.ensure(name, &packs, &device, &mut baked, &mut standard);
    }

    let state = &mut *state;
    let mut previews = HashMap::new();
    for name in &state.materials {
        let Some(entry) = library.get(name) else { continue };
        let asset = entry.color_map();
        if let Some(old) = state.registered.insert(name.clone(), asset)
            && old != asset
        {
            contexts.remove_image(old);
        }
        previews.insert(name.as_str(), contexts.add_image(EguiTextureHandle::Weak(asset)));
    }

    let role = get!("editor.material_picker.role");
    let roles: Vec<Choice> = palette
        .every_role()
        .map(|(name, paint)| Choice {
            badge: Some(role.clone()),
            tooltip: Some(get!("editor.material_picker.role_named", "name", name)),
            preview: paint
                .unsided
                .iter()
                .chain(paint.sides.values())
                .find_map(|material| previews.get(material.as_str()).copied()),
            ..Choice::named(name.clone())
        })
        .collect();
    let mut materials = vec![Choice {
        preview: Some(contexts.add_image(EguiTextureHandle::Weak(sky.preview.id()))),
        ..Choice::named(SKYBOX)
    }];
    materials.push(Choice {
        preview: Some(contexts.add_image(EguiTextureHandle::Weak(pit.preview.id()))),
        ..Choice::named(PIT)
    });
    materials.extend(state.materials.iter().filter(|name| *name != SKYBOX && *name != PIT).map(|name| Choice {
        preview: previews.get(name.as_str()).copied(),
        ..Choice::named(name.clone())
    }));

    let Ok(ctx) = contexts.ctx_mut() else { return };
    if with_roles {
        MATERIALS_AND_ROLES.publish(ctx, roles.into_iter().chain(materials.iter().cloned()).collect());
    }
    MATERIALS.publish(ctx, materials);
}
