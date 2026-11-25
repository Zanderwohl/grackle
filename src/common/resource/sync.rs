//! A server's files reaching its clients: the server publishes a [`Manifest`] of what the match
//! needs, each client compares it with its own packs and store and asks for what it lacks, and the
//! files come back in chunks. Once every file is stored the client mounts the manifest as the
//! session pack and every loader reads again ([`PacksChanged`]). See
//! documentation/resource-streaming.md, stage 3.
//!
//! Nothing a client sends carries content: a [`Want`] names hashes, and only hashes in the
//! manifest the server last sent are answered.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::assets::{default_packs, Assets as PackAssets, PacksChanged};
use crate::common::net::NetRole;
use crate::common::resource::closure::{closure, match_roots};
use crate::common::resource::session::SessionPack;
use crate::common::resource::store::ObjectStore;
use crate::common::resource::{ContentHash, LogicalPath, Manifest, ManifestEntry, Named, ResourceKey, ResourceKind, MAX_MANIFEST_ENTRIES};
use crate::editor::editable::FeatureTimeline;
use crate::editor::map_metadata::MapMetadata;

/// Ordered and reliable, so a file's chunks arrive in order.
pub struct ResourceChannel;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ManifestMessage {
    pub manifest: Manifest,
    /// Bumped by the server whenever the manifest changes.
    pub generation: u64,
    /// The server's listed name, or its host's player name; cut to [`MAX_SERVER_NAME`] characters
    /// on arrival.
    pub server: String,
}

pub const MAX_SERVER_NAME: usize = 64;

fn server_name(listing: Option<&crate::common::round_settings::ServerListing>, player: Option<&crate::common::player_settings::PlayerSettings>) -> String {
    match listing.map(|listing| listing.name.trim()).unwrap_or_default() {
        "" => player.map(|player| player.name.trim().to_owned()).unwrap_or_default(),
        name => name.to_owned(),
    }
}

/// Answered only for hashes in the server's current manifest.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Want {
    pub hashes: Vec<ContentHash>,
}

/// The bytes of chunks a client took this frame, which frees that much of its [`WINDOW_BYTES`].
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Received {
    pub bytes: u64,
}

/// Part of one file, in order from offset 0.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Chunk {
    pub hash: ContentHash,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

const CHUNK_BYTES: usize = 32 * 1024;
/// Sent per frame to all clients together. Every client is sent from one socket, and a burst
/// larger than the system will queue is dropped (`ENOBUFS` on macOS) and sent again.
const BYTES_PER_FRAME: usize = 128 * 1024;
/// Sent to one client and not yet reported [`Received`]. A client that stops reading, as while it
/// bakes, is sent no more than this, and Lightyear resends no more than this to it.
const WINDOW_BYTES: u64 = 256 * 1024;
/// Before Lightyear sends an unacknowledged chunk again. Its own delay is 1.5 round trips, a few
/// milliseconds on a LAN and shorter than a client takes to acknowledge, so every chunk would go
/// out twice.
const RESEND_AFTER: std::time::Duration = std::time::Duration::from_millis(100);
/// Seconds between manifest rebuilds at least: a drag changes the map every frame.
const REBUILD_INTERVAL: f32 = 0.25;
/// After which a file that keeps arriving damaged is given up on, and reads as missing.
const RETRIES: u32 = 3;

/// Where what servers sent is kept: `objects/` and `sessions/` under it.
#[derive(Resource, Clone, Debug)]
pub struct DownloadsRoot(pub PathBuf);

impl Default for DownloadsRoot {
    /// Tests keep out of the real folder, in one per process.
    fn default() -> Self {
        match cfg!(test) {
            true => Self(std::env::temp_dir().join(format!("grackle-downloads-{}", std::process::id()))),
            false => Self(PathBuf::from(crate::common::resource::store::DOWNLOADS_ROOT)),
        }
    }
}

impl DownloadsRoot {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn store(&self) -> Option<Box<dyn ObjectStore>> {
        Some(Box::new(crate::common::resource::store::DiskObjects::new(self.0.join("objects"))))
    }

    #[cfg(target_arch = "wasm32")]
    pub fn store(&self) -> Option<Box<dyn ObjectStore>> {
        None
    }
}

pub struct ResourceSyncPlugin;

impl Plugin for ResourceSyncPlugin {
    fn build(&self, app: &mut App) {
        crate::common::assets::ensure_packs_changed(app);
        app.add_channel::<ResourceChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings { rtt_resend_min_delay: RESEND_AFTER, ..default() }),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.register_message::<ManifestMessage>().add_direction(NetworkDirection::ServerToClient);
        app.register_message::<Chunk>().add_direction(NetworkDirection::ServerToClient);
        app.register_message::<Want>().add_direction(NetworkDirection::ClientToServer);
        app.register_message::<Received>().add_direction(NetworkDirection::ClientToServer);

        app.init_resource::<PackAssets>()
            .init_resource::<DownloadsRoot>()
            .init_resource::<Served>()
            .init_resource::<Fetch>()
            .init_resource::<crate::common::resource::lock::LockedMap>()
            .add_message::<crate::common::resource::lock::KeepACopy>()
            .add_systems(Update, (crate::common::resource::lock::lock_kept_maps.after(forget_the_server_when_not_a_client), crate::common::resource::lock::keep_a_copy))
            .add_systems(Startup, crate::common::resource::prune::prune_old_downloads.run_if(resource_exists::<crate::common::resource::prune::PruneDownloads>))
            .add_systems(
                OnEnter(crate::common::app_mode::AppMode::Play),
                remember_the_rounds_map.run_if(crate::common::net::has_authority),
            )
            .add_systems(
                Update,
                (
                    (rebuild_the_manifest, send_the_manifest_to_new_clients, take_wants, take_receipts, send_chunks).chain(),
                    (forget_the_server_when_not_a_client, read_manifests, compare_some, read_chunks, mount_when_everything_is_stored).chain(),
                ),
            );
    }
}

enum Source {
    /// Its highest local pack's copy, checked against the hash again when sent.
    Pack(LogicalPath),
    Bytes(Vec<u8>),
    File(PathBuf),
}

#[derive(Resource, Default)]
struct Served {
    manifest: Manifest,
    generation: u64,
    sources: HashMap<ContentHash, Source>,
    /// What the map named at the last rebuild, which decides whether another is needed.
    named: Vec<Named>,
    saves: u64,
    lightmap: Option<u64>,
    /// The map as the last round started.
    round: Option<(ManifestEntry, Vec<u8>)>,
    since: f32,
    /// The map or its settings changed, so what it names is looked at again.
    pending: bool,
    /// Something changed that calls for a rebuild whatever the map names.
    force: bool,
    /// A sun bake whose cache file was not on disk yet at the last rebuild; its file is written on
    /// the IO pool after the bake lands.
    lightmap_pending: Option<u64>,
    /// The closure's problems at the last rebuild, so each is warned of once.
    problems: Vec<String>,
    hosting: bool,
}

fn map_path(file: Option<&crate::editor::panels::CurrentFilePath>) -> LogicalPath {
    let stem = file.and_then(|file| file.path.as_ref()?.file_stem()?.to_str().map(str::to_owned));
    stem.and_then(|stem| LogicalPath::new(&format!("{}/{stem}.{}", ResourceKind::Blueprint.folder(), crate::constants::MAP_BLUEPRINT_EXTENSION)).ok())
        .unwrap_or_else(|| LogicalPath::new("blueprints/untitled.gmb").expect("a valid path"))
}

fn map_pack(file: Option<&crate::editor::panels::CurrentFilePath>) -> String {
    file.and_then(|file| crate::common::cache_path::in_pack(file.path.as_ref()?))
        .map_or_else(|| crate::common::cache_path::NO_PACK.to_owned(), |(pack, _)| pack)
}

/// The map as it stands as a round starts, unsaved edits included, which every client keeps.
fn remember_the_rounds_map(
    timeline: Option<Res<FeatureTimeline>>,
    metadata: Option<Res<MapMetadata>>,
    file: Option<Res<crate::editor::panels::CurrentFilePath>>,
    mut served: ResMut<Served>,
) {
    let (Some(timeline), Some(metadata)) = (timeline, metadata) else { return };
    let text = match crate::editor::save::to_text(&timeline, &metadata) {
        Ok(text) => text,
        Err(e) => return error!("Could not write the round's map to send it: {e}"),
    };
    let bytes = text.into_bytes();
    let entry = ManifestEntry {
        key: ResourceKey { path: map_path(file.as_deref()), hash: ContentHash::of(&bytes) },
        size: bytes.len() as u64,
        pack: map_pack(file.as_deref()),
        authors: metadata.authors.clone(),
    };
    served.round = Some((entry, bytes));
    served.force = true;
    served.since = REBUILD_INTERVAL;
}

#[cfg(not(target_arch = "wasm32"))]
fn lightmap_entry(hash: u64) -> Option<(ManifestEntry, Source)> {
    let file = crate::tool::bake_cache::file(crate::tool::sun_bake::CACHE_FOLDER, hash);
    let bytes = std::fs::read(&file).ok()?;
    let path = LogicalPath::new(&format!("{}/{hash:016x}.bin", ResourceKind::Lightmap.folder())).ok()?;
    let entry = ManifestEntry {
        key: ResourceKey { path, hash: ContentHash::of(&bytes) },
        size: bytes.len() as u64,
        pack: "cache".into(),
        authors: Vec::new(),
    };
    Some((entry, Source::File(file)))
}

#[cfg(target_arch = "wasm32")]
fn lightmap_entry(_: u64) -> Option<(ManifestEntry, Source)> {
    None
}

/// A stat a frame, rather than the closure, while a lightmap's file is awaited.
#[cfg(not(target_arch = "wasm32"))]
fn lightmap_on_disk(hash: u64) -> bool {
    crate::tool::bake_cache::file(crate::tool::sun_bake::CACHE_FOLDER, hash).is_file()
}

#[cfg(target_arch = "wasm32")]
fn lightmap_on_disk(_: u64) -> bool {
    false
}

#[allow(clippy::too_many_arguments)]
fn rebuild_the_manifest(
    time: Res<Time>,
    assets: Res<PackAssets>,
    timeline: Option<Res<FeatureTimeline>>,
    metadata: Option<Res<MapMetadata>>,
    file: Option<Res<crate::editor::panels::CurrentFilePath>>,
    catalog: Option<Res<crate::common::weapon::WeaponCatalog>>,
    editor: Option<Res<crate::prop::document::PropEditor>>,
    sun: Option<Res<crate::tool::sun_bake::SunBake>>,
    named: (Option<Res<crate::common::round_settings::ServerListing>>, Option<Res<crate::common::player_settings::PlayerSettings>>),
    server: Option<Single<&Endpoint, With<Server>>>,
    mut packs_changed: MessageReader<PacksChanged>,
    mut served: ResMut<Served>,
    mut sender: crate::common::net_transport::ToClients,
) -> Result {
    // Read every frame, so a change while not hosting is not taken for one after.
    let packs_changed = packs_changed.read().count() > 0;
    let was_hosting = std::mem::replace(&mut served.hosting, server.is_some());
    let Some(server) = server else { return Ok(()) };

    let saves = editor.map_or(0, |editor| editor.saves());
    let lightmap = sun.and_then(|sun| sun.baked_hash());
    served.pending |= timeline.as_ref().is_some_and(|timeline| timeline.is_changed()) || metadata.as_ref().is_some_and(|metadata| metadata.is_changed());
    // A kept map's lock mounted or unmounted changes the files served under the same names.
    served.force |= !was_hosting || packs_changed || catalog.as_ref().is_some_and(|catalog| catalog.is_changed()) || saves != served.saves || lightmap != served.lightmap;
    if let Some(hash) = served.lightmap_pending
        && lightmap_on_disk(hash)
    {
        served.lightmap_pending = None;
        served.force = true;
    }
    served.since += time.delta_secs();
    if !(served.pending || served.force) || served.since < REBUILD_INTERVAL {
        return Ok(());
    }
    served.since = 0.0;
    served.pending = false;

    // Reading what the map names is cheap; the closure reads and hashes every file, music
    // included, so it runs only when the names or something else it reads changed.
    let described = match (&timeline, &metadata) {
        (Some(timeline), Some(metadata)) => crate::editor::save::resources_named_by(timeline, metadata),
        _ => Default::default(),
    };
    if !std::mem::take(&mut served.force) && described.named == served.named {
        return Ok(());
    }
    served.named = described.named;
    served.saves = saves;
    served.lightmap = lightmap;

    // The live map is walked for what it names but not sent: it changes at frame rate and travels
    // as `map_sync`'s stream. Only a round's map is a file a client keeps.
    let live = match (&timeline, &metadata) {
        (Some(timeline), Some(metadata)) => crate::editor::save::to_text(timeline, metadata).ok().map(String::into_bytes),
        _ => None,
    };
    let map = map_path(file.as_deref());
    let given = live.map(|bytes| (map.clone(), (bytes, map_pack(file.as_deref())))).into_iter().collect::<BTreeMap<_, _>>();
    let roots = match given.is_empty() {
        true => match_roots(map.clone()).into_iter().skip(1).collect(),
        false => match_roots(map.clone()),
    };
    // `packs()`, not `default_packs()`: it includes a mounted lock, which `read_from_packs` serves.
    let found = closure(&assets, &assets.packs(), &given, roots);
    for problem in found.problems.iter().filter(|problem| !served.problems.contains(problem)) {
        warn!("Left out of what clients are sent: {problem}");
    }
    served.problems = found.problems.clone();
    for missing in &found.missing {
        debug!("{} is named but no pack has it", missing.path);
    }

    let mut entries = Vec::new();
    let mut sources = HashMap::new();
    let host = server_name(None, named.1.as_deref());
    for mut entry in found.manifest.entries {
        if entry.authors.is_empty() && !host.is_empty() {
            entry.authors = vec![host.clone()];
        }
        if entry.key.kind() == ResourceKind::Blueprint {
            continue;
        }
        sources.insert(entry.key.hash, Source::Pack(entry.key.path.clone()));
        entries.push(entry);
    }
    if let Some((entry, bytes)) = &served.round {
        sources.insert(entry.key.hash, Source::Bytes(bytes.clone()));
        entries.push(entry.clone());
    }
    served.lightmap_pending = None;
    match lightmap.map(|hash| (hash, lightmap_entry(hash))) {
        Some((_, Some((entry, source)))) => {
            sources.insert(entry.key.hash, source);
            entries.push(entry);
        }
        Some((hash, None)) => served.lightmap_pending = Some(hash),
        None => {}
    }
    let manifest = match Manifest::checked(entries) {
        Ok(manifest) => manifest,
        Err(why) => return Ok(error!("The manifest for clients is refused: {why}")),
    };
    served.sources = sources;
    if manifest == served.manifest {
        return Ok(());
    }
    served.generation += 1;
    served.manifest = manifest;
    info!("Clients need {} files, {} bytes (manifest {})", served.manifest.entries.len(), served.manifest.total_bytes(), served.generation);
    let server_name = server_name(named.0.as_deref(), named.1.as_deref());
    let message = ManifestMessage { manifest: served.manifest.clone(), generation: served.generation, server: server_name };
    sender.send::<_, ResourceChannel>(&message, &server, &NetworkTarget::All)?;
    Ok(())
}

fn send_the_manifest_to_new_clients(
    served: Res<Served>,
    listing: Option<Res<crate::common::round_settings::ServerListing>>,
    player: Option<Res<crate::common::player_settings::PlayerSettings>>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: crate::common::net_transport::ToClients,
) -> Result {
    if joined.is_empty() || served.generation == 0 {
        return Ok(());
    }
    let server = server_name(listing.as_deref(), player.as_deref());
    let message = ManifestMessage { manifest: served.manifest.clone(), generation: served.generation, server };
    for client in &joined {
        sender.send_to_entities::<_, ResourceChannel>(&message, core::iter::once(client))?;
    }
    Ok(())
}

#[derive(Component, Default)]
struct Outbox {
    queue: VecDeque<ContentHash>,
    current: Option<(ContentHash, Vec<u8>, usize)>,
    /// Bytes sent and not yet reported [`Received`].
    in_flight: u64,
    /// So a send that keeps failing, or a client that keeps reporting more than it was sent, is
    /// warned about once.
    send_failing: bool,
    overreported: bool,
}

impl Outbox {
    fn has_work(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty()
    }
}

fn take_receipts(mut links: Query<(&mut MessageReceiver<Received>, &mut Outbox), With<ClientOf>>) {
    for (mut receiver, mut outbox) in &mut links {
        for received in receiver.receive() {
            if received.bytes > outbox.in_flight && !outbox.overreported {
                warn!("A client reported {} bytes received with {} in flight", received.bytes, outbox.in_flight);
                outbox.overreported = true;
            }
            outbox.in_flight = outbox.in_flight.saturating_sub(received.bytes);
        }
    }
}

fn take_wants(
    mut commands: Commands,
    served: Res<Served>,
    mut links: Query<(Entity, &mut MessageReceiver<Want>, Option<&mut Outbox>), With<ClientOf>>,
) {
    for (link, mut receiver, outbox) in &mut links {
        let mut asked = Vec::new();
        for want in receiver.receive() {
            asked.extend(want.hashes.into_iter().take(MAX_MANIFEST_ENTRIES));
        }
        if asked.is_empty() {
            continue;
        }
        let mut fresh = Outbox::default();
        let outbox = match outbox {
            Some(outbox) => outbox.into_inner(),
            None => &mut fresh,
        };
        for hash in asked {
            let queued = outbox.queue.contains(&hash) || outbox.current.as_ref().is_some_and(|(current, _, _)| *current == hash);
            match served.sources.contains_key(&hash) {
                true if !queued => outbox.queue.push_back(hash),
                true => {}
                false => debug!("A client asked for {}, which is not in the manifest", hash.short()),
            }
        }
        if !fresh.queue.is_empty() {
            commands.entity(link).insert(fresh);
        }
    }
}

fn read_source(assets: &PackAssets, source: &Source) -> Option<Vec<u8>> {
    match source {
        Source::Pack(path) => assets.read_from_packs(path.as_str()),
        Source::Bytes(bytes) => Some(bytes.clone()),
        Source::File(file) => std::fs::read(file).ok(),
    }
}

/// Clients take turns a chunk at a time, starting a place further along each frame, until the
/// frame's budget is spent or every client is out of work or window.
fn send_chunks(
    assets: Res<PackAssets>,
    mut served: ResMut<Served>,
    mut links: Query<(Entity, &mut Outbox), With<ClientOf>>,
    mut sender: crate::common::net_transport::ToClients,
    mut turn: Local<usize>,
) -> Result {
    let mut order: Vec<Entity> = links.iter().filter(|(_, outbox)| outbox.has_work()).map(|(link, _)| link).collect();
    if order.is_empty() {
        return Ok(());
    }
    *turn = turn.wrapping_add(1);
    let start = *turn % order.len();
    order.rotate_left(start);
    let mut budget = BYTES_PER_FRAME;
    while budget > 0 && !order.is_empty() {
        order.retain(|link| {
            if budget == 0 {
                return true;
            }
            let Ok((_, mut outbox)) = links.get_mut(*link) else { return false };
            match next_chunk(&assets, &mut served, &mut outbox, budget) {
                Some(chunk) => {
                    let sent = chunk.bytes.len();
                    if let Err(e) = sender.send_to_entities::<_, ResourceChannel>(&chunk, core::iter::once(*link)) {
                        if !outbox.send_failing {
                            warn!("Could not send a piece of {} to a client: {e}", chunk.hash.short());
                            outbox.send_failing = true;
                        }
                        return false;
                    }
                    outbox.send_failing = false;
                    sent_from(&mut outbox, sent);
                    // At least one, so empty files still cost something and the loop ends.
                    budget = budget.saturating_sub(sent.max(1));
                    true
                }
                None => false,
            }
        });
    }
    Ok(())
}

/// Moves past a piece once it is sent, so one that failed to send is sent again.
fn sent_from(outbox: &mut Outbox, sent: usize) {
    outbox.in_flight += sent as u64;
    let Some((_, bytes, offset)) = outbox.current.as_mut() else { return };
    *offset += sent;
    if *offset >= bytes.len() {
        outbox.current = None;
    }
}

/// The next piece of what the client asked for, at most `limit` bytes, if it has window left.
/// Takes nothing off the outbox until [`sent_from`].
fn next_chunk(assets: &PackAssets, served: &mut Served, outbox: &mut Outbox, limit: usize) -> Option<Chunk> {
    let room = WINDOW_BYTES.saturating_sub(outbox.in_flight);
    if room == 0 {
        return None;
    }
    while outbox.current.is_none() {
        let hash = outbox.queue.pop_front()?;
        let Some(bytes) = served.sources.get(&hash).and_then(|source| read_source(assets, source)) else {
            warn!("{} is in the manifest but could not be read to send", hash.short());
            continue;
        };
        if ContentHash::of(&bytes) != hash {
            // Changed on disk since the manifest was built; the next one names the new bytes.
            served.force = true;
            continue;
        }
        outbox.current = Some((hash, bytes, 0));
    }
    let (hash, bytes, offset) = outbox.current.as_ref()?;
    let end = (*offset + CHUNK_BYTES.min(room as usize).min(limit)).min(bytes.len());
    Some(Chunk { hash: *hash, offset: *offset as u64, bytes: bytes[*offset..end].to_vec() })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FetchPhase {
    /// No manifest yet.
    #[default]
    Idle,
    /// A few files a frame.
    Comparing,
    Downloading,
    /// Every file is stored and the server's pack is mounted.
    Mounted,
}

/// Local bytes read and hashed per frame while comparing, so a manifest naming music does not
/// stall a frame.
const COMPARE_BYTES_PER_FRAME: u64 = 8 * 1024 * 1024;

/// What a client is fetching from its server, for the join screen.
#[derive(Resource, Default)]
pub struct Fetch {
    pub phase: FetchPhase,
    pub server: String,
    pub entries: usize,
    pub entries_checked: usize,
    pub bytes_wanted: u64,
    pub bytes_received: u64,
    pub files_wanted: usize,
    pub current: Option<LogicalPath>,
    manifest: Option<Manifest>,
    comparing: VecDeque<ManifestEntry>,
    generation: u64,
    /// By hash: its size, and a path it is listed at, for messages and its extension.
    wanted: BTreeMap<ContentHash, (u64, LogicalPath)>,
    /// What a comparison under way has found lacking, swapped in for `wanted` when it ends, so
    /// files still downloading keep being taken meanwhile.
    next_wanted: BTreeMap<ContentHash, (u64, LogicalPath)>,
    asked: BTreeSet<ContentHash>,
    /// How many times each file has been asked for again after arriving damaged.
    retries: HashMap<ContentHash, u32>,
    partial: HashMap<ContentHash, Vec<u8>>,
    mounted: Option<u64>,
    record: Option<SessionRecord>,
}

impl Fetch {
    pub fn mounted_generation(&self) -> Option<u64> {
        self.mounted
    }

    pub fn files_received(&self) -> usize {
        self.files_wanted.saturating_sub(self.wanted.len())
    }

    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
    }
}

/// One connection's record in `downloads/sessions/`: which map each round played, by hash.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub(crate) struct SessionRecord {
    server: String,
    started: String,
    rounds: Vec<RoundRecord>,
    /// Every object the session used, by its name in the store; what keeps one from being pruned.
    #[serde(default)]
    pub(crate) objects: BTreeSet<String>,
    #[serde(skip)]
    written: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct RoundRecord {
    seen: String,
    map: String,
    hash: String,
}

fn label(role: &NetRole) -> String {
    match role {
        NetRole::Client { host, port } => format!("{host}:{port}"),
        _ => "this server".to_owned(),
    }
}

fn forget_the_server_when_not_a_client(
    role: Res<NetRole>,
    mut assets: ResMut<PackAssets>,
    mut fetch: ResMut<Fetch>,
    cache: Option<ResMut<crate::prop::baked::PropCache>>,
    mut changed: MessageWriter<PacksChanged>,
) {
    if !role.is_changed() {
        return;
    }
    *fetch = Fetch::default();
    if assets.unmount().is_some() {
        info!("The server's files are unmounted");
        if let Some(mut cache) = cache {
            cache.forget_all();
        }
        changed.write(PacksChanged::ALL);
    }
}

/// Only a byte-identical copy.
fn local_copy(assets: &PackAssets, entry: &ManifestEntry) -> Option<Vec<u8>> {
    for pack in default_packs().iter().rev() {
        if assets.size_local(pack, entry.key.path.as_str()).is_some_and(|size| size != entry.size) {
            continue;
        }
        let Ok(bytes) = assets.read_local(pack, entry.key.path.as_str()) else { continue };
        if ContentHash::of(&bytes) == entry.key.hash {
            return Some(bytes);
        }
    }
    None
}

fn read_manifests(
    role: Res<NetRole>,
    mut fetch: ResMut<Fetch>,
    mut receivers: Query<&mut MessageReceiver<ManifestMessage>, With<Client>>,
) {
    if role.is_authority() {
        return;
    }
    let Some(message) = receivers.iter_mut().flat_map(|mut receiver| receiver.receive().collect::<Vec<_>>()).last() else { return };
    info!("The server's manifest {} lists {} files", message.generation, message.manifest.entries.len());
    fetch.server = message.server.chars().take(MAX_SERVER_NAME).collect();
    fetch.entries = message.manifest.entries.len();
    fetch.entries_checked = 0;
    fetch.next_wanted.clear();
    fetch.comparing = message.manifest.entries.iter().cloned().collect();
    fetch.manifest = Some(message.manifest);
    fetch.generation = message.generation;
    fetch.phase = FetchPhase::Comparing;
}

fn compare_some(
    role: Res<NetRole>,
    assets: Res<PackAssets>,
    root: Res<DownloadsRoot>,
    mut fetch: ResMut<Fetch>,
    mut senders: Query<&mut MessageSender<Want>, (With<Client>, With<Connected>)>,
) {
    if role.is_authority() || fetch.phase != FetchPhase::Comparing {
        return;
    }
    let Some(store) = root.store() else { return };
    let Fetch { comparing, next_wanted: wanted, entries_checked, .. } = &mut *fetch;
    let mut read = 0;
    while read < COMPARE_BYTES_PER_FRAME {
        let Some(entry) = comparing.pop_front() else { break };
        *entries_checked += 1;
        let extension = entry.key.path.extension().unwrap_or_default();
        if store.has(entry.key.hash, extension) || wanted.contains_key(&entry.key.hash) {
            continue;
        }
        read += entry.size;
        if let Some(bytes) = local_copy(&assets, &entry)
            && store.put(entry.key.hash, extension, &bytes).is_ok()
        {
            continue;
        }
        wanted.insert(entry.key.hash, (entry.size, entry.key.path.clone()));
    }
    if !fetch.comparing.is_empty() {
        return;
    }

    // A file that finished downloading while it was compared is stored by now.
    let mut wanted = std::mem::take(&mut fetch.next_wanted);
    wanted.retain(|hash, (_, path)| !store.has(*hash, path.extension().unwrap_or_default()));
    fetch.partial.retain(|hash, _| wanted.contains_key(hash));
    fetch.asked.retain(|hash| wanted.contains_key(hash));
    let ask: Vec<ContentHash> = wanted.keys().filter(|hash| !fetch.asked.contains(*hash)).copied().collect();
    info!("{} of the server's files to fetch, {} bytes", wanted.len(), wanted.values().map(|(size, _)| size).sum::<u64>());
    if !ask.is_empty() {
        for mut sender in &mut senders {
            sender.send::<ResourceChannel>(Want { hashes: ask.clone() });
        }
        fetch.asked.extend(ask);
    }
    fetch.bytes_wanted = wanted.values().map(|(size, _)| size).sum();
    fetch.bytes_received = fetch.partial.values().map(|bytes| bytes.len() as u64).sum();
    fetch.files_wanted = wanted.len();
    fetch.wanted = wanted;
    fetch.phase = FetchPhase::Downloading;
}

fn read_chunks(
    role: Res<NetRole>,
    root: Res<DownloadsRoot>,
    mut fetch: ResMut<Fetch>,
    mut receivers: Query<&mut MessageReceiver<Chunk>, With<Client>>,
    mut senders: Query<(&mut MessageSender<Want>, &mut MessageSender<Received>), (With<Client>, With<Connected>)>,
) {
    if role.is_authority() {
        return;
    }
    // Without a store nothing is kept, but every chunk is still reported, or the server stops.
    let store = root.store();
    let mut taken = 0u64;
    for mut receiver in &mut receivers {
        for chunk in receiver.receive() {
            // Every chunk counts, wanted or not, since each was sent against our window.
            taken += chunk.bytes.len() as u64;
            let Some(store) = store.as_ref() else { continue };
            let Some((size, path)) = fetch.wanted.get(&chunk.hash).cloned() else { continue };
            let partial = fetch.partial.entry(chunk.hash).or_default();
            if chunk.offset != partial.len() as u64 || partial.len() as u64 + chunk.bytes.len() as u64 > size {
                warn!("A piece of {path} arrived out of place, so it is fetched again");
                partial.clear();
                fetch.asked.remove(&chunk.hash);
                continue;
            }
            partial.extend_from_slice(&chunk.bytes);
            let done = partial.len() as u64 == size;
            fetch.bytes_received += chunk.bytes.len() as u64;
            fetch.current = Some(path.clone());
            if !done {
                continue;
            }
            let bytes = fetch.partial.remove(&chunk.hash).unwrap_or_default();
            match store.put(chunk.hash, path.extension().unwrap_or_default(), &bytes) {
                Ok(()) => {
                    fetch.wanted.remove(&chunk.hash);
                    debug!("Stored {path} from the server");
                }
                Err(e) => {
                    warn!("{path} from the server is refused: {e}");
                    fetch.bytes_received = fetch.bytes_received.saturating_sub(bytes.len() as u64);
                    fetch.asked.remove(&chunk.hash);
                }
            }
        }
    }

    if taken > 0 {
        for (_, mut receipts) in &mut senders {
            receipts.send::<ResourceChannel>(Received { bytes: taken });
        }
    }

    // While comparing, `wanted` is still being filled and nothing in it has been asked for yet.
    if store.is_none() || fetch.phase != FetchPhase::Downloading {
        return;
    }
    let mut again = Vec::new();
    let unasked: Vec<ContentHash> = fetch.wanted.keys().filter(|hash| !fetch.asked.contains(*hash)).copied().collect();
    for hash in unasked {
        let tries = fetch.retries.entry(hash).or_default();
        *tries += 1;
        match *tries > RETRIES {
            true => {
                if let Some((size, path)) = fetch.wanted.remove(&hash) {
                    warn!("{path} kept arriving damaged, so it is given up on and reads as missing");
                    fetch.bytes_wanted = fetch.bytes_wanted.saturating_sub(size);
                }
            }
            false => again.push(hash),
        }
    }
    if !again.is_empty() {
        for (mut sender, _) in &mut senders {
            sender.send::<ResourceChannel>(Want { hashes: again.clone() });
        }
        fetch.asked.extend(again);
    }
}

/// Into the sun bake's cache folder, where it looks.
#[cfg(not(target_arch = "wasm32"))]
fn place_lightmaps(manifest: &Manifest, store: &dyn ObjectStore) -> bool {
    let mut placed = false;
    for entry in manifest.entries.iter().filter(|entry| entry.key.kind() == ResourceKind::Lightmap) {
        let name = entry.key.path.as_str().rsplit('/').next().unwrap_or_default();
        let Some(hash) = name.strip_suffix(".bin").and_then(|hex| u64::from_str_radix(hex, 16).ok()) else { continue };
        let folder = crate::tool::sun_bake::CACHE_FOLDER;
        if crate::tool::bake_cache::file(folder, hash).is_file() {
            continue;
        }
        let Ok(bytes) = store.get(entry.key.hash, "bin") else { continue };
        match crate::tool::bake_cache::put_bytes(folder, hash, &bytes) {
            Ok(()) => placed = true,
            Err(e) => warn!("Could not keep the server's lightmap: {e}"),
        }
    }
    placed
}

#[cfg(target_arch = "wasm32")]
fn place_lightmaps(_: &Manifest, _: &dyn ObjectStore) -> bool {
    false
}

#[allow(clippy::too_many_arguments)]
fn mount_when_everything_is_stored(
    role: Res<NetRole>,
    root: Res<DownloadsRoot>,
    mut assets: ResMut<PackAssets>,
    mut fetch: ResMut<Fetch>,
    cache: Option<ResMut<crate::prop::baked::PropCache>>,
    sun: Option<ResMut<crate::tool::sun_bake::SunBake>>,
    mut changed: MessageWriter<PacksChanged>,
) {
    if role.is_authority() || fetch.phase != FetchPhase::Downloading || !fetch.wanted.is_empty() || fetch.mounted == Some(fetch.generation) {
        return;
    }
    let Some(manifest) = fetch.manifest.clone() else { return };
    let Some(store) = root.store() else { return };
    if place_lightmaps(&manifest, store.as_ref())
        && let Some(mut sun) = sun
    {
        sun.look_on_disk_again();
    }
    record_rounds(&root, &role, &mut fetch, &manifest);

    // Only changed paths, and never a round's map or a lightmap, which no loader reads; else each
    // round start would reload every table and drop every bake on every client.
    let only = assets.session().map(|before| {
        let differs = |entry: &ManifestEntry, other: &Manifest| other.get(&entry.key.path).is_none_or(|known| known.key.hash != entry.key.hash);
        let mut paths: Vec<String> = manifest
            .entries
            .iter()
            .filter(|entry| differs(entry, &before.manifest))
            .chain(before.manifest.entries.iter().filter(|entry| differs(entry, &manifest)))
            .filter(|entry| !matches!(entry.key.kind(), ResourceKind::Blueprint | ResourceKind::Lightmap))
            .map(|entry| entry.key.path.to_string())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    });
    info!("The server's {} files are mounted", manifest.entries.len());
    assets.mount(SessionPack::new(label(&role), manifest, store));
    match (&only, cache) {
        (Some(paths), Some(mut cache)) => {
            for path in paths {
                if let Some(stem) = path.strip_prefix("props/").and_then(|rest| rest.strip_suffix(".gpp")) {
                    cache.forget(stem);
                }
            }
        }
        (None, Some(mut cache)) => cache.forget_all(),
        (_, None) => {}
    }
    if only.as_ref().is_none_or(|paths| !paths.is_empty()) {
        changed.write(PacksChanged { only });
    }
    fetch.mounted = Some(fetch.generation);
    fetch.phase = FetchPhase::Mounted;
    fetch.current = None;
}

/// Adds the manifest's map to the session record if it is a round not seen yet.
fn record_rounds(root: &DownloadsRoot, role: &NetRole, fetch: &mut Fetch, manifest: &Manifest) {
    let now = chrono::Local::now();
    let record = fetch.record.get_or_insert_with(|| SessionRecord {
        server: label(role),
        started: now.format("%Y-%m-%d_%H-%M-%S").to_string(),
        rounds: Vec::new(),
        objects: BTreeSet::new(),
        written: false,
    });
    let mut added = false;
    for entry in &manifest.entries {
        added |= record.objects.insert(crate::common::resource::store::object_name(entry.key.hash, entry.key.path.extension().unwrap_or_default()));
    }
    for entry in manifest.entries.iter().filter(|entry| entry.key.kind() == ResourceKind::Blueprint) {
        let round = RoundRecord { seen: now.to_rfc3339(), map: entry.key.path.to_string(), hash: entry.key.hash.to_string() };
        if record.rounds.last().is_none_or(|last| last.hash != round.hash) {
            record.rounds.push(round);
            added = true;
        }
    }
    if !added && record.written {
        return;
    }
    record.written = true;
    let folder: String = record.server.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    let file = root.0.join("sessions").join(folder).join(format!("{}.toml", record.started));
    let written = toml::to_string(record).map_err(|e| e.to_string()).and_then(|text| {
        std::fs::create_dir_all(file.parent().expect("a session file has a folder")).map_err(|e| e.to_string())?;
        std::fs::write(&file, text).map_err(|e| e.to_string())
    });
    if let Err(e) = written {
        warn!("Could not write {}: {e}", file.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if a client is sent more than its window before it reports what it took, or is sent
    /// nothing more once it has.
    #[test]
    fn a_client_is_sent_no_more_than_its_window_until_it_reports() {
        let bytes = vec![7u8; 3 * WINDOW_BYTES as usize];
        let hash = ContentHash::of(&bytes);
        let assets = PackAssets::default();
        let mut served = Served::default();
        served.sources.insert(hash, Source::Bytes(bytes));
        let mut outbox = Outbox { queue: VecDeque::from([hash]), ..default() };
        let mut sent = 0u64;
        while let Some(chunk) = next_chunk(&assets, &mut served, &mut outbox, usize::MAX) {
            sent += chunk.bytes.len() as u64;
            sent_from(&mut outbox, chunk.bytes.len());
        }
        assert_eq!(sent, WINDOW_BYTES);
        outbox.in_flight -= CHUNK_BYTES as u64;
        let chunk = next_chunk(&assets, &mut served, &mut outbox, usize::MAX).expect("a report frees room");
        assert_eq!(chunk.offset, WINDOW_BYTES);
        assert_eq!(chunk.bytes.len(), CHUNK_BYTES);
    }

    #[test]
    fn every_message_crosses_the_wire() {
        let path = LogicalPath::new("props/crate.gpp").unwrap();
        let hash = ContentHash::of(b"crate");
        let manifest = Manifest::checked(vec![ManifestEntry { key: ResourceKey { path, hash }, size: 5, pack: "user".into(), authors: vec![] }]).unwrap();
        let message = ManifestMessage { manifest, generation: 3, server: "a server".into() };
        assert_eq!(postcard::from_bytes::<ManifestMessage>(&postcard::to_stdvec(&message).unwrap()).unwrap(), message);
        let want = Want { hashes: vec![hash] };
        assert_eq!(postcard::from_bytes::<Want>(&postcard::to_stdvec(&want).unwrap()).unwrap(), want);
        let chunk = Chunk { hash, offset: 0, bytes: b"crate".to_vec() };
        assert_eq!(postcard::from_bytes::<Chunk>(&postcard::to_stdvec(&chunk).unwrap()).unwrap(), chunk);
        let received = Received { bytes: 32 * 1024 };
        assert_eq!(postcard::from_bytes::<Received>(&postcard::to_stdvec(&received).unwrap()).unwrap(), received);
    }
}
