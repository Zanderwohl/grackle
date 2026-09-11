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
use crate::common::net::NetRole;
use crate::editor::multicam::MulticamState;
use crate::get;
use crate::prop::document::{self, PropDoc, PropEditor, PROP_EXTENSION};
use crate::common::class::Class;
use crate::common::shortcuts::chords;
use crate::prop::feature::{Axis, BooleanOp, FeatureOp, PropFeatureId};
use crate::prop::figure::ScaleFigure;
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
    Scene,
    Hold,
    Problems,
}

/// The panel layout.
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
            right: DockState::new(vec![PropTab::Inspector, PropTab::Scene, PropTab::Hold]),
            bottom: DockState::new(vec![PropTab::Problems]),
            menu_bar_height: 0.0,
            left_width: 260.0,
            right_width: 300.0,
            bottom_height: 80.0,
        }
    }
}

/// What the panels asked for this frame, so the systems that answer need not be
/// reachable from inside an egui closure.
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
    handles: &'a mut HoldHandles,
    requests: &'a mut PropRequests,
}

impl<'a> TabViewer for PropTabs<'a> {
    type Tab = PropTab;

    fn id(&mut self, tab: &mut Self::Tab) -> Id {
        Id::new(match tab {
            PropTab::Features => "prop_features",
            PropTab::Inspector => "prop_inspector",
            PropTab::Scene => "prop_scene",
            PropTab::Hold => "prop_hold",
            PropTab::Problems => "prop_problems",
        })
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        match tab {
            PropTab::Features => get!("prop.panels.features").into(),
            PropTab::Inspector => get!("prop.panels.inspector").into(),
            PropTab::Scene => get!("prop.panels.scene").into(),
            PropTab::Hold => get!("prop.panels.hold").into(),
            PropTab::Problems => get!("prop.panels.problems").into(),
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        match tab {
            PropTab::Features => feature_tree(ui, self.editor, self.build),
            PropTab::Inspector => inspector(ui, self.editor, self.build),
            PropTab::Scene => scene(ui, self.figure),
            PropTab::Hold => hold(ui, self.editor, *self.figure, self.handles),
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
            .add_systems(Update, prop_shortcuts.run_if(in_state(AppMode::Prop)));
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
        handles: &mut handles,
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

    // One undo step per gesture. The keyboard half is what makes typing a name
    // a single step — the pointer is idle the whole time somebody types.
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
    let mut select: Option<PropFeatureId> = None;
    let mut pending: Option<Box<dyn FnOnce(&mut PropEditor)>> = None;

    // Used once every row has been drawn: where an insertion line goes is a
    // question about the whole list. Discovered this way rather than remembered
    // between frames, so there is no drag state to get out of step.
    let mut rects: Vec<(PropFeatureId, egui::Rect)> = Vec::new();
    let mut dragging: Option<usize> = None;
    let mut dropped = false;

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
                select = Some(id);
            }

            // Selection outranks hover, or a chosen row dims when pointed at.
            let fill = if response.dragged() {
                Some(ui.visuals().widgets.active.weak_bg_fill)
            } else if selected == Some(id) {
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
                // Right-clicking is a way of pointing at a thing, so it selects
                // too — otherwise the inspector shows something else.
                select = Some(id);

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
                        if editor.selected == Some(id) {
                            editor.selected = None;
                        }
                    }));
                }
            });
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

    if let Some(id) = select {
        editor.selected = Some(id);
    }
    if let Some(action) = pending {
        action(editor);
    }
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

/// Adding a feature.
///
/// Booleans arrive **already pointed at the last two bodies**: one that landed
/// broken and had to be wired up in the inspector would be two steps where the
/// intent — nearly always "cut this out of that" — was one.
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
        // **Positive, unlike the tree's menu, and that is not an
        // inconsistency.** A menu item names an *action*, and the useful thing
        // to say there is what clicking will do — "Suppress". A checkbox names
        // a *state*, and a ticked box meaning "off" is the kind of thing you
        // have to read twice. So the two differ because they are different
        // questions, and the box is bound straight to the stored field with
        // nothing to invert.
        changed |= ui.checkbox(&mut feature.enabled, get!("prop.inspector.active")).changed();
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

/// What is in the viewport besides the prop. Its own tab because the inspector
/// is about the selected feature and this is about the whole view — the
/// distinction the map editor's **Show** tab makes.
fn scene(ui: &mut Ui, figure: &mut ScaleFigure) {
    ui.label(get!("prop.scene.figure"));
    // `None` first and default: the figure is a ruler, not part of the prop.
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

/// How this prop is held.
///
/// Numbers rather than handles, for now — the gizmos are the next piece. What
/// makes them worth typing at all is that the reference figure performs *this*
/// hold, so the effect of a change is on screen the moment it is made.
///
/// **The class on show is the class being edited.** One question, asked once:
/// ticking the override edits the entry for whoever is standing there, which
/// is also the body you are judging it against.
fn hold(ui: &mut Ui, editor: &mut PropEditor, figure: ScaleFigure, handles: &mut HoldHandles) {
    let base = editor.doc().hold.clone();
    let key = figure.0.map(Class::key);
    let overriding = key.as_ref().is_some_and(|key| base.per_class.contains_key(key));

    let mut changed = false;
    let mut edited = base.for_class(figure.0);
    // Not per class: a viewmodel is a framing on a screen, not a fact about
    // whoever is holding the thing.
    let mut viewmodel = editor.doc().viewmodel;
    let mut view_changed = false;

    // Off by default and exclusive with the feature handles: a grip sits at
    // the prop's origin by convention, which is exactly where a feature's move
    // arrows are.
    ui.checkbox(&mut handles.0, get!("prop.hold.handles"));
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        match (&key, figure.0) {
            (Some(key), Some(class)) => {
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
                let _ = class;
            }
            _ => {
                ui.label(egui::RichText::new(get!("prop.hold.no_figure")).weak());
            }
        }
        ui.separator();

        ui.label(get!("prop.hold.trigger_hand"));
        changed |= vec3_ui(ui, get!("prop.hold.at"), &mut edited.grip, 0.001, "m");
        changed |= degrees_ui(ui, get!("prop.inspector.rotation"), &mut edited.grip_rotation);

        ui.separator();
        changed |= ui.checkbox(&mut edited.two_handed, get!("prop.hold.two_handed")).changed();
        ui.add_enabled_ui(edited.two_handed, |ui| {
            changed |= vec3_ui(ui, get!("prop.hold.at"), &mut edited.support, 0.001, "m");
            changed |= degrees_ui(ui, get!("prop.inspector.rotation"), &mut edited.support_rotation);
        });

        ui.separator();
        ui.label(get!("prop.hold.carry"));
        // Arm lengths, not metres, so a carry means the same thing on a Heavy
        // as on a Scout. Dragged in hundredths because the whole useful range
        // is under one arm.
        changed |= vec3_ui(ui, get!("prop.hold.offset"), &mut edited.carry, 0.005, "");
        changed |= degrees_ui(ui, get!("prop.inspector.rotation"), &mut edited.carry_rotation);

        ui.separator();
        ui.label(get!("prop.hold.viewmodel"));
        ui.label(egui::RichText::new(get!("prop.hold.viewmodel_note")).weak().small());
        // Fractions of the frustum, so the same numbers mean the same place on
        // screen at any field of view and any window — which is what stops a
        // weapon drifting in from the edge it is meant to hang off.
        ui.horizontal(|ui| {
            ui.label(get!("prop.hold.across"));
            for (component, axis) in viewmodel.across.iter_mut().zip(["x", "y"]) {
                view_changed |= ui
                    .add(
                        egui::DragValue::new(component)
                            .speed(0.01)
                            .range(-2.0..=2.0)
                            .prefix(format!("{axis} ")),
                    )
                    .changed();
            }
        });
        view_changed |= drag(ui, get!("prop.hold.depth"), &mut viewmodel.depth, 0.005, "m");
        view_changed |= degrees_ui(ui, get!("prop.inspector.rotation"), &mut viewmodel.rotation);
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
        // A viewport that stopped following a drag looks like one that stopped
        // working.
        if build.settling() {
            ui.separator();
            ui.label(egui::RichText::new(get!("prop.stats.settling")).weak());
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

fn placement_ui(ui: &mut Ui, placement: &mut Placement) -> bool {
    let mut changed = vec3_ui(ui, get!("prop.inspector.origin"), &mut placement.origin, 0.001, "");
    // Degrees in the boxes and radians in the field, as a map feature's
    // rotation is: parts are lined up in degrees, maths is done in radians.
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

/// `Ctrl+S`, `Ctrl+Shift+S`, and the undo pair, for the prop editor.
///
/// The map editor's undo has always had this; the prop editor's lived only on
/// two buttons in the Features panel, so `Ctrl+Z` did nothing at all — which
/// reads as a bug rather than as an absence.
fn prop_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    egui: Res<bevy_egui::input::EguiWantsInput>,
    dialog: Res<PropFileDialog>,
    mut editor: ResMut<PropEditor>,
) {
    let chords = chords(&keys, egui.wants_keyboard_input());
    if chords.redo {
        editor.redo();
    } else if chords.undo {
        editor.undo();
    }

    if chords.save {
        save_or_ask(&mut editor, &dialog);
    } else if chords.save_as {
        ask_where_to_save(&editor, &dialog);
    }
}

/// New, open, save, including the file dialog on its own thread.
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

    if requests.save {
        save_or_ask(editor, dialog);
    } else if requests.save_as {
        ask_where_to_save(editor, dialog);
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
