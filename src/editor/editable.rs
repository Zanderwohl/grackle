use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy_egui::{egui, EguiPrimaryContextPass, EguiContexts};
use bevy_egui::egui::Context;
use lazy_static::lazy_static;
use serde::{Serialize, Deserialize};
use crate::common::PointResolutionError;
use crate::editor::editor_room::EditorRoom;
use crate::editor::global_point::GlobalPoint;
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
            .add_systems(Update, (
                EditorActions::undo_redo_shortcuts,
                EditorActions::sync_entities,
                EditorActions::handle_edits,
            ).chain())
            .add_systems(EguiPrimaryContextPass, EditorActions::floating_ui)
        ;
    }
}

#[derive(Component)]
pub struct EditorObjectTag {
    pub editor_id: u64,
}

#[typetag::serde]
pub trait EditorObject: Send + Sync {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError>;
    /// Returns true if the object was modified this frame.
    fn editor_ui(&mut self, ctx: &mut Context) -> bool;
    fn type_name(&self) -> String;
    fn debug_gizmos(&self, gizmos: &mut Gizmos);
    fn entity(&self) -> Option<Entity>;
    fn set_entity(&mut self, entity: Option<Entity>);
    fn apply_to_entity(&self, commands: &mut Commands, entity: Entity);
    /// Resolve all PointRef fields against the current state of the actions map.
    fn resolve_references(&mut self, actions: &HashMap<EditorActionId, EditorAction>);
    /// Return the EditorActionIds this object's PointRefs depend on.
    fn parent_ids(&self) -> Vec<EditorActionId>;
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
    cursor: u64,
    pending_despawns: Vec<Entity>,
}

impl Default for EditorActions {
    fn default() -> Self {
        let mut a = Self {
            actions: HashMap::new(),
            action_order: vec![],
            id_counter: 0,
            selected_action: None,
            cursor: 0,
            pending_despawns: vec![],
        };
        
        let p1 = a.take_action(Box::new(GlobalPoint::new(-3.0, 0.0, -3.0)));
        let p2 = a.take_action(Box::new(GlobalPoint::new(3.0, 3.0, 3.0)));
        a.take_action(Box::new(EditorRoom::from_points(p1, p2)));
        
        a
    }
}

impl EditorActions {
    pub fn next_id(&mut self) -> EditorActionId {
        let id = EditorActionId { _id: self.id_counter };
        self.id_counter += 1;
        id
    }
    
    pub fn take_action(&mut self, object: Box<dyn EditorObject>) -> EditorActionId {
        let cur = self.cursor as usize;
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
        self.cursor = self.action_order.len() as u64;
        new_id
    }
    
    pub fn get_action(&self, id: &EditorActionId) -> Option<&EditorAction> {
        self.actions.get(id)
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.action_order.len() as u64
    }

    pub fn undo(&mut self) {
        if !self.can_undo() { return; }
        self.cursor -= 1;
        if let Some(selected) = self.selected_action {
            if let Some(idx) = self.action_order.iter().position(|id| *id == selected) {
                if idx as u64 >= self.cursor {
                    self.selected_action = None;
                }
            }
        }
    }

    pub fn redo(&mut self) {
        if !self.can_redo() { return; }
        self.cursor += 1;
    }
    
    pub fn ui(
        ui: &mut egui::Ui,
        actions: &mut Self,
    ) {
        ui.horizontal(|ui| {
            if ui.add_enabled(actions.can_undo(), egui::Button::new("⮪ ".to_owned() + &get!("editor.timeline.undo"))).clicked() {
                actions.undo();
            }
            if ui.add_enabled(actions.can_redo(), egui::Button::new(get!("editor.timeline.redo") + " ⮫")).clicked() {
                actions.redo();
            }
        });

        ui.separator();

        let mut selection_changed = false;
        let mut next_selected = actions.selected_action;

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, id) in actions.action_order.iter().enumerate() {
                let action = actions.get_action(id).unwrap();
                let is_selected = actions.selected_action == Some(*id);
                let is_active = (i as u64) < actions.cursor;

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
        });

        if selection_changed {
            actions.selected_action = next_selected;
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

    fn floating_ui(
        mut contexts: EguiContexts,
        mut actions: ResMut<Self>,
        mut gizmos: Gizmos,
        mut edit_events: MessageWriter<EditEvent>,
    ) {
        let ctx = contexts.ctx_mut();
        if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return; }
        let ctx = ctx.unwrap();
        
        if let Some(selected_id) = actions.selected_action {
            let is_active = actions.action_order.iter()
                .position(|id| *id == selected_id)
                .is_some_and(|idx| (idx as u64) < actions.cursor);

            if is_active {
                let action = actions.actions.get_mut(&selected_id).unwrap();
                let was_edited = action.object.editor_ui(ctx);
                action.object.debug_gizmos(&mut gizmos);

                if was_edited {
                    if let Some(entity) = action.object.entity() {
                        edit_events.write(EditEvent {
                            editor_id: selected_id._id(),
                            action_id: selected_id,
                            entity,
                        });
                    }
                }
            }
        }
    }

    fn sync_entities(mut actions: ResMut<Self>, mut commands: Commands) {
        for entity in actions.pending_despawns.drain(..) {
            commands.entity(entity).despawn();
        }

        let cursor = actions.cursor;
        let order: Vec<(usize, EditorActionId)> = actions.action_order.iter()
            .enumerate()
            .map(|(i, id)| (i, *id))
            .collect();

        for (i, id) in order {
            let should_exist = (i as u64) < cursor;
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
        let mut queue: Vec<EditorActionId> = edit_events.read()
            .map(|e| e.action_id)
            .collect();

        if queue.is_empty() { return; }

        // Resolve + apply the initially edited objects
        for id in queue.clone() {
            if let Some(mut action) = actions.actions.remove(&id) {
                action.object.resolve_references(&actions.actions);
                if let Some(entity) = action.object.entity() {
                    action.object.apply_to_entity(&mut commands, entity);
                }
                actions.actions.insert(id, action);
            }
        }

        // BFS: propagate through every downstream child in the DAG
        let mut visited: HashSet<EditorActionId> = queue.iter().copied().collect();

        while !queue.is_empty() {
            let parent_set: HashSet<EditorActionId> = queue.drain(..).collect();

            let children: Vec<EditorActionId> = actions.actions.iter()
                .filter(|(_, action)| action.object.parent_ids().iter().any(|p| parent_set.contains(p)))
                .map(|(id, _)| *id)
                .filter(|id| visited.insert(*id))
                .collect();

            for child_id in &children {
                if let Some(mut action) = actions.actions.remove(child_id) {
                    action.object.resolve_references(&actions.actions);
                    if let Some(entity) = action.object.entity() {
                        action.object.apply_to_entity(&mut commands, entity);
                    }
                    actions.actions.insert(*child_id, action);
                }
            }

            queue = children;
        }
    }
}

impl EditorActionId {
    pub fn new() -> Self {
        EditorActionId { _id: 0 }
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
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum Axis { X, Y, Z }

#[derive(Serialize, Deserialize, Clone)]
pub enum AxisRef {
    Absolute(f32),
    Relative {
        action: EditorActionId,
        point_key: String,
        axis: Axis,
        offset: f32,
    },
}

impl AxisRef {
    pub fn resolve(&self, actions: &HashMap<EditorActionId, EditorAction>) -> Result<f32, PointResolutionError> {
        match self {
            AxisRef::Absolute(v) => Ok(*v),
            AxisRef::Relative { action, point_key, axis, offset } => {
                let a = actions.get(action).ok_or(PointResolutionError::NoSuchReferent)?;
                let point = a.object.get_point(point_key)?;
                let base = match axis {
                    Axis::X => point.x,
                    Axis::Y => point.y,
                    Axis::Z => point.z,
                };
                Ok(base + offset)
            }
        }
    }

    pub fn value_mut(&mut self) -> &mut f32 {
        match self {
            AxisRef::Absolute(v) => v,
            AxisRef::Relative { offset, .. } => offset,
        }
    }

    pub fn value(&self) -> f32 {
        match self {
            AxisRef::Absolute(v) => *v,
            AxisRef::Relative { offset, .. } => *offset,
        }
    }

    pub fn referenced_action(&self) -> Option<EditorActionId> {
        match self {
            AxisRef::Absolute(_) => None,
            AxisRef::Relative { action, .. } => Some(*action),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PointRef {
    pub x: AxisRef,
    pub y: AxisRef,
    pub z: AxisRef,
}

impl PointRef {
    pub fn absolute(x: f32, y: f32, z: f32) -> Self {
        Self {
            x: AxisRef::Absolute(x),
            y: AxisRef::Absolute(y),
            z: AxisRef::Absolute(z),
        }
    }

    pub fn reference(action: EditorActionId) -> Self {
        Self {
            x: AxisRef::Relative { action, point_key: String::new(), axis: Axis::X, offset: 0.0 },
            y: AxisRef::Relative { action, point_key: String::new(), axis: Axis::Y, offset: 0.0 },
            z: AxisRef::Relative { action, point_key: String::new(), axis: Axis::Z, offset: 0.0 },
        }
    }

    pub fn resolve(&self, actions: &HashMap<EditorActionId, EditorAction>) -> Result<Vec3, PointResolutionError> {
        Ok(Vec3::new(
            self.x.resolve(actions)?,
            self.y.resolve(actions)?,
            self.z.resolve(actions)?,
        ))
    }

    pub fn referenced_actions(&self) -> Vec<EditorActionId> {
        let mut set = HashSet::new();
        if let Some(id) = self.x.referenced_action() { set.insert(id); }
        if let Some(id) = self.y.referenced_action() { set.insert(id); }
        if let Some(id) = self.z.referenced_action() { set.insert(id); }
        set.into_iter().collect()
    }
}
