//! The animation editor: a timeline over a prop's clips and the pack's body clips.
//!
//! Its own [`AppMode`] sharing the prop editor's scene (stage, lights, cameras, world hiding) by
//! widening `prop::view`'s run conditions, and editing the same [`PropEditor`] document, so there is
//! one open file, one undo stack and one save.
//!
//! The strip is Bevy UI under an egui bottom panel that is reserved but not drawn (`Frame::NONE`
//! with a hover sense). The viewports shrink for it, and `EditorInputPlugin` and `prop::camera`
//! already bail on `is_pointer_over_egui`, so an orbit cannot start on the timeline. egui does paint
//! the transport and the track headers there; the strip's input never reads those places.

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

/// A weapon's motion rides in its `.gpp` beside its hold; a body's belongs to the pack, named by its
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClipSource {
    #[default]
    Prop,
    /// One per `AnimationState`.
    Body,
}

#[derive(Resource, Debug, Default)]
pub struct AnimEditor {
    pub source: ClipSource,
    /// A name rather than an index into the sorted `BTreeMap`, which would shift as clips are added.
    pub selected: Option<String>,
    /// Separate from the key selection, so a track with no keys can still be picked.
    pub track: Option<usize>,
    /// The exception, so `Default` snaps. Alt during a drag does the same for one drag.
    pub free_drag: bool,
    /// Preview a blended state's whole blend at this heading. Radians; `None` is the edited variant
    /// alone.
    pub blend_preview: Option<f32>,
    /// Framing cannot happen on entry: egui has not measured the strip yet, and the figure is spawned
    /// through `Commands`. Waits for both rather than for a frame count.
    pending_frame: bool,
}

/// A resource rather than an initial state, so entering goes through `OnEnter(AppMode::Animation)`,
/// as with [`StartInPropEditor`](crate::prop::StartInPropEditor).
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
            // Ungated: it has to run in whatever mode the process started in.
            .add_systems(Update, start_in_the_anim_editor)
            .add_plugins(ui::AnimUiPlugin)
            .add_systems(OnEnter(AppMode::Animation), (spawn_the_strip, frame_on_entering).chain())
            .add_systems(OnExit(AppMode::Animation), (take_the_strip_down, stop_performing))
            .add_systems(
                Update,
                (
                    load_the_body_clips,
                    // After loading, so it sees the real list.
                    keep_a_clip_selected,
                    body::publish_the_body_clips,
                    frame_when_there_is_something_to_frame,
                    follow_the_document,
                    timeline_input,
                    edit_keys,
                    follow_the_key_selection,
                    play_or_pause,
                    play_the_cursor,
                                    lay_out_the_timeline,
                    // Before the scene poses the figure, so a scrub shows on the same frame.
                    drive_the_preview.before(PropSceneSystems::Build),
                )
                    .chain()
                    .run_if(in_state(AppMode::Animation)),
            );
    }
}

/// Cloned rather than borrowed, so a borrow of one document does not tie up the other for a whole
/// panel. A clip is a couple of kilobytes.
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

pub fn clip_names(anim: &AnimEditor, prop: &PropEditor, body: &body::BodyClips) -> Vec<String> {
    match anim.source {
        ClipSource::Prop => prop.doc().animations.clips.keys().cloned().collect(),
        ClipSource::Body => body.names(),
    }
}

/// Both documents share the `begin_gesture` / `doc_mut` / `end_gesture` shape, so callers need not
/// know which owns the clip.
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

/// Both counters, since the source not shown can still change underneath.
pub fn documents_at(prop: &PropEditor, body: &body::BodyClips) -> (u64, u64) {
    (prop.generation(), body.generation())
}

/// Here rather than at startup, so a process that never opens this mode does not pay.
fn load_the_body_clips(assets: Res<crate::common::assets::Assets>, mut clips: ResMut<body::BodyClips>) {
    clips.ensure_loaded(&assets);
}

fn frame_on_entering(
    mut anim: ResMut<AnimEditor>,
    view: Res<TimelineView>,
    mut cursor: ResMut<TimelineCursor>,
    mut figure: ResMut<crate::prop::figure::ScaleFigure>,
) {
    // A body by default here, unlike the modelling surface, unless somebody already chose (including
    // "none").
    if figure.is_added() || figure.0.is_none() {
        figure.0 = Some(crate::common::class::Class::Mercenary);
    }
    cursor.time = 0.0;
    anim.pending_frame = true;
    let _ = view;
}

/// Self-correcting: body clips arrive after `OnEnter`, switching source invalidates the pick, and a
/// deleted clip leaves the name dangling. One rule about absence covers all three.
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
    // Written only when it differs: `AnimEditor` is watched, and writing every frame would re-frame the
    // strip continuously.
    if anim.selected != wanted {
        anim.selected = wanted;
        anim.track = None;
    }
}

/// The figure is spawned through `Commands`, so at `OnEnter` framing would see only the prop. Keyed
/// on the figure being present rather than a frame count.
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

    frame.write(crate::prop::camera::FrameTheProp::default());
    anim.pending_frame = false;
}

/// The resource outlives the mode, and a posed figure would show in the modelling surface.
fn stop_performing(
    mut performance: ResMut<FigurePerformance>,
    mut corrections: ResMut<crate::prop::figure::FigureCorrections>,
    mut held: ResMut<crate::prop::figure::FigureWeapon>,
) {
    performance.0 .0.clear();
    corrections.0 = None;
    // Or the modelling surface would open with the geometry wherever a clip left it.
    held.0 = Transform::IDENTITY;
}

/// On the `generation` counter rather than `Res::is_changed`, which selection and hovers also trip.
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

    // A newly picked clip is fitted across the strip; an edited one is left where it was being looked
    // at. Keyed on which clip is picked, not on `AnimEditor` changing, which happens every frame the
    // panels borrow it.
    if repicked {
        if let Some(clip) = &wanted {
            view.frame(clip.length);
        }
    }
    shown.0 = wanted;
}

/// Does not ask `is_pointer_over_egui`: the strip is an egui panel's rect, so that is true across
/// all of it. Asks whether egui is using the pointer instead (an open menu, a widget mid-drag).
///
/// The ruler scrubs and the lanes select, told apart by where the press landed, so a press on an
/// empty lane starts a band.
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
    // Read every frame: a `MessageReader` keeps its own cursor, so unread scrolls would all arrive when
    // the pointer came back.
    let notches: f32 = scrolls
        .read()
        .map(|scroll| match scroll.unit {
            MouseScrollUnit::Line => scroll.y,
            MouseScrollUnit::Pixel => scroll.y / crate::prop::camera::PIXELS_PER_NOTCH,
        })
        .sum();

    let Ok(window) = windows.single() else { return };
    let Some(at) = window.cursor_position() else {
        // The pointer left the window mid-drag; a surviving grab would resume from wherever it came back.
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

    // Finish first, so a release is never also a press. A band acts on release.
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
                        // Pressing a selected key keeps the rest, so a multiple selection drags as a whole.
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
            // A scrub fighting playback would not stay put.
            cursor.playing = false;
            let moved = (at.x - from_x) / view.pixels_per_second;
            cursor.time = (from_time + moved).clamp(0.0, length);
        }
        Some(Grab::Pan { from_x, from_start }) => {
            view.start = from_start - (at.x - from_x) / view.pixels_per_second;
        }
        Some(Grab::Keys { from_x, tracks }) => {
            let mut by = (at.x - from_x) / view.pixels_per_second;
            // The offset is snapped rather than each key, so keys dragged together keep their spacing. Alt
            // disables snapping.
            if !free && !anim.free_drag {
                let step = grid_step(view.pixels_per_second);
                by = (by / step).round() * step;
            }
            nudge_the_selection(&mut editor, &mut clips, &anim, tracks, by, length, &mut selection);
        }
        Some(Grab::Band { from, .. }) => {
            let from = *from;
            grab.0 = Some(Grab::Band { from, to: at });
        }
        None => {}
    }
}

/// `None` for an empty selection means "no opinion", so a track picked by its header stays picked.
pub fn track_of_the_selection(selection: &TimelineSelection) -> Option<usize> {
    selection.0.iter().next().map(|picked| picked.track)
}

/// Picking a key picks its track. Runs after everything that writes the key selection.
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

/// Not while egui wants the keyboard: a space typed into the note is a space.
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

/// Play on a finished one-shot restarts it.
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

/// Playback only moves the playhead, so playing and scrubbing share one path into `apply`. A stride
/// clip plays at one stride a second, a viewing rate.
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
        // Held on the last frame, as a one-shot does in a match.
        cursor.time = length;
        cursor.playing = false;
    } else {
        cursor.time = next;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackOp {
    Toggle,
    Duplicate,
    MoveUp,
    MoveDown,
    Remove,
}

impl TrackOp {
    /// The action, never the state.
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

/// Returns which track to pick afterwards. Order matters: tracks apply bottom layer first, so a
/// `Compose` above a `Replace` composes onto it and below is discarded.
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

/// One `begin_gesture` per drag and one `doc_mut` per frame, so a drag is one undo step. The panels
/// close the gesture; see `AnimPanels`.
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
            // From the original every frame: an accumulated drag drifts on every clamp and cannot come back to
            // where it started.
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

/// Not while egui wants the keyboard: `Delete` in a text field deletes a character.
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

    // One step per press, opened and closed here, since a keypress is not an ongoing gesture.
    edit_selected(&anim, &mut editor, &mut body, |clip| {
    let (length, looping) = (clip.length, clip.looping);
    let selection = &mut picked;

    if removing {
        // Back to front per track, so one removal cannot shift another's index.
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
        // On every track the selection touches, else the picked track, else all tracks.
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

/// The playhead is on the clip's own clock, so a stride clip's time is strides. `heading` is
/// relative to the gait's facing, so the feet plant for that direction.
pub fn preview_inputs(
    clip: &Clip,
    time: f32,
    state: crate::common::skeleton::state::AnimationState,
    heading: f32,
) -> crate::common::skeleton::state::PoseInputs {
    use crate::common::skeleton::clip::{strides_spanned, Clock};
    let stride = match clip.clock {
        Clock::Stride => (time / strides_spanned(clip.length)).rem_euclid(1.0),
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

/// Through the same `apply` a body in a match uses, so the editor shows what the game performs.
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

    // A body clip is previewed through its state's corrections, as a match plays it, at the variant's
    // heading.
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

            // The whole blend: the two variants either side, the upper at the blend weight, the same lerp
            // `animation_pose` makes.
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
        clip: Arc::new(clip.clone()),
        time: cursor.time,
        weight: 1.0,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Body clips arrive late, switching source invalidates the pick, and deletion leaves it dangling;
    /// the selection must settle on a clip that exists in each case.
    #[test]
    fn the_selection_settles_on_a_clip_that_exists() {
        let (editor, _) = editor_with_a_clip(&[0.0, 0.5]);
        let mut body = body::BodyClips::default();
        body.edit(|clips| {
            clips.insert("idle".into(), Clip { length: 1.0, ..Clip::default() }.into());
        });

        let mut anim = AnimEditor::default();
        assert_eq!(clip_names(&anim, &editor, &body), vec!["fire".to_string()]);
        assert!(selected_clip(&anim, &editor, &body).is_none());

        anim.selected = Some("fire".into());
        assert!(selected_clip(&anim, &editor, &body).is_some());

        anim.source = ClipSource::Body;
        assert!(selected_clip(&anim, &editor, &body).is_none());
        assert_eq!(clip_names(&anim, &editor, &body), vec!["idle".to_string()]);
    }

    /// A write landing in the wrong document would be an edit you cannot undo and a file dirtied by
    /// mistake.
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

    /// A track picked by its header must not be unpicked by the empty key selection that comes with it.
    #[test]
    fn the_key_selection_names_its_track_or_says_nothing() {
        let mut selection = TimelineSelection::default();
        assert_eq!(track_of_the_selection(&selection), None);
        selection.0.insert(KeyAt { track: 2, key: 1 });
        selection.0.insert(KeyAt { track: 3, key: 0 });
        assert_eq!(track_of_the_selection(&selection), Some(2));
    }

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

    /// Without the gesture each frame of a drag would be its own undo step.
    #[test]
    fn one_drag_is_one_undo_step() {
        let (mut editor, anim) = editor_with_a_clip(&[0.0, 0.5]);
        let mut selection = TimelineSelection::default();
        selection.0.insert(KeyAt { track: 0, key: 0 });
        let tracks = holding(&editor, &[0]);

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

    /// Catches a drag accumulating from last frame's result instead of the original, which drifts on
    /// every clamp and re-sort.
    #[test]
    fn dragging_a_key_and_dragging_it_back_changes_nothing() {
        let (mut editor, anim) = editor_with_a_clip(&[0.1, 0.4, 0.9]);
        let before = times(&editor);

        let mut selection = TimelineSelection::default();
        selection.0.insert(KeyAt { track: 0, key: 1 });
        let tracks = holding(&editor, &[1]);

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

    /// A selection not remapped through the re-sort would hold whichever key inherited the index, so
    /// keys swap under the pointer.
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
