use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;
use crate::editor::editable::{EditEvent, FeatureId, FeatureHistory};
use crate::editor::input::CurrentMouseInput;
use crate::tool::tool_helpers::closest_param_on_axis;
use crate::tool::Tools;

pub struct PointDragPlugin;

impl Plugin for PointDragPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<PointDragState>()
            .add_systems(Update, (
                PointDragState::spawn_arrows_system,
                PointDragState::update_arrow_positions,
                PointDragState::handle_arrow_drag,
            ).chain().run_if(in_state(Tools::Select)))
            .add_systems(OnExit(Tools::Select), PointDragState::despawn_arrows);
    }
}

#[derive(Component)]
struct PointDragArrow {
    axis: u8,
}

#[derive(Resource, Default)]
pub struct PointDragState {
    tracked_feature: Option<FeatureId>,
    shaft_mesh: Option<Handle<Mesh>>,
    head_mesh: Option<Handle<Mesh>>,
    materials: [Option<Handle<StandardMaterial>>; 3],
    highlight_materials: [Option<Handle<StandardMaterial>>; 3],
    grabbed_axis: Option<u8>,
    grab_offset: Option<f32>,
}

const AXIS_DIRS: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

fn axis_rotation(axis: u8) -> Quat {
    match axis {
        0 => Quat::from_rotation_z(-FRAC_PI_2),
        2 => Quat::from_rotation_x(FRAC_PI_2),
        _ => Quat::IDENTITY,
    }
}

fn is_point_like(type_key: &str) -> bool {
    type_key == "global_point" || type_key == "grackle_point_light"
}

impl PointDragState {
    pub fn is_dragging(&self) -> bool {
        self.grabbed_axis.is_some()
    }

    fn ensure_assets(
        &mut self,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
    ) {
        if self.shaft_mesh.is_some() {
            return;
        }
        self.shaft_mesh = Some(meshes.add(Cylinder::new(0.04, 0.75)));
        self.head_mesh = Some(meshes.add(Cone::new(0.12, 0.25)));

        let colors = [
            Color::srgb(1.0, 0.0, 0.0),
            Color::srgb(0.0, 1.0, 0.0),
            Color::srgb(0.0, 0.0, 1.0),
        ];
        let highlight_colors = [
            Color::srgb(1.0, 0.5, 0.5),
            Color::srgb(0.5, 1.0, 0.5),
            Color::srgb(0.5, 0.5, 1.0),
        ];
        let emissive = [
            LinearRgba::new(4.0, 0.0, 0.0, 1.0),
            LinearRgba::new(0.0, 4.0, 0.0, 1.0),
            LinearRgba::new(0.0, 0.0, 4.0, 1.0),
        ];
        let highlight_emissive = [
            LinearRgba::new(4.0, 2.0, 2.0, 1.0),
            LinearRgba::new(2.0, 4.0, 2.0, 1.0),
            LinearRgba::new(2.0, 2.0, 4.0, 1.0),
        ];

        for i in 0..3 {
            self.materials[i] = Some(materials.add(StandardMaterial {
                base_color: colors[i],
                emissive: emissive[i],
                ..default()
            }));
            self.highlight_materials[i] = Some(materials.add(StandardMaterial {
                base_color: highlight_colors[i],
                emissive: highlight_emissive[i],
                ..default()
            }));
        }
    }

    fn spawn_arrows_system(
        mut state: ResMut<Self>,
        features: Res<FeatureHistory>,
        arrows: Query<Entity, With<PointDragArrow>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        let current = features.selected_feature();

        let should_track = current.filter(|id| {
            features.get_feature(id)
                .map(|a| is_point_like(a.object().type_key()))
                .unwrap_or(false)
        });

        if state.tracked_feature == should_track {
            return;
        }

        for entity in &arrows {
            commands.entity(entity).despawn();
        }
        state.tracked_feature = should_track;
        state.grabbed_axis = None;
        state.grab_offset = None;

        let Some(feature_id) = should_track else { return; };
        let Some(feature) = features.get_feature(&feature_id) else { return; };
        let Ok(pos) = feature.object().get_point("") else { return; };

        state.ensure_assets(&mut meshes, &mut materials);
        let shaft = state.shaft_mesh.clone().unwrap();
        let head = state.head_mesh.clone().unwrap();

        for axis in 0u8..3 {
            let mat = state.materials[axis as usize].clone().unwrap();
            let root = commands.spawn((
                PointDragArrow { axis },
                Transform::from_translation(pos).with_rotation(axis_rotation(axis)),
                Visibility::Inherited,
            )).id();

            let shaft_entity = commands.spawn((
                Mesh3d(shaft.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::from_translation(Vec3::new(0.0, 0.375, 0.0)),
            )).id();

            let head_entity = commands.spawn((
                Mesh3d(head.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::from_translation(Vec3::new(0.0, 0.875, 0.0)),
            )).id();

            commands.entity(root).add_children(&[shaft_entity, head_entity]);
        }
    }

    fn update_arrow_positions(
        features: Res<FeatureHistory>,
        state: Res<PointDragState>,
        mut arrows: Query<(&PointDragArrow, &mut Transform)>,
    ) {
        let Some(feature_id) = state.tracked_feature else { return; };
        let Some(feature) = features.get_feature(&feature_id) else { return; };
        let Ok(pos) = feature.object().get_point("") else { return; };

        for (arrow, mut tfm) in &mut arrows {
            tfm.translation = pos;
            tfm.rotation = axis_rotation(arrow.axis);
        }
    }

    fn handle_arrow_drag(
        arrows: Query<(Entity, &PointDragArrow, &Children)>,
        child_meshes: Query<Entity, With<Mesh3d>>,
        mut ray_cast: MeshRayCast,
        mouse_input: Res<CurrentMouseInput>,
        mut commands: Commands,
        mut state: ResMut<Self>,
        mut features: ResMut<FeatureHistory>,
        mut edit_events: MessageWriter<EditEvent>,
    ) {
        let Some(feature_id) = state.tracked_feature else { return; };

        let all_arrow_children: Vec<Entity> = arrows.iter()
            .flat_map(|(_, _, children)| children.iter())
            .filter(|e| child_meshes.get(*e).is_ok())
            .collect();

        let filter = |entity: Entity| all_arrow_children.contains(&entity);
        let settings = MeshRayCastSettings::default().with_filter(&filter);

        let ray = mouse_input.world_pos;
        let mouse_released = mouse_input.released == Some(MouseButton::Left);
        let mouse_just_pressed = mouse_input.just_pressed && mouse_input.pressed == Some(MouseButton::Left);
        let mouse_held = mouse_input.pressed == Some(MouseButton::Left);

        if mouse_released || ray.is_none() {
            if state.grabbed_axis.is_some() {
                state.grabbed_axis = None;
                state.grab_offset = None;
                Self::restore_all_materials(&arrows, &mut commands, &state);
            }
            if !mouse_held {
                Self::restore_all_materials(&arrows, &mut commands, &state);
            }
            if state.grabbed_axis.is_none() {
                return;
            }
        }

        let ray = match ray {
            Some(r) => r,
            None => return,
        };

        if state.grabbed_axis.is_some() {
            let axis = state.grabbed_axis.unwrap();
            let axis_dir = AXIS_DIRS[axis as usize];

            let Ok(current_pos) = features.get_feature(&feature_id)
                .and_then(|a| a.object().get_point("").ok())
                .ok_or(()) else { return; };

            let axis_origin = current_pos - axis_dir * current_pos.dot(axis_dir);

            let Some(projected) = closest_param_on_axis(ray, axis_origin, axis_dir) else { return; };

            let offset = match state.grab_offset {
                Some(off) => off,
                None => {
                    let current_value = current_pos.dot(axis_dir);
                    let off = projected - current_value;
                    state.grab_offset = Some(off);
                    off
                }
            };

            let new_value = projected - offset;

            if let Some(mut feature) = features.features_mut().remove(&feature_id) {
                let modified = feature.object_mut().drag_handle(false, axis, new_value);
                if modified {
                    if let Some(entity) = feature.object().entity() {
                        feature.object_mut().apply_to_entity(&mut commands, entity);
                        edit_events.write(EditEvent {
                            editor_id: feature_id._id(),
                            feature_id,
                            entity,
                        });
                    }
                }
                features.features_mut().insert(feature_id, feature);
            }
        } else {
            let hits = ray_cast.cast_ray(ray, &settings);
            if let Some((hit_entity, _hit_data)) = hits.first() {
                let hit_axis = arrows.iter().find(|(_, _, children)| {
                    children.iter().any(|c| c == *hit_entity)
                }).map(|(_, arrow, _)| arrow.axis);

                if let Some(axis) = hit_axis {
                    Self::highlight_axis(&arrows, axis, &mut commands, &state);

                    if mouse_just_pressed {
                        state.grabbed_axis = Some(axis);
                        state.grab_offset = None;
                    }
                } else {
                    Self::restore_all_materials(&arrows, &mut commands, &state);
                }
            } else {
                Self::restore_all_materials(&arrows, &mut commands, &state);
            }
        }
    }

    fn highlight_axis(
        arrows: &Query<(Entity, &PointDragArrow, &Children)>,
        axis: u8,
        commands: &mut Commands,
        state: &PointDragState,
    ) {
        for (_, arrow, children) in arrows.iter() {
            let mat = if arrow.axis == axis {
                state.highlight_materials[arrow.axis as usize].clone()
            } else {
                state.materials[arrow.axis as usize].clone()
            };
            if let Some(mat) = mat {
                for child in children.iter() {
                    commands.entity(child)
                        .remove::<MeshMaterial3d<StandardMaterial>>()
                        .insert(MeshMaterial3d(mat.clone()));
                }
            }
        }
    }

    fn restore_all_materials(
        arrows: &Query<(Entity, &PointDragArrow, &Children)>,
        commands: &mut Commands,
        state: &PointDragState,
    ) {
        for (_, arrow, children) in arrows.iter() {
            if let Some(mat) = &state.materials[arrow.axis as usize] {
                for child in children.iter() {
                    commands.entity(child)
                        .remove::<MeshMaterial3d<StandardMaterial>>()
                        .insert(MeshMaterial3d(mat.clone()));
                }
            }
        }
    }

    fn despawn_arrows(
        arrows: Query<Entity, With<PointDragArrow>>,
        mut commands: Commands,
        mut state: ResMut<Self>,
    ) {
        for entity in &arrows {
            commands.entity(entity).despawn();
        }
        state.tracked_feature = None;
        state.grabbed_axis = None;
        state.grab_offset = None;
    }
}
