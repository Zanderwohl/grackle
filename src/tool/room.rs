use bevy::app::App;
use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;
use crate::common::app_mode::AppMode;
use crate::editor::editable::{EditEvent, FeatureId, FeatureTimeline, PointRef};
use crate::editor::editor_room::EditorRoom;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const DEFAULT_SNAP_GRANULARITY: f32 = 0.1;

pub struct RoomPlugin;

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<RoomTool>()
            .init_resource::<RoomDragState>()
            .add_systems(Update, (
                RoomTool::interface,
                RoomTool::draw_gizmos,
                RoomTool::draw_room_bounds,
            ).chain().run_if(in_state(Tools::Room)).run_if(in_state(AppMode::Editor)))
            .add_systems(OnExit(Tools::Room), RoomTool::on_exit)
            .add_systems(Update, (
                RoomDragState::spawn_handles_system,
                RoomDragState::handle_dragging,
                RoomDragState::update_handle_positions,
            ).chain().run_if(in_state(Tools::Select)).run_if(in_state(AppMode::Editor)))
            .add_systems(OnExit(Tools::Select), RoomDragState::despawn_handles)
        ;
    }
}

#[derive(Clone)]
enum RoomCornerMode {
    Normal,
    Picking,
    RelativeSelected {
        reference_feature: FeatureId,
        reference_key: String,
        reference_resolved: Vec3,
    },
}

enum FarCorner {
    FromMin(Vec3),
    Point(PointRef),
}

enum RoomToolMode {
    PlacingMin(RoomCornerMode),
    PlacingMax(RoomCornerMode),
}

#[derive(Resource)]
struct RoomTool {
    mode: RoomToolMode,
    last_min: Vec3,
    /// The last room's far corner from its first, so the next is suggested at
    /// that size from wherever its own first corner went.
    last_size: Vec3,
    cursor: Option<Vec3>,
    hovered_point: Option<(FeatureId, String, Vec3)>,
    min_point: Option<PointRef>,
    min_resolved: Option<Vec3>,
    /// What Shift interrupted, put back if the Shift was held to fly faster.
    before_picking: Option<RoomCornerMode>,
    flew: bool,
    snap: bool,
    snap_granularity: f32,
}

impl Default for RoomTool {
    fn default() -> Self {
        Self {
            mode: RoomToolMode::PlacingMin(RoomCornerMode::Normal),
            last_min: Vec3::new(-1.0, 0.0, -1.0),
            last_size: Vec3::new(2.0, 2.0, 2.0),
            cursor: None,
            hovered_point: None,
            min_point: None,
            min_resolved: None,
            before_picking: None,
            flew: false,
            snap: true,
            snap_granularity: DEFAULT_SNAP_GRANULARITY,
        }
    }
}

impl RoomTool {
    fn on_exit(mut tool: ResMut<Self>) {
        tool.mode = RoomToolMode::PlacingMin(RoomCornerMode::Normal);
        tool.cursor = None;
        tool.hovered_point = None;
        tool.min_point = None;
        tool.min_resolved = None;
        tool.before_picking = None;
    }

    fn suggestion(&self) -> Vec3 {
        match &self.mode {
            RoomToolMode::PlacingMin(_) => self.last_min,
            RoomToolMode::PlacingMax(_) => self.min_resolved.unwrap_or(self.last_min) + self.last_size,
        }
    }

    fn interface(
        mut tool: ResMut<Self>,
        cameras: Query<(Entity, &Multicam)>,
        mouse_input: Res<CurrentMouseInput>,
        keys: Res<ButtonInput<KeyCode>>,
        mut features: ResMut<FeatureTimeline>,
        rooms: Query<&Room>,
        mut next_tool: ResMut<NextState<Tools>>,
        egui: Res<EguiWantsInput>,
    ) {
        let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let shift_just_pressed = keys.just_pressed(KeyCode::ShiftLeft) || keys.just_pressed(KeyCode::ShiftRight);

        let suggestion = tool.suggestion();

        tool.cursor = compute_cursor(
            &mouse_input, &cameras, suggestion,
            tool.snap, tool.snap_granularity, &rooms,
        );

        // Escape while placing max discards the min, unless placing relatively, where it means zero offset.
        if matches!(tool.mode, RoomToolMode::PlacingMax(ref cm) if !matches!(cm, RoomCornerMode::RelativeSelected { .. }))
            && keys.just_pressed(KeyCode::Escape)
        {
            tool.mode = RoomToolMode::PlacingMin(RoomCornerMode::Normal);
            tool.min_point = None;
            tool.min_resolved = None;
            tool.hovered_point = None;
            return;
        }

        // Cloned to avoid holding a borrow on tool.mode.
        let is_placing_min = matches!(tool.mode, RoomToolMode::PlacingMin(_));
        let corner_mode = match &tool.mode {
            RoomToolMode::PlacingMin(cm) | RoomToolMode::PlacingMax(cm) => cm.clone(),
        };

        fn set_mode(tool: &mut RoomTool, is_min: bool, cm: RoomCornerMode) {
            tool.mode = if is_min {
                RoomToolMode::PlacingMin(cm)
            } else {
                RoomToolMode::PlacingMax(cm)
            };
        }

        match corner_mode {
            RoomCornerMode::Normal => {
                if shift_held {
                    set_mode(&mut tool, is_placing_min, RoomCornerMode::Picking);
                    tool.hovered_point = None;
                    tool.before_picking = None;
                    tool.flew = false;
                } else if is_placing_min {
                    if let Some(cursor) = tool.cursor
                        && commit_at_cursor(&mouse_input, &keys, egui.wants_keyboard_input())
                    {
                        tool.min_point = Some(PointRef::absolute(cursor.x, cursor.y, cursor.z));
                        tool.min_resolved = Some(cursor);
                        tool.last_min = cursor;
                        tool.mode = RoomToolMode::PlacingMax(RoomCornerMode::Normal);
                        tool.hovered_point = None;
                    }
                } else if let (Some(cursor), Some(min)) = (tool.cursor, tool.min_resolved) {
                    // The far corner is a size, measured from the first.
                    let typing = egui.wants_keyboard_input();
                    if commit_at_cursor(&mouse_input, &keys, typing) {
                        Self::create_room(&mut tool, &mut features, &mut next_tool, FarCorner::FromMin(cursor - min), cursor);
                    }
                }
            }
            RoomCornerMode::Picking => {
                tool.flew |= camera_dragging(&mouse_input);
                if !shift_held {
                    let back = tool.before_picking.take().filter(|_| tool.flew);
                    set_mode(&mut tool, is_placing_min, back.unwrap_or(RoomCornerMode::Normal));
                    tool.hovered_point = None;
                    return;
                }

                tool.hovered_point = mouse_input.world_pos
                    .and_then(|ray| find_hovered_point(&ray, &features, PICK_RADIUS))
                    .map(|(fid, key, pos)| {
                        let pos = if tool.snap {
                            snap_vec3(pos, tool.snap_granularity)
                        } else {
                            pos
                        };
                        (fid, key, pos)
                    });

                if mouse_input.released == Some(MouseButton::Left) {
                    if let Some((feature_id, key, resolved)) = tool.hovered_point.take() {
                        set_mode(&mut tool, is_placing_min, RoomCornerMode::RelativeSelected {
                            reference_feature: feature_id,
                            reference_key: key,
                            reference_resolved: resolved,
                        });
                    }
                }
            }
            RoomCornerMode::RelativeSelected { reference_feature, reference_key, reference_resolved } => {
                // Kept until Shift comes up, in case it was held to fly faster.
                if shift_just_pressed {
                    tool.before_picking = Some(RoomCornerMode::RelativeSelected {
                        reference_feature,
                        reference_key: reference_key.clone(),
                        reference_resolved,
                    });
                    tool.flew = false;
                    tool.hovered_point = None;
                    set_mode(&mut tool, is_placing_min, RoomCornerMode::Picking);
                    return;
                }

                let typing = egui.wants_keyboard_input();
                if let Some(cursor) = relative_commit(&mouse_input, &keys, typing, tool.cursor, reference_resolved) {
                    let d = cursor - reference_resolved;
                    let mut pr = PointRef::reference_with_offset(reference_feature, d.x, d.y, d.z);
                    if !reference_key.is_empty() {
                        pr.point_key = reference_key.clone();
                    }
                    if is_placing_min {
                        tool.min_point = Some(pr);
                        tool.min_resolved = Some(cursor);
                        tool.last_min = cursor;
                        tool.mode = RoomToolMode::PlacingMax(RoomCornerMode::Normal);
                    } else {
                        Self::create_room(&mut tool, &mut features, &mut next_tool, FarCorner::Point(pr), cursor);
                    }
                }
            }
        }
    }

    fn create_room(
        tool: &mut ResMut<Self>,
        features: &mut ResMut<FeatureTimeline>,
        next_tool: &mut ResMut<NextState<Tools>>,
        max: FarCorner,
        max_resolved: Vec3,
    ) {
        if let Some(min_point) = tool.min_point.take() {
            let room = match max {
                FarCorner::FromMin(size) => EditorRoom::from_min_and_size(min_point, size),
                FarCorner::Point(max_point) => EditorRoom::from_point_refs(min_point, max_point),
            };
            let id = features.apply_feature(Box::new(room));
            features.select(Some(id));
            if let Some(min) = tool.min_resolved {
                tool.last_size = max_resolved - min;
            }
            tool.min_resolved = None;
            tool.mode = RoomToolMode::PlacingMin(RoomCornerMode::Normal);
            next_tool.set(Tools::Select);
        }
    }

    fn draw_gizmos(
        tool: Res<RoomTool>,
        features: Res<FeatureTimeline>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        let corner_mode = match &tool.mode {
            RoomToolMode::PlacingMin(cm) | RoomToolMode::PlacingMax(cm) => cm,
        };
        let from_min = matches!((&tool.mode, corner_mode), (RoomToolMode::PlacingMax(_), RoomCornerMode::Normal));
        let is_relative = from_min || matches!(corner_mode, RoomCornerMode::RelativeSelected { .. });

        if let Some(cursor) = tool.cursor {
            let color = if is_relative {
                Color::srgb_u8(80, 140, 255)
            } else {
                Color::srgb_u8(60, 120, 255)
            };
            gizmos.sphere(Isometry3d::from_translation(cursor), 0.15, color);

            // When placing max, draw the min point and a bounds preview
            if let (RoomToolMode::PlacingMax(_), Some(min_resolved)) = (&tool.mode, tool.min_resolved) {
                let min_color = Color::srgb_u8(100, 255, 100);
                gizmos.sphere(Isometry3d::from_translation(min_resolved), 0.15, min_color);
                let preview_color = Color::srgb_u8(40, 40, 200);
                bounds_gizmo(&mut gizmos, min_resolved, cursor, preview_color);
            }

            if let RoomCornerMode::RelativeSelected { reference_resolved, .. } = corner_mode {
                draw_taxicab_path(&mut gizmos, *reference_resolved, cursor);
            } else if let (true, Some(min)) = (from_min, tool.min_resolved) {
                draw_taxicab_path(&mut gizmos, min, cursor);
            }
        }

        if matches!(corner_mode, RoomCornerMode::Picking) {
            if let Some(ray) = mouse_input.world_pos {
                draw_picking_gizmos(&mut gizmos, &ray, &features, &tool.hovered_point);
            }
        }
    }

    fn draw_room_bounds(
        mut gizmos: Gizmos,
        rooms: Query<(Entity, &Room)>,
    ) {
        let color = Color::srgb_u8(100, 100, 100);
        for (_, room) in rooms {
            bounds_gizmo(&mut gizmos, room.min, room.max, color);
        }
    }
}

#[derive(Component)]
pub struct RoomDragHandle {
    axis: HandleAxis,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum HandleAxis {
    MinX, MinY, MinZ,
    MaxX, MaxY, MaxZ,
}

impl HandleAxis {
    fn is_max(&self) -> bool {
        matches!(self, HandleAxis::MaxX | HandleAxis::MaxY | HandleAxis::MaxZ)
    }

    fn axis_index(&self) -> u8 {
        match self {
            HandleAxis::MinX | HandleAxis::MaxX => 0,
            HandleAxis::MinY | HandleAxis::MaxY => 1,
            HandleAxis::MinZ | HandleAxis::MaxZ => 2,
        }
    }

    fn face_centers(min: Vec3, max: Vec3) -> [(Vec3, HandleAxis); 6] {
        let c = (min + max) / 2.0;
        [
            (Vec3::new(min.x, c.y, c.z), HandleAxis::MinX),
            (Vec3::new(max.x, c.y, c.z), HandleAxis::MaxX),
            (Vec3::new(c.x, min.y, c.z), HandleAxis::MinY),
            (Vec3::new(c.x, max.y, c.z), HandleAxis::MaxY),
            (Vec3::new(c.x, c.y, min.z), HandleAxis::MinZ),
            (Vec3::new(c.x, c.y, max.z), HandleAxis::MaxZ),
        ]
    }
}

#[derive(Resource, Default)]
pub struct RoomDragState {
    handle_mesh: Option<Handle<Mesh>>,
    idle_material: Option<Handle<StandardMaterial>>,
    highlight_material: Option<Handle<StandardMaterial>>,
    tracked_feature: Option<FeatureId>,
    grabbed_handle: Option<HandleAxis>,
    grab_offset: Option<f32>,
}

impl RoomDragState {
    pub fn is_dragging(&self) -> bool {
        self.grabbed_handle.is_some()
    }

    fn ensure_assets(
        &mut self,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
    ) {
        if self.handle_mesh.is_none() {
            self.handle_mesh = Some(meshes.add(Cuboid::new(0.3, 0.3, 0.3)));
            self.idle_material = Some(materials.add(StandardMaterial {
                base_color: Color::srgb_u8(180, 230, 180),
                emissive: LinearRgba::rgb(0.2, 0.3, 0.2),
                ..Default::default()
            }));
            self.highlight_material = Some(materials.add(StandardMaterial {
                base_color: Color::srgb_u8(220, 255, 255),
                emissive: LinearRgba::rgb(0.3, 0.4, 0.4),
                ..Default::default()
            }));
        }
    }

    fn spawn_handles_system(
        mut state: ResMut<Self>,
        features: Res<FeatureTimeline>,
        handles: Query<Entity, With<RoomDragHandle>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        let current_feature = features.selected_feature();

        let bounds = current_feature
            .and_then(|id| features.get_feature(&id))
            .and_then(|a| a.object().drag_handle_bounds());

        let should_track = current_feature.filter(|_| bounds.is_some());

        if state.tracked_feature != should_track {
            for entity in &handles {
                commands.entity(entity).despawn();
            }
            state.tracked_feature = should_track;
            state.grabbed_handle = None;
            state.grab_offset = None;

            if let (Some(_feature_id), Some((min, max))) = (should_track, bounds) {
                state.ensure_assets(&mut meshes, &mut materials);
                let mesh = state.handle_mesh.clone().unwrap();
                let mat = state.idle_material.clone().unwrap();

                for (center, axis) in HandleAxis::face_centers(min, max) {
                    commands.spawn((
                        RoomDragHandle { axis },
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(mat.clone()),
                        crate::editor::multicam::editor_only(),
                        Transform::from_translation(center),
                    ));
                }
            }
        }
    }

    fn update_handle_positions(
        features: Res<FeatureTimeline>,
        state: Res<RoomDragState>,
        mut handles: Query<(&RoomDragHandle, &mut Transform)>,
    ) {
        let Some(feature_id) = state.tracked_feature else { return; };
        let Some(feature) = features.get_feature(&feature_id) else { return; };
        let Some((min, max)) = feature.object().drag_handle_bounds() else { return; };

        let centers = HandleAxis::face_centers(min, max);
        for (handle, mut tfm) in &mut handles {
            for (pos, axis) in &centers {
                if handle.axis == *axis {
                    tfm.translation = *pos;
                    break;
                }
            }
        }
    }

    fn handle_dragging(
        mut handles: Query<(Entity, &RoomDragHandle, &mut MeshMaterial3d<StandardMaterial>)>,
        mut ray_cast: MeshRayCast,
        mouse_input: Res<CurrentMouseInput>,
        mut commands: Commands,
        mut state: ResMut<Self>,
        mut features: ResMut<FeatureTimeline>,
        mut edit_events: MessageWriter<EditEvent>,
    ) {
        let Some(feature_id) = state.tracked_feature else { return; };

        let idle = state.idle_material.clone();
        let highlight = state.highlight_material.clone();
        let (Some(idle), Some(highlight)) = (idle, highlight) else { return; };

        let ray = mouse_input.world_pos;
        let mouse_released = mouse_input.released == Some(MouseButton::Left);
        let mouse_just_pressed = mouse_input.just_pressed && mouse_input.pressed == Some(MouseButton::Left);

        if mouse_released || ray.is_none() {
            if state.grabbed_handle.is_some() {
                state.grabbed_handle = None;
                state.grab_offset = None;
            }
            Self::paint(&mut handles, None, &idle, &highlight);
            return;
        }

        let ray = ray.unwrap();

        if let Some(handle_axis) = state.grabbed_handle {
            let axis_index = handle_axis.axis_index();
            let axis_dir = match axis_index {
                0 => Vec3::X,
                1 => Vec3::Y,
                _ => Vec3::Z,
            };

            let Some((current_min, current_max)) = features.get_feature(&feature_id)
                .and_then(|a| a.object().drag_handle_bounds()) else { return; };

            let face_center = HandleAxis::face_centers(current_min, current_max)
                .into_iter()
                .find(|(_, a)| *a == handle_axis)
                .map(|(pos, _)| pos)
                .unwrap_or(Vec3::ZERO);

            let axis_origin = face_center - axis_dir * face_center.dot(axis_dir);

            let Some(projected) = closest_param_on_axis(ray, axis_origin, axis_dir) else { return; };

            let offset = match state.grab_offset {
                Some(off) => off,
                None => {
                    let current_value = face_center.dot(axis_dir);
                    let off = projected - current_value;
                    state.grab_offset = Some(off);
                    off
                }
            };

            let g = DEFAULT_SNAP_GRANULARITY;
            let raw = projected - offset;
            let new_value = match handle_axis {
                HandleAxis::MinX | HandleAxis::MinY | HandleAxis::MinZ =>
                    f32::min((raw / g).ceil() * g, match axis_index { 0 => current_max.x, 1 => current_max.y, _ => current_max.z } - g),
                HandleAxis::MaxX | HandleAxis::MaxY | HandleAxis::MaxZ =>
                    f32::max((raw / g).ceil() * g, match axis_index { 0 => current_min.x, 1 => current_min.y, _ => current_min.z } + g),
            };

            if let Some(mut feature) = features.features_mut().remove(&feature_id) {
                let modified = feature.object_mut().drag_handle(
                    handle_axis.is_max(),
                    axis_index,
                    new_value,
                );
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
            let hit = {
                let filter = |entity: Entity| handles.contains(entity);
                let settings = MeshRayCastSettings::default().with_filter(&filter);
                ray_cast.cast_ray(ray, &settings).first().map(|(entity, _)| *entity)
            };

            Self::paint(&mut handles, hit, &idle, &highlight);

            if let Some(hit) = hit {
                if mouse_just_pressed {
                    if let Ok((_, handle, _)) = handles.get(hit) {
                        state.grabbed_handle = Some(handle.axis);
                        state.grab_offset = None;
                    }
                }
            }
        }
    }

    /// Lights the handle under the pointer and puts the rest back, writing a
    /// material only when it is not already the one wanted. This runs every
    /// frame the pointer is over a viewport, and an unconditional write is a
    /// changed component on every handle, each re-extracted for rendering.
    fn paint(
        handles: &mut Query<(Entity, &RoomDragHandle, &mut MeshMaterial3d<StandardMaterial>)>,
        highlighted: Option<Entity>,
        idle: &Handle<StandardMaterial>,
        highlight: &Handle<StandardMaterial>,
    ) {
        for (entity, _, mut material) in handles.iter_mut() {
            let wanted = if highlighted == Some(entity) { highlight } else { idle };
            if material.0 != *wanted {
                material.0 = wanted.clone();
            }
        }
    }

    fn despawn_handles(
        handles: Query<Entity, With<RoomDragHandle>>,
        mut commands: Commands,
        mut state: ResMut<Self>,
    ) {
        for entity in &handles {
            commands.entity(entity).despawn();
        }
        state.tracked_feature = None;
        state.grabbed_handle = None;
        state.grab_offset = None;
    }
}

#[cfg(test)]
mod drag_handle_tests {
    use super::*;

    /// Hovering a handle frame after frame must not keep rewriting the
    /// materials: a written `MeshMaterial3d` is a changed one, and every
    /// changed one is re-extracted for rendering.
    #[test]
    fn hovering_the_same_handle_again_writes_nothing() {
        let mut world = World::new();
        let mut materials = Assets::<StandardMaterial>::default();
        let idle = materials.add(StandardMaterial::default());
        let lit = materials.add(StandardMaterial::default());
        let handles: Vec<Entity> = HandleAxis::face_centers(Vec3::ZERO, Vec3::ONE)
            .into_iter()
            .map(|(_, axis)| world.spawn((RoomDragHandle { axis }, MeshMaterial3d(idle.clone()))).id())
            .collect();
        let hovered = handles[3];

        let mut schedule = Schedule::default();
        let (i, l) = (idle.clone(), lit.clone());
        schedule.add_systems(move |
            mut handles: Query<(Entity, &RoomDragHandle, &mut MeshMaterial3d<StandardMaterial>)>,
        | RoomDragState::paint(&mut handles, Some(hovered), &i, &l));

        schedule.run(&mut world);
        assert_eq!(world.get::<MeshMaterial3d<StandardMaterial>>(hovered).unwrap().0, lit);
        assert_eq!(world.get::<MeshMaterial3d<StandardMaterial>>(handles[0]).unwrap().0, idle);

        let archetypes: Vec<_> = handles.iter().map(|e| world.entity(*e).archetype().id()).collect();
        world.clear_trackers();
        schedule.run(&mut world);
        for (handle, archetype) in handles.iter().zip(archetypes) {
            let entity = world.entity(*handle);
            assert_eq!(entity.archetype().id(), archetype);
            assert!(!entity.get_ref::<MeshMaterial3d<StandardMaterial>>().unwrap().is_changed());
        }
    }
}

#[derive(Component, Clone, Debug)]
pub struct Room {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for Room {
    fn default() -> Self {
        Self::new(Vec3::ZERO, Vec3::ONE)
    }
}

impl Room {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// Walk the solid parts of this room's six faces, after every opening
    /// other rooms carve. Rendering and collision both use this so they agree.
    fn for_each_solid_rect(
        &self,
        others: &[Room],
        mut visit: impl FnMut(&RoomFace, &crate::common::rect_subtract::Rect2D),
    ) {
        use crate::common::rect_subtract::subtract_rects;

        for face in RoomFace::enumerate(self) {
            let mut holes = Vec::new();
            for other in others {
                if let Some(clip) = face.clip_rect(other) {
                    holes.push(clip);
                }
            }

            for rect in subtract_rects(&face.rect, &holes) {
                visit(&face, &rect);
            }
        }
    }

    /// The solid wall rectangles [`Room::bake_faces`] renders, as world-space
    /// boxes. `thickness` extends each slab away from the interior.
    pub fn collision_slabs(&self, others: &[Room], thickness: f32) -> Vec<WallSlab> {
        let mut slabs = Vec::new();
        self.for_each_solid_rect(others, |face, rect| {
            slabs.push(face.rect_to_slab(rect, thickness));
        });
        slabs
    }

    /// How far along `ray` it first meets this room's solid wall from the
    /// inside, and where.
    ///
    /// Outsides are culled by the renderer and ignored here too, so whatever
    /// stands inside a room is nearer than the room. Openings use the same
    /// holes [`Room::bake_faces`] cuts, so a pick goes through a doorway.
    /// `others` is an iterator, not a slice, so a picker asking this of every
    /// room needs no `Vec` of everybody else.
    pub fn ray_hit_from_inside<'a>(
        &self,
        others: impl IntoIterator<Item = &'a Room, IntoIter: Clone>,
        ray: &Ray3d,
    ) -> Option<(f32, Vec3)> {
        let others = others.into_iter();
        let dir = Vec3::from(ray.direction);
        let mut best: Option<(f32, Vec3)> = None;

        for face in RoomFace::enumerate(self) {
            if face.normal.dot(dir) >= 0.0 {
                continue;
            }
            let axis = face.fixed_axis as usize;
            if dir[axis].abs() < 1e-8 {
                continue;
            }
            let t = (face.fixed_value - ray.origin[axis]) / dir[axis];
            if t < 0.0 || best.is_some_and(|(best_t, _)| t >= best_t) {
                continue;
            }

            let hit = ray.origin + dir * t;
            let (u, v) = face.uv(hit);
            let on_face = u >= face.rect.min_u && u <= face.rect.max_u
                && v >= face.rect.min_v && v <= face.rect.max_v;
            // Strictly inside: a hit on a hole's edge is still on wall.
            let in_a_hole = others.clone().filter_map(|other| face.clip_rect(other)).any(|hole| {
                u > hole.min_u && u < hole.max_u && v > hole.min_v && v < hole.max_v
            });

            if on_face && !in_a_hole {
                best = Some((t, hit));
            }
        }

        best
    }

    /// Bake this room's wall geometry, carving openings where other rooms
    /// overlap or share walls. Returns a single Mesh with inward-facing normals.
    pub fn bake_faces(&self, others: &[Room]) -> Mesh {
        let mut vertices: Vec<[f32; 3]> = Vec::new();
        let mut normals: Vec<[f32; 3]> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();

        self.for_each_solid_rect(others, |face, rect| {
            {
                let base = vertices.len() as u32;
                let (p0, p1, p2, p3) = face.rect_to_3d(rect);
                vertices.push(p0.into());
                vertices.push(p1.into());
                vertices.push(p2.into());
                vertices.push(p3.into());
                let n: [f32; 3] = face.normal.into();
                normals.extend_from_slice(&[n, n, n, n]);

                let face_u_span = face.rect.max_u - face.rect.min_u;
                let face_v_span = face.rect.max_v - face.rect.min_v;
                let u0 = if face_u_span > 0.0 { (rect.min_u - face.rect.min_u) / face_u_span } else { 0.0 };
                let u1 = if face_u_span > 0.0 { (rect.max_u - face.rect.min_u) / face_u_span } else { 1.0 };
                let v0 = if face_v_span > 0.0 { (rect.min_v - face.rect.min_v) / face_v_span } else { 0.0 };
                let v1 = if face_v_span > 0.0 { (rect.max_v - face.rect.min_v) / face_v_span } else { 1.0 };
                uvs.push([u0, v0]);
                uvs.push([u1, v0]);
                uvs.push([u1, v1]);
                uvs.push([u0, v1]);

                // Two triangles per quad, winding matches the face's inward normal
                if face.winding_flip {
                    indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
                } else {
                    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                }
            }
        });

        let mut mesh = Mesh::new(
            bevy::render::render_resource::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vertices);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(bevy::mesh::Indices::U32(indices));
        mesh
    }
    
}

/// One solid piece of wall, as a world-space axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallSlab {
    pub min: Vec3,
    pub max: Vec3,
}

/// Represents one face of a room cuboid, projected into a 2D coordinate system.
struct RoomFace {
    /// Which axis is fixed (0=X, 1=Y, 2=Z)
    fixed_axis: u8,
    /// The value on the fixed axis where this face lives
    fixed_value: f32,
    /// The opposite end of the room on the fixed axis
    opposite_value: f32,
    /// Whether this is the min or max face on that axis
    is_max: bool,
    /// The 2D rectangle in (u, v) space for the two remaining axes
    rect: crate::common::rect_subtract::Rect2D,
    /// The inward-facing normal
    normal: Vec3,
    /// Whether the triangle winding needs to be flipped for this face
    winding_flip: bool,
}

impl RoomFace {
    fn enumerate(room: &Room) -> [RoomFace; 6] {
        use crate::common::rect_subtract::Rect2D;
        [
            // -X face (left wall), normal +X (inward)
            RoomFace {
                fixed_axis: 0, fixed_value: room.min.x, opposite_value: room.max.x, is_max: false,
                rect: Rect2D::new(room.min.z, room.min.y, room.max.z, room.max.y),
                normal: Vec3::X,
                winding_flip: true,
            },
            // +X face (right wall), normal -X (inward)
            RoomFace {
                fixed_axis: 0, fixed_value: room.max.x, opposite_value: room.min.x, is_max: true,
                rect: Rect2D::new(room.min.z, room.min.y, room.max.z, room.max.y),
                normal: Vec3::NEG_X,
                winding_flip: false,
            },
            // -Y face (floor), normal +Y (inward)
            RoomFace {
                fixed_axis: 1, fixed_value: room.min.y, opposite_value: room.max.y, is_max: false,
                rect: Rect2D::new(room.min.x, room.min.z, room.max.x, room.max.z),
                normal: Vec3::Y,
                winding_flip: true,
            },
            // +Y face (ceiling), normal -Y (inward)
            RoomFace {
                fixed_axis: 1, fixed_value: room.max.y, opposite_value: room.min.y, is_max: true,
                rect: Rect2D::new(room.min.x, room.min.z, room.max.x, room.max.z),
                normal: Vec3::NEG_Y,
                winding_flip: false,
            },
            // -Z face (front wall), normal +Z (inward)
            RoomFace {
                fixed_axis: 2, fixed_value: room.min.z, opposite_value: room.max.z, is_max: false,
                rect: Rect2D::new(room.min.x, room.min.y, room.max.x, room.max.y),
                normal: Vec3::Z,
                winding_flip: false,
            },
            // +Z face (back wall), normal -Z (inward)
            RoomFace {
                fixed_axis: 2, fixed_value: room.max.z, opposite_value: room.min.z, is_max: true,
                rect: Rect2D::new(room.min.x, room.min.y, room.max.x, room.max.y),
                normal: Vec3::NEG_Z,
                winding_flip: true,
            },
        ]
    }

    /// Returns the 2D clipping rectangle if `other` room overlaps this face.
    /// Covers both shared-wall (coplanar) and penetration cases,
    /// but rejects engulfment (other room fully contains this room on the fixed axis).
    fn clip_rect(&self, other: &Room) -> Option<crate::common::rect_subtract::Rect2D> {
        let (other_min_fixed, other_max_fixed, other_min_u, other_max_u, other_min_v, other_max_v) =
            match self.fixed_axis {
                0 => (other.min.x, other.max.x, other.min.z, other.max.z, other.min.y, other.max.y),
                1 => (other.min.y, other.max.y, other.min.x, other.max.x, other.min.z, other.max.z),
                2 => (other.min.z, other.max.z, other.min.x, other.max.x, other.min.y, other.max.y),
                _ => unreachable!(),
            };

        // The other room must cross this face from outside: a shared wall
        // (it starts at the face plane F and extends outward) or penetration
        // (other_min < F < other_max). Engulfing the room on this axis does
        // not count as connecting to it.

        let straddles = if self.is_max {
            other_min_fixed <= self.fixed_value && other_max_fixed > self.fixed_value
        } else {
            other_min_fixed < self.fixed_value && other_max_fixed >= self.fixed_value
        };

        if !straddles {
            return None;
        }

        let engulfs = if self.is_max {
            other_min_fixed <= self.opposite_value && other_max_fixed >= self.fixed_value
        } else {
            other_min_fixed <= self.fixed_value && other_max_fixed >= self.opposite_value
        };

        if engulfs {
            return None;
        }

        let clip = crate::common::rect_subtract::Rect2D::new(
            other_min_u, other_min_v,
            other_max_u, other_max_v,
        );

        self.rect.intersection(&clip)
    }

    /// A world-space point on this face's plane, in the face's `(u, v)`.
    /// The inverse of [`RoomFace::rect_to_3d`]'s axis mapping.
    fn uv(&self, p: Vec3) -> (f32, f32) {
        match self.fixed_axis {
            0 => (p.z, p.y),
            1 => (p.x, p.z),
            2 => (p.x, p.y),
            _ => unreachable!(),
        }
    }

    /// Convert a 2D sub-rectangle into a solid box behind this face, grown
    /// away from `opposite_value`. Mirrors [`RoomFace::rect_to_3d`]'s axis
    /// mapping.
    fn rect_to_slab(&self, r: &crate::common::rect_subtract::Rect2D, thickness: f32) -> WallSlab {
        let (near, far) = if self.is_max {
            (self.fixed_value, self.fixed_value + thickness)
        } else {
            (self.fixed_value - thickness, self.fixed_value)
        };
        match self.fixed_axis {
            0 => WallSlab {
                min: Vec3::new(near, r.min_v, r.min_u),
                max: Vec3::new(far, r.max_v, r.max_u),
            },
            1 => WallSlab {
                min: Vec3::new(r.min_u, near, r.min_v),
                max: Vec3::new(r.max_u, far, r.max_v),
            },
            2 => WallSlab {
                min: Vec3::new(r.min_u, r.min_v, near),
                max: Vec3::new(r.max_u, r.max_v, far),
            },
            _ => unreachable!(),
        }
    }

    /// Convert a 2D sub-rectangle back into four 3D vertices on this face's plane.
    /// Returns corners in order: (min_u, min_v), (max_u, min_v), (max_u, max_v), (min_u, max_v)
    fn rect_to_3d(&self, r: &crate::common::rect_subtract::Rect2D) -> (Vec3, Vec3, Vec3, Vec3) {
        match self.fixed_axis {
            0 => {
                let x = self.fixed_value;
                (Vec3::new(x, r.min_v, r.min_u), Vec3::new(x, r.min_v, r.max_u),
                 Vec3::new(x, r.max_v, r.max_u), Vec3::new(x, r.max_v, r.min_u))
            }
            1 => {
                let y = self.fixed_value;
                (Vec3::new(r.min_u, y, r.min_v), Vec3::new(r.max_u, y, r.min_v),
                 Vec3::new(r.max_u, y, r.max_v), Vec3::new(r.min_u, y, r.max_v))
            }
            2 => {
                let z = self.fixed_value;
                (Vec3::new(r.min_u, r.min_v, z), Vec3::new(r.max_u, r.min_v, z),
                 Vec3::new(r.max_u, r.max_v, z), Vec3::new(r.min_u, r.max_v, z))
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Message)]
pub struct CalculateRoomGeometry;

#[derive(Message)]
pub struct ClearRoomGeometry;

#[cfg(test)]
mod tests {
    use super::*;
    
    fn triangle_count(mesh: &Mesh) -> usize {
        match mesh.indices() {
            Some(bevy::mesh::Indices::U32(v)) => v.len() / 3,
            _ => 0,
        }
    }

    #[test]
    fn bake_isolated_room_has_12_tris() {
        let room = Room::new(Vec3::ZERO, Vec3::new(10.0, 3.0, 10.0));
        let mesh = room.bake_faces(&[]);
        // 6 faces * 1 quad each * 2 tris/quad = 12
        assert_eq!(triangle_count(&mesh), 12);
    }

    #[test]
    fn bake_shared_full_wall_removes_both_faces() {
        // Two rooms sharing the x=10 / x=10 wall with identical y/z extents
        let a = Room::new(Vec3::ZERO, Vec3::new(10.0, 3.0, 10.0));
        let b = Room::new(Vec3::new(10.0, 0.0, 0.0), Vec3::new(20.0, 3.0, 10.0));

        let mesh_a = a.bake_faces(&[b.clone()]);
        let mesh_b = b.bake_faces(&[a.clone()]);

        // Each room should have 5 full faces = 10 tris (the shared wall is fully carved)
        assert_eq!(triangle_count(&mesh_a), 10);
        assert_eq!(triangle_count(&mesh_b), 10);
    }

    #[test]
    fn bake_partial_wall_carves_opening() {
        // Room B only covers part of room A's +X face
        let a = Room::new(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0));
        let b = Room::new(Vec3::new(10.0, 2.0, 2.0), Vec3::new(20.0, 8.0, 8.0));

        let mesh_a = a.bake_faces(&[b.clone()]);

        // A 6x6 hole in A's 10x10 +X face. 5 unmodified faces = 10 tris.
        let tris = triangle_count(&mesh_a);
        assert!(tris > 12, "expected more than 12 tris due to carving, got {}", tris);
        // The carved face's frame is at most 4 rectangles = 8 tris: 10 + 8.
        assert!(tris <= 18, "expected at most 18 tris, got {}", tris);
    }

    #[test]
    fn bake_overlap_penetration() {
        // Room B penetrates through Room A's +X face
        let a = Room::new(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0));
        let b = Room::new(Vec3::new(5.0, 3.0, 3.0), Vec3::new(15.0, 7.0, 7.0));

        let mesh_a = a.bake_faces(&[b.clone()]);

        // A's +X face has a 4x4 hole; -X is untouched.
        let tris = triangle_count(&mesh_a);
        assert!(tris > 12, "overlap should produce more tris, got {}", tris);
    }

    #[test]
    fn bake_engulfed_room_keeps_all_faces() {
        // Room B is fully inside Room A -- B's walls should remain
        let a = Room::new(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0));
        let b = Room::new(Vec3::new(3.0, 3.0, 3.0), Vec3::new(7.0, 7.0, 7.0));

        let mesh_b = b.bake_faces(&[a.clone()]);
        assert_eq!(triangle_count(&mesh_b), 12, "engulfed room should keep all 6 faces");

        // A should also be unaffected (B is fully inside, doesn't reach A's faces)
        let mesh_a = a.bake_faces(&[b.clone()]);
        assert_eq!(triangle_count(&mesh_a), 12, "engulfing room should keep all 6 faces");
    }

    fn ray(origin: Vec3, direction: Vec3) -> Ray3d {
        Ray3d::new(origin, Dir3::new(direction).unwrap())
    }

    #[test]
    fn a_pick_from_outside_passes_the_near_wall_and_lands_on_the_far_one() {
        let room = Room::new(Vec3::ZERO, Vec3::splat(10.0));
        let (t, hit) = room.ray_hit_from_inside(&[], &ray(Vec3::new(-5.0, 5.0, 5.0), Vec3::X)).unwrap();
        assert!((t - 15.0).abs() < 1e-4, "{t}");
        assert!((hit.x - 10.0).abs() < 1e-4, "{hit}");
    }

    #[test]
    fn a_pick_from_inside_lands_on_the_wall_in_front() {
        let room = Room::new(Vec3::ZERO, Vec3::splat(10.0));
        let (t, _) = room.ray_hit_from_inside(&[], &ray(Vec3::splat(5.0), Vec3::X)).unwrap();
        assert!((t - 5.0).abs() < 1e-4, "{t}");
    }

    /// Breaks if picking tests the whole box instead of the baked holes.
    #[test]
    fn a_pick_goes_through_an_opening_and_not_through_the_wall_beside_it() {
        let a = Room::new(Vec3::ZERO, Vec3::splat(10.0));
        let b = Room::new(Vec3::new(10.0, 0.0, 2.0), Vec3::new(20.0, 4.0, 4.0));

        let through = ray(Vec3::new(5.0, 2.0, 3.0), Vec3::X);
        assert_eq!(a.ray_hit_from_inside(&[b.clone()], &through), None, "A has no wall in the doorway");
        let (t, _) = b.ray_hit_from_inside(&[a.clone()], &through).unwrap();
        assert!((t - 15.0).abs() < 1e-4, "B's far wall, got {t}");

        let beside = ray(Vec3::new(5.0, 7.0, 7.0), Vec3::X);
        let (t, _) = a.ray_hit_from_inside(&[b.clone()], &beside).unwrap();
        assert!((t - 5.0).abs() < 1e-4, "A's wall beside the doorway, got {t}");
    }

    #[test]
    fn bake_non_overlapping_rooms_unchanged() {
        let a = Room::new(Vec3::ZERO, Vec3::new(5.0, 5.0, 5.0));
        let b = Room::new(Vec3::new(100.0, 100.0, 100.0), Vec3::new(105.0, 105.0, 105.0));

        let mesh_a = a.bake_faces(&[b.clone()]);
        let mesh_b = b.bake_faces(&[a.clone()]);

        assert_eq!(triangle_count(&mesh_a), 12);
        assert_eq!(triangle_count(&mesh_b), 12);
    }
}
