//! Pictures of props: a studio on a render layer of its own, a camera framed
//! so the prop just fits, and a cache on disk so a prop is photographed once
//! per edit rather than once per run.
//!
//! A prop is looked for here whenever [`PropCache`] bakes it. If its file is
//! no newer than `cache/thumbnail/props/{name}.png`, the picture is read back;
//! otherwise the prop sits for a new one, which is written there. A model
//! with no file of its own — one the server sent — is photographed and kept in
//! memory only, since there is no date to compare a picture against.
//!
//! Sittings are one at a time: every studio is on the same layer, so two up at
//! once would photograph each other's props.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::RenderTarget;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

use crate::common::assets::default_packs;
use crate::common::render_layers;
use crate::prop::baked::PropCache;
use crate::prop::document::prop_path;
use crate::prop::view::PropScene;

/// Square, in pixels.
pub const SIZE: u32 = 256;

pub const CACHE_DIR: &str = "cache/thumbnail";
pub const PROPS_DIR: &str = "props";

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
/// material can come back empty. An empty picture is retried this many times
/// before the prop is given up on.
const ATTEMPTS: u32 = 10;

/// What a camera turned to some rotation needs to see a set of points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Framing {
    pub translation: Vec3,
    /// Half the distance to the nearest point, so nothing is clipped.
    pub near: f32,
}

/// Where a camera turned to `rotation` must stand for every point to be inside
/// a square frame of field of view `fov`, `padding` of the half-width in from
/// each edge, as close as that allows. `None` with no points.
///
/// Exact rather than a bounding sphere: for a fixed direction, keeping a point
/// inside the frame is a pair of linear bounds on the camera's position, so
/// the closest position that satisfies all of them has a closed form.
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
    queue: VecDeque<Request>,
    sitting: Option<Sitting>,
}

impl Thumbnails {
    pub fn prop(&self, name: &str) -> Option<&Handle<Image>> {
        self.props.get(name)
    }

    pub fn busy(&self) -> bool {
        self.sitting.is_some() || !self.queue.is_empty()
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
}

struct Request {
    name: String,
    /// `None` for a model with no file to date the picture against.
    save_to: Option<PathBuf>,
}

struct Sitting {
    request: Request,
    studio: Entity,
    target: Handle<Image>,
    frames: u32,
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
            .add_systems(Update, (look_at_new_bakes, photograph).chain());
    }
}

pub fn cache_path(folder: &str, name: &str) -> PathBuf {
    Path::new(CACHE_DIR).join(folder).join(format!("{name}.png"))
}

/// Of the file the cache would bake: the highest-priority pack that has one.
fn source_modified(name: &str) -> Option<SystemTime> {
    default_packs()
        .iter()
        .rev()
        .find_map(|pack| std::fs::metadata(prop_path(pack, name)).ok())
        .and_then(|metadata| metadata.modified().ok())
}

/// A picture taken no earlier than the file it is of.
fn read_cached(path: &Path, source: SystemTime) -> Option<Image> {
    let taken = std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok()?;
    if taken < source {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
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
            warn!("Retaking thumbnail {}: {e}", path.display());
            None
        }
    }
}

fn look_at_new_bakes(
    cache: Res<PropCache>,
    mut thumbnails: ResMut<Thumbnails>,
    mut images: ResMut<Assets<Image>>,
    mut seen: Local<u64>,
) {
    if cache.bakes() == *seen {
        return;
    }
    *seen = cache.bakes();

    for (name, baked) in cache.iter() {
        if thumbnails.looked_at.get(name) == Some(&baked.bake) {
            continue;
        }
        thumbnails.looked_at.insert(name.to_owned(), baked.bake);

        let source = source_modified(name);
        let save_to = source.map(|_| cache_path(PROPS_DIR, name));
        if let (Some(source), Some(path)) = (source, &save_to)
            && let Some(image) = read_cached(path, source)
        {
            thumbnails.keep(name, image, &mut images);
            continue;
        }
        // A save while the old picture is still queued supersedes it.
        thumbnails.queue.retain(|request| request.name != name);
        thumbnails.queue.push_back(Request { name: name.to_owned(), save_to });
    }
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
        if sitting.frames == SETTLE_FRAMES {
            commands.spawn(Screenshot::image(sitting.target.clone())).observe(develop);
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
                // Or the prop editor hides it with the rest of the world, and
                // every picture taken in that mode is empty.
                PropScene,
                Transform::IDENTITY,
                Visibility::Inherited,
                Name::new(format!("Thumbnail studio: {}", request.name)),
            ))
            .with_children(|studio| {
                studio.spawn((
                    Camera3d::default(),
                    Camera {
                        // Before the window's cameras, though nothing depends on it.
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

        thumbnails.sitting = Some(Sitting { request, studio, target, frames: 0, attempts: 0 });
        return;
    }
}

fn develop(
    captured: On<ScreenshotCaptured>,
    mut commands: Commands,
    mut thumbnails: ResMut<Thumbnails>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut sitting) = thumbnails.sitting.take() else { return };

    let picture = match captured.image.clone().try_into_dynamic() {
        Ok(picture) => picture.to_rgba8(),
        Err(e) => {
            error!("Thumbnail of {} came back unreadable: {e}", sitting.request.name);
            finish(&mut commands, &mut images, sitting);
            return;
        }
    };

    // Every pixel the clear colour: the pipelines were not ready yet.
    let first = picture.pixels().next().copied();
    if picture.pixels().all(|pixel| Some(*pixel) == first) {
        sitting.attempts += 1;
        if sitting.attempts < ATTEMPTS {
            sitting.frames = 0;
            thumbnails.sitting = Some(sitting);
        } else {
            warn!("Gave up on a thumbnail of {}: every picture was empty", sitting.request.name);
            finish(&mut commands, &mut images, sitting);
        }
        return;
    }

    if let Some(path) = &sitting.request.save_to {
        let saved = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|e| e.to_string())
            .and_then(|()| picture.save(path).map_err(|e| e.to_string()));
        match saved {
            Ok(()) => info!("Thumbnail saved to {}", path.display()),
            Err(e) => warn!("Could not save thumbnail {}: {e}", path.display()),
        }
    }

    let image = Image::from_dynamic(picture.into(), true, RenderAssetUsages::default());
    thumbnails.keep(&sitting.request.name, image, &mut images);
    finish(&mut commands, &mut images, sitting);
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
