//! The map editor's tools as wizards on [`crate::common::wizard`]: each tool
//! is a [`MapEntry`] naming its steps, and one driver takes picks for all of
//! them. Points are picked by a [`PointPicker`], so a click on a named point
//! grows the step by an offset (a second click, or a right click for none).
//! Escape takes back the last pick and leaves with nothing picked; Enter moves
//! on or finishes. The plan is
//! [documentation/tool-wizards.md](../../documentation/tool-wizards.md).

use bevy::prelude::*;
use bevy_egui::egui;

use crate::common::app_mode::AppMode;
use crate::common::keybind::{Action, Actions};
use crate::common::toolbar::{self, Segment};
use crate::common::wizard::{self, Filled, Steps};
use crate::editor::editable::{AxisRef, EditEvent, FeatureId, FeatureTimeline, FeatureTrait, PointRef};
use crate::editor::input::CurrentMouseInput;
use crate::editor::mirror::Mirror;
use crate::editor::multicam::Multicam;
use crate::get;
use crate::tool::point_picker::{Picked, PointPicker};
use crate::tool::point_placement::PlaceablePoint;
use crate::tool::room::MapVolumes;
use crate::tool::show::GizmoVisibility;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const SNAP: f32 = 0.1;

const PICKED_COLOR: Color = Color::srgb_u8(100, 255, 100);
/// Where a Mirror being made or edited would put each picked source.
const MIRRORED_COLOR: Color = Color::srgb_u8(190, 90, 255);
const CURSOR_COLOR: Color = Color::srgb_u8(60, 120, 255);
const REFUSED_COLOR: Color = Color::srgb_u8(240, 70, 50);

#[derive(Clone)]
pub enum MapPick {
    /// Where it is, and how the feature will name it; `absolute` when clicked
    /// at the cursor rather than measured from a named point.
    Point { point: PointRef, at: Vec3, absolute: bool },
    Feature(FeatureId),
    /// Closes a chain of corners on its first.
    Close,
}

impl PartialEq for MapPick {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (MapPick::Point { point: a, at: p, .. }, MapPick::Point { point: b, at: q, .. }) => a == b && p == q,
            (MapPick::Feature(a), MapPick::Feature(b)) => a == b,
            (MapPick::Close, MapPick::Close) => true,
            _ => false,
        }
    }
}

impl MapPick {
    fn at(&self) -> Option<Vec3> {
        match self {
            MapPick::Point { at, .. } => Some(*at),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MapKind {
    Point,
    /// Corners chained into a loop, answered once a click closes it on the first.
    Corners,
    Features,
    /// One point some feature already names: what a re-target measures from.
    NamedPoint,
}

pub struct MapStep {
    pub kind: MapKind,
    /// Lang key of the step's name, in the list of picks.
    pub name: &'static str,
    /// Lang key of what is asked for while the step is under way.
    pub prompt: &'static str,
}

pub struct MapEntry {
    pub tool: Tools,
    pub steps: &'static [MapStep],
    /// Where a point picked at `cursor` lands, given the points picked before
    /// it; `None` refuses it.
    pub place: fn(&[Vec3], Vec3) -> Option<Vec3>,
    /// The feature from each step's picks, in step order.
    pub build: fn(&Tools, &[Vec<MapPick>], &FeatureTimeline) -> Option<Box<dyn FeatureTrait>>,
    /// What would be made with `at` as the next point.
    pub preview: fn(&mut Gizmos, &[Vec3], Vec3),
}

const POINT: &[MapStep] = &[MapStep { kind: MapKind::Point, name: "tools.step.point", prompt: "tools.pick.point" }];
const BOX: &[MapStep] = &[
    MapStep { kind: MapKind::Point, name: "tools.step.corner", prompt: "tools.pick.corner" },
    MapStep { kind: MapKind::Point, name: "tools.step.far_corner", prompt: "tools.pick.far_corner" },
];
const STAIRS: &[MapStep] = &[
    MapStep { kind: MapKind::Point, name: "tools.step.start", prompt: "tools.pick.start" },
    MapStep { kind: MapKind::Point, name: "tools.step.end", prompt: "tools.pick.end" },
    MapStep { kind: MapKind::Point, name: "tools.step.width", prompt: "tools.pick.width" },
    MapStep { kind: MapKind::Point, name: "tools.step.height", prompt: "tools.pick.height" },
];
const CORNERS: &[MapStep] = &[MapStep { kind: MapKind::Corners, name: "tools.step.corners", prompt: "tools.pick.corners" }];
const MIRROR: &[MapStep] = &[
    MapStep { kind: MapKind::Point, name: "tools.step.center", prompt: "tools.pick.center" },
    MapStep { kind: MapKind::Features, name: "tools.step.sources", prompt: "tools.pick.sources" },
];
const RETARGET: &[MapStep] = &[MapStep { kind: MapKind::NamedPoint, name: "tools.step.measured_from", prompt: "tools.pick.measured_from" }];
const SOURCES: &[MapStep] = &[MapStep { kind: MapKind::Features, name: "tools.step.sources", prompt: "tools.pick.sources" }];

fn anywhere(_: &[Vec3], at: Vec3) -> Option<Vec3> {
    Some(at)
}

fn nothing(_: &mut Gizmos, _: &[Vec3], _: Vec3) {}

fn placed_point<F: PlaceablePoint>(_: &Tools, steps: &[Vec<MapPick>], _: &FeatureTimeline) -> Option<Box<dyn FeatureTrait>> {
    match steps.first()?.first()? {
        MapPick::Point { at, absolute: true, .. } => Some(F::from_position(*at)),
        MapPick::Point { point, .. } => Some(F::from_point_ref(point.clone())),
        _ => None,
    }
}

fn point_preview<F: PlaceablePoint>(gizmos: &mut Gizmos, _: &[Vec3], at: Vec3) {
    F::draw_preview(gizmos, at, F::normal_color());
}

const fn point_entry<F: PlaceablePoint>() -> MapEntry {
    MapEntry { tool: F::TOOL, steps: POINT, place: anywhere, build: placed_point::<F>, preview: point_preview::<F> }
}

fn mirror_build(_: &Tools, steps: &[Vec<MapPick>], _: &FeatureTimeline) -> Option<Box<dyn FeatureTrait>> {
    let center = match steps.first()?.first()? {
        MapPick::Point { at, absolute: true, .. } => PointRef::absolute(at.x, at.y, at.z),
        MapPick::Point { point, .. } => point.clone(),
        _ => return None,
    };
    let sources = steps.get(1)?.iter().filter_map(|pick| match pick {
        MapPick::Feature(id) => Some(*id),
        _ => None,
    });
    Some(Box::new(Mirror::new(center).with_sources(sources.collect())))
}

fn retarget_build(_: &Tools, _: &[Vec<MapPick>], _: &FeatureTimeline) -> Option<Box<dyn FeatureTrait>> {
    None
}

pub const ENTRIES: &[MapEntry] = &[
    point_entry::<crate::editor::global_point::GlobalPoint>(),
    point_entry::<crate::editor::grackle_point_light::GracklePointLight>(),
    point_entry::<crate::editor::spawn_point::SpawnPoint>(),
    point_entry::<crate::editor::spectator_camera::SpectatorCamera>(),
    point_entry::<crate::editor::decal::EditorDecal>(),
    point_entry::<crate::editor::prop::Prop>(),
    point_entry::<crate::editor::weapon_pickup::WeaponPickup>(),
    point_entry::<crate::editor::animation_display::AnimationDisplay>(),
    point_entry::<crate::editor::animation_grid::AnimationGrid>(),
    MapEntry { tool: Tools::Room, steps: BOX, place: anywhere, build: crate::tool::room::box_build, preview: crate::tool::room::box_preview },
    MapEntry { tool: Tools::Block, steps: BOX, place: anywhere, build: crate::tool::room::box_build, preview: crate::tool::room::box_preview },
    MapEntry { tool: Tools::Trigger, steps: BOX, place: anywhere, build: crate::tool::room::box_build, preview: crate::tool::room::box_preview },
    MapEntry {
        tool: Tools::Stairs,
        steps: STAIRS,
        place: crate::tool::stairs::placed,
        build: crate::tool::stairs::build,
        preview: crate::tool::stairs::preview,
    },
    MapEntry {
        tool: Tools::Footprint,
        steps: CORNERS,
        place: anywhere,
        build: crate::tool::footprint::build,
        preview: crate::tool::footprint::preview,
    },
    MapEntry {
        tool: Tools::FootprintBlock,
        steps: CORNERS,
        place: anywhere,
        build: crate::tool::footprint::build,
        preview: crate::tool::footprint::preview,
    },
    MapEntry { tool: Tools::Mirror, steps: MIRROR, place: anywhere, build: mirror_build, preview: point_preview::<Mirror> },
    MapEntry { tool: Tools::Retarget, steps: RETARGET, place: anywhere, build: retarget_build, preview: nothing },
    MapEntry { tool: Tools::MirrorSources, steps: SOURCES, place: anywhere, build: retarget_build, preview: nothing },
];

pub fn entry_for(tool: &Tools) -> Option<&'static MapEntry> {
    ENTRIES.iter().find(|entry| entry.tool == *tool)
}

/// An entry's steps against the map as it stands.
pub struct MapSteps<'a> {
    pub entry: &'a MapEntry,
    pub features: &'a FeatureTimeline,
    /// The feature being edited: a feature step takes only what comes before it.
    pub editing: Option<FeatureId>,
}

impl Steps for MapSteps<'_> {
    type Pick = MapPick;

    fn count(&self) -> usize {
        self.entry.steps.len()
    }

    fn answered(&self, step: usize, has: &[MapPick]) -> bool {
        match self.entry.steps[step].kind {
            MapKind::Corners => has.last() == Some(&MapPick::Close),
            _ => !has.is_empty(),
        }
    }

    fn open(&self, step: usize, has: &[MapPick]) -> bool {
        match self.entry.steps[step].kind {
            MapKind::Point | MapKind::NamedPoint => has.is_empty(),
            MapKind::Corners => has.last() != Some(&MapPick::Close),
            MapKind::Features => true,
        }
    }

    fn fits(&self, step: usize, pick: &MapPick, filled: &Filled<MapPick>) -> bool {
        let has = &filled.has[step];
        match (self.entry.steps[step].kind, pick) {
            (MapKind::Point | MapKind::NamedPoint, MapPick::Point { .. }) => true,
            (MapKind::Corners, MapPick::Point { .. }) => true,
            (MapKind::Corners, MapPick::Close) => has.iter().filter(|pick| pick.at().is_some()).count() >= 3,
            (MapKind::Features, MapPick::Feature(id)) => {
                let before = self.editing.is_none_or(|editing| prior_features(self.features, editing).contains(id));
                self.features.features_map().contains_key(id) && !has.contains(pick) && before
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Confirm,
    Cancel,
}

/// The map wizard under way: its picks are its own, unlike the prop editor's,
/// since the map's selection is of whole features.
#[derive(Resource)]
pub struct MapWizard {
    picks: Vec<MapPick>,
    moved_on: Vec<MapPick>,
    picker: PointPicker,
    cursor: Option<Vec3>,
    /// The depth an orthographic view puts the cursor at.
    last_position: Vec3,
    hovered: Option<FeatureId>,
    hovered_point: Option<(FeatureId, String, Vec3)>,
    pub request: Option<Request>,
    /// The feature and point key a re-target re-points.
    retarget: Option<(FeatureId, String)>,
    /// The Mirror whose sources are re-picked.
    sources_of: Option<FeatureId>,
}

impl Default for MapWizard {
    fn default() -> Self {
        Self {
            picks: vec![],
            moved_on: vec![],
            picker: PointPicker::default(),
            cursor: None,
            last_position: Vec3::ZERO,
            hovered: None,
            hovered_point: None,
            request: None,
            retarget: None,
            sources_of: None,
        }
    }
}

impl MapWizard {
    /// Re-points `key` on `feature`; the caller then switches to [`Tools::Retarget`].
    pub fn retarget(&mut self, feature: FeatureId, key: String) {
        self.retarget = Some((feature, key));
    }

    /// Re-picks a Mirror's sources; the caller then switches to [`Tools::MirrorSources`].
    pub fn pick_sources(&mut self, mirror: FeatureId) {
        self.sources_of = Some(mirror);
    }

    /// The feature an edit is of, which a feature step must come before.
    fn editing(&self) -> Option<FeatureId> {
        self.retarget.as_ref().map(|(feature, _)| *feature).or(self.sources_of)
    }

    fn points(&self) -> Vec<Vec3> {
        self.picks.iter().filter_map(MapPick::at).collect()
    }

    /// Everything but where the cursor last stood, and what is being edited when `keep_edit`.
    fn clear(&mut self, keep_edit: bool) {
        let retarget = self.retarget.take().filter(|_| keep_edit);
        let sources_of = self.sources_of.take().filter(|_| keep_edit);
        *self = MapWizard { last_position: self.last_position, retarget, sources_of, ..default() };
    }
}

/// What a wizard has picked, drawn over everything else: it shows only while a
/// tool runs, and the map's own lines run along the same edges.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct PickedGizmos;

pub struct MapWizardPlugin;

impl Plugin for MapWizardPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app.insert_gizmo_config(PickedGizmos, GizmoConfig { depth_bias: -1.0, ..default() });
        app.init_resource::<MapWizard>().add_systems(
            Update,
            (start_on_a_new_tool.run_if(state_changed::<Tools>), run_map_wizard, draw_map_wizard)
                .chain()
                .run_if(in_state(AppMode::Editor)),
        );
    }
}

/// Every tool starts empty but for what the selection gives it: a selected
/// point-like feature for a point step, other features for a feature step. A
/// selection that would finish the tool gives it nothing, since finishing
/// selects what it made and the same tool picked again would copy it; a named
/// point picked with Shift may.
fn start_on_a_new_tool(
    tool: Res<State<Tools>>,
    features: Res<FeatureTimeline>,
    mut wizard: ResMut<MapWizard>,
    selection: Option<ResMut<crate::tool::selection::SelectionState>>,
) {
    wizard.clear(matches!(tool.get(), Tools::Retarget | Tools::MirrorSources));
    // A Mirror's sources start from what it has, so a click adds and Escape takes one back.
    if *tool.get() == Tools::MirrorSources {
        let sources = wizard
            .sources_of
            .and_then(|id| features.features_map().get(&id))
            .and_then(|feature| feature.object().as_mirror().map(|mirror| mirror.sources().to_vec()))
            .unwrap_or_default();
        wizard.picks = sources.into_iter().map(MapPick::Feature).collect();
        return;
    }
    let Some(entry) = entry_for(tool.get()).filter(|entry| entry.tool != Tools::Retarget) else { return };
    // A Shift-picked point comes first, and is used up by the tool it starts.
    let named = selection.and_then(|mut selection| selection.point.take()).and_then(|id| {
        let mut point = PointRef::reference(id.feature);
        point.point_key = id.key;
        let at = point.resolve(features.features_map()).ok()?;
        Some(MapPick::Point { point, at, absolute: false })
    });
    let chose_a_point = named.is_some();
    let candidates: Vec<MapPick> = named.into_iter().chain(features
        .selection()
        .iter()
        .filter_map(|id| {
            let feature = features.features_map().get(id)?;
            match feature.object().picked_by() == crate::editor::editable::PickedBy::Marker {
                true => {
                    let mut point = PointRef::reference(*id);
                    let at = point.resolve(features.features_map()).ok()?;
                    Some(MapPick::Point { point, at, absolute: false })
                }
                false => Some(MapPick::Feature(*id)),
            }
        }))
        .collect();
    let steps = MapSteps { entry, features: &features, editing: None };
    let (filled, moved_on) = wizard::seed(&steps, &candidates);
    if wizard::is_finished(&steps, &filled) && !chose_a_point {
        return;
    }
    wizard.picks = candidates.into_iter().filter(|pick| filled.picks().any(|held| held == pick)).collect();
    wizard.moved_on = moved_on;
}

#[allow(clippy::too_many_arguments)]
fn run_map_wizard(
    mut wizard: ResMut<MapWizard>,
    cameras: Query<(Entity, &Multicam)>,
    mouse: Res<CurrentMouseInput>,
    actions: Res<Actions>,
    mut features: ResMut<FeatureTimeline>,
    volumes: MapVolumes,
    tool: Res<State<Tools>>,
    mut next_tool: ResMut<NextState<Tools>>,
    visibility: Res<GizmoVisibility>,
    mut commands: Commands,
    mut edits: MessageWriter<EditEvent>,
    mut products: ResMut<crate::tool::map_products::MapProducts>,
) {
    let Some(entry) = entry_for(tool.get()) else { return };
    let wizard = &mut *wizard;
    let request = wizard.request.take();
    if request == Some(Request::Cancel) {
        leave(wizard, &mut next_tool);
        return;
    }
    let steps = MapSteps { entry, features: &features, editing: wizard.editing() };
    let filled = wizard::fill(&steps, &wizard.picks, &wizard.moved_on);
    let can_move_on = wizard::can_move_on(&steps, &filled);
    let ready = wizard::is_ready(&steps, &filled);

    // Enter moves on or finishes where it can; otherwise it places at the cursor.
    let confirm = request == Some(Request::Confirm) || actions.just_pressed(Action::Confirm);
    if confirm && can_move_on {
        wizard::move_on(&steps, &wizard.picks, &mut wizard.moved_on);
        return;
    }
    // An edit always waits for Enter, so the picks it starts from do not
    // finish it, and may finish with none, as a Mirror with no sources.
    let editing = matches!(entry.tool, Tools::MirrorSources);
    if confirm && (ready || editing) || !editing && wizard::is_finished(&steps, &filled) {
        finish(entry, wizard, &filled, &mut features, &mut next_tool, &mut commands, &mut edits);
        return;
    }

    let under_way = wizard::under_way(&steps, &filled).or_else(|| {
        let last = entry.steps.len().checked_sub(1)?;
        (!filled.closed[last] && steps.open(last, &filled.has[last])).then_some(last)
    });
    let Some(step) = under_way else { return };
    if matches!(entry.steps[step].kind, MapKind::Features | MapKind::NamedPoint) {
        wizard.cursor = None;
    }
    let taken = match entry.steps[step].kind {
        MapKind::Features => {
            products.refresh(&features, crate::tool::tool_helpers::PickCache::key(features.last_changed(), &features));
            pick_a_feature(wizard, &mouse, &actions, &mut products, &visibility)
        }
        MapKind::NamedPoint => pick_a_named_point(wizard, &mouse, &actions, &features),
        kind => pick_a_point(wizard, entry, kind, &filled.has[step], &cameras, &mouse, &actions, &features, &volumes),
    };
    match taken {
        Taken::Nothing => {}
        Taken::Pick(pick) => {
            if let Some(at) = pick.at() {
                wizard.last_position = at;
            }
            // A feature picked again is unpicked; a point at a picked spot is
            // another point, which the step's rules take or refuse.
            match wizard.picks.iter().position(|held| *held == pick) {
                Some(at) if matches!(pick, MapPick::Feature(_)) => {
                    wizard.picks.remove(at);
                }
                _ => wizard.picks.push(pick),
            }
        }
        Taken::Back => {
            let mut picks = std::mem::take(&mut wizard.picks);
            let back = wizard::back(&mut picks, &mut wizard.moved_on);
            wizard.picks = picks;
            if back == wizard::Back::Exit {
                leave(wizard, &mut next_tool);
            }
        }
    }
}

enum Taken {
    Nothing,
    /// Added, or taken off again if it was already picked.
    Pick(MapPick),
    /// Escape with nothing under way inside the pick.
    Back,
}

fn leave(wizard: &mut MapWizard, next_tool: &mut NextState<Tools>) {
    wizard.clear(false);
    next_tool.set(Tools::Select);
}

#[allow(clippy::too_many_arguments)]
fn pick_a_point(
    wizard: &mut MapWizard,
    entry: &MapEntry,
    kind: MapKind,
    has: &[MapPick],
    cameras: &Query<(Entity, &Multicam)>,
    mouse: &CurrentMouseInput,
    actions: &Actions,
    features: &FeatureTimeline,
    volumes: &MapVolumes,
) -> Taken {
    let points = wizard.points();
    let suggestion = points.last().copied().unwrap_or(wizard.last_position);
    wizard.cursor = match (kind, has.first().and_then(MapPick::at)) {
        (MapKind::Corners, Some(first)) => {
            crate::tool::footprint::floor_cursor(first.y, points.last().copied().unwrap_or(first), mouse)
        }
        _ => compute_cursor(mouse, cameras, suggestion, true, SNAP, volumes),
    };
    let picked = wizard.picker.update(mouse, actions, features, wizard.cursor, Some(SNAP), |at| (entry.place)(&points, at));
    let pick = match picked {
        Picked::Nothing => return Taken::Nothing,
        Picked::Canceled => return Taken::Back,
        Picked::Absolute(at) => MapPick::Point { point: PointRef::absolute(at.x, at.y, at.z), at, absolute: true },
        Picked::Relative(point, at) => MapPick::Point { point, at, absolute: false },
    };
    let closes = kind == MapKind::Corners
        && pick.at().is_some_and(|at| crate::tool::footprint::closes(&points, at));
    Taken::Pick(if closes { MapPick::Close } else { pick })
}

fn pick_a_feature(
    wizard: &mut MapWizard,
    mouse: &CurrentMouseInput,
    actions: &Actions,
    products: &mut crate::tool::map_products::MapProducts,
    visibility: &GizmoVisibility,
) -> Taken {
    if actions.just_pressed(Action::Cancel) {
        return Taken::Back;
    }
    wizard.hovered = mouse.world_pos.and_then(|ray| products.hit(&ray, visibility)).map(|(id, _)| id);
    match (mouse.released == Some(MouseButton::Left) && mouse.in_camera.is_some(), wizard.hovered) {
        (true, Some(id)) => Taken::Pick(MapPick::Feature(id)),
        _ => Taken::Nothing,
    }
}

/// The features before `target` in the list, which are all its point may be
/// measured from.
fn prior_features(features: &FeatureTimeline, target: FeatureId) -> Vec<FeatureId> {
    let order = features.feature_order();
    let cursor = features.active_features().count();
    let target_idx = order.iter().position(|id| *id == target).unwrap_or(0);
    order[..target_idx.min(cursor)].to_vec()
}

fn own_bases(features: &FeatureTimeline, target: FeatureId, key: &str) -> Vec<(String, Vec3)> {
    features.features_map().get(&target).map(|feature| feature.object().own_bases(key)).unwrap_or_default()
}

fn pick_a_named_point(wizard: &mut MapWizard, mouse: &CurrentMouseInput, actions: &Actions, features: &FeatureTimeline) -> Taken {
    if actions.just_pressed(Action::Cancel) {
        return Taken::Back;
    }
    let Some((target, key)) = wizard.retarget.clone() else { return Taken::Back };
    let allowed = prior_features(features, target);
    let own = own_bases(features, target, &key);
    wizard.hovered_point = mouse.world_pos.and_then(|ray| {
        let elsewhere = find_hovered_point_filtered(&ray, features, PICK_RADIUS, &allowed);
        let distance = |pos: Vec3| ray_point_distance(&ray, pos);
        let ours = own
            .iter()
            .filter(|(_, pos)| distance(*pos) < PICK_RADIUS)
            .min_by(|a, b| distance(a.1).total_cmp(&distance(b.1)))
            .map(|(key, pos)| (target, key.clone(), *pos));
        match (elsewhere, ours) {
            (Some(e), Some(o)) => Some(if distance(o.2) <= distance(e.2) { o } else { e }),
            (e, o) => e.or(o),
        }
    });
    match (mouse.released == Some(MouseButton::Left), wizard.hovered_point.clone()) {
        (true, Some((feature, point_key, at))) => {
            let mut point = PointRef::reference(feature);
            point.point_key = point_key;
            Taken::Pick(MapPick::Point { point, at, absolute: false })
        }
        _ => Taken::Nothing,
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    entry: &MapEntry,
    wizard: &mut MapWizard,
    filled: &Filled<MapPick>,
    features: &mut FeatureTimeline,
    next_tool: &mut NextState<Tools>,
    commands: &mut Commands,
    edits: &mut MessageWriter<EditEvent>,
) {
    if entry.tool == Tools::Retarget {
        if let (Some((target, key)), Some(MapPick::Point { point, at, .. })) = (wizard.retarget.clone(), filled.picks().next()) {
            retarget(features, target, &key, point, *at, commands, edits);
            features.select(Some(target));
        }
    } else if entry.tool == Tools::MirrorSources {
        if let Some(mirror) = wizard.sources_of {
            let sources = filled.picks().filter_map(|pick| match pick {
                MapPick::Feature(id) => Some(*id),
                _ => None,
            });
            set_sources(features, mirror, sources.collect(), commands, edits);
            features.select(Some(mirror));
        }
    } else if let Some(feature) = (entry.build)(&entry.tool, &filled.has, features) {
        let id = features.apply_feature(feature);
        features.select(Some(id));
    }
    wizard.clear(false);
    next_tool.set(Tools::Select);
}

fn set_sources(
    features: &mut FeatureTimeline,
    mirror: FeatureId,
    sources: Vec<FeatureId>,
    commands: &mut Commands,
    edits: &mut MessageWriter<EditEvent>,
) {
    let Some(mut feature) = features.features_mut().remove(&mirror) else { return };
    if let Some(standing) = feature.object_mut().mirror_mut() {
        standing.set_sources(sources);
    }
    feature.object_mut().resolve_references(features.features_map());
    if let Some(entity) = feature.object().entity() {
        feature.object_mut().apply_to_entity(commands, entity);
        edits.write(EditEvent { editor_id: mirror._id(), feature_id: mirror, entity });
    }
    features.features_mut().insert(mirror, feature);
}

/// Measures `key` on `target` from `point` instead, keeping where it is.
fn retarget(
    features: &mut FeatureTimeline,
    target: FeatureId,
    key: &str,
    point: &PointRef,
    at: Vec3,
    commands: &mut Commands,
    edits: &mut MessageWriter<EditEvent>,
) {
    let Some(reference) = point.reference else { return };
    let Some(mut feature) = features.features_mut().remove(&target) else { return };
    // Its own point: measured from itself, which a `PointRef` cannot say.
    if reference == target {
        feature.object_mut().rebase_on_own(key, &point.point_key);
    } else if let Some(pr) = feature.object_mut().get_point_ref_mut(key) {
        let old_base = pr.resolved_reference.unwrap_or(Vec3::ZERO);
        let current = Vec3::new(
            pr.x.resolve_with_base(Some(old_base.x)).unwrap_or(0.0),
            pr.y.resolve_with_base(Some(old_base.y)).unwrap_or(0.0),
            pr.z.resolve_with_base(Some(old_base.z)).unwrap_or(0.0),
        );
        pr.reference = Some(reference);
        pr.point_key = point.point_key.clone();
        pr.resolved_reference = Some(at);
        pr.x = AxisRef::Relative(current.x - at.x);
        pr.y = AxisRef::Relative(current.y - at.y);
        pr.z = AxisRef::Relative(current.z - at.z);
    }
    feature.object_mut().resolve_references(features.features_map());
    if let Some(entity) = feature.object().entity() {
        feature.object_mut().apply_to_entity(commands, entity);
        edits.write(EditEvent { editor_id: target._id(), feature_id: target, entity });
    }
    features.features_mut().insert(target, feature);
}

fn draw_map_wizard(
    wizard: Res<MapWizard>,
    tool: Res<State<Tools>>,
    features: Res<FeatureTimeline>,
    mouse: Res<CurrentMouseInput>,
    mut gizmos: Gizmos,
    mut picked: Gizmos<PickedGizmos>,
) {
    let Some(entry) = entry_for(tool.get()) else { return };
    let points = wizard.points();
    for at in &points {
        picked.sphere(Isometry3d::from_translation(*at), 0.15, PICKED_COLOR);
    }
    let mirrorings = mirrorings(entry, &wizard, &features);
    for pick in &wizard.picks {
        let MapPick::Feature(id) = pick else { continue };
        let Some(feature) = features.features_map().get(id) else { continue };
        let edges = outline(feature.object());
        for (a, b) in &edges {
            picked.line(*a, *b, PICKED_COLOR);
        }
        for mirroring in &mirrorings {
            for (a, b) in &edges {
                picked.line(mirroring.point(*a), mirroring.point(*b), MIRRORED_COLOR);
            }
        }
    }
    if entry.tool == Tools::Retarget {
        if let (Some((target, key)), Some(ray)) = (wizard.retarget.as_ref(), mouse.world_pos) {
            draw_picking_gizmos_filtered(&mut gizmos, &ray, &features, &wizard.hovered_point, &prior_features(&features, *target));
            for (base, at) in own_bases(&features, *target, key) {
                let hovered = wizard.hovered_point.as_ref().is_some_and(|(id, k, _)| id == target && *k == base);
                let color = if hovered { Color::srgb_u8(0, 230, 0) } else { Color::srgb_u8(200, 200, 200) };
                gizmos.sphere(Isometry3d::from_translation(at), 0.1, color);
            }
        }
        return;
    }
    let Some(cursor) = wizard.cursor else { return };
    match (entry.place)(&points, cursor) {
        Some(at) => {
            gizmos.sphere(Isometry3d::from_translation(at), 0.15, CURSOR_COLOR);
            (entry.preview)(&mut gizmos, &points, at);
        }
        None => {
            gizmos.sphere(Isometry3d::from_translation(cursor), 0.15, REFUSED_COLOR);
        }
    }
    wizard.picker.draw(&mut gizmos, &mouse, &features, Some(cursor));
}

/// The lines a picked feature is drawn by: its volumes' edges, else its pick
/// boxes turned with it, else a cross at its point.
fn outline(feature: &dyn FeatureTrait) -> Vec<(Vec3, Vec3)> {
    let volumes = feature.volumes();
    if !volumes.is_empty() {
        return volumes.iter().flat_map(|volume| volume.shape.edges().collect::<Vec<_>>()).collect();
    }
    let boxes = feature.pick_boxes();
    if !boxes.is_empty() {
        return boxes
            .iter()
            .flat_map(|pick_box| {
                let corner = |i: usize| {
                    let sign = Vec3::new(
                        if i & 1 == 0 { -1.0 } else { 1.0 },
                        if i & 2 == 0 { -1.0 } else { 1.0 },
                        if i & 4 == 0 { -1.0 } else { 1.0 },
                    );
                    pick_box.center + pick_box.rotation * (pick_box.half_extents * sign)
                };
                // The twelve edges join corners that differ in one sign.
                (0..8usize)
                    .flat_map(move |i| [1usize, 2, 4].into_iter().filter(move |bit| i & bit == 0).map(move |bit| (corner(i), corner(i | bit))))
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    let Ok(at) = feature.get_point("") else { return vec![] };
    const ARM: f32 = 0.25;
    [Vec3::X, Vec3::Y, Vec3::Z].into_iter().map(|axis| (at - axis * ARM, at + axis * ARM)).collect()
}

/// The copies a Mirror being made or re-picked would add: from its picked
/// center with the default symmetry, or the edited Mirror's own.
fn mirrorings(entry: &MapEntry, wizard: &MapWizard, features: &FeatureTimeline) -> Vec<crate::editor::mirror::Mirroring> {
    match entry.tool {
        Tools::Mirror => {
            let Some(at) = wizard.picks.iter().find_map(MapPick::at) else { return vec![] };
            let mut mirror = Mirror::new(PointRef::absolute(at.x, at.y, at.z));
            mirror.resolve_references(features.features_map());
            mirror.mirrorings().collect()
        }
        Tools::MirrorSources => wizard
            .sources_of
            .and_then(|id| features.features_map().get(&id))
            .and_then(|feature| feature.object().as_mirror().map(|mirror| mirror.mirrorings().collect()))
            .unwrap_or_default(),
        _ => vec![],
    }
}

/// Under the toolbar: Next or Done and Cancel first, so they stay put; then
/// how many picks the step under way holds, with every step's picks on hover;
/// then what that step still wants.
pub fn status_ui(ui: &mut egui::Ui, tool: &Tools, wizard: &mut MapWizard, features: &FeatureTimeline) {
    let Some(entry) = entry_for(tool) else { return };
    let steps = MapSteps { entry, features, editing: wizard.editing() };
    let filled = wizard::fill(&steps, &wizard.picks, &wizard.moved_on);
    let next = wizard::can_move_on(&steps, &filled);
    let ready = wizard::is_ready(&steps, &filled) || *tool == Tools::MirrorSources;
    let under_way = wizard::under_way(&steps, &filled);
    // A pick inside the point picker asks for its own point, whatever the step.
    let asked = if wizard.picker.reference().is_some() {
        get!("tools.pick.relative")
    } else if wizard.picker.is_picking() {
        get!("tools.pick.measured_from")
    } else {
        match under_way {
            Some(step) => get!(entry.steps[step].prompt),
            None => get!("tools.pick.more"),
        }
    };
    let current = under_way.unwrap_or(entry.steps.len().saturating_sub(1));
    let count = filled.has.get(current).map_or(0, |has| has.iter().filter(|pick| **pick != MapPick::Close).count());
    ui.horizontal(|ui| {
        // Always two segments, so neither moves as the first changes.
        let first = match next {
            true => Segment::new("chevron_right", get!("tools.wizard.next")),
            false => Segment::new("check_mark", get!("tools.wizard.done")).enabled(ready),
        };
        match toolbar::clicked(&toolbar::joined(ui, [first, Segment::new("trash_can", get!("tools.wizard.cancel"))])) {
            Some(1) => wizard.request = Some(Request::Cancel),
            Some(_) => wizard.request = Some(Request::Confirm),
            None => {}
        }
        ui.label(get!("tools.wizard.objects", "count", count)).on_hover_ui(|ui| {
            for (step, has) in entry.steps.iter().zip(&filled.has) {
                ui.label(egui::RichText::new(get!(step.name)).strong());
                ui.indent(step.name, |ui| {
                    for pick in has {
                        ui.label(describe(pick, features));
                    }
                });
            }
        });
        ui.label(egui::RichText::new(asked).color(ui.visuals().warn_fg_color));
    });
}

/// A pick as the list of picks shows it.
fn describe(pick: &MapPick, features: &FeatureTimeline) -> String {
    let name = |id: &FeatureId| features.features_map().get(id).map_or_else(|| id.to_string(), |feature| feature.label());
    match pick {
        MapPick::Point { at, absolute: true, .. } => {
            get!("tools.wizard.at", "x", format!("{:.2}", at.x), "y", format!("{:.2}", at.y), "z", format!("{:.2}", at.z))
        }
        MapPick::Point { point, .. } => match point.reference {
            Some(id) if point.point_key.is_empty() => name(&id),
            Some(id) => get!("tools.wizard.point_of", "point", point.point_key.clone(), "feature", name(&id)),
            None => get!("tools.wizard.at", "x", format!("{:.2}", point.x.value()), "y", format!("{:.2}", point.y.value()), "z", format!("{:.2}", point.z.value())),
        },
        MapPick::Feature(id) => name(id),
        MapPick::Close => get!("tools.wizard.closed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::global_point::GlobalPoint;

    fn point(at: Vec3) -> MapPick {
        MapPick::Point { point: PointRef::absolute(at.x, at.y, at.z), at, absolute: true }
    }

    #[test]
    fn every_tool_but_select_and_spectate_has_an_entry() {
        use strum::IntoEnumIterator;
        for tool in Tools::iter() {
            let wanted = !matches!(tool, Tools::Select | Tools::Spectate);
            assert_eq!(entry_for(&tool).is_some(), wanted, "{tool:?}");
        }
    }

    #[test]
    fn escape_walks_back_through_the_stairs_one_point_at_a_time() {
        let features = FeatureTimeline::default();
        let entry = entry_for(&Tools::Stairs).unwrap();
        let steps = MapSteps { entry, features: &features, editing: None };
        let mut picks = vec![point(Vec3::ZERO), point(Vec3::X * 4.0), point(Vec3::new(0.0, 0.0, -2.0))];
        let mut moved_on = vec![];
        assert_eq!(wizard::under_way(&steps, &wizard::fill(&steps, &picks, &moved_on)), Some(3), "the height is next");
        for remaining in [2, 1, 0] {
            assert!(matches!(wizard::back(&mut picks, &mut moved_on), wizard::Back::Took(_)));
            assert_eq!(picks.len(), remaining);
            assert_eq!(wizard::under_way(&steps, &wizard::fill(&steps, &picks, &moved_on)), Some(remaining));
        }
        assert!(matches!(wizard::back(&mut picks, &mut moved_on), wizard::Back::Exit));
    }

    #[test]
    fn a_mirror_takes_its_center_then_any_number_of_sources() {
        let mut features = FeatureTimeline::default();
        let a = features.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        let b = features.apply_feature(Box::new(GlobalPoint::new(2.0, 0.0, 0.0)));
        let entry = entry_for(&Tools::Mirror).unwrap();
        let steps = MapSteps { entry, features: &features, editing: None };
        let picks = vec![point(Vec3::ZERO), MapPick::Feature(a), MapPick::Feature(b)];
        let filled = wizard::fill(&steps, &picks, &[]);
        assert_eq!(filled.has[1].len(), 2);
        assert!(wizard::is_ready(&steps, &filled));
        assert!(!wizard::is_finished(&steps, &filled), "it waits for Enter in case more sources follow");
        assert!(mirror_build(&Tools::Mirror, &filled.has, &features).is_some());
    }

    #[test]
    fn a_far_corner_clicked_at_the_cursor_is_a_size_from_the_first() {
        let features = FeatureTimeline::default();
        let entry = entry_for(&Tools::Room).unwrap();
        let steps = MapSteps { entry, features: &features, editing: None };
        let picks = vec![point(Vec3::ZERO), point(Vec3::new(4.0, 3.0, 2.0))];
        let filled = wizard::fill(&steps, &picks, &[]);
        assert!(wizard::is_finished(&steps, &filled));
        assert!((entry.build)(&Tools::Room, &filled.has, &features).is_some());
    }

    #[test]
    fn a_chain_of_corners_closes_on_its_first_only_with_three() {
        let features = FeatureTimeline::default();
        let entry = entry_for(&Tools::Footprint).unwrap();
        let steps = MapSteps { entry, features: &features, editing: None };
        let two = vec![point(Vec3::ZERO), point(Vec3::X), MapPick::Close];
        assert!(!wizard::is_ready(&steps, &wizard::fill(&steps, &two, &[])), "two corners do not close");
        let three = vec![point(Vec3::ZERO), point(Vec3::X), point(Vec3::Z), MapPick::Close];
        let filled = wizard::fill(&steps, &three, &[]);
        assert!(wizard::is_finished(&steps, &filled));
        assert!((entry.build)(&Tools::Footprint, &filled.has, &features).is_some());
    }

    #[test]
    fn re_picked_sources_come_from_before_the_mirror() {
        let mut features = FeatureTimeline::default();
        let before = features.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        let mirror = features.apply_feature(Box::new(Mirror::new(PointRef::absolute(0.0, 0.0, 0.0))));
        let after = features.apply_feature(Box::new(GlobalPoint::new(2.0, 0.0, 0.0)));
        let entry = entry_for(&Tools::MirrorSources).unwrap();
        let steps = MapSteps { entry, features: &features, editing: Some(mirror) };
        let filled = wizard::fill(&steps, &[MapPick::Feature(before), MapPick::Feature(after)], &[]);
        assert!(filled.has[0] == vec![MapPick::Feature(before)], "the feature after the mirror is not taken");
    }

    /// The driver with no camera, so no click lands: what seeding and Escape do.
    fn driven(tool: Tools, features: FeatureTimeline, actions: Actions) -> App {
        let mut app = App::new();
        app.insert_resource(actions);
        let mut app = driven_with(app, tool, features);
        app.update();
        app
    }

    fn driven_with(mut app: App, tool: Tools, features: FeatureTimeline) -> App {
        app.init_resource::<Actions>();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(AppMode::Editor)
            .insert_state(tool)
            .insert_resource(features)
            .init_resource::<MapWizard>()
            .init_resource::<CurrentMouseInput>()
            .init_resource::<GizmoVisibility>()
            .init_resource::<crate::tool::map_products::MapProducts>()
            .add_message::<EditEvent>()
            .add_systems(Update, (start_on_a_new_tool.run_if(state_changed::<Tools>), run_map_wizard).chain());
        app
    }

    #[test]
    fn a_tool_picked_again_does_not_copy_what_it_just_made() {
        let mut features = FeatureTimeline::default();
        let made = features.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        features.select(Some(made));
        let app = driven(Tools::Point, features, Actions::default());
        assert_eq!(app.world().resource::<FeatureTimeline>().features_map().len(), 1, "nothing made without a click");
        assert!(app.world().resource::<MapWizard>().picks.is_empty());
    }

    #[test]
    fn a_selected_point_is_a_rooms_first_corner_and_escape_takes_it_back() {
        let mut features = FeatureTimeline::default();
        let corner = features.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        features.select(Some(corner));
        let mut app = driven(Tools::Room, features, Actions::default());
        assert_eq!(app.world().resource::<MapWizard>().picks.len(), 1, "the selected point");
        let mut escape = Actions::default();
        escape.press(Action::Cancel);
        app.insert_resource(escape);
        app.update();
        assert!(app.world().resource::<MapWizard>().picks.is_empty());
        app.update();
        // The state changes in the frame after it is asked for.
        app.update();
        assert_eq!(*app.world().resource::<State<Tools>>().get(), Tools::Select, "a second Escape leaves");
    }

    #[test]
    fn a_named_point_picked_with_shift_is_a_rooms_first_corner() {
        let mut features = FeatureTimeline::default();
        let room = features.apply_feature(Box::new(crate::editor::editor_room::EditorRoom::from_point_refs(
            PointRef::absolute(0.0, 0.0, 0.0),
            PointRef::absolute(4.0, 3.0, 4.0),
        )));
        let key = features.features_map()[&room].object().available_point_keys()[0].0.clone();
        let mut app = App::new();
        app.insert_resource(crate::tool::selection::SelectionState {
            point: Some(crate::tool::map_products::PointId { feature: room, key: key.clone() }),
            ..default()
        });
        let mut app = driven_with(app, Tools::Room, features);
        app.update();
        let wizard = app.world().resource::<MapWizard>();
        assert_eq!(wizard.picks.len(), 1);
        assert!(matches!(&wizard.picks[0], MapPick::Point { point, .. } if point.reference == Some(room) && point.point_key == key));
        assert!(app.world().resource::<crate::tool::selection::SelectionState>().point.is_none(), "used up");
    }

    #[test]
    fn a_mirror_takes_a_named_point_as_its_center_and_selected_rooms_as_sources() {
        let mut features = FeatureTimeline::default();
        let center = features.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
        let room = features.apply_feature(Box::new(crate::editor::editor_room::EditorRoom::from_point_refs(
            PointRef::absolute(1.0, 0.0, 1.0),
            PointRef::absolute(4.0, 3.0, 4.0),
        )));
        features.select(Some(room));
        let key = features.features_map()[&center].object().available_point_keys()[0].0.clone();
        let mut app = App::new();
        app.insert_resource(crate::tool::selection::SelectionState {
            point: Some(crate::tool::map_products::PointId { feature: center, key }),
            ..default()
        });
        let mut app = driven_with(app, Tools::Mirror, features);
        app.update();
        let wizard = app.world().resource::<MapWizard>();
        assert_eq!(wizard.picks.len(), 2);
        assert!(matches!(&wizard.picks[0], MapPick::Point { point, .. } if point.reference == Some(center)));
        assert!(wizard.picks[1] == MapPick::Feature(room));
        assert_eq!(app.world().resource::<FeatureTimeline>().features_map().len(), 2, "it waits for Enter");
    }

    #[test]
    fn a_named_point_picked_with_shift_may_finish_a_point_tool() {
        let mut features = FeatureTimeline::default();
        let corner = features.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 1.0)));
        let key = features.features_map()[&corner].object().available_point_keys()[0].0.clone();
        let mut app = App::new();
        app.insert_resource(crate::tool::selection::SelectionState {
            point: Some(crate::tool::map_products::PointId { feature: corner, key }),
            ..default()
        });
        let mut app = driven_with(app, Tools::SpawnPoint, features);
        app.update();
        assert_eq!(app.world().resource::<FeatureTimeline>().features_map().len(), 2, "a spawn at the corner");
    }

    #[test]
    fn a_picked_feature_is_outlined_by_its_walls_its_box_or_its_point() {
        let mut features = FeatureTimeline::default();
        let room = features.apply_feature(Box::new(crate::editor::editor_room::EditorRoom::from_point_refs(
            PointRef::absolute(0.0, 0.0, 0.0),
            PointRef::absolute(4.0, 3.0, 4.0),
        )));
        let point = features.apply_feature(Box::new(GlobalPoint::new(1.0, 0.0, 0.0)));
        let map = features.features_map();
        assert!(!outline(map[&room].object()).is_empty(), "a room by its walls");
        assert_eq!(outline(map[&point].object()).len(), 3, "a point by a cross");
        let mirroring = crate::editor::mirror::Mirroring { motion: crate::editor::mirror::Motion::ReflectX, center: Vec3::ZERO };
        assert_eq!(mirroring.point(Vec3::new(1.0, 0.0, 0.0)), Vec3::new(-1.0, 0.0, 0.0), "copies are drawn across the plane");
    }
}
