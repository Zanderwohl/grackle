//! Cameras that draw everything on [`render_layers::WARM_UP`] for a few frames
//! at startup, so each material's pipelines are compiled before the material is
//! first seen. Bevy compiles a pipeline on the frame it is first drawn with,
//! synchronously on macOS and wasm.
//!
//! A pipeline is keyed by the view as well as the material, so there is one
//! camera per kind of view that draws the world: the play camera and the
//! editor's perspective viewport, the editor's orthographic viewports, and the
//! scope's forward camera. A view setting added to any of them must be added
//! here too, or the first draw through it stalls.
//!
//! A user spawns its meshes on the layer with [`WarmUp`] and calls [`ensure`].

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Hdr, RenderTarget};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

use crate::common::render_layers;

const FRAMES: u32 = 10;

/// Despawned when the warm-up ends.
#[derive(Component)]
pub struct WarmUp;

pub fn ensure(app: &mut App) {
    if !app.is_plugin_added::<WarmUpPlugin>() {
        app.add_plugins(WarmUpPlugin);
    }
}

struct WarmUpPlugin;

impl Plugin for WarmUpPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_cameras).add_systems(Update, end_the_warm_up);
    }
}

fn spawn_cameras(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut camera = |tonemapping: Tonemapping, dither: DebandDither, projection: Projection, name: &'static str| {
        let target = images.add(Image::new_target_texture(4, 4, TextureFormat::Rgba16Float, None));
        commands.spawn((
            WarmUp,
            Camera3d::default(),
            RenderTarget::Image(target.into()),
            Hdr,
            tonemapping,
            dither,
            projection,
            RenderLayers::layer(render_layers::WARM_UP),
            Name::new(name),
        ));
    };
    let perspective = || Projection::Perspective(PerspectiveProjection::default());
    camera(Tonemapping::TonyMcMapface, DebandDither::Enabled, perspective(), "Warm-up camera (play)");
    camera(
        Tonemapping::TonyMcMapface,
        DebandDither::Enabled,
        Projection::Orthographic(OrthographicProjection::default_3d()),
        "Warm-up camera (orthographic)",
    );
    camera(Tonemapping::None, DebandDither::Disabled, perspective(), "Warm-up camera (forward)");
}

fn end_the_warm_up(mut commands: Commands, mut frames: Local<u32>, drawn: Query<Entity, With<WarmUp>>) {
    if drawn.is_empty() {
        return;
    }
    *frames += 1;
    if *frames < FRAMES {
        return;
    }
    for entity in &drawn {
        commands.entity(entity).despawn();
    }
}
