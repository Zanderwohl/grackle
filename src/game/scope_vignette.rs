//! Black closing in over the world as the viewed body's sights come up, until
//! only the weapon and its scope's lens are left.
//!
//! A quad drawn by the viewmodel camera behind the weapon: that camera draws
//! over the world, and depth keeps the quad behind the weapon.

use bevy::asset::embedded_asset;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::light::NotShadowCaster;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::game::player::{ViewMode, Viewed};
use crate::game::sights::{shown, Rechamber, Sights};
use crate::game::viewmodel::{ViewmodelCamera, VIEWMODEL_LAYER};
use crate::game::warm_up::{self, WarmUp};

const SHADER: &str = "embedded://grackle/game/scope_vignette.wesl";

/// Meters in front of the eye. A viewmodel's muzzle sits about a meter out.
const DEPTH: f32 = 3.0;

/// In half screen heights.
const SOFT: f32 = 0.6;

/// The scale of how far closed in the mesh's `MeshTag`, which a frame of the
/// raise can write without touching an asset. Matches the shader's.
const AMOUNT_STEPS: f32 = 65535.0;

/// Changes only with the window's shape.
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
struct Settings {
    aspect: f32,
    soft: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct VignetteMaterial {
    #[uniform(0)]
    settings: Settings,
}

impl VignetteMaterial {
    fn new(aspect: f32) -> Self {
        Self { settings: Settings { aspect, soft: SOFT } }
    }
}

impl Material for VignetteMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

#[derive(Component)]
struct ScopeVignette;

#[derive(Component)]
struct Vignetted;

#[derive(Resource)]
struct VignetteLook {
    mesh: Handle<Mesh>,
    material: Handle<VignetteMaterial>,
}

pub struct ScopeVignettePlugin;

impl Plugin for ScopeVignettePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "scope_vignette.wesl");
        warm_up::ensure(app);
        app.add_plugins(MaterialPlugin::<VignetteMaterial>::default())
            .add_systems(Startup, (make_the_look, warm_up).chain())
            .add_systems(
                Update,
                (give_the_viewmodel_a_vignette, close_in)
                    .chain()
                    .after(crate::game::viewmodel::ViewmodelSystems)
                    .run_if(crate::common::app_mode::simulating),
            );
    }
}

fn make_the_look(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<VignetteMaterial>>) {
    commands.insert_resource(VignetteLook {
        mesh: meshes.add(Rectangle::new(1.0, 1.0)),
        material: materials.add(VignetteMaterial::new(1.0)),
    });
}

fn warm_up(mut commands: Commands, look: Res<VignetteLook>) {
    commands.spawn((
        WarmUp,
        Mesh3d(look.mesh.clone()),
        MeshMaterial3d(look.material.clone()),
        RenderLayers::layer(crate::common::render_layers::WARM_UP),
        NoFrustumCulling,
        NotShadowCaster,
        // Too small to cover a pixel of the 4×4 target.
        Transform::from_xyz(0.0, 0.0, -1.0).with_scale(Vec3::splat(1.0e-4)),
        Visibility::default(),
    ));
}

fn give_the_viewmodel_a_vignette(
    mut commands: Commands,
    look: Res<VignetteLook>,
    cameras: Query<Entity, (With<ViewmodelCamera>, Without<Vignetted>)>,
) {
    for camera in &cameras {
        commands.entity(camera).insert(Vignetted).with_children(|camera| {
            camera.spawn((
                ScopeVignette,
                Mesh3d(look.mesh.clone()),
                MeshMaterial3d(look.material.clone()),
                RenderLayers::layer(VIEWMODEL_LAYER),
                NotShadowCaster,
                MeshTag::new(0),
                Transform::from_xyz(0.0, 0.0, -DEPTH),
                Visibility::Hidden,
            ));
        });
    }
}

/// Sized every frame, since the window's shape can change. Hidden in third
/// person, where no viewmodel is drawn.
fn close_in(
    look: Res<VignetteLook>,
    view: Option<Res<ViewMode>>,
    catalog: Res<crate::common::weapon::WeaponCatalog>,
    rechamber: Option<Res<Rechamber>>,
    viewed: Query<(Entity, &Sights, &crate::common::weapon::Equipped), Viewed>,
    cameras: Query<(&Camera, &Projection), With<ViewmodelCamera>>,
    mut vignettes: Query<(&ChildOf, &mut Transform, &mut Visibility, &mut MeshTag), With<ScopeVignette>>,
    mut materials: ResMut<Assets<VignetteMaterial>>,
) {
    let raised = match view.is_some_and(|view| view.shows_own_body()) {
        true => 0.0,
        false => viewed.single().map_or(0.0, |(body, sights, equipped)| {
            shown(body, sights, equipped, &catalog, rechamber.as_deref()).raised
        }),
    };
    for (camera, mut transform, mut visibility, mut tag) in &mut vignettes {
        let Ok((camera, Projection::Perspective(perspective))) = cameras.get(camera.parent()) else { continue };
        let Some(size) = camera.logical_viewport_size().filter(|size| size.y > 0.0) else { continue };
        let aspect = size.x / size.y;

        visibility.set_if_neq(match raised > 0.0 {
            true => Visibility::Inherited,
            false => Visibility::Hidden,
        });
        if raised <= 0.0 {
            continue;
        }
        // A little over the frustum, so its edge never shows.
        let height = 2.0 * DEPTH * (perspective.fov / 2.0).tan() * 1.05;
        transform.set_if_neq(Transform::from_xyz(0.0, 0.0, -DEPTH).with_scale(Vec3::new(height * aspect, height, 1.0)));

        tag.set_if_neq(MeshTag::new((raised.clamp(0.0, 1.0) * AMOUNT_STEPS).round() as u32));
        let wanted = VignetteMaterial::new(aspect).settings;
        if materials.get(&look.material).is_some_and(|material| material.settings != wanted)
            && let Some(mut material) = materials.get_mut(&look.material)
        {
            material.settings = wanted;
        }
    }
}
