//! Places [`Prop`] features, and hangs the stand-in cube on the entities they
//! drive.
//!
//! Deliberately the point tool with a different feature at the end of it, the
//! same way the spawn point tool is: placing a prop *is* placing a point, and
//! a mapper who has learnt one has learnt the other — relative placement and
//! shift-picking included. Turning it comes afterwards, from the rotation
//! rings in [`crate::tool::rotate_drag`]. The shared half lives in
//! [`crate::tool::point_placement`].

use bevy::prelude::*;

use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::prop::{Prop, PropMarker, PROP_SIZE};
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

/// Shared handles for the stand-in cube, so every prop on the map is one mesh
/// and one material rather than one of each per prop.
#[derive(Resource, Default)]
struct PropAssets {
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
}

pub struct PropPlugin;

impl Plugin for PropPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<Prop>(app);
        app
            .init_resource::<PropAssets>()
            // Deliberately not gated on `AppMode::Editor`: a prop is map
            // content, and a prop that vanished on F5 would be a prop you
            // could not stand next to and judge the size of.
            .add_systems(Update, attach_prop_meshes)
        ;
    }
}

/// Give every [`PropMarker`] entity the placeholder cube once.
///
/// `apply_to_entity` only has `Commands`, so it cannot reach `Assets` to make
/// a mesh; it marks the entity instead and this puts the box on. When props
/// grow real models this is the seam that loads them.
fn attach_prop_meshes(
    new_props: Query<Entity, (With<PropMarker>, Without<Mesh3d>)>,
    mut assets: ResMut<PropAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    if new_props.is_empty() {
        return;
    }

    let mesh = assets
        .mesh
        .get_or_insert_with(|| meshes.add(Cuboid::from_length(PROP_SIZE)))
        .clone();
    let material = assets
        .material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb_u8(90, 220, 200),
                ..default()
            })
        })
        .clone();

    for entity in &new_props {
        commands
            .entity(entity)
            .insert((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone())));
    }
}

impl PlaceablePoint for Prop {
    const TOOL: Tools = Tools::Prop;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Box::new(Prop::new(position.x, position.y, position.z))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(Prop::from_point_ref(point_ref))
    }

    fn normal_color() -> Color {
        Color::srgb_u8(90, 220, 200)
    }

    fn relative_color() -> Color {
        Color::srgb_u8(160, 240, 230)
    }

    /// The box that would land here, at the size it would land at, so its
    /// footprint is visible before it is committed rather than after.
    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        gizmos.cube(
            Transform::from_translation(cursor).with_scale(Vec3::splat(PROP_SIZE)),
            color,
        );
    }
}
