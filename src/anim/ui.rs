//! The animation editor's panels, and the strip they leave room for.
//!
//! Laid out like the prop editor's, on purpose: the same panels around the
//! same four viewports, so the window does not rearrange itself under somebody
//! swapping modes. What is new is the bottom, which is **reserved and not
//! drawn** — see [`panels`].

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::egui::{Id, LayerId, Ui, UiBuilder, WidgetText};
use bevy_egui::{egui, EguiContexts};
use egui_dock::{DockArea, DockState, TabViewer};

use crate::anim::timeline::{KeyAt, TimelineCursor, TimelineSelection, TimelineView};
use crate::anim::AnimEditor;
use crate::common::skeleton::clip::{Axis, Channel};
use crate::common::skeleton::Interpolation;
use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::class::Class;
use crate::common::net::NetRole;
use crate::editor::multicam::MulticamState;
use crate::get;
use crate::prop::document::PropEditor;
use crate::prop::figure::ScaleFigure;

/// How tall the strip is when the mode first opens.
///
/// No minimum: `egui::Panel` has none to set, so the strip is draggable to
/// nothing exactly as the prop editor's Problems panel is. The layout pass
/// switches the root off rather than drawing a strip with no room in it.
const DEFAULT_STRIP: f32 = 180.0;

/// The top few pixels of the strip belong to egui's resize handle.
///
/// Without the inset, grabbing the splitter and grabbing a key on the first
/// track are the same gesture in the same place, and which one you get depends
/// on a pixel.
const SPLITTER_BAND: f32 = 6.0;

enum AnimTab {
    Clips,
    Track,
    Key,
    Scene,
    Problems,
}

/// The panel layout, and what it measured this frame.
#[derive(Resource)]
pub struct AnimPanels {
    left: DockState<AnimTab>,
    right: DockState<AnimTab>,
    menu_bar_height: f32,
    left_width: f32,
    right_width: f32,
    timeline_height: f32,
    /// Whether egui is actively *using* the pointer — a menu open, a widget
    /// mid-drag — as against merely having a panel under it.
    ///
    /// Recorded here rather than asked in `Update`, so egui is touched in one
    /// place. The distinction is the whole of the timeline's pointer
    /// arbitration: the strip *is* a panel's rect, so "is the pointer over
    /// egui" is true across all of it and answering that would mean the
    /// timeline never saw a click.
    pub egui_wants_pointer: bool,
    /// The same question for the keyboard, and the reason is the one
    /// `shortcuts` already gives: `Delete` in a text field deletes a
    /// character, and stealing it loses what somebody typed.
    pub egui_wants_keyboard: bool,
}

impl Default for AnimPanels {
    fn default() -> Self {
        AnimPanels {
            left: DockState::new(vec![AnimTab::Clips]),
            right: DockState::new(vec![
                AnimTab::Track,
                AnimTab::Key,
                AnimTab::Scene,
                // On the right rather than along the bottom, which is where
                // the prop editor keeps its own: the bottom is the strip now.
                AnimTab::Problems,
            ]),
            menu_bar_height: 0.0,
            left_width: 220.0,
            right_width: 300.0,
            timeline_height: DEFAULT_STRIP,
            egui_wants_pointer: false,
            egui_wants_keyboard: false,
        }
    }
}

pub struct AnimUiPlugin;

impl Plugin for AnimUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimPanels>().add_systems(
            bevy_egui::EguiPrimaryContextPass,
            panels.run_if(in_state(AppMode::Animation)),
        );
    }
}

struct AnimTabs<'a> {
    editor: &'a mut PropEditor,
    anim: &'a mut AnimEditor,
    figure: &'a mut ScaleFigure,
    cursor: &'a mut TimelineCursor,
    selection: &'a TimelineSelection,
    mode: &'a mut Option<AppMode>,
}

impl TabViewer for AnimTabs<'_> {
    type Tab = AnimTab;

    fn id(&mut self, tab: &mut AnimTab) -> Id {
        Id::new(match tab {
            AnimTab::Clips => "anim_clips",
            AnimTab::Track => "anim_track",
            AnimTab::Key => "anim_key",
            AnimTab::Scene => "anim_scene",
            AnimTab::Problems => "anim_problems",
        })
    }

    fn title(&mut self, tab: &mut AnimTab) -> WidgetText {
        match tab {
            AnimTab::Clips => get!("anim.panels.clips"),
            AnimTab::Track => get!("anim.panels.track"),
            AnimTab::Key => get!("anim.panels.key"),
            AnimTab::Scene => get!("anim.panels.scene"),
            AnimTab::Problems => get!("anim.panels.problems"),
        }
        .into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut AnimTab) {
        match tab {
            AnimTab::Clips => self.clips(ui),
            AnimTab::Track => self.tracks(ui),
            AnimTab::Key => self.key(ui),
            AnimTab::Scene => self.scene(ui),
            AnimTab::Problems => self.problems(ui),
        }
    }
}

impl AnimTabs<'_> {
    fn clips(&mut self, ui: &mut Ui) {
        let names: Vec<String> = self.editor.doc().animations.clips.keys().cloned().collect();
        if names.is_empty() {
            ui.label(get!("anim.no_clips"));
            return;
        }
        for name in names {
            let picked = self.anim.selected.as_deref() == Some(name.as_str());
            if ui.selectable_label(picked, &name).clicked() && !picked {
                self.anim.selected = Some(name);
            }
        }
    }

    fn tracks(&mut self, ui: &mut Ui) {
        let Some(clip) = self
            .anim
            .selected
            .as_deref()
            .and_then(|name| self.editor.doc().animations.get(name))
        else {
            ui.label(get!("anim.no_clips"));
            return;
        };

        ui.label(format!("{:.3} s", clip.length));
        ui.label(get!(
            "anim.clock_is",
            "clock",
            get!(match clip.clock {
                crate::common::skeleton::clip::Clock::Seconds => "anim.clock.seconds",
                crate::common::skeleton::clip::Clock::Stride => "anim.clock.stride",
                crate::common::skeleton::clip::Clock::Shared => "anim.clock.shared",
            })
        ));
        ui.separator();

        for (index, track) in clip.tracks.iter().enumerate() {
            ui.label(format!(
                "{index}. {} — {}",
                get!(track.channel.kind_key()),
                track.channel.key_count(),
            ));
        }
    }

    /// The per-axis control, and the centre of the whole feature.
    ///
    /// A target is what to move, where, in which space and unit — and **per
    /// axis**, so "don't hit a height, hit the floor" is one dropdown and not
    /// a different kind of key. The three axes are drawn as three identical
    /// rows on purpose: they are the same question asked three times, and a
    /// vertical axis that looked special would suggest `Ground` is a property
    /// of the key rather than of the axis.
    fn key(&mut self, ui: &mut Ui) {
        let Some(name) = self.anim.selected.clone() else {
            ui.label(get!("anim.no_clips"));
            return;
        };
        let picked: Vec<KeyAt> = self.selection.0.iter().copied().collect();
        if picked.is_empty() {
            ui.label(get!("anim.no_key"));
            return;
        }
        if picked.len() > 1 {
            ui.label(get!("anim.many_keys", "count", picked.len().to_string()));
            ui.separator();
        }

        // The first of the selection is what is edited, and a write goes to
        // every picked key of the same kind — which is what makes setting six
        // feet to the floor one gesture rather than six.
        let first = picked[0];
        let Some(clip) = self.editor.doc().animations.get(&name) else { return };
        let Some(track) = clip.tracks.get(first.track) else { return };

        let mut edited = match &track.channel {
            Channel::Effector { keys, .. } | Channel::Weapon { keys } => {
                keys.get(first.key).copied()
            }
            _ => None,
        };

        let Some(key) = edited.as_mut() else {
            // A rotation key is three angles and has no datum to choose, so it
            // is the ordinary `degrees_ui` case rather than this one. Stated
            // rather than silently drawing nothing.
            ui.label(get!("anim.rotation_key"));
            return;
        };

        let mut changed = false;
        ui.label(get!("anim.at", "time", format!("{:.3}", key.at)));
        ui.separator();

        for (index, label) in ["anim.axis.x", "anim.axis.y", "anim.axis.z"].iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(get!(*label));
                let axis = &mut key.to[index];

                let mut kind = match axis {
                    Axis::Free => 0,
                    Axis::Ground { .. } => 1,
                    Axis::Value(_) => 2,
                };
                let before = kind;
                egui::ComboBox::from_id_salt(("anim_axis", index))
                    .selected_text(get!(match kind {
                        0 => "anim.axis.free",
                        1 => "anim.axis.ground",
                        _ => "anim.axis.value",
                    }))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut kind, 0, get!("anim.axis.free"));
                        // Only the vertical axis has a floor under it. Offered
                        // and refused would be a control that does nothing;
                        // not offered is an answer.
                        if index == 1 {
                            ui.selectable_value(&mut kind, 1, get!("anim.axis.ground"));
                        }
                        ui.selectable_value(&mut kind, 2, get!("anim.axis.value"));
                    });

                if kind != before {
                    // Carrying the number across, so switching a height to a
                    // clearance keeps the number somebody typed rather than
                    // resetting it to nothing.
                    let carried = axis.offset().unwrap_or(0.0);
                    *axis = match kind {
                        0 => Axis::Free,
                        1 => Axis::Ground { clearance: carried },
                        _ => Axis::Value(carried),
                    };
                    changed = true;
                }

                if let Some(mut number) = axis.offset() {
                    if ui
                        .add(egui::DragValue::new(&mut number).speed(0.005).max_decimals(4))
                        .changed()
                    {
                        *axis = match axis {
                            Axis::Ground { .. } => Axis::Ground { clearance: number },
                            _ => Axis::Value(number),
                        };
                        changed = true;
                    }
                }
            });
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label(get!("anim.leaving"));
            egui::ComboBox::from_id_salt("anim_ease")
                .selected_text(get!(key.ease.name_key()))
                .show_ui(ui, |ui| {
                    for ease in Interpolation::ALL {
                        if ui
                            .selectable_label(key.ease == *ease, get!(ease.name_key()))
                            .clicked()
                            && key.ease != *ease
                        {
                            key.ease = *ease;
                            changed = true;
                        }
                    }
                });
        });

        if !changed {
            return;
        }

        // Through the gesture, like every other write: egui reports a drag as
        // a change per frame, and per-change undo steps would turn one pull on
        // a number into forty presses of Ctrl+Z.
        let written = *key;
        self.editor.begin_gesture();
        let Some(clip) = self.editor.doc_mut().animations.clips.get_mut(&name) else { return };
        for picked in &picked {
            let Some(track) = clip.tracks.get_mut(picked.track) else { continue };
            match &mut track.channel {
                Channel::Effector { keys, .. } | Channel::Weapon { keys } => {
                    if let Some(key) = keys.get_mut(picked.key) {
                        // The time is not written across: a multiple selection
                        // edited together should change its shape, not collapse
                        // onto one moment.
                        key.to = written.to;
                        key.ease = written.ease;
                    }
                }
                _ => {}
            }
        }
    }

    fn scene(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(get!("prop.scene.figure"));
            egui::ComboBox::from_id_salt("anim_figure")
                .selected_text(match self.figure.0 {
                    Some(class) => class.name(),
                    None => get!("prop.scene.no_figure"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.figure.0, None, get!("prop.scene.no_figure"));
                    for class in <Class as strum::IntoEnumIterator>::iter() {
                        ui.selectable_value(&mut self.figure.0, Some(class), class.name());
                    }
                });
        });
        ui.separator();
        ui.label(format!("{:.3} s", self.cursor.time));
    }

    fn problems(&mut self, ui: &mut Ui) {
        let Some(clip) = self
            .anim
            .selected
            .as_deref()
            .and_then(|name| self.editor.doc().animations.get(name))
        else {
            return;
        };
        // The rig the figure is actually wearing, so a bone that exists on one
        // build and not another is reported against the one on screen.
        let rig = crate::common::skeleton::humanoid(
            self.figure.0.unwrap_or(Class::Mercenary).proportions(),
        );
        for problem in clip.problems(|bone| rig.index_of(bone).is_some()) {
            ui.colored_label(egui::Color32::from_rgb(220, 140, 60), problem);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn panels(
    mut contexts: EguiContexts,
    mut layout: ResMut<AnimPanels>,
    mut editor: ResMut<PropEditor>,
    mut anim: ResMut<AnimEditor>,
    mut figure: ResMut<ScaleFigure>,
    mut cursor: ResMut<TimelineCursor>,
    selection: Res<TimelineSelection>,
    mut view: ResMut<TimelineView>,
    grab: Res<crate::anim::timeline::TimelineGrab>,
    mut multicam: ResMut<MulticamState>,
    mut next_mode: ResMut<NextState<AppMode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    role: Option<Res<NetRole>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();

    let mut viewport_ui = Ui::new(
        ctx.clone(),
        Id::new("grackle_anim_viewport"),
        UiBuilder::new()
            .layer_id(LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    let mut requested: Option<AppMode> = None;
    let mut tabs = AnimTabs {
        editor: &mut editor,
        anim: &mut anim,
        figure: &mut figure,
        cursor: &mut cursor,
        selection: &selection,
        mode: &mut requested,
    };

    layout.menu_bar_height = egui::Panel::top("anim_menu_bar")
        .resizable(false)
        .show(&mut viewport_ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                // Spelled out rather than read from the state, the way the
                // prop editor's is: this system is gated on the mode it draws
                // for, so which one that is is not in question.
                mode_menu(ui, AppMode::Animation, role.as_deref(), tabs.mode);
                ui.separator();
                ui.label(match tabs.anim.selected.as_deref() {
                    Some(name) => name.to_string(),
                    None => get!("anim.no_clips"),
                });
            });
        })
        .response
        .rect
        .height();

    layout.left_width = egui::Panel::left("anim_left")
        .resizable(true)
        .show(&mut viewport_ui, |ui| {
            DockArea::new(&mut layout.left)
                .id(Id::new("egui_dock::DockArea::anim_left"))
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

    layout.right_width = egui::Panel::right("anim_right")
        .resizable(true)
        .show(&mut viewport_ui, |ui| {
            DockArea::new(&mut layout.right)
                .id(Id::new("egui_dock::DockArea::anim_right"))
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

    drop(tabs);

    // **Reserved, not drawn.** egui owns the *layout* — so the viewports
    // shrink, and the splitter between them and the strip is egui's own resize
    // handle rather than a second one written here — and Bevy UI owns the
    // *pixels*. `Frame::NONE` is what keeps egui from painting over the nodes,
    // and the hover sense allocated inside is what makes
    // `is_pointer_over_egui` true across the strip, which is how an orbit is
    // stopped from starting on it without `prop::camera` learning what a
    // timeline is.
    let strip = egui::Panel::bottom("anim_timeline")
        .resizable(true)
        .frame(egui::Frame::NONE)
        .default_size(DEFAULT_STRIP)
        .min_size(DEFAULT_STRIP * 0.5)
        .show(&mut viewport_ui, |ui| {
            ui.allocate_rect(ui.available_rect_before_wrap(), egui::Sense::hover());
        })
        .response
        .rect;
    layout.timeline_height = strip.height();

    // egui points are not Bevy's logical pixels when the context is zoomed,
    // and `Val::Px` is Bevy's. Converting here rather than in the layout pass
    // keeps the whole of the timeline in one coordinate system.
    view.rect = strip_rect(
        Rect::from_corners(
            Vec2::new(strip.min.x, strip.min.y),
            Vec2::new(strip.max.x, strip.max.y),
        ),
        ctx.zoom_factor(),
    );

    layout.egui_wants_pointer = ctx.egui_wants_pointer_input();
    layout.egui_wants_keyboard = ctx.egui_wants_keyboard_input();

    // One undo step per gesture, exactly as the prop editor closes its own —
    // **and the timeline's drags count.** egui is not "using the pointer"
    // while somebody drags a key, because the strip is a frameless panel with
    // nothing in it but a hover sense, so without the third clause a drag
    // would close its gesture on every frame and land in the undo stack forty
    // times over. That is the whole reason this system knows a grab exists.
    if !ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input() && grab.0.is_none() {
        editor.end_gesture();
    }

    if let Some(mode) = requested {
        next_mode.set(mode);
    }

    if let Ok(window) = windows.single() {
        let left = layout.left_width / window.width();
        let right = layout.right_width / window.width();
        let top = layout.menu_bar_height / window.height();
        // The strip and the dock below it share the bottom inset. There is no
        // bottom dock here, so this is the strip alone — but adding one later
        // means adding its height in, not replacing this.
        let bottom = layout.timeline_height / window.height();
        multicam.start = Vec2::new(left, top);
        multicam.end = Vec2::new(1.0 - right, 1.0 - bottom);
    }
}

/// Where the strip's own content starts, given what egui reserved.
///
/// Pulled out of the closure so the partition can be checked without an egui
/// context, the way `AppMode::selectable_from` is.
pub fn strip_rect(reserved: Rect, zoom: f32) -> Rect {
    Rect::from_corners(
        Vec2::new(reserved.min.x * zoom, reserved.min.y * zoom + SPLITTER_BAND),
        Vec2::new(reserved.max.x * zoom, reserved.max.y * zoom),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strip and the viewports partition the window: neither eats the
    /// other, and the strip never reaches above what egui gave it.
    ///
    /// The failure this guards is not a crash. Reserve too little and the
    /// bottom row of keys is drawn under the viewports; reserve too much and
    /// there is a dead band nothing draws in.
    #[test]
    fn the_reserved_strip_does_not_overlap_a_viewport() {
        let window = Vec2::new(1600.0, 900.0);
        let reserved = Rect::from_corners(Vec2::new(0.0, 720.0), Vec2::new(1600.0, 900.0));

        let strip = strip_rect(reserved, 1.0);
        let viewport_bottom = window.y - reserved.height();

        assert!(
            strip.min.y >= viewport_bottom,
            "the strip starts above the viewports: {} vs {viewport_bottom}",
            strip.min.y,
        );
        assert!(strip.max.y <= window.y, "the strip runs off the window");
        assert!(
            strip.min.y - viewport_bottom <= SPLITTER_BAND + 1e-3,
            "more was reserved than the strip uses",
        );
    }

    /// The context's zoom is not Bevy's logical pixels. Ignoring it puts the
    /// strip somewhere else entirely on a scaled display — and it is the kind
    /// of thing that looks perfect on the machine it was written on.
    #[test]
    fn a_zoomed_context_still_lands_on_the_right_pixels() {
        let reserved = Rect::from_corners(Vec2::new(0.0, 360.0), Vec2::new(800.0, 450.0));
        let strip = strip_rect(reserved, 2.0);
        assert!((strip.max.x - 1600.0).abs() < 1e-3);
        assert!((strip.max.y - 900.0).abs() < 1e-3);
    }

    /// The names column has to fit inside whatever the strip was given, or the
    /// lanes start off the right-hand edge and nothing is drawn at all.
    #[test]
    fn the_names_column_fits_the_narrowest_strip() {
        use crate::anim::timeline::NAMES_WIDTH;
        let view = TimelineView {
            rect: Rect::from_corners(Vec2::new(0.0, 0.0), Vec2::new(NAMES_WIDTH * 2.0, 40.0)),
            ..TimelineView::default()
        };
        assert!(view.lanes_left() < view.rect.max.x);
    }
}
