use bevy::app::App;
use bevy::prelude::*;
use crate::editor::editable::{EditEvent, EditorActionId, EditorActions, PointRef};
use crate::editor::editor_room::EditorRoom;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::get;
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
            ).chain().run_if(in_state(Tools::Room)))
            .add_systems(OnExit(Tools::Room), RoomTool::on_exit)
            .add_systems(Update, (
                RoomDragState::spawn_handles_system,
                RoomDragState::handle_dragging,
                RoomDragState::update_handle_positions,
            ).chain().run_if(in_state(Tools::Select)))
            .add_systems(OnExit(Tools::Select), RoomDragState::despawn_handles)
        ;
    }
}

#[derive(Clone)]
enum RoomCornerMode {
    Normal,
    Picking,
    RelativeSelected {
        reference_action: EditorActionId,
        reference_key: String,
        reference_resolved: Vec3,
    },
}

enum RoomToolMode {
    PlacingMin(RoomCornerMode),
    PlacingMax(RoomCornerMode),
}

#[derive(Resource)]
struct RoomTool {
    mode: RoomToolMode,
    last_min: Vec3,
    last_max: Vec3,
    cursor: Option<Vec3>,
    hovered_point: Option<(EditorActionId, String, Vec3)>,
    min_point: Option<PointRef>,
    min_resolved: Option<Vec3>,
    snap: bool,
    snap_granularity: f32,
}

impl Default for RoomTool {
    fn default() -> Self {
        Self {
            mode: RoomToolMode::PlacingMin(RoomCornerMode::Normal),
            last_min: Vec3::new(-1.0, 0.0, -1.0),
            last_max: Vec3::new(1.0, 2.0, 1.0),
            cursor: None,
            hovered_point: None,
            min_point: None,
            min_resolved: None,
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
    }

    fn suggestion(&self) -> Vec3 {
        match &self.mode {
            RoomToolMode::PlacingMin(_) => self.last_min,
            RoomToolMode::PlacingMax(_) => self.last_max,
        }
    }

    fn interface(
        mut tool: ResMut<Self>,
        cameras: Query<(Entity, &Multicam)>,
        mouse_input: Res<CurrentMouseInput>,
        keys: Res<ButtonInput<KeyCode>>,
        mut actions: ResMut<EditorActions>,
        rooms: Query<&Room>,
        mut next_tool: ResMut<NextState<Tools>>,
    ) {
        let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let shift_just_pressed = keys.just_pressed(KeyCode::ShiftLeft) || keys.just_pressed(KeyCode::ShiftRight);

        let suggestion = tool.suggestion();

        tool.cursor = compute_cursor(
            &mouse_input, &cameras, suggestion,
            tool.snap, tool.snap_granularity, &rooms,
        );

        // ESC while placing max: discard min and reset to PlacingMin(Normal)
        if matches!(tool.mode, RoomToolMode::PlacingMax(_))
            && keys.just_pressed(KeyCode::Escape)
        {
            tool.mode = RoomToolMode::PlacingMin(RoomCornerMode::Normal);
            tool.min_point = None;
            tool.min_resolved = None;
            tool.hovered_point = None;
            return;
        }

        // Extract the corner mode (cloned to avoid holding a borrow on tool.mode)
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
                } else if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        let pr = PointRef::absolute(cursor.x, cursor.y, cursor.z);
                        if is_placing_min {
                            tool.min_point = Some(pr);
                            tool.min_resolved = Some(cursor);
                            tool.last_min = cursor;
                            tool.mode = RoomToolMode::PlacingMax(RoomCornerMode::Normal);
                            tool.hovered_point = None;
                        } else {
                            Self::create_room(&mut tool, &mut actions, &mut next_tool, pr, cursor);
                        }
                    }
                }
            }
            RoomCornerMode::Picking => {
                if !shift_held {
                    set_mode(&mut tool, is_placing_min, RoomCornerMode::Normal);
                    tool.hovered_point = None;
                    return;
                }

                tool.hovered_point = mouse_input.world_pos
                    .and_then(|ray| find_hovered_point(&ray, &actions, PICK_RADIUS));

                if mouse_input.released == Some(MouseButton::Left) {
                    if let Some((action_id, key, resolved)) = tool.hovered_point.take() {
                        set_mode(&mut tool, is_placing_min, RoomCornerMode::RelativeSelected {
                            reference_action: action_id,
                            reference_key: key,
                            reference_resolved: resolved,
                        });
                    }
                }
            }
            RoomCornerMode::RelativeSelected { reference_action, reference_key, reference_resolved } => {
                if shift_just_pressed {
                    set_mode(&mut tool, is_placing_min, RoomCornerMode::Normal);
                    return;
                }

                if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        let d = cursor - reference_resolved;
                        let mut pr = PointRef::reference_with_offset(reference_action, d.x, d.y, d.z);
                        if !reference_key.is_empty() {
                            pr.point_key = reference_key.clone();
                        }
                        if is_placing_min {
                            tool.min_point = Some(pr);
                            tool.min_resolved = Some(cursor);
                            tool.last_min = cursor;
                            tool.mode = RoomToolMode::PlacingMax(RoomCornerMode::RelativeSelected {
                                reference_action,
                                reference_key: reference_key.clone(),
                                reference_resolved,
                            });
                        } else {
                            Self::create_room(&mut tool, &mut actions, &mut next_tool, pr, cursor);
                        }
                    }
                }
            }
        }
    }

    fn create_room(
        tool: &mut ResMut<Self>,
        actions: &mut ResMut<EditorActions>,
        next_tool: &mut ResMut<NextState<Tools>>,
        max_point: PointRef,
        max_resolved: Vec3,
    ) {
        if let Some(min_point) = tool.min_point.take() {
            let room = EditorRoom::from_point_refs(min_point, max_point);
            let id = actions.take_action(Box::new(room));
            actions.select(Some(id));
            tool.last_max = max_resolved;
            tool.min_resolved = None;
            tool.mode = RoomToolMode::PlacingMin(RoomCornerMode::Normal);
            next_tool.set(Tools::Select);
        }
    }

    fn draw_gizmos(
        tool: Res<RoomTool>,
        actions: Res<EditorActions>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        let corner_mode = match &tool.mode {
            RoomToolMode::PlacingMin(cm) | RoomToolMode::PlacingMax(cm) => cm,
        };
        let is_relative = matches!(corner_mode, RoomCornerMode::RelativeSelected { .. });

        // Preview sphere at cursor
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
            }
        }

        if matches!(corner_mode, RoomCornerMode::Picking) {
            if let Some(ray) = mouse_input.world_pos {
                draw_picking_gizmos(&mut gizmos, &ray, &actions, &tool.hovered_point);
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

// --- Drag Handles ---

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
    tracked_action: Option<EditorActionId>,
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
        actions: Res<EditorActions>,
        handles: Query<Entity, With<RoomDragHandle>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        let current_action = actions.selected_action();

        let bounds = current_action
            .and_then(|id| actions.get_action(&id))
            .and_then(|a| a.object().drag_handle_bounds());

        let should_track = current_action.filter(|_| bounds.is_some());

        if state.tracked_action != should_track {
            for entity in &handles {
                commands.entity(entity).despawn();
            }
            state.tracked_action = should_track;
            state.grabbed_handle = None;
            state.grab_offset = None;

            if let (Some(_action_id), Some((min, max))) = (should_track, bounds) {
                state.ensure_assets(&mut meshes, &mut materials);
                let mesh = state.handle_mesh.clone().unwrap();
                let mat = state.idle_material.clone().unwrap();

                for (center, axis) in HandleAxis::face_centers(min, max) {
                    commands.spawn((
                        RoomDragHandle { axis },
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(mat.clone()),
                        Transform::from_translation(center),
                    ));
                }
            }
        }
    }

    fn update_handle_positions(
        actions: Res<EditorActions>,
        state: Res<RoomDragState>,
        mut handles: Query<(&RoomDragHandle, &mut Transform)>,
    ) {
        let Some(action_id) = state.tracked_action else { return; };
        let Some(action) = actions.get_action(&action_id) else { return; };
        let Some((min, max)) = action.object().drag_handle_bounds() else { return; };

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
        handles: Query<(Entity, &RoomDragHandle)>,
        mut ray_cast: MeshRayCast,
        mouse_input: Res<CurrentMouseInput>,
        mut commands: Commands,
        mut state: ResMut<Self>,
        mut actions: ResMut<EditorActions>,
        mut edit_events: MessageWriter<EditEvent>,
    ) {
        let Some(action_id) = state.tracked_action else { return; };

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
            Self::restore_all_materials(&handles, &idle, &mut commands);
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

            let Some((current_min, current_max)) = actions.get_action(&action_id)
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

            if let Some(mut action) = actions.actions_mut().remove(&action_id) {
                let modified = action.object_mut().drag_handle(
                    handle_axis.is_max(),
                    axis_index,
                    new_value,
                );
                if modified {
                    if let Some(entity) = action.object().entity() {
                        action.object_mut().apply_to_entity(&mut commands, entity);
                        edit_events.write(EditEvent {
                            editor_id: action_id._id(),
                            action_id,
                            entity,
                        });
                    }
                }
                actions.actions_mut().insert(action_id, action);
            }
        } else {
            let filter = |entity: Entity| handles.get(entity).is_ok();
            let settings = MeshRayCastSettings::default().with_filter(&filter);

            if let Some((hit_entity, _)) = ray_cast.cast_ray(ray, &settings).first() {
                if let Ok((_, handle)) = handles.get(*hit_entity) {
                    commands.entity(*hit_entity)
                        .remove::<MeshMaterial3d<StandardMaterial>>()
                        .insert(MeshMaterial3d(highlight.clone()));
                    for (entity, _) in &handles {
                        if entity != *hit_entity {
                            commands.entity(entity)
                                .remove::<MeshMaterial3d<StandardMaterial>>()
                                .insert(MeshMaterial3d(idle.clone()));
                        }
                    }

                    if mouse_just_pressed {
                        state.grabbed_handle = Some(handle.axis);
                        state.grab_offset = None;
                    }
                }
            } else {
                Self::restore_all_materials(&handles, &idle, &mut commands);
            }
        }
    }

    fn restore_all_materials(
        handles: &Query<(Entity, &RoomDragHandle)>,
        idle: &Handle<StandardMaterial>,
        commands: &mut Commands,
    ) {
        for (entity, _) in handles.iter() {
            commands.entity(entity)
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .insert(MeshMaterial3d(idle.clone()));
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
        state.tracked_action = None;
        state.grabbed_handle = None;
        state.grab_offset = None;
    }
}

#[derive(Component, Clone, Debug)]
pub struct Room {
    pub min: Vec3,
    pub max: Vec3,
    ghost: Option<Entity>,
}

impl Default for Room {
    fn default() -> Self {
        Self::new(Vec3::ZERO, Vec3::ONE)
    }
}

impl Room {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self {
            min,
            max,
            ghost: None,
        }
    }

    /// Bake this room's wall geometry, carving openings where other rooms
    /// overlap or share walls. Returns a single Mesh with inward-facing normals.
    pub fn bake_faces(&self, others: &[Room]) -> Mesh {
        use crate::common::rect_subtract::{Rect2D, subtract_rects};

        let mut vertices: Vec<[f32; 3]> = Vec::new();
        let mut normals: Vec<[f32; 3]> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();

        for face in RoomFace::enumerate(self) {
            let mut holes = Vec::new();
            for other in others {
                if let Some(clip) = face.clip_rect(other) {
                    holes.push(clip);
                }
            }

            let solid_rects = subtract_rects(&face.rect, &holes);

            for rect in &solid_rects {
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
        }

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
    
    pub fn messages(&self, my_entity: Entity) -> Vec<String> {
        let mut messages = Vec::new();
        if let Some(entity) = self.ghost {
            messages.push(get!("room.messages.ghost", "me", my_entity, "other", entity));
        }
        messages
    }
    
    pub fn point_inside(&self, point: Vec3) -> bool {
        point.x >= self.min.x && point.x <= self.max.x
        && point.y >= self.min.y && point.y <= self.max.y
        && point.z >= self.min.z && point.z <= self.max.z
    }
    
    pub fn extremes(&self) -> Vec<Vec3> {
        let mut extremes = Vec::with_capacity(8);
        
        extremes.push(Vec3::new(self.min.x, self.min.y, self.min.z));
        extremes.push(Vec3::new(self.max.x, self.min.y, self.min.z));
        extremes.push(Vec3::new(self.max.x, self.max.y, self.min.z));
        extremes.push(Vec3::new(self.min.x, self.max.y, self.min.z));

        extremes.push(Vec3::new(self.min.x, self.min.y, self.max.z));
        extremes.push(Vec3::new(self.max.x, self.min.y, self.max.z));
        extremes.push(Vec3::new(self.max.x, self.max.y, self.max.z));
        extremes.push(Vec3::new(self.min.x, self.max.y, self.max.z));
        
        extremes
    }

    pub fn count_points_inside(&self, points: &Vec<Vec3>) -> usize {
        points.iter().map(|p| self.point_inside(p.clone()) as usize).sum() 
    }
    
    pub fn test_intersection(left: &Self, right: &Self) -> IntersectionResult {
        let engulfed_right_points = left.count_points_inside(&right.extremes());
        let engulfed_left_points = right.count_points_inside(&left.extremes());
        if engulfed_right_points == 0 || engulfed_left_points == 0 {
            return IntersectionResult::None
        }
        if engulfed_right_points == 8 && engulfed_left_points == 8 {
            return IntersectionResult::Identical
        }
        if engulfed_right_points == 8 {
            return IntersectionResult::LeftEngulfsRight
        }
        if engulfed_left_points == 8 {
            return IntersectionResult::RightEngulfsLeft
        }
        IntersectionResult::Intersection
    }
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

        // The other room must cross through this face from the exterior side.
        //
        // For a max face (e.g., +X at F, interior toward opposite_value < F):
        //   Shared wall:  other starts at the face and extends outward (other_min == F, other_max > F)
        //   Penetration:  other straddles the face (other_min < F < other_max)
        //   Reject if:    other fully contains the room on this axis
        //                 (other_min <= opposite_value AND other_max >= F)
        //                 because that means the room is engulfed, not connected.
        //
        // For a min face (e.g., -X at F, interior toward opposite_value > F):
        //   Shared wall:  other_max == F, other_min < F
        //   Penetration:  other_min < F < other_max
        //   Same engulfment rejection.

        let straddles = if self.is_max {
            other_min_fixed <= self.fixed_value && other_max_fixed > self.fixed_value
        } else {
            other_min_fixed < self.fixed_value && other_max_fixed >= self.fixed_value
        };

        if !straddles {
            return None;
        }

        // Reject engulfment: other room fully contains this room on the fixed axis
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

pub enum IntersectionResult {
    None,
    LeftEngulfsRight,
    RightEngulfsLeft,
    Identical,
    Intersection,
}

#[derive(Message)]
pub struct CalculateRoomGeometry;

#[derive(Message)]
pub struct ClearRoomGeometry;

#[cfg(test)]
mod tests {
    use bevy::ecs::relationship::RelationshipSourceCollection;
    use bevy::prelude::*;
    use super::*;
    
    #[test]
    fn test_no_messages() {
        let a = Entity::from_bits(23);
        let _b = Entity::from_bits(45);

        let good_room = Room::default();
        let no_messages = good_room.messages(a);
        assert_eq!(no_messages.len(), 0);
    }

    #[test]
    fn test_ghost_message() {
        let a = Entity::from_bits(23);
        let b = Entity::from_bits(45);
        
        let mut ghost_room = Room::default();
        ghost_room.ghost = Some(b);
        let ghost_message = ghost_room.messages(a);
        assert_eq!(ghost_message.len(), 1);
        assert_eq!(ghost_message[0], "Room 23v1 is fully inside 45v1 and will not appear!");
    }
    
    #[test]
    fn test_point_inside() {
        let room = Room::new(Vec3::ZERO, Vec3::ONE);
        
        assert!(room.point_inside(Vec3::ZERO));
        assert!(room.point_inside(Vec3::ONE));
        assert!(room.point_inside(Vec3::new(0.5, 0.5, 0.5)));
        assert!(room.point_inside(Vec3::new(0.5, 1.0, 0.5)));
        
        assert!(!room.point_inside(Vec3::new(0.5, 1.1, 0.5)));
        assert!(!room.point_inside(Vec3::new(1.1, 0.5, 0.5)));
        assert!(!room.point_inside(Vec3::new(0.5, 0.5, 1.1)));
        assert!(!room.point_inside(Vec3::new(0.5, -1.1, 0.5)));
        assert!(!room.point_inside(Vec3::new(-1.1, 0.5, 0.5)));
        assert!(!room.point_inside(Vec3::new(0.5, 0.5, -1.1)));
    }

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

        // A's +X face had a 6x6 hole carved in a 10x10 face.
        // The remaining area produces multiple quads, so more than 12 tris total.
        // 5 unmodified faces = 10 tris. The carved face should have at least 2 tris (1 quad).
        let tris = triangle_count(&mesh_a);
        assert!(tris > 12, "expected more than 12 tris due to carving, got {}", tris);
        // But should have fewer tris than 12 + 8 (4 border quads from the frame)
        // since the greedy merge keeps it efficient.
        // The carved face has a frame: up to 4 rectangles = 8 tris.
        // Total: 10 + 8 = 18 max for optimal merge.
        assert!(tris <= 18, "expected at most 18 tris, got {}", tris);
    }

    #[test]
    fn bake_overlap_penetration() {
        // Room B penetrates through Room A's +X face
        let a = Room::new(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0));
        let b = Room::new(Vec3::new(5.0, 3.0, 3.0), Vec3::new(15.0, 7.0, 7.0));

        let mesh_a = a.bake_faces(&[b.clone()]);

        // A's +X face has a 4x4 hole, producing a frame.
        // A's -X face is not touched (b doesn't reach x=0).
        // A's +Y, -Y, +Z, -Z faces: b penetrates into their planes at various points.
        // Total tris should be more than the isolated 12.
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
