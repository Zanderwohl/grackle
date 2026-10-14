//! Places [`Prop`] features, and hangs the stand-in cube on the entities they
//! drive. The point tool with a different feature at the end; the shared half
//! is [`crate::tool::point_placement`].

use bevy::prelude::*;

use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::prop::{Prop, PropMarker, PROP_SIZE};
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

/// Shared handles for the stand-in cube.
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
            // Ungated: props are map content and must show in Play.
            .add_systems(Update, attach_prop_meshes)
        ;
    }
}

/// Give every [`PropMarker`] entity the placeholder cube once. Separate
/// because `apply_to_entity` only has `Commands` and cannot reach `Assets`.
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

    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        gizmos.cube(
            Transform::from_translation(cursor).with_scale(Vec3::splat(PROP_SIZE)),
            color,
        );
    }
}
