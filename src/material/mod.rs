//! The material editor: texture-graph's panels in Grackle's window, editing one `.tgraph` under a
//! pack's `materials/`. What a material is for is planned in
//! [documentation/surfaces.md](../../documentation/surfaces.md).
//!
//! The panels and their text are texture-graph's, so they are English whatever `--lang` says;
//! Grackle's menus around them go through the lang layer. Previews take texture-graph's CPU path
//! until they share Bevy's device.
//!
//! Undo is whole-graph snapshots taken when the editor is idle, as the map's is: texture-graph
//! queues its edits as commands and has no history of its own.

pub mod ui;

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use texture_graph_core::{FileMetadata, Graph, TextureGraphFile};
use texture_graph_editor::{EditCmd, Editor};

use crate::common::app_mode::AppMode;
use crate::editor::multicam::Multicam;

pub const MATERIALS_DIR: &str = "materials";
pub use texture_graph_core::FILE_EXTENSION;

const UNDO_DEPTH: usize = 256;

/// Square. The CPU path evaluates every pixel on the main thread, so it stays small.
const PREVIEW_SIZE: u32 = 256;

/// Non-`Send`: the editor holds its host's texture registry as a trait object.
pub struct MaterialEditor {
    pub editor: Editor,
    /// `None` until saved; a save then asks where.
    pub path: Option<PathBuf>,
    undo: Vec<Graph>,
    redo: Vec<Graph>,
    /// The graph as of the last checkpoint, and its text. `Graph` has no `PartialEq`, so changes
    /// are found by comparing text.
    checkpoint: (Graph, String),
    /// The text as last opened or saved, which is what dirty is measured from.
    clean: String,
    /// Set when an edit lands; the checkpoint waits for idle.
    unsettled: bool,
}

impl Default for MaterialEditor {
    fn default() -> Self {
        let mut material = Self {
            editor: Editor::new(None, PREVIEW_SIZE),
            path: None,
            undo: Vec::new(),
            redo: Vec::new(),
            checkpoint: (Graph::new(), String::new()),
            clean: String::new(),
            unsettled: false,
        };
        material.open(Graph::new(), None);
        material
    }
}

fn text_of(graph: &Graph) -> String {
    ron::to_string(graph).unwrap_or_default()
}

impl MaterialEditor {
    pub fn graph(&self) -> &Graph {
        &self.editor.graph
    }

    /// Changed since it was opened or saved, as of the last checkpoint.
    pub fn dirty(&self) -> bool {
        self.clean != self.checkpoint.1
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What the panels queued this frame has landed.
    pub fn edited(&mut self, changed: bool) {
        self.unsettled |= changed;
    }

    /// One undo step per stretch of activity: a drag or a typed name is one step, not one per
    /// frame or letter.
    pub fn checkpoint_when_idle(&mut self, idle: bool) {
        if !idle || !self.unsettled {
            return;
        }
        self.unsettled = false;
        let text = text_of(&self.editor.graph);
        if text == self.checkpoint.1 {
            return;
        }
        let before = std::mem::replace(&mut self.checkpoint, (self.editor.graph.clone(), text));
        self.undo.push(before.0);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self) {
        // An edit not yet checkpointed is the step being undone.
        self.checkpoint_when_idle(true);
        let Some(previous) = self.undo.pop() else { return };
        self.redo.push(self.editor.graph.clone());
        self.show(previous);
    }

    pub fn redo(&mut self) {
        let Some(next) = self.redo.pop() else { return };
        self.undo.push(self.editor.graph.clone());
        self.show(next);
    }

    /// Replaces the graph through texture-graph's own command, so its selection and previews are
    /// reset the way an open resets them, and applies it now rather than next frame.
    ///
    /// The canvas the panels draw on is added here if the graph lacks it. The panels would add it
    /// themselves on their first frame, and that is housekeeping rather than an edit: counted, a
    /// file would read as changed the moment it opened, and undo would take the canvas away.
    fn show(&mut self, graph: Graph) {
        let canvas = self.editor.ui.active_canvas.clone();
        let lacks_canvas = !graph.canvases.contains_key(&canvas);
        self.editor.ui.push(EditCmd::Replace(graph));
        if lacks_canvas {
            self.editor.ui.push(EditCmd::AddCanvas(canvas));
        }
        self.editor.end_frame();
        self.checkpoint = (self.editor.graph.clone(), text_of(&self.editor.graph));
        self.unsettled = false;
    }

    /// A different document: history does not carry over.
    pub fn open(&mut self, graph: Graph, path: Option<PathBuf>) {
        self.undo.clear();
        self.redo.clear();
        self.path = path;
        self.show(graph);
        self.clean = self.checkpoint.1.clone();
    }

    /// Back to what is on disk, as one undo step.
    pub fn revert_to(&mut self, saved: Graph) {
        self.checkpoint_when_idle(true);
        self.undo.push(self.editor.graph.clone());
        self.redo.clear();
        self.show(saved);
        self.clean = self.checkpoint.1.clone();
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.checkpoint_when_idle(true);
        self.clean = self.checkpoint.1.clone();
        self.path = Some(path);
    }
}

/// The file's stem is its name, as a prop's is.
pub fn to_text(graph: &Graph, path: &Path) -> Result<String, String> {
    let name = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let metadata = FileMetadata {
        name,
        written_by: format!("grackle {}", env!("CARGO_PKG_VERSION")),
        ..Default::default()
    };
    texture_graph_core::save_to_string(&TextureGraphFile::new(metadata, graph.clone()))
        .map_err(|e| e.to_string())
}

/// Authoring side, so `std::fs`; the game will read materials through `Assets`.
pub fn save(path: &Path, graph: &Graph) -> Result<(), String> {
    let text = to_text(graph, path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load(path: &Path) -> Result<Graph, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    texture_graph_core::load_from_str(&text)
        .map(|file| file.graph)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `--material`. A resource rather than an initial state, so entering goes through
/// `OnEnter(AppMode::Material)`.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StartInMaterialEditor(pub bool);

fn start_in_the_material_editor(
    start: Res<StartInMaterialEditor>,
    mut next: ResMut<NextState<AppMode>>,
    mut done: Local<bool>,
) {
    if !start.0 || *done {
        return;
    }
    info!("Opening the material editor, as asked");
    next.set(AppMode::Material);
    *done = true;
}

/// The panels cover the window, so the viewports would draw for nobody. The next mode that shows
/// them switches them back on.
fn switch_the_viewports_off(mut cameras: Query<&mut Camera, With<Multicam>>) {
    for mut camera in &mut cameras {
        camera.is_active = false;
    }
}

pub struct MaterialEditorPlugin;

impl Plugin for MaterialEditorPlugin {
    fn build(&self, app: &mut App) {
        app.insert_non_send(MaterialEditor::default())
            .init_resource::<StartInMaterialEditor>()
            // Ungated: it has to run in whatever mode the process started in.
            .add_systems(Update, start_in_the_material_editor)
            .add_systems(OnEnter(AppMode::Material), switch_the_viewports_off)
            .add_plugins(ui::MaterialUiPlugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use texture_graph_core::LayerKind;

    /// A graph that differs from `Graph::new()`.
    fn edited(editor: &mut MaterialEditor) {
        let mut graph = editor.graph().clone();
        graph.add_layer("edit", LayerKind::Noise(Default::default())).expect("a noise layer can be added");
        editor.editor.ui.push(EditCmd::Replace(graph));
        let changed = editor.editor.end_frame();
        editor.edited(changed);
    }

    #[test]
    fn an_edit_is_one_undo_step_once_idle() {
        let mut material = MaterialEditor::default();
        let fresh = text_of(material.graph());
        edited(&mut material);
        material.checkpoint_when_idle(false);
        assert!(!material.can_undo(), "checkpointed while busy");
        material.checkpoint_when_idle(true);
        assert!(material.can_undo());

        material.undo();
        assert_eq!(text_of(material.graph()), fresh);
        assert!(material.can_redo());
        material.redo();
        assert_ne!(text_of(material.graph()), fresh);
    }

    #[test]
    fn undo_takes_an_edit_that_was_never_checkpointed() {
        let mut material = MaterialEditor::default();
        let fresh = text_of(material.graph());
        edited(&mut material);
        material.undo();
        assert_eq!(text_of(material.graph()), fresh);
    }

    /// The panels add their canvas on the first frame; if `show` did not, a new material would
    /// open with an asterisk and an undo step.
    #[test]
    fn opening_brings_the_canvas_the_panels_draw_on() {
        let material = MaterialEditor::default();
        assert!(material.graph().canvases.contains_key(&material.editor.ui.active_canvas));
        assert!(!material.dirty());
        assert!(!material.can_undo());
    }

    #[test]
    fn saving_clears_dirty_and_editing_sets_it() {
        let mut material = MaterialEditor::default();
        assert!(!material.dirty(), "an empty untitled graph has nothing to lose");
        edited(&mut material);
        material.checkpoint_when_idle(true);
        assert!(material.dirty());
        material.mark_saved(PathBuf::from("assets/user/materials/x.tgraph"));
        assert!(!material.dirty());
    }

    #[test]
    fn a_saved_material_reads_back_the_same() {
        let mut material = MaterialEditor::default();
        edited(&mut material);
        let path = Path::new("assets/user/materials/round_trip.tgraph");
        let text = to_text(material.graph(), path).expect("writes");
        let read = texture_graph_core::load_from_str(&text).expect("reads").graph;
        assert_eq!(text_of(&read), text_of(material.graph()));
    }
}
