use bevy::app::App;
use bevy::prelude::*;
use crate::editor::editable::{EditorActionId, EditorActions, PointRef};
use crate::editor::editor_room::EditorRoom;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::{CameraAxis, Multicam};
use crate::get;
use crate::tool::Tools;

const PICK_RADIUS: f32 = 0.1;
const DEFAULT_SNAP_GRANULARITY: f32 = 0.1;

pub struct RoomPlugin;

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<RoomTool>()
            .add_systems(Update, (
                RoomTool::interface,
                RoomTool::draw_gizmos,
                RoomTool::draw_room_bounds,
            ).chain().run_if(in_state(Tools::Room)))
            .add_systems(OnExit(Tools::Room), RoomTool::on_exit)
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
    ) {
        let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let shift_just_pressed = keys.just_pressed(KeyCode::ShiftLeft) || keys.just_pressed(KeyCode::ShiftRight);

        let suggestion = tool.suggestion();

        // Cursor computation
        tool.cursor = None;
        if let Some(camera_entity) = mouse_input.in_camera {
            if let Some(world_pos) = mouse_input.world_pos {
                for (entity, multicam) in &cameras {
                    if camera_entity == entity && multicam.axis != CameraAxis::None {
                        let origin = world_pos.origin;
                        let cursor = match multicam.axis {
                            CameraAxis::None => unreachable!(),
                            CameraAxis::X => Vec3::new(suggestion.x, origin.y, origin.z),
                            CameraAxis::Y => Vec3::new(origin.x, suggestion.y, origin.z),
                            CameraAxis::Z => Vec3::new(origin.x, origin.y, suggestion.z),
                        };
                        let cursor = if tool.snap {
                            let g = tool.snap_granularity;
                            Vec3::new(
                                f32::ceil(cursor.x / g) * g,
                                f32::ceil(cursor.y / g) * g,
                                f32::ceil(cursor.z / g) * g,
                            )
                        } else {
                            cursor
                        };
                        tool.cursor = Some(cursor);
                    }
                }
            }
        }

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
                        } else {
                            Self::create_room(&mut tool, &mut actions, pr, cursor);
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

                tool.hovered_point = None;
                if let Some(ray) = mouse_input.world_pos {
                    let mut best_dist = PICK_RADIUS;
                    let mut best: Option<(EditorActionId, String, Vec3)> = None;

                    for (action_id, action) in actions.active_actions() {
                        let points = action.object().reference_points_for_ray(&ray);
                        for (key, pos) in points {
                            let dist = ray_point_distance(&ray, pos);
                            if dist < best_dist {
                                best_dist = dist;
                                best = Some((action_id, key, pos));
                            }
                        }
                    }
                    tool.hovered_point = best;
                }

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
                            tool.mode = RoomToolMode::PlacingMax(RoomCornerMode::Normal);
                        } else {
                            Self::create_room(&mut tool, &mut actions, pr, cursor);
                        }
                    }
                }
            }
        }
    }

    fn create_room(
        tool: &mut ResMut<Self>,
        actions: &mut ResMut<EditorActions>,
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
                Color::srgb_u8(0, 220, 220)
            } else {
                Color::srgb_u8(255, 220, 0)
            };
            gizmos.sphere(Isometry3d::from_translation(cursor), 0.15, color);

            // When placing max, draw the min point and a bounds preview
            if let (RoomToolMode::PlacingMax(_), Some(min_resolved)) = (&tool.mode, tool.min_resolved) {
                let min_color = Color::srgb_u8(100, 255, 100);
                gizmos.sphere(Isometry3d::from_translation(min_resolved), 0.15, min_color);
                let preview_color = Color::srgb_u8(40, 40, 200);
                bounds_gizmo(&mut gizmos, min_resolved, cursor, preview_color);
            }

            // In RelativeSelected mode, draw taxicab dashed path from reference to cursor
            if let RoomCornerMode::RelativeSelected { reference_resolved, .. } = corner_mode {
                let base = *reference_resolved;
                const DASH: f32 = 0.15;
                const GAP: f32 = 0.1;

                let d = cursor - base;
                let segments: [(f32, Vec3, Color); 3] = [
                    (d.x, Vec3::X, Color::srgb_u8(255, 80, 80)),
                    (d.z, Vec3::Z, Color::srgb_u8(80, 80, 255)),
                    (d.y, Vec3::Y, Color::srgb_u8(80, 255, 80)),
                ];

                let mut pos = base;
                for (offset, unit, seg_color) in segments {
                    if offset.abs() < f32::EPSILON { continue; }
                    let next = pos + unit * offset;
                    dashed_line(&mut gizmos, pos, next, seg_color, DASH, GAP);
                    pos = next;
                }
            }
        }

        // In Picking mode, draw all reference point candidates
        if matches!(corner_mode, RoomCornerMode::Picking) {
            if let Some(ray) = mouse_input.world_pos {
                let dim_color = Color::srgba(0.5, 0.5, 0.5, 0.4);
                let highlight_color = Color::srgb_u8(180, 240, 255);

                for (action_id, action) in actions.active_actions() {
                    let points = action.object().reference_points_for_ray(&ray);
                    for (key, pos) in &points {
                        let is_hovered = tool.hovered_point.as_ref()
                            .is_some_and(|(hid, hkey, _)| *hid == action_id && hkey == key);
                        let color = if is_hovered { highlight_color } else { dim_color };
                        gizmos.sphere(Isometry3d::from_translation(*pos), 0.1, color);
                    }
                }
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

fn bounds_gizmo(gizmos: &mut Gizmos, min: Vec3, max: Vec3, color: Color) {
    gizmos.line(Vec3::new(min.x, min.y, min.z), Vec3::new(max.x, min.y, min.z), color);
    gizmos.line(Vec3::new(max.x, min.y, min.z), Vec3::new(max.x, max.y, min.z), color);
    gizmos.line(Vec3::new(max.x, max.y, min.z), Vec3::new(min.x, max.y, min.z), color);
    gizmos.line(Vec3::new(min.x, max.y, min.z), Vec3::new(min.x, min.y, min.z), color);

    gizmos.line(Vec3::new(min.x, min.y, max.z), Vec3::new(max.x, min.y, max.z), color);
    gizmos.line(Vec3::new(max.x, min.y, max.z), Vec3::new(max.x, max.y, max.z), color);
    gizmos.line(Vec3::new(max.x, max.y, max.z), Vec3::new(min.x, max.y, max.z), color);
    gizmos.line(Vec3::new(min.x, max.y, max.z), Vec3::new(min.x, min.y, max.z), color);

    gizmos.line(Vec3::new(min.x, min.y, min.z), Vec3::new(min.x, min.y, max.z), color);
    gizmos.line(Vec3::new(max.x, min.y, min.z), Vec3::new(max.x, min.y, max.z), color);
    gizmos.line(Vec3::new(max.x, max.y, min.z), Vec3::new(max.x, max.y, max.z), color);
    gizmos.line(Vec3::new(min.x, max.y, min.z), Vec3::new(min.x, max.y, max.z), color);
}

fn ray_point_distance(ray: &Ray3d, point: Vec3) -> f32 {
    let to_point = point - ray.origin;
    let dir = Vec3::from(ray.direction);
    let cross = to_point.cross(dir);
    cross.length() / dir.length()
}

fn dashed_line(gizmos: &mut Gizmos, start: Vec3, end: Vec3, color: Color, dash: f32, gap: f32) {
    let dir = end - start;
    let len = dir.length();
    if len < 0.001 { return; }
    let norm = dir / len;
    let mut t = 0.0;
    while t < len {
        let dash_end = (t + dash).min(len);
        gizmos.line(start + norm * t, start + norm * dash_end, color);
        t = dash_end + gap;
    }
}

#[derive(Component)]
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
    
    
    pub fn mesh(&self) -> Mesh {
        let min = self.min;
        let max = self.max;
        
        let vertices = vec![
            [min.x, min.y, min.z], // 0: bottom-left-front
            [max.x, min.y, min.z], // 1: bottom-right-front
            [max.x, max.y, min.z], // 2: top-right-front
            [min.x, max.y, min.z], // 3: top-left-front
            [min.x, min.y, max.z], // 4: bottom-left-back
            [max.x, min.y, max.z], // 5: bottom-right-back
            [max.x, max.y, max.z], // 6: top-right-back
            [min.x, max.y, max.z], // 7: top-left-back
        ];
        
        let indices = vec![
            // Front face (z = min.z) - normal points toward +z (inward)
            0, 1, 2, 0, 2, 3,
            // Back face (z = max.z) - normal points toward -z (inward)
            4, 6, 5, 4, 7, 6,
            // Left face (x = min.x) - normal points toward +x (inward)
            4, 0, 3, 4, 3, 7,
            // Right face (x = max.x) - normal points toward -x (inward)
            1, 5, 2, 5, 6, 2,
            // Bottom face (y = min.y) - normal points toward +y (inward)
            4, 1, 0, 4, 5, 1,
            // Top face (y = max.y) - normal points toward -y (inward)
            3, 2, 6, 3, 6, 7,
        ];
        
        let normals = vec![
            [0.0, 0.0, 1.0], // 0
            [0.0, 0.0, 1.0], // 1
            [0.0, 0.0, 1.0], // 2
            [0.0, 0.0, 1.0], // 3
            [0.0, 0.0, -1.0], // 4
            [0.0, 0.0, -1.0], // 5
            [0.0, 0.0, -1.0], // 6
            [0.0, 0.0, -1.0], // 7
        ];
        
        let uvs = vec![
            [0.0, 0.0], // 0
            [1.0, 0.0], // 1
            [1.0, 1.0], // 2
            [0.0, 1.0], // 3
            [0.0, 0.0], // 4
            [1.0, 0.0], // 5
            [1.0, 1.0], // 6
            [0.0, 1.0], // 7
        ];
        
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

pub enum IntersectionResult {
    None,
    LeftEngulfsRight,
    RightEngulfsLeft,
    Identical,
    Intersection,
}

#[derive(Message)]
pub struct CalculateRoomGeometry;

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
}
