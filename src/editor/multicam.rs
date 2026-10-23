use bevy::core_pipeline::tonemapping::Tonemapping;
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

/// The layer only the editor viewports draw, for the handles a tool stands in
/// the world. Hidden by layer rather than despawned on leaving the editor, so
/// a handle cannot be forgotten by some tool's teardown. 0 is the world, 1 the
/// viewmodel and 31 the shape painter.
pub const EDITOR_ONLY_LAYER: usize = 2;

/// Not inherited, so every mesh entity of a handle needs its own.
pub fn editor_only() -> RenderLayers {
    RenderLayers::layer(EDITOR_ONLY_LAYER)
}

/// How much of each window edge something else has taken, in logical pixels.
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
    /// The one place a window size is divided by, so a zero-width window is handled once (see
    /// `calculate_resize`).
    pub fn reserve(&mut self, insets: Insets, window: &Window) {
        self.reserve_from(insets, window.width(), window.height());
    }

    pub fn reserve_from(&mut self, insets: Insets, width: f32, height: f32) {
        // A window under a pixel across is mid-move. Dividing by it turns an inset into a fraction in the
        // thousands, and the crash that follows names only a bloom texture.
        if (0.0..1.0).contains(&width) || (0.0..1.0).contains(&height) {
            warn_once!(
                "window reported an implausible logical size of {width}x{height}; \
                 viewports left where they were",
            );
        }
        if width < 1.0 || height < 1.0 {
            // Keep the last good rect.
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

/// Whether a camera this size can be rendered: big enough to be worth it, and not so flat that
/// bloom asks wgpu for a texture past `max_texture_dimension_2d`.
///
/// Switched off rather than clamped: inventing a viewport the panels did not leave would draw over
/// them.
fn renderable(camera: UVec2) -> bool {
    if camera.x < MIN_CAMERA_SIDE || camera.y < MIN_CAMERA_SIDE {
        return false;
    }
    bloom_texture_width(camera) <= MAX_TEXTURE_SIDE as u64
}

/// Bloom sizes its mip chain by height (`size = viewport * max_mip / viewport.y`), so a viewport
/// one pixel tall asks for a texture `viewport.x * 512` wide. Measured: a viewport 1760 wide and
/// one pixel tall asked for 901120.
fn bloom_texture_width(camera: UVec2) -> u64 {
    camera.x as u64 * BLOOM_MAX_MIP as u64 / camera.y.max(1) as u64
}

/// Below this a viewport shows nothing worth a render pass.
const MIN_CAMERA_SIDE: u32 = 16;

/// `Bloom::max_mip_dimension`'s default; the crash arithmetic depends on it and it lives upstream.
const BLOOM_MAX_MIP: u32 = 512;

/// wgpu's `max_texture_dimension_2d`: 8192 at its lowest tier, 16384 on anything this runs on and
/// the number the validation error names. A sanity bound, not a capability query.
const MAX_TEXTURE_SIDE: u32 = 16384;

/// `None` if the fractions describe nothing a texture could be made from. Out of the system so the
/// mid-drag crash is testable without a window.
fn clamped_viewport(start: Vec2, end: Vec2, window: UVec2) -> Option<(UVec2, UVec2)> {
    if window.x == 0 || window.y == 0 || !start.is_finite() || !end.is_finite() {
        return None;
    }

    // A window this size is a measurement taken mid-move. Refused rather than clamped: a viewport 16384
    // across on a 2560 target is a validation error one frame later.
    if window.x > MAX_TEXTURE_SIDE || window.y > MAX_TEXTURE_SIDE {
        return None;
    }

    let window_f = window.as_vec2();
    let first = start.clamp(Vec2::ZERO, Vec2::ONE);
    let second = end.clamp(Vec2::ZERO, Vec2::ONE);
    // `end` before `start` would be a negative size, and cast, a huge one.
    let (low, high) = (first.min(second), first.max(second));

    // At least two pixels, since each camera gets half and zero size is invalid.
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
            // Applying the rects runs in every authoring mode, since each mode's panels write `MulticamState`.
            // The debug boxes stay in the map editor.
            .add_systems(
                Update,
                Self::set_camera_viewports.run_if(crate::common::app_mode::authoring),
            )
            .add_systems(Update, Self::debug_boxes.run_if(in_state(AppMode::Editor)))
            // After transform propagation, so gizmos use this frame's global transforms.
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
                    RenderLayers::from_layers(&[0, EDITOR_ONLY_LAYER]),
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

        if state.test_scene {
            Self::spawn_test_scene(&mut commands, meshes, materials);
        }
    }

    fn spawn_test_scene(commands: &mut Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
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

    /// Nothing is rendered on the frame the window changes size. Moving between monitors of different
    /// scale, `Window` reports the new size for a frame while the surface is still the old one; a
    /// viewport for the new size gets clamped to a sliver of the old surface and bloom asks for a texture
    /// wgpu refuses. Switching the cameras off for that frame avoids it.
    fn set_camera_viewports(
        window: Single<&Window, With<PrimaryWindow>>,
        mut resize_events: MessageReader<WindowResized>,
        mut cameras: Query<(&mut Camera, &Multicam)>,
        state: Res<MulticamState>,
        mut last_size: Local<UVec2>,
    ) {
        let size = window.physical_size();
        let settling = size != *last_size;
        *last_size = size;

        if settling {
            for (mut camera, _) in &mut cameras {
                if camera.is_active {
                    camera.is_active = false;
                }
            }
            // Spent here: the rects are applied next frame, when window and surface agree.
            resize_events.clear();
            return;
        }

        // Unconditional: the cameras must be switched back on after a settling frame even when the panels
        // did not move. `calculate_resize` compares before writing, so idle frames are cheap.
        Self::calculate_resize(&mut cameras, &state, &window);
    }
    
    fn calculate_resize(cameras: &mut Query<(&mut Camera, &Multicam)>, state: &Res<MulticamState>, window: &Window) {
        let window_size = window.physical_size();

        // A viewport may never exceed the window. While a window is dragged between monitors the
        // measurements go through frames of nonsense (a zero-width window, an inverted egui rect whose
        // negative width makes `1.0 - right - left` far greater than one), and the viewport sizes the
        // render textures, so wgpu refuses and Bevy exits. Clamped here because this is where the fractions
        // become a texture.
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

        let camera_size = UVec2::new(
            (viewport_size.x / 2).max(1),
            (viewport_size.y / 2).max(1),
        );

        // See `renderable`.
        let showing = renderable(camera_size);

        for (mut camera, multicam) in cameras {
            if camera.is_active != showing {
                camera.is_active = showing;
            }
            if !showing {
                continue;
            }
            let camera_pos = viewport_start + UVec2::new(
                multicam.screen_pos.x * camera_size.x,
                multicam.screen_pos.y * camera_size.y,
            );

            let wanted = Viewport {
                physical_position: camera_pos,
                physical_size: camera_size,
                ..Default::default()
            };
            // Compared before writing, so running every frame does not mark every camera changed.
            let same = camera.viewport.as_ref().is_some_and(|had| {
                had.physical_position == wanted.physical_position
                    && had.physical_size == wanted.physical_size
            });
            if !same {
                camera.viewport = Some(wanted);
            }
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
        ui_cam: Query<(&Camera, &Camera2d), (With<PrimaryEguiContext>, Without<Multicam>)>,
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

    /// Breaks if the play camera, which has the default layers, can see an
    /// editor handle, or if the layer is shared with something else's.
    #[test]
    fn handles_are_seen_by_the_editor_viewports_and_nothing_else() {
        assert!(!RenderLayers::default().intersects(&editor_only()), "the play camera sees handles");
        assert!(RenderLayers::from_layers(&[0, EDITOR_ONLY_LAYER]).intersects(&editor_only()));
        assert_ne!(EDITOR_ONLY_LAYER, crate::game::viewmodel::VIEWMODEL_LAYER);
        assert_ne!(EDITOR_ONLY_LAYER, 31);
    }

    /// A fraction from mid-drag nonsense must never reach a texture; wgpu refuses and Bevy exits with an
    /// error naming only `bloom_texture`.
    #[test]
    fn no_measurement_can_ask_for_a_viewport_bigger_than_the_window() {
        let window = Vec2::new(1280.0, 720.0);

        // An inverted panel rect reads as a negative width.
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

    /// A window reporting millions of pixels mid-drag must be refused, not clamped to. Measured: 2211840
    /// against a limit of 16384, half a viewport about 4.4 million across.
    #[test]
    fn a_window_reporting_nonsense_is_refused_rather_than_clamped() {
        for absurd in [
            UVec2::new(4_423_680, 2_160),
            UVec2::new(2_560, 4_423_680),
            UVec2::splat(u32::MAX),
        ] {
            assert!(
                clamped_viewport(Vec2::ZERO, Vec2::ONE, absurd).is_none(),
                "{absurd:?} was given a viewport rather than refused",
            );
        }

        assert!(clamped_viewport(Vec2::ZERO, Vec2::ONE, UVec2::splat(MAX_TEXTURE_SIDE)).is_some());
    }

    /// Bevy's own arithmetic, copied on purpose: a test with its own formula would pass while the
    /// program still crashed.
    fn bloom_texture_width(camera: UVec2) -> u64 {
        let ratio = BLOOM_MAX_MIP as f64 / camera.y as f64;
        (camera.x as f64 * ratio).round() as u64
    }

    /// No layout may ask for a bloom texture wgpu refuses. The reports were 901120 and 2211840: 1760 and
    /// 4320 wide against one pixel of height. Sweeps window sizes and insets, including ones that leave
    /// nothing.
    #[test]
    fn no_layout_asks_for_a_bloom_texture_the_device_refuses() {
        let windows = [
            UVec2::new(1280, 720),
            UVec2::new(2560, 1440),
            UVec2::new(3840, 2160),
            UVec2::new(1760, 100),
            UVec2::new(400, 300),
        ];
        // Fractions of the window, several taking more than all of it.
        let insets = [
            (0.0, 0.0, 0.0, 0.0),
            (0.2, 0.25, 0.05, 0.3),
            (0.0, 0.0, 0.6, 0.6),
            (0.0, 0.0, 0.98, 0.02),
            (0.6, 0.6, 0.0, 0.0),
            (0.9, 0.9, 0.9, 0.9),
            (0.0, 0.0, 0.999, 0.0),
        ];

        for window in windows {
            for (left, right, top, bottom) in insets {
                let mut state = MulticamState::default();
                state.reserve_from(
                    Insets {
                        left: left * window.x as f32,
                        right: right * window.x as f32,
                        top: top * window.y as f32,
                        bottom: bottom * window.y as f32,
                    },
                    window.x as f32,
                    window.y as f32,
                );

                let Some((_, viewport)) = clamped_viewport(state.start, state.end, window) else {
                    continue;
                };
                let camera = UVec2::new((viewport.x / 2).max(1), (viewport.y / 2).max(1));

                if !renderable(camera) {
                    continue;
                }

                let width = bloom_texture_width(camera);
                assert!(
                    width <= MAX_TEXTURE_SIDE as u64,
                    "window {window:?} with insets {left},{right},{top},{bottom} \
                     gave a camera of {camera:?}, whose bloom texture would be \
                     {width} wide against a limit of {}",
                    MAX_TEXTURE_SIDE,
                );
            }
        }
    }

    /// The two viewports from the reported crashes are refused outright.
    #[test]
    fn the_viewports_that_crashed_are_not_rendered() {
        for camera in [UVec2::new(1760, 1), UVec2::new(4320, 1), UVec2::new(1760, 16)] {
            assert!(
                !renderable(camera),
                "{camera:?} would be rendered, asking for a bloom texture {} wide",
                bloom_texture_width(camera),
            );
        }
    }

    #[test]
    fn an_ordinary_layout_still_renders() {
        for camera in [UVec2::new(640, 360), UVec2::new(1280, 720), UVec2::splat(16)] {
            assert!(renderable(camera), "{camera:?} was refused");
        }
    }

    #[test]
    fn an_ordinary_reservation_is_untouched() {
        let window = UVec2::new(1000, 1000);
        let (position, size) =
            clamped_viewport(Vec2::new(0.2, 0.1), Vec2::new(0.9, 0.8), window).expect("sane");
        assert_eq!(position, UVec2::new(200, 100));
        assert_eq!(size, UVec2::new(700, 700));
    }

    /// A window with no area keeps the last good rect rather than dividing by zero.
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
