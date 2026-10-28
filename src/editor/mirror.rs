//! A feature that stands a mirror image of other features across a plane, or turned copies about an
//! axis — one, or three for a four-way turn — and puts the original and each copy on a team. See "Mirroring half a map" in CLAUDE.md.

use std::collections::BTreeMap;
use std::f32::consts::FRAC_PI_2;

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::team::{Side, Team};
use crate::common::PointResolutionError;
use crate::editor::editable::{Feature, FeatureId, FeatureTag, FeatureTrait, PointRef};
use crate::get;

/// How the copies are made from the original. Every one keeps a room an axis-aligned box.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Symmetry {
    /// Across the plane through the center facing X: east becomes west.
    #[default]
    MirrorX,
    /// Across the plane through the center facing Z.
    MirrorZ,
    /// Half a turn about the vertical through the center. Keeps handedness.
    Rotate180,
    /// Three copies, a quarter, a half and three quarters of a turn about the vertical. Keeps
    /// handedness.
    Rotate90,
}

impl Symmetry {
    pub const ALL: [Symmetry; 4] = [Symmetry::MirrorX, Symmetry::MirrorZ, Symmetry::Rotate180, Symmetry::Rotate90];

    pub fn name(self) -> String {
        match self {
            Symmetry::MirrorX => get!("editor.features.mirror.mirror_x"),
            Symmetry::MirrorZ => get!("editor.features.mirror.mirror_z"),
            Symmetry::Rotate180 => get!("editor.features.mirror.rotate_180"),
            Symmetry::Rotate90 => get!("editor.features.mirror.rotate_90"),
        }
    }

    /// One per copy of each source, in the order their teams are listed after the originals'.
    pub fn images(self) -> &'static [Motion] {
        match self {
            Symmetry::MirrorX => &[Motion::ReflectX],
            Symmetry::MirrorZ => &[Motion::ReflectZ],
            Symmetry::Rotate180 => &[Motion::Turn(2)],
            Symmetry::Rotate90 => &[Motion::Turn(1), Motion::Turn(2), Motion::Turn(3)],
        }
    }
}

/// What takes an original to one of its copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    ReflectX,
    ReflectZ,
    /// Quarter turns about +Y, counter-clockwise seen from above.
    Turn(u8),
}

/// A [`Motion`] placed in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mirroring {
    pub motion: Motion,
    pub center: Vec3,
}

impl Mirroring {
    pub fn point(&self, point: Vec3) -> Vec3 {
        self.center + self.direction(point - self.center)
    }

    /// Exact rather than through a quaternion, so a room's corners land on the numbers they mirror.
    pub fn direction(&self, v: Vec3) -> Vec3 {
        match self.motion {
            Motion::ReflectX => Vec3::new(-v.x, v.y, v.z),
            Motion::ReflectZ => Vec3::new(v.x, v.y, -v.z),
            Motion::Turn(quarters) => match quarters % 4 {
                0 => v,
                1 => Vec3::new(v.z, v.y, -v.x),
                2 => Vec3::new(-v.x, v.y, -v.z),
                _ => Vec3::new(-v.z, v.y, v.x),
            },
        }
    }

    /// Sorted, since a mirror swaps which corner is the minimum.
    pub fn bounds(&self, a: Vec3, b: Vec3) -> (Vec3, Vec3) {
        let (a, b) = (self.point(a), self.point(b));
        (a.min(b), a.max(b))
    }

    /// Forward and up are kept exactly and handedness is not, so a model is turned rather than
    /// scaled inside out.
    pub fn rotation(&self, turn: Quat) -> Quat {
        if let Motion::Turn(quarters) = self.motion {
            return Quat::from_rotation_y(f32::from(quarters) * FRAC_PI_2) * turn;
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

/// What side a Mirror's copy of something is, by the side of what it copied.
#[derive(Clone, Debug, PartialEq)]
pub struct TeamMap {
    /// The originals' team, then each image's, in [`Symmetry::images`] order.
    pub teams: Vec<Team>,
    /// For a team arriving from a Mirror this one copies, under a two-way symmetry only. A team not
    /// named moves round `teams` as far as the image does, or passes through if it is not there.
    pub remap: BTreeMap<Team, Team>,
}

impl TeamMap {
    /// `image` indexes [`Symmetry::images`].
    pub fn becomes(&self, team: Team, image: usize) -> Team {
        if team != self.teams[0]
            && self.teams.len() == 2
            && let Some(to) = self.remap.get(&team)
        {
            return *to;
        }
        match self.teams.iter().position(|at| *at == team) {
            Some(at) => self.teams[(at + image + 1) % self.teams.len()],
            None => team,
        }
    }

    fn by_default(&self, team: Team) -> Team {
        TeamMap { teams: self.teams.clone(), remap: BTreeMap::new() }.becomes(team, 0)
    }
}

/// On a Mirror's entity: what [`assign_sides`] reads.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct MirrorSides {
    pub map: TeamMap,
    /// The originals' entities. The copies are this entity's [`MirrorCopy`] children.
    pub sources: Vec<Entity>,
}

/// One copy a Mirror stands, a child of the Mirror's entity so the two go together.
#[derive(Component, Debug)]
pub struct MirrorCopy {
    /// Indexes [`Symmetry::images`], and so which team it is on.
    pub image: usize,
}

/// Why a source has no copy, for the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeftOut {
    Missing,
    NoMirrorImage,
}

struct MirroredCopy {
    source: FeatureId,
    image: usize,
    source_entity: Option<Entity>,
    object: Box<dyn FeatureTrait>,
}

/// The copies are rebuilt from the sources on every resolve. A copy is not a feature, and nothing
/// references it.
#[derive(Serialize, Deserialize)]
pub struct Mirror {
    center: PointRef,
    symmetry: Symmetry,
    sources: Vec<FeatureId>,
    /// The originals' team, then the first copy's.
    teams: [Team; 2],
    /// Under [`Symmetry::Rotate90`], the half and three-quarter turns'. `None` alternates `teams`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turned_teams: Option<[Team; 2]>,
    /// See [`TeamMap::remap`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    remap: BTreeMap<Team, Team>,
    #[serde(skip)]
    resolved_center: Vec3,
    #[serde(skip)]
    copies: Vec<MirroredCopy>,
    #[serde(skip)]
    left_out: Vec<(FeatureId, LeftOut)>,
    #[serde(skip)]
    entity: Option<Entity>,
    /// By source, so an edit re-applies the standing copy: a respawned spawn point rebuilds its body
    /// mesh every frame of a drag.
    #[serde(skip)]
    copy_entities: Vec<((FeatureId, usize), Entity)>,
    /// A copy draws no plane, since no feature owns one there.
    #[serde(skip)]
    is_copy: bool,
}

impl Mirror {
    pub fn new(center: PointRef) -> Self {
        Self {
            center,
            symmetry: Symmetry::default(),
            sources: Vec::new(),
            teams: [Team::Red, Team::Blue],
            turned_teams: None,
            remap: BTreeMap::new(),
            resolved_center: Vec3::ZERO,
            copies: Vec::new(),
            left_out: Vec::new(),
            entity: None,
            copy_entities: Vec::new(),
            is_copy: false,
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

    /// The originals' team, then each quarter turn's, for [`Symmetry::Rotate90`].
    pub fn with_four_teams(mut self, teams: [Team; 4]) -> Self {
        self.teams = [teams[0], teams[1]];
        self.set_turned_teams([teams[2], teams[3]]);
        self
    }

    fn set_turned_teams(&mut self, turned: [Team; 2]) {
        self.turned_teams = (turned != self.teams).then_some(turned);
    }

    pub fn remapping(mut self, from: Team, to: Team) -> Self {
        self.remap.insert(from, to);
        self
    }

    /// The originals' team, then each image's.
    fn teams(&self) -> Vec<Team> {
        let mut teams = self.teams.to_vec();
        if self.symmetry.images().len() == 3 {
            teams.extend(self.turned_teams.unwrap_or(self.teams));
        }
        teams
    }

    pub fn team_map(&self) -> TeamMap {
        TeamMap { teams: self.teams(), remap: self.remap.clone() }
    }

    /// The sides this Mirror's copies come out on, nested copies' copies included.
    fn copy_sides(&mut self) -> Vec<Team> {
        let map = self.team_map();
        let mut sides = Vec::new();
        for copy in &mut self.copies {
            match copy.object.mirror_mut() {
                Some(nested) => sides.extend(nested.copy_sides().into_iter().map(|team| map.becomes(team, copy.image))),
                None => sides.push(map.becomes(map.teams[0], copy.image)),
            }
        }
        sides
    }

    /// Teams arriving from the Mirrors this one copies, other than the originals' own. None under a
    /// four-way turn, where a remap would have to be stated per quarter.
    fn arriving_teams(&mut self) -> Vec<Team> {
        if self.teams().len() != 2 {
            return Vec::new();
        }
        let mut arriving = Vec::new();
        for copy in &mut self.copies {
            if let Some(nested) = copy.object.mirror_mut() {
                arriving.extend(nested.copy_sides());
            }
        }
        Team::ALL.into_iter().filter(|team| *team != self.teams[0] && arriving.contains(team)).collect()
    }

    pub fn mirrorings(&self) -> impl Iterator<Item = Mirroring> + use<> {
        let center = self.resolved_center;
        self.symmetry.images().iter().map(move |motion| Mirroring { motion: *motion, center })
    }

    /// A copy of a Mirror is rebuilt on every resolve, so it takes over its predecessor's entities.
    fn inherit_entities(&mut self, previous: &mut Mirror) {
        self.entity = previous.entity;
        self.copy_entities = std::mem::take(&mut previous.copy_entities);
        inherit_nested(&mut self.copies, &mut previous.copies);
    }
}

fn inherit_nested(copies: &mut [MirroredCopy], previous: &mut [MirroredCopy]) {
    for copy in copies {
        let Some(nested) = copy.object.mirror_mut() else { continue };
        if let Some(old) = previous
            .iter_mut()
            .find(|old| (old.source, old.image) == (copy.source, copy.image))
            .and_then(|old| old.object.mirror_mut())
        {
            nested.inherit_entities(old);
        }
    }
}

/// Reached only by Mirrors listing each other, which would otherwise recurse until the stack runs out.
const MAX_NESTING: usize = 8;

thread_local! {
    /// The sources being copied further up, so a Mirror reached again through a cycle stops there
    /// rather than at `MAX_NESTING`: a four-way turn resolves each source once per image, so the
    /// cost of every extra level is multiplied by three.
    static COPYING: std::cell::RefCell<Vec<FeatureId>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Through the feature's own serde, the same way undo and saving copy one, so a new feature type is
/// copyable the day it is written.
fn mirrored_copy(
    source: &Feature,
    features: &HashMap<FeatureId, Feature>,
    mirroring: &Mirroring,
) -> Option<Box<dyn FeatureTrait>> {
    let cyclic = COPYING.with_borrow(|copying| copying.len() >= MAX_NESTING || copying.contains(&source.id()));
    if cyclic {
        return None;
    }
    let value = serde_json::to_value(source.object()).ok()?;
    let mut copy: Box<dyn FeatureTrait> = serde_json::from_value(value).ok()?;
    COPYING.with_borrow_mut(|copying| copying.push(source.id()));
    copy.resolve_references(features);
    COPYING.with_borrow_mut(|copying| copying.pop());
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

        let labels = match self.symmetry {
            Symmetry::Rotate90 => vec![
                get!("editor.features.mirror.originals_team"),
                get!("editor.features.mirror.quarter_turn_team"),
                get!("editor.features.mirror.half_turn_team"),
                get!("editor.features.mirror.three_quarter_turn_team"),
            ],
            _ => vec![get!("editor.features.mirror.originals_team"), get!("editor.features.mirror.copies_team")],
        };
        let mut teams = self.teams();
        for (slot, label) in labels.into_iter().enumerate() {
            egui::ComboBox::from_label(label)
                .selected_text(teams[slot].name())
                .show_ui(ui, |ui| {
                    for team in Team::ALL {
                        if ui.selectable_label(teams[slot] == team, team.name()).clicked() && teams[slot] != team {
                            teams[slot] = team;
                            changed = true;
                        }
                    }
                });
        }
        if changed {
            self.teams = [teams[0], teams[1]];
            // Left alone when two-way, so switching back and forth loses nothing.
            if teams.len() == 4 {
                self.set_turned_teams([teams[2], teams[3]]);
            }
        }

        let map = self.team_map();
        for arriving in self.arriving_teams() {
            let current = map.becomes(arriving, 0);
            egui::ComboBox::from_label(get!("editor.features.mirror.becomes", "team", arriving.name()))
                .selected_text(current.name())
                .show_ui(ui, |ui| {
                    for team in Team::ALL {
                        if ui.selectable_label(current == team, team.name()).clicked() && current != team {
                            if team == map.by_default(arriving) {
                                self.remap.remove(&arriving);
                            } else {
                                self.remap.insert(arriving, team);
                            }
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

        let Some(mirroring) = self.mirrorings().next() else { return changed };
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
                            && ui.selectable_label(false, feature.label()).clicked()
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

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let center = self.resolved_center;
        let reach = self
            .copies
            .iter()
            .filter_map(|copy| copy.object.get_point("").ok())
            .map(|point| (point - center).with_y(0.0).length())
            .fold(4.0, f32::max)
            + 1.0;

        // The copies are drawn by whoever asks for `standing_copies`, each under its own kind's switch.
        if self.is_copy {
            return;
        }

        gizmos.sphere(Isometry3d::from_translation(center), 0.2, MIRROR_COLOR);
        match self.symmetry {
            Symmetry::MirrorX => {
                gizmos.rect(Isometry3d::new(center, Quat::from_rotation_y(FRAC_PI_2)), Vec2::splat(reach * 2.0), MIRROR_COLOR);
            }
            Symmetry::MirrorZ => {
                gizmos.rect(Isometry3d::from_translation(center), Vec2::splat(reach * 2.0), MIRROR_COLOR);
            }
            Symmetry::Rotate180 | Symmetry::Rotate90 => {
                gizmos.line(center - Vec3::Y * reach, center + Vec3::Y * reach, MIRROR_COLOR);
                gizmos.circle(Isometry3d::new(center, Quat::from_rotation_x(FRAC_PI_2)), reach, MIRROR_COLOR);
                if self.symmetry == Symmetry::Rotate90 {
                    gizmos.line(center - Vec3::X * reach, center + Vec3::X * reach, MIRROR_COLOR);
                    gizmos.line(center - Vec3::Z * reach, center + Vec3::Z * reach, MIRROR_COLOR);
                }
            }
        }
        self.center.debug_gizmos(center, gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    /// The copies are children, so they went with the entity.
    fn set_entity(&mut self, entity: Option<Entity>) {
        if self.entity != entity {
            self.copy_entities.clear();
        }
        self.entity = entity;
    }

    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        // The identity, so a copy's `Transform` is where it is: spawn choice reads `Transform`.
        commands
            .entity(entity)
            .insert(MirrorSides {
                map: self.team_map(),
                sources: self.copies.iter().filter_map(|copy| copy.source_entity).collect(),
            })
            .insert_if_new((Transform::IDENTITY, Visibility::Inherited));

        let copies = &self.copies;
        self.copy_entities.retain(|(key, child)| {
            let kept = copies.iter().any(|copy| (copy.source, copy.image) == *key);
            if !kept {
                commands.entity(*child).despawn();
            }
            kept
        });

        for copy in &mut self.copies {
            let key = (copy.source, copy.image);
            let child = match self.copy_entities.iter().find(|(at, _)| *at == key) {
                Some((_, child)) => *child,
                None => {
                    let child = commands.spawn((MirrorCopy { image: copy.image }, ChildOf(entity))).id();
                    self.copy_entities.push((key, child));
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
        let mirrorings: Vec<Mirroring> = self.mirrorings().collect();
        let mut previous = std::mem::take(&mut self.copies);
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
            let images: Option<Vec<_>> =
                mirrorings.iter().map(|mirroring| mirrored_copy(source, features, mirroring)).collect();
            match images {
                Some(images) => self.copies.extend(images.into_iter().enumerate().map(|(image, object)| MirroredCopy {
                    source: *id,
                    image,
                    source_entity: source.object().entity(),
                    object,
                })),
                None => self.left_out.push((*id, LeftOut::NoMirrorImage)),
            }
        }
        inherit_nested(&mut self.copies, &mut previous);
    }

    /// Only the center moves: each copy is carried across on its own, and a copy is never resolved
    /// again, so its own symmetry is not consulted.
    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        self.resolved_center = mirroring.point(self.resolved_center);
        self.center.resolved_reference = None;
        self.is_copy = true;
        self.copies.retain_mut(|copy| copy.object.mirror(mirroring));
        true
    }

    fn mirror_mut(&mut self) -> Option<&mut Mirror> {
        Some(self)
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
        self.copies
            .iter()
            .flat_map(|copy| std::iter::once(&*copy.object).chain(copy.object.standing_copies()))
            .collect()
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

/// The one writer of [`Side`], keyed on what stands now so that deleting a Mirror, rolling back past
/// it or dropping a source unsides with nothing to remember. Compares before it writes, since it runs
/// every frame. Where two Mirrors claim one entity the earlier feature id wins; query order shifts
/// with archetypes.
pub fn assign_sides(
    mirrors: Query<(Entity, &MirrorSides, &FeatureTag)>,
    nested: Query<&MirrorSides>,
    children: Query<&Children>,
    copies: Query<&MirrorCopy>,
    sided: Query<(Entity, &Side)>,
    mut commands: Commands,
) {
    if mirrors.is_empty() && sided.is_empty() {
        return;
    }
    let mut ordered: Vec<_> = mirrors.iter().collect();
    ordered.sort_by_key(|(_, _, tag)| tag.feature_id);
    let mut wanted: HashMap<Entity, Team> = HashMap::new();
    for (entity, mirror, _) in ordered {
        for source in &mirror.sources {
            if !nested.contains(*source) {
                wanted.entry(*source).or_insert(mirror.map.teams[0]);
            }
        }
        claim_copies(entity, &mirror.map, &mut Vec::new(), &nested, &children, &copies, &mut wanted);
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

fn claim_copies(
    mirror: Entity,
    map: &TeamMap,
    around: &mut Vec<(TeamMap, usize)>,
    nested: &Query<&MirrorSides>,
    children: &Query<&Children>,
    copies: &Query<&MirrorCopy>,
    wanted: &mut HashMap<Entity, Team>,
) {
    let Ok(standing) = children.get(mirror) else { return };
    for (child, copy) in standing.iter().filter_map(|child| Some((child, copies.get(child).ok()?))) {
        if let Ok(inner) = nested.get(child) {
            around.push((map.clone(), copy.image));
            claim_copies(child, &inner.map, around, nested, children, copies, wanted);
            around.pop();
            continue;
        }
        let team = around
            .iter()
            .rev()
            .fold(map.becomes(map.teams[0], copy.image), |team, (outer, image)| outer.becomes(team, *image));
        wanted.entry(child).or_insert(team);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::editable::FeatureTimeline;
    use crate::editor::animation_display::AnimationDisplay;
    use crate::editor::editor_room::EditorRoom;
    use crate::editor::spawn_point::{SpawnPoint, SpawnPointMarker};

    fn at(motion: Motion, center: Vec3) -> Mirroring {
        Mirroring { motion, center }
    }

    /// Otherwise the copies are not copies of each other: two of a reflection or a half turn, four
    /// of a quarter turn.
    #[test]
    fn a_motion_repeated_comes_back_to_the_start() {
        let point = Vec3::new(3.0, 1.5, -7.0);
        let turn = Quat::from_euler(EulerRot::YXZ, 0.7, 0.3, -0.2);
        for (motion, period) in [(Motion::ReflectX, 2), (Motion::ReflectZ, 2), (Motion::Turn(2), 2), (Motion::Turn(1), 4), (Motion::Turn(3), 4)] {
            let m = at(motion, Vec3::new(1.0, 0.0, 2.0));
            let (mut p, mut r, mut y) = (point, turn, 0.9);
            for _ in 0..period {
                (p, r, y) = (m.point(p), m.rotation(r), m.yaw(y));
            }
            assert!(p.abs_diff_eq(point, 1e-5), "{motion:?}");
            assert!(r.abs_diff_eq(turn, 1e-5) || r.abs_diff_eq(-turn, 1e-5), "{motion:?}");
            assert!((Quat::from_rotation_y(y) * Vec3::Z).abs_diff_eq(Quat::from_rotation_y(0.9) * Vec3::Z, 1e-5), "{motion:?}");
        }
    }

    /// The exact turn agrees with the quaternion every rotation is built from.
    #[test]
    fn a_quarter_turn_is_the_rotation_it_claims() {
        let v = Vec3::new(3.0, 1.5, -7.0);
        for quarters in 0..4u8 {
            let exact = at(Motion::Turn(quarters), Vec3::ZERO).direction(v);
            let rotated = Quat::from_rotation_y(f32::from(quarters) * FRAC_PI_2) * v;
            assert!(exact.abs_diff_eq(rotated, 1e-5), "{quarters}: {exact} {rotated}");
        }
    }

    #[test]
    fn a_facing_is_carried_across() {
        let facing = |yaw: f32| Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
        let east = -FRAC_PI_2;
        assert!(facing(east).abs_diff_eq(Vec3::X, 1e-5));

        let x = at(Motion::ReflectX, Vec3::ZERO);
        assert!(facing(x.yaw(east)).abs_diff_eq(Vec3::NEG_X, 1e-5));
        assert!(facing(x.yaw(0.0)).abs_diff_eq(Vec3::NEG_Z, 1e-5), "facing along the plane is unchanged");

        let z = at(Motion::ReflectZ, Vec3::ZERO);
        assert!(facing(z.yaw(0.0)).abs_diff_eq(Vec3::Z, 1e-5));
        assert!(facing(z.yaw(east)).abs_diff_eq(Vec3::X, 1e-5));

        let turned = at(Motion::Turn(2), Vec3::ZERO);
        assert!(facing(turned.yaw(east)).abs_diff_eq(Vec3::NEG_X, 1e-5));

        let quarter = at(Motion::Turn(1), Vec3::ZERO);
        assert!(facing(quarter.yaw(east)).abs_diff_eq(Vec3::NEG_Z, 1e-5));
    }

    /// A prop's forward and up go where the reflection takes them, and it is still a rotation.
    #[test]
    fn a_mirrored_turn_keeps_forward_and_up() {
        let turn = Quat::from_euler(EulerRot::YXZ, 0.7, 0.3, -0.2);
        for motion in [Motion::ReflectX, Motion::ReflectZ, Motion::Turn(1), Motion::Turn(3)] {
            let m = at(motion, Vec3::ZERO);
            let copy = m.rotation(turn);
            assert!((copy * Vec3::NEG_Z).abs_diff_eq(m.direction(turn * Vec3::NEG_Z), 1e-5), "{motion:?}");
            assert!((copy * Vec3::Y).abs_diff_eq(m.direction(turn * Vec3::Y), 1e-5), "{motion:?}");
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

    /// A type with no mirror image is said to be left out rather than dropped.
    #[test]
    fn what_cannot_be_mirrored_is_left_out() {
        let mut timeline = FeatureTimeline::default();
        let display = timeline.apply_feature(Box::new(AnimationDisplay::new(1.0, 0.0, 0.0)));
        let mut mirror = Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_sources(vec![display]);
        mirror.resolve_references(timeline.features_map());
        assert!(mirror.standing_copies().is_empty());
        assert_eq!(mirror.left_out, vec![(display, LeftOut::NoMirrorImage)]);
    }

    fn four_ways(second: [Team; 2]) -> App {
        four_ways_remapping(second, None)
    }

    fn four_ways_remapping(second: [Team; 2], remap: Option<(Team, Team)>) -> App {
        let (mut timeline, room, spawn) = spawn_and_room();
        let across_x = timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_sources(vec![room, spawn]),
        ));
        timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 5.0))
                .with_symmetry(Symmetry::MirrorZ)
                .with_sources(vec![room, spawn, across_x])
                .with_teams(second[0], second[1]),
        ));
        if let Some((from, to)) = remap {
            let outer = *timeline.feature_order().last().unwrap();
            timeline.features_mut().get_mut(&outer).unwrap().object_mut().mirror_mut().unwrap().remap.insert(from, to);
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(timeline);
        app.add_systems(Update, (FeatureTimeline::sync_entities, assign_sides).chain());
        app.update();
        app.update();
        app
    }

    fn spawns(app: &mut App) -> Vec<(Vec3, Option<Team>)> {
        let world = app.world_mut();
        world
            .query_filtered::<(&Transform, Option<&Side>), With<SpawnPointMarker>>()
            .iter(world)
            .map(|(at, side)| (at.translation, side.map(|s| s.0)))
            .collect()
    }

    fn side_at(spawns: &[(Vec3, Option<Team>)], at: Vec3) -> Option<Team> {
        spawns
            .iter()
            .find(|(here, _)| here.abs_diff_eq(at, 1e-4))
            .unwrap_or_else(|| panic!("nothing at {at}: {spawns:?}"))
            .1
    }

    /// By default the second Mirror's pair swaps the first's copies, giving a checkerboard.
    #[test]
    fn a_mirror_of_a_mirror_makes_four_quarters() {
        let mut app = four_ways([Team::Red, Team::Blue]);
        app.update();
        let found = spawns(&mut app);
        assert_eq!(found.len(), 4, "{found:?}");
        assert_eq!(side_at(&found, Vec3::new(4.0, 0.0, 0.0)), Some(Team::Red));
        assert_eq!(side_at(&found, Vec3::new(-4.0, 0.0, 0.0)), Some(Team::Blue));
        assert_eq!(side_at(&found, Vec3::new(4.0, 0.0, 10.0)), Some(Team::Blue));
        assert_eq!(side_at(&found, Vec3::new(-4.0, 0.0, 10.0)), Some(Team::Red));

        let world = app.world_mut();
        assert_eq!(world.query::<&crate::tool::room::Room>().iter(world).count(), 4);
    }

    #[test]
    fn a_remap_puts_four_quarters_on_four_teams() {
        let mut app = four_ways_remapping([Team::Red, Team::Yellow], Some((Team::Blue, Team::Green)));
        let found = spawns(&mut app);
        assert_eq!(side_at(&found, Vec3::new(4.0, 0.0, 0.0)), Some(Team::Red));
        assert_eq!(side_at(&found, Vec3::new(-4.0, 0.0, 0.0)), Some(Team::Blue));
        assert_eq!(side_at(&found, Vec3::new(4.0, 0.0, 10.0)), Some(Team::Yellow));
        assert_eq!(side_at(&found, Vec3::new(-4.0, 0.0, 10.0)), Some(Team::Green));
    }

    /// The originals' team gets no row: the copies' team already answers it.
    #[test]
    fn a_mirror_offers_the_teams_its_nested_copies_bring() {
        let (mut timeline, room, spawn) = spawn_and_room();
        let inner = timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_sources(vec![room, spawn]),
        ));
        let mut outer = Mirror::new(PointRef::absolute(0.0, 0.0, 5.0))
            .with_symmetry(Symmetry::MirrorZ)
            .with_sources(vec![room, inner])
            .with_teams(Team::Red, Team::Yellow)
            .remapping(Team::Blue, Team::Green);
        outer.resolve_references(timeline.features_map());
        assert_eq!(outer.arriving_teams(), vec![Team::Blue]);
        let mut sides = outer.copy_sides();
        sides.sort();
        assert_eq!(sides, vec![Team::Yellow, Team::Green, Team::Green]);
    }

    #[test]
    fn a_remap_survives_a_save() {
        let mirror = Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).remapping(Team::Blue, Team::Green);
        let text = ron::to_string(&mirror).unwrap();
        let back: Mirror = ron::from_str(&text).unwrap();
        assert_eq!(back.remap, mirror.remap);
        let plain: Mirror = ron::from_str(&ron::to_string(&Mirror::new(PointRef::absolute(0.0, 0.0, 0.0))).unwrap()).unwrap();
        assert!(plain.remap.is_empty());
    }

    #[test]
    fn a_mirror_onto_one_team_keeps_the_inner_split() {
        let mut app = four_ways([Team::Red, Team::Red]);
        let found = spawns(&mut app);
        assert_eq!(side_at(&found, Vec3::new(4.0, 0.0, 10.0)), Some(Team::Red));
        assert_eq!(side_at(&found, Vec3::new(-4.0, 0.0, 10.0)), Some(Team::Blue));
    }

    fn standing(app: &mut App) -> Vec<Entity> {
        let world = app.world_mut();
        let mut found: Vec<Entity> = world
            .query_filtered::<Entity, Or<(With<MirrorCopy>, With<SpawnPointMarker>, With<crate::tool::room::Room>)>>()
            .iter(world)
            .collect();
        found.sort();
        found
    }

    /// The copy of the inner Mirror is rebuilt on every resolve; without taking over its
    /// predecessor's entities, each edit stands another set.
    #[test]
    fn an_edit_reuses_the_nested_copies() {
        let mut app = four_ways([Team::Red, Team::Blue]);
        let before = standing(&mut app);
        let reapply = |mut timeline: ResMut<FeatureTimeline>, mut commands: Commands| {
            for id in timeline.feature_order().to_vec() {
                let mut feature = timeline.features_mut().remove(&id).unwrap();
                feature.object_mut().resolve_references(timeline.features_map());
                if let Some(entity) = feature.object().entity() {
                    feature.object_mut().apply_to_entity(&mut commands, entity);
                }
                timeline.features_mut().insert(id, feature);
            }
        };
        app.add_systems(PostUpdate, reapply);
        for step in 0..3 {
            {
                let mut timeline = app.world_mut().resource_mut::<FeatureTimeline>();
                let room = timeline.feature_order()[0];
                let corner = timeline.features_mut().get_mut(&room).unwrap().object_mut().get_point_ref_mut("max").unwrap();
                assert!(corner.drag_axis(0, 7.0 + step as f32));
            }
            app.update();
        }
        assert_eq!(before, standing(&mut app));

        let world = app.world_mut();
        let far_room = world
            .query::<&crate::tool::room::Room>()
            .iter(world)
            .any(|room| (room.bounds().0.x + 9.0).abs() < 1e-4);
        assert!(far_room, "the edit reached the copy of a copy");
    }

    fn quartered(teams: Option<[Team; 4]>) -> App {
        let (mut timeline, room, spawn) = spawn_and_room();
        let mut mirror = Mirror::new(PointRef::absolute(0.0, 0.0, 0.0))
            .with_symmetry(Symmetry::Rotate90)
            .with_sources(vec![room, spawn]);
        if let Some(teams) = teams {
            mirror = mirror.with_four_teams(teams);
        }
        timeline.apply_feature(Box::new(mirror));

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(timeline);
        app.add_systems(Update, (FeatureTimeline::sync_entities, assign_sides).chain());
        app.update();
        app.update();
        app
    }

    const QUARTERS: [Vec3; 4] =
        [Vec3::new(4.0, 0.0, 0.0), Vec3::new(0.0, 0.0, -4.0), Vec3::new(-4.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 4.0)];

    /// Unless told otherwise the quarters alternate between the two teams, so opposite quarters
    /// are on one side.
    #[test]
    fn a_quarter_turn_stands_three_copies_on_alternating_sides() {
        let mut app = quartered(None);
        let found = spawns(&mut app);
        assert_eq!(found.len(), 4, "{found:?}");
        let sides: Vec<_> = QUARTERS.iter().map(|at| side_at(&found, *at)).collect();
        assert_eq!(sides, [Some(Team::Red), Some(Team::Blue), Some(Team::Red), Some(Team::Blue)]);

        let world = app.world_mut();
        let rooms: Vec<crate::tool::room::Room> = world.query::<&crate::tool::room::Room>().iter(world).cloned().collect();
        assert_eq!(rooms.len(), 4);
        assert!(rooms.iter().any(|room| room.bounds().0.abs_diff_eq(Vec3::new(-1.0, 0.0, -6.0), 1e-5)), "the quarter turn's room");

        let yaw = world
            .query_filtered::<&Transform, With<SpawnPointMarker>>()
            .iter(world)
            .find(|at| at.translation.abs_diff_eq(QUARTERS[1], 1e-4))
            .map(|at| at.rotation * Vec3::NEG_Z)
            .unwrap();
        assert!(yaw.abs_diff_eq(Vec3::NEG_Z, 1e-5), "faces away from the axis, as the original does: {yaw}");
    }

    #[test]
    fn a_quarter_turn_takes_four_teams_or_any_repeat_of_them() {
        for teams in [
            [Team::Red, Team::Blue, Team::Yellow, Team::Green],
            [Team::Red, Team::Blue, Team::Blue, Team::Green],
            [Team::Red, Team::Red, Team::Blue, Team::Blue],
        ] {
            let mut app = quartered(Some(teams));
            let found = spawns(&mut app);
            let sides: Vec<_> = QUARTERS.iter().map(|at| side_at(&found, *at)).collect();
            assert_eq!(sides, teams.map(Some), "{teams:?}");
        }
    }

    /// A two-way Mirror's file does not change, and a four-way one's turned teams come back.
    #[test]
    fn four_teams_survive_a_save() {
        let alternating = Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_symmetry(Symmetry::Rotate90);
        assert!(!ron::to_string(&alternating).unwrap().contains("turned_teams"));
        let four = alternating.with_four_teams([Team::Red, Team::Blue, Team::Yellow, Team::Green]);
        let back: Mirror = ron::from_str(&ron::to_string(&four).unwrap()).unwrap();
        assert_eq!(back.teams(), vec![Team::Red, Team::Blue, Team::Yellow, Team::Green]);
    }

    /// A quarter-turned Mirror of a two-way one: each quarter's copy of the inner pair moves round
    /// the outer cycle as far as its quarter does.
    #[test]
    fn a_quarter_turn_of_a_mirror_moves_its_sides_round() {
        let (mut timeline, room, spawn) = spawn_and_room();
        let inner = timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 2.0))
                .with_symmetry(Symmetry::MirrorZ)
                .with_sources(vec![room, spawn])
                .with_teams(Team::Red, Team::Blue),
        ));
        let mut outer = Mirror::new(PointRef::absolute(0.0, 0.0, 0.0))
            .with_symmetry(Symmetry::Rotate90)
            .with_sources(vec![room, spawn, inner])
            .with_four_teams([Team::Red, Team::Blue, Team::Yellow, Team::Green]);
        outer.resolve_references(timeline.features_map());
        assert!(outer.arriving_teams().is_empty(), "a remap row would have to name a quarter");
        let mut sides = outer.copy_sides();
        sides.sort();
        // Red's copies go Blue, Yellow, Green; the inner Blue's go Yellow, Green, Red. Two of each,
        // for the room and the spawn.
        let mut expected = [Team::Blue, Team::Yellow, Team::Green, Team::Yellow, Team::Green, Team::Red].repeat(2);
        expected.sort();
        assert_eq!(sides, expected);
    }

    #[test]
    fn mirrors_listing_each_other_do_not_recurse_forever() {
        let mut timeline = FeatureTimeline::default();
        let first = timeline.apply_feature(Box::new(Mirror::new(PointRef::absolute(0.0, 0.0, 0.0))));
        let second = timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 5.0)).with_sources(vec![first]),
        ));
        let mut cyclic = Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_sources(vec![second]);
        cyclic.resolve_references(timeline.features_map());
        timeline.features_mut().get_mut(&first).unwrap().object_mut().mirror_mut().unwrap().sources = vec![second];
        cyclic.resolve_references(timeline.features_map());
    }
}
