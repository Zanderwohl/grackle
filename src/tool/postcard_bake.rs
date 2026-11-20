//! The postcard bake: photographs of the map, 3:2 at [`HEIGHT`] pixels high whatever the window's
//! size, with no HUD, halved once to the size a postcard prints its picture at ([`KEPT_HEIGHT`]),
//! which is all that is stored. One is taken through each spectator camera at its own vertical field of view,
//! and one straight down, orthographic, turned in [`ANGLE_STEP_DEGREES`] steps to whichever angle
//! lets the map's rooms and blocks fill the most of the frame.
//!
//! They are written to `cache/postcards/{hash}/`, each named by its [`PostcardChoice::file_stem`]:
//! `top.png`, `camera_{feature}.png`, `camera_{feature}_copy_{image}.png`. The hash is FNV over every
//! feature as it would be saved, the map's settings that a picture can show
//! ([`SavedMetadata::pictured`]) and halfbright, which photographs without sunlight. Pack content
//! (materials, props) is not in it: after editing one, delete `cache/postcards/`.
//!
//! Only the authority takes them; any machine reads a set already on disk. The set read or taken
//! most recently is kept as [`CapturedPostcards`] for the postcard picker ([`postcard_button`]) and
//! the printed cards ([`crate::common::postcard_sync`]), until another set replaces it or another
//! map is opened ([`FeatureTimeline::adoptions`]) or the machine joins or leaves a server, which
//! evict it.
//!
//! Taken only in the map editor and not while rolled back, once the rooms and the sunlight (unless
//! halfbright skips it) are baked and the map has been still for [`bake_job::SETTLE_SECONDS`]. One
//! camera is re-aimed for each shot, so only one target exists at a time. Spawn-point
//! bodies are hidden while it shoots ([`PostcardBake::is_shooting`]).

use std::hash::{Hash, Hasher};
use std::collections::HashMap;
use std::path::PathBuf;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Hdr, RenderTarget, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

use bevy::tasks::futures::check_ready;
use bevy::tasks::{AsyncComputeTaskPool, IoTaskPool, Task};
use bevy_egui::{egui, EguiContexts, EguiTextureHandle};

use crate::common::app_mode::AppMode;
use crate::common::render_layers;
use crate::common::stable_hash::Fnv;
use crate::editor::editable::FeatureTimeline;
use crate::common::net::NetRole;
use crate::common::picker::{picker_button_or_missing, Choice, PickerKind};
use crate::prop::thumbnail::{is_blank, ATTEMPTS, CAPTURE_TIMEOUT, RETRY_FRAMES};
use crate::editor::editable::FeatureId;
use crate::editor::map_metadata::{BuiltInPostcard, MapMetadata, PostcardChoice};
use crate::editor::save::SavedMetadata;
use crate::game::spectate::{self, Cameras};
use crate::get;
use crate::tool::bake_job::{self, BakeKind, BakeProgress, BakeState, BakeStatus, VolumeLabels};
use crate::tool::room::MapVolumes;
use crate::tool::sun_bake::SunBake;

/// Pixels rendered: twice the size kept, so halving it is an even average of four pixels each.
pub const HEIGHT: u32 = 2 * KEPT_HEIGHT;
pub const WIDTH: u32 = HEIGHT * 3 / 2;
const ASPECT: f32 = WIDTH as f32 / HEIGHT as f32;

/// Bump after changing how a postcard is taken, so old ones are a miss.
const FORMAT: u32 = 2;

pub const CACHE_DIR: &str = "cache/postcards";

/// Pixels: a picture's height in the picker.
pub const PREVIEW_HEIGHT: u32 = 200;
const PREVIEW_WIDTH: u32 = PREVIEW_HEIGHT * 3 / 2;

/// Pixels of a picture as stored and kept: exactly the size a postcard prints its picture at, so it
/// is never resampled again.
pub const KEPT_WIDTH: u32 = crate::common::postcard_print::PICTURE_WIDTH;
pub const KEPT_HEIGHT: u32 = crate::common::postcard_print::PICTURE_HEIGHT;

/// One wide, so each is big enough to see.
const PICKER: PickerKind = PickerKind::captioned("postcard", 1, [PREVIEW_WIDTH as f32, PREVIEW_HEIGHT as f32]);

pub const ANGLE_STEP_DEGREES: u32 = 5;

/// Of each half-extent, in from each edge of the top-down view.
const PADDING: f32 = 0.04;

/// Meters the top-down camera stands above the highest ceiling, and its far plane below the lowest
/// floor.
const CLEARANCE: f32 = 1.0;

/// Half-height of the top-down frame for a map with no extent, in meters.
const SMALLEST_HALF_HEIGHT: f32 = 1.0;

/// Frames the camera renders before its first capture, so the pipelines a new target needs are
/// compiled; then frames between re-aiming it and the next capture.
const FIRST_SHOT_FRAMES: u32 = 30;
const NEXT_SHOT_FRAMES: u32 = 4;

/// Seconds the feature list must go untouched before it is hashed again, so a drag is serialized
/// once when it stops rather than every frame.
const FEATURES_QUIET_SECONDS: f32 = 0.25;

/// Seconds the bake's inputs must have stood ready before a bake asked for starts, so it does not
/// start in a frame the room or sun bake has yet to notice an edit in.
const ASKED_SETTLE_SECONDS: f32 = 0.25;

/// A request to take the postcards now, whatever auto-bake is doing.
#[derive(Message)]
pub struct BakePostcards;

/// A request to delete this map's postcards, which auto-bake then takes again.
#[derive(Message)]
pub struct ClearPostcards;

#[derive(Component)]
struct PostcardCamera;

/// Disjoint from [`Cameras`], which reads every spectator camera's `Transform`.
type Rig = (With<PostcardCamera>, Without<crate::editor::spectator_camera::SpectatorCameraSpec>);

#[derive(Resource)]
pub struct PostcardBake {
    auto: bool,
    asked: bool,
    /// [`FeatureTimeline::content_hash`], recomputed once the timeline has been quiet for
    /// [`FEATURES_QUIET_SECONDS`].
    features: Option<u64>,
    /// When the timeline last changed, while `features` is out of date.
    features_stale_since: Option<f32>,
    /// The settings last hashed, and their hash, so unchanged settings are not encoded again.
    settings: Option<(SavedMetadata, u64)>,
    /// `changed_at` is when `current` was first seen, in real seconds.
    current: Option<u64>,
    changed_at: f32,
    /// Since when the other bakes have been done and the map editor open.
    ready_since: Option<f32>,
    /// Whose postcards are on disk.
    baked: Option<u64>,
    /// The last hash looked for on disk, so a miss is not looked for every frame.
    looked_up: Option<u64>,
    sitting: Option<Sitting>,
    writing: Option<Writing>,
    /// Asked while the set of this hash was being written, so it is deleted once written.
    clear_after_write: bool,
    /// A hash whose bake gave up (no capture came back, or the write failed): not tried again
    /// until the map changes or a bake is asked for.
    gave_up: Option<u64>,
    completed: usize,
    scheduled: usize,
    /// The map last opened and the role it was opened in.
    opened: Option<(u64, Option<NetRole>)>,
    /// Bumped by every eviction, so a write landing afterward does not bring the old map's pictures
    /// back.
    openings: u64,
}

impl Default for PostcardBake {
    fn default() -> Self {
        Self {
            auto: true,
            asked: false,
            features: None,
            features_stale_since: None,
            settings: None,
            current: None,
            changed_at: 0.0,
            ready_since: None,
            baked: None,
            looked_up: None,
            sitting: None,
            writing: None,
            clear_after_write: false,
            gave_up: None,
            completed: 0,
            scheduled: 0,
            opened: None,
            openings: 0,
        }
    }
}

impl PostcardBake {
    pub fn auto(&self) -> bool {
        self.auto
    }

    pub fn set_auto(&mut self, auto: bool) {
        self.auto = auto;
    }

    /// So spawn-point bodies can be hidden from the pictures.
    pub fn is_shooting(&self) -> bool {
        self.sitting.is_some()
    }

    fn status(&self) -> BakeStatus {
        let state = if self.sitting.is_some() || self.writing.is_some() {
            BakeState::Baking
        } else if self.current.is_none() || self.baked == self.current {
            BakeState::UpToDate
        } else if (self.auto || self.asked) && self.gave_up != self.current {
            BakeState::Waiting
        } else {
            BakeState::Stale
        };
        let doing = self.sitting.as_ref().and_then(|sitting| sitting.shots.get(sitting.next)).map(|shot| shot.label.clone());
        BakeStatus { state, auto: self.auto, completed: self.completed, scheduled: self.scheduled, doing: doing.unwrap_or_default() }
    }

    /// `openings` is the count the write started under.
    fn landed_write(
        &mut self,
        captured: &mut CapturedPostcards,
        images: &mut Assets<Image>,
        (hash, openings): (u64, u64),
        saved: Result<(), String>,
        kept: Vec<(String, image::RgbaImage)>,
    ) {
        let clear = std::mem::take(&mut self.clear_after_write);
        if openings == self.openings {
            captured.keep(hash, kept, images);
        }
        match saved {
            Ok(()) if clear => disk::remove(hash),
            Ok(()) => {
                info!("Saved postcards to {}", folder(hash).display());
                self.baked = Some(hash);
            }
            Err(e) => {
                warn!("Could not save postcards to {}: {e}", folder(hash).display());
                self.gave_up = Some(hash);
            }
        }
    }
}

struct Writing {
    hash: u64,
    openings: u64,
    task: Task<(Result<(), String>, Vec<(String, image::RgbaImage)>)>,
}

/// Halves the set on the compute pool, then writes it on the IO pool.
fn write_and_shrink(hash: u64, pictures: Vec<(String, image::RgbaImage)>) -> Task<(Result<(), String>, Vec<(String, image::RgbaImage)>)> {
    AsyncComputeTaskPool::get().spawn(async move {
        let kept = halved(&pictures);
        let written = kept.clone();
        let saved = IoTaskPool::get().spawn(async move { disk::store(hash, &written) }).await;
        (saved, kept)
    })
}

struct Shot {
    /// The file's stem.
    name: String,
    label: String,
    transform: Transform,
    projection: Projection,
    clear: ClearColorConfig,
}

struct Sitting {
    hash: u64,
    shots: Vec<Shot>,
    /// The shot being taken; equal to the count once all are.
    next: usize,
    aimed: bool,
    camera: Entity,
    target: Handle<Image>,
    frames: u32,
    shoot_at: u32,
    /// The capture in flight, so a late answer to an abandoned one is not taken for this.
    shot: Option<Entity>,
    attempts: u32,
    developed: Vec<(String, image::RgbaImage)>,
}

/// Of the settings a picture can show; JSON writes an `f32` as its shortest decimal, so equal
/// settings hash equal.
pub fn settings_hash(settings: &SavedMetadata) -> u64 {
    crate::common::stable_hash::of_text(&serde_json::to_string(&settings.pictured()).unwrap_or_default())
}

pub fn postcard_hash(features: u64, settings: u64, halfbright: bool) -> u64 {
    let mut hasher = Fnv::default();
    FORMAT.hash(&mut hasher);
    HEIGHT.hash(&mut hasher);
    ANGLE_STEP_DEGREES.hash(&mut hasher);
    features.hash(&mut hasher);
    settings.hash(&mut hasher);
    halfbright.hash(&mut hasher);
    hasher.finish()
}

pub fn folder(hash: u64) -> PathBuf {
    PathBuf::from(CACHE_DIR).join(format!("{hash:016x}"))
}

/// A camera looking straight down, turned `yaw` radians about `+Y`: at zero, screen up is `-Z`
/// and screen right `+X`. View-space `z` is world `y`.
fn looking_down(yaw: f32) -> Quat {
    Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopDown {
    /// Radians about `+Y`, a multiple of [`ANGLE_STEP_DEGREES`] in `[0°, 180°)`.
    pub yaw: f32,
    pub transform: Transform,
    /// Of the frame, in meters.
    pub width: f32,
    pub height: f32,
    /// The camera's near plane is at zero.
    pub far: f32,
}

/// Each yaw is tried and the one needing the smallest frame wins, the smallest yaw on a tie, so an
/// axis-aligned map is not turned.
pub fn top_down(points: &[Vec3]) -> Option<TopDown> {
    let (low, high) = points.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| (lo.min(p.y), hi.max(p.y)));
    if !low.is_finite() {
        return None;
    }
    let (half_height, yaw, center) = (0..180 / ANGLE_STEP_DEGREES)
        .map(|step| {
            let yaw = ((step * ANGLE_STEP_DEGREES) as f32).to_radians();
            let to_view = looking_down(yaw).inverse();
            let (lo, hi) = points.iter().fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), p| {
                let v = (to_view * *p).xy();
                (lo.min(v), hi.max(v))
            });
            let half = (hi - lo) / 2.0;
            (half.y.max(half.x / ASPECT), yaw, (lo + hi) / 2.0)
        })
        // `min_by` keeps the first of equals, which is the smallest yaw.
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    let rotation = looking_down(yaw);
    let height = 2.0 * half_height.max(SMALLEST_HALF_HEIGHT) / (1.0 - PADDING);
    let translation = rotation * center.extend(0.0) + Vec3::Y * (high + CLEARANCE);
    Some(TopDown {
        yaw,
        transform: Transform::from_translation(translation).with_rotation(rotation),
        width: height * ASPECT,
        height,
        far: high - low + 2.0 * CLEARANCE,
    })
}

/// Footprint rooms and blocks included, at floor and ceiling.
fn volume_corners(volumes: &MapVolumes) -> Vec<Vec3> {
    volumes
        .each()
        .flat_map(|(_, room, _, _)| {
            room.footprint()
                .iter()
                .flat_map(move |p| [Vec3::new(p.x, room.floor, p.y), Vec3::new(p.x, room.ceiling, p.y)])
                .collect::<Vec<_>>()
        })
        .collect()
}

/// What a spectator camera's postcard is named by; `None` for one standing for no feature.
fn camera_choice(tag: Option<&crate::editor::editable::FeatureTag>, copy: Option<&crate::editor::mirror::MirrorCopy>) -> Option<PostcardChoice> {
    match (tag, copy) {
        (_, Some(copy)) => Some(PostcardChoice::SpectatorCamera { feature: copy.source, copy: Some(copy.image as u32) }),
        (Some(tag), None) => Some(PostcardChoice::SpectatorCamera { feature: FeatureId::from_raw(tag.feature_id), copy: None }),
        (None, None) => None,
    }
}

/// Every spectator camera with a postcard, in spectating order. A copy of a copy can share its
/// source and image with another: the first keeps the postcard.
fn postcard_cameras(cameras: &Cameras) -> Vec<(Entity, PostcardChoice)> {
    let mut found: Vec<(Entity, PostcardChoice)> = Vec::new();
    for (camera, _) in spectate::every_camera(cameras) {
        let Ok((_, _, _, tag, copy, _)) = cameras.get(camera) else { continue };
        let Some(choice) = camera_choice(tag, copy) else { continue };
        if !found.iter().any(|(_, seen)| *seen == choice) {
            found.push((camera, choice));
        }
    }
    found
}

fn plan_shots(volumes: &MapVolumes, cameras: &Cameras, placed: &Query<&GlobalTransform>, labels: &VolumeLabels) -> Vec<Shot> {
    let mut shots = Vec::new();
    if let Some(top) = top_down(&volume_corners(volumes)) {
        shots.push(Shot {
            name: PostcardChoice::BuiltIn(BuiltInPostcard::Top).file_stem(),
            label: get!("bakes.doing.top_down"),
            transform: top.transform,
            projection: Projection::Orthographic(OrthographicProjection {
                near: 0.0,
                far: top.far,
                scaling_mode: ScalingMode::Fixed { width: top.width, height: top.height },
                ..OrthographicProjection::default_3d()
            }),
            clear: ClearColorConfig::Custom(Color::NONE),
        });
    }
    for (camera, choice) in postcard_cameras(cameras) {
        let Ok((_, _, spec, ..)) = cameras.get(camera) else { continue };
        let Ok(placed) = placed.get(camera) else { continue };
        let (_, rotation, translation) = placed.to_scale_rotation_translation();
        shots.push(Shot {
            name: choice.file_stem(),
            label: labels.label(camera),
            transform: Transform::from_translation(translation).with_rotation(rotation),
            projection: Projection::Perspective(PerspectiveProjection { fov: spec.fov, aspect_ratio: ASPECT, ..default() }),
            clear: ClearColorConfig::Default,
        });
    }
    shots
}

/// Native only, as the other caches are: a browser tab has nowhere to keep them.
#[cfg(not(target_arch = "wasm32"))]
mod disk {
    use super::*;

    pub fn exists(hash: u64) -> bool {
        folder(hash).is_dir()
    }

    pub fn remove(hash: u64) {
        let _ = std::fs::remove_dir_all(folder(hash));
    }

    /// The pictures, by file stem. A picture that will not decode is left out.
    pub fn read(hash: u64) -> Vec<(String, image::RgbaImage)> {
        let Ok(entries) = std::fs::read_dir(folder(hash)) else { return Vec::new() };
        entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
            .filter_map(|path| {
                let stem = path.file_stem()?.to_str()?.to_owned();
                match image::open(&path) {
                    Ok(picture) => Some((stem, picture.to_rgba8())),
                    Err(e) => {
                        warn!("Ignoring postcard {}: {e}", path.display());
                        None
                    }
                }
            })
            .collect()
    }

    /// Into a partial folder renamed into place, so a folder that exists is a whole set.
    pub fn store(hash: u64, pictures: &[(String, image::RgbaImage)]) -> Result<(), String> {
        let (done, partial) = (folder(hash), folder(hash).with_extension("partial"));
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
        for (name, picture) in pictures {
            picture.save(partial.join(format!("{name}.png"))).map_err(|e| e.to_string())?;
        }
        let _ = std::fs::remove_dir_all(&done);
        std::fs::rename(&partial, &done).map_err(|e| e.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
mod disk {
    pub fn exists(_: u64) -> bool {
        false
    }

    pub fn remove(_: u64) {}

    pub fn read(_: u64) -> Vec<(String, image::RgbaImage)> {
        Vec::new()
    }

    pub fn store(_: u64, _: &[(String, image::RgbaImage)]) -> Result<(), String> {
        Ok(())
    }
}

fn put_away(commands: &mut Commands, images: &mut Assets<Image>, sitting: Sitting) {
    commands.entity(sitting.camera).despawn();
    images.remove(&sitting.target);
}

fn ready(mode: &AppMode, timeline: &FeatureTimeline, progress: &BakeProgress) -> bool {
    let done = |kind, states: &[BakeState]| progress.get(kind).is_none_or(|status| states.contains(&status.state));
    *mode == AppMode::Editor
        && timeline.rollback().is_none()
        && done(BakeKind::Rooms, &[BakeState::UpToDate])
        && done(BakeKind::Sunlight, &[BakeState::UpToDate, BakeState::Skipped])
}

#[derive(SystemParam)]
struct Subjects<'w, 's> {
    volumes: MapVolumes<'w, 's>,
    cameras: Cameras<'w, 's>,
    placed: Query<'w, 's, &'static GlobalTransform>,
    labels: VolumeLabels<'w, 's>,
}

#[allow(clippy::too_many_arguments)]
fn bake_postcards(
    mut commands: Commands,
    mut state: ResMut<PostcardBake>,
    mode: Res<State<AppMode>>,
    timeline: Res<FeatureTimeline>,
    metadata: Res<MapMetadata>,
    sun: Res<SunBake>,
    progress: Res<BakeProgress>,
    time: Res<Time<Real>>,
    (mut asks, mut clears): (MessageReader<BakePostcards>, MessageReader<ClearPostcards>),
    subjects: Subjects,
    (role, mut captured): (Option<Res<NetRole>>, ResMut<CapturedPostcards>),
    mut images: ResMut<Assets<Image>>,
    mut rigs: Query<(&mut Transform, &mut Projection, &mut Camera), Rig>,
) {
    let state = &mut *state;
    let now = time.elapsed_secs();
    let authority = role.as_ref().is_none_or(|role| role.is_authority());
    let opened = (timeline.adoptions(), role.as_deref().cloned());
    if state.opened.as_ref() != Some(&opened) {
        if state.opened.is_some() {
            captured.evict(&mut images);
            state.openings += 1;
            // So the new map's set is looked for even when it is the one this was last.
            state.baked = None;
            state.looked_up = None;
        }
        state.opened = Some(opened);
    }

    if timeline.is_changed() {
        state.features_stale_since = Some(now);
    }
    let features_quiet = state.features_stale_since.is_none_or(|since| now - since >= FEATURES_QUIET_SECONDS);
    if state.features.is_none() || (state.features_stale_since.is_some() && features_quiet) {
        state.features = Some(timeline.content_hash());
        state.features_stale_since = None;
    }
    let features_stale = state.features_stale_since.is_some();
    if !state.settings.as_ref().is_some_and(|(settings, _)| settings.matches(&metadata)) {
        let settings = SavedMetadata::of(&metadata);
        let hash = settings_hash(&settings);
        state.settings = Some((settings, hash));
    }
    let settings = state.settings.as_ref().map_or(0, |(_, hash)| *hash);
    let hash = postcard_hash(state.features.unwrap_or_default(), settings, sun.halfbright());
    if state.current != Some(hash) || features_stale {
        state.current = Some(hash);
        state.changed_at = now;
    }

    if let Some(writing) = &mut state.writing
        && let Some((saved, kept)) = check_ready(&mut writing.task)
    {
        let key = (writing.hash, writing.openings);
        state.writing = None;
        state.landed_write(&mut captured, &mut images, key, saved, kept);
    }

    if clears.read().count() > 0 {
        match &state.writing {
            Some(writing) if writing.hash == hash => state.clear_after_write = true,
            _ => disk::remove(hash),
        }
        state.baked = None;
        state.looked_up = None;
        state.gave_up = None;
        if let Some(sitting) = state.sitting.take() {
            put_away(&mut commands, &mut images, sitting);
        }
        info!("Cleared the postcards");
    }
    if asks.read().count() > 0 {
        state.asked = true;
        state.gave_up = None;
    }
    let is_ready = ready(mode.get(), &timeline, &progress);

    // An edit not yet hashed may already be in the picture, which would be filed under the old hash.
    if let Some(sitting) = &mut state.sitting {
        if sitting.hash != hash || features_stale || *mode.get() != AppMode::Editor || !authority {
            let sitting = state.sitting.take().unwrap();
            put_away(&mut commands, &mut images, sitting);
        } else {
            sit(&mut commands, state, &mut images, &mut rigs);
            return;
        }
    }
    if state.writing.is_some() {
        return;
    }

    if state.baked == Some(hash) {
        state.asked = false;
        return;
    }
    if state.looked_up != Some(hash) {
        state.looked_up = Some(hash);
        if disk::exists(hash) {
            state.baked = Some(hash);
            state.asked = false;
            captured.read(hash);
            return;
        }
    }

    if !is_ready || !authority || features_stale {
        state.ready_since = None;
        return;
    }
    if state.gave_up == Some(hash) {
        return;
    }
    let ready_since = *state.ready_since.get_or_insert(now);
    if !(state.auto || state.asked) {
        return;
    }
    let settled = if state.asked {
        now - ready_since >= ASKED_SETTLE_SECONDS
    } else {
        now - ready_since.max(state.changed_at) >= bake_job::SETTLE_SECONDS
    };
    if !settled {
        return;
    }
    state.asked = false;

    let shots = plan_shots(&subjects.volumes, &subjects.cameras, &subjects.placed, &subjects.labels);
    (state.completed, state.scheduled) = (0, shots.len());
    let Some(first) = shots.first() else {
        state.baked = Some(hash);
        return;
    };
    let target = images.add(Image::new_target_texture(WIDTH, HEIGHT, TextureFormat::Rgba8UnormSrgb, None));
    let camera = commands
        .spawn((
            PostcardCamera,
            Camera3d::default(),
            Camera { order: -2, clear_color: first.clear, ..default() },
            RenderTarget::Image(target.clone().into()),
            // As the play camera, so the main pass shares its pipelines.
            Hdr,
            Tonemapping::TonyMcMapface,
            first.projection.clone(),
            first.transform,
            RenderLayers::layer(render_layers::WORLD),
            Name::new("Postcard camera"),
        ))
        .id();
    state.sitting = Some(Sitting {
        hash,
        shots,
        next: 0,
        aimed: true,
        camera,
        target,
        frames: 0,
        shoot_at: FIRST_SHOT_FRAMES,
        shot: None,
        attempts: 0,
        developed: Vec::new(),
    });
}

fn sit(
    commands: &mut Commands,
    state: &mut PostcardBake,
    images: &mut Assets<Image>,
    rigs: &mut Query<(&mut Transform, &mut Projection, &mut Camera), Rig>,
) {
    let Some((next, shots)) = state.sitting.as_ref().map(|sitting| (sitting.next, sitting.shots.len())) else { return };
    state.completed = next;
    if next >= shots {
        let sitting = state.sitting.take().unwrap();
        commands.entity(sitting.camera).despawn();
        images.remove(&sitting.target);
        state.writing = Some(Writing { hash: sitting.hash, openings: state.openings, task: write_and_shrink(sitting.hash, sitting.developed) });
        return;
    }
    let Some(sitting) = &mut state.sitting else { return };
    if !sitting.aimed {
        let shot = &sitting.shots[sitting.next];
        if let Ok((mut transform, mut projection, mut camera)) = rigs.get_mut(sitting.camera) {
            *transform = shot.transform;
            *projection = shot.projection.clone();
            camera.clear_color = shot.clear;
        }
        sitting.aimed = true;
    }
    sitting.frames += 1;
    if sitting.frames == sitting.shoot_at {
        sitting.shot = Some(commands.spawn(Screenshot::image(sitting.target.clone())).observe(develop).id());
    } else if sitting.shot.is_some() && sitting.frames > sitting.shoot_at + CAPTURE_TIMEOUT {
        sitting.shot = None;
        sitting.shoot_at = sitting.frames + 1;
        sitting.attempts += 1;
        if sitting.attempts >= ATTEMPTS {
            let sitting = state.sitting.take().unwrap();
            warn!("Gave up on the postcards: no picture came back");
            state.gave_up = Some(sitting.hash);
            put_away(commands, images, sitting);
        }
    }
}

fn develop(captured: On<ScreenshotCaptured>, mut state: ResMut<PostcardBake>) {
    let Some(sitting) = &mut state.sitting else { return };
    if sitting.shot != Some(captured.entity) {
        return;
    }
    sitting.shot = None;
    let name = sitting.shots[sitting.next].name.clone();
    let picture = match captured.image.clone().try_into_dynamic() {
        Ok(picture) => picture.to_rgba8(),
        Err(e) => {
            error!("Postcard {name} came back unreadable: {e}");
            next_shot(sitting);
            return;
        }
    };
    // A blank picture is kept once out of attempts: it may be of a blank wall.
    if is_blank(&picture) && sitting.attempts + 1 < ATTEMPTS {
        sitting.attempts += 1;
        sitting.shoot_at = sitting.frames + RETRY_FRAMES;
        return;
    }
    sitting.developed.push((name, picture));
    next_shot(sitting);
}

fn next_shot(sitting: &mut Sitting) {
    sitting.next += 1;
    sitting.aimed = false;
    sitting.attempts = 0;
    sitting.shoot_at = sitting.frames + NEXT_SHOT_FRAMES;
}

/// The set of postcards read or taken most recently.
#[derive(Resource, Default)]
pub struct CapturedPostcards {
    hash: Option<u64>,
    /// By file stem.
    pictures: HashMap<String, Handle<Image>>,
    /// Bumped whenever `pictures` is replaced.
    generation: u64,
    loading: Option<(u64, Task<Vec<(String, image::RgbaImage)>>)>,
}

impl CapturedPostcards {
    pub fn get(&self, stem: &str) -> Option<&Handle<Image>> {
        self.pictures.get(stem)
    }

    #[cfg(test)]
    pub fn keep_for_test(&mut self, stem: &str, picture: Handle<Image>) {
        self.pictures.insert(stem.into(), picture);
    }

    /// Changes whenever the set is replaced or evicted.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Forgets the set kept and any read in progress.
    fn evict(&mut self, images: &mut Assets<Image>) {
        for (_, handle) in self.pictures.drain() {
            images.remove(&handle);
        }
        self.hash = None;
        self.loading = None;
        self.generation += 1;
    }

    /// The pictures on disk under `hash`, unless it is the one kept or being read, read on the
    /// IO pool.
    fn read(&mut self, hash: u64) {
        if self.hash == Some(hash) || self.loading.as_ref().is_some_and(|(loading, _)| *loading == hash) {
            return;
        }
        let task = IoTaskPool::get().spawn(async move { disk::read(hash) });
        self.loading = Some((hash, task));
    }

    fn keep(&mut self, hash: u64, kept: Vec<(String, image::RgbaImage)>, images: &mut Assets<Image>) {
        self.evict(images);
        for (stem, picture) in kept {
            let image = Image::from_dynamic(image::DynamicImage::ImageRgba8(picture), true, RenderAssetUsages::default());
            self.pictures.insert(stem, images.add(image));
        }
        self.hash = Some(hash);
    }
}

fn halved(pictures: &[(String, image::RgbaImage)]) -> Vec<(String, image::RgbaImage)> {
    pictures
        .iter()
        .map(|(stem, picture)| {
            (stem.clone(), image::imageops::resize(picture, KEPT_WIDTH, KEPT_HEIGHT, image::imageops::FilterType::Triangle))
        })
        .collect()
}

fn finish_reading_postcards(mut captured: ResMut<CapturedPostcards>, mut images: ResMut<Assets<Image>>) {
    let Some((hash, task)) = &mut captured.loading else { return };
    let Some(pictures) = check_ready(task) else { return };
    let hash = *hash;
    captured.loading = None;
    captured.keep(hash, pictures, &mut images);
}

/// True when a choice was made.
pub fn postcard_button(ui: &mut egui::Ui, slot: &mut Option<PostcardChoice>) -> bool {
    let current = slot.map(|choice| choice.file_stem());
    let plain = get!("editor.metadata.postcard_none");
    let missing = get!("editor.metadata.postcard_missing");
    let Some(pick) = picker_button_or_missing(ui, PICKER, current.as_deref(), Some(&plain), &missing) else { return false };
    let pick = pick.and_then(|stem| PostcardChoice::from_file_stem(&stem));
    let changed = *slot != pick;
    *slot = pick;
    changed
}

/// Every frame, not only while a picker is open: a slot's button is labeled from these, and a slot
/// naming no choice reads as a missing camera.
fn publish_postcard_choices(
    mut contexts: EguiContexts,
    captured: Res<CapturedPostcards>,
    cameras: Cameras,
    labels: VolumeLabels,
    mut registered: Local<(u64, Vec<AssetId<Image>>)>,
) {
    if registered.0 != captured.generation {
        for id in registered.1.drain(..) {
            contexts.remove_image(id);
        }
        registered.0 = captured.generation;
    }
    let not_generated = get!("bakes.postcard.not_generated");
    let mut offered = vec![(PostcardChoice::BuiltIn(BuiltInPostcard::Top), get!("bakes.doing.top_down"))];
    offered.extend(postcard_cameras(&cameras).into_iter().map(|(camera, choice)| (choice, labels.label(camera))));
    let mut choices = Vec::new();
    for (choice, label) in offered {
        let stem = choice.file_stem();
        let preview = captured.get(&stem).map(|handle| {
            if !registered.1.contains(&handle.id()) {
                registered.1.push(handle.id());
            }
            contexts.add_image(EguiTextureHandle::Weak(handle.id()))
        });
        choices.push(Choice { label, preview, placeholder: Some(not_generated.clone()), ..Choice::named(stem) });
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    PICKER.publish(ctx, choices);
}

fn report_postcards(state: Res<PostcardBake>, mut progress: ResMut<BakeProgress>) {
    bake_job::report(&mut progress, BakeKind::Postcards, state.status());
}

/// The panel's reads and writes, bundled to stay inside its parameter limit.
#[derive(SystemParam)]
pub struct PostcardPanel<'w> {
    bake: ResMut<'w, PostcardBake>,
    ask: MessageWriter<'w, BakePostcards>,
    clear: MessageWriter<'w, ClearPostcards>,
}

impl PostcardPanel<'_> {
    pub fn auto(&self) -> bool {
        self.bake.auto()
    }

    pub fn apply(&mut self, auto: bool, bake: bool, clear: bool) {
        if auto != self.bake.auto() {
            self.bake.set_auto(auto);
        }
        if bake {
            self.ask.write(BakePostcards);
        }
        if clear {
            self.clear.write(ClearPostcards);
        }
    }
}

/// Added in `main.rs` beside the thumbnails rather than with the other bakes, because it renders.
pub struct PostcardBakePlugin;

impl Plugin for PostcardBakePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PostcardBake>()
            .init_resource::<CapturedPostcards>()
            .init_resource::<BakeProgress>()
            .add_message::<BakePostcards>()
            .add_message::<ClearPostcards>()
            .add_systems(
                Update,
                (bake_postcards, finish_reading_postcards, report_postcards, publish_postcard_choices.run_if(in_state(AppMode::Editor)))
                    .chain()
                    .after(crate::tool::bakes::BakePlugin::auto_bake_rooms),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rectangle(center: Vec2, length: f32, width: f32, yaw_degrees: f32) -> Vec<Vec3> {
        let (along, across) = (Vec2::from_angle(yaw_degrees.to_radians()), Vec2::from_angle((yaw_degrees + 90.0).to_radians()));
        let mut points = Vec::new();
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = center + along * a * length / 2.0 + across * b * width / 2.0;
            points.push(Vec3::new(p.x, 0.0, p.y));
            points.push(Vec3::new(p.x, 4.0, p.y));
        }
        points
    }

    /// Where each point lands as a fraction of the frame's half-extents, and its depth.
    fn projected(points: &[Vec3], top: &TopDown) -> Vec<(Vec2, f32)> {
        let to_view = top.transform.rotation.inverse();
        let half = Vec2::new(top.width, top.height) / 2.0;
        points
            .iter()
            .map(|p| {
                let v = to_view * (*p - top.transform.translation);
                (v.xy() / half, -v.z)
            })
            .collect()
    }

    /// Breaks if any corner of the map is outside the frame or the depth range.
    #[test]
    fn every_corner_is_in_the_top_down_frame() {
        let mut points = rectangle(Vec2::new(30.0, -12.0), 40.0, 6.0, 33.0);
        points.extend(rectangle(Vec2::new(-5.0, 8.0), 10.0, 10.0, 0.0));
        let top = top_down(&points).unwrap();
        for (at, depth) in projected(&points, &top) {
            assert!(at.abs().max_element() <= 1.0 - PADDING + 1e-4, "a corner lands at {at}");
            assert!(depth >= 0.0 && depth <= top.far, "a corner at depth {depth} is outside 0..{}", top.far);
        }
        assert!((top.width / top.height - ASPECT).abs() < 1e-5);
    }

    /// Breaks if a long map is not turned to lie along the frame's width.
    #[test]
    fn a_long_diagonal_map_is_turned_to_fill_the_frame() {
        let points = rectangle(Vec2::ZERO, 60.0, 8.0, 35.0);
        let top = top_down(&points).unwrap();
        let reach = projected(&points, &top).iter().fold(0.0f32, |reach, (at, _)| reach.max(at.x.abs()));
        assert!((reach - (1.0 - PADDING)).abs() < 1e-3, "the long axis reaches {reach} of the width");
        let unturned = top_down(&rectangle(Vec2::ZERO, 60.0, 8.0, 0.0)).unwrap();
        assert!((top.height - unturned.height).abs() < 1e-3, "the turned frame is {} high, not {}", top.height, unturned.height);
    }

    #[test]
    fn an_axis_aligned_map_is_not_turned() {
        let top = top_down(&rectangle(Vec2::new(3.0, 4.0), 30.0, 10.0, 0.0)).unwrap();
        assert_eq!(top.yaw, 0.0);
    }

    #[test]
    fn nothing_to_frame_is_no_top_down_view() {
        assert_eq!(top_down(&[]), None);
    }

    /// Breaks if halfbright pictures are filed with sunlit ones, or equal inputs hash differently.
    #[test]
    fn the_hash_follows_the_map_and_halfbright() {
        let settings = settings_hash(&SavedMetadata::of(&MapMetadata::default()));
        assert_eq!(postcard_hash(1, settings, false), postcard_hash(1, settings, false));
        assert_ne!(postcard_hash(1, settings, false), postcard_hash(1, settings, true));
        assert_ne!(postcard_hash(1, settings, false), postcard_hash(2, settings, false));
    }

    /// Breaks if choosing a postcard, renaming the map or changing its music would make every
    /// postcard stale.
    #[test]
    fn choices_that_no_picture_shows_leave_the_hash_alone() {
        let mut metadata = MapMetadata::default();
        let hash = |metadata: &MapMetadata| settings_hash(&SavedMetadata::of(metadata));
        let before = hash(&metadata);
        metadata.postcards[0] = Some(PostcardChoice::BuiltIn(BuiltInPostcard::Top));
        metadata.display_name = Some("Harvest".into());
        metadata.music = vec!["music/Pump/Pump.mp3".into()];
        assert_eq!(hash(&metadata), before);
        metadata.sun.altitude += 10.0;
        assert_ne!(hash(&metadata), before);
    }

    #[test]
    fn a_choice_reads_back_from_its_file_stem() {
        for choice in [
            PostcardChoice::BuiltIn(BuiltInPostcard::Top),
            PostcardChoice::SpectatorCamera { feature: FeatureId::from_raw(12), copy: None },
            PostcardChoice::SpectatorCamera { feature: FeatureId::from_raw(12), copy: Some(3) },
        ] {
            assert_eq!(PostcardChoice::from_file_stem(&choice.file_stem()), Some(choice));
        }
        assert_eq!(PostcardChoice::from_file_stem("camera_x"), None);
    }

    /// Breaks if a picture of the map that was open survives into the next one.
    #[test]
    fn eviction_drops_every_picture_and_any_read_in_flight() {
        bevy::tasks::IoTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut images = Assets::<Image>::default();
        let mut captured = CapturedPostcards::default();
        let handle = images.add(Image::default());
        captured.pictures.insert("top".into(), handle.clone());
        captured.hash = Some(1);
        captured.read(2);
        let generation = captured.generation;
        captured.evict(&mut images);
        assert!(captured.get("top").is_none());
        assert!(images.get(&handle).is_none(), "the picture's image was kept");
        assert!(captured.hash.is_none() && captured.loading.is_none());
        assert_ne!(captured.generation, generation, "the picker would keep showing the old textures");
    }

    fn kept_set() -> Vec<(String, image::RgbaImage)> {
        vec![("top".into(), image::RgbaImage::new(3, 2))]
    }

    /// Breaks if a set written after another map was opened is shown for the new map.
    #[test]
    fn a_write_landing_after_an_eviction_keeps_no_pictures() {
        let (mut state, mut captured, mut images) = (PostcardBake::default(), CapturedPostcards::default(), Assets::<Image>::default());
        state.openings = 1;
        state.landed_write(&mut captured, &mut images, (7, 0), Ok(()), kept_set());
        assert!(captured.get("top").is_none(), "the old map's picture came back");
        state.landed_write(&mut captured, &mut images, (7, 1), Ok(()), kept_set());
        assert!(captured.get("top").is_some());
    }

    /// Breaks if a failed write is taken as baked, so auto-bake never tries again.
    #[test]
    fn a_failed_write_is_not_baked() {
        let (mut state, mut captured, mut images) = (PostcardBake::default(), CapturedPostcards::default(), Assets::<Image>::default());
        state.current = Some(7);
        state.landed_write(&mut captured, &mut images, (7, 0), Err("disk full".into()), kept_set());
        assert_eq!(state.baked, None);
        assert_eq!(state.gave_up, Some(7));
        assert_eq!(state.status().state, BakeState::Stale, "auto-bake would retry a failing write every second");
    }

    /// Breaks if a clear asked for during a write leaves the set marked baked.
    #[test]
    fn a_clear_during_a_write_is_honored_when_it_lands() {
        let (mut state, mut captured, mut images) = (PostcardBake::default(), CapturedPostcards::default(), Assets::<Image>::default());
        state.clear_after_write = true;
        state.landed_write(&mut captured, &mut images, (7, 0), Ok(()), kept_set());
        assert_eq!(state.baked, None);
        assert!(!state.clear_after_write);
    }

    #[test]
    fn a_map_s_postcards_are_a_folder_under_its_hash() {
        assert_eq!(folder(0xab), PathBuf::from("cache/postcards/00000000000000ab"));
    }
}
