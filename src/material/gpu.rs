//! texture-graph's bakes, drawn through Bevy on Bevy's own device: the material editor's previews
//! and the map's materials alike.
//!
//! texture-graph bakes into plain wgpu textures, and Bevy draws only `Image` assets. So a baked
//! texture is given an invented [`AssetId`] — no `Image` behind it — and in the render world a
//! [`GpuImage`] wrapping it is inserted under that id. A preview registers the id with bevy_egui
//! as a weak handle; a material names it as its texture through [`BakedTextures::handle`]. Both
//! look `RenderAssets<GpuImage>` up when they draw, so they draw ours like any other.
//!
//! An id with no asset behind it gets no asset events, so nothing in Bevy removes our entries;
//! [`swap_in_baked_textures`] does, when one is removed from [`BakedTextures`].
//!
//! Bakes submit from the main world while the render world runs on its own thread, and wgpu
//! refuses to configure a surface while another thread is submitting — it waits for the queue to
//! empty and fails if it did not stay empty. So every bake goes through [`submitting`], which the
//! render world shuts while it prepares the windows.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

use bevy::asset::uuid::Uuid;
use bevy::platform::collections::HashSet;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::{prepare_assets, RenderAssets};
use bevy::render::render_resource::{Sampler, TextureDescriptor};
use bevy::render::renderer::{RenderAdapter, RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::view::{create_surfaces, prepare_windows};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy_egui::egui;
use bevy_egui::{EguiContexts, EguiTextureHandle};
use texture_graph_editor::{Gpu, TextureRegistry};
use texture_graph_gpu::{Baker, DeviceCtx};

/// Above any `AssetId` Bevy hands out by index, and unlikely to meet another invented one.
const BAKED_ID_BASE: u128 = 0x7e47_93a1_0000_4000_8000_0000_0000_0000;

/// How a baked texture is sampled where it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    /// A preview in a panel.
    Clamp,
    /// A material on a wall, which tiles.
    Repeat,
}

/// What a baked texture's bytes mean, which decides how it is viewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// sRGB-encoded color, as texture-graph stores anything meant for display: viewed as sRGB so
    /// the GPU decodes it. Drawn as it is, Bevy, which blends in linear space, washes it out.
    Color,
    /// Linear data — roughness, metallic, a normal — read as it is stored.
    Data,
}

/// Every baked texture drawn through Bevy, by the invented id it is drawn under. Extracted to the
/// render world each frame; cloning a wgpu handle is a reference count.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(RenderApp)]
pub struct BakedTextures {
    textures: HashMap<AssetId<Image>, (wgpu::Texture, wgpu::TextureView, Sampling)>,
    by_egui_id: HashMap<egui::TextureId, AssetId<Image>>,
    next: u128,
}

impl BakedTextures {
    pub fn insert(&mut self, texture: &wgpu::Texture, sampling: Sampling, encoding: Encoding) -> AssetId<Image> {
        let asset = AssetId::Uuid { uuid: Uuid::from_u128(BAKED_ID_BASE + self.next) };
        self.next += 1;
        let format = match (encoding, texture.format()) {
            (Encoding::Color, wgpu::TextureFormat::Rgba8Unorm) => Some(texture_graph_gpu::SRGB_VIEW[0]),
            _ => None,
        };
        // Narrowed to sampling: a bake's outputs are storage textures too, and an sRGB format
        // cannot be a storage view.
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            format,
            usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
            ..default()
        });
        self.textures.insert(asset, (texture.clone(), view, sampling));
        asset
    }

    pub fn remove(&mut self, asset: AssetId<Image>) {
        self.textures.remove(&asset);
    }

    /// A handle to draw an inserted texture with, such as a material's `base_color_texture`. It
    /// owns nothing: the texture lives until [`BakedTextures::remove`].
    pub fn handle(asset: AssetId<Image>) -> Handle<Image> {
        match asset {
            AssetId::Uuid { uuid } => Handle::from(uuid),
            AssetId::Index { .. } => unreachable!("baked textures are only given invented ids"),
        }
    }
}

/// Borrowed for one frame and lent to the editor. Through `EguiContexts`, which already holds
/// bevy_egui's user textures mutably.
pub struct BevyEguiTextures<'a, 'w, 's> {
    pub egui: &'a mut EguiContexts<'w, 's>,
    pub baked: &'a mut BakedTextures,
}

impl TextureRegistry for BevyEguiTextures<'_, '_, '_> {
    fn register(&mut self, texture: &wgpu::Texture, _view: &wgpu::TextureView) -> egui::TextureId {
        let asset = self.baked.insert(texture, Sampling::Clamp, Encoding::Color);
        let id = self.egui.add_image(EguiTextureHandle::Weak(asset));
        self.baked.by_egui_id.insert(id, asset);
        id
    }

    fn free(&mut self, id: egui::TextureId) {
        let Some(asset) = self.baked.by_egui_id.remove(&id) else { return };
        self.egui.remove_image(asset);
        self.baked.remove(asset);
    }
}

/// Bevy's adapter, device and queue, so a bake is visible to the frame that draws it without a
/// copy. Absent until the renderer has made them, and in a test with no renderer.
#[derive(SystemParam)]
pub struct BevyDevice<'w> {
    adapter: Option<Res<'w, RenderAdapter>>,
    device: Option<Res<'w, RenderDevice>>,
    queue: Option<Res<'w, RenderQueue>>,
}

impl BevyDevice<'_> {
    pub fn baker(&self) -> Option<Baker> {
        let ctx = DeviceCtx::from_shared(
            Arc::new((**self.adapter.as_deref()?).clone()),
            Arc::new(self.device.as_deref()?.wgpu_device().clone()),
            Arc::new((**self.queue.as_deref()?).clone()),
        );
        Some(Baker::new(ctx))
    }

    pub fn gpu(&self) -> Option<Gpu> {
        self.baker().map(Gpu::new)
    }
}

struct GateState {
    configuring: bool,
    submitting: usize,
}

static GATE: (Mutex<GateState>, Condvar) =
    (Mutex::new(GateState { configuring: false, submitting: 0 }), Condvar::new());

/// Runs `f`, which submits on Bevy's queue, outside any surface configuration. Blocks for at most
/// the render world's window preparation.
pub fn submitting<R>(f: impl FnOnce() -> R) -> R {
    let (lock, signal) = &GATE;
    {
        let state = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = signal.wait_while(state, |s| s.configuring).unwrap_or_else(|e| e.into_inner());
        state.submitting += 1;
    }
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            let (lock, signal) = &GATE;
            lock.lock().unwrap_or_else(|e| e.into_inner()).submitting -= 1;
            signal.notify_all();
        }
    }
    let _done = Done;
    f()
}

fn shut_the_gate() {
    let (lock, signal) = &GATE;
    let state = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = signal.wait_while(state, |s| s.submitting > 0).unwrap_or_else(|e| e.into_inner());
    state.configuring = true;
}

fn open_the_gate() {
    let (lock, signal) = &GATE;
    lock.lock().unwrap_or_else(|e| e.into_inner()).configuring = false;
    signal.notify_all();
}

/// Puts each baked texture into `RenderAssets<GpuImage>` under its invented id, after Bevy's own
/// image preparation and before bevy_egui builds bind groups, and takes freed ones out.
fn swap_in_baked_textures(
    baked: Res<BakedTextures>,
    device: Res<RenderDevice>,
    mut images: ResMut<RenderAssets<GpuImage>>,
    mut inserted: Local<HashSet<AssetId<Image>>>,
    mut samplers: Local<Option<(Sampler, Sampler)>>,
) {
    inserted.retain(|asset| {
        let keep = baked.textures.contains_key(asset);
        if !keep {
            images.remove(*asset);
        }
        keep
    });
    let (clamp, repeat) = samplers.get_or_insert_with(|| {
        let linear = wgpu::SamplerDescriptor {
            label: Some("baked textures"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..default()
        };
        // Trilinear and anisotropic: a wall seen edge-on samples far down the mip chain along
        // one axis only, and isotropic filtering blurs it into a smear.
        let repeat = wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..linear.clone()
        };
        (device.create_sampler(&linear), device.create_sampler(&repeat))
    });
    for (asset, (texture, view, sampling)) in &baked.textures {
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
                sampler: match sampling {
                    Sampling::Clamp => clamp.clone(),
                    Sampling::Repeat => repeat.clone(),
                },
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
            render_app.add_systems(
                Render,
                (
                    shut_the_gate.before(create_surfaces).before(prepare_windows),
                    open_the_gate.after(prepare_windows),
                ),
            );
        }
    }
}
