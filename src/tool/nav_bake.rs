//! The third and fourth bakes: the nav graph ([`crate::game::nav`]) with its knockback and pickup
//! fields, then its sightlines ([`crate::game::nav_field::SightBake`]), each in the background and
//! kept until a new one replaces it. Sightlines are a bake of their own because they are most of
//! the time, and a round waits only for the first. See documentation/bots.md.
//!
//! Built from the map's volumes rather than from the collision world standing now: entering Play
//! puts rooms past the rollback bar in before the collision world is rebuilt, and the bake for the
//! round runs between the two.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::movement_rules::MovementRules;
use crate::common::trigger::TriggerVolume;
use crate::common::world_surface::{Face, Volume};
use crate::common::weapon::WeaponCatalog;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::editor::weapon_pickup::WeaponPickupSpec;
use crate::game::world_prop::{self, GivesWeapon, WorldProp};
use crate::game::collision::CollisionWorld;
use crate::game::nav::{self, Cell, CellId, Link, NavGraph};
use crate::game::nav_field::{FieldBake, Pickup, Sight, SightBake};
use crate::game::nav_rating::MapStatistics;
use crate::game::player::running_speed;
use crate::get;
use crate::prop::archetype::RoundedBox;
use crate::tool::bake_job::{self, BackgroundBake, BakeJob, BakeKind, BakeProgress};
use crate::tool::bakes::BakeSystems;
use crate::tool::nav_cache;
use crate::tool::room::{Block, MapVolumes, Room, VolumeOrder};

/// Cells linked per item, so progress moves while the links, the bulk of the work, are found.
/// Walks and leaps are separate passes of batches, since a leap is kept only if walking is slower.
const LINK_BATCH: usize = 256;

#[derive(Resource)]
pub struct NavBake {
    graph: NavGraph,
    /// Bumped whenever `graph` or its fields change, for what draws it.
    generation: u64,
    /// Bumped whenever `graph` is replaced: what the sightlines are a bake of.
    built: u64,
    /// What `graph` was built from, for the sightlines to be cast against.
    built_from: Vec<Volume>,
    /// The snapshot hash `graph` is a bake of, which names its cache file.
    built_hash: Option<u64>,
    /// The last hash looked for in the cache, so a miss is not read again every frame.
    looked_up: Option<u64>,
    /// What the map was when last looked at; taken again only on a change.
    snapshot: Option<Snapshot>,
    bake: BackgroundBake<Job>,
    sight: BackgroundBake<SightJob>,
    /// Of `graph`, kept rather than worked out each frame the panel is up.
    statistics: MapStatistics,
}

impl Default for NavBake {
    fn default() -> Self {
        Self {
            graph: NavGraph::default(),
            generation: 0,
            built: 0,
            built_from: Vec::new(),
            built_hash: None,
            looked_up: None,
            snapshot: None,
            // Off: several seconds on Halls in a debug build, too much to redo on every still moment.
            bake: BackgroundBake::new(false),
            // On: it follows the graph rather than the map.
            sight: BackgroundBake::new(true),
            statistics: MapStatistics::default(),
        }
    }
}

impl NavBake {
    pub fn graph(&self) -> &NavGraph {
        &self.graph
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn statistics(&self) -> &MapStatistics {
        &self.statistics
    }

    fn replace(&mut self, hash: Option<u64>, (graph, volumes): (NavGraph, Vec<Volume>)) {
        self.graph = graph;
        self.built_from = volumes;
        self.built_hash = hash;
        self.generation += 1;
        self.built += 1;
        self.statistics = MapStatistics::of(&self.graph);
    }

    /// A bake that just landed, which is also written to the cache.
    fn landed(&mut self, hash: Option<u64>, built: (NavGraph, Vec<Volume>)) {
        if let Some(hash) = hash {
            nav_cache::write(hash, built.0.clone());
        }
        self.replace(hash, built);
    }

    /// Takes the graph of `snapshot` from the cache, sightlines and all if they were written.
    /// Looked for once per hash.
    fn take_from_cache(&mut self, snapshot: &Snapshot) -> bool {
        if self.bake.baked() == Some(snapshot.hash) || self.looked_up == Some(snapshot.hash) {
            return false;
        }
        self.looked_up = Some(snapshot.hash);
        let Some(graph) = nav_cache::read(snapshot.hash) else { return false };
        let sighted = graph.fields().has_sight();
        info!("Read the nav graph from the cache: {} cell(s){}", graph.cells().len(), if sighted { ", with sightlines" } else { "" });
        self.replace(Some(snapshot.hash), (graph, snapshot.volumes.clone()));
        self.bake.adopt(snapshot.hash);
        if sighted {
            self.sight.adopt(self.built);
        }
        true
    }

    /// Dropped if the graph was replaced while the sightlines were cast.
    fn add_sight(&mut self, (of, sight): (u64, Sight)) {
        if of != self.built {
            return;
        }
        let mut fields = self.graph.fields().clone();
        fields.add_sight(sight);
        self.graph.set_fields(fields);
        self.generation += 1;
        self.statistics = MapStatistics::of(&self.graph);
        if let Some(hash) = self.built_hash {
            nav_cache::write(hash, self.graph.clone());
        }
    }

    pub fn auto(&self) -> bool {
        self.bake.auto()
    }

    pub fn set_auto(&mut self, auto: bool) {
        self.bake.set_auto(auto);
    }

    pub fn is_baking(&self) -> bool {
        self.bake.is_baking() || self.sight.is_baking()
    }
}

/// A request to bake now, whatever auto-bake is doing.
#[derive(Message)]
pub struct BakeNav;

/// A request to forget the graph.
#[derive(Message)]
pub struct ClearNav;

/// Everything the graph is built from.
#[derive(Clone)]
struct Snapshot {
    volumes: Vec<Volume>,
    props: Vec<RoundedBox>,
    triggers: Vec<TriggerVolume>,
    spawns: Vec<Vec3>,
    pickups: Vec<Pickup>,
    rules: MovementRules,
    hash: u64,
}

impl Snapshot {
    fn job(&self) -> Job {
        Job { snapshot: self.clone(), world: None, faces: Vec::new(), cells: None, found: None, links: Vec::new(), leaps: Vec::new(), graph: None, fields: None, fielded: false }
    }
}

#[derive(SystemParam)]
struct NavInputs<'w, 's> {
    volumes: MapVolumes<'w, 's>,
    collision: Res<'w, CollisionWorld>,
    rules: Res<'w, MovementRules>,
    triggers: Query<'w, 's, &'static TriggerVolume>,
    spawns: Query<'w, 's, &'static Transform, With<SpawnPointMarker>>,
    edited: Query<'w, 's, (), Or<(Changed<Room>, Changed<Block>, Changed<VolumeOrder>, Changed<TriggerVolume>)>>,
    moved_spawns: Query<'w, 's, (), (With<SpawnPointMarker>, Changed<Transform>)>,
    removed_rooms: RemovedComponents<'w, 's, Room>,
    removed_blocks: RemovedComponents<'w, 's, Block>,
    removed_triggers: RemovedComponents<'w, 's, TriggerVolume>,
    removed_spawns: RemovedComponents<'w, 's, SpawnPointMarker>,
    weapon_pickups: Query<'w, 's, (&'static Transform, &'static WeaponPickupSpec)>,
    prop_pickups: Query<'w, 's, &'static WorldProp, (With<world_prop::Pickup>, Without<GivesWeapon>)>,
    catalog: Option<Res<'w, WeaponCatalog>>,
}

impl NavInputs<'_, '_> {
    /// Drains the removals either way, or one seen on a frame with a change is seen again next frame.
    fn changed_since_last_frame(&mut self) -> bool {
        let removed = self.removed_rooms.read().count()
            + self.removed_blocks.read().count()
            + self.removed_triggers.read().count()
            + self.removed_spawns.read().count();
        removed > 0 || !self.edited.is_empty() || !self.moved_spawns.is_empty() || self.rules.is_changed()
    }

    /// Compared rather than change-detected, like props: placed pickups are stood up again by
    /// systems that do not mark them.
    fn pickups(&self) -> Vec<Pickup> {
        let weapon = |spec: &WeaponPickupSpec| {
            let weapon = self.catalog.as_ref().and_then(|catalog| catalog.get(spec.weapon));
            weapon.map_or_else(|| format!("{:?}", spec.weapon), |weapon| get!(&weapon.name_key))
        };
        let mut pickups: Vec<Pickup> = self
            .weapon_pickups
            .iter()
            .map(|(placed, spec)| Pickup { label: weapon(spec), feet: placed.translation })
            .chain(self.prop_pickups.iter().map(|prop| Pickup { label: prop.model.clone(), feet: prop.at }))
            .collect();
        pickups.sort_by(|a, b| a.label.cmp(&b.label).then(a.feet.to_array().partial_cmp(&b.feet.to_array()).unwrap_or(std::cmp::Ordering::Equal)));
        pickups
    }

    fn snapshot(&self) -> Snapshot {
        let volumes = self.volumes.volumes();
        // Props are rebuilt every tick without change detection, so they are compared instead.
        let props = self.collision.props.clone();
        let triggers: Vec<TriggerVolume> = self.triggers.iter().copied().collect();
        let mut spawns: Vec<Vec3> = self.spawns.iter().map(|placed| placed.translation).collect();
        spawns.sort_by(|a, b| a.to_array().partial_cmp(&b.to_array()).unwrap_or(std::cmp::Ordering::Equal));
        let rules = *self.rules;
        let pickups = self.pickups();
        // Debug writes an `f32` as its shortest exact decimal, so equal inputs hash equal.
        let hash = nav_cache::stable_hash(&format!("{volumes:?}{props:?}{triggers:?}{spawns:?}{pickups:?}{rules:?}"));
        Snapshot { volumes, props, triggers, spawns, pickups, rules, hash }
    }
}

/// Items: the surface, the cells, a batch of walks and then of leaps per [`LINK_BATCH`] cells,
/// pruning to what a spawn can reach, then the fields' batches. Until the cells are found their number is not known, so one batch is
/// assumed and `scheduled` grows once.
struct Job {
    snapshot: Snapshot,
    world: Option<CollisionWorld>,
    faces: Vec<Face>,
    cells: Option<(Vec<Cell>, std::collections::HashMap<IVec2, Vec<CellId>>)>,
    /// How many cells were found, kept once they are handed to the graph.
    found: Option<usize>,
    links: Vec<Vec<Link>>,
    leaps: Vec<Vec<Link>>,
    graph: Option<NavGraph>,
    fields: Option<FieldBake>,
    /// Every field batch is done; only [`FieldBake::finish`] is left.
    fielded: bool,
}

impl BakeJob for Job {
    type Output = (NavGraph, Vec<Volume>);

    fn progress(&self) -> (usize, usize) {
        let batches = self.found.map_or(1, |found| found.div_ceil(LINK_BATCH));
        let linked = if self.graph.is_some() {
            2 * batches
        } else {
            self.links.len().div_ceil(LINK_BATCH) + self.leaps.len().div_ceil(LINK_BATCH)
        };
        let (fielded, fields) = match &self.fields {
            Some(fields) if self.fielded => (fields.progress().1, fields.progress().1),
            Some(fields) => fields.progress(),
            None => (0, 1),
        };
        let completed = usize::from(self.world.is_some()) + usize::from(self.found.is_some()) + linked + usize::from(self.graph.is_some()) + fielded;
        (completed, 3 + 2 * batches + fields)
    }

    fn doing(&self) -> String {
        match (&self.world, &self.cells) {
            (None, _) => get!("bakes.doing.surface"),
            (Some(_), None) if self.found.is_none() => get!("bakes.doing.floors"),
            (Some(_), Some((cells, _))) if self.links.len() < cells.len() => get!("bakes.doing.links"),
            (Some(_), Some((cells, _))) if self.leaps.len() < cells.len() => get!("bakes.doing.leaps"),
            _ if self.fields.is_none() => get!("bakes.doing.reach"),
            _ => get!("bakes.doing.knockback"),
        }
    }

    fn step(&mut self) {
        if let (Some(graph), Some(world), Some(fields)) = (&self.graph, &self.world, &mut self.fields) {
            let snapshot = &self.snapshot;
            self.fielded = fields.step(world, graph, &snapshot.triggers, &snapshot.rules);
            return;
        }
        let Some(world) = &self.world else {
            let mut world = CollisionWorld::default();
            self.faces = world.rebuild_volumes(&self.snapshot.volumes);
            world.props = self.snapshot.props.clone();
            self.world = Some(world);
            return;
        };
        let Some((cells, columns)) = &self.cells else {
            let cells = nav::floor_cells(world, &self.faces, &self.snapshot.triggers);
            let columns = nav::columns(&cells);
            self.found = Some(cells.len());
            self.cells = Some((cells, columns));
            return;
        };
        if self.links.len() < cells.len() {
            let speed = running_speed(&self.snapshot.rules);
            let end = (self.links.len() + LINK_BATCH).min(cells.len());
            for id in self.links.len()..end {
                self.links.push(nav::walk_links(world, cells, columns, id as CellId, speed));
            }
            return;
        }
        if self.leaps.len() < cells.len() {
            let end = (self.leaps.len() + LINK_BATCH).min(cells.len());
            for id in self.leaps.len()..end {
                let snapshot = &self.snapshot;
                self.leaps.push(nav::leap_links(world, cells, columns, &self.links, &snapshot.triggers, &snapshot.rules, id as CellId));
            }
            return;
        }
        let (cells, _) = self.cells.take().expect("found above");
        let mut links = std::mem::take(&mut self.links);
        for (links, leaps) in links.iter_mut().zip(std::mem::take(&mut self.leaps)) {
            links.extend(leaps);
        }
        let graph = NavGraph::reachable(cells, links, &self.snapshot.spawns);
        self.fields = Some(FieldBake::new(&graph, self.snapshot.pickups.clone(), self.snapshot.spawns.clone()));
        self.graph = Some(graph);
    }

    fn finish(self) -> (NavGraph, Vec<Volume>) {
        let mut graph = self.graph.unwrap_or_default();
        if let Some(fields) = self.fields {
            let fields = fields.finish(&graph);
            graph.set_fields(fields);
        }
        info!("Baked the nav graph: {} cell(s)", graph.cells().len());
        (graph, self.snapshot.volumes)
    }
}

/// Items: the surface, then the sightlines' batches.
struct SightJob {
    /// Which graph, as [`NavBake::built`] counts them.
    of: u64,
    volumes: Vec<Volume>,
    world: Option<CollisionWorld>,
    graph: NavGraph,
    bake: SightBake,
    /// Only [`SightBake::finish`] is left.
    cast: bool,
}

impl SightJob {
    fn new(of: u64, volumes: Vec<Volume>, graph: NavGraph) -> Self {
        let bake = SightBake::new(&graph);
        SightJob { of, volumes, world: None, graph, bake, cast: false }
    }
}

impl BakeJob for SightJob {
    type Output = (u64, Sight);

    fn progress(&self) -> (usize, usize) {
        let (cast, rows) = self.bake.progress();
        let cast = if self.cast { rows } else { cast };
        (usize::from(self.world.is_some()) + cast, 1 + rows)
    }

    fn doing(&self) -> String {
        if self.world.is_none() { get!("bakes.doing.surface") } else { get!("bakes.doing.sight") }
    }

    fn step(&mut self) {
        let Some(world) = &self.world else {
            let mut world = CollisionWorld::default();
            world.rebuild_volumes(&self.volumes);
            self.world = Some(world);
            return;
        };
        self.cast = self.bake.step(world);
    }

    fn finish(self) -> (u64, Sight) {
        (self.of, self.bake.finish(&self.graph))
    }
}

fn auto_bake_nav(
    mut requests: MessageReader<BakeNav>,
    mut clears: MessageReader<ClearNav>,
    mut state: ResMut<NavBake>,
    mut inputs: NavInputs,
    time: Res<Time<Real>>,
) {
    let state = &mut *state;
    if clears.read().count() > 0 {
        state.replace(None, (NavGraph::default(), Vec::new()));
        state.bake.forget();
        state.sight.forget();
        info!("Cleared the nav graph");
    }
    if requests.read().count() > 0 {
        state.bake.ask();
    }
    if let Some(built) = state.bake.poll() {
        let hash = state.bake.baked();
        state.landed(hash, built);
    }
    let moved = state.snapshot.as_ref().is_some_and(|seen| seen.props != inputs.collision.props || seen.pickups != inputs.pickups());
    if inputs.changed_since_last_frame() || moved || state.snapshot.is_none() {
        state.snapshot = Some(inputs.snapshot());
    }
    let snapshot = state.snapshot.take().expect("taken above");
    state.take_from_cache(&snapshot);
    let snapshot = state.snapshot.insert(snapshot);
    state.bake.update(snapshot.hash, true, time.elapsed_secs(), || snapshot.job());

    if let Some(sight) = state.sight.poll() {
        state.add_sight(sight);
    }
    let NavBake { graph, built, built_from, sight, .. } = state;
    sight.update(*built, true, time.elapsed_secs(), || SightJob::new(*built, built_from.clone(), graph.clone()));
}

/// A round never starts on a stale graph: bots will walk it.
fn bake_nav_for_the_round(mut state: ResMut<NavBake>, inputs: NavInputs) {
    let snapshot = inputs.snapshot();
    state.take_from_cache(&snapshot);
    if let Some(built) = state.bake.for_the_round(snapshot.hash, || snapshot.job()) {
        state.landed(Some(snapshot.hash), built);
    }
    state.snapshot = Some(snapshot);
}

fn report_nav(state: Res<NavBake>, mut progress: ResMut<BakeProgress>) {
    bake_job::report(&mut progress, BakeKind::Nav, state.bake.status());
    bake_job::report(&mut progress, BakeKind::Sight, state.sight.status());
}

pub struct NavBakePlugin;

impl Plugin for NavBakePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavBake>()
            .add_message::<BakeNav>()
            .add_message::<ClearNav>()
            // Ungated: the map can change during a round.
            .add_systems(Update, (auto_bake_nav, report_nav).chain())
            .add_systems(OnEnter(AppMode::Play), bake_nav_for_the_round.in_set(BakeSystems::All));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::GamePlugin;
    use crate::tool::bakes::{BakePlugin, RoomBake};

    fn snapshot_of(volumes: Vec<Volume>) -> Snapshot {
        Snapshot { volumes, props: Vec::new(), triggers: Vec::new(), spawns: Vec::new(), pickups: Vec::new(), rules: MovementRules::default(), hash: 0 }
    }

    /// Breaks if a bar drawn from this job could run backwards or stop short.
    #[test]
    fn progress_only_climbs_and_ends_complete() {
        let mut job = snapshot_of(vec![Volume::air(Room::new(Vec3::ZERO, Vec3::new(20.0, 3.0, 20.0)))]).job();
        let mut last = job.progress();
        while !job.is_done() {
            job.step();
            let now = job.progress();
            assert!(now.0 > last.0, "a step made no progress: {last:?} then {now:?}");
            assert!(now.0 <= now.1, "completed passed scheduled: {now:?}");
            last = now;
        }
        assert!(last.1 > 4, "a 20 m room was linked in one batch: {last:?}");
        assert!(!job.finish().0.cells().is_empty());
    }

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
        app.add_plugins((GamePlugin, BakePlugin, NavBakePlugin));
        app.world_mut().resource_mut::<RoomBake>().set_auto(false);
        app.world_mut().spawn(Room::new(Vec3::ZERO, Vec3::new(6.0, 3.0, 6.0)));
        app.update();
        app
    }

    #[test]
    fn entering_play_bakes_the_graph_and_reports_it() {
        let mut app = headless();
        assert!(app.world().resource::<NavBake>().graph().cells().is_empty());
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        app.update();
        assert!(!app.world().resource::<NavBake>().graph().cells().is_empty(), "the round started with no graph");
        let status = app.world().resource::<BakeProgress>().get(BakeKind::Nav).cloned().unwrap();
        assert_eq!(status.state, bake_job::BakeState::UpToDate);
        assert_eq!(status.completed, status.scheduled);
    }

    /// Times each stage of the bake on a real map:
    /// `NAV_MAP=path/to/map.gmb cargo test --lib time_the_nav_bake -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn time_the_nav_bake() {
        use bevy::ecs::system::RunSystemOnce;
        use crate::editor::editable::FeatureTimeline;
        use crate::editor::volume_order::place_volumes;

        let path = std::env::var("NAV_MAP").expect("NAV_MAP names the map to time");
        let loaded = crate::editor::save::load(std::path::Path::new(&path)).expect("the map loads");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(loaded.timeline);
        app.add_systems(Update, (FeatureTimeline::sync_entities, place_volumes).chain());
        app.update();
        app.update();
        let snapshot = app
            .world_mut()
            .run_system_once(|volumes: MapVolumes, triggers: Query<&TriggerVolume>, spawns: Query<&Transform, With<SpawnPointMarker>>, pickups: Query<(&Transform, &WeaponPickupSpec)>| Snapshot {
                volumes: volumes.volumes(),
                props: Vec::new(),
                triggers: triggers.iter().copied().collect(),
                spawns: spawns.iter().map(|placed| placed.translation).collect(),
                pickups: pickups.iter().map(|(placed, spec)| Pickup { label: format!("{:?}", spec.weapon), feet: placed.translation }).collect(),
                rules: MovementRules::default(),
                hash: 0,
            })
            .unwrap();
        println!("{} volume(s), {} trigger(s), {} spawn(s), {} weapon pickup(s)", snapshot.volumes.len(), snapshot.triggers.len(), snapshot.spawns.len(), snapshot.pickups.len());

        fn timed<J: BakeJob>(mut job: J) -> J::Output {
            let mut stage = job.doing();
            let (mut started, whole) = (std::time::Instant::now(), std::time::Instant::now());
            while !job.is_done() {
                job.step();
                let doing = job.doing();
                if doing != stage || job.is_done() {
                    println!("{stage:>24}: {:.2} s", started.elapsed().as_secs_f32());
                    (stage, started) = (doing, std::time::Instant::now());
                }
            }
            let finished = std::time::Instant::now();
            let output = job.finish();
            println!("{:>24}: {:.2} s", "finishing", finished.elapsed().as_secs_f32());
            println!("{:>24}: {:.2} s", "in all", whole.elapsed().as_secs_f32());
            output
        }
        let (mut graph, volumes) = timed(snapshot.job());
        let (_, sight) = timed(SightJob::new(0, volumes, graph.clone()));
        let mut fields = graph.fields().clone();
        fields.add_sight(sight);
        graph.set_fields(fields);
        let lines = &graph.fields().sightlines;
        let pairs: usize = lines.standing.iter().map(Vec::len).sum();
        println!("{} cell(s), {} viewpoint(s), {} visible ordered pair(s)", graph.cells().len(), lines.viewpoints.len(), pairs);
        let bytes = postcard::to_allocvec(&graph).unwrap();
        if let Ok(out) = std::env::var("NAV_OUT") {
            std::fs::write(out, &bytes).unwrap();
        }
        let reading = std::time::Instant::now();
        let _: NavGraph = postcard::from_bytes(&bytes).unwrap();
        println!("cached as {:.1} MB, read back in {:.2} s", bytes.len() as f32 / 1.0e6, reading.elapsed().as_secs_f32());
    }
}
