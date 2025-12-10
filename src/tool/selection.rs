use bevy::app::App;
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};
use crate::editor::input::CurrentMouseInput;
use crate::tool::Tools;

pub struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<SelectionState>()
            .add_systems(Update, (
                Self::select.run_if(in_state(Tools::Select)),
                Self::draw_bounds,
            ))
        ;
    }
}

impl SelectionPlugin {
    fn select(
        mut state: ResMut<SelectionState>,
        current_input: Res<CurrentMouseInput>,
        selectables: Query<&EditorSelectable>,
        mut ray_cast: MeshRayCast,
        mut gizmos: Gizmos,
        cursor_options: Single<&CursorOptions, With<PrimaryWindow>>,
    ) {
        if !cursor_options.visible {
            state.hovered = None;
            return;
        }

        let filter = |entity| selectables.get(entity).is_ok();
        let settings = MeshRayCastSettings::default().with_filter(&filter);
        
        if let Some(ray) = current_input.world_pos {
            if let Some((hit_entity, hit_data)) = ray_cast
                .cast_ray(ray, &settings)
                .first() {
                if state.debug_probe {
                    gizmos.line(ray.origin, hit_data.point, Color::srgb_u8(0, 255, 0));
                    gizmos.sphere(Isometry3d::from_translation(hit_data.point), 0.2, Color::srgb_u8(0, 255, 0));
                }
                
                state.hovered = Some(*hit_entity);
                if current_input.released == Some(MouseButton::Left) {
                    state.selected = Some(*hit_entity);
                }
            } else {
                if state.debug_probe {
                    gizmos.line(ray.origin, ray.origin + ray.direction * 100.0, Color::srgb_u8(255, 0, 0));
                }
                state.hovered = None;
                if current_input.released == Some(MouseButton::Left) {
                    state.selected = None;
                }
            }
        } else {
            state.hovered = None;
        }
    }

    fn draw_bounds(
        selectables: Query<(Entity, &Transform, &EditorSelectable)>,
        state: Res<SelectionState>,
        mut gizmos: Gizmos,
    ) {
        let selected_color = Color::srgb_u8(0, 255, 0);
        let hovered_color = Color::srgb_u8(230, 230, 230);
        let same_color = Color::srgb_u8(230, 230, 0);
        for (entity, transform, select) in selectables {
            let same = match (state.selected, state.hovered) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            if let Some(selected) = state.selected {
                let color = if same { same_color } else { selected_color };
                if selected == entity {
                    Self::draw_bounding_box(&mut gizmos, color, transform, select);
                }
            }
            if let Some(hovered) = state.hovered {
                if hovered == entity {
                    Self::draw_bounding_box(&mut gizmos, hovered_color, transform, select);
                }
            }
        }
    }
    
    fn local_to_world(transform: &Transform, point: &Vec3) -> Vec3 {
        transform.transform_point(*point)
    }

    fn draw_bounding_box(gizmos: &mut Gizmos, color: Color, transform: &Transform, select: &EditorSelectable) {
        let a = transform.transform_point(Vec3::ZERO.with_x(select.bounding_box.half_size.x).with_y(select.bounding_box.half_size.y).with_z(select.bounding_box.half_size.z));
        let b = transform.transform_point(Vec3::ZERO.with_x(-select.bounding_box.half_size.x).with_y(select.bounding_box.half_size.y).with_z(select.bounding_box.half_size.z));
        let c = transform.transform_point(Vec3::ZERO.with_x(-select.bounding_box.half_size.x).with_y(-select.bounding_box.half_size.y).with_z(select.bounding_box.half_size.z));
        let d = transform.transform_point(Vec3::ZERO.with_x(select.bounding_box.half_size.x).with_y(-select.bounding_box.half_size.y).with_z(select.bounding_box.half_size.z));

        let e = transform.transform_point(Vec3::ZERO.with_x(select.bounding_box.half_size.x).with_y(select.bounding_box.half_size.y).with_z(-select.bounding_box.half_size.z));
        let f = transform.transform_point(Vec3::ZERO.with_x(-select.bounding_box.half_size.x).with_y(select.bounding_box.half_size.y).with_z(-select.bounding_box.half_size.z));
        let g = transform.transform_point(Vec3::ZERO.with_x(-select.bounding_box.half_size.x).with_y(-select.bounding_box.half_size.y).with_z(-select.bounding_box.half_size.z));
        let h = transform.transform_point(Vec3::ZERO.with_x(select.bounding_box.half_size.x).with_y(-select.bounding_box.half_size.y).with_z(-select.bounding_box.half_size.z));

        gizmos.line(a, b, color);
        gizmos.line(b, c, color);
        gizmos.line(c, d, color);
        gizmos.line(d, a, color);

        gizmos.line(e, f, color);
        gizmos.line(f, g, color);
        gizmos.line(g, h, color);
        gizmos.line(h, e, color);

        gizmos.line(a, e, color);
        gizmos.line(b, f, color);
        gizmos.line(c, g, color);
        gizmos.line(d, h, color);
    }
}

#[derive(Component)]
pub struct EditorSelectable {
    pub id: String,
    pub bounding_box: Cuboid,
}

#[derive(Resource)]
pub struct SelectionState {
    pub hovered: Option<Entity>,
    pub selected: Option<Entity>,
    debug_probe: bool,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            hovered: None,
            selected: None,
            debug_probe: false,
        }
    }
}
