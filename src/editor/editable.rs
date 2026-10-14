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
use crate::editor::map_metadata::MapMetadata;
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
            .add_message::<EditEvent>()
            .add_systems(Startup, load_startup_blueprint)
            // `sync_entities` and `handle_edits` are deliberately not
            // gated on `AppMode::Editor`: features have to keep reaching their
            // entities while playing, which is what lets an edit land
            // mid-match without a reload. Only the two that read editor input
            // or draw editor gizmos are switched off.
            .add_systems(Update, (
                FeatureTimeline::undo_redo_shortcuts.run_if(in_state(AppMode::Editor)),
                FeatureTimeline::sync_entities,
                FeatureTimeline::handle_edits,
                FeatureTimeline::draw_affected_gizmos.run_if(in_state(AppMode::Editor)),
                checkpoint_when_idle.run_if(in_state(AppMode::Editor)),
            ).chain())
        ;
    }
}

/// Record an undo step whenever the editor goes idle with the map changed.
///
/// Idle is **no mouse button held and no text field holding the keyboard**:
/// a drag in a viewport or on a number is one step because the button is down
/// for all of it, and typing a name is one step because the field keeps the
/// keyboard until it is let go. See [`FeatureTimeline::checkpoint`].
///
/// Through `bypass_change_detection`, or this system's own write would read
/// as a change next frame and it would checkpoint forever.
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
    fn entity(&self) -> Option<Entity>;
    fn set_entity(&mut self, entity: Option<Entity>);
    fn apply_to_entity(&self, commands: &mut Commands, entity: Entity);
    /// Resolve all PointRef fields against the current state of the features map.
    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>);
    /// Return the FeatureIds this object's PointRefs depend on.
    fn parent_ids(&self) -> Vec<FeatureId>;
    /// Return the named points this object exposes for referencing.
    fn available_point_keys(&self) -> Vec<(String, String)>;
    /// Return reference points relevant to the given ray. Point-like objects
    /// always return their location; volumetric objects test ray-AABB intersection first.
    fn reference_points_for_ray(&self, ray: &Ray3d) -> Vec<(String, Vec3)>;

    /// Adjust a single axis of a bound point (used by drag handles).
    /// `is_max`: true for the max point, false for the min point.
    /// `axis`: 0=X, 1=Y, 2=Z.
    /// `new_world_value`: the desired world-space coordinate for this axis.
    /// Returns true if the object was modified.
    fn drag_handle(&mut self, _is_max: bool, _axis: u8, _new_world_value: f32) -> bool { false }

    /// Returns the resolved min and max bounds if this object is a room-like
    /// object with drag handles. Used to position handles.
    fn drag_handle_bounds(&self) -> Option<(Vec3, Vec3)> { None }

    /// Get a reference to a named PointRef on this object.
    fn get_point_ref(&self, _key: &str) -> Option<&PointRef> { None }

    /// Get a mutable reference to a named PointRef on this object.
    /// Keys: GlobalPoint/GracklePointLight use "location" (or ""),
    /// EditorRoom uses "min" / "max".
    fn get_point_ref_mut(&mut self, _key: &str) -> Option<&mut PointRef> { None }

    /// Which Euler components (0=X pitch, 1=Y yaw, 2=Z roll) this feature lets
    /// the user turn. Empty — the default — means the feature has no facing,
    /// and the rotation rings are not drawn for it at all.
    ///
    /// A spawn point returns just `1`: a body stands upright, so pitching or
    /// rolling one would only ever describe a spawn that cannot be used.
    fn rotation_axes(&self) -> Vec<u8> { vec![] }

    /// The three angles in radians, in axis order — see
    /// [`crate::common::rotation`] for what they mean together.
    fn euler_angles(&self) -> Vec3 { Vec3::ZERO }

    /// Set one Euler component, in radians. Returns true if it was modified,
    /// so a drag on an axis the feature does not expose changes nothing.
    fn set_euler_angle(&mut self, _axis: u8, _radians: f32) -> bool { false }
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
    /// Topologically sorted list of the selected feature and all its DAG descendants.
    /// Parents always appear before their dependants.
    selection_affected: Option<Vec<FeatureId>>,
    rollback_bar: u64,
    pending_despawns: Vec<Entity>,
    /// Whole-map snapshots to go back to, oldest first. **Undo is not a list
    /// of what each edit did** — it is what the map *was*, the prop editor's
    /// model: no before/after pair to get the wrong way round, no edit that
    /// can forget to record itself. Lives in memory only; a blueprint does not
    /// carry it.
    undo: Vec<MapState>,
    redo: Vec<MapState>,
    /// The map as of the last checkpoint — what an undo goes back *from*, and
    /// what the next checkpoint compares against. `None` until the first.
    checkpoint: Option<MapState>,
    /// The map as last saved or loaded, so the editor can say whether there
    /// is anything to save and anything to revert.
    saved: Option<MapState>,
    /// Whether the last checkpoint differs from what was saved. Kept rather
    /// than computed, because computing it means serializing the map.
    dirty: bool,
}

/// One feature as undo stores it. JSON because it is the fastest of the
/// encodings `typetag` gives and these strings never reach a file — the
/// blueprint is RON, written by `save`.
fn serialize_feature(feature: &Feature) -> String {
    serde_json::to_string(feature).unwrap_or_default()
}

/// How many steps back undo reaches.
const UNDO_DEPTH: usize = 200;

/// The map as undo sees it: every feature's serialized form, in order.
///
/// Serialized rather than cloned because a feature is a trait object, and
/// because comparing two of these is then a string comparison — which is what
/// lets a restore rebuild **only the features that differ** rather than every
/// room on the map. Stable because everything a feature caches about the
/// world (a `PointRef`'s resolved position, the entity) is `#[serde(skip)]`.
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
            rollback_bar: 0,
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
    /// A map from its parts, with nothing rolled back and no history: what
    /// loading a file and adopting a server's map both mean.
    pub fn from_parts(
        features: HashMap<FeatureId, Feature>,
        feature_order: Vec<FeatureId>,
        id_counter: u64,
    ) -> Self {
        let rollback_bar = feature_order.len() as u64;
        let mut timeline = Self {
            features,
            feature_order,
            id_counter,
            rollback_bar,
            ..Default::default()
        };
        // Whatever it came from — a file, a server — is what it matches.
        timeline.reset_history();
        timeline
    }

    /// Replace the open map with another one, taking the old map's entities
    /// down with it.
    ///
    /// The despawns are *queued* rather than done here, so they run in
    /// `sync_entities` alongside the spawns of the incoming features — one
    /// place that owns the world's shape, rather than two that have to agree
    /// about which frame they run on. Leaving them out is what would strand
    /// the old map's rooms in the world with nothing left pointing at them.
    ///
    /// The caller is responsible for re-baking room geometry afterwards: the
    /// entities do not exist until `sync_entities` has run, so a bake fired in
    /// the same frame would bake nothing.
    pub fn adopt(&mut self, incoming: FeatureTimeline) {
        let old: Vec<Entity> = self
            .active_features()
            .filter_map(|(_, feature)| feature.object().entity())
            .collect();
        *self = incoming;
        for entity in old {
            self.queue_despawn(entity);
        }
        // A selection is an index into the map that just went away.
        self.select(None);
        // And so is the history: undoing back through opening a file, or
        // through a server handing over its map, is editing a different map.
        self.reset_history();
    }

    /// Remove a feature. Undoable like any other edit: the next checkpoint
    /// records it.
    pub fn delete_feature(&mut self, feature_id: FeatureId) {
        self.remove_feature_internal(feature_id);
    }

    pub fn id_counter(&self) -> u64 {
        self.id_counter
    }

    pub fn rollback_bar(&self) -> u64 {
        self.rollback_bar
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

    /// Returns an iterator of (FeatureId, &Feature) for all active features
    /// (those before the rollback bar).
    pub fn active_features(&self) -> impl Iterator<Item = (FeatureId, &Feature)> {
        let rollback_bar = self.rollback_bar as usize;
        self.feature_order[..rollback_bar].iter().filter_map(move |id| {
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

    pub fn apply_feature(&mut self, feature_object: Box<dyn FeatureTrait>) -> FeatureId {
        let rollback_at_end = self.rollback_bar == self.feature_order.len() as u64;
        let cur = self.rollback_bar as usize;
        if cur < self.feature_order.len() {
            for id in self.feature_order.drain(cur..) {
                if let Some(feature) = self.features.remove(&id) {
                    if let Some(entity) = feature.object.entity() {
                        self.pending_despawns.push(entity);
                    }
                }
            }
        }

        let parents = feature_object.parent_ids();
        let new_id = self.next_id();
        let new_feature = Feature {
            id: new_id,
            object: feature_object,
            parents,
        };
        self.features.insert(new_feature.id, new_feature);
        self.feature_order.push(new_id);
        if rollback_at_end {
            self.rollback_bar = self.feature_order.len() as u64;
        } else {
            self.clamp_rollback_bar();
        }

        new_id
    }
    
    pub fn get_feature(&self, id: &FeatureId) -> Option<&Feature> {
        self.features.get(id)
    }

    pub fn can_step_back(&self) -> bool {
        self.rollback_bar > 0
    }

    pub fn can_step_forward(&self) -> bool {
        self.rollback_bar < self.feature_order.len() as u64
    }

    /// Move the rollback bar back one feature.
    pub fn step_back(&mut self) {
        if !self.can_step_back() { return; }
        self.rollback_bar -= 1;
        if let Some(selected) = self.selected_feature {
            if let Some(idx) = self.feature_order.iter().position(|id| *id == selected) {
                if idx as u64 >= self.rollback_bar {
                    self.select(None);
                }
            }
        }
    }

    /// Move the rollback bar forward one feature.
    pub fn step_forward(&mut self) {
        if !self.can_step_forward() { return; }
        self.rollback_bar += 1;
    }

    /// The map as undo sees it right now.
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

    /// Record the map as one undo step if it has changed since the last
    /// checkpoint.
    ///
    /// **Called when the editor is idle, not by each edit.** A drag is one
    /// step because the button is held for all of it; typing a name is one
    /// step because the field has the keyboard until it is let go. Nothing
    /// that edits the map has to remember to record itself, which is the
    /// failure the per-edit model had: a tool that forgot was an edit undo
    /// walked straight past.
    pub fn checkpoint(&mut self) {
        let now = self.state();
        match self.checkpoint.take() {
            Some(before) if before != now => {
                self.undo.push(before);
                if self.undo.len() > UNDO_DEPTH {
                    self.undo.remove(0);
                }
                // A fresh edit ends the redo branch.
                self.redo.clear();
            }
            _ => {}
        }
        self.dirty = self.saved.as_ref() != Some(&now);
        self.checkpoint = Some(now);
    }

    /// Forget every step: the map in front of us is a different map, opened
    /// from a file or handed over by a server. Undoing back through that is
    /// not what anybody means — and on a client, undo would be editing a map
    /// it does not own.
    pub fn reset_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        let now = self.state();
        self.checkpoint = Some(now.clone());
        self.saved = Some(now);
        self.dirty = false;
    }

    /// The map now matches its file.
    pub fn mark_saved(&mut self) {
        let now = self.state();
        self.checkpoint = Some(now.clone());
        self.saved = Some(now);
        self.dirty = false;
    }

    /// Whether the map differs from what was last saved or loaded, as of the
    /// last checkpoint.
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
        // Anything done since the last checkpoint is an edit too, and the one
        // this undo should take back.
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

    /// Replace the map with `incoming` **as one undo step** — what reverting
    /// to the file is. Then it matches the file, so it is not dirty; undoing
    /// the revert makes it dirty again through the ordinary path.
    pub fn replace_as_edit(&mut self, incoming: FeatureTimeline) {
        self.checkpoint();
        let target = incoming.state();
        if let Some(current) = self.checkpoint.take()
            && current != target
        {
            self.undo.push(current);
            self.redo.clear();
        }
        self.restore(&target);
        self.mark_saved();
    }

    /// Make the map `target`, rebuilding **only the features that differ**.
    ///
    /// A feature whose serialized form is unchanged keeps its object and its
    /// entity; one that changed, appeared or vanished is rebuilt, and so is
    /// everything built on it, since a `PointRef` that moved moves its
    /// dependants. An undo of one wall therefore respawns one wall rather
    /// than every room on the map.
    fn restore(&mut self, target: &MapState) {
        let was_at_end = self.rollback_bar >= self.feature_order.len() as u64;
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
                // Cannot happen for a string this process wrote, and losing
                // one feature is better than losing the undo.
                Err(error) => error!("Undo could not rebuild feature {id:?}: {error}"),
            }
        }

        // Whatever is left was not in the target: it goes.
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
        // Resolved now, in order, rather than left for `sync_entities` to do as
        // it respawns: a rebuilt feature's resolved position is not part of
        // what was stored, and anything that reads it before the next spawn —
        // a gizmo, the panel — would otherwise see the origin for a frame.
        for id in self.feature_order.clone() {
            if let Some(mut feature) = self.features.remove(&id) {
                feature.object.resolve_references(&self.features);
                self.features.insert(id, feature);
            }
        }

        if was_at_end {
            self.rollback_bar = self.feature_order.len() as u64;
        }
        self.clamp_rollback_bar();
        let selected = self.selected_feature.filter(|id| self.features.contains_key(id));
        self.select(selected);

        self.dirty = self.saved.as_ref() != Some(target);
        self.checkpoint = Some(target.clone());
    }

    /// Keeps the construction rollback bar valid after feature-order changes from actions.
    /// Action undo/redo does not otherwise move the bar except to stay in `[0, len]`.
    fn clamp_rollback_bar(&mut self) {
        let len = self.feature_order.len() as u64;
        if self.rollback_bar > len {
            self.rollback_bar = len;
        }
        if let Some(selected) = self.selected_feature {
            if let Some(idx) = self.feature_order.iter().position(|id| *id == selected) {
                if idx as u64 >= self.rollback_bar {
                    self.select(None);
                }
            }
        }
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

        self.clamp_rollback_bar();
    }
    
    pub fn ui(
        ui: &mut egui::Ui,
        features: &mut Self,
        edit_events: &mut Vec<EditEvent>,
        retarget_out: &mut Option<(FeatureId, String)>,
    ) {
        // Section 1: Undo/Redo (compact, top)
        ui.horizontal(|ui| {
            if ui.add_enabled(features.can_step_back(), egui::Button::new("⮪ ".to_owned() + &get!("editor.timeline.undo"))).clicked() {
                features.step_back();
            }
            if ui.add_enabled(features.can_step_forward(), egui::Button::new(get!("editor.timeline.redo") + " ⮫")).clicked() {
                features.step_forward();
            }
        });

        ui.separator();

        // Section 3: Editor for selected feature (bottom, takes as much as needed)
        // We render this into a bottom panel so it pins to the bottom,
        // then the scroll area fills whatever's left.
        let mut was_edited = false;
        let mut edited_id = None;
        let mut entity_for_event: Option<Entity> = None;

        if let Some(selected_id) = features.selected_feature {
            let selected_idx = features.feature_order.iter()
                .position(|id| *id == selected_id);
            let is_active = selected_idx.is_some_and(|idx| (idx as u64) < features.rollback_bar);

            if is_active {
                let selected_idx = selected_idx.unwrap();
                let prior_order: Vec<FeatureId> = features.feature_order[..selected_idx].to_vec();

                // Resizable, and opening at half the tab: a feature editor is
                // where the actual work happens, and the construction list
                // above it is a list of names you glance at. A fixed panel
                // sized to whatever fits left a room's eight axis rows to be
                // scrolled through four at a time.
                //
                // The bounds are stated against the space the tab actually
                // has rather than as two constants, so neither half can be
                // dragged — or opened — out of existence in a short panel.
                let available = ui.available_height();
                egui::Panel::bottom("editor_feature_panel")
                    .resizable(true)
                    .default_size(feature_editor_default_height(available))
                    .size_range(feature_editor_height_range(available))
                    .show(ui, |ui| {
                        ui.separator();
                        if let Some(mut feature) = features.features.remove(&selected_id) {
                            let mut retarget_request: Option<String> = None;
                            // The whole feature editor is padded and loosened
                            // here rather than in each `editor_ui`: a feature
                            // type describes its own fields and should not
                            // also have to know what the panel around them
                            // looks like, and nine implementations each
                            // choosing their own margins is nine chances for
                            // one of them to look like a different program.
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

        // Section 2: Feature order (construction timeline; fills remaining space)
        let mut selection_changed = false;
        let mut next_selected = features.selected_feature;
        let pending_delete = Rc::new(RefCell::new(None::<FeatureId>));

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, id) in features.feature_order.iter().enumerate() {
                let feature = features.get_feature(id).unwrap();
                let is_selected = features.selected_feature == Some(*id);
                let is_active = (i as u64) < features.rollback_bar;

                // The trailing `Atom::grow` is what left-justifies the title.
                // These buttons are stretched to the panel width and egui
                // centres a button's contents, so without it every title
                // floats in the middle of the row and the list has no edge to
                // read down.
                let label_text = feature.type_name_with_id();
                let label = if is_active {
                    egui::Button::selectable(is_selected, (label_text, egui::Atom::grow()))
                } else {
                    egui::Button::selectable(false, (
                        egui::RichText::new(label_text).strikethrough().weak(),
                        egui::Atom::grow(),
                    ))
                };

                let response = ui.add_sized([ui.available_width(), 0.0], label);
                if response.clicked() && is_active {
                    next_selected = Some(if is_selected { None } else { Some(*id) }).flatten();
                    selection_changed = true;
                }

                let fid = *id;
                let pending = pending_delete.clone();
                response.context_menu(move |menu_ui| {
                    if menu_ui.button(get!("editor.timeline.delete")).clicked() {
                        *pending.borrow_mut() = Some(fid);
                        menu_ui.close();
                    }
                });
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

        if let Some(id) = pending_delete.borrow_mut().take() {
            features.delete_feature(id);
        }
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

    // `pub(crate)` so a test elsewhere in the crate can drive it directly:
    // what a map adoption does to the world only shows up once this has run.
    pub(crate) fn sync_entities(mut features: ResMut<Self>, mut commands: Commands) {
        // Which entity stands for which feature is bookkeeping about this
        // world, not an edit to the map, so it does not mark the timeline
        // changed. Marked, it would read as changed every frame — to the undo
        // checkpoint and to `map_sync`'s pre-filter alike.
        let features = features.bypass_change_detection();
        for entity in features.pending_despawns.drain(..) {
            commands.entity(entity).despawn();
        }

        let rollback_bar = features.rollback_bar;
        let order: Vec<(usize, FeatureId)> = features.feature_order.iter()
            .enumerate()
            .map(|(i, id)| (i, *id))
            .collect();

        for (i, id) in order {
            let should_exist = (i as u64) < rollback_bar;
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

// Essentially, these are nodes on a directed acyclic graph.
// Editor features may depend on previous features to resolve.
// They may also not depend on anything, with all descendants tracing their ancestry back --
// such a case would lead to multiple disjoint graphs, which is okay.
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
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).strong());
            if ui.small_button("⊕").on_hover_text("Retarget").clicked() {
                *retarget_request = Some(label.to_string());
            }
        });

        // Reference feature dropdown
        let ref_label = self.reference
            .and_then(|id| features.get(&id))
            .map(|a| a.type_name_with_id())
            .unwrap_or_else(|| "None".to_string());

        let mut new_reference = self.reference;
        let mut ref_changed = false;

        egui::ComboBox::from_id_salt(format!("{}_ref", label))
            .selected_text(&ref_label)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                if ui.selectable_label(self.reference.is_none(), "None").clicked() && self.reference.is_some() {
                    new_reference = None;
                    ref_changed = true;
                }
                for &id in prior_feature_order {
                    if let Some(feature) = features.get(&id) {
                        let is_selected = self.reference == Some(id);
                        if ui.selectable_label(is_selected, feature.type_name_with_id()).clicked() && !is_selected {
                            new_reference = Some(id);
                            ref_changed = true;
                        }
                    }
                }
            });

        if ref_changed {
            let old_base = self.resolved_reference.unwrap_or(Vec3::ZERO);

            if new_reference.is_none() {
                for (axis, base_val) in [(&mut self.x, old_base.x), (&mut self.y, old_base.y), (&mut self.z, old_base.z)] {
                    if axis.is_relative() {
                        *axis = AxisRef::Absolute(base_val + axis.value());
                    }
                }
                self.resolved_reference = None;
            } else {
                let new_base = new_reference
                    .and_then(|id| features.get(&id))
                    .map(|a| a.object.get_point(&self.point_key).unwrap_or(Vec3::ZERO))
                    .unwrap_or(Vec3::ZERO);
                for (axis, old_b, new_b) in [(&mut self.x, old_base.x, new_base.x), (&mut self.y, old_base.y, new_base.y), (&mut self.z, old_base.z, new_base.z)] {
                    if axis.is_relative() {
                        *axis = AxisRef::Relative(axis.value() + old_b - new_b);
                    }
                }
                self.resolved_reference = Some(new_base);
            }
            self.reference = new_reference;
            changed = true;
        }

        // Point key dropdown (only when a reference is set and it has multiple keys)
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

        // Per-axis rows: checkbox (relative toggle) + slider
        let has_ref = self.reference.is_some();
        let base = self.resolved_reference.unwrap_or(Vec3::ZERO);
        changed |= axis_row(ui, &mut self.x, "X", has_ref, base.x);
        changed |= axis_row(ui, &mut self.y, "Y", has_ref, base.y);
        changed |= axis_row(ui, &mut self.z, "Z", has_ref, base.z);

        changed
    }

    /// Draw a taxicab path from the reference point to the resolved point,
    /// stepping along X then Z then Y, with per-axis colored dashed lines.
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

/// Least room the construction list keeps, and the least the feature editor
/// opens with, whatever the panel is dragged to.
const FEATURE_LIST_MIN_HEIGHT: f32 = 96.0;
const FEATURE_EDITOR_MIN_HEIGHT: f32 = 120.0;

/// How tall the feature editor opens in a tab with `available` height.
///
/// Half, so the two halves of the tab read as equals — but never at the cost
/// of the list's own minimum, which is what keeps a short panel from opening
/// with nothing to select in it.
fn feature_editor_default_height(available: f32) -> f32 {
    let range = feature_editor_height_range(available);
    (available * 0.5).clamp(range.min, range.max)
}

/// What the feature editor may be dragged to in a tab with `available` height.
///
/// Both ends give way in a panel too short to honour them: the maximum is
/// never below the minimum, so the range stays valid rather than inverting.
fn feature_editor_height_range(available: f32) -> egui::Rangef {
    let min = FEATURE_EDITOR_MIN_HEIGHT.min(available);
    let max = (available - FEATURE_LIST_MIN_HEIGHT).max(min);
    egui::Rangef::new(min, max)
}

/// Gap left around and between the fields of the feature editor.
pub const FEATURE_EDITOR_PADDING: f32 = 8.0;

/// Draw a feature's own editor with room to breathe.
///
/// Everything a feature puts in the panel goes through here: a margin so no
/// control is flush against the panel edge, and a taller row so stacked
/// sliders and dropdowns are not one continuous band of widget. The scroll
/// area is part of the padding rather than around it — a feature with more
/// fields than the panel is tall (a room has two `PointRef`s, which is eight
/// rows) otherwise loses the last of them off the bottom with no way to reach
/// them.
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

    /// Edits between checkpoints are one step; undo goes back over it and
    /// redo comes forward again — including an edit not yet checkpointed,
    /// which an undo has to take back first rather than skip.
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

    /// Adding and deleting are edits like any other — there is nothing for a
    /// new kind of edit to remember to do.
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

    /// A restore rebuilds only what differs: an untouched feature keeps its
    /// entity, a changed one gives its entity up to be respawned.
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

    /// Reverting to the file is one undo step and leaves the map clean;
    /// undoing it brings the edits back and makes it dirty again.
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

    /// A map that arrives from somewhere else — a file, a server — brings no
    /// history with it and leaves none behind.
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

    /// A tab too short for both halves must still give a valid range.
    ///
    /// `feature_editor_default_height` clamps into it, and `f32::clamp`
    /// panics when its bounds are inverted — so a panel dragged narrow enough
    /// would take the editor down rather than merely look wrong.
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
