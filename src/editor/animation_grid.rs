//! Every class in every animation state, laid out on the floor.
//!
//! One placed point turns into a body per cell: a column per [`Class`] and a
//! row per [`AnimationState`]. That is the whole reason it is a grid rather
//! than ten separate displays — an animation that reads well on one build and
//! badly on another is only visible with the two side by side, and a state
//! that was never given a clip is only obvious next to the ones that were.
//!
//! It states no animation of its own: adding a state adds a row to every grid
//! already placed on every map. A grid that had to be told which states exist
//! would be a grid that silently stopped covering them.
//!
//! The bodies are ordinary skeleton entities, spawned as children by
//! [`crate::tool::animation_grid`] and animated by the same systems that
//! animate a player.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use std::sync::OnceLock;

use crate::common::class::Class;
use crate::common::skeleton::{
    default_humanoid, finish_pose, humanoid, AnimationState, Pose, PoseInputs,
};
use crate::common::PointResolutionError;
use crate::editor::action::FeatureData;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::get;

/// Marks the entity an [`AnimationGrid`] drives, so the bodies can be hung off
/// it without the feature reaching past `Commands` — the same arrangement
/// `Prop` has with `PropMarker`.
#[derive(Component, Debug)]
pub struct AnimationGridMarker;

/// Clear floor either side of a body, on top of its own width.
const COLUMN_MARGIN: f32 = 0.3;

/// Clear floor between one row of bodies and the next, on top of how far they
/// stride.
const ROW_MARGIN: f32 = 0.4;

/// Walking, running and sprinting, in leg-lengths per second.
///
/// The upright gait changes shape with speed — the stance fraction falls, the
/// stride grows, and somewhere in between the walk becomes a run — so one row
/// of it would only ever show one of those. See
/// [`crate::common::skeleton::GaitShape::for_speed`].
const DISPLAY_SPEEDS: [f32; 3] = [2.0, 4.5, 7.5];

/// One row of the grid: a state, and the speed to show it at.
///
/// A row is not simply a state, because some states look like two different
/// animations at two different speeds and there is no use in a grid that shows
/// one of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridRow {
    pub state: AnimationState,
    /// In leg-lengths per second. Zero for the states that do not move.
    pub speed: f32,
}

/// Every row, in the order they are read.
///
/// Written out rather than taken from [`AnimationState::iter`] so that related
/// states stand next to each other: the enum's own order is the order states
/// were added, since its indices are on disk and cannot be shuffled.
/// `every_state_has_a_row` is what stops a state being added and quietly never
/// shown.
pub fn rows() -> Vec<GridRow> {
    let mut rows = vec![GridRow { state: AnimationState::Idle, speed: 0.0 }];

    // The one state worth seeing three times.
    rows.extend(DISPLAY_SPEEDS.map(|speed| GridRow {
        state: AnimationState::RunForward,
        speed,
    }));

    for state in [
        AnimationState::RunBackward,
        AnimationState::StrafeLeft,
        AnimationState::StrafeRight,
        AnimationState::PushingWall,
        AnimationState::Airborne,
        AnimationState::Crouch,
        AnimationState::CrouchWalk,
        AnimationState::CrouchWalkBackward,
        AnimationState::CrouchStrafeLeft,
        AnimationState::CrouchStrafeRight,
    ] {
        // A running pace for the ones that move, and nothing for the ones that
        // do not — a body standing still at speed would be a lie the gait
        // would then have to animate.
        let speed = if state.uses_gait() { DISPLAY_SPEEDS[1] } else { 0.0 };
        rows.push(GridRow { state, speed });
    }

    rows
}

/// One body's place in the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridCell {
    pub class: Class,
    pub state: AnimationState,
    /// How fast this body is pretending to move, in leg-lengths per second.
    pub speed: f32,
    /// Where this body's feet go, relative to the grid's point.
    pub offset: Vec3,
}

/// How far apart columns stand.
///
/// Measured off the widest body rather than typed in, so a class that is given
/// broader shoulders later pushes the grid apart instead of overlapping its
/// neighbour. An A-pose is the widest a rest pose gets, which is what makes
/// this measurable at all.
pub fn column_spacing() -> f32 {
    static SPACING: OnceLock<f32> = OnceLock::new();
    *SPACING.get_or_init(|| {
        Class::iter().map(body_width).fold(0.0_f32, f32::max) + COLUMN_MARGIN
    })
}

/// How far apart rows stand.
///
/// Measured off how far a body actually reaches front to back while it is
/// moving, rather than a number picked to look right at the time: rows are
/// read from in front, and a body whose stride overlapped the row ahead of it
/// would be one you could not judge. Sampled across the cycle because the
/// deepest moment of a stride is not the moment anyone thinks to check.
pub fn row_spacing() -> f32 {
    static SPACING: OnceLock<f32> = OnceLock::new();
    *SPACING.get_or_init(|| {
        let skeleton = default_humanoid();
        let deepest = rows()
            .iter()
            .flat_map(|row| (0..8).map(move |step| (row, step)))
            .map(|(row, step)| {
                let inputs = PoseInputs {
                    seconds: step as f32 * 0.4,
                    stride: step as f32 / 8.0,
                    speed: row.speed,
                };
                let pose = finish_pose(skeleton, row.state, &inputs, &Transform::IDENTITY);
                skeleton
                    .posed_bones(&pose, &Transform::IDENTITY)
                    .iter()
                    .map(|bone| bone.head.z.abs().max(bone.tail.z.abs()))
                    .fold(0.0_f32, f32::max)
            })
            .fold(0.0_f32, f32::max);

        deepest * 2.0 + ROW_MARGIN
    })
}

/// How wide a class's body is across, in its rest pose.
fn body_width(class: Class) -> f32 {
    let proportions = class.proportions();
    let owned;
    let skeleton = if proportions == default_humanoid().proportions() {
        default_humanoid()
    } else {
        owned = humanoid(proportions);
        &owned
    };

    skeleton
        .posed_bones(&Pose::rest(), &Transform::IDENTITY)
        .iter()
        .map(|bone| bone.head.x.abs().max(bone.tail.x.abs()) + bone.thickness.x * 0.5)
        .fold(0.0_f32, f32::max)
        * 2.0
}

/// Every body the grid stands up, in reading order: a row per state, a column
/// per class, columns centred on the grid's own point.
pub fn cells() -> Vec<GridCell> {
    let classes: Vec<Class> = Class::iter().collect();
    let rows = rows();
    let (columns, depth) = (column_spacing(), row_spacing());
    // Centred, so moving the point moves the middle of the grid rather than
    // its left edge — a grid placed in a room should be placed in the middle
    // of it.
    let centre = (classes.len() as f32 - 1.0) * 0.5;

    let mut cells = Vec::with_capacity(classes.len() * rows.len());
    for (index, row) in rows.iter().enumerate() {
        for (column, class) in classes.iter().enumerate() {
            cells.push(GridCell {
                class: *class,
                state: row.state,
                speed: row.speed,
                // Rows recede away from the front, which is the side the
                // bodies face and therefore the side they are read from.
                offset: Vec3::new(
                    (column as f32 - centre) * columns,
                    0.0,
                    index as f32 * depth,
                ),
            });
        }
    }
    cells
}

/// The floor the grid covers, relative to its own point.
///
/// A grid is tens of metres across, and both a mapper placing one and the
/// template that ships one want to know that before the bodies appear — the
/// new-map room is sized from this rather than guessed at, so the room grows
/// when the roster does.
pub fn footprint() -> (Vec3, Vec3) {
    let cells = cells();
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    let half = Vec3::new(column_spacing(), 0.0, row_spacing()) * 0.5;
    for cell in &cells {
        min = min.min(cell.offset - half);
        max = max.max(cell.offset + half);
    }
    (min, max)
}

/// A place to stand the whole roster.
#[derive(Serialize, Deserialize)]
pub struct AnimationGrid {
    /// The **feet** of the middle of the front row, so the point sits where a
    /// mapper would stand to look at the grid.
    location: PointRef,
    /// Which way the bodies face, in radians about world Y. The grid turns
    /// with them.
    yaw: f32,
    #[serde(skip)]
    resolved_location: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

#[typetag::serde(name = "animation_grid")]
impl FeatureTrait for AnimationGrid {
    fn get_point(&self, _key: &str) -> Result<Vec3, PointResolutionError> {
        Ok(self.resolved_location)
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let mut changed = false;
        let label = get!("editor.features.animation_grid.feet");
        changed |= self.location.editor_ui(ui, &label, features, prior_feature_order, retarget_request);

        let mut degrees = self.yaw.to_degrees();
        if ui
            .add(
                egui::DragValue::new(&mut degrees)
                    .speed(1.0)
                    .prefix(format!("{}: ", get!("editor.features.animation_grid.facing")))
                    .suffix("\u{b0}"),
            )
            .changed()
        {
            self.yaw = degrees.to_radians();
            changed = true;
        }

        // What it is about to put on the floor, in numbers: the answer is
        // sixty bodies over eighteen metres by twelve, which is bigger than
        // most rooms and worth knowing before placing one rather than after.
        let (min, max) = footprint();
        let size = max - min;
        ui.label(get!(
            "editor.features.animation_grid.summary",
            "bodies",
            cells().len().to_string(),
            "width",
            format!("{:.1}", size.x),
            "depth",
            format!("{:.1}", size.z)
        ));

        if changed {
            if let Ok(v) = self.location.resolve(features) {
                self.resolved_location = v;
            }
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.animation_grid.title")
    }

    fn type_key(&self) -> &'static str { "animation_grid" }

    fn snapshot(&self) -> FeatureData {
        FeatureData::AnimationGrid {
            location: self.location.clone(),
            yaw: self.yaw,
        }
    }

    fn apply_snapshot(&mut self, data: &FeatureData) {
        let FeatureData::AnimationGrid { location, yaw } = data else { return; };
        self.location = location.clone();
        self.yaw = *yaw;
    }

    /// The point, the facing, and the floor it will cover.
    ///
    /// The bodies are entities and draw themselves; this draws the feature,
    /// which is a placed point with a large footprint.
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let feet = self.resolved_location;
        let rotation = Quat::from_rotation_y(self.yaw);
        let green = Color::srgb_u8(150, 230, 150);

        gizmos.sphere(Isometry3d::from_translation(feet), 0.2, green);
        gizmos.arrow(feet, feet + rotation * Vec3::NEG_Z, green);

        let (min, max) = footprint();
        let corners = [
            Vec3::new(min.x, 0.0, min.z),
            Vec3::new(max.x, 0.0, min.z),
            Vec3::new(max.x, 0.0, max.z),
            Vec3::new(min.x, 0.0, max.z),
        ];
        for i in 0..4 {
            gizmos.line(
                feet + rotation * corners[i],
                feet + rotation * corners[(i + 1) % 4],
                green,
            );
        }

        self.location.debug_gizmos(feet, gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    /// Marks the entity and turns it; the bodies are hung off it by
    /// [`crate::tool::animation_grid::sync_animation_grids`].
    ///
    /// Spawning them here would mean spawning them again on every edit, since
    /// this runs each time the feature changes — sixty more bodies for every
    /// nudge of the point.
    fn apply_to_entity(&self, commands: &mut Commands, entity: Entity) {
        commands.entity(entity).insert((
            Transform::from_translation(self.resolved_location)
                .with_rotation(Quat::from_rotation_y(self.yaw)),
            AnimationGridMarker,
        ));
    }

    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>) {
        if let Ok(v) = self.location.resolve(features) {
            self.resolved_location = v;
        }
    }

    fn parent_ids(&self) -> Vec<FeatureId> {
        self.location.referenced_features()
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        vec![("".into(), get!("editor.features.animation_grid.feet"))]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![("".into(), self.resolved_location)]
    }

    fn point_ref_slots(&self) -> Vec<&str> { vec!["location"] }

    fn scalar_fields(&self) -> Vec<(&str, f32)> {
        vec![("yaw", self.yaw)]
    }

    fn set_scalar_field(&mut self, key: &str, value: f32) {
        if key == "yaw" {
            self.yaw = value;
        }
    }

    fn rotation_axes(&self) -> Vec<u8> { vec![1] }

    fn euler_angles(&self) -> Vec3 { Vec3::new(0.0, self.yaw, 0.0) }

    fn set_euler_angle(&mut self, axis: u8, radians: f32) -> bool {
        if axis != 1 {
            return false;
        }
        self.yaw = radians;
        true
    }

    fn get_point_ref(&self, _key: &str) -> Option<&PointRef> {
        Some(&self.location)
    }

    fn get_point_ref_mut(&mut self, _key: &str) -> Option<&mut PointRef> {
        Some(&mut self.location)
    }

    fn drag_handle(&mut self, _is_max: bool, axis: u8, new_world_value: f32) -> bool {
        let axis_ref = match axis {
            0 => &mut self.location.x,
            1 => &mut self.location.y,
            2 => &mut self.location.z,
            _ => return false,
        };
        let base = self.location.resolved_reference.map(|b| match axis {
            0 => b.x, 1 => b.y, _ => b.z,
        });
        match axis_ref {
            AxisRef::Absolute(v) => *v = new_world_value,
            AxisRef::Relative(offset) => *offset = new_world_value - base.unwrap_or(0.0),
        }
        match axis {
            0 => self.resolved_location.x = new_world_value,
            1 => self.resolved_location.y = new_world_value,
            _ => self.resolved_location.z = new_world_value,
        }
        true
    }
}

impl AnimationGrid {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self {
            location: PointRef::absolute(x, y, z),
            yaw: 0.0,
            resolved_location: Vec3::new(x, y, z),
            entity: None,
        }
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            yaw: 0.0,
            resolved_location: Vec3::ZERO,
            entity: None,
        }
    }

    pub fn with_yaw(mut self, yaw: f32) -> Self {
        self.yaw = yaw;
        self
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every class in every row, once each. A grid that quietly dropped a
    /// combination would be a grid you could not trust to have shown you the
    /// problem.
    #[test]
    fn the_grid_covers_every_class_in_every_row() {
        let cells = cells();
        assert_eq!(cells.len(), Class::iter().count() * rows().len());

        for class in Class::iter() {
            for row in rows() {
                let matches = cells
                    .iter()
                    .filter(|cell| {
                        cell.class == class && cell.state == row.state && cell.speed == row.speed
                    })
                    .count();
                assert_eq!(matches, 1, "{class:?} in {row:?} appears {matches} times");
            }
        }
    }

    /// The rows are written out by hand so that related states stand together,
    /// which is exactly the arrangement that lets one be forgotten. Adding a
    /// state and not showing it is the failure this catches.
    #[test]
    fn every_state_has_a_row() {
        for state in AnimationState::iter() {
            assert!(
                rows().iter().any(|row| row.state == state),
                "{state:?} is not shown anywhere on the grid"
            );
        }
    }

    /// The states that look like two animations at two speeds get shown at
    /// both, which is the whole reason a row is a state *and* a speed.
    #[test]
    fn a_gait_that_changes_with_speed_is_shown_at_several() {
        use crate::common::skeleton::GaitShape;

        let speeds: Vec<f32> = rows()
            .iter()
            .filter(|row| row.state == AnimationState::RunForward)
            .map(|row| row.speed)
            .collect();
        assert!(speeds.len() > 1, "the run is only shown at one speed");

        // And they are far enough apart to be different gaits, not three rows
        // of the same one: at least one walks and at least one runs.
        assert!(speeds.iter().any(|speed| !GaitShape::for_speed(*speed).has_flight()));
        assert!(speeds.iter().any(|speed| GaitShape::for_speed(*speed).has_flight()));

        // A body that is not going anywhere is shown standing still, rather
        // than running on the spot.
        assert!(rows()
            .iter()
            .all(|row| row.state.uses_gait() || row.speed == 0.0));
    }

    /// Bodies stand in an A-pose, which is the widest a rest pose gets. If the
    /// columns are packed tighter than that, the arms overlap and the grid is
    /// unreadable exactly where it is meant to be useful.
    #[test]
    fn neighbouring_bodies_do_not_overlap() {
        let widest = Class::iter().map(body_width).fold(0.0_f32, f32::max);
        assert!(
            column_spacing() > widest,
            "columns are {} apart for bodies {widest} wide",
            column_spacing()
        );

        // And the arithmetic that turns that spacing into positions has to
        // keep the gap: a centring bug would show up here and nowhere else.
        let front: Vec<f32> = cells()
            .iter()
            .filter(|cell| cell.offset.z == 0.0)
            .map(|cell| cell.offset.x)
            .collect();
        for pair in front.windows(2) {
            assert!(
                (pair[1] - pair[0]) >= widest,
                "two bodies {} apart are {widest} wide",
                pair[1] - pair[0]
            );
        }
    }

    /// Rows are far enough apart that a body's stride does not reach into the
    /// row ahead of it — which is the whole of what makes a grid readable from
    /// in front.
    #[test]
    fn a_stride_does_not_reach_the_row_in_front() {
        use crate::common::skeleton::{finish_pose, PoseInputs};

        let skeleton = default_humanoid();
        for row in rows() {
            for step in 0..8 {
                let inputs = PoseInputs {
                    seconds: step as f32 * 0.4,
                    stride: step as f32 / 8.0,
                    speed: row.speed,
                };
                let pose = finish_pose(skeleton, row.state, &inputs, &Transform::IDENTITY);
                let deepest = skeleton
                    .posed_bones(&pose, &Transform::IDENTITY)
                    .iter()
                    .map(|bone| bone.head.z.abs().max(bone.tail.z.abs()))
                    .fold(0.0_f32, f32::max);

                assert!(
                    deepest * 2.0 < row_spacing(),
                    "{:?} reaches {:.2} m front to back, in rows {:.2} m apart",
                    row.state,
                    deepest * 2.0,
                    row_spacing()
                );
            }
        }
    }

    /// The point is the middle of the front row, not a corner: a grid is
    /// placed by standing where you will look at it from.
    #[test]
    fn the_grid_is_centred_on_its_point() {
        let front: Vec<Vec3> = cells()
            .iter()
            .filter(|cell| cell.offset.z == 0.0)
            .map(|cell| cell.offset)
            .collect();

        let mean: f32 = front.iter().map(|offset| offset.x).sum::<f32>() / front.len() as f32;
        assert!(mean.abs() < 1e-4, "the front row is off-centre by {mean}");
        assert!(front.iter().all(|offset| offset.z.abs() < 1e-6), "the front row is not at the point");
        // Rows go away from the viewer, never towards.
        assert!(cells().iter().all(|cell| cell.offset.z >= 0.0));
    }
}



