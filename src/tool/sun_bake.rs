//! The second bake: sunlight through sky surfaces, into one lightmap per room. See
//! documentation/sun.md.
//!
//! Light anywhere can depend on a room anywhere — open a skylight and the corridor beyond it
//! brightens — so this is never incremental: any change to the faces, what is sky, the sun or
//! the sky's time bakes the whole map, once the map has been still for [`bake_job::SETTLE_SECONDS`], in the
//! background ([`BackgroundBake`]), a room at a time.
//!
//! **Light is only ever replaced, never dropped.** A room's light is kept by its `Room` entity,
//! which outlives every rebuild of its meshes; a rebuilt room has the light it had carried onto its
//! new layout ([`Atlas::carry_over`]) until a bake returns, and only [`ClearSunlight`] takes it
//! away.
//!
//! Every bake is also written to `cache/lightmap/{hash}.bin` ([`bake_cache`]), keyed by the
//! snapshot, and a map whose snapshot is there takes its light from it instead of baking. After
//! changing how light is baked, bump [`FORMAT`] or delete `cache/lightmap/`.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::pbr::Lightmap;
use std::time::Duration;

use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::common::app_mode::AppMode;
use crate::common::lightmap::{self, daylight, Atlas};
use crate::common::palette::Palette;
use crate::common::sky::{SkyTime, SunPosition, SKYBOX};
use crate::common::stable_hash::Fnv;
use crate::common::sunlight::{self, Daylight, LitSurface};
use crate::common::team::Side;
use crate::common::world_surface::Face;
use crate::get;
use crate::tool::bake_cache;
use crate::tool::bake_job::{self, BackgroundBake, BakeJob, BakeKind, BakeProgress, VolumeLabels};
use crate::tool::bakes::{BakePlugin, BakeSystems, BakedRoomGeometry, RoomBake, RoomSlotMesh};
use crate::tool::room::{Block, Room, RoomSlot, RoomSurfaces};

const FORMAT: u32 = 1;

const CACHE_FOLDER: &str = "lightmap";

#[derive(Resource)]
pub struct SunBake {
    /// The light standing now, by `Room` entity. May since have been carried onto new layouts.
    lit: HashMap<Entity, RoomLight>,
    bake: BackgroundBake<Job>,
    /// The last snapshot looked for on disk, so a miss is not read again every frame.
    looked_up: Option<u64>,
    /// Off in tests, which would otherwise read each other's bakes back.
    use_disk: bool,
}

impl Default for SunBake {
    fn default() -> Self {
        Self { lit: HashMap::new(), bake: BackgroundBake::new(true), looked_up: None, use_disk: true }
    }
}

impl SunBake {
    pub fn auto(&self) -> bool {
        self.bake.auto()
    }

    pub fn set_auto(&mut self, auto: bool) {
        self.bake.set_auto(auto);
    }

    pub fn is_baking(&self) -> bool {
        self.bake.is_baking()
    }

    /// Lands every room of a bake, as one replacement for the whole map's light. A room whose
    /// layout is the one already lit is written into the same image, so its `Lightmap` never
    /// changes: Bevy reads a new handle only by a removal and an insertion, a frame unlit.
    fn land(&mut self, baked: Vec<BakedLight>, images: &mut Assets<Image>) {
        for room in baked {
            let new = lightmap_image(room.atlas.size, &room.light);
            let image = match self.lit.get(&room.room).filter(|old| old.faces_hash == room.faces_hash) {
                Some(old) if images.insert(&old.image, new.clone()).is_ok() => old.image.clone(),
                _ => images.add(new),
            };
            self.lit.insert(
                room.room,
                RoomLight { faces_hash: room.faces_hash, atlas: room.atlas, light: room.light, image },
            );
        }
    }

    /// Writes a finished bake to disk, then lands it.
    fn landed(&mut self, baked: Vec<BakedLight>, images: &mut Assets<Image>) {
        if self.use_disk
            && let Some(hash) = self.bake.baked()
        {
            let rooms = baked.iter().map(|room| CachedRoom { faces_hash: room.faces_hash, light: room.light.clone() });
            bake_cache::write(CACHE_FOLDER, hash, rooms.collect::<Vec<_>>());
        }
        self.land(baked, images);
    }

    /// Lands the light of `snapshot` from disk, if it was written there. Looked for once per hash.
    fn take_from_cache(&mut self, snapshot: &Snapshot, bakes: &Query<(Entity, &BakedRoomGeometry)>, images: &mut Assets<Image>) {
        if !self.use_disk || self.bake.baked() == Some(snapshot.hash) || self.looked_up == Some(snapshot.hash) {
            return;
        }
        self.looked_up = Some(snapshot.hash);
        let Some(cached) = bake_cache::read::<Vec<CachedRoom>>(CACHE_FOLDER, snapshot.hash) else { return };
        let Some(baked) = snapshot.from_cache(cached, bakes) else {
            warn!("Ignoring cached sunlight {:016x}: it does not fit the rooms", snapshot.hash);
            return;
        };
        info!("Read sunlight for {} room(s) from the cache", baked.len());
        self.bake.adopt(snapshot.hash);
        self.land(baked, images);
    }

    #[cfg(test)]
    fn using_the_disk(&mut self, use_disk: bool) {
        self.use_disk = use_disk;
    }
}

/// One room's light as written to disk. Its atlas is not: it is [`Atlas::of`] its faces.
#[derive(serde::Serialize, serde::Deserialize)]
struct CachedRoom {
    faces_hash: u64,
    light: Vec<Vec3>,
}

/// A request to bake now, whatever auto-bake is doing.
#[derive(Message)]
pub struct BakeSunlight;

/// A request to forget every room's light.
#[derive(Message)]
pub struct ClearSunlight;

/// One room's light, for the layout its faces chart to.
struct RoomLight {
    faces_hash: u64,
    atlas: Atlas,
    /// Kept so an edit can carry it over without baking.
    light: Vec<Vec3>,
    image: Handle<Image>,
}

/// What a bake hands back for a room. Images are made on the main thread, when it lands.
struct BakedLight {
    room: Entity,
    faces_hash: u64,
    atlas: Atlas,
    light: Vec<Vec3>,
}

/// What the light depends on, cheap enough to take every frame: rooms by their faces' hash and
/// which of their slots are sky. Turned into a [`Job`] only when there is something to bake.
struct Snapshot {
    /// The `BakedRoomGeometry`, its `Room`, its faces' hash, and whether each slot is sky.
    rooms: Vec<(Entity, Entity, u64, [bool; 3])>,
    light: Daylight,
    hash: u64,
}

impl Snapshot {
    fn take(
        bakes: &Query<(Entity, &BakedRoomGeometry)>,
        paint: &Query<(Option<&RoomSurfaces>, Option<&Side>)>,
        palette: &Palette,
        sun: &SunPosition,
        sky: SkyTime,
    ) -> Snapshot {
        let toward = sun.toward();
        let mut rooms: Vec<(Entity, Entity, u64, [bool; 3])> = bakes
            .iter()
            .map(|(entity, baked)| {
                let (surfaces, side) = paint.get(baked.room).unwrap_or((None, None));
                let is_sky = RoomSlot::ALL.map(|slot| {
                    surfaces.and_then(|surfaces| palette.paint(surfaces, slot, side.copied())) == Some(SKYBOX)
                });
                (entity, baked.room, baked.faces_hash, is_sky)
            })
            .collect();
        // By what the rooms are, not by entity, which a client adopting a map renumbers.
        rooms.sort_by_key(|room| (room.2, room.3));
        let mut hasher = Fnv::default();
        FORMAT.hash(&mut hasher);
        // So retuning the bake is a miss rather than old light.
        lightmap::tuning().hash(&mut hasher);
        sunlight::tuning().hash(&mut hasher);
        toward.to_array().map(f32::to_bits).hash(&mut hasher);
        sky.hash(&mut hasher);
        for (_, _, faces_hash, is_sky) in &rooms {
            (faces_hash, is_sky).hash(&mut hasher);
        }
        Snapshot { rooms, light: daylight(toward, sky), hash: hasher.finish() }
    }

    /// Each room's cached light on the layout its faces chart to now, or `None` if any room has
    /// none or it does not fit. Matched by place, since a bake is written in snapshot order and
    /// rooms may share a faces hash (no faces of their own).
    fn from_cache(&self, cached: Vec<CachedRoom>, bakes: &Query<(Entity, &BakedRoomGeometry)>) -> Option<Vec<BakedLight>> {
        if cached.len() != self.rooms.len() {
            return None;
        }
        self.rooms
            .iter()
            .zip(cached)
            .map(|((entity, room, faces_hash, _), cached)| {
                let (_, baked) = bakes.get(*entity).ok()?;
                let atlas = Atlas::of(&baked.faces);
                let light = cached.light;
                (cached.faces_hash == *faces_hash && light.len() == (atlas.size.x * atlas.size.y) as usize)
                    .then_some(BakedLight { room: *room, faces_hash: *faces_hash, atlas, light })
            })
            .collect()
    }

    fn job(&self, bakes: &Query<(Entity, &BakedRoomGeometry)>, labels: &VolumeLabels) -> Job {
        let rooms = self
            .rooms
            .iter()
            .filter_map(|(entity, room, faces_hash, is_sky)| {
                let (_, baked) = bakes.get(*entity).ok()?;
                let sky = baked
                    .faces
                    .iter()
                    .map(|face| {
                        let slot = RoomSlot::of(face.plane.normal);
                        is_sky[RoomSlot::ALL.iter().position(|s| *s == slot).expect("every slot is in ALL")]
                    })
                    .collect();
                Some(JobRoom { room: *room, label: labels.label(*room), faces: baked.faces.clone(), sky, faces_hash: *faces_hash })
            })
            .collect();
        Job { rooms, light: self.light, surface: None, baked: Vec::new(), busy: Duration::ZERO }
    }
}

struct JobRoom {
    room: Entity,
    label: String,
    faces: Vec<Face>,
    sky: Vec<bool>,
    faces_hash: u64,
}

/// Everything a bake reads, copied out of the world so it can run elsewhere. The first item is the
/// whole map's surface, which every room's rays are cast against; then a room an item.
struct Job {
    rooms: Vec<JobRoom>,
    light: Daylight,
    surface: Option<LitSurface>,
    baked: Vec<BakedLight>,
    /// Time spent stepping, not counting a wasm bake's waits between chunks.
    busy: Duration,
}

impl BakeJob for Job {
    type Output = Vec<BakedLight>;

    fn progress(&self) -> (usize, usize) {
        (usize::from(self.surface.is_some()) + self.baked.len(), 1 + self.rooms.len())
    }

    fn doing(&self) -> String {
        match self.surface {
            None => get!("bakes.doing.surface"),
            Some(_) => self.rooms.get(self.baked.len()).map(|room| room.label.clone()).unwrap_or_default(),
        }
    }

    fn step(&mut self) {
        let started = Instant::now();
        self.do_step();
        self.busy += started.elapsed();
    }

    fn finish(self) -> Vec<BakedLight> {
        info!(
            "Baked sunlight for {} room(s), {} texels, in {:.0} ms",
            self.baked.len(),
            self.baked.iter().map(|room| room.light.len()).sum::<usize>(),
            self.busy.as_secs_f32() * 1000.0
        );
        self.baked
    }
}

impl Job {
    fn do_step(&mut self) {
        let Some(surface) = &self.surface else {
            let faces = self.rooms.iter().flat_map(|room| room.faces.iter().cloned()).collect();
            let sky = self.rooms.iter().flat_map(|room| room.sky.iter().copied()).collect();
            self.surface = Some(LitSurface::from_faces(faces, sky));
            return;
        };
        let room = &self.rooms[self.baked.len()];
        let atlas = Atlas::of(&room.faces);
        let light = atlas.bake(&room.faces, &room.sky, surface, &self.light);
        self.baked.push(BakedLight { room: room.room, faces_hash: room.faces_hash, atlas, light });
    }
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
        // `RoomLight::light` is the CPU copy.
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

/// A room rebuilt since its light was baked keeps that light, laid onto its new layout. No rays,
/// so it is cheap enough to do the frame the room changes. A room that is gone loses its light.
fn carry_light_over(
    mut state: ResMut<SunBake>,
    bakes: Query<&BakedRoomGeometry>,
    rooms: Query<(), Or<(With<Room>, With<Block>)>>,
    mut images: ResMut<Assets<Image>>,
) {
    state.lit.retain(|room, _| rooms.contains(*room));
    for baked in &bakes {
        let Some(old) = state.lit.get(&baked.room) else { continue };
        if old.faces_hash == baked.faces_hash {
            continue;
        }
        let atlas = Atlas::of(&baked.faces);
        let light = atlas.carry_over(&old.atlas, &old.light);
        let image = images.add(lightmap_image(atlas.size, &light));
        state.lit.insert(baked.room, RoomLight { faces_hash: baked.faces_hash, atlas, light, image });
    }
}

#[allow(clippy::too_many_arguments)]
fn auto_bake_sunlight(
    mut requests: MessageReader<BakeSunlight>,
    mut clears: MessageReader<ClearSunlight>,
    mut state: ResMut<SunBake>,
    rooms: Res<RoomBake>,
    time: Res<Time<Real>>,
    bakes: Query<(Entity, &BakedRoomGeometry)>,
    paint: Query<(Option<&RoomSurfaces>, Option<&Side>)>,
    labels: VolumeLabels,
    palette: Res<Palette>,
    sun: Res<SunPosition>,
    sky: Res<SkyTime>,
    edited: Query<(), Or<(Changed<BakedRoomGeometry>, Changed<RoomSurfaces>, Changed<Side>)>>,
    mut rebuilt: RemovedComponents<BakedRoomGeometry>,
    mut images: ResMut<Assets<Image>>,
) {
    let state = &mut *state;
    let inputs_changed = rebuilt.read().count() > 0
        || !edited.is_empty()
        || palette.is_changed()
        || sun.is_changed()
        || sky.is_changed();
    if clears.read().count() > 0 {
        state.lit.clear();
        state.bake.forget();
        info!("Cleared the sunlight");
    }
    if requests.read().count() > 0 {
        state.bake.ask();
    }
    if let Some(baked) = state.bake.poll() {
        state.landed(baked, &mut images);
    }
    // Half-rebuilt rooms leave holes for light to come through. With nothing to bake, a snapshot
    // only tells stale from up to date, so it is taken only on a change.
    if !rooms.is_settled() || !(state.bake.would_bake() || inputs_changed) {
        return;
    }
    let snapshot = Snapshot::take(&bakes, &paint, &palette, &sun, *sky);
    state.take_from_cache(&snapshot, &bakes, &mut images);
    state.bake.update(snapshot.hash, rooms.is_settled(), time.elapsed_secs(), || snapshot.job(&bakes, &labels));
}

/// A round is never played in stale light. A map unchanged since its last bake needs nothing: its
/// rooms were rebuilt just before this, but their light is kept by room.
#[allow(clippy::too_many_arguments)]
fn bake_sunlight_for_the_round(
    mut state: ResMut<SunBake>,
    bakes: Query<(Entity, &BakedRoomGeometry)>,
    paint: Query<(Option<&RoomSurfaces>, Option<&Side>)>,
    labels: VolumeLabels,
    palette: Res<Palette>,
    sun: Res<SunPosition>,
    sky: Res<SkyTime>,
    mut images: ResMut<Assets<Image>>,
) {
    let snapshot = Snapshot::take(&bakes, &paint, &palette, &sun, *sky);
    state.take_from_cache(&snapshot, &bakes, &mut images);
    if let Some(baked) = state.bake.for_the_round(snapshot.hash, || snapshot.job(&bakes, &labels)) {
        state.landed(baked, &mut images);
    }
}

fn report_sunlight(state: Res<SunBake>, mut progress: ResMut<BakeProgress>) {
    bake_job::report(&mut progress, BakeKind::Sunlight, state.bake.status());
}

/// Every frame, as `paint_rooms` is: a slot takes its room's lightmap while it is drawn with a
/// `StandardMaterial`, which is the only material that reads one, and while that light is for the
/// layout its `UV_1` is. A sky slot never has one.
///
/// Two things about Bevy 0.20 shape this. Adding or removing a `Lightmap` does not re-specialize
/// the mesh's pipeline, so the bind group and pipeline disagree and wgpu stops the frame; marking
/// the material changed is what does. And a lightmap's image is only read when the component
/// first arrives, so a new image is a removal one frame and an insertion the next.
fn attach_lightmaps(
    mut commands: Commands,
    state: Res<SunBake>,
    mut slots: Query<(Entity, &ChildOf, Option<&mut MeshMaterial3d<StandardMaterial>>, Option<&Lightmap>), With<RoomSlotMesh>>,
    bakes: Query<&BakedRoomGeometry>,
) {
    for (entity, parent, material, drawn) in &mut slots {
        let Some(mut material) = material else {
            if drawn.is_some() {
                commands.entity(entity).remove::<Lightmap>();
            }
            continue;
        };
        let wanted = bakes
            .get(parent.parent())
            .ok()
            .and_then(|baked| state.lit.get(&baked.room).filter(|light| light.faces_hash == baked.faces_hash))
            .map(|light| &light.image);
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
            .add_message::<ClearSunlight>()
            // Ungated, as the room bake is: the map can change during a round.
            .add_systems(
                Update,
                (auto_bake_sunlight, report_sunlight, carry_light_over, attach_lightmaps)
                    .chain()
                    .after(BakePlugin::auto_bake_rooms),
            )
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
    use crate::tool::bake_job::BakeState;
    use crate::tool::room::CalculateRoomGeometry;

    /// Enough to swap into Play, with one room under an open sky. Both auto-bakes off unless a test
    /// turns one on.
    fn headless() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::state::app::StatesPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::diagnostic::FrameCountPlugin,
            bevy::input::InputPlugin,
            bevy::time::TimePlugin,
            bevy::app::TaskPoolPlugin::default(),
        ));
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Image>();
        app.init_resource::<Palette>().init_resource::<SunPosition>().init_resource::<SkyTime>();
        app.add_plugins((GamePlugin, BakePlugin, SunBakePlugin));
        app.world_mut().resource_mut::<RoomBake>().set_auto(false);
        app.world_mut().resource_mut::<SunBake>().set_auto(false);
        app.world_mut().resource_mut::<SunBake>().using_the_disk(false);
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

    fn lightmaps(app: &App) -> Vec<AssetId<Image>> {
        app.world().resource::<SunBake>().lit.values().map(|light| light.image.id()).collect()
    }

    fn light(app: &App) -> Vec<Vec<Vec3>> {
        app.world().resource::<SunBake>().lit.values().map(|light| light.light.clone()).collect()
    }

    /// Slots drawn with a lightmap right now.
    fn lit_slots(app: &mut App) -> usize {
        app.world_mut().query_filtered::<(), (With<RoomSlotMesh>, With<Lightmap>)>().iter(app.world()).count()
    }

    fn the_room(app: &mut App) -> Entity {
        app.world_mut().query_filtered::<Entity, With<Room>>().single(app.world()).unwrap()
    }

    /// Every room is rebuilt on entering Play; the light is the same, so it is not baked again.
    #[test]
    fn a_second_round_on_an_unchanged_map_reuses_the_light() {
        let mut app = headless();
        enter(&mut app, AppMode::Play);
        let first = lightmaps(&app);
        assert_eq!(first.len(), 1, "the round started unlit");

        enter(&mut app, AppMode::Editor);
        enter(&mut app, AppMode::Play);
        assert_eq!(lightmaps(&app), first, "an unchanged map was baked again");

        let before = light(&app);
        enter(&mut app, AppMode::Editor);
        app.world_mut().resource_mut::<SunPosition>().altitude = 20.0;
        enter(&mut app, AppMode::Play);
        assert_ne!(light(&app), before, "the sun moved and the light did not");
        // Written into the image already drawn, so no slot loses its lightmap to a new handle.
        assert_eq!(lightmaps(&app), first);
    }

    /// With the sun's auto-bake off nothing will replace it, and a reshaped room still keeps the
    /// light it had, laid onto its new layout.
    #[test]
    fn a_reshaped_room_keeps_its_light_until_a_bake_replaces_it() {
        let mut app = headless();
        enter(&mut app, AppMode::Play);
        enter(&mut app, AppMode::Editor);
        app.update();
        assert!(lit_slots(&mut app) > 0, "nothing was lit to begin with");

        let room = the_room(&mut app);
        *app.world_mut().get_mut::<Room>(room).unwrap() = Room::new(Vec3::ZERO, Vec3::new(6.0, 3.0, 4.0));
        app.world_mut().write_message(CalculateRoomGeometry);
        for _ in 0..3 {
            app.update();
        }

        let baked = app.world_mut().query::<&BakedRoomGeometry>().single(app.world()).unwrap().faces_hash;
        let light = &app.world().resource::<SunBake>().lit[&room];
        assert_eq!(light.faces_hash, baked, "the light was not carried onto the new layout");
        assert!(lit_slots(&mut app) > 0, "the room went unlit while it waited for a bake");
    }

    /// The bake runs on the task pool, and the light already standing stays up until it lands.
    #[test]
    fn auto_bake_replaces_the_light_in_the_background() {
        let mut app = headless();
        enter(&mut app, AppMode::Play);
        enter(&mut app, AppMode::Editor);
        let first = light(&app);
        let slots = lit_slots(&mut app);
        assert!(slots > 0, "nothing was lit to begin with");
        app.world_mut().resource_mut::<SunBake>().set_auto(true);
        app.world_mut().resource_mut::<SunPosition>().azimuth = 300.0;

        let started = std::time::Instant::now();
        while light(&app) == first {
            assert!(started.elapsed().as_secs() < 10, "the bake never landed");
            app.update();
            assert_eq!(lit_slots(&mut app), slots, "a slot lost its light before the replacement came");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_bake_reports_the_surface_and_each_room_as_it_goes() {
        let mut app = headless();
        app.world_mut().write_message(CalculateRoomGeometry);
        app.update();
        let status = |app: &App| app.world().resource::<BakeProgress>().get(BakeKind::Sunlight).cloned().unwrap();
        assert_eq!(status(&app).state, BakeState::Stale);

        app.world_mut().write_message(BakeSunlight);
        app.update();
        assert_eq!(status(&app).state, BakeState::Baking);
        let started = std::time::Instant::now();
        while status(&app).state != BakeState::UpToDate {
            assert!(started.elapsed().as_secs() < 10, "the bake never landed");
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let done = status(&app);
        assert_eq!((done.completed, done.scheduled), (2, 2), "the surface, then the one room");
    }

    /// A second launch on the same map, as a second app, takes the light off disk without baking.
    #[test]
    fn a_map_baked_before_reads_its_light_from_the_cache() {
        let with_disk = || {
            let mut app = headless();
            app.world_mut().resource_mut::<SunBake>().using_the_disk(true);
            // Nobody else bakes under this sun, so nothing but this test's bake is on disk for it.
            *app.world_mut().resource_mut::<SunPosition>() = SunPosition { azimuth: 123.25, altitude: 31.5 };
            app
        };
        let mut first = with_disk();
        enter(&mut first, AppMode::Play);
        let hash = first.world().resource::<SunBake>().bake.baked().expect("the round was baked");
        let started = std::time::Instant::now();
        while bake_cache::read::<Vec<CachedRoom>>(CACHE_FOLDER, hash).is_none() {
            assert!(started.elapsed().as_secs() < 10, "the bake was never written");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let mut second = with_disk();
        second.world_mut().write_message(CalculateRoomGeometry);
        second.update();
        second.update();
        let status = second.world().resource::<BakeProgress>().get(BakeKind::Sunlight).cloned().unwrap();
        assert_eq!(status.state, BakeState::UpToDate, "the light was not taken from the cache");
        assert!(!second.world().resource::<SunBake>().is_baking());
        assert_eq!(light(&second), light(&first));
    }

    #[test]
    fn clearing_forgets_the_light() {
        let mut app = headless();
        enter(&mut app, AppMode::Play);
        enter(&mut app, AppMode::Editor);
        app.world_mut().write_message(ClearSunlight);
        app.update();
        app.update();
        assert!(lightmaps(&app).is_empty());
        assert_eq!(lit_slots(&mut app), 0);
    }
}
