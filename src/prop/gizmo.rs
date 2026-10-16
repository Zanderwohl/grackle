//! Moving and sizing a feature by dragging it in the viewport.
//!
//! Each handle works in the feature's own frame. Drawn with `Gizmos` and picked
//! analytically rather than spawned as meshes like the map editor's, since
//! those are driven by `FeatureTimeline` and would need exempting from the
//! hiding pass.
//!
//! Dragging a box's face leaves the opposite face put, so the origin shifts. A
//! radial axis (`Shape::axis_is_radial`) has no opposite face: it sets the
//! radius and the origin stays still.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::rotation::quat_from_euler;
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{Pose, Skeleton};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::prop::archetype::ColliderShape;
use crate::prop::document::{Pick, PropEditor};
use crate::prop::feature::FeatureOp;
use crate::prop::figure::{FigureReach, ScaleFigure, ScaleFigureMarker};
use crate::prop::hold::HoldSpec;
use crate::prop::profile::Placement;
use crate::prop::plane::plane_under;
use crate::prop::reference::{element_under, Pointer};
use crate::prop::solid::{Shape, MIN_EXTENT};
use crate::tool::tool_helpers::{closest_param_on_axis, ray_point_distance};

/// Gizmo size as a fraction of the feature's largest extent. Not camera-relative,
/// because four viewports would want four sizes.
const HANDLE_SIZE: f32 = 0.09;

/// Move arrow length, in multiples of the gizmo scale.
const ARROW_REACH: f32 = 1.7;

/// Pick radius relative to the handle size. Generous, as a wireframe has almost
/// no area to hit.
const PICK_SLACK: f32 = 1.8;

/// Metres. A floor so a tiny shape keeps something to grab.
const MIN_GIZMO_SCALE: f32 = 0.02;

/// X, Y, Z.
const AXIS_COLOURS: [Color; 3] =
    [Color::srgb(0.9, 0.3, 0.3), Color::srgb(0.3, 0.9, 0.3), Color::srgb(0.4, 0.5, 1.0)];

/// The handle under the pointer, or being dragged.
const HIGHLIGHT: Color = Color::srgb(1.0, 0.95, 0.6);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Move(usize),
    /// Move one face along its own axis. `max` is the `+` end.
    Face { axis: usize, max: bool },
    Grip { support: bool, axis: usize },
    /// Move the figure, which is how a carry is authored.
    Carry(usize),
    Muzzle(usize),
}

impl Handle {
    fn axis(self) -> usize {
        match self {
            Handle::Move(axis)
            | Handle::Face { axis, .. }
            | Handle::Grip { axis, .. }
            | Handle::Carry(axis)
            | Handle::Muzzle(axis) => axis,
        }
    }
}

/// The tracer's colour, since that is what will come out of it.
const MUZZLE_COLOUR: Color = Color::srgb(1.0, 0.85, 0.4);
/// Metres.
const MUZZLE_MARK_RADIUS: f32 = 0.012;

/// Whether the hold's handles are up instead of the selected feature's. Never
/// both: a grip sits at the prop's origin, where a feature's move arrows are.
#[derive(Resource, Default)]
pub struct HoldHandles(pub bool);

/// Where an axis arrow is grabbed. Out along its own axis: three arrows picked
/// at one shared point tie, and `min_by` would always pick X.
fn arrow_tip(anchor: Vec3, axis: usize, length: f32) -> Vec3 {
    anchor + Vec3::AXES[axis] * length
}

/// Grip handle size, in metres.
const GRIP_SIZE: f32 = 0.02;

/// Metres from an arrow's tip that still grabs it.
const GRIP_PICK_RADIUS: f32 = GRIP_SIZE * 2.0;

/// Also sets how far apart the pick points are: more than twice the pick
/// radius, so no two are in range at once.
const GRIP_LENGTH: f32 = GRIP_SIZE * 3.0;

/// The drag in progress: the handle, and the offset between where the pointer
/// landed on the axis and the dragged thing, so it does not snap to the pointer.
/// `answered` is the ray and document generation last answered: a re-solve of
/// the same ray writes rounding noise, so only a moved ray or document re-solves.
#[derive(Resource, Default)]
pub struct GizmoGrab {
    held: Option<(Handle, f32)>,
    answered: Option<(Ray3d, u64)>,
}

pub struct PropGizmoPlugin;

impl Plugin for PropGizmoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GizmoGrab>()
            .init_resource::<HoldHandles>()
            .init_resource::<HoveredPick>()
            // Picking after dragging, so a press that grabbed a handle does not
            // also re-select.
            .add_systems(
                Update,
                // Not while sketching: a click then belongs to the sketch.
                (drag_the_selected, pick_in_the_viewport)
                    .chain()
                    .run_if(in_state(AppMode::Prop))
                    .run_if(crate::prop::sketch_edit::not_sketching),
            )
            .add_systems(Update, draw_colliders.run_if(in_state(AppMode::Prop)))
            .add_systems(OnExit(AppMode::Prop), |mut grab: ResMut<GizmoGrab>, mut hovered: ResMut<HoveredPick>| {
                grab.held = None;
                hovered.0 = None;
            });
    }
}

/// A placed prop's colliders, which nothing else in the viewport shows: they are
/// authored apart from the geometry.
fn draw_colliders(editor: Res<PropEditor>, mut gizmos: Gizmos) {
    let doc = editor.doc();
    if !doc.archetype.is_placed() {
        return;
    }
    let colour = match doc.archetype.blocks() {
        true => Color::srgb(1.0, 0.55, 0.1),
        // A trigger, not a wall.
        false => Color::srgb(0.3, 0.9, 0.5),
    };
    for collider in &doc.colliders {
        let at = Vec3::from_array(collider.origin);
        match collider.shape {
            ColliderShape::Box { size } => {
                gizmos.cube(Transform::from_translation(at).with_scale(Vec3::from_array(size)), colour);
            }
            ColliderShape::Sphere { radius } => {
                gizmos.sphere(Isometry3d::from_translation(at), radius, colour);
            }
            ColliderShape::Capsule { radius, height } => {
                let length = (height - 2.0 * radius).max(0.0);
                gizmos.primitive_3d(&Capsule3d::new(radius, length), Isometry3d::from_translation(at), colour);
            }
        }
    }
}

/// Every handle, in the feature's own space. Drawing and picking both use it.
fn handles(shape: Option<&Shape>, scale: f32) -> Vec<(Handle, Vec3)> {
    let mut found = Vec::with_capacity(9);
    for axis in 0..3 {
        found.push((Handle::Move(axis), Vec3::AXES[axis] * scale * ARROW_REACH));
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

fn gizmo_scale(extents: Option<Vec3>) -> f32 {
    extents.map_or(MIN_GIZMO_SCALE, |extents| {
        (extents.max_element() * HANDLE_SIZE).max(MIN_GIZMO_SCALE)
    })
}

/// Move one face to `local_t` along its own axis. Returns a new placement too,
/// because a shape is centred on its origin and the far face must stay put.
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

    // Clamped against the far face, so dragging through stops instead of inverting.
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

/// Pick, drag and draw in one system: all three want the same handle positions
/// and pointer ray.
fn drag_the_selected(
    mut editor: ResMut<PropEditor>,
    mouse: Res<CurrentMouseInput>,
    mut grab: ResMut<GizmoGrab>,
    hold_handles: Res<HoldHandles>,
    choice: Res<ScaleFigure>,
    figures: Query<(&Skeleton, &Pose, &Transform, &FigureReach), With<ScaleFigureMarker>>,
    build: Res<crate::prop::view::PropBuild>,
    mut gizmos: Gizmos,
) {
    if hold_handles.0 {
        drag_the_hold(&mut editor, &mouse, &mut grab, *choice, &figures, &mut gizmos);
        return;
    }

    let Some(selected) = editor.primary_feature(&build.evaluated) else {
        grab.held = None;
        return;
    };
    let Some(feature) = editor.doc().feature(selected) else {
        grab.held = None;
        return;
    };

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
    let axis_direction = |axis: usize| rotation * Vec3::AXES[axis];

    // Off every viewport: let go, or a drag continued from off screen keeps moving.
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
        let axis = handle.axis();
        let local = points
            .iter()
            .find(|(candidate, _)| *candidate == handle)
            .map(|(_, local)| *local)
            .unwrap_or(Vec3::ZERO);
        if let Some(on_axis) = closest_param_on_axis(ray, origin, axis_direction(axis)) {
            // Measured against what the drag moves: a move arrow slides the
            // origin (parameter zero), not its tip, or the feature jumps by the
            // arrow's length on grab.
            let anchor = match handle {
                Handle::Move(_) => 0.0,
                _ => local[axis],
            };
            grab.held = Some((handle, on_axis - anchor));
            grab.answered = None;
            editor.begin_gesture();
        }
    }

    if let Some((handle, offset)) = grab.held
        && grab.answered != Some((ray, editor.generation()))
    {
        let axis = handle.axis();
        if let Some(on_axis) = closest_param_on_axis(ray, origin, axis_direction(axis)) {
            let local_t = on_axis - offset;
            match handle {
                Handle::Move(axis) => {
                    let moved = (origin + axis_direction(axis) * local_t).to_array();
                    // Only on a real move: a button held still is not an edit,
                    // and writing one anyway bumps the generation and dirties
                    // the document every frame somebody pauses mid-drag.
                    if placement.origin != moved {
                        editor.doc_mut().feature_mut(selected).map(|feature| {
                            feature.op.placement_mut().map(|placement| {
                                placement.origin = moved;
                            })
                        });
                    }
                }
                Handle::Face { axis, max } => {
                    if let Some(shape) = shape.as_ref() {
                        let (resized, moved) = drag_face(shape, &placement, axis, max, local_t);
                        if (&resized, &moved) != (shape, &placement)
                            && let Some(feature) = editor.doc_mut().feature_mut(selected)
                            && let FeatureOp::Primitive { shape, placement, .. } = &mut feature.op
                        {
                            *shape = resized;
                            *placement = moved;
                        }
                    }
                }
                Handle::Grip { .. } | Handle::Carry(_) | Handle::Muzzle(_) => {}
            }
        }
        // After the write, so the drag's own edit is not mistaken next frame
        // for somebody else's.
        grab.answered = Some((ray, editor.generation()));
    }

    draw(&mut gizmos, &points, &world, scale, grab.held.map(|(handle, _)| handle).or(nearest));
}

/// What a click in the viewport would pick right now; drawn by the view.
#[derive(Resource, Default)]
pub struct HoveredPick(pub Option<Pick>);

/// A click toggles what is under the pointer — a corner, else an edge, else
/// a plane's border, else a flat face, else a body — and a click on nothing
/// clears. On the press, so its gizmos are up by release. Off while the hold's
/// handles are up, so a near-miss on a grip does not change the modelling
/// selection.
fn pick_in_the_viewport(
    mut editor: ResMut<PropEditor>,
    mut hovered: ResMut<HoveredPick>,
    mouse: Res<CurrentMouseInput>,
    grab: Res<GizmoGrab>,
    hold_handles: Res<HoldHandles>,
    build: Res<crate::prop::view::PropBuild>,
    cameras: Query<(&Camera, &GlobalTransform), With<Multicam>>,
) {
    let under = (!hold_handles.0 && grab.held.is_none())
        .then(|| {
            let ray = mouse.world_pos?;
            let (camera, at) = cameras.get(mouse.in_camera?).ok()?;
            let evaluated = &build.evaluated;
            let pointer = Pointer {
                ray,
                face: evaluated.face_distance(ray),
                project: |p: Vec3| camera.world_to_viewport(at, p).ok(),
            };
            let pick = element_under(&evaluated.pickables, &pointer)
                .map(|pickable| Pick::Geom(pickable.reference.clone()))
                .or_else(|| plane_under(&evaluated.planes, &pointer).map(Pick::Plane))
                .or_else(|| {
                    evaluated
                        .surface_under(ray)
                        .map(|(body, face)| face.map_or(Pick::Body(body), Pick::Geom))
                });
            Some(pick)
        })
        .flatten();
    let pointing = under.is_some();
    let under = under.flatten();
    if hovered.0 != under {
        hovered.0 = under.clone();
    }

    if !pointing || !(mouse.just_pressed && mouse.pressed == Some(MouseButton::Left)) {
        return;
    }
    match under {
        Some(pick) => editor.toggle(pick),
        None => editor.clear_selection(),
    }
}

/// Drag the hold's handles: a grip per hand, and the figure for the carry.
///
/// Takes the `ResMut` so a frame that only reads it does not mark it changed.
fn drag_the_hold(
    editor: &mut ResMut<PropEditor>,
    mouse: &CurrentMouseInput,
    grab: &mut GizmoGrab,
    choice: ScaleFigure,
    figures: &Query<(&Skeleton, &Pose, &Transform, &FigureReach), With<ScaleFigureMarker>>,
    gizmos: &mut Gizmos,
) {
    let base = editor.doc().hold.clone();
    let hold = base.for_class(choice.0);
    let figure = figures.iter().next();

    // In the prop's own space, where the weapon sits at the origin.
    let mut points = vec![
        (Handle::Grip { support: false, axis: 0 }, hold.grip()),
        (Handle::Grip { support: false, axis: 1 }, hold.grip()),
        (Handle::Grip { support: false, axis: 2 }, hold.grip()),
    ];
    if let Some(support) = hold.support() {
        for axis in 0..3 {
            points.push((Handle::Grip { support: true, axis }, support));
        }
    }
    let muzzle = editor.doc().muzzle;
    if let Some(muzzle) = muzzle {
        for axis in 0..3 {
            points.push((Handle::Muzzle(axis), muzzle.point()));
        }
    }
    // The carry handle needs a figure standing there.
    let arm = figure.map(|(skeleton, ..)| skeleton.proportions().arm_metres());
    let carry_rotation = quat_from_euler(Vec3::from_array(hold.carry_rotation));
    if let Some(arm) = arm {
        let chest = carry_handle(&hold, carry_rotation, arm);
        for axis in 0..3 {
            points.push((Handle::Carry(axis), chest));
        }
    }

    let Some(ray) = mouse.world_pos else {
        if grab.held.take().is_some() {
            editor.end_gesture();
        }
        draw_hold(gizmos, &points, &hold, figure, None);
        return;
    };
    if mouse.released == Some(MouseButton::Left) || mouse.pressed != Some(MouseButton::Left) {
        if grab.held.take().is_some() {
            editor.end_gesture();
        }
    }

    let nearest = points
        .iter()
        .map(|(handle, at)| {
            (*handle, ray_point_distance(&ray, arrow_tip(*at, handle.axis(), GRIP_LENGTH)))
        })
        .filter(|(_, distance)| *distance < GRIP_PICK_RADIUS)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(handle, _)| handle);

    if grab.held.is_none()
        && mouse.just_pressed
        && mouse.pressed == Some(MouseButton::Left)
        && let Some(handle) = nearest
    {
        let axis = handle.axis();
        let at = points
            .iter()
            .find(|(candidate, _)| *candidate == handle)
            .map(|(_, at)| *at)
            .unwrap_or(Vec3::ZERO);
        if let Some(on_axis) = closest_param_on_axis(ray, Vec3::ZERO, Vec3::AXES[axis]) {
            grab.held = Some((handle, on_axis - at[axis]));
            grab.answered = None;
            editor.begin_gesture();
        }
    }

    if let Some((handle, offset)) = grab.held
        && grab.answered != Some((ray, editor.generation()))
    {
        let axis = handle.axis();
        if let Some(on_axis) = closest_param_on_axis(ray, Vec3::ZERO, Vec3::AXES[axis]) {
            let at = on_axis - offset;
            let mut edited = hold.clone();
            let mut moved_muzzle = muzzle;
            match handle {
                Handle::Grip { support: false, axis } => edited.grip[axis] = at,
                Handle::Grip { support: true, axis } => edited.support[axis] = at,
                Handle::Carry(axis) => {
                    if let Some(arm) = arm {
                        // Inverse of `carry_handle`, back into arm lengths.
                        let mut chest = carry_handle(&hold, carry_rotation, arm);
                        chest[axis] = at;
                        edited.carry = ((carry_rotation * -chest) / arm).to_array();
                    }
                }
                Handle::Muzzle(axis) => {
                    if let Some(muzzle) = moved_muzzle.as_mut() {
                        muzzle.at[axis] = at;
                    }
                }
                Handle::Move(_) | Handle::Face { .. } => {}
            }
            let applied = base.applied(choice.0, &edited);
            // A held button with the pointer still is not an edit.
            if applied != base {
                editor.doc_mut().hold = applied;
            }
            // Not through `applied`: a muzzle belongs to the model, not a class.
            if moved_muzzle != muzzle {
                editor.doc_mut().muzzle = moved_muzzle;
            }
        }
        grab.answered = Some((ray, editor.generation()));
    }

    draw_hold(gizmos, &points, &hold, figure, grab.held.map(|(handle, _)| handle).or(nearest));
}

/// Where the figure's chest ends up for a given carry: `pose_the_figure`
/// inverts the hold, which puts it at `-R⁻¹ · carry`.
fn carry_handle(hold: &HoldSpec, carry_rotation: Quat, arm: f32) -> Vec3 {
    carry_rotation.inverse() * -hold.carry(arm)
}

/// The hold's handles, plus a line from each hand that fell short to its grip.
fn draw_hold(
    gizmos: &mut Gizmos,
    points: &[(Handle, Vec3)],
    hold: &HoldSpec,
    figure: Option<(&Skeleton, &Pose, &Transform, &FigureReach)>,
    lit: Option<Handle>,
) {
    let unreachable = Color::srgb(1.0, 0.45, 0.2);

    let short = |support: bool| {
        figure.is_some_and(|(_, _, _, reach)| {
            reach.0[usize::from(support)] != crate::common::skeleton::ik::Reach::Reached
        })
    };

    for (handle, at) in points {
        let axis = handle.axis();
        let colour = match (lit == Some(*handle), handle) {
            (true, _) => HIGHLIGHT,
            (false, Handle::Grip { support, .. }) if short(*support) => unreachable,
            _ => AXIS_COLOURS[axis],
        };
        gizmos.arrow(*at, arrow_tip(*at, axis, GRIP_LENGTH), colour);
    }

    // Arrows alone would read as a third grip.
    if let Some((_, at)) = points.iter().find(|(handle, _)| matches!(handle, Handle::Muzzle(_))) {
        let colour = match lit {
            Some(Handle::Muzzle(_)) => HIGHLIGHT,
            _ => MUZZLE_COLOUR,
        };
        gizmos.sphere(Isometry3d::from_translation(*at), MUZZLE_MARK_RADIUS, colour);
    }

    if let Some((skeleton, pose, root, reach)) = figure {
        let bones = skeleton.posed_bones(pose, root);
        for (index, (name, target)) in [
            (bone::HAND_R, Some(hold.grip())),
            (bone::HAND_L, hold.support()),
        ]
        .into_iter()
        .enumerate()
        {
            let (Some(target), Some(at)) = (target, skeleton.index_of(name)) else { continue };
            if reach.0[index] == crate::common::skeleton::ik::Reach::Reached {
                continue;
            }
            gizmos.line(bones[at].head, target, unreachable);
        }
    }
}

fn draw(
    gizmos: &mut Gizmos,
    points: &[(Handle, Vec3)],
    world: &impl Fn(Vec3) -> Vec3,
    scale: f32,
    lit: Option<Handle>,
) {
    for (handle, local) in points {
        let colour = match lit == Some(*handle) {
            true => HIGHLIGHT,
            false => AXIS_COLOURS[handle.axis()],
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
            Handle::Grip { .. } | Handle::Carry(_) | Handle::Muzzle(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_shape() -> Shape {
        Shape::Box { size: [0.4, 0.4, 0.4] }
    }

    /// Fails if the origin is not moved, which slides both faces.
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

    /// A cylinder resized by its side must not move off its own axis.
    #[test]
    fn resizing_a_radius_leaves_the_axis_alone() {
        let shape = Shape::Prism { sides: 12, radius: 0.05, height: 0.5 };
        let placement = Placement::at(0.2, 0.0, 0.0);

        for axis in [0, 2] {
            let (resized, moved) = drag_face(&shape, &placement, axis, true, 0.12);
            assert_eq!(moved.origin, placement.origin, "the cylinder slid sideways");
            assert!((resized.half_extents()[axis] - 0.12).abs() < 1e-6);
            assert!((resized.half_extents().y - 0.25).abs() < 1e-6);
        }

        let (resized, moved) = drag_face(&shape, &placement, 1, true, 0.4);
        assert!((resized.half_extents().y - 0.325).abs() < 1e-5);
        assert!(moved.origin()[1] > placement.origin()[1], "the base did not stay put");
    }

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

    /// Fails if arrow pick points tie, which makes every grip arrow pick as X.
    #[test]
    fn the_three_arrows_of_a_handle_can_be_told_apart() {
        let anchor = Vec3::new(0.1, -0.2, 0.3);
        let tips: Vec<Vec3> = (0..3).map(|axis| arrow_tip(anchor, axis, GRIP_LENGTH)).collect();

        for (a, first) in tips.iter().enumerate() {
            for second in &tips[a + 1..] {
                assert!(
                    (*first - *second).length() > GRIP_PICK_RADIUS * 2.0,
                    "two arrows are within each other's pick radius: {first} and {second}",
                );
            }
        }
    }

    /// Fails if a move drag is measured from the arrow's tip, which jumps the
    /// feature on grab.
    #[test]
    fn grabbing_a_move_arrow_does_not_shift_what_it_moves() {
        let scale = gizmo_scale(Some(Vec3::splat(0.3)));
        let points = handles(Some(&box_shape()), scale);
        let (_, tip) = points
            .iter()
            .find(|(handle, _)| *handle == Handle::Move(0))
            .expect("a move arrow per axis");

        assert!(tip.x > 0.0, "the arrow is drawn away from the origin");
        let anchor = 0.0;
        let grabbed_at = tip.x;
        let offset = grabbed_at - anchor;
        assert!((grabbed_at - offset - anchor).abs() < 1e-6);
    }

    /// A sign error here is a carry handle that runs away from the pointer.
    #[test]
    fn a_dragged_chest_inverts_back_into_the_carry_that_put_it_there() {
        let arm = 0.55;
        for rotation in [Vec3::ZERO, Vec3::new(0.2, -0.4, 0.1)] {
            let mut hold = HoldSpec::default();
            hold.carry_rotation = rotation.to_array();
            let turn = quat_from_euler(rotation);

            let chest = carry_handle(&hold, turn, arm);
            let back = ((turn * -chest) / arm).to_array();

            assert!(
                Vec3::from_array(back).abs_diff_eq(Vec3::from_array(hold.carry), 1e-5),
                "a carry of {:?} came back as {back:?}",
                hold.carry,
            );
        }
    }

    #[test]
    fn a_tiny_shape_still_gets_gizmos_big_enough_to_grab() {
        assert!(gizmo_scale(Some(Vec3::splat(MIN_EXTENT))) >= MIN_GIZMO_SCALE);
        assert!(gizmo_scale(None) >= MIN_GIZMO_SCALE);
        assert!(gizmo_scale(Some(Vec3::splat(1.0))) > gizmo_scale(Some(Vec3::splat(0.1))));
    }

    use crate::prop::document::PropDoc;
    use crate::prop::surface::Surface;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::gizmos::config::DefaultGizmoConfigGroup;
    use bevy::gizmos::AppGizmoBuilder;

    /// A world `drag_the_selected` can run in, holding an editor open on one
    /// rotated box — rotated so a re-solved drag would round differently from
    /// frame to frame, which is the noise a pointer held still must not write.
    fn a_world_holding_a_box(hold_handles: bool) -> (World, Placement) {
        let placement = Placement { origin: [0.1, 0.2, 0.3], rotation: [0.3, 0.7, 0.1] };
        let mut doc = PropDoc::new("prop.test");
        let id = doc.push(FeatureOp::Primitive {
            shape: box_shape(),
            placement,
            surface: Surface::default(),
        });
        let mut editor = PropEditor::default();
        editor.open(doc, None);
        editor.select(id);

        // `Gizmos` needs its group registered, which only an `App` can do.
        let mut app = App::new();
        app.init_gizmo_group::<DefaultGizmoConfigGroup>();
        let mut world = std::mem::take(app.world_mut());
        world.insert_resource(editor);
        world.init_resource::<crate::prop::view::PropBuild>();
        world.init_resource::<CurrentMouseInput>();
        world.init_resource::<GizmoGrab>();
        world.insert_resource(HoldHandles(hold_handles));
        world.insert_resource(ScaleFigure(None));
        (world, placement)
    }

    /// One frame with the left button down and the pointer along `ray`.
    fn frame(world: &mut World, ray: Ray3d, just_pressed: bool) -> u64 {
        {
            let mut mouse = world.resource_mut::<CurrentMouseInput>();
            mouse.world_pos = Some(ray);
            mouse.pressed = Some(MouseButton::Left);
            mouse.just_pressed = just_pressed;
            mouse.released = None;
        }
        world.run_system_once(drag_the_selected).unwrap();
        world.resource::<PropEditor>().generation()
    }

    fn release(world: &mut World, ray: Ray3d) {
        {
            let mut mouse = world.resource_mut::<CurrentMouseInput>();
            mouse.world_pos = Some(ray);
            mouse.pressed = None;
            mouse.just_pressed = false;
            mouse.released = Some(MouseButton::Left);
        }
        world.run_system_once(drag_the_selected).unwrap();
    }

    /// A ray straight down onto `at` from across the given axis.
    fn ray_onto(at: Vec3, across: Vec3) -> Ray3d {
        Ray3d::new(at + across * 5.0, Dir3::new(-across).unwrap())
    }

    /// **A move arrow held still writes nothing**, and one that moves does.
    ///
    /// Every frame of a held drag used to write the placement back, so a
    /// mapper pausing mid-drag bumped the generation — and dirtied the
    /// document — sixty times a second for nothing. The press itself is not a
    /// write either, and the whole drag is still one undo step.
    /// **A drag held still answers a document that moved under it.** Undo
    /// mid-drag puts the feature back; the handle is still in the hand, so the
    /// next frame puts the feature under it again rather than waiting for the
    /// mouse to move and then jumping.
    #[test]
    fn a_held_drag_answers_an_undo_under_a_still_pointer() {
        let (mut world, placement) = a_world_holding_a_box(false);
        let rotation = placement.rotation();
        let scale = gizmo_scale(Some(box_shape().half_extents()));
        let tip = handles(Some(&box_shape()), scale)
            .into_iter()
            .find(|(handle, _)| *handle == Handle::Move(0))
            .map(|(_, local)| placement.origin() + rotation * local)
            .unwrap();
        let along = rotation * Vec3::X;
        let across = rotation * Vec3::Z;
        let origin_of = |world: &World| {
            let editor = world.resource::<PropEditor>();
            let op = editor.doc().feature(editor.primary_feature(&Default::default()).unwrap()).unwrap().op.clone();
            let FeatureOp::Primitive { placement, .. } = op else { unreachable!() };
            placement.origin()
        };

        frame(&mut world, ray_onto(tip, across), true);
        let pulled = ray_onto(tip + along * 0.1, across);
        frame(&mut world, pulled, false);
        let dragged = origin_of(&world);

        // Somebody else moves the document: the feature goes back.
        {
            let mut editor = world.resource_mut::<PropEditor>();
            let selected = editor.primary_feature(&Default::default()).unwrap();
            editor.doc_mut().feature_mut(selected).unwrap().op.placement_mut().unwrap().origin =
                placement.origin;
        }
        assert!((origin_of(&world) - placement.origin()).length() < 1e-6);

        frame(&mut world, pulled, false);
        assert!(
            (origin_of(&world) - dragged).length() < 1e-4,
            "the drag waited for the mouse: {} rather than {dragged}",
            origin_of(&world),
        );
    }

    #[test]
    fn a_move_arrow_held_still_writes_nothing() {
        let (mut world, placement) = a_world_holding_a_box(false);
        let rotation = placement.rotation();
        let scale = gizmo_scale(Some(box_shape().half_extents()));
        let tip = handles(Some(&box_shape()), scale)
            .into_iter()
            .find(|(handle, _)| *handle == Handle::Move(0))
            .map(|(_, local)| placement.origin() + rotation * local)
            .unwrap();
        let along = rotation * Vec3::X;
        let across = rotation * Vec3::Z;

        let before = world.resource::<PropEditor>().generation();
        let pressed = frame(&mut world, ray_onto(tip, across), true);
        assert_eq!(pressed, before, "pressing a handle wrote the document");
        for _ in 0..10 {
            frame(&mut world, ray_onto(tip, across), false);
        }
        assert_eq!(
            world.resource::<PropEditor>().generation(),
            before,
            "a drag held still kept writing",
        );

        let pulled = ray_onto(tip + along * 0.1, across);
        let moved = frame(&mut world, pulled, false);
        assert!(moved > before, "a moving drag did not write");
        for _ in 0..10 {
            frame(&mut world, pulled, false);
        }
        assert_eq!(
            world.resource::<PropEditor>().generation(),
            moved,
            "a drag held still after moving kept writing",
        );

        release(&mut world, pulled);
        let mut editor = world.resource_mut::<PropEditor>();
        let selected = editor.primary_feature(&Default::default()).unwrap();
        let landed = editor.doc().feature(selected).unwrap().op.clone();
        let FeatureOp::Primitive { placement: landed, .. } = landed else { unreachable!() };
        assert!(
            (landed.origin() - (placement.origin() + along * 0.1)).length() < 1e-4,
            "the drag did not land: {:?}",
            landed.origin,
        );
        assert!(editor.can_undo(), "the drag left nothing to undo");
        editor.undo();
        assert!(!editor.can_undo(), "the drag was more than one undo step");
    }

    /// A press on a handle that is let go without moving records nothing.
    #[test]
    fn a_handle_pressed_and_let_go_is_not_an_edit() {
        let (mut world, placement) = a_world_holding_a_box(false);
        let rotation = placement.rotation();
        let scale = gizmo_scale(Some(box_shape().half_extents()));
        let face = handles(Some(&box_shape()), scale)
            .into_iter()
            .find(|(handle, _)| *handle == Handle::Face { axis: 0, max: true })
            .map(|(_, local)| placement.origin() + rotation * local)
            .unwrap();
        let ray = ray_onto(face, rotation * Vec3::Z);

        let before = world.resource::<PropEditor>().generation();
        for frame_number in 0..10 {
            frame(&mut world, ray, frame_number == 0);
        }
        release(&mut world, ray);

        let editor = world.resource::<PropEditor>();
        assert_eq!(editor.generation(), before, "a still press wrote the document");
        assert!(!editor.can_undo(), "a still press left an undo step");
    }

    /// The hold's handles keep the same rule: a grip held still is not an edit.
    #[test]
    fn a_grip_held_still_writes_nothing() {
        let (mut world, _) = a_world_holding_a_box(true);
        let hold = world.resource::<PropEditor>().doc().hold.for_class(None);
        let tip = arrow_tip(hold.grip(), 0, GRIP_LENGTH);

        let before = world.resource::<PropEditor>().generation();
        for frame_number in 0..10 {
            frame(&mut world, ray_onto(tip, Vec3::Z), frame_number == 0);
        }
        assert_eq!(
            world.resource::<PropEditor>().generation(),
            before,
            "a grip held still wrote the document",
        );

        let pulled = ray_onto(tip + Vec3::X * 0.05, Vec3::Z);
        let moved = frame(&mut world, pulled, false);
        assert!(moved > before, "a moving grip did not write");
        for _ in 0..10 {
            frame(&mut world, pulled, false);
        }
        assert_eq!(world.resource::<PropEditor>().generation(), moved);

        release(&mut world, pulled);
        assert!(world.resource::<PropEditor>().can_undo());
    }
}
