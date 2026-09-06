//! Rotation rings for the selected feature.
//!
//! The translation counterpart is [`crate::tool::point_drag`], and this is
//! deliberately built to the same shape: real meshes picked with
//! [`MeshRayCast`], a grab that lives in a resource, and every change routed
//! through the feature rather than the entity.
//!
//! That last part is why this is hand-rolled rather than Bevy 0.19's
//! `TransformGizmoPlugin`. Bevy's gizmo writes `Transform` on the entity it is
//! focused on, and in Grackle a feature's entity is *output*: `apply_to_entity`
//! overwrites it from the resolved feature on the next edit, so a rotation
//! written there would last until the next frame that touched the feature and
//! then vanish. It also reads `Window::cursor_position` against a single
//! resolved camera, and the editor draws four viewports inside `egui_dock`
//! whose rays come from [`CurrentMouseInput`] instead. Going through
//! `set_euler_angle` also means a drag lands in the undo timeline, between
//! `begin_edit` and `end_edit`, as one entry.
//!
//! A drag turns about *one* Euler component, and the ring for that component
//! is drawn on the axis that component actually turns about — see
//! [`crate::common::rotation`]. So the ring the mouse is on is always the ring
//! the object moves along, whatever the other two angles are.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::rotation::euler_axis_world;
use crate::editor::editable::{EditEvent, FeatureId, FeatureTimeline};
use crate::editor::input::CurrentMouseInput;
use crate::tool::point_drag::PointDragState;
use crate::tool::tool_helpers::angle_on_plane;
use crate::tool::Tools;

/// Outside the translation arrows, which reach about 1.0, so the two gizmos
/// do not sit on top of each other and fight over the same click.
const RING_RADIUS: f32 = 1.3;
const RING_THICKNESS: f32 = 0.035;

pub struct RotateDragPlugin;

impl Plugin for RotateDragPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<RotateDragState>()
            .add_systems(Update, (
                RotateDragState::spawn_rings,
                RotateDragState::update_ring_transforms,
                RotateDragState::handle_ring_drag,
            ).chain().run_if(in_state(Tools::Select)).run_if(in_state(AppMode::Editor)))
            .add_systems(OnExit(Tools::Select), RotateDragState::despawn_rings);
    }
}

#[derive(Component)]
struct RotateDragRing {
    axis: u8,
}

/// What a grab froze at the moment the mouse went down.
///
/// The plane has to stay put for the duration: recomputing it from the
/// object's current rotation would move the plane as the object turned on it,
/// and the drag would run away from the cursor.
struct RingGrab {
    axis: u8,
    axis_world: Vec3,
    cos_dir: Vec3,
    sin_dir: Vec3,
    initial_angle: f32,
    initial_value: f32,
}

#[derive(Resource, Default)]
pub struct RotateDragState {
    tracked_feature: Option<FeatureId>,
    tracked_axes: Vec<u8>,
    ring_mesh: Option<Handle<Mesh>>,
    materials: [Option<Handle<StandardMaterial>>; 3],
    highlight_materials: [Option<Handle<StandardMaterial>>; 3],
    grab: Option<RingGrab>,
}

impl RotateDragState {
    pub fn is_dragging(&self) -> bool {
        self.grab.is_some()
    }

    fn ensure_assets(
        &mut self,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
    ) {
        if self.ring_mesh.is_some() {
            return;
        }
        self.ring_mesh = Some(meshes.add(Torus::new(
            RING_RADIUS - RING_THICKNESS,
            RING_RADIUS + RING_THICKNESS,
        )));

        // The same axis colours the arrows use, so a red ring and a red arrow
        // are obviously the same axis seen two ways.
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

    /// A feature with no `rotation_axes` gets no rings at all, so a plain point
    /// is not cluttered with a gizmo that would do nothing.
    fn axes_for(features: &FeatureTimeline, id: Option<FeatureId>) -> Vec<u8> {
        id.and_then(|id| features.get_feature(&id))
            .map(|f| f.object().rotation_axes())
            .unwrap_or_default()
    }

    fn spawn_rings(
        mut state: ResMut<Self>,
        features: Res<FeatureTimeline>,
        rings: Query<Entity, With<RotateDragRing>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        let selected = features.selected_feature();
        let axes = Self::axes_for(&features, selected);
        let should_track = if axes.is_empty() { None } else { selected };

        if state.tracked_feature == should_track && state.tracked_axes == axes {
            return;
        }

        for entity in &rings {
            commands.entity(entity).despawn();
        }
        state.tracked_feature = should_track;
        state.tracked_axes = axes.clone();
        state.grab = None;

        let Some(feature_id) = should_track else { return; };
        let Some(feature) = features.get_feature(&feature_id) else { return; };
        let Ok(pos) = feature.object().get_point("") else { return; };
        let angles = feature.object().euler_angles();

        state.ensure_assets(&mut meshes, &mut materials);
        let ring = state.ring_mesh.clone().unwrap();

        for axis in axes {
            let mat = state.materials[axis as usize].clone().unwrap();
            let root = commands.spawn((
                RotateDragRing { axis },
                ring_transform(pos, angles, axis),
                Visibility::Inherited,
            )).id();

            let mesh_entity = commands.spawn((
                Mesh3d(ring.clone()),
                MeshMaterial3d(mat),
                Transform::IDENTITY,
            )).id();

            commands.entity(root).add_children(&[mesh_entity]);
        }
    }

    /// Follows the feature while it is dragged around by the arrows, and
    /// re-aims each ring as the other angles change — except the ring being
    /// dragged, whose plane was frozen at the grab.
    fn update_ring_transforms(
        features: Res<FeatureTimeline>,
        state: Res<RotateDragState>,
        mut rings: Query<(&RotateDragRing, &mut Transform)>,
    ) {
        let Some(feature_id) = state.tracked_feature else { return; };
        let Some(feature) = features.get_feature(&feature_id) else { return; };
        let Ok(pos) = feature.object().get_point("") else { return; };
        let angles = feature.object().euler_angles();

        for (ring, mut tfm) in &mut rings {
            if state.grab.as_ref().is_some_and(|g| g.axis == ring.axis) {
                tfm.translation = pos;
                continue;
            }
            *tfm = ring_transform(pos, angles, ring.axis);
        }
    }

    fn handle_ring_drag(
        rings: Query<(Entity, &RotateDragRing, &Children)>,
        child_meshes: Query<Entity, With<Mesh3d>>,
        mut ray_cast: MeshRayCast,
        mouse_input: Res<CurrentMouseInput>,
        point_drag: Res<PointDragState>,
        mut commands: Commands,
        mut state: ResMut<Self>,
        mut features: ResMut<FeatureTimeline>,
        mut edit_events: MessageWriter<EditEvent>,
    ) {
        let Some(feature_id) = state.tracked_feature else { return; };

        let ray = mouse_input.world_pos;
        let mouse_released = mouse_input.released == Some(MouseButton::Left);
        let mouse_just_pressed =
            mouse_input.just_pressed && mouse_input.pressed == Some(MouseButton::Left);

        if mouse_released || ray.is_none() {
            if state.grab.is_some() {
                features.end_edit(feature_id);
                state.grab = None;
            }
            Self::restore_all_materials(&rings, &mut commands, &state);
            return;
        }

        let Some(ray) = ray else { return; };

        if let Some(grab) = &state.grab {
            let Some(center) = features
                .get_feature(&feature_id)
                .and_then(|f| f.object().get_point("").ok())
            else {
                return;
            };

            let plane = InfinitePlane3d::new(grab.axis_world);
            let Some(hit) = ray.plane_intersection_point(center, plane) else { return; };

            let angle = angle_on_plane(hit, center, grab.cos_dir, grab.sin_dir);
            let new_value = grab.initial_value + (angle - grab.initial_angle);
            let axis = grab.axis;

            if let Some(mut feature) = features.features_mut().remove(&feature_id) {
                let modified = feature.object_mut().set_euler_angle(axis, new_value);
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
            return;
        }

        // Not dragging: hover, and possibly grab. An arrow drag in progress
        // owns the mouse — the rings are within reach of the arrows and would
        // otherwise steal a translation halfway through.
        if point_drag.is_dragging() {
            Self::restore_all_materials(&rings, &mut commands, &state);
            return;
        }

        let ring_meshes: Vec<Entity> = rings
            .iter()
            .flat_map(|(_, _, children)| children.iter())
            .filter(|e| child_meshes.get(*e).is_ok())
            .collect();
        let filter = |entity: Entity| ring_meshes.contains(&entity);
        let settings = MeshRayCastSettings::default().with_filter(&filter);

        let hit_axis = ray_cast.cast_ray(ray, &settings).first().and_then(|(hit, _)| {
            rings
                .iter()
                .find(|(_, _, children)| children.iter().any(|c| c == *hit))
                .map(|(_, ring, _)| ring.axis)
        });

        let Some(axis) = hit_axis else {
            Self::restore_all_materials(&rings, &mut commands, &state);
            return;
        };

        Self::highlight_axis(&rings, axis, &mut commands, &state);

        if !mouse_just_pressed {
            return;
        }

        let Some(feature) = features.get_feature(&feature_id) else { return; };
        let Ok(center) = feature.object().get_point("") else { return; };
        let angles = feature.object().euler_angles();
        let axis_world = euler_axis_world(angles, axis);
        let (cos_dir, sin_dir) = axis_world.any_orthonormal_pair();

        let plane = InfinitePlane3d::new(axis_world);
        // Looking straight down the ring's edge there is no angle to read, so
        // there is nothing to grab either.
        let Some(hit) = ray.plane_intersection_point(center, plane) else { return; };

        features.begin_edit(feature_id);
        state.grab = Some(RingGrab {
            axis,
            axis_world,
            cos_dir,
            sin_dir,
            initial_angle: angle_on_plane(hit, center, cos_dir, sin_dir),
            initial_value: angles[axis as usize],
        });
    }

    fn highlight_axis(
        rings: &Query<(Entity, &RotateDragRing, &Children)>,
        axis: u8,
        commands: &mut Commands,
        state: &RotateDragState,
    ) {
        for (_, ring, children) in rings.iter() {
            let mat = if ring.axis == axis {
                state.highlight_materials[ring.axis as usize].clone()
            } else {
                state.materials[ring.axis as usize].clone()
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
        rings: &Query<(Entity, &RotateDragRing, &Children)>,
        commands: &mut Commands,
        state: &RotateDragState,
    ) {
        for (_, ring, children) in rings.iter() {
            if let Some(mat) = &state.materials[ring.axis as usize] {
                for child in children.iter() {
                    commands.entity(child)
                        .remove::<MeshMaterial3d<StandardMaterial>>()
                        .insert(MeshMaterial3d(mat.clone()));
                }
            }
        }
    }

    fn despawn_rings(
        rings: Query<Entity, With<RotateDragRing>>,
        mut commands: Commands,
        mut state: ResMut<Self>,
    ) {
        for entity in &rings {
            commands.entity(entity).despawn();
        }
        state.tracked_feature = None;
        state.tracked_axes.clear();
        state.grab = None;
    }
}

/// A [`Torus`] is built lying in XZ with its axis up, so aiming a ring is a
/// matter of turning `Y` onto the axis that ring's component turns about.
fn ring_transform(center: Vec3, angles: Vec3, axis: u8) -> Transform {
    let axis_world = euler_axis_world(angles, axis);
    Transform::from_translation(center)
        .with_rotation(Quat::from_rotation_arc(Vec3::Y, axis_world))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::rotation::quat_from_euler;

    /// Every ring has to lie in the plane its drag reads angles on, or the
    /// mouse would be picking one circle and turning the object on another.
    #[test]
    fn a_ring_faces_the_axis_its_component_turns_about() {
        let angles = Vec3::new(0.3, 1.4, -0.8);
        for axis in 0u8..3 {
            let up = ring_transform(Vec3::ZERO, angles, axis).rotation * Vec3::Y;
            assert!(
                up.angle_between(euler_axis_world(angles, axis)) < 1e-4,
                "the ring for axis {axis} is not in that axis's plane"
            );
        }
    }

    /// The drag's arithmetic in miniature: the cursor moves a quarter turn
    /// around the ring, so the object does too — and lands with its front
    /// pointing where the cursor is, not where the cursor started.
    #[test]
    fn a_quarter_turn_of_the_cursor_is_a_quarter_turn_of_the_object() {
        let axis_world = Vec3::Y;
        let (cos_dir, sin_dir) = axis_world.any_orthonormal_pair();
        let center = Vec3::new(2.0, 0.0, -5.0);

        let start = center + cos_dir * RING_RADIUS;
        let end = center + sin_dir * RING_RADIUS;

        let initial_angle = angle_on_plane(start, center, cos_dir, sin_dir);
        let angle = angle_on_plane(end, center, cos_dir, sin_dir);
        let delta = angle - initial_angle;

        assert!((delta - std::f32::consts::FRAC_PI_2).abs() < 1e-5, "delta was {delta}");
        assert!(quat_from_euler(Vec3::new(0.0, delta, 0.0))
            .abs_diff_eq(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), 1e-5));
    }
}
