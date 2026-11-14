//! The material for things with no lightmap: standard, with the ambient scaled and the sun added
//! from the mesh's `MeshTag` ([`crate::common::air_light::pack`]). A mesh with no tag draws as plain
//! standard. See documentation/sun.md.

use bevy::asset::embedded_asset;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

pub type LitMaterial = ExtendedMaterial<StandardMaterial, AirLit>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct AirLit {
    /// Unit, toward the sun, and its illuminance in `w`.
    #[uniform(100)]
    pub sun: Vec4,
}

impl MaterialExtension for AirLit {
    fn fragment_shader() -> ShaderRef {
        "embedded://grackle/common/air_lit.wesl".into()
    }
}

pub fn lit(base: StandardMaterial) -> LitMaterial {
    ExtendedMaterial { base, extension: AirLit::default() }
}

/// Added through [`ensure`] by every plugin that makes the material, since those plugins also run
/// alone in tests.
pub struct AirLitPlugin;

impl Plugin for AirLitPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "air_lit.wesl");
        app.add_plugins(MaterialPlugin::<LitMaterial>::default());
    }
}

pub fn ensure(app: &mut App) {
    if !app.is_plugin_added::<AirLitPlugin>() {
        app.add_plugins(AirLitPlugin);
    }
}
