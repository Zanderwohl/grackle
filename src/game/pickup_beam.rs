//! A beam of light from the floor up through a weapon pickup: a truncated cone
//! fading out as it rises, and a spot light pointing up into the weapon. Both
//! dim while the pickup restocks, so an empty pad still says where a weapon
//! will be.
//!
//! Drawing only, on every machine, keyed on a `GivesWeapon` instance having no
//! [`Beam`]: the instance is replicated, and the beam is its children, so it
//! goes when the instance does and is hidden when the prop editor hides it.

use std::f32::consts::TAU;

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::game::world_prop::{GivesWeapon, Pickup};

/// The cone, in meters from the floor.
const BOTTOM_RADIUS: f32 = 0.3;
const TOP_RADIUS: f32 = 0.7;
const HEIGHT: f32 = 2.0;
/// Opacity at the floor; it fades to none at the top.
const BOTTOM_OPACITY: f32 = 0.1;
const SEGMENTS: usize = 32;

/// Lumens, against the 40 000 to 250 000 the template map's lights use: enough
/// to light the weapon, not the room.
const LIGHT_LUMENS: f32 = 30_000.0;
/// Meters: the floor to a little past the weapon, which hovers at 1 m.
const LIGHT_RANGE: f32 = 1.5;
/// Wider than the cone, so the whole weapon is lit and not only its middle.
const LIGHT_OUTER_ANGLE: f32 = 0.45;
const LIGHT_INNER_ANGLE: f32 = 0.25;
/// Off the floor, so the floor under the light is not lit edge-on.
const LIGHT_HEIGHT: f32 = 0.05;

/// Of both the cone and the light, while the pickup restocks.
const TAKEN_BRIGHTNESS: f32 = 0.2;

/// On the pickup instance: its two children.
#[derive(Component)]
pub struct Beam {
    cone: Entity,
    light: Entity,
}

/// Shared by every beam; made the first time one is.
#[derive(Resource)]
struct BeamAssets {
    mesh: Handle<Mesh>,
    lit: Handle<StandardMaterial>,
    dim: Handle<StandardMaterial>,
}

pub struct PickupBeamPlugin;

impl Plugin for PickupBeamPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (raise_beams, dim_taken_beams).chain());
    }
}

fn raise_beams(
    mut commands: Commands,
    assets: Option<Res<BeamAssets>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    pickups: Query<Entity, (With<GivesWeapon>, Without<Beam>)>,
) {
    if pickups.is_empty() {
        return;
    }
    let (mesh, lit) = match assets {
        Some(assets) => (assets.mesh.clone(), assets.lit.clone()),
        None => {
            let made = BeamAssets {
                mesh: meshes.add(cone()),
                lit: materials.add(glow(1.0)),
                dim: materials.add(glow(TAKEN_BRIGHTNESS)),
            };
            let handles = (made.mesh.clone(), made.lit.clone());
            commands.insert_resource(made);
            handles
        }
    };

    for pickup in &pickups {
        let cone = commands
            .spawn((Mesh3d(mesh.clone()), MeshMaterial3d(lit.clone()), NotShadowCaster, Transform::IDENTITY))
            .id();
        let light = commands
            .spawn((
                SpotLight {
                    color: Color::WHITE,
                    intensity: LIGHT_LUMENS,
                    range: LIGHT_RANGE,
                    outer_angle: LIGHT_OUTER_ANGLE,
                    inner_angle: LIGHT_INNER_ANGLE,
                    shadow_maps_enabled: false,
                    ..default()
                },
                // A spot light shines down its -Z.
                Transform::from_xyz(0.0, LIGHT_HEIGHT, 0.0).looking_to(Vec3::Y, Vec3::X),
            ))
            .id();
        commands.entity(pickup).add_children(&[cone, light]).insert(Beam { cone, light });
    }
}

/// Compared before writing, so a still pickup is not a changed one.
fn dim_taken_beams(
    assets: Option<Res<BeamAssets>>,
    pickups: Query<(&Pickup, &Beam)>,
    mut cones: Query<&mut MeshMaterial3d<StandardMaterial>>,
    mut lights: Query<&mut SpotLight>,
) {
    let Some(assets) = assets else { return };
    for (pickup, beam) in &pickups {
        let (material, lumens) = match pickup.taken {
            false => (&assets.lit, LIGHT_LUMENS),
            true => (&assets.dim, LIGHT_LUMENS * TAKEN_BRIGHTNESS),
        };
        if let Ok(mut cone) = cones.get_mut(beam.cone)
            && cone.0 != *material
        {
            cone.0 = material.clone();
        }
        if let Ok(mut light) = lights.get_mut(beam.light)
            && light.intensity != lumens
        {
            light.intensity = lumens;
        }
    }
}

/// Unlit and blended, its opacity the vertices' times `brightness`.
fn glow(brightness: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE.with_alpha(brightness),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..default()
    }
}

/// The side of a truncated cone, open at both ends, from the floor up, its
/// opacity in the vertex colors.
fn cone() -> Mesh {
    let mut positions = Vec::with_capacity(2 * (SEGMENTS + 1));
    let mut normals = Vec::with_capacity(positions.capacity());
    let mut colors = Vec::with_capacity(positions.capacity());
    let slope = (BOTTOM_RADIUS - TOP_RADIUS) / HEIGHT;
    for step in 0..=SEGMENTS {
        let angle = step as f32 / SEGMENTS as f32 * TAU;
        let (sin, cos) = angle.sin_cos();
        let normal = Vec3::new(cos, -slope, sin).normalize().to_array();
        for (radius, y, opacity) in [(BOTTOM_RADIUS, 0.0, BOTTOM_OPACITY), (TOP_RADIUS, HEIGHT, 0.0)] {
            positions.push([cos * radius, y, sin * radius]);
            normals.push(normal);
            colors.push([1.0, 1.0, 1.0, opacity]);
        }
    }
    let mut indices = Vec::with_capacity(SEGMENTS * 6);
    for step in 0..SEGMENTS as u32 {
        let (bottom, top, next_bottom, next_top) = (2 * step, 2 * step + 1, 2 * step + 2, 2 * step + 3);
        indices.extend([bottom, top, next_bottom, next_bottom, top, next_top]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(indices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cone_runs_from_the_floor_to_its_height_and_fades_out_going_up() {
        let mesh = cone();
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("no positions");
        };
        let Some(bevy::mesh::VertexAttributeValues::Float32x4(colors)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR) else {
            panic!("no colors");
        };
        for (position, color) in positions.iter().zip(colors) {
            let radius = Vec2::new(position[0], position[2]).length();
            match position[1] {
                0.0 => assert!((radius - BOTTOM_RADIUS).abs() < 1e-5 && color[3] == BOTTOM_OPACITY),
                HEIGHT => assert!((radius - TOP_RADIUS).abs() < 1e-5 && color[3] == 0.0),
                y => panic!("a vertex at {y}"),
            }
        }
    }
}
