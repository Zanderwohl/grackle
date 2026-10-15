//! The timeline strip, in Bevy UI.
//!
//! [`TimelineView::start`] and [`TimelineView::pixels_per_second`] place every mark. Zoom and pan
//! change only those; the layout pass moves existing nodes. `x_of`, `time_at` and `zoom_about` are
//! pure and tested without an App.
//!
//! Three pools (gridlines, rows, keys) grow to fit and never shrink; surplus nodes get
//! `Display::None`. A ten-second clip with eight tracks and sixty keys is about 120 entities,
//! allocated once.
//!
//! In an authoring mode there is no `IsDefaultUiCamera` (it is on the play camera), and a UI node
//! with no [`UiTargetCamera`] renders nowhere without an error. So every node targets the multicam's
//! egui context camera explicitly.

#[cfg(test)]
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use crate::common::skeleton::clip::Clip;

pub const RULER_HEIGHT: f32 = 20.0;
pub const ROW_HEIGHT: f32 = 22.0;
pub const NAMES_WIDTH: f32 = 150.0;

/// Drawn and picked this wide. Round, since every other mark on the strip is axis-aligned.
const KEY_SIZE: f32 = 11.0;

const HEAD_SIZE: f32 = 9.0;

/// Unlabelled divisions between labelled gridlines; the eye judges distance by counting them.
const MINOR_PER_MAJOR: usize = 5;

/// The grab zone is invisible, centred on the line, and wider so it can be hit.
const PLAYHEAD_WIDTH: f32 = 1.0;
const GRAB_WIDTH: f32 = 13.0;

/// Bounded by how many fit at a readable spacing, not by clip length, so zooming out does not
/// allocate.
const MAX_GRID: usize = 200;

/// In logical pixels.
const MIN_GRID_GAP: f32 = 55.0;

mod paint {
    use bevy::prelude::Color;

    pub const STRIP: Color = Color::srgb(0.075, 0.075, 0.085);
    pub const NAMES: Color = Color::srgb(0.105, 0.105, 0.118);
    pub const RULER: Color = Color::srgb(0.125, 0.125, 0.140);
    /// The band the clip occupies; darker outside it, so the clip's end is visible.
    pub const EXTENT: Color = Color::srgb(0.098, 0.098, 0.110);
    pub const ROW_ODD: Color = Color::srgb(0.118, 0.118, 0.132);
    pub const EDGE: Color = Color::srgb(0.215, 0.215, 0.235);
    pub const MINOR: Color = Color::srgb(0.140, 0.140, 0.155);
    pub const MAJOR: Color = Color::srgb(0.190, 0.190, 0.210);
    pub const LABEL: Color = Color::srgb(0.560, 0.560, 0.600);
    /// A switched-off track's keys: dimmed rather than hidden, since they can still be edited.
    pub const KEY_OFF: Color = Color::srgb(0.420, 0.380, 0.280);
    /// The picked track's header: the one place the strip says which track the Inspector describes.
    pub const ROW_PICKED: Color = Color::srgb(0.200, 0.255, 0.350);
    /// Its lane, quieter, so keys still read against it.
    pub const ROW_PICKED_LANE: Color = Color::srgb(0.135, 0.150, 0.185);
    pub const KEY: Color = Color::srgb(0.855, 0.720, 0.350);
    pub const KEY_EDGE: Color = Color::srgb(0.420, 0.340, 0.140);
    pub const KEY_PICKED: Color = Color::srgb(1.0, 0.945, 0.760);
    pub const KEY_PICKED_EDGE: Color = Color::srgb(1.0, 1.0, 1.0);
    pub const PLAYHEAD: Color = Color::srgb(0.950, 0.300, 0.250);
    pub const BAND: Color = Color::srgba(0.55, 0.70, 0.95, 0.18);
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct TimelineView {
    pub start: f32,
    pub pixels_per_second: f32,
    /// Logical pixels, from the rect egui reserved.
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
    pub fn lanes_left(&self) -> f32 {
        self.rect.min.x + NAMES_WIDTH
    }

    pub fn x_of(&self, time: f32) -> f32 {
        self.lanes_left() + (time - self.start) * self.pixels_per_second
    }

    pub fn time_at(&self, x: f32) -> f32 {
        self.start + (x - self.lanes_left()) / self.pixels_per_second
    }

    /// Zoom about a screen x, so the time under the pointer stays there. Multiplicative, since a clip is
    /// viewed at both whole seconds and single frames.
    pub fn zoom_about(&mut self, x: f32, notches: f32) {
        let before = self.time_at(x);
        self.pixels_per_second =
            (self.pixels_per_second * 1.2f32.powf(notches)).clamp(8.0, 8000.0);
        self.start += before - self.time_at(x);
    }

    pub fn frame(&mut self, length: f32) {
        let lanes = (self.rect.width() - NAMES_WIDTH).max(1.0);
        let length = length.max(0.01);
        self.pixels_per_second = (lanes * 0.92 / length).clamp(8.0, 8000.0);
        self.start = -(lanes * 0.04) / self.pixels_per_second;
    }

    /// The rect is zero-sized until egui lays the panel out, and the arithmetic is nonsense before then.
    pub fn has_room(&self) -> bool {
        self.rect.width() > NAMES_WIDTH && self.rect.height() > RULER_HEIGHT
    }

    pub fn over_lanes(&self, at: Vec2) -> bool {
        self.rect.contains(at) && at.x >= self.lanes_left()
    }
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct TimelineCursor {
    pub time: f32,
    pub playing: bool,
}

/// Addressed by position, not identity: a drag reorders a track, so [`Channel::nudge`] reports new
/// indices and the selection is remapped. The alternative was a per-key id on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyAt {
    pub track: usize,
    pub key: usize,
}

#[derive(Resource, Debug, Default)]
pub struct TimelineSelection(pub std::collections::BTreeSet<KeyAt>);

#[derive(Debug, Clone)]
pub struct DraggedTrack {
    pub track: usize,
    /// The channel as the drag began. Every frame recomputes from this; accumulating drifts on every
    /// clamp and re-sort.
    pub original: crate::common::skeleton::clip::Channel,
    /// Indices into `original`.
    pub held: Vec<usize>,
}

/// Anchored to where it began, and applied as a delta. Mapping to `time_at(cursor)` would jump the
/// playhead on press, since the grab zone is wider than the line, and a jumping scrub cannot nudge.
#[derive(Debug, Clone)]
pub enum Grab {
    Scrub { from_x: f32, from_time: f32 },
    Pan { from_x: f32, from_start: f32 },
    /// `last_by` is the offset last applied. The drag is absolute, so the same
    /// offset is the same keys, and a pointer held still writes nothing.
    Keys { from_x: f32, tracks: Vec<DraggedTrack>, last_by: Option<f32> },
    /// Both corners, so the layout need not know where the pointer is.
    Band { from: Vec2, to: Vec2 },
}

#[derive(Resource, Debug, Default)]
pub struct TimelineGrab(pub Option<Grab>);

#[derive(Component)]
pub struct TimelineCamera;

#[derive(Component)]
struct TimelineRoot;

#[derive(Component)]
struct PlayheadGrab;

#[derive(Component)]
struct GridLine;

#[derive(Component)]
struct GridLabel;

#[derive(Component)]
struct RowStripe;


#[derive(Component)]
struct KeyMark;

#[derive(Component)]
struct BandBox;

/// Held by index so the layout pass can address them without a query sort.
#[derive(Resource, Default)]
pub struct TimelinePools {
    camera: Option<Entity>,
    root: Option<Entity>,
    extent: Option<Entity>,
    ruler: Option<Entity>,
    ruler_edge: Option<Entity>,
    names: Option<Entity>,
    names_edge: Option<Entity>,
    picked_row: Option<Entity>,
    playhead: Option<Entity>,
    head: Option<Entity>,
    grab: Option<Entity>,
    band: Option<Entity>,
    /// Per division: lane line, ruler tick, label. The last two show only on labelled divisions.
    grid: Vec<(Entity, Entity, Entity)>,
    rows: Vec<Entity>,
    keys: Vec<Entity>,
}

fn off() -> Node {
    Node {
        display: Display::None,
        ..default()
    }
}

pub fn spawn_the_strip(
    mut commands: Commands,
    mut pools: ResMut<TimelinePools>,
    egui_camera: Query<Entity, With<bevy_egui::PrimaryEguiContext>>,
) {
    // The multicam's egui context camera rather than a second `Camera2d`, which rendered nothing with
    // no diagnostic. Bevy's UI pass runs before egui's node, so an egui popup still draws on top.
    let Ok(camera) = egui_camera.single() else {
        warn!("no egui camera to hang the timeline on; the strip will not be drawn");
        return;
    };

    // Bevy UI has no painter, so every band, hairline and tick is a node.
    let bare = |commands: &mut Commands, colour: Color, z: i32| {
        commands
            .spawn((
                UiTargetCamera(camera),
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                BackgroundColor(colour),
                GlobalZIndex(z),
            ))
            .id()
    };

    let root = commands
        .spawn((
            TimelineRoot,
            UiTargetCamera(camera),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            BackgroundColor(paint::STRIP),
        ))
        .id();

    let extent = bare(&mut commands, paint::EXTENT, 0);
    let ruler = bare(&mut commands, paint::RULER, 1);
    let ruler_edge = bare(&mut commands, paint::EDGE, 2);

    // Above the lanes, so a key scrolled left goes behind the names.
    let names = bare(&mut commands, paint::NAMES, 4);
    let names_edge = bare(&mut commands, paint::EDGE, 5);
    // Over the names ground, under the row labels, which share its z and spawn after it.
    let picked_row = bare(&mut commands, paint::ROW_PICKED, 5);

    let playhead = bare(&mut commands, paint::PLAYHEAD, 3);
    // A head, so it is not mistaken for a gridline.
    let head = commands
        .spawn((
            UiTargetCamera(camera),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(HEAD_SIZE),
                height: Val::Px(HEAD_SIZE),
                border_radius: BorderRadius {
                    top_left: Val::Px(1.0),
                    top_right: Val::Px(1.0),
                    bottom_left: Val::Px(HEAD_SIZE / 2.0),
                    bottom_right: Val::Px(HEAD_SIZE / 2.0),
                },
                ..default()
            },
            BackgroundColor(paint::PLAYHEAD),
            GlobalZIndex(3),
        ))
        .id();

    let grab = commands
        .spawn((
            PlayheadGrab,
            UiTargetCamera(camera),
            ChildOf(root),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(GRAB_WIDTH),
                ..default()
            },
            GlobalZIndex(6),
        ))
        .id();

    let band = commands
        .spawn((
            BandBox,
            UiTargetCamera(camera),
            ChildOf(root),
            off(),
            BackgroundColor(paint::BAND),
            GlobalZIndex(2),
        ))
        .id();

    for piece in [extent, ruler, ruler_edge, names, names_edge, picked_row, playhead, head] {
        commands.entity(piece).insert(ChildOf(root));
    }

    pools.extent = Some(extent);
    pools.ruler = Some(ruler);
    pools.ruler_edge = Some(ruler_edge);
    pools.names = Some(names);
    pools.names_edge = Some(names_edge);
    pools.picked_row = Some(picked_row);
    pools.playhead = Some(playhead);
    pools.head = Some(head);
    pools.grab = Some(grab);
    pools.band = Some(band);
    pools.root = Some(root);
    pools.camera = Some(camera);
}

pub fn take_the_strip_down(mut commands: Commands, mut pools: ResMut<TimelinePools>) {
    let mut everything: Vec<Entity> = Vec::new();
    everything.extend(pools.grid.drain(..).flat_map(|(line, tick, label)| [line, tick, label]));
    everything.extend(pools.rows.drain(..));
    everything.extend(pools.keys.drain(..));
    everything.extend(pools.band.take());
    everything.extend(pools.extent.take());
    everything.extend(pools.ruler.take());
    everything.extend(pools.ruler_edge.take());
    everything.extend(pools.names.take());
    everything.extend(pools.names_edge.take());
    everything.extend(pools.picked_row.take());
    everything.extend(pools.head.take());
    everything.extend(pools.root.take());
    everything.extend(pools.playhead.take());
    everything.extend(pools.grab.take());
    // Not `pools.camera`'s entity: it belongs to the multicam.
    pools.camera = None;

    for entity in everything {
        // The pools are addressed by index, and a stale entity is worse than a double despawn, so drain
        // rather than relying on the hierarchy.
        commands.entity(entity).try_despawn();
    }
}

/// Grows and never shrinks; a leftover node costs a pointer and saves the next allocation.
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
                ChildOf(root),
                off(),
                BackgroundColor(paint::MINOR),
                GlobalZIndex(1),
            ))
            .id();
        // Shown only where there is a label. Lane lines stop at the ruler so the two do not merge into one
        // bar.
        let tick = commands
            .spawn((
                GridLine,
                UiTargetCamera(camera),
                ChildOf(root),
                off(),
                BackgroundColor(paint::MAJOR),
                GlobalZIndex(2),
            ))
            .id();
        let label = commands
            .spawn((
                GridLabel,
                UiTargetCamera(camera),
                ChildOf(root),
                off(),
                Text::new(String::new()),
                TextFont {
                    font_size: bevy::text::FontSize::Px(10.0),
                    ..default()
                },
                TextColor(paint::LABEL),
                GlobalZIndex(2),
            ))
            .id();
        pools.grid.push((line, tick, label));
    }

    // A stripe only: the header is egui, drawn by the panels, since Bevy UI has no controls.
    while pools.rows.len() < rows {
        let stripe = commands
            .spawn((
                RowStripe,
                UiTargetCamera(camera),
                ChildOf(root),
                off(),
                BackgroundColor(paint::ROW_ODD),
            ))
            .id();
        pools.rows.push(stripe);
    }

    // In chunks, so adding keys one at a time does not spawn per keystroke.
    while pools.keys.len() < keys {
        for _ in 0..64 {
            let key = commands
                .spawn((
                    KeyMark,
                    UiTargetCamera(camera),
                    ChildOf(root),
                    off(),
                    BackgroundColor(paint::KEY),
                    BorderColor::all(paint::KEY_EDGE),
                    GlobalZIndex(2),
                ))
                .id();
            pools.keys.push(key);
        }
    }
}

pub fn grab_radius() -> f32 {
    GRAB_WIDTH / 2.0
}

/// Measured down from the top of the strip.
pub fn row_top(slot: usize) -> f32 {
    RULER_HEIGHT + slot as f32 * ROW_HEIGHT
}

pub fn row_at(view: &TimelineView, y: f32) -> Option<usize> {
    let local = y - view.rect.min.y - RULER_HEIGHT;
    (local >= 0.0).then(|| (local / ROW_HEIGHT) as usize)
}

/// The ruler scrubs and the lanes select; a click on an empty lane starts a band.
pub fn over_ruler(view: &TimelineView, at: Vec2) -> bool {
    view.rect.contains(at) && at.y < view.rect.min.y + RULER_HEIGHT && at.x >= view.lanes_left()
}

/// Geometric, from the same `x_of` the layout uses, and testable without a renderer.
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

/// A 1-2-5 ladder, so a label is always a round number.
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

/// Write a pooled node through `write`, marking it changed only if it differs:
/// a changed `Node` is a UI re-layout, and every pooled node is placed every
/// frame. These carry no grid tracks, so the copy does not allocate.
fn place(nodes: &mut Query<&mut Node>, entity: Entity, write: impl FnOnce(&mut Node)) {
    let Ok(mut node) = nodes.get_mut(entity) else {
        return;
    };
    let mut wanted = node.clone();
    write(&mut wanted);
    node.set_if_neq(wanted);
}

/// The only writer of `Node` on a pooled entity.
#[allow(clippy::too_many_arguments)]
pub fn lay_out_the_timeline(
    mut commands: Commands,
    view: Res<TimelineView>,
    cursor: Res<TimelineCursor>,
    clip: Option<Res<ShownClip>>,
    selection: Option<Res<TimelineSelection>>,
    grab: Option<Res<TimelineGrab>>,
    anim: Option<Res<crate::anim::AnimEditor>>,
    mut pools: ResMut<TimelinePools>,
    mut nodes: Query<&mut Node>,
    mut colours: Query<&mut BackgroundColor>,
    mut borders: Query<&mut BorderColor>,
    mut texts: Query<&mut Text>,
    mut label_text: Local<String>,
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
    place(&mut nodes, root, |node| {
        node.left = Val::Px(rect.min.x);
        node.top = Val::Px(rect.min.y);
        node.width = Val::Px(rect.width());
        node.height = Val::Px(rect.height());
        node.display = Display::Flex;
    });

    // Before egui measures the strip the rect is zero-sized, and the lanes-to-width clamp becomes
    // `clamp(150, 0)`, which panics.
    if !view.has_room() {
        return;
    }

    let length = clip.map_or(0.0, |clip| clip.length);
    let lanes = view.lanes_left() - rect.min.x;
    let inside = |x: f32| (x - rect.min.x).clamp(lanes, rect.width());

    if let Some(extent) = pools.extent {
        place(&mut nodes, extent, |node| {
            let from = inside(view.x_of(0.0));
            let to = inside(view.x_of(length));
            node.display = if to > from { Display::Flex } else { Display::None };
            node.left = Val::Px(from);
            node.top = Val::Px(RULER_HEIGHT);
            node.width = Val::Px((to - from).max(0.0));
            node.height = Val::Px((rect.height() - RULER_HEIGHT).max(0.0));
        });
    }

    for (piece, top, height) in [
        (pools.ruler, 0.0, RULER_HEIGHT),
        (pools.ruler_edge, RULER_HEIGHT - 1.0, 1.0),
    ] {
        let Some(piece) = piece else { continue };
        place(&mut nodes, piece, |node| {
            node.display = Display::Flex;
            node.left = Val::Px(0.0);
            node.top = Val::Px(top);
            node.width = Val::Px(rect.width());
            node.height = Val::Px(height);
        });
    }

    // Laid out from the view, so zooming allocates nothing. Drawn at minor divisions, labelled every
    // fifth.
    let step = grid_step(view.pixels_per_second);
    let minor = step / MINOR_PER_MAJOR as f32;
    // Counted from zero rather than the left edge, so labels land on round numbers however far it is
    // panned.
    let first = (view.start / minor).ceil() as i64;
    for slot in 0..pools.grid.len() {
        let division = first + slot as i64;
        let time = division as f32 * minor;
        let x = view.x_of(time);
        let (line, tick, label) = pools.grid[slot];
        let major = division.rem_euclid(MINOR_PER_MAJOR as i64) == 0;

        if x > rect.max.x || x < view.lanes_left() {
            for entity in [line, tick, label] {
                place(&mut nodes, entity, |node| {
                    node.display = Display::None;
                });
            }
            continue;
        }

        place(&mut nodes, line, |node| {
            node.display = Display::Flex;
            node.left = Val::Px(x - rect.min.x);
            node.top = Val::Px(RULER_HEIGHT);
            node.width = Val::Px(1.0);
            node.height = Val::Px((rect.height() - RULER_HEIGHT).max(0.0));
        });
        if let Ok(mut colour) = colours.get_mut(line) {
            let wanted = if major { paint::MAJOR } else { paint::MINOR };
            if colour.0 != wanted {
                colour.0 = wanted;
            }
        }

        place(&mut nodes, tick, |node| {
            node.display = if major { Display::Flex } else { Display::None };
            node.left = Val::Px(x - rect.min.x);
            node.top = Val::Px(RULER_HEIGHT - 6.0);
            node.width = Val::Px(1.0);
            node.height = Val::Px(5.0);
        });
        place(&mut nodes, label, |node| {
            node.display = if major { Display::Flex } else { Display::None };
            node.left = Val::Px(x - rect.min.x + 3.0);
            node.top = Val::Px(2.0);
        });
        if major {
            if let Ok(mut text) = texts.get_mut(label) {
                // Written only when it changes: a `Text` write re-lays out the
                // node. Formatted into a kept buffer, so no allocation either.
                write_time(&mut label_text, time, step);
                if text.0 != *label_text {
                    text.0.clone_from(&label_text);
                }
            }
        }
    }

    let picked_track = anim
        .as_deref()
        .and_then(|anim| anim.track)
        .filter(|track| *track < tracks);
    if let Some(header) = pools.picked_row {
        place(&mut nodes, header, |node| {
            match picked_track {
                Some(track) => {
                    node.display = Display::Flex;
                    node.left = Val::Px(0.0);
                    node.top = Val::Px(row_top(track));
                    node.width = Val::Px((lanes - 1.0).max(0.0));
                    node.height = Val::Px(ROW_HEIGHT);
                }
                None => node.display = Display::None,
            }
        });
    }
    let mut key_slot = 0;
    for slot in 0..pools.rows.len() {
        let stripe = pools.rows[slot];
        let Some(track) = clip.and_then(|clip| clip.tracks.get(slot)) else {
            place(&mut nodes, stripe, |node| {
                node.display = Display::None;
            });
            continue;
        };

        let top = row_top(slot);
        let picked = picked_track == Some(slot);
        if let Ok(mut colour) = colours.get_mut(stripe) {
            let wanted = match picked {
                true => paint::ROW_PICKED_LANE,
                false => paint::ROW_ODD,
            };
            if colour.0 != wanted {
                colour.0 = wanted;
            }
        }
        place(&mut nodes, stripe, |node| {
            // Every other row, and the picked one.
            node.display = if slot % 2 == 1 || picked { Display::Flex } else { Display::None };
            node.position_type = PositionType::Absolute;
            node.left = Val::Px(0.0);
            node.top = Val::Px(top);
            node.width = Val::Px(rect.width());
            node.height = Val::Px(ROW_HEIGHT);
        });
        for (index, at) in track.channel.keys().iter().map(|key| key.at).enumerate() {
            let Some(key) = pools.keys.get(key_slot).copied() else {
                break;
            };
            key_slot += 1;
            let x = view.x_of(at);

            // Lit rather than outlined: an outline on a nine-pixel mark reads as a smudge.
            let picked = selection.as_deref().is_some_and(|picked| {
                picked.0.contains(&KeyAt {
                    track: slot,
                    key: index,
                })
            });
            if let Ok(mut colour) = colours.get_mut(key) {
                let wanted = match (picked, track.enabled) {
                    (true, _) => paint::KEY_PICKED,
                    (false, true) => paint::KEY,
                    (false, false) => paint::KEY_OFF,
                };
                if colour.0 != wanted {
                    colour.0 = wanted;
                }
            }
            if let Ok(mut border) = borders.get_mut(key) {
                let wanted = match picked {
                    true => paint::KEY_PICKED_EDGE,
                    false => paint::KEY_EDGE,
                };
                if border.left != wanted {
                    *border = BorderColor::all(wanted);
                }
            }

            place(&mut nodes, key, |node| {
                // Hidden before the names column, which also keeps it out of the pointer's way.
                let on = x >= view.lanes_left() && x <= rect.max.x;
                node.display = if on { Display::Flex } else { Display::None };
                node.position_type = PositionType::Absolute;
                node.left = Val::Px(x - rect.min.x - KEY_SIZE / 2.0);
                node.top = Val::Px(top + (ROW_HEIGHT - KEY_SIZE) / 2.0);
                node.width = Val::Px(KEY_SIZE);
                node.height = Val::Px(KEY_SIZE);
                node.border = UiRect::all(Val::Px(1.0));
                node.border_radius = BorderRadius::all(Val::Percent(50.0));
            });
        }
    }

    for slot in key_slot..pools.keys.len() {
        place(&mut nodes, pools.keys[slot], |node| {
            node.display = Display::None;
        });
    }

    if let Some(band) = pools.band {
        let pulling = match grab.as_deref().and_then(|grab| grab.0.as_ref()) {
            Some(Grab::Band { from, to }) => Some(Rect::from_corners(*from, *to)),
            _ => None,
        };
        place(&mut nodes, band, |node| {
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
        });
    }

    // Last of the grounds, so anything scrolled left goes behind it.
    for (piece, left, width) in [
        (pools.names, 0.0, lanes),
        (pools.names_edge, lanes - 1.0, 1.0),
    ] {
        let Some(piece) = piece else { continue };
        place(&mut nodes, piece, |node| {
            node.display = Display::Flex;
            node.left = Val::Px(left);
            node.top = Val::Px(0.0);
            node.width = Val::Px(width.max(0.0));
            node.height = Val::Px(rect.height());
        });
    }

    let x = view.x_of(cursor.time);
    let on = x >= view.lanes_left() && x <= rect.max.x;
    if let (Some(line), Some(head), Some(grab)) = (pools.playhead, pools.head, pools.grab) {
        place(&mut nodes, line, |node| {
            node.display = if on { Display::Flex } else { Display::None };
            node.position_type = PositionType::Absolute;
            node.left = Val::Px(x - rect.min.x);
            node.top = Val::Px(0.0);
            node.width = Val::Px(PLAYHEAD_WIDTH);
            node.height = Val::Px(rect.height());
        });
        place(&mut nodes, head, |node| {
            node.display = if on { Display::Flex } else { Display::None };
            node.position_type = PositionType::Absolute;
            node.left = Val::Px(x - rect.min.x - HEAD_SIZE / 2.0);
            node.top = Val::Px(0.0);
        });
        place(&mut nodes, grab, |node| {
            node.display = if on { Display::Flex } else { Display::None };
            node.position_type = PositionType::Absolute;
            node.left = Val::Px(x - rect.min.x - GRAB_WIDTH / 2.0);
            node.top = Val::Px(0.0);
            node.width = Val::Px(GRAB_WIDTH);
            node.height = Val::Px(rect.height());
        });
    }
}

/// A copy rather than a borrow: the layout wants the document immutably while the panels want it
/// mutably. Refreshed on the editor's generation counter.
#[derive(Resource, Default)]
pub struct ShownClip(pub Option<Clip>);

/// Also the Inspector's name for it. A bone is named by its rig name, an identifier rather than
/// display text; everything else goes through the lang layer.
pub fn track_name(track: &crate::common::skeleton::clip::Track) -> String {
    use crate::common::skeleton::clip::Channel;
    match &track.channel {
        Channel::Rotation { bone, .. } => bone.clone(),
        Channel::Effector { chain, .. } => crate::get!(chain.name_key()),
        other => crate::get!(other.kind_key()),
    }
}

/// Decimals derived from the step, since the ruler is read at seconds and milliseconds.
pub fn format_time(time: f32, step: f32) -> String {
    let mut out = String::new();
    write_time(&mut out, time, step);
    out
}

/// [`format_time`] into a buffer that already exists, replacing what was in it.
fn write_time(out: &mut String, time: f32, step: f32) {
    use std::fmt::Write;
    let decimals = if step >= 1.0 {
        0
    } else if step >= 0.1 {
        1
    } else if step >= 0.01 {
        2
    } else {
        3
    };
    // `-0.0` prints with a sign.
    let time = if time == 0.0 { 0.0 } else { time };
    out.clear();
    // Writing to a `String` cannot fail.
    let _ = write!(out, "{time:.decimals$}");
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::common::skeleton::clip::{
        Axis, Channel, Clip, Combine, Key, Space, Track, Unit,
    };
    use crate::common::skeleton::Interpolation;

    fn a_clip() -> Clip {
        Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Replace,
                space: Space::Body,
                unit: Unit::Metres,
                enabled: true,
                timing: Default::default(),
                channel: Channel::Weapon {
                    keys: [0.0_f32, 0.5, 1.0]
                        .into_iter()
                        .map(|at| Key {
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

    /// `spawn_the_strip` builds nothing without a UI camera; here it only has to exist.
    fn with_a_ui_camera() -> World {
        let mut world = World::new();
        world.spawn(bevy_egui::PrimaryEguiContext);
        world
    }

    fn laid_out(view: TimelineView, clip: Option<Clip>) -> (World, TimelinePools) {
        let mut world = with_a_ui_camera();
        world.insert_resource(view);
        world.insert_resource(TimelineCursor { time: 0.25, playing: false });
        world.insert_resource(ShownClip(clip));
        world.init_resource::<TimelinePools>();

        world.run_system_once(spawn_the_strip).unwrap();
        world.run_system_once(lay_out_the_timeline).unwrap();
        // The pools grow through `Commands`, so the second pass places what the first reserved.
        world.run_system_once(lay_out_the_timeline).unwrap();

        let pools = world.remove_resource::<TimelinePools>().unwrap();
        (world, pools)
    }

    /// On the first frame egui has not measured the panel, so a zero-sized rect is the ordinary case and
    /// must not panic.
    #[test]
    fn a_strip_with_no_room_lays_nothing_out() {
        for rect in [
            Rect::from_corners(Vec2::ZERO, Vec2::ZERO),
            Rect::from_corners(Vec2::ZERO, Vec2::new(NAMES_WIDTH, 400.0)),
            Rect::from_corners(Vec2::ZERO, Vec2::new(900.0, RULER_HEIGHT)),
        ] {
            let view = TimelineView { rect, ..TimelineView::default() };
            let (_world, pools) = laid_out(view, Some(a_clip()));
            assert!(pools.root.is_some(), "the strip was not built at all");
        }
    }

    /// Without a UI camera nothing is built, and the warning explains the blank strip.
    #[test]
    fn without_a_camera_the_strip_is_not_built() {
        let mut world = World::new();
        world.insert_resource(view());
        world.insert_resource(TimelineCursor::default());
        world.insert_resource(ShownClip(None));
        world.init_resource::<TimelinePools>();

        world.run_system_once(spawn_the_strip).unwrap();

        let pools = world.resource::<TimelinePools>();
        assert!(pools.root.is_none(), "a root was built with nothing to draw it");
        assert!(pools.keys.is_empty());
    }

    /// Catches off-by-a-column mistakes: root placed at the window origin, a key measured from the wrong
    /// edge, the names column counted twice.
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
            // Relative to the strip, and centred on the time.
            let wanted = view.x_of(at) - view.rect.min.x - KEY_SIZE / 2.0;
            assert!(
                (left - wanted).abs() < 1e-3,
                "a key at {at}s was drawn at {left}, not {wanted}",
            );
        }
    }

    /// A reused pool must not leave a stale row behind when switching to an empty clip.
    #[test]
    fn switching_to_an_empty_clip_leaves_nothing_behind() {
        let view = view();
        let mut world = with_a_ui_camera();
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
            .chain(pools.rows.iter())
            .filter_map(|entity| world.get::<Node>(*entity))
            .filter(|node| node.display != Display::None)
            .count();
        assert_eq!(drawn, 0, "{drawn} nodes were left over from the old clip");
    }

    /// Fails if the pool is turned into a rebuild, which could not zoom without allocating.
    #[test]
    fn the_pool_grows_and_never_despawns() {
        let view = view();
        let mut world = with_a_ui_camera();
        world.insert_resource(view);
        world.insert_resource(TimelineCursor::default());
        world.insert_resource(ShownClip(Some(a_clip())));
        world.init_resource::<TimelinePools>();

        world.run_system_once(spawn_the_strip).unwrap();
        world.run_system_once(lay_out_the_timeline).unwrap();
        world.run_system_once(lay_out_the_timeline).unwrap();

        let before: Vec<Entity> = world.resource::<TimelinePools>().keys.clone();
        assert!(!before.is_empty(), "no keys were pooled at all");

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

    /// A strip with nothing moving leaves every node it placed unchanged.
    ///
    /// A `Node` written is a `Node` changed even when the value is the same,
    /// and a changed `Node` is the whole strip laid out again — every frame,
    /// with the clip paused and nobody touching it. A persistent `Schedule`
    /// rather than `run_system_once`, so the system is the same one each time
    /// and its `Local`s and change ticks carry over the way they do in a frame.
    #[test]
    fn an_idle_strip_changes_no_node() {
        let mut world = with_a_ui_camera();
        world.insert_resource(view());
        world.insert_resource(TimelineCursor { time: 0.25, playing: false });
        world.insert_resource(ShownClip(Some(a_clip())));
        world.init_resource::<TimelinePools>();
        world.run_system_once(spawn_the_strip).unwrap();

        let mut schedule = Schedule::default();
        schedule.add_systems(lay_out_the_timeline);
        // Twice to settle: the first pass reserves the pools through
        // `Commands`, the second places them.
        schedule.run(&mut world);
        schedule.run(&mut world);

        let ticks = |world: &mut World| {
            let nodes: Vec<_> = world
                .query::<(Entity, Ref<Node>)>()
                .iter(world)
                .map(|(entity, node)| (entity, node.last_changed()))
                .collect();
            let texts: Vec<_> = world
                .query::<(Entity, Ref<Text>)>()
                .iter(world)
                .map(|(entity, text)| (entity, text.last_changed()))
                .collect();
            (nodes, texts)
        };
        let before = ticks(&mut world);
        assert!(before.0.len() > MAX_GRID, "the pools were never filled");

        schedule.run(&mut world);
        let after = ticks(&mut world);

        let moved = before
            .0
            .iter()
            .zip(&after.0)
            .filter(|(was, now)| was != now)
            .count();
        assert_eq!(moved, 0, "{moved} nodes were marked changed with nothing moving");
        assert_eq!(before.1, after.1, "a ruler label was rewritten with nothing moving");
    }

    fn view() -> TimelineView {
        TimelineView {
            start: 0.0,
            pixels_per_second: 200.0,
            rect: Rect::from_corners(Vec2::new(0.0, 400.0), Vec2::new(1000.0, 580.0)),
        }
    }

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

    /// Unclamped, a few notches reach a scale that cannot be zoomed back from by eye.
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

    /// A mismatch is a key that does not sit where it was dropped.
    #[test]
    fn a_time_and_its_pixel_are_the_same_fact() {
        let view = view();
        for time in [-1.0_f32, 0.0, 0.37, 4.5] {
            assert!((view.time_at(view.x_of(time)) - time).abs() < 1e-4);
        }
    }

    /// A drag starting on a track name is not a scrub.
    #[test]
    fn the_names_column_is_not_the_lanes() {
        let view = view();
        assert!(!view.over_lanes(Vec2::new(view.rect.min.x + 4.0, 500.0)));
        assert!(view.over_lanes(Vec2::new(view.lanes_left() + 4.0, 500.0)));
        assert!(!view.over_lanes(Vec2::new(view.lanes_left() + 4.0, 100.0)));
    }

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

    #[test]
    fn a_ruler_label_says_what_the_spacing_can_show() {
        assert_eq!(format_time(0.0, 1.0), "0");
        assert_eq!(format_time(-0.0, 1.0), "0");
        assert_eq!(format_time(2.0, 1.0), "2");
        assert_eq!(format_time(0.25, 0.05), "0.25");
        assert_eq!(format_time(0.125, 0.005), "0.125");
    }
}
