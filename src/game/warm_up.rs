//! Cameras that draw everything on [`render_layers::WARM_UP`] for a few frames
//! at startup, so each material's pipelines are compiled before the material is
//! first seen. Bevy compiles a pipeline on the frame it is first drawn with,
//! synchronously on macOS and wasm.
//!
//! A pipeline is keyed by the view as well as the material, so there is one
//! camera per kind of view that draws the world: the play camera and the
//! editor's perspective viewport, the editor's orthographic viewports, the
//! scope's forward camera, and the class picker's portraits (orthographic, not
//! HDR). A view setting added to any of them must be added here too, or the
//! first draw through it stalls.
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

/// While shut, the warm-up cameras are off and the warm-up does not count down, so the frames
/// before it draw only what is on screen. The startup screen shuts it until its picture is up,
/// since every pipeline compiled in a frame holds that frame back from the window.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarmUpGate {
    pub open: bool,
}

impl Default for WarmUpGate {
    fn default() -> Self {
        Self { open: true }
    }
}

pub fn is_open(gate: Option<Res<WarmUpGate>>) -> bool {
    gate.is_none_or(|gate| gate.open)
}

pub fn ensure(app: &mut App) {
    if !app.is_plugin_added::<WarmUpPlugin>() {
        app.add_plugins(WarmUpPlugin);
    }
}

struct WarmUpPlugin;

impl Plugin for WarmUpPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_cameras).add_systems(Update, (follow_the_gate, end_the_warm_up.run_if(is_open)));
    }
}

/// Every frame rather than on change: a camera spawned shut is switched on by the first frame the
/// gate is open.
fn follow_the_gate(gate: Option<Res<WarmUpGate>>, mut cameras: Query<&mut Camera, With<WarmUp>>) {
    let open = is_open(gate);
    for mut camera in &mut cameras {
        if camera.is_active != open {
            camera.is_active = open;
        }
    }
}

fn spawn_cameras(mut commands: Commands, mut images: ResMut<Assets<Image>>, gate: Option<Res<WarmUpGate>>) {
    let active = is_open(gate);
    let mut camera = |tonemapping: Tonemapping, dither: DebandDither, projection: Projection, name: &'static str| {
        let target = images.add(Image::new_target_texture(4, 4, TextureFormat::Rgba16Float, None));
        commands.spawn((
            WarmUp,
            Camera3d::default(),
            Camera { is_active: active, ..default() },
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
    // As `class_picker` sets its portrait cameras up: not HDR, into an sRGB target.
    let portrait = images.add(Image::new_target_texture(4, 4, TextureFormat::Rgba8UnormSrgb, None));
    commands.spawn((
        WarmUp,
        Camera3d::default(),
        Camera { is_active: active, ..default() },
        RenderTarget::Image(portrait.into()),
        Projection::Orthographic(OrthographicProjection::default_3d()),
        RenderLayers::layer(render_layers::WARM_UP),
        Name::new("Warm-up camera (portrait)"),
    ));
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
