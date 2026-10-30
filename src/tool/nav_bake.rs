//! The third bake: the nav graph ([`crate::game::nav`]), in the background, kept until a new one
//! replaces it. See documentation/bots.md.
//!
//! Built from the map's volumes rather than from the collision world standing now: entering Play
//! puts rooms past the rollback bar in before the collision world is rebuilt, and the bake for the
//! round runs between the two.

use std::hash::{Hash, Hasher};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::movement_rules::MovementRules;
use crate::common::trigger::TriggerVolume;
use crate::common::world_surface::{Face, Volume};
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::nav::{self, Cell, CellId, Link, NavGraph};
use crate::game::player::running_speed;
use crate::get;
use crate::prop::archetype::RoundedBox;
use crate::tool::bake_job::{self, BackgroundBake, BakeJob, BakeKind, BakeProgress};
use crate::tool::bakes::BakeSystems;
use crate::tool::room::{Block, MapVolumes, Room, VolumeOrder};

/// Cells linked per item, so progress moves while the links, the bulk of the work, are found.
const LINK_BATCH: usize = 256;

#[derive(Resource)]
pub struct NavBake {
    graph: NavGraph,
    /// Bumped whenever `graph` is replaced, for what draws it.
    generation: u64,
    /// What the map was when last looked at; taken again only on a change.
    snapshot: Option<Snapshot>,
    bake: BackgroundBake<Job>,
}

impl Default for NavBake {
    fn default() -> Self {
        Self { graph: NavGraph::default(), generation: 0, snapshot: None, bake: BackgroundBake::new(true) }
    }
}

impl NavBake {
    pub fn graph(&self) -> &NavGraph {
        &self.graph
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn replace(&mut self, graph: NavGraph) {
        self.graph = graph;
        self.generation += 1;
    }

    pub fn auto(&self) -> bool {
        self.bake.auto()
    }

    pub fn set_auto(&mut self, auto: bool) {
        self.bake.set_auto(auto);
    }

    pub fn is_baking(&self) -> bool {
        self.bake.is_baking()
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
    rules: MovementRules,
    hash: u64,
}

impl Snapshot {
    fn job(&self) -> Job {
        Job { snapshot: self.clone(), world: None, faces: Vec::new(), cells: None, found: None, links: Vec::new(), graph: None }
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

    fn snapshot(&self) -> Snapshot {
        let volumes = self.volumes.volumes();
        // Props are rebuilt every tick without change detection, so they are compared instead.
        let props = self.collision.props.clone();
        let triggers: Vec<TriggerVolume> = self.triggers.iter().copied().collect();
        let mut spawns: Vec<Vec3> = self.spawns.iter().map(|placed| placed.translation).collect();
        spawns.sort_by(|a, b| a.to_array().partial_cmp(&b.to_array()).unwrap_or(std::cmp::Ordering::Equal));
        let rules = *self.rules;
        // Debug writes an `f32` as its shortest exact decimal, so equal inputs hash equal.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{volumes:?}{props:?}{triggers:?}{spawns:?}{rules:?}").hash(&mut hasher);
        Snapshot { volumes, props, triggers, spawns, rules, hash: hasher.finish() }
    }
}

/// Items: the surface, the cells, a batch of links per [`LINK_BATCH`] cells, then pruning to what
/// a spawn can reach. Until the cells are found their number is not known, so one batch is
/// assumed and `scheduled` grows once.
struct Job {
    snapshot: Snapshot,
    world: Option<CollisionWorld>,
    faces: Vec<Face>,
    cells: Option<(Vec<Cell>, std::collections::HashMap<IVec2, Vec<CellId>>)>,
    /// How many cells were found, kept once they are handed to the graph.
    found: Option<usize>,
    links: Vec<Vec<Link>>,
    graph: Option<NavGraph>,
}

impl BakeJob for Job {
    type Output = NavGraph;

    fn progress(&self) -> (usize, usize) {
        let batches = self.found.map_or(1, |found| found.div_ceil(LINK_BATCH));
        let linked = if self.graph.is_some() { batches } else { self.links.len().div_ceil(LINK_BATCH) };
        let completed = usize::from(self.world.is_some()) + usize::from(self.found.is_some()) + linked + usize::from(self.graph.is_some());
        (completed, 3 + batches)
    }

    fn doing(&self) -> String {
        match (&self.world, &self.cells) {
            (None, _) => get!("bakes.doing.surface"),
            (Some(_), None) if self.found.is_none() => get!("bakes.doing.floors"),
            (Some(_), Some((cells, _))) if self.links.len() < cells.len() => get!("bakes.doing.links"),
            _ => get!("bakes.doing.reach"),
        }
    }

    fn step(&mut self) {
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
        let (cells, _) = self.cells.take().expect("found above");
        self.graph = Some(NavGraph::reachable(cells, std::mem::take(&mut self.links), &self.snapshot.spawns));
    }

    fn finish(self) -> NavGraph {
        let graph = self.graph.unwrap_or_default();
        info!("Baked the nav graph: {} cell(s)", graph.cells().len());
        graph
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
        state.replace(NavGraph::default());
        state.bake.forget();
        info!("Cleared the nav graph");
    }
    if requests.read().count() > 0 {
        state.bake.ask();
    }
    if let Some(graph) = state.bake.poll() {
        state.replace(graph);
    }
    let props_moved = state.snapshot.as_ref().is_some_and(|seen| seen.props != inputs.collision.props);
    if inputs.changed_since_last_frame() || props_moved || state.snapshot.is_none() {
        state.snapshot = Some(inputs.snapshot());
    }
    let snapshot = state.snapshot.as_ref().expect("taken above");
    state.bake.update(snapshot.hash, true, time.elapsed_secs(), || snapshot.job());
}

/// A round never starts on a stale graph: bots will walk it.
fn bake_nav_for_the_round(mut state: ResMut<NavBake>, inputs: NavInputs) {
    let snapshot = inputs.snapshot();
    if let Some(graph) = state.bake.for_the_round(snapshot.hash, || snapshot.job()) {
        state.replace(graph);
    }
    state.snapshot = Some(snapshot);
}

fn report_nav(state: Res<NavBake>, mut progress: ResMut<BakeProgress>) {
    bake_job::report(&mut progress, BakeKind::Nav, state.bake.status());
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
        Snapshot { volumes, props: Vec::new(), triggers: Vec::new(), spawns: Vec::new(), rules: MovementRules::default(), hash: 0 }
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
        assert!(!job.finish().cells().is_empty());
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
        app.world_mut().resource_mut::<NavBake>().set_auto(false);
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
}
