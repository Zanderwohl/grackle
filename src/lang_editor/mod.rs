//! The lang editor: one `assets/<pack>/lang/<code>.toml` as a tree of keys, checked against every
//! pack's `en-US`. Its own `AppMode`, taking the whole window, as the material editor does.
//!
//! Undo is whole-document snapshots taken when idle, compared as text. Saving remerges the
//! strings in force, so the editor's own labels change on the spot.

pub mod doc;
pub mod ui;

use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::lang::{default_packs, lang_files, merged_layer, remerge, LangFile, FALLBACK_LANG, LANG_DIR};
use doc::{LangDoc, Reference};

pub const LANG_SUBDIR: &str = "lang";
pub const FILE_EXTENSION: &str = "toml";

const UNDO_DEPTH: usize = 256;

#[derive(Resource)]
pub struct LangEditor {
    pub doc: LangDoc,
    /// `None` until saved; a save then asks where.
    pub path: Option<PathBuf>,
    undo: Vec<String>,
    redo: Vec<String>,
    checkpoint: String,
    /// The text as last opened or saved.
    clean: String,
    unsettled: bool,
    /// Bumped on every change to `doc`, so the panels know when to rebuild the tree.
    pub generation: u64,
    pub reference: Reference,
}

impl Default for LangEditor {
    /// Opens onto the shipped `en-US`, as the prop editor opens onto its example.
    fn default() -> Self {
        let mut editor = Self::untitled();
        editor.reload_reference();
        let path = Path::new(LANG_DIR).join(format!("{FALLBACK_LANG}.{FILE_EXTENSION}"));
        match load(&path) {
            Ok(doc) => editor.open(doc, Some(path)),
            Err(e) => warn!("The lang editor opens untitled: {e}"),
        }
        editor
    }
}

impl LangEditor {
    pub fn untitled() -> Self {
        Self {
            doc: LangDoc::default(),
            path: None,
            undo: Vec::new(),
            redo: Vec::new(),
            checkpoint: String::new(),
            clean: String::new(),
            unsettled: false,
            generation: 0,
            reference: Reference::default(),
        }
    }

    pub fn reload_reference(&mut self) {
        self.reference = Reference::from_table(&merged_layer(FALLBACK_LANG, &default_packs()));
        self.generation += 1;
    }

    pub fn code(&self) -> Option<String> {
        code_of(self.path.as_deref()?)
    }

    /// `None` when the open file is an `en-US` itself. An untitled file counts as a translation.
    pub fn reference(&self) -> Option<&Reference> {
        let is_reference = self.code().as_deref() == Some(FALLBACK_LANG);
        (!is_reference && !self.reference.is_empty()).then_some(&self.reference)
    }

    pub fn dirty(&self) -> bool {
        self.clean != self.checkpoint
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.unsettled
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Every change to the document goes through here.
    pub fn edit<R>(&mut self, change: impl FnOnce(&mut LangDoc) -> R) -> R {
        let result = change(&mut self.doc);
        self.unsettled = true;
        self.generation += 1;
        result
    }

    /// One undo step per stretch of activity: a typed value is one step, not one per letter.
    pub fn checkpoint_when_idle(&mut self, idle: bool) {
        if !idle || !self.unsettled {
            return;
        }
        self.unsettled = false;
        let text = self.doc.text();
        if text == self.checkpoint {
            return;
        }
        let before = std::mem::replace(&mut self.checkpoint, text);
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self) {
        // An edit not yet checkpointed is the step being undone.
        self.checkpoint_when_idle(true);
        let Some(previous) = self.undo.pop() else { return };
        self.redo.push(self.checkpoint.clone());
        self.show(&previous);
    }

    pub fn redo(&mut self) {
        let Some(next) = self.redo.pop() else { return };
        self.undo.push(self.checkpoint.clone());
        self.show(&next);
    }

    /// The snapshots are text this editor wrote, so they parse.
    fn show(&mut self, text: &str) {
        self.doc = LangDoc::parse(text).unwrap_or_default();
        self.checkpoint = text.to_string();
        self.unsettled = false;
        self.generation += 1;
    }

    /// A different document: history does not carry over.
    pub fn open(&mut self, doc: LangDoc, path: Option<PathBuf>) {
        self.undo.clear();
        self.redo.clear();
        self.path = path;
        self.doc = doc;
        self.checkpoint = self.doc.text();
        self.clean = self.checkpoint.clone();
        self.unsettled = false;
        self.generation += 1;
    }

    /// Back to what is on disk, as one undo step.
    pub fn revert_to(&mut self, saved: LangDoc) {
        self.checkpoint_when_idle(true);
        self.undo.push(self.checkpoint.clone());
        self.redo.clear();
        let text = saved.text();
        self.show(&text);
        self.clean = text;
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.checkpoint_when_idle(true);
        self.clean = self.checkpoint.clone();
        self.path = Some(path);
    }

    pub fn mark_deleted(&mut self) {
        self.path = None;
        self.clean.clear();
    }
}

pub fn code_of(path: &Path) -> Option<String> {
    path.file_stem().map(|stem| stem.to_string_lossy().into_owned())
}

pub fn load(path: &Path) -> Result<LangDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    LangDoc::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Authoring side, so `std::fs`.
pub fn save(path: &Path, doc: &LangDoc) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, doc.text()).map_err(|e| format!("{}: {e}", path.display()))?;
    remerge(&default_packs());
    Ok(())
}

/// The default pack first.
pub fn scan(packs: &[PathBuf]) -> Vec<LangFile> {
    packs.iter().rev().flat_map(|pack| lang_files(pack)).collect()
}

/// `--lang-editor`. A resource rather than an initial state, so entering goes through
/// `OnEnter(AppMode::Lang)`.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StartInLangEditor(pub bool);

fn start_in_the_lang_editor(
    start: Res<StartInLangEditor>,
    mut next: ResMut<NextState<AppMode>>,
    mut done: Local<bool>,
) {
    if !start.0 || *done {
        return;
    }
    info!("Opening the lang editor, as asked");
    next.set(AppMode::Lang);
    *done = true;
}

pub struct LangEditorPlugin;

impl Plugin for LangEditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LangEditor>()
            .init_resource::<StartInLangEditor>()
            // Ungated: it has to run in whatever mode the process started in.
            .add_systems(Update, start_in_the_lang_editor)
            .add_plugins(ui::LangUiPlugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> LangEditor {
        let mut editor = LangEditor::untitled();
        editor.open(LangDoc::parse("[a]\nx = \"X\"\n").unwrap(), None);
        editor
    }

    fn key(text: &str) -> Vec<String> {
        doc::parse_path(text).unwrap()
    }

    #[test]
    fn an_edit_is_one_undo_step_once_idle() {
        let mut editor = editor();
        let fresh = editor.doc.text();
        editor.edit(|doc| doc.set(&key("a.x"), "Y")).unwrap();
        editor.edit(|doc| doc.set(&key("a.x"), "YZ")).unwrap();
        editor.checkpoint_when_idle(false);
        assert!(editor.undo.is_empty(), "checkpointed while busy");
        editor.checkpoint_when_idle(true);
        assert_eq!(editor.undo.len(), 1, "two edits in one stretch are one step");

        editor.undo();
        assert_eq!(editor.doc.text(), fresh);
        editor.redo();
        assert_eq!(editor.doc.leaf(&key("a.x")), Some(doc::Leaf::Text("YZ".into())));
    }

    #[test]
    fn undo_takes_an_edit_that_was_never_checkpointed() {
        let mut editor = editor();
        let fresh = editor.doc.text();
        editor.edit(|doc| doc.remove(&key("a.x")));
        assert!(editor.can_undo());
        editor.undo();
        assert_eq!(editor.doc.text(), fresh);
    }

    #[test]
    fn saving_clears_dirty_and_reverting_is_undoable() {
        let mut editor = editor();
        editor.edit(|doc| doc.insert(&key("a.y"), "Y")).unwrap();
        editor.checkpoint_when_idle(true);
        assert!(editor.dirty());
        editor.mark_saved(PathBuf::from("assets/user/lang/xx-XX.toml"));
        assert!(!editor.dirty());
        let saved = editor.doc.text();

        editor.edit(|doc| doc.set(&key("a.y"), "Z")).unwrap();
        editor.revert_to(LangDoc::parse(&saved).unwrap());
        assert!(!editor.dirty());
        assert_eq!(editor.doc.text(), saved);
        editor.undo();
        assert_eq!(editor.doc.leaf(&key("a.y")), Some(doc::Leaf::Text("Z".into())));
    }

    #[test]
    fn an_en_us_file_is_not_checked_against_itself() {
        let mut editor = editor();
        editor.reference = Reference::from_table(&toml::from_str("[a]\nx = \"X\"").unwrap());
        assert!(editor.reference().is_some());
        editor.path = Some(PathBuf::from("assets/user/lang/en-US.toml"));
        assert!(editor.reference().is_none());
    }

    #[test]
    fn the_browser_lists_the_shipped_files() {
        if !Path::new(LANG_DIR).is_dir() {
            return;
        }
        let files = scan(&default_packs());
        let codes: Vec<&str> = files.iter().filter(|file| file.pack == Path::new("assets/default"))
            .map(|file| file.code.as_str()).collect();
        assert_eq!(codes, ["de-DE", "en-US"]);
        assert_eq!(files.iter().find(|file| file.code == "de-DE").unwrap().name.as_deref(), Some("Deutsch"));
    }
}
