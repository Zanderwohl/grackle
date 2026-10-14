use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::diagnostic::FrameCount;
use bevy::ecs::query::QuerySingleError;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::camera::{Viewport};
use bevy::camera::visibility::RenderLayers;
use bevy::camera::Hdr;
use bevy::window::{PrimaryWindow, WindowResized};
use bevy_egui::{egui, EguiPrimaryContextPass, EguiContexts, EguiGlobalSettings, PrimaryEguiContext};
use bevy_vector_shapes::prelude::*;
use crate::common::app_mode::AppMode;
use crate::common::painter;
use crate::tool::selection::EditorSelectable;
use crate::get;

pub struct MulticamPlugin {
    pub test_scene: bool,
}

/// How much of each edge of the window something else has taken, in logical
/// pixels.
///
/// A struct rather than four arguments because every caller computes all four
/// and the ordering of four `f32`s is exactly the kind of thing that is wrong
/// once and then wrong forever.
#[derive(Debug, Clone, Copy, Default)]
pub struct Insets {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

#[derive(Resource)]
pub struct MulticamState {
    pub test_scene: bool,
    pub start: Vec2,
    pub end: Vec2,
    pub debug_window: bool,
    pub debug_viewport_box: bool,
    pub debug_mouseover_boxes: bool,
    pub debug_mouse_circle: bool,
    pub draw_ortho_cameras: bool,
    pub draw_perspective_cameras: bool,
}

#[derive(Component)]
pub struct Multicam {
    pub name: String,
    pub screen_pos: UVec2,
    pub id: u32,
    pub axis: CameraAxis,
}

#[derive(PartialEq, Clone, Copy, Eq)]
pub enum CameraAxis {
    None,
    X,
    Y,
    Z,
}

#[derive(Component)]
pub struct MulticamTestScene;

impl MulticamState {
    /// Give the viewports what is left of the window after the panels.
    ///
    /// One function because there are three surfaces that reserve space — the
    /// map editor, the prop editor and the animation editor — and three copies
    /// of a division by a window size is three places for a zero-width window
    /// to produce an infinity. What that costs is in `calculate_resize`.
    pub fn reserve(&mut self, insets: Insets, window: &Window) {
        self.reserve_from(insets, window.width(), window.height());
    }

    /// The arithmetic half, without a `Window` to stand up in a test.
    pub fn reserve_from(&mut self, insets: Insets, width: f32, height: f32) {
        // A window under a pixel across is a window mid-move, not a layout.
        // Said out loud because dividing by it is how a panel inset a few
        // hundred pixels wide becomes a *fraction* in the thousands, and the
        // crash that follows names a bloom texture and nothing else.
        if (0.0..1.0).contains(&width) || (0.0..1.0).contains(&height) {
            warn_once!(
                "window reported an implausible logical size of {width}x{height}; \
                 viewports left where they were",
            );
        }
        if width < 1.0 || height < 1.0 {
            // Nothing sensible to say this frame, so the last good rect
            // stands. A window with no area is a window mid-move, not a
            // layout anybody asked for.
            return;
        }

        let start = Vec2::new(insets.left / width, insets.top / height);
        let end = Vec2::new(1.0 - insets.right / width, 1.0 - insets.bottom / height);
        if !start.is_finite() || !end.is_finite() {
            return;
        }

        self.start = start.clamp(Vec2::ZERO, Vec2::ONE);
        self.end = end.clamp(Vec2::ZERO, Vec2::ONE);
    }
}

/// The largest a render texture may be, which is what bounds a viewport.
///
/// wgpu's `max_texture_dimension_2d` is 8192 at its lowest tier and 16384 on
/// anything this runs on — and 16384 is the number the validation error names
/// when it refuses. Written down rather than read off the adapter because this
/// is a sanity bound on a transitional window size, not a capability query: a
/// viewport anywhere near it is already nonsense.
const MAX_TEXTURE_SIDE: u32 = 16384;

/// The viewport a pair of fractions describes, or `None` if they describe
/// nothing a texture could be made from.
///
/// Out of the system so it can be tested without a window, a device or a
/// render world — which is the only way to check a crash that only happens
/// while a window is being dragged between two monitors.
fn clamped_viewport(start: Vec2, end: Vec2, window: UVec2) -> Option<(UVec2, UVec2)> {
    if window.x == 0 || window.y == 0 || !start.is_finite() || !end.is_finite() {
        return None;
    }

    // **The window's own size is not to be trusted either.**
    //
    // Clamping the viewport to the window is only a guard while the window is
    // a plausible size, and during a drag between two monitors it is not:
    // winit reports a transitional size as the scale factor changes, and a
    // window a few million pixels across lets a viewport be half of that,
    // which is the same crash with the clamp in place. The first version of
    // this guard clamped to the window alone and the crash survived it.
    //
    // So the real bound is the device's, not the window's. Every dimension
    // here ends up as a render texture — the bloom chain most of all — and
    // `max_texture_dimension_2d` is what wgpu refuses past.
    let window = window.min(UVec2::splat(MAX_TEXTURE_SIDE));
    let window_f = window.as_vec2();
    let first = start.clamp(Vec2::ZERO, Vec2::ONE);
    let second = end.clamp(Vec2::ZERO, Vec2::ONE);
    // Ordered rather than assumed: `end` before `start` is a rectangle inside
    // out, which is a negative size and then, cast, a very large one.
    let (low, high) = (first.min(second), first.max(second));

    // Two pixels at least, since each camera gets half of it and a zero-sized
    // viewport is as invalid as an enormous one.
    let size = ((high - low) * window_f)
        .max(Vec2::splat(2.0))
        .min(window_f)
        .as_uvec2();
    let position = (low * window_f)
        .min(window_f - size.as_vec2())
        .max(Vec2::ZERO)
        .as_uvec2();

    Some((position, size))
}

impl Default for MulticamState {
    fn default() -> Self {
        Self {
            test_scene: false,
            start: Vec2::new(0.1, 0.1),
            end: Vec2::new(0.9, 0.9), // This MUST be more than start or else the first frame will crash.
            debug_viewport_box: false,
            debug_mouseover_boxes: false,
            debug_mouse_circle: false,
            debug_window: false,
            draw_ortho_cameras: false,
            draw_perspective_cameras: true,
        }
    }
}

impl Plugin for MulticamPlugin {
    fn build(&self, app: &mut App) {
        app
            .insert_resource(MulticamState {
                test_scene: self.test_scene,
                ..Default::default()
            })
            .add_systems(Startup, (
                Self::setup_first_camera,
                Self::setup.after(Self::setup_first_camera),
            ))
            // **Applying the rects runs in every authoring mode**, not just
            // the map editor. `MulticamState` is written by whichever panel
            // module is up — each measures its own docks and turns the insets
            // into normalised rects — but nothing was reading it outside the
            // map editor, so the four cameras simply kept whatever viewport
            // the editor had last set. The prop editor gets away with it
            // because its panels happen to be about the same size; a mode that
            // reserves a strip along the bottom does not, and the symptom is
            // viewports drawn straight over it.
            //
            // The debug boxes stay in the map editor, being a map editor tool.
            .add_systems(
                Update,
                Self::set_camera_viewports.run_if(crate::common::app_mode::authoring),
            )
            .add_systems(Update, Self::debug_boxes.run_if(in_state(AppMode::Editor)))
            // Global transforms are propagated from transforms during PostUpdate, so we need to draw the camera after that.
            .add_systems(PostUpdate, Self::draw_camera_gizmos.after(TransformSystems::Propagate).run_if(in_state(AppMode::Editor)))
            .add_systems(EguiPrimaryContextPass, Self::debug_window.run_if(in_state(AppMode::Editor)))
        ;
    }
}

impl MulticamPlugin {
    fn setup_first_camera(_commands: Commands) {

    }

    fn setup(
        mut commands: Commands,
        state: Res<MulticamState>,
        meshes: ResMut<Assets<Mesh>>,
        materials: ResMut<Assets<StandardMaterial>>,
        mut egui_global_settings: ResMut<EguiGlobalSettings>,
    ) {
        egui_global_settings.auto_create_primary_context = false;

        let perspective = Projection::Perspective(PerspectiveProjection {
            fov: 120.0,
            ..Default::default()
        });
        let orthographic = Projection::Orthographic(OrthographicProjection {
            near: 0.05,
            far: 1000.0,
            scaling_mode: Default::default(),
            scale: 0.01,
            ..OrthographicProjection::default_2d()
        });

        let dist = 5.0;
        let cameras = [
            (get!("viewport.free"), Transform::from_xyz(-2.5, 4.5, 9.0).looking_at(Vec3::ZERO, Vec3::Y), &perspective, CameraAxis::None),
            (get!("viewport.front"), Transform::from_xyz(dist, 0.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y), &orthographic, CameraAxis::X),
            (get!("viewport.top"), Transform::from_xyz(0.0, dist, 0.0).looking_at(Vec3::ZERO, -Vec3::X), &orthographic, CameraAxis::Y),
            (get!("viewport.right"), Transform::from_xyz(0.0, 0.0, dist).looking_at(Vec3::ZERO, Vec3::Y), &orthographic, CameraAxis::Z),
        ];
        let cameras_len = cameras.len();

        for (idx, (camera_name, camera_pos, projection, axis)) in cameras.into_iter().enumerate() {
            let camera = commands
                .spawn((
                    Camera3d::default(),
                    Camera {
                        order: (cameras_len - idx) as isize,
                        ..Default::default()
                    },
                    Hdr,
                    camera_pos,
                    Bloom::NATURAL,
                    Tonemapping::TonyMcMapface,
                    Multicam {
                        name: camera_name.to_string(),
                        screen_pos: UVec2::new((idx % 2) as u32, (idx / 2) as u32),
                        id: idx as u32,
                        axis,
                    },
                    projection.clone(),
                ))
                .id();

                commands
                    .spawn((
                        UiTargetCamera(camera),
                        Node {
                            width: Val::Percent(100.),
                            height: Val::Percent(100.),
                            ..Default::default()
                        }
                    ))
                    .with_children(|parent| {
                        parent.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                top: Val::Px(12.),
                                left: Val::Px(12.),
                                ..Default::default()
                            },
                            Text::new(camera_name),
                        ));
                    });
        }

        commands.spawn((
            PrimaryEguiContext,
            Camera2d::default(),
            GlobalTransform::default(),
            Camera {
                order: isize::MAX,
                ..Default::default()
            },
            Hdr,
            RenderLayers::layer(31)
        ));

        // Only spawn the test cube if test_scene is true
        if state.test_scene {
            Self::spawn_test_scene(&mut commands, meshes, materials);
        }
    }

    fn spawn_test_scene(commands: &mut Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
        // circular base
        commands.spawn((
            Mesh3d(meshes.add(Circle::new(4.0))),
            MeshMaterial3d(materials.add(Color::WHITE)),
            Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
            MulticamTestScene,
            EditorSelectable {
                id: "Base".to_owned(),
                bounding_box: Cuboid::new(8.0, 8.0, 0.2),
            },
        ));
        // cube
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
            MeshMaterial3d(materials.add(Color::srgb_u8(124, 144, 255))),
            Transform::from_xyz(0.0, 0.5, 0.0),
            MulticamTestScene,
            EditorSelectable {
                id: "Cube 1".to_owned(),
                bounding_box: Cuboid::new(1.0, 1.0, 1.0),
            },
        ));
        // cube
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
            MeshMaterial3d(materials.add(Color::srgb_u8(255, 144, 124))),
            Transform::from_xyz(1.3, 0.5, 1.0).with_rotation(Quat::from_rotation_y(2.0 * std::f32::consts::FRAC_PI_2 / 3.0)),
            MulticamTestScene,
            EditorSelectable {
                id: "Cube 2".to_owned(),
                bounding_box: Cuboid::new(1.0, 1.0, 1.0),
            },
        ));
        
        // light
        commands.spawn((
            PointLight {
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_xyz(4.0, 8.0, 4.0),
            MulticamTestScene,
        ));
    }

    pub fn draw_camera_gizmos(
        state: Res<MulticamState>,
        cameras: Query<(&Camera, &Multicam, &GlobalTransform, &Projection)>,
        mut gizmos: Gizmos,
    ) {
        let color = Color::srgb_u8(255, 0 ,0);
        for (camera, _, camera_tfm, projection) in cameras {
            match projection {
                Projection::Perspective(_) => if !state.draw_perspective_cameras { continue; }
                Projection::Orthographic(_) => if !state.draw_ortho_cameras { continue; }
                Projection::Custom(_) => { continue; }
            }
            
            let (s, t) = match projection {
                Projection::Perspective(_) => (-1.0, 1.5),
                Projection::Orthographic(_) => (0.0, 1.0),
                _ => (0.0, 0.0),
            };
            
            let a = camera.ndc_to_world(camera_tfm, Vec3::new(-1.05, -1.05, 1.0));
            let b = camera.ndc_to_world(camera_tfm, Vec3::new(1.05, -1.05, 1.0));
            let c = camera.ndc_to_world(camera_tfm, Vec3::new(-1.05, 1.05, 1.0));
            let d = camera.ndc_to_world(camera_tfm, Vec3::new(1.05, 1.05, 1.0));
            let e = camera.ndc_to_world(camera_tfm, Vec3::new(-0.4 * t, 1.1, 1.0));
            let f = camera.ndc_to_world(camera_tfm, Vec3::new(0.4 * t, 1.1, 1.0));
            let g = camera.ndc_to_world(camera_tfm, Vec3::new(0.0 * t, 1.4 * t, 1.0));
            match (a, b, c, d, e, f, g) {
                (Some(a), Some(b), Some(c), Some(d), Some(e), Some(f), Some(g)) => {
                    let a = a.lerp(camera_tfm.translation(), s);
                    let b = b.lerp(camera_tfm.translation(), s);
                    let c = c.lerp(camera_tfm.translation(), s);
                    let d = d.lerp(camera_tfm.translation(), s);
                    let e = e.lerp(camera_tfm.translation(), s);
                    let f = f.lerp(camera_tfm.translation(), s);
                    let g = g.lerp(camera_tfm.translation(), s);

                    gizmos.line(a, b, color);
                    gizmos.line(a, c, color);
                    gizmos.line(d, c, color);
                    gizmos.line(b, d, color);

                    gizmos.line(a, camera_tfm.translation(), color);
                    gizmos.line(b, camera_tfm.translation(), color);
                    gizmos.line(c, camera_tfm.translation(), color);
                    gizmos.line(d, camera_tfm.translation(), color);
                    
                    gizmos.line(e, f, color);
                    gizmos.line(f, g, color);
                    gizmos.line(e, g, color);
                }
                (_, _, _, _, _, _, _) => { info!("?") }
            }
        }
    }

    fn set_camera_viewports(
        window: Single<&Window, With<PrimaryWindow>>,
        mut resize_events: MessageReader<WindowResized>,
        mut cameras: Query<(&mut Camera, &Multicam)>,
        state: Res<MulticamState>,
        frames: Res<FrameCount>,
    ) {
        if frames.0 < 3 || state.is_changed() || !resize_events.is_empty() {
            resize_events.clear();
            Self::calculate_resize(&mut cameras, &state, &window);
        }
    }
    
    fn calculate_resize(cameras: &mut Query<(&mut Camera, &Multicam)>, state: &Res<MulticamState>, window: &Window) {
        let window_size = window.physical_size();

        // **A viewport is never allowed to be bigger than the window it is
        // in.** The fractions are arithmetic on sizes somebody else measured,
        // and while a window is being dragged between monitors those
        // measurements go through a frame or two of nonsense: a zero-width
        // window divides to infinity, and egui can hand back an inverted panel
        // rect whose width is negative, which turns `1.0 - right - left` into
        // a number far greater than one.
        //
        // What that costs is not a wrong-looking frame. The viewport sizes the
        // camera's render textures, so a fraction of a thousand asks for a
        // bloom texture a couple of million pixels across, wgpu refuses it,
        // and Bevy takes the process down — a crash on the way from one screen
        // to another, with nothing in the message about viewports.
        //
        // Clamped here rather than at the three places that write the
        // fractions, because this is the one place they become a texture.
        // Named once if it ever happens again, because the crash this guards
        // reports a bloom texture and a number, and nothing about where the
        // number came from.
        if window_size.x > MAX_TEXTURE_SIDE || window_size.y > MAX_TEXTURE_SIDE {
            warn_once!(
                "window reported an implausible physical size of {window_size:?}; \
                 viewports clamped to {MAX_TEXTURE_SIDE}",
            );
        }

        let Some((viewport_start, viewport_size)) =
            clamped_viewport(state.start, state.end, window_size)
        else {
            return;
        };

        // Calculate the size of each camera's viewport (2x2 grid)
        let camera_size = UVec2::new(
            (viewport_size.x / 2).max(1),
            (viewport_size.y / 2).max(1),
        );

        for (mut camera, multicam) in cameras {
            // Calculate this camera's position within the viewport
            let camera_pos = viewport_start + UVec2::new(
                multicam.screen_pos.x * camera_size.x,
                multicam.screen_pos.y * camera_size.y,
            );

            camera.viewport = Some(Viewport {
                physical_position: camera_pos,
                physical_size: camera_size,
                ..Default::default()
            });
        }
    }

    fn debug_window(
        mut state: ResMut<MulticamState>,
        mut contexts: EguiContexts,
    ) {
        let ctx = contexts.ctx_mut();
        if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return; }
        let ctx = ctx.unwrap();
        
        if !state.debug_window {
            return;
        }

        egui::Window::new(get!("debug.viewport.title")).show(ctx, |ui| {
            ui.heading(get!("debug.viewport.controls"));
            
            // Start coordinates
            ui.heading("X");
            ui.horizontal(|ui| {
                ui.label(get!("debug.viewport.start"));
                let mut start_x = state.start.x;
                if ui.add(egui::Slider::new(&mut start_x, 0.0..=state.end.x - 0.01)).changed() {
                    state.start.x = start_x;
                }
            });
            ui.horizontal(|ui| {
                ui.label(get!("debug.viewport.end"));
                let mut end_x = state.end.x;
                if ui.add(egui::Slider::new(&mut end_x, (state.start.x + 0.01)..=1.0)).changed() {
                    state.end.x = end_x;
                }
            });

            ui.separator();

            // End coordinates
            ui.heading("Y");
            ui.horizontal(|ui| {
                ui.label(get!("debug.viewport.start"));
                let mut start_y = state.start.y;
                if ui.add(egui::Slider::new(&mut start_y, 0.0..=state.end.y - 0.01)).changed() {
                    state.start.y = start_y;
                }
            });
            ui.horizontal(|ui| {
                ui.label(get!("debug.viewport.end"));
                let mut end_y = state.end.y;
                if ui.add(egui::Slider::new(&mut end_y, (state.start.y + 0.01)..=1.0)).changed() {
                    state.end.y = end_y;
                }
            });
            
            ui.separator();
            ui.heading(get!("debug.viewport.draw.title"));
            ui.checkbox(&mut state.debug_mouse_circle, get!("debug.viewport.draw.mouse"));
            ui.checkbox(&mut state.debug_viewport_box, get!("debug.viewport.draw.box"));
            ui.checkbox(&mut state.debug_mouseover_boxes, get!("debug.viewport.draw.mouseover"));
        });
    }

    fn debug_boxes(
        state: ResMut<MulticamState>,
        mouse_buttons: Res<ButtonInput<MouseButton>>,
        windows: Query<&Window, With<PrimaryWindow>>,
        cameras_q: Query<(Entity, &Camera, &GlobalTransform, &Multicam)>,
        ui_cam: Query<(&Camera, &Camera2d), Without<Multicam>>,
        mut painter: ShapePainter,
        mut contexts: EguiContexts,
    ) {
        let ctx = contexts.ctx_mut();
        if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return; }
        let ctx = ctx.unwrap();
        
        if ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input() {
            return;
        }

        let window = windows.single().unwrap();
        let ui_cam = ui_cam.single();

        let left_pressed = mouse_buttons.pressed(MouseButton::Left);
        let right_pressed = mouse_buttons.pressed(MouseButton::Right);

        let mut button: Option<MouseButton> = None;

        if left_pressed && right_pressed {
            // If both were just pressed, discard for this interaction
        } else if left_pressed {
            button = Some(MouseButton::Left);
        } else if right_pressed {
            button = Some(MouseButton::Right);
        }

        Self::debug_ui_boxes(&state, &mut painter, window, &ui_cam);

        if let Some(cursor_pos_window) = window.cursor_position() {
            for (_camera_entity, camera, _camera_tfm, _camera_multicam) in &cameras_q {
                if let Ok((ui_cam, _)) = ui_cam {
                    if let Some(viewport) = &camera.viewport {
                        let vp_min = viewport.physical_position.as_vec2();
                        let vp_max = vp_min + viewport.physical_size.as_vec2();

                        let physical_cursor_x = cursor_pos_window.x * window.scale_factor();
                        let physical_cursor_y = cursor_pos_window.y * window.scale_factor();

                        // Check if cursor is within this viewport's bounds
                        if physical_cursor_x >= vp_min.x && physical_cursor_x < vp_max.x &&
                            physical_cursor_y >= vp_min.y && physical_cursor_y < vp_max.y
                        {
                            if let Some(button) = button {
                                if mouse_buttons.pressed(button) && state.debug_mouseover_boxes {
                                    Self::draw_indicator_box(&mut painter, &ui_cam, viewport, button);
                                }
                            } else {
                                painter.reset(); // Reset painter properties for this specific drawing
                                painter.render_layers = Some(RenderLayers::layer(31));
                                painter.color = Color::srgb_u8(200, 200, 200);
                                painter.thickness = 1.0; // Define border thickness

                                let min = viewport.physical_position.as_vec2();
                                let max = min + viewport.physical_size.as_vec2();
                                let min = painter::window_to_painter(&ui_cam, min);
                                let max = painter::window_to_painter(&ui_cam, max);

                                if state.debug_mouseover_boxes {
                                    draw_rect(&mut painter, min, max);
                                }
                            }

                            break; // Border drawn for the first viewport found under cursor
                        }
                    }
                }
            }
        }
    }

    fn debug_ui_boxes(state: &ResMut<MulticamState>, mut painter: &mut ShapePainter, window: &Window, ui_cam: &Result<(&Camera, &Camera2d), QuerySingleError>) {
        if let Ok((ui_cam, _)) = ui_cam {
            if state.debug_viewport_box {
                painter.reset();
                painter.render_layers = Some(RenderLayers::layer(31));
                let viewport_start = painter::window_to_painter_frac(&ui_cam, state.start).extend(1.0);
                let viewport_end = painter::window_to_painter_frac(&ui_cam, state.end).extend(1.0);
                painter.color = Color::srgb_u8(0, 0, 255);
                draw_rect(&mut painter, viewport_start.truncate(), viewport_end.truncate());
            }

            if state.debug_mouse_circle {
                if let Some(cursor_window_pos) = window.cursor_position() {
                    painter.reset();
                    painter.render_layers = Some(RenderLayers::layer(31));
                    painter.set_translation(painter::window_to_painter(&ui_cam, cursor_window_pos).extend(1.0));
                    painter.circle(10.0);
                }
            }
        }
    }

    fn draw_indicator_box(mut painter: &mut ShapePainter, ui_cam: &&Camera, viewport: &Viewport, button: MouseButton) {
        let color = match button {
            MouseButton::Left => Color::WHITE,
            MouseButton::Right => Color::hsv(0.0, 0.4, 1.0),
            _ => Color::srgb_u8(0, 255, 0), // Should not be reached due to earlier logic
        };

        painter.reset(); // Reset painter properties for this specific drawing
        painter.render_layers = Some(RenderLayers::layer(31));
        painter.color = color;
        painter.thickness = 2.0; // Define border thickness

        let min = viewport.physical_position.as_vec2();
        let max = min + viewport.physical_size.as_vec2();
        let min = painter::window_to_painter(&ui_cam, min);
        let max = painter::window_to_painter(&ui_cam, max);

        draw_rect(&mut painter, min, max);
    }
}

fn draw_rect(
    painter: &mut ShapePainter,
    min: Vec2,
    max: Vec2,
) {
    painter.line(Vec3::new(min.x, min.y, 0.0), Vec3::new(max.x, min.y, 0.0)); // Bottom
    painter.line(Vec3::new(min.x, max.y, 0.0), Vec3::new(max.x, max.y, 0.0)); // Top
    painter.line(Vec3::new(min.x, min.y, 0.0), Vec3::new(min.x, max.y, 0.0)); // Left
    painter.line(Vec3::new(max.x, min.y, 0.0), Vec3::new(max.x, max.y, 0.0)); // Right
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A viewport fraction is arithmetic on sizes somebody else measured, and
    /// while a window is dragged between monitors those measurements go
    /// through a frame or two of nonsense. None of it may reach a texture.
    ///
    /// The failure this guards is not a wrong-looking frame: the viewport
    /// sizes the camera's render textures, so a fraction of a thousand asks
    /// wgpu for a bloom texture a couple of million pixels across, wgpu
    /// refuses, and Bevy quits the application. The message says `bloom_texture`
    /// and nothing about viewports, which is why it is worth pinning here.
    #[test]
    fn no_measurement_can_ask_for_a_viewport_bigger_than_the_window() {
        let window = Vec2::new(1280.0, 720.0);

        // An inverted panel rect reads as a negative width, which turns
        // `1.0 - right - left` into a number far greater than one.
        let nonsense = [
            (Vec2::new(-1000.0, -1000.0), Vec2::new(1024.0, 1024.0)),
            (Vec2::new(0.0, 0.0), Vec2::new(f32::INFINITY, 1.0)),
            (Vec2::new(f32::NAN, 0.0), Vec2::new(1.0, 1.0)),
            // Inside out: `end` before `start`.
            (Vec2::new(0.9, 0.9), Vec2::new(0.1, 0.1)),
            // Degenerate: no area at all.
            (Vec2::new(0.5, 0.5), Vec2::new(0.5, 0.5)),
        ];

        for (start, end) in nonsense {
            let Some((position, size)) = clamped_viewport(start, end, window.as_uvec2()) else {
                // Refusing outright is a fine answer; the last good rect stands.
                continue;
            };
            assert!(
                size.x >= 1 && size.y >= 1,
                "{start:?}..{end:?} gave a viewport with no area: {size:?}",
            );
            assert!(
                size.x <= window.x as u32 && size.y <= window.y as u32,
                "{start:?}..{end:?} gave a viewport bigger than the window: {size:?}",
            );
            assert!(
                position.x + size.x <= window.x as u32
                    && position.y + size.y <= window.y as u32,
                "{start:?}..{end:?} put a viewport off the edge: {position:?} + {size:?}",
            );
        }
    }

    /// **The window's own size is not to be trusted either**, which is what
    /// the first version of this guard got wrong: it clamped the viewport to
    /// the window, and a window reporting a few million pixels across while
    /// being dragged between two monitors let the viewport be half of that.
    /// The crash survived the fix.
    ///
    /// The number in that crash — 2211840 against a limit of 16384 — is half
    /// a viewport, so the viewport was about four and a half million across,
    /// which no fraction of a real window produces.
    #[test]
    fn a_window_reporting_nonsense_cannot_make_a_texture_wgpu_refuses() {
        let absurd = UVec2::new(4_423_680, 2_160);
        let (position, size) =
            clamped_viewport(Vec2::ZERO, Vec2::ONE, absurd).expect("a size is still describable");

        assert!(
            size.x <= MAX_TEXTURE_SIDE && size.y <= MAX_TEXTURE_SIDE,
            "a viewport larger than any texture: {size:?}",
        );
        assert!(
            position.x <= MAX_TEXTURE_SIDE && position.y <= MAX_TEXTURE_SIDE,
            "a viewport positioned off the end of any texture: {position:?}",
        );
    }

    /// And an ordinary reservation still lands where it used to.
    #[test]
    fn an_ordinary_reservation_is_untouched() {
        let window = UVec2::new(1000, 1000);
        let (position, size) =
            clamped_viewport(Vec2::new(0.2, 0.1), Vec2::new(0.9, 0.8), window).expect("sane");
        assert_eq!(position, UVec2::new(200, 100));
        assert_eq!(size, UVec2::new(700, 700));
    }

    /// A window with no area is a window mid-move; the last good rect stands
    /// rather than being replaced with a division by zero.
    #[test]
    fn a_window_with_no_area_changes_nothing() {
        let mut state = MulticamState::default();
        state.start = Vec2::new(0.25, 0.25);
        state.end = Vec2::new(0.75, 0.75);

        state.reserve_from(Insets { left: 260.0, right: 300.0, top: 24.0, bottom: 180.0 }, 0.0, 0.0);

        assert_eq!(state.start, Vec2::new(0.25, 0.25));
        assert_eq!(state.end, Vec2::new(0.75, 0.75));
    }
}
