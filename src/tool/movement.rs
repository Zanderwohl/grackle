 use bevy::app::App;
 use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
 use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use bevy_egui::{egui, EguiPrimaryContextPass, EguiContexts};
use crate::editor::input::{CurrentKeyboardInput, CurrentMouseInput};
use crate::editor::multicam::Multicam;
use crate::get;

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<MovementSettings>()
            .add_systems(Update, (
                Self::handle,
                )
            )
            .add_systems(EguiPrimaryContextPass, Self::debug_window)
        ;
    }
}

impl MovementPlugin {
    fn handle(
        mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
        settings: Res<MovementSettings>,
        mouse_input: Res<CurrentMouseInput>,
        keyboard_input: Res<CurrentKeyboardInput>,
        mut cameras: Query<(Entity, &mut Transform, &GlobalTransform, &Multicam, &mut Projection, &Camera)>,
        mut evr_scroll: MessageReader<MouseWheel>,
    ) {
        if mouse_input.started_in_camera.is_none() {
            cursor_options.grab_mode = CursorGrabMode::None;
            cursor_options.visible = true;
        }

        // For now, let's make middle click orbit for 3d cams and turn for 2d cam
        // and shift + middle click as pan
        if let Some(cam_id) = mouse_input.started_in_camera {
            if let Some(button) = mouse_input.pressed {
                if button == MouseButton::Middle {
                    // grab and hold mouse
                    cursor_options.grab_mode = CursorGrabMode::Locked;
                    cursor_options.visible = false;

                    for (entity, mut transform, global_transform, multicam, mut projection, camera) in &mut cameras {
                        if cam_id == entity {
                            let delta = mouse_input.delta_pos;
                            let forward = transform.forward();
                            match projection.into_inner() {
                                Projection::Perspective(projection) => {
                                    Self::perspective_move(&mut transform, global_transform, delta, &settings, &keyboard_input);

                                    for ev in evr_scroll.read() {
                                        match ev.unit {
                                            MouseScrollUnit::Line => {
                                                transform.translation += forward * settings.perspective_scroll * ev.y;
                                            }
                                            MouseScrollUnit::Pixel => {
                                                transform.translation += forward * settings.perspective_scroll * ev.y;
                                            }
                                        }
                                    }
                                }
                                Projection::Orthographic(projection) => {
                                    Self::ortho_move(&mut transform, delta, &projection, &settings, &keyboard_input);

                                    for ev in evr_scroll.read() {
                                        match ev.unit {
                                            MouseScrollUnit::Line => {
                                                projection.scale -= settings.orthographic_scroll * ev.y;
                                            }
                                            MouseScrollUnit::Pixel => {
                                                projection.scale -= settings.orthographic_scroll * ev.y;
                                            }
                                        }
                                        if projection.scale < 0.001 {
                                            projection.scale = 0.001;
                                        }
                                        if projection.scale > 0.1 {
                                            projection.scale = 0.1;
                                        }
                                    }
                                }
                                Projection::Custom(_) => {}
                            }
                        }
                    }
                }
            }
        }
    }

    fn perspective_move(transform: &mut Mut<Transform>, global_transform: &GlobalTransform, delta: Vec2, movement_settings: &Res<MovementSettings>, keyboard_input: &Res<CurrentKeyboardInput>) {
        if keyboard_input.modify {
            let pan_scaled_x = delta.x * movement_settings.perspective_pan;
            let pan_scaled_y = delta.y * movement_settings.perspective_pan;

            let local_x = transform.local_x();
            transform.translation -= local_x * pan_scaled_x;
            let local_y = transform.local_y();
            transform.translation += local_y * pan_scaled_y;
        } else {
            let pi_halves = std::f32::consts::FRAC_PI_2;
            let pi_fourths = std::f32::consts::PI / 2.0;
            let max = pi_fourths * 0.95;
            let min = -pi_fourths * 0.95;
            
            transform.rotate_y(-delta.x * movement_settings.perspective_rotate);
            transform.rotate_local_x(-delta.y * movement_settings.perspective_rotate);


            let (ry, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
            let local_dx = keyboard_input.backward() * movement_settings.perspective_speed;
            let local_dz = keyboard_input.right() * movement_settings.perspective_speed;
            let dx = local_dx * f32::sin(ry) + local_dz * f32::sin(ry + pi_halves);
            let dz = local_dx * f32::cos(ry) + local_dz * f32::cos(ry + pi_halves);
            
            let dy = keyboard_input.up() * movement_settings.perspective_speed;

            transform.translation += Vec3::new(dx, dy, dz);
        }
    }

    fn ortho_move(transform: &mut Mut<Transform>, delta: Vec2, projection: &OrthographicProjection, movement_settings: &Res<MovementSettings>, keyboard_input: &Res<CurrentKeyboardInput>) {
        let pi_halves = std::f32::consts::FRAC_PI_2;

        let pan_scaled_x = delta.x * projection.scale;
        let pan_scaled_y = delta.y * projection.scale;

        let local_x = transform.local_x();
        transform.translation -= local_x * pan_scaled_x;
        let local_y = transform.local_y();
        transform.translation += local_y * pan_scaled_y;

        let (ry, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
        let local_dx = keyboard_input.backward() * movement_settings.orthographic_speed;
        let local_dz = keyboard_input.right() * movement_settings.orthographic_speed;
        let dx = local_dx * f32::sin(ry) + local_dz * f32::sin(ry + pi_halves);
        let dz = local_dx * f32::cos(ry) + local_dz * f32::cos(ry + pi_halves);

        let dy = keyboard_input.up() * movement_settings.orthographic_speed;

        transform.translation += Vec3::new(dx, dy, dz);
    }
    
    fn debug_window(
        mut contexts: EguiContexts,
        mut settings: ResMut<MovementSettings>,
    ) {
        let ctx = contexts.ctx_mut();
        if ctx.is_err() { return; }
        let ctx = ctx.unwrap();
        
        if !settings.debug_window {
            return;
        }
        
        egui::Window::new(get!("debug.movement.title")).show(ctx, |ui| {
            ui.heading(get!("debug.movement.perspective.title"));
            ui.add(egui::Slider::new(&mut settings.perspective_pan, 0.0..=1.0).text(get!("debug.movement.perspective.pan")));
            ui.add(egui::Slider::new(&mut settings.perspective_rotate, 0.0..=0.1).text(get!("debug.movement.perspective.rotate")));
            ui.add(egui::Slider::new(&mut settings.perspective_speed, 0.0..=10.0).text(get!("debug.movement.perspective.speed")));
            ui.add(egui::Slider::new(&mut settings.perspective_scroll, 0.0..=1.0).text(get!("debug.movement.perspective.scroll")));

            ui.heading(get!("debug.movement.orthographic.title"));
            ui.add(egui::Slider::new(&mut settings.orthographic_scroll, 0.0..=1.0).text(get!("debug.movement.orthographic.scroll")));
        });
    }
}

#[derive(Resource)]
pub struct MovementSettings {
    debug_window: bool,
    perspective_pan: f32,
    perspective_rotate: f32,
    perspective_speed: f32,
    perspective_scroll: f32,
    orthographic_speed: f32,
    orthographic_scroll: f32,
}

impl Default for MovementSettings {
    fn default() -> Self {
        Self {
            debug_window: false,
            perspective_pan: 0.1,
            perspective_rotate: 0.01,
            perspective_speed: 0.2,
            perspective_scroll: 0.5,
            orthographic_speed: 0.2,
            orthographic_scroll: 0.01,
        }
    }
}
