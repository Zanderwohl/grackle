//! Getting around a prop in the editor's four viewports.
//!
//! The viewports themselves are the map editor's — reusing them is the whole
//! reason the prop editor can be a mode rather than a second program — but
//! **how you move in them is not the same job**, and pretending otherwise is
//! what this module exists to avoid:
//!
//! - A map editor's perspective view is a **free-look camera you fly**: there
//!   is no centre to a map, so turning in place and walking is the only thing
//!   that could be meant. A prop *is* a centre, and the thing you want a
//!   hundred times an hour is to see the other side of it. So the perspective
//!   view **orbits a pivot** rather than turning in place.
//! - Zooming is **multiplicative**, not a fixed step. A map's step is metres
//!   and a prop's is millimetres, and an additive step tuned for one is either
//!   imperceptible or catastrophic in the other. A constant ratio per notch
//!   feels the same at any magnitude, which is the only thing that can be true
//!   of both a receiver and a whole weapon.
//! - **The scroll wheel needs no button held.** In the map editor it is read
//!   only while the right button is down, because the same wheel drives other
//!   things. Here it is the only thing a wheel could mean.
//!
//! What *is* the same is the gesture: **right button drags**, and the modifier
//! key turns a rotate into a pan. A mapper who has learnt one view has learnt
//! the other, which matters more than any of the above.
//!
//! `Numpad0` puts the view under the cursor back where framing would have put
//! it. Under the cursor rather than all four, because the reason to reach for
//! it is that *one* view has been dragged somewhere useless — resetting the
//! other three as collateral would cost more than it saved.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::common::app_mode::AppMode;
use crate::editor::input::{CurrentKeyboardInput, CurrentMouseInput};
use crate::editor::multicam::{CameraAxis, Multicam};
use crate::prop::view::{PropBuild, PropSceneSystems};

/// How much room to leave around a framed prop.
const FRAMING_MARGIN: f32 = 1.6;

/// What the viewport falls back to when the prop is empty — a hand's breadth,
/// so a new document is already at weapon scale rather than at map scale.
const EMPTY_RADIUS: f32 = 0.25;

/// Radians of orbit per pixel dragged.
const ORBIT_RATE: f32 = 0.006;

/// Metres panned per pixel, **per metre of distance to the pivot**. Scaled by
/// distance so a drag moves the prop the same distance across the screen
/// whether you are looking at the whole weapon or at one screw.
const PAN_RATE: f32 = 0.0016;

/// How far one notch of the wheel zooms, as a ratio. A fifteenth is about
/// seven percent a notch: fine enough to creep up on a fit, coarse enough that
/// half a turn of the wheel crosses the whole range.
const ZOOM_PER_NOTCH: f32 = 1.0 / 15.0;

/// A trackpad reports in pixels and a wheel in lines, and a pixel is worth a
/// great deal less than a line. Without this a trackpad flick would cross the
/// entire zoom range in one gesture.
const PIXELS_PER_NOTCH: f32 = 40.0;

/// How close and how far the perspective camera may get to its pivot.
///
/// The near end is inside a rifle's receiver, which is as close as anybody
/// needs; the far end is the width of a room, which is as far as a prop is
/// ever worth looking at from.
const DISTANCE_RANGE: std::ops::RangeInclusive<f32> = 0.02..=40.0;

/// World units per pixel in the orthographic views. The low end is a tenth of
/// a millimetre across a viewport, which is finer than anything anybody will
/// model; the high end is a few metres.
const ORTHO_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.000_02..=0.05;

/// Stops the view flipping over at the poles, where "up" stops being a
/// direction and `look_at` has nothing to keep the horizon level with.
const MAX_ORBIT_PITCH: f32 = 1.54;

/// Where the editor's cameras were before the prop editor moved them.
///
/// A resource rather than a component on each camera, because it is *one*
/// question — "has this been saved yet" — and a per-camera answer could come
/// back half yes.
#[derive(Resource, Default)]
pub struct MapViewpoints(Vec<(Entity, Transform, Projection)>);

/// What the perspective view turns around.
///
/// Held rather than derived from the prop's bounds each frame, because panning
/// moves it: a pivot recomputed from the geometry would snap the view back to
/// the middle of the prop the instant you tried to look at a corner of it.
#[derive(Resource, Default)]
pub struct OrbitPivot(pub Vec3);

/// Asked for by the panel's *Frame* button, and by anything else that wants
/// the prop centred again.
///
/// A message rather than a function the panel calls, because framing needs
/// every camera's `Transform`, `Projection` and viewport, and the panel system
/// is already carrying most of the editor's resources. This keeps the camera
/// work in the module that owns the cameras.
#[derive(Message, Default)]
pub struct FrameTheProp;

pub struct PropCameraPlugin;

impl Plugin for PropCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapViewpoints>()
            .init_resource::<OrbitPivot>()
            .add_message::<FrameTheProp>()
            // After the scene is built, because framing reads the prop's
            // bounds: on the way in the last build belongs to a previous visit
            // or to no visit at all, and framing on stale bounds puts the
            // cameras somewhere plausible and wrong.
            .add_systems(
                OnEnter(AppMode::Prop),
                frame_on_entering.after(PropSceneSystems::Build),
            )
            .add_systems(OnExit(AppMode::Prop), (restore_viewpoints, let_go_of_the_cursor))
            .add_systems(
                Update,
                (frame_on_request, navigate, reset_the_hovered_view)
                    .after(PropSceneSystems::Build)
                    .run_if(in_state(AppMode::Prop)),
            );
    }
}

/// The point and the size everything frames itself against.
fn subject(bounds: Option<(Vec3, Vec3)>) -> (Vec3, f32) {
    match bounds {
        Some((min, max)) => ((min + max) / 2.0, ((max - min).length() / 2.0).max(1e-3)),
        None => (Vec3::ZERO, EMPTY_RADIUS),
    }
}

/// Put one camera where framing says it belongs.
///
/// One camera rather than four, so that `Numpad0` on a single view and framing
/// all of them are the same code. Two views that disagreed about what "framed"
/// means would be two views you could not compare.
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
        // The free camera comes in over the shoulder rather than square on, so
        // a box reads as a box in the one view that could show it as a square.
        CameraAxis::None => (Vec3::new(-0.6, 0.5, 1.0).normalize(), Vec3::Y),
        CameraAxis::X => (Vec3::X, Vec3::Y),
        CameraAxis::Y => (Vec3::Y, Vec3::NEG_Z),
        CameraAxis::Z => (Vec3::Z, Vec3::Y),
    };
    *transform = Transform::from_translation(centre + offset * distance).looking_at(centre, up);

    // An orthographic scale is world units per pixel, so how much of the prop
    // fits depends on how many pixels the viewport got. Reading it off the
    // camera is what makes framing mean the same thing in a maximised window
    // and in a narrow one.
    if let Projection::Orthographic(ortho) = projection {
        let across = camera
            .viewport
            .as_ref()
            .map(|viewport| viewport.physical_size.x.max(1) as f32)
            .unwrap_or(600.0);
        ortho.scale = (radius * FRAMING_MARGIN * 2.0 / across).clamp(
            *ORTHO_SCALE_RANGE.start(),
            *ORTHO_SCALE_RANGE.end(),
        );
        // Behind the camera as well as in front: an orthographic view placed a
        // short distance from a prop would otherwise clip the near half of it
        // away, and the whole point of the side views is to see the silhouette.
        ortho.near = -1000.0;
    }
}

/// Point every editor camera at the prop, remembering where each one was.
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

/// Save a camera's viewpoint the first time the prop editor touches it.
///
/// Only the first time: a second save would record where *we* put it, and
/// leaving the mode would then put the map editor's camera back to where the
/// prop editor had it.
fn remember(saved: &mut MapViewpoints, entity: Entity, transform: &Transform, projection: &Projection) {
    if !saved.0.iter().any(|(known, _, _)| *known == entity) {
        saved.0.push((entity, *transform, projection.clone()));
    }
}

fn frame_on_entering(
    mut cameras: Query<(Entity, &mut Transform, &mut Projection, &Camera, &Multicam)>,
    mut saved: ResMut<MapViewpoints>,
    mut pivot: ResMut<OrbitPivot>,
    build: Res<PropBuild>,
) {
    frame_all(&mut cameras, &mut saved, &mut pivot, build.evaluated.bounds());
}

fn frame_on_request(
    mut requests: MessageReader<FrameTheProp>,
    mut cameras: Query<(Entity, &mut Transform, &mut Projection, &Camera, &Multicam)>,
    mut saved: ResMut<MapViewpoints>,
    mut pivot: ResMut<OrbitPivot>,
    build: Res<PropBuild>,
) {
    if requests.read().count() == 0 {
        return;
    }
    frame_all(&mut cameras, &mut saved, &mut pivot, build.evaluated.bounds());
}

/// `Numpad0` puts the view under the cursor back to its default.
///
/// **Under the cursor, and nowhere else.** The reason to reach for this is
/// that one view has been dragged somewhere useless; resetting the other three
/// as collateral would cost more than it saved. With the cursor over no
/// viewport at all it does nothing, which is the honest answer to "reset
/// which one?".
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
    // The pivot is shared, and a reset is the clearest statement anybody makes
    // about what they are looking at.
    pivot.0 = centre;
}

/// Drag to move, wheel to zoom.
#[allow(clippy::too_many_arguments)]
fn navigate(
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<CurrentMouseInput>,
    keyboard: Res<CurrentKeyboardInput>,
    mut pivot: ResMut<OrbitPivot>,
    mut scrolls: MessageReader<MouseWheel>,
    mut cameras: Query<(Entity, &mut Transform, &mut Projection, &Multicam)>,
) {
    let dragging = mouse.started_in_camera.filter(|_| mouse.pressed == Some(MouseButton::Right));

    // Held still and hidden for the length of a drag, so a long orbit does not
    // run the pointer off the edge of the screen and stop. Released the moment
    // the button is, rather than on some later event, because a cursor nobody
    // can see is the one failure a user cannot work around.
    let (grab, visible) = match dragging {
        Some(_) => (CursorGrabMode::Locked, false),
        None => (CursorGrabMode::None, true),
    };
    if cursor.grab_mode != grab {
        cursor.grab_mode = grab;
        cursor.visible = visible;
    }

    // The wheel acts on whatever the pointer is over, with no button held: in
    // the map editor the same wheel drives other things, and here it cannot
    // mean anything else. Read once and applied to one camera, because a
    // `MessageReader` has its own cursor and reading it per camera would give
    // the first camera every notch and the rest none.
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

/// How much a number of notches multiplies a distance or a scale by.
///
/// Multiplicative rather than a fixed step: a constant ratio per notch feels
/// the same whether you are looking at a whole weapon or at one screw, and an
/// additive step cannot be true of both.
fn ratio(notches: f32) -> f32 {
    (1.0 + ZOOM_PER_NOTCH).powf(-notches)
}

/// Swing the camera around the pivot, keeping its distance.
///
/// Spherical coordinates rather than composed rotations, because the thing
/// that must not happen is drift: a camera turned by accumulating quaternions
/// slowly picks up roll, and a horizon that is a degree off after ten minutes
/// of looking at a model is worse than one that is obviously wrong.
fn orbit(transform: &mut Transform, pivot: Vec3, drag: Vec2) {
    let offset = transform.translation - pivot;
    let radius = offset.length();
    if radius < 1e-5 {
        return;
    }

    // Dragging right swings the camera left, so the prop turns the way the
    // hand does — and dragging down lifts the camera, so you end up looking
    // further *down* on it, which is the direction the map editor's free-look
    // moves the view for the same drag.
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

/// Slide the camera sideways, taking the pivot with it.
///
/// **With it**, which is the whole of what makes panning useful: leave the
/// pivot behind and the next orbit swings around a point that is no longer
/// anywhere near what you are looking at.
fn pan_perspective(transform: &mut Transform, pivot: &mut OrbitPivot, drag: Vec2) {
    let distance = (transform.translation - pivot.0).length().max(0.01);
    let local_x = transform.local_x();
    let local_y = transform.local_y();
    let movement = -local_x * drag.x * PAN_RATE * distance
        + local_y * drag.y * PAN_RATE * distance;
    transform.translation += movement;
    pivot.0 += movement;
}

/// Move the camera along the line to the pivot.
///
/// Dolly rather than a field-of-view change, and clamped short of the pivot:
/// zoom that could reach zero is zoom you cannot come back out of, because
/// every further notch multiplies nothing by something.
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

/// Put the cameras back exactly where the map editor left them.
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

/// Hand the pointer back on the way out, in case the mode was left mid-drag.
///
/// A grabbed cursor is not something a user can get out of by clicking
/// elsewhere — the same reason the pause menu exists — so releasing it is not
/// tidiness, it is the difference between leaving the mode and killing the
/// process.
fn let_go_of_the_cursor(mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>) {
    cursor.grab_mode = CursorGrabMode::None;
    cursor.visible = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two properties an orbit must have, and the two that are easy to
    /// lose: the camera stays the same distance away, and it keeps looking at
    /// what it is turning around. A composed-rotation implementation drifts on
    /// both.
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

    /// At the poles "up" is not a direction and `look_at` has nothing to keep
    /// the horizon level with, so the view rolls over. Dragging past the top
    /// has to stop rather than flip.
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

    /// Zoom is a ratio, so two notches one way and two back is where you
    /// started — at any magnitude. An additive step passes this and still
    /// feels wrong at one end of the range, which is why the ratio is the
    /// thing being asserted.
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

    /// Zoom that could reach the pivot is zoom you cannot come back out of:
    /// every further notch multiplies nothing by something.
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

    /// Panning has to carry the pivot, or the next orbit swings around a point
    /// that is no longer anywhere near what is on screen.
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
