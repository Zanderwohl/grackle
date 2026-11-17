use std::collections::{HashMap, HashSet, VecDeque};

use bevy::diagnostic::FrameCount;
use bevy::prelude::*;

use crate::common::lightmap;
use crate::common::world_surface;
use crate::common::app_mode::AppMode;
use bevy_egui::egui;
use crate::editor::editable::FeatureTag;
use crate::get;
use crate::common::world_surface::Volume;
use crate::tool::nav_overlay::{self, NavCommands};
use crate::tool::bake_progress;
use crate::tool::bake_job::{self, BakeKind, BakeProgress, BakeState, BakeStatus, VolumeLabels, SETTLE_SECONDS};
use crate::tool::room::{Block, CalculateRoomGeometry, ClearRoomGeometry, MapVolumes, Room, RoomSlot, VolumeOrder};

#[derive(Message)]
pub struct LogECS;

/// Everything rebuilt from the map before a round starts. A set so a new kind
/// of bake joins by naming it, and the round setup orders after all of them.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BakeSystems {
    All,
}

pub struct BakePlugin;

/// How many rooms auto-bake rebuilds a frame, so opening a big map leaves the
/// editor usable while it fills in.
const ROOMS_PER_FRAME: usize = 8;

impl Plugin for BakePlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<RoomBake>()
            .init_resource::<BakeProgress>()
            .add_message::<CalculateRoomGeometry>()
            .add_message::<ClearRoomGeometry>()
            .add_message::<LogECS>()
            .add_systems(Update, (Self::post_startup, Self::clear_room_geometry, Self::log_ecs).run_if(in_state(AppMode::Editor)))
            // Ungated: the map can be edited during a round.
            .add_systems(Update, (Self::bake_room_geometry, Self::auto_bake_rooms, Self::report_rooms).chain())
            .configure_sets(OnEnter(AppMode::Play), BakeSystems::All)
            .add_systems(
                OnEnter(AppMode::Play),
                Self::bake_everything_for_the_round.in_set(BakeSystems::All),
            )
        ;
    }
}

/// Room and block meshes rebuilt as the map changes, a few a frame.
///
/// A volume's mesh depends on every volume touching it, since each cuts the
/// other. So a volume whose box or place changed is queued with whatever
/// touched its old box and whatever touches its new one. The old box is the
/// one seen when last queued rather than the one its mesh was baked from, which a
/// room dragged before its first bake does not have.
///
/// An edit is queued [`SETTLE_SECONDS`] after the last change to any volume,
/// so a drag rebuilds its rooms once, when it stops. A load-in (nothing seen
/// yet) and a bake asked for are queued at once.
#[derive(Resource)]
pub struct RoomBake {
    auto: bool,
    queue: VecDeque<Entity>,
    /// Rebuilt since the queue was last empty.
    batch_done: usize,
    /// Auto-bake is off and a volume has changed since the last bake.
    stale: bool,
    seen: HashMap<Entity, (Volume, VolumeOrder)>,
    /// Bumped whenever `seen` is replaced or cleared, so an idle frame can skip comparing it.
    seen_version: u64,
    /// When a volume last changed, while that change waits to be queued. Rewritten on every
    /// change, so a map being edited keeps waiting.
    last_stale: Option<f32>,
    /// The volumes as they stood at `last_stale`. A volume re-inserted unchanged is not a change.
    observed: HashMap<Entity, (Volume, VolumeOrder)>,
    /// When a volume last changed, kept after it is queued, so a bake that reads the rooms can
    /// count its own wait from the edit rather than from the rebuild.
    last_edit: f32,
    /// Queue what changed without waiting for it to settle.
    asked: bool,
    material: Option<Handle<StandardMaterial>>,
}

impl Default for RoomBake {
    fn default() -> Self {
        Self {
            auto: true,
            queue: VecDeque::new(),
            batch_done: 0,
            stale: false,
            seen: HashMap::new(),
            seen_version: 0,
            last_stale: None,
            observed: HashMap::new(),
            last_edit: 0.0,
            asked: false,
            material: None,
        }
    }
}

impl RoomBake {
    pub fn auto(&self) -> bool {
        self.auto
    }

    /// Nothing waiting: every room's mesh is of the room as it stands.
    pub fn is_settled(&self) -> bool {
        self.queue.is_empty() && self.last_stale.is_none()
    }

    /// When a volume last changed, in real seconds.
    pub fn last_edit(&self) -> f32 {
        self.last_edit
    }

    pub fn set_auto(&mut self, auto: bool) {
        if auto && !self.auto {
            // Rooms changed while it was off; every room reads as new.
            self.seen.clear();
            self.seen_version += 1;
        }
        if !auto {
            self.last_stale = None;
        }
        self.auto = auto;
    }

    /// `doing` names the volume rebuilt next.
    pub fn status(&self, labels: &VolumeLabels) -> BakeStatus {
        // The queue only drains with auto-bake on; switched off partway, what is left is stale.
        let state = if self.auto && !self.queue.is_empty() {
            BakeState::Baking
        } else if self.auto && self.last_stale.is_some() {
            BakeState::Waiting
        } else if self.stale || !self.queue.is_empty() {
            BakeState::Stale
        } else {
            BakeState::UpToDate
        };
        BakeStatus {
            state,
            auto: self.auto,
            completed: self.batch_done,
            scheduled: self.batch_done + self.queue.len(),
            doing: self.queue.front().map(|room| labels.label(*room)).unwrap_or_default(),
        }
    }

    pub fn existing_material(&self) -> Option<&Handle<StandardMaterial>> {
        self.material.as_ref()
    }

    /// What a slot is drawn with when not even the default material can be baked.
    pub fn material(&mut self, materials: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
        self.material
            .get_or_insert_with(|| materials.add(StandardMaterial {
                base_color: Color::srgb_u8(255, 255, 255),
                ..Default::default()
            }))
            .clone()
    }

    /// Queue `urgent` ahead of everything else, keeping its order. An edit
    /// is rebaked next frame however much of a load-in is still waiting.
    fn jump_the_queue(&mut self, urgent: Vec<Entity>) {
        let mut set = HashSet::new();
        let urgent: Vec<Entity> = urgent.into_iter().filter(|e| set.insert(*e)).collect();
        if self.queue.is_empty() && !urgent.is_empty() {
            self.batch_done = 0;
        }
        self.queue.retain(|e| !set.contains(e));
        for entity in urgent.into_iter().rev() {
            self.queue.push_front(entity);
        }
    }
}

/// Whether `seen` is every volume as it stands now.
fn matches(seen: &HashMap<Entity, (Volume, VolumeOrder)>, volumes: &MapVolumes) -> bool {
    seen.len() == volumes.entities().count()
        && volumes.each().all(|(entity, shape, fill, order)| {
            seen.get(&entity).is_some_and(|(seen, seen_order)| seen.fill == fill && seen.shape == *shape && seen_order == order)
        })
}

fn snapshot(volumes: &MapVolumes) -> HashMap<Entity, (Volume, VolumeOrder)> {
    volumes.placed().into_iter().map(|(order, volume, entity)| (entity, (volume, order))).collect()
}

/// Inclusive, because a shared wall is an opening too. Engulfing counts,
/// which `clip_rect` would reject: a few spare rebakes, never a missed one.
fn touches(a: &Volume, b: &Volume) -> bool {
    let (a, b) = (&a.shape, &b.shape);
    let ((a_min, a_max), (b_min, b_max)) = (a.bounds(), b.bounds());
    a_min.cmple(b_max).all() && b_min.cmple(a_max).all()
}

impl BakePlugin {
    pub fn ui(ui: &mut egui::Ui, auto: bool, auto_sun: bool, halfbright: bool, nav: &mut NavCommands, progress: &BakeProgress) -> BakeCommands {
        let mut commands = BakeCommands { auto, auto_sun, halfbright, ..default() };
        ui.vertical(|ui| {
            ui.checkbox(&mut commands.auto, get!("bakes.auto_rooms"));
            ui.horizontal(|ui| {
                let bake = ui.add_enabled(!auto, egui::Button::new(get!("bakes.room_geometry")));
                if bake.clicked() {
                    commands.calculate_room_geometry = true;
                }
                if ui.small_button("x").clicked() {
                    commands.clear_room_geometry = true;
                }
            });
            ui.checkbox(&mut commands.halfbright, get!("bakes.halfbright"));
            ui.add_enabled(!halfbright, egui::Checkbox::new(&mut commands.auto_sun, get!("bakes.auto_sunlight")));
            ui.horizontal(|ui| {
                if ui.add_enabled(!auto_sun && !halfbright, egui::Button::new(get!("bakes.sunlight"))).clicked() {
                    commands.bake_sunlight = true;
                }
                if ui.small_button("x").clicked() {
                    commands.clear_sunlight = true;
                }
            });
            nav_overlay::section(ui, nav);
            ui.separator();
            if ui.button("Log ECS").clicked() {
                commands.log_ecs = true;
            }
            bake_progress::section(ui, progress);
        });
        commands
    }

    /// With auto-bake on the load-in covers it, and the request would only let an edit skip the
    /// wait for the map to settle.
    fn post_startup(
        frames: Res<FrameCount>,
        bake: Res<RoomBake>,
        mut room_events: MessageWriter<CalculateRoomGeometry>,
    ) {
        if frames.0 == 5 && !bake.auto {
            room_events.write(CalculateRoomGeometry);
        }
    }

    /// A request for everything at once. With auto-bake on it only skips the
    /// wait for the map to settle: rebuilding everything this frame would stall
    /// the frame the budget exists to protect.
    fn bake_room_geometry(
        mut events: MessageReader<CalculateRoomGeometry>,
        mut bake: ResMut<RoomBake>,
        rooms: MapVolumes,
        existing_bakes: Query<Entity, With<BakedRoomGeometry>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        if events.read().next().is_none() { return; }
        events.clear();
        if bake.auto {
            bake.asked = true;
            return;
        }

        bake_rooms(&mut bake, &rooms, &existing_bakes, &mut commands, &mut meshes, &mut materials);
    }

    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub(crate) fn auto_bake_rooms(
        mut bake: ResMut<RoomBake>,
        volumes: MapVolumes,
        bakes: Query<(Entity, &BakedRoomGeometry)>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
        edited: Query<(), Or<(Changed<Room>, Changed<Block>, Changed<VolumeOrder>)>>,
        new_bakes: Query<(), Added<BakedRoomGeometry>>,
        mut removed_rooms: RemovedComponents<Room>,
        mut removed_blocks: RemovedComponents<Block>,
        mut removed_orders: RemovedComponents<VolumeOrder>,
        time: Res<Time<Real>>,
        // The last answer and the `seen_version` it was against.
        mut known: Local<Option<(u64, bool)>>,
    ) {
        let removed = removed_rooms.read().count() + removed_blocks.read().count() + removed_orders.read().count() > 0;
        let touched = removed || !edited.is_empty();
        let version_moved = known.is_none_or(|(version, _)| version != bake.seen_version);
        let unchanged = match *known {
            Some((version, unchanged)) if !touched && version == bake.seen_version => unchanged,
            _ => {
                let unchanged = matches(&bake.seen, &volumes);
                *known = Some((bake.seen_version, unchanged));
                unchanged
            }
        };
        if !bake.auto {
            if bake.stale != !unchanged {
                bake.stale = !unchanged;
            }
            bake.asked = false;
            return;
        }
        if bake.stale {
            bake.stale = false;
        }

        // Cheap, and outside the budget: a deleted room's walls vanish at once.
        if removed || version_moved || !new_bakes.is_empty() {
            for (entity, baked) in &bakes {
                if !volumes.contains(baked.room) {
                    commands.entity(entity).despawn();
                }
            }
        }

        let now = time.elapsed_secs();
        let due = if unchanged {
            // A request with nothing to bake is answered, not kept for the next edit.
            bake.last_stale = None;
            bake.asked = false;
            false
        } else {
            if bake.last_stale.is_none() || (touched && !matches(&bake.observed, &volumes)) {
                bake.last_stale = Some(now);
                bake.last_edit = now;
                bake.observed = snapshot(&volumes);
            }
            let settled = bake.last_stale.is_some_and(|since| now - since >= SETTLE_SECONDS);
            bake.seen.is_empty() || bake.asked || settled
        };
        if due {
            bake.last_stale = None;
            bake.observed.clear();
            bake.asked = false;
            Self::queue_changes(&mut bake, &volumes);
        }

        if bake.queue.is_empty() { return; }
        let placed = volumes.placed();
        let material = bake.material(&mut materials);
        let existing: HashMap<Entity, Entity> = bakes.iter().map(|(bake, baked)| (baked.room, bake)).collect();
        let (entities, volumes): (Vec<Entity>, Vec<Volume>) = placed.into_iter().map(|(_, volume, entity)| (entity, volume)).unzip();
        let mut done = 0;
        while done < ROOMS_PER_FRAME {
            let Some(entity) = bake.queue.pop_front() else { break };
            // A volume deleted since it was queued still counts as done.
            bake.batch_done += 1;
            let Some(index) = entities.iter().position(|e| *e == entity) else { continue };
            bake_one(index, &entities, &volumes, existing.get(&entity).copied(), &material, &mut commands, &mut meshes);
            done += 1;
        }
    }

    /// Queue every volume that changed since `seen`, with whatever touched it before and touches
    /// it now, ahead of the rest of the queue.
    fn queue_changes(bake: &mut RoomBake, volumes: &MapVolumes) {
        let placed = volumes.placed();
        let touching = |b: &Volume| placed.iter().filter(|(_, v, _)| touches(v, b)).map(|(_, _, e)| *e).collect::<Vec<_>>();
        let mut urgent = Vec::new();
        // Queued in the order the query walks them, so a map loads in the order it was built; only
        // the shares need the map's order.
        let by_entity: HashMap<Entity, &(VolumeOrder, Volume, Entity)> = placed.iter().map(|item| (item.2, item)).collect();
        for (order, volume, entity) in volumes.entities().map(|entity| by_entity[&entity]) {
            match bake.seen.get(entity) {
                Some((old, old_order)) if old == volume && old_order == order => continue,
                Some((old, _)) => urgent.extend(touching(old)),
                None => {}
            }
            urgent.push(*entity);
            urgent.extend(touching(volume));
        }
        let gone: Vec<Volume> = bake.seen.iter()
            .filter(|(entity, _)| !volumes.contains(**entity))
            .map(|(_, (volume, _))| volume.clone())
            .collect();
        for old in &gone {
            urgent.extend(touching(old));
        }
        // Only on a change: rebuilding it every idle frame is a map's worth of clones for nothing.
        if !urgent.is_empty() || !gone.is_empty() {
            bake.jump_the_queue(urgent);
            bake.seen = placed.iter().map(|(order, volume, entity)| (*entity, (volume.clone(), order.clone()))).collect();
            bake.seen_version += 1;
        }
    }

    /// Rebuild the room meshes before a round starts, unconditionally. A
    /// client adopting the server's map enters Play the same frame, and the
    /// bake request it wrote is read a frame later. Collision does not need
    /// these meshes, so a missed bake is an invisible map, not a fall.
    pub(crate) fn bake_everything_for_the_round(
        mut bake: ResMut<RoomBake>,
        rooms: MapVolumes,
        existing_bakes: Query<Entity, With<BakedRoomGeometry>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        bake_rooms(&mut bake, &rooms, &existing_bakes, &mut commands, &mut meshes, &mut materials);
    }

    fn report_rooms(bake: Res<RoomBake>, labels: VolumeLabels, mut progress: ResMut<BakeProgress>) {
        bake_job::report(&mut progress, BakeKind::Rooms, bake.status(&labels));
    }

    /// Clearing forgets what auto-bake has seen, so with it on this is a
    /// rebuild from scratch.
    fn clear_room_geometry(
        mut events: MessageReader<ClearRoomGeometry>,
        mut bake: ResMut<RoomBake>,
        existing_bakes: Query<Entity, With<BakedRoomGeometry>>,
        mut commands: Commands,
    ) {
        if events.read().next().is_none() { return; }
        events.clear();

        bake.queue.clear();
        bake.batch_done = 0;
        bake.last_stale = None;
        bake.seen.clear();
        bake.seen_version += 1;
        let count = existing_bakes.iter().count();
        for entity in &existing_bakes {
            commands.entity(entity).despawn();
        }
        if count > 0 {
            info!("Cleared {} baked room mesh(es)", count);
        }
    }

    fn log_ecs(
        mut events: MessageReader<LogECS>,
        query: Query<(Entity, &FeatureTag, &Transform, Option<&Room>, Option<&PointLight>)>,
    ) {
        if events.read().next().is_none() { return; }
        events.clear();

        info!("=== ECS Entity Dump ===");
        for (entity, tag, transform, room, point_light) in &query {
            let room_str = match room {
                Some(r) => format!("{:?}", r),
                None => "None".to_string(),
            };
            let light_str = match point_light {
                Some(l) => format!("{:?}", l),
                None => "None".to_string(),
            };
            info!(
                "{:?} | {:?} | {:?} | Room: {} | PointLight: {}",
                entity, tag, transform.translation, room_str, light_str,
            );
        }
        info!("=== {} entities total ===", query.iter().count());
    }
}

/// Build one mesh per room and block, its share of the map's surface, all in
/// this frame. Shared by the editor's bake and the round-start bake.
fn bake_rooms(
    bake: &mut RoomBake,
    volumes: &MapVolumes,
    existing_bakes: &Query<Entity, With<BakedRoomGeometry>>,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    for entity in existing_bakes {
        commands.entity(entity).despawn();
    }

    let material = bake.material(materials);
    let placed = volumes.placed();
    bake.queue.clear();
    bake.batch_done = placed.len();
    bake.stale = false;
    bake.last_stale = None;
    bake.asked = false;
    bake.seen = placed.iter().map(|(order, volume, entity)| (*entity, (volume.clone(), order.clone()))).collect();
    bake.seen_version += 1;
    let (entities, volumes): (Vec<Entity>, Vec<Volume>) = placed.into_iter().map(|(_, volume, entity)| (entity, volume)).unzip();

    for index in 0..entities.len() {
        bake_one(index, &entities, &volumes, None, &material, commands, meshes);
    }
    info!("Baked geometry for {} volume(s)", entities.len());
}

/// `volumes[index]`'s share against every other volume, in order, rebuilt on
/// `existing` if it already has a bake.
fn bake_one(
    index: usize,
    entities: &[Entity],
    volumes: &[Volume],
    existing: Option<Entity>,
    material: &Handle<StandardMaterial>,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
) {
    let entity = entities[index];
    let faces = world_surface::share(volumes, index);
    let slots = world_surface::slot_meshes(&faces);
    let volume = &volumes[index];
    let air = (volume.fill == world_surface::Fill::Air).then(|| volume.shape.bounds());
    let bounds = crate::common::decal::bounds_of(&faces);
    let geometry = BakedRoomGeometry { room: entity, faces_hash: lightmap::faces_hash(&faces), faces, bounds, air };
    let mut baked = match existing {
        Some(baked) => {
            let mut baked = commands.entity(baked);
            baked.despawn_children().insert(geometry);
            baked
        }
        None => commands.spawn((geometry, Transform::default(), Visibility::default())),
    };
    baked.with_children(|parts| {
        for (slot, mesh) in slots {
            // The default until `paint_rooms` finds the slot's own material.
            parts.spawn((RoomSlotMesh { room: entity, slot }, Mesh3d(meshes.add(mesh)), MeshMaterial3d(material.clone())));
        }
    });
}

/// One room's or block's walls, rebuilt in place when it is rebaked. Its children are a
/// [`RoomSlotMesh`] each.
#[derive(Component)]
pub struct BakedRoomGeometry {
    /// The `Room` or `Block` entity.
    pub room: Entity,
    /// What its meshes were built from, kept for the sun bake, which charts them the same way.
    pub faces: Vec<world_surface::Face>,
    /// [`lightmap::faces_hash`] of `faces`: which lightmap layout its meshes' `UV_1` is.
    pub faces_hash: u64,
    /// The box around `faces`; `None` when there are none.
    pub bounds: Option<(Vec3, Vec3)>,
    /// `None` for a solid. Not its faces' bounds, which miss the air of a room whose walls
    /// neighbors opened up.
    pub air: Option<(Vec3, Vec3)>,
}

/// One slot of a room's baked walls, so it can take its own material.
#[derive(Component)]
pub struct RoomSlotMesh {
    pub room: Entity,
    pub slot: RoomSlot,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::GamePlugin;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// A headless app with nothing in it, enough to swap into Play.
    fn headless() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::state::app::StatesPlugin,
            bevy::asset::AssetPlugin::default(),
            // `post_startup` bakes at frame five, past anything these tests do.
            bevy::diagnostic::FrameCountPlugin,
            bevy::input::InputPlugin,
            bevy::time::TimePlugin,
        ));
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.add_plugins(GamePlugin);
        app.add_plugins(BakePlugin);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(FRAME));
        app
    }

    /// How much real time an update takes in these tests.
    const FRAME: Duration = Duration::from_millis(100);

    /// One update that takes `seconds`.
    fn advance(app: &mut App, seconds: f32) {
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(seconds)));
        app.update();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(FRAME));
    }

    /// Auto-bake off, so only the round bakes.
    fn headless_with_a_room() -> App {
        let mut app = headless();
        app.world_mut().resource_mut::<RoomBake>().set_auto(false);
        app.world_mut().spawn(Room::new(
            Vec3::new(-4.0, 0.0, -4.0),
            Vec3::new(4.0, 3.0, 4.0),
        ));
        app.update();
        app
    }

    /// A 2 m cube with its floor corner at `x` along a row.
    fn cube_at(x: f32) -> Room {
        Room::new(Vec3::new(x, 0.0, 0.0), Vec3::new(x + 2.0, 2.0, 2.0))
    }

    /// Each room's current walls mesh, to tell which were rebaked. Every room in these tests has
    /// walls left standing.
    fn meshes(app: &mut App) -> HashMap<Entity, AssetId<Mesh>> {
        app.world_mut()
            .query::<(&RoomSlotMesh, &Mesh3d)>()
            .iter(app.world())
            .filter(|(part, _)| part.slot == RoomSlot::Walls)
            .map(|(part, mesh)| (part.room, mesh.id()))
            .collect()
    }

    fn settle(app: &mut App) {
        for _ in 0..64 {
            app.update();
            if app.world().resource::<RoomBake>().is_settled() {
                app.update();
                return;
            }
        }
        panic!("auto-bake never caught up");
    }

    fn baked(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<BakedRoomGeometry>>()
            .iter(app.world())
            .count()
    }

    /// Breaks if entering Play stops baking unasked; a client's round is then
    /// played in an invisible map.
    #[test]
    fn entering_play_bakes_without_being_asked() {
        let mut app = headless_with_a_room();
        assert_eq!(baked(&mut app), 0, "something baked before the round started");

        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();

        assert_eq!(baked(&mut app), 1, "the round started against an unbaked map");
    }

    #[test]
    fn a_room_added_since_the_last_bake_is_in_the_new_one() {
        let mut app = headless_with_a_room();
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();
        assert_eq!(baked(&mut app), 1);

        // Back to the editor, add a room, and start another round.
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Editor);
        app.update();
        app.world_mut().spawn(Room::new(
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(14.0, 3.0, 14.0),
        ));
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();

        assert_eq!(baked(&mut app), 2, "the new round was played in the old map");
    }

    #[test]
    fn a_map_bakes_a_few_rooms_a_frame() {
        let mut app = headless();
        let count = ROOMS_PER_FRAME * 2 + 1;
        for i in 0..count {
            app.world_mut().spawn(cube_at(i as f32 * 10.0));
        }
        app.update();
        assert_eq!(baked(&mut app), ROOMS_PER_FRAME);
        settle(&mut app);
        assert_eq!(baked(&mut app), count);
    }

    fn rooms_status(app: &App) -> BakeStatus {
        app.world().resource::<BakeProgress>().get(BakeKind::Rooms).cloned().expect("the room bake reports")
    }

    #[test]
    fn a_load_in_reports_how_far_it_has_got() {
        let mut app = headless();
        let count = ROOMS_PER_FRAME * 2 + 1;
        for i in 0..count {
            app.world_mut().spawn(cube_at(i as f32 * 10.0));
        }
        app.update();
        let status = rooms_status(&app);
        assert_eq!((status.state, status.completed, status.scheduled), (BakeState::Baking, ROOMS_PER_FRAME, count));
        assert!(!status.doing.is_empty());
        settle(&mut app);
        let status = rooms_status(&app);
        assert_eq!((status.state, status.completed, status.scheduled), (BakeState::UpToDate, count, count));
        assert!(status.doing.is_empty());
    }

    #[test]
    fn with_auto_bake_off_a_moved_room_is_stale_until_baked() {
        let mut app = headless_with_a_room();
        app.world_mut().write_message(CalculateRoomGeometry);
        app.update();
        assert_eq!(rooms_status(&app).state, BakeState::UpToDate);
        let room = app.world_mut().query_filtered::<Entity, With<Room>>().single(app.world()).unwrap();
        *app.world_mut().get_mut::<Room>(room).unwrap() = cube_at(3.0);
        app.update();
        assert_eq!(rooms_status(&app).state, BakeState::Stale);
        app.world_mut().write_message(CalculateRoomGeometry);
        app.update();
        assert_eq!(rooms_status(&app).state, BakeState::UpToDate);
    }

    /// Breaks if a move is taken to affect only the room moved: the room it
    /// left keeps a hole into nothing, the room it joined keeps a wall across
    /// the opening.
    #[test]
    fn moving_a_room_rebakes_whoever_it_left_and_whoever_it_joined() {
        let mut app = headless();
        let moved = app.world_mut().spawn(cube_at(0.0)).id();
        let left = app.world_mut().spawn(cube_at(2.0)).id();
        let joined = app.world_mut().spawn(cube_at(20.0)).id();
        let bystander = app.world_mut().spawn(cube_at(40.0)).id();
        settle(&mut app);
        let before = meshes(&mut app);

        *app.world_mut().get_mut::<Room>(moved).unwrap() = cube_at(18.0);
        settle(&mut app);
        let after = meshes(&mut app);

        for room in [moved, left, joined] {
            assert_ne!(before[&room], after[&room], "{room} was not rebaked");
        }
        assert_eq!(before[&bystander], after[&bystander], "a room nowhere near was rebaked");
    }

    #[test]
    fn deleting_a_room_drops_its_walls_and_rebakes_its_neighbor() {
        let mut app = headless();
        let deleted = app.world_mut().spawn(cube_at(0.0)).id();
        let neighbor = app.world_mut().spawn(cube_at(2.0)).id();
        settle(&mut app);
        let before = meshes(&mut app);

        app.world_mut().despawn(deleted);
        settle(&mut app);
        let after = meshes(&mut app);

        assert!(!after.contains_key(&deleted), "a deleted room's walls are still standing");
        assert_ne!(before[&neighbor], after[&neighbor], "the opening into the deleted room is still there");
    }

    /// Breaks if edits wait their turn behind a load-in: the room being
    /// dragged would not follow the pointer until the whole map had baked.
    #[test]
    fn an_edit_jumps_the_load_in() {
        let mut app = headless();
        let edited = app.world_mut().spawn(cube_at(0.0)).id();
        for i in 1..ROOMS_PER_FRAME * 4 {
            app.world_mut().spawn(cube_at(i as f32 * 10.0));
        }
        app.update();
        let before = meshes(&mut app);
        assert!(before.contains_key(&edited), "the first room spawned was not in the first batch");

        *app.world_mut().get_mut::<Room>(edited).unwrap() = cube_at(-5.0);
        app.update();
        advance(&mut app, SETTLE_SECONDS);

        assert_ne!(before[&edited], meshes(&mut app)[&edited], "the edit waited behind the load-in");
        assert!(!app.world().resource::<RoomBake>().queue.is_empty(), "the load-in finished, so this proved nothing");
    }

    /// Breaks if a room is rebuilt while it is still being dragged: every frame of a drag would
    /// be a bake of the rooms it touches.
    #[test]
    fn an_edit_waits_until_the_map_has_been_still_for_a_while() {
        let mut app = headless();
        let dragged = app.world_mut().spawn(cube_at(0.0)).id();
        settle(&mut app);
        let before = meshes(&mut app)[&dragged];

        let frames = (2.0 * SETTLE_SECONDS / FRAME.as_secs_f32()) as usize;
        for i in 0..frames {
            *app.world_mut().get_mut::<Room>(dragged).unwrap() = cube_at((i + 1) as f32 * 0.1);
            app.update();
            assert_eq!(meshes(&mut app)[&dragged], before, "rebaked {i} frames into the drag");
            assert_eq!(rooms_status(&app).state, BakeState::Waiting);
        }
        advance(&mut app, SETTLE_SECONDS * 0.5);
        assert_eq!(meshes(&mut app)[&dragged], before, "rebaked before the map had been still long enough");
        advance(&mut app, SETTLE_SECONDS * 0.5);
        assert_ne!(meshes(&mut app)[&dragged], before, "never rebaked once the drag stopped");
    }

    #[test]
    fn asking_for_a_bake_skips_the_wait() {
        let mut app = headless();
        let moved = app.world_mut().spawn(cube_at(0.0)).id();
        settle(&mut app);
        let before = meshes(&mut app)[&moved];
        *app.world_mut().get_mut::<Room>(moved).unwrap() = cube_at(3.0);
        app.world_mut().write_message(CalculateRoomGeometry);
        app.update();
        assert_ne!(meshes(&mut app)[&moved], before, "a bake asked for waited for the map to settle");
    }

    #[test]
    fn a_bake_request_with_auto_bake_on_leaves_the_budget_alone() {
        let mut app = headless();
        for i in 0..ROOMS_PER_FRAME * 3 {
            app.world_mut().spawn(cube_at(i as f32 * 10.0));
        }
        app.world_mut().write_message(CalculateRoomGeometry);
        app.update();
        assert_eq!(baked(&mut app), ROOMS_PER_FRAME);
    }
}

#[derive(Default)]
pub struct BakeCommands {
    pub auto: bool,
    pub auto_sun: bool,
    pub halfbright: bool,
    pub bake_sunlight: bool,
    pub clear_sunlight: bool,
    pub calculate_room_geometry: bool,
    pub clear_room_geometry: bool,
    pub log_ecs: bool,
}
