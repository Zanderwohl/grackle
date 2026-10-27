use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy_egui::egui;
use lazy_static::lazy_static;
use serde::{Serialize, Deserialize};
use crate::common::app_mode::AppMode;
use crate::common::shortcuts::chords;
use crate::common::PointResolutionError;
use crate::constants::MAP_BLUEPRINT_EXTENSION;
use crate::common::movement_rules::MovementRules;
use crate::common::palette::{publish_palette, Palette};
use crate::editor::map_metadata::{publish_game_rules, publish_movement_rules, MapMetadata};
use crate::editor::mirror::{assign_sides, Mirroring};
use crate::editor::save;
use crate::get;

lazy_static! {
    static ref MAP_EXT: String = "gmp".to_owned(); // Grackle MaP
    static ref MAP_ART: String = "gma".to_owned(); // Grackle Map Artifact
}

pub struct EditorStepsPlugin;
impl Plugin for EditorStepsPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<FeatureTimeline>()
            .init_resource::<MapMetadata>()
            .init_resource::<MovementRules>()
            .init_resource::<crate::common::game_rules::Modes>()
            .init_resource::<crate::common::game_rules::GameRules>()
            .add_message::<EditEvent>()
            .add_systems(Startup, load_startup_blueprint)
            .init_resource::<Palette>()
            .add_systems(Update, (publish_movement_rules, publish_game_rules, publish_palette))
            // A round is played on the whole map. On the transition rather than `OnEnter(Play)`, so the
            // features past the bar have entities before the bake and collision rebuild run there.
            .add_systems(
                OnTransition { exited: AppMode::Editor, entered: AppMode::Play },
                (
                    |mut features: ResMut<FeatureTimeline>| features.roll_back_to(None),
                    FeatureTimeline::sync_entities,
                )
                    .chain(),
            )
            // `sync_entities`, `handle_edits` and `assign_sides` are deliberately ungated so an edit lands
            // mid-match.
            .add_systems(Update, (
                FeatureTimeline::undo_redo_shortcuts.run_if(in_state(AppMode::Editor)),
                FeatureTimeline::sync_entities,
                FeatureTimeline::handle_edits,
                assign_sides,
                FeatureTimeline::draw_affected_gizmos.run_if(in_state(AppMode::Editor)),
                checkpoint_when_idle.run_if(in_state(AppMode::Editor)),
            ).chain())
        ;
    }
}

/// Idle is no mouse button held and no text field holding the keyboard, so a drag or a typed name is
/// one step. Through `bypass_change_detection`, or this system's own write would read as a change
/// next frame and it would checkpoint forever.
fn checkpoint_when_idle(
    mut features: ResMut<FeatureTimeline>,
    mouse: Res<ButtonInput<MouseButton>>,
    egui: Res<bevy_egui::input::EguiWantsInput>,
    mut pending: Local<bool>,
) {
    *pending |= features.is_changed();
    let idle = mouse.get_pressed().next().is_none() && !egui.wants_keyboard_input();
    if *pending && idle {
        *pending = false;
        features.bypass_change_detection().checkpoint();
    }
}

fn load_startup_blueprint(
    mut features: ResMut<FeatureTimeline>,
    mut map_metadata: ResMut<MapMetadata>,
) {
    let path = PathBuf::from(format!(
        "assets/default/blueprints/new.{}",
        MAP_BLUEPRINT_EXTENSION
    ));
    match save::load(&path) {
        Ok(loaded) => {
            *features = loaded.timeline;
            *map_metadata = loaded.metadata;
            info!("Loaded template from {:?}", path);
        }
        Err(e) => error!("Failed to load template {:?}: {}", path, e),
    }
}

#[derive(Component, Debug)]
pub struct FeatureTag {
    pub feature_id: u64,
}

#[typetag::serde]
pub trait FeatureTrait: Send + Sync {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError>;
    /// Returns true if the object was modified this frame.
    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool;
    fn type_name(&self) -> String;
    fn type_key(&self) -> &'static str;
    fn debug_gizmos(&self, gizmos: &mut Gizmos);
    /// Drawn on top of `debug_gizmos` while this feature is the one selected.
    fn selected_gizmos(&self, _gizmos: &mut Gizmos) {}
    /// Points of this feature's own that the point ref under `point_ref_key`
    /// may be measured from, by key — which no other feature can name.
    fn own_bases(&self, _point_ref_key: &str) -> Vec<(String, Vec3)> { Vec::new() }
    /// Measure `point_ref_key` from its own `base`, keeping where it resolves.
    fn rebase_on_own(&mut self, _point_ref_key: &str, _base: &str) -> bool { false }
    fn entity(&self) -> Option<Entity>;
    fn set_entity(&mut self, entity: Option<Entity>);
    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity);
    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>);
    fn parent_ids(&self) -> Vec<FeatureId>;
    fn available_point_keys(&self) -> Vec<(String, String)>;
    /// Point-like objects always return their location; volumetric ones test the ray against their AABB
    /// first.
    fn reference_points_for_ray(&self, ray: &Ray3d) -> Vec<(String, Vec3)>;

    /// For drag handles. `axis`: 0=X, 1=Y, 2=Z. `new_world_value` is in world space. Returns true if
    /// modified.
    fn drag_handle(&mut self, _is_max: bool, _axis: u8, _new_world_value: f32) -> bool { false }

    /// Boxes this feature can be clicked by, beside its point. Computed from the feature alone, never
    /// its entities, so it agrees on every machine.
    fn pick_boxes(&self) -> Vec<crate::common::ray::PickBox> { Vec::new() }

    /// Resolved bounds of a room-like object, for placing drag handles.
    fn drag_handle_bounds(&self) -> Option<(Vec3, Vec3)> { None }

    fn get_point_ref(&self, _key: &str) -> Option<&PointRef> { None }

    /// Keys: GlobalPoint/GracklePointLight use "location" (or ""), EditorRoom uses "min" / "max".
    fn get_point_ref_mut(&mut self, _key: &str) -> Option<&mut PointRef> { None }

    /// Which Euler components (0=X pitch, 1=Y yaw, 2=Z roll) the user may turn. Empty means no facing
    /// and no rotation rings.
    fn rotation_axes(&self) -> Vec<u8> { vec![] }

    /// Radians, in axis order; see [`crate::common::rotation`].
    fn euler_angles(&self) -> Vec3 { Vec3::ZERO }

    /// Radians. Returns false for an axis the feature does not expose.
    fn set_euler_angle(&mut self, _axis: u8, _radians: f32) -> bool { false }

    /// Carry the resolved geometry across `mirroring`, on a copy a [`Mirror`] has just resolved. False
    /// for a type with no mirror image yet, which the Mirror leaves out.
    ///
    /// [`Mirror`]: crate::editor::mirror::Mirror
    fn mirror(&mut self, _mirroring: &Mirroring) -> bool { false }

    /// The volumes of air this feature stands up, as its entities carry them.
    /// Asked by picking, which works from features rather than entities.
    fn rooms(&self) -> Vec<crate::tool::room::Room> { Vec::new() }

    /// The footprint this feature is, for the tools that edit a footprint's
    /// own drawing. Two methods rather than a downcast because a typetag
    /// trait object has no `Any`.
    fn footprint(&self) -> Option<&crate::editor::footprint_room::FootprintRoom> { None }
    fn footprint_mut(&mut self) -> Option<&mut crate::editor::footprint_room::FootprintRoom> { None }

    /// What this feature stands in the world beyond itself, as features: a Mirror's copies.
    fn standing_copies(&self) -> Vec<&dyn FeatureTrait> { Vec::new() }
}

enum RowAction {
    Delete(FeatureId),
    RollBackTo(Option<FeatureId>),
}

#[derive(Message)]
pub struct EditEvent {
    pub editor_id: u64,
    pub feature_id: FeatureId,
    pub entity: Entity,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Debug, Clone, Copy)]
#[derive(Hash)]
pub struct FeatureId {
    _id: u64,
}

#[derive(Resource)]
pub struct FeatureTimeline {
    features: HashMap<FeatureId, Feature>,
    feature_order: Vec<FeatureId>,
    id_counter: u64,
    selected_feature: Option<FeatureId>,
    /// The selected feature and all its descendants, parents before dependants.
    selection_affected: Option<Vec<FeatureId>>,
    /// The rollback bar: the last feature shown, `None` for the whole map. An id rather than a count, so
    /// an edit above it cannot move it. View state: not saved, not sent, not in undo snapshots.
    rollback: Option<FeatureId>,
    pending_despawns: Vec<Entity>,
    /// Whole-map snapshots, oldest first. In memory only.
    undo: Vec<MapState>,
    redo: Vec<MapState>,
    /// The map as of the last checkpoint: what undo goes back from and the next checkpoint compares
    /// against.
    checkpoint: Option<MapState>,
    saved: Option<MapState>,
    /// Kept rather than computed, because computing it means serializing the map.
    dirty: bool,
}

/// JSON because it is the fastest encoding `typetag` gives; these never reach a file.
fn serialize_feature(feature: &Feature) -> String {
    serde_json::to_string(feature).unwrap_or_default()
}

const UNDO_DEPTH: usize = 200;

/// Every feature's serialized form, in order. Serialized because a feature is a trait object, and so
/// a restore can compare strings and rebuild only what differs. Stable because everything cached
/// from the world is `#[serde(skip)]`.
#[derive(Clone, PartialEq, Debug)]
struct MapState {
    features: Vec<(FeatureId, String)>,
    id_counter: u64,
}

impl Default for FeatureTimeline {
    fn default() -> Self {
        Self {
            features: HashMap::new(),
            feature_order: vec![],
            id_counter: 0,
            selected_feature: None,
            selection_affected: None,
            rollback: None,
            pending_despawns: vec![],
            undo: vec![],
            redo: vec![],
            checkpoint: None,
            saved: None,
            dirty: false,
        }
    }
}

impl FeatureTimeline {
    /// No rollback and no history: what loading a file and adopting a server's map both mean.
    pub fn from_parts(
        features: HashMap<FeatureId, Feature>,
        feature_order: Vec<FeatureId>,
        id_counter: u64,
    ) -> Self {
        let mut timeline = Self {
            features,
            feature_order,
            id_counter,
            ..Default::default()
        };
        timeline.reset_history();
        timeline
    }

    /// Replace the open map. The old entities' despawns are queued for `sync_entities` to perform with
    /// the incoming spawns; dropping them strands the old rooms in the world.
    ///
    /// The caller must re-bake room geometry a frame later: entities do not exist until
    /// `sync_entities` has run.
    pub fn adopt(&mut self, incoming: FeatureTimeline) {
        let old: Vec<Entity> = self
            .active_features()
            .filter_map(|(_, feature)| feature.object().entity())
            .collect();
        *self = incoming;
        for entity in old {
            self.queue_despawn(entity);
        }
        self.select(None);
        // History too: undoing through an adoption would edit a different map.
        self.reset_history();
    }

    /// Undoable through the next checkpoint.
    pub fn delete_feature(&mut self, feature_id: FeatureId) {
        self.remove_feature_internal(feature_id);
    }

    pub fn id_counter(&self) -> u64 {
        self.id_counter
    }

    pub fn rollback(&self) -> Option<FeatureId> {
        self.rollback
    }

    /// How many features, from the top, are in force.
    pub fn shown_len(&self) -> usize {
        self.rollback
            .and_then(|id| self.feature_order.iter().position(|candidate| *candidate == id))
            .map_or(self.feature_order.len(), |at| at + 1)
    }

    /// A bar after the last feature is stored as `None`, so "everything" has one form.
    pub fn roll_back_to(&mut self, at: Option<FeatureId>) {
        let at = at.filter(|id| {
            self.features.contains_key(id) && self.feature_order.last() != Some(id)
        });
        if at == self.rollback {
            return;
        }
        self.rollback = at;
        self.deselect_past_the_bar();
    }

    /// A selection past the bar has no entity to edit against.
    fn deselect_past_the_bar(&mut self) {
        let shown = self.shown_len();
        if let Some(selected) = self.selected_feature
            && self.feature_order.iter().position(|id| *id == selected).is_none_or(|at| at >= shown)
        {
            self.select(None);
        }
    }

    pub fn next_id(&mut self) -> FeatureId {
        let id = FeatureId { _id: self.id_counter };
        self.id_counter += 1;
        id
    }
    
    pub fn select(&mut self, selection: Option<FeatureId>) {
        self.selected_feature = selection;
        self.selection_affected = selection.map(|root| {
            let mut result = vec![root];
            let mut visited: HashSet<FeatureId> = HashSet::from([root]);
            let mut queue = vec![root];

            while !queue.is_empty() {
                let parent_set: HashSet<FeatureId> = queue.drain(..).collect();

                let children: Vec<FeatureId> = self.features.iter()
                    .filter(|(_, feature)| feature.parents.iter().any(|p| parent_set.contains(p)))
                    .map(|(id, _)| *id)
                    .filter(|id| visited.insert(*id))
                    .collect();

                result.extend(&children);
                queue = children;
            }

            result
        });
    }

    pub fn selection_affected(&self) -> Option<&[FeatureId]> {
        self.selection_affected.as_deref()
    }

    /// Features before the rollback bar.
    pub fn active_features(&self) -> impl Iterator<Item = (FeatureId, &Feature)> {
        let shown = self.shown_len();
        self.feature_order[..shown].iter().filter_map(move |id| {
            self.features.get(id).map(|a| (*id, a))
        })
    }

    pub fn selected_feature(&self) -> Option<FeatureId> {
        self.selected_feature
    }

    pub fn features_map(&self) -> &HashMap<FeatureId, Feature> {
        &self.features
    }

    pub fn feature_order(&self) -> &[FeatureId] {
        &self.feature_order
    }

    pub fn features_mut(&mut self) -> &mut HashMap<FeatureId, Feature> {
        &mut self.features
    }

    pub fn queue_despawn(&mut self, entity: Entity) {
        self.pending_despawns.push(entity);
    }

    /// Add at the rollback bar and move the bar past it, or at the end with no bar.
    pub fn apply_feature(&mut self, feature_object: Box<dyn FeatureTrait>) -> FeatureId {
        let parents = feature_object.parent_ids();
        let new_id = self.next_id();
        let new_feature = Feature {
            id: new_id,
            object: feature_object,
            parents,
        };
        self.features.insert(new_feature.id, new_feature);
        let at = self.shown_len();
        self.feature_order.insert(at, new_id);
        if self.rollback.is_some() {
            self.rollback = Some(new_id);
        }

        new_id
    }
    
    pub fn get_feature(&self, id: &FeatureId) -> Option<&Feature> {
        self.features.get(id)
    }

    fn state(&self) -> MapState {
        MapState {
            features: self
                .feature_order
                .iter()
                .filter_map(|id| self.features.get(id))
                .map(|feature| (feature.id, serialize_feature(feature)))
                .collect(),
            id_counter: self.id_counter,
        }
    }

    /// Record one undo step if the map changed since the last checkpoint. Called when idle, not by each
    /// edit, so no tool can forget to record itself.
    pub fn checkpoint(&mut self) {
        let now = self.state();
        match self.checkpoint.take() {
            Some(before) if before != now => self.push_undo(before),
            _ => {}
        }
        self.dirty = self.saved.as_ref() != Some(&now);
        self.checkpoint = Some(now);
    }

    /// The one place history grows, so the one place it is capped at [`UNDO_DEPTH`].
    fn push_undo(&mut self, before: MapState) {
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// For a map opened from a file or handed over by a server; on a client, undo would edit a map it
    /// does not own.
    pub fn reset_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        let now = self.state();
        self.checkpoint = Some(now.clone());
        self.saved = Some(now);
        self.dirty = false;
    }

    pub fn mark_saved(&mut self) {
        let now = self.state();
        self.checkpoint = Some(now.clone());
        self.saved = Some(now);
        self.dirty = false;
    }

    /// As of the last checkpoint.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        // An edit not yet checkpointed is the one this undo takes back.
        self.checkpoint();
        let Some(previous) = self.undo.pop() else { return };
        if let Some(current) = self.checkpoint.take() {
            self.redo.push(current);
        }
        self.restore(&previous);
    }

    pub fn redo(&mut self) {
        self.checkpoint();
        let Some(next) = self.redo.pop() else { return };
        if let Some(current) = self.checkpoint.take() {
            self.undo.push(current);
        }
        self.restore(&next);
    }

    /// One undo step, as reverting to the file is. Leaves the map clean.
    pub fn replace_as_edit(&mut self, incoming: FeatureTimeline) {
        self.checkpoint();
        let target = incoming.state();
        if let Some(current) = self.checkpoint.take()
            && current != target
        {
            self.push_undo(current);
        }
        self.restore(&target);
        self.mark_saved();
    }

    /// Rebuild only the features whose serialized form differs, and everything built on them.
    fn restore(&mut self, target: &MapState) {
        let mut changed: Vec<FeatureId> = Vec::new();
        let mut next: HashMap<FeatureId, Feature> = HashMap::new();
        let mut order = Vec::with_capacity(target.features.len());

        for (id, text) in &target.features {
            order.push(*id);
            let unchanged = self
                .features
                .get(id)
                .is_some_and(|feature| serialize_feature(feature) == *text);
            if unchanged {
                if let Some(feature) = self.features.remove(id) {
                    next.insert(*id, feature);
                }
                continue;
            }
            match serde_json::from_str::<Feature>(text) {
                Ok(feature) => {
                    if let Some(old) = self.features.remove(id)
                        && let Some(entity) = old.object.entity()
                    {
                        self.pending_despawns.push(entity);
                    }
                    next.insert(*id, feature);
                    changed.push(*id);
                }
                // Cannot happen for a string this process wrote; losing one feature beats losing the undo.
                Err(error) => error!("Undo could not rebuild feature {id:?}: {error}"),
            }
        }

        for (_, feature) in self.features.drain() {
            if let Some(entity) = feature.object.entity() {
                self.pending_despawns.push(entity);
            }
        }

        self.features = next;
        self.feature_order = order;
        self.id_counter = target.id_counter;
        for id in changed {
            self.queue_refresh_for_feature_and_descendants(id);
        }
        // Resolved now rather than by `sync_entities`, or a gizmo or the panel would read the origin for a
        // frame.
        for id in self.feature_order.clone() {
            if let Some(mut feature) = self.features.remove(&id) {
                feature.object.resolve_references(&self.features);
                self.features.insert(id, feature);
            }
        }

        self.forget_a_missing_bar();
        let selected = self.selected_feature.filter(|id| self.features.contains_key(id));
        self.select(selected);

        self.dirty = self.saved.as_ref() != Some(target);
        self.checkpoint = Some(target.clone());
    }

    fn forget_a_missing_bar(&mut self) {
        if self.rollback.is_some_and(|id| !self.features.contains_key(&id)) {
            self.rollback = None;
        }
        self.deselect_past_the_bar();
    }

    fn queue_refresh_for_feature_and_descendants(&mut self, root: FeatureId) {
        let mut stack = vec![root];
        let mut visited: HashSet<FeatureId> = HashSet::new();

        while let Some(current) = stack.pop() {
            if !visited.insert(current) {
                continue;
            }

            if let Some(mut feature) = self.features.remove(&current) {
                if let Some(entity) = feature.object().entity() {
                    self.pending_despawns.push(entity);
                    feature.object_mut().set_entity(None);
                }
                self.features.insert(current, feature);
            }

            for (child_id, child) in self.features.iter() {
                if child.parents().contains(&current) {
                    stack.push(*child_id);
                }
            }
        }
    }

    fn remove_feature_internal(&mut self, feature_id: FeatureId) {
        // Rebuilt rather than left standing on a parent that is gone: a Mirror would keep the copy.
        self.queue_refresh_for_feature_and_descendants(feature_id);
        if let Some(order_index) = self.feature_order.iter().position(|id| *id == feature_id) {
            self.feature_order.remove(order_index);
        }

        if let Some(feature) = self.features.remove(&feature_id) {
            if let Some(entity) = feature.object.entity() {
                self.pending_despawns.push(entity);
            }
        }

        if self.selected_feature == Some(feature_id) {
            self.select(None);
        } else if self.selected_feature.is_some() {
            self.select(self.selected_feature);
        }

        self.forget_a_missing_bar();
    }
    
    /// The Features panel. Says whether it changed the timeline, because the
    /// caller hands it one with change detection bypassed — see
    /// `EditorPanels::ui`.
    pub fn ui(
        ui: &mut egui::Ui,
        features: &mut Self,
        edit_events: &mut Vec<EditEvent>,
        retarget_out: &mut Option<(FeatureId, String)>,
    ) -> bool {
        // In a bottom panel so it pins to the bottom; the list scrolls in what is left.
        let mut was_edited = false;
        let mut edited_id = None;
        let mut entity_for_event: Option<Entity> = None;

        if let Some(selected_id) = features.selected_feature {
            let selected_idx = features.feature_order.iter()
                .position(|id| *id == selected_id);
            let is_active = selected_idx.is_some_and(|idx| idx < features.shown_len());

            if is_active {
                let selected_idx = selected_idx.unwrap();
                let prior_order: Vec<FeatureId> = features.feature_order[..selected_idx].to_vec();

                // Resizable, opening at half the tab; bounds are relative to the tab so neither half can vanish.
                let available = ui.available_height();
                egui::Panel::bottom("editor_feature_panel")
                    .resizable(true)
                    .default_size(feature_editor_default_height(available))
                    .size_range(feature_editor_height_range(available))
                    .show(ui, |ui| {
                        ui.separator();
                        if let Some(mut feature) = features.features.remove(&selected_id) {
                            let mut retarget_request: Option<String> = None;
                            // Padded here rather than in each `editor_ui`, so every feature type's panel looks the same.
                            let edited = feature_editor_frame(ui, |ui| {
                                ui.heading(feature.type_name_with_id());
                                ui.add_space(FEATURE_EDITOR_PADDING * 0.5);
                                feature.object_mut().editor_ui(ui, &features.features, &prior_order, &mut retarget_request)
                            });
                            if let Some(label) = retarget_request {
                                *retarget_out = Some((selected_id, label));
                            }
                            if edited {
                                feature.parents = feature.object().parent_ids().to_vec();
                                entity_for_event = feature.object().entity();
                                was_edited = true;
                                edited_id = Some(selected_id);
                            }
                            features.features.insert(selected_id, feature);
                        }
                    });
            }
        }

        if was_edited {
            if let Some(id) = edited_id {
                features.select(Some(id));
                if let Some(entity) = entity_for_event {
                    edit_events.push(EditEvent {
                        editor_id: id._id(),
                        feature_id: id,
                        entity,
                    });
                }
            }
        }

        let mut selection_changed = false;
        let mut next_selected = features.selected_feature;
        let pending: Rc<RefCell<Option<RowAction>>> = Rc::new(RefCell::new(None));
        let shown = features.shown_len();
        let rolled_back = features.rollback.is_some();
        let mut rows: Vec<(FeatureId, egui::Rect)> = Vec::new();
        let mut bar_dragged = false;

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, id) in features.feature_order.iter().enumerate() {
                let feature = features.get_feature(id).unwrap();
                let is_selected = features.selected_feature == Some(*id);
                let is_active = i < shown;

                // The trailing `Atom::grow` left-justifies the title; egui centres a stretched button's contents.
                let label_text = feature.type_name_with_id();
                // Past the bar: faded rather than struck through, which reads as deleted.
                let response = ui
                    .scope(|ui| {
                        if !is_active {
                            ui.multiply_opacity(0.4);
                        }
                        let label = egui::Button::selectable(
                            is_selected && is_active,
                            (label_text, egui::Atom::grow()),
                        );
                        ui.add_sized([ui.available_width(), 0.0], label)
                    })
                    .inner;
                rows.push((*id, response.rect));
                if response.clicked() && is_active {
                    next_selected = Some(if is_selected { None } else { Some(*id) }).flatten();
                    selection_changed = true;
                }

                let fid = *id;
                let menu = pending.clone();
                let at_the_bar = i + 1 == shown;
                response.context_menu(move |menu_ui| {
                    if !at_the_bar && menu_ui.button(get!("editor.timeline.roll_back_here")).clicked() {
                        *menu.borrow_mut() = Some(RowAction::RollBackTo(Some(fid)));
                        menu_ui.close();
                    }
                    if rolled_back && menu_ui.button(get!("editor.timeline.roll_to_end")).clicked() {
                        *menu.borrow_mut() = Some(RowAction::RollBackTo(None));
                        menu_ui.close();
                    }
                    menu_ui.separator();
                    if menu_ui.button(get!("editor.timeline.delete")).clicked() {
                        *menu.borrow_mut() = Some(RowAction::Delete(fid));
                        menu_ui.close();
                    }
                });

                // Faint at the bottom, where it changes nothing; bold elsewhere so a partial map is not mistaken
                // for the whole one.
                if at_the_bar {
                    let (rect, bar) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::drag());
                    let bar = bar.on_hover_cursor(egui::CursorIcon::ResizeVertical);
                    let lit = rolled_back || bar.hovered() || bar.dragged();
                    let stroke = match lit {
                        true => egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
                        false => egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
                    };
                    ui.painter().hline(rect.x_range(), rect.center().y, stroke);
                    bar_dragged = bar.dragged();
                }
            }

            // Live while dragging. Never above the first feature: an empty world says nothing.
            if bar_dragged
                && let Some(pointer) = ui.ctx().pointer_interact_pos()
            {
                let above = rows.iter().take_while(|(_, rect)| rect.center().y < pointer.y).count();
                if let Some((anchor, _)) = rows.get(above.max(1) - 1) {
                    *pending.borrow_mut() = Some(RowAction::RollBackTo(Some(*anchor)));
                }
            }

            let remaining = ui.available_size();
            if remaining.y > 0.0 {
                let response = ui.allocate_response(remaining, egui::Sense::click());
                if response.clicked() {
                    next_selected = None;
                    selection_changed = true;
                }
            }
        });

        if selection_changed {
            features.select(next_selected);
        }

        let row_action = pending.borrow_mut().take();
        let acted = row_action.is_some();
        match row_action {
            Some(RowAction::Delete(id)) => features.delete_feature(id),
            Some(RowAction::RollBackTo(at)) => features.roll_back_to(at),
            None => {}
        }

        was_edited || selection_changed || acted
    }

    fn undo_redo_shortcuts(
        keys: Res<ButtonInput<KeyCode>>,
        egui: Res<bevy_egui::input::EguiWantsInput>,
        mut features: ResMut<FeatureTimeline>,
    ) {
        let chords = chords(&keys, egui.wants_keyboard_input());
        if chords.redo {
            features.redo();
        } else if chords.undo {
            features.undo();
        }
    }

    // `pub(crate)` so tests elsewhere can drive it.
    pub(crate) fn sync_entities(mut features: ResMut<Self>, mut commands: Commands) {
        // Entity bookkeeping is not an edit. Marked as a change, the timeline would read as edited every
        // frame, to the undo checkpoint and `map_sync`'s pre-filter alike.
        let features = features.bypass_change_detection();
        for entity in features.pending_despawns.drain(..) {
            commands.entity(entity).despawn();
        }

        let shown = features.shown_len();
        let order: Vec<(usize, FeatureId)> = features.feature_order.iter()
            .enumerate()
            .map(|(i, id)| (i, *id))
            .collect();

        for (i, id) in order {
            let should_exist = i < shown;
            let needs_spawn = should_exist
                && features.features.get(&id).is_some_and(|a| a.object.entity().is_none());

            if needs_spawn {
                let mut feature = features.features.remove(&id).unwrap();
                feature.object.resolve_references(&features.features);
                let entity = commands.spawn(FeatureTag { feature_id: id._id }).id();
                feature.object.set_entity(Some(entity));
                feature.object.apply_to_entity(&mut commands, entity);
                features.features.insert(id, feature);
            } else if !should_exist {
                if let Some(mut feature) = features.features.remove(&id) {
                    if let Some(entity) = feature.object.entity() {
                        commands.entity(entity).despawn();
                        feature.object.set_entity(None);
                    }
                    features.features.insert(id, feature);
                }
            }
        }
    }

    fn handle_edits(
        mut features: ResMut<FeatureTimeline>,
        mut edit_events: MessageReader<EditEvent>,
        mut commands: Commands,
    ) {
        if edit_events.read().next().is_none() { return; }
        edit_events.clear();

        let affected = match &features.selection_affected {
            Some(ids) => ids.clone(),
            None => return,
        };

        for id in &affected {
            if let Some(mut feature) = features.features.remove(id) {
                feature.object.resolve_references(&features.features);
                if let Some(entity) = feature.object.entity() {
                    feature.object.apply_to_entity(&mut commands, entity);
                }
                features.features.insert(*id, feature);
            }
        }
    }

    fn draw_affected_gizmos(features: Res<FeatureTimeline>, mut gizmos: Gizmos) {
        let Some(affected) = &features.selection_affected else { return; };
        for id in affected {
            if let Some(feature) = features.features.get(id) {
                feature.object.debug_gizmos(&mut gizmos);
            }
        }
        if let Some(selected) = features.selected_feature.and_then(|id| features.features.get(&id)) {
            selected.object.selected_gizmos(&mut gizmos);
        }
    }
}

impl FeatureId {
    pub fn new() -> Self {
        FeatureId { _id: 0 }
    }

    pub fn from_raw(id: u64) -> Self {
        FeatureId { _id: id }
    }

    pub fn _id(&self) -> u64 {
        self._id
    }
}

impl std::fmt::Display for FeatureId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self._id)
    }
}

// A node in a DAG: a feature may depend on earlier features. Disjoint graphs are fine.
#[derive(Serialize, Deserialize)]
pub struct Feature {
    id: FeatureId,
    object: Box<dyn FeatureTrait>,
    parents: Vec<FeatureId>,
}

impl Feature {
    pub fn new(id: FeatureId, object: Box<dyn FeatureTrait>, parents: Vec<FeatureId>) -> Self {
        Self { id, object, parents }
    }

    pub fn id(&self) -> FeatureId {
        self.id
    }

    pub fn object(&self) -> &dyn FeatureTrait {
        &*self.object
    }

    pub fn object_mut(&mut self) -> &mut dyn FeatureTrait {
        &mut *self.object
    }

    pub fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        self.object.get_point(key)
    }
    
    pub fn type_name(&self) -> String {
        self.object.type_name()
    }
    
    pub fn type_name_with_id(&self) -> String {
        format!("{} {}", self.object.type_name(), self.id)
    }

    pub fn parents(&self) -> &[FeatureId] {
        &self.parents
    }

    pub fn set_parents(&mut self, parents: Vec<FeatureId>) {
        self.parents = parents;
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum AxisRef {
    Absolute(f32),
    Relative(f32),
}

impl AxisRef {
    pub fn resolve_with_base(&self, base: Option<f32>) -> Result<f32, PointResolutionError> {
        match self {
            AxisRef::Absolute(v) => Ok(*v),
            AxisRef::Relative(offset) => Ok(base.ok_or(PointResolutionError::NoSuchReferent)? + offset),
        }
    }

    pub fn value_mut(&mut self) -> &mut f32 {
        match self {
            AxisRef::Absolute(v) => v,
            AxisRef::Relative(offset) => offset,
        }
    }

    pub fn value(&self) -> f32 {
        match self {
            AxisRef::Absolute(v) => *v,
            AxisRef::Relative(offset) => *offset,
        }
    }

    pub fn is_relative(&self) -> bool {
        matches!(self, AxisRef::Relative(_))
    }
}

/// A point of a point ref's own feature, offered as its base by
/// [`PointRef::editor_ui_with_own`]. `active` is whether it is the base now.
pub struct OwnBase<'a> {
    pub label: &'a str,
    pub point: Vec3,
    pub active: &'a mut bool,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct PointRef {
    pub reference: Option<FeatureId>,
    pub point_key: String,
    pub x: AxisRef,
    pub y: AxisRef,
    pub z: AxisRef,
    #[serde(skip)]
    pub(crate) resolved_reference: Option<Vec3>,
}

impl PointRef {
    pub fn absolute(x: f32, y: f32, z: f32) -> Self {
        Self {
            reference: None,
            point_key: String::new(),
            x: AxisRef::Absolute(x),
            y: AxisRef::Absolute(y),
            z: AxisRef::Absolute(z),
            resolved_reference: None,
        }
    }

    pub fn reference(feature: FeatureId) -> Self {
        Self::reference_with_offset(feature, 0.0, 0.0, 0.0)
    }

    pub fn reference_with_offset(feature: FeatureId, dx: f32, dy: f32, dz: f32) -> Self {
        Self {
            reference: Some(feature),
            point_key: String::new(),
            x: AxisRef::Relative(dx),
            y: AxisRef::Relative(dy),
            z: AxisRef::Relative(dz),
            resolved_reference: None,
        }
    }

    pub fn resolve(&mut self, features: &HashMap<FeatureId, Feature>) -> Result<Vec3, PointResolutionError> {
        let base = self.reference
            .and_then(|id| features.get(&id))
            .map(|a| a.object.get_point(&self.point_key))
            .transpose()?;
        self.resolved_reference = base;
        Ok(Vec3::new(
            self.x.resolve_with_base(base.map(|b| b.x))?,
            self.y.resolve_with_base(base.map(|b| b.y))?,
            self.z.resolve_with_base(base.map(|b| b.z))?,
        ))
    }

    /// Writes one axis so it resolves to `world`, however that axis is stored. False for an axis that
    /// is not 0, 1 or 2.
    pub fn drag_axis(&mut self, axis: u8, world: f32) -> bool {
        let base = self.resolved_reference.map(|b| match axis {
            0 => b.x, 1 => b.y, _ => b.z,
        });
        let axis_ref = match axis {
            0 => &mut self.x,
            1 => &mut self.y,
            2 => &mut self.z,
            _ => return false,
        };
        match axis_ref {
            AxisRef::Absolute(v) => *v = world,
            AxisRef::Relative(offset) => *offset = world - base.unwrap_or(0.0),
        }
        true
    }

    pub fn referenced_features(&self) -> Vec<FeatureId> {
        self.reference.into_iter().collect()
    }

    pub fn editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        features: &HashMap<FeatureId, Feature>,
        prior_feature_order: &[FeatureId],
        retarget_request: &mut Option<String>,
    ) -> bool {
        self.editor_ui_with_own(ui, label, features, prior_feature_order, retarget_request, None)
    }

    /// [`Self::editor_ui`], also offering `own` — a point of the feature this
    /// belongs to — as a base, which no other feature can name.
    pub fn editor_ui_with_own(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        features: &HashMap<FeatureId, Feature>,
        prior_feature_order: &[FeatureId],
        retarget_request: &mut Option<String>,
        mut own: Option<OwnBase>,
    ) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).strong());
            // In egui's emoji-icon-font, which proportional text falls back to; `⊕` is only in the
            // monospace font and drew as a missing glyph.
            if ui.small_button("⌖").on_hover_text(get!("tools.retarget")).clicked() {
                *retarget_request = Some(label.to_string());
            }
        });

        let own_active = own.as_ref().is_some_and(|own| *own.active);
        let ref_label = match &own {
            Some(own) if own_active => own.label.to_string(),
            _ => self.reference
                .and_then(|id| features.get(&id))
                .map(|a| a.type_name_with_id())
                .unwrap_or_else(|| get!("editor.point_ref.none")),
        };

        enum Target { Absolute, Own, Feature(FeatureId) }
        let mut target = None;

        egui::ComboBox::from_id_salt(format!("{}_ref", label))
            .selected_text(&ref_label)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                let absolute = self.reference.is_none() && !own_active;
                if ui.selectable_label(absolute, get!("editor.point_ref.none")).clicked() && !absolute {
                    target = Some(Target::Absolute);
                }
                if let Some(own) = &own
                    && ui.selectable_label(own_active, own.label).clicked() && !own_active
                {
                    target = Some(Target::Own);
                }
                for &id in prior_feature_order {
                    if let Some(feature) = features.get(&id) {
                        let is_selected = self.reference == Some(id);
                        if ui.selectable_label(is_selected, feature.type_name_with_id()).clicked() && !is_selected {
                            target = Some(Target::Feature(id));
                        }
                    }
                }
            });

        if let Some(target) = target {
            let old_base = self.resolved_reference.unwrap_or(Vec3::ZERO);
            let new_base = match target {
                Target::Absolute => None,
                Target::Own => own.as_ref().map(|own| own.point),
                Target::Feature(id) => Some(features.get(&id)
                    .map(|a| a.object.get_point(&self.point_key).unwrap_or(Vec3::ZERO))
                    .unwrap_or(Vec3::ZERO)),
            };
            match new_base {
                None => {
                    for (axis, base_val) in [(&mut self.x, old_base.x), (&mut self.y, old_base.y), (&mut self.z, old_base.z)] {
                        if axis.is_relative() {
                            *axis = AxisRef::Absolute(base_val + axis.value());
                        }
                    }
                }
                Some(new_base) => {
                    for (axis, old_b, new_b) in [(&mut self.x, old_base.x, new_base.x), (&mut self.y, old_base.y, new_base.y), (&mut self.z, old_base.z, new_base.z)] {
                        if axis.is_relative() {
                            *axis = AxisRef::Relative(axis.value() + old_b - new_b);
                        }
                    }
                }
            }
            self.resolved_reference = new_base;
            self.reference = match target {
                Target::Feature(id) => Some(id),
                _ => None,
            };
            if matches!(target, Target::Own) {
                self.point_key.clear();
            }
            if let Some(own) = &mut own {
                *own.active = matches!(target, Target::Own);
            }
            changed = true;
        }

        // Only when a reference is set and it has several keys.
        if let Some(ref_id) = self.reference {
            if let Some(ref_feature) = features.get(&ref_id) {
                let keys = ref_feature.object.available_point_keys();
                if keys.len() > 1 {
                    let current_display = keys.iter()
                        .find(|(k, _)| k == &self.point_key)
                        .map(|(_, d)| d.as_str())
                        .unwrap_or("Default");

                    let mut new_key = self.point_key.clone();
                    let mut key_changed = false;

                    egui::ComboBox::from_id_salt(format!("{}_key", label))
                        .selected_text(current_display)
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for (key, display) in &keys {
                                if ui.selectable_label(&self.point_key == key, display).clicked() && &self.point_key != key {
                                    new_key = key.clone();
                                    key_changed = true;
                                }
                            }
                        });

                    if key_changed {
                        let old_base = self.resolved_reference.unwrap_or(Vec3::ZERO);
                        let new_base = ref_feature.object.get_point(&new_key).unwrap_or(Vec3::ZERO);
                        for (axis, old_b, new_b) in [(&mut self.x, old_base.x, new_base.x), (&mut self.y, old_base.y, new_base.y), (&mut self.z, old_base.z, new_base.z)] {
                            if axis.is_relative() {
                                *axis = AxisRef::Relative(axis.value() + old_b - new_b);
                            }
                        }
                        self.point_key = new_key;
                        self.resolved_reference = Some(new_base);
                        changed = true;
                    }
                }
            }
        }

        let has_ref = self.reference.is_some() || own.as_ref().is_some_and(|own| *own.active);
        let base = self.resolved_reference.unwrap_or(Vec3::ZERO);
        changed |= axis_row(ui, &mut self.x, "X", has_ref, base.x);
        changed |= axis_row(ui, &mut self.y, "Y", has_ref, base.y);
        changed |= axis_row(ui, &mut self.z, "Z", has_ref, base.z);

        changed
    }

    /// A taxicab path from the reference to the resolved point: X, then Z, then Y.
    pub fn debug_gizmos(&self, _resolved: Vec3, gizmos: &mut Gizmos) {
        let Some(base) = self.resolved_reference else { return; };
        const DASH: f32 = 0.15;
        const GAP: f32 = 0.1;

        let segments: [(&AxisRef, Vec3, Color); 3] = [
            (&self.x, Vec3::X, Color::srgb_u8(255, 80, 80)),
            (&self.z, Vec3::Z, Color::srgb_u8(80, 80, 255)),
            (&self.y, Vec3::Y, Color::srgb_u8(80, 255, 80)),
        ];

        let mut cursor = base;
        for (axis_ref, unit, color) in segments {
            if let AxisRef::Relative(offset) = axis_ref {
                if offset.abs() < f32::EPSILON { continue; }
                let next = cursor + unit * *offset;
                dashed_line(gizmos, cursor, next, color, DASH, GAP);
                cursor = next;
            }
        }
    }
}

const FEATURE_LIST_MIN_HEIGHT: f32 = 96.0;
const FEATURE_EDITOR_MIN_HEIGHT: f32 = 120.0;

/// Half, but never below the list's own minimum.
fn feature_editor_default_height(available: f32) -> f32 {
    let range = feature_editor_height_range(available);
    (available * 0.5).clamp(range.min, range.max)
}

/// The maximum is never below the minimum, so the range does not invert in a short panel.
fn feature_editor_height_range(available: f32) -> egui::Rangef {
    let min = FEATURE_EDITOR_MIN_HEIGHT.min(available);
    let max = (available - FEATURE_LIST_MIN_HEIGHT).max(min);
    egui::Rangef::new(min, max)
}

pub const FEATURE_EDITOR_PADDING: f32 = 8.0;

/// A margin and taller rows for a feature's own editor. The scroll area is inside the padding, or a
/// room's eight rows run off the bottom.
fn feature_editor_frame<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin::same(FEATURE_EDITOR_PADDING as i8))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(
                FEATURE_EDITOR_PADDING,
                FEATURE_EDITOR_PADDING * 0.75,
            );
            ui.spacing_mut().interact_size.y = 22.0;
            ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(80.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, add_contents)
                .inner
        })
        .inner
}

fn axis_row(ui: &mut egui::Ui, axis_ref: &mut AxisRef, label: &str, has_ref: bool, base_val: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut is_rel = axis_ref.is_relative();
        if ui.add_enabled(has_ref, egui::Checkbox::new(&mut is_rel, "")).changed() {
            if is_rel {
                let abs_val = axis_ref.value();
                *axis_ref = AxisRef::Relative(abs_val - base_val);
            } else {
                let offset = axis_ref.value();
                *axis_ref = AxisRef::Absolute(base_val + offset);
            }
            changed = true;
        }
        changed |= ui.add(egui::Slider::new(axis_ref.value_mut(), -100.0..=100.0)
            .text(label)
            .clamping(egui::SliderClamping::Never)
            .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 1.0 })
        ).changed();
    });
    changed
}

fn dashed_line(gizmos: &mut Gizmos, start: Vec3, end: Vec3, color: Color, dash: f32, gap: f32) {
    let dir = end - start;
    let len = dir.length();
    if len < 0.001 { return; }
    let norm = dir / len;
    let mut t = 0.0;
    while t < len {
        let dash_end = (t + dash).min(len);
        gizmos.line(start + norm * t, start + norm * dash_end, color);
        t = dash_end + gap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::global_point::GlobalPoint;

    fn x_of(timeline: &FeatureTimeline, id: FeatureId) -> Option<f32> {
        timeline.get_feature(&id).map(|f| f.object().get_point("").unwrap().x)
    }

    fn move_to_x(timeline: &mut FeatureTimeline, id: FeatureId, x: f32) {
        let feature = timeline.features_mut().get_mut(&id).unwrap();
        feature.object_mut().get_point_ref_mut("location").unwrap().x = AxisRef::Absolute(x);
        feature.object_mut().resolve_references(&HashMap::new());
    }

    /// An un-checkpointed edit must be taken back by undo first rather than skipped.
    #[test]
    fn undo_and_redo_walk_whole_map_checkpoints() {
        let mut timeline = FeatureTimeline::default();
        let a = timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        timeline.checkpoint();
        move_to_x(&mut timeline, a, 2.0);
        move_to_x(&mut timeline, a, 3.0);
        timeline.checkpoint();
        move_to_x(&mut timeline, a, 4.0);

        timeline.undo();
        assert_eq!(x_of(&timeline, a), Some(3.0), "the unrecorded edit is what undo takes back");
        timeline.undo();
        assert_eq!(x_of(&timeline, a), Some(1.0), "a whole gesture is one step");
        timeline.redo();
        timeline.redo();
        assert_eq!(x_of(&timeline, a), Some(4.0));
    }

    /// Breaks if an idle Features panel reports a change: the panel marks the
    /// timeline only on that answer, or the checkpoint serialises every frame.
    #[test]
    fn the_features_panel_left_alone_changes_nothing() {
        let mut timeline = FeatureTimeline::default();
        let a = timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        timeline.apply_feature(Box::new(GlobalPoint::new(2.0, 0.0, 0.0)));
        timeline.select(Some(a));

        let ctx = egui::Context::default();
        let mut events = Vec::new();
        let mut retarget = None;
        // Several frames: egui lays out on the first and settles on the second.
        for frame in 0..3 {
            let mut touched = false;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                touched = FeatureTimeline::ui(ui, &mut timeline, &mut events, &mut retarget);
            });
            // Nothing renders here, so the font atlas is discarded on purpose.
            output.textures_delta.clear();
            assert!(!touched, "frame {frame} of an idle Features panel reported a change");
        }
        assert!(events.is_empty());
        assert_eq!(timeline.selected_feature, Some(a));
    }

    #[test]
    fn deleting_is_undone_by_the_same_mechanism() {
        let mut timeline = FeatureTimeline::default();
        let a = timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        timeline.checkpoint();
        timeline.delete_feature(a);
        timeline.undo();
        assert_eq!(x_of(&timeline, a), Some(1.0));
        assert_eq!(timeline.feature_order(), &[a]);
    }

    /// An untouched feature keeps its entity; a changed one is respawned.
    #[test]
    fn undo_rebuilds_only_the_features_that_changed() {
        let mut timeline = FeatureTimeline::default();
        let still = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
        let moved = timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        for id in [still, moved] {
            timeline.features_mut().get_mut(&id).unwrap().object_mut().set_entity(Some(Entity::PLACEHOLDER));
        }
        timeline.checkpoint();
        move_to_x(&mut timeline, moved, 5.0);
        timeline.undo();

        let entity = |id| timeline.get_feature(&id).unwrap().object().entity();
        assert_eq!(entity(still), Some(Entity::PLACEHOLDER), "an unchanged feature was rebuilt");
        assert_eq!(entity(moved), None, "a changed feature kept its stale entity");
        assert_eq!(timeline.pending_despawns.len(), 1);
    }

    #[test]
    fn reverting_is_one_undoable_step() {
        let mut timeline = FeatureTimeline::default();
        let a = timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        timeline.mark_saved();
        let on_disk = crate::editor::save::parse(
            &crate::editor::save::to_text(&timeline, &Default::default()).unwrap(),
        )
        .unwrap()
        .timeline;

        move_to_x(&mut timeline, a, 9.0);
        timeline.checkpoint();
        assert!(timeline.is_dirty());

        timeline.replace_as_edit(on_disk);
        assert_eq!(x_of(&timeline, a), Some(1.0));
        assert!(!timeline.is_dirty());

        timeline.undo();
        assert_eq!(x_of(&timeline, a), Some(9.0));
        assert!(timeline.is_dirty());
    }

    #[test]
    fn adding_while_rolled_back_builds_at_the_bar() {
        let mut timeline = FeatureTimeline::default();
        let a = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
        let b = timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        timeline.roll_back_to(Some(a));
        assert_eq!(timeline.shown_len(), 1);
        assert_eq!(timeline.active_features().count(), 1);

        let c = timeline.apply_feature(Box::new(GlobalPoint::new(2.0, 0.0, 0.0)));
        assert_eq!(timeline.feature_order(), &[a, c, b]);
        assert_eq!(timeline.rollback(), Some(c));
        assert_eq!(timeline.shown_len(), 2);
    }

    #[test]
    fn the_bar_never_points_at_nothing() {
        let mut timeline = FeatureTimeline::default();
        let a = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
        let b = timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        timeline.roll_back_to(Some(b));
        assert_eq!(timeline.rollback(), None);

        timeline.roll_back_to(Some(a));
        timeline.delete_feature(a);
        assert_eq!(timeline.rollback(), None);
        assert_eq!(timeline.shown_len(), 1);
    }

    /// Moving the bar is no undo step, and a save writes every feature regardless.
    #[test]
    fn rolling_back_is_not_an_edit() {
        let mut timeline = FeatureTimeline::default();
        let a = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
        timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        timeline.mark_saved();
        timeline.roll_back_to(Some(a));
        timeline.checkpoint();
        assert!(!timeline.can_undo() && !timeline.is_dirty());

        let text = crate::editor::save::to_text(&timeline, &Default::default()).unwrap();
        let loaded = crate::editor::save::parse(&text).unwrap().timeline;
        assert_eq!(loaded.feature_order().len(), 2);
        assert_eq!(loaded.rollback(), None);
    }

    /// Rolling to the end must stand up the rest, which entering Play relies on.
    #[test]
    fn the_world_holds_what_is_above_the_bar() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let mut timeline = FeatureTimeline::default();
        let a = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
        let b = timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        timeline.roll_back_to(Some(a));
        app.insert_resource(timeline);
        app.add_systems(Update, FeatureTimeline::sync_entities);
        app.update();

        let has_entity = |app: &App, id| {
            app.world().resource::<FeatureTimeline>().get_feature(&id).unwrap().object().entity().is_some()
        };
        assert!(has_entity(&app, a));
        assert!(!has_entity(&app, b), "a feature past the bar stood in the world");

        app.world_mut().resource_mut::<FeatureTimeline>().roll_back_to(None);
        app.update();
        assert!(has_entity(&app, b));
    }

    #[test]
    fn the_history_is_bounded() {
        let mut timeline = FeatureTimeline::default();
        let a = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
        timeline.mark_saved();
        let on_disk = || {
            let mut saved = FeatureTimeline::default();
            saved.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
            saved
        };
        for step in 0..UNDO_DEPTH * 2 {
            move_to_x(&mut timeline, a, step as f32 + 1.0);
            timeline.checkpoint();
            timeline.replace_as_edit(on_disk());
        }
        assert_eq!(timeline.undo.len(), UNDO_DEPTH);
    }

    #[test]
    fn adopting_a_map_forgets_the_history() {
        let mut timeline = FeatureTimeline::default();
        let a = timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        timeline.checkpoint();
        move_to_x(&mut timeline, a, 2.0);
        timeline.checkpoint();
        assert!(timeline.can_undo());

        timeline.adopt(FeatureTimeline::default());
        assert!(!timeline.can_undo() && !timeline.can_redo());
        assert!(!timeline.is_dirty());
    }

    /// `f32::clamp` panics on inverted bounds, so a short panel would crash the editor.
    #[test]
    fn the_feature_editor_range_never_inverts() {
        for available in [0.0, 1.0, 50.0, 120.0, 200.0, 216.0, 400.0, 2000.0] {
            let range = feature_editor_height_range(available);
            assert!(
                range.min <= range.max,
                "inverted range at {available}: {range:?}"
            );
            let default = feature_editor_default_height(available);
            assert!(
                (range.min..=range.max).contains(&default),
                "default {default} outside {range:?} at {available}"
            );
        }
    }
}
