//! The whole roster running through every animation together: one body per [`Class`] in a line,
//! all holding the same state for [`HOLD`] seconds, then the next. Comparing builds across time
//! rather than laying states out in space keeps it to one readable line.
//!
//! The order is a shuffle seeded from the shared clock, so every viewer sees the same animation at
//! the same moment. Bodies are spawned by [`crate::tool::animation_grid`] and animated by the same
//! systems as a player.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use std::sync::OnceLock;

use crate::common::class::Class;
use crate::common::team::Team;
use crate::common::skeleton::{
    default_humanoid, finish_pose, AnimationState, PoseInputs,
};
use crate::common::PointResolutionError;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::get;

/// Marks the entity an [`AnimationGrid`] drives.
#[derive(Component, Debug)]
pub struct AnimationGridMarker;

/// What side the roster is on. Striped puts one body on each team and the rest on none, each of
/// those in a color of its own, which makes the grid a friendly-fire range; one team makes it a
/// plain firing range.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RosterTeam {
    #[default]
    Striped,
    One(Team),
}

impl RosterTeam {
    /// In panel order.
    pub fn choices() -> Vec<RosterTeam> {
        std::iter::once(RosterTeam::Striped)
            .chain(Team::ALL.map(RosterTeam::One))
            .collect()
    }

    pub fn name(self) -> String {
        match self {
            RosterTeam::Striped => get!("editor.features.animation_grid.striped"),
            RosterTeam::One(team) => team.name(),
        }
    }

    /// By cell rather than rolled, so every viewer agrees on who may shoot whom.
    pub fn team_for(self, cell: usize) -> Option<Team> {
        match self {
            RosterTeam::Striped => Team::ALL.get(cell).copied(),
            RosterTeam::One(team) => Some(team),
        }
    }
}

/// Clear floor either side of a body, on top of its own width, in meters.
#[cfg(test)]
const COLUMN_MARGIN: f32 = 0.3;

/// Meters between bodies. Fixed rather than measured off the builds, which
/// are pack content the layout would otherwise have to be handed; a test
/// checks the widest shipped build plus [`COLUMN_MARGIN`] fits.
pub const COLUMN_SPACING: f32 = 2.33;

/// Clear floor between rows, on top of the stride, in meters.
const ROW_MARGIN: f32 = 0.4;

/// Walking, running and sprinting, in leg-lengths per second. The upright gait changes shape with
/// speed; see [`crate::common::skeleton::GaitShape::for_speed`].
const DISPLAY_SPEEDS: [f32; 3] = [2.0, 4.5, 7.5];

/// A state and the speed to show it at, since some states look different at different speeds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridRow {
    pub state: AnimationState,
    /// Leg-lengths per second. Zero for states that do not move.
    pub speed: f32,
}

/// Written out rather than from [`AnimationState::iter`] so related states stand together; the enum's
/// order is fixed by its on-disk indices. `every_state_has_a_row` catches a state left out.
pub fn rows() -> Vec<GridRow> {
    let mut rows = vec![GridRow { state: AnimationState::Idle, speed: 0.0 }];

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
        AnimationState::CrouchAirborne,
        AnimationState::Crouch,
        AnimationState::CrouchWalk,
        AnimationState::CrouchWalkBackward,
        AnimationState::CrouchStrafeLeft,
        AnimationState::CrouchStrafeRight,
    ] {
        let speed = if state.uses_gait() { DISPLAY_SPEEDS[1] } else { 0.0 };
        rows.push(GridRow { state, speed });
    }

    rows
}

/// Seconds each animation is held.
pub const HOLD: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Showing {
    pub row: GridRow,
    /// Which shuffle this is; the order follows from it.
    pub cycle: u64,
    pub slot: usize,
}

/// A function of the shared clock only, so every viewer agrees without being told.
pub fn showing(seconds: f32) -> Showing {
    let rows = rows();
    let period = rows.len() as f32 * HOLD;
    let seconds = seconds.max(0.0);

    let cycle = (seconds / period).floor();
    let slot = (((seconds - cycle * period) / HOLD).floor() as usize).min(rows.len() - 1);
    let cycle = cycle as u64;

    Showing { row: order(cycle)[slot], cycle, slot }
}

/// Deterministic per rotation, so every machine sees the same order, as with [`AnimationPhase`].
///
/// [`AnimationPhase`]: crate::common::skeleton::AnimationPhase
pub fn order(cycle: u64) -> Vec<GridRow> {
    let mut rows = rows();
    let mut seed = cycle;

    // Fisher-Yates from an integer hash, so every machine draws the same numbers.
    for index in (1..rows.len()).rev() {
        seed = mixed(seed);
        rows.swap(index, (seed % (index as u64 + 1)) as usize);
    }
    rows
}

/// SplitMix64's finalizer: integer-only, so every machine agrees, and it scatters consecutive seeds.
fn mixed(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One body's place in the line. What it is doing is the same for all; see [`showing`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridCell {
    pub class: Class,
    /// Feet, relative to the grid's point.
    pub offset: Vec3,
}

/// Front-to-back floor a body needs, measured across the stride cycle since the deepest moment is
/// not an obvious one.
pub fn body_depth() -> f32 {
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
                    ..default()
                };
                let pose = finish_pose(skeleton, row.state, &inputs, &Transform::IDENTITY, None);
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

/// One body per class, centered on the grid's point.
pub fn cells() -> Vec<GridCell> {
    let classes: Vec<Class> = Class::iter().collect();
    let columns = COLUMN_SPACING;
    let center = (classes.len() as f32 - 1.0) * 0.5;

    classes
        .iter()
        .enumerate()
        .map(|(column, class)| GridCell {
            class: *class,
            offset: Vec3::new((column as f32 - center) * columns, 0.0, 0.0),
        })
        .collect()
}

/// The floor the grid covers, relative to its point. The new-map room is sized from this.
pub fn footprint() -> (Vec3, Vec3) {
    let cells = cells();
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    let half = Vec3::new(COLUMN_SPACING, 0.0, body_depth()) * 0.5;
    for cell in &cells {
        min = min.min(cell.offset - half);
        max = max.max(cell.offset + half);
    }
    (min, max)
}

#[derive(Serialize, Deserialize)]
pub struct AnimationGrid {
    /// The feet of the middle of the line.
    location: PointRef,
    /// Radians about world Y. The grid turns with the bodies.
    yaw: f32,
    /// `serde(default)` so an older grid loads striped.
    #[serde(default)]
    teams: RosterTeam,
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

        egui::ComboBox::from_label(get!("editor.features.animation_grid.teams"))
            .selected_text(self.teams.name())
            .show_ui(ui, |ui| {
                for choice in RosterTeam::choices() {
                    if ui.selectable_label(self.teams == choice, choice.name()).clicked()
                        && self.teams != choice
                    {
                        self.teams = choice;
                        changed = true;
                    }
                }
            });

        // Shown because a grid is bigger than most rooms.
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

    /// The point, the facing, and the floor it will cover; the bodies draw themselves.
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

    /// One per body, not the footprint, which would hide anything placed between them.
    fn pick_boxes(&self) -> Vec<crate::common::ray::PickBox> {
        let rotation = Quat::from_rotation_y(self.yaw);
        cells()
            .iter()
            .map(|cell| crate::common::ray::PickBox::standing_body(self.resolved_location + rotation * cell.offset, rotation))
            .collect()
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    /// The bodies are spawned by [`crate::tool::animation_grid::sync_animation_grids`]; spawning them
    /// here would respawn them on every edit.
    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        commands
            .entity(entity)
            .insert((
                Transform::from_translation(self.resolved_location)
                    .with_rotation(Quat::from_rotation_y(self.yaw)),
                AnimationGridMarker,
                // Re-inserted on every edit, so it reads as changed every frame of a drag; `team_carousels`
                // compares before writing.
                self.teams,
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
            teams: RosterTeam::default(),
            resolved_location: Vec3::new(x, y, z),
            entity: None,
        }
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            yaw: 0.0,
            teams: RosterTeam::default(),
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

    pub fn with_teams(mut self, teams: RosterTeam) -> Self {
        self.teams = teams;
        self
    }

    pub fn teams(&self) -> RosterTeam {
        self.teams
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::{humanoid, Pose};

    #[test]
    fn the_line_is_one_body_per_class() {
        let cells = cells();
        assert_eq!(cells.len(), Class::iter().count());

        for class in Class::iter() {
            assert_eq!(
                cells.iter().filter(|cell| cell.class == class).count(),
                1,
                "{class:?} is not on the line exactly once"
            );
        }
        assert!(cells.iter().all(|cell| cell.offset.z == 0.0), "the line is not a line");
    }

    /// Catches a state added and never shown, since the rows are written by hand.
    #[test]
    fn every_state_has_a_row() {
        for state in AnimationState::iter() {
            assert!(
                rows().iter().any(|row| row.state == state),
                "{state:?} is never shown"
            );
        }
    }

    #[test]
    fn a_gait_that_changes_with_speed_is_shown_at_several() {
        use crate::common::skeleton::GaitShape;

        let speeds: Vec<f32> = rows()
            .iter()
            .filter(|row| row.state == AnimationState::RunForward)
            .map(|row| row.speed)
            .collect();
        assert!(speeds.len() > 1, "the run is only shown at one speed");

        // At least one walks and at least one runs.
        assert!(speeds.iter().any(|speed| !GaitShape::for_speed(*speed).has_flight()));
        assert!(speeds.iter().any(|speed| GaitShape::for_speed(*speed).has_flight()));

        assert!(rows().iter().all(|row| row.state.uses_gait() || row.speed == 0.0));
    }

    #[test]
    fn a_rotation_shows_everything_exactly_once() {
        for cycle in 0..64 {
            let order = order(cycle);
            assert_eq!(order.len(), rows().len());

            for row in rows() {
                assert_eq!(
                    order.iter().filter(|shown| **shown == row).count(),
                    1,
                    "{row:?} appears the wrong number of times on rotation {cycle}"
                );
            }
        }
    }

    #[test]
    fn the_shuffle_is_the_same_for_everyone() {
        for cycle in 0..16 {
            assert_eq!(order(cycle), order(cycle), "the order is not reproducible");
        }

        let orders: Vec<Vec<GridRow>> = (0..32).map(order).collect();
        assert!(orders[0] != orders[1], "two rotations in a row are identical");

        let mut distinct = orders.clone();
        distinct.dedup();
        assert!(distinct.len() > 28, "the shuffle repeats itself: {} of 32", distinct.len());
    }

    #[test]
    fn each_animation_is_held_for_its_turn() {
        let rows = rows();
        let period = rows.len() as f32 * HOLD;

        for slot in 0..rows.len() {
            let start = slot as f32 * HOLD;
            let expected = order(0)[slot];

            for moment in [start + 0.01, start + HOLD * 0.5, start + HOLD - 0.01] {
                let showing = showing(moment);
                assert_eq!(showing.row, expected, "at {moment:.2}s");
                assert_eq!(showing.slot, slot);
                assert_eq!(showing.cycle, 0);
            }
        }

        let next = showing(period + 0.01);
        assert_eq!(next.cycle, 1);
        assert_eq!(next.slot, 0);
        assert_eq!(next.row, order(1)[0]);
    }

    fn body_width(class: Class) -> f32 {
        let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));

        skeleton
            .posed_bones(&Pose::rest(), &Transform::IDENTITY)
            .iter()
            .map(|bone| bone.head.x.abs().max(bone.tail.x.abs()) + bone.thickness.x * 0.5)
            .fold(0.0_f32, f32::max)
            * 2.0
    }

    #[test]
    fn neighboring_bodies_do_not_overlap() {
        let widest = Class::iter().map(body_width).fold(0.0_f32, f32::max);
        assert!(
            COLUMN_SPACING >= widest + COLUMN_MARGIN,
            "columns are {COLUMN_SPACING} apart for bodies {widest} wide"
        );

        let places: Vec<f32> = cells().iter().map(|cell| cell.offset.x).collect();
        for pair in places.windows(2) {
            assert!((pair[1] - pair[0]) >= widest, "two bodies are {} apart", pair[1] - pair[0]);
        }
    }

    /// A stride reaching past its own patch would reach into the next one.
    #[test]
    fn a_stride_stays_on_its_own_patch() {
        let skeleton = default_humanoid();
        for row in rows() {
            for step in 0..8 {
                let inputs = PoseInputs {
                    seconds: step as f32 * 0.4,
                    stride: step as f32 / 8.0,
                    speed: row.speed,
                    ..default()
                };
                let pose = finish_pose(skeleton, row.state, &inputs, &Transform::IDENTITY, None);
                let deepest = skeleton
                    .posed_bones(&pose, &Transform::IDENTITY)
                    .iter()
                    .map(|bone| bone.head.z.abs().max(bone.tail.z.abs()))
                    .fold(0.0_f32, f32::max);

                assert!(
                    deepest * 2.0 <= body_depth(),
                    "{:?} reaches {:.2} m front to back, on {:.2} m of floor",
                    row.state,
                    deepest * 2.0,
                    body_depth()
                );
            }
        }
    }

    #[test]
    fn the_line_is_centered_on_its_point() {
        let places: Vec<f32> = cells().iter().map(|cell| cell.offset.x).collect();
        let mean: f32 = places.iter().sum::<f32>() / places.len() as f32;
        assert!(mean.abs() < 1e-4, "the line is off-center by {mean}");
    }
}
