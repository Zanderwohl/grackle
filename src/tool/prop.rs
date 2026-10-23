//! Places [`Prop`] features, and hangs the stand-in cube on the entities they
//! drive. The point tool with a different feature at the end; the shared half
//! is [`crate::tool::point_placement`].

use bevy::prelude::*;

use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::prop::{Prop, PropMarker, PropModel, PROP_SIZE};
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

/// The placeholder cube on every [`PropMarker`] entity that names no model,
/// and off one that has come to name one; a named model is drawn by
/// [`crate::game::world_prop`]. Separate because `apply_to_entity` only has
/// `Commands` and cannot reach `Assets`.
fn attach_prop_meshes(
    props: Query<(Entity, &PropModel, Has<Mesh3d>), (With<PropMarker>, Changed<PropModel>)>,
    mut assets: ResMut<PropAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for (entity, model, has_mesh) in &props {
        match (model.0.is_empty(), has_mesh) {
            (true, false) => {
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
                commands.entity(entity).insert((Mesh3d(mesh), MeshMaterial3d(material)));
            }
            (false, true) => {
                commands.entity(entity).remove::<(Mesh3d, MeshMaterial3d<StandardMaterial>)>();
            }
            _ => {}
        }
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
