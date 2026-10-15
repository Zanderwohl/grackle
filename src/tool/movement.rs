 use bevy::app::App;
 use bevy::input::mouse::MouseWheel;
 use bevy::prelude::*;
use crate::common::app_mode::AppMode;
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
            .add_systems(Update, Self::handle.run_if(in_state(AppMode::Editor)))
            .add_systems(EguiPrimaryContextPass, Self::debug_window.run_if(in_state(AppMode::Editor)))
        ;
    }
}

impl MovementPlugin {
    fn handle(
        mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
        settings: Res<MovementSettings>,
        mouse_input: Res<CurrentMouseInput>,
        keyboard_input: Res<CurrentKeyboardInput>,
        mut cameras: Query<(&mut Transform, &mut Projection), With<Multicam>>,
        mut evr_scroll: MessageReader<MouseWheel>,
    ) {
        // Drained every frame, whether or not anything is dragging. A reader
        // only read while dragging hands the first frame of a drag every notch
        // turned since the last one ended.
        let scroll: f32 = evr_scroll.read().map(|ev| ev.y).sum();

        let Some(cam_id) = mouse_input.started_in_camera else {
            Self::set_cursor(&mut *cursor_options, CursorGrabMode::None, true);
            return;
        };
        if mouse_input.pressed != Some(MouseButton::Right) {
            return;
        }

        Self::set_cursor(&mut *cursor_options, CursorGrabMode::Locked, false);

        let Ok((mut transform, mut projection)) = cameras.get_mut(cam_id) else {
            return;
        };
        let delta = mouse_input.delta_pos;
        let forward = transform.forward();
        // Matched through a shared borrow, so only a real zoom marks
        // `Projection` changed.
        match &*projection {
            Projection::Perspective(_) => {
                Self::perspective_move(&mut transform, delta, &settings, &keyboard_input);

                transform.translation += forward * settings.perspective_scroll * scroll;
            }
            Projection::Orthographic(ortho) => {
                Self::ortho_move(&mut transform, delta, ortho, &settings, &keyboard_input);

                let scale = (ortho.scale - settings.orthographic_scroll * scroll).clamp(0.001, 0.1);
                if scroll != 0.0 && scale != ortho.scale {
                    if let Projection::Orthographic(ortho) = &mut *projection {
                        ortho.scale = scale;
                    }
                }
            }
            Projection::Custom(_) => {}
        }
    }

    /// Writes the cursor only when it differs. Any write marks `CursorOptions`
    /// changed, and bevy_winit answers every change by re-issuing the OS grab
    /// call — so assigning the same values each frame is an OS call per frame
    /// for nothing.
    fn set_cursor(cursor_options: &mut Mut<CursorOptions>, grab_mode: CursorGrabMode, visible: bool) {
        if cursor_options.grab_mode != grab_mode {
            cursor_options.grab_mode = grab_mode;
        }
        if cursor_options.visible != visible {
            cursor_options.visible = visible;
        }
    }

    fn perspective_move(transform: &mut Mut<Transform>, delta: Vec2, movement_settings: &Res<MovementSettings>, keyboard_input: &Res<CurrentKeyboardInput>) {
        if keyboard_input.modify {
            let pan_scaled_x = delta.x * movement_settings.perspective_pan;
            let pan_scaled_y = delta.y * movement_settings.perspective_pan;

            let local_x = transform.local_x();
            transform.translation -= local_x * pan_scaled_x;
            let local_y = transform.local_y();
            transform.translation += local_y * pan_scaled_y;
        } else {
            let pi_halves = std::f32::consts::FRAC_PI_2;

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
        if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return; }
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::schedule::Schedule;
    use crate::editor::multicam::CameraAxis;

    #[derive(Resource, Default)]
    struct CursorChanged(bool);

    #[derive(Resource, Default)]
    struct CameraChanged {
        transform: bool,
        projection: bool,
    }

    fn note_changes(
        cursor: Query<(), (Changed<CursorOptions>, With<PrimaryWindow>)>,
        transforms: Query<(), (Changed<Transform>, With<Multicam>)>,
        projections: Query<(), (Changed<Projection>, With<Multicam>)>,
        mut cursor_seen: ResMut<CursorChanged>,
        mut camera_seen: ResMut<CameraChanged>,
    ) {
        cursor_seen.0 = !cursor.is_empty();
        camera_seen.transform = !transforms.is_empty();
        camera_seen.projection = !projections.is_empty();
    }

    /// A world with a window, one editor camera, and the input resources
    /// `handle` reads, plus a schedule that runs it and then records whether
    /// it touched the cursor. One schedule throughout, so change detection
    /// measures from the previous frame rather than from the system's birth.
    fn setup() -> (World, Schedule, Entity) {
        let mut world = World::new();
        world.init_resource::<MovementSettings>();
        world.init_resource::<CurrentMouseInput>();
        world.init_resource::<CurrentKeyboardInput>();
        world.init_resource::<Messages<MouseWheel>>();
        world.init_resource::<CursorChanged>();
        world.init_resource::<CameraChanged>();
        world.spawn((CursorOptions::default(), PrimaryWindow));
        let camera = world
            .spawn((
                Transform::default(),
                Projection::Perspective(PerspectiveProjection::default()),
                Multicam { name: String::new(), screen_pos: UVec2::ZERO, id: 0, axis: CameraAxis::None },
            ))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems((MovementPlugin::handle, note_changes).chain());
        (world, schedule, camera)
    }

    #[test]
    fn an_idle_frame_leaves_the_cursor_alone() {
        let (mut world, mut schedule, _) = setup();
        // The first frame counts the window's own insertion as a change.
        schedule.run(&mut world);
        schedule.run(&mut world);
        assert!(!world.resource::<CursorChanged>().0);
        schedule.run(&mut world);
        assert!(!world.resource::<CursorChanged>().0);
    }

    #[test]
    fn starting_a_right_drag_grabs_the_cursor_once() {
        let (mut world, mut schedule, camera) = setup();
        schedule.run(&mut world);
        schedule.run(&mut world);

        {
            let mut mouse = world.resource_mut::<CurrentMouseInput>();
            mouse.started_in_camera = Some(camera);
            mouse.pressed = Some(MouseButton::Right);
        }
        schedule.run(&mut world);
        assert!(world.resource::<CursorChanged>().0);
        let mut windows = world.query_filtered::<&CursorOptions, With<PrimaryWindow>>();
        let cursor = windows.single(&world).unwrap();
        assert_eq!(cursor.grab_mode, CursorGrabMode::Locked);
        assert!(!cursor.visible);

        // Holding the drag is not a second grab.
        schedule.run(&mut world);
        assert!(!world.resource::<CursorChanged>().0);
    }

    #[test]
    fn a_perspective_drag_does_not_touch_the_projection() {
        let (mut world, mut schedule, camera) = setup();
        {
            let mut mouse = world.resource_mut::<CurrentMouseInput>();
            mouse.started_in_camera = Some(camera);
            mouse.pressed = Some(MouseButton::Right);
            mouse.delta_pos = Vec2::new(3.0, 2.0);
        }
        // The first frame counts the camera's own insertion as a change.
        schedule.run(&mut world);
        schedule.run(&mut world);
        let seen = world.resource::<CameraChanged>();
        assert!(seen.transform, "the drag should still move the camera");
        assert!(!seen.projection);
    }
}
