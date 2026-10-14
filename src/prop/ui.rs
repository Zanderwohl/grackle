//! The prop editor's panels.
//!
//! Its own `DockState` rather than tabs on
//! [`EditorPanels`](crate::editor::panels::EditorPanels), which is already at
//! Bevy's parameter limit — and the two surfaces are modes, never up at once,
//! so a shared dock would need its tab set swapped anyway. What *is* shared is
//! the shape: the same panels around the same four viewports, sized the same
//! way, so the window does not rearrange itself under somebody swapping modes.
//!
//! Writes go through [`PropEditor::begin_gesture`] and
//! [`PropEditor::end_gesture`] rather than `edit`: egui reports a drag as a
//! change every frame, so per-change undo steps would turn one pull on a radius
//! into forty presses of Ctrl+Z. The gesture closes when the pointer *and* the
//! keyboard are idle, which makes typing a name one step rather than one per
//! letter.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::egui::{Id, LayerId, Ui, UiBuilder, UiKind, WidgetText};
use bevy_egui::{egui, EguiContexts};
use egui_dock::{DockArea, DockState, TabViewer};
use strum::IntoEnumIterator;

use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::edit_menu::{edit_menu, EditAction, EditAvailability};
use crate::common::file_menu::{file_menu, FileAction};
use crate::common::net::NetRole;
use crate::editor::multicam::{Insets, MulticamState};
use crate::get;
use crate::prop::document::{self, PropDoc, PropEditor, PROP_EXTENSION};
use crate::common::class::Class;
use crate::common::rotation::{aim_and_spin, euler_from_quat, from_aim_and_spin};
use crate::common::shortcuts::chords;
use crate::prop::feature::{Axis, BooleanOp, FeatureOp, PatternKind, PropFeatureId};
use crate::prop::figure::ScaleFigure;
use crate::prop::camera::ViewmodelPreview;
use crate::prop::gizmo::HoldHandles;
use crate::prop::hold::HoldOverride;
use crate::prop::profile::{Placement, Profile, MIN_SIDES};
use crate::prop::solid::Shape;
use crate::prop::surface::{Style, Surface};
use crate::prop::camera::FrameTheProp;
use crate::prop::view::PropBuild;

/// What a file dialog came back with. Mirrors the map editor's arrangement,
/// including the background thread and its wasm problem. The *reading* half of
/// the prop format deliberately does not come through here.
enum DialogResult {
    Save(PathBuf),
    Open(PathBuf),
}

#[derive(Resource, Clone)]
pub struct PropFileDialog {
    result: Arc<Mutex<Option<DialogResult>>>,
}

impl Default for PropFileDialog {
    fn default() -> Self {
        Self { result: Arc::new(Mutex::new(None)) }
    }
}

enum PropTab {
    Features,
    Inspector,
    Hold,
    Problems,
}

/// Which job the editor is set up for.
///
/// **Modelling a prop and fitting it to a hand are different activities**, and
/// they were one panel apart with a checkbox between them that silently
/// swapped which handles the viewport answered. A workspace makes that swap
/// the thing you chose: Model has the verbs, the inspector and the feature
/// handles; Fit has the hold editor, the hold's handles and a body standing
/// there holding the thing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Workspace {
    #[default]
    Model,
    Fit,
}

/// The panel layout.
#[derive(Resource)]
pub struct PropPanels {
    left: DockState<PropTab>,
    /// The right-hand panel per workspace: the inspector to model with, the
    /// hold editor to fit with.
    right_model: DockState<PropTab>,
    right_fit: DockState<PropTab>,
    bottom: DockState<PropTab>,
    workspace: Workspace,
    /// What the reference figure was in Model before Fit stood a body up, so
    /// coming back puts it back — usually to nobody.
    model_figure: Option<Class>,
    /// Who was standing there last time Fit was open, so a hold tuned for a
    /// Heavy is still being tuned for a Heavy after a trip to Model.
    fit_class: Option<Class>,
    menu_bar_height: f32,
    left_width: f32,
    right_width: f32,
    bottom_height: f32,
}

impl Default for PropPanels {
    fn default() -> Self {
        Self {
            left: DockState::new(vec![PropTab::Features]),
            right_model: DockState::new(vec![PropTab::Inspector]),
            right_fit: DockState::new(vec![PropTab::Hold]),
            bottom: DockState::new(vec![PropTab::Problems]),
            workspace: Workspace::Model,
            model_figure: None,
            fit_class: None,
            menu_bar_height: 0.0,
            left_width: 260.0,
            right_width: 300.0,
            bottom_height: 80.0,
        }
    }
}

impl PropPanels {
    /// Move to another workspace, standing up or taking away the figure as
    /// the move implies.
    ///
    /// **Fit always has a body**, since fitting to nobody is not fitting; the
    /// class is whoever was there last time, then whoever Model happened to be
    /// showing, then the placeholder everybody plays as. **Model gets back
    /// what it had**, so a trip to Fit does not leave somebody standing in the
    /// modelling view.
    ///
    /// Writes the figure only when it differs: `ScaleFigure` changing is what
    /// rebuilds a rig.
    fn switch_to(&mut self, to: Workspace, figure: &mut ScaleFigure) {
        if self.workspace == to {
            return;
        }
        let wanted = match to {
            Workspace::Fit => {
                self.model_figure = figure.0;
                Some(self.fit_class.or(figure.0).unwrap_or(Class::Mercenary))
            }
            Workspace::Model => {
                self.fit_class = figure.0;
                self.model_figure
            }
        };
        if figure.0 != wanted {
            figure.0 = wanted;
        }
        self.workspace = to;
    }
}

/// What the panels asked for this frame, so the systems that answer need not be
/// reachable from inside an egui closure.
#[derive(Default)]
struct PropRequests {
    /// What the File menu asked of the prop in front of us.
    file: Option<FileAction>,
    /// A framing asked for, and whether of the selection rather than the
    /// whole prop.
    frame: Option<bool>,
    /// The mode picked from the **Mode** menu, if one was.
    mode: Option<AppMode>,
    /// The workspace picked from the toolbar, if one was.
    workspace: Option<Workspace>,
    /// What the Edit menu asked of the prop.
    edit: Option<EditAction>,
}

struct PropTabs<'a> {
    editor: &'a mut PropEditor,
    build: &'a PropBuild,
    figure: &'a mut ScaleFigure,
    preview: &'a mut ViewmodelPreview,
    requests: &'a mut PropRequests,
}

impl<'a> TabViewer for PropTabs<'a> {
    type Tab = PropTab;

    fn id(&mut self, tab: &mut Self::Tab) -> Id {
        Id::new(match tab {
            PropTab::Features => "prop_features",
            PropTab::Inspector => "prop_inspector",
            PropTab::Hold => "prop_hold",
            PropTab::Problems => "prop_problems",
        })
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        match tab {
            PropTab::Features => get!("prop.panels.features").into(),
            PropTab::Inspector => get!("prop.panels.inspector").into(),
            PropTab::Hold => get!("prop.panels.hold").into(),
            // The count in the title, so problems are visible with the tab
            // folded behind another one.
            PropTab::Problems => match self.build.evaluated.problems.len() {
                0 => get!("prop.panels.problems").into(),
                count => get!("prop.panels.problems_count", "count", count).into(),
            },
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        match tab {
            PropTab::Features => feature_tree(ui, self.editor, self.build),
            PropTab::Inspector => inspector(ui, self.editor, self.build),
            PropTab::Hold => hold(ui, self.editor, *self.figure, self.preview),
            PropTab::Problems => problems(ui, self.editor, self.build),
        }
    }
}

pub struct PropUiPlugin;

impl Plugin for PropUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PropPanels>()
            .init_resource::<PropFileDialog>()
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                panels.run_if(in_state(AppMode::Prop)),
            )
            .add_systems(Update, prop_shortcuts.run_if(in_state(AppMode::Prop)))
            // The rollback bar is a way of looking at the list, which only the
            // prop editor shows. Left in, the animation editor — which builds
            // the same prop — would animate a weapon missing its last features
            // with nothing on screen saying so. Before `OnEnter` of the next
            // mode, which is where that rebuild happens.
            .add_systems(OnExit(AppMode::Prop), |mut editor: ResMut<PropEditor>| {
                editor.roll_back_to(None);
            });
    }
}

#[allow(clippy::too_many_arguments)]
fn panels(
    mut contexts: EguiContexts,
    mut layout: ResMut<PropPanels>,
    mut editor: ResMut<PropEditor>,
    build: Res<PropBuild>,
    mut figure: ResMut<ScaleFigure>,
    mut handles: ResMut<HoldHandles>,
    mut preview: ResMut<ViewmodelPreview>,
    mut multicam: ResMut<MulticamState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    dialog: Res<PropFileDialog>,
    mut frame: MessageWriter<FrameTheProp>,
    mut next_mode: ResMut<NextState<AppMode>>,
    // Optional so the prop editor does not drag in the network layer: a build
    // with no `NetPlugin` is a closed game, and a closed game is its own
    // authority.
    role: Option<Res<NetRole>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();

    let mut viewport_ui = Ui::new(
        ctx.clone(),
        Id::new("grackle_prop_viewport"),
        UiBuilder::new().layer_id(LayerId::background()).max_rect(ctx.viewport_rect()),
    );

    // Off unless this frame's Hold tab turns it back on — a tab closed
    // mid-drag would otherwise leave the view standing at the muzzle forever.
    if preview.0 {
        preview.0 = false;
    }

    let mut requests = PropRequests::default();
    let mut tabs = PropTabs {
        editor: &mut editor,
        build: &build,
        // Through the `ResMut` deref rather than cloned and written back:
        // `ScaleFigure` drives a system that rebuilds a rig when it changes,
        // and a write every frame would throw the figure's meshes away every
        // frame.
        figure: &mut figure,
        preview: &mut preview,
        requests: &mut requests,
    };
    let workspace = layout.workspace;

    layout.menu_bar_height = egui::Panel::top("prop_menu_bar")
        .resizable(false)
        .show(&mut viewport_ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                // The shared pulldown rather than four buttons written
                // here, which is what it used to be.
                file_menu(ui, &mut tabs.requests.file);
                // Just right of File in every mode, and greyed per entry.
                edit_menu(ui, edit_availability(tabs.editor), &mut tabs.requests.edit);
                view_menu(ui, workspace, tabs.figure);
                // `Prop` spelled out rather than read from the state: this
                // whole system is gated on it, so the mode it is drawing for
                // is not in question.
                mode_menu(
                    ui,
                    AppMode::Prop,
                    role.as_deref(),
                    &mut tabs.requests.mode,
                );
                ui.separator();
                ui.label(match &tabs.editor.path {
                    Some(path) => path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    None => get!("prop.untitled"),
                });
                if tabs.editor.dirty {
                    ui.label("*");
                }
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                // First, and the same in both workspaces, so the way back is
                // always where the way in was.
                for (candidate, key) in
                    [(Workspace::Model, "prop.workspace.model"), (Workspace::Fit, "prop.workspace.fit")]
                {
                    if ui.selectable_label(workspace == candidate, get!(key)).clicked() {
                        tabs.requests.workspace = Some(candidate);
                    }
                }
                ui.separator();
                match workspace {
                    Workspace::Model => toolbar(ui, tabs.editor, tabs.build, &mut tabs.requests.frame),
                    Workspace::Fit => fit_toolbar(ui, tabs.figure, &mut tabs.requests.frame),
                }
            });
        })
        .response
        .rect
        .height();

    layout.left_width = egui::Panel::left("prop_left")
        .resizable(true)
        .show(&mut viewport_ui, |ui| {
            DockArea::new(&mut layout.left)
                .id(Id::new("egui_dock::DockArea::prop_left"))
                .show_close_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .draggable_tabs(false)
                .show_inside(ui, &mut tabs);
            ui.allocate_rect(ui.available_rect_before_wrap(), egui::Sense::hover());
        })
        .response
        .rect
        .width();

    layout.right_width = egui::Panel::right("prop_right")
        .resizable(true)
        .show(&mut viewport_ui, |ui| {
            let right = match workspace {
                Workspace::Model => &mut layout.right_model,
                Workspace::Fit => &mut layout.right_fit,
            };
            DockArea::new(right)
                .id(Id::new("egui_dock::DockArea::prop_right"))
                .show_close_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .draggable_tabs(false)
                .show_inside(ui, &mut tabs);
            ui.allocate_rect(ui.available_rect_before_wrap(), egui::Sense::hover());
        })
        .response
        .rect
        .width();

    // Shown before the dock's bottom panel so it sits beneath it, along the
    // window's edge.
    let status_height = egui::Panel::bottom("prop_status")
        .resizable(false)
        .show(&mut viewport_ui, |ui| status_line(ui, tabs.editor, tabs.build))
        .response
        .rect
        .height();

    layout.bottom_height = egui::Panel::bottom("prop_bottom")
        .resizable(true)
        .show(&mut viewport_ui, |ui| {
            DockArea::new(&mut layout.bottom)
                .id(Id::new("egui_dock::DockArea::prop_bottom"))
                .show_close_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .draggable_tabs(false)
                .show_inside(ui, &mut tabs);
        })
        .response
        .rect
        .height();

    drop(tabs);

    // One undo step per gesture. The keyboard half is what makes typing a name
    // a single step — the pointer is idle the whole time somebody types.
    if !ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input() {
        editor.end_gesture();
    }

    handle_edit(requests.edit, &mut editor);

    if let Some(to) = requests.workspace {
        layout.switch_to(to, &mut figure);
    }
    // Derived from the workspace rather than ticked: in Fit the viewport's
    // handles are the hold's, in Model the selected feature's.
    let fitting = layout.workspace == Workspace::Fit;
    if handles.0 != fitting {
        handles.0 = fitting;
    }

    if let Some(selected) = requests.frame {
        frame.write(FrameTheProp { selected });
    }
    // `set`, not `set_if_neq`: `mode_menu` never offers the mode already in
    // force, so anything arriving here is a real change.
    if let Some(mode) = requests.mode {
        next_mode.set(mode);
    }
    handle_files(requests.file, &mut editor, &dialog);
    if requests.file == Some(FileAction::Quit) {
        info!("Quitting, as asked");
        exit.write(AppExit::Success);
    }

    if let Ok(window) = windows.single() {
        multicam.reserve(
            Insets {
                left: layout.left_width,
                right: layout.right_width,
                top: layout.menu_bar_height,
                bottom: layout.bottom_height + status_height,
            },
            window,
        );
    }
}

/// One line of the feature list, read out before any of it is drawn: the rows
/// write back, and holding a borrow on the editor across the closures that draw
/// them would leave the context menu with nothing it could change.
struct Row {
    id: PropFeatureId,
    name: String,
    enabled: bool,
    /// How many features name this one as an operand, so deleting it can say
    /// what that costs.
    dependants: usize,
    /// Operands and dependants together — what this may not be dragged past.
    /// Marked during a drag, so the reason the line stops is on screen.
    pins: Vec<PropFeatureId>,
    /// Where this feature may end up, as indices in the finished list.
    legal: std::ops::RangeInclusive<usize>,
    /// What this feature eats, by name, listed under it.
    operands: Vec<(PropFeatureId, String)>,
}

/// The ordered list, which *is* the prop.
///
/// Rows and nothing else. Creating things is the toolbar's job and undoing
/// them the Edit menu's; neither is a property of the list, and both used to
/// sit on top of it pushing the rows down.
fn feature_tree(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild) {
    let entries: Vec<Row> = editor
        .doc()
        .features
        .iter()
        .map(|feature| {
            let dependants = editor.doc().dependants(feature.id);
            Row {
                id: feature.id,
                name: feature.name.clone(),
                enabled: feature.enabled,
                dependants: dependants.len(),
                pins: feature.op.consumes().into_iter().chain(dependants).collect(),
                legal: editor
                    .doc()
                    .legal_range(feature.id)
                    .unwrap_or(0..=0),
                operands: feature
                    .op
                    .consumes()
                    .into_iter()
                    .map(|id| {
                        let name = editor.doc().feature(id).map(|f| f.name.clone());
                        (id, name.unwrap_or_else(|| get!("prop.inspector.missing", "id", id)))
                    })
                    .collect(),
            }
        })
        .collect();
    let live: Vec<PropFeatureId> = build.evaluated.bodies.iter().map(|(id, _)| *id).collect();
    let broken: Vec<PropFeatureId> =
        build.evaluated.problems.iter().map(|problem| problem.feature).collect();

    // Read out rather than borrowed through the closures below: the rows write
    // back a selection and an action, and holding `editor` across them would
    // mean the context menu could not touch either.
    let selected = editor.selected;
    let also_selected = editor.also_selected.clone();
    let mut select: Option<(PropFeatureId, Pick)> = None;
    let mut pending: Option<Box<dyn FnOnce(&mut PropEditor)>> = None;

    // Used once every row has been drawn: where an insertion line goes is a
    // question about the whole list. Discovered this way rather than remembered
    // between frames, so there is no drag state to get out of step.
    let mut rects: Vec<(PropFeatureId, egui::Rect)> = Vec::new();
    let mut dragging: Option<usize> = None;
    let mut dropped = false;

    // The rollback bar sits after this row. With no rollback it sits after the
    // last, where it is a handle to grab rather than a statement.
    let rollback = editor.rollback();
    let bar_after = rollback
        .and_then(|id| entries.iter().position(|row| row.id == id))
        .unwrap_or(entries.len().saturating_sub(1));
    let mut bar_dragged = false;

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (at, row) in entries.iter().enumerate() {
            let Row { id, enabled, dependants, .. } = *row;
            let name = &row.name;

            // Reserved before the row and filled in after: the fill depends on
            // a hover the row has not reported yet, and painting afterwards
            // without reserving would put the highlight over the name.
            let background = ui.painter().add(egui::Shape::Noop);

            // **The row is the widget; the name is just text.** egui's click
            // hit-testing filters to widgets that sense clicks, so a plain
            // hover-only `Label` lets both clicks fall through to the row — a
            // `SelectableLabel` swallowed the secondary click over the one part
            // of the row anybody aims at.
            //
            // `.interact(Sense::click())` is what makes the row a widget at
            // all: a `Ui`'s own response is `Sense::hover()`, and
            // `Popup::context_menu` opens on `secondary_clicked()`, which such
            // a response can never report.
            let past_the_bar = at > bar_after;
            let response = ui
                .horizontal(|ui| {
                    // Past the rollback bar: in the document, not in the view.
                    // Faded rather than struck or dimmed, which already mean
                    // suppressed and eaten.
                    if past_the_bar {
                        ui.multiply_opacity(0.4);
                    }
                    // Three states, and they are not the same thing, so they
                    // are not drawn the same way. **Struck through** is
                    // suppressed — deliberately switched off, and the only cue
                    // left now that the checkbox has moved into the menu.
                    // **Weak** is consumed: a body a later feature has already
                    // eaten, which is history rather than a problem. **Orange**
                    // is broken, which is the one worth looking at.
                    let mut text = egui::RichText::new(if broken.contains(&id) {
                        format!("{name}  {}", get!("prop.problems.marker"))
                    } else {
                        name.clone()
                    });
                    if !enabled {
                        text = text.strikethrough();
                    }
                    if broken.contains(&id) {
                        text = text.color(egui::Color32::from_rgb(230, 130, 80));
                    } else if !live.contains(&id) {
                        text = text.weak();
                    }

                    // `.selectable(false)` is the other half, and not about the
                    // caret: a selectable label picks up
                    // `Sense::click_and_drag()` to support that, which puts it
                    // back in the hit-test and wins over the row.
                    ui.add(egui::Label::new(text).selectable(false));
                    // Claims the rest of the line, or `horizontal` shrinks to
                    // the text and so does the grabbable area.
                    ui.allocate_space(egui::vec2(ui.available_width(), 0.0));

                })
                .response
                .interact(egui::Sense::click_and_drag());

            rects.push((id, response.rect));
            // `dragged` stays true for the row the drag began on even once the
            // pointer has left it.
            if response.dragged() {
                dragging = Some(at);
            }
            if response.drag_stopped() {
                dragging = Some(at);
                dropped = true;
            }
            if response.clicked() {
                let extending = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                select = Some((id, if extending { Pick::Toggle } else { Pick::Only }));
            }

            // Selection outranks hover, or a chosen row dims when pointed at.
            let fill = if response.dragged() {
                Some(ui.visuals().widgets.active.weak_bg_fill)
            } else if selected == Some(id) {
                Some(ui.visuals().selection.bg_fill)
            } else if also_selected.contains(&id) {
                // Picked but not primary: visibly in the selection, visibly
                // not the one the inspector is showing.
                Some(ui.visuals().selection.bg_fill.gamma_multiply(0.45))
            } else if response.hovered() {
                Some(ui.visuals().widgets.hovered.weak_bg_fill)
            } else {
                None
            };
            if let Some(fill) = fill {
                ui.painter().set(
                    background,
                    egui::Shape::rect_filled(
                        response.rect,
                        ui.visuals().widgets.active.corner_radius,
                        fill,
                    ),
                );
            }

            response.context_menu(|ui| {
                // Right-clicking is a way of pointing at a thing, so it selects
                // too — otherwise the inspector shows something else. Unless it
                // is already picked: right-clicking one of two bodies you have
                // just Ctrl-clicked should not throw the other away.
                if selected != Some(id) && !also_selected.contains(&id) {
                    select = Some((id, Pick::Only));
                }

                // Names the **action**, not the state: a label naming the state
                // reads as a checkbox with no box.
                let action = match enabled {
                    true => get!("prop.tree.suppress"),
                    false => get!("prop.tree.unsuppress"),
                };
                if ui.button(action).clicked() {
                    ui.close_kind(UiKind::Menu);
                    pending = Some(Box::new(move |editor: &mut PropEditor| {
                        editor.edit(|doc| {
                            if let Some(feature) = doc.feature_mut(id) {
                                feature.enabled = !enabled;
                            }
                        });
                    }));
                }

                if at != bar_after && ui.button(get!("prop.tree.roll_back_here")).clicked() {
                    ui.close_kind(UiKind::Menu);
                    pending = Some(Box::new(move |editor: &mut PropEditor| {
                        editor.roll_back_to(Some(id));
                    }));
                }
                if rollback.is_some() && ui.button(get!("prop.tree.roll_to_end")).clicked() {
                    ui.close_kind(UiKind::Menu);
                    pending = Some(Box::new(|editor: &mut PropEditor| editor.roll_back_to(None)));
                }

                ui.separator();

                // Only for what consumes nothing: a boolean's copy would find
                // its operands already eaten by the original.
                let copyable = editor.doc().feature(id).is_some_and(|feature| feature.op.consumes().is_empty());
                if ui
                    .add_enabled(copyable, egui::Button::new(get!("prop.tree.duplicate")))
                    .clicked()
                {
                    ui.close_kind(UiKind::Menu);
                    let name = get!("prop.tree.copy_of", "name", name);
                    pending = Some(Box::new(move |editor: &mut PropEditor| {
                        if let Some(copy) = editor.edit(|doc| doc.duplicate(id, name)) {
                            // A copy of the last thing above the bar is above
                            // it too.
                            if editor.rollback() == Some(id) {
                                editor.roll_back_to(Some(copy));
                            }
                            editor.select(copy);
                        }
                    }));
                }

                // **Repaint lives here, not beside Extrude.** It is a thing you
                // do *to a body*, and a body-producing feature already carries
                // its own surface — so it is the exception, reached from the
                // body it applies to. Starts from what the body looks like
                // now, so opening it changes nothing until something is moved.
                let standing = build.evaluated.bodies.iter().find(|(body, _)| *body == id);
                let repaint = ui.add_enabled(
                    standing.is_some(),
                    egui::Button::new(get!("prop.tree.repaint")),
                );
                let repaint = repaint.on_disabled_hover_text(get!("prop.tree.repaint_eaten"));
                if repaint.clicked() {
                    ui.close_kind(UiKind::Menu);
                    let surface = standing.map(|(_, solid)| solid.dominant_surface()).unwrap_or_default();
                    pending = Some(Box::new(move |editor: &mut PropEditor| {
                        let added = editor.add(FeatureOp::Paint { target: id, surface });
                        editor.select(added);
                    }));
                }

                ui.separator();

                let delete = ui.button(get!("prop.tree.delete"));
                // Beforehand rather than after: everything naming this feature
                // breaks the moment it goes, and reading that here is cheaper
                // than four warnings you have to find the common cause of.
                let delete = match dependants {
                    0 => delete,
                    count => delete.on_hover_text(get!(
                        "prop.tree.delete_breaks",
                        "count",
                        count
                    )),
                };
                if delete.clicked() {
                    ui.close_kind(UiKind::Menu);
                    pending = Some(Box::new(move |editor: &mut PropEditor| {
                        editor.edit(|doc| doc.remove(id));
                        editor.forget_missing();
                    }));
                }
            });

            // **What this feature ate, beneath it.** The list stays in document
            // order — the order *is* the prop, and reordering needs it flat —
            // so the structure a boolean makes is shown under it rather than by
            // moving its operands there. Clicking one selects it.
            for (operand, operand_name) in &row.operands {
                ui.horizontal(|ui| {
                    if past_the_bar {
                        ui.multiply_opacity(0.4);
                    }
                    ui.add_space(16.0);
                    let text = egui::RichText::new(format!("· {operand_name}")).weak().small();
                    let line = ui.add(egui::Label::new(text).selectable(false).sense(egui::Sense::click()));
                    if line.clicked() {
                        select = Some((*operand, Pick::Only));
                    }
                });
            }

            // **The rollback bar**, dragged up the list to see the prop as it
            // stood there. Thin and faint at the bottom where it changes
            // nothing; bold anywhere else, because a view that is not showing
            // the whole prop must not be mistaken for one that is.
            if at == bar_after {
                let (rect, bar) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::drag());
                let bar = bar.on_hover_cursor(egui::CursorIcon::ResizeVertical);
                let lit = rollback.is_some() || bar.hovered() || bar.dragged();
                let stroke = match lit {
                    true => egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
                    false => egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
                };
                ui.painter().hline(rect.x_range(), rect.center().y, stroke);
                bar_dragged = bar.dragged();
            }
        }

        // Live while dragging, so the viewport scrubs through the history
        // under the pointer. Never above the first feature: a prop rolled back
        // to nothing is an empty viewport, which says nothing.
        if bar_dragged
            && let Some(pointer) = ui.ctx().pointer_interact_pos()
        {
            let above = rects.iter().take_while(|(_, rect)| rect.center().y < pointer.y).count();
            if let Some((anchor, _)) = rects.get(above.max(1) - 1) {
                let anchor = *anchor;
                pending = Some(Box::new(move |editor: &mut PropEditor| {
                    editor.roll_back_to(Some(anchor));
                }));
            }
        }

        // After every row has reported its rect: where the line goes is a fact
        // about the list, not about any one row.
        if let Some(at) = dragging {
            let row = &entries[at];
            let target = show_the_drop(ui, &rects, row);
            if dropped {
                let id = row.id;
                pending = Some(Box::new(move |editor: &mut PropEditor| {
                    editor.edit(|doc| doc.move_to(id, target));
                }));
            }
        }
    });

    match select {
        Some((id, Pick::Only)) => editor.select(id),
        Some((id, Pick::Toggle)) => editor.toggle_selected(id),
        None => {}
    }
    if let Some(action) = pending {
        action(editor);
    }
}

/// How a click on a row changes the selection.
#[derive(Clone, Copy)]
enum Pick {
    /// A plain click: this and nothing else.
    Only,
    /// Ctrl/Cmd held: add it, or take it out if it was in.
    Toggle,
}

/// Draw where a dragged feature would land, and say where it may land.
///
/// Returns the index it would drop at, **clamped into the legal range**. The
/// legal positions are one contiguous span (see `PropDoc::legal_range`), so
/// clamping is all it takes: the line follows the pointer and stops dead
/// against whatever is pinning it, which you feel rather than have to read.
///
/// The pinning features are marked at the same time, because "it stopped" and
/// "it stopped *there*" are different amounts of help.
fn show_the_drop(ui: &egui::Ui, rects: &[(PropFeatureId, egui::Rect)], row: &Row) -> usize {
    let others: Vec<egui::Rect> = rects
        .iter()
        .filter(|(id, _)| *id != row.id)
        .map(|(_, rect)| *rect)
        .collect();
    if others.is_empty() {
        return 0;
    }

    let pointer = ui
        .ctx()
        .pointer_interact_pos()
        .map(|pointer| pointer.y)
        .unwrap_or(f32::NEG_INFINITY);
    // Midpoints rather than edges, so the line flips to the next gap when the
    // pointer is more than half way into a row.
    let wanted = others.iter().take_while(|rect| rect.center().y < pointer).count();
    let target = wanted.clamp(*row.legal.start(), *row.legal.end());

    let painter = ui.painter();
    for (id, rect) in rects {
        if row.pins.contains(id) {
            // A bar rather than an outline, so it does not compete with the
            // selection highlight the row may already be wearing.
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                0.0,
                ui.visuals().warn_fg_color,
            );
        }
    }

    let line = match others.get(target) {
        Some(rect) => rect.top(),
        // Past the last row, where a feature with no dependants may go.
        None => others.last().expect("checked non-empty above").bottom(),
    };
    painter.hline(
        others[0].x_range(),
        line,
        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
    );

    target
}

/// The verbs, in a row under the menu bar.
///
/// **They act on the selection, not on whatever was added last.** Combine
/// takes the two bodies picked (first as target, second as tool), Mirror the
/// one in the inspector; each is greyed out when the selection does not fit.
/// No hover text explaining what to pick — the UI states what is, it is not a
/// tutorial. Creating a solid needs nothing picked.
///
/// `frame` is here rather than in a View menu because it is the one button
/// somebody reaches for after losing the prop, and a menu is one click further
/// from it.
fn toolbar(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild, frame: &mut Option<bool>) {
    let live: Vec<PropFeatureId> = build.evaluated.bodies.iter().map(|(id, _)| *id).collect();
    let pair = editor.pair_to_combine(&live);
    let single = editor.body_to_act_on(&live);
    let mut added: Option<FeatureOp> = None;

    ui.horizontal_wrapped(|ui| {
        ui.menu_button(get!("prop.add.solid"), |ui| {
            let shapes = [
                Shape::Box { size: [0.1, 0.1, 0.3] },
                Shape::Prism { sides: 8, radius: 0.04, height: 0.4 },
                Shape::Cone { sides: 8, radius: 0.05, height: 0.1 },
                Shape::Sphere { sides: 8, rings: 5, radius: 0.04 },
                Shape::Wedge { size: [0.1, 0.1, 0.2] },
            ];
            for shape in shapes {
                if ui.button(shape.name()).clicked() {
                    ui.close_kind(UiKind::Menu);
                    added = Some(FeatureOp::Primitive {
                        shape,
                        placement: Placement::default(),
                        surface: Surface::default(),
                    });
                }
            }
        });

        ui.menu_button(get!("prop.add.sweep"), |ui| {
            if ui.button(get!("prop.features.extrude")).clicked() {
                ui.close_kind(UiKind::Menu);
                added = Some(FeatureOp::Extrude {
                    profile: Profile::Ngon { sides: 8, radius: 0.05 },
                    placement: Placement::default(),
                    depth: 0.2,
                    midplane: true,
                    surface: Surface::default(),
                });
            }
            if ui.button(get!("prop.features.revolve")).clicked() {
                ui.close_kind(UiKind::Menu);
                added = Some(FeatureOp::Revolve {
                    profile: Profile::Rect { half: [0.03, 0.05] },
                    placement: Placement::at(0.06, 0.0, 0.0),
                    degrees: 360.0,
                    segments: 12,
                    surface: Surface::default(),
                });
            }
        });

        ui.separator();

        ui.add_enabled_ui(pair.is_some(), |ui| {
            ui.menu_button(get!("prop.add.combine"), |ui| {
                let Some((target, tool)) = pair else { return };
                for op in BooleanOp::ALL {
                    if ui.button(op.name()).clicked() {
                        ui.close_kind(UiKind::Menu);
                        added = Some(FeatureOp::Boolean { op, target, tool });
                    }
                }
            });
        });

        let mirror = ui
            .add_enabled(single.is_some(), egui::Button::new(get!("prop.features.mirror")));
        if let (true, Some(target)) = (mirror.clicked(), single) {
            added = Some(FeatureOp::Mirror {
                target,
                axis: Axis::X,
                offset: 0.0,
                keep_original: true,
            });
        }
        let pattern = ui
            .add_enabled(single.is_some(), egui::Button::new(get!("prop.features.pattern")));
        if let (true, Some(target)) = (pattern.clicked(), single) {
            added = Some(FeatureOp::Pattern {
                target,
                count: 3,
                along: PatternKind::Linear { step: [0.0, 0.0, 0.05] },
            });
        }

        ui.separator();

        if ui.button(get!("prop.menu.frame")).clicked() {
            *frame = Some(false);
        }
        let framable = build.evaluated.bounds_of_bodies(&editor.selection()).is_some();
        if ui
            .add_enabled(framable, egui::Button::new(get!("prop.menu.frame_selected")))
            .clicked()
        {
            *frame = Some(true);
        }
    });

    if let Some(op) = added {
        let id = editor.add(op);
        editor.select(id);
    }
}

/// What the Edit menu can do for the prop right now.
///
/// Revert needs a file to go back to and something to throw away: a document
/// that matches its file has nothing to revert.
pub(crate) fn edit_availability(editor: &PropEditor) -> EditAvailability {
    EditAvailability {
        undo: editor.can_undo(),
        redo: editor.can_redo(),
        revert: editor.path.is_some() && editor.dirty,
    }
}

/// Answer the Edit menu, or the chord for one of its entries, for the prop.
///
/// Shared with the animation editor, whose clips live in the same file — two
/// answers to "what does Revert do to a prop" would be two that drift.
pub(crate) fn handle_edit(action: Option<EditAction>, editor: &mut PropEditor) {
    match action {
        Some(EditAction::Undo) => editor.undo(),
        Some(EditAction::Redo) => editor.redo(),
        Some(EditAction::Revert) => revert_from_disk(editor),
        None => {}
    }
}

/// Read the prop's file again and go back to it, as one undo step.
///
/// Through `std::fs` like Open, because both are the authoring side; the game
/// reads props through `Assets`.
fn revert_from_disk(editor: &mut PropEditor) {
    let Some(path) = editor.path.clone() else { return };
    match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| {
        document::parse(&text).map_err(|e| e.to_string())
    }) {
        Ok(saved) => {
            info!("Reverted the prop to {path:?}");
            editor.revert_to(saved);
        }
        Err(e) => error!("Reverting to {}: {e}", path.display()),
    }
}

/// The selected feature's numbers, or the prop's own when nothing is picked.
///
/// **In sections, in a fixed order** — Feature, Shape or Profile, Placement,
/// Material, Operands — so a number is in the same place whatever kind of
/// feature it is on, and a section somebody never touches can be folded away
/// for good. egui remembers which are open by id, so the ids are per section
/// rather than per feature: folding Placement folds it everywhere.
fn inspector(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild) {
    let Some(selected) = editor.selected else {
        prop_overview(ui, editor, build);
        return;
    };
    let Some(index) = editor.doc().index_of(selected) else {
        prop_overview(ui, editor, build);
        return;
    };
    // What this feature could point at is what was live *when it ran* — not
    // what is live now, which is after it has already eaten something.
    // Recorded by the evaluator so there is one answer rather than two.
    //
    // Past the rollback bar there is no record, since the view did not
    // evaluate that far; everything listed above the feature stands in.
    // Re-evaluating the whole prop here would run the kernel every frame for as
    // long as the feature was selected.
    let choices: Vec<PropFeatureId> = build
        .evaluated
        .live_before
        .get(index)
        .cloned()
        .unwrap_or_else(|| editor.doc().features[..index].iter().map(|f| f.id).collect());
    let names: Vec<(PropFeatureId, String)> = choices
        .iter()
        .map(|id| {
            (*id, editor.doc().feature(*id).map(|f| f.name.clone()).unwrap_or_else(|| id.to_string()))
        })
        .collect();
    let picked = editor.selection().len();

    // Drawn against a **copy**, written back only if something moved.
    // `PropEditor::doc_mut` marks the document changed by being called, which
    // is what makes the viewport rebuild — so an inspector that reached for it
    // every frame would re-run the boolean kernel, and light the unsaved-work
    // asterisk, for as long as anything was merely selected.
    let Some(mut edited) = editor.doc().feature(selected).cloned() else { return };
    let feature = &mut edited;

    let mut changed = false;
    egui::ScrollArea::vertical().show(ui, |ui| {
        // States that more than one thing is selected, since the numbers below
        // are only the primary's.
        if picked > 1 {
            ui.label(egui::RichText::new(get!("prop.inspector.picked", "count", picked)).weak());
        }

        section(ui, "feature", get!("prop.inspector.sections.feature"), |ui| {
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.kind"));
                ui.label(feature.op.type_name());
            });
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.name"));
                changed |= ui.text_edit_singleline(&mut feature.name).changed();
            });
            // **Positive, unlike the tree's menu, and that is not an
            // inconsistency.** A menu item names an *action*, and the useful
            // thing to say there is what clicking will do — "Suppress". A
            // checkbox names a *state*, and a ticked box meaning "off" is the
            // kind of thing you have to read twice. So the two differ because
            // they are different questions, and the box is bound straight to
            // the stored field with nothing to invert.
            changed |= ui.checkbox(&mut feature.enabled, get!("prop.inspector.active")).changed();
        });

        let main = feature.op.main_axis();
        match &mut feature.op {
            FeatureOp::Primitive { shape, placement, surface } => {
                section(ui, "shape", get!("prop.inspector.sections.shape"), |ui| {
                    changed |= shape_ui(ui, shape);
                });
                section(ui, "placement", get!("prop.inspector.sections.placement"), |ui| {
                    changed |= placement_ui(ui, placement, main);
                });
                section(ui, "material", get!("prop.inspector.sections.material"), |ui| {
                    changed |= surface_ui(ui, surface);
                });
            }
            FeatureOp::Extrude { profile, placement, depth, midplane, surface } => {
                section(ui, "profile", get!("prop.inspector.sections.profile"), |ui| {
                    changed |= profile_ui(ui, profile);
                    ui.separator();
                    changed |= drag(ui, get!("prop.inspector.depth"), depth, 0.001, "m");
                    changed |= ui.checkbox(midplane, get!("prop.inspector.midplane")).changed();
                });
                section(ui, "placement", get!("prop.inspector.sections.placement"), |ui| {
                    changed |= placement_ui(ui, placement, main);
                });
                section(ui, "material", get!("prop.inspector.sections.material"), |ui| {
                    changed |= surface_ui(ui, surface);
                });
            }
            FeatureOp::Revolve { profile, placement, degrees, segments, surface } => {
                section(ui, "profile", get!("prop.inspector.sections.profile"), |ui| {
                    changed |= profile_ui(ui, profile);
                    ui.separator();
                    changed |= drag(ui, get!("prop.inspector.degrees"), degrees, 1.0, "°");
                    changed |= count(ui, get!("prop.inspector.segments"), segments);
                });
                section(ui, "placement", get!("prop.inspector.sections.placement"), |ui| {
                    changed |= placement_ui(ui, placement, main);
                });
                section(ui, "material", get!("prop.inspector.sections.material"), |ui| {
                    changed |= surface_ui(ui, surface);
                });
            }
            FeatureOp::Boolean { op, target, tool } => {
                section(ui, "operands", get!("prop.inspector.sections.operands"), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(get!("prop.inspector.operation"));
                        for candidate in BooleanOp::ALL {
                            changed |= ui
                                .selectable_value(op, candidate, candidate.name())
                                .changed();
                        }
                    });
                    changed |= body_picker(ui, get!("prop.inspector.target"), target, &names);
                    changed |= body_picker(ui, get!("prop.inspector.tool"), tool, &names);
                });
            }
            FeatureOp::Mirror { target, axis, offset, keep_original } => {
                section(ui, "operands", get!("prop.inspector.sections.operands"), |ui| {
                    changed |= body_picker(ui, get!("prop.inspector.target"), target, &names);
                });
                section(ui, "mirror", get!("prop.inspector.sections.mirror"), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(get!("prop.inspector.plane"));
                        for candidate in Axis::ALL {
                            changed |= ui
                                .selectable_value(axis, candidate, candidate.name())
                                .changed();
                        }
                    });
                    changed |= drag(ui, get!("prop.inspector.offset"), offset, 0.001, "m");
                    changed |= ui
                        .checkbox(keep_original, get!("prop.inspector.keep_original"))
                        .changed();
                });
            }
            FeatureOp::Paint { target, surface } => {
                section(ui, "operands", get!("prop.inspector.sections.operands"), |ui| {
                    changed |= body_picker(ui, get!("prop.inspector.target"), target, &names);
                });
                section(ui, "material", get!("prop.inspector.sections.material"), |ui| {
                    changed |= surface_ui(ui, surface);
                });
            }
            FeatureOp::Pattern { target, count, along } => {
                section(ui, "operands", get!("prop.inspector.sections.operands"), |ui| {
                    changed |= body_picker(ui, get!("prop.inspector.target"), target, &names);
                });
                section(ui, "pattern", get!("prop.inspector.sections.pattern"), |ui| {
                    changed |= pattern_ui(ui, count, along);
                });
            }
        }
    });

    if changed {
        editor.begin_gesture();
        if let Some(slot) = editor.doc_mut().feature_mut(selected) {
            *slot = edited;
        }
    }
}

/// One fold of the inspector, open until somebody closes it.
fn section(ui: &mut Ui, id: &str, title: String, body: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(egui::RichText::new(title).strong())
        .id_salt(("prop_section", id))
        .default_open(true)
        .show(ui, body);
}

/// What the inspector shows with nothing picked: the prop itself.
///
/// An empty panel is a panel that says nothing, and there *is* something to
/// say — the name a weapon list will show and how big the thing is, which
/// otherwise lived only in a corner of the Problems tab.
fn prop_overview(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild) {
    let mut name_key = editor.doc().name_key.clone();
    let mut changed = false;

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.label(egui::RichText::new(get!("prop.inspector.nothing")).weak());
        section(ui, "prop", get!("prop.inspector.sections.prop"), |ui| {
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.name_key"));
                changed |= ui.text_edit_singleline(&mut name_key).changed();
            });
            // Resolved, so a key with a typo in it reads as `<key>` here
            // rather than first in somebody's loadout.
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.shown_as"));
                ui.label(egui::RichText::new(get!(name_key)).italics());
            });
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.file"));
                ui.label(editor.saved_name().map(str::to_owned).unwrap_or_else(|| get!("prop.untitled")));
            });
        });
        section(ui, "size", get!("prop.inspector.sections.size"), |ui| {
            if let Some((min, max)) = build.evaluated.bounds() {
                let size = max - min;
                ui.label(format!("{:.3} × {:.3} × {:.3} m", size.x, size.y, size.z));
            }
            ui.label(get!("prop.stats.features", "count", editor.doc().features.len()));
            ui.label(get!("prop.stats.bodies", "count", build.evaluated.bodies.len()));
            ui.label(get!("prop.stats.triangles", "count", build.evaluated.triangle_count()));
        });
    });

    if changed {
        editor.begin_gesture();
        editor.doc_mut().name_key = name_key;
    }
}

/// What is in the viewport besides the prop: for now, who is standing there.
///
/// In both workspaces, so it is in one place; in Fit "nobody" is not offered,
/// since a hold fitted to nobody is not being fitted.
fn view_menu(ui: &mut Ui, workspace: Workspace, figure: &mut ScaleFigure) {
    ui.menu_button(get!("prop.menu.view"), |ui| {
        ui.label(egui::RichText::new(get!("prop.scene.figure")).weak());
        if workspace == Workspace::Model {
            ui.selectable_value(&mut figure.0, None, get!("prop.scene.no_figure"));
        }
        for class in Class::iter() {
            ui.selectable_value(&mut figure.0, Some(class), class.name());
        }
    });
}

/// Fit's toolbar: who the hold is being fitted to, and framing.
///
/// **The class on show is the class being edited** — one question, asked
/// once. Picking a class here both changes who is standing there and which
/// per-class override the Hold panel is editing.
fn fit_toolbar(ui: &mut Ui, figure: &mut ScaleFigure, frame: &mut Option<bool>) {
    ui.label(get!("prop.scene.figure"));
    egui::ComboBox::from_id_salt("prop_fit_class")
        .selected_text(figure.0.map(|class| class.name()).unwrap_or_default())
        .show_ui(ui, |ui| {
            for class in Class::iter() {
                ui.selectable_value(&mut figure.0, Some(class), class.name());
            }
        });
    ui.separator();
    if ui.button(get!("prop.menu.frame")).clicked() {
        *frame = Some(false);
    }
}

/// How this prop is held.
///
/// Numbers rather than handles, for now — the gizmos are the next piece. What
/// makes them worth typing at all is that the reference figure performs *this*
/// hold, so the effect of a change is on screen the moment it is made.
///
/// **The class on show is the class being edited.** One question, asked once:
/// ticking the override edits the entry for whoever is standing there, which
/// is also the body you are judging it against.
#[allow(clippy::too_many_arguments)]
fn hold(
    ui: &mut Ui,
    editor: &mut PropEditor,
    figure: ScaleFigure,
    preview: &mut ViewmodelPreview,
) {
    let base = editor.doc().hold.clone();
    let key = figure.0.map(Class::key);
    let overriding = key.as_ref().is_some_and(|key| base.per_class.contains_key(key));

    let mut changed = false;
    let mut edited = base.for_class(figure.0);
    // Not per class: a viewmodel is a framing on a screen, not a fact about
    // whoever is holding the thing.
    let mut viewmodel = editor.doc().viewmodel;
    let mut view_changed = false;

    egui::ScrollArea::vertical().show(ui, |ui| {
        // Whether the numbers below are this class's own or everybody's. Fit
        // always has somebody standing there, so there is always a class to
        // ask about.
        if let (Some(key), Some(class)) = (&key, figure.0) {
            section(ui, "hold_class", class.name(), |ui| {
                let mut on = overriding;
                if ui
                    .checkbox(&mut on, get!("prop.hold.override_for", "class", class.name()))
                    .changed()
                {
                    let mut doc_hold = base.clone();
                    match on {
                        true => {
                            doc_hold.per_class.insert(key.clone(), HoldOverride::default());
                        }
                        false => {
                            doc_hold.per_class.remove(key);
                        }
                    }
                    editor.edit(|doc| doc.hold = doc_hold);
                }
            });
        }

        section(ui, "hold_grip", get!("prop.hold.grip"), |ui| {
            ui.label(get!("prop.hold.trigger_hand"));
            changed |= vec3_ui(ui, get!("prop.hold.at"), &mut edited.grip, 0.001, "m");
            changed |= degrees_ui(ui, get!("prop.inspector.rotation"), &mut edited.grip_rotation);

            ui.separator();
            changed |= ui.checkbox(&mut edited.two_handed, get!("prop.hold.two_handed")).changed();
            ui.add_enabled_ui(edited.two_handed, |ui| {
                changed |= vec3_ui(ui, get!("prop.hold.at"), &mut edited.support, 0.001, "m");
                changed |=
                    degrees_ui(ui, get!("prop.inspector.rotation"), &mut edited.support_rotation);
            });
        });

        section(ui, "hold_carry", get!("prop.hold.carry"), |ui| {
            // Arm lengths, not metres, so a carry means the same thing on a
            // Heavy as on a Scout. Dragged in hundredths because the whole
            // useful range is under one arm.
            changed |= vec3_ui(ui, get!("prop.hold.offset"), &mut edited.carry, 0.005, "");
            changed |= degrees_ui(ui, get!("prop.inspector.rotation"), &mut edited.carry_rotation);
        });

        // Outside the fold, so a folded section is a preview that is off.
        let mut holding = false;
        section(ui, "hold_viewmodel", get!("prop.hold.viewmodel"), |ui| {
            // Fractions of the frustum, so the same numbers mean the same place on
            // screen at any field of view and any window — which is what stops a
            // weapon drifting in from the edge it is meant to hang off.
            // **The preview is on while a viewmodel number is being held.** Which
            // is the only moment it is worth having: you are looking for where the
            // weapon lands, and the answer is on screen while your hand is still
            // on the number rather than after a save and a round.
            ui.horizontal(|ui| {
                ui.label(get!("prop.hold.across"));
                for (component, axis) in viewmodel.across.iter_mut().zip(["x", "y"]) {
                    let response = ui.add(
                        egui::DragValue::new(component)
                            .speed(0.01)
                            .range(-2.0..=2.0)
                            .prefix(format!("{axis} ")),
                    );
                    view_changed |= response.changed();
                    holding |= response.dragged() || response.has_focus();
                }
            });
            let (depth_changed, depth_held) =
                held_drag(ui, get!("prop.hold.depth"), &mut viewmodel.depth, 0.005, "m");
            view_changed |= depth_changed;
            holding |= depth_held;

            let mut degrees = viewmodel.rotation.map(f32::to_degrees);
            let (turned, turn_held) =
                held_vec3(ui, get!("prop.inspector.rotation"), &mut degrees, 1.0, "°");
            if turned {
                viewmodel.rotation = degrees.map(f32::to_radians);
                view_changed = true;
            }
            holding |= turn_held;
        });

        if preview.0 != holding {
            preview.0 = holding;
        }
    });

    if view_changed {
        editor.begin_gesture();
        editor.doc_mut().viewmodel = viewmodel;
    }

    if !changed {
        return;
    }
    editor.begin_gesture();
    let applied = base.applied(figure.0, &edited);
    editor.doc_mut().hold = applied;
}

/// Three drag boxes in degrees, over a field stored in radians.
fn degrees_ui(ui: &mut Ui, label: String, radians: &mut [f32; 3]) -> bool {
    let mut degrees = radians.map(f32::to_degrees);
    if vec3_ui(ui, label, &mut degrees, 1.0, "°") {
        *radians = degrees.map(f32::to_radians);
        return true;
    }
    false
}

/// One line along the bottom: what is selected, what the prop amounts to,
/// and whether the viewport is still catching up.
///
/// Out of the Problems tab, where it used to be, because it is true whichever
/// tab is in front.
fn status_line(ui: &mut Ui, editor: &PropEditor, build: &PropBuild) {
    ui.horizontal(|ui| {
        let names: Vec<String> = editor
            .selection()
            .iter()
            .filter_map(|id| editor.doc().feature(*id).map(|feature| feature.name.clone()))
            .collect();
        ui.label(match names.is_empty() {
            true => get!("prop.inspector.nothing"),
            false => get!("prop.stats.selected", "names", names.join(", ")),
        });
        // Said in the one place visible whatever tab is up, and in the warning
        // colour: a rolled-back view is a partial prop.
        if let Some(name) = editor.rollback().and_then(|id| editor.doc().feature(id)).map(|f| f.name.clone()) {
            ui.separator();
            ui.label(
                egui::RichText::new(get!("prop.stats.rolled_back", "name", name))
                    .color(ui.visuals().warn_fg_color),
            );
        }
        ui.separator();
        ui.label(get!("prop.stats.bodies", "count", build.evaluated.bodies.len()));
        ui.separator();
        ui.label(get!("prop.stats.triangles", "count", build.evaluated.triangle_count()));
        if let Some((min, max)) = build.evaluated.bounds() {
            ui.separator();
            let size = max - min;
            ui.label(format!("{:.3} × {:.3} × {:.3} m", size.x, size.y, size.z));
        }
        // A viewport that stopped following a drag looks like one that stopped
        // working.
        if build.settling() {
            ui.separator();
            ui.label(egui::RichText::new(get!("prop.stats.settling")).weak());
        }
    });
}

fn problems(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild) {
    if build.evaluated.problems.is_empty() {
        ui.label(get!("prop.problems.none"));
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for problem in &build.evaluated.problems {
            let name = editor
                .doc()
                .feature(problem.feature)
                .map(|feature| feature.name.clone())
                .unwrap_or_else(|| problem.feature.to_string());
            if ui
                .selectable_label(
                    editor.selected == Some(problem.feature),
                    egui::RichText::new(format!(
                        "{} {name}: {}",
                        get!("prop.problems.marker"),
                        problem.message,
                    ))
                        .color(egui::Color32::from_rgb(230, 130, 80)),
                )
                .clicked()
            {
                editor.select(problem.feature);
            }
        }
    });
}

/// As [`drag`], but says whether the box is being held as well as whether it
/// changed — which is what the viewmodel preview is on for.
fn held_drag(
    ui: &mut Ui,
    label: String,
    value: &mut f32,
    speed: f64,
    suffix: &str,
) -> (bool, bool) {
    ui.horizontal(|ui| {
        ui.label(label);
        let response = ui.add(egui::DragValue::new(value).speed(speed).suffix(suffix));
        (response.changed(), response.dragged() || response.has_focus())
    })
    .inner
}

/// Three of them.
fn held_vec3(
    ui: &mut Ui,
    label: String,
    value: &mut [f32; 3],
    speed: f64,
    suffix: &str,
) -> (bool, bool) {
    let mut changed = false;
    let mut holding = false;
    ui.horizontal(|ui| {
        ui.label(label);
        for (component, axis) in value.iter_mut().zip(["x", "y", "z"]) {
            let response = ui.add(
                egui::DragValue::new(component)
                    .speed(speed)
                    .prefix(format!("{axis} "))
                    .suffix(suffix),
            );
            changed |= response.changed();
            holding |= response.dragged() || response.has_focus();
        }
    });
    (changed, holding)
}

fn drag(ui: &mut Ui, label: String, value: &mut f32, speed: f64, suffix: &str) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).speed(speed).suffix(suffix)).changed()
    })
    .inner
}

fn count(ui: &mut Ui, label: String, value: &mut u32) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        // Clamped low: fewer than three sides is not a shape.
        ui.add(egui::DragValue::new(value).speed(0.2).range(MIN_SIDES..=128)).changed()
    })
    .inner
}

fn vec3_ui(ui: &mut Ui, label: String, value: &mut [f32; 3], speed: f64, suffix: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        for (component, axis) in value.iter_mut().zip(["x", "y", "z"]) {
            changed |= ui
                .add(egui::DragValue::new(component).speed(speed).prefix(format!("{axis} ")).suffix(suffix))
                .changed();
        }
    });
    changed
}

/// Where a feature sits and which way it faces.
///
/// **Orientation is asked as "points along" and "spin"**, not as three Euler
/// angles: lining up a barrel is "make its length run down −Z", and the
/// answer to that in Euler angles depends on which of the three is applied
/// first. `main` is the feature's own length axis
/// ([`FeatureOp::main_axis`]). The angles themselves are still what is
/// stored, and are behind a fold for the cases the presets do not reach.
fn placement_ui(ui: &mut Ui, placement: &mut Placement, main: Vec3) -> bool {
    let mut changed = vec3_ui(ui, get!("prop.inspector.origin"), &mut placement.origin, 0.001, "");

    let (aim, spin) = aim_and_spin(placement.rotation(), main);
    let mut wanted: Option<(Vec3, f32)> = None;

    ui.horizontal(|ui| {
        ui.label(get!("prop.inspector.points_along"));
        let directions = [
            ("+X", Vec3::X),
            ("−X", Vec3::NEG_X),
            ("+Y", Vec3::Y),
            ("−Y", Vec3::NEG_Y),
            ("+Z", Vec3::Z),
            ("−Z", Vec3::NEG_Z),
        ];
        for (label, direction) in directions {
            // Lit when it is already the answer, so the current state reads
            // off the row; an aim between the axes lights none of them.
            let current = aim.dot(direction) > 0.9999;
            if ui.selectable_label(current, label).clicked() && !current {
                wanted = Some((direction, spin));
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(get!("prop.inspector.spin"));
        let mut degrees = spin.to_degrees();
        if ui.add(egui::DragValue::new(&mut degrees).speed(1.0).suffix("°")).changed() {
            wanted = Some((aim, degrees.to_radians()));
        }
    });

    if let Some((aim, spin)) = wanted {
        placement.rotation = tidy_angles(euler_from_quat(from_aim_and_spin(main, aim, spin)));
        changed = true;
    }

    egui::CollapsingHeader::new(get!("prop.inspector.euler"))
        .id_salt("prop_section_euler")
        .default_open(false)
        .show(ui, |ui| {
            // Degrees in the boxes and radians in the field, as a map
            // feature's rotation is: parts are lined up in degrees, maths is
            // done in radians.
            let mut degrees = placement.rotation.map(f32::to_degrees);
            if vec3_ui(ui, get!("prop.inspector.rotation"), &mut degrees, 1.0, "°") {
                placement.rotation = degrees.map(f32::to_radians);
                changed = true;
            }
        });
    changed
}

/// Angles as they go on disk after a preset: the rounding residue of a
/// quaternion round trip snapped to zero, so pointing a barrel down −Z writes
/// `0.0` rather than `-0.00000004` into a file somebody reads.
fn tidy_angles(angles: Vec3) -> [f32; 3] {
    angles.to_array().map(|angle| if angle.abs() < 1e-6 { 0.0 } else { angle })
}

fn surface_ui(ui: &mut Ui, surface: &mut Surface) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(get!("prop.inspector.style"));
        for style in Style::ALL {
            changed |= ui.selectable_value(&mut surface.style, style, style.name()).changed();
        }
    });
    ui.horizontal(|ui| {
        ui.label(get!("prop.inspector.tint"));
        changed |= ui.color_edit_button_rgb(&mut surface.tint.0).changed();
    });
    changed
}

fn shape_ui(ui: &mut Ui, shape: &mut Shape) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(get!("prop.inspector.shape"));
        ui.label(shape.name());
    });
    match shape {
        Shape::Box { size } | Shape::Wedge { size } => {
            changed |= vec3_ui(ui, get!("prop.inspector.size"), size, 0.001, "m");
        }
        Shape::Prism { sides, radius, height } | Shape::Cone { sides, radius, height } => {
            changed |= count(ui, get!("prop.inspector.sides"), sides);
            changed |= drag(ui, get!("prop.inspector.radius"), radius, 0.001, "m");
            changed |= drag(ui, get!("prop.inspector.height"), height, 0.001, "m");
        }
        Shape::Sphere { sides, rings, radius } => {
            changed |= count(ui, get!("prop.inspector.sides"), sides);
            changed |= ui
                .horizontal(|ui| {
                    ui.label(get!("prop.inspector.rings"));
                    ui.add(egui::DragValue::new(rings).speed(0.2).range(2..=64)).changed()
                })
                .inner;
            changed |= drag(ui, get!("prop.inspector.radius"), radius, 0.001, "m");
        }
    }
    changed
}

fn profile_ui(ui: &mut Ui, profile: &mut Profile) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(get!("prop.inspector.profile"));
        // Switching kind replaces rather than converts: a rectangle has no side
        // count to carry into an n-gon, so anything else invents numbers.
        if ui.selectable_label(matches!(profile, Profile::Rect { .. }), Profile::Rect { half: [0.0; 2] }.name()).clicked()
            && !matches!(profile, Profile::Rect { .. })
        {
            *profile = Profile::Rect { half: [0.05, 0.05] };
            changed = true;
        }
        if ui.selectable_label(matches!(profile, Profile::Ngon { .. }), Profile::Ngon { sides: 0, radius: 0.0 }.name()).clicked()
            && !matches!(profile, Profile::Ngon { .. })
        {
            *profile = Profile::Ngon { sides: 8, radius: 0.05 };
            changed = true;
        }
        if ui.selectable_label(matches!(profile, Profile::Points { .. }), Profile::Points { points: vec![] }.name()).clicked()
            && !matches!(profile, Profile::Points { .. })
        {
            *profile = Profile::Points {
                points: vec![[-0.05, -0.05], [0.05, -0.05], [0.05, 0.05], [-0.05, 0.05]],
            };
            changed = true;
        }
    });

    match profile {
        Profile::Rect { half } => {
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.half"));
                for (component, axis) in half.iter_mut().zip(["x", "y"]) {
                    changed |= ui
                        .add(egui::DragValue::new(component).speed(0.001).prefix(format!("{axis} ")).suffix("m"))
                        .changed();
                }
            });
        }
        Profile::Ngon { sides, radius } => {
            changed |= count(ui, get!("prop.inspector.sides"), sides);
            changed |= drag(ui, get!("prop.inspector.radius"), radius, 0.001, "m");
        }
        Profile::Points { points } => {
            let mut remove = None;
            for (at, point) in points.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    for component in point.iter_mut() {
                        changed |= ui.add(egui::DragValue::new(component).speed(0.001)).changed();
                    }
                    if ui
                        .small_button(get!("prop.inspector.remove_point"))
                        .on_hover_text(get!("prop.inspector.remove_point_hint"))
                        .clicked()
                    {
                        remove = Some(at);
                    }
                });
            }
            if let Some(at) = remove {
                points.remove(at);
                changed = true;
            }
            if ui.button(get!("prop.inspector.add_point")).clicked() {
                points.push(points.last().copied().unwrap_or([0.0, 0.0]));
                changed = true;
            }
        }
    }
    changed
}

/// How many copies and where they go. Switching between a row and a circle
/// replaces the layout rather than converting it, as a profile does: a step
/// has no angle to carry across.
fn pattern_ui(ui: &mut Ui, count: &mut u32, along: &mut PatternKind) -> bool {
    let mut changed = ui
        .horizontal(|ui| {
            ui.label(get!("prop.inspector.copies"));
            // Capped, because every copy is a boolean union and sixty-four is
            // already more vents than anything has.
            ui.add(egui::DragValue::new(count).speed(0.1).range(1..=64)).changed()
        })
        .inner;
    ui.horizontal(|ui| {
        let linear = matches!(along, PatternKind::Linear { .. });
        if ui.selectable_label(linear, get!("prop.inspector.pattern_linear")).clicked() && !linear {
            *along = PatternKind::Linear { step: [0.0, 0.0, 0.05] };
            changed = true;
        }
        if ui.selectable_label(!linear, get!("prop.inspector.pattern_radial")).clicked() && linear {
            *along = PatternKind::Radial { axis: Axis::Z, centre: [0.0; 3], degrees: 360.0 };
            changed = true;
        }
    });
    match along {
        PatternKind::Linear { step } => {
            changed |= vec3_ui(ui, get!("prop.inspector.step"), step, 0.001, "m");
        }
        PatternKind::Radial { axis, centre, degrees } => {
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.axis"));
                for candidate in Axis::ALL {
                    changed |= ui.selectable_value(axis, candidate, candidate.name()).changed();
                }
            });
            changed |= vec3_ui(ui, get!("prop.inspector.centre"), centre, 0.001, "m");
            changed |= drag(ui, get!("prop.inspector.degrees"), degrees, 1.0, "°");
        }
    }
    changed
}

fn body_picker(
    ui: &mut Ui,
    label: String,
    value: &mut PropFeatureId,
    names: &[(PropFeatureId, String)],
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(&label);
        let selected = names
            .iter()
            .find(|(id, _)| id == value)
            .map(|(_, name)| name.clone())
            // Falling back to the first thing in the list would silently
            // repoint the feature.
            .unwrap_or_else(|| get!("prop.inspector.missing", "id", *value));
        egui::ComboBox::from_id_salt(format!("prop_body_{label}"))
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (id, name) in names {
                    changed |= ui.selectable_value(value, *id, name).changed();
                }
            });
    });
    changed
}

/// Write the prop to the path it came from, or ask for one if it has none.
///
/// **A save with nowhere to save to is a save-as**, which is the same question
/// either way. Shared so the menu item and `Ctrl+S` cannot answer it
/// differently.
pub fn save_or_ask(editor: &mut PropEditor, dialog: &PropFileDialog) {
    let Some(path) = editor.path.clone() else {
        ask_where_to_save(editor, dialog);
        return;
    };
    match document::save(&path, editor.doc()) {
        Ok(()) => {
            info!("Saved prop to {path:?}");
            editor.mark_saved(path);
        }
        Err(e) => error!("Saving the prop failed: {e}"),
    }
}

/// Ask where to put the prop, on a background thread.
pub fn ask_where_to_save(editor: &PropEditor, dialog: &PropFileDialog) {
    let slot = dialog.result.clone();
    let existing = editor.path.clone();
    std::thread::spawn(move || {
        let mut chooser = rfd::AsyncFileDialog::new()
            .add_filter("Grackle Prop", &[PROP_EXTENSION])
            .set_file_name(format!("untitled.{PROP_EXTENSION}"));
        if let Some(directory) = existing.as_ref().and_then(|path| path.parent()) {
            chooser = chooser.set_directory(directory);
        }
        if let Some(handle) = pollster::block_on(chooser.save_file()) {
            *slot.lock().unwrap() = Some(DialogResult::Save(handle.path().to_path_buf()));
        }
    });
}

/// `Ctrl+N`, `Ctrl+S`, `Ctrl+Shift+S`, and the undo pair, for the prop editor.
///
/// The map editor's undo has always had this; the prop editor's lived only on
/// two buttons in the Features panel, so `Ctrl+Z` did nothing at all — which
/// reads as a bug rather than as an absence.
///
/// The file half goes through `handle_files`, which is what the File menu
/// feeds: a chord and the menu item it is the shortcut for have to be the same
/// action, and the way to guarantee that is for them to *be* the same call.
fn prop_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    egui: Res<bevy_egui::input::EguiWantsInput>,
    dialog: Res<PropFileDialog>,
    mut editor: ResMut<PropEditor>,
    mut frame: MessageWriter<FrameTheProp>,
) {
    let typing = egui.wants_keyboard_input();
    let chords = chords(&keys, typing);
    // Through the menu's own handler, so the chord and the entry it is the
    // shortcut for are one action.
    handle_edit(
        match (chords.redo, chords.undo) {
            (true, _) => Some(EditAction::Redo),
            (false, true) => Some(EditAction::Undo),
            _ => None,
        },
        &mut editor,
    );
    // `F` frames the selection, or the whole prop with nothing picked —
    // `FrameTheProp` falls back by itself. A bare key, so it is answered only
    // while nobody is typing a name.
    if !typing && keys.just_pressed(KeyCode::KeyF) {
        frame.write(FrameTheProp { selected: true });
    }
    if let Some(action) = FileAction::from_chords(chords) {
        handle_files(Some(action), &mut editor, &dialog);
    }
}

/// New, open, save, including the file dialog on its own thread.
///
/// Takes the action rather than the whole request bundle, because the
/// **animation editor** calls it too: a prop's clips live in its `.gpp`, so
/// *Save* there is this save, and two answers to "where does this prop go"
/// would be two that drift.
///
/// Quit is not here. It is a message, not a file, and this function has no
/// writer — see the caller.
pub(crate) fn handle_files(
    action: Option<FileAction>,
    editor: &mut PropEditor,
    dialog: &PropFileDialog,
) {
    if action == Some(FileAction::New) {
        editor.open(PropDoc::new("prop.untitled"), None);
    }

    if let Some(result) = dialog.result.lock().unwrap().take() {
        match result {
            DialogResult::Save(path) => match document::save(&path, editor.doc()) {
                Ok(()) => {
                    info!("Saved prop to {path:?}");
                    editor.mark_saved(path);
                }
                Err(e) => error!("Saving the prop failed: {e}"),
            },
            DialogResult::Open(path) => match std::fs::read_to_string(&path) {
                Ok(text) => match document::parse(&text) {
                    Ok(doc) => {
                        info!("Opened prop {path:?}");
                        editor.open(doc, Some(path));
                    }
                    Err(e) => error!("{}: {e}", path.display()),
                },
                Err(e) => error!("{}: {e}", path.display()),
            },
        }
    }

    if action == Some(FileAction::Save) {
        save_or_ask(editor, dialog);
    } else if action == Some(FileAction::SaveAs) {
        ask_where_to_save(editor, dialog);
    }

    if action == Some(FileAction::Open) {
        let slot = dialog.result.clone();
        std::thread::spawn(move || {
            let chooser = rfd::AsyncFileDialog::new().add_filter("Grackle Prop", &[PROP_EXTENSION]);
            if let Some(handle) = pollster::block_on(chooser.pick_file()) {
                *slot.lock().unwrap() = Some(DialogResult::Open(handle.path().to_path_buf()));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fit always stands somebody up and Model gets back what it had — so a
    /// trip to Fit does not leave a body in the modelling view, and the class
    /// being fitted survives a trip back to Model.
    #[test]
    fn workspaces_hand_the_figure_back() {
        let mut layout = PropPanels::default();
        let mut figure = ScaleFigure(None);

        layout.switch_to(Workspace::Fit, &mut figure);
        assert_eq!(figure.0, Some(Class::Mercenary), "Fit is never fitted to nobody");
        let other = Class::iter().find(|class| *class != Class::Mercenary);
        figure.0 = other.or(Some(Class::Mercenary));
        let fitted = figure.0;

        layout.switch_to(Workspace::Model, &mut figure);
        assert_eq!(figure.0, None, "Model gets its own choice back");

        layout.switch_to(Workspace::Fit, &mut figure);
        assert_eq!(figure.0, fitted, "Fit remembers who it was fitting");
    }
}
