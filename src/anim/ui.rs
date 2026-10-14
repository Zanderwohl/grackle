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
use crate::anim::body::{BodyClipDialog, BodyClips};
use crate::anim::{edit_selected, selected_clip, AnimEditor, ClipSource};
use crate::common::skeleton::clip::{
    Axis, ChainRef, Channel, Clip, Clock, Combine, Key, Space, StateAnimation, Timing, Track,
    Unit, Variant,
};
use crate::common::skeleton::state::AnimationState;
use crate::common::skeleton::{default_humanoid, Interpolation};
use strum::IntoEnumIterator;
use crate::common::app_mode::{mode_menu, AppMode};
use crate::common::assets::Assets;
use crate::common::file_menu::{file_menu, FileAction};
use crate::common::class::Class;
use crate::common::net::NetRole;
use crate::editor::multicam::{Insets, MulticamState};
use crate::get;
use crate::prop::document::PropEditor;
use crate::prop::ui::PropFileDialog;
use crate::prop::figure::{Anchor, FigureStage, ScaleFigure};
use crate::common::edit_menu::{edit_menu, EditAction, EditAvailability};

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

/// The orange every authoring mode draws a problem in.
const PROBLEM_COLOUR: egui::Color32 = egui::Color32::from_rgb(220, 140, 60);

enum AnimTab {
    Clips,
    /// The clip, the picked track and the picked keys, stacked — one panel,
    /// because a key's numbers mean nothing without the space and unit its
    /// track measures them in, and two tabs are two things never on screen
    /// together.
    Inspector,
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
                AnimTab::Inspector,
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
        app.init_resource::<AnimPanels>()
            .init_resource::<BodyClipDialog>()
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                panels.run_if(in_state(AppMode::Animation)),
            )
            .add_systems(Update, anim_shortcuts.run_if(in_state(AppMode::Animation)));
    }
}

/// The two dialogs and the pack reader, in one parameter.
///
/// Bundled for the reason `PanelMessages` is: `panels` is near Bevy's limit,
/// and these four are one thing — everything answering the File menu needs and
/// nothing a panel does.
#[derive(bevy::ecs::system::SystemParam)]
struct AnimFiles<'w> {
    /// A prop's clips are in its `.gpp`, so its file dialog is the prop
    /// editor's own — the same resource, not a second one.
    prop: Res<'w, PropFileDialog>,
    body: Res<'w, BodyClipDialog>,
    /// Reading goes through `Assets` so it would reach a browser, the way
    /// every other read of pack content does.
    assets: Res<'w, Assets>,
    exit: MessageWriter<'w, AppExit>,
}

struct AnimTabs<'a> {
    editor: &'a mut PropEditor,
    body: &'a mut BodyClips,
    anim: &'a mut AnimEditor,
    figure: &'a mut ScaleFigure,
    stage: &'a mut FigureStage,
    cursor: &'a mut TimelineCursor,
    selection: &'a TimelineSelection,
    mode: &'a mut Option<AppMode>,
    /// What the File menu asked of whichever document the current source
    /// edits, answered outside the closure that drew the menu.
    file: Option<FileAction>,
    /// What the Edit menu asked, likewise.
    edit: Option<EditAction>,
    /// Set when an edit has moved the one picked key, to wherever it landed.
    reselect: Option<KeyAt>,
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
            AnimTab::Inspector => "anim_inspector",
            AnimTab::Problems => "anim_problems",
        })
    }

    fn title(&mut self, tab: &mut AnimTab) -> WidgetText {
        match tab {
            AnimTab::Clips => get!("anim.panels.library"),
            AnimTab::Inspector => get!("anim.panels.inspector"),
            AnimTab::Problems => get!("anim.panels.problems"),
        }
        .into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut AnimTab) {
        match tab {
            AnimTab::Clips => self.library(ui),
            AnimTab::Inspector => self.inspector(ui),
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

    /// Every clip there is to edit, grouped by the file it is saved in.
    ///
    /// **This is the source selector.** Picking a clip picks the document it
    /// lives in, so which one Save writes and which undo stack Ctrl+Z walks
    /// follow what you are looking at rather than a toggle somewhere else.
    fn library(&mut self, ui: &mut Ui) {
        let prop_names: Vec<String> = self.editor.doc().animations.clips.keys().cloned().collect();
        let body_names: Vec<String> = self.body.names();

        let weapon_title = format!(
            "{} — {}",
            get!("anim.library.weapon"),
            prop_file_name(self.editor),
        );
        egui::CollapsingHeader::new(weapon_title)
            .id_salt("anim_library_weapon")
            .default_open(true)
            .show(ui, |ui| {
                for name in &prop_names {
                    self.library_row(ui, ClipSource::Prop, name, &prop_names);
                }
                // The names the game plays a prop's clips by, offered until
                // they exist: a clip under any other name is one nothing will
                // ever play.
                for wanted in PLAYED_PROP_CLIPS {
                    if !prop_names.iter().any(|name| name == wanted) {
                        self.unmade_row(ui, ClipSource::Prop, wanted, wanted.to_string());
                    }
                }
                if ui.small_button("+").on_hover_text(get!("anim.library.new_clip")).clicked() {
                    let name = unused_name("clip", &prop_names);
                    self.make_clip(ClipSource::Prop, &name, String::new());
                }
            });

        egui::CollapsingHeader::new(get!("anim.library.body"))
            .id_salt("anim_library_body")
            .default_open(true)
            .show(ui, |ui| {
                // Every state, in the enum's order, whether or not it has a
                // clip — a state with none is still played, by its built-in
                // pose, and that is worth seeing rather than inferring.
                // A copy, because the rows edit the document they are drawn
                // from. A handful of clips is a few kilobytes.
                let animations = self.body.animations().clone();
                for state in AnimationState::iter() {
                    let name = state.clip_name();
                    match animations.get(name) {
                        None => self.unmade_row(
                            ui,
                            ClipSource::Body,
                            name,
                            format!("animation.states.{name}"),
                        ),
                        Some(StateAnimation::Single(_)) => {
                            self.library_row(ui, ClipSource::Body, name, &body_names)
                        }
                        Some(StateAnimation::Blend(variants)) => {
                            self.blend_rows(ui, state, variants)
                        }
                    }
                }
                // A file under a name no state has, which nothing will play.
                for name in animations.keys() {
                    if BodyClips::state_of(name).is_none() {
                        self.library_row(ui, ClipSource::Body, name, &body_names);
                    }
                }
            });
    }

    /// One clip that exists: pick it, or act on it from its menu.
    fn library_row(&mut self, ui: &mut Ui, source: ClipSource, name: &str, siblings: &[String]) {
        let picked = self.anim.source == source && self.anim.selected.as_deref() == Some(name);
        let label = match source {
            ClipSource::Body => BodyClips::state_of(name)
                .map_or_else(|| name.to_string(), |state| state.name()),
            ClipSource::Prop => name.to_string(),
        };
        let row = ui.selectable_label(picked, label);
        if row.clicked() && !picked {
            self.pick(source, name);
        }
        row.context_menu(|ui| {
            if source == ClipSource::Prop {
                let id = Id::new(("anim_rename", name));
                let mut typed = ui
                    .data_mut(|data| data.get_temp::<String>(id))
                    .unwrap_or_else(|| name.to_string());
                let field = ui.text_edit_singleline(&mut typed);
                let taken = typed != name && siblings.iter().any(|had| *had == typed);
                let usable = !typed.trim().is_empty() && !taken && typed != name;
                ui.data_mut(|data| data.insert_temp(id, typed.clone()));
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui
                    .add_enabled(usable, egui::Button::new(get!("anim.library.rename")))
                    .clicked()
                    || (entered && usable)
                {
                    self.rename_prop_clip(name, typed.trim());
                    ui.data_mut(|data| data.remove::<String>(id));
                    ui.close_kind(UiKind::Menu);
                }
                ui.separator();
                if ui.button(get!("anim.library.duplicate")).clicked() {
                    let copy = unused_name(&format!("{name}_"), siblings);
                    if let Some(clip) = self.editor.doc().animations.get(name).cloned() {
                        self.editor.begin_gesture();
                        self.editor.doc_mut().animations.clips.insert(copy.clone(), clip);
                        self.pick(ClipSource::Prop, &copy);
                    }
                    ui.close_kind(UiKind::Menu);
                }
                if ui.button(get!("anim.library.delete")).clicked() {
                    self.editor.begin_gesture();
                    self.editor.doc_mut().animations.clips.remove(name);
                    ui.close_kind(UiKind::Menu);
                }
            } else {
                // A body clip's name is the state it plays for, so there is no
                // renaming one — and deleting one is putting the state back on
                // its built-in pose, so that is what it is called.
                if ui.button(get!("anim.library.blend")).clicked() {
                    let state = name.to_string();
                    self.body.edit(|clips| {
                        if let Some(animation) = clips.get_mut(&state) {
                            crate::anim::body::blend_by_heading(animation);
                        }
                    });
                    // The straight-ahead variant, which is the clip it was.
                    self.pick(ClipSource::Body, &crate::anim::body::variant_name(name, 1));
                    ui.close_kind(UiKind::Menu);
                }
                if ui.button(get!("anim.library.revert")).clicked() {
                    let name = name.to_string();
                    self.body.edit(|clips| clips.remove(&name));
                    ui.close_kind(UiKind::Menu);
                }
            }
        });
    }

    /// A blended state: its name, and a row per variant beneath it.
    fn blend_rows(&mut self, ui: &mut Ui, state: AnimationState, variants: &[Variant]) {
        let name = state.clip_name();
        let header = ui.add(egui::Label::new(state.name()).selectable(false).sense(egui::Sense::click()));
        header.context_menu(|ui| {
            if ui.button(get!("anim.library.revert")).clicked() {
                let name = name.to_string();
                self.body.edit(|clips| clips.remove(&name));
                ui.close_kind(UiKind::Menu);
            }
        });

        ui.indent(("anim_variants", name), |ui| {
            for (index, variant) in variants.iter().enumerate() {
                let address = crate::anim::body::variant_name(name, index);
                let picked = self.anim.source == ClipSource::Body
                    && self.anim.selected.as_deref() == Some(address.as_str());
                let label = get!(
                    "anim.library.variant",
                    "heading",
                    format!("{:+.0}°", variant.heading.to_degrees())
                );
                let row = ui.selectable_label(picked, label);
                if row.clicked() && !picked {
                    self.pick(ClipSource::Body, &address);
                }
                row.context_menu(|ui| {
                    if ui.button(get!("anim.library.duplicate")).clicked() {
                        let mut landed = None;
                        self.body.edit(|clips| {
                            if let Some(StateAnimation::Blend(variants)) = clips.get_mut(name) {
                                landed = Some(crate::anim::body::duplicate_variant(variants, index));
                            }
                        });
                        if let Some(landed) = landed {
                            self.pick(ClipSource::Body, &crate::anim::body::variant_name(name, landed));
                        }
                        ui.close_kind(UiKind::Menu);
                    }
                    // The last variant is the blend; removing it is reverting
                    // the state, which the state's own menu says.
                    if ui
                        .add_enabled(variants.len() > 1, egui::Button::new(get!("anim.library.remove_variant")))
                        .clicked()
                    {
                        self.body.edit(|clips| {
                            if let Some(StateAnimation::Blend(variants)) = clips.get_mut(name) {
                                variants.remove(index);
                            }
                        });
                        self.clear_keys = true;
                        ui.close_kind(UiKind::Menu);
                    }
                });
            }
        });
    }

    /// A clip the game would play that does not exist yet.
    fn unmade_row(&mut self, ui: &mut Ui, source: ClipSource, name: &str, name_key: String) {
        ui.horizontal(|ui| {
            let label = match source {
                ClipSource::Body => BodyClips::state_of(name)
                    .map_or_else(|| name.to_string(), |state| state.name()),
                ClipSource::Prop => name.to_string(),
            };
            ui.add(egui::Label::new(egui::RichText::new(label).weak()).selectable(false))
                .on_hover_text(get!("anim.library.built_in"));
            if ui
                .small_button("+")
                .on_hover_text(get!("anim.library.animate"))
                .clicked()
            {
                self.make_clip(source, name, name_key);
            }
        });
    }

    fn make_clip(&mut self, source: ClipSource, name: &str, name_key: String) {
        let clip = Clip { name_key, length: 1.0, ..Clip::default() };
        match source {
            ClipSource::Prop => {
                self.editor.begin_gesture();
                self.editor.doc_mut().animations.clips.insert(name.to_string(), clip);
            }
            ClipSource::Body => {
                let name = name.to_string();
                self.body.edit(|clips| clips.insert(name, clip.into()));
            }
        }
        self.pick(source, name);
    }

    fn rename_prop_clip(&mut self, from: &str, to: &str) {
        self.editor.begin_gesture();
        let clips = &mut self.editor.doc_mut().animations.clips;
        if let Some(clip) = clips.remove(from) {
            clips.insert(to.to_string(), clip);
        }
        if self.anim.source == ClipSource::Prop && self.anim.selected.as_deref() == Some(from) {
            self.anim.selected = Some(to.to_string());
        }
    }

    /// Pick a clip, and with it the document it lives in.
    fn pick(&mut self, source: ClipSource, name: &str) {
        self.anim.source = source;
        self.anim.selected = Some(name.to_string());
        // Tracks and keys are addressed by index, and an index means nothing
        // in another clip.
        self.anim.track = None;
        self.clear_keys = true;
    }

    /// Everything about what is picked, outermost first.
    ///
    /// Shown as deep as the selection reaches: a clip always, its track once
    /// one is picked, and keys once those are. The sections are collapsible so
    /// somebody who only ever retimes keys can fold the rest away, and open by
    /// default because the first time is when you need to see what there is.
    fn inspector(&mut self, ui: &mut Ui) {
        let Some(clip) = self.clip() else {
            ui.label(get!("anim.no_clips"));
            return;
        };

        let name = self.anim.selected.clone().unwrap_or_default();
        egui::CollapsingHeader::new(format!("{} — {name}", get!("anim.section.clip")))
            .id_salt("anim_section_clip")
            .default_open(true)
            .show(ui, |ui| self.clip_section(ui, &clip));

        let picked = self.anim.track.and_then(|index| clip.tracks.get(index));
        let title = match picked {
            Some(track) => format!(
                "{} — {}",
                get!("anim.section.track"),
                crate::anim::timeline::track_name(track),
            ),
            None => get!("anim.section.track"),
        };
        egui::CollapsingHeader::new(title)
            .id_salt("anim_section_track")
            .default_open(true)
            .show(ui, |ui| self.track_section(ui, &clip));

        egui::CollapsingHeader::new(get!("anim.section.key"))
            .id_salt("anim_section_key")
            .default_open(true)
            .show(ui, |ui| self.key(ui));
    }

    /// How long the clip is, what drives its time, and whether it wraps.
    ///
    /// Here rather than under the clip list, because these are what set the
    /// scale of the strip the clip is being edited on.
    fn clip_section(&mut self, ui: &mut Ui, clip: &Clip) {
        // A variant of a blend says which heading it is for.
        if self.anim.source == ClipSource::Body {
            if let Some((state, Some(index))) = self
                .anim
                .selected
                .as_deref()
                .map(crate::anim::body::address)
                .map(|(state, index)| (state.to_string(), index))
            {
                let heading = match self.body.animations().get(&state) {
                    Some(StateAnimation::Blend(variants)) => variants.get(index).map(|v| v.heading),
                    _ => None,
                };
                if let Some(heading) = heading {
                    ui.horizontal(|ui| {
                        ui.label(get!("anim.label.heading"));
                        let mut degrees = heading.to_degrees();
                        if ui
                            .add(egui::DragValue::new(&mut degrees).speed(0.5).range(-180.0..=180.0).suffix("°"))
                            .changed()
                        {
                            self.body.begin_gesture();
                            if let Some(StateAnimation::Blend(variants)) =
                                self.body.doc_mut().get_mut(&state)
                            {
                                if let Some(variant) = variants.get_mut(index) {
                                    variant.heading = degrees.to_radians();
                                }
                            }
                        }
                    });
                }
            }
        }

        // The why, kept with the numbers. Saved as data because the file is
        // rewritten on every save and a comment would not survive one.
        ui.label(get!("anim.label.note"));
        let mut note = clip.note.clone();
        if ui
            .add(egui::TextEdit::multiline(&mut note).desired_rows(2).desired_width(f32::INFINITY))
            .changed()
        {
            self.edit(|clip| clip.note = note);
        }
    }

    /// Playback, where the playhead is, and the clip's own timing — along the
    /// top of the strip, because these are what set the strip's scale.
    fn transport(&mut self, ui: &mut Ui) {
        let Some(clip) = self.clip() else {
            ui.label(get!("anim.no_clips"));
            return;
        };

        let playing = self.cursor.playing;
        if ui
            .selectable_label(playing, if playing { "⏸" } else { "▶" })
            .clicked()
        {
            crate::anim::toggle_playback(self.cursor, &clip);
        }

        let mut time = self.cursor.time;
        if ui
            .add(
                egui::DragValue::new(&mut time)
                    .speed(0.005)
                    .range(0.0..=clip.length)
                    .max_decimals(3),
            )
            .changed()
        {
            self.cursor.time = time;
            self.cursor.playing = false;
        }
        ui.label("/");

        let mut edited = clip.clone();
        let mut changed = false;
        changed |= ui
            .add(
                egui::DragValue::new(&mut edited.length)
                    .speed(0.01)
                    .range(0.01..=600.0)
                    .max_decimals(3),
            )
            .on_hover_text(get!("anim.label.length"))
            .changed();
        egui::ComboBox::from_id_salt("anim_clock")
            .selected_text(get!(clock_key(edited.clock)))
            .show_ui(ui, |ui| {
                for clock in [Clock::Seconds, Clock::Stride, Clock::Shared] {
                    changed |= ui
                        .selectable_value(&mut edited.clock, clock, get!(clock_key(clock)))
                        .changed();
                }
            });
        changed |= ui.checkbox(&mut edited.looping, get!("anim.label.looping")).changed();
        if changed {
            self.edit(|clip| {
                clip.length = edited.length;
                clip.clock = edited.clock;
                clip.looping = edited.looping;
            });
        }

        // A variant of a blend can be previewed as the whole blend instead.
        if self.anim.source == ClipSource::Body
            && self.anim.selected.as_deref().is_some_and(|name| name.contains('#'))
        {
            ui.separator();
            let mut blending = self.anim.blend_preview.is_some();
            if ui.checkbox(&mut blending, get!("anim.label.blend_preview")).changed() {
                self.anim.blend_preview = blending.then_some(0.0);
            }
            if let Some(heading) = self.anim.blend_preview.as_mut() {
                let mut degrees = heading.to_degrees();
                if ui
                    .add(egui::DragValue::new(&mut degrees).speed(0.5).range(-180.0..=180.0).suffix("°"))
                    .changed()
                {
                    *heading = degrees.to_radians();
                }
            }
        }

        ui.separator();
        let mut snap = !self.anim.free_drag;
        if ui.checkbox(&mut snap, get!("anim.label.snap")).changed() {
            self.anim.free_drag = !snap;
        }

        ui.separator();
        let mut made: Option<Channel> = None;
        ui.menu_button(get!("anim.add_track"), |ui| {
            for (label, entry) in new_tracks() {
                match entry {
                    NewTrack::One(channel) => {
                        if ui.button(get!(label)).clicked() {
                            made = Some(channel);
                            ui.close();
                        }
                    }
                    NewTrack::Choose(options) => {
                        ui.menu_button(get!(label), |ui| {
                            for (option, channel) in options {
                                if ui.button(option).clicked() {
                                    made = Some(channel);
                                    ui.close();
                                }
                            }
                        });
                    }
                }
            }
        });
        if let Some(channel) = made {
            let mut landed = None;
            self.edit(|clip| {
                clip.tracks.push(Track {
                    combine: Combine::Replace,
                    space: Space::Body,
                    unit: Unit::Metres,
                    enabled: true,
                    timing: Default::default(),
                    channel,
                });
                landed = Some(clip.tracks.len() - 1);
            });
            self.anim.track = landed;
            self.clear_keys = true;
        }
    }

    /// A row of controls over each track's place in the strip's names column.
    ///
    /// **egui over Bevy UI, on purpose.** The strip is Bevy UI because it is a
    /// field of marks laid out from two numbers; a header is a checkbox, a
    /// name, a warning and a menu, which is what egui is for and Bevy UI has
    /// none of. egui paints after the UI pass, so these land on top of the
    /// column's ground and the picked row's highlight, and the strip's own
    /// input never looks at the names column — so the two cannot both answer
    /// one click.
    ///
    /// `strip` is the strip in Bevy's logical pixels, as the layout pass
    /// reads it; `zoom` turns it back into egui points.
    fn track_headers(&mut self, ui: &mut Ui, strip: Rect, zoom: f32) {
        use crate::anim::timeline::{row_top, track_name, NAMES_WIDTH, ROW_HEIGHT};
        let Some(clip) = self.clip() else { return };
        let rig = crate::common::skeleton::humanoid(
            self.figure.0.unwrap_or(Class::Mercenary).proportions(),
        );
        let problems = clip.problems(|bone| rig.index_of(bone).is_some());

        let points = |x: f32, y: f32| egui::pos2(x / zoom, y / zoom);
        let mut asked: Option<(usize, crate::anim::TrackOp)> = None;
        let mut picked_header: Option<usize> = None;

        for (slot, track) in clip.tracks.iter().enumerate() {
            let top = strip.min.y + row_top(slot);
            if top + ROW_HEIGHT > strip.max.y {
                break;
            }
            let rect = egui::Rect::from_min_max(
                points(strip.min.x, top),
                points(strip.min.x + NAMES_WIDTH - 1.0, top + ROW_HEIGHT),
            );

            let row = ui.interact(rect, Id::new(("anim_header", slot)), egui::Sense::click());
            if row.clicked() {
                picked_header = Some(slot);
            }
            row.context_menu(|ui| {
                for op in [
                    crate::anim::TrackOp::Toggle,
                    crate::anim::TrackOp::Duplicate,
                    crate::anim::TrackOp::MoveUp,
                    crate::anim::TrackOp::MoveDown,
                    crate::anim::TrackOp::Remove,
                ] {
                    if ui.button(get!(op.name_key(track.enabled))).clicked() {
                        asked = Some((slot, op));
                        ui.close_kind(UiKind::Menu);
                    }
                }
            });

            let notes: Vec<&str> = problems
                .iter()
                .filter(|problem| problem.track == Some(slot))
                .map(|problem| problem.message.as_str())
                .collect();
            let picked = self.anim.track == Some(slot);

            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(4.0 / zoom, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let mut on = track.enabled;
                    if ui.checkbox(&mut on, "").changed() {
                        asked = Some((slot, crate::anim::TrackOp::Toggle));
                    }
                    let name = egui::RichText::new(track_name(track)).size(11.0);
                    let name = match (track.enabled, picked) {
                        (false, _) => name.weak(),
                        (true, true) => name.strong(),
                        (true, false) => name,
                    };
                    ui.add(egui::Label::new(name).selectable(false).truncate());
                    if !notes.is_empty() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.colored_label(PROBLEM_COLOUR, "⚠")
                                .on_hover_text(notes.join("\n"));
                        });
                    }
                },
            );
        }

        if let Some(slot) = picked_header {
            // A header picks the track, and only the track: keys left picked
            // on another row would have the Inspector describing one track's
            // settings above another track's key.
            self.anim.track = Some(slot);
            self.clear_keys = true;
        }
        if let Some((slot, op)) = asked {
            let mut landed = None;
            self.edit(|clip| landed = crate::anim::apply_track_op(clip, slot, op));
            // Every op but a toggle moves the rows under the key selection,
            // which addresses them by index.
            if op != crate::anim::TrackOp::Toggle {
                self.clear_keys = true;
            }
            self.anim.track = landed;
        }
    }

    /// What the picked track drives, and how its keys are read.
    ///
    /// Tracks are *picked*, added, reordered and removed on the strip — by
    /// their header or by any of their keys. The strip already is the list,
    /// and a second list here was a second selection to keep in step with the
    /// first.
    fn track_section(&mut self, ui: &mut Ui, clip: &Clip) {
        if clip.tracks.is_empty() {
            ui.label(get!("anim.no_tracks"));
            return;
        }
        let Some(index) = self.anim.track.filter(|index| *index < clip.tracks.len()) else {
            ui.label(get!("anim.no_track"));
            return;
        };
        let mut edited = clip.tracks[index].clone();
        let mut changed = false;

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

        // Only a stride clip has feet to time anything by.
        if clip.clock == Clock::Stride {
            ui.horizontal(|ui| {
                ui.label(get!("anim.label.timing"));
                egui::ComboBox::from_id_salt("anim_timing")
                    .selected_text(get!(edited.timing.name_key()))
                    .show_ui(ui, |ui| {
                        for timing in Timing::ALL {
                            changed |= ui
                                .selectable_value(&mut edited.timing, timing, get!(timing.name_key()))
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
            Channel::Effector { chain, pole, .. } => {
                // Which way the elbow or knee points. Absent is the chain's
                // own default, which is the right answer almost always — so it
                // is a choice to override rather than three numbers to fill.
                ui.horizontal(|ui| {
                    let mut set = pole.is_some();
                    if ui.checkbox(&mut set, get!("anim.label.pole")).changed() {
                        // Seeded with the default it replaces, so switching
                        // the override on changes nothing until it is edited.
                        let seed = match chain.is_leg() {
                            true => Vec3::NEG_Z,
                            false => crate::common::skeleton::ik::elbow_pole(chain.side()),
                        };
                        *pole = set.then_some(seed.to_array());
                        changed = true;
                    }
                    if let Some(pole) = pole {
                        for value in pole.iter_mut() {
                            changed |= ui
                                .add(egui::DragValue::new(value).speed(0.01).max_decimals(3))
                                .changed();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(get!("anim.label.chain"));
                    egui::ComboBox::from_id_salt("anim_chain")
                        .selected_text(get!(chain_key(*chain)))
                        .show_ui(ui, |ui| {
                            for option in ChainRef::ALL {
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

        // The first of the selection is what is shown, and a write goes to
        // every picked key of the same kind — which is what makes setting six
        // feet to the floor one gesture rather than six. Only what was
        // *changed* is written, so retiming the ease on six keys does not also
        // flatten them all onto the first one's position.
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
        let original = key;
        let key = &mut key;

        let others = picked
            .iter()
            .filter(|at| {
                clip.tracks
                    .get(at.track)
                    .is_some_and(|track| track.channel.is_rotation() != angles)
            })
            .count();
        if others > 0 {
            ui.label(get!("anim.mixed_keys", "count", others.to_string()));
        }

        let mut changed = false;
        // A single key's time is a number you can type; several keys keep
        // their spacing, and are moved on the strip.
        if picked.len() == 1 {
            ui.horizontal(|ui| {
                ui.label(get!("anim.label.at"));
                let mut at = key.at;
                if ui
                    .add(
                        egui::DragValue::new(&mut at)
                            .speed(0.005)
                            .range(0.0..=clip.length)
                            .max_decimals(3),
                    )
                    .changed()
                {
                    let mut landed = None;
                    let by = at - key.at;
                    self.edit(|clip| {
                        let length = clip.length;
                        if let Some(track) = clip.tracks.get_mut(first.track) {
                            landed = track.channel.nudge(&[first.key], by, length).first().copied();
                        }
                    });
                    // Through `nudge`, so the track is re-sorted and the pick
                    // follows the key to wherever that put it.
                    if let Some(key) = landed {
                        self.reselect = Some(KeyAt { track: first.track, key });
                    }
                }
            });
        } else {
            ui.label(get!("anim.at", "time", format!("{:.3}", key.at)));
        }
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
        let written = KeyEdit::between(&original, key);
        self.edit(|clip| written.write_across(clip, &picked, angles));
    }

    /// Which build stands in the viewports.
    ///
    /// On the menu bar rather than in a tab, because checking a clip authored
    /// in leg-lengths on somebody else's legs is the reason it was authored
    /// that way, and it should cost one click.
    fn preview_picker(&mut self, ui: &mut Ui) {
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

        // Only meaningful with somebody standing there.
        if self.figure.0.is_none() {
            return;
        }
        ui.checkbox(&mut self.stage.holding, get!("anim.label.holding"));
        let before = self.stage.anchor;
        ui.label(get!("anim.label.anchor"));
        egui::ComboBox::from_id_salt("anim_anchor")
            .selected_text(get!(self.stage.anchor.name_key()))
            .show_ui(ui, |ui| {
                for anchor in Anchor::ALL {
                    ui.selectable_value(&mut self.stage.anchor, anchor, get!(anchor.name_key()));
                }
            });
        if self.stage.anchor != before {
            // The figure has moved by a body's height; frame it again once it
            // has been posed where it now stands.
            self.anim.pending_frame = true;
        }
    }

    fn problems(&mut self, ui: &mut Ui) {
        let Some(clip) = self.clip() else { return };
        // The rig the figure is actually wearing, so a bone that exists on one
        // build and not another is reported against the one on screen.
        let rig = crate::common::skeleton::humanoid(
            self.figure.0.unwrap_or(Class::Mercenary).proportions(),
        );
        for problem in clip.problems(|bone| rig.index_of(bone).is_some()) {
            // Named the way the strip names the row, so the two can be read
            // against each other.
            let text = match problem.track.and_then(|index| clip.tracks.get(index)) {
                Some(track) => format!(
                    "{}: {}",
                    crate::anim::timeline::track_name(track),
                    problem.message,
                ),
                None => problem.message,
            };
            ui.colored_label(PROBLEM_COLOUR, text);
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
    mut stage: ResMut<FigureStage>,
    mut cursor: ResMut<TimelineCursor>,
    mut selection: ResMut<TimelineSelection>,
    mut view: ResMut<TimelineView>,
    grab: Res<crate::anim::timeline::TimelineGrab>,
    mut multicam: ResMut<MulticamState>,
    mut next_mode: ResMut<NextState<AppMode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    role: Option<Res<NetRole>>,
    mut files: AnimFiles,
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
        stage: &mut stage,
        cursor: &mut cursor,
        selection: &selection,
        mode: &mut requested,
        clear_keys: false,
        reselect: None,
        file: None,
        edit: None,
    };

    layout.menu_bar_height = egui::Panel::top("anim_menu_bar")
        .resizable(false)
        .show(&mut viewport_ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                // Spelled out rather than read from the state, the way the
                // prop editor's is: this system is gated on the mode it draws
                // for, so which one that is is not in question.
                // Second rather than first, which is the order both other
                // menu bars have.
                file_menu(ui, &mut tabs.file);
                let available = edit_availability(tabs.anim.source, tabs.editor, tabs.body);
                edit_menu(ui, available, &mut tabs.edit);
                mode_menu(ui, AppMode::Animation, role.as_deref(), tabs.mode);
                ui.separator();
                // **What Save will write**, named: the clip, and the document
                // it lives in. Picking a clip in the Library is what changes
                // it, so this is the one place that says so.
                let (document, dirty) = match tabs.anim.source {
                    ClipSource::Prop => (prop_file_name(tabs.editor), tabs.editor.dirty),
                    ClipSource::Body => (get!("anim.library.body_pack"), tabs.body.dirty),
                };
                let clip = tabs
                    .anim
                    .selected
                    .clone()
                    .unwrap_or_else(|| get!("anim.no_clips"));
                ui.label(format!("{clip} — {document}{}", if dirty { "*" } else { "" }));

                ui.separator();
                tabs.preview_picker(ui);
            });
        })
        .response
        .rect
        .height();

    let dock_frame = crate::common::dock::dock_panel_frame(viewport_ui.style());
    layout.left_width = egui::Panel::left("anim_left")
        .resizable(true)
        .frame(dock_frame)
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
        .frame(dock_frame)
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

    // **Reserved, not drawn.** egui owns the *layout* — so the viewports
    // shrink, and the splitter between them and the strip is egui's own resize
    // handle rather than a second one written here — and Bevy UI owns the
    // *pixels*. `Frame::NONE` is what keeps egui from painting over the nodes,
    // and the hover sense allocated inside is what makes
    // `is_pointer_over_egui` true across the strip, which is how an orbit is
    // stopped from starting on it without `prop::camera` learning what a
    // timeline is.
    //
    // The one thing egui *does* paint at the top is the transport, in a frame
    // of its own, and the strip is whatever is left under it.
    let panel = egui::Panel::bottom("anim_timeline")
        .resizable(true)
        .frame(egui::Frame::NONE)
        .default_size(DEFAULT_STRIP)
        .min_size(DEFAULT_STRIP * 0.5)
        .show(&mut viewport_ui, |ui| {
            egui::Frame::new()
                .fill(ui.visuals().panel_fill)
                .inner_margin(egui::Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| tabs.transport(ui));
                });
            let rest = ui.available_rect_before_wrap();
            ui.allocate_rect(rest, egui::Sense::hover());
            rest
        });
    layout.timeline_height = panel.response.rect.height();
    let strip = panel.inner;

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
    tabs.track_headers(&mut viewport_ui, view.rect, ctx.zoom_factor());

    let clear_keys = tabs.clear_keys;
    let reselect = tabs.reselect;
    let asked = tabs.file;
    let edit = tabs.edit;
    drop(tabs);
    if clear_keys {
        selection.0.clear();
    }
    if let Some(key) = reselect {
        selection.0 = [key].into();
    }
    handle_files(asked, anim.source, &mut editor, &mut body, &mut files);
    handle_edit(edit, anim.source, &mut editor, &mut body, &files.assets);

    // **Using the pointer, or with something floating under it** — a menu, a
    // combo's list, a context menu. Not "over egui": the strip is a panel's
    // rect, so that is true across all of it, and the timeline would never
    // see the wheel. Background panels are what the window is tiled with;
    // anything above them is something somebody opened.
    let floating = ctx.pointer_hover_pos().is_some_and(|at| {
        ctx.layer_id_at(at)
            .is_some_and(|layer| layer.order != egui::Order::Background)
    });
    layout.egui_wants_pointer = ctx.egui_is_using_pointer() || floating;
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

/// Answer the File menu for whichever document the current source edits.
///
/// **This is the whole of what "appropriate to the mode" means here**, and the
/// animation editor is the one mode where it is not a single answer: the source
/// selector decides which document the panels are writing to, so it decides
/// what *Save* saves.
///
/// - **A prop's clips live in its `.gpp`**, so every action is the prop
///   editor's own — literally `prop::ui::handle_files`, dialog and all. Saving
///   a clip set and saving the model are the same save, because they are the
///   same file, and an animation editor with a second idea of where that file
///   goes is two paths for one document.
/// - **The body clips are a pack**, one file per state under `animations/`.
///   There is no file to browse for, so *Open* re-reads the packs and *Save As*
///   asks which pack to write into. *New* is an empty set, which is exactly
///   what a pack that ships no animations has — every state back on its Rust
///   function, and one `Ctrl+Z` from where it was.
fn handle_files(
    action: Option<FileAction>,
    source: ClipSource,
    editor: &mut PropEditor,
    body: &mut BodyClips,
    files: &mut AnimFiles,
) {
    // Quit is neither document's business, and is answered before either of
    // them so that it reads the same from any source.
    if action == Some(FileAction::Quit) {
        info!("Quitting, as asked");
        files.exit.write(AppExit::Success);
        return;
    }

    match source {
        ClipSource::Prop => crate::prop::ui::handle_files(action, editor, &files.prop),
        ClipSource::Body => {
            // Polled every frame rather than only when something was asked
            // for: the chooser answers on its own thread, whole seconds after
            // the click that opened it.
            if let Some(pack) = files.body.take() {
                save_the_body_clips(body, &pack);
            }
            match action {
                Some(FileAction::New) => {
                    body.clear();
                    info!("Every state is back on its built-in pose");
                }
                Some(FileAction::Open) => {
                    let count = body.reload(&files.assets);
                    info!("Re-read {count} body animation(s) from the packs");
                }
                Some(FileAction::Save) => save_the_body_clips(body, &BodyClips::default_pack()),
                Some(FileAction::SaveAs) => files.body.ask_where_to_save(),
                Some(FileAction::Quit) | None => {}
            }
        }
    }
}

/// What the Edit menu can do for whichever document the source edits.
///
/// The body clips have no single file — they are a pack — so "something to go
/// back to" is simply having changed them.
fn edit_availability(source: ClipSource, editor: &PropEditor, body: &BodyClips) -> EditAvailability {
    match source {
        ClipSource::Prop => crate::prop::ui::edit_availability(editor),
        ClipSource::Body => EditAvailability {
            undo: body.can_undo(),
            redo: body.can_redo(),
            revert: body.dirty,
        },
    }
}

/// Answer the Edit menu, or one of its chords, for whichever document the
/// source edits. A prop goes to the prop editor's own handler; the body clips
/// revert by re-reading the packs, which `BodyClips::reload` already does as
/// one undo step.
fn handle_edit(
    action: Option<EditAction>,
    source: ClipSource,
    editor: &mut PropEditor,
    body: &mut BodyClips,
    assets: &Assets,
) {
    match source {
        ClipSource::Prop => crate::prop::ui::handle_edit(action, editor),
        ClipSource::Body => match action {
            Some(EditAction::Undo) => body.undo(),
            Some(EditAction::Redo) => body.redo(),
            Some(EditAction::Revert) => {
                let count = body.reload(assets);
                info!("Reverted to the {count} body animation(s) in the packs");
            }
            None => {}
        },
    }
}

fn save_the_body_clips(body: &mut BodyClips, pack: &std::path::Path) {
    match body.save(pack) {
        Ok(count) => info!("Saved {count} body animation(s) to {}", pack.display()),
        Err(why) => warn!("Could not save body animations: {why}"),
    }
}

/// `Ctrl+N`, `Ctrl+S`, `Ctrl+Shift+S` and the undo pair, for the animation
/// editor.
///
/// The other two authoring modes have had these and this one had none, so the
/// chord hints the File menu draws would have been a promise the keyboard did
/// not keep — and `Ctrl+Z` doing nothing at all in a timeline reads as lost
/// work rather than as an absence.
///
/// Which document they reach is the source selector's answer, exactly as the
/// menu's is.
fn anim_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    egui: Res<bevy_egui::input::EguiWantsInput>,
    anim: Res<AnimEditor>,
    mut editor: ResMut<PropEditor>,
    mut body: ResMut<BodyClips>,
    mut files: AnimFiles,
) {
    let chords = crate::common::shortcuts::chords(&keys, egui.wants_keyboard_input());

    // Through the Edit menu's handler, so a chord and its entry are one action.
    let edit = match (chords.redo, chords.undo) {
        (true, _) => Some(EditAction::Redo),
        (false, true) => Some(EditAction::Undo),
        _ => None,
    };
    handle_edit(edit, anim.source, &mut editor, &mut body, &files.assets);

    if let Some(action) = FileAction::from_chords(chords) {
        handle_files(Some(action), anim.source, &mut editor, &mut body, &mut files);
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
    chain.name_key()
}

/// One entry in the Add track menu.
///
/// A track that drives *something* — a bone, a limb — is chosen by what it
/// drives, in a submenu, so it arrives already pointing at the right thing
/// rather than at a placeholder that then has to be changed in the Inspector.
/// The ones that drive the only thing of their kind are a plain item.
enum NewTrack {
    One(Channel),
    /// Each option's label, and the track it makes.
    Choose(Vec<(String, Channel)>),
}

/// Every kind of track that can be added, each with the emptiest form of
/// itself.
///
/// A fifth channel joins by being written here next to the arm in `apply`
/// that answers for it.
fn new_tracks() -> [(&'static str, NewTrack); 5] {
    let bones = default_humanoid()
        .bones()
        .iter()
        .map(|bone| {
            (
                bone.name.to_string(),
                Channel::Rotation {
                    bone: bone.name.to_string(),
                    keys: Vec::new(),
                },
            )
        })
        .collect();
    let limbs = ChainRef::ALL
        .iter()
        .map(|chain| {
            (
                get!(chain.name_key()),
                Channel::Effector {
                    chain: *chain,
                    pole: None,
                    keys: Vec::new(),
                },
            )
        })
        .collect();
    [
        ("anim.channel.rotation", NewTrack::Choose(bones)),
        ("anim.channel.effector", NewTrack::Choose(limbs)),
        ("anim.channel.root", NewTrack::One(Channel::Root { keys: Vec::new() })),
        ("anim.channel.weapon", NewTrack::One(Channel::Weapon { keys: Vec::new() })),
        (
            "anim.channel.weapon_rotation",
            NewTrack::One(Channel::WeaponRotation { keys: Vec::new() }),
        ),
    ]
}

/// What one Inspector edit changed about a key, to be written across a
/// selection.
///
/// **Only what changed.** Writing the whole of the first key's `to` onto every
/// picked key collapses a selection onto one pose the moment somebody touches
/// its ease — and a selection spanning a bone and a hand writes an angle into
/// a coordinate. So the edit names the axes it touched and nothing else.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KeyEdit {
    pub axes: [Option<Axis>; 3],
    pub ease: Option<Interpolation>,
}

impl KeyEdit {
    pub fn between(before: &Key, after: &Key) -> KeyEdit {
        let mut edit = KeyEdit::default();
        for axis in 0..3 {
            if before.to[axis] != after.to[axis] {
                edit.axes[axis] = Some(after.to[axis]);
            }
        }
        if before.ease != after.ease {
            edit.ease = Some(after.ease);
        }
        edit
    }

    /// Write this onto every picked key **of the same kind** — angles onto
    /// angles, places onto places. The time is never written: a selection
    /// edited together changes its shape, not when it happens.
    pub fn write_across(&self, clip: &mut Clip, picked: &[KeyAt], angles: bool) {
        for at in picked {
            let Some(track) = clip.tracks.get_mut(at.track) else { continue };
            if track.channel.is_rotation() != angles {
                continue;
            }
            let Some(key) = track.channel.keys_mut().get_mut(at.key) else { continue };
            for (axis, written) in self.axes.iter().enumerate() {
                if let Some(written) = written {
                    key.to[axis] = *written;
                }
            }
            if let Some(ease) = self.ease {
                key.ease = ease;
            }
        }
    }
}

/// The clips the game plays a prop's animations by.
///
/// The game's own constants rather than a second spelling of them, so a clip
/// the game starts playing is offered here the day it does.
const PLAYED_PROP_CLIPS: &[&str] = &[crate::game::held::DEPLOY];

/// The open prop's file name, or what an unsaved one is called.
fn prop_file_name(editor: &PropEditor) -> String {
    editor
        .path
        .as_ref()
        .and_then(|path| path.file_name())
        .map_or_else(|| get!("anim.library.unsaved"), |name| name.to_string_lossy().into_owned())
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

    fn two_tracks() -> Clip {
        let key = |at: f32, to: [f32; 3]| Key::all(at, to, Interpolation::Linear);
        Clip {
            length: 1.0,
            tracks: vec![
                Track {
                    combine: Combine::Replace,
                    space: Space::Body,
                    unit: Unit::Metres,
                    enabled: true,
                    timing: Default::default(),
                    channel: Channel::Weapon {
                        keys: vec![key(0.0, [0.1, 0.2, 0.3]), key(0.5, [0.4, 0.5, 0.6])],
                    },
                },
                Track {
                    channel: Channel::Rotation {
                        bone: "chest".into(),
                        keys: vec![key(0.0, [1.0, 1.0, 1.0])],
                    },
                    ..Track::rotation("chest")
                },
            ],
            ..Clip::default()
        }
    }

    /// Changing one axis on several keys changes that axis and leaves the
    /// rest of each key where it was.
    ///
    /// The bug this replaced wrote the first key's whole target across the
    /// selection, so touching an ease on two keys collapsed both onto one
    /// position — silently, and only visible on scrubbing.
    #[test]
    fn a_multi_key_edit_writes_only_what_changed() {
        let mut clip = two_tracks();
        let picked = [KeyAt { track: 0, key: 0 }, KeyAt { track: 0, key: 1 }];
        let before = clip.tracks[0].channel.keys()[0];
        let mut after = before;
        after.to[1] = Axis::ON_THE_GROUND;
        after.ease = Interpolation::CubicOut;

        KeyEdit::between(&before, &after).write_across(&mut clip, &picked, false);

        let keys = clip.tracks[0].channel.keys();
        assert_eq!(keys[1].to[0], Axis::Value(0.4), "an untouched axis was overwritten");
        assert_eq!(keys[1].to[2], Axis::Value(0.6), "an untouched axis was overwritten");
        assert_eq!(keys[1].to[1], Axis::ON_THE_GROUND);
        assert_eq!(keys[1].ease, Interpolation::CubicOut);
        assert_eq!(keys[1].at, 0.5, "the time was written across");
    }

    /// An edit to a place never lands on an angle, or the other way round.
    #[test]
    fn a_multi_key_edit_skips_keys_of_another_kind() {
        let mut clip = two_tracks();
        let picked = [KeyAt { track: 0, key: 0 }, KeyAt { track: 1, key: 0 }];
        let before = clip.tracks[0].channel.keys()[0];
        let mut after = before;
        after.to[0] = Axis::Value(9.0);

        KeyEdit::between(&before, &after).write_across(&mut clip, &picked, false);

        assert_eq!(clip.tracks[0].channel.keys()[0].to[0], Axis::Value(9.0));
        assert_eq!(
            clip.tracks[1].channel.keys()[0].to[0],
            Axis::Value(1.0),
            "metres were written into an angle",
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
        let mut kinds = std::collections::BTreeSet::new();
        for (_, entry) in new_tracks() {
            let channels: Vec<Channel> = match entry {
                NewTrack::One(channel) => vec![channel],
                NewTrack::Choose(options) => {
                    assert!(!options.is_empty(), "a submenu with nothing in it");
                    options.into_iter().map(|(_, channel)| channel).collect()
                }
            };
            for channel in channels {
                assert_eq!(channel.key_count(), 0, "a new track arrived with keys on it");
                kinds.insert(channel.kind_key());
            }
        }
        assert_eq!(kinds.len(), 5, "a kind of track cannot be made: {kinds:?}");
    }

    /// The Bone submenu offers every bone the rig has, each making a track on
    /// that bone — not a placeholder bone to be changed afterwards.
    #[test]
    fn the_bone_submenu_offers_every_bone() {
        let rig = default_humanoid();
        let (_, NewTrack::Choose(options)) = &new_tracks()[0] else {
            panic!("bones are chosen from a submenu");
        };
        assert_eq!(options.len(), rig.bones().len());
        for ((label, channel), bone) in options.iter().zip(rig.bones()) {
            assert_eq!(label, bone.name);
            assert!(matches!(channel, Channel::Rotation { bone: named, .. } if named == bone.name));
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
