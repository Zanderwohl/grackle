//! The prop editor's panels.
//!
//! Its own `DockState` rather than tabs added to
//! [`EditorPanels`](crate::editor::panels::EditorPanels), for two reasons. The
//! map editor's panel system already carries most of the editor's resources
//! and is at Bevy's parameter limit, so a prop tab would have to arrive by
//! pushing something else out. And the two surfaces are never up at once —
//! they are modes — so sharing a dock would mean a tab set that has to be
//! swapped anyway. What is shared is the *shape*: the same egui panels around
//! the same four viewports, sized the same way, so the window does not
//! rearrange itself under somebody swapping modes.
//!
//! Everything that writes to the document goes through
//! [`PropEditor::begin_gesture`] and [`PropEditor::end_gesture`] rather than
//! through `edit`. egui reports a drag as a change every frame it moves, so
//! per-change undo steps would turn one pull on a radius into forty presses of
//! Ctrl+Z. The gesture closes when the pointer *and* the keyboard are both
//! idle, which is what makes typing a name one step rather than one per
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
use crate::common::net::NetRole;
use crate::editor::multicam::MulticamState;
use crate::get;
use crate::prop::document::{self, PropDoc, PropEditor, PROP_EXTENSION};
use crate::common::class::Class;
use crate::prop::feature::{Axis, BooleanOp, FeatureOp, PropFeatureId};
use crate::prop::figure::ScaleFigure;
use crate::prop::profile::{Placement, Profile, MIN_SIDES};
use crate::prop::solid::Shape;
use crate::prop::surface::{Style, Surface};
use crate::prop::camera::FrameTheProp;
use crate::prop::view::PropBuild;

/// What a file dialog came back with. Mirrors the map editor's arrangement,
/// including the background thread, and inherits its wasm problem — see
/// "Targeting wasm" in `CLAUDE.md`. The *reading* half of the prop format
/// deliberately does not go through here.
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
    Scene,
    Problems,
}

/// The panel layout, kept beside the map editor's own rather than in it.
#[derive(Resource)]
pub struct PropPanels {
    left: DockState<PropTab>,
    right: DockState<PropTab>,
    bottom: DockState<PropTab>,
    menu_bar_height: f32,
    left_width: f32,
    right_width: f32,
    bottom_height: f32,
}

impl Default for PropPanels {
    fn default() -> Self {
        Self {
            left: DockState::new(vec![PropTab::Features]),
            right: DockState::new(vec![PropTab::Inspector, PropTab::Scene]),
            bottom: DockState::new(vec![PropTab::Problems]),
            menu_bar_height: 0.0,
            left_width: 260.0,
            right_width: 300.0,
            bottom_height: 80.0,
        }
    }
}

/// What the panels asked for this frame, collected so the systems that answer
/// do not have to be reachable from inside an egui closure.
#[derive(Default)]
struct PropRequests {
    new: bool,
    open: bool,
    save: bool,
    save_as: bool,
    frame: bool,
    /// The mode picked from the **Mode** menu, if one was.
    mode: Option<AppMode>,
}

struct PropTabs<'a> {
    editor: &'a mut PropEditor,
    build: &'a PropBuild,
    figure: &'a mut ScaleFigure,
    requests: &'a mut PropRequests,
}

impl<'a> TabViewer for PropTabs<'a> {
    type Tab = PropTab;

    fn id(&mut self, tab: &mut Self::Tab) -> Id {
        Id::new(match tab {
            PropTab::Features => "prop_features",
            PropTab::Inspector => "prop_inspector",
            PropTab::Scene => "prop_scene",
            PropTab::Problems => "prop_problems",
        })
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        match tab {
            PropTab::Features => get!("prop.panels.features").into(),
            PropTab::Inspector => get!("prop.panels.inspector").into(),
            PropTab::Scene => get!("prop.panels.scene").into(),
            PropTab::Problems => get!("prop.panels.problems").into(),
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        match tab {
            PropTab::Features => feature_tree(ui, self.editor, self.build),
            PropTab::Inspector => inspector(ui, self.editor, self.build),
            PropTab::Scene => scene(ui, self.figure),
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
            );
    }
}

#[allow(clippy::too_many_arguments)]
fn panels(
    mut contexts: EguiContexts,
    mut layout: ResMut<PropPanels>,
    mut editor: ResMut<PropEditor>,
    build: Res<PropBuild>,
    mut figure: ResMut<ScaleFigure>,
    mut multicam: ResMut<MulticamState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    dialog: Res<PropFileDialog>,
    mut frame: MessageWriter<FrameTheProp>,
    mut next_mode: ResMut<NextState<AppMode>>,
    // Optional so the prop editor does not drag in the network layer: a build
    // with no `NetPlugin` is a closed game, and a closed game is its own
    // authority.
    role: Option<Res<NetRole>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();

    let mut viewport_ui = Ui::new(
        ctx.clone(),
        Id::new("grackle_prop_viewport"),
        UiBuilder::new().layer_id(LayerId::background()).max_rect(ctx.viewport_rect()),
    );

    let mut requests = PropRequests::default();
    let mut tabs = PropTabs {
        editor: &mut editor,
        build: &build,
        // Through the `ResMut` deref rather than cloned and written back:
        // `ScaleFigure` drives a system that rebuilds a rig when it changes,
        // and a write every frame would throw the figure's meshes away every
        // frame.
        figure: &mut figure,
        requests: &mut requests,
    };

    layout.menu_bar_height = egui::Panel::top("prop_menu_bar")
        .resizable(false)
        .show(&mut viewport_ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(get!("prop.menu.file"), |ui| {
                    if ui.button(get!("prop.menu.new")).clicked() {
                        ui.close_kind(UiKind::Menu);
                        tabs.requests.new = true;
                    }
                    if ui.button(get!("prop.menu.open")).clicked() {
                        ui.close_kind(UiKind::Menu);
                        tabs.requests.open = true;
                    }
                    if ui.button(get!("prop.menu.save")).clicked() {
                        ui.close_kind(UiKind::Menu);
                        tabs.requests.save = true;
                    }
                    if ui.button(get!("prop.menu.save_as")).clicked() {
                        ui.close_kind(UiKind::Menu);
                        tabs.requests.save_as = true;
                    }
                });
                // `Prop` spelled out rather than read from the state: this
                // whole system is gated on it, so the mode it is drawing for
                // is not in question.
                mode_menu(
                    ui,
                    AppMode::Prop,
                    role.as_deref(),
                    &mut tabs.requests.mode,
                );
                if ui.button(get!("prop.menu.frame")).clicked() {
                    tabs.requests.frame = true;
                }
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
            DockArea::new(&mut layout.right)
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

    // One undo step per gesture. The keyboard half of the test is what makes
    // typing a feature's name a single step: the pointer is idle the whole
    // time somebody is typing, so pointer alone would record a step per
    // keystroke.
    if !ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input() {
        editor.end_gesture();
    }

    if requests.frame {
        frame.write(FrameTheProp);
    }
    // `set`, not `set_if_neq`: `mode_menu` never offers the mode already in
    // force, so anything arriving here is a real change.
    if let Some(mode) = requests.mode {
        next_mode.set(mode);
    }
    handle_files(&requests, &mut editor, &dialog);

    if let Ok(window) = windows.single() {
        let left = layout.left_width / window.width();
        let right = layout.right_width / window.width();
        let top = layout.menu_bar_height / window.height();
        let bottom = layout.bottom_height / window.height();
        multicam.start = Vec2::new(left, top);
        multicam.end = Vec2::new(1.0 - right, 1.0 - bottom);
    }
}

/// One line of the feature list, read out of the document before any of it is
/// drawn.
///
/// Collected up front because the rows write back — a selection, an action —
/// and holding a borrow on the editor across the closures that draw them would
/// leave the context menu with nothing it could change.
struct Row {
    id: PropFeatureId,
    name: String,
    enabled: bool,
    /// How many features name this one as an operand, so deleting it can say
    /// what that costs.
    dependants: usize,
}

/// The ordered list, which *is* the prop.
fn feature_tree(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild) {
    ui.horizontal(|ui| {
        ui.add_enabled_ui(editor.can_undo(), |ui| {
            if ui.button(get!("prop.tree.undo")).clicked() {
                editor.undo();
            }
        });
        ui.add_enabled_ui(editor.can_redo(), |ui| {
            if ui.button(get!("prop.tree.redo")).clicked() {
                editor.redo();
            }
        });
    });

    ui.separator();
    add_menu(ui, editor, build);
    ui.separator();

    let entries: Vec<Row> = editor
        .doc()
        .features
        .iter()
        .map(|feature| Row {
            id: feature.id,
            name: feature.name.clone(),
            enabled: feature.enabled,
            dependants: editor.doc().dependants(feature.id).len(),
        })
        .collect();
    let live: Vec<PropFeatureId> = build.evaluated.bodies.iter().map(|(id, _)| *id).collect();
    let broken: Vec<PropFeatureId> =
        build.evaluated.problems.iter().map(|problem| problem.feature).collect();

    // Read out rather than borrowed through the closures below: the rows write
    // back a selection and an action, and holding `editor` across them would
    // mean the context menu could not touch either.
    let selected = editor.selected;
    let mut select: Option<PropFeatureId> = None;
    let mut pending: Option<Box<dyn FnOnce(&mut PropEditor)>> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        for row in entries {
            let Row { id, name, enabled, dependants } = row;

            // Reserved before the row is drawn and filled in after, because
            // whether to paint it depends on a hover the row has not reported
            // yet. Painting afterwards without reserving would put the
            // highlight *over* the name.
            let background = ui.painter().add(egui::Shape::Noop);

            // **The row is the widget; the name is just text.** A
            // `SelectableLabel` senses clicks, so it swallowed the secondary
            // click over the one part of the row anybody aims at, and the menu
            // appeared everywhere except on the name. A plain `Label` senses
            // only hover, and egui's click hit-testing filters to widgets that
            // sense clicks — so the click falls through to the row.
            //
            // `.interact(Sense::click())` is then what makes the row a widget
            // at all: a `Ui`'s own response is allocated with `Sense::hover()`,
            // and `Popup::context_menu` opens on `secondary_clicked()`, which a
            // hover-only response can never report. It updates the row in
            // place rather than moving it to the top of the order, so the
            // reorder buttons — registered after it — still win their own
            // clicks.
            let response = ui
                .horizontal(|ui| {
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

                    // `.selectable(false)` is the other half of making the row
                    // the widget, and it is not about the caret. A label that
                    // can have its text selected picks up
                    // `Sense::click_and_drag()` for that, which puts it back in
                    // the click hit-test and wins over the row — so both the
                    // select and the context menu stop working over the name,
                    // exactly as they did when it was a `SelectableLabel`.
                    // The I-beam cursor is the visible half of the same thing.
                    ui.add(egui::Label::new(text).selectable(false));

                    // Reordering stays on the row rather than joining the menu:
                    // it is the one thing here done several times in a row, and
                    // a menu per nudge would be three clicks a place.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button(get!("prop.tree.move_down"))
                            .on_hover_text(get!("prop.tree.move_down_hint"))
                            .clicked()
                        {
                            pending = Some(Box::new(move |editor: &mut PropEditor| {
                                editor.edit(|doc| doc.shift(id, true));
                            }));
                        }
                        if ui
                            .small_button(get!("prop.tree.move_up"))
                            .on_hover_text(get!("prop.tree.move_up_hint"))
                            .clicked()
                        {
                            pending = Some(Box::new(move |editor: &mut PropEditor| {
                                editor.edit(|doc| doc.shift(id, false));
                            }));
                        }
                    });
                })
                .response
                .interact(egui::Sense::click());

            if response.clicked() {
                select = Some(id);
            }

            // Selection outranks hover: a row you are pointing at that is
            // already chosen should not dim to say so.
            let fill = if selected == Some(id) {
                Some(ui.visuals().selection.bg_fill)
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
                // Right-clicking a thing is a way of pointing at it, so it
                // selects as well as opening the menu — otherwise the inspector
                // goes on showing whatever was selected before, beside a menu
                // acting on something else.
                select = Some(id);

                // The item names the **action**, not the state: "Suppress" on
                // something that is on. A label that named the state would read
                // as a checkbox with no box, and you would have to guess
                // whether clicking it agreed or disagreed with what it said.
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

                ui.separator();

                let delete = ui.button(get!("prop.tree.delete"));
                // Said before rather than after: everything that names this
                // feature as an operand breaks the moment it goes, and the
                // evaluator will report each of them as a problem. Undo is
                // there, but knowing beforehand is cheaper than reading four
                // warnings and working out what they have in common.
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
                        if editor.selected == Some(id) {
                            editor.selected = None;
                        }
                    }));
                }
            });
        }
    });

    if let Some(id) = select {
        editor.selected = Some(id);
    }
    if let Some(action) = pending {
        action(editor);
    }
}

/// Adding a feature.
///
/// The booleans are only offered when there is something to point them at,
/// and they arrive **already pointed at the last two bodies** rather than at
/// nothing. A boolean that lands broken and has to be wired up in the
/// inspector is two steps where the intent was one, and the intent is nearly
/// always "cut this out of that".
fn add_menu(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild) {
    let live: Vec<PropFeatureId> = build.evaluated.bodies.iter().map(|(id, _)| *id).collect();
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

        ui.add_enabled_ui(live.len() >= 2, |ui| {
            ui.menu_button(get!("prop.add.boolean"), |ui| {
                for op in BooleanOp::ALL {
                    if ui.button(op.name()).clicked() {
                        ui.close_kind(UiKind::Menu);
                        added = Some(FeatureOp::Boolean {
                            op,
                            target: live[live.len() - 2],
                            tool: live[live.len() - 1],
                        });
                    }
                }
            });
        });

        ui.add_enabled_ui(!live.is_empty(), |ui| {
            let target = live.last().copied().unwrap_or_default();
            if ui.button(get!("prop.features.mirror")).clicked() {
                added = Some(FeatureOp::Mirror {
                    target,
                    axis: Axis::X,
                    offset: 0.0,
                    keep_original: true,
                });
            }
            if ui.button(get!("prop.features.paint")).clicked() {
                added = Some(FeatureOp::Paint { target, surface: Surface::default() });
            }
        });
    });

    if let Some(op) = added {
        let id = editor.edit(|doc| doc.push(op));
        editor.selected = Some(id);
    }
}

/// The selected feature's numbers.
fn inspector(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild) {
    let Some(selected) = editor.selected else {
        ui.label(get!("prop.inspector.nothing"));
        return;
    };
    let Some(index) = editor.doc().index_of(selected) else {
        ui.label(get!("prop.inspector.nothing"));
        return;
    };
    // What this feature could point at is what was live *when it ran* — not
    // what is live now, which is after it has already eaten something.
    // Recorded by the evaluator so there is one answer rather than two.
    let choices: Vec<PropFeatureId> =
        build.evaluated.live_before.get(index).cloned().unwrap_or_default();
    let names: Vec<(PropFeatureId, String)> = choices
        .iter()
        .map(|id| {
            (*id, editor.doc().feature(*id).map(|f| f.name.clone()).unwrap_or_else(|| id.to_string()))
        })
        .collect();

    // Drawn against a **copy**, written back only if something moved.
    // `PropEditor::doc_mut` marks the document changed by being called, which
    // is what makes the viewport rebuild — so an inspector that reached for it
    // every frame would re-run the boolean kernel, and light the unsaved-work
    // asterisk, for as long as anything was merely selected.
    let Some(mut edited) = editor.doc().feature(selected).cloned() else { return };
    let feature = &mut edited;

    let mut changed = false;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(get!("prop.inspector.name"));
            changed |= ui.text_edit_singleline(&mut feature.name).changed();
        });
        // Phrased as *suppressed* rather than *enabled*, so the inspector and
        // the tree's context menu are one word for one concept. The stored
        // field stays `enabled` — it is on disk, and the format has no
        // migration chain to rename it through.
        let mut suppressed = !feature.enabled;
        if ui.checkbox(&mut suppressed, get!("prop.inspector.suppressed")).changed() {
            feature.enabled = !suppressed;
            changed = true;
        }
        ui.separator();

        match &mut feature.op {
            FeatureOp::Primitive { shape, placement, surface } => {
                changed |= shape_ui(ui, shape);
                ui.separator();
                changed |= placement_ui(ui, placement);
                ui.separator();
                changed |= surface_ui(ui, surface);
            }
            FeatureOp::Extrude { profile, placement, depth, midplane, surface } => {
                changed |= profile_ui(ui, profile);
                ui.separator();
                changed |= drag(ui, get!("prop.inspector.depth"), depth, 0.001, "m");
                changed |= ui.checkbox(midplane, get!("prop.inspector.midplane")).changed();
                ui.separator();
                changed |= placement_ui(ui, placement);
                ui.separator();
                changed |= surface_ui(ui, surface);
            }
            FeatureOp::Revolve { profile, placement, degrees, segments, surface } => {
                changed |= profile_ui(ui, profile);
                ui.separator();
                changed |= drag(ui, get!("prop.inspector.degrees"), degrees, 1.0, "°");
                changed |= count(ui, get!("prop.inspector.segments"), segments);
                ui.separator();
                changed |= placement_ui(ui, placement);
                ui.separator();
                changed |= surface_ui(ui, surface);
            }
            FeatureOp::Boolean { op, target, tool } => {
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
            }
            FeatureOp::Mirror { target, axis, offset, keep_original } => {
                changed |= body_picker(ui, get!("prop.inspector.target"), target, &names);
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
            }
            FeatureOp::Paint { target, surface } => {
                changed |= body_picker(ui, get!("prop.inspector.target"), target, &names);
                ui.separator();
                changed |= surface_ui(ui, surface);
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

/// What is in the viewport besides the prop.
///
/// Its own tab rather than a corner of the inspector: the inspector is about
/// the selected feature and this is about how you are looking at the whole
/// thing, which is the same distinction the map editor's **Show** tab makes.
fn scene(ui: &mut Ui, figure: &mut ScaleFigure) {
    ui.label(get!("prop.scene.figure"));
    // `None` first and default, because the figure is a ruler rather than part
    // of the prop: a modeller who wants the silhouette on its own should get
    // the silhouette on its own.
    egui::ComboBox::from_id_salt("prop_scale_figure")
        .selected_text(match figure.0 {
            Some(class) => class.name(),
            None => get!("prop.scene.no_figure"),
        })
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut figure.0, None, get!("prop.scene.no_figure"));
            for class in Class::iter() {
                ui.selectable_value(&mut figure.0, Some(class), class.name());
            }
        });
}

fn problems(ui: &mut Ui, editor: &mut PropEditor, build: &PropBuild) {
    ui.horizontal(|ui| {
        ui.label(get!("prop.stats.bodies", "count", build.evaluated.bodies.len()));
        ui.separator();
        ui.label(get!("prop.stats.triangles", "count", build.evaluated.triangle_count()));
        if let Some((min, max)) = build.evaluated.bounds() {
            ui.separator();
            let size = max - min;
            ui.label(format!("{:.3} × {:.3} × {:.3} m", size.x, size.y, size.z));
        }
    });
    ui.separator();

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
                editor.selected = Some(problem.feature);
            }
        }
    });
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
        // Clamped at the low end because fewer than three sides is not a
        // shape, and a box you cannot drag down through 2 on the way to 3 is a
        // box that fights you.
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

fn placement_ui(ui: &mut Ui, placement: &mut Placement) -> bool {
    let mut changed = vec3_ui(ui, get!("prop.inspector.origin"), &mut placement.origin, 0.001, "");
    // Degrees in the boxes and radians in the field, the same way a map
    // feature's rotation is: a part is lined up in degrees and everything
    // downstream is trigonometry.
    let mut degrees = placement.rotation.map(f32::to_degrees);
    if vec3_ui(ui, get!("prop.inspector.rotation"), &mut degrees, 1.0, "°") {
        placement.rotation = degrees.map(f32::to_radians);
        changed = true;
    }
    changed
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
        // Switching kind replaces the profile rather than converting it. A
        // rectangle has no side count to carry into an n-gon and an n-gon has
        // no corner list to carry into a polyline, so anything else would be
        // inventing numbers.
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
            // A reference to something that is no longer a body still has to
            // be shown as what it is, rather than falling back to the first
            // thing in the list — which would silently repoint the feature.
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

/// New, open, save — including the file dialog, which runs on its own thread
/// exactly the way the map editor's does.
fn handle_files(requests: &PropRequests, editor: &mut PropEditor, dialog: &PropFileDialog) {
    if requests.new {
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

    if requests.save && editor.path.is_some() {
        let path = editor.path.clone().expect("checked just above");
        match document::save(&path, editor.doc()) {
            Ok(()) => {
                info!("Saved prop to {path:?}");
                editor.mark_saved(path);
            }
            Err(e) => error!("Saving the prop failed: {e}"),
        }
        return;
    }

    // A save with nowhere to save to is a save-as, rather than an error or a
    // silent nothing.
    if requests.save_as || requests.save {
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

    if requests.open {
        let slot = dialog.result.clone();
        std::thread::spawn(move || {
            let chooser = rfd::AsyncFileDialog::new().add_filter("Grackle Prop", &[PROP_EXTENSION]);
            if let Some(handle) = pollster::block_on(chooser.pick_file()) {
                *slot.lock().unwrap() = Some(DialogResult::Open(handle.path().to_path_buf()));
            }
        });
    }
}
