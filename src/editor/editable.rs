use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::{egui, EguiPrimaryContextPass, EguiContexts};
use bevy_egui::egui::{Context, Widget};
use lazy_static::lazy_static;
use serde::{Serialize, Deserialize};
use crate::common::cuboid::{CuboidPoint, GrackleCuboid};
use crate::common::PointResolutionError;
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
            .add_systems(EguiPrimaryContextPass, EditorActions::floating_ui)
        ;
    }
}

#[typetag::serde]
pub trait EditorObject: Send + Sync {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError>;
    fn editor_ui(&mut self, ctx: &mut Context);
    fn type_name(&self) -> String;
    fn debug_gizmos(&self, gizmos: &mut Gizmos);
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
}

impl Default for EditorActions {
    fn default() -> Self {
        let mut a = Self {
            actions: HashMap::new(),
            action_order: vec![],
            id_counter: 0,
            selected_action: None,
        };
        
        a.take_action(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
        a.take_action(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        a.take_action(Box::new(GlobalPoint::new(0.0, 5.0, 0.0)));
        
        a
    }
}

impl EditorActions {
    fn next_id(&mut self) -> EditorActionId {
        let id = EditorActionId { _id: self.id_counter };
        self.id_counter += 1;
        id
    }
    
    pub fn take_action(&mut self, object: Box<dyn EditorObject>) {
        let new_id = self.next_id();
        let new_action = EditorAction {
            id: new_id,
            object,
            parents: vec![],
        };
        self.actions.insert(new_action.id, new_action);
        self.action_order.push(new_id);
    }
    
    pub fn get_action(&self, id: &EditorActionId) -> Option<&EditorAction> {
        self.actions.get(id)
    }
    
    pub fn ui(
        ui: &mut egui::Ui,
        mut actions: &mut Self,
    ) {
        let mut selection_changed = false;
        let mut next_selected = actions.selected_action;

        egui::ScrollArea::vertical().show(ui, |ui| {
            for id in actions.action_order.iter() {
                let action = actions.get_action(id).unwrap();
                let is_selected = actions.selected_action == Some(*id);

                if ui.add_sized([ui.available_width(), 0.0],
                    egui::SelectableLabel::new(is_selected, action.type_name_with_id())).clicked() {
                    if is_selected {
                        next_selected = None;
                    } else {
                        next_selected = Some(*id);
                    }
                    selection_changed = true;
                }
            }
        });

        if selection_changed {
            actions.selected_action = next_selected;
        }
    }
    
    fn floating_ui(mut contexts: EguiContexts, mut actions: ResMut<Self>, mut gizmos: Gizmos,) {
        let ctx = contexts.ctx_mut();
        if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return; }
        let ctx = ctx.unwrap();
        
        if let Some(selected_id) = actions.selected_action {
            let action = actions.actions.get_mut(&selected_id).unwrap();
            action.object.editor_ui(ctx);

            action.object.debug_gizmos(&mut gizmos);
        }
    }
}

impl EditorActionId {
    pub fn new() -> Self {
        EditorActionId { _id: 0 }
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
}

pub struct RefVec3 {
    x: Ref32,
    y: Ref32,
    z: Ref32,
}

pub enum Ref32 {
    Absolute(f32),
    Relative(EditorActionId, CuboidPoint, f32),
}

impl Ref32 {
    pub fn resolve(&self, actions: ResMut<EditorActions>) -> Result<f32, PointResolutionError> {
        match self {
            Ref32::Absolute(f) => Ok(*f),
            Ref32::Relative(id, p, f) => {
                let action = actions.get_action(id).ok_or(PointResolutionError::NoSuchReferent)?;
                todo!()
            }
        }
    }
}
