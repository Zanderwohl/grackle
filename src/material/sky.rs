//! The GPU half of the map's sky ([`crate::common::sky`]): one unlit material, shared by every
//! slot painted [`SKYBOX`](crate::common::sky::SKYBOX), that colors a wall by the direction it
//! is seen in rather than by where it is. The wall is still a wall — it collides, takes shots and
//! hides what is behind it — and there is no sky anywhere else, so from outside the map, where
//! every surface is a back face, there is none to see.
//!
//! Sky slots are `NotShadowCaster`, so a sun added later shines in through them.

use bevy::asset::{embedded_asset, RenderAssetUsages};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;

use crate::common::sky::{SkyTime, STOPS};

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct SkyGradient {
    /// Oklab in `xyz`, elevation in `w`.
    stops: [Vec4; STOPS],
}

impl From<SkyTime> for SkyGradient {
    fn from(sky: SkyTime) -> Self {
        Self { stops: sky.oklab_stops().map(|(elevation, c)| Vec4::new(c.lightness, c.a, c.b, elevation)) }
    }
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct SkyMaterial {
    #[uniform(0)]
    gradient: SkyGradient,
}

impl Material for SkyMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://grackle/material/sky.wesl".into()
    }
}

/// The one sky material, and a picture of it for the material picker, both rewritten when the
/// map's [`SkyTime`] changes.
#[derive(Resource)]
pub struct Sky {
    pub material: Handle<SkyMaterial>,
    pub preview: Handle<Image>,
}

const PREVIEW_TEXELS: u32 = 64;
/// The preview runs from straight up down to a little under the horizon, which is the part of a
/// sky anybody looks at.
const PREVIEW_LOWEST: f32 = -0.2;

fn preview_of(sky: SkyTime) -> Image {
    let mut data = Vec::with_capacity((PREVIEW_TEXELS * PREVIEW_TEXELS * 4) as usize);
    for row in 0..PREVIEW_TEXELS {
        let elevation = 1.0 - (1.0 - PREVIEW_LOWEST) * row as f32 / (PREVIEW_TEXELS - 1) as f32;
        let texel = Srgba::from(sky.color_at(elevation)).to_u8_array();
        for _ in 0..PREVIEW_TEXELS {
            data.extend_from_slice(&texel);
        }
    }
    Image::new(
        Extent3d { width: PREVIEW_TEXELS, height: PREVIEW_TEXELS, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

impl FromWorld for Sky {
    fn from_world(world: &mut World) -> Self {
        let sky = world.get_resource::<SkyTime>().copied().unwrap_or_default();
        let material = world.resource_mut::<Assets<SkyMaterial>>().add(SkyMaterial { gradient: sky.into() });
        let preview = world.resource_mut::<Assets<Image>>().add(preview_of(sky));
        Self { material, preview }
    }
}

fn follow_the_sky_time(
    time: Res<SkyTime>,
    sky: Res<Sky>,
    mut materials: ResMut<Assets<SkyMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if !time.is_changed() {
        return;
    }
    if let Some(mut material) = materials.get_mut(&sky.material) {
        material.gradient = (*time).into();
    }
    if let Some(mut image) = images.get_mut(&sky.preview) {
        *image = preview_of(*time);
    }
}

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "sky.wesl");
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            .init_resource::<SkyTime>()
            .init_resource::<Sky>()
            .add_systems(Update, follow_the_sky_time);
    }
}
