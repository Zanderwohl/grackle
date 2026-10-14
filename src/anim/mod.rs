//! The animation editor: a timeline over the clips a prop carries.
//!
//! A fourth [`AppMode`], not a tab in the prop editor. The two are one scene
//! asked two different questions — what shape is this, and what does it do —
//! so the stage, the lights, the four framed cameras and the rule that sets
//! the world aside are all *shared*, by widening `prop::view`'s run conditions
//! rather than by writing a second copy of any of it. What this mode adds is a
//! strip along the bottom and a body that moves.
//!
//! It edits the same [`PropEditor`] document, which means one open file, one
//! undo stack and one save between the two modes — the argument for a mode
//! over a separate tool.
//!
//! **The strip is Bevy UI under an egui dock**, and the seam that makes that
//! work already existed: the prop editor measures each egui panel and turns
//! the insets into the four cameras' viewport rects. So the bottom panel here
//! is *reserved and not drawn* — `Frame::NONE`, with a hover sense allocated
//! inside it — which gets three things at once. The viewports shrink for it;
//! `EditorInputPlugin` and `prop::camera` both already bail on
//! `is_pointer_over_egui`, so an orbit cannot start on the timeline and
//! neither of them has to learn what a timeline is; and egui paints nothing
//! over the lanes. It does paint the transport along the top of that panel and
//! the track headers over the names column — rows of controls, which Bevy UI
//! has none of — and the strip's own input never reads either place.

pub mod body;
pub mod timeline;
pub mod ui;

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::common::app_mode::AppMode;
use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::play::{key_the_channel, Playing};
use crate::prop::document::PropEditor;
use crate::prop::figure::FigurePerformance;
use crate::prop::view::PropSceneSystems;
use std::sync::Arc;

use timeline::{
    grab_radius, grid_step, key_at, keys_within, lay_out_the_timeline, over_ruler,
    spawn_the_strip, take_the_strip_down, DraggedTrack, Grab, KeyAt,
    ShownClip, TimelineCursor, TimelineGrab, TimelinePools, TimelineSelection, TimelineView,
};

/// Where the clip being edited lives.
///
/// Two homes because there are two genuinely different things: a weapon's
/// motion belongs to the weapon and rides in its `.gpp` beside the hold it
/// deviates from, and a body's belongs to the pack and is named by the state
/// it plays for. One editor, because they are the same kind of thing and a
/// second timeline would be a second set of bugs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClipSource {
    /// The open prop's own animations.
    #[default]
    Prop,
    /// The pack's body animations, one per `AnimationState`.
    Body,
}

/// What the animation editor is looking at.
#[derive(Resource, Debug, Default)]
pub struct AnimEditor {
    /// Which set of clips the panels and the strip are showing.
    pub source: ClipSource,
    /// Which clip, by the name it is filed under.
    ///
    /// A name rather than an index: a clip lives in a `BTreeMap` keyed by the
    /// name the game asks for it by, and an index into a sorted map is a fact
    /// about what else is in the map.
    pub selected: Option<String>,
    /// Which track the panels are showing, if any.
    ///
    /// Separate from the key selection: you pick a track to change what it
    /// *is* — its space, its unit, what it drives — and keys to change when
    /// they happen. Conflating them would mean choosing a track by choosing
    /// one of its keys, which is impossible on a track that has none yet.
    pub track: Option<usize>,
    /// Keys dragged on the strip land anywhere rather than on the grid.
    ///
    /// Stated as the exception so the default — snapping — is what a
    /// `Default` gives. Alt held during a drag is the same thing for one drag.
    pub free_drag: bool,
    /// Preview a blended state's whole blend at this heading, rather than the
    /// variant being edited on its own. Radians; `None` is the variant alone.
    pub blend_preview: Option<f32>,
    /// Still waiting to frame the clip and the figure.
    ///
    /// Neither can be done on the way in. egui has not measured the strip yet,
    /// so the view's rect is zero-sized and fitting a clip across it fits it
    /// across nothing; and `refresh_the_figure` spawns the body through
    /// `Commands`, so there is nobody to frame the cameras on either. Both
    /// wait for the thing they need rather than for a count of frames.
    pending_frame: bool,
}

/// Whether `--anim` was given.
///
/// A resource rather than a different initial state, for the reason
/// [`StartInPropEditor`](crate::prop::StartInPropEditor) is one: entering has
/// to go through `OnEnter(AppMode::Animation)`, where the strip is stood up
/// and the cameras are framed. A state set before the app runs skips all of it.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StartInAnimEditor(pub bool);

fn start_in_the_anim_editor(
    start: Res<StartInAnimEditor>,
    mut next: ResMut<NextState<AppMode>>,
    mut done: Local<bool>,
) {
    if !start.0 || *done {
        return;
    }
    info!("Opening the animation editor, as asked");
    next.set(AppMode::Animation);
    *done = true;
}

pub struct AnimPlugin;

impl Plugin for AnimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimEditor>()
            .init_resource::<TimelineView>()
            .init_resource::<TimelineCursor>()
            .init_resource::<TimelineGrab>()
            .init_resource::<TimelineSelection>()
            .init_resource::<TimelinePools>()
            .init_resource::<ShownClip>()
            .init_resource::<FigurePerformance>()
            .init_resource::<crate::prop::figure::FigureCorrections>()
            .init_resource::<crate::prop::figure::FigureWeapon>()
            .init_resource::<StartInAnimEditor>()
            .init_resource::<body::BodyClips>()
            .init_resource::<crate::common::skeleton::state::ClipLibrary>()
            // Ungated, like the prop editor's own: it has to run in whatever
            // mode the process started in.
            .add_systems(Update, start_in_the_anim_editor)
            .add_plugins(ui::AnimUiPlugin)
            .add_systems(OnEnter(AppMode::Animation), (spawn_the_strip, frame_on_entering).chain())
            .add_systems(OnExit(AppMode::Animation), (take_the_strip_down, stop_performing))
            .add_systems(
                Update,
                (
                    // What is on the strip, before anything asks where to draw
                    // it or what to perform.
                    // Before anything reads them, and only the first time.
                    load_the_body_clips,
                    // After loading, so the first thing it sees is the real
                    // list rather than an empty one.
                    keep_a_clip_selected,
                    // And straight back out to the game, so an idle retimed
                    // here is retimed on every body in the map.
                    body::publish_the_body_clips,
                    frame_when_there_is_something_to_frame,
                    follow_the_document,
                    timeline_input,
                    edit_keys,
                    follow_the_key_selection,
                    play_or_pause,
                    play_the_cursor,
                                    lay_out_the_timeline,
                    // Before the scene poses the figure, so a scrub is seen on
                    // the frame it happened rather than the one after.
                    drive_the_preview.before(PropSceneSystems::Build),
                )
                    .chain()
                    .run_if(in_state(AppMode::Animation)),
            );
    }
}

/// The clip being edited, whichever document it lives in.
///
/// Cloned rather than borrowed, because the two documents are two resources
/// and a borrow of one would tie up the other for the whole of a panel. A clip
/// is a couple of kilobytes and this runs once a frame.
pub fn selected_clip(
    anim: &AnimEditor,
    prop: &PropEditor,
    body: &body::BodyClips,
) -> Option<Clip> {
    let name = anim.selected.as_deref()?;
    match anim.source {
        ClipSource::Prop => prop.doc().animations.get(name).cloned(),
        ClipSource::Body => body.get(name).cloned(),
    }
}

/// Every clip the current source has, in the order it files them.
pub fn clip_names(anim: &AnimEditor, prop: &PropEditor, body: &body::BodyClips) -> Vec<String> {
    match anim.source {
        ClipSource::Prop => prop.doc().animations.clips.keys().cloned().collect(),
        ClipSource::Body => body.names(),
    }
}

/// Change the selected clip, through whichever document owns it.
///
/// **One gesture either way.** Both documents have the same `begin_gesture` /
/// `doc_mut` / `end_gesture` shape, which is why the source can be switched
/// without any caller learning what a `BodyClips` is — and why adding a third
/// source would be one arm here rather than a branch at every write.
pub fn edit_selected(
    anim: &AnimEditor,
    prop: &mut PropEditor,
    body: &mut body::BodyClips,
    change: impl FnOnce(&mut Clip),
) {
    let Some(name) = anim.selected.as_deref() else { return };
    match anim.source {
        ClipSource::Prop => {
            prop.begin_gesture();
            if let Some(clip) = prop.doc_mut().animations.clips.get_mut(name) {
                change(clip);
            }
        }
        ClipSource::Body => {
            body.begin_gesture();
            if let Some(clip) = body.clip_mut(name) {
                change(clip);
            }
        }
    }
}

/// How to tell whether either document has moved.
///
/// Both counters, because the strip shows one source at a time but the *other*
/// can still change underneath — a prop saved in the modelling surface, say —
/// and a single counter would have to be written by two owners.
pub fn documents_at(prop: &PropEditor, body: &body::BodyClips) -> (u64, u64) {
    (prop.generation(), body.generation())
}

/// Read the pack's body animations, once, the first time the mode is open.
///
/// Here rather than at startup because a process that never opens this mode
/// should not pay for them. The *game* gets its own copy through
/// `clip_file::load_body_clips_at_startup`, which is a different path with a
/// different lifetime — this one is a document somebody is editing.
fn load_the_body_clips(assets: Res<crate::common::assets::Assets>, mut clips: ResMut<body::BodyClips>) {
    clips.ensure_loaded(&assets);
}

/// Pick a clip and fit it across the strip on the way in.
///
/// Opening onto nothing is a blank strip with no sense of scale, which is the
/// same argument that has the prop editor open onto the shipped launcher.
fn frame_on_entering(
    mut anim: ResMut<AnimEditor>,
    view: Res<TimelineView>,
    mut cursor: ResMut<TimelineCursor>,
    mut figure: ResMut<crate::prop::figure::ScaleFigure>,
) {
    // **A body by default here**, unlike the modelling surface. The prop
    // editor is about what shape a thing is and a figure is a scale reference
    // you ask for; this is about how a body moves holding it, and an animation
    // editor with nobody in it is a weapon swinging around on its own. Left
    // alone if somebody has already chosen, including "none".
    if figure.is_added() || figure.0.is_none() {
        figure.0 = Some(crate::common::class::Class::Mercenary);
    }
    cursor.time = 0.0;
    anim.pending_frame = true;
    let _ = view;
}

/// Keep the selection on a clip that actually exists.
///
/// **Self-correcting rather than chosen once.** The body clips are read on the
/// first `Update` of the mode, so at `OnEnter` there is nothing to pick yet;
/// switching source invalidates whatever was picked; and deleting a clip
/// leaves the name pointing at nothing. One rule about absence covers all
/// three, which is the same shape `give_bodies_to_whoever_needs_one` takes.
fn keep_a_clip_selected(
    editor: Res<PropEditor>,
    body: Res<body::BodyClips>,
    mut anim: ResMut<AnimEditor>,
) {
    let names = clip_names(&anim, &editor, &body);
    let still_there = anim
        .selected
        .as_deref()
        .is_some_and(|name| names.iter().any(|had| had == name));
    if still_there {
        return;
    }

    let wanted = names.first().cloned();
    // Written only when it differs: `AnimEditor` is watched for changes, and a
    // write every frame with nothing to select would re-frame the strip
    // continuously.
    if anim.selected != wanted {
        anim.selected = wanted;
        anim.track = None;
    }
}

/// Frame again once the figure is actually standing there.
///
/// `OnEnter` cannot do it: `refresh_the_figure` spawns the body through
/// `Commands`, so during the transition the figure does not exist yet and
/// framing sees only the prop — a weapon thirty centimetres across, with a
/// body nearly two metres tall arriving a frame later and filling the screen.
///
/// Keyed on the figure being **present** rather than on a frame count, so it
/// waits exactly as long as it needs to and not a frame more.
fn frame_when_there_is_something_to_frame(
    figures: Query<(), With<crate::prop::figure::ScaleFigureMarker>>,
    editor: Res<PropEditor>,
    clips: Res<body::BodyClips>,
    mut anim: ResMut<AnimEditor>,
    mut view: ResMut<TimelineView>,
    mut frame: MessageWriter<crate::prop::camera::FrameTheProp>,
) {
    if !anim.pending_frame || figures.is_empty() || !view.has_room() {
        return;
    }

    let length = selected_clip(&anim, &editor, &clips).map_or(1.0, |clip| clip.length);
    view.frame(length);

    frame.write(crate::prop::camera::FrameTheProp);
    anim.pending_frame = false;
}

/// The figure goes back to standing in its hold when the mode closes.
///
/// Not tidiness: the resource outlives the mode, and a figure left holding
/// frame nineteen of a reload is what the *modelling* surface would then show.
fn stop_performing(
    mut performance: ResMut<FigurePerformance>,
    mut corrections: ResMut<crate::prop::figure::FigureCorrections>,
    mut held: ResMut<crate::prop::figure::FigureWeapon>,
) {
    performance.0 .0.clear();
    corrections.0 = None;
    // And the prop goes back to its own origin, or the modelling surface would
    // open with the geometry parked wherever a clip left it.
    held.0 = Transform::IDENTITY;
}

/// Keep [`ShownClip`] in step with the document.
///
/// Watched through the editor's `generation` counter rather than
/// `Res::is_changed`, for the reason `prop::view` gives about rebuilding on a
/// hover: selecting a feature marks the editor changed, and cloning a clip
/// because somebody moved the mouse is work nobody asked for.
fn follow_the_document(
    editor: Res<PropEditor>,
    clips: Res<body::BodyClips>,
    anim: Res<AnimEditor>,
    mut shown: ResMut<ShownClip>,
    mut view: ResMut<TimelineView>,
    mut seen: Local<(u64, u64)>,
    mut showing: Local<Option<(ClipSource, String)>>,
) {
    let generation = documents_at(&editor, &clips);
    let picked = anim
        .selected
        .as_ref()
        .map(|name| (anim.source, name.clone()));
    let repicked = picked != *showing;
    if generation == *seen && !repicked {
        return;
    }
    *seen = generation;
    *showing = picked;

    let wanted = selected_clip(&anim, &editor, &clips);

    // A clip picked in the panel is fitted across the strip; one that merely
    // *changed* is left where it was being looked at, because re-framing under
    // somebody mid-edit is worse than a clip running off the edge.
    //
    // **Asked of which clip is picked, not of `AnimEditor` changing.** Picking
    // a track writes the same resource, and so does the panels' mutable borrow
    // every frame — keyed on the resource, the strip snapped back to fit on
    // every frame anybody had zoomed it.
    if repicked {
        if let Some(clip) = &wanted {
            view.frame(clip.length);
        }
    }
    shown.0 = wanted;
}

/// Scrub, zoom, pan, select and drag.
///
/// Deliberately does **not** ask whether the pointer is over egui. The strip
/// *is* an egui panel's rect — that is what reserves the space — so
/// `is_pointer_over_egui` is true across the whole of it, and consulting it
/// would mean the timeline never saw a click. What it asks instead is whether
/// egui is actively *using* the pointer, which is true for an open menu or a
/// widget mid-drag and false for a panel merely sitting there.
///
/// The ruler scrubs and the lanes select. Told apart by where the press
/// landed rather than by what is under it, because a press on an empty lane is
/// the start of a band rather than a jump of the playhead — and "nothing is
/// under the pointer" is exactly when a band is what you meant.
#[allow(clippy::too_many_arguments)]
fn timeline_input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut scrolls: MessageReader<MouseWheel>,
    windows: Query<&Window, With<PrimaryWindow>>,
    panels: Res<ui::AnimPanels>,
    anim: Res<AnimEditor>,
    mut editor: ResMut<PropEditor>,
    mut clips: ResMut<body::BodyClips>,
    mut view: ResMut<TimelineView>,
    mut cursor: ResMut<TimelineCursor>,
    mut grab: ResMut<TimelineGrab>,
    mut selection: ResMut<TimelineSelection>,
    shown: Res<ShownClip>,
) {
    // Read every frame whatever happens: a `MessageReader` has its own cursor,
    // so leaving them unread while the pointer is elsewhere means they all
    // arrive at once the moment it comes back.
    let notches: f32 = scrolls
        .read()
        .map(|scroll| match scroll.unit {
            MouseScrollUnit::Line => scroll.y,
            // Trackpads report pixels, and a pixel is not a notch.
            MouseScrollUnit::Pixel => scroll.y / 40.0,
        })
        .sum();

    let Ok(window) = windows.single() else { return };
    let Some(at) = window.cursor_position() else {
        // The pointer left the window mid-drag. Ending the gesture is the
        // honest answer — a grab that survived would resume from wherever the
        // pointer came back in.
        grab.0 = None;
        return;
    };

    if panels.egui_wants_pointer {
        grab.0 = None;
        return;
    }

    let length = shown.0.as_ref().map_or(1.0, |clip| clip.length);
    let over = view.rect.contains(at);
    let adding = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let free = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

    if over && notches.abs() > 0.0 {
        view.zoom_about(at.x, notches);
    }

    // Finish first, so a release is never also a press. A band is the one
    // gesture whose whole effect happens on the way up.
    if !buttons.pressed(MouseButton::Left) && !buttons.pressed(MouseButton::Middle) {
        if let Some(Grab::Band { from, to }) = grab.0 {
            if let Some(clip) = shown.0.as_ref() {
                if !adding {
                    selection.0.clear();
                }
                selection
                    .0
                    .extend(keys_within(&view, clip, Rect::from_corners(from, to)));
            }
        }
        grab.0 = None;
    }

    if over && buttons.just_pressed(MouseButton::Middle) {
        grab.0 = Some(Grab::Pan {
            from_x: at.x,
            from_start: view.start,
        });
    } else if buttons.just_pressed(MouseButton::Left) && over {
        let on_playhead = (view.x_of(cursor.time) - at.x).abs() <= grab_radius();
        if over_ruler(&view, at) || (on_playhead && at.x >= view.lanes_left()) {
            grab.0 = Some(Grab::Scrub {
                from_x: at.x,
                from_time: cursor.time,
            });
        } else if view.over_lanes(at) {
            let hit = shown.0.as_ref().and_then(|clip| key_at(&view, clip, at));
            match hit {
                Some(hit) => {
                    if adding {
                        if !selection.0.remove(&hit) {
                            selection.0.insert(hit);
                        }
                    } else if !selection.0.contains(&hit) {
                        // Pressing an unselected key selects just it. Pressing
                        // one that is already selected keeps the rest, so a
                        // multiple selection can be dragged as a whole.
                        selection.0.clear();
                        selection.0.insert(hit);
                    }
                    if let Some(clip) = shown.0.as_ref() {
                        grab.0 = Some(Grab::Keys {
                            from_x: at.x,
                            tracks: dragged_tracks(clip, &selection),
                        });
                    }
                }
                None => {
                    if !adding {
                        selection.0.clear();
                    }
                    grab.0 = Some(Grab::Band { from: at, to: at });
                }
            }
        }
    }

    match &grab.0 {
        Some(Grab::Scrub { from_x, from_time }) => {
            // Taking hold of the playhead takes it from the transport too: a
            // scrub fighting playback is a playhead that will not stay put.
            cursor.playing = false;
            let moved = (at.x - from_x) / view.pixels_per_second;
            cursor.time = (from_time + moved).clamp(0.0, length);
        }
        Some(Grab::Pan { from_x, from_start }) => {
            view.start = from_start - (at.x - from_x) / view.pixels_per_second;
        }
        Some(Grab::Keys { from_x, tracks }) => {
            let mut by = (at.x - from_x) / view.pixels_per_second;
            // Snapped by default, and **the offset is snapped rather than each
            // key** — so several keys dragged together keep the spacing they
            // were authored with instead of collapsing onto the grid. Alt is
            // the escape hatch, the way it is in every editor that snaps.
            if !free && !anim.free_drag {
                let step = grid_step(view.pixels_per_second);
                by = (by / step).round() * step;
            }
            nudge_the_selection(&mut editor, &mut clips, &anim, tracks, by, length, &mut selection);
        }
        // The band follows the pointer here rather than in the layout, which
        // has no idea where the pointer is and is better off not learning.
        Some(Grab::Band { from, .. }) => {
            let from = *from;
            grab.0 = Some(Grab::Band { from, to: at });
        }
        None => {}
    }
}

/// The track a key selection implies: the track of its first key.
///
/// `None` for an empty selection, which means "no opinion" rather than "no
/// track" — a track picked by its header has no keys picked and must stay
/// picked.
pub fn track_of_the_selection(selection: &TimelineSelection) -> Option<usize> {
    selection.0.iter().next().map(|picked| picked.track)
}

/// **Picking a key picks its track.**
///
/// One selection rather than two: the Inspector shows a key under its track's
/// settings, and the two are only ever both meaningful if they describe the
/// same row. Run after everything that writes the key selection, so whichever
/// of the strip, `K` or `Delete` changed it, the track follows on the same
/// frame.
fn follow_the_key_selection(selection: Res<TimelineSelection>, mut anim: ResMut<AnimEditor>) {
    if !selection.is_changed() {
        return;
    }
    if let Some(track) = track_of_the_selection(&selection) {
        if anim.track != Some(track) {
            anim.track = Some(track);
        }
    }
}

/// `Space` starts and stops playback.
///
/// Gated on egui wanting the keyboard, for the reason `edit_keys` is: a space
/// typed into the note is a space.
fn play_or_pause(
    keyboard: Res<ButtonInput<KeyCode>>,
    panels: Res<ui::AnimPanels>,
    shown: Res<ShownClip>,
    mut cursor: ResMut<TimelineCursor>,
) {
    if panels.egui_wants_keyboard || !keyboard.just_pressed(KeyCode::Space) {
        return;
    }
    let Some(clip) = shown.0.as_ref() else { return };
    toggle_playback(&mut cursor, clip);
}

/// Start or stop, and start from the top if a one-shot has already finished —
/// pressing play on a clip that has ended and seeing nothing happen reads as
/// the button being broken.
pub fn toggle_playback(cursor: &mut TimelineCursor, clip: &Clip) {
    if cursor.playing {
        cursor.playing = false;
        return;
    }
    if !clip.looping && cursor.time >= clip.length {
        cursor.time = 0.0;
    }
    cursor.playing = true;
}

/// Move the playhead along while playing.
///
/// The preview reads the playhead and nothing else, so playback is the
/// playhead moving on its own — one path into `apply` whether somebody is
/// scrubbing or watching. A stride clip is played at one stride a second,
/// which is a viewing rate rather than any body's walking speed.
fn play_the_cursor(time: Res<Time>, shown: Res<ShownClip>, mut cursor: ResMut<TimelineCursor>) {
    if !cursor.playing {
        return;
    }
    let Some(clip) = shown.0.as_ref() else {
        cursor.playing = false;
        return;
    };
    advance(&mut cursor, clip, time.delta_secs());
}

pub fn advance(cursor: &mut TimelineCursor, clip: &Clip, by: f32) {
    let length = clip.length.max(1e-3);
    let next = cursor.time + by;
    if clip.looping {
        cursor.time = next.rem_euclid(length);
    } else if next >= length {
        // Held on the last frame rather than wrapped, which is what a one-shot
        // does in a match too.
        cursor.time = length;
        cursor.playing = false;
    } else {
        cursor.time = next;
    }
}

/// Something done to a whole track from its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackOp {
    Toggle,
    Duplicate,
    MoveUp,
    MoveDown,
    Remove,
}

impl TrackOp {
    /// What the menu item says: the action, never the state.
    pub fn name_key(self, enabled: bool) -> &'static str {
        match self {
            TrackOp::Toggle if enabled => "anim.track_menu.switch_off",
            TrackOp::Toggle => "anim.track_menu.switch_on",
            TrackOp::Duplicate => "anim.track_menu.duplicate",
            TrackOp::MoveUp => "anim.track_menu.move_up",
            TrackOp::MoveDown => "anim.track_menu.move_down",
            TrackOp::Remove => "anim.track_menu.remove",
        }
    }
}

/// Do it, and say which track should be picked afterwards.
///
/// **Order is meaning**, not tidiness: tracks apply bottom layer first, so a
/// `Compose` above a `Replace` is a recoil on a pose and below it is a recoil
/// thrown away. Which is why moving one is a real operation.
pub fn apply_track_op(clip: &mut Clip, track: usize, op: TrackOp) -> Option<usize> {
    if track >= clip.tracks.len() {
        return None;
    }
    match op {
        TrackOp::Toggle => {
            clip.tracks[track].enabled = !clip.tracks[track].enabled;
            Some(track)
        }
        TrackOp::Duplicate => {
            let copy = clip.tracks[track].clone();
            clip.tracks.insert(track + 1, copy);
            Some(track + 1)
        }
        TrackOp::MoveUp if track > 0 => {
            clip.tracks.swap(track, track - 1);
            Some(track - 1)
        }
        TrackOp::MoveDown if track + 1 < clip.tracks.len() => {
            clip.tracks.swap(track, track + 1);
            Some(track + 1)
        }
        TrackOp::MoveUp | TrackOp::MoveDown => Some(track),
        TrackOp::Remove => {
            clip.tracks.remove(track);
            None
        }
    }
}

/// Turn the current selection into the per-track snapshots a drag works from.
fn dragged_tracks(clip: &Clip, selection: &TimelineSelection) -> Vec<DraggedTrack> {
    let mut tracks: Vec<DraggedTrack> = Vec::new();
    for picked in &selection.0 {
        let Some(track) = clip.tracks.get(picked.track) else {
            continue;
        };
        match tracks.iter_mut().find(|held| held.track == picked.track) {
            Some(held) => held.held.push(picked.key),
            None => tracks.push(DraggedTrack {
                track: picked.track,
                original: track.channel.clone(),
                held: vec![picked.key],
            }),
        }
    }
    tracks
}

/// Apply a drag, and follow the keys to wherever the re-sort put them.
///
/// One `begin_gesture` for the whole drag and one `doc_mut` per frame, which
/// is what makes a pull on a key **one** press of Ctrl+Z rather than forty.
/// The gesture is closed by the panels, which is also why they have to know a
/// drag is running — see `AnimPanels`.
fn nudge_the_selection(
    editor: &mut PropEditor,
    clips: &mut body::BodyClips,
    anim: &AnimEditor,
    tracks: &[DraggedTrack],
    by: f32,
    length: f32,
    selection: &mut TimelineSelection,
) {
    let mut landed = std::collections::BTreeSet::new();

    edit_selected(anim, editor, clips, |clip| {
        for dragged in tracks {
            let Some(track) = clip.tracks.get_mut(dragged.track) else {
                continue;
            };
            // From the original every frame, never from last frame's result:
            // an accumulated drag drifts on every clamp and cannot be dragged
            // back to where it started.
            track.channel = dragged.original.clone();
            for key in track.channel.nudge(&dragged.held, by, length) {
                landed.insert(KeyAt {
                    track: dragged.track,
                    key,
                });
            }
        }
    });

    selection.0 = landed;
}

/// Add a key at the playhead, or take the selected ones away.
///
/// Gated on egui wanting the keyboard, the way `shortcuts` is: `Delete` in a
/// text field deletes a character, and stealing it loses what somebody typed
/// rather than merely looking wrong.
fn edit_keys(
    keyboard: Res<ButtonInput<KeyCode>>,
    panels: Res<ui::AnimPanels>,
    anim: Res<AnimEditor>,
    cursor: Res<TimelineCursor>,
    mut editor: ResMut<PropEditor>,
    mut body: ResMut<body::BodyClips>,
    mut selection: ResMut<TimelineSelection>,
) {
    if panels.egui_wants_keyboard {
        return;
    }

    let removing = keyboard.any_just_pressed([KeyCode::Delete, KeyCode::Backspace]);
    let keying = keyboard.just_pressed(KeyCode::KeyK);
    if !removing && !keying {
        return;
    }

    let time = cursor.time;
    let picked_track = anim.track;
    let mut picked = std::mem::take(&mut selection.0);

    // One step per press, opened and closed here rather than left to the
    // panels: a keypress is not a gesture somebody is still in the middle of.
    edit_selected(&anim, &mut editor, &mut body, |clip| {
    let (length, looping) = (clip.length, clip.looping);
    let selection = &mut picked;

    if removing {
        // Grouped by track and removed back to front, so one index cannot
        // invalidate another.
        for track in 0..clip.tracks.len() {
            let doomed: Vec<usize> = selection
                .iter()
                .filter(|picked| picked.track == track)
                .map(|picked| picked.key)
                .collect();
            if !doomed.is_empty() {
                clip.tracks[track].channel.remove_keys(&doomed);
            }
        }
        selection.clear();
    } else {
        // On every track a selection touches; failing that the picked track;
        // and on all of them when nothing at all is picked — which is what
        // "key everything here" means and what you want the first time you
        // press it.
        let tracks: Vec<usize> = match (selection.is_empty(), picked_track) {
            (true, Some(track)) if track < clip.tracks.len() => vec![track],
            (true, _) => (0..clip.tracks.len()).collect(),
            (false, _) => {
                let mut tracks: Vec<usize> = selection.iter().map(|picked| picked.track).collect();
                tracks.dedup();
                tracks
            }
        };
        let mut landed = std::collections::BTreeSet::new();
        for track in tracks {
            let Some(spec) = clip.tracks.get_mut(track) else { continue };
            let key = key_the_channel(&mut spec.channel, time, length, looping);
            landed.insert(KeyAt { track, key });
        }
        *selection = landed;
    }
    });

    selection.0 = picked;
    editor.end_gesture();
    body.end_gesture();
}

/// What a body would be telling its state at this point of the clip.
///
/// The playhead is on the clip's own clock, so a stride clip's time is
/// strides and becomes the stride a gait's foot targets are read from.
///
/// A gait is also told which way it is going, `heading` off its own facing,
/// so its feet are planted for that direction.
pub fn preview_inputs(
    clip: &Clip,
    time: f32,
    state: crate::common::skeleton::state::AnimationState,
    heading: f32,
) -> crate::common::skeleton::state::PoseInputs {
    use crate::common::skeleton::clip::Clock;
    let stride = match clip.clock {
        Clock::Stride => (time / clip.length.max(1.0)).rem_euclid(1.0),
        Clock::Seconds | Clock::Shared => 0.0,
    };
    let travel = match state.uses_gait() {
        true => {
            let facing = crate::common::skeleton::gait::direction_of(state);
            let (sin, cos) = heading.sin_cos();
            Vec2::new(facing.x * cos - facing.y * sin, facing.x * sin + facing.y * cos)
        }
        false => Vec2::ZERO,
    };
    crate::common::skeleton::state::PoseInputs {
        seconds: time,
        stride,
        travel,
        ..Default::default()
    }
}

/// Put the clip on the reference figure at the playhead.
///
/// Through the **same** `apply` a body in a match goes through, which is the
/// whole argument `documentation/weapons-in-hand.md` makes about holds one
/// layer down: an editor that showed something the game does not perform is
/// worth less than no editor at all.
fn drive_the_preview(
    shown: Res<ShownClip>,
    cursor: Res<TimelineCursor>,
    anim: Res<AnimEditor>,
    clips: Res<body::BodyClips>,
    mut performance: ResMut<FigurePerformance>,
    mut corrections: ResMut<crate::prop::figure::FigureCorrections>,
) {
    performance.0 .0.clear();
    corrections.0 = None;
    let Some(clip) = shown.0.as_ref() else { return };

    // A body clip is played through its state's corrections in a match, so it
    // is previewed through them too — at the heading it is for, so a
    // diagonal variant is seen with the feet stepping diagonally.
    if anim.source == ClipSource::Body {
        let (filed, variant) = anim.selected.as_deref().map_or(("", None), body::address);
        let animation = clips.animations().get(filed);
        if let Some(state) = body::BodyClips::state_of(filed) {
            let blend = match animation {
                Some(crate::common::skeleton::clip::StateAnimation::Blend(variants)) => Some(variants),
                _ => None,
            };
            let heading = match (anim.blend_preview, blend) {
                (Some(heading), Some(_)) => heading,
                (None, Some(variants)) => variant
                    .and_then(|index| variants.get(index))
                    .map_or(0.0, |variant| variant.heading),
                _ => 0.0,
            };
            corrections.0 = Some((state, preview_inputs(clip, cursor.time, state, heading)));

            // The whole blend, as a match plays it: the two variants either
            // side, the upper at the blend weight. `apply_stack` lerps a
            // partial layer, which is the same lerp `animation_pose` makes.
            if let (Some(heading), Some(animation)) = (anim.blend_preview, animation.filter(|_| blend.is_some())) {
                if let Some((first, second)) = animation.at_heading(heading) {
                    performance.0 .0.push(Playing { clip: Arc::new(first.clone()), time: cursor.time, weight: 1.0 });
                    if let Some((other, t)) = second {
                        performance.0 .0.push(Playing { clip: Arc::new(other.clone()), time: cursor.time, weight: t });
                    }
                    return;
                }
            }
        }
    }

    performance.0 .0.push(Playing {
        // Cloned per change rather than held: `ShownClip` is already a copy
        // taken off the document, and a clip is a couple of kilobytes.
        clip: Arc::new(clip.clone()),
        time: cursor.time,
        weight: 1.0,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Switching source cannot leave a selection pointing at nothing, and the
    /// selection re-resolves on its own rather than being chosen once.
    ///
    /// Three cases with one rule, which is why it is asked about absence: the
    /// body clips are read on the first `Update` of the mode so there is
    /// nothing to pick at `OnEnter`; switching source invalidates whatever was
    /// picked; and deleting a clip leaves the name pointing at nothing.
    #[test]
    fn the_selection_settles_on_a_clip_that_exists() {
        let (editor, _) = editor_with_a_clip(&[0.0, 0.5]);
        let mut body = body::BodyClips::default();
        body.edit(|clips| {
            clips.insert("idle".into(), Clip { length: 1.0, ..Clip::default() }.into());
        });

        // Nothing picked yet: the prop's own clip is what there is.
        let mut anim = AnimEditor::default();
        assert_eq!(clip_names(&anim, &editor, &body), vec!["fire".to_string()]);
        assert!(selected_clip(&anim, &editor, &body).is_none());

        anim.selected = Some("fire".into());
        assert!(selected_clip(&anim, &editor, &body).is_some());

        // The same name means nothing on the other side.
        anim.source = ClipSource::Body;
        assert!(selected_clip(&anim, &editor, &body).is_none());
        assert_eq!(clip_names(&anim, &editor, &body), vec!["idle".to_string()]);
    }

    /// An edit goes to whichever document owns the clip, and leaves the other
    /// alone.
    ///
    /// The whole of what the source selector has to get right: two documents,
    /// two undo stacks, one set of panels — and a write that landed in the
    /// wrong one would be an edit you cannot undo and a file you did not mean
    /// to dirty.
    #[test]
    fn an_edit_goes_to_the_document_that_owns_the_clip() {
        let (mut editor, _) = editor_with_a_clip(&[0.0, 0.5]);
        let mut body = body::BodyClips::default();
        body.edit(|clips| {
            clips.insert("idle".into(), Clip { length: 1.0, ..Clip::default() }.into());
        });

        let prop_generation = editor.generation();
        let mut anim = AnimEditor {
            source: ClipSource::Body,
            selected: Some("idle".into()),
            ..AnimEditor::default()
        };

        edit_selected(&anim, &mut editor, &mut body, |clip| clip.length = 2.0);
        assert_eq!(body.get("idle").map(|clip| clip.length), Some(2.0));
        assert_eq!(
            editor.generation(),
            prop_generation,
            "editing a body clip touched the prop",
        );

        anim.source = ClipSource::Prop;
        anim.selected = Some("fire".into());
        edit_selected(&anim, &mut editor, &mut body, |clip| clip.length = 3.0);
        assert_eq!(
            editor.doc().animations.get("fire").map(|clip| clip.length),
            Some(3.0),
        );
        assert_eq!(
            body.get("idle").map(|clip| clip.length),
            Some(2.0),
            "editing a prop clip touched the body clips",
        );
    }

    use crate::common::skeleton::clip::{
        Axis, Channel, Clip, Combine, Key, Space, Track, Unit,
    };
    use crate::common::skeleton::Interpolation;
    use crate::prop::document::PropDoc;
    use timeline::KeyAt;

    fn editor_with_a_clip(at: &[f32]) -> (PropEditor, AnimEditor) {
        let mut doc = PropDoc::new("prop.test");
        doc.animations.clips.insert(
            "fire".into(),
            Clip {
                length: 1.0,
                tracks: vec![Track {
                    combine: Combine::Replace,
                    space: Space::Body,
                    unit: Unit::Metres,
                    enabled: true,
                    timing: Default::default(),
                    channel: Channel::Weapon {
                        keys: at
                            .iter()
                            .map(|at| Key {
                                at: *at,
                                to: [Axis::Value(*at), Axis::Free, Axis::Free],
                                ease: Interpolation::Linear,
                            })
                            .collect(),
                    },
                }],
                ..Clip::default()
            },
        );

        let mut editor = PropEditor::default();
        editor.open(doc, None);
        let anim = AnimEditor {
            source: ClipSource::Prop,
            selected: Some("fire".into()),
            track: None,
            free_drag: false,
            blend_preview: None,
            pending_frame: false,
        };
        (editor, anim)
    }

    fn times(editor: &PropEditor) -> Vec<f32> {
        editor.doc().animations.get("fire").unwrap().tracks[0]
            .channel
            .key_times()
    }

    fn holding(editor: &PropEditor, keys: &[usize]) -> Vec<DraggedTrack> {
        let clip = editor.doc().animations.get("fire").unwrap();
        vec![DraggedTrack {
            track: 0,
            original: clip.tracks[0].channel.clone(),
            held: keys.to_vec(),
        }]
    }

    /// Picking keys implies a track, and picking none implies nothing — so a
    /// track picked by its header is not unpicked by the empty key selection
    /// that comes with it.
    #[test]
    fn the_key_selection_names_its_track_or_says_nothing() {
        let mut selection = TimelineSelection::default();
        assert_eq!(track_of_the_selection(&selection), None);
        selection.0.insert(KeyAt { track: 2, key: 1 });
        selection.0.insert(KeyAt { track: 3, key: 0 });
        assert_eq!(track_of_the_selection(&selection), Some(2));
    }

    /// Moving a track carries the pick with it, and the ends stay put.
    #[test]
    fn track_operations_keep_the_pick_on_the_track() {
        let (mut editor, _) = editor_with_a_clip(&[0.0]);
        let mut clip = editor.doc().animations.get("fire").unwrap().clone();
        clip.tracks.push(Track::rotation("chest"));

        assert_eq!(apply_track_op(&mut clip, 0, TrackOp::MoveDown), Some(1));
        assert!(clip.tracks[0].channel.is_rotation(), "the tracks did not swap");
        assert_eq!(apply_track_op(&mut clip, 1, TrackOp::MoveDown), Some(1));
        assert_eq!(apply_track_op(&mut clip, 0, TrackOp::MoveUp), Some(0));

        assert_eq!(apply_track_op(&mut clip, 0, TrackOp::Duplicate), Some(1));
        assert_eq!(clip.tracks.len(), 3);
        assert_eq!(apply_track_op(&mut clip, 1, TrackOp::Toggle), Some(1));
        assert!(!clip.tracks[1].enabled);
        assert_eq!(apply_track_op(&mut clip, 1, TrackOp::Remove), None);
        assert_eq!(clip.tracks.len(), 2);
        assert_eq!(apply_track_op(&mut clip, 9, TrackOp::Remove), None);
        let _ = &mut editor;
    }

    /// A one-shot stops on its last frame; a loop comes round; and play on a
    /// finished one-shot starts it again.
    #[test]
    fn playback_holds_a_one_shot_and_wraps_a_loop() {
        let mut clip = Clip { length: 1.0, ..Clip::default() };
        let mut cursor = TimelineCursor { time: 0.9, playing: true };
        advance(&mut cursor, &clip, 0.25);
        assert_eq!(cursor.time, 1.0);
        assert!(!cursor.playing, "a finished one-shot kept playing");

        toggle_playback(&mut cursor, &clip);
        assert!(cursor.playing);
        assert_eq!(cursor.time, 0.0, "play on a finished clip did not restart it");

        clip.looping = true;
        cursor.time = 0.9;
        advance(&mut cursor, &clip, 0.25);
        assert!((cursor.time - 0.15).abs() < 1e-5, "a loop did not come round: {}", cursor.time);
        assert!(cursor.playing);
    }

    /// **One drag is one press of Ctrl+Z.**
    ///
    /// A drag writes the document on every frame it moves, so without the
    /// gesture each frame would be its own undo step and undoing a pull on a
    /// key would take forty presses. The failure is not visible while
    /// dragging — it is only felt later, which is why it is worth pinning.
    #[test]
    fn one_drag_is_one_undo_step() {
        let (mut editor, anim) = editor_with_a_clip(&[0.0, 0.5]);
        let mut selection = TimelineSelection::default();
        selection.0.insert(KeyAt { track: 0, key: 0 });
        let tracks = holding(&editor, &[0]);

        // Forty frames of a drag, as one gesture.
        for frame in 1..=40 {
            nudge_the_selection(
                &mut editor,
                &mut body::BodyClips::default(),
                &anim,
                &tracks,
                frame as f32 * 0.005,
                1.0,
                &mut selection,
            );
        }
        editor.end_gesture();

        assert!((times(&editor)[0] - 0.2).abs() < 1e-5, "the drag did not land");

        assert!(editor.can_undo(), "a drag left nothing to undo");
        editor.undo();
        assert_eq!(times(&editor), vec![0.0, 0.5], "one undo did not undo the drag");
        assert!(!editor.can_undo(), "the drag left more than one step behind");
    }

    /// Dragging out and back leaves the clip exactly as it was.
    ///
    /// This is what recomputing from the original every frame buys. An
    /// accumulated drag drifts on every clamp and every re-sort, so a nudge
    /// you talked yourself out of would not come home — and you would not
    /// notice until the numbers were already wrong.
    #[test]
    fn dragging_a_key_and_dragging_it_back_changes_nothing() {
        let (mut editor, anim) = editor_with_a_clip(&[0.1, 0.4, 0.9]);
        let before = times(&editor);

        let mut selection = TimelineSelection::default();
        selection.0.insert(KeyAt { track: 0, key: 1 });
        let tracks = holding(&editor, &[1]);

        // Out past both neighbours, hard into the end, and back to nothing.
        for by in [0.2_f32, 0.6, 2.0, -3.0, -0.4, 0.0] {
            nudge_the_selection(
                &mut editor,
                &mut body::BodyClips::default(),
                &anim,
                &tracks,
                by,
                1.0,
                &mut selection,
            );
        }

        assert_eq!(times(&editor), before, "a drag out and back did not come home");
    }

    /// The selection follows the keys through a re-sort.
    ///
    /// Addressed by index and dragged past a neighbour, a selection that was
    /// not remapped would be holding whichever key inherited the index — so
    /// the next frame would move a different key and leave the one you grabbed
    /// behind. Nothing errors; the keys just start swapping under the pointer.
    #[test]
    fn the_selection_follows_a_key_across_its_neighbour() {
        let (mut editor, anim) = editor_with_a_clip(&[0.1, 0.4, 0.9]);
        let mut selection = TimelineSelection::default();
        selection.0.insert(KeyAt { track: 0, key: 0 });
        let tracks = holding(&editor, &[0]);

        nudge_the_selection(
            &mut editor,
            &mut body::BodyClips::default(),
            &anim,
            &tracks,
            0.5,
            1.0,
            &mut selection,
        );

        assert_eq!(times(&editor), vec![0.4, 0.6, 0.9]);
        assert_eq!(
            selection.0.iter().copied().collect::<Vec<_>>(),
            vec![KeyAt { track: 0, key: 1 }],
            "the selection did not follow the key it was holding",
        );
    }
}
