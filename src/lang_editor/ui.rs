//! The menu bar, the browser of every pack's lang files, the key tree, and the dialogs and chords
//! around them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, Id, LayerId, RichText, Ui, UiBuilder};
use bevy_egui::EguiContexts;

use super::doc::{dotted, parse_path, KeyPath, Leaf, Marks, Node};
use super::{code_of, load, save, scan, LangEditor, FILE_EXTENSION, LANG_SUBDIR};
use crate::common::lang::LangFile;
use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::edit_menu::{edit_menu, EditAction, EditAvailability};
use crate::common::file_menu::{dialog_directory, file_menu, FileAction};
use crate::common::lang::{default_packs, FALLBACK_LANG};
use crate::common::net::NetRole;
use crate::common::shortcuts::chords;
use crate::get;

const MISSING: Color32 = Color32::from_rgb(230, 90, 80);
const STUB: Color32 = Color32::from_rgb(230, 170, 60);
const PLACEHOLDERS: Color32 = Color32::from_rgb(240, 130, 40);
const EXTRA: Color32 = Color32::from_rgb(120, 160, 230);

const KEY_WIDTH: f32 = 220.0;

/// What a file dialog came back with. On a background thread, like the other editors', so not
/// wasm-ready.
enum DialogResult {
    Save(PathBuf),
    Open(PathBuf),
}

#[derive(Resource, Clone, Default)]
pub struct LangFileDialog {
    result: Arc<Mutex<Option<DialogResult>>>,
}

#[derive(Debug, Clone)]
enum Dialog {
    AddKey { under: KeyPath, key: String, value: String, error: Option<String> },
    Rename { path: KeyPath, name: String, error: Option<String> },
    Duplicate { from: PathBuf, code: String, pack: usize, error: Option<String> },
    Delete { path: PathBuf },
    /// Opening another file over unsaved edits.
    Discard { open: PathBuf },
}

/// The tree and each leaf's marks, rebuilt when the document's generation moves.
#[derive(Default)]
struct Tree {
    generation: Option<u64>,
    root: Node,
    marks: HashMap<KeyPath, Marks>,
    counts: [usize; 4],
    /// Marked leaves at or under each node.
    problems: HashMap<KeyPath, usize>,
    shown: Shown,
}

/// What a filter lets through, kept until the document, the filter or the toggle changes.
#[derive(Default)]
struct Shown {
    key: Option<(u64, String, bool)>,
    leaves: HashSet<KeyPath>,
    nodes: HashSet<KeyPath>,
}

impl Tree {
    fn refresh(&mut self, editor: &LangEditor) {
        if self.generation == Some(editor.generation) {
            return;
        }
        self.generation = Some(editor.generation);
        let reference = editor.reference();
        self.root = Node::build(&editor.doc, reference);
        self.marks.clear();
        self.counts = [0; 4];
        let mut leaves = Vec::new();
        self.root.leaves(&mut leaves);
        for node in leaves {
            let value = editor.doc.leaf(&node.path);
            let marks = Marks::of(value.as_ref().map(Leaf::as_text), reference, &node.path);
            for (count, mark) in self.counts.iter_mut().zip([marks.missing, marks.stub, marks.placeholders, marks.extra]) {
                *count += mark as usize;
            }
            self.marks.insert(node.path.clone(), marks);
        }
        self.problems.clear();
        count_problems(&self.root, &self.marks, &mut self.problems);
    }

    /// `filter` is lowercased.
    fn filter(&mut self, editor: &LangEditor, filter: &str, problems_only: bool) {
        let key = (editor.generation, filter.to_string(), problems_only);
        if self.shown.key.as_ref() == Some(&key) {
            return;
        }
        let mut shown = Shown { key: Some(key), ..Default::default() };
        let reference = editor.reference();
        let matches = |node: &Node| {
            if problems_only && !self.marks.get(&node.path).is_some_and(|marks| marks.any()) {
                return false;
            }
            if filter.is_empty() {
                return true;
            }
            let value = editor.doc.leaf(&node.path);
            let wanted = reference.and_then(|reference| reference.get(&node.path));
            [Some(dotted(&node.path).as_str()), value.as_ref().map(Leaf::as_text), wanted]
                .into_iter()
                .flatten()
                .any(|text| text.to_lowercase().contains(filter))
        };
        fn walk(node: &Node, matches: &dyn Fn(&Node) -> bool, shown: &mut Shown) -> bool {
            let leaf = node.leaf && matches(node);
            if leaf {
                shown.leaves.insert(node.path.clone());
            }
            let mut any = leaf;
            for child in &node.children {
                any |= walk(child, matches, shown);
            }
            if any {
                shown.nodes.insert(node.path.clone());
            }
            any
        }
        walk(&self.root, &matches, &mut shown);
        self.shown = shown;
    }
}

fn count_problems(node: &Node, marks: &HashMap<KeyPath, Marks>, into: &mut HashMap<KeyPath, usize>) -> usize {
    let own = (node.leaf && marks.get(&node.path).is_some_and(|marks| marks.any())) as usize;
    let total = own + node.children.iter().map(|child| count_problems(child, marks, into)).sum::<usize>();
    into.insert(node.path.clone(), total);
    total
}

#[derive(Resource, Default)]
struct LangUi {
    files: Option<Vec<LangFile>>,
    dialog: Option<Dialog>,
    filter: String,
    problems_only: bool,
    tree: Tree,
}

impl LangUi {
    fn files(&mut self) -> &[LangFile] {
        self.files.get_or_insert_with(|| scan(&default_packs()))
    }

    fn rescan(&mut self) {
        self.files = None;
    }
}

#[derive(Default)]
struct Requests {
    file: Option<FileAction>,
    edit: Option<EditAction>,
    mode: Option<AppMode>,
    open: Option<PathBuf>,
}

pub struct LangUiPlugin;

impl Plugin for LangUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LangFileDialog>()
            .init_resource::<LangUi>()
            .add_systems(OnEnter(AppMode::Lang), |mut state: ResMut<LangUi>| state.rescan())
            .add_systems(bevy_egui::EguiPrimaryContextPass, panels.run_if(in_state(AppMode::Lang)))
            .add_systems(Update, lang_shortcuts.run_if(in_state(AppMode::Lang)));
    }
}

fn edit_availability(editor: &LangEditor) -> EditAvailability {
    EditAvailability {
        undo: editor.can_undo(),
        redo: editor.can_redo(),
        revert: editor.path.is_some() && editor.dirty(),
    }
}

fn panels(
    mut contexts: EguiContexts,
    mut editor: ResMut<LangEditor>,
    mut state: ResMut<LangUi>,
    dialog: Res<LangFileDialog>,
    mut next_mode: ResMut<NextState<AppMode>>,
    role: Option<Res<NetRole>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();
    let mut window_ui = Ui::new(
        ctx.clone(),
        Id::new("grackle_lang_window"),
        UiBuilder::new().layer_id(LayerId::background()).max_rect(ctx.viewport_rect()),
    );

    let mut requests = Requests::default();
    egui::Panel::top("lang_menu_bar").resizable(false).show(&mut window_ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            file_menu(ui, false, &mut requests.file);
            edit_menu(ui, edit_availability(&editor), &mut requests.edit);
            mode_menu(ui, AppMode::Lang, role.as_deref(), &mut requests.mode);
            ui.separator();
            ui.label(match &editor.path {
                Some(path) => path.display().to_string(),
                None => get!("lang_editor.untitled"),
            });
            if editor.dirty() {
                ui.label("*");
            }
        });
    });

    egui::Panel::left("lang_browser").resizable(true).default_size(240.0).show(&mut window_ui, |ui| {
        browser(ui, &editor, &mut state, &mut requests);
    });

    egui::CentralPanel::default().show(&mut window_ui, |ui| {
        state.tree.refresh(&editor);
        keys_panel(ui, &mut editor, &mut state);
    });

    dialogs(&ctx, &mut editor, &mut state, &mut requests);

    editor.checkpoint_when_idle(!ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input());

    handle_edit(requests.edit, &mut editor);
    if let Some(mode) = requests.mode {
        next_mode.set(mode);
    }
    // The file already open is not reloaded: that would throw its edits away.
    if let Some(path) = requests.open.filter(|path| editor.path.as_ref() != Some(path)) {
        match editor.dirty() {
            true => state.dialog = Some(Dialog::Discard { open: path }),
            false => open_file(&mut editor, path),
        }
    }
    handle_files(requests.file, &mut editor, &mut state, &dialog);
    if requests.file == Some(FileAction::Quit) {
        info!("Quitting, as asked");
        exit.write(AppExit::Success);
    }
}

fn browser(ui: &mut Ui, editor: &LangEditor, state: &mut LangUi, requests: &mut Requests) {
    let open = editor.path.clone();
    let files = state.files().to_vec();
    let mut dialog = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        let mut pack: Option<&Path> = None;
        for file in &files {
            if pack != Some(file.pack.as_path()) {
                pack = Some(file.pack.as_path());
                ui.add_space(4.0);
                ui.label(RichText::new(file.pack.display().to_string()).strong());
            }
            let label = match &file.name {
                Some(name) => format!("{}  {}", file.code, name),
                None => file.code.clone(),
            };
            let row = ui.selectable_label(open.as_ref() == Some(&file.path), label);
            if row.clicked() {
                requests.open = Some(file.path.clone());
            }
            row.context_menu(|ui| {
                if ui.button(get!("lang_editor.browser.open")).clicked() {
                    requests.open = Some(file.path.clone());
                }
                if ui.button(get!("lang_editor.browser.duplicate")).clicked() {
                    dialog = Some(duplicate_dialog(&file.path));
                }
                if ui.button(get!("lang_editor.browser.delete")).clicked() {
                    dialog = Some(Dialog::Delete { path: file.path.clone() });
                }
            });
        }
        if files.is_empty() {
            ui.weak(get!("lang_editor.browser.empty"));
        }
    });
    if dialog.is_some() {
        state.dialog = dialog;
    }
}

/// Into the source's own pack, so a copy of a shipped file ships beside it.
fn duplicate_dialog(from: &Path) -> Dialog {
    let packs = default_packs();
    let pack = packs
        .iter()
        .position(|pack| from.starts_with(pack))
        .unwrap_or_else(|| packs.iter().position(|pack| pack == Path::new(crate::common::assets::USER_PACK)).unwrap_or(0));
    Dialog::Duplicate { from: from.to_path_buf(), code: String::new(), pack, error: None }
}

fn keys_panel(ui: &mut Ui, editor: &mut LangEditor, state: &mut LangUi) {
    let comparing = editor.reference().is_some();
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut state.filter).hint_text(get!("lang_editor.filter")).desired_width(200.0));
        ui.checkbox(&mut state.problems_only, get!("lang_editor.problems_only"));
        if ui.button(get!("lang_editor.add_key")).clicked() {
            state.dialog = Some(Dialog::AddKey { under: Vec::new(), key: String::new(), value: String::new(), error: None });
        }
        if let Some(path) = &editor.path {
            if ui.button(get!("lang_editor.browser.duplicate")).clicked() {
                state.dialog = Some(duplicate_dialog(path));
            }
        }
        ui.separator();
        let [missing, stubs, placeholders, extra] = state.tree.counts;
        if comparing {
            ui.colored_label(MISSING, get!("lang_editor.count.missing", "count", missing));
        }
        ui.colored_label(STUB, get!("lang_editor.count.stub", "count", stubs));
        if comparing {
            ui.colored_label(PLACEHOLDERS, get!("lang_editor.count.placeholders", "count", placeholders));
            ui.colored_label(EXTRA, get!("lang_editor.count.extra", "count", extra));
        }
    });
    ui.separator();

    let filter = state.filter.trim().to_lowercase();
    let filtering = !filter.is_empty() || state.problems_only;
    if filtering {
        state.tree.filter(editor, &filter, state.problems_only);
    }
    let tree = &state.tree;
    let mut view = View { editor, tree, filtering, comparing, dialog: None };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if tree.root.children.is_empty() {
            ui.weak(get!("lang_editor.no_keys"));
        }
        view.children(ui, &tree.root);
    });
    if let Some(dialog) = view.dialog {
        state.dialog = Some(dialog);
    }
}

struct View<'a> {
    editor: &'a mut LangEditor,
    tree: &'a Tree,
    filtering: bool,
    comparing: bool,
    dialog: Option<Dialog>,
}

impl View<'_> {
    fn shows_leaf(&self, node: &Node) -> bool {
        !self.filtering || self.tree.shown.leaves.contains(&node.path)
    }

    fn shows(&self, node: &Node) -> bool {
        !self.filtering || self.tree.shown.nodes.contains(&node.path)
    }

    fn children(&mut self, ui: &mut Ui, node: &Node) {
        for child in &node.children {
            if !self.shows(child) {
                continue;
            }
            if child.leaf && self.shows_leaf(child) {
                self.leaf(ui, child);
            }
            if !child.children.is_empty() {
                self.table(ui, child);
            }
        }
    }

    fn table(&mut self, ui: &mut Ui, node: &Node) {
        let problems = self.tree.problems.get(&node.path).copied().unwrap_or(0);
        let mut title = RichText::new(&node.name).monospace();
        if problems > 0 {
            title = RichText::new(format!("{}  ({problems})", node.name)).monospace().color(STUB);
        }
        let header = egui::CollapsingHeader::new(title)
            .id_salt(("lang_table", dotted(&node.path)))
            .open(self.filtering.then_some(true))
            .show(ui, |ui| self.children(ui, node));
        let in_file = self.editor.doc.contains(&node.path);
        header.header_response.context_menu(|ui| {
            if ui.button(get!("lang_editor.add_key")).clicked() {
                self.dialog = Some(Dialog::AddKey { under: node.path.clone(), key: String::new(), value: String::new(), error: None });
            }
            if ui.add_enabled(in_file, egui::Button::new(get!("lang_editor.rename"))).clicked() {
                self.dialog = Some(Dialog::Rename { path: node.path.clone(), name: node.name.clone(), error: None });
            }
            if ui.add_enabled(in_file, egui::Button::new(get!("lang_editor.remove"))).clicked() {
                self.editor.edit(|doc| doc.remove(&node.path));
            }
        });
    }

    fn leaf(&mut self, ui: &mut Ui, node: &Node) {
        let marks = self.tree.marks.get(&node.path).copied().unwrap_or_default();
        let value = self.editor.doc.leaf(&node.path);
        let reference = self.editor.reference().and_then(|r| r.get(&node.path)).map(str::to_string);
        ui.horizontal(|ui| {
            let color = if marks.missing { MISSING } else if marks.stub { STUB } else if marks.placeholders { PLACEHOLDERS } else if marks.extra { EXTRA } else { ui.visuals().text_color() };
            let name = ui.add_sized(
                [KEY_WIDTH, ui.spacing().interact_size.y],
                egui::Label::new(RichText::new(&node.name).monospace().color(color)).truncate().sense(egui::Sense::click()),
            ).on_hover_text(dotted(&node.path));
            name.context_menu(|ui| {
                if ui.add_enabled(value.is_some(), egui::Button::new(get!("lang_editor.rename"))).clicked() {
                    self.dialog = Some(Dialog::Rename { path: node.path.clone(), name: node.name.clone(), error: None });
                }
                if ui.add_enabled(value.is_some(), egui::Button::new(get!("lang_editor.remove"))).clicked() {
                    self.editor.edit(|doc| doc.remove(&node.path));
                }
            });

            let width = match self.comparing {
                true => (ui.available_width() - 110.0) / 2.0,
                false => ui.available_width() - 110.0,
            }
            .max(120.0);
            match &value {
                Some(Leaf::Text(text)) => {
                    let mut text = text.clone();
                    let edit = match text.contains('\n') {
                        true => egui::TextEdit::multiline(&mut text),
                        false => egui::TextEdit::singleline(&mut text),
                    };
                    if ui.add(edit.desired_width(width).id_salt(("lang_value", dotted(&node.path)))).changed() {
                        let _ = self.editor.edit(|doc| doc.set(&node.path, &text));
                    }
                }
                Some(Leaf::Other(raw)) => {
                    ui.add_sized([width, ui.spacing().interact_size.y], egui::Label::new(RichText::new(raw).monospace()));
                }
                None => {
                    let copy = ui.add_sized([width, ui.spacing().interact_size.y], egui::Button::new(get!("lang_editor.copy_reference")));
                    if copy.clicked() {
                        if let Some(reference) = &reference {
                            let _ = self.editor.edit(|doc| doc.insert(&node.path, reference));
                        }
                    }
                }
            }
            if self.comparing {
                ui.add_sized(
                    [width, ui.spacing().interact_size.y],
                    egui::Label::new(RichText::new(reference.clone().unwrap_or_default()).weak()).truncate(),
                );
            }
            tags(ui, marks, value.as_ref().map(Leaf::as_text), reference.as_deref());
        });
    }
}

fn tags(ui: &mut Ui, marks: Marks, value: Option<&str>, reference: Option<&str>) {
    if marks.missing {
        ui.colored_label(MISSING, get!("lang_editor.mark.missing"));
    }
    if marks.stub {
        ui.colored_label(STUB, get!("lang_editor.mark.stub"));
    }
    if marks.placeholders {
        let slots = |text: Option<&str>| {
            let found: Vec<String> = text.map(super::doc::placeholders).unwrap_or_default().into_iter().collect();
            found.join(", ")
        };
        ui.colored_label(PLACEHOLDERS, get!("lang_editor.mark.placeholders")).on_hover_text(get!(
            "lang_editor.mark.placeholders_detail",
            "reference",
            slots(reference),
            "here",
            slots(value)
        ));
    }
    if marks.extra {
        ui.colored_label(EXTRA, get!("lang_editor.mark.extra"));
    }
}

fn dialogs(ctx: &egui::Context, editor: &mut LangEditor, state: &mut LangUi, requests: &mut Requests) {
    let Some(mut dialog) = state.dialog.take() else { return };
    let mut done = false;
    let modal = egui::Modal::new(Id::new("lang_editor_dialog")).show(ctx, |ui| {
        ui.set_min_width(320.0);
        match &mut dialog {
            Dialog::AddKey { under, key, value, error } => {
                ui.heading(get!("lang_editor.add_key"));
                if !under.is_empty() {
                    ui.monospace(format!("{}.", dotted(under)));
                }
                ui.add(egui::TextEdit::singleline(key).hint_text(get!("lang_editor.dialog.key")));
                let value_field = ui.add(egui::TextEdit::singleline(value).hint_text(get!("lang_editor.dialog.value")));
                show_error(ui, error);
                // Enter in the key field would add the key before its value is typed.
                let enter = value_field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if confirm_row(ui, enter, &mut done) {
                    match parse_path(key) {
                        Some(relative) => {
                            let mut path = under.clone();
                            path.extend(relative);
                            match editor.edit(|doc| doc.insert(&path, value)) {
                                Ok(()) => done = true,
                                Err(e) => *error = Some(e.message()),
                            }
                        }
                        None => *error = Some(super::doc::LangError::BadKey.message()),
                    }
                }
            }
            Dialog::Rename { path, name, error } => {
                ui.heading(get!("lang_editor.rename"));
                ui.monospace(dotted(path));
                ui.text_edit_singleline(name);
                show_error(ui, error);
                if confirm_row(ui, ui.input(|input| input.key_pressed(egui::Key::Enter)), &mut done) {
                    match editor.edit(|doc| doc.rename(path, name.trim())) {
                        Ok(()) => done = true,
                        Err(e) => *error = Some(e.message()),
                    }
                }
            }
            Dialog::Duplicate { from, code, pack, error } => {
                ui.heading(get!("lang_editor.browser.duplicate"));
                ui.monospace(from.display().to_string());
                ui.add(egui::TextEdit::singleline(code).hint_text(get!("lang_editor.dialog.code")));
                let packs = default_packs();
                egui::ComboBox::from_label(get!("lang_editor.dialog.pack"))
                    .selected_text(packs.get(*pack).map(|p| p.display().to_string()).unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for (index, candidate) in packs.iter().enumerate() {
                            ui.selectable_value(pack, index, candidate.display().to_string());
                        }
                    });
                show_error(ui, error);
                if confirm_row(ui, ui.input(|input| input.key_pressed(egui::Key::Enter)), &mut done) {
                    match duplicate(editor, from, code.trim(), &packs[*pack]) {
                        Ok(path) => {
                            state.rescan();
                            if code_of(&path).as_deref() == Some(FALLBACK_LANG) {
                                editor.reload_reference();
                            }
                            // A copy of the open file carries its unsaved edits, so nothing is lost.
                            match editor.path.as_deref() == Some(from.as_path()) {
                                true => open_file(editor, path),
                                false => requests.open = Some(path),
                            }
                            done = true;
                        }
                        Err(e) => *error = Some(e),
                    }
                }
            }
            Dialog::Delete { path } => {
                ui.heading(get!("lang_editor.browser.delete"));
                ui.label(get!("lang_editor.dialog.delete", "file", path.display()));
                if confirm_row(ui, ui.input(|input| input.key_pressed(egui::Key::Enter)), &mut done) {
                    match std::fs::remove_file(&*path) {
                        Ok(()) => {
                            info!("Deleted lang file {path:?}");
                            if editor.path.as_ref() == Some(path) {
                                editor.mark_deleted();
                            }
                            crate::common::lang::remerge(&default_packs());
                            if code_of(path).as_deref() == Some(FALLBACK_LANG) {
                                editor.reload_reference();
                            }
                            state.rescan();
                        }
                        Err(e) => error!("Deleting {path:?}: {e}"),
                    }
                    done = true;
                }
            }
            Dialog::Discard { open } => {
                ui.heading(get!("lang_editor.dialog.unsaved"));
                ui.label(get!("lang_editor.dialog.discard", "file", open.display()));
                if confirm_row(ui, ui.input(|input| input.key_pressed(egui::Key::Enter)), &mut done) {
                    open_file(editor, open.clone());
                    done = true;
                }
            }
        }
    });
    if modal.should_close() {
        done = true;
    }
    if !done {
        state.dialog = Some(dialog);
    }
}

fn show_error(ui: &mut Ui, error: &Option<String>) {
    if let Some(error) = error {
        ui.colored_label(MISSING, error);
    }
}

/// OK or `enter` is `true`; Cancel sets `done`.
fn confirm_row(ui: &mut Ui, enter: bool, done: &mut bool) -> bool {
    let mut confirmed = enter;
    ui.horizontal(|ui| {
        confirmed |= ui.button(get!("lang_editor.dialog.ok")).clicked();
        if ui.button(get!("lang_editor.dialog.cancel")).clicked() {
            *done = true;
        }
    });
    confirmed && !*done
}

/// The open document's text if it is the source, so unsaved edits come along.
fn duplicate(editor: &LangEditor, from: &Path, code: &str, pack: &Path) -> Result<PathBuf, String> {
    if !super::doc::valid_segment(code) {
        return Err(get!("lang_editor.error.bad_code"));
    }
    let to = pack.join(LANG_SUBDIR).join(format!("{code}.{FILE_EXTENSION}"));
    if to.exists() {
        return Err(get!("lang_editor.error.file_exists", "file", to.display()));
    }
    let text = match editor.path.as_deref() == Some(from) {
        true => editor.doc.text(),
        false => std::fs::read_to_string(from).map_err(|e| format!("{}: {e}", from.display()))?,
    };
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&to, text).map_err(|e| format!("{}: {e}", to.display()))?;
    info!("Duplicated {from:?} to {to:?}");
    crate::common::lang::remerge(&default_packs());
    Ok(to)
}

fn open_file(editor: &mut LangEditor, path: PathBuf) {
    match load(&path) {
        Ok(doc) => {
            info!("Opened lang file {path:?}");
            editor.open(doc, Some(path));
        }
        Err(e) => error!("Opening the lang file: {e}"),
    }
}

/// Chords go through the menus' handlers, so a chord and its menu entry are one action.
fn lang_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    egui: Res<bevy_egui::input::EguiWantsInput>,
    dialog: Res<LangFileDialog>,
    mut editor: ResMut<LangEditor>,
    mut state: ResMut<LangUi>,
) {
    let chords = chords(&keys, egui.wants_keyboard_input());
    handle_edit(
        match (chords.redo, chords.undo) {
            (true, _) => Some(EditAction::Redo),
            (false, true) => Some(EditAction::Undo),
            _ => None,
        },
        &mut editor,
    );
    if let Some(action) = FileAction::from_chords(chords) {
        handle_files(Some(action), &mut editor, &mut state, &dialog);
    }
}

fn handle_edit(action: Option<EditAction>, editor: &mut LangEditor) {
    match action {
        Some(EditAction::Undo) => editor.undo(),
        Some(EditAction::Redo) => editor.redo(),
        Some(EditAction::Revert) => {
            let Some(path) = editor.path.clone() else { return };
            match load(&path) {
                Ok(saved) => {
                    info!("Reverted the lang file to {path:?}");
                    editor.revert_to(saved);
                }
                Err(e) => error!("Reverting the lang file: {e}"),
            }
        }
        None => {}
    }
}

/// New, open and save. Quit is the caller's. Called every frame, to collect a dialog's answer.
fn handle_files(action: Option<FileAction>, editor: &mut LangEditor, state: &mut LangUi, dialog: &LangFileDialog) {
    if action == Some(FileAction::New) {
        editor.open(Default::default(), None);
    }

    if let Some(result) = dialog.result.lock().unwrap().take() {
        match result {
            DialogResult::Save(path) => save_to(editor, state, path),
            DialogResult::Open(path) => open_file(editor, path),
        }
    }

    match action {
        Some(FileAction::Save) => match editor.path.clone() {
            Some(path) => save_to(editor, state, path),
            None => ask_where_to_save(editor, dialog),
        },
        Some(FileAction::SaveAs) => ask_where_to_save(editor, dialog),
        Some(FileAction::Open) => {
            let slot = dialog.result.clone();
            let from = dialog_directory(editor.path.as_deref(), LANG_SUBDIR);
            std::thread::spawn(move || {
                let mut chooser = rfd::AsyncFileDialog::new().add_filter("TOML", &[FILE_EXTENSION]);
                if let Some(from) = from {
                    chooser = chooser.set_directory(from);
                }
                if let Some(handle) = pollster::block_on(chooser.pick_file()) {
                    *slot.lock().unwrap() = Some(DialogResult::Open(handle.path().to_path_buf()));
                }
            });
        }
        _ => {}
    }
}

fn save_to(editor: &mut LangEditor, state: &mut LangUi, path: PathBuf) {
    match save(&path, &editor.doc) {
        Ok(()) => {
            info!("Saved lang file to {path:?}");
            editor.mark_saved(path.clone());
            if code_of(&path).as_deref() == Some(FALLBACK_LANG) {
                editor.reload_reference();
            }
            state.rescan();
        }
        Err(e) => error!("Saving the lang file: {e}"),
    }
}

fn ask_where_to_save(editor: &LangEditor, dialog: &LangFileDialog) {
    let slot = dialog.result.clone();
    let from = dialog_directory(editor.path.as_deref(), LANG_SUBDIR);
    let name = editor.code().unwrap_or_else(|| "untitled".to_string());
    std::thread::spawn(move || {
        let mut chooser = rfd::AsyncFileDialog::new()
            .add_filter("TOML", &[FILE_EXTENSION])
            .set_file_name(format!("{name}.{FILE_EXTENSION}"));
        if let Some(from) = from {
            chooser = chooser.set_directory(from);
        }
        if let Some(handle) = pollster::block_on(chooser.save_file()) {
            *slot.lock().unwrap() = Some(DialogResult::Save(handle.path().to_path_buf()));
        }
    });
}
