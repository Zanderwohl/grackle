use std::collections::{HashMap, HashSet, VecDeque};

use bevy::diagnostic::FrameCount;
use bevy::prelude::*;

use crate::common::lightmap;
use crate::common::world_surface;
use crate::common::app_mode::AppMode;
use bevy_egui::egui;
use crate::editor::editable::FeatureTag;
use crate::get;
use crate::tool::room::{CalculateRoomGeometry, ClearRoomGeometry, Room, RoomSlot};

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
            .add_message::<CalculateRoomGeometry>()
            .add_message::<ClearRoomGeometry>()
            .add_message::<LogECS>()
            .add_systems(Update, (Self::post_startup, Self::clear_room_geometry, Self::log_ecs).run_if(in_state(AppMode::Editor)))
            // Ungated: the map can be edited during a round.
            .add_systems(Update, (Self::bake_room_geometry, Self::auto_bake_rooms).chain())
            .configure_sets(OnEnter(AppMode::Play), BakeSystems::All)
            .add_systems(
                OnEnter(AppMode::Play),
                Self::bake_everything_for_the_round.in_set(BakeSystems::All),
            )
        ;
    }
}

/// Room meshes rebuilt as the map changes, a few a frame.
///
/// A room's mesh depends on every room touching it, since each cuts holes in
/// the other. So a room whose box changed is queued with whatever touched its
/// old box and whatever touches its new one. The old box is the one seen last
/// frame rather than the one its mesh was baked from, which a room dragged
/// before its first bake does not have.
#[derive(Resource)]
pub struct RoomBake {
    auto: bool,
    queue: VecDeque<Entity>,
    seen: HashMap<Entity, Room>,
    material: Option<Handle<StandardMaterial>>,
}

impl Default for RoomBake {
    fn default() -> Self {
        Self {
            auto: true,
            queue: VecDeque::new(),
            seen: HashMap::new(),
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
        self.queue.is_empty()
    }

    pub fn set_auto(&mut self, auto: bool) {
        if auto && !self.auto {
            // Rooms changed while it was off; every room reads as new.
            self.seen.clear();
        }
        self.auto = auto;
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
        self.queue.retain(|e| !set.contains(e));
        for entity in urgent.into_iter().rev() {
            self.queue.push_front(entity);
        }
    }
}

/// Inclusive, because a shared wall is an opening too. Engulfing counts,
/// which `clip_rect` would reject: a few spare rebakes, never a missed one.
fn touches(a: &Room, b: &Room) -> bool {
    let ((a_min, a_max), (b_min, b_max)) = (a.bounds(), b.bounds());
    a_min.cmple(b_max).all() && b_min.cmple(a_max).all()
}

impl BakePlugin {
    pub fn ui(ui: &mut egui::Ui, auto: bool, auto_sun: bool, baking_sun: bool) -> BakeCommands {
        let mut commands = BakeCommands { auto, auto_sun, ..default() };
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
            ui.checkbox(&mut commands.auto_sun, get!("bakes.auto_sunlight"));
            ui.horizontal(|ui| {
                if ui.add_enabled(!auto_sun, egui::Button::new(get!("bakes.sunlight"))).clicked() {
                    commands.bake_sunlight = true;
                }
                if ui.small_button("x").clicked() {
                    commands.clear_sunlight = true;
                }
                if baking_sun {
                    ui.spinner();
                    ui.label(get!("bakes.baking_sunlight"));
                }
            });
            if ui.button("Log ECS").clicked() {
                commands.log_ecs = true;
            }
        });
        commands
    }

    fn post_startup(
        frames: Res<FrameCount>,
        mut room_events: MessageWriter<CalculateRoomGeometry>,
    ) {
        if frames.0 == 5 {
            room_events.write(CalculateRoomGeometry);
        }
    }

    /// A request for everything at once. With auto-bake on it is already
    /// covered, and honoring it would stall the frame the budget exists to
    /// protect.
    fn bake_room_geometry(
        mut events: MessageReader<CalculateRoomGeometry>,
        mut bake: ResMut<RoomBake>,
        rooms: Query<(Entity, &Room)>,
        existing_bakes: Query<Entity, With<BakedRoomGeometry>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        if events.read().next().is_none() { return; }
        events.clear();
        if bake.auto { return; }

        bake_rooms(&mut bake, &rooms, &existing_bakes, &mut commands, &mut meshes, &mut materials);
    }

    pub(crate) fn auto_bake_rooms(
        mut bake: ResMut<RoomBake>,
        rooms: Query<(Entity, &Room)>,
        bakes: Query<(Entity, &BakedRoomGeometry)>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        if !bake.auto { return; }

        // Cheap, and outside the budget: a deleted room's walls vanish at once.
        for (entity, baked) in &bakes {
            if !rooms.contains(baked.room) {
                commands.entity(entity).despawn();
            }
        }

        let all: Vec<(Entity, &Room)> = rooms.iter().collect();
        let touching = |b: &Room| all.iter().filter(|(_, r)| touches(r, b)).map(|(e, _)| *e).collect::<Vec<_>>();
        let mut urgent = Vec::new();
        for (entity, room) in &all {
            match bake.seen.get(entity) {
                Some(old) if old == *room => continue,
                Some(old) => urgent.extend(touching(old)),
                None => {}
            }
            urgent.push(*entity);
            urgent.extend(touching(room));
        }
        let gone: Vec<Room> = bake.seen.iter()
            .filter(|(entity, _)| !rooms.contains(**entity))
            .map(|(_, room)| room.clone())
            .collect();
        for old in &gone {
            urgent.extend(touching(old));
        }
        // Only on a change: rebuilding it every idle frame is a map's worth of clones for nothing.
        if !urgent.is_empty() || !gone.is_empty() {
            bake.jump_the_queue(urgent);
            bake.seen = all.iter().map(|(e, r)| (*e, (*r).clone())).collect();
        }

        if bake.queue.is_empty() { return; }
        let material = bake.material(&mut materials);
        let existing: HashMap<Entity, Entity> = bakes.iter().map(|(bake, baked)| (baked.room, bake)).collect();
        let mut done = 0;
        while done < ROOMS_PER_FRAME {
            let Some(entity) = bake.queue.pop_front() else { break };
            if !rooms.contains(entity) { continue; }
            bake_one(entity, &all, existing.get(&entity).copied(), &material, &mut commands, &mut meshes);
            done += 1;
        }
    }

    /// Rebuild the room meshes before a round starts, unconditionally. A
    /// client adopting the server's map enters Play the same frame, and the
    /// bake request it wrote is read a frame later. Collision does not need
    /// these meshes, so a missed bake is an invisible map, not a fall.
    pub(crate) fn bake_everything_for_the_round(
        mut bake: ResMut<RoomBake>,
        rooms: Query<(Entity, &Room)>,
        existing_bakes: Query<Entity, With<BakedRoomGeometry>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        bake_rooms(&mut bake, &rooms, &existing_bakes, &mut commands, &mut meshes, &mut materials);
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
        bake.seen.clear();
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

/// Build one mesh per room, its share of the map's surface, all in
/// this frame. Shared by the editor's bake and the round-start bake.
fn bake_rooms(
    bake: &mut RoomBake,
    rooms: &Query<(Entity, &Room)>,
    existing_bakes: &Query<Entity, With<BakedRoomGeometry>>,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    for entity in existing_bakes {
        commands.entity(entity).despawn();
    }

    let material = bake.material(materials);
    let all: Vec<(Entity, &Room)> = rooms.iter().collect();

    for (entity, _) in &all {
        bake_one(*entity, &all, None, &material, commands, meshes);
    }

    bake.queue.clear();
    bake.seen = all.iter().map(|(e, r)| (*e, (*r).clone())).collect();
    info!("Baked geometry for {} room(s)", all.len());
}

/// One room's walls against every other room in `all`, rebuilt on `existing`
/// if it already has a bake.
fn bake_one(
    entity: Entity,
    all: &[(Entity, &Room)],
    existing: Option<Entity>,
    material: &Handle<StandardMaterial>,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
) {
    let rooms: Vec<Room> = all.iter().map(|(_, r)| (*r).clone()).collect();
    let index = all.iter().position(|(e, _)| *e == entity).expect("a room is baked against a list it is in");
    let faces = world_surface::room_share(&rooms, index);
    let slots = world_surface::slot_meshes(&faces);
    let geometry = BakedRoomGeometry { room: entity, faces_hash: lightmap::faces_hash(&faces), faces };
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

/// One room's walls, rebuilt in place when that room is rebaked. Its children are a
/// [`RoomSlotMesh`] each.
#[derive(Component)]
pub struct BakedRoomGeometry {
    pub room: Entity,
    /// What its meshes were built from, kept for the sun bake, which charts them the same way.
    pub faces: Vec<world_surface::Face>,
    /// [`lightmap::faces_hash`] of `faces`: which lightmap layout its meshes' `UV_1` is.
    pub faces_hash: u64,
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
        app
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
            if app.world().resource::<RoomBake>().queue.is_empty() {
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

        assert_ne!(before[&edited], meshes(&mut app)[&edited], "the edit waited behind the load-in");
        assert!(!app.world().resource::<RoomBake>().queue.is_empty(), "the load-in finished, so this proved nothing");
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
    pub bake_sunlight: bool,
    pub clear_sunlight: bool,
    pub calculate_room_geometry: bool,
    pub clear_room_geometry: bool,
    pub log_ecs: bool,
}
