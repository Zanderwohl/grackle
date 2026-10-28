//! The second bake: sunlight through sky surfaces, into one lightmap per room. See
//! documentation/sun.md.
//!
//! Light anywhere can depend on a room anywhere — open a skylight and the corridor beyond it
//! brightens — so this is never incremental: any change to the faces, what is sky, or the sun
//! bakes the whole map, once the map has been still for [`SETTLE_SECONDS`].

use std::hash::{Hash, Hasher};

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::pbr::Lightmap;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::common::app_mode::AppMode;
use crate::common::lightmap::Atlas;
use crate::common::palette::Palette;
use crate::common::sky::{SunPosition, SKYBOX};
use crate::common::sunlight::LitSurface;
use crate::common::team::Side;
use crate::common::world_surface::Face;
use crate::tool::bakes::{BakePlugin, BakeSystems, BakedRoomGeometry, RoomBake, RoomSlotMesh};
use crate::tool::room::{RoomSlot, RoomSurfaces};

/// How long the map has to be left alone before auto-bake bakes it, so a drag is one bake.
const SETTLE_SECONDS: f32 = 0.5;

#[derive(Resource)]
pub struct SunBake {
    auto: bool,
    /// What the lightmaps standing now were baked from.
    baked: Option<u64>,
    /// A change not yet baked, and when it was first seen.
    pending: Option<(u64, f32)>,
}

impl Default for SunBake {
    fn default() -> Self {
        Self { auto: true, baked: None, pending: None }
    }
}

impl SunBake {
    pub fn auto(&self) -> bool {
        self.auto
    }

    pub fn set_auto(&mut self, auto: bool) {
        self.auto = auto;
    }
}

/// A request to bake now, whatever auto-bake is doing.
#[derive(Message)]
pub struct BakeSunlight;

/// A room's lightmap, on its [`BakedRoomGeometry`]. Taken off by the room bake when it rebuilds
/// the room, whose charts moved.
#[derive(Component)]
pub struct RoomLightmap(pub Handle<Image>);

struct Inputs {
    rooms: Vec<(Entity, Vec<Face>, Vec<bool>)>,
    toward: Vec3,
    hash: u64,
    /// Some room has no lightmap, whatever the hash says: it was rebuilt, or is new.
    missing: bool,
}

fn gather(
    bakes: &Query<(Entity, &BakedRoomGeometry, Has<RoomLightmap>)>,
    paint: &Query<(Option<&RoomSurfaces>, Option<&Side>)>,
    palette: &Palette,
    sun: &SunPosition,
) -> Inputs {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let toward = sun.toward();
    toward.to_array().map(f32::to_bits).hash(&mut hasher);
    let mut missing = false;
    let mut rooms: Vec<(Entity, Vec<Face>, Vec<bool>)> = bakes
        .iter()
        .map(|(entity, baked, lit)| {
            missing |= !lit;
            let (surfaces, side) = paint.get(baked.room).unwrap_or((None, None));
            let sky = baked
                .faces
                .iter()
                .map(|face| {
                    let slot = RoomSlot::of(face.plane.normal);
                    surfaces.and_then(|surfaces| palette.paint(surfaces, slot, side.copied())) == Some(SKYBOX)
                })
                .collect();
            (entity, baked.faces.clone(), sky)
        })
        .collect();
    // By where the faces are, not by entity, which every rebuild renumbers.
    rooms.sort_by(|a, b| first_vertex(&a.1).cmp(&first_vertex(&b.1)));
    for (_, faces, sky) in &rooms {
        sky.hash(&mut hasher);
        for face in faces {
            for vertex in &face.vertices {
                vertex.to_array().map(f32::to_bits).hash(&mut hasher);
            }
        }
    }
    Inputs { rooms, toward, hash: hasher.finish(), missing }
}

fn first_vertex(faces: &[Face]) -> [u32; 3] {
    faces.first().and_then(|face| face.vertices.first()).map_or([0; 3], |v| v.to_array().map(f32::to_bits))
}

fn bake(inputs: Inputs, commands: &mut Commands, images: &mut Assets<Image>) {
    let started = Instant::now();
    let faces: Vec<Face> = inputs.rooms.iter().flat_map(|(_, faces, _)| faces.iter().cloned()).collect();
    let sky: Vec<bool> = inputs.rooms.iter().flat_map(|(_, _, sky)| sky.iter().copied()).collect();
    let surface = LitSurface::from_faces(faces, sky);
    let mut texels = 0;
    for (entity, faces, sky) in &inputs.rooms {
        let atlas = Atlas::of(faces);
        let light = atlas.bake(faces, sky, &surface, inputs.toward);
        texels += light.len();
        let image = images.add(lightmap_image(atlas.size, &light));
        commands.entity(*entity).try_insert(RoomLightmap(image));
    }
    info!(
        "Baked sunlight for {} room(s), {texels} texels, in {:.0} ms",
        inputs.rooms.len(),
        started.elapsed().as_secs_f32() * 1000.0
    );
}

/// Half floats: light runs well past 1, and a 32-bit float texture is not filterable everywhere.
fn lightmap_image(size: UVec2, light: &[Vec3]) -> Image {
    let data = light
        .iter()
        .flat_map(|texel| [texel.x, texel.y, texel.z, 1.0])
        .flat_map(|channel| half::f16::from_f32(channel).to_le_bytes())
        .collect();
    let mut image = Image::new(
        Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    image
}

#[allow(clippy::too_many_arguments)]
fn auto_bake_sunlight(
    mut requests: MessageReader<BakeSunlight>,
    mut state: ResMut<SunBake>,
    rooms: Res<RoomBake>,
    time: Res<Time<Real>>,
    bakes: Query<(Entity, &BakedRoomGeometry, Has<RoomLightmap>)>,
    paint: Query<(Option<&RoomSurfaces>, Option<&Side>)>,
    palette: Res<Palette>,
    sun: Res<SunPosition>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
) {
    let asked = requests.read().count() > 0;
    // Half a map's rooms rebuilt is a map with holes in it, and light would pour through them.
    if !rooms.is_settled() || !(asked || state.auto) {
        return;
    }
    let inputs = gather(&bakes, &paint, &palette, &sun);
    if !asked {
        if state.baked == Some(inputs.hash) && !inputs.missing {
            state.pending = None;
            return;
        }
        let now = time.elapsed_secs();
        match state.pending {
            Some((hash, since)) if hash == inputs.hash => {
                if now - since < SETTLE_SECONDS {
                    return;
                }
            }
            _ => {
                state.pending = Some((inputs.hash, now));
                return;
            }
        }
    }
    state.baked = Some(inputs.hash);
    state.pending = None;
    bake(inputs, &mut commands, &mut images);
}

/// A round is never played in stale light. The rooms were all rebuilt just before this, so every
/// lightmap is missing.
fn bake_sunlight_for_the_round(
    mut state: ResMut<SunBake>,
    bakes: Query<(Entity, &BakedRoomGeometry, Has<RoomLightmap>)>,
    paint: Query<(Option<&RoomSurfaces>, Option<&Side>)>,
    palette: Res<Palette>,
    sun: Res<SunPosition>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
) {
    let inputs = gather(&bakes, &paint, &palette, &sun);
    state.baked = Some(inputs.hash);
    state.pending = None;
    bake(inputs, &mut commands, &mut images);
}

/// Every frame, as `paint_rooms` is: a slot takes its room's lightmap while it is drawn with a
/// `StandardMaterial`, which is the only material that reads one. A sky slot never has one.
///
/// Two things about Bevy 0.20 shape this. Adding or removing a `Lightmap` does not re-specialize
/// the mesh's pipeline, so the bind group and pipeline disagree and wgpu stops the frame; marking
/// the material changed is what does. And a lightmap's image is only read when the component
/// first arrives, so a new image is a removal one frame and an insertion the next.
fn attach_lightmaps(
    mut commands: Commands,
    mut slots: Query<(Entity, &ChildOf, Option<&mut MeshMaterial3d<StandardMaterial>>, Option<&Lightmap>), With<RoomSlotMesh>>,
    rooms: Query<&RoomLightmap>,
) {
    for (entity, parent, material, drawn) in &mut slots {
        let Some(mut material) = material else {
            if drawn.is_some() {
                commands.entity(entity).remove::<Lightmap>();
            }
            continue;
        };
        let wanted = rooms.get(parent.parent()).ok().map(|lightmap| &lightmap.0);
        match (wanted, drawn) {
            (Some(image), Some(drawn)) if drawn.image == *image => continue,
            (None, None) => continue,
            (Some(image), None) => {
                commands.entity(entity).insert(Lightmap {
                    image: image.clone(),
                    uv_rect: Rect::new(0.0, 0.0, 1.0, 1.0),
                    bicubic_sampling: false,
                });
            }
            (_, Some(_)) => {
                commands.entity(entity).remove::<Lightmap>();
            }
        }
        material.set_changed();
    }
}

pub struct SunBakePlugin;

impl Plugin for SunBakePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SunBake>()
            .add_message::<BakeSunlight>()
            // Ungated, as the room bake is: the map can change during a round.
            .add_systems(Update, (auto_bake_sunlight, attach_lightmaps).chain().after(BakePlugin::auto_bake_rooms))
            .add_systems(
                OnEnter(AppMode::Play),
                bake_sunlight_for_the_round.in_set(BakeSystems::All).after(BakePlugin::bake_everything_for_the_round),
            );
    }
}
