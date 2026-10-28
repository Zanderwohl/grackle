//! The second bake: sunlight through sky surfaces, into one lightmap per room. See
//! documentation/sun.md.
//!
//! Light anywhere can depend on a room anywhere — open a skylight and the corridor beyond it
//! brightens — so this is never incremental: any change to the faces, what is sky, the sun or
//! the sky's time bakes the whole map, once the map has been still for [`SETTLE_SECONDS`].

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::pbr::Lightmap;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::common::app_mode::AppMode;
use crate::common::lightmap::{daylight, Atlas};
use crate::common::palette::Palette;
use crate::common::sky::{SkyTime, SunPosition, SKYBOX};
use crate::common::sunlight::{Daylight, LitSurface};
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
    /// The last bake's lightmaps by what each room was, so a room rebuilt as it was — every room,
    /// on entering Play — takes its light back rather than baking it again.
    lightmaps: HashMap<u64, Handle<Image>>,
}

impl Default for SunBake {
    fn default() -> Self {
        Self { auto: true, baked: None, pending: None, lightmaps: HashMap::new() }
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
    /// Each room's bake, faces, which are sky, and a hash of the last two.
    rooms: Vec<(Entity, Vec<Face>, Vec<bool>, u64)>,
    light: Daylight,
    hash: u64,
    /// Some room has no lightmap, whatever the hash says: it was rebuilt, or is new.
    missing: bool,
}

fn gather(
    bakes: &Query<(Entity, &BakedRoomGeometry, Has<RoomLightmap>)>,
    paint: &Query<(Option<&RoomSurfaces>, Option<&Side>)>,
    palette: &Palette,
    sun: &SunPosition,
    sky: SkyTime,
) -> Inputs {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let toward = sun.toward();
    toward.to_array().map(f32::to_bits).hash(&mut hasher);
    sky.hash(&mut hasher);
    let mut missing = false;
    let mut rooms: Vec<(Entity, Vec<Face>, Vec<bool>, u64)> = bakes
        .iter()
        .map(|(entity, baked, lit)| {
            missing |= !lit;
            let (surfaces, side) = paint.get(baked.room).unwrap_or((None, None));
            let sky: Vec<bool> = baked
                .faces
                .iter()
                .map(|face| {
                    let slot = RoomSlot::of(face.plane.normal);
                    surfaces.and_then(|surfaces| palette.paint(surfaces, slot, side.copied())) == Some(SKYBOX)
                })
                .collect();
            let mut room = std::collections::hash_map::DefaultHasher::new();
            sky.hash(&mut room);
            for face in &baked.faces {
                for vertex in &face.vertices {
                    vertex.to_array().map(f32::to_bits).hash(&mut room);
                }
            }
            (entity, baked.faces.clone(), sky, room.finish())
        })
        .collect();
    // By what the rooms are, not by entity, which every rebuild renumbers.
    rooms.sort_by_key(|room| room.3);
    for room in &rooms {
        room.3.hash(&mut hasher);
    }
    Inputs { rooms, light: daylight(toward, sky), hash: hasher.finish(), missing }
}

/// Puts the last bake's light back on rooms that are what they were then. Only sound when nothing
/// else changed either, which the caller checks against the whole map's hash.
fn reuse(state: &SunBake, inputs: &Inputs, commands: &mut Commands) -> bool {
    let found: Option<Vec<_>> =
        inputs.rooms.iter().map(|(entity, _, _, room)| Some((*entity, state.lightmaps.get(room)?.clone()))).collect();
    let Some(found) = found else { return false };
    for (entity, image) in found {
        commands.entity(entity).try_insert(RoomLightmap(image));
    }
    true
}

fn bake(state: &mut SunBake, inputs: Inputs, commands: &mut Commands, images: &mut Assets<Image>) {
    let started = Instant::now();
    state.baked = Some(inputs.hash);
    state.pending = None;
    state.lightmaps.clear();
    let faces: Vec<Face> = inputs.rooms.iter().flat_map(|(_, faces, _, _)| faces.iter().cloned()).collect();
    let sky: Vec<bool> = inputs.rooms.iter().flat_map(|(_, _, sky, _)| sky.iter().copied()).collect();
    let surface = LitSurface::from_faces(faces, sky);
    let mut texels = 0;
    for (entity, faces, sky, room) in &inputs.rooms {
        let atlas = Atlas::of(faces);
        let light = atlas.bake(faces, sky, &surface, &inputs.light);
        texels += light.len();
        let image = images.add(lightmap_image(atlas.size, &light));
        state.lightmaps.insert(*room, image.clone());
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
    sky: Res<SkyTime>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
) {
    let asked = requests.read().count() > 0;
    // Half a map's rooms rebuilt is a map with holes in it, and light would pour through them.
    if !rooms.is_settled() || !(asked || state.auto) {
        return;
    }
    let inputs = gather(&bakes, &paint, &palette, &sun, *sky);
    if !asked {
        if state.baked == Some(inputs.hash) && (!inputs.missing || reuse(&state, &inputs, &mut commands)) {
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
    bake(&mut state, inputs, &mut commands, &mut images);
}

/// A round is never played in stale light. The rooms were all rebuilt just before this, so every
/// lightmap is missing, but a map nobody changed since its last bake takes that bake back.
fn bake_sunlight_for_the_round(
    mut state: ResMut<SunBake>,
    bakes: Query<(Entity, &BakedRoomGeometry, Has<RoomLightmap>)>,
    paint: Query<(Option<&RoomSurfaces>, Option<&Side>)>,
    palette: Res<Palette>,
    sun: Res<SunPosition>,
    sky: Res<SkyTime>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
) {
    let inputs = gather(&bakes, &paint, &palette, &sun, *sky);
    if state.baked != Some(inputs.hash) || !reuse(&state, &inputs, &mut commands) {
        bake(&mut state, inputs, &mut commands, &mut images);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::GamePlugin;
    use crate::tool::room::Room;

    /// Enough to swap into Play, with one room under an open sky.
    fn headless() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::state::app::StatesPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::diagnostic::FrameCountPlugin,
            bevy::input::InputPlugin,
            bevy::time::TimePlugin,
        ));
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Image>();
        app.init_resource::<Palette>().init_resource::<SunPosition>().init_resource::<SkyTime>();
        app.add_plugins((GamePlugin, BakePlugin, SunBakePlugin));
        app.world_mut().resource_mut::<RoomBake>().set_auto(false);
        app.world_mut().resource_mut::<SunBake>().set_auto(false);
        app.world_mut().spawn((
            Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)),
            RoomSurfaces { ceiling: Some(SKYBOX.into()), ..default() },
        ));
        app.update();
        app
    }

    fn enter(app: &mut App, mode: AppMode) {
        app.world_mut().resource_mut::<NextState<AppMode>>().set(mode);
        app.update();
    }

    fn lightmaps(app: &mut App) -> Vec<AssetId<Image>> {
        app.world_mut().query::<&RoomLightmap>().iter(app.world()).map(|lightmap| lightmap.0.id()).collect()
    }

    /// Every room is rebuilt on entering Play; the light is the same, so it is not baked again.
    #[test]
    fn a_second_round_on_an_unchanged_map_reuses_the_light() {
        let mut app = headless();
        enter(&mut app, AppMode::Play);
        let first = lightmaps(&mut app);
        assert_eq!(first.len(), 1, "the round started unlit");

        enter(&mut app, AppMode::Editor);
        enter(&mut app, AppMode::Play);
        assert_eq!(lightmaps(&mut app), first, "an unchanged map was baked again");

        enter(&mut app, AppMode::Editor);
        app.world_mut().resource_mut::<SunPosition>().altitude = 20.0;
        enter(&mut app, AppMode::Play);
        let moved = lightmaps(&mut app);
        assert_eq!(moved.len(), 1);
        assert_ne!(moved, first, "the sun moved and the light did not");
    }
}
