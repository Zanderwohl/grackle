//! A lens that shows what is ahead: a [`Style::ForwardCamera`] surface in the
//! viewmodel is drawn from a square out of the middle of the play camera's
//! view, rendered by a camera of its own into an image and warped by
//! `lens.wesl` as a convex lens would.
//!
//! Only the viewmodel's copy is live. A body's hand, a placed prop, the prop
//! editor and a thumbnail draw the style's glass: somebody else's scope showing
//! *your* view would be wrong, and a lens in the world could be seen by the
//! very camera writing the texture it samples.
//!
//! Added in `main.rs` beside `GamePlugin` rather than inside it, since the
//! game's tests run with no render plugins to give a material to.

use bevy::asset::embedded_asset;
use bevy::camera::{Hdr, RenderTarget};
use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType, TextureFormat};
use bevy::shader::ShaderRef;

use crate::common::app_mode::AppMode;
use crate::common::render_layers;
use crate::game::player::{PlayerCamera, ViewMode, PLAY_CAMERA_ORDER};
use crate::game::viewmodel::{dress_the_viewmodel, ViewmodelPart};
use crate::prop::baked::SurfaceMaterials;
use crate::prop::surface::{Style, Tint};

/// The square's side as a fraction of the play camera's height: a half is
/// twice the magnification.
pub const FORWARD_CAMERA_PORTION: f32 = 0.5;

/// Side of the forward camera's square image, in texels.
pub const FORWARD_VIEW_TEXELS: u32 = 512;

/// How far the view is pulled toward the surface's tint.
const LENS_TINT: f32 = 0.2;
/// The middle is sampled from a square this much smaller than the rim's.
const LENS_BULGE: f32 = 0.4;
/// How much darker the rim is than the middle.
const LENS_VIGNETTE: f32 = 0.5;
/// How far red and blue part at the rim, as a fraction of the view.
const LENS_FRINGE: f32 = 0.015;

/// Before the viewmodel camera, which samples what this one drew.
const FORWARD_CAMERA_ORDER: isize = PLAY_CAMERA_ORDER - 1;

#[derive(Component)]
pub struct ForwardCamera;

#[derive(Clone, Copy, Debug, ShaderType)]
struct LensSettings {
    tint: Vec4,
    bulge: f32,
    vignette: f32,
    fringe: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct LensMaterial {
    #[uniform(0)]
    settings: LensSettings,
    #[texture(1)]
    #[sampler(2)]
    view: Handle<Image>,
}

impl Material for LensMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://grackle/game/lens.wesl".into()
    }
}

#[derive(Resource)]
pub struct Lenses {
    view: Handle<Image>,
    made: Vec<(Tint, Handle<LensMaterial>)>,
}

impl FromWorld for Lenses {
    fn from_world(world: &mut World) -> Self {
        let view = world.resource_mut::<Assets<Image>>().add(Image::new_target_texture(
            FORWARD_VIEW_TEXELS,
            FORWARD_VIEW_TEXELS,
            // Linear and unclamped: the viewmodel camera tonemaps it.
            TextureFormat::Rgba16Float,
            None,
        ));
        Self { view, made: Vec::new() }
    }
}

impl Lenses {
    fn get(&mut self, tint: Tint, materials: &mut Assets<LensMaterial>) -> Handle<LensMaterial> {
        if let Some((_, handle)) = self.made.iter().find(|(known, _)| *known == tint) {
            return handle.clone();
        }
        let [r, g, b] = tint.0.map(|channel| 1.0 + (channel - 1.0) * LENS_TINT);
        let handle = materials.add(LensMaterial {
            settings: LensSettings {
                tint: Color::srgb(r, g, b).to_linear().to_vec4(),
                bulge: LENS_BULGE,
                vignette: LENS_VIGNETTE,
                fringe: LENS_FRINGE,
            },
            view: self.view.clone(),
        });
        self.made.push((tint, handle.clone()));
        handle
    }
}

pub struct ForwardCameraPlugin;

impl Plugin for ForwardCameraPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "lens.wesl");
        app.add_plugins(MaterialPlugin::<LensMaterial>::default())
            .init_resource::<Lenses>()
            .add_systems(Startup, draw_gizmos_on_their_own_layer)
            .add_systems(
                Update,
                (swap_in_the_lenses, give_the_view_a_forward_camera, aim_the_forward_camera)
                    .chain()
                    .after(dress_the_viewmodel)
                    .run_if(in_state(AppMode::Play)),
            );
    }
}

/// At startup, after every plugin has registered its gizmo groups.
fn draw_gizmos_on_their_own_layer(mut store: ResMut<GizmoConfigStore>) {
    for (_, config, _) in store.iter_mut() {
        config.render_layers = RenderLayers::layer(render_layers::GIZMOS);
    }
}

/// The viewmodel copies the body's materials, so its lens is swapped in here.
fn swap_in_the_lenses(
    mut commands: Commands,
    surfaces: Res<SurfaceMaterials>,
    mut lenses: ResMut<Lenses>,
    mut materials: ResMut<Assets<LensMaterial>>,
    parts: Query<(Entity, &MeshMaterial3d<StandardMaterial>), Added<ViewmodelPart>>,
) {
    for (part, material) in &parts {
        let Some(surface) = surfaces.surface_of(material.id()) else { continue };
        if surface.style != Style::ForwardCamera {
            continue;
        }
        commands
            .entity(part)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert(MeshMaterial3d(lenses.get(surface.tint, &mut materials)));
    }
}

/// The field of view of a square `portion` of a view `fov` tall.
pub fn portion_fov(fov: f32, portion: f32) -> f32 {
    2.0 * ((fov / 2.0).tan() * portion).atan()
}

/// A child of the play camera, so it looks wherever the view does.
fn give_the_view_a_forward_camera(
    mut commands: Commands,
    lenses: Res<Lenses>,
    views: Query<Entity, With<PlayerCamera>>,
    existing: Query<(), With<ForwardCamera>>,
) {
    if !existing.is_empty() {
        return;
    }
    for camera in &views {
        commands.entity(camera).with_child((
            ForwardCamera,
            Camera3d::default(),
            Camera { order: FORWARD_CAMERA_ORDER, is_active: false, ..default() },
            RenderTarget::Image(lenses.view.clone().into()),
            Hdr,
            Tonemapping::None,
            DebandDither::Disabled,
            Projection::Perspective(PerspectiveProjection::default()),
            // Not the gizmos: an overlay drawn from the eye would sit in the lens.
            RenderLayers::layer(render_layers::WORLD),
            Transform::IDENTITY,
            Name::new("Forward camera"),
        ));
    }
}

/// Every frame, since the play camera's field of view is not ours to watch.
/// Off unless a live lens is on screen: it is the whole world drawn again.
fn aim_the_forward_camera(
    view: Res<ViewMode>,
    lenses: Query<(), (With<ViewmodelPart>, With<MeshMaterial3d<LensMaterial>>)>,
    plays: Query<&Projection, (With<PlayerCamera>, Without<ForwardCamera>)>,
    mut cameras: Query<(&mut Camera, &mut Projection, &ChildOf), With<ForwardCamera>>,
) {
    let showing = !view.shows_own_body() && !lenses.is_empty();
    for (mut camera, mut projection, parent) in &mut cameras {
        if camera.is_active != showing {
            camera.is_active = showing;
        }
        let Ok(Projection::Perspective(play)) = plays.get(parent.parent()) else { continue };
        let fov = portion_fov(play.fov, FORWARD_CAMERA_PORTION);
        if let Projection::Perspective(perspective) = projection.as_ref()
            && perspective.fov == fov
        {
            continue;
        }
        *projection = Projection::Perspective(PerspectiveProjection { fov, aspect_ratio: 1.0, ..play.clone() });
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::prop::surface::Surface;

    #[test]
    fn only_a_forward_camera_part_is_given_the_lens() {
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<LensMaterial>>();
        world.init_resource::<SurfaceMaterials>();
        world.init_resource::<Lenses>();
        let mut part = |style| {
            let material = world.resource_scope(|world, mut surfaces: Mut<SurfaceMaterials>| {
                surfaces.get(Surface::new(style, Tint::default()), &mut world.resource_mut())
            });
            world.spawn((ViewmodelPart, MeshMaterial3d(material))).id()
        };
        let (glass, barrel) = (part(Style::ForwardCamera), part(Style::Metal));

        world.run_system_once(swap_in_the_lenses).unwrap();

        assert!(world.get::<MeshMaterial3d<LensMaterial>>(glass).is_some(), "the glass is not a lens");
        assert!(world.get::<MeshMaterial3d<StandardMaterial>>(glass).is_none(), "the glass has two materials");
        assert!(world.get::<MeshMaterial3d<LensMaterial>>(barrel).is_none(), "the barrel became a lens");
    }

    #[test]
    fn the_whole_height_is_the_whole_view() {
        let fov = 90f32.to_radians();
        assert!((portion_fov(fov, 1.0) - fov).abs() < 1e-6);
        // tan(45°) / 2 = 0.5, so a half is about 53°.
        assert!((portion_fov(fov, 0.5) - 2.0 * 0.5f32.atan()).abs() < 1e-6);
    }
}
