//! Handles on a selected footprint room: one per corner, dragged across the
//! floor plane, and its floor and ceiling, dragged up and down.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::editor::editable::{EditEvent, FeatureId, FeatureTimeline};
use crate::editor::footprint_room::FootprintRoom;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::{on_top, HandleMaterial};
use crate::prop::sketch::SketchId;
use crate::tool::tool_helpers::{closest_param_on_axis, snap_vec3};
use crate::tool::Tools;

const SNAP: f32 = 0.1;

pub struct FootprintDragPlugin;

impl Plugin for FootprintDragPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FootprintDragState>()
            .add_systems(
                Update,
                (
                    FootprintDragState::spawn_handles,
                    FootprintDragState::place_handles,
                    FootprintDragState::drag,
                )
                    .chain()
                    .run_if(in_state(Tools::Select))
                    .run_if(in_state(AppMode::Editor)),
            )
            .add_systems(OnExit(Tools::Select), FootprintDragState::despawn_handles);
    }
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FootprintHandle {
    Corner(SketchId),
    Floor,
    Ceiling,
}

#[derive(Resource, Default)]
pub struct FootprintDragState {
    mesh: Option<Handle<Mesh>>,
    idle: Option<Handle<HandleMaterial>>,
    highlight: Option<Handle<HandleMaterial>>,
    /// The footprint and the corners its handles were made for: a corner
    /// added or taken away is a new set of handles.
    tracked: Option<(FeatureId, Vec<SketchId>)>,
    grabbed: Option<FootprintHandle>,
    /// From where the pointer met the handle's line or plane to the handle.
    grab_offset: Option<Vec3>,
}

/// Where each handle goes: corners on the floor, and the floor and ceiling
/// over the middle of the first room.
fn handle_positions(footprint: &FootprintRoom) -> Vec<(FootprintHandle, Vec3)> {
    let mut handles: Vec<(FootprintHandle, Vec3)> =
        footprint.corner_points().map(|(id, at)| (FootprintHandle::Corner(id), at)).collect();
    let middle = footprint.rooms().first().map(|room| room.floor_point()).or_else(|| {
        let count = handles.len() as f32;
        (count > 0.0).then(|| handles.iter().map(|(_, at)| *at).sum::<Vec3>() / count)
    });
    if let Some(middle) = middle {
        let floor = footprint.floor_height();
        handles.push((FootprintHandle::Floor, Vec3::new(middle.x, floor, middle.z)));
        handles.push((FootprintHandle::Ceiling, Vec3::new(middle.x, floor + footprint.height(), middle.z)));
    }
    handles
}

impl FootprintDragState {
    pub fn is_dragging(&self) -> bool {
        self.grabbed.is_some()
    }

    fn ensure_assets(&mut self, meshes: &mut Assets<Mesh>, materials: &mut Assets<HandleMaterial>) {
        if self.mesh.is_some() {
            return;
        }
        self.mesh = Some(meshes.add(Cuboid::new(0.25, 0.25, 0.25)));
        self.idle = Some(materials.add(on_top(StandardMaterial {
            base_color: Color::srgb_u8(230, 220, 160),
            emissive: LinearRgba::rgb(0.3, 0.25, 0.1),
            ..default()
        })));
        self.highlight = Some(materials.add(on_top(StandardMaterial {
            base_color: Color::srgb_u8(255, 255, 220),
            emissive: LinearRgba::rgb(0.45, 0.4, 0.25),
            ..default()
        })));
    }

    fn spawn_handles(
        mut state: ResMut<Self>,
        features: Res<FeatureTimeline>,
        handles: Query<Entity, With<FootprintHandle>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<HandleMaterial>>,
    ) {
        let selected = features.selected_feature().and_then(|id| {
            let footprint = features.get_feature(&id)?.object().footprint()?;
            Some((id, footprint))
        });
        let should_track = selected.map(|(id, footprint)| (id, footprint.corner_points().map(|(corner, _)| corner).collect()));
        if state.tracked == should_track {
            return;
        }

        for entity in &handles {
            commands.entity(entity).despawn();
        }
        state.tracked = should_track;
        state.grabbed = None;
        state.grab_offset = None;

        let Some((_, footprint)) = selected else { return };
        state.ensure_assets(&mut meshes, &mut materials);
        let (Some(mesh), Some(idle)) = (state.mesh.clone(), state.idle.clone()) else { return };
        for (handle, at) in handle_positions(footprint) {
            commands.spawn((
                handle,
                Mesh3d(mesh.clone()),
                MeshMaterial3d(idle.clone()),
                crate::editor::multicam::editor_only(),
                Transform::from_translation(at),
            ));
        }
    }

    fn place_handles(
        state: Res<Self>,
        features: Res<FeatureTimeline>,
        mut handles: Query<(&FootprintHandle, &mut Transform)>,
    ) {
        let Some((id, _)) = &state.tracked else { return };
        let Some(footprint) = features.get_feature(id).and_then(|f| f.object().footprint()) else { return };
        let positions = handle_positions(footprint);
        for (handle, mut transform) in &mut handles {
            if let Some((_, at)) = positions.iter().find(|(h, _)| h == handle) {
                if transform.translation != *at {
                    transform.translation = *at;
                }
            }
        }
    }

    fn drag(
        mut handles: Query<(Entity, &FootprintHandle, &mut MeshMaterial3d<HandleMaterial>)>,
        mut ray_cast: MeshRayCast,
        mouse_input: Res<CurrentMouseInput>,
        mut commands: Commands,
        mut state: ResMut<Self>,
        mut features: ResMut<FeatureTimeline>,
        mut edit_events: MessageWriter<EditEvent>,
    ) {
        let Some((feature_id, _)) = state.tracked.clone() else { return };
        let (Some(idle), Some(highlight)) = (state.idle.clone(), state.highlight.clone()) else { return };

        let released = mouse_input.released == Some(MouseButton::Left);
        let just_pressed = mouse_input.just_pressed && mouse_input.pressed == Some(MouseButton::Left);
        let Some(ray) = mouse_input.world_pos.filter(|_| !released) else {
            state.grabbed = None;
            state.grab_offset = None;
            paint(&mut handles, None, &idle, &highlight);
            return;
        };

        let Some(grabbed) = state.grabbed else {
            let hit = {
                let filter = |entity: Entity| handles.contains(entity);
                let settings = MeshRayCastSettings::default().with_filter(&filter);
                ray_cast.cast_ray(ray, &settings).first().map(|(entity, _)| *entity)
            };
            paint(&mut handles, hit, &idle, &highlight);
            if let (Some(hit), true) = (hit, just_pressed) {
                if let Ok((_, handle, _)) = handles.get(hit) {
                    state.grabbed = Some(*handle);
                    state.grab_offset = None;
                }
            }
            return;
        };

        let Some(footprint) = features.get_feature(&feature_id).and_then(|f| f.object().footprint()) else { return };
        let Some((_, handle_at)) = handle_positions(footprint).into_iter().find(|(h, _)| *h == grabbed) else { return };

        // Where the pointer is on the handle's own line or plane.
        let pointer = match grabbed {
            FootprintHandle::Corner(_) => {
                let direction = Vec3::from(ray.direction);
                if direction.y.abs() < 1.0e-4 {
                    return;
                }
                let t = (handle_at.y - ray.origin.y) / direction.y;
                if t < 0.0 {
                    return;
                }
                ray.origin + direction * t
            }
            FootprintHandle::Floor | FootprintHandle::Ceiling => {
                let origin = Vec3::new(handle_at.x, 0.0, handle_at.z);
                let Some(y) = closest_param_on_axis(ray, origin, Vec3::Y) else { return };
                Vec3::new(handle_at.x, y, handle_at.z)
            }
        };
        let offset = *state.grab_offset.get_or_insert(handle_at - pointer);
        let wanted = snap_vec3(pointer + offset, SNAP);

        let Some(mut feature) = features.features_mut().remove(&feature_id) else { return };
        let moved = match (grabbed, feature.object_mut().footprint_mut()) {
            (FootprintHandle::Corner(id), Some(footprint)) => footprint.drag_corner(id, Vec2::new(wanted.x, wanted.z)),
            (FootprintHandle::Floor, _) => feature.object_mut().drag_handle(false, 1, wanted.y),
            (FootprintHandle::Ceiling, _) => feature.object_mut().drag_handle(true, 1, wanted.y),
            _ => false,
        };
        if moved {
            feature.object_mut().resolve_references(features.features_map());
            if let Some(entity) = feature.object().entity() {
                feature.object_mut().apply_to_entity(&mut commands, entity);
                edit_events.write(EditEvent { editor_id: feature_id._id(), feature_id, entity });
            }
        }
        features.features_mut().insert(feature_id, feature);
    }

    fn despawn_handles(handles: Query<Entity, With<FootprintHandle>>, mut commands: Commands, mut state: ResMut<Self>) {
        for entity in &handles {
            commands.entity(entity).despawn();
        }
        state.tracked = None;
        state.grabbed = None;
        state.grab_offset = None;
    }
}

/// Lights the handle under the pointer, writing a material only when it is
/// not already the one wanted: this runs every frame, and a written material
/// is re-extracted for rendering.
fn paint(
    handles: &mut Query<(Entity, &FootprintHandle, &mut MeshMaterial3d<HandleMaterial>)>,
    highlighted: Option<Entity>,
    idle: &Handle<HandleMaterial>,
    highlight: &Handle<HandleMaterial>,
) {
    for (entity, _, mut material) in handles.iter_mut() {
        let wanted = if highlighted == Some(entity) { highlight } else { idle };
        if material.0 != *wanted {
            material.0 = wanted.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::editable::{FeatureTrait, PointRef};
    use bevy::platform::collections::HashMap;

    #[test]
    fn a_footprint_has_a_handle_per_corner_and_its_floor_and_ceiling() {
        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 1.0, 0.0), 2.0);
        let ids: Vec<SketchId> = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0)]
            .iter()
            .map(|(x, z)| room.add_corner(PointRef::absolute(*x, 0.0, *z)))
            .collect();
        for i in 0..3 {
            room.add_line(ids[i], ids[(i + 1) % 3]);
        }
        room.resolve_references(&HashMap::new());
        room.name_region_at(Vec2::new(3.0, 1.0));
        room.resolve_references(&HashMap::new());

        let handles = handle_positions(&room);
        assert_eq!(handles.len(), 5);
        assert!(handles.contains(&(FootprintHandle::Corner(ids[1]), Vec3::new(4.0, 1.0, 0.0))));
        let floor = handles.iter().find(|(h, _)| *h == FootprintHandle::Floor).unwrap().1;
        let ceiling = handles.iter().find(|(h, _)| *h == FootprintHandle::Ceiling).unwrap().1;
        assert_eq!((floor.y, ceiling.y), (1.0, 3.0));
        assert!(room.rooms()[0].contains(floor), "the floor handle is outside the room");
    }
}
