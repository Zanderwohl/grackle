//! The timeline strip, in Bevy UI.
//!
//! # Everything is a function of two numbers
//!
//! [`TimelineView::start`] and [`TimelineView::pixels_per_second`] decide where
//! every mark on the strip goes. Zoom and pan change those two and **rebuild
//! nothing**: the layout pass writes new `left`s onto nodes that were already
//! there. `x_of`, `time_at` and `zoom_about` are pure and tested with no App,
//! the same move `AppMode::selectable_from` makes.
//!
//! # Nothing is ever despawned
//!
//! Three pools — gridlines, rows, keys — grow to fit and never shrink, and
//! surplus nodes are switched off with `Display::None` rather than removed.
//! A realistic reload (ten seconds, eight tracks, sixty keys) is about a
//! hundred and twenty entities, allocated once.
//!
//! This is deliberately the opposite of the reference implementation that
//! prompted it, which spawned one UI entity per *time cell* plus one per
//! marker — some four hundred and fifty for ten seconds — despawned them
//! wholesale on any change, and used "the markers are missing" as its dirty
//! flag. Keyframes are sparse; none of that is necessary, and the version that
//! rebuilds cannot be zoomed without allocating.
//!
//! # It has its own camera, and that is not tidiness
//!
//! In an authoring mode there is **no `IsDefaultUiCamera` anywhere** — it is on
//! the play camera, which does not exist here. A UI node with no explicit
//! [`UiTargetCamera`] therefore renders *nowhere*, with no error and no
//! warning: the strip is simply blank. So every node carries one, and the
//! camera sits one below the egui context camera so an egui popup straying
//! over the strip draws on top of it, which is the right way round.

use bevy::camera::visibility::RenderLayers;
#[cfg(test)]
use bevy::ecs::system::RunSystemOnce;
use bevy::camera::{ClearColorConfig, Viewport};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::common::skeleton::clip::Clip;

pub const RULER_HEIGHT: f32 = 18.0;
pub const ROW_HEIGHT: f32 = 18.0;
pub const NAMES_WIDTH: f32 = 150.0;

/// A key is drawn this wide, and picked in a box this wide.
const KEY_SIZE: f32 = 9.0;

/// The playhead is one pixel and its grab zone is thirteen.
///
/// A hit target the width of the thing it grabs is a hit target nobody can
/// hit. The zone is invisible and centred on the line.
const PLAYHEAD_WIDTH: f32 = 1.0;
const GRAB_WIDTH: f32 = 13.0;

/// How many gridlines to keep around. Bounded by how many fit on a strip at a
/// readable spacing rather than by how long a clip is, which is why zooming
/// out does not allocate.
const MAX_GRID: usize = 64;

/// The narrowest a labelled gridline may be before the spacing steps up.
const MIN_GRID_GAP: f32 = 55.0;

/// What part of the clip is on screen, and how big a second is.
#[derive(Resource, Debug, Clone, Copy)]
pub struct TimelineView {
    /// The time at the left edge of the lanes.
    pub start: f32,
    pub pixels_per_second: f32,
    /// Where the strip is in the window, in Bevy's logical pixels — written
    /// from the rect egui reserved for it.
    pub rect: Rect,
}

impl Default for TimelineView {
    fn default() -> Self {
        TimelineView {
            start: 0.0,
            pixels_per_second: 320.0,
            rect: Rect::from_corners(Vec2::ZERO, Vec2::ZERO),
        }
    }
}

impl TimelineView {
    /// Where the lanes begin — past the column of track names.
    pub fn lanes_left(&self) -> f32 {
        self.rect.min.x + NAMES_WIDTH
    }

    pub fn x_of(&self, time: f32) -> f32 {
        self.lanes_left() + (time - self.start) * self.pixels_per_second
    }

    pub fn time_at(&self, x: f32) -> f32 {
        self.start + (x - self.lanes_left()) / self.pixels_per_second
    }

    /// Zoom about a fixed point on screen, so the time under the pointer stays
    /// under the pointer.
    ///
    /// Multiplicative, for the reason `prop::camera`'s zoom already gives: a
    /// step tuned at one scale is imperceptible or catastrophic at the other,
    /// and a clip is looked at both a whole second and a single frame at a
    /// time.
    pub fn zoom_about(&mut self, x: f32, notches: f32) {
        let before = self.time_at(x);
        self.pixels_per_second =
            (self.pixels_per_second * 1.2f32.powf(notches)).clamp(8.0, 8000.0);
        self.start += before - self.time_at(x);
    }

    /// Fit `length` across the lanes, with a little air either side.
    pub fn frame(&mut self, length: f32) {
        let lanes = (self.rect.width() - NAMES_WIDTH).max(1.0);
        let length = length.max(0.01);
        self.pixels_per_second = (lanes * 0.92 / length).clamp(8.0, 8000.0);
        self.start = -(lanes * 0.04) / self.pixels_per_second;
    }

    /// Whether a window point is over the lanes rather than the names column
    /// or off the strip entirely.
    pub fn over_lanes(&self, at: Vec2) -> bool {
        self.rect.contains(at) && at.x >= self.lanes_left()
    }
}

/// Where the playhead is, and whether it is moving on its own.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct TimelineCursor {
    pub time: f32,
    pub playing: bool,
}

/// One key, addressed by where it sits.
///
/// An address rather than an identity, and the consequence is faced rather
/// than avoided: a drag reorders a track, so the address of a key you are
/// holding changes underneath you. [`Channel::nudge`] reports the new indices
/// for exactly that reason, and the selection is remapped from what it says.
/// The alternative is an id per key on disk, existing only so a pointer can
/// hold something for a few hundred milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyAt {
    pub track: usize,
    pub key: usize,
}

/// Which keys are picked.
#[derive(Resource, Debug, Default)]
pub struct TimelineSelection(pub std::collections::BTreeSet<KeyAt>);

/// One track caught up in a drag.
#[derive(Debug, Clone)]
pub struct DraggedTrack {
    pub track: usize,
    /// The channel exactly as it was when the drag began.
    ///
    /// **Every frame recomputes from this rather than accumulating.** A drag
    /// that added this frame's delta to last frame's result drifts on every
    /// clamp and every re-sort, and dragging out to the edge and back would
    /// not come home — which is the difference between a nudge you can undo by
    /// eye and one you have to undo with Ctrl+Z.
    pub original: crate::common::skeleton::clip::Channel,
    /// Which keys, by index **in `original`**, are moving.
    pub held: Vec<usize>,
}

/// A gesture in progress, anchored to where it began.
///
/// **Delta, not mapping.** Setting the time to `time_at(cursor)` makes the
/// playhead teleport to the pointer the moment a grab starts, which is wrong
/// twice over: the grab zone is deliberately wider than the line, so a
/// legitimate grab is usually a few pixels off it, and a scrub that jumps on
/// mouse-down cannot be used to nudge.
#[derive(Debug, Clone)]
pub enum Grab {
    Scrub { from_x: f32, from_time: f32 },
    Pan { from_x: f32, from_start: f32 },
    Keys { from_x: f32, tracks: Vec<DraggedTrack> },
    /// Both corners, because the layout has no idea where the pointer is and
    /// should not have to: the gesture is the thing that knows, so it carries
    /// what it has swept.
    Band { from: Vec2, to: Vec2 },
}

#[derive(Resource, Debug, Default)]
pub struct TimelineGrab(pub Option<Grab>);

#[derive(Component)]
pub struct TimelineCamera;

#[derive(Component)]
struct TimelineRoot;

#[derive(Component)]
struct PlayheadLine;

#[derive(Component)]
struct PlayheadGrab;

#[derive(Component)]
struct GridLine;

#[derive(Component)]
struct GridLabel;

#[derive(Component)]
struct RowStripe;

#[derive(Component)]
struct RowLabel;

#[derive(Component)]
struct KeyMark;

#[derive(Component)]
struct BandBox;

/// The pooled nodes, held by index so the layout pass can address them without
/// a query sort.
#[derive(Resource, Default)]
pub struct TimelinePools {
    camera: Option<Entity>,
    root: Option<Entity>,
    playhead: Option<Entity>,
    grab: Option<Entity>,
    band: Option<Entity>,
    grid: Vec<(Entity, Entity)>,
    rows: Vec<(Entity, Entity)>,
    keys: Vec<Entity>,
}

/// The layer the strip lives on.
///
/// On the camera **and on every node**. A layer on one and not the other is a
/// camera that renders nothing, silently — an empty render is not an error —
/// which is exactly how this was got wrong the first time.
const LAYER: usize = 30;

fn layer() -> RenderLayers {
    RenderLayers::layer(LAYER)
}

fn off() -> Node {
    Node {
        display: Display::None,
        ..default()
    }
}

/// Stand the strip up: a camera, a root, and the two nodes that are always
/// there.
pub fn spawn_the_strip(mut commands: Commands, mut pools: ResMut<TimelinePools>) {
    let camera = commands
        .spawn((
            TimelineCamera,
            Camera2d,
            Camera {
                // One below the egui context camera at `isize::MAX`, so an
                // egui menu dropping over the strip draws on top of it.
                order: isize::MAX - 1,
                // **Composited, not cleared.** A camera at this order with the
                // default clear config paints the whole window its own
                // background first, and the four viewports underneath it
                // vanish. It is drawing a strip over a scene, not a scene.
                clear_color: ClearColorConfig::None,
                ..default()
            },
            layer(),
        ))
        .id();

    let root = commands
        .spawn((
            TimelineRoot,
            UiTargetCamera(camera),
            layer(),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            BackgroundColor(Color::srgb(0.09, 0.09, 0.10)),
        ))
        .id();

    let playhead = commands
        .spawn((
            PlayheadLine,
            UiTargetCamera(camera),
            layer(),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(PLAYHEAD_WIDTH),
                ..default()
            },
            BackgroundColor(Color::srgb(0.95, 0.30, 0.25)),
            GlobalZIndex(3),
        ))
        .id();

    // Invisible, and wider than what it grabs.
    let grab = commands
        .spawn((
            PlayheadGrab,
            UiTargetCamera(camera),
            layer(),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(GRAB_WIDTH),
                ..default()
            },
            GlobalZIndex(4),
        ))
        .id();

    // The rubber band, drawn only while one is being pulled.
    let band = commands
        .spawn((
            BandBox,
            UiTargetCamera(camera),
            layer(),
            off(),
            BackgroundColor(Color::srgba(0.55, 0.70, 0.95, 0.18)),
            GlobalZIndex(1),
        ))
        .id();

    pools.band = Some(band);
    pools.camera = Some(camera);
    pools.root = Some(root);
    pools.playhead = Some(playhead);
    pools.grab = Some(grab);
}

pub fn take_the_strip_down(mut commands: Commands, mut pools: ResMut<TimelinePools>) {
    let mut everything: Vec<Entity> = Vec::new();
    everything.extend(pools.grid.drain(..).flat_map(|(line, label)| [line, label]));
    everything.extend(pools.rows.drain(..).flat_map(|(stripe, label)| [stripe, label]));
    everything.extend(pools.keys.drain(..));
    everything.extend(pools.band.take());
    everything.extend(pools.root.take());
    everything.extend(pools.playhead.take());
    everything.extend(pools.grab.take());
    everything.extend(pools.camera.take());

    for entity in everything {
        // The root's children go with it, but the pools are addressed by
        // index and a stale entity in one is worse than a double despawn —
        // hence draining rather than relying on the hierarchy.
        commands.entity(entity).try_despawn();
    }
}

/// Grow the pools to fit what is about to be drawn.
///
/// Grows and never shrinks. A clip briefly opened with four hundred keys
/// leaves four hundred switched-off nodes behind, which costs a pointer each
/// and saves the allocation when the next one is opened.
fn reserve(
    commands: &mut Commands,
    camera: Entity,
    root: Entity,
    pools: &mut TimelinePools,
    rows: usize,
    keys: usize,
) {
    while pools.grid.len() < MAX_GRID {
        let line = commands
            .spawn((
                GridLine,
                UiTargetCamera(camera),
                layer(),
                ChildOf(root),
                off(),
                BackgroundColor(Color::srgb(0.17, 0.17, 0.19)),
            ))
            .id();
        let label = commands
            .spawn((
                GridLabel,
                UiTargetCamera(camera),
                layer(),
                ChildOf(root),
                off(),
                Text::new(String::new()),
                TextFont {
                    font_size: bevy::text::FontSize::Px(10.0),
                    ..default()
                },
                TextColor(Color::srgb(0.55, 0.55, 0.58)),
            ))
            .id();
        pools.grid.push((line, label));
    }

    while pools.rows.len() < rows {
        let stripe = commands
            .spawn((
                RowStripe,
                UiTargetCamera(camera),
                layer(),
                ChildOf(root),
                off(),
                BackgroundColor(Color::srgb(0.13, 0.13, 0.15)),
            ))
            .id();
        let label = commands
            .spawn((
                RowLabel,
                UiTargetCamera(camera),
                layer(),
                ChildOf(root),
                off(),
                Text::new(String::new()),
                TextFont {
                    font_size: bevy::text::FontSize::Px(11.0),
                    ..default()
                },
                TextColor(Color::srgb(0.78, 0.78, 0.82)),
            ))
            .id();
        pools.rows.push((stripe, label));
    }

    // In chunks, so a clip gaining one key at a time does not spawn one entity
    // per keystroke.
    while pools.keys.len() < keys {
        for _ in 0..64 {
            let key = commands
                .spawn((
                    KeyMark,
                    UiTargetCamera(camera),
                    ChildOf(root),
                    off(),
                    BackgroundColor(Color::srgb(0.85, 0.72, 0.35)),
                    GlobalZIndex(2),
                ))
                .id();
            pools.keys.push(key);
        }
    }
}

/// How close to the playhead counts as grabbing it.
///
/// Half the grab zone, which is deliberately much wider than the line it
/// grabs: a hit target the width of one pixel is a hit target nobody can hit.
pub fn grab_radius() -> f32 {
    GRAB_WIDTH / 2.0
}

/// Where a track's row starts, measured down from the top of the strip.
pub fn row_top(slot: usize) -> f32 {
    RULER_HEIGHT + slot as f32 * ROW_HEIGHT
}

/// Which track row a window point is over, if any.
pub fn row_at(view: &TimelineView, y: f32) -> Option<usize> {
    let local = y - view.rect.min.y - RULER_HEIGHT;
    (local >= 0.0).then(|| (local / ROW_HEIGHT) as usize)
}

/// Whether a window point is in the ruler band along the top.
///
/// The ruler scrubs and the lanes select, which is why they are told apart
/// here rather than by whatever happens to be under the pointer: a click on an
/// empty lane is the start of a band, not a jump of the playhead.
pub fn over_ruler(view: &TimelineView, at: Vec2) -> bool {
    view.rect.contains(at) && at.y < view.rect.min.y + RULER_HEIGHT && at.x >= view.lanes_left()
}

/// Which key a window point is on, if any.
///
/// Geometric rather than picked off the node, and derived from the **same**
/// `x_of` the layout uses — so it is one function read two ways rather than
/// two copies of where a key is. It is also then testable without standing up
/// a renderer, which is what most of the strip's tests rely on.
pub fn key_at(view: &TimelineView, clip: &Clip, at: Vec2) -> Option<KeyAt> {
    let track = row_at(view, at.y)?;
    let channel = &clip.tracks.get(track)?.channel;

    channel
        .key_times()
        .into_iter()
        .enumerate()
        .find(|(_, time)| (view.x_of(*time) - at.x).abs() <= KEY_SIZE)
        .map(|(key, _)| KeyAt { track, key })
}

/// Every key inside a rectangle dragged across the lanes.
pub fn keys_within(view: &TimelineView, clip: &Clip, band: Rect) -> Vec<KeyAt> {
    let mut found = Vec::new();
    for (track, spec) in clip.tracks.iter().enumerate() {
        let top = view.rect.min.y + row_top(track);
        if top + ROW_HEIGHT < band.min.y || top > band.max.y {
            continue;
        }
        for (key, time) in spec.channel.key_times().into_iter().enumerate() {
            let x = view.x_of(time);
            if x >= band.min.x && x <= band.max.x {
                found.push(KeyAt { track, key });
            }
        }
    }
    found
}

/// How far apart to put labelled gridlines so they stay readable at this zoom.
///
/// A 1-2-5 ladder, which is what every ruler uses: the steps are all round
/// numbers in decimal, so a label never reads `0.3333`.
pub fn grid_step(pixels_per_second: f32) -> f32 {
    let mut step = 0.001_f32;
    while step * pixels_per_second < MIN_GRID_GAP {
        let decade = 10.0_f32.powf(step.log10().floor());
        step = match (step / decade).round() as i32 {
            1 => decade * 2.0,
            2 => decade * 5.0,
            _ => decade * 10.0,
        };
        if step > 1e6 {
            break;
        }
    }
    step
}

/// Give the camera the strip and nothing else.
///
/// A `Camera2d` with no viewport owns the **whole window**, and one ordered
/// above the four editor views then renders over all of them — which is a
/// black window, because what it has to draw there is nothing. Cutting it down
/// to the reserved rect is what makes it a strip rather than a curtain, and it
/// is why the root node sits at the viewport's own origin rather than at the
/// window's.
pub fn size_the_strip_camera(
    view: Res<TimelineView>,
    windows: Query<&Window, With<PrimaryWindow>>,
    pools: Res<TimelinePools>,
    mut cameras: Query<&mut Camera, With<TimelineCamera>>,
) {
    let (Some(entity), Ok(window)) = (pools.camera, windows.single()) else {
        return;
    };
    let Ok(mut camera) = cameras.get_mut(entity) else {
        return;
    };

    let scale = window.scale_factor();
    let size = (view.rect.size() * scale).max(Vec2::ONE).as_uvec2();
    let at = (view.rect.min * scale).max(Vec2::ZERO).as_uvec2();

    // A viewport running off the window is a validation error rather than a
    // clamp, and the strip is briefly off-window on the frame the mode opens,
    // before egui has measured anything.
    let window_size = UVec2::new(window.physical_width(), window.physical_height());
    let fits = at.x + size.x <= window_size.x && at.y + size.y <= window_size.y;

    let wanted = fits.then(|| Viewport {
        physical_position: at,
        physical_size: size,
        ..default()
    });
    // Compared by hand because `Viewport` is not `PartialEq`, and writing it
    // every frame would mark the camera changed every frame.
    let same = match (&camera.viewport, &wanted) {
        (None, None) => true,
        (Some(have), Some(want)) => {
            have.physical_position == want.physical_position
                && have.physical_size == want.physical_size
        }
        _ => false,
    };
    if !same {
        camera.viewport = wanted;
    }
}

/// Put every pooled node where the view says it goes.
///
/// The only writer of `Node` on a pooled entity, which is what keeps "where is
/// this drawn" a single question with a single answer.
#[allow(clippy::too_many_arguments)]
pub fn lay_out_the_timeline(
    mut commands: Commands,
    view: Res<TimelineView>,
    cursor: Res<TimelineCursor>,
    clip: Option<Res<ShownClip>>,
    selection: Option<Res<TimelineSelection>>,
    grab: Option<Res<TimelineGrab>>,
    mut pools: ResMut<TimelinePools>,
    mut nodes: Query<&mut Node>,
    mut colours: Query<&mut BackgroundColor>,
    mut texts: Query<&mut Text>,
) {
    let (Some(camera), Some(root)) = (pools.camera, pools.root) else {
        return;
    };

    let clip = clip.as_deref().and_then(|shown| shown.0.as_ref());
    let tracks = clip.map_or(0, |clip| clip.tracks.len());
    let keys: usize = clip.map_or(0, |clip| {
        clip.tracks.iter().map(|track| track.channel.key_count()).sum()
    });
    reserve(&mut commands, camera, root, &mut pools, tracks, keys);

    let rect = view.rect;
    if let Ok(mut node) = nodes.get_mut(root) {
        // At the viewport's own origin, not the window's: the camera has been
        // cut down to the strip, so `0, 0` here *is* the strip's top-left.
        node.left = Val::Px(0.0);
        node.top = Val::Px(0.0);
        node.width = Val::Px(rect.width());
        node.height = Val::Px(rect.height());
        node.display = if rect.width() > 1.0 {
            Display::Flex
        } else {
            Display::None
        };
    }

    // Gridlines. Laid out from the *view*, so zooming moves them and allocates
    // nothing.
    let step = grid_step(view.pixels_per_second);
    let first = (view.start / step).ceil() * step;
    let mut used = 0;
    for slot in 0..pools.grid.len() {
        let time = first + slot as f32 * step;
        let x = view.x_of(time);
        let (line, label) = pools.grid[slot];
        if x > rect.max.x || used >= MAX_GRID {
            for entity in [line, label] {
                if let Ok(mut node) = nodes.get_mut(entity) {
                    node.display = Display::None;
                }
            }
            continue;
        }
        used += 1;

        if let Ok(mut node) = nodes.get_mut(line) {
            node.display = Display::Flex;
            node.position_type = PositionType::Absolute;
            node.left = Val::Px(x - rect.min.x);
            node.top = Val::Px(0.0);
            node.width = Val::Px(1.0);
            node.height = Val::Px(rect.height());
        }
        if let Ok(mut node) = nodes.get_mut(label) {
            node.display = Display::Flex;
            node.position_type = PositionType::Absolute;
            node.left = Val::Px(x - rect.min.x + 3.0);
            node.top = Val::Px(2.0);
        }
        if let Ok(mut text) = texts.get_mut(label) {
            // Written only when it changes: a `Text` write re-lays out the
            // node, and a ruler that rewrote every label every frame would be
            // the most expensive thing on the strip.
            let wanted = format_time(time, step);
            if text.0 != wanted {
                text.0 = wanted;
            }
        }
    }

    // Rows, and the keys on them.
    let mut key_slot = 0;
    for slot in 0..pools.rows.len() {
        let (stripe, label) = pools.rows[slot];
        let Some(track) = clip.and_then(|clip| clip.tracks.get(slot)) else {
            for entity in [stripe, label] {
                if let Ok(mut node) = nodes.get_mut(entity) {
                    node.display = Display::None;
                }
            }
            continue;
        };

        let top = RULER_HEIGHT + slot as f32 * ROW_HEIGHT;
        if let Ok(mut node) = nodes.get_mut(stripe) {
            node.display = if slot % 2 == 0 { Display::Flex } else { Display::None };
            node.position_type = PositionType::Absolute;
            node.left = Val::Px(0.0);
            node.top = Val::Px(top);
            node.width = Val::Px(rect.width());
            node.height = Val::Px(ROW_HEIGHT);
        }
        if let Ok(mut node) = nodes.get_mut(label) {
            node.display = Display::Flex;
            node.position_type = PositionType::Absolute;
            node.left = Val::Px(6.0);
            node.top = Val::Px(top + 3.0);
        }
        if let Ok(mut text) = texts.get_mut(label) {
            let wanted = track_name(track);
            if text.0 != wanted {
                text.0 = wanted;
            }
        }

        for (index, at) in track.channel.key_times().into_iter().enumerate() {
            let Some(key) = pools.keys.get(key_slot).copied() else {
                break;
            };
            key_slot += 1;
            let x = view.x_of(at);

            // Picked keys are lit rather than outlined: an outline on a nine
            // pixel diamond is two pixels of fill and reads as a smudge.
            let picked = selection.as_deref().is_some_and(|picked| {
                picked.0.contains(&KeyAt {
                    track: slot,
                    key: index,
                })
            });
            if let Ok(mut colour) = colours.get_mut(key) {
                let wanted = match picked {
                    true => Color::srgb(1.0, 0.93, 0.70),
                    false => Color::srgb(0.85, 0.72, 0.35),
                };
                if colour.0 != wanted {
                    colour.0 = wanted;
                }
            }

            if let Ok(mut node) = nodes.get_mut(key) {
                let on = x >= view.lanes_left() - KEY_SIZE && x <= rect.max.x;
                node.display = if on { Display::Flex } else { Display::None };
                node.position_type = PositionType::Absolute;
                node.left = Val::Px(x - rect.min.x - KEY_SIZE / 2.0);
                node.top = Val::Px(top + (ROW_HEIGHT - KEY_SIZE) / 2.0);
                node.width = Val::Px(KEY_SIZE);
                node.height = Val::Px(KEY_SIZE);
            }
        }
    }

    // Whatever is left over is switched off rather than removed.
    for slot in key_slot..pools.keys.len() {
        if let Ok(mut node) = nodes.get_mut(pools.keys[slot]) {
            node.display = Display::None;
        }
    }

    // The rubber band, while one is being pulled.
    if let Some(band) = pools.band {
        let pulling = match grab.as_deref().and_then(|grab| grab.0.as_ref()) {
            Some(Grab::Band { from, to }) => Some(Rect::from_corners(*from, *to)),
            _ => None,
        };
        if let Ok(mut node) = nodes.get_mut(band) {
            match pulling {
                Some(box_rect) => {
                    node.display = Display::Flex;
                    node.position_type = PositionType::Absolute;
                    node.left = Val::Px(box_rect.min.x - rect.min.x);
                    node.top = Val::Px(box_rect.min.y - rect.min.y);
                    node.width = Val::Px(box_rect.width().max(1.0));
                    node.height = Val::Px(box_rect.height().max(1.0));
                }
                None => node.display = Display::None,
            }
        }
    }

    // The playhead, and the wider zone that grabs it.
    let x = view.x_of(cursor.time);
    let on = x >= view.lanes_left() && x <= rect.max.x;
    if let (Some(line), Some(grab)) = (pools.playhead, pools.grab) {
        if let Ok(mut node) = nodes.get_mut(line) {
            node.display = if on { Display::Flex } else { Display::None };
            node.position_type = PositionType::Absolute;
            node.left = Val::Px(x - rect.min.x);
            node.top = Val::Px(0.0);
            node.width = Val::Px(PLAYHEAD_WIDTH);
            node.height = Val::Px(rect.height());
        }
        if let Ok(mut node) = nodes.get_mut(grab) {
            node.display = if on { Display::Flex } else { Display::None };
            node.position_type = PositionType::Absolute;
            node.left = Val::Px(x - rect.min.x - GRAB_WIDTH / 2.0);
            node.top = Val::Px(0.0);
            node.width = Val::Px(GRAB_WIDTH);
            node.height = Val::Px(rect.height());
        }
    }
}

/// The clip the strip is showing, cloned out of the document once per change.
///
/// A copy rather than a borrow because the layout pass wants the document
/// immutably while the panels want it mutably, and a clip is a couple of
/// kilobytes. Refreshed on the editor's generation counter — the same one the
/// mesh is rebuilt on, and the honest version of a dirty flag.
#[derive(Resource, Default)]
pub struct ShownClip(pub Option<Clip>);

fn track_name(track: &crate::common::skeleton::clip::Track) -> String {
    use crate::common::skeleton::clip::Channel;
    match &track.channel {
        Channel::Rotation { bone, .. } => bone.clone(),
        Channel::Effector { chain, .. } => format!("{chain:?}"),
        Channel::Weapon { .. } => "weapon".into(),
        Channel::WeaponRotation { .. } => "weapon turn".into(),
    }
}

/// A ruler label, with only as many decimals as the spacing needs.
///
/// Derived from the step rather than fixed, because the same ruler is read at
/// whole seconds and at milliseconds and neither `0` nor `0.250` is right for
/// both.
pub fn format_time(time: f32, step: f32) -> String {
    let decimals = if step >= 1.0 {
        0
    } else if step >= 0.1 {
        1
    } else if step >= 0.01 {
        2
    } else {
        3
    };
    // `-0.0` prints with a sign and reads as a bug on a ruler.
    let time = if time == 0.0 { 0.0 } else { time };
    format!("{time:.decimals$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::common::skeleton::clip::{
        Axis, Channel, Clip, Combine, PointKey, Space, Track, Unit,
    };
    use crate::common::skeleton::Interpolation;

    /// A clip with keys at known times, so a layout can be checked against
    /// arithmetic rather than against a screenshot.
    fn a_clip() -> Clip {
        Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Replace,
                space: Space::Body,
                unit: Unit::Metres,
                enabled: true,
                channel: Channel::Weapon {
                    keys: [0.0_f32, 0.5, 1.0]
                        .into_iter()
                        .map(|at| PointKey {
                            at,
                            to: [Axis::Free; 3],
                            ease: Interpolation::Linear,
                        })
                        .collect(),
                },
            }],
            ..Clip::default()
        }
    }

    /// Stand the strip up and lay it out, for real, in a `World`.
    fn laid_out(view: TimelineView, clip: Option<Clip>) -> (World, TimelinePools) {
        let mut world = World::new();
        world.insert_resource(view);
        world.insert_resource(TimelineCursor { time: 0.25, playing: false });
        world.insert_resource(ShownClip(clip));
        world.init_resource::<TimelinePools>();

        world.run_system_once(spawn_the_strip).unwrap();
        world.run_system_once(lay_out_the_timeline).unwrap();
        // The pools grow through `Commands`, so the second pass is the one
        // that can actually place what the first pass reserved.
        world.run_system_once(lay_out_the_timeline).unwrap();

        let pools = world.remove_resource::<TimelinePools>().unwrap();
        (world, pools)
    }

    /// Keys land where the arithmetic says, in the strip's own coordinates.
    ///
    /// The closest thing to looking at it that a test can do, and it catches
    /// the whole class of off-by-a-column mistakes — the root being placed at
    /// the window's origin rather than the viewport's, a key measured from the
    /// wrong edge, the names column counted twice.
    #[test]
    fn a_key_is_drawn_where_its_time_says() {
        let view = view();
        let (world, pools) = laid_out(view, Some(a_clip()));

        let shown: Vec<f32> = pools
            .keys
            .iter()
            .filter_map(|key| world.get::<Node>(*key))
            .filter(|node| node.display != Display::None)
            .filter_map(|node| match node.left {
                Val::Px(left) => Some(left),
                _ => None,
            })
            .collect();

        assert_eq!(shown.len(), 3, "expected three keys on the strip: {shown:?}");
        for (at, left) in [0.0_f32, 0.5, 1.0].into_iter().zip(&shown) {
            // Relative to the strip, because the camera's viewport starts
            // there — and drawn from its own left edge, so the mark is centred
            // on the time rather than starting at it.
            let wanted = view.x_of(at) - view.rect.min.x - KEY_SIZE / 2.0;
            assert!(
                (left - wanted).abs() < 1e-3,
                "a key at {at}s was drawn at {left}, not {wanted}",
            );
        }
    }

    /// A clip with nothing in it draws nothing, rather than one stale row left
    /// over from the last one — which is what a pool that is reused rather
    /// than rebuilt has to get right, and the failure newseum's despawn-based
    /// version cannot have.
    #[test]
    fn switching_to_an_empty_clip_leaves_nothing_behind() {
        let view = view();
        let mut world = World::new();
        world.insert_resource(view);
        world.insert_resource(TimelineCursor::default());
        world.insert_resource(ShownClip(Some(a_clip())));
        world.init_resource::<TimelinePools>();

        world.run_system_once(spawn_the_strip).unwrap();
        world.run_system_once(lay_out_the_timeline).unwrap();
        world.run_system_once(lay_out_the_timeline).unwrap();

        world.resource_mut::<ShownClip>().0 = None;
        world.run_system_once(lay_out_the_timeline).unwrap();

        let pools = world.remove_resource::<TimelinePools>().unwrap();
        let drawn = pools
            .keys
            .iter()
            .chain(pools.rows.iter().map(|(stripe, _)| stripe))
            .filter_map(|entity| world.get::<Node>(*entity))
            .filter(|node| node.display != Display::None)
            .count();
        assert_eq!(drawn, 0, "{drawn} nodes were left over from the old clip");
    }

    /// The pool grows and never shrinks, and nothing is despawned while the
    /// strip is up. This is the whole design — the version that despawns
    /// cannot be zoomed without allocating — so it is worth a test that would
    /// fail if somebody "tidied" it into a rebuild.
    #[test]
    fn the_pool_grows_and_never_despawns() {
        let view = view();
        let mut world = World::new();
        world.insert_resource(view);
        world.insert_resource(TimelineCursor::default());
        world.insert_resource(ShownClip(Some(a_clip())));
        world.init_resource::<TimelinePools>();

        world.run_system_once(spawn_the_strip).unwrap();
        world.run_system_once(lay_out_the_timeline).unwrap();
        world.run_system_once(lay_out_the_timeline).unwrap();

        let before: Vec<Entity> = world.resource::<TimelinePools>().keys.clone();
        assert!(!before.is_empty(), "no keys were pooled at all");

        // Zoom hard, twice, and change what is shown.
        world.resource_mut::<TimelineView>().zoom_about(500.0, 6.0);
        world.run_system_once(lay_out_the_timeline).unwrap();
        world.resource_mut::<ShownClip>().0 = None;
        world.run_system_once(lay_out_the_timeline).unwrap();

        let after = world.resource::<TimelinePools>().keys.clone();
        assert_eq!(before, after, "the key pool was rebuilt rather than reused");
        for key in &after {
            assert!(world.get_entity(*key).is_ok(), "a pooled node was despawned");
        }
    }

    fn view() -> TimelineView {
        TimelineView {
            start: 0.0,
            pixels_per_second: 200.0,
            rect: Rect::from_corners(Vec2::new(0.0, 400.0), Vec2::new(1000.0, 580.0)),
        }
    }

    /// Zooming about the pointer has to keep the time under it *under it*.
    /// Anything else is a strip that slides away from what you were looking
    /// at, which makes zooming in on a key impossible.
    #[test]
    fn zooming_keeps_the_time_under_the_pointer() {
        for x in [200.0_f32, 640.0, 999.0] {
            for notches in [1.0_f32, -1.0, 3.0, -4.5] {
                let mut view = view();
                let before = view.time_at(x);
                view.zoom_about(x, notches);
                let after = view.time_at(x);
                assert!(
                    (before - after).abs() < 1e-4,
                    "zooming {notches} at {x} moved {before} to {after}",
                );
            }
        }
    }

    /// Zoom is clamped at both ends. Unclamped, a few notches reach a scale
    /// where a whole clip is one pixel wide or a single frame fills the
    /// window, and neither can be zoomed back out of by eye.
    #[test]
    fn zoom_stops_at_both_ends() {
        let mut view = view();
        for _ in 0..200 {
            view.zoom_about(500.0, 1.0);
        }
        assert!(view.pixels_per_second <= 8000.0);
        for _ in 0..400 {
            view.zoom_about(500.0, -1.0);
        }
        assert!(view.pixels_per_second >= 8.0);
    }

    /// `x_of` and `time_at` are each other's inverse. They are used in
    /// opposite directions a few lines apart — placing a key and picking one
    /// — and a mismatch is a key that does not sit where you dropped it.
    #[test]
    fn a_time_and_its_pixel_are_the_same_fact() {
        let view = view();
        for time in [-1.0_f32, 0.0, 0.37, 4.5] {
            assert!((view.time_at(view.x_of(time)) - time).abs() < 1e-4);
        }
    }

    /// The names column is not part of the lanes: a drag starting on a track
    /// name is not a scrub to somewhere before the clip begins.
    #[test]
    fn the_names_column_is_not_the_lanes() {
        let view = view();
        assert!(!view.over_lanes(Vec2::new(view.rect.min.x + 4.0, 500.0)));
        assert!(view.over_lanes(Vec2::new(view.lanes_left() + 4.0, 500.0)));
        assert!(!view.over_lanes(Vec2::new(view.lanes_left() + 4.0, 100.0)));
    }

    /// Framing puts the whole clip on the strip with a little air, whatever
    /// its length — which is what the button and the first sight of a clip
    /// both want.
    #[test]
    fn framing_fits_the_whole_clip() {
        for length in [0.05_f32, 0.45, 12.0] {
            let mut view = view();
            view.frame(length);
            assert!(view.time_at(view.lanes_left()) <= 0.0, "the start is off-screen");
            assert!(
                view.time_at(view.rect.max.x) >= length,
                "the end is off-screen at length {length}",
            );
        }
    }

    /// Gridline spacing walks a 1-2-5 ladder so a label is always a round
    /// number, and is never so tight that the labels overlap.
    #[test]
    fn the_ruler_stays_readable_at_every_zoom() {
        let mut scale = 8.0_f32;
        while scale <= 8000.0 {
            let step = grid_step(scale);
            assert!(
                step * scale >= MIN_GRID_GAP,
                "at {scale} px/s the gridlines are {} px apart",
                step * scale,
            );
            let decade = 10.0_f32.powf(step.log10().floor());
            let mantissa = (step / decade).round() as i32;
            assert!(
                [1, 2, 5].contains(&mantissa),
                "step {step} is not a round number at {scale} px/s",
            );
            scale *= 1.3;
        }
    }

    /// A ruler that printed `-0.0`, or `0.250` where it means a quarter of a
    /// second at whole-second spacing, reads as broken.
    #[test]
    fn a_ruler_label_says_what_the_spacing_can_show() {
        assert_eq!(format_time(0.0, 1.0), "0");
        assert_eq!(format_time(-0.0, 1.0), "0");
        assert_eq!(format_time(2.0, 1.0), "2");
        assert_eq!(format_time(0.25, 0.05), "0.25");
        assert_eq!(format_time(0.125, 0.005), "0.125");
    }
}
