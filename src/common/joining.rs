//! [`AppMode::Joining`]: from connecting to a server until its first screen. The client fetches the
//! server's files ([`crate::common::resource::sync`]) and then builds them — every prop and material
//! the manifest lists, the map's rooms and its light — so the first frame of the server's mode bakes
//! nothing. See "Joining is a loading state" in documentation/resource-streaming.md; the screen is
//! [`crate::joining_screen`].

use std::collections::VecDeque;

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::match_state::ServerMode;
use crate::common::net::NetRole;
use crate::common::resource::sync::{Fetch, FetchPhase};
use crate::common::resource::ResourceKind;
use crate::editor::editable::FeatureTimeline;
use crate::get;
use crate::tool::bake_job::{BakeKind, BakeProgress, BakeState, BakeStatus};

pub struct JoiningPlugin;

impl Plugin for JoiningPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<JoinBuild>()
            .init_resource::<Fetch>()
            .init_resource::<ServerMode>()
            .add_systems(
                Update,
                enter_joining
                    .after(crate::common::net::apply_net_requests)
                    .before(crate::common::match_state::back_to_the_map_editor_when_networked),
            )
            .add_systems(
                Update,
                (plan_the_build, bake_a_prop, skip_materials_without_a_library, finish_joining)
                    .chain()
                    .run_if(in_state(AppMode::Joining)),
            );
    }
}

/// What the build band has left: the files to bake, a frame each, then waiting on the room and
/// sun bakes.
#[derive(Resource, Default)]
pub struct JoinBuild {
    /// The mounted manifest generation the build was planned for; a remount plans it again.
    planned: Option<u64>,
    /// Prop file stems.
    pub props: VecDeque<String>,
    /// Material names, baked by the material editor's plugin.
    pub materials: VecDeque<String>,
    props_total: usize,
    materials_total: usize,
    /// The file baked last, for the phase line.
    pub current: Option<(ResourceKind, String)>,
    /// [`FeatureTimeline::adoptions`] as joining began; the server's map is in once it moves.
    adoptions_at_start: Option<u64>,
}

const PROP_WEIGHT: f32 = 1.0;
const MATERIAL_WEIGHT: f32 = 2.0;
const ROOMS_WEIGHT: f32 = 2.0;
const LIGHT_WEIGHT: f32 = 4.0;

fn enter_joining(
    role: Res<NetRole>,
    timeline: Option<Res<FeatureTimeline>>,
    mut build: ResMut<JoinBuild>,
    mut server_mode: ResMut<ServerMode>,
    mut next: ResMut<NextState<AppMode>>,
) {
    if !role.is_changed() || !matches!(*role, NetRole::Client { .. }) {
        return;
    }
    *build = JoinBuild { adoptions_at_start: timeline.map(|timeline| timeline.adoptions()), ..default() };
    server_mode.0 = None;
    next.set(AppMode::Joining);
}

fn stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem).to_owned()
}

/// Planned again on each remount; a prop or material already baked costs nothing to ask for.
fn plan_the_build(fetch: Res<Fetch>, mut build: ResMut<JoinBuild>) {
    if fetch.phase != FetchPhase::Mounted || build.planned == fetch.mounted_generation() {
        return;
    }
    let Some(manifest) = fetch.manifest() else { return };
    let of_kind = |kind: ResourceKind| -> VecDeque<String> {
        manifest.entries.iter().filter(|entry| entry.key.kind() == kind).map(|entry| stem(entry.key.path.as_str())).collect()
    };
    build.props = of_kind(ResourceKind::Prop);
    build.materials = of_kind(ResourceKind::Material);
    build.props_total = build.props.len();
    build.materials_total = build.materials.len();
    build.planned = fetch.mounted_generation();
}

#[derive(bevy::ecs::system::SystemParam)]
struct PropBaking<'w> {
    cache: Option<ResMut<'w, crate::prop::baked::PropCache>>,
    packs: Res<'w, crate::common::assets::Assets>,
    meshes: Option<ResMut<'w, Assets<Mesh>>>,
    materials: Option<ResMut<'w, Assets<crate::common::air_lit::LitMaterial>>>,
    surfaces: Option<ResMut<'w, crate::prop::baked::SurfaceMaterials>>,
    clips: Option<ResMut<'w, Assets<crate::common::skeleton::clip::Clip>>>,
}

/// One a frame. Without the prop cache's resources (a test with no renderer) the props are passed
/// over, since nothing here would draw them.
fn bake_a_prop(mut build: ResMut<JoinBuild>, baking: PropBaking) {
    let Some(name) = build.props.pop_front() else { return };
    let PropBaking { cache, packs, meshes, materials, surfaces, clips } = baking;
    if let (Some(mut cache), Some(mut meshes), Some(mut materials), Some(mut surfaces), Some(mut clips)) = (cache, meshes, materials, surfaces, clips) {
        cache.ensure(&name, &packs, &mut meshes, &mut materials, &mut surfaces, &mut clips);
    }
    build.current = Some((ResourceKind::Prop, name));
}

fn skip_materials_without_a_library(library: Option<Res<crate::material::library::MaterialLibrary>>, mut build: ResMut<JoinBuild>) {
    if library.is_none() {
        build.materials.clear();
    }
}

/// Nothing will bake it further: up to date, turned off, or out of date with auto-bake off.
fn settled(status: Option<&BakeStatus>) -> bool {
    status.is_none_or(|status| matches!(status.state, BakeState::UpToDate | BakeState::Skipped | BakeState::Stale))
}

fn bake_fraction(status: Option<&BakeStatus>) -> f32 {
    match settled(status) {
        true => 1.0,
        // Short of 1 until it settles, since a bake can finish and be asked for again.
        false => status.map_or(0.0, |status| status.fraction().min(0.99)),
    }
}

impl JoinBuild {
    fn map_in(&self, timeline: Option<&FeatureTimeline>) -> bool {
        match (self.adoptions_at_start, timeline) {
            (Some(at_start), Some(timeline)) => timeline.adoptions() > at_start,
            _ => true,
        }
    }

    fn files_left(&self) -> bool {
        !self.props.is_empty() || !self.materials.is_empty()
    }
}

fn finish_joining(
    build: Res<JoinBuild>,
    fetch: Res<Fetch>,
    timeline: Option<Res<FeatureTimeline>>,
    bakes: Option<Res<BakeProgress>>,
    server_mode: Res<ServerMode>,
    mut next: ResMut<NextState<AppMode>>,
) {
    let Some(mode) = server_mode.0 else { return };
    let bakes = bakes.as_deref();
    let done = fetch.phase == FetchPhase::Mounted
        && build.planned.is_some()
        && build.planned == fetch.mounted_generation()
        && !build.files_left()
        && build.map_in(timeline.as_deref())
        && settled(bakes.and_then(|bakes| bakes.get(BakeKind::Rooms)))
        && settled(bakes.and_then(|bakes| bakes.get(BakeKind::Sunlight)));
    if done {
        info!("Joined; the server is in {}", mode.name());
        next.set(mode);
    }
}

/// How far joining has got, as the screen draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct JoinProgress {
    /// 0 to 1: comparing up to 0.05, downloading up to 0.8, building up to 1.
    pub fraction: f32,
    pub line: String,
}

const COMPARED: f32 = 0.05;
const DOWNLOADED: f32 = 0.8;

pub fn progress(fetch: &Fetch, build: &JoinBuild, timeline: Option<&FeatureTimeline>, bakes: Option<&BakeProgress>) -> JoinProgress {
    let share = |done: f32, total: f32| if total <= 0.0 { 1.0 } else { (done / total).clamp(0.0, 1.0) };
    match fetch.phase {
        FetchPhase::Idle => JoinProgress { fraction: 0.0, line: get!("joining.waiting") },
        FetchPhase::Comparing => JoinProgress {
            fraction: COMPARED * share(fetch.entries_checked as f32, fetch.entries as f32),
            line: get!("joining.comparing", "done", fetch.entries_checked, "total", fetch.entries),
        },
        FetchPhase::Downloading => {
            let file = fetch.current.as_ref().map_or_else(String::new, ToString::to_string);
            let done = (fetch.files_received() + 1).min(fetch.files_wanted);
            JoinProgress {
                fraction: COMPARED + (DOWNLOADED - COMPARED) * share(fetch.bytes_received as f32, fetch.bytes_wanted as f32),
                line: get!("joining.downloading", "file", file, "done", done, "total", fetch.files_wanted),
            }
        }
        FetchPhase::Mounted => {
            let rooms = bakes.and_then(|bakes| bakes.get(BakeKind::Rooms));
            let light = bakes.and_then(|bakes| bakes.get(BakeKind::Sunlight));
            let total = build.props_total as f32 * PROP_WEIGHT + build.materials_total as f32 * MATERIAL_WEIGHT + ROOMS_WEIGHT + LIGHT_WEIGHT;
            let props_done = build.props_total.saturating_sub(build.props.len()) as f32;
            let materials_done = build.materials_total.saturating_sub(build.materials.len()) as f32;
            let map_in = build.map_in(timeline);
            let (rooms_done, light_done) = match map_in {
                true => (bake_fraction(rooms), bake_fraction(light)),
                false => (0.0, 0.0),
            };
            let done = props_done * PROP_WEIGHT + materials_done * MATERIAL_WEIGHT + rooms_done * ROOMS_WEIGHT + light_done * LIGHT_WEIGHT;
            let line = match (&build.current, build.files_left()) {
                (Some((ResourceKind::Material, name)), true) => get!("joining.baking_material", "file", name),
                (Some((_, name)), true) => get!("joining.building_prop", "file", name),
                _ if !map_in => get!("joining.waiting"),
                _ if !settled(rooms) => get!("joining.rooms"),
                _ => get!("joining.light"),
            };
            JoinProgress { fraction: DOWNLOADED + (1.0 - DOWNLOADED) * share(done, total), line }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_phase_stays_in_its_band() {
        let mut fetch = Fetch::default();
        let build = JoinBuild::default();
        assert_eq!(progress(&fetch, &build, None, None).fraction, 0.0);

        fetch.phase = FetchPhase::Comparing;
        fetch.entries = 10;
        fetch.entries_checked = 5;
        assert!((progress(&fetch, &build, None, None).fraction - 0.025).abs() < 1e-6);

        fetch.phase = FetchPhase::Downloading;
        fetch.bytes_wanted = 100;
        fetch.bytes_received = 50;
        assert!((progress(&fetch, &build, None, None).fraction - 0.425).abs() < 1e-6);
        fetch.bytes_wanted = 0;
        fetch.bytes_received = 0;
        assert!((progress(&fetch, &build, None, None).fraction - 0.8).abs() < 1e-6, "nothing to download is the band done");

        fetch.phase = FetchPhase::Mounted;
        assert!((progress(&fetch, &build, None, None).fraction - 1.0).abs() < 1e-6, "with nothing to build or bake, joining is done");
        let building = JoinBuild { props: ["crate".to_owned()].into(), props_total: 1, planned: Some(1), ..default() };
        let fraction = progress(&fetch, &building, None, None).fraction;
        assert!(fraction > 0.8 && fraction < 1.0, "{fraction}");
    }
}
