use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;
use crate::common::app_mode::AppMode;
use crate::editor::editable::{EditEvent, FeatureId, FeatureTimeline};
use crate::editor::input::CurrentMouseInput;
use crate::tool::rotate_drag::RotateDragState;
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
            ).chain().run_if(in_state(Tools::Select)).run_if(in_state(AppMode::Editor)))
            .add_systems(OnExit(Tools::Select), PointDragState::despawn_arrows);
    }
}

#[derive(Component)]
struct PointDragArrow {
    axis: u8,
}

/// A mesh that is part of an arrow, carrying the arrow's axis itself so that
/// picking and highlighting are one lookup rather than a walk of every
/// arrow's children each frame.
#[derive(Component)]
struct PointDragArrowPart {
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
    type_key == "global_point"
        || type_key == "grackle_point_light"
        || type_key == "spawn_point"
        || type_key == "prop"
        || type_key == "animation_display"
        || type_key == "animation_grid"
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
        features: Res<FeatureTimeline>,
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
                PointDragArrowPart { axis },
                Mesh3d(shaft.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::from_translation(Vec3::new(0.0, 0.375, 0.0)),
            )).id();

            let head_entity = commands.spawn((
                PointDragArrowPart { axis },
                Mesh3d(head.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::from_translation(Vec3::new(0.0, 0.875, 0.0)),
            )).id();

            commands.entity(root).add_children(&[shaft_entity, head_entity]);
        }
    }

    fn update_arrow_positions(
        features: Res<FeatureTimeline>,
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
        mut parts: Query<(&PointDragArrowPart, &mut MeshMaterial3d<StandardMaterial>)>,
        mut ray_cast: MeshRayCast,
        mouse_input: Res<CurrentMouseInput>,
        rotate_drag: Res<RotateDragState>,
        mut commands: Commands,
        mut state: ResMut<Self>,
        mut features: ResMut<FeatureTimeline>,
        mut edit_events: MessageWriter<EditEvent>,
    ) {
        let Some(feature_id) = state.tracked_feature else { return; };

        // A ring drag in progress owns the mouse: the arrows reach into the
        // rings, and grabbing one mid-rotation would move the feature as well
        // as turn it, in what the user meant as one rotation.
        if rotate_drag.is_dragging() {
            return;
        }

        let ray = mouse_input.world_pos;
        let mouse_released = mouse_input.released == Some(MouseButton::Left);
        let mouse_just_pressed = mouse_input.just_pressed && mouse_input.pressed == Some(MouseButton::Left);
        let mouse_held = mouse_input.pressed == Some(MouseButton::Left);

        if mouse_released || ray.is_none() {
            if state.grabbed_axis.is_some() {
                state.grabbed_axis = None;
                state.grab_offset = None;
                Self::paint(&mut parts, None, &state);
            }
            if !mouse_held {
                Self::paint(&mut parts, None, &state);
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
            let hit_axis = {
                let filter = |entity: Entity| parts.contains(entity);
                let settings = MeshRayCastSettings::default().with_filter(&filter);
                ray_cast.cast_ray(ray, &settings).first()
                    .and_then(|(hit, _)| parts.get(*hit).ok())
                    .map(|(part, _)| part.axis)
            };

            Self::paint(&mut parts, hit_axis, &state);

            if let Some(axis) = hit_axis {
                if mouse_just_pressed {
                    state.grabbed_axis = Some(axis);
                    state.grab_offset = None;
                }
            }
        }
    }

    /// Lights the arrow on `highlighted` and puts every other one back to
    /// its own colour. A material is only written when it is not already the
    /// one wanted: this runs every frame the pointer is over a viewport, and
    /// an unconditional write is a changed component on every arrow mesh, so
    /// every one of them re-extracted for rendering for nothing.
    fn paint(
        parts: &mut Query<(&PointDragArrowPart, &mut MeshMaterial3d<StandardMaterial>)>,
        highlighted: Option<u8>,
        state: &PointDragState,
    ) {
        for (part, mut material) in parts.iter_mut() {
            let set = if highlighted == Some(part.axis) {
                &state.highlight_materials
            } else {
                &state.materials
            };
            let Some(wanted) = &set[part.axis as usize] else { continue; };
            if material.0 != *wanted {
                material.0 = wanted.clone();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Hovering an arrow frame after frame must not keep rewriting the
    /// materials: a written `MeshMaterial3d` is a changed one, and every
    /// changed one is re-extracted for rendering.
    #[test]
    fn hovering_the_same_arrow_again_writes_nothing() {
        let mut world = World::new();
        let mut materials = Assets::<StandardMaterial>::default();
        let mut state = PointDragState::default();
        for i in 0..3 {
            state.materials[i] = Some(materials.add(StandardMaterial::default()));
            state.highlight_materials[i] = Some(materials.add(StandardMaterial::default()));
        }
        let parts: Vec<Entity> = (0u8..3)
            .map(|axis| world.spawn((
                PointDragArrowPart { axis },
                MeshMaterial3d(state.materials[axis as usize].clone().unwrap()),
            )).id())
            .collect();
        world.insert_resource(state);

        let mut schedule = Schedule::default();
        schedule.add_systems(|
            mut parts: Query<(&PointDragArrowPart, &mut MeshMaterial3d<StandardMaterial>)>,
            state: Res<PointDragState>,
        | PointDragState::paint(&mut parts, Some(1), &state));

        schedule.run(&mut world);
        let state = world.resource::<PointDragState>();
        let lit = state.highlight_materials[1].clone().unwrap();
        let idle = state.materials[0].clone().unwrap();
        assert_eq!(world.get::<MeshMaterial3d<StandardMaterial>>(parts[1]).unwrap().0, lit);
        assert_eq!(world.get::<MeshMaterial3d<StandardMaterial>>(parts[0]).unwrap().0, idle);

        let archetypes: Vec<_> = parts.iter().map(|e| world.entity(*e).archetype().id()).collect();
        world.clear_trackers();
        schedule.run(&mut world);
        for (part, archetype) in parts.iter().zip(archetypes) {
            let entity = world.entity(*part);
            assert_eq!(entity.archetype().id(), archetype);
            assert!(!entity.get_ref::<MeshMaterial3d<StandardMaterial>>().unwrap().is_changed());
        }
    }
}
