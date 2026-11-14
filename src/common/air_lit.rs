//! The material for things that move, which carry no lightmap: the standard one, lit by where the
//! thing stands. [`crate::game::sunlit`] packs that into each mesh's `MeshTag`
//! ([`crate::common::air_light::pack`]): the light in the air, which scales the ambient term, and
//! whether it is in the sun, which adds the sun. A mesh with no tag draws as plain standard. See
//! "Things that move" in documentation/sun.md.

use bevy::asset::embedded_asset;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

pub type LitMaterial = ExtendedMaterial<StandardMaterial, AirLit>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct AirLit {
    /// Unit, toward the sun, and its illuminance in `w`. The same on every lit material, kept so
    /// by `game::sunlit`.
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

/// Registers the material. Every plugin that makes one adds this through [`ensure`], since bodies
/// and props are made by plugins that also stand alone in tests.
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
