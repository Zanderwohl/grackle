//! The prop editor's panels.
//!
//! Its own `DockState` rather than tabs on
//! [`EditorPanels`](crate::editor::panels::EditorPanels), which is already at
//! Bevy's parameter limit. Laid out like the map editor so swapping modes does
//! not rearrange the window.
//!
//! Writes go through [`PropEditor::begin_gesture`] and
//! [`PropEditor::end_gesture`] rather than `edit`, because egui reports a drag
//! as a change every frame. The gesture closes when the pointer and the
//! keyboard are both idle, so typing a name is one undo step.

use std::collections::BTreeMap;
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
use crate::common::file_menu::{dialog_directory, file_menu, FileAction};
use crate::common::net::NetRole;
use crate::editor::multicam::{Insets, MulticamState};
use crate::get;
use crate::prop::archetype::{Archetype, Collider, ColliderShape, PickupKind};
use crate::prop::document::{self, Pick, PropDoc, PropEditor, PROP_EXTENSION};
use crate::prop::plane::{OriginPlane, PickedPlane, Plane, PlaneRef};
use crate::prop::reference::{Element, GeomRef, Located};
use crate::prop::sketch::Sketch;
use crate::common::class::Class;
use crate::common::rotation::{aim_and_spin, euler_from_quat, from_aim_and_spin};
use crate::common::shortcuts::chords;
use crate::prop::feature::{Axis, BooleanOp, FeatureOp, PatternKind, PropFeatureId};
use crate::prop::figure::ScaleFigure;
use crate::prop::camera::ViewmodelPreview;
use crate::prop::gizmo::HoldHandles;
use crate::prop::hold::MuzzleSpec;
use crate::prop::profile::{Placement, Profile, MIN_SIDES};
use crate::prop::solid::Shape;
use crate::prop::surface::{Style, Surface};
use crate::prop::camera::FrameTheProp;
use crate::prop::view::PropBuild;

/// What a file dialog came back with. Uses a background thread like the map
/// editor, so it has the same wasm problem; reading props does not come
/// through here.
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

/// Which job the editor is set up for. Model has the verbs, the inspector and
/// the feature handles; Fit has the hold editor, the hold's handles and a body
/// holding the prop. The two sets of handles both sit at the prop's origin, so
/// only one can be up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Workspace {
    #[default]
    Model,
    Fit,
}

#[derive(Resource)]
pub struct PropPanels {
    left: DockState<PropTab>,
    right_model: DockState<PropTab>,
    right_fit: DockState<PropTab>,
    bottom: DockState<PropTab>,
    workspace: Workspace,
    /// The reference figure Model had before Fit stood a body up.
    model_figure: Option<Class>,
    /// The class Fit last showed, restored on returning to Fit.
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
    /// Fit always has a body; Model gets back the figure it had.
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
    file: Option<FileAction>,
    /// `true` frames the selection rather than the whole prop.
    frame: Option<bool>,
    mode: Option<AppMode>,
    workspace: Option<Workspace>,
    edit: Option<EditAction>,
    /// Something that would use the picked plane is hovered.
    preview_plane: bool,
}

struct PropTabs<'a> {
    editor: &'a mut PropEditor,
    build: &'a PropBuild,
    plane: Option<Plane>,
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
            PropTab::Hold => match self.editor.doc().archetype {
                Archetype::Hat => get!("prop.panels.hat").into(),
                _ => get!("prop.panels.hold").into(),
            },
            // The count in the title, so problems show while the tab is behind another.
            PropTab::Problems => match self.build.evaluated.problems.len() {
                0 => get!("prop.panels.problems").into(),
                count => get!("prop.panels.problems_count", "count", count).into(),
            },
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        match tab {
            PropTab::Features => feature_tree(ui, self.editor, self.build),
            PropTab::Inspector => {
                inspector(ui, self.editor, self.build, self.plane, &mut self.requests.preview_plane)
            }
            PropTab::Hold => match self.editor.doc().archetype {
                Archetype::Held => {
                    let front = self.build.evaluated.bounds().map(front_of);
                    hold(ui, self.editor, *self.figure, self.preview, front)
                }
                Archetype::Hat => hat_fit(ui, self.editor, *self.figure),
                // Nothing holds or wears a placed prop; the figure is there
                // for scale.
                _ => {}
            },
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
            // Left in, the animation editor would build the prop missing its
            // last features with nothing on screen saying so. `OnExit` runs
            // before the next mode's `OnEnter` rebuild.
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
    mut plane: ResMut<PickedPlane>,
    mut figure: ResMut<ScaleFigure>,
    mut handles: ResMut<HoldHandles>,
    mut preview: ResMut<ViewmodelPreview>,
    mut multicam: ResMut<MulticamState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    dialog: Res<PropFileDialog>,
    mut frame: MessageWriter<FrameTheProp>,
    mut next_mode: ResMut<NextState<AppMode>>,
    // Optional: a build with no `NetPlugin` is a closed game, its own authority.
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

    // Off unless this frame's Hold tab turns it back on, so a tab closed
    // mid-drag cannot leave the preview stuck on.
    if preview.0 {
        preview.0 = false;
    }

    let mut requests = PropRequests::default();
    let mut tabs = PropTabs {
        editor: &mut editor,
        build: &build,
        plane: plane.plane,
        // Not cloned and written back: a write every frame would rebuild the
        // figure's rig every frame.
        figure: &mut figure,
        preview: &mut preview,
        requests: &mut requests,
    };
    let workspace = layout.workspace;

    layout.menu_bar_height = egui::Panel::top("prop_menu_bar")
        .resizable(false)
        .show(&mut viewport_ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                file_menu(ui, &mut tabs.requests.file);
                edit_menu(ui, edit_availability(tabs.editor), &mut tabs.requests.edit);
                view_menu(ui, workspace, tabs.figure);
                // This system is gated on `AppMode::Prop`.
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
                // First and identical in both workspaces, so it does not move.
                for (candidate, key) in
                    [(Workspace::Model, "prop.workspace.model"), (Workspace::Fit, "prop.workspace.fit")]
                {
                    if ui.selectable_label(workspace == candidate, get!(key)).clicked() {
                        tabs.requests.workspace = Some(candidate);
                    }
                }
                ui.separator();
                match workspace {
                    Workspace::Model => {
                        toolbar(ui, tabs.editor, tabs.build, tabs.plane, tabs.requests)
                    }
                    Workspace::Fit => fit_toolbar(ui, tabs.figure, &mut tabs.requests.frame),
                }
            });
        })
        .response
        .rect
        .height();

    let dock_frame = crate::common::dock::dock_panel_frame(viewport_ui.style());
    layout.left_width = egui::Panel::left("prop_left")
        .resizable(true)
        .frame(dock_frame)
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
        .frame(dock_frame)
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

    // Before the dock's bottom panel, so it sits beneath it.
    let status_height = egui::Panel::bottom("prop_status")
        .resizable(false)
        .show(&mut viewport_ui, |ui| status_line(ui, tabs.editor, tabs.build))
        .response
        .rect
        .height();

    layout.bottom_height = egui::Panel::bottom("prop_bottom")
        .resizable(true)
        .frame(dock_frame)
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

    if plane.previewed != requests.preview_plane {
        plane.previewed = requests.preview_plane;
    }

    if let Some(to) = requests.workspace {
        layout.switch_to(to, &mut figure);
    }
    // Derived from the workspace rather than ticked: in Fit the viewport's
    // handles are the hold's, in Model the selected feature's.
    let fitting = layout.workspace == Workspace::Fit
        && matches!(editor.doc().archetype, Archetype::Held | Archetype::Hat);
    if handles.0 != fitting {
        handles.0 = fitting;
    }

    if let Some(selected) = requests.frame {
        frame.write(FrameTheProp { selected });
    }
    // `mode_menu` never offers the current mode, so this is always a change.
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

/// One line of the feature list, copied out before drawing so the closures
/// need not borrow the editor.
struct Row {
    id: PropFeatureId,
    name: String,
    enabled: bool,
    /// How many features name this one as an operand.
    dependants: usize,
    /// Operands and dependants: what this may not be dragged past.
    pins: Vec<PropFeatureId>,
    /// Indices in the finished list.
    legal: std::ops::RangeInclusive<usize>,
    operands: Vec<(PropFeatureId, String)>,
}

/// The ordered feature list.
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
                pins: feature.op.depends_on().into_iter().chain(dependants).collect(),
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

    // Copied out, and changes deferred to after the closures, so `editor` is
    // not borrowed across them.
    let selected = editor.primary_feature(&build.evaluated);
    let also_selected: Vec<PropFeatureId> =
        editor.picked_features(&build.evaluated).into_iter().filter(|id| Some(*id) != selected).collect();
    let mut select: Option<(PropFeatureId, RowClick)> = None;
    let mut pending: Option<Box<dyn FnOnce(&mut PropEditor)>> = None;

    // Rebuilt every frame rather than kept, so there is no drag state to get
    // out of step.
    let mut rects: Vec<(PropFeatureId, egui::Rect)> = Vec::new();
    let mut dragging: Option<usize> = None;
    let mut dropped = false;

    // The rollback bar sits after this row; with no rollback, after the last.
    let rollback = editor.rollback();
    let bar_after = rollback
        .and_then(|id| entries.iter().position(|row| row.id == id))
        .unwrap_or(entries.len().saturating_sub(1));
    let mut bar_dragged = false;

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (at, row) in entries.iter().enumerate() {
            let Row { id, enabled, dependants, .. } = *row;
            let name = &row.name;

            // Reserved before the row and filled after: the fill depends on a
            // hover the row has not reported yet, and painting later without
            // reserving would cover the name.
            let background = ui.painter().add(egui::Shape::Noop);

            // The row is the widget; the name is plain text. egui hit-tests
            // only widgets that sense clicks, so a hover-only label lets clicks
            // through to the row (a `SelectableLabel` swallowed the right
            // click). A `Ui`'s own response is `Sense::hover()`, which can
            // never report the `secondary_clicked()` a context menu opens on,
            // hence the `.interact`.
            let past_the_bar = at > bar_after;
            let response = ui
                .horizontal(|ui| {
                    // Faded, since struck and weak already mean suppressed
                    // and eaten.
                    if past_the_bar {
                        ui.multiply_opacity(0.4);
                    }
                    // Struck through: suppressed. Weak: eaten by a later
                    // feature. Orange: broken.
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

                    // A selectable label senses click-and-drag, which puts it
                    // back in the hit-test ahead of the row.
                    ui.add(egui::Label::new(text).selectable(false));
                    // Otherwise `horizontal` shrinks to the text, and so does
                    // the grabbable area.
                    ui.allocate_space(egui::vec2(ui.available_width(), 0.0));

                })
                .response
                .interact(egui::Sense::click_and_drag());

            rects.push((id, response.rect));
            // `dragged` stays true for the row the drag began on after the
            // pointer leaves it.
            if response.dragged() {
                dragging = Some(at);
            }
            if response.drag_stopped() {
                dragging = Some(at);
                dropped = true;
            }
            if response.clicked() {
                let extending = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                select = Some((id, if extending { RowClick::Toggle } else { RowClick::Only }));
            }

            // Selection outranks hover, or a chosen row dims when pointed at.
            let fill = if response.dragged() {
                Some(ui.visuals().widgets.active.weak_bg_fill)
            } else if selected == Some(id) {
                Some(ui.visuals().selection.bg_fill)
            } else if also_selected.contains(&id) {
                // Picked but not primary.
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
                // Right-click selects, unless already in the selection, so a
                // Ctrl-clicked pair is not thrown away.
                if selected != Some(id) && !also_selected.contains(&id) {
                    select = Some((id, RowClick::Only));
                }

                // Names the action, not the state: a state label reads as a
                // checkbox with no box.
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
                            // Keep the copy above the bar too.
                            if editor.rollback() == Some(id) {
                                editor.roll_back_to(Some(copy));
                            }
                            editor.select(copy);
                        }
                    }));
                }

                // Here rather than in the toolbar because it acts on an
                // existing body. Starts from the body's current surface, so
                // adding it changes nothing yet.
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
                // Said before deleting rather than as warnings after.
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

            // What this feature ate, listed under it. The rows stay in document
            // order, which reordering needs.
            for (operand, operand_name) in &row.operands {
                ui.horizontal(|ui| {
                    if past_the_bar {
                        ui.multiply_opacity(0.4);
                    }
                    ui.add_space(16.0);
                    let text = egui::RichText::new(format!("· {operand_name}")).weak().small();
                    let line = ui.add(egui::Label::new(text).selectable(false).sense(egui::Sense::click()));
                    if line.clicked() {
                        select = Some((*operand, RowClick::Only));
                    }
                });
            }

            // The rollback bar. Bold whenever it is not at the end, so a
            // partial view is not mistaken for the whole prop.
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

        // Applied live while dragging. Never above the first feature: an empty
        // viewport shows nothing useful.
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

        // After every row has reported its rect.
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
        Some((id, RowClick::Only)) => editor.select(id),
        Some((id, RowClick::Toggle)) => editor.toggle_selected(id),
        None => {}
    }
    if let Some(action) = pending {
        action(editor);
    }
}

#[derive(Clone, Copy)]
enum RowClick {
    Only,
    /// Ctrl/Cmd held.
    Toggle,
}

/// Draw where a dragged feature would land, and mark the features pinning it.
///
/// Returns the drop index clamped into the legal range, which is one
/// contiguous span (see `PropDoc::legal_range`), so clamping is enough.
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
            // A bar rather than an outline, so it does not clash with the
            // selection highlight.
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

/// The verbs under the menu bar. They act on the selection: Combine takes the
/// two bodies picked (first as target, second as tool), Mirror and Pattern the
/// primary.
///
/// Framing is here rather than in a menu so it is one click away after losing
/// the prop.
fn toolbar(
    ui: &mut Ui,
    editor: &mut PropEditor,
    build: &PropBuild,
    plane: Option<Plane>,
    requests: &mut PropRequests,
) {
    let pair = editor.pair_to_combine(&build.evaluated);
    let single = editor.body_to_act_on(&build.evaluated);
    let mut added: Option<FeatureOp> = None;
    let mut created_plane: Option<Vec<GeomRef>> = None;

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

        let through = plane_through_picks(editor, plane);
        let plane_tool = ui.add_enabled(through.is_some(), egui::Button::new(get!("prop.features.plane")));
        requests.preview_plane |= plane_tool.hovered();
        if let (true, Some(through)) = (plane_tool.clicked(), through) {
            created_plane = Some(through);
        }
        let on = match editor.picks() {
            [Pick::Plane(on)] => Some(*on),
            _ => None,
        };
        let sketch_tool = ui.add_enabled(on.is_some(), egui::Button::new(get!("prop.features.sketch")));
        if let (true, Some(on)) = (sketch_tool.clicked(), on) {
            added = Some(FeatureOp::Sketch { on, sketch: Sketch::default() });
        }

        ui.separator();

        if ui.button(get!("prop.menu.frame")).clicked() {
            requests.frame = Some(false);
        }
        let framable = build.evaluated.bounds_of_bodies(&editor.picked_bodies(&build.evaluated)).is_some();
        if ui
            .add_enabled(framable, egui::Button::new(get!("prop.menu.frame_selected")))
            .clicked()
        {
            requests.frame = Some(true);
        }
    });

    if let Some(op) = added {
        let id = editor.add(op);
        editor.select(id);
    }
    if let Some(through) = created_plane {
        add_plane(editor, through);
    }
}

/// What a new plane would be made through: the picks, when they make a plane
/// and are all geometry. A lone plane pick is a plane but makes no new one —
/// it would be the same plane again. The toolbar's Plane and the Inspector's
/// Use plane both ask this, so they are offered together.
fn plane_through_picks(editor: &PropEditor, plane: Option<Plane>) -> Option<Vec<GeomRef>> {
    plane?;
    editor
        .picks()
        .iter()
        .map(|pick| match pick {
            Pick::Geom(reference) => Some(reference.clone()),
            Pick::Body(_) | Pick::Plane(_) => None,
        })
        .collect()
}

fn add_plane(editor: &mut PropEditor, through: Vec<GeomRef>) {
    let id = editor.add(FeatureOp::Plane { through });
    editor.select_pick(Pick::Plane(PlaneRef::Feature(id)));
}

pub(crate) fn edit_availability(editor: &PropEditor) -> EditAvailability {
    EditAvailability {
        undo: editor.can_undo(),
        redo: editor.can_redo(),
        revert: editor.path.is_some() && editor.dirty,
    }
}

/// Answers both the Edit menu and its chords. Shared with the animation editor,
/// whose clips live in the same file.
pub(crate) fn handle_edit(action: Option<EditAction>, editor: &mut PropEditor) {
    match action {
        Some(EditAction::Undo) => editor.undo(),
        Some(EditAction::Redo) => editor.redo(),
        Some(EditAction::Revert) => revert_from_disk(editor),
        None => {}
    }
}

/// One undo step. `std::fs` is fine here because this is the authoring side;
/// the game reads props through `Assets`.
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

/// The selection as a list when there is more than one pick, then the
/// primary's details: a feature's numbers, a corner's or edge's whereabouts,
/// or the prop's own numbers when nothing is picked.
fn inspector(
    ui: &mut Ui,
    editor: &mut PropEditor,
    build: &PropBuild,
    plane: Option<Plane>,
    preview_plane: &mut bool,
) {
    let listed = editor.picks().len() > 1;
    if listed {
        selection_list(ui, editor, build);
    }
    let through = plane_through_picks(editor, plane);
    let offered = through.is_some();
    if let Some(through) = through {
        let use_plane = ui.button(get!("prop.picks.use_plane"));
        *preview_plane |= use_plane.hovered();
        if use_plane.clicked() {
            add_plane(editor, through);
        }
    }
    if listed || offered {
        ui.separator();
    }
    match editor.primary().cloned() {
        None => prop_overview(ui, editor, build),
        Some(Pick::Body(id) | Pick::Plane(PlaneRef::Feature(id))) => feature_inspector(ui, editor, build, id),
        Some(Pick::Plane(PlaneRef::Origin(which))) => origin_plane_inspector(ui, which),
        Some(Pick::Geom(reference)) => {
            element_inspector(ui, editor, build, &reference);
            // A face stands for its body, so the body's numbers follow.
            if let Some(body) = Pick::Geom(reference).body(&build.evaluated) {
                feature_inspector(ui, editor, build, body);
            }
        }
    }
}

/// Every pick, oldest first, each with a button that drops it.
fn selection_list(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild) {
    ui.label(egui::RichText::new(get!("prop.inspector.picked", "count", editor.picks().len())).weak());
    let primary = editor.primary().cloned();
    let mut dropped = None;
    egui::ScrollArea::vertical().id_salt("prop_picks").max_height(160.0).show(ui, |ui| {
        for pick in editor.picks() {
            ui.horizontal(|ui| {
                let text = egui::RichText::new(pick_label(editor, build, pick));
                ui.label(match Some(pick) == primary.as_ref() {
                    true => text.strong(),
                    false => text,
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(get!("prop.picks.remove")).clicked() {
                        dropped = Some(pick.clone());
                    }
                });
            });
        }
    });
    if let Some(pick) = dropped {
        editor.unpick(&pick);
    }
}

/// A feature's name, or a corner or edge as the feature it came from and the
/// copies it went through. Counted from one, as a person counts corners.
fn pick_label(editor: &PropEditor, build: &PropBuild, pick: &Pick) -> String {
    let name = |id: PropFeatureId| {
        editor
            .doc()
            .feature(id)
            .map(|feature| feature.name.clone())
            .unwrap_or_else(|| get!("prop.inspector.missing", "id", id))
    };
    let reference = match pick {
        Pick::Body(id) | Pick::Plane(PlaneRef::Feature(id)) => return name(*id),
        Pick::Plane(PlaneRef::Origin(which)) => return which.name(),
        Pick::Geom(reference) => reference,
    };
    let source = reference.source.copies.iter().fold(name(reference.source.feature), |base, copy| {
        get!("prop.picks.copy", "base", base, "by", name(copy.by), "index", copy.index)
    });
    let label = match reference.element {
        Element::Point(index) => get!("prop.picks.point", "source", source, "index", index + 1),
        Element::Edge(index) => get!("prop.picks.edge", "source", source, "index", index + 1),
        Element::Face(index) => get!("prop.picks.face", "source", source, "index", index + 1),
    };
    match build.evaluated.locate(reference) {
        Some(_) => label,
        None => get!("prop.picks.absent", "pick", label),
    }
}

/// Where a corner, edge or face is. Read-only: it is wherever its feature put
/// it.
fn element_inspector(ui: &mut Ui, editor: &PropEditor, build: &PropBuild, reference: &GeomRef) {
    let triple = |v: Vec3| format!("{:.4}, {:.4}, {:.4} m", v.x, v.y, v.z);
    section(ui, "element", get!("prop.inspector.sections.element"), |ui| {
        ui.label(egui::RichText::new(pick_label(editor, build, &Pick::Geom(reference.clone()))).strong());
        if let Some(body) = build.evaluated.body_of(reference) {
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.on_body"));
                ui.label(pick_label(editor, build, &Pick::Body(body)));
            });
        }
        match build.evaluated.locate(reference) {
            Some(Located::Point(p)) => {
                ui.horizontal(|ui| {
                    ui.label(get!("prop.inspector.position"));
                    ui.label(triple(p));
                });
            }
            Some(Located::Edge(a, b)) => {
                ui.horizontal(|ui| {
                    ui.label(get!("prop.inspector.from"));
                    ui.label(triple(a));
                });
                ui.horizontal(|ui| {
                    ui.label(get!("prop.inspector.to"));
                    ui.label(triple(b));
                });
                ui.horizontal(|ui| {
                    ui.label(get!("prop.inspector.length"));
                    ui.label(format!("{:.4} m", a.distance(b)));
                });
            }
            Some(Located::Face { normal, .. }) => {
                ui.horizontal(|ui| {
                    ui.label(get!("prop.inspector.normal"));
                    ui.label(format!("{:.4}, {:.4}, {:.4}", normal.x, normal.y, normal.z));
                });
            }
            None => {}
        }
    });
}

/// An origin plane is not a feature: nothing about it can be edited.
fn origin_plane_inspector(ui: &mut Ui, which: OriginPlane) {
    let plane = which.plane();
    let triple = |v: Vec3| format!("{:.4}, {:.4}, {:.4}", v.x, v.y, v.z);
    section(ui, "placement", get!("prop.inspector.sections.placement"), |ui| {
        ui.label(egui::RichText::new(which.name()).strong());
        ui.horizontal(|ui| {
            ui.label(get!("prop.inspector.normal"));
            ui.label(triple(*plane.normal));
        });
    });
}

/// The selected feature's numbers.
///
/// Sections come in a fixed order so a number is in the same place on every
/// kind of feature. Section ids are per section, not per feature, so folding
/// one folds it everywhere.
fn feature_inspector(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild, selected: PropFeatureId) {
    let Some(index) = editor.doc().index_of(selected) else {
        prop_overview(ui, editor, build);
        return;
    };
    // What was live when this feature ran, as recorded by the evaluator. Past
    // the rollback bar there is no record, so everything above the feature
    // stands in; re-evaluating here would run the kernel every frame.
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

    // Drawn against a copy: calling `PropEditor::doc_mut` marks the document
    // changed, which would rebuild the viewport and mark it dirty every frame.
    let Some(mut edited) = editor.doc().feature(selected).cloned() else { return };
    let through: Vec<String> = match &edited.op {
        FeatureOp::Plane { through } => {
            through.iter().map(|reference| pick_label(editor, build, &Pick::Geom(reference.clone()))).collect()
        }
        _ => vec![],
    };
    let datum = build.evaluated.datum(PlaneRef::Feature(selected));
    let plane_name = |on: PlaneRef| pick_label(editor, build, &Pick::Plane(on));
    let sketch_plane = match &edited.op {
        FeatureOp::Sketch { on, .. } => Some(plane_name(*on)),
        _ => None,
    };
    let feature = &mut edited;

    let mut changed = false;
    egui::ScrollArea::vertical().show(ui, |ui| {
        section(ui, "feature", get!("prop.inspector.sections.feature"), |ui| {
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.kind"));
                ui.label(feature.op.type_name());
            });
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.name"));
                changed |= ui.text_edit_singleline(&mut feature.name).changed();
            });
            // Positive, unlike the tree's "Suppress": a menu item names an
            // action, a checkbox names a state.
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
            FeatureOp::Sketch { sketch, .. } => {
                section(ui, "drawing", get!("prop.inspector.sections.drawing"), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(get!("prop.inspector.on_plane"));
                        ui.label(sketch_plane.clone().unwrap_or_default());
                    });
                    ui.label(get!(
                        "prop.inspector.drawing",
                        "points",
                        sketch.points.len(),
                        "lines",
                        sketch.entities.len()
                    ));
                });
            }
            FeatureOp::Plane { .. } => {
                section(ui, "through", get!("prop.inspector.sections.through"), |ui| {
                    for label in &through {
                        ui.label(label);
                    }
                });
                if let Some(datum) = datum {
                    let triple = |v: Vec3| format!("{:.4}, {:.4}, {:.4}", v.x, v.y, v.z);
                    section(ui, "placement", get!("prop.inspector.sections.placement"), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(get!("prop.inspector.origin"));
                            ui.label(triple(datum.plane.origin));
                        });
                        ui.horizontal(|ui| {
                            ui.label(get!("prop.inspector.normal"));
                            ui.label(triple(*datum.plane.normal));
                        });
                    });
                }
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

fn section(ui: &mut Ui, id: &str, title: String, body: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(egui::RichText::new(title).strong())
        .id_salt(("prop_section", id))
        .default_open(true)
        .show(ui, body);
}

/// What the inspector shows with nothing picked: the prop itself.
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
            // Resolved, so a mistyped key shows as `<key>` here.
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.shown_as"));
                ui.label(egui::RichText::new(get!(name_key)).italics());
            });
            ui.horizontal(|ui| {
                ui.label(get!("prop.inspector.file"));
                ui.label(editor.saved_name().map(str::to_owned).unwrap_or_else(|| get!("prop.untitled")));
            });
        });
        archetype_sections(ui, editor);
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

/// What the prop is to the game, and the sections that archetype reads. The
/// others are kept in the file but not shown.
fn archetype_sections(ui: &mut Ui, editor: &mut PropEditor) {
    section(ui, "archetype", get!("prop.inspector.sections.archetype"), |ui| {
        let mut archetype = editor.doc().archetype;
        egui::ComboBox::from_id_salt("prop_archetype")
            .selected_text(archetype.name())
            .show_ui(ui, |ui| {
                for candidate in Archetype::ALL {
                    ui.selectable_value(&mut archetype, candidate, candidate.name());
                }
            });
        if archetype != editor.doc().archetype {
            editor.edit(|doc| doc.archetype = archetype);
        }
    });

    let archetype = editor.doc().archetype;
    if archetype == Archetype::Breakable {
        section(ui, "breakable", get!("prop.inspector.sections.breakable"), |ui| {
            let mut health = editor.doc().breakable.health;
            let changed = ui
                .horizontal(|ui| {
                    ui.label(get!("prop.breakable.health"));
                    ui.add(egui::DragValue::new(&mut health).speed(1.0).range(1..=100_000)).changed()
                })
                .inner;
            if changed {
                editor.begin_gesture();
                editor.doc_mut().breakable.health = health;
            }
        });
    }
    if archetype == Archetype::Pickup {
        section(ui, "pickup", get!("prop.inspector.sections.pickup"), |ui| pickup_ui(ui, editor));
    }
    if archetype.is_placed() {
        section(ui, "colliders", get!("prop.inspector.sections.colliders"), |ui| {
            colliders_ui(ui, editor)
        });
    }
}

fn pickup_ui(ui: &mut Ui, editor: &mut PropEditor) {
    let mut pickup = editor.doc().pickup;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(get!("prop.inspector.kind"));
        egui::ComboBox::from_id_salt("prop_pickup_kind")
            .selected_text(pickup.kind.name())
            .show_ui(ui, |ui| {
                for kind in PickupKind::ALL {
                    changed |= ui.selectable_value(&mut pickup.kind, kind, kind.name()).changed();
                }
            });
    });
    let mut percent = pickup.amount * 100.0;
    // Written back only when dragged: through `* 100 / 100` an `f32` need not
    // come back as itself, and the file would gain digits nobody typed.
    if ui
        .horizontal(|ui| {
            ui.label(get!("prop.pickup.amount"));
            ui.add(egui::DragValue::new(&mut percent).speed(1.0).range(0.0..=100.0).suffix("%"))
                .changed()
        })
        .inner
    {
        pickup.amount = percent / 100.0;
        changed = true;
    }
    changed |= ui
        .horizontal(|ui| {
            ui.label(get!("prop.pickup.respawn"));
            ui.add(egui::DragValue::new(&mut pickup.respawn).speed(0.1).range(0.0..=600.0).suffix("s"))
                .changed()
        })
        .inner;
    if matches!(pickup.kind, PickupKind::Armor | PickupKind::Flag) {
        ui.label(egui::RichText::new(get!("prop.pickup.unbuilt")).weak());
    }
    if changed {
        editor.begin_gesture();
        editor.doc_mut().pickup = pickup;
    }
}

/// Each collider's shape, where it is and how big. Adding and removing are
/// whole steps; dragging a number is a gesture.
fn colliders_ui(ui: &mut Ui, editor: &mut PropEditor) {
    let mut colliders = editor.doc().colliders.clone();
    let mut changed = false;
    let mut remove = None;

    for (index, collider) in colliders.iter_mut().enumerate() {
        ui.push_id(("prop_collider", index), |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("shape")
                    .selected_text(collider.shape.name())
                    .show_ui(ui, |ui| {
                        for candidate in ColliderShape::defaults() {
                            let same = std::mem::discriminant(&candidate)
                                == std::mem::discriminant(&collider.shape);
                            if ui.selectable_label(same, candidate.name()).clicked() && !same {
                                collider.shape = candidate;
                                changed = true;
                            }
                        }
                    });
                if ui.button(get!("prop.inspector.remove_point")).on_hover_text(get!("prop.collider.remove")).clicked() {
                    remove = Some(index);
                }
            });
            changed |= vec3_ui(ui, get!("prop.inspector.origin"), &mut collider.origin, 0.005, "m");
            match &mut collider.shape {
                ColliderShape::Box { size } => {
                    changed |= vec3_ui(ui, get!("prop.inspector.size"), size, 0.005, "m");
                }
                ColliderShape::Sphere { radius } => {
                    changed |= drag(ui, get!("prop.inspector.radius"), radius, 0.005, "m");
                }
                ColliderShape::Capsule { radius, height } => {
                    changed |= drag(ui, get!("prop.inspector.radius"), radius, 0.005, "m");
                    changed |= drag(ui, get!("prop.inspector.height"), height, 0.005, "m");
                }
            }
            ui.separator();
        });
    }

    if let Some(index) = remove {
        editor.edit(|doc| {
            doc.colliders.remove(index);
        });
        return;
    }
    if ui.button(get!("prop.collider.add")).clicked() {
        editor.edit(|doc| {
            doc.colliders.push(Collider {
                origin: [0.0, 0.25, 0.0],
                shape: ColliderShape::Box { size: [0.5; 3] },
            })
        });
        return;
    }
    if changed {
        editor.begin_gesture();
        editor.doc_mut().colliders = colliders;
    }
}

/// How a hat sits on the class standing in the viewport. The override
/// checkbox edits that class's entry, as the Hold panel's does.
fn hat_fit(ui: &mut Ui, editor: &mut PropEditor, figure: ScaleFigure) {
    let base = editor.doc().hat.clone();
    let mut edited = base.for_class(figure.0);
    let mut changed = false;

    egui::ScrollArea::vertical().show(ui, |ui| {
        if let Some(class) = figure.0
            && let Some(per_class) = override_toggle(ui, "hat_class", class, &base.per_class)
        {
            editor.edit(|doc| doc.hat.per_class = per_class);
        }
        section(ui, "hat_fit", get!("prop.hat.on_the_head"), |ui| {
            changed |= vec3_ui(ui, get!("prop.inspector.offset"), &mut edited.offset, 0.001, "m");
            changed |= degrees_ui(ui, get!("prop.inspector.rotation"), &mut edited.rotation);
            changed |= drag(ui, get!("prop.hat.scale"), &mut edited.scale, 0.005, "×");
        });
    });

    if changed {
        editor.begin_gesture();
        editor.doc_mut().hat = base.applied(figure.0, &edited);
    }
}

/// Who is standing in the viewport. Fit does not offer "nobody".
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

/// Fit's toolbar. The class picked here is both the figure and the per-class
/// override the Hold panel edits.
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

/// Metres ahead of the grip a muzzle starts on a prop with no geometry.
const EMPTY_MUZZLE_REACH: f32 = 0.3;

/// The middle of the bounds' `-Z` face: forward is `-Z`, so a barrel ends there.
fn front_of((min, max): (Vec3, Vec3)) -> Vec3 {
    let middle = (min + max) / 2.0;
    Vec3::new(middle.x, middle.y, min.z)
}

/// How this prop is held. The override checkbox edits the entry for the class
/// standing in the viewport.
fn hold(
    ui: &mut Ui,
    editor: &mut PropEditor,
    figure: ScaleFigure,
    preview: &mut ViewmodelPreview,
    front: Option<Vec3>,
) {
    let base = editor.doc().hold.clone();

    let mut changed = false;
    let mut edited = base.for_class(figure.0);
    // Not per class: a viewmodel is a screen framing, not a property of a body.
    let mut viewmodel = editor.doc().viewmodel;
    let mut view_changed = false;

    egui::ScrollArea::vertical().show(ui, |ui| {
        if let Some(class) = figure.0
            && let Some(per_class) = override_toggle(ui, "hold_class", class, &base.per_class)
        {
            editor.edit(|doc| doc.hold.per_class = per_class);
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

        section(ui, "hold_muzzle", get!("prop.hold.muzzle"), |ui| {
            let mut muzzle = editor.doc().muzzle;
            let mut has = muzzle.is_some();
            if ui.checkbox(&mut has, get!("prop.hold.has_muzzle")).changed() {
                // Not on the grip, whose handles would sit on top of its own
                // and win every tie.
                let at = front.unwrap_or(edited.grip() + Vec3::NEG_Z * EMPTY_MUZZLE_REACH);
                muzzle = has.then_some(MuzzleSpec { at: at.to_array() });
            }
            if let Some(muzzle) = muzzle.as_mut() {
                vec3_ui(ui, get!("prop.hold.at"), &mut muzzle.at, 0.001, "m");
            }
            if muzzle != editor.doc().muzzle {
                editor.begin_gesture();
                editor.doc_mut().muzzle = muzzle;
            }
        });

        section(ui, "hold_carry", get!("prop.hold.carry"), |ui| {
            // In arm lengths, not metres. Fine speed because the useful range
            // is under one arm.
            changed |= vec3_ui(ui, get!("prop.hold.offset"), &mut edited.carry, 0.005, "");
            changed |= degrees_ui(ui, get!("prop.inspector.rotation"), &mut edited.carry_rotation);
        });

        // Outside the fold, so a folded section leaves the preview off.
        let mut holding = false;
        section(ui, "hold_viewmodel", get!("prop.hold.viewmodel"), |ui| {
            // `across` is a fraction of the frustum, so it holds its place on
            // screen at any field of view or aspect. The preview is on only
            // while one of these numbers is held.
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

/// The "override for this class" checkbox the Hold and Hat panels share. The
/// per-class map to write back, when it was ticked or unticked.
fn override_toggle<T: Default + Clone>(
    ui: &mut Ui,
    id: &str,
    class: Class,
    per_class: &BTreeMap<String, T>,
) -> Option<BTreeMap<String, T>> {
    let key = Class::key(class);
    let mut toggled = None;
    section(ui, id, class.name(), |ui| {
        let mut on = per_class.contains_key(&key);
        if ui.checkbox(&mut on, get!("prop.hold.override_for", "class", class.name())).changed() {
            let mut per_class = per_class.clone();
            match on {
                true => {
                    per_class.insert(key.clone(), T::default());
                }
                false => {
                    per_class.remove(&key);
                }
            }
            toggled = Some(per_class);
        }
    });
    toggled
}

fn degrees_ui(ui: &mut Ui, label: String, radians: &mut [f32; 3]) -> bool {
    let mut degrees = radians.map(f32::to_degrees);
    if vec3_ui(ui, label, &mut degrees, 1.0, "°") {
        *radians = degrees.map(f32::to_radians);
        return true;
    }
    false
}

fn status_line(ui: &mut Ui, editor: &PropEditor, build: &PropBuild) {
    ui.horizontal(|ui| {
        let names: Vec<String> = editor.picks().iter().map(|pick| pick_label(editor, build, pick)).collect();
        ui.label(match names.is_empty() {
            true => get!("prop.inspector.nothing"),
            false => get!("prop.stats.selected", "names", names.join(", ")),
        });
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
        // Otherwise a viewport lagging a drag looks broken.
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
                    editor.primary_feature(&build.evaluated) == Some(problem.feature),
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

/// As [`drag`], also returning whether the box is held.
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

/// Cosine above which an aim lights an axis button; an aim between the axes
/// lights none.
const ON_AXIS_COS: f32 = 0.9999;

/// Orientation is edited as "points along" and "spin" about `main`, the
/// feature's length axis ([`FeatureOp::main_axis`]), because the Euler answer
/// to "point down −Z" depends on rotation order. The stored Euler angles are
/// behind a fold.
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
            let current = aim.dot(direction) > ON_AXIS_COS;
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
            // Degrees in the boxes, radians in the field.
            let mut degrees = placement.rotation.map(f32::to_degrees);
            if vec3_ui(ui, get!("prop.inspector.rotation"), &mut degrees, 1.0, "°") {
                placement.rotation = degrees.map(f32::to_radians);
                changed = true;
            }
        });
    changed
}

/// Snaps the residue of a quaternion round trip to zero, so the file says
/// `0.0` rather than `-0.00000004`.
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
        // Switching kind replaces rather than converts; there is nothing to
        // carry across.
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

/// Every copy is a boolean union, so this is a cost cap.
const MAX_PATTERN_COPIES: u32 = 64;

/// Switching between linear and radial replaces the layout, as with profiles.
fn pattern_ui(ui: &mut Ui, count: &mut u32, along: &mut PatternKind) -> bool {
    let mut changed = ui
        .horizontal(|ui| {
            ui.label(get!("prop.inspector.copies"));
            ui.add(egui::DragValue::new(count).speed(0.1).range(1..=MAX_PATTERN_COPIES)).changed()
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
            // Not the first entry: that would silently repoint the feature.
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

/// Write the prop to its path, or ask for one if it has none.
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

pub fn ask_where_to_save(editor: &PropEditor, dialog: &PropFileDialog) {
    let slot = dialog.result.clone();
    let existing = editor.path.clone();
    std::thread::spawn(move || {
        let mut chooser = rfd::AsyncFileDialog::new()
            .add_filter("Grackle Prop", &[PROP_EXTENSION])
            .set_file_name(format!("untitled.{PROP_EXTENSION}"));
        if let Some(directory) = dialog_directory(existing.as_deref(), document::PROPS_DIR) {
            chooser = chooser.set_directory(directory);
        }
        if let Some(handle) = pollster::block_on(chooser.save_file()) {
            *slot.lock().unwrap() = Some(DialogResult::Save(handle.path().to_path_buf()));
        }
    });
}

/// Chords go through the same handlers as the menus, so a chord and its menu
/// entry are one action.
fn prop_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    egui: Res<bevy_egui::input::EguiWantsInput>,
    dialog: Res<PropFileDialog>,
    mut editor: ResMut<PropEditor>,
    mut frame: MessageWriter<FrameTheProp>,
) {
    let typing = egui.wants_keyboard_input();
    let chords = chords(&keys, typing);
    handle_edit(
        match (chords.redo, chords.undo) {
            (true, _) => Some(EditAction::Redo),
            (false, true) => Some(EditAction::Undo),
            _ => None,
        },
        &mut editor,
    );
    // `FrameTheProp` falls back to the whole prop with nothing picked. A bare
    // key, so not while typing.
    if !typing && keys.just_pressed(KeyCode::KeyF) {
        frame.write(FrameTheProp { selected: true });
    }
    if let Some(action) = FileAction::from_chords(chords) {
        handle_files(Some(action), &mut editor, &dialog);
    }
}

/// New, open and save. Also called by the animation editor, since a prop's
/// clips live in its `.gpp`. Quit is handled by the caller.
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
        let from = dialog_directory(editor.path.as_deref(), document::PROPS_DIR);
        std::thread::spawn(move || {
            let mut chooser = rfd::AsyncFileDialog::new().add_filter("Grackle Prop", &[PROP_EXTENSION]);
            if let Some(from) = from {
                chooser = chooser.set_directory(from);
            }
            if let Some(handle) = pollster::block_on(chooser.pick_file()) {
                *slot.lock().unwrap() = Some(DialogResult::Open(handle.path().to_path_buf()));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if Fit can show nobody, if Model does not get its figure back, or
    /// if Fit forgets its class across a trip to Model.
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
