use std::path::PathBuf;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use lazy_static::lazy_static;
use serde::{Serialize, Deserialize};
use crate::common::PointResolutionError;
use crate::constants::MAP_BLUEPRINT_EXTENSION;
use crate::editor::editor_room::EditorRoom;
use crate::editor::global_point::GlobalPoint;
use crate::editor::grackle_point_light::GracklePointLight;
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
            .init_resource::<EditorActions>()
            .add_message::<EditEvent>()
            .add_systems(Startup, EditorActions::load_template)
            .add_systems(Update, (
                EditorActions::undo_redo_shortcuts,
                EditorActions::sync_entities,
                EditorActions::handle_edits,
                EditorActions::draw_affected_gizmos,
            ).chain())
        ;
    }
}

#[derive(Component, Debug)]
pub struct EditorObjectTag {
    pub editor_id: u64,
}

#[typetag::serde]
pub trait EditorObject: Send + Sync {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError>;
    /// Returns true if the object was modified this frame.
    fn editor_ui(&mut self, ui: &mut egui::Ui, actions: &HashMap<EditorActionId, EditorAction>, prior_action_order: &[EditorActionId], retarget_request: &mut Option<String>) -> bool;
    fn type_name(&self) -> String;
    fn type_key(&self) -> &'static str;
    fn debug_gizmos(&self, gizmos: &mut Gizmos);
    fn entity(&self) -> Option<Entity>;
    fn set_entity(&mut self, entity: Option<Entity>);
    fn apply_to_entity(&self, commands: &mut Commands, entity: Entity);
    /// Resolve all PointRef fields against the current state of the actions map.
    fn resolve_references(&mut self, actions: &HashMap<EditorActionId, EditorAction>);
    /// Return the EditorActionIds this object's PointRefs depend on.
    fn parent_ids(&self) -> Vec<EditorActionId>;
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
    fn drag_handle(&mut self, is_max: bool, axis: u8, new_world_value: f32) -> bool { false }

    /// Returns the resolved min and max bounds if this object is a room-like
    /// object with drag handles. Used to position handles.
    fn drag_handle_bounds(&self) -> Option<(Vec3, Vec3)> { None }

    /// Return all named PointRef slots on this object (for save/load).
    fn point_ref_slots(&self) -> Vec<&str> { vec![] }

    /// Return extra scalar fields for save/load (e.g. light intensity).
    fn scalar_fields(&self) -> Vec<(&str, f32)> { vec![] }

    /// Set a scalar field by name (for loading).
    fn set_scalar_field(&mut self, _key: &str, _value: f32) {}

    /// Get a reference to a named PointRef on this object.
    fn get_point_ref(&self, _key: &str) -> Option<&PointRef> { None }

    /// Get a mutable reference to a named PointRef on this object.
    /// Keys: GlobalPoint/GracklePointLight use "location" (or ""),
    /// EditorRoom uses "min" / "max".
    fn get_point_ref_mut(&mut self, _key: &str) -> Option<&mut PointRef> { None }
}

/// Create a blank EditorObject from a type_key string (for loading from DB).
/// PointRefs are initialized to absolute zero and must be overwritten after construction.
pub fn create_object_from_type_key(type_key: &str) -> Option<Box<dyn EditorObject>> {
    match type_key {
        "global_point" => Some(Box::new(GlobalPoint::new(0.0, 0.0, 0.0))),
        "grackle_point_light" => Some(Box::new(GracklePointLight::new(0.0, 0.0, 0.0))),
        "editor_room" => Some(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(0.0, 0.0, 0.0),
            PointRef::absolute(0.0, 0.0, 0.0),
        ))),
        _ => None,
    }
}

#[derive(Message)]
pub struct EditEvent {
    pub editor_id: u64,
    pub action_id: EditorActionId,
    pub entity: Entity,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Debug, Clone, Copy)]
#[derive(Hash)]
pub struct EditorActionId {
    _id: u64,
}

#[derive(Resource)]
pub struct EditorActions {
    actions: HashMap<EditorActionId, EditorAction>,
    action_order: Vec<EditorActionId>,
    id_counter: u64,
    selected_action: Option<EditorActionId>,
    /// Topologically sorted list of the selected action and all its DAG descendants.
    /// Parents always appear before their dependants.
    selection_affected: Option<Vec<EditorActionId>>,
    rollback_bar: u64,
    pending_despawns: Vec<Entity>,
}

impl Default for EditorActions {
    fn default() -> Self {
        Self {
            actions: HashMap::new(),
            action_order: vec![],
            id_counter: 0,
            selected_action: None,
            selection_affected: None,
            rollback_bar: 0,
            pending_despawns: vec![],
        }
    }
}

impl EditorActions {
    pub fn from_parts(
        actions: HashMap<EditorActionId, EditorAction>,
        action_order: Vec<EditorActionId>,
        id_counter: u64,
        rollback_bar: u64,
    ) -> Self {
        Self {
            actions,
            action_order,
            id_counter,
            selected_action: None,
            selection_affected: None,
            rollback_bar,
            pending_despawns: vec![],
        }
    }

    fn load_template(mut actions: ResMut<Self>) {
        let path = PathBuf::from(format!(
            "assets/default/blueprints/new.{}", MAP_BLUEPRINT_EXTENSION
        ));
        match save::load(&path) {
            Ok(loaded) => {
                *actions = loaded;
                info!("Loaded template from {:?}", path);
            }
            Err(e) => error!("Failed to load template {:?}: {}", path, e),
        }
    }

    pub fn id_counter(&self) -> u64 {
        self.id_counter
    }

    pub fn rollback_bar(&self) -> u64 {
        self.rollback_bar
    }

    pub fn next_id(&mut self) -> EditorActionId {
        let id = EditorActionId { _id: self.id_counter };
        self.id_counter += 1;
        id
    }
    
    pub fn select(&mut self, selection: Option<EditorActionId>) {
        self.selected_action = selection;
        self.selection_affected = selection.map(|root| {
            let mut result = vec![root];
            let mut visited: HashSet<EditorActionId> = HashSet::from([root]);
            let mut queue = vec![root];

            while !queue.is_empty() {
                let parent_set: HashSet<EditorActionId> = queue.drain(..).collect();

                let children: Vec<EditorActionId> = self.actions.iter()
                    .filter(|(_, action)| action.parents.iter().any(|p| parent_set.contains(p)))
                    .map(|(id, _)| *id)
                    .filter(|id| visited.insert(*id))
                    .collect();

                result.extend(&children);
                queue = children;
            }

            result
        });
    }

    pub fn selection_affected(&self) -> Option<&[EditorActionId]> {
        self.selection_affected.as_deref()
    }

    /// Returns an iterator of (EditorActionId, &EditorAction) for all active actions
    /// (those before the rollback bar).
    pub fn active_actions(&self) -> impl Iterator<Item = (EditorActionId, &EditorAction)> {
        let rollback_bar = self.rollback_bar as usize;
        self.action_order[..rollback_bar].iter().filter_map(move |id| {
            self.actions.get(id).map(|a| (*id, a))
        })
    }

    pub fn selected_action(&self) -> Option<EditorActionId> {
        self.selected_action
    }

    pub fn actions_map(&self) -> &HashMap<EditorActionId, EditorAction> {
        &self.actions
    }

    pub fn action_order(&self) -> &[EditorActionId] {
        &self.action_order
    }

    pub fn actions_mut(&mut self) -> &mut HashMap<EditorActionId, EditorAction> {
        &mut self.actions
    }

    pub fn queue_despawn(&mut self, entity: Entity) {
        self.pending_despawns.push(entity);
    }

    pub fn take_action(&mut self, object: Box<dyn EditorObject>) -> EditorActionId {
        let cur = self.rollback_bar as usize;
        if cur < self.action_order.len() {
            for id in self.action_order.drain(cur..) {
                if let Some(action) = self.actions.remove(&id) {
                    if let Some(entity) = action.object.entity() {
                        self.pending_despawns.push(entity);
                    }
                }
            }
        }

        let parents = object.parent_ids();
        let new_id = self.next_id();
        let new_action = EditorAction {
            id: new_id,
            object,
            parents,
        };
        self.actions.insert(new_action.id, new_action);
        self.action_order.push(new_id);
        self.rollback_bar = self.action_order.len() as u64;
        new_id
    }
    
    pub fn get_action(&self, id: &EditorActionId) -> Option<&EditorAction> {
        self.actions.get(id)
    }

    pub fn can_undo(&self) -> bool {
        self.rollback_bar > 0
    }

    pub fn can_redo(&self) -> bool {
        self.rollback_bar < self.action_order.len() as u64
    }

    pub fn undo(&mut self) {
        if !self.can_undo() { return; }
        self.rollback_bar -= 1;
        if let Some(selected) = self.selected_action {
            if let Some(idx) = self.action_order.iter().position(|id| *id == selected) {
                if idx as u64 >= self.rollback_bar {
                    self.select(None);
                }
            }
        }
    }

    pub fn redo(&mut self) {
        if !self.can_redo() { return; }
        self.rollback_bar += 1;
    }
    
    pub fn ui(
        ui: &mut egui::Ui,
        actions: &mut Self,
        edit_events: &mut Vec<EditEvent>,
        retarget_out: &mut Option<(EditorActionId, String)>,
    ) {
        // Section 1: Undo/Redo (compact, top)
        ui.horizontal(|ui| {
            if ui.add_enabled(actions.can_undo(), egui::Button::new("⮪ ".to_owned() + &get!("editor.timeline.undo"))).clicked() {
                actions.undo();
            }
            if ui.add_enabled(actions.can_redo(), egui::Button::new(get!("editor.timeline.redo") + " ⮫")).clicked() {
                actions.redo();
            }
        });

        ui.separator();

        // Section 3: Editor for selected action (bottom, takes as much as needed)
        // We render this into a bottom panel so it pins to the bottom,
        // then the scroll area fills whatever's left.
        let mut was_edited = false;
        let mut edited_id = None;
        let mut entity_for_event: Option<Entity> = None;

        if let Some(selected_id) = actions.selected_action {
            let selected_idx = actions.action_order.iter()
                .position(|id| *id == selected_id);
            let is_active = selected_idx.is_some_and(|idx| (idx as u64) < actions.rollback_bar);

            if is_active {
                let selected_idx = selected_idx.unwrap();
                let prior_order: Vec<EditorActionId> = actions.action_order[..selected_idx].to_vec();

                egui::TopBottomPanel::bottom("editor_action_panel")
                    .resizable(false)
                    .show_inside(ui, |ui| {
                        ui.separator();
                        if let Some(mut action) = actions.actions.remove(&selected_id) {
                            ui.heading(action.type_name_with_id());
                            let mut retarget_request: Option<String> = None;
                            let edited = action.object_mut().editor_ui(ui, &actions.actions, &prior_order, &mut retarget_request);
                            if let Some(label) = retarget_request {
                                *retarget_out = Some((selected_id, label));
                            }
                            if edited {
                                action.parents = action.object().parent_ids().to_vec();
                                entity_for_event = action.object().entity();
                                was_edited = true;
                                edited_id = Some(selected_id);
                            }
                            actions.actions.insert(selected_id, action);
                        }
                    });
            }
        }

        if was_edited {
            if let Some(id) = edited_id {
                actions.select(Some(id));
                if let Some(entity) = entity_for_event {
                    edit_events.push(EditEvent {
                        editor_id: id._id(),
                        action_id: id,
                        entity,
                    });
                }
            }
        }

        // Section 2: History list (fills remaining space)
        let mut selection_changed = false;
        let mut next_selected = actions.selected_action;

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, id) in actions.action_order.iter().enumerate() {
                let action = actions.get_action(id).unwrap();
                let is_selected = actions.selected_action == Some(*id);
                let is_active = (i as u64) < actions.rollback_bar;

                let label_text = action.type_name_with_id();
                let label = if is_active {
                    egui::SelectableLabel::new(is_selected, label_text)
                } else {
                    egui::SelectableLabel::new(false,
                        egui::RichText::new(label_text).strikethrough().weak())
                };

                if ui.add_sized([ui.available_width(), 0.0], label).clicked() && is_active {
                    next_selected = Some(if is_selected { None } else { Some(*id) }).flatten();
                    selection_changed = true;
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
            actions.select(next_selected);
        }
    }

    fn undo_redo_shortcuts(
        keys: Res<ButtonInput<KeyCode>>,
        mut actions: ResMut<EditorActions>,
        mut egui_contexts: EguiContexts,
    ) {
        if let Ok(ctx) = egui_contexts.ctx_mut() {
            if ctx.wants_keyboard_input() {
                return;
            }
        }

        let cmd = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
        let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

        let undo = (cmd || ctrl) && !shift && keys.just_pressed(KeyCode::KeyZ);
        let redo = ((cmd || ctrl) && shift && keys.just_pressed(KeyCode::KeyZ))
            || (ctrl && keys.just_pressed(KeyCode::KeyY));

        if redo {
            actions.redo();
        } else if undo {
            actions.undo();
        }
    }


    fn sync_entities(mut actions: ResMut<Self>, mut commands: Commands) {
        for entity in actions.pending_despawns.drain(..) {
            commands.entity(entity).despawn();
        }

        let rollback_bar = actions.rollback_bar;
        let order: Vec<(usize, EditorActionId)> = actions.action_order.iter()
            .enumerate()
            .map(|(i, id)| (i, *id))
            .collect();

        for (i, id) in order {
            let should_exist = (i as u64) < rollback_bar;
            let needs_spawn = should_exist
                && actions.actions.get(&id).is_some_and(|a| a.object.entity().is_none());

            if needs_spawn {
                let mut action = actions.actions.remove(&id).unwrap();
                action.object.resolve_references(&actions.actions);
                let entity = commands.spawn(EditorObjectTag { editor_id: id._id }).id();
                action.object.set_entity(Some(entity));
                action.object.apply_to_entity(&mut commands, entity);
                actions.actions.insert(id, action);
            } else if !should_exist {
                if let Some(mut action) = actions.actions.remove(&id) {
                    if let Some(entity) = action.object.entity() {
                        commands.entity(entity).despawn();
                        action.object.set_entity(None);
                    }
                    actions.actions.insert(id, action);
                }
            }
        }
    }

    fn handle_edits(
        mut actions: ResMut<EditorActions>,
        mut edit_events: MessageReader<EditEvent>,
        mut commands: Commands,
    ) {
        if edit_events.read().next().is_none() { return; }
        edit_events.clear();

        let affected = match &actions.selection_affected {
            Some(ids) => ids.clone(),
            None => return,
        };

        for id in &affected {
            if let Some(mut action) = actions.actions.remove(id) {
                action.object.resolve_references(&actions.actions);
                if let Some(entity) = action.object.entity() {
                    action.object.apply_to_entity(&mut commands, entity);
                }
                actions.actions.insert(*id, action);
            }
        }
    }

    fn draw_affected_gizmos(actions: Res<EditorActions>, mut gizmos: Gizmos) {
        let Some(affected) = &actions.selection_affected else { return; };
        for id in affected {
            if let Some(action) = actions.actions.get(id) {
                action.object.debug_gizmos(&mut gizmos);
            }
        }
    }
}

impl EditorActionId {
    pub fn new() -> Self {
        EditorActionId { _id: 0 }
    }

    pub fn from_raw(id: u64) -> Self {
        EditorActionId { _id: id }
    }

    pub fn _id(&self) -> u64 {
        self._id
    }
}

impl std::fmt::Display for EditorActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self._id)
    }
}

// Essentially, these are nodes on a directed acyclic graph.
// Editor actions may depend on previous actions to resolve.
// They may also not depend on anything, with all descendants tracing their ancestry back --
// such a case would lead to multiple disjoint graphs, which is okay.
#[derive(Serialize, Deserialize)]
pub struct EditorAction {
    id: EditorActionId,
    object: Box<dyn EditorObject>,
    parents: Vec<EditorActionId>,
}

impl EditorAction {
    pub fn new(id: EditorActionId, object: Box<dyn EditorObject>, parents: Vec<EditorActionId>) -> Self {
        Self { id, object, parents }
    }

    pub fn id(&self) -> EditorActionId {
        self.id
    }

    pub fn object(&self) -> &dyn EditorObject {
        &*self.object
    }

    pub fn object_mut(&mut self) -> &mut dyn EditorObject {
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

    pub fn parents(&self) -> &[EditorActionId] {
        &self.parents
    }

    pub fn set_parents(&mut self, parents: Vec<EditorActionId>) {
        self.parents = parents;
    }
}

#[derive(Serialize, Deserialize, Clone)]
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

#[derive(Serialize, Deserialize, Clone)]
pub struct PointRef {
    pub reference: Option<EditorActionId>,
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

    pub fn reference(action: EditorActionId) -> Self {
        Self::reference_with_offset(action, 0.0, 0.0, 0.0)
    }

    pub fn reference_with_offset(action: EditorActionId, dx: f32, dy: f32, dz: f32) -> Self {
        Self {
            reference: Some(action),
            point_key: String::new(),
            x: AxisRef::Relative(dx),
            y: AxisRef::Relative(dy),
            z: AxisRef::Relative(dz),
            resolved_reference: None,
        }
    }

    pub fn resolve(&mut self, actions: &HashMap<EditorActionId, EditorAction>) -> Result<Vec3, PointResolutionError> {
        let base = self.reference
            .and_then(|id| actions.get(&id))
            .map(|a| a.object.get_point(&self.point_key))
            .transpose()?;
        self.resolved_reference = base;
        Ok(Vec3::new(
            self.x.resolve_with_base(base.map(|b| b.x))?,
            self.y.resolve_with_base(base.map(|b| b.y))?,
            self.z.resolve_with_base(base.map(|b| b.z))?,
        ))
    }

    pub fn referenced_actions(&self) -> Vec<EditorActionId> {
        self.reference.into_iter().collect()
    }

    pub fn editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        actions: &HashMap<EditorActionId, EditorAction>,
        prior_action_order: &[EditorActionId],
        retarget_request: &mut Option<String>,
    ) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).strong());
            if ui.small_button("⊕").on_hover_text("Retarget").clicked() {
                *retarget_request = Some(label.to_string());
            }
        });

        // Reference action dropdown
        let ref_label = self.reference
            .and_then(|id| actions.get(&id))
            .map(|a| a.type_name_with_id())
            .unwrap_or_else(|| "None".to_string());

        let mut new_reference = self.reference;
        let mut ref_changed = false;

        egui::ComboBox::from_id_salt(format!("{}_ref", label))
            .selected_text(&ref_label)
            .show_ui(ui, |ui| {
                if ui.selectable_label(self.reference.is_none(), "None").clicked() && self.reference.is_some() {
                    new_reference = None;
                    ref_changed = true;
                }
                for &id in prior_action_order {
                    if let Some(action) = actions.get(&id) {
                        let is_selected = self.reference == Some(id);
                        if ui.selectable_label(is_selected, action.type_name_with_id()).clicked() && !is_selected {
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
                    .and_then(|id| actions.get(&id))
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
            if let Some(ref_action) = actions.get(&ref_id) {
                let keys = ref_action.object.available_point_keys();
                if keys.len() > 1 {
                    let current_display = keys.iter()
                        .find(|(k, _)| k == &self.point_key)
                        .map(|(_, d)| d.as_str())
                        .unwrap_or("Default");

                    let mut new_key = self.point_key.clone();
                    let mut key_changed = false;

                    egui::ComboBox::from_id_salt(format!("{}_key", label))
                        .selected_text(current_display)
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
                        let new_base = ref_action.object.get_point(&new_key).unwrap_or(Vec3::ZERO);
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
    pub fn debug_gizmos(&self, resolved: Vec3, gizmos: &mut Gizmos) {
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
