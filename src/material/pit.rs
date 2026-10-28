//! A slot painted [`PIT`] is drawn black whatever lights it: unlit, so no light, lightmap or
//! ambient reaches it. Meant for the floor under a kill trigger, where a pit's bottom should
//! read as having none.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// The material name a slot gives to be drawn as a pit. Shadows a pack material of the same name.
pub const PIT: &str = "pit";

/// The one pit material, and a picture of it for the material picker.
#[derive(Resource)]
pub struct Pit {
    pub material: Handle<StandardMaterial>,
    pub preview: Handle<Image>,
}

impl FromWorld for Pit {
    fn from_world(world: &mut World) -> Self {
        let material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial { base_color: Color::BLACK, unlit: true, ..default() });
        let preview = world.resource_mut::<Assets<Image>>().add(Image::new_fill(
            Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        ));
        Self { material, preview }
    }
}
