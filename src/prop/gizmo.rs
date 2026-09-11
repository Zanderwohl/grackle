//! Moving and sizing a feature by dragging it in the viewport.
//!
//! An **arrow per axis** moves the selected feature and a **handle per face**
//! resizes it, both in the feature's *own* frame — a room has no rotation, and
//! a prop's parts have nothing but.
//!
//! The map editor's equivalents ([`crate::tool::point_drag`] and the handles in
//! [`crate::tool::room`]) spawn meshes and pick them with `MeshRayCast`, driven
//! by `FeatureTimeline`. None of that transfers to an enum in a different
//! resource, and the entities would need exempting from the hiding pass
//! besides. What is reused is the arithmetic — `closest_param_on_axis` and
//! `ray_point_distance`. These gizmos are **drawn with `Gizmos` and picked
//! analytically**: nothing to spawn or despawn, no assets per selection.
//!
//! **A radius is not a free extent.** Dragging a box's face leaves the opposite
//! face put, so the box grows one way and its origin shifts. A cylinder has no
//! opposite face — widening it widens it both ways — so a radial handle sets
//! the radius and the axis stays still. `Shape::axis_is_radial` decides, and
//! getting it wrong slides a cylinder sideways on every resize.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::editor::input::CurrentMouseInput;
use crate::prop::document::PropEditor;
use crate::prop::feature::FeatureOp;
use crate::prop::profile::Placement;
use crate::prop::solid::{Shape, MIN_EXTENT};
use crate::tool::tool_helpers::{closest_param_on_axis, ray_point_distance};

/// How big the gizmos are, as a fraction of the feature's largest extent.
///
/// Sized to the **feature**, not the camera: four viewports look at one world,
/// so a camera-relative size would have to be four sizes for one set of gizmos.
const HANDLE_SIZE: f32 = 0.09;

/// How far the move arrows reach past the shape.
const ARROW_REACH: f32 = 1.7;

/// How close the pointer has to come, relative to the handle size. Generous,
/// because a wireframe handle has almost no area to hit.
const PICK_SLACK: f32 = 1.8;

/// A floor, so a shape dragged down to a millimetre keeps something to grab.
const MIN_GIZMO_SCALE: f32 = 0.02;

/// What the pointer has hold of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// Slide along one of the feature's own axes.
    Move(usize),
    /// Move one face along its own axis. `max` is the `+` end.
    Face { axis: usize, max: bool },
}

/// The drag in progress, if any.
///
/// `offset` is the gap between where the pointer landed on the axis and where
/// the handle was, held for the drag so the handle moves *with* the pointer
/// rather than snapping its centre under it.
#[derive(Resource, Default)]
pub struct GizmoGrab {
    held: Option<(Handle, f32)>,
}

pub struct PropGizmoPlugin;

impl Plugin for PropGizmoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GizmoGrab>()
            .add_systems(Update, drag_the_selected.run_if(in_state(AppMode::Prop)))
            .add_systems(OnExit(AppMode::Prop), |mut grab: ResMut<GizmoGrab>| {
                grab.held = None;
            });
    }
}

/// Where every handle sits, in the feature's own space — one list, so drawing
/// and picking cannot disagree about where a handle is.
fn handles(shape: Option<&Shape>, scale: f32) -> Vec<(Handle, Vec3)> {
    let mut found = Vec::with_capacity(9);
    for axis in 0..3 {
        let mut direction = Vec3::ZERO;
        direction[axis] = 1.0;
        found.push((Handle::Move(axis), direction * scale * ARROW_REACH));
    }
    let Some(shape) = shape else { return found };

    let extents = shape.half_extents();
    for axis in 0..3 {
        for max in [false, true] {
            let mut point = Vec3::ZERO;
            point[axis] = if max { extents[axis] } else { -extents[axis] };
            found.push((Handle::Face { axis, max }, point));
        }
    }
    found
}

/// How big to draw the gizmos for a feature of these extents.
fn gizmo_scale(extents: Option<Vec3>) -> f32 {
    extents.map_or(MIN_GIZMO_SCALE, |extents| {
        (extents.max_element() * HANDLE_SIZE).max(MIN_GIZMO_SCALE)
    })
}

/// Move one face to `local_t` along its own axis.
///
/// Returns a placement as well as a shape, because resizing a box **moves**
/// it: the far face stays put and a shape is centred on its own origin, so the
/// origin takes up the difference. A radial axis leaves the origin alone.
pub fn drag_face(
    shape: &Shape,
    placement: &Placement,
    axis: usize,
    max: bool,
    local_t: f32,
) -> (Shape, Placement) {
    let mut shape = shape.clone();
    let mut placement = *placement;
    let half = shape.half_extents()[axis];

    if shape.axis_is_radial(axis) {
        shape.set_half_extent(axis, local_t.abs());
        return (shape, placement);
    }

    // Clamped against the *far* face rather than zero, so a face dragged
    // through the shape stops flush instead of turning it inside out.
    let (low, high) = match max {
        true => (-half, local_t.max(-half + MIN_EXTENT * 2.0)),
        false => (local_t.min(half - MIN_EXTENT * 2.0), half),
    };

    shape.set_half_extent(axis, (high - low) / 2.0);

    let mut along = Vec3::ZERO;
    along[axis] = (high + low) / 2.0;
    placement.origin = (placement.origin() + placement.rotation() * along).to_array();

    (shape, placement)
}

/// Draw the gizmos for the selected feature, and drag them.
///
/// One system rather than three: picking, dragging and drawing all want the
/// same handle positions and the same frame's pointer ray.
fn drag_the_selected(
    mut editor: ResMut<PropEditor>,
    mouse: Res<CurrentMouseInput>,
    mut grab: ResMut<GizmoGrab>,
    mut gizmos: Gizmos,
) {
    let Some(selected) = editor.selected else {
        grab.held = None;
        return;
    };
    let Some(feature) = editor.doc().feature(selected) else {
        grab.held = None;
        return;
    };

    // A shape gets size handles; anything with a placement gets move arrows —
    // so a sweep is moveable without pretending to have faces.
    let (shape, placement) = match &feature.op {
        FeatureOp::Primitive { shape, placement, .. } => (Some(shape.clone()), *placement),
        FeatureOp::Extrude { placement, .. } | FeatureOp::Revolve { placement, .. } => {
            (None, *placement)
        }
        _ => {
            grab.held = None;
            return;
        }
    };

    let scale = gizmo_scale(shape.as_ref().map(Shape::half_extents));
    let points = handles(shape.as_ref(), scale);
    let rotation = placement.rotation();
    let origin = placement.origin();
    let world = |local: Vec3| origin + rotation * local;
    let axis_direction = |axis: usize| {
        let mut unit = Vec3::ZERO;
        unit[axis] = 1.0;
        rotation * unit
    };

    // Over a panel, or outside every viewport. Anything held is let go: a drag
    // that continued from off screen would keep moving the feature.
    let Some(ray) = mouse.world_pos else {
        if grab.held.take().is_some() {
            editor.end_gesture();
        }
        draw(&mut gizmos, &points, &world, scale, None);
        return;
    };

    if mouse.released == Some(MouseButton::Left) || mouse.pressed != Some(MouseButton::Left) {
        if grab.held.take().is_some() {
            editor.end_gesture();
        }
    }

    let nearest = points
        .iter()
        .map(|(handle, local)| (*handle, ray_point_distance(&ray, world(*local))))
        .filter(|(_, distance)| *distance < scale * PICK_SLACK)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(handle, _)| handle);

    if grab.held.is_none()
        && mouse.just_pressed
        && mouse.pressed == Some(MouseButton::Left)
        && let Some(handle) = nearest
    {
        let axis = match handle {
            Handle::Move(axis) | Handle::Face { axis, .. } => axis,
        };
        let local = points
            .iter()
            .find(|(candidate, _)| *candidate == handle)
            .map(|(_, local)| *local)
            .unwrap_or(Vec3::ZERO);
        if let Some(on_axis) = closest_param_on_axis(ray, origin, axis_direction(axis)) {
            // See `GizmoGrab`: kept so the handle follows the pointer rather
            // than jumping its centre under it.
            grab.held = Some((handle, on_axis - local[axis]));
            editor.begin_gesture();
        }
    }

    if let Some((handle, offset)) = grab.held {
        let axis = match handle {
            Handle::Move(axis) | Handle::Face { axis, .. } => axis,
        };
        if let Some(on_axis) = closest_param_on_axis(ray, origin, axis_direction(axis)) {
            let local_t = on_axis - offset;
            match handle {
                Handle::Move(axis) => {
                    let moved = origin + axis_direction(axis) * local_t;
                    editor.doc_mut().feature_mut(selected).map(|feature| {
                        feature.op.placement_mut().map(|placement| {
                            placement.origin = moved.to_array();
                        })
                    });
                }
                Handle::Face { axis, max } => {
                    if let Some(shape) = shape.as_ref() {
                        let (resized, moved) = drag_face(shape, &placement, axis, max, local_t);
                        if let Some(feature) = editor.doc_mut().feature_mut(selected)
                            && let FeatureOp::Primitive { shape, placement, .. } = &mut feature.op
                        {
                            *shape = resized;
                            *placement = moved;
                        }
                    }
                }
            }
        }
    }

    draw(&mut gizmos, &points, &world, scale, grab.held.map(|(handle, _)| handle).or(nearest));
}

/// The arrows and the little cubes.
fn draw(
    gizmos: &mut Gizmos,
    points: &[(Handle, Vec3)],
    world: &impl Fn(Vec3) -> Vec3,
    scale: f32,
    lit: Option<Handle>,
) {
    // Red, green, blue for X, Y, Z, as the map editor's arrows already are.
    const AXIS_COLOURS: [Color; 3] = [
        Color::srgb(0.9, 0.3, 0.3),
        Color::srgb(0.3, 0.9, 0.3),
        Color::srgb(0.4, 0.5, 1.0),
    ];
    let highlight = Color::srgb(1.0, 0.95, 0.6);

    for (handle, local) in points {
        let axis = match handle {
            Handle::Move(axis) | Handle::Face { axis, .. } => *axis,
        };
        let colour = match lit == Some(*handle) {
            true => highlight,
            false => AXIS_COLOURS[axis],
        };
        match handle {
            Handle::Move(_) => {
                gizmos.arrow(world(Vec3::ZERO), world(*local), colour);
            }
            Handle::Face { .. } => {
                gizmos.cube(
                    Transform::from_translation(world(*local)).with_scale(Vec3::splat(scale)),
                    colour,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_shape() -> Shape {
        Shape::Box { size: [0.4, 0.4, 0.4] }
    }

    /// The whole point of a face handle: the face you are *not* dragging does
    /// not move. Checked in world space, since the origin is what takes up the
    /// difference — forgetting to move it slides both faces instead of one.
    #[test]
    fn dragging_a_face_leaves_the_opposite_one_where_it_was() {
        let placement = Placement::at(1.0, 2.0, 3.0);
        for axis in 0..3 {
            for max in [false, true] {
                let before = box_shape().half_extents()[axis];
                let far_before = placement.origin()[axis] + if max { -before } else { before };

                let target = if max { 0.9 } else { -0.9 };
                let (shape, moved) = drag_face(&box_shape(), &placement, axis, max, target);

                let after = shape.half_extents()[axis];
                let far_after = moved.origin()[axis] + if max { -after } else { after };
                assert!(
                    (far_after - far_before).abs() < 1e-5,
                    "axis {axis} max={max}: the far face moved from {far_before} to {far_after}",
                );

                let near = moved.origin()[axis] + if max { after } else { -after };
                assert!(
                    (near - (placement.origin()[axis] + target)).abs() < 1e-5,
                    "axis {axis} max={max}: the dragged face did not land where it was put",
                );
            }
        }
    }

    /// A cylinder resized by its side must not wander off its own axis. The
    /// failure is invisible in one drag and obvious after ten.
    #[test]
    fn resizing_a_radius_leaves_the_axis_alone() {
        let shape = Shape::Prism { sides: 12, radius: 0.05, height: 0.5 };
        let placement = Placement::at(0.2, 0.0, 0.0);

        for axis in [0, 2] {
            let (resized, moved) = drag_face(&shape, &placement, axis, true, 0.12);
            assert_eq!(moved.origin, placement.origin, "the cylinder slid sideways");
            assert!((resized.half_extents()[axis] - 0.12).abs() < 1e-6);
            // And the length is untouched by a radial drag.
            assert!((resized.half_extents().y - 0.25).abs() < 1e-6);
        }

        // Its height, though, is a free extent like a box's.
        let (resized, moved) = drag_face(&shape, &placement, 1, true, 0.4);
        assert!((resized.half_extents().y - 0.325).abs() < 1e-5);
        assert!(moved.origin()[1] > placement.origin()[1], "the base did not stay put");
    }

    /// A face dragged through the shape stops rather than inverting it.
    #[test]
    fn a_face_cannot_be_dragged_through_the_far_side() {
        for max in [false, true] {
            let past = if max { -5.0 } else { 5.0 };
            let (shape, _) = drag_face(&box_shape(), &Placement::default(), 0, max, past);
            assert!(
                shape.half_extents().x >= MIN_EXTENT,
                "the shape collapsed to {:?}",
                shape.half_extents(),
            );
        }
    }

    /// A drag in a rotated frame moves along *its* axis, not the world's.
    #[test]
    fn resizing_follows_the_features_own_axes() {
        // A quarter turn about Y takes local +X to world -Z.
        let placement = Placement {
            origin: [0.0; 3],
            rotation: [0.0, std::f32::consts::FRAC_PI_2, 0.0],
        };
        let (_, moved) = drag_face(&box_shape(), &placement, 0, true, 0.6);
        let origin = moved.origin();
        assert!(origin.z < -0.05, "the origin moved to {origin}, not along local +X");
        assert!(origin.x.abs() < 1e-5 && origin.y.abs() < 1e-5);
    }

    /// A sweep has no faces and still gets its arrows.
    #[test]
    fn a_shape_offers_three_arrows_and_six_faces_and_a_sweep_offers_three() {
        let with_shape = handles(Some(&box_shape()), 0.05);
        assert_eq!(with_shape.len(), 9);
        for axis in 0..3 {
            assert!(with_shape.iter().any(|(handle, _)| *handle == Handle::Move(axis)));
            for max in [false, true] {
                assert!(
                    with_shape.iter().any(|(handle, _)| *handle == Handle::Face { axis, max }),
                );
            }
        }
        assert_eq!(handles(None, 0.05).len(), 3);
    }

    /// A shape dragged down to nothing still has to be grabbable.
    #[test]
    fn a_tiny_shape_still_gets_gizmos_big_enough_to_grab() {
        assert!(gizmo_scale(Some(Vec3::splat(MIN_EXTENT))) >= MIN_GIZMO_SCALE);
        assert!(gizmo_scale(None) >= MIN_GIZMO_SCALE);
        assert!(gizmo_scale(Some(Vec3::splat(1.0))) > gizmo_scale(Some(Vec3::splat(0.1))));
    }
}
