//! The material editor's GPU path on Bevy's own device.
//!
//! texture-graph bakes into plain wgpu textures and asks its host to make them drawable by egui.
//! bevy_egui only draws `Image` assets, so a baked texture is given an invented [`AssetId`] — no
//! `Image` behind it — registered with bevy_egui as a weak handle, and in the render world a
//! [`GpuImage`] wrapping the baked texture is inserted under that id. bevy_egui rebuilds its
//! texture bind groups from `RenderAssets<GpuImage>` every frame, so it draws ours like any other.
//!
//! An id with no asset behind it gets no asset events, so nothing in Bevy removes our entries;
//! [`swap_in_baked_textures`] does, when the editor frees one.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::uuid::Uuid;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::{prepare_assets, RenderAssets};
use bevy::render::render_resource::{Sampler, TextureDescriptor};
use bevy::render::renderer::{RenderAdapter, RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy_egui::egui;
use bevy_egui::{EguiContexts, EguiTextureHandle};
use texture_graph_editor::{Gpu, TextureRegistry};
use texture_graph_gpu::{Baker, DeviceCtx};

/// Above any `AssetId` Bevy hands out by index, and unlikely to meet another invented one.
const BAKED_ID_BASE: u128 = 0x7e47_93a1_0000_4000_8000_0000_0000_0000;

/// The editor's baked textures, by the id egui knows them as. Extracted to the render world each
/// frame; cloning a wgpu handle is a reference count.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(RenderApp)]
pub struct BakedTextures {
    textures: HashMap<AssetId<Image>, (wgpu::Texture, wgpu::TextureView)>,
    by_egui_id: HashMap<egui::TextureId, AssetId<Image>>,
    next: u128,
}

/// Borrowed for one frame and lent to the editor. Through `EguiContexts`, which already holds
/// bevy_egui's user textures mutably.
pub struct BevyEguiTextures<'a, 'w, 's> {
    pub egui: &'a mut EguiContexts<'w, 's>,
    pub baked: &'a mut BakedTextures,
}

impl TextureRegistry for BevyEguiTextures<'_, '_, '_> {
    fn register(&mut self, texture: &wgpu::Texture, view: &wgpu::TextureView) -> egui::TextureId {
        let asset = AssetId::Uuid { uuid: Uuid::from_u128(BAKED_ID_BASE + self.baked.next) };
        self.baked.next += 1;
        let id = self.egui.add_image(EguiTextureHandle::Weak(asset));
        // bevy_egui blends in linear space and these bytes are sRGB-encoded, so the GPU decodes
        // them through an sRGB view; drawn as they are, every preview is washed out.
        let view = match texture.format() {
            wgpu::TextureFormat::Rgba8Unorm => texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(texture_graph_gpu::SRGB_VIEW[0]),
                ..default()
            }),
            _ => view.clone(),
        };
        self.baked.textures.insert(asset, (texture.clone(), view));
        self.baked.by_egui_id.insert(id, asset);
        id
    }

    fn free(&mut self, id: egui::TextureId) {
        let Some(asset) = self.baked.by_egui_id.remove(&id) else { return };
        self.egui.remove_image(asset);
        self.baked.textures.remove(&asset);
    }
}

/// Bevy's adapter, device and queue, so a bake is visible to the frame that draws it without a
/// copy. `None` until the renderer has made them.
pub fn shared_gpu(
    adapter: Option<&RenderAdapter>,
    device: Option<&RenderDevice>,
    queue: Option<&RenderQueue>,
) -> Option<Gpu> {
    let ctx = DeviceCtx::from_shared(
        Arc::new((**adapter?).clone()),
        Arc::new(device?.wgpu_device().clone()),
        Arc::new((**queue?).clone()),
    );
    Some(Gpu::new(Baker::new(ctx)))
}

/// Puts each baked texture into `RenderAssets<GpuImage>` under its invented id, after Bevy's own
/// image preparation and before bevy_egui builds bind groups, and takes freed ones out.
fn swap_in_baked_textures(
    baked: Res<BakedTextures>,
    device: Res<RenderDevice>,
    mut images: ResMut<RenderAssets<GpuImage>>,
    mut inserted: Local<HashSet<AssetId<Image>>>,
    mut sampler: Local<Option<Sampler>>,
) {
    inserted.retain(|asset| {
        let keep = baked.textures.contains_key(asset);
        if !keep {
            images.remove(*asset);
        }
        keep
    });
    let sampler = sampler.get_or_insert_with(|| {
        device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material editor previews"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..default()
        })
    });
    for (asset, (texture, view)) in &baked.textures {
        if !inserted.insert(*asset) {
            continue;
        }
        let descriptor = TextureDescriptor {
            label: None,
            size: texture.size(),
            mip_level_count: texture.mip_level_count(),
            sample_count: texture.sample_count(),
            dimension: texture.dimension(),
            format: texture.format(),
            usage: texture.usage(),
            view_formats: &[],
        };
        images.insert(
            *asset,
            GpuImage {
                texture: texture.clone().into(),
                texture_view: view.clone().into(),
                sampler: sampler.clone(),
                texture_descriptor: descriptor,
                texture_view_descriptor: None,
                had_data: true,
            },
        );
    }
}

pub struct MaterialGpuPlugin;

impl Plugin for MaterialGpuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BakedTextures>()
            .add_plugins(ExtractResourcePlugin::<BakedTextures>::default());
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(
                Render,
                swap_in_baked_textures
                    .in_set(RenderSystems::PrepareAssets)
                    .after(prepare_assets::<GpuImage>),
            );
        }
    }
}
