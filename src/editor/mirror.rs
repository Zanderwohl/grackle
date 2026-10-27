//! A feature that stands a mirror image of other features across a plane, or a turned copy about an
//! axis, and puts the two halves on two teams. See "Mirroring half a map" in CLAUDE.md.

use std::f32::consts::{FRAC_PI_2, PI};

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::team::{Side, Team};
use crate::common::PointResolutionError;
use crate::editor::editable::{Feature, FeatureId, FeatureTag, FeatureTrait, PointRef};
use crate::get;

/// How the copy is made from the original. Every one keeps a room an axis-aligned box.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Symmetry {
    /// Across the plane through the center facing X: east becomes west.
    #[default]
    MirrorX,
    /// Across the plane through the center facing Z.
    MirrorZ,
    /// Half a turn about the vertical through the center. Keeps handedness, so the copy is the same
    /// base turned round rather than its mirror image.
    Rotate180,
}

impl Symmetry {
    pub const ALL: [Symmetry; 3] = [Symmetry::MirrorX, Symmetry::MirrorZ, Symmetry::Rotate180];

    pub fn name(self) -> String {
        match self {
            Symmetry::MirrorX => get!("editor.features.mirror.mirror_x"),
            Symmetry::MirrorZ => get!("editor.features.mirror.mirror_z"),
            Symmetry::Rotate180 => get!("editor.features.mirror.rotate_180"),
        }
    }
}

/// A [`Symmetry`] placed in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mirroring {
    pub symmetry: Symmetry,
    pub center: Vec3,
}

impl Mirroring {
    pub fn point(&self, point: Vec3) -> Vec3 {
        self.center + self.direction(point - self.center)
    }

    pub fn direction(&self, v: Vec3) -> Vec3 {
        match self.symmetry {
            Symmetry::MirrorX => Vec3::new(-v.x, v.y, v.z),
            Symmetry::MirrorZ => Vec3::new(v.x, v.y, -v.z),
            Symmetry::Rotate180 => Vec3::new(-v.x, v.y, -v.z),
        }
    }

    /// Sorted, since a mirror swaps which corner is the minimum.
    pub fn bounds(&self, a: Vec3, b: Vec3) -> (Vec3, Vec3) {
        let (a, b) = (self.point(a), self.point(b));
        (a.min(b), a.max(b))
    }

    /// Forward and up are carried across exactly. Under a reflection the copy's own left and right
    /// cannot also be and stay a rotation, so that is what gives: a model is turned, never turned
    /// inside out by a negative scale.
    pub fn rotation(&self, turn: Quat) -> Quat {
        if self.symmetry == Symmetry::Rotate180 {
            return Quat::from_rotation_y(PI) * turn;
        }
        let back = self.direction(turn * Vec3::Z);
        let up = self.direction(turn * Vec3::Y);
        Quat::from_mat3(&Mat3::from_cols(up.cross(back), up, back)).normalize()
    }

    /// Radians about world Y, where `-Z` faces a yaw of zero.
    pub fn yaw(&self, yaw: f32) -> f32 {
        let forward = self.direction(Quat::from_rotation_y(yaw) * Vec3::NEG_Z);
        f32::atan2(-forward.x, -forward.z)
    }
}

/// On a Mirror's entity: what [`assign_sides`] reads.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct MirrorSides {
    /// The originals' team, then the copies'.
    pub teams: [Team; 2],
    /// The originals' entities. The copies are this entity's [`MirrorCopy`] children.
    pub sources: Vec<Entity>,
}

/// One copy a Mirror stands, a child of the Mirror's entity so the two go together.
#[derive(Component, Debug)]
pub struct MirrorCopy;

/// Why a source has no copy, for the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeftOut {
    Missing,
    NoMirrorImage,
}

struct MirroredCopy {
    source: FeatureId,
    source_entity: Option<Entity>,
    object: Box<dyn FeatureTrait>,
}

/// Live, not a one-off duplicate: the copies are rebuilt from the sources on every resolve, so an
/// edit to one half is an edit to both. A copy is never a feature itself, and nothing references it.
#[derive(Serialize, Deserialize)]
pub struct Mirror {
    center: PointRef,
    symmetry: Symmetry,
    sources: Vec<FeatureId>,
    /// The originals' team, then the copies'.
    teams: [Team; 2],
    #[serde(skip)]
    resolved_center: Vec3,
    #[serde(skip)]
    copies: Vec<MirroredCopy>,
    #[serde(skip)]
    left_out: Vec<(FeatureId, LeftOut)>,
    #[serde(skip)]
    entity: Option<Entity>,
    /// By source, so an edit lands on the copy already standing rather than rebuilding it: a
    /// re-spawned spawn point rebuilds its body mesh, every frame of a drag.
    #[serde(skip)]
    copy_entities: Vec<(FeatureId, Entity)>,
}

impl Mirror {
    pub fn new(center: PointRef) -> Self {
        Self {
            center,
            symmetry: Symmetry::default(),
            sources: Vec::new(),
            teams: [Team::Red, Team::Blue],
            resolved_center: Vec3::ZERO,
            copies: Vec::new(),
            left_out: Vec::new(),
            entity: None,
            copy_entities: Vec::new(),
        }
    }

    pub fn with_symmetry(mut self, symmetry: Symmetry) -> Self {
        self.symmetry = symmetry;
        self
    }

    pub fn with_sources(mut self, sources: Vec<FeatureId>) -> Self {
        self.sources = sources;
        self
    }

    pub fn with_teams(mut self, originals: Team, copies: Team) -> Self {
        self.teams = [originals, copies];
        self
    }

    pub fn mirroring(&self) -> Mirroring {
        Mirroring { symmetry: self.symmetry, center: self.resolved_center }
    }

}

/// Through the feature's own serde, the same way undo and saving copy one, so a new feature type is
/// copyable the day it is written.
fn mirrored_copy(
    source: &Feature,
    features: &HashMap<FeatureId, Feature>,
    mirroring: &Mirroring,
) -> Option<Box<dyn FeatureTrait>> {
    let value = serde_json::to_value(source.object()).ok()?;
    let mut copy: Box<dyn FeatureTrait> = serde_json::from_value(value).ok()?;
    copy.resolve_references(features);
    copy.mirror(mirroring).then_some(copy)
}

const MIRROR_COLOR: Color = Color::srgb(0.9, 0.35, 0.9);

#[typetag::serde(name = "mirror")]
impl FeatureTrait for Mirror {
    fn get_point(&self, _key: &str) -> Result<Vec3, PointResolutionError> {
        Ok(self.resolved_center)
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let mut changed = false;
        let label = get!("editor.features.mirror.center");
        changed |= self.center.editor_ui(ui, &label, features, prior_feature_order, retarget_request);

        ui.separator();
        egui::ComboBox::from_label(get!("editor.features.mirror.symmetry"))
            .selected_text(self.symmetry.name())
            .show_ui(ui, |ui| {
                for choice in Symmetry::ALL {
                    if ui.selectable_label(self.symmetry == choice, choice.name()).clicked()
                        && self.symmetry != choice
                    {
                        self.symmetry = choice;
                        changed = true;
                    }
                }
            });

        let labels = [get!("editor.features.mirror.originals_team"), get!("editor.features.mirror.copies_team")];
        for (slot, label) in labels.into_iter().enumerate() {
            egui::ComboBox::from_label(label)
                .selected_text(self.teams[slot].name())
                .show_ui(ui, |ui| {
                    for team in Team::ALL {
                        if ui.selectable_label(self.teams[slot] == team, team.name()).clicked()
                            && self.teams[slot] != team
                        {
                            self.teams[slot] = team;
                            changed = true;
                        }
                    }
                });
        }

        ui.separator();
        ui.label(egui::RichText::new(get!("editor.features.mirror.sources")).strong());
        let mut dropped = None;
        for (index, id) in self.sources.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button(get!("editor.features.mirror.drop")).clicked() {
                    dropped = Some(index);
                }
                let name = features
                    .get(id)
                    .map(Feature::type_name_with_id)
                    .unwrap_or_else(|| id.to_string());
                let text = match self.left_out.iter().find(|(left, _)| left == id) {
                    None => egui::RichText::new(name),
                    Some((_, why)) => {
                        let why = match why {
                            LeftOut::Missing => get!("editor.features.mirror.missing"),
                            LeftOut::NoMirrorImage => get!("editor.features.mirror.no_mirror_image"),
                        };
                        egui::RichText::new(format!("{name} ({why})")).color(egui::Color32::from_rgb(230, 130, 80))
                    }
                };
                ui.label(text);
            });
        }
        if let Some(index) = dropped {
            self.sources.remove(index);
            changed = true;
        }

        let mirroring = self.mirroring();
        let candidates = |sources: &[FeatureId]| -> Vec<FeatureId> {
            prior_feature_order
                .iter()
                .copied()
                .filter(|id| !sources.contains(id))
                .filter(|id| {
                    features
                        .get(id)
                        .is_some_and(|feature| mirrored_copy(feature, features, &mirroring).is_some())
                })
                .collect()
        };
        ui.horizontal(|ui| {
            let mut added = None;
            egui::ComboBox::from_id_salt("mirror_add_source")
                .selected_text(get!("editor.features.mirror.add"))
                .show_ui(ui, |ui| {
                    for id in candidates(&self.sources) {
                        if let Some(feature) = features.get(&id)
                            && ui.selectable_label(false, feature.type_name_with_id()).clicked()
                        {
                            added = Some(id);
                        }
                    }
                });
            if let Some(id) = added {
                self.sources.push(id);
                changed = true;
            }
            if ui.button(get!("editor.features.mirror.add_all")).clicked() {
                let more = candidates(&self.sources);
                changed |= !more.is_empty();
                self.sources.extend(more);
            }
        });

        if changed {
            self.resolve_references(features);
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.mirror.title")
    }

    fn type_key(&self) -> &'static str { "mirror" }

    /// The plane or axis, sized to reach the copies, and every copy's own gizmo.
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let center = self.resolved_center;
        let reach = self
            .copies
            .iter()
            .filter_map(|copy| copy.object.get_point("").ok())
            .map(|point| (point - center).with_y(0.0).length())
            .fold(4.0, f32::max)
            + 1.0;

        gizmos.sphere(Isometry3d::from_translation(center), 0.2, MIRROR_COLOR);
        match self.symmetry {
            Symmetry::MirrorX => {
                gizmos.rect(Isometry3d::new(center, Quat::from_rotation_y(FRAC_PI_2)), Vec2::splat(reach * 2.0), MIRROR_COLOR);
            }
            Symmetry::MirrorZ => {
                gizmos.rect(Isometry3d::from_translation(center), Vec2::splat(reach * 2.0), MIRROR_COLOR);
            }
            Symmetry::Rotate180 => {
                gizmos.line(center - Vec3::Y * reach, center + Vec3::Y * reach, MIRROR_COLOR);
                gizmos.circle(Isometry3d::new(center, Quat::from_rotation_x(FRAC_PI_2)), reach, MIRROR_COLOR);
            }
        }
        self.center.debug_gizmos(center, gizmos);

        for copy in &self.copies {
            copy.object.debug_gizmos(gizmos);
        }
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    /// The copies are children, so they went with the entity.
    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
        self.copy_entities.clear();
    }

    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        // The identity, so a copy's `Transform` is where it is: spawn choice reads `Transform`.
        commands
            .entity(entity)
            .insert(MirrorSides {
                teams: self.teams,
                sources: self.copies.iter().filter_map(|copy| copy.source_entity).collect(),
            })
            .insert_if_new((Transform::IDENTITY, Visibility::Inherited));

        let copies = &self.copies;
        self.copy_entities.retain(|(source, child)| {
            let kept = copies.iter().any(|copy| copy.source == *source);
            if !kept {
                commands.entity(*child).despawn();
            }
            kept
        });

        for copy in &mut self.copies {
            let child = match self.copy_entities.iter().find(|(source, _)| *source == copy.source) {
                Some((_, child)) => *child,
                None => {
                    let child = commands.spawn((MirrorCopy, ChildOf(entity))).id();
                    self.copy_entities.push((copy.source, child));
                    child
                }
            };
            copy.object.set_entity(Some(child));
            copy.object.apply_to_entity(commands, child);
        }
    }

    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>) {
        if let Ok(v) = self.center.resolve(features) {
            self.resolved_center = v;
        }
        let mirroring = self.mirroring();
        self.copies.clear();
        self.left_out.clear();
        for id in &self.sources {
            // A duplicated id would share its copy's entity with the first.
            if self.copies.iter().any(|copy| copy.source == *id)
                || self.left_out.iter().any(|(left, _)| left == id)
            {
                continue;
            }
            let Some(source) = features.get(id) else {
                self.left_out.push((*id, LeftOut::Missing));
                continue;
            };
            match mirrored_copy(source, features, &mirroring) {
                Some(object) => self.copies.push(MirroredCopy {
                    source: *id,
                    source_entity: source.object().entity(),
                    object,
                }),
                None => self.left_out.push((*id, LeftOut::NoMirrorImage)),
            }
        }
    }

    /// The sources too: an edit to one is an edit to its copy, which is how it reaches here.
    fn parent_ids(&self) -> Vec<FeatureId> {
        let mut ids = self.center.referenced_features();
        for id in &self.sources {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        ids
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        vec![("".into(), get!("editor.features.mirror.center"))]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![("".into(), self.resolved_center)]
    }

    fn standing_copies(&self) -> Vec<&dyn FeatureTrait> {
        self.copies.iter().map(|copy| &*copy.object).collect()
    }

    /// Clicking a copy selects the Mirror, which is what edits it.
    fn pick_boxes(&self) -> Vec<crate::common::ray::PickBox> {
        self.copies.iter().flat_map(|copy| copy.object.pick_boxes()).collect()
    }

    fn get_point_ref(&self, _key: &str) -> Option<&PointRef> {
        Some(&self.center)
    }

    fn get_point_ref_mut(&mut self, _key: &str) -> Option<&mut PointRef> {
        Some(&mut self.center)
    }

    fn drag_handle(&mut self, _is_max: bool, axis: u8, new_world_value: f32) -> bool {
        if !self.center.drag_axis(axis, new_world_value) {
            return false;
        }
        self.resolved_center[axis as usize] = new_world_value;
        true
    }
}

/// The one writer of [`Side`]: a Mirror's originals get its first team and its copies its second, and
/// anything no Mirror claims has no side. Keyed on what stands now rather than on edits, so deleting a
/// Mirror, rolling back past it or dropping a source takes the side away with nothing to remember.
/// Compares before it writes, since this runs every frame.
///
/// Where two Mirrors claim one entity the earlier feature wins, by feature id rather than by query
/// order, which shifts with archetypes.
pub fn assign_sides(
    mirrors: Query<(&MirrorSides, Option<&Children>, &FeatureTag)>,
    copies: Query<(), With<MirrorCopy>>,
    sided: Query<(Entity, &Side)>,
    mut commands: Commands,
) {
    if mirrors.is_empty() && sided.is_empty() {
        return;
    }
    let mut ordered: Vec<_> = mirrors.iter().collect();
    ordered.sort_by_key(|(_, _, tag)| tag.feature_id);
    let mut wanted: HashMap<Entity, Team> = HashMap::new();
    for (mirror, children, _) in ordered {
        for source in &mirror.sources {
            wanted.entry(*source).or_insert(mirror.teams[0]);
        }
        let children: &[Entity] = children.map_or(&[], |children| children);
        for child in children {
            if copies.contains(*child) {
                wanted.entry(*child).or_insert(mirror.teams[1]);
            }
        }
    }

    for (entity, side) in &sided {
        match wanted.remove(&entity) {
            Some(team) if team == side.0 => {}
            Some(team) => {
                commands.entity(entity).try_insert(Side(team));
            }
            None => {
                commands.entity(entity).try_remove::<Side>();
            }
        }
    }
    for (entity, team) in wanted {
        commands.entity(entity).try_insert(Side(team));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::editable::FeatureTimeline;
    use crate::editor::editor_room::EditorRoom;
    use crate::editor::spawn_point::{SpawnPoint, SpawnPointMarker};

    fn at(symmetry: Symmetry, center: Vec3) -> Mirroring {
        Mirroring { symmetry, center }
    }

    /// Doing it twice is doing nothing, for every symmetry: the property that makes the two halves
    /// copies of each other rather than of a third thing.
    #[test]
    fn mirroring_twice_is_the_identity() {
        let point = Vec3::new(3.0, 1.5, -7.0);
        let turn = Quat::from_euler(EulerRot::YXZ, 0.7, 0.3, -0.2);
        for symmetry in Symmetry::ALL {
            let m = at(symmetry, Vec3::new(1.0, 0.0, 2.0));
            assert!(m.point(m.point(point)).abs_diff_eq(point, 1e-5), "{symmetry:?}");
            assert!(m.rotation(m.rotation(turn)).abs_diff_eq(turn, 1e-5) || m.rotation(m.rotation(turn)).abs_diff_eq(-turn, 1e-5), "{symmetry:?}");
            let yaw = 0.9;
            let back = m.yaw(m.yaw(yaw));
            assert!((Quat::from_rotation_y(back) * Vec3::Z).abs_diff_eq(Quat::from_rotation_y(yaw) * Vec3::Z, 1e-5), "{symmetry:?}");
        }
    }

    /// A body facing the plane on one side faces it on the other; under a half turn, it faces back
    /// the way it came.
    #[test]
    fn a_facing_is_carried_across() {
        let facing = |yaw: f32| Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
        let east = -FRAC_PI_2;
        assert!(facing(east).abs_diff_eq(Vec3::X, 1e-5));

        let x = at(Symmetry::MirrorX, Vec3::ZERO);
        assert!(facing(x.yaw(east)).abs_diff_eq(Vec3::NEG_X, 1e-5));
        assert!(facing(x.yaw(0.0)).abs_diff_eq(Vec3::NEG_Z, 1e-5), "facing along the plane is unchanged");

        let z = at(Symmetry::MirrorZ, Vec3::ZERO);
        assert!(facing(z.yaw(0.0)).abs_diff_eq(Vec3::Z, 1e-5));
        assert!(facing(z.yaw(east)).abs_diff_eq(Vec3::X, 1e-5));

        let turned = at(Symmetry::Rotate180, Vec3::ZERO);
        assert!(facing(turned.yaw(east)).abs_diff_eq(Vec3::NEG_X, 1e-5));
    }

    /// A prop's forward and up go where the reflection takes them, and it is still a rotation.
    #[test]
    fn a_mirrored_turn_keeps_forward_and_up() {
        let turn = Quat::from_euler(EulerRot::YXZ, 0.7, 0.3, -0.2);
        for symmetry in [Symmetry::MirrorX, Symmetry::MirrorZ] {
            let m = at(symmetry, Vec3::ZERO);
            let copy = m.rotation(turn);
            assert!((copy * Vec3::NEG_Z).abs_diff_eq(m.direction(turn * Vec3::NEG_Z), 1e-5), "{symmetry:?}");
            assert!((copy * Vec3::Y).abs_diff_eq(m.direction(turn * Vec3::Y), 1e-5), "{symmetry:?}");
            assert!((copy.length() - 1.0).abs() < 1e-5);
        }
    }

    fn spawn_and_room() -> (FeatureTimeline, FeatureId, FeatureId) {
        let mut timeline = FeatureTimeline::default();
        let room = timeline.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(2.0, 0.0, -1.0),
            PointRef::absolute(6.0, 3.0, 1.0),
        )));
        let spawn = timeline.apply_feature(Box::new(SpawnPoint::new(4.0, 0.0, 0.0).with_yaw(-FRAC_PI_2)));
        (timeline, room, spawn)
    }

    /// The copies stand where the symmetry says, on the copies' side, and the originals on theirs.
    #[test]
    fn a_mirror_stands_copies_on_the_other_side() {
        let (mut timeline, room, spawn) = spawn_and_room();
        timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 0.0))
                .with_sources(vec![room, spawn])
                .with_teams(Team::Yellow, Team::Green),
        ));

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(timeline);
        app.add_systems(Update, (FeatureTimeline::sync_entities, assign_sides).chain());
        app.update();
        app.update();

        let world = app.world_mut();
        let spawns: Vec<(Vec3, f32, Option<Team>)> = world
            .query_filtered::<(&Transform, Option<&Side>), With<SpawnPointMarker>>()
            .iter(world)
            .map(|(at, side)| (at.translation, at.rotation.to_euler(EulerRot::YXZ).0, side.map(|s| s.0)))
            .collect();
        assert_eq!(spawns.len(), 2, "{spawns:?}");
        let original = spawns.iter().find(|(at, ..)| at.x > 0.0).expect("the original");
        let copy = spawns.iter().find(|(at, ..)| at.x < 0.0).expect("the copy");
        assert!(copy.0.abs_diff_eq(Vec3::new(-4.0, 0.0, 0.0), 1e-5));
        assert!((copy.1 - FRAC_PI_2).abs() < 1e-5, "the copy faces away from the plane, as the original does: {}", copy.1);
        assert_eq!(original.2, Some(Team::Yellow));
        assert_eq!(copy.2, Some(Team::Green));

        let rooms: Vec<crate::tool::room::Room> =
            world.query::<&crate::tool::room::Room>().iter(world).cloned().collect();
        assert_eq!(rooms.len(), 2, "the copy is not a room the bake and collision can see");
    }

    /// Taking the Mirror away takes its sides and copies with it.
    #[test]
    fn deleting_a_mirror_unsides_its_originals() {
        let (mut timeline, room, spawn) = spawn_and_room();
        let mirror = timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_sources(vec![room, spawn]),
        ));

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(timeline);
        app.add_systems(Update, (FeatureTimeline::sync_entities, assign_sides).chain());
        app.update();
        app.update();

        app.world_mut().resource_mut::<FeatureTimeline>().delete_feature(mirror);
        app.update();
        app.update();

        let world = app.world_mut();
        assert_eq!(world.query::<&Side>().iter(world).count(), 0, "a side outlived its Mirror");
        assert_eq!(world.query::<&MirrorCopy>().iter(world).count(), 0, "a copy outlived its Mirror");
        assert_eq!(world.query::<&SpawnPointMarker>().iter(world).count(), 1);
    }

    /// Deleting a source takes its copy down rather than leaving it standing.
    #[test]
    fn deleting_a_source_takes_its_copy_down() {
        let (mut timeline, room, spawn) = spawn_and_room();
        timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_sources(vec![room, spawn]),
        ));

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(timeline);
        app.add_systems(Update, (FeatureTimeline::sync_entities, assign_sides).chain());
        app.update();

        app.world_mut().resource_mut::<FeatureTimeline>().delete_feature(spawn);
        app.update();

        let world = app.world_mut();
        assert_eq!(world.query::<&SpawnPointMarker>().iter(world).count(), 0, "the copy outlived its source");
        assert_eq!(world.query::<&MirrorCopy>().iter(world).count(), 1, "the room's copy went too");
    }

    #[test]
    fn a_source_listed_twice_is_copied_once() {
        let (timeline, room, _) = spawn_and_room();
        let mut mirror = Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_sources(vec![room, room]);
        mirror.resolve_references(timeline.features_map());
        assert_eq!(mirror.standing_copies().len(), 1);
    }

    /// A Mirror is not something a Mirror can copy, and is said to be left out rather than dropped.
    #[test]
    fn what_cannot_be_mirrored_is_left_out() {
        let mut timeline = FeatureTimeline::default();
        let inner = timeline.apply_feature(Box::new(Mirror::new(PointRef::absolute(0.0, 0.0, 0.0))));
        let mut outer = Mirror::new(PointRef::absolute(1.0, 0.0, 0.0)).with_sources(vec![inner]);
        outer.resolve_references(timeline.features_map());
        assert!(outer.standing_copies().is_empty());
        assert_eq!(outer.left_out, vec![(inner, LeftOut::NoMirrorImage)]);
    }
}
