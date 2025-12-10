use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::EguiContexts;
use std::fmt::Display;
use std::panic::Location;
use bevy::input::mouse::MouseMotion;
use bevy::picking::pointer::{PointerId, PointerLocation};
use bevy::tasks::futures_lite::StreamExt;
use crate::editor::multicam::Multicam;

pub struct EditorInputPlugin;

impl Plugin for EditorInputPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<CurrentMouseInput>()
            .init_resource::<CurrentKeyboardInput>()
            .add_systems(PreUpdate, (
                Self::mouse_input,
                Self::keyboard_input,
            ))
        ;
    }
}

#[derive(Resource)]
pub struct CurrentMouseInput {
    pub pressed: Option<MouseButton>,
    pub just_pressed: bool,
    pub released: Option<MouseButton>,
    pub in_camera: Option<Entity>,
    pub started_in_camera: Option<Entity>,
    pub local_pos: Option<Vec2>,
    pub delta_pos: Vec2,
    pub normalized_pos: Option<Vec2>,
    pub global_pos: Option<Vec2>,
    pub world_pos: Option<Ray3d>,
}

impl Default for CurrentMouseInput {
    fn default() -> Self {
        Self {
            pressed: None,
            just_pressed: false,
            released: None,
            in_camera: None,
            started_in_camera: None,
            local_pos: None,
            delta_pos: Vec2::ZERO,
            normalized_pos: None,
            global_pos: None,
            world_pos: None,
        }
    }
}

#[derive(Resource, Default)]
pub struct CurrentKeyboardInput {
    pub modify: bool,
    pub confirm: bool,
    pub cancel: bool,
    forward: bool,
    left: bool,
    right: bool,
    backward: bool,
    up: bool,
    down: bool,
}

impl CurrentKeyboardInput {
    pub fn forward(&self) -> f32 {
        if self.forward && !self.backward {
            1.0
        } else if self.backward && !self.forward {
            -1.0
        } else {
            0.0
        }
    }
    
    pub fn left(&self) -> f32 {
        if self.left && !self.right {
            1.0
        } else if self.right && !self.left {
            -1.0
        } else {
            0.0
        }
    }
    
    pub fn right(&self) -> f32 {
        if self.right && !self.left {
            1.0
        } else if self.left && !self.right {
            -1.0
        } else {
            0.0
        }
    }
    
    pub fn backward(&self) -> f32 {
        if self.backward && !self.forward {
            1.0
        } else if self.forward && !self.backward {
            -1.0
        } else {
            0.0
        }
    }
    
    pub fn up(&self) -> f32 {
        if self.up && !self.down {
            1.0
        } else if self.down && !self.up {
            -1.0
        } else {
            0.0
        }
    }
    
    pub fn down(&self) -> f32 {
        if self.down && !self.up {
            1.0
        } else if self.up && !self.down {
            -1.0
        } else {
            0.0
        }
    }

    pub fn update(&mut self, keys: &Res<ButtonInput<KeyCode>>) {
        self.forward = keys.pressed(KeyCode::KeyW);
        self.left = keys.pressed(KeyCode::KeyA);
        self.right = keys.pressed(KeyCode::KeyD);
        self.backward = keys.pressed(KeyCode::KeyS);
        self.up = keys.pressed(KeyCode::KeyQ);
        self.down = keys.pressed(KeyCode::KeyE);
    }
}

impl Display for CurrentMouseInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "local: {:?}, normalized: {:?}, global: {:?}", self.local_pos, self.normalized_pos, self.global_pos)
    }
}

impl EditorInputPlugin {
    fn mouse_input(
        mut egui_contexts: EguiContexts,
        primary_window_entity: Query<Entity, With<PrimaryWindow>>,
        window: Single<&Window, With<PrimaryWindow>>,
        mouse_buttons: Res<ButtonInput<MouseButton>>,
        mut current_input: ResMut<CurrentMouseInput>,
        cameras: Query<(Entity, &Camera, &GlobalTransform, &Multicam)>,
        pointers: Query<(&PointerId, &PointerLocation)>,
        mut evr_motion: MessageReader<MouseMotion>,
    ) {
        // We don't want to grab mouse input while over egui windows or panels.
        let ctx = egui_contexts.ctx_mut();
        if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return; }
        let ctx = ctx.unwrap();
        if ctx.is_pointer_over_area() || ctx.wants_pointer_input() {
            return;
        }

        let (just_pressed, pressed, released) = mouse_precedence(mouse_buttons);
        current_input.just_pressed = just_pressed;
        current_input.pressed = pressed;
        current_input.released = released;

        let mut locations = Vec::new();
        for (_, pointer) in pointers {
            for (camera_entity, camera, camera_transform, cam_multicam) in &cameras {
                if let Some(pointer_loc) = pointer.location() {
                    if pointer_loc.is_in_viewport(camera, &primary_window_entity) {
                        if pressed.is_some() {
                            if just_pressed {
                                current_input.started_in_camera = Some(camera_entity);
                            }
                        } else {
                            current_input.started_in_camera = None;
                        }
                        let (position, normalized, ray) = match &camera.viewport {
                            Some(viewport) => {
                                let pos = pointer_loc.position - viewport.physical_position.as_vec2();
                                let normalized = pos / viewport.physical_size.as_vec2();
                                let ray = make_ray(&primary_window_entity, camera, camera_transform, &pointer);
                                (pos, normalized, Some(ray))
                            },
                            None => {
                                let normalized = pointer_loc.position / window.physical_size().as_vec2();
                                (pointer_loc.position, normalized, None)
                            }
                        };
                        locations.push((camera_entity, position, normalized, pointer_loc.position, camera.viewport_to_world(camera_transform, pointer_loc.position).ok(), ray));
                    }
                }
            }
        }
        if let Some(location) = locations.first() {
            current_input.in_camera = Some(location.0);
            current_input.local_pos = Some(location.1);
            current_input.normalized_pos = Some(location.2);
            current_input.global_pos = Some(location.3);
            current_input.world_pos = location.4;
        } else {
            current_input.in_camera = None;
            current_input.local_pos = None;
            current_input.normalized_pos = None;
            current_input.global_pos = None;
            current_input.world_pos = None;
        }
        
        current_input.delta_pos = Vec2::ZERO;
        for ev in evr_motion.read() {
            current_input.delta_pos.x += ev.delta.x;
            current_input.delta_pos.y += ev.delta.y;
        }
    }
    
    fn keyboard_input(
        mut current_input: ResMut<CurrentKeyboardInput>,
        keys: Res<ButtonInput<KeyCode>>,
    ) {
        current_input.modify = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        current_input.update(&keys);
        current_input.confirm = keys.just_released(KeyCode::Enter) || keys.just_released(KeyCode::NumpadEnter);
        current_input.cancel = keys.just_released(KeyCode::Escape);
    }
}

fn make_ray(
    primary_window_entity: &Query<Entity, With<PrimaryWindow>>,
    camera: &Camera,
    camera_tfm: &GlobalTransform,
    pointer_loc: &PointerLocation,
) -> Option<Ray3d> {
    let pointer_loc = pointer_loc.location()?;
    if !pointer_loc.is_in_viewport(camera, primary_window_entity) {
        return None;
    }
    camera.viewport_to_world(camera_tfm, pointer_loc.position).ok()
}

fn mouse_precedence(mouse_buttons: Res<ButtonInput<MouseButton>>) -> (bool, Option<MouseButton>, Option<MouseButton>) {
    let left = mouse_buttons.pressed(MouseButton::Left);
    let right = mouse_buttons.pressed(MouseButton::Right);
    let middle = mouse_buttons.pressed(MouseButton::Middle);
    
    let left_released = mouse_buttons.just_released(MouseButton::Left);
    let right_released = mouse_buttons.just_released(MouseButton::Right);
    let middle_released = mouse_buttons.just_released(MouseButton::Middle);
    
    if !left && !right && !middle {
        if left_released && !right_released && !middle_released {
            return (false, None, Some(MouseButton::Left));
        }
        if right_released && !left_released && !middle_released {
            return (false, None, Some(MouseButton::Right));
        }
        if middle_released && !left_released && !right_released {
            return (false, None, Some(MouseButton::Middle));
        }
    }

    if left && !right && !middle {
        return (mouse_buttons.just_pressed(MouseButton::Left), Some(MouseButton::Left), None);
    }
    if right && !left && !middle {
        return (mouse_buttons.just_pressed(MouseButton::Right), Some(MouseButton::Right), None);
    }
    if middle && !left && !right {
        return (mouse_buttons.just_pressed(MouseButton::Middle), Some(MouseButton::Middle), None);
    }

    (false, None, None)
}
