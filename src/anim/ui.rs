//! The animation editor's panels, and the strip they leave room for.
//!
//! Laid out like the prop editor's, on purpose: the same panels around the
//! same four viewports, so the window does not rearrange itself under somebody
//! swapping modes. What is new is the bottom, which is **reserved and not
//! drawn** — see [`panels`].

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::egui::{Id, LayerId, Ui, UiBuilder, UiKind, WidgetText};
use bevy_egui::{egui, EguiContexts};
use egui_dock::{DockArea, DockState, TabViewer};

use crate::anim::timeline::{KeyAt, TimelineCursor, TimelineSelection, TimelineView};
use crate::anim::body::BodyClips;
use crate::anim::{clip_names, edit_selected, selected_clip, AnimEditor, ClipSource};
use crate::common::skeleton::clip::{
    Axis, ChainRef, Channel, Clip, Clock, Combine, Space, Track, Unit,
};
use crate::common::skeleton::state::AnimationState;
use crate::common::skeleton::{default_humanoid, Interpolation};
use strum::IntoEnumIterator;
use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::class::Class;
use crate::common::net::NetRole;
use crate::editor::multicam::{Insets, MulticamState};
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
    body: &'a mut BodyClips,
    anim: &'a mut AnimEditor,
    figure: &'a mut ScaleFigure,
    cursor: &'a mut TimelineCursor,
    selection: &'a TimelineSelection,
    mode: &'a mut Option<AppMode>,
    /// Set when the menu bar asked for a save, so the writing happens outside
    /// the closure that drew the button.
    save: bool,
    /// Set when an edit has invalidated the key selection — a track removed,
    /// say — so it can be cleared outside the closures that draw the panels,
    /// which only hold it by shared reference.
    clear_keys: bool,
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
    /// The clip on show, whichever source it came from.
    fn clip(&self) -> Option<Clip> {
        selected_clip(self.anim, self.editor, self.body)
    }

    /// Change it, through whichever document owns it.
    fn edit(&mut self, change: impl FnOnce(&mut Clip)) {
        edit_selected(self.anim, self.editor, self.body, change);
    }

    fn clips(&mut self, ui: &mut Ui) {
        // **Which set of clips**, before which clip. A weapon's motion lives
        // in its `.gpp` beside the hold it deviates from; a body's is pack
        // content named by the state it plays for. Same editor, because they
        // are the same kind of thing.
        ui.horizontal(|ui| {
            for source in [ClipSource::Prop, ClipSource::Body] {
                let picked = self.anim.source == source;
                if ui
                    .selectable_label(picked, get!(source.name_key()))
                    .clicked()
                    && !picked
                {
                    self.anim.source = source;
                    // The name means nothing in the other source, and a
                    // selection carried across would point at a clip that is
                    // not there — or worse, at one that is and is unrelated.
                    self.anim.selected = None;
                    self.anim.track = None;
                    self.clear_keys = true;
                }
            }
        });
        ui.separator();

        let names: Vec<String> = clip_names(self.anim, self.editor, self.body);

        // A body clip is one of thirteen fixed states, so there is nothing to
        // create or delete — a state either has an animation or falls back to
        // its built-in pose. Making one is `New`, which fills in the state's
        // own name; the states without a clip are offered below.
        ui.horizontal(|ui| {
            if self.anim.source == ClipSource::Prop
                && ui.button(get!("anim.new_clip")).clicked()
            {
                // Named by counting rather than by asking. A dialog before the
                // first keyframe is a dialog between somebody and the thing
                // they came here to do; the name is editable below.
                let name = unused_name("clip", &names);
                self.editor.begin_gesture();
                self.editor
                    .doc_mut()
                    .animations
                    .clips
                    .insert(name.clone(), Clip { length: 1.0, ..Clip::default() });
                self.anim.selected = Some(name);
            }
            let picked = self.anim.selected.clone();
            if self.anim.source == ClipSource::Prop
                && ui
                    .add_enabled(picked.is_some(), egui::Button::new(get!("anim.delete_clip")))
                    .clicked()
            {
                if let Some(name) = picked {
                    self.editor.begin_gesture();
                    self.editor.doc_mut().animations.clips.remove(&name);
                    self.anim.selected = None;
                }
            }
        });

        if self.anim.source == ClipSource::Body {
            self.offer_the_unanimated_states(ui, &names);
        }
        ui.separator();

        if names.is_empty() {
            ui.label(get!("anim.no_clips"));
            return;
        }
        for name in &names {
            let picked = self.anim.selected.as_deref() == Some(name.as_str());
            if ui.selectable_label(picked, name).clicked() && !picked {
                self.anim.selected = Some(name.clone());
                self.anim.track = None;
                self.clear_keys = true;
            }
        }

        let Some(clip) = self.clip() else { return };
        let mut edited = clip.clone();
        let mut changed = false;

        ui.separator();
        ui.horizontal(|ui| {
            ui.label(get!("anim.label.length"));
            changed |= ui
                .add(
                    egui::DragValue::new(&mut edited.length)
                        .speed(0.01)
                        .range(0.01..=600.0)
                        .max_decimals(3),
                )
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label(get!("anim.label.clock"));
            egui::ComboBox::from_id_salt("anim_clock")
                .selected_text(get!(clock_key(edited.clock)))
                .show_ui(ui, |ui| {
                    for clock in [Clock::Seconds, Clock::Stride, Clock::Shared] {
                        changed |= ui
                            .selectable_value(&mut edited.clock, clock, get!(clock_key(clock)))
                            .changed();
                    }
                });
        });
        changed |= ui.checkbox(&mut edited.looping, get!("anim.label.looping")).changed();

        if changed {
            self.edit(|clip| {
                clip.length = edited.length;
                clip.clock = edited.clock;
                clip.looping = edited.looping;
            });
        }
    }

    /// The states that have no animation yet, each one click from having one.
    ///
    /// Listed rather than left to be guessed: a body clip's name is not free
    /// text, it is one of thirteen, and a file under any other name is one
    /// nothing will ever play.
    fn offer_the_unanimated_states(&mut self, ui: &mut Ui, have: &[String]) {
        let missing: Vec<AnimationState> = AnimationState::iter()
            .filter(|state| !have.iter().any(|name| name == state.clip_name()))
            .collect();
        if missing.is_empty() {
            return;
        }

        egui::ComboBox::from_id_salt("anim_new_body")
            .selected_text(get!("anim.animate_a_state"))
            .show_ui(ui, |ui| {
                for state in missing {
                    if ui.button(state.name()).clicked() {
                        let name = state.clip_name().to_string();
                        self.body.edit(|clips| {
                            clips.insert(
                                name.clone(),
                                Clip {
                                    name_key: format!("animation.states.{name}"),
                                    length: 1.0,
                                    ..Clip::default()
                                },
                            );
                        });
                        self.anim.selected = Some(name);
                        self.anim.track = None;
                        ui.close_kind(UiKind::Menu);
                    }
                }
            });
    }

    /// The tracks a clip has, and how to get another one.
    ///
    /// Adding a track is where the format's choices actually become reachable:
    /// until there is a way to make an effector track in a space and a unit,
    /// "a key holds a target" is a sentence in a file nobody can write.
    fn tracks(&mut self, ui: &mut Ui) {
        let Some(clip) = self.clip() else {
            ui.label(get!("anim.no_clips"));
            return;
        };

        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("anim_add_track")
                .selected_text(get!("anim.add_track"))
                .show_ui(ui, |ui| {
                    for (label, channel) in new_channels() {
                        if ui.button(get!(label)).clicked() {
                            let mut landed = None;
                            self.edit(|clip| {
                                clip.tracks.push(Track {
                                    combine: Combine::Replace,
                                    space: Space::Body,
                                    unit: Unit::Metres,
                                    enabled: true,
                                    channel,
                                });
                                landed = Some(clip.tracks.len() - 1);
                            });
                            self.anim.track = landed;
                            ui.close_kind(UiKind::Menu);
                        }
                    }
                });

            let picked = self.anim.track.filter(|index| *index < clip.tracks.len());
            if ui
                .add_enabled(picked.is_some(), egui::Button::new(get!("anim.delete_track")))
                .clicked()
            {
                if let Some(index) = picked {
                    self.edit(|clip| {
                        clip.tracks.remove(index);
                    });
                    self.anim.track = None;
                    // The key selection addresses tracks by index, so leaving
                    // it alone after a removal would leave it pointing at
                    // whichever track slid down into the gap.
                    self.clear_keys = true;
                }
            }
        });
        ui.separator();

        if clip.tracks.is_empty() {
            ui.label(get!("anim.no_tracks"));
            return;
        }

        for (index, track) in clip.tracks.iter().enumerate() {
            let picked = self.anim.track == Some(index);
            let label = format!(
                "{index}. {} — {}",
                get!(track.channel.kind_key()),
                track.channel.key_count(),
            );
            if ui.selectable_label(picked, label).clicked() {
                self.anim.track = Some(index);
            }
        }

        let Some(index) = self.anim.track.filter(|index| *index < clip.tracks.len()) else {
            return;
        };
        let mut edited = clip.tracks[index].clone();
        let mut changed = false;

        ui.separator();
        changed |= ui
            .checkbox(&mut edited.enabled, get!("anim.label.enabled"))
            .changed();

        // Space and unit mean nothing to a rotation, which is stated in the
        // bone's own rest frame — so they are not offered rather than offered
        // and ignored.
        // A root offset is positional but has no frame to be measured in — it
        // is a displacement of the body from where it would otherwise stand,
        // and hip-heights is the only unit that could mean. So it is left out
        // here with the rotations rather than offered a choice that does
        // nothing.
        let positional = matches!(
            edited.channel,
            Channel::Effector { .. } | Channel::Weapon { .. }
        );
        if positional {
            ui.horizontal(|ui| {
                ui.label(get!("anim.label.space"));
                egui::ComboBox::from_id_salt("anim_space")
                    .selected_text(get!(space_key(edited.space)))
                    .show_ui(ui, |ui| {
                        for space in [Space::Body, Space::Carry, Space::Weapon, Space::Eye] {
                            changed |= ui
                                .selectable_value(&mut edited.space, space, get!(space_key(space)))
                                .changed();
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label(get!("anim.label.unit"));
                egui::ComboBox::from_id_salt("anim_unit")
                    .selected_text(get!(unit_key(edited.unit)))
                    .show_ui(ui, |ui| {
                        for unit in [
                            Unit::Metres,
                            Unit::LegLengths,
                            Unit::ArmLengths,
                            Unit::HipHeights,
                        ] {
                            changed |= ui
                                .selectable_value(&mut edited.unit, unit, get!(unit_key(unit)))
                                .changed();
                        }
                    });
            });
        }

        ui.horizontal(|ui| {
            ui.label(get!("anim.label.combine"));
            egui::ComboBox::from_id_salt("anim_combine")
                .selected_text(get!(combine_key(edited.combine)))
                .show_ui(ui, |ui| {
                    for combine in [Combine::Replace, Combine::Compose] {
                        changed |= ui
                            .selectable_value(
                                &mut edited.combine,
                                combine,
                                get!(combine_key(combine)),
                            )
                            .changed();
                    }
                });
        });

        // Which bone or which limb, depending on what kind of track it is.
        match &mut edited.channel {
            Channel::Rotation { bone, .. } => {
                ui.horizontal(|ui| {
                    ui.label(get!("anim.label.bone"));
                    egui::ComboBox::from_id_salt("anim_bone")
                        .selected_text(bone.clone())
                        .show_ui(ui, |ui| {
                            for name in default_humanoid().bones().iter().map(|b| b.name) {
                                if ui.selectable_label(bone == name, name).clicked()
                                    && bone != name
                                {
                                    *bone = name.to_string();
                                    changed = true;
                                }
                            }
                        });
                });
            }
            Channel::Effector { chain, .. } => {
                ui.horizontal(|ui| {
                    ui.label(get!("anim.label.chain"));
                    egui::ComboBox::from_id_salt("anim_chain")
                        .selected_text(get!(chain_key(*chain)))
                        .show_ui(ui, |ui| {
                            for option in [
                                ChainRef::RightArm,
                                ChainRef::LeftArm,
                                ChainRef::RightLeg,
                                ChainRef::LeftLeg,
                            ] {
                                changed |= ui
                                    .selectable_value(chain, option, get!(chain_key(option)))
                                    .changed();
                            }
                        });
                });
            }
            _ => {}
        }

        if changed {
            self.edit(|clip| {
                if let Some(track) = clip.tracks.get_mut(index) {
                    *track = edited;
                }
            });
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
        let Some(clip) = self.clip() else { return };
        let Some(track) = clip.tracks.get(first.track) else { return };

        // **Every kind of key is the same three optional axes.** What differs
        // is only what they are measured in — angles or a place — which is why
        // there is one inspector rather than one per channel.
        let angles = track.channel.is_rotation();
        let Some(mut key) = track.channel.keys().get(first.key).copied() else {
            return;
        };
        let key = &mut key;

        let mut changed = false;
        ui.label(get!("anim.at", "time", format!("{:.3}", key.at)));
        ui.separator();

        let labels = match angles {
            true => ["anim.axis.pitch", "anim.axis.yaw", "anim.axis.roll"],
            false => ["anim.axis.x", "anim.axis.y", "anim.axis.z"],
        };
        for (index, label) in labels.iter().enumerate() {
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
                        // Only a vertical *place* has a floor under it — not a
                        // sideways one, and certainly not an angle. Offered and
                        // refused would be a control that does nothing; not
                        // offered is an answer.
                        if index == 1 && !angles {
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

                if let Some(stored) = axis.offset() {
                    // Degrees for an angle, for the reason `degrees_ui` exists
                    // one layer along: nobody types radians, and a file full
                    // of them is one nobody can check by eye. Stored in
                    // radians either way — the conversion is the panel's, not
                    // the format's.
                    let mut shown = match angles {
                        true => stored.to_degrees(),
                        false => stored,
                    };
                    let drag = match angles {
                        true => egui::DragValue::new(&mut shown).speed(0.25).max_decimals(2),
                        false => egui::DragValue::new(&mut shown).speed(0.005).max_decimals(4),
                    };
                    if ui.add(drag).changed() {
                        let number = match angles {
                            true => shown.to_radians(),
                            false => shown,
                        };
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
        self.edit(|clip| {
            for picked in &picked {
                let Some(track) = clip.tracks.get_mut(picked.track) else { continue };
                if let Some(key) = track.channel.keys_mut().get_mut(picked.key) {
                    // The time is not written across: a multiple selection
                    // edited together should change its shape, not collapse
                    // onto one moment.
                    key.to = written.to;
                    key.ease = written.ease;
                }
            }
        });
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
        let Some(clip) = self.clip() else { return };
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
    mut body: ResMut<BodyClips>,
    mut anim: ResMut<AnimEditor>,
    mut figure: ResMut<ScaleFigure>,
    mut cursor: ResMut<TimelineCursor>,
    mut selection: ResMut<TimelineSelection>,
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
        body: &mut body,
        anim: &mut anim,
        figure: &mut figure,
        cursor: &mut cursor,
        selection: &selection,
        mode: &mut requested,
        clear_keys: false,
        save: false,
    };

    layout.menu_bar_height = egui::Panel::top("anim_menu_bar")
        .resizable(false)
        .show(&mut viewport_ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                // Spelled out rather than read from the state, the way the
                // prop editor's is: this system is gated on the mode it draws
                // for, so which one that is is not in question.
                mode_menu(ui, AppMode::Animation, role.as_deref(), tabs.mode);
                if ui.button(get!("anim.save")).clicked() {
                    tabs.save = true;
                }
                ui.separator();
                ui.label(match tabs.anim.selected.as_deref() {
                    Some(name) => name.to_string(),
                    None => get!("anim.no_clips"),
                });
                // The same asterisk the prop editor shows, and the same
                // question: whichever document this source writes to.
                let dirty = match tabs.anim.source {
                    ClipSource::Prop => tabs.editor.dirty,
                    ClipSource::Body => tabs.body.dirty,
                };
                if dirty {
                    ui.label("*");
                }
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

    let clear_keys = tabs.clear_keys;
    let asked_to_save = tabs.save;
    drop(tabs);
    if clear_keys {
        selection.0.clear();
    }
    if asked_to_save {
        save_the_clips(anim.source, &mut editor, &mut body);
    }

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

    // Both documents, because the source can be switched mid-session and a
    // gesture left open on the one not showing would fold the next unrelated
    // edit into it.
    // One undo step per gesture, exactly as the prop editor closes its own —
    // **and the timeline's drags count.** egui is not "using the pointer"
    // while somebody drags a key, because the strip is a frameless panel with
    // nothing in it but a hover sense, so without the third clause a drag
    // would close its gesture on every frame and land in the undo stack forty
    // times over. That is the whole reason this system knows a grab exists.
    if !ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input() && grab.0.is_none() {
        editor.end_gesture();
        body.end_gesture();
    }

    if let Some(mode) = requested {
        next_mode.set(mode);
    }

    if let Ok(window) = windows.single() {
        multicam.reserve(
            Insets {
                left: layout.left_width,
                right: layout.right_width,
                top: layout.menu_bar_height,
                // The strip and any dock below it share the bottom inset.
                // There is no bottom dock here, so this is the strip alone —
                // but adding one later means adding its height in, not
                // replacing this.
                bottom: layout.timeline_height,
            },
            window,
        );
    }
}

/// Write whichever document the current source edits.
///
/// The two save quite differently and that is not hidden behind a trait: a
/// prop is one file the editor already has a path for, and the body clips are
/// a directory of them named by state. A trait over "save yourself" would have
/// to invent a shared idea of *where*, which they do not have.
///
/// A prop that has never been written has no path, and **save-as belongs to
/// the prop editor** — it owns the file dialog and the thread it runs on. Said
/// in the log rather than silently doing nothing.
fn save_the_clips(source: ClipSource, editor: &mut PropEditor, body: &mut BodyClips) {
    match source {
        ClipSource::Prop => match editor.path.clone() {
            Some(path) => match crate::prop::document::save(&path, editor.doc()) {
                Ok(()) => {
                    editor.mark_saved(path.clone());
                    info!("Saved {}", path.display());
                }
                Err(why) => warn!("Could not save {}: {why}", path.display()),
            },
            None => warn!(
                "This prop has never been saved; use Save As in the prop editor, \
                 which owns the file dialog",
            ),
        },
        ClipSource::Body => {
            let pack = BodyClips::default_pack();
            match body.save(&pack) {
                Ok(count) => info!("Saved {count} body animation(s) to {}", pack.display()),
                Err(why) => warn!("Could not save body animations: {why}"),
            }
        }
    }
}

fn clock_key(clock: Clock) -> &'static str {
    match clock {
        Clock::Seconds => "anim.clock.seconds",
        Clock::Stride => "anim.clock.stride",
        Clock::Shared => "anim.clock.shared",
    }
}

fn space_key(space: Space) -> &'static str {
    match space {
        Space::Body => "anim.space.body",
        Space::Carry => "anim.space.carry",
        Space::Weapon => "anim.space.weapon",
        Space::Eye => "anim.space.eye",
    }
}

fn unit_key(unit: Unit) -> &'static str {
    match unit {
        Unit::Metres => "anim.unit.metres",
        Unit::LegLengths => "anim.unit.leg_lengths",
        Unit::ArmLengths => "anim.unit.arm_lengths",
        Unit::HipHeights => "anim.unit.hip_heights",
    }
}

fn combine_key(combine: Combine) -> &'static str {
    match combine {
        Combine::Replace => "anim.combine.replace",
        Combine::Compose => "anim.combine.compose",
    }
}

fn chain_key(chain: ChainRef) -> &'static str {
    match chain {
        ChainRef::RightArm => "anim.chain.right_arm",
        ChainRef::LeftArm => "anim.chain.left_arm",
        ChainRef::RightLeg => "anim.chain.right_leg",
        ChainRef::LeftLeg => "anim.chain.left_leg",
    }
}

/// The kinds of track that can be added, each with the emptiest form of
/// itself.
///
/// A list rather than an enum-of-enums: what a new track *is* is one of four
/// literal values, and a fifth channel joins by being written here next to the
/// arm in `apply` that answers for it.
fn new_channels() -> [(&'static str, Channel); 5] {
    [
        (
            "anim.channel.rotation",
            Channel::Rotation {
                bone: crate::common::skeleton::rig::bone::CHEST.to_string(),
                keys: Vec::new(),
            },
        ),
        (
            "anim.channel.effector",
            Channel::Effector {
                chain: ChainRef::RightArm,
                pole: None,
                keys: Vec::new(),
            },
        ),
        ("anim.channel.root", Channel::Root { keys: Vec::new() }),
        ("anim.channel.weapon", Channel::Weapon { keys: Vec::new() }),
        (
            "anim.channel.weapon_rotation",
            Channel::WeaponRotation { keys: Vec::new() },
        ),
    ]
}

/// A name nothing else is using, by counting.
///
/// Counting rather than asking: a dialog before the first keyframe stands
/// between somebody and the thing they opened the editor to do, and the name
/// is editable afterwards.
fn unused_name(stem: &str, taken: &[String]) -> String {
    (1..)
        .map(|n| format!("{stem}{n}"))
        .find(|name| !taken.contains(name))
        .unwrap_or_else(|| stem.to_string())
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

    /// A new clip gets a name nothing else is using, without asking for one.
    ///
    /// The counting matters: `clip1` taken and `clip2` free has to produce
    /// `clip2`, not `clip1` again — a second clip silently replacing the first
    /// through a `BTreeMap` insert is a lost afternoon with no error.
    #[test]
    fn a_new_name_is_one_nothing_else_has() {
        let taken = vec!["clip1".to_string(), "clip3".to_string()];
        assert_eq!(unused_name("clip", &[]), "clip1");
        assert_eq!(unused_name("clip", &taken), "clip2");
        assert_eq!(
            unused_name("clip", &["clip1".into(), "clip2".into()]),
            "clip3",
        );
    }

    /// Every kind of channel can be made from the panel.
    ///
    /// The list is hand-written beside the arms in `apply` that answer for
    /// each, so a fifth channel added to the format and forgotten here would
    /// be one nobody can author. Counted rather than named, because the names
    /// are the lang keys and those are checked elsewhere.
    #[test]
    fn every_kind_of_track_can_be_added() {
        let made = new_channels();
        assert_eq!(made.len(), 5);
        for (_, channel) in &made {
            assert_eq!(channel.key_count(), 0, "a new track arrived with keys on it");
        }
    }

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
