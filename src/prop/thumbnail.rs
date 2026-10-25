//! Pictures of props: a studio on a render layer of its own, a camera framed
//! so the prop just fits, and a cache on disk so a prop is photographed once
//! per edit rather than once per run.
//!
//! A prop is looked for here whenever [`PropCache`] bakes it. If
//! `cache/thumbnail/props/{name}.png` is a picture of the file it was baked
//! from, taken since that file last changed, it is read back; otherwise the
//! prop sits for a new one, which is written there. A model with no file of
//! its own — one the server sent — is photographed and kept in memory only.
//! Disk work runs on the IO pool, off the frame.
//!
//! Sittings are one at a time: every studio is on the same layer, so two up at
//! once would photograph each other's props.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::RenderTarget;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::futures::check_ready;
use bevy::tasks::{IoTaskPool, Task};

use crate::common::render_layers;
use crate::prop::baked::PropCache;
use crate::prop::document::PROPS_DIR;

/// Square, in pixels.
pub const SIZE: u32 = 256;

pub const CACHE_DIR: &str = "cache/thumbnail";

/// Narrow, so a prop is not distorted by being close, and not orthographic,
/// so it still has depth.
const FOV: f32 = 25.0 * std::f32::consts::PI / 180.0;

/// Of the half-width, in from each edge.
const PADDING: f32 = 0.06;

/// The camera's offset from the prop, before framing decides how far. A
/// weapon points down `-Z`, so this is its right side, barrel to the right,
/// turned a little towards the muzzle and seen from a little above.
const SEEN_FROM: Vec3 = Vec3::new(1.0, 0.45, -0.45);

/// Towards each light, in the camera's frame: `+X` right, `+Y` up, `+Z` back
/// towards the camera.
const KEY_FROM: Vec3 = Vec3::new(-0.7, 0.8, 0.9);
const FILL_FROM: Vec3 = Vec3::new(0.9, -0.1, 0.6);
const BACK_FROM: Vec3 = Vec3::new(0.3, 0.7, -1.0);

const KEY_LUX: f32 = 7_000.0;
const FILL_LUX: f32 = 2_000.0;
const BACK_LUX: f32 = 6_000.0;
const AMBIENT: f32 = 150.0;

/// Frames between standing the studio up and taking the picture: meshes and
/// the target have to reach the render world first.
const SETTLE_FRAMES: u32 = 3;

/// Pipelines compile in the background, so the first pictures of a new
/// material can come back empty: an empty one is retaken this many frames
/// later, up to [`ATTEMPTS`] times — about ten seconds at 60 Hz.
const RETRY_FRAMES: u32 = 15;
const ATTEMPTS: u32 = 40;

/// Frames to wait for a picture before taking it again. A capture can be
/// dropped outright, e.g. while the window is minimised.
const CAPTURE_TIMEOUT: u32 = 120;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Framing {
    pub translation: Vec3,
    /// Half the distance to the nearest point, so nothing is clipped.
    pub near: f32,
}

/// The closest a camera at `rotation` can stand with every point `padding` of
/// the half-width inside a square frame.
///
/// Exact rather than a bounding sphere: for a fixed direction, keeping a point
/// in frame is a pair of linear bounds on the camera's position.
pub fn frame(points: impl IntoIterator<Item = Vec3>, rotation: Quat, fov: f32, padding: f32) -> Option<Framing> {
    let slope = (fov / 2.0).tan() * (1.0 - padding);
    let to_view = rotation.inverse();

    // In view space, with the camera at (c, d), a point (p, z) is in frame
    // when |p - c| <= slope * (d - z) on each axis: c >= p + slope*z - slope*d
    // and c <= p - slope*z + slope*d.
    let mut above = Vec2::NEG_INFINITY;
    let mut below = Vec2::INFINITY;
    let mut nearest = f32::NEG_INFINITY;
    for point in points {
        let v = to_view * point;
        above = above.max(v.xy() + slope * v.z);
        below = below.min(v.xy() - slope * v.z);
        nearest = nearest.max(v.z);
    }
    if !nearest.is_finite() {
        return None;
    }

    let centre = (above + below) / 2.0;
    let fitted = ((above - below) / (2.0 * slope)).max_element();
    // A point dead centre is in frame at any distance, including zero.
    let clearance = ((above - below).max_element() * 0.05).max(1e-3);
    let distance = fitted.max(nearest + clearance);

    Some(Framing {
        translation: rotation * centre.extend(distance),
        near: (distance - nearest) / 2.0,
    })
}

/// The prop's pictures, by name. Handles are kept across a re-take, so
/// whatever is drawing one shows the new picture without asking again.
#[derive(Resource, Default)]
pub struct Thumbnails {
    props: HashMap<String, Handle<Image>>,
    /// The [`BakedProp::bake`](crate::prop::baked::BakedProp::bake) each
    /// name was last looked at for.
    looked_at: HashMap<String, u64>,
    /// Cached pictures being read off the frame, with the bake they are for.
    reading: Vec<(Request, u64, Task<Option<Image>>)>,
    queue: VecDeque<Request>,
    sitting: Option<Sitting>,
}

impl Thumbnails {
    pub fn prop(&self, name: &str) -> Option<&Handle<Image>> {
        self.props.get(name)
    }

    fn keep(&mut self, name: &str, image: Image, images: &mut Assets<Image>) {
        match self.props.get(name) {
            Some(handle) => {
                let _ = images.insert(handle.id(), image);
            }
            None => {
                self.props.insert(name.to_owned(), images.add(image));
            }
        }
    }

    fn sit_for(&mut self, request: Request) {
        // A save while the old picture is still queued supersedes it.
        self.queue.retain(|queued| queued.name != request.name);
        self.queue.push_back(request);
    }
}

#[derive(Clone)]
struct Request {
    name: String,
    /// `None` for a model with no file to date the picture against.
    save_to: Option<CacheEntry>,
}

#[derive(Clone)]
struct CacheEntry {
    picture: PathBuf,
    /// The prop's file, which the picture must be no older than.
    source: PathBuf,
}

struct Sitting {
    request: Request,
    studio: Entity,
    target: Handle<Image>,
    frames: u32,
    shoot_at: u32,
    /// The screenshot in flight, so a late answer to an abandoned one is not
    /// taken for this.
    shot: Option<Entity>,
    attempts: u32,
}

#[derive(Component)]
struct ThumbnailStudio;

/// Added beside [`WorldPropPlugin`](crate::game::world_prop::WorldPropPlugin)
/// rather than inside a plugin the tests build, because it renders.
pub struct ThumbnailPlugin;

impl Plugin for ThumbnailPlugin {
    fn build(&self, app: &mut App) {
        // Ungated: a prop baked mid-round is as much in need of a picture.
        app.init_resource::<Thumbnails>()
            .add_systems(Update, (look_at_new_bakes, finish_reading, photograph).chain());
    }
}

pub fn cache_path(folder: &str, name: &str) -> PathBuf {
    Path::new(CACHE_DIR).join(folder).join(format!("{name}.png"))
}

/// The cache is native-only: a browser tab has nowhere to keep it, so there
/// every picture is taken fresh.
#[cfg(not(target_arch = "wasm32"))]
mod disk {
    use super::*;
    use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};

    /// Beside the picture: the file it is of, so a picture of an override that
    /// has since been deleted is not taken for the file now winning.
    fn source_note(picture: &Path) -> PathBuf {
        picture.with_extension("source")
    }

    /// A picture of this very file, taken no earlier than its last change.
    pub fn read(entry: &CacheEntry) -> Option<Image> {
        let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let of = std::fs::read_to_string(source_note(&entry.picture)).ok()?;
        if Path::new(&of) != entry.source || modified(&entry.picture)? < modified(&entry.source)? {
            return None;
        }
        let bytes = std::fs::read(&entry.picture).ok()?;
        match Image::from_buffer(
            &bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::Default,
            RenderAssetUsages::default(),
        ) {
            Ok(image) => Some(image),
            // Half-written by a run that was killed: take it again.
            Err(e) => {
                warn!("Retaking thumbnail {}: {e}", entry.picture.display());
                None
            }
        }
    }

    /// The note goes last, so a picture interrupted halfway is not vouched for.
    pub fn write(entry: &CacheEntry, save: impl FnOnce(&Path) -> Result<(), String>) {
        let _ = std::fs::remove_file(source_note(&entry.picture));
        let saved = entry
            .picture
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|e| e.to_string())
            .and_then(|()| save(&entry.picture))
            .and_then(|()| {
                std::fs::write(source_note(&entry.picture), entry.source.to_string_lossy().as_bytes())
                    .map_err(|e| e.to_string())
            });
        match saved {
            Ok(()) => info!("Thumbnail saved to {}", entry.picture.display()),
            Err(e) => warn!("Could not save thumbnail {}: {e}", entry.picture.display()),
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod disk {
    use super::*;

    pub fn read(_: &CacheEntry) -> Option<Image> {
        None
    }

    pub fn write(_: &CacheEntry, _: impl FnOnce(&Path) -> Result<(), String>) {}
}

fn look_at_new_bakes(cache: Res<PropCache>, mut thumbnails: ResMut<Thumbnails>, mut seen: Local<u64>) {
    if cache.bakes() == *seen {
        return;
    }
    *seen = cache.bakes();

    for (name, baked) in cache.iter() {
        if thumbnails.looked_at.get(name) == Some(&baked.bake) {
            continue;
        }
        thumbnails.looked_at.insert(name.to_owned(), baked.bake);

        let save_to = baked.file.as_ref().map(|source| CacheEntry {
            picture: cache_path(PROPS_DIR, name),
            source: source.clone(),
        });
        let request = Request { name: name.to_owned(), save_to };
        match &request.save_to {
            Some(entry) => {
                let entry = entry.clone();
                let task = IoTaskPool::get().spawn(async move { disk::read(&entry) });
                thumbnails.reading.push((request, baked.bake, task));
            }
            None => thumbnails.sit_for(request),
        }
    }
}

fn finish_reading(mut thumbnails: ResMut<Thumbnails>, mut images: ResMut<Assets<Image>>) {
    let mut reading = std::mem::take(&mut thumbnails.reading);
    reading.retain_mut(|(request, bake, task)| {
        let Some(read) = check_ready(task) else { return true };
        // Re-baked while it was being read: that bake has its own read.
        if thumbnails.looked_at.get(&request.name) == Some(bake) {
            match read {
                Some(image) => thumbnails.keep(&request.name, image, &mut images),
                None => thumbnails.sit_for(request.clone()),
            }
        }
        false
    });
    thumbnails.reading.append(&mut reading);
}

fn photograph(
    mut commands: Commands,
    mut thumbnails: ResMut<Thumbnails>,
    cache: Res<PropCache>,
    meshes: Res<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    if let Some(sitting) = &mut thumbnails.sitting {
        sitting.frames += 1;
        if sitting.frames == sitting.shoot_at {
            let shot = commands.spawn(Screenshot::image(sitting.target.clone())).observe(develop).id();
            sitting.shot = Some(shot);
        } else if sitting.shot.is_some() && sitting.frames > sitting.shoot_at + CAPTURE_TIMEOUT {
            sitting.shot = None;
            if !retake(sitting) {
                let sitting = thumbnails.sitting.take().unwrap();
                warn!("Gave up on a thumbnail of {}: no picture came back", sitting.request.name);
                give_up(&mut commands, &mut thumbnails, &mut images, sitting);
            }
        }
        return;
    }

    while let Some(request) = thumbnails.queue.pop_front() {
        // Replaced by a failure since it was queued.
        let Some(baked) = cache.get(&request.name) else { continue };

        let rotation = Transform::default().looking_to(-SEEN_FROM, Vec3::Y).rotation;
        let points = baked
            .parts
            .iter()
            .filter_map(|(mesh, _)| meshes.get(mesh))
            .filter_map(|mesh| mesh.attribute(Mesh::ATTRIBUTE_POSITION)?.as_float3())
            .flatten()
            .map(|&p| Vec3::from(p));
        let Some(framing) = frame(points, rotation, FOV, PADDING) else {
            warn!("No thumbnail for prop {}: it has no geometry", request.name);
            continue;
        };

        let target = images.add(Image::new_target_texture(
            SIZE,
            SIZE,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
        let layer = RenderLayers::layer(render_layers::THUMBNAIL);
        let studio = commands
            .spawn((
                ThumbnailStudio,
                Transform::IDENTITY,
                Visibility::Inherited,
                // On the root too, which is what tells the prop editor's
                // hiding rule that no viewport draws it.
                layer.clone(),
                Name::new(format!("Thumbnail studio: {}", request.name)),
            ))
            .with_children(|studio| {
                studio.spawn((
                    Camera3d::default(),
                    Camera {
                        order: -1,
                        clear_color: ClearColorConfig::Custom(Color::NONE),
                        ..default()
                    },
                    RenderTarget::Image(target.clone().into()),
                    Projection::Perspective(PerspectiveProjection {
                        fov: FOV,
                        near: framing.near,
                        ..default()
                    }),
                    AmbientLight { brightness: AMBIENT, ..default() },
                    Transform::from_translation(framing.translation).with_rotation(rotation),
                    layer.clone(),
                ));
                for (from, lux) in [(KEY_FROM, KEY_LUX), (FILL_FROM, FILL_LUX), (BACK_FROM, BACK_LUX)] {
                    studio.spawn((
                        DirectionalLight { illuminance: lux, shadow_maps_enabled: false, ..default() },
                        Transform::default().looking_to(-(rotation * from), Vec3::Y),
                        layer.clone(),
                    ));
                }
                for (mesh, material) in &baked.parts {
                    studio.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), layer.clone()));
                }
            })
            .id();

        thumbnails.sitting = Some(Sitting {
            request,
            studio,
            target,
            frames: 0,
            shoot_at: SETTLE_FRAMES,
            shot: None,
            attempts: 0,
        });
        return;
    }
}

/// Schedules another picture; `false` once out of attempts.
fn retake(sitting: &mut Sitting) -> bool {
    sitting.attempts += 1;
    sitting.shoot_at = sitting.frames + RETRY_FRAMES;
    sitting.attempts < ATTEMPTS
}

fn develop(
    captured: On<ScreenshotCaptured>,
    mut commands: Commands,
    mut thumbnails: ResMut<Thumbnails>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(sitting) = &mut thumbnails.sitting else { return };
    if sitting.shot != Some(captured.entity) {
        return;
    }
    sitting.shot = None;

    let picture = match captured.image.clone().try_into_dynamic() {
        Ok(picture) => picture.to_rgba8(),
        Err(e) => {
            let sitting = thumbnails.sitting.take().unwrap();
            error!("Thumbnail of {} came back unreadable: {e}", sitting.request.name);
            give_up(&mut commands, &mut thumbnails, &mut images, sitting);
            return;
        }
    };

    // Every pixel the clear colour: the pipelines were not ready yet.
    let first = picture.pixels().next().copied();
    if picture.pixels().all(|pixel| Some(*pixel) == first) {
        if !retake(sitting) {
            let sitting = thumbnails.sitting.take().unwrap();
            warn!("Gave up on a thumbnail of {}: every picture was empty", sitting.request.name);
            give_up(&mut commands, &mut thumbnails, &mut images, sitting);
        }
        return;
    }

    let sitting = thumbnails.sitting.take().unwrap();
    let image = Image::from_dynamic(picture.clone().into(), true, RenderAssetUsages::default());
    thumbnails.keep(&sitting.request.name, image, &mut images);
    if let Some(entry) = sitting.request.save_to.clone() {
        IoTaskPool::get()
            .spawn(async move { disk::write(&entry, |path| picture.save(path).map_err(|e| e.to_string())) })
            .detach();
    }
    finish(&mut commands, &mut images, sitting);
}

/// Forgets the bake too, so the next bake of anything tries this one again
/// rather than it going without a picture for the rest of the process.
fn give_up(commands: &mut Commands, thumbnails: &mut Thumbnails, images: &mut Assets<Image>, sitting: Sitting) {
    thumbnails.looked_at.remove(&sitting.request.name);
    finish(commands, images, sitting);
}

fn finish(commands: &mut Commands, images: &mut Assets<Image>, sitting: Sitting) {
    commands.entity(sitting.studio).despawn();
    images.remove(&sitting.target);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where each point lands in a square frame, as fractions of the
    /// half-width, and how far in front of the camera it is.
    fn projected(points: &[Vec3], rotation: Quat, framing: Framing) -> Vec<(Vec2, f32)> {
        let slope = (FOV / 2.0).tan();
        points
            .iter()
            .map(|&p| {
                let v = rotation.inverse() * (p - framing.translation);
                (v.xy() / (-v.z * slope), -v.z)
            })
            .collect()
    }

    fn rifle() -> Vec<Vec3> {
        // A long thin box along -Z with a stock hanging below, off centre.
        let mut points = Vec::new();
        for x in [-0.02, 0.03] {
            for y in [-0.05, 0.04] {
                for z in [-0.9, 0.25] {
                    points.push(Vec3::new(x, y, z));
                }
            }
        }
        points.push(Vec3::new(0.0, -0.22, 0.2));
        points
    }

    /// Breaks if any part of a prop is cut off by the frame or the near plane.
    #[test]
    fn everything_is_in_frame_and_in_front_of_the_camera() {
        let points = rifle();
        for from in [SEEN_FROM, Vec3::X, Vec3::new(-0.3, 0.9, 0.2), Vec3::new(0.1, -0.4, 1.0)] {
            let rotation = Transform::default().looking_to(-from, Vec3::Y).rotation;
            let framing = frame(points.iter().copied(), rotation, FOV, PADDING).unwrap();
            for (at, depth) in projected(&points, rotation, framing) {
                assert!(depth > framing.near, "a point at depth {depth} is behind the near plane");
                assert!(
                    at.abs().max_element() <= 1.0 - PADDING + 1e-4,
                    "a point lands at {at} from {from}, outside the padding",
                );
            }
        }
    }

    /// Breaks if the framing leaves more than the padding on every side —
    /// a prop drawn small in its own picture.
    #[test]
    fn the_prop_fills_the_frame() {
        let points = rifle();
        let rotation = Transform::default().looking_to(-SEEN_FROM, Vec3::Y).rotation;
        let framing = frame(points.iter().copied(), rotation, FOV, PADDING).unwrap();
        let at = projected(&points, rotation, framing);

        let reach = |axis: fn(Vec2) -> f32| {
            let (lo, hi) = at.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (p, _)| {
                (lo.min(axis(*p)), hi.max(axis(*p)))
            });
            (lo, hi)
        };
        let (left, right) = reach(|p| p.x);
        let (bottom, top) = reach(|p| p.y);
        let edge = 1.0 - PADDING;
        // Both ends of the long axis.
        let tight = (left + edge).abs() < 1e-3 && (right - edge).abs() < 1e-3
            || (bottom + edge).abs() < 1e-3 && (top - edge).abs() < 1e-3;
        assert!(tight, "nothing touches the padding: x {left}..{right}, y {bottom}..{top}");
    }

    #[test]
    fn nothing_to_frame_is_no_framing() {
        assert_eq!(frame([], Quat::IDENTITY, FOV, PADDING), None);
    }

    /// A single point is in frame at any distance, so only the near plane
    /// stops the camera standing on it.
    #[test]
    fn a_point_is_not_inside_the_camera() {
        let framing = frame([Vec3::new(1.0, 2.0, 3.0)], Quat::IDENTITY, FOV, PADDING).unwrap();
        let depth = (Vec3::new(1.0, 2.0, 3.0) - framing.translation).length();
        assert!(depth > framing.near && framing.near > 0.0);
    }

    #[test]
    fn the_cache_is_one_file_per_prop_under_its_kind() {
        assert_eq!(
            cache_path(PROPS_DIR, "rocket_launcher"),
            PathBuf::from("cache/thumbnail/props/rocket_launcher.png"),
        );
    }
}
