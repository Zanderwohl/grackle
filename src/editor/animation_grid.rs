//! The whole roster, running through every animation together.
//!
//! One placed point turns into one body per [`Class`], standing in a line — and
//! the second axis of the grid is *time* rather than floor. All ten hold the
//! same animation at the same moment, for two seconds, and then move on to the
//! next; when the list runs out it is shuffled and starts again.
//!
//! Laying the same thing out in space instead meant a body per class per state,
//! which was a hundred and forty of them across a field forty metres deep. Ten
//! is a line you can stand in front of and read, and a comparison across time
//! is the one that matters anyway: an animation that reads well on one build
//! and badly on another shows it in the moment they are both doing it.
//!
//! The order is shuffled so that neighbouring states are not always seen next
//! to each other — but shuffled from the shared clock rather than rolled, so
//! every viewer of the same map is watching the same animation at the same
//! moment. It states no animation of its own: adding a state adds it to the
//! rotation everywhere one of these is already placed.
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
use crate::common::team::Team;
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

/// What side the roster is on.
///
/// A grid is a harness, and which harness you want depends on what you are
/// looking at. **Striped** deals the four teams round the roster in cell
/// order, which makes the grid a friendly-fire range: shoot along a row and
/// every fourth body refuses the hit. **One team** makes it a plain firing
/// range again — all of them shoot back, or none of them do, depending on
/// whether you picked your own.
///
/// `One(Team)` rather than four more variants, so a fifth team is a fifth
/// entry in the combo box and nothing else. The one thing that is written out
/// by hand is [`RosterTeam::index`], because those numbers are on disk.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RosterTeam {
    /// One of each, round the roster in cell order.
    #[default]
    Striped,
    /// All of them on the one side.
    One(Team),
}

impl RosterTeam {
    /// Every choice, in the order the panel lists them.
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

    /// What side the body in `cell` is on.
    ///
    /// The stripe is by cell rather than rolled, like the animation phase
    /// beside it: teams drawn locally would put two viewers of one map out of
    /// step with each other about who is allowed to shoot whom.
    pub fn team_for(self, cell: usize) -> Team {
        match self {
            RosterTeam::Striped => Team::ALL[cell % Team::ALL.len()],
            RosterTeam::One(team) => team,
        }
    }

    /// Stable index for the save format, as [`AnimationState::index`] is.
    /// Zero is the default, so a file that predates this field loads striped.
    pub fn index(self) -> u32 {
        match self {
            RosterTeam::Striped => 0,
            RosterTeam::One(Team::Red) => 1,
            RosterTeam::One(Team::Blue) => 2,
            RosterTeam::One(Team::Yellow) => 3,
            RosterTeam::One(Team::Green) => 4,
        }
    }

    /// The choice an index names, or striped for one this build does not know
    /// — a grid saved by a later build still stands up a roster.
    pub fn from_index(index: u32) -> RosterTeam {
        match index {
            1 => RosterTeam::One(Team::Red),
            2 => RosterTeam::One(Team::Blue),
            3 => RosterTeam::One(Team::Yellow),
            4 => RosterTeam::One(Team::Green),
            _ => RosterTeam::Striped,
        }
    }
}

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
        AnimationState::CrouchAirborne,
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

/// How long each animation is held before the next, in seconds.
///
/// Long enough to see a cycle or two of it and to compare it across ten
/// bodies; short enough that watching the whole rotation is not a chore.
pub const HOLD: f32 = 2.0;

/// The rotation the bodies are part way through, and which of it they are on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Showing {
    pub row: GridRow,
    /// Which shuffle this is. Everything about the order follows from it.
    pub cycle: u64,
    /// How far through that shuffle, counting from zero.
    pub slot: usize,
}

/// What every body on a carousel is showing at `seconds` on the shared clock.
///
/// A function of the clock and nothing else, which is what makes ten bodies on
/// one map and two people watching them agree without anybody being told.
pub fn showing(seconds: f32) -> Showing {
    let rows = rows();
    let period = rows.len() as f32 * HOLD;
    let seconds = seconds.max(0.0);

    let cycle = (seconds / period).floor();
    let slot = (((seconds - cycle * period) / HOLD).floor() as usize).min(rows.len() - 1);
    let cycle = cycle as u64;

    Showing { row: order(cycle)[slot], cycle, slot }
}

/// The order the rows are shown in on a given rotation.
///
/// A shuffle rather than a roll: the same rotation number gives the same order
/// on every machine, for the same reason an [`AnimationPhase`] does. Two people
/// watching one body have to see the same thing, and an order nobody could
/// reproduce would be the one thing on the map that only made sense from one
/// seat.
///
/// [`AnimationPhase`]: crate::common::skeleton::AnimationPhase
pub fn order(cycle: u64) -> Vec<GridRow> {
    let mut rows = rows();
    let mut seed = cycle;

    // Fisher-Yates, drawing from an integer hash so that every machine draws
    // the same numbers.
    for index in (1..rows.len()).rev() {
        seed = mixed(seed);
        rows.swap(index, (seed % (index as u64 + 1)) as usize);
    }
    rows
}

/// SplitMix64's finaliser: integer-only, so every machine agrees, and it
/// scatters consecutive seeds rather than leaving them in step.
fn mixed(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One body's place in the line: which class it is, and where it stands.
///
/// What it is *doing* is not here, because it is the same for all of them and
/// changes every two seconds. See [`showing`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridCell {
    pub class: Class,
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

/// How much floor a body needs front to back.
///
/// Measured off how far one actually reaches while it is moving, rather than a
/// number picked to look right at the time. Sampled across the cycle because
/// the deepest moment of a stride is not the moment anyone thinks to check.
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
    let columns = column_spacing();
    // Centred, so moving the point moves the middle of the line rather than
    // its left edge.
    let centre = (classes.len() as f32 - 1.0) * 0.5;

    classes
        .iter()
        .enumerate()
        .map(|(column, class)| GridCell {
            class: *class,
            offset: Vec3::new((column as f32 - centre) * columns, 0.0, 0.0),
        })
        .collect()
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
    let half = Vec3::new(column_spacing(), 0.0, body_depth()) * 0.5;
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
    /// What side the roster is on. Defaulted rather than required, so a grid
    /// saved before this existed loads striped instead of failing to load.
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

        // Every choice from the type rather than written out here, for the
        // same reason the display lists its states from the state machine: a
        // team that existed and could not be picked would be one nobody could
        // put on the floor.
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
            teams: self.teams,
        }
    }

    fn apply_snapshot(&mut self, data: &FeatureData) {
        let FeatureData::AnimationGrid { location, yaw, teams } = data else { return; };
        self.location = location.clone();
        self.yaw = *yaw;
        self.teams = *teams;
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
        commands
            .entity(entity)
            .insert((
                Transform::from_translation(self.resolved_location)
                    .with_rotation(Quat::from_rotation_y(self.yaw)),
                AnimationGridMarker,
                // Plain `insert`, unlike the `Visibility` below: this is the
                // authored value and re-asserting it on every edit is correct.
                // It is re-inserted on every edit and so reads as changed on
                // every frame of a drag, which is why `team_carousels`
                // compares before it writes rather than trusting that.
                self.teams,
            ))
            // The bodies hang off this entity and each carries a `Visibility`
            // of its own, which Bevy expects to inherit from a parent that has
            // one. Without it every body warns (B0004) and the grid cannot be
            // hidden as a unit.
            //
            // `insert_if_new`, like the rig on an animation display: this runs
            // on every edit, and re-inserting a default would undo anything
            // that had hidden the grid on the frame the point was nudged.
            .insert_if_new(Visibility::default());
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
        // The teams go out as an index, which is stable across builds by
        // contract — see [`RosterTeam::index`].
        vec![("yaw", self.yaw), ("teams", self.teams.index() as f32)]
    }

    fn set_scalar_field(&mut self, key: &str, value: f32) {
        match key {
            "yaw" => self.yaw = value,
            "teams" => self.teams = RosterTeam::from_index(value.max(0.0) as u32),
            _ => {}
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
    use crate::common::skeleton::{finish_pose, PoseInputs};

    /// One body per class, standing in a line. What they are doing is a
    /// question about the clock, not about where they are standing.
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

    /// The rows are written out by hand so that related states stand together,
    /// which is exactly the arrangement that lets one be forgotten. Adding a
    /// state and never showing it is the failure this catches.
    #[test]
    fn every_state_has_a_row() {
        for state in AnimationState::iter() {
            assert!(
                rows().iter().any(|row| row.state == state),
                "{state:?} is never shown"
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

        // Far enough apart to be different gaits rather than three rows of the
        // same one: at least one walks and at least one runs.
        assert!(speeds.iter().any(|speed| !GaitShape::for_speed(*speed).has_flight()));
        assert!(speeds.iter().any(|speed| GaitShape::for_speed(*speed).has_flight()));

        // A body that is not going anywhere is shown standing still, rather
        // than running on the spot.
        assert!(rows().iter().all(|row| row.state.uses_gait() || row.speed == 0.0));
    }

    /// Every rotation is a permutation: nothing is shown twice and nothing is
    /// skipped, however the shuffle falls.
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

    /// The grid entity carries a `Visibility` so the bodies hung off it have
    /// one to inherit. Without it Bevy warns once per body per spawn (B0004)
    /// and there is no way to hide the grid as a unit.
    #[test]
    fn a_grid_can_be_a_parent_to_visible_bodies() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let grid = AnimationGrid::new(0.0, 0.0, 0.0);
        world
            .run_system_once(move |mut commands: Commands| {
                grid.apply_to_entity(&mut commands, entity);
            })
            .unwrap();

        assert!(world.get::<Visibility>(entity).is_some());
        assert!(
            world.get::<InheritedVisibility>(entity).is_some(),
            "the bodies have nothing to inherit visibility from"
        );
    }

    /// Shuffled from the clock rather than rolled: asking twice gives the same
    /// answer, which is what lets two people watching the same map watch the
    /// same thing.
    #[test]
    fn the_shuffle_is_the_same_for_everyone() {
        for cycle in 0..16 {
            assert_eq!(order(cycle), order(cycle), "the order is not reproducible");
        }

        // And it is genuinely shuffled: consecutive rotations differ, and over
        // a few dozen the orders are nearly all distinct.
        let orders: Vec<Vec<GridRow>> = (0..32).map(order).collect();
        assert!(orders[0] != orders[1], "two rotations in a row are identical");

        let mut distinct = orders.clone();
        distinct.dedup();
        assert!(distinct.len() > 28, "the shuffle repeats itself: {} of 32", distinct.len());
    }

    /// Two seconds each, in the order the shuffle chose, then round again.
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

        // Round again, on a different shuffle.
        let next = showing(period + 0.01);
        assert_eq!(next.cycle, 1);
        assert_eq!(next.slot, 0);
        assert_eq!(next.row, order(1)[0]);
    }

    /// Bodies do not overlap, and the spacing is measured off the widest of
    /// them rather than guessed.
    #[test]
    fn neighbouring_bodies_do_not_overlap() {
        let widest = Class::iter().map(body_width).fold(0.0_f32, f32::max);
        assert!(
            column_spacing() > widest,
            "columns are {} apart for bodies {widest} wide",
            column_spacing()
        );

        let places: Vec<f32> = cells().iter().map(|cell| cell.offset.x).collect();
        for pair in places.windows(2) {
            assert!((pair[1] - pair[0]) >= widest, "two bodies are {} apart", pair[1] - pair[0]);
        }
    }

    /// The floor a body is given is enough for what it does on it — a stride
    /// that reached past its own patch would reach into the next one.
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
                let pose = finish_pose(skeleton, row.state, &inputs, &Transform::IDENTITY);
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

    /// The point is the middle of the line, not one end: a grid is placed by
    /// standing where you will look at it from.
    #[test]
    fn the_line_is_centred_on_its_point() {
        let places: Vec<f32> = cells().iter().map(|cell| cell.offset.x).collect();
        let mean: f32 = places.iter().sum::<f32>() / places.len() as f32;
        assert!(mean.abs() < 1e-4, "the line is off-centre by {mean}");
    }
}
