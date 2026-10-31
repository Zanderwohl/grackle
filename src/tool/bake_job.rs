//! What every bake reports, so a panel can draw any of them without knowing which it is, and
//! the runner the bakes that work in the background share.
//!
//! A background bake is a [`BakeJob`] stepped one item at a time on the async compute pool. On
//! native it runs straight through and sends its progress back every [`CHUNK`]; on wasm, where the
//! pool runs a whole task in one go on the main thread, it gives itself up after [`CHUNK`] and is
//! sent out again next frame, which is what spreads it across frames.

use std::collections::BTreeMap;

use bevy::ecs::system::SystemParam;
use bevy::platform::time::Instant;
use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::tasks::block_on;
use bevy::tasks::{futures::check_ready, AsyncComputeTaskPool, Task};

use crate::editor::editable::{FeatureId, FeatureTag, FeatureTimeline};
use crate::editor::mirror::MirrorCopy;
use crate::get;

/// How often a native bake reports its progress, and how long a wasm chunk runs: frame time.
#[cfg(not(target_arch = "wasm32"))]
const CHUNK: std::time::Duration = std::time::Duration::from_millis(50);
#[cfg(target_arch = "wasm32")]
const CHUNK: std::time::Duration = std::time::Duration::from_millis(8);

/// How long the map has to be left alone before a background bake starts, so a drag is one bake.
pub const SETTLE_SECONDS: f32 = 0.5;

/// In the order a panel lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BakeKind {
    Rooms,
    Sunlight,
    Nav,
    Sight,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BakeState {
    #[default]
    UpToDate,
    /// Out of date, with auto-bake off and nobody asking.
    Stale,
    /// Will bake once the map has been still long enough and what it reads has settled.
    Waiting,
    Baking,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BakeStatus {
    pub state: BakeState,
    pub auto: bool,
    /// Of the bake running, or else of the last one; equal to `scheduled` once it is done.
    pub completed: usize,
    pub scheduled: usize,
    /// For display. Empty when idle.
    pub doing: String,
}

impl BakeStatus {
    /// A bake of nothing is done.
    pub fn fraction(&self) -> f32 {
        if self.scheduled == 0 { 1.0 } else { self.completed as f32 / self.scheduled as f32 }
    }
}

/// Every bake's latest [`BakeStatus`], written by the bake itself each frame.
#[derive(Resource, Default)]
pub struct BakeProgress(BTreeMap<BakeKind, BakeStatus>);

impl BakeProgress {
    pub fn get(&self, kind: BakeKind) -> Option<&BakeStatus> {
        self.0.get(&kind)
    }

    pub fn iter(&self) -> impl Iterator<Item = (BakeKind, &BakeStatus)> {
        self.0.iter().map(|(kind, status)| (*kind, status))
    }
}

/// Marks the resource changed only when the status did, so a panel can redraw on change.
pub fn report(progress: &mut ResMut<BakeProgress>, kind: BakeKind, status: BakeStatus) {
    if progress.0.get(&kind) != Some(&status) {
        progress.0.insert(kind, status);
    }
}

/// Everything a background bake reads, copied out of the world, done an item at a time.
pub trait BakeJob: Send + 'static {
    type Output: Send + 'static;

    /// Items done, and items in all.
    fn progress(&self) -> (usize, usize);

    /// The next item, for display.
    fn doing(&self) -> String;

    /// Never called once every item is done.
    fn step(&mut self);

    fn finish(self) -> Self::Output;

    fn is_done(&self) -> bool {
        let (completed, scheduled) = self.progress();
        completed >= scheduled
    }
}

/// The job's output, and how many items it was.
fn run_to_end<J: BakeJob>(mut job: J) -> (J::Output, usize) {
    while !job.is_done() {
        job.step();
    }
    let items = job.progress().1;
    (job.finish(), items)
}

struct Progress {
    completed: usize,
    scheduled: usize,
    doing: String,
}

impl Progress {
    fn of(job: &impl BakeJob) -> Self {
        let (completed, scheduled) = job.progress();
        Progress { completed, scheduled, doing: job.doing() }
    }
}

struct Running<J> {
    hash: u64,
    task: Task<J>,
    #[cfg(not(target_arch = "wasm32"))]
    progress: crossbeam_channel::Receiver<Progress>,
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn<J: BakeJob>(hash: u64, mut job: J) -> Running<J> {
    let (send, progress) = crossbeam_channel::unbounded();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let mut reported = Instant::now();
        while !job.is_done() {
            job.step();
            if reported.elapsed() >= CHUNK {
                // Nobody listening means the bake was dropped, and so is this task.
                let _ = send.send(Progress::of(&job));
                reported = Instant::now();
            }
        }
        job
    });
    Running { hash, task, progress }
}

#[cfg(target_arch = "wasm32")]
fn spawn<J: BakeJob>(hash: u64, mut job: J) -> Running<J> {
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let started = Instant::now();
        while !job.is_done() && started.elapsed() < CHUNK {
            job.step();
        }
        job
    });
    Running { hash, task }
}

/// A bake of the whole map in the background, keyed by a hash of what it reads. One runs at a
/// time; a change made while it runs is baked after it lands. Keeping the last result up until
/// then is the owner's job.
pub struct BackgroundBake<J: BakeJob> {
    auto: bool,
    /// What the last bake to land was of.
    baked: Option<u64>,
    /// What the map is now, as last told to [`BackgroundBake::update`].
    current: Option<u64>,
    /// A change not yet baked, and when it was first seen.
    pending: Option<(u64, f32)>,
    running: Option<Running<J>>,
    /// A bake asked for whatever auto-bake is doing.
    asked: bool,
    completed: usize,
    scheduled: usize,
    doing: String,
}

impl<J: BakeJob> BackgroundBake<J> {
    pub fn new(auto: bool) -> Self {
        Self {
            auto,
            baked: None,
            current: None,
            pending: None,
            running: None,
            asked: false,
            completed: 0,
            scheduled: 0,
            doing: String::new(),
        }
    }

    pub fn auto(&self) -> bool {
        self.auto
    }

    pub fn set_auto(&mut self, auto: bool) {
        self.auto = auto;
    }

    pub fn is_baking(&self) -> bool {
        self.running.is_some()
    }

    /// Whether a change would be baked, and so whether [`BackgroundBake::update`] needs calling.
    pub fn would_bake(&self) -> bool {
        self.auto || self.asked
    }

    /// Bake now, without waiting for the map to settle, whatever auto-bake is doing.
    pub fn ask(&mut self) {
        self.asked = true;
    }

    /// What the last bake to land was of.
    pub fn baked(&self) -> Option<u64> {
        self.baked
    }

    /// Takes `hash` as baked, by some other means than a bake: read back from a cache.
    pub fn adopt(&mut self, hash: u64) {
        self.baked = Some(hash);
        self.current = Some(hash);
        self.pending = None;
        self.asked = false;
    }

    /// Forgets what was baked and drops any bake running.
    pub fn forget(&mut self) {
        *self = Self::new(self.auto);
    }

    pub fn status(&self) -> BakeStatus {
        let state = if self.running.is_some() {
            BakeState::Baking
        } else if self.current.is_none() || self.baked == self.current {
            BakeState::UpToDate
        } else if self.auto || self.asked {
            BakeState::Waiting
        } else {
            BakeState::Stale
        };
        BakeStatus {
            state,
            auto: self.auto,
            completed: self.completed,
            scheduled: self.scheduled,
            doing: if self.running.is_some() { self.doing.clone() } else { String::new() },
        }
    }

    fn track(&mut self, progress: Progress) {
        (self.completed, self.scheduled, self.doing) = (progress.completed, progress.scheduled, progress.doing);
    }

    /// Reads the bake's progress and returns its output once it finishes. On wasm, sends an
    /// unfinished chunk out again.
    pub fn poll(&mut self) -> Option<J::Output> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(progress) = self.running.as_ref()?.progress.try_iter().last() {
            self.track(progress);
        }
        let running = self.running.as_mut()?;
        let job = check_ready(&mut running.task)?;
        let hash = running.hash;
        self.track(Progress::of(&job));
        if job.is_done() {
            self.running = None;
            self.baked = Some(hash);
            return Some(job.finish());
        }
        self.running = Some(spawn(hash, job));
        None
    }

    /// Told every frame what the map is now. `ready` is false while what the bake reads is still
    /// being rebuilt; `job` makes the bake, and is only called to start one.
    pub fn update(&mut self, hash: u64, ready: bool, now: f32, job: impl FnOnce() -> J) {
        self.current = Some(hash);
        // A press is answered by a bake of the map as it stands, which one already landed may be.
        if self.baked == Some(hash) {
            self.pending = None;
            self.asked = false;
            return;
        }
        // Seen while a bake runs too, so the map's stillness is counted from the change.
        let since = match self.pending {
            Some((pending, since)) if pending == hash => since,
            _ => {
                self.pending = Some((hash, now));
                now
            }
        };
        if !(self.auto || self.asked) || !ready || self.running.is_some() {
            return;
        }
        if !self.asked && now - since < SETTLE_SECONDS {
            return;
        }
        self.asked = false;
        self.pending = None;
        let job = job();
        self.track(Progress::of(&job));
        self.running = Some(spawn(hash, job));
    }

    /// Finishes a bake of `hash` now, or returns `None` if it is already baked. A bake of it
    /// already running is waited for rather than begun again.
    pub fn for_the_round(&mut self, hash: u64, job: impl FnOnce() -> J) -> Option<J::Output> {
        self.current = Some(hash);
        if self.baked == Some(hash) {
            return None;
        }
        let (output, items) = match self.running.take() {
            // On wasm the pool runs its tasks on this thread, so waiting for one would never end.
            #[cfg(not(target_arch = "wasm32"))]
            Some(running) if running.hash == hash => run_to_end(block_on(running.task)),
            _ => run_to_end(job()),
        };
        self.pending = None;
        self.asked = false;
        self.baked = Some(hash);
        (self.completed, self.scheduled) = (items, items);
        Some(output)
    }
}

/// What a room or block is called, for a bake's [`BakeStatus::doing`]: its feature's label, or the
/// label of what it is a Mirror's copy of.
#[derive(SystemParam)]
pub struct VolumeLabels<'w, 's> {
    timeline: Option<Res<'w, FeatureTimeline>>,
    tags: Query<'w, 's, &'static FeatureTag>,
    copies: Query<'w, 's, &'static MirrorCopy>,
    parents: Query<'w, 's, &'static ChildOf>,
}

impl VolumeLabels<'_, '_> {
    pub fn label(&self, entity: Entity) -> String {
        let feature = |id: FeatureId| {
            self.timeline.as_deref().and_then(|timeline| timeline.get_feature(&id)).map(|feature| feature.label())
        };
        let mut at = Some(entity);
        while let Some(here) = at {
            if let Ok(copy) = self.copies.get(here) {
                if let Some(name) = feature(copy.source) {
                    return get!("bakes.doing.copy", "name", name, "image", copy.image + 1);
                }
            }
            if let Some(name) = self.tags.get(here).ok().and_then(|tag| feature(FeatureId::from_raw(tag.feature_id))) {
                return name;
            }
            at = self.parents.get(here).ok().map(ChildOf::parent);
        }
        get!("bakes.doing.unnamed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts to `items`, a millisecond an item.
    struct Count {
        done: usize,
        items: usize,
    }

    impl BakeJob for Count {
        type Output = usize;

        fn progress(&self) -> (usize, usize) {
            (self.done, self.items)
        }

        fn doing(&self) -> String {
            format!("item {}", self.done)
        }

        fn step(&mut self) {
            std::thread::sleep(std::time::Duration::from_millis(1));
            self.done += 1;
        }

        fn finish(self) -> usize {
            self.done
        }
    }

    fn pool() {
        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    }

    fn count(items: usize) -> impl FnOnce() -> Count {
        move || Count { done: 0, items }
    }

    /// With every status seen on the way.
    fn until_landed(bake: &mut BackgroundBake<Count>) -> (usize, Vec<BakeStatus>) {
        let started = std::time::Instant::now();
        let mut seen = Vec::new();
        loop {
            assert!(started.elapsed().as_secs() < 10, "the bake never landed");
            if let Some(output) = bake.poll() {
                return (output, seen);
            }
            seen.push(bake.status());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_change_waits_for_the_map_to_settle_then_bakes() {
        pool();
        let mut bake = BackgroundBake::new(true);
        bake.update(1, true, 0.0, count(3));
        assert_eq!(bake.status().state, BakeState::Waiting);
        bake.update(1, true, SETTLE_SECONDS, count(3));
        assert_eq!(bake.status().state, BakeState::Baking);
        let (output, _) = until_landed(&mut bake);
        assert_eq!(output, 3);
        bake.update(1, true, 10.0, || panic!("an unchanged map was baked again"));
        assert_eq!(bake.status().state, BakeState::UpToDate);
    }

    #[test]
    fn a_long_bake_reports_its_progress_between_chunks() {
        pool();
        let items = 3 * CHUNK.as_millis() as usize;
        let mut bake = BackgroundBake::new(false);
        bake.ask();
        bake.update(1, true, 0.0, count(items));
        let (_, seen) = until_landed(&mut bake);
        let partway: Vec<&BakeStatus> = seen.iter().filter(|s| s.completed > 0 && s.completed < items).collect();
        assert!(!partway.is_empty(), "no progress was seen before the end");
        assert!(partway.iter().all(|s| s.scheduled == items && s.state == BakeState::Baking && !s.doing.is_empty()));
        let done = bake.status();
        assert_eq!((done.completed, done.scheduled, done.fraction()), (items, items, 1.0));
        assert!(done.doing.is_empty());
    }

    #[test]
    fn with_auto_off_a_change_is_stale_until_asked() {
        pool();
        let mut bake = BackgroundBake::new(false);
        bake.update(1, true, 0.0, count(1));
        assert_eq!(bake.status().state, BakeState::Stale);
        bake.update(1, true, 10.0, count(1));
        assert_eq!(bake.status().state, BakeState::Stale, "a stale map baked without being asked");
        bake.ask();
        bake.update(1, true, 10.0, count(1));
        assert_eq!(bake.status().state, BakeState::Baking, "asking does not wait for the map to settle");
    }

    #[test]
    fn nothing_starts_while_what_it_reads_is_unsettled() {
        pool();
        let mut bake = BackgroundBake::<Count>::new(true);
        bake.ask();
        bake.update(1, false, 10.0, || panic!("baked an unsettled map"));
        assert_eq!(bake.status().state, BakeState::Waiting);
    }

    #[test]
    fn a_round_waits_for_the_bake_of_its_map_and_reuses_it() {
        pool();
        let mut bake = BackgroundBake::new(false);
        bake.ask();
        bake.update(1, true, 0.0, count(20));
        assert_eq!(bake.for_the_round(1, || panic!("started a second bake of the same map")), Some(20));
        assert_eq!(bake.for_the_round(1, || panic!("baked an unchanged map")), None);
        assert_eq!(bake.for_the_round(2, count(4)), Some(4));
        assert_eq!(bake.status().state, BakeState::UpToDate);
    }

    #[test]
    fn forgetting_drops_a_bake_and_keeps_auto() {
        pool();
        let mut bake = BackgroundBake::new(true);
        bake.ask();
        bake.update(1, true, 0.0, count(1000));
        bake.forget();
        assert!(!bake.is_baking());
        assert!(bake.auto());
        assert_eq!(bake.for_the_round(1, count(1)), Some(1), "forgetting kept what was baked");
    }
}
