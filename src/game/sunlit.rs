//! The sun on things that move: bodies, corpses, placed props and projectiles, which carry no
//! lightmap. One `DirectionalLight` on [`render_layers::SUNLIT`] and
//! [`render_layers::SUNLIT_VIEWMODEL`], with no wall on either, and each moving thing's meshes on
//! the layer while a ray from it reaches sky. See "Things that move" in documentation/sun.md.
//!
//! It is a switch, not a fade: a body half out of a doorway is one or the other. Nothing reads it
//! but the renderer.
//!
//! The same pass hands each one the light in the air where it stands ([`AirLight`]), as a
//! `MeshTag` that a body's material ([`BodyMaterial`](crate::game::body_mesh::BodyMaterial)) scales
//! its ambient by. Anything else that moves ignores its tag and keeps the global ambient.

use std::f32::consts::PI;

use bevy::camera::visibility::RenderLayers;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::common::air_light::{self, AirLight};
use crate::common::app_mode::AppMode;
use crate::common::lightmap::{BASE_AMBIENT, SUN_BRIGHTNESS};
use crate::common::render_layers::{SUNLIT, SUNLIT_VIEWMODEL, VIEWMODEL, WORLD};
use crate::common::skeleton::rig::Skeleton;
use crate::common::sky::SunPosition;
use crate::game::player::LocalPlayer;
use crate::game::projectile::Projectile;
use crate::game::world_prop::WorldProp;
use crate::tool::sun_bake::{OpenSky, SunBake};

/// Where a body's sun ray starts, above its feet: about its chest.
const CHEST: f32 = 1.2;

/// Where any other moving thing's sun ray starts, above its origin, so a prop standing on a
/// floor does not start its ray in the floor.
const ABOVE_ORIGIN: f32 = 0.1;

#[derive(Component)]
pub struct Sun;

/// Lambertian diffuse in Bevy is albedo over π times illuminance, and a lightmap texel is albedo
/// times its value, so this makes a body facing the sun as bright as a wall facing it.
const SUN_ILLUMINANCE: f32 = SUN_BRIGHTNESS * PI;

fn stand_the_sun_up(mut commands: Commands) {
    commands.spawn((
        Sun,
        DirectionalLight { illuminance: SUN_ILLUMINANCE, shadow_maps_enabled: false, ..default() },
        RenderLayers::from_layers(&[SUNLIT, SUNLIT_VIEWMODEL]),
        Transform::default(),
        Name::new("Sun"),
    ));
}

fn aim_the_sun(position: Res<SunPosition>, mut suns: Query<&mut Transform, With<Sun>>, added: Query<(), Added<Sun>>) {
    if !position.is_changed() && added.is_empty() {
        return;
    }
    let toward = position.toward();
    let up = if toward.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    for mut transform in &mut suns {
        *transform = Transform::IDENTITY.looking_to(-toward, up);
    }
}

/// `current` with the sunlit layer that goes with each of its layers set to `lit`, or `None` if
/// that changes nothing. Only a mesh drawn with the world, or with the viewmodel, is touched: an
/// editor handle or a thumbnail stays where it is.
fn sunlit_layers(current: Option<&RenderLayers>, lit: bool) -> Option<RenderLayers> {
    let current = current.cloned().unwrap_or_default();
    let mut next = current.clone();
    for (drawn_with, sunlit) in [(WORLD, SUNLIT), (VIEWMODEL, SUNLIT_VIEWMODEL)] {
        if current.intersects(&RenderLayers::layer(drawn_with)) {
            next = if lit { next.with(sunlit) } else { next.without(sunlit) };
        }
    }
    (next != current).then_some(next)
}

/// How a moving thing is lit where it stands.
#[derive(Clone, Copy, Default, PartialEq)]
struct Lighting {
    sunlit: bool,
    /// [`air_light::pack`] of its ambient over the global one; `None` keeps the global.
    tag: Option<u32>,
}

impl Lighting {
    fn at(probe: Vec3, toward: Vec3, open_sky: &OpenSky, air: &AirLight) -> Lighting {
        Lighting {
            sunlit: open_sky.sees_sun(probe, toward),
            tag: air.at(probe).map(|light| air_light::pack(light / BASE_AMBIENT)),
        }
    }

    /// Writes this onto one mesh, touching only what differs.
    fn apply(self, entity: Entity, layers: Option<&RenderLayers>, tag: Option<&MeshTag>, commands: &mut Commands) {
        if let Some(next) = sunlit_layers(layers, self.sunlit) {
            commands.entity(entity).insert(next);
        }
        match (self.tag, tag.map(|tag| tag.value)) {
            (Some(wanted), current) if current != Some(wanted) => {
                commands.entity(entity).insert(MeshTag::new(wanted));
            }
            (None, Some(_)) => {
                commands.entity(entity).remove::<MeshTag>();
            }
            _ => {}
        }
    }
}

/// Every frame, since a body walks out of a doorway without anything about it changing but where
/// it is. A mesh's layers and tag are written only when they change.
#[allow(clippy::type_complexity)]
fn light_what_moves(
    mode: Res<State<AppMode>>,
    open_sky: Res<OpenSky>,
    sun_bake: Res<SunBake>,
    position: Res<SunPosition>,
    roots: Query<(Entity, &GlobalTransform, Has<Skeleton>, Has<LocalPlayer>), Or<(With<Skeleton>, With<WorldProp>, With<Projectile>)>>,
    children: Query<&Children>,
    meshes: Query<(Option<&RenderLayers>, Option<&MeshTag>), With<Mesh3d>>,
    viewmodel: Query<(Entity, &RenderLayers, Option<&MeshTag>), With<Mesh3d>>,
    mut commands: Commands,
) {
    let toward = position.toward();
    // The prop editor stands a figure at the origin with the map hidden round it.
    let shows_the_world = mode.get().shows_the_world();
    let mut ours = Lighting::default();
    for (root, at, body, local) in &roots {
        let probe = at.translation() + Vec3::Y * if body { CHEST } else { ABOVE_ORIGIN };
        let lighting = match shows_the_world {
            true => Lighting::at(probe, toward, &open_sky, sun_bake.air()),
            false => Lighting::default(),
        };
        if local {
            ours = lighting;
        }
        for entity in std::iter::once(root).chain(children.iter_descendants(root)) {
            if let Ok((layers, tag)) = meshes.get(entity) {
                lighting.apply(entity, layers, tag, &mut commands);
            }
        }
    }
    for (entity, layers, tag) in &viewmodel {
        if layers.intersects(&RenderLayers::layer(VIEWMODEL)) {
            ours.apply(entity, Some(layers), tag, &mut commands);
        }
    }
}

pub struct SunlitPlugin;

impl Plugin for SunlitPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, stand_the_sun_up)
            // Ungated: bodies stand in the editor too.
            .add_systems(Update, (aim_the_sun, light_what_moves));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_world_mesh_joins_the_sunlit_layer_and_leaves_it() {
        let lit = sunlit_layers(None, true).expect("an unlayered mesh is on the world");
        assert!(lit.intersects(&RenderLayers::layer(WORLD)) && lit.intersects(&RenderLayers::layer(SUNLIT)));
        assert_eq!(sunlit_layers(Some(&lit), true), None, "rewritten when nothing changed");
        let dark = sunlit_layers(Some(&lit), false).expect("left lit");
        assert!(!dark.intersects(&RenderLayers::layer(SUNLIT)));
    }

    /// An editor handle never joins: the play camera, which draws the sunlit layer, would draw it.
    #[test]
    fn a_mesh_not_drawn_with_the_world_is_left_alone() {
        let handle = RenderLayers::layer(crate::common::render_layers::EDITOR_ONLY);
        assert_eq!(sunlit_layers(Some(&handle), true), None);
    }

    #[test]
    fn the_viewmodel_has_a_sunlit_layer_of_its_own() {
        let lit = sunlit_layers(Some(&RenderLayers::layer(VIEWMODEL)), true).unwrap();
        assert!(lit.intersects(&RenderLayers::layer(SUNLIT_VIEWMODEL)));
        assert!(!lit.intersects(&RenderLayers::layer(SUNLIT)), "the world's cameras would draw the arms");
    }
}
