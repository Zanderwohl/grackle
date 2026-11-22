//! The postcard bake: photographs of the map, 3:2 at [`HEIGHT`] pixels high whatever the window's
//! size, with no HUD, halved once to the size a postcard prints its picture at ([`KEPT_HEIGHT`]),
//! which is all that is stored. One is taken through each spectator camera at its own vertical field of view,
//! and one straight down, orthographic, turned in [`ANGLE_STEP_DEGREES`] steps to whichever angle
//! lets the map's rooms and blocks fill the most of the frame.
//!
//! They are written to `cache/postcards/{pack}/{map}/` ([`folder`]), each named by its
//! [`PostcardChoice::file_stem`]: `top.png`, `camera_{order}_{feature}.png`,
//! `camera_{order}_{feature}_copy_{image}.png`, beside a `hash.txt` saying what they are of. The hash is FNV
//! over every feature as it would be saved, the map's settings that a picture can show
//! ([`SavedMetadata::pictured`]), halfbright, which photographs without sunlight, and the folder
//! ([`filed_hash`]). A folder whose hash differs is taken again. Pack content (materials, props)
//! is not in it: after editing one, delete `cache/postcards/`.
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
use std::path::{Path, PathBuf};

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
use crate::editor::panels::CurrentFilePath;
use crate::editor::save::SavedMetadata;
use crate::game::spectate::{self, Cameras};
use crate::get;
use crate::tool::bake_job::{self, BakeKind, BakeProgress, BakeState, BakeStatus, VolumeLabels};
use crate::tool::room::MapVolumes;
use crate::tool::sun_bake::SunBake;

pub const HEIGHT: u32 = 2 * KEPT_HEIGHT;
pub const WIDTH: u32 = HEIGHT * 3 / 2;
const ASPECT: f32 = WIDTH as f32 / HEIGHT as f32;

/// Bump after changing how a postcard is taken, so old ones are a miss.
const FORMAT: u32 = 2;

/// Under [`cache_path::CACHE_ROOT`](crate::common::cache_path::CACHE_ROOT).
pub const CACHE_KIND: &str = "postcards";

/// Beside the pictures in a set's folder: the hash they were taken under, in hex.
const HASH_FILE: &str = "hash.txt";

/// Pixels: a picture's height in the picker.
pub const PREVIEW_HEIGHT: u32 = 200;
const PREVIEW_WIDTH: u32 = PREVIEW_HEIGHT * 3 / 2;

/// The size a postcard prints its picture at, so a kept picture is never resampled again.
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
    /// Where the open map's set goes, from its file.
    folder: PathBuf,
    /// The file `folder` was last worked out for, so it is not worked out every frame.
    filed: Option<PathBuf>,
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
            folder: folder(None),
            filed: None,
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
        folder: &Path,
        saved: Result<(), String>,
        kept: Kept,
    ) {
        let clear = std::mem::take(&mut self.clear_after_write);
        if openings == self.openings {
            captured.keep(hash, kept, images);
        }
        match saved {
            Ok(()) if clear => disk::remove(folder),
            Ok(()) => {
                info!("Saved postcards to {}", folder.display());
                self.baked = Some(hash);
            }
            Err(e) => {
                warn!("Could not save postcards to {}: {e}", folder.display());
                self.gave_up = Some(hash);
            }
        }
    }
}

struct Writing {
    hash: u64,
    folder: PathBuf,
    openings: u64,
    task: Task<(Result<(), String>, Kept)>,
}

/// Halves the set on the compute pool, then writes it on the IO pool.
fn write_and_shrink(hash: u64, folder: PathBuf, pictures: Vec<(String, image::RgbaImage)>) -> Task<(Result<(), String>, Kept)> {
    AsyncComputeTaskPool::get().spawn(async move {
        let kept = halved(&pictures);
        let written = kept.clone();
        let saved = IoTaskPool::get().spawn(async move { disk::store(&folder, hash, &written) }).await;
        (saved, with_previews(kept))
    })
}

struct Shot {
    /// The file's stem, [`PostcardChoice::file_stem`].
    name: String,
    label: String,
    transform: Transform,
    projection: Projection,
    clear: ClearColorConfig,
}

struct Sitting {
    hash: u64,
    folder: PathBuf,
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

/// [`postcard_hash`] with the folder the set is filed in, so a map saved somewhere new is
/// photographed there.
pub fn filed_hash(hash: u64, folder: &Path) -> u64 {
    let mut hasher = Fnv::default();
    hash.hash(&mut hasher);
    folder.hash(&mut hasher);
    hasher.finish()
}

/// `cache/postcards/{pack}/{map}`, for the map saved at `file`.
pub fn folder(file: Option<&Path>) -> PathBuf {
    crate::common::cache_path::document_folder(CACHE_KIND, file)
}

/// A map with no file is filed by its [`postcard_hash`], so two unsaved maps keep a set each.
fn untitled_folder(hash: u64) -> PathBuf {
    folder(None).with_file_name(format!("{}_{hash:016x}", crate::common::cache_path::UNTITLED))
}

/// The picture a Library shows for the map at `file`: its first spectator camera's, else the
/// top-down one; `None` when neither has been taken.
pub fn preview_file(file: &Path) -> Option<PathBuf> {
    preview_in(&folder(Some(file)))
}

fn preview_in(folder: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(folder).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .filter_map(|path| {
            let (choice, order) = PostcardChoice::from_file_stem(path.file_stem()?.to_str()?)?;
            // A built-in sorts after every camera.
            Some((choice.camera_rank(order).is_none(), choice.camera_rank(order), path))
        })
        .min_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)))
        .map(|(_, _, path)| path)
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
            name: PostcardChoice::BuiltIn(BuiltInPostcard::Top).file_stem(0),
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
            name: choice.file_stem(spec.order),
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

    pub fn exists(folder: &Path, hash: u64) -> bool {
        std::fs::read_to_string(folder.join(HASH_FILE)).is_ok_and(|written| written.trim() == format!("{hash:016x}"))
    }

    pub fn remove(folder: &Path) {
        let _ = std::fs::remove_dir_all(folder);
    }

    /// The pictures, by file stem. A picture that will not decode is left out.
    pub fn read(folder: &Path) -> Vec<(String, image::RgbaImage)> {
        let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
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

    /// Into a partial folder renamed into place, so a folder with a hash is a whole set.
    pub fn store(folder: &Path, hash: u64, pictures: &[(String, image::RgbaImage)]) -> Result<(), String> {
        let mut partial = folder.as_os_str().to_owned();
        partial.push(".partial");
        let partial = PathBuf::from(partial);
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
        for (name, picture) in pictures {
            picture.save(partial.join(format!("{name}.png"))).map_err(|e| e.to_string())?;
        }
        std::fs::write(partial.join(HASH_FILE), format!("{hash:016x}")).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(folder);
        std::fs::rename(&partial, folder).map_err(|e| e.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
mod disk {
    use std::path::Path;

    pub fn exists(_: &Path, _: u64) -> bool {
        false
    }

    pub fn remove(_: &Path) {}

    pub fn read(_: &Path) -> Vec<(String, image::RgbaImage)> {
        Vec::new()
    }

    pub fn store(_: &Path, _: u64, _: &[(String, image::RgbaImage)]) -> Result<(), String> {
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
    (role, mut captured, file): (Option<Res<NetRole>>, ResMut<CapturedPostcards>, Option<Res<CurrentFilePath>>),
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
    let content = postcard_hash(state.features.unwrap_or_default(), settings, sun.halfbright());
    match file.as_ref().and_then(|file| file.path.as_ref()) {
        Some(path) if state.filed.as_ref() != Some(path) => {
            state.folder = folder(Some(path));
            state.filed = Some(path.clone());
        }
        Some(_) => {}
        None => {
            let at = untitled_folder(content);
            if state.folder != at {
                state.folder = at;
            }
            state.filed = None;
        }
    }
    let hash = filed_hash(content, &state.folder);
    if state.current != Some(hash) || features_stale {
        state.current = Some(hash);
        state.changed_at = now;
    }

    if let Some(writing) = &mut state.writing
        && let Some((saved, kept)) = check_ready(&mut writing.task)
    {
        let key = (writing.hash, writing.openings);
        let written_to = std::mem::take(&mut writing.folder);
        state.writing = None;
        state.landed_write(&mut captured, &mut images, key, &written_to, saved, kept);
    }

    if clears.read().count() > 0 {
        match &state.writing {
            Some(writing) if writing.hash == hash => state.clear_after_write = true,
            _ => disk::remove(&state.folder),
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
        if disk::exists(&state.folder, hash) {
            state.baked = Some(hash);
            state.asked = false;
            captured.read(hash, state.folder.clone());
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
        folder: state.folder.clone(),
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
        let task = write_and_shrink(sitting.hash, sitting.folder.clone(), sitting.developed);
        state.writing = Some(Writing { hash: sitting.hash, folder: sitting.folder, openings: state.openings, task });
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

/// Each picture by [`PostcardChoice::file_stem`], with a preview at the picker's size.
type Kept = Vec<(String, image::RgbaImage, image::RgbaImage)>;

/// The set of postcards read or taken most recently.
#[derive(Resource, Default)]
pub struct CapturedPostcards {
    hash: Option<u64>,
    /// The previews, by [`PostcardChoice::key`].
    pictures: HashMap<String, Handle<Image>>,
    /// The pictures at [`KEPT_HEIGHT`], by [`PostcardChoice::key`].
    kept: HashMap<String, image::RgbaImage>,
    /// Bumped whenever `pictures` is replaced.
    generation: u64,
    loading: Option<(u64, Task<Kept>)>,
}

impl CapturedPostcards {
    pub fn get(&self, stem: &str) -> Option<&Handle<Image>> {
        self.pictures.get(stem)
    }

    pub fn picture(&self, stem: &str) -> Option<&image::RgbaImage> {
        self.kept.get(stem)
    }

    #[cfg(test)]
    pub fn keep_for_test(&mut self, stem: &str, picture: image::RgbaImage) {
        self.kept.insert(stem.into(), picture);
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
        self.kept.clear();
        self.hash = None;
        self.loading = None;
        self.generation += 1;
    }

    /// The pictures on disk under `hash`, unless it is the one kept or being read: read on the
    /// IO pool, previewed on the compute pool.
    fn read(&mut self, hash: u64, folder: PathBuf) {
        if self.hash == Some(hash) || self.loading.as_ref().is_some_and(|(loading, _)| *loading == hash) {
            return;
        }
        let task = AsyncComputeTaskPool::get().spawn(async move {
            with_previews(IoTaskPool::get().spawn(async move { disk::read(&folder) }).await)
        });
        self.loading = Some((hash, task));
    }

    fn keep(&mut self, hash: u64, kept: Kept, images: &mut Assets<Image>) {
        self.evict(images);
        for (stem, picture, preview) in kept {
            let Some((choice, _)) = PostcardChoice::from_file_stem(&stem) else { continue };
            let image = Image::from_dynamic(image::DynamicImage::ImageRgba8(preview), true, RenderAssetUsages::RENDER_WORLD);
            self.pictures.insert(choice.key(), images.add(image));
            self.kept.insert(choice.key(), picture);
        }
        self.hash = Some(hash);
    }
}

/// Each picture at half its size, every pixel the average of the four it covers.
fn halved(pictures: &[(String, image::RgbaImage)]) -> Vec<(String, image::RgbaImage)> {
    pictures
        .iter()
        .map(|(stem, picture)| {
            let (width, height) = ((picture.width() / 2).max(1), (picture.height() / 2).max(1));
            let half = image::RgbaImage::from_fn(width, height, |x, y| {
                let mut sum = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let pixel = picture.get_pixel((2 * x + dx).min(picture.width() - 1), (2 * y + dy).min(picture.height() - 1));
                    for (total, channel) in sum.iter_mut().zip(pixel.0) {
                        *total += channel as u32;
                    }
                }
                image::Rgba(sum.map(|total| ((total + 2) / 4) as u8))
            });
            (stem.clone(), half)
        })
        .collect()
}

fn with_previews(pictures: Vec<(String, image::RgbaImage)>) -> Kept {
    pictures
        .into_iter()
        .map(|(stem, picture)| {
            let preview = image::imageops::resize(&picture, PREVIEW_WIDTH, PREVIEW_HEIGHT, image::imageops::FilterType::Triangle);
            (stem, picture, preview)
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
    let current = slot.map(|choice| choice.key());
    let plain = get!("editor.metadata.postcard_none");
    let missing = get!("editor.metadata.postcard_missing");
    let Some(pick) = picker_button_or_missing(ui, PICKER, current.as_deref(), Some(&plain), &missing) else { return false };
    let pick = pick.and_then(|key| PostcardChoice::from_key(&key));
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
        let stem = choice.key();
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

/// Each map's [`preview_file`], read again each time the Library opens and freed when it closes.
#[derive(Resource, Default)]
pub struct MapPreviews {
    opened: u64,
    /// By the map's file.
    pictures: HashMap<PathBuf, Handle<Image>>,
    reading: Vec<(PathBuf, Task<Option<image::RgbaImage>>)>,
}

pub fn free_map_previews(mut contexts: EguiContexts, mut images: ResMut<Assets<Image>>, mut previews: ResMut<MapPreviews>) {
    if previews.pictures.is_empty() && previews.reading.is_empty() {
        return;
    }
    for (_, handle) in previews.pictures.drain() {
        contexts.remove_image(handle.id());
        images.remove(&handle);
    }
    previews.reading.clear();
}

pub fn show_map_library(
    mut contexts: EguiContexts,
    mut library: ResMut<crate::common::library::Library>,
    mut images: ResMut<Assets<Image>>,
    mut previews: ResMut<MapPreviews>,
) {
    let MapPreviews { opened, pictures, reading } = &mut *previews;
    if *opened != library.opened {
        *opened = library.opened;
        for (_, handle) in pictures.drain() {
            contexts.remove_image(handle.id());
            images.remove(&handle);
        }
        *reading = library
            .entries()
            .iter()
            .map(|entry| {
                let file = entry.path.clone();
                let task = IoTaskPool::get().spawn(async move { image::open(preview_file(&file)?).ok().map(|picture| picture.to_rgba8()) });
                (entry.path.clone(), task)
            })
            .collect();
    }
    reading.retain_mut(|(file, task)| {
        let Some(read) = check_ready(task) else { return true };
        if let Some(picture) = read {
            let image = Image::from_dynamic(image::DynamicImage::ImageRgba8(picture), true, RenderAssetUsages::RENDER_WORLD);
            pictures.insert(file.clone(), images.add(image));
        }
        false
    });
    let textures: HashMap<PathBuf, egui::TextureId> =
        pictures.iter().map(|(file, handle)| (file.clone(), contexts.add_image(EguiTextureHandle::Weak(handle.id())))).collect();
    let Ok(ctx) = contexts.ctx_mut() else { return };
    library.show(ctx, &mut |entry| textures.get(&entry.path).copied(), &|_| None);
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
        let (here, there) = (Path::new("cache/postcards/user/a"), Path::new("cache/postcards/user/b"));
        assert_ne!(filed_hash(1, here), filed_hash(1, there));
        assert_ne!(untitled_folder(1), untitled_folder(2), "two unsaved maps would share a set");
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
    fn a_choice_reads_back_from_its_key_and_its_file_stem() {
        for choice in [
            PostcardChoice::BuiltIn(BuiltInPostcard::Top),
            PostcardChoice::SpectatorCamera { feature: FeatureId::from_raw(12), copy: None },
            PostcardChoice::SpectatorCamera { feature: FeatureId::from_raw(12), copy: Some(3) },
        ] {
            assert_eq!(PostcardChoice::from_key(&choice.key()), Some(choice));
            let order = if matches!(choice, PostcardChoice::BuiltIn(_)) { 0 } else { 7 };
            assert_eq!(PostcardChoice::from_file_stem(&choice.file_stem(order)), Some((choice, order)));
        }
        let camera = PostcardChoice::SpectatorCamera { feature: FeatureId::from_raw(12), copy: Some(3) };
        assert_eq!(camera.file_stem(7), "camera_7_12_copy_3");
        assert_eq!(PostcardChoice::from_file_stem("camera_x"), None);
        assert_eq!(PostcardChoice::from_file_stem("camera_12"), None);
    }

    /// Fails if the Library's preview is not the first spectator camera, or the top-down picture
    /// without one.
    #[test]
    fn the_preview_is_the_first_camera_in_the_carousel() {
        let at = std::env::temp_dir().join(format!("grackle_preview_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&at);
        std::fs::create_dir_all(&at).unwrap();
        let touch = |name: &str| std::fs::write(at.join(name), b"").unwrap();
        assert_eq!(preview_in(&at), None);
        touch("top.png");
        assert_eq!(preview_in(&at), Some(at.join("top.png")));
        for name in ["camera_10_3.png", "camera_2_40.png", "camera_2_9_copy_0.png", "camera_2_9.png", "hash.txt"] {
            touch(name);
        }
        assert_eq!(preview_in(&at), Some(at.join("camera_2_9.png")));
        std::fs::remove_dir_all(&at).unwrap();
    }

    /// Breaks if a picture of the map that was open survives into the next one.
    #[test]
    fn eviction_drops_every_picture_and_any_read_in_flight() {
        bevy::tasks::IoTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut images = Assets::<Image>::default();
        let mut captured = CapturedPostcards::default();
        let handle = images.add(Image::default());
        captured.pictures.insert("top".into(), handle.clone());
        captured.hash = Some(1);
        captured.read(2, PathBuf::from("unused"));
        let generation = captured.generation;
        captured.evict(&mut images);
        assert!(captured.get("top").is_none());
        assert!(images.get(&handle).is_none(), "the picture's image was kept");
        assert!(captured.hash.is_none() && captured.loading.is_none());
        assert_ne!(captured.generation, generation, "the picker would keep showing the old textures");
    }

    fn kept_set() -> Kept {
        vec![("top".into(), image::RgbaImage::new(6, 4), image::RgbaImage::new(3, 2))]
    }

    /// Breaks if a set written after another map was opened is shown for the new map.
    #[test]
    fn a_write_landing_after_an_eviction_keeps_no_pictures() {
        let (mut state, mut captured, mut images) = (PostcardBake::default(), CapturedPostcards::default(), Assets::<Image>::default());
        state.openings = 1;
        state.landed_write(&mut captured, &mut images, (7, 0), Path::new("unused"), Ok(()), kept_set());
        assert!(captured.get("top").is_none(), "the old map's picture came back");
        state.landed_write(&mut captured, &mut images, (7, 1), Path::new("unused"), Ok(()), kept_set());
        assert!(captured.get("top").is_some());
    }

    /// Breaks if a failed write is taken as baked, so auto-bake never tries again.
    #[test]
    fn a_failed_write_is_not_baked() {
        let (mut state, mut captured, mut images) = (PostcardBake::default(), CapturedPostcards::default(), Assets::<Image>::default());
        state.current = Some(7);
        state.landed_write(&mut captured, &mut images, (7, 0), Path::new("unused"), Err("disk full".into()), kept_set());
        assert_eq!(state.baked, None);
        assert_eq!(state.gave_up, Some(7));
        assert_eq!(state.status().state, BakeState::Stale, "auto-bake would retry a failing write every second");
    }

    /// Breaks if a clear asked for during a write leaves the set marked baked.
    #[test]
    fn a_clear_during_a_write_is_honored_when_it_lands() {
        let (mut state, mut captured, mut images) = (PostcardBake::default(), CapturedPostcards::default(), Assets::<Image>::default());
        state.clear_after_write = true;
        state.landed_write(&mut captured, &mut images, (7, 0), Path::new("unused"), Ok(()), kept_set());
        assert_eq!(state.baked, None);
        assert!(!state.clear_after_write);
    }

    #[test]
    fn a_map_s_postcards_are_a_folder_under_its_pack() {
        assert_eq!(folder(Some(Path::new("assets/default/blueprints/arena.gmb"))), PathBuf::from("cache/postcards/default/arena"));
    }
}
