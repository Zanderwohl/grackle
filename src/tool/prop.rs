//! Places [`Prop`] features, and hangs the stand-in cube on the entities they
//! drive.
//!
//! Deliberately the point tool with a different feature at the end of it, the
//! same way the spawn point tool is: placing a prop *is* placing a point, and
//! a mapper who has learnt one has learnt the other — relative placement and
//! shift-picking included. Turning it comes afterwards, from the rotation
//! rings in [`crate::tool::rotate_drag`].

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::editor::editable::{FeatureId, FeatureTimeline, PointRef};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::editor::prop::{Prop, PropMarker, PROP_SIZE};
use crate::tool::room::Room;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const DEFAULT_SNAP_GRANULARITY: f32 = 0.1;

#[derive(PartialEq, Eq, Clone, Copy)]
enum PropToolMode {
    Normal,
    Picking,
    RelativeSelected,
}

#[derive(Resource)]
struct PropTool {
    mode: PropToolMode,
    last_position: Vec3,
    cursor: Option<Vec3>,
    reference_feature: Option<FeatureId>,
    reference_key: String,
    reference_resolved: Option<Vec3>,
    hovered_point: Option<(FeatureId, String, Vec3)>,
    snap: bool,
    snap_granularity: f32,
}

impl Default for PropTool {
    fn default() -> Self {
        Self {
            mode: PropToolMode::Normal,
            last_position: Vec3::ZERO,
            cursor: None,
            reference_feature: None,
            reference_key: String::new(),
            reference_resolved: None,
            hovered_point: None,
            snap: true,
            snap_granularity: DEFAULT_SNAP_GRANULARITY,
        }
    }
}

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
        app
            .init_resource::<PropTool>()
            .init_resource::<PropAssets>()
            .add_systems(Update, (
                PropTool::interface,
                PropTool::draw_gizmos,
            ).chain().run_if(in_state(Tools::Prop)).run_if(in_state(AppMode::Editor)))
            // Deliberately not gated on `AppMode::Editor`: a prop is map
            // content, and a prop that vanished on F5 would be a prop you
            // could not stand next to and judge the size of.
            .add_systems(Update, attach_prop_meshes)
            .add_systems(OnExit(Tools::Prop), PropTool::on_exit)
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

impl PropTool {
    fn on_exit(mut tool: ResMut<Self>) {
        tool.mode = PropToolMode::Normal;
        tool.cursor = None;
        tool.hovered_point = None;
        tool.reference_feature = None;
        tool.reference_key.clear();
        tool.reference_resolved = None;
    }

    fn interface(
        mut tool: ResMut<Self>,
        cameras: Query<(Entity, &Multicam)>,
        mouse_input: Res<CurrentMouseInput>,
        keys: Res<ButtonInput<KeyCode>>,
        mut features: ResMut<FeatureTimeline>,
        rooms: Query<&Room>,
        mut next_tool: ResMut<NextState<Tools>>,
    ) {
        let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let shift_just_pressed = keys.just_pressed(KeyCode::ShiftLeft) || keys.just_pressed(KeyCode::ShiftRight);

        tool.cursor = compute_cursor(
            &mouse_input, &cameras, tool.last_position,
            tool.snap, tool.snap_granularity, &rooms,
        );

        match tool.mode {
            PropToolMode::Normal => {
                if shift_held {
                    tool.mode = PropToolMode::Picking;
                    tool.hovered_point = None;
                } else if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        let prop = Prop::new(cursor.x, cursor.y, cursor.z);
                        let id = features.apply_feature(Box::new(prop));
                        features.select(Some(id));
                        tool.last_position = cursor;
                        next_tool.set(Tools::Select);
                    }
                }
            }
            PropToolMode::Picking => {
                if !shift_held {
                    tool.mode = PropToolMode::Normal;
                    tool.hovered_point = None;
                    return;
                }

                tool.hovered_point = mouse_input.world_pos
                    .and_then(|ray| find_hovered_point(&ray, &features, PICK_RADIUS));

                if mouse_input.released == Some(MouseButton::Left) {
                    if let Some((feature_id, key, resolved)) = tool.hovered_point.take() {
                        tool.reference_feature = Some(feature_id);
                        tool.reference_key = key;
                        tool.reference_resolved = Some(resolved);
                        tool.mode = PropToolMode::RelativeSelected;
                    }
                }
            }
            PropToolMode::RelativeSelected => {
                if shift_just_pressed {
                    tool.mode = PropToolMode::Normal;
                    tool.reference_feature = None;
                    tool.reference_key.clear();
                    tool.reference_resolved = None;
                    return;
                }

                if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        if let (Some(ref_feature), Some(ref_resolved)) = (tool.reference_feature, tool.reference_resolved) {
                            let d = cursor - ref_resolved;
                            let mut pr = PointRef::reference_with_offset(ref_feature, d.x, d.y, d.z);
                            if !tool.reference_key.is_empty() {
                                pr.point_key = tool.reference_key.clone();
                            }
                            let prop = Prop::from_point_ref(pr);
                            let id = features.apply_feature(Box::new(prop));
                            features.select(Some(id));
                            tool.last_position = cursor;
                            next_tool.set(Tools::Select);
                        }
                    }
                }
            }
        }
    }

    fn draw_gizmos(
        tool: Res<PropTool>,
        features: Res<FeatureTimeline>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        if let Some(cursor) = tool.cursor {
            let color = match tool.mode {
                PropToolMode::RelativeSelected => Color::srgb_u8(160, 240, 230),
                _ => Color::srgb_u8(90, 220, 200),
            };
            // The box that would land here, at the size it would land at, so
            // its footprint is visible before it is committed rather than after.
            gizmos.cube(
                Transform::from_translation(cursor).with_scale(Vec3::splat(PROP_SIZE)),
                color,
            );

            if tool.mode == PropToolMode::RelativeSelected {
                if let Some(base) = tool.reference_resolved {
                    draw_taxicab_path(&mut gizmos, base, cursor);
                }
            }
        }

        if tool.mode == PropToolMode::Picking {
            if let Some(ray) = mouse_input.world_pos {
                draw_picking_gizmos(&mut gizmos, &ray, &features, &tool.hovered_point);
            }
        }
    }
}
