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
//! over the nodes.

pub mod timeline;
pub mod ui;

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::common::app_mode::AppMode;
use crate::common::skeleton::play::Playing;
use crate::prop::document::PropEditor;
use crate::prop::figure::FigurePerformance;
use crate::prop::view::PropSceneSystems;
use std::sync::Arc;

use timeline::{
    lay_out_the_timeline, size_the_strip_camera, spawn_the_strip, take_the_strip_down, Grab,
    ShownClip, TimelineCursor, TimelineGrab, TimelinePools, TimelineView,
};

/// Which clip is being looked at.
///
/// A name rather than an index: a clip lives in a `BTreeMap` keyed by the name
/// the game asks for it by, and an index into a sorted map is a fact about
/// what else is in the map.
#[derive(Resource, Debug, Default)]
pub struct AnimEditor {
    pub selected: Option<String>,
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
            .init_resource::<TimelinePools>()
            .init_resource::<ShownClip>()
            .init_resource::<FigurePerformance>()
            .init_resource::<StartInAnimEditor>()
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
                    follow_the_document,
                    timeline_input,
                    size_the_strip_camera,
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

/// Pick a clip and fit it across the strip on the way in.
///
/// Opening onto nothing is a blank strip with no sense of scale, which is the
/// same argument that has the prop editor open onto the shipped launcher.
fn frame_on_entering(
    editor: Res<PropEditor>,
    mut anim: ResMut<AnimEditor>,
    mut view: ResMut<TimelineView>,
    mut cursor: ResMut<TimelineCursor>,
) {
    let clips = &editor.doc().animations;
    if anim.selected.as_deref().is_none_or(|name| clips.get(name).is_none()) {
        anim.selected = clips.clips.keys().next().cloned();
    }
    let length = anim
        .selected
        .as_deref()
        .and_then(|name| clips.get(name))
        .map_or(1.0, |clip| clip.length);

    cursor.time = 0.0;
    view.frame(length);
}

/// The figure goes back to standing in its hold when the mode closes.
///
/// Not tidiness: the resource outlives the mode, and a figure left holding
/// frame nineteen of a reload is what the *modelling* surface would then show.
fn stop_performing(mut performance: ResMut<FigurePerformance>) {
    performance.0 .0.clear();
}

/// Keep [`ShownClip`] in step with the document.
///
/// Watched through the editor's `generation` counter rather than
/// `Res::is_changed`, for the reason `prop::view` gives about rebuilding on a
/// hover: selecting a feature marks the editor changed, and cloning a clip
/// because somebody moved the mouse is work nobody asked for.
fn follow_the_document(
    editor: Res<PropEditor>,
    anim: Res<AnimEditor>,
    mut shown: ResMut<ShownClip>,
    mut view: ResMut<TimelineView>,
    mut seen: Local<u64>,
) {
    let generation = editor.generation();
    if generation == *seen && !anim.is_changed() {
        return;
    }
    *seen = generation;

    let wanted = anim
        .selected
        .as_deref()
        .and_then(|name| editor.doc().animations.get(name))
        .cloned();

    // A clip picked in the panel is fitted across the strip; one that merely
    // *changed* is left where it was being looked at, because re-framing under
    // somebody mid-edit is worse than a clip running off the edge.
    if anim.is_changed() {
        if let Some(clip) = &wanted {
            view.frame(clip.length);
        }
    }
    shown.0 = wanted;
}

/// Scrub, zoom and pan.
///
/// Deliberately does **not** ask whether the pointer is over egui. The strip
/// *is* an egui panel's rect — that is what reserves the space — so
/// `is_pointer_over_egui` is true across the whole of it, and consulting it
/// would mean the timeline never saw a click. What it asks instead is whether
/// egui is actively *using* the pointer, which is true for an open menu or a
/// widget mid-drag and false for a panel merely sitting there.
fn timeline_input(
    buttons: Res<ButtonInput<MouseButton>>,
    mut scrolls: MessageReader<MouseWheel>,
    windows: Query<&Window, With<PrimaryWindow>>,
    panels: Res<ui::AnimPanels>,
    mut view: ResMut<TimelineView>,
    mut cursor: ResMut<TimelineCursor>,
    mut grab: ResMut<TimelineGrab>,
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

    if over && notches.abs() > 0.0 {
        view.zoom_about(at.x, notches);
    }

    // Finish whatever was running first, so a release is never also a press.
    if !buttons.pressed(MouseButton::Left) && !buttons.pressed(MouseButton::Middle) {
        grab.0 = None;
    }

    if over && buttons.just_pressed(MouseButton::Middle) {
        grab.0 = Some(Grab::Pan {
            from_x: at.x,
            from_start: view.start,
        });
    } else if view.over_lanes(at) && buttons.just_pressed(MouseButton::Left) {
        grab.0 = Some(Grab::Scrub {
            from_x: at.x,
            from_time: cursor.time,
        });
    }

    match grab.0 {
        Some(Grab::Scrub { from_x, from_time }) => {
            let moved = (at.x - from_x) / view.pixels_per_second;
            cursor.time = (from_time + moved).clamp(0.0, length);
        }
        Some(Grab::Pan { from_x, from_start }) => {
            view.start = from_start - (at.x - from_x) / view.pixels_per_second;
        }
        None => {}
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
    mut performance: ResMut<FigurePerformance>,
) {
    performance.0 .0.clear();
    let Some(clip) = shown.0.as_ref() else { return };

    performance.0 .0.push(Playing {
        // Cloned per change rather than held: `ShownClip` is already a copy
        // taken off the document, and a clip is a couple of kilobytes.
        clip: Arc::new(clip.clone()),
        time: cursor.time,
        weight: 1.0,
    });
}
