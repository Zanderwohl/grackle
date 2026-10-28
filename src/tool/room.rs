use bevy::app::App;
use bevy::prelude::*;
use crate::common::csg::{self, Polygon};
use crate::common::triangulate::triangulate;
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
            && !egui.wants_keyboard_input()
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
            for (a, b) in room.edges() {
                gizmos.line(a, b, color);
            }
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

/// Which of a room's materials a surface takes. Decided by its inward normal
/// rather than by which side of the room it is, so a room of any shape sorts
/// its surfaces the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RoomSlot {
    Floor,
    Walls,
    Ceiling,
}

impl RoomSlot {
    pub const ALL: [RoomSlot; 3] = [RoomSlot::Floor, RoomSlot::Walls, RoomSlot::Ceiling];

    pub fn name_key(self) -> &'static str {
        match self {
            RoomSlot::Floor => "editor.features.room.floor",
            RoomSlot::Walls => "editor.features.room.walls",
            RoomSlot::Ceiling => "editor.features.room.ceiling",
        }
    }

    pub fn of(inward: Vec3) -> Self {
        if inward.y > 0.5 {
            RoomSlot::Floor
        } else if inward.y < -0.5 {
            RoomSlot::Ceiling
        } else {
            RoomSlot::Walls
        }
    }
}

/// The material each of a room's slots takes, by name under a pack's
/// `materials/`, or a role the map's palette resolves. `None` is the plain
/// default, which is also what a name with no file behind it draws as. Copied
/// onto the room's entity from its feature.
#[derive(Component, serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct RoomSurfaces {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walls: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling: Option<String>,
}

impl RoomSurfaces {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn of(&self, slot: RoomSlot) -> Option<&str> {
        self.slot(slot).as_deref()
    }

    pub fn slot(&self, slot: RoomSlot) -> &Option<String> {
        match slot {
            RoomSlot::Floor => &self.floor,
            RoomSlot::Walls => &self.walls,
            RoomSlot::Ceiling => &self.ceiling,
        }
    }

    pub fn slot_mut(&mut self, slot: RoomSlot) -> &mut Option<String> {
        match slot {
            RoomSlot::Floor => &mut self.floor,
            RoomSlot::Walls => &mut self.walls,
            RoomSlot::Ceiling => &mut self.ceiling,
        }
    }
}

/// Meters one repeat of a material covers. A stand-in: the scale belongs to
/// the material, which has nowhere to say it yet.
pub const MATERIAL_TILE_METERS: f32 = 2.0;


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

/// A volume of air: a floor plan stood between two heights. A box is the
/// four-corner case. The map is the union of these
/// ([`crate::common::world_surface`]).
#[derive(Component, Clone, Debug, PartialEq)]
pub struct Room {
    /// Meters along `x` and `z`, counter-clockwise by `perp_dot` (which is
    /// clockwise seen from above). Need not be convex. Private so the
    /// winding cannot be got wrong from outside.
    footprint: Vec<Vec2>,
    /// Loops inside the footprint that are not room — a pillar — wound the
    /// same way as it.
    holes: Vec<Vec<Vec2>>,
    pub floor: f32,
    pub ceiling: f32,
}

impl Default for Room {
    fn default() -> Self {
        Self::new(Vec3::ZERO, Vec3::ONE)
    }
}

impl Room {
    /// A box, whichever way round the corners are given.
    pub fn new(a: Vec3, b: Vec3) -> Self {
        let (min, max) = (a.min(b), a.max(b));
        Self::prism(
            vec![Vec2::new(min.x, min.z), Vec2::new(max.x, min.z), Vec2::new(max.x, max.z), Vec2::new(min.x, max.z)],
            min.y,
            max.y,
        )
    }

    /// Any floor plan, wound either way round.
    pub fn prism(footprint: Vec<Vec2>, floor: f32, ceiling: f32) -> Self {
        Self::with_holes(footprint, Vec::new(), floor, ceiling)
    }

    /// A floor plan with loops inside it taken out, each wound either way.
    pub fn with_holes(footprint: Vec<Vec2>, holes: Vec<Vec<Vec2>>, floor: f32, ceiling: f32) -> Self {
        let wound = |mut points: Vec<Vec2>| {
            if signed_area_x2(&points) < 0.0 {
                points.reverse();
            }
            points
        };
        Self {
            footprint: wound(footprint),
            holes: holes.into_iter().map(wound).collect(),
            floor: floor.min(ceiling),
            ceiling: floor.max(ceiling),
        }
    }

    pub fn translated(&self, by: Vec3) -> Room {
        let moved = |points: &Vec<Vec2>| points.iter().map(|p| *p + Vec2::new(by.x, by.z)).collect();
        Room {
            footprint: moved(&self.footprint),
            holes: self.holes.iter().map(moved).collect(),
            floor: self.floor + by.y,
            ceiling: self.ceiling + by.y,
        }
    }

    pub fn footprint(&self) -> &[Vec2] {
        &self.footprint
    }

    pub fn holes(&self) -> &[Vec<Vec2>] {
        &self.holes
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        let (low, high) = self.footprint.iter().fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        (Vec3::new(low.x, self.floor, low.y), Vec3::new(high.x, self.ceiling, high.y))
    }

    pub fn volume(&self) -> f32 {
        let holes: f32 = self.holes.iter().map(|hole| signed_area_x2(hole)).sum();
        (signed_area_x2(&self.footprint) - holes) / 2.0 * (self.ceiling - self.floor)
    }

    /// Inclusive: a point on a wall, a pillar's included, is in the room.
    pub fn contains(&self, point: Vec3) -> bool {
        let q = Vec2::new(point.x, point.z);
        point.y >= self.floor
            && point.y <= self.ceiling
            && in_footprint(&self.footprint, q)
            && !self.holes.iter().any(|hole| in_footprint(hole, q) && !on_edge(hole, q))
    }

    /// A point on the floor inside the plan: the middle of its bounds if that
    /// is in it, otherwise the middle of the biggest triangle of the outline
    /// whose middle is not in a hole.
    pub fn floor_point(&self) -> Vec3 {
        let (min, max) = self.bounds();
        let middle = Vec3::new((min.x + max.x) / 2.0, self.floor, (min.z + max.z) / 2.0);
        if self.contains(middle) {
            return middle;
        }
        let p = &self.footprint;
        let mut triangles = triangulate(p);
        triangles.sort_by(|a, b| triangle_area(p, b).total_cmp(&triangle_area(p, a)));
        triangles
            .into_iter()
            .map(|[a, b, c]| (p[a] + p[b] + p[c]) / 3.0)
            .map(|center| Vec3::new(center.x, self.floor, center.y))
            .find(|point| self.contains(*point))
            .unwrap_or(middle)
    }

    /// The edges round the floor and the ceiling and the uprights between,
    /// for drawing it.
    pub fn edges(&self) -> impl Iterator<Item = (Vec3, Vec3)> + '_ {
        let at = |p: Vec2, y: f32| Vec3::new(p.x, y, p.y);
        std::iter::once(&self.footprint).chain(&self.holes).flat_map(move |points| {
            let count = points.len();
            (0..count).flat_map(move |i| {
                let (a, b) = (points[i], points[(i + 1) % count]);
                [
                    (at(a, self.floor), at(b, self.floor)),
                    (at(a, self.ceiling), at(b, self.ceiling)),
                    (at(a, self.floor), at(a, self.ceiling)),
                ]
            })
        })
    }

    /// The volume as a closed solid, faces out. A convex plan's floor and
    /// ceiling are one face each, so a box is six; a notched one's are
    /// triangles, since the kernel's faces must be convex. Each hole is cut
    /// out by its own prism, reaching a little past the floor and ceiling so
    /// no cap of the cutter is coplanar with the room's — coplanar faces are
    /// where a BSP boolean is least sure which side it is on.
    pub fn solid<S: Copy>(&self, surface: S) -> Vec<Polygon<S>> {
        let outer = prism_faces(&self.footprint, self.floor, self.ceiling, surface);
        let over = (self.ceiling - self.floor) * 0.01 + 1.0e-4;
        self.holes.iter().fold(outer, |solid, hole| {
            csg::subtract(&solid, &prism_faces(hole, self.floor - over, self.ceiling + over, surface))
        })
    }
}

/// A closed prism over `p`, which is wound as [`Room::footprint`] is.
fn prism_faces<S: Copy>(p: &[Vec2], floor: f32, ceiling: f32, surface: S) -> Vec<Polygon<S>> {
    let at = |q: Vec2, y: f32| Vec3::new(q.x, y, q.y);
    let mut faces = Vec::new();
    let caps: Vec<Vec<usize>> = if is_convex(p) {
        vec![(0..p.len()).collect()]
    } else {
        triangulate(p).into_iter().map(|t| t.to_vec()).collect()
    };
    for cap in &caps {
        // Counter-clockwise in `perp_dot` faces down.
        faces.extend(Polygon::new(cap.iter().map(|i| at(p[*i], floor)).collect(), surface));
        faces.extend(Polygon::new(cap.iter().rev().map(|i| at(p[*i], ceiling)).collect(), surface));
    }
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        faces.extend(Polygon::new(
            vec![at(a, floor), at(a, ceiling), at(b, ceiling), at(b, floor)],
            surface,
        ));
    }
    faces
}

fn signed_area_x2(points: &[Vec2]) -> f32 {
    (0..points.len()).map(|i| points[i].perp_dot(points[(i + 1) % points.len()])).sum()
}

fn triangle_area(points: &[Vec2], [a, b, c]: &[usize; 3]) -> f32 {
    (points[*b] - points[*a]).perp_dot(points[*c] - points[*a])
}

fn is_convex(points: &[Vec2]) -> bool {
    let count = points.len();
    (0..count).all(|i| {
        let (a, b, c) = (points[i], points[(i + 1) % count], points[(i + 2) % count]);
        (b - a).perp_dot(c - b) >= 0.0
    })
}

/// Even-odd, with the boundary counted in.
fn in_footprint(points: &[Vec2], q: Vec2) -> bool {
    if on_edge(points, q) {
        return true;
    }
    let count = points.len();
    let mut inside = false;
    for i in 0..count {
        let (a, b) = (points[i], points[(i + 1) % count]);
        if (a.y > q.y) != (b.y > q.y) && q.x < a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

fn on_edge(points: &[Vec2], q: Vec2) -> bool {
    const ON_THE_EDGE: f32 = 1.0e-5;
    let count = points.len();
    (0..count).any(|i| {
        let (a, b) = (points[i], points[(i + 1) % count]);
        let along = b - a;
        let t = (q - a).dot(along) / along.length_squared().max(f32::MIN_POSITIVE);
        (0.0..=1.0).contains(&t) && (a + along * t).distance(q) <= ON_THE_EDGE
    })
}

#[derive(Message)]
pub struct CalculateRoomGeometry;

#[derive(Message)]
pub struct ClearRoomGeometry;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::csg::signed_volume_x6;

    /// An L, 3 × 3 with the 2 × 2 corner out of it, two meters high.
    fn an_l() -> Room {
        Room::prism(
            vec![Vec2::new(0.0, 0.0), Vec2::new(3.0, 0.0), Vec2::new(3.0, 1.0), Vec2::new(1.0, 1.0), Vec2::new(1.0, 3.0), Vec2::new(0.0, 3.0)],
            0.0,
            2.0,
        )
    }

    #[test]
    fn a_box_is_a_four_cornered_prism() {
        let room = Room::new(Vec3::new(4.0, 3.0, 2.0), Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(room.bounds(), (Vec3::new(-1.0, 0.0, 0.0), Vec3::new(4.0, 3.0, 2.0)));
        assert!((room.volume() - 30.0).abs() < 1e-5);
        assert_eq!(room.solid(()).len(), 6);
    }

    #[test]
    fn a_plan_wound_the_other_way_round_is_the_same_room() {
        let mut backwards: Vec<Vec2> = an_l().footprint().to_vec();
        backwards.reverse();
        assert_eq!(Room::prism(backwards, 0.0, 2.0).volume(), an_l().volume());
    }

    #[test]
    fn a_notched_room_is_closed_faces_out_and_holds_what_its_plan_says() {
        let room = an_l();
        assert!((room.volume() - 10.0).abs() < 1e-5);
        assert!((signed_volume_x6(&room.solid(())) / 6.0 - 10.0).abs() < 1e-4, "inside out or open");
    }

    #[test]
    fn the_notch_of_an_l_is_not_in_it() {
        let room = an_l();
        assert!(room.contains(Vec3::new(0.5, 1.0, 2.5)));
        assert!(!room.contains(Vec3::new(2.0, 1.0, 2.0)), "the notch counted as room");
        assert!(room.contains(Vec3::new(3.0, 1.0, 0.5)), "a point on the wall is in the room");
        assert!(!room.contains(Vec3::new(0.5, 2.5, 0.5)), "above the ceiling counted as room");
    }

    /// Four by four with a one-meter pillar in the middle, three high.
    fn a_room_with_a_pillar() -> Room {
        let square = |lo: f32, hi: f32| vec![Vec2::new(lo, lo), Vec2::new(hi, lo), Vec2::new(hi, hi), Vec2::new(lo, hi)];
        Room::with_holes(square(0.0, 4.0), vec![square(1.5, 2.5)], 0.0, 3.0)
    }

    #[test]
    fn a_pillar_is_not_room_but_its_face_is() {
        let room = a_room_with_a_pillar();
        assert!((room.volume() - 45.0).abs() < 1e-4);
        assert!(!room.contains(Vec3::new(2.0, 1.0, 2.0)), "inside the pillar counted as room");
        assert!(room.contains(Vec3::new(1.5, 1.0, 2.0)), "the pillar's face is not in the room");
        assert!(room.contains(Vec3::new(0.5, 1.0, 0.5)));
    }

    #[test]
    fn a_room_with_a_pillar_is_closed_faces_out() {
        let room = a_room_with_a_pillar();
        assert!((signed_volume_x6(&room.solid(())) / 6.0 - 45.0).abs() < 1e-3, "inside out, open or uncut");
    }

    /// The middle of its bounds is inside the pillar.
    #[test]
    fn a_floor_point_is_not_inside_a_pillar() {
        let room = a_room_with_a_pillar();
        let point = room.floor_point();
        let in_the_pillar = (1.5..=2.5).contains(&point.x) && (1.5..=2.5).contains(&point.z);
        assert!(room.contains(point) && !in_the_pillar, "{point}");
    }

    /// The middle of an L's bounds is in its notch.
    #[test]
    fn a_floor_point_is_on_the_floor_inside_the_plan() {
        let room = an_l();
        let point = room.floor_point();
        assert_eq!(point.y, 0.0);
        assert!(room.contains(point), "{point}");
        assert_eq!(Room::new(Vec3::ZERO, Vec3::new(4.0, 1.0, 2.0)).floor_point(), Vec3::new(2.0, 0.0, 1.0));
    }
}
