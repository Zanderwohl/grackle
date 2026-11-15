//! Drawing something once, out of sight, so its pipelines compile before it is
//! needed: Bevy compiles a pipeline on its first draw, synchronously on macOS
//! and wasm. The lens in [`crate::game::forward_camera`] has its own warm-up,
//! since it waits for a shader.

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

const WARM_UP_FRAMES: u32 = 10;

/// Despawned [`WARM_UP_FRAMES`] after it is spawned.
#[derive(Component, Default)]
pub struct WarmUp(u32);

/// Set up as the play camera is, since a camera's settings are part of a
/// pipeline's key.
pub fn warm_up_camera(images: &mut Assets<Image>, name: &'static str) -> impl Bundle {
    use bevy::camera::{Hdr, RenderTarget};
    use bevy::core_pipeline::tonemapping::Tonemapping;
    use bevy::render::render_resource::TextureFormat;

    let target = images.add(Image::new_target_texture(4, 4, TextureFormat::Rgba16Float, None));
    (
        WarmUp::default(),
        Camera3d::default(),
        RenderTarget::Image(target.into()),
        Hdr,
        Tonemapping::TonyMcMapface,
        bevy::camera::Projection::Perspective(PerspectiveProjection::default()),
        RenderLayers::layer(crate::common::render_layers::WARM_UP),
        Name::new(name),
    )
}

fn end_the_warm_ups(mut commands: Commands, mut drawn: Query<(Entity, &mut WarmUp)>) {
    for (entity, mut frames) in &mut drawn {
        frames.0 += 1;
        if frames.0 >= WARM_UP_FRAMES {
            commands.entity(entity).despawn();
        }
    }
}

pub struct WarmUpPlugin;

impl WarmUpPlugin {
    pub fn add_once(app: &mut App) {
        if !app.is_plugin_added::<WarmUpPlugin>() {
            app.add_plugins(WarmUpPlugin);
        }
    }
}

impl Plugin for WarmUpPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, end_the_warm_ups);
    }
}
