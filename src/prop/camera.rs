//! Getting around a prop in the editor's four viewports: orbit a pivot,
//! multiplicative zoom, wheel needs no button. CLAUDE.md, "Getting around",
//! has the reasons.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;

use crate::common::skeleton::{Pose, Skeleton};
use crate::prop::figure::ScaleFigureMarker;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::common::app_mode::AppMode;
use crate::editor::input::{CurrentKeyboardInput, CurrentMouseInput};
use crate::editor::multicam::{CameraAxis, Multicam};
use crate::game::viewmodel::{VIEWMODEL_FOV, VIEWMODEL_NEAR};
use crate::prop::document::PropEditor;
use crate::prop::view::{PropBuild, PropSceneSystems};

/// How much room to leave around a framed prop.
const FRAMING_MARGIN: f32 = 1.6;

/// Radius framed when the prop is empty, in metres: weapon scale, not map scale.
const EMPTY_RADIUS: f32 = 0.25;

/// Radians of orbit per pixel dragged.
const ORBIT_RATE: f32 = 0.006;

/// Metres panned per pixel, per metre of distance to the pivot.
const PAN_RATE: f32 = 0.0016;

/// Zoom ratio per notch, about seven percent.
const ZOOM_PER_NOTCH: f32 = 1.0 / 15.0;

/// A trackpad reports pixels and a wheel lines; without this a trackpad flick
/// crosses the whole zoom range.
pub(crate) const PIXELS_PER_NOTCH: f32 = 40.0;

/// Metres from the pivot the perspective camera may be.
const DISTANCE_RANGE: std::ops::RangeInclusive<f32> = 0.02..=40.0;

/// World units per pixel in the orthographic views.
const ORTHO_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.000_02..=0.05;

/// Physical pixels assumed across a camera whose viewport is not laid out yet.
const UNLAID_VIEWPORT_WIDTH: f32 = 600.0;

/// Radians. Short of the poles, where `look_at` has no horizon to keep level.
const MAX_ORBIT_PITCH: f32 = 1.54;

/// Whether the perspective view is standing in for the player's eye. Set by
/// the Hold panel while a viewmodel field is being dragged.
#[derive(Resource, Default)]
pub struct ViewmodelPreview(pub bool);

/// Where the perspective camera was before it stood in for an eye. Separate
/// from [`MapViewpoints`] so a preview is never restored as the way back to
/// the map.
#[derive(Resource, Default)]
struct PreviewViewpoint(Option<(Entity, Transform, Projection)>);

/// Where the editor's cameras were before the prop editor moved them. One
/// resource rather than a component each, so "saved yet" cannot be half true.
#[derive(Resource, Default)]
pub struct MapViewpoints(Vec<(Entity, Transform, Projection)>);

/// What the perspective view turns around. Held rather than derived from the
/// bounds, because panning moves it.
#[derive(Resource, Default)]
pub struct OrbitPivot(pub Vec3);

/// Asked for by the panel's *Frame* button. A message because the panel system
/// cannot also carry every camera.
///
/// `selected` frames the selection's standing bodies; if none are standing it
/// falls back to the whole prop.
#[derive(Message, Default)]
pub struct FrameTheProp {
    pub selected: bool,
}

pub struct PropCameraPlugin;

impl Plugin for PropCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapViewpoints>()
            .init_resource::<OrbitPivot>()
            .init_resource::<ViewmodelPreview>()
            .init_resource::<PreviewViewpoint>()
            .add_message::<FrameTheProp>()
            // After the build, because framing reads the prop's bounds.
            // Swapping between the two prop modes therefore re-frames.
            .add_systems(
                OnEnter(AppMode::Prop),
                frame_on_entering.after(PropSceneSystems::Build),
            )
            .add_systems(
                OnEnter(AppMode::Animation),
                frame_on_entering.after(PropSceneSystems::Build),
            )
            // Forget the preview first, or leaving mid-drag leaves a stale
            // viewpoint to paste over the next framing.
            .add_systems(
                OnExit(AppMode::Prop),
                (forget_the_preview, restore_viewpoints, let_go_of_the_cursor).chain(),
            )
            .add_systems(
                OnExit(AppMode::Animation),
                (forget_the_preview, restore_viewpoints, let_go_of_the_cursor).chain(),
            )
            .add_systems(
                Update,
                (
                    preview_the_viewmodel,
                    frame_on_request,
                    // Skipped while the preview owns the camera.
                    navigate.run_if(|preview: Res<ViewmodelPreview>| !preview.0),
                    reset_the_hovered_view,
                )
                    .after(PropSceneSystems::Build)
                    .run_if(crate::common::app_mode::showing_a_prop),
            );
    }
}

fn subject(bounds: Option<(Vec3, Vec3)>) -> (Vec3, f32) {
    match bounds {
        Some((min, max)) => ((min + max) / 2.0, ((max - min).length() / 2.0).max(1e-3)),
        None => (Vec3::ZERO, EMPTY_RADIUS),
    }
}

/// Frame one camera. `Numpad0` and framing all four share it.
fn frame_one(
    transform: &mut Transform,
    projection: &mut Projection,
    camera: &Camera,
    multicam: &Multicam,
    centre: Vec3,
    radius: f32,
) {
    let distance = (radius * FRAMING_MARGIN * 2.0).max(*DISTANCE_RANGE.start());

    let (offset, up) = match multicam.axis {
        // Over the shoulder rather than square on, so a box reads as a box.
        CameraAxis::None => (Vec3::new(-0.6, 0.5, 1.0).normalize(), Vec3::Y),
        CameraAxis::X => (Vec3::X, Vec3::Y),
        CameraAxis::Y => (Vec3::Y, Vec3::NEG_Z),
        CameraAxis::Z => (Vec3::Z, Vec3::Y),
    };
    *transform = Transform::from_translation(centre + offset * distance).looking_at(centre, up);

    // Orthographic scale is world units per pixel, so it depends on viewport width.
    if let Projection::Orthographic(ortho) = projection {
        let across = camera
            .viewport
            .as_ref()
            .map(|viewport| viewport.physical_size.x.max(1) as f32)
            .unwrap_or(UNLAID_VIEWPORT_WIDTH);
        ortho.scale = (radius * FRAMING_MARGIN * 2.0 / across).clamp(
            *ORTHO_SCALE_RANGE.start(),
            *ORTHO_SCALE_RANGE.end(),
        );
        // Behind the camera as well as in front, or a side view clips away the
        // near half of the silhouette it exists to show.
        ortho.near = -1000.0;
    }
}

fn frame_all(
    cameras: &mut Query<(Entity, &mut Transform, &mut Projection, &Camera, &Multicam)>,
    saved: &mut MapViewpoints,
    pivot: &mut OrbitPivot,
    bounds: Option<(Vec3, Vec3)>,
) {
    let (centre, radius) = subject(bounds);
    pivot.0 = centre;
    for (entity, mut transform, mut projection, camera, multicam) in cameras.iter_mut() {
        remember(saved, entity, &transform, &projection);
        frame_one(&mut transform, &mut projection, camera, multicam, centre, radius);
    }
}

/// Only the first save counts; a second would record where we put it.
fn remember(saved: &mut MapViewpoints, entity: Entity, transform: &Transform, projection: &Projection) {
    if !saved.0.iter().any(|(known, _, _)| *known == entity) {
        saved.0.push((entity, *transform, projection.clone()));
    }
}

type FigureQuery<'w, 's> =
    Query<'w, 's, (&'static Transform, &'static Skeleton), (With<ScaleFigureMarker>, Without<Multicam>)>;

/// The prop plus the figure, if one is up: a body is two metres and a prop
/// centimetres, so framing the prop alone puts the figure off-screen.
///
/// The figure is measured at its rest pose, or the view would jump every time
/// the playhead moves.
fn framing_bounds(build: &PropBuild, figure: &FigureQuery) -> Option<(Vec3, Vec3)> {
    figure
        .iter()
        .flat_map(|(root, skeleton)| {
            skeleton
                .posed_bones(&Pose::rest(), root)
                .into_iter()
                .flat_map(|bone| [bone.head, bone.tail])
        })
        .fold(build.evaluated.bounds(), |bounds, point| match bounds {
            Some((min, max)) => Some((min.min(point), max.max(point))),
            None => Some((point, point)),
        })
}

fn frame_on_entering(
    mut cameras: Query<(Entity, &mut Transform, &mut Projection, &Camera, &Multicam)>,
    mut saved: ResMut<MapViewpoints>,
    mut pivot: ResMut<OrbitPivot>,
    build: Res<PropBuild>,
    figure: FigureQuery,
) {
    let bounds = framing_bounds(&build, &figure);
    frame_all(&mut cameras, &mut saved, &mut pivot, bounds);
}

fn frame_on_request(
    mut requests: MessageReader<FrameTheProp>,
    mut cameras: Query<(Entity, &mut Transform, &mut Projection, &Camera, &Multicam)>,
    mut saved: ResMut<MapViewpoints>,
    mut pivot: ResMut<OrbitPivot>,
    build: Res<PropBuild>,
    editor: Res<PropEditor>,
    figure: FigureQuery,
) {
    let Some(request) = requests.read().last() else {
        return;
    };
    let selected = request
        .selected
        .then(|| build.evaluated.bounds_of_bodies(&editor.picked_bodies(&build.evaluated)))
        .flatten();
    let bounds = selected.or_else(|| framing_bounds(&build, &figure));
    frame_all(&mut cameras, &mut saved, &mut pivot, bounds);
}

/// `Numpad0` resets the view under the cursor; over no viewport it does nothing.
fn reset_the_hovered_view(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<CurrentMouseInput>,
    build: Res<PropBuild>,
    mut saved: ResMut<MapViewpoints>,
    mut pivot: ResMut<OrbitPivot>,
    mut cameras: Query<(Entity, &mut Transform, &mut Projection, &Camera, &Multicam)>,
) {
    if !keys.just_pressed(KeyCode::Numpad0) {
        return;
    }
    let Some(hovered) = mouse.in_camera else {
        return;
    };
    let Ok((entity, mut transform, mut projection, camera, multicam)) = cameras.get_mut(hovered)
    else {
        return;
    };

    let (centre, radius) = subject(build.evaluated.bounds());
    remember(&mut saved, entity, &transform, &projection);
    frame_one(&mut transform, &mut projection, camera, multicam, centre, radius);
    pivot.0 = centre;
}

fn navigate(
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<CurrentMouseInput>,
    keyboard: Res<CurrentKeyboardInput>,
    mut pivot: ResMut<OrbitPivot>,
    mut scrolls: MessageReader<MouseWheel>,
    mut cameras: Query<(Entity, &mut Transform, &mut Projection, &Multicam)>,
) {
    let dragging = mouse.started_in_camera.filter(|_| mouse.pressed == Some(MouseButton::Right));

    // Locked during a drag so a long orbit does not run off the screen.
    let (grab, visible) = match dragging {
        Some(_) => (CursorGrabMode::Locked, false),
        None => (CursorGrabMode::None, true),
    };
    if cursor.grab_mode != grab {
        cursor.grab_mode = grab;
        cursor.visible = visible;
    }

    // Read once, outside the loop: a `MessageReader` has its own cursor, so
    // reading per camera gives the first every notch and the rest none.
    let notches: f32 = scrolls
        .read()
        .map(|scroll| match scroll.unit {
            MouseScrollUnit::Line => scroll.y,
            MouseScrollUnit::Pixel => scroll.y / PIXELS_PER_NOTCH,
        })
        .sum();

    for (entity, mut transform, mut projection, multicam) in &mut cameras {
        let drag = (dragging == Some(entity)).then_some(mouse.delta_pos).unwrap_or(Vec2::ZERO);
        let zoom = if mouse.in_camera == Some(entity) { notches } else { 0.0 };
        if drag == Vec2::ZERO && zoom == 0.0 {
            continue;
        }

        match &mut *projection {
            Projection::Perspective(_) => {
                let _ = multicam;
                if keyboard.modify {
                    pan_perspective(&mut transform, &mut pivot, drag);
                } else {
                    orbit(&mut transform, pivot.0, drag);
                }
                dolly(&mut transform, pivot.0, zoom);
            }
            Projection::Orthographic(ortho) => {
                let scale = ortho.scale;
                let local_x = transform.local_x();
                let local_y = transform.local_y();
                transform.translation -= local_x * drag.x * scale;
                transform.translation += local_y * drag.y * scale;

                ortho.scale = (scale * ratio(zoom))
                    .clamp(*ORTHO_SCALE_RANGE.start(), *ORTHO_SCALE_RANGE.end());
            }
            Projection::Custom(_) => {}
        }
    }
}

fn ratio(notches: f32) -> f32 {
    (1.0 + ZOOM_PER_NOTCH).powf(-notches)
}

/// Spherical coordinates rather than composed rotations, which accumulate roll.
fn orbit(transform: &mut Transform, pivot: Vec3, drag: Vec2) {
    let offset = transform.translation - pivot;
    let radius = offset.length();
    if radius < 1e-5 {
        return;
    }

    // Dragging right swings the camera left, so the prop turns with the hand;
    // dragging down lifts it, matching the map editor's free-look.
    let yaw = offset.x.atan2(offset.z) - drag.x * ORBIT_RATE;
    let pitch = ((offset.y / radius).clamp(-1.0, 1.0).asin() + drag.y * ORBIT_RATE)
        .clamp(-MAX_ORBIT_PITCH, MAX_ORBIT_PITCH);

    let direction = Vec3::new(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        yaw.cos() * pitch.cos(),
    );
    transform.translation = pivot + direction * radius;
    transform.look_at(pivot, Vec3::Y);
}

/// Takes the pivot along, or the next orbit swings around something off screen.
fn pan_perspective(transform: &mut Transform, pivot: &mut OrbitPivot, drag: Vec2) {
    let distance = (transform.translation - pivot.0).length().max(0.01);
    let local_x = transform.local_x();
    let local_y = transform.local_y();
    let movement = -local_x * drag.x * PAN_RATE * distance
        + local_y * drag.y * PAN_RATE * distance;
    transform.translation += movement;
    pivot.0 += movement;
}

/// Clamped short of the pivot: a distance of zero cannot be multiplied back out.
fn dolly(transform: &mut Transform, pivot: Vec3, notches: f32) {
    if notches == 0.0 {
        return;
    }
    let offset = transform.translation - pivot;
    let distance = offset.length();
    if distance < 1e-5 {
        return;
    }
    let wanted = (distance * ratio(notches))
        .clamp(*DISTANCE_RANGE.start(), *DISTANCE_RANGE.end());
    transform.translation = pivot + offset / distance * wanted;
}

/// Stand the perspective view where the player's eye would be, with the
/// viewmodel camera's FOV and near plane. The camera moves by the inverse of
/// the viewmodel transform; the prop stays at its origin.
fn preview_the_viewmodel(
    preview: Res<ViewmodelPreview>,
    editor: Res<PropEditor>,
    mut saved: ResMut<PreviewViewpoint>,
    mut cameras: Query<(Entity, &mut Transform, &mut Projection, &Camera, &Multicam)>,
) {
    if !preview.0 {
        if let Some((entity, transform, projection)) = saved.0.take()
            && let Ok((_, mut current, mut current_projection, _, _)) = cameras.get_mut(entity)
        {
            *current = transform;
            *current_projection = projection;
        }
        return;
    }

    let spec = editor.doc().viewmodel;
    for (entity, mut transform, mut projection, camera, multicam) in &mut cameras {
        if multicam.axis != CameraAxis::None {
            continue;
        }
        let Some(size) = camera.logical_viewport_size().filter(|size| size.y > 0.0) else {
            continue;
        };
        if saved.0.is_none() {
            saved.0 = Some((entity, *transform, projection.clone()));
        }

        *projection = Projection::Perspective(PerspectiveProjection {
            fov: VIEWMODEL_FOV,
            near: VIEWMODEL_NEAR,
            ..default()
        });
        let weapon = spec.transform(VIEWMODEL_FOV, size.x / size.y);
        let rotation = weapon.rotation.inverse();
        *transform = Transform::from_translation(rotation * -weapon.translation)
            .with_rotation(rotation);
    }
}

fn forget_the_preview(mut preview: ResMut<ViewmodelPreview>, mut saved: ResMut<PreviewViewpoint>) {
    preview.0 = false;
    saved.0 = None;
}

fn restore_viewpoints(
    mut cameras: Query<(&mut Transform, &mut Projection), With<Multicam>>,
    mut saved: ResMut<MapViewpoints>,
) {
    for (entity, transform, projection) in saved.0.drain(..) {
        let Ok((mut current, mut current_projection)) = cameras.get_mut(entity) else {
            continue;
        };
        *current = transform;
        *current_projection = projection;
    }
}

/// In case the mode was left mid-drag; a grabbed cursor cannot be clicked away.
fn let_go_of_the_cursor(mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>) {
    cursor.grab_mode = CursorGrabMode::None;
    cursor.visible = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbiting_keeps_its_distance_and_its_aim() {
        let pivot = Vec3::new(0.2, -0.1, 0.4);
        let mut transform =
            Transform::from_translation(pivot + Vec3::new(0.0, 0.3, 1.0)).looking_at(pivot, Vec3::Y);
        let started = (transform.translation - pivot).length();

        for _ in 0..200 {
            orbit(&mut transform, pivot, Vec2::new(9.0, 3.0));
        }

        assert!(
            ((transform.translation - pivot).length() - started).abs() < 1e-4,
            "the camera drifted to {} m from {started} m",
            (transform.translation - pivot).length(),
        );
        let to_pivot = (pivot - transform.translation).normalize();
        assert!(
            transform.forward().dot(to_pivot) > 0.9999,
            "the camera stopped looking at what it was orbiting",
        );
    }

    /// At the poles the view rolls over.
    #[test]
    fn orbiting_cannot_be_dragged_over_the_top() {
        let pivot = Vec3::ZERO;
        let mut transform = Transform::from_xyz(0.0, 0.0, 1.0).looking_at(pivot, Vec3::Y);
        for _ in 0..500 {
            orbit(&mut transform, pivot, Vec2::new(0.0, 40.0));
        }
        assert!(
            transform.translation.y < (1.0 - 1e-4),
            "the camera reached the pole at {}",
            transform.translation,
        );
        assert!(transform.up().y > 0.0, "the view rolled over");
    }

    #[test]
    fn zooming_in_and_back_out_returns_to_the_same_distance() {
        for distance in [0.05_f32, 0.5, 5.0] {
            let pivot = Vec3::ZERO;
            let mut transform =
                Transform::from_xyz(0.0, 0.0, distance).looking_at(pivot, Vec3::Y);
            for _ in 0..5 {
                dolly(&mut transform, pivot, 1.0);
            }
            for _ in 0..5 {
                dolly(&mut transform, pivot, -1.0);
            }
            assert!(
                (transform.translation.z - distance).abs() < 1e-3,
                "zooming in and out from {distance} m ended at {} m",
                transform.translation.z,
            );
        }
    }

    #[test]
    fn zooming_all_the_way_in_still_leaves_somewhere_to_zoom_out_from() {
        let pivot = Vec3::ZERO;
        let mut transform = Transform::from_xyz(0.0, 0.0, 1.0).looking_at(pivot, Vec3::Y);
        for _ in 0..500 {
            dolly(&mut transform, pivot, 1.0);
        }
        assert!(transform.translation.z >= *DISTANCE_RANGE.start() - 1e-6);

        dolly(&mut transform, pivot, -1.0);
        assert!(
            transform.translation.z > *DISTANCE_RANGE.start(),
            "the camera could not be backed out again",
        );
    }

    #[test]
    fn panning_takes_the_pivot_with_it() {
        let mut pivot = OrbitPivot(Vec3::ZERO);
        let mut transform = Transform::from_xyz(0.0, 0.0, 2.0).looking_at(Vec3::ZERO, Vec3::Y);
        let before = transform.translation - pivot.0;

        pan_perspective(&mut transform, &mut pivot, Vec2::new(60.0, -25.0));

        assert!(pivot.0.length() > 0.01, "the pivot stayed behind");
        assert!(
            (transform.translation - pivot.0 - before).length() < 1e-5,
            "panning changed where the camera is relative to its pivot",
        );
    }
}
