//! The nav graph and its fields drawn over the floor for mappers, the probe and the baseline, and
//! the Bakes panel section that picks what is drawn. See "The overlays" in documentation/bots.md.
//!
//! Every scale is saturated at a fixed number in the field's own units, never a percentile, so a
//! wall placed in one corner does not recolor the rest of the map.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::ecs::system::SystemParam;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy_egui::egui;

use crate::common::app_mode::AppMode;
use crate::common::render_layers;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::editor_only;
use crate::game::collision::CollisionWorld;
use crate::game::nav::{CellId, LinkKind, NavGraph, SPACING};
use crate::game::nav_field::{self, Field};
use crate::game::nav_rating::{MapStatistics, Rating};
use crate::get;
use crate::tool::nav_bake::{BakeNav, ClearNav, NavBake};

/// Above the floor, in meters, so the overlay does not fight the floor for depth.
const CELL_LIFT: f32 = 0.03;
const LINK_LIFT: f32 = 0.05;
/// Of [`SPACING`], so neighboring cells read as separate squares.
const CELL_SIZE: f32 = 0.8;
/// Meters, the length of each stroke of a one-way link's arrowhead.
const ARROWHEAD: f32 = 0.2;

const CELL: Color = Color::srgb(0.2, 0.6, 0.9);
const HURT_CELL: Color = Color::srgb(1.0, 0.45, 0.1);
const WALK: Color = Color::srgb(0.85, 0.9, 0.95);
const STEP: Color = Color::srgb(1.0, 0.85, 0.2);
const DROP: Color = Color::srgb(0.9, 0.3, 0.9);
const JUMP: Color = Color::srgb(0.3, 0.95, 0.4);

/// Share of routes at which the traffic scale is full.
const TRAFFIC_FULL: f32 = 0.5;

/// The low end of every single-hue scale.
const DARK: Color = Color::srgb(0.06, 0.06, 0.08);
/// A field not baked yet, or a cell the baseline did not have.
const UNKNOWN: Color = Color::srgb(0.3, 0.3, 0.3);
const SEES: Color = Color::srgb(0.1, 0.95, 0.2);
const SEEN: Color = Color::srgb(1.0, 0.15, 0.1);
const PEEK: Color = Color::srgb(0.2, 0.9, 1.0);
const HIGH: Color = Color::srgb(0.35, 0.5, 1.0);
const NEAR: Color = Color::srgb(1.0, 0.9, 0.2);
const BUSY: Color = Color::srgb(0.95, 0.5, 1.0);
const RATED: Color = Color::srgb(1.0, 1.0, 0.85);
const MORE: Color = Color::srgb(0.25, 0.55, 1.0);
const LESS: Color = Color::srgb(1.0, 0.55, 0.1);
const PROBED: Color = Color::WHITE;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shown {
    #[default]
    Off,
    CellsAndLinks,
    Sight,
    Exposure,
    /// Bivariate: green sees and is not seen, red is seen and does not see.
    SightExposure,
    Advantage,
    /// Peek spots: much sight with little exposure close by.
    Cover,
    Danger,
    Height,
    Traffic,
    /// Seconds to the pickup at this index.
    Distance(usize),
    Rating(Rating),
}

/// Whose eye and what of the body the sightline overlays and the probe are of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Body {
    #[default]
    Standing,
    /// A crouched eye for sight, a crouched body for exposure.
    Crouched,
    /// The floor at a standing body's feet: where splash lands.
    Feet,
}

impl Body {
    const ALL: [Body; 3] = [Body::Standing, Body::Crouched, Body::Feet];

    fn label(self) -> String {
        match self {
            Body::Standing => get!("nav.body.standing"),
            Body::Crouched => get!("nav.body.crouched"),
            Body::Feet => get!("nav.body.feet"),
        }
    }

    fn sight(self) -> Field {
        match self {
            Body::Standing => Field::Sight,
            Body::Crouched => Field::CrouchedSight,
            Body::Feet => Field::FeetSight,
        }
    }

    fn exposure(self) -> Field {
        match self {
            Body::Standing => Field::Exposure,
            Body::Crouched => Field::CrouchedExposure,
            Body::Feet => Field::FeetExposure,
        }
    }

    fn advantage(self) -> Field {
        match self {
            Body::Standing => Field::Advantage,
            Body::Crouched => Field::CrouchedAdvantage,
            Body::Feet => Field::FeetAdvantage,
        }
    }
}

impl Shown {
    const LISTED: [Shown; 10] = [
        Shown::Off,
        Shown::CellsAndLinks,
        Shown::Sight,
        Shown::Exposure,
        Shown::SightExposure,
        Shown::Advantage,
        Shown::Cover,
        Shown::Danger,
        Shown::Height,
        Shown::Traffic,
    ];

    fn label(self) -> String {
        match self {
            Shown::Off => get!("nav.overlay.off"),
            Shown::CellsAndLinks => get!("nav.overlay.cells_and_links"),
            Shown::Sight => get!("nav.overlay.sight"),
            Shown::Exposure => get!("nav.overlay.exposure"),
            Shown::SightExposure => get!("nav.overlay.sight_exposure"),
            Shown::Advantage => get!("nav.overlay.advantage"),
            Shown::Cover => get!("nav.overlay.cover"),
            Shown::Danger => get!("nav.overlay.danger"),
            Shown::Height => get!("nav.overlay.height"),
            Shown::Traffic => get!("nav.overlay.traffic"),
            Shown::Distance(_) => get!("nav.overlay.distance"),
            Shown::Rating(_) => get!("nav.overlay.rating"),
        }
    }

    /// A single number per cell, which is what a baseline can be taken of.
    fn scalar(self) -> bool {
        !matches!(self, Shown::Off | Shown::CellsAndLinks | Shown::SightExposure | Shown::Rating(_))
    }
}

#[derive(Resource, Clone, Debug, PartialEq)]
pub struct NavOverlay {
    pub shown: Shown,
    pub body: Body,
    /// Square meters at which a sightline scale is full.
    pub area: f32,
    /// Seconds at which a distance scale is empty.
    pub seconds: f32,
    /// Meters at which the height scale is full.
    pub height: f32,
    /// Hovering a cell lights what it sees and what sees it.
    pub probe: bool,
    /// Draws the difference from the pinned baseline rather than the field.
    pub against_baseline: bool,
    /// Shows the overlay to the play camera too.
    pub in_play: bool,
}

impl Default for NavOverlay {
    fn default() -> Self {
        NavOverlay {
            shown: Shown::Off,
            body: Body::Standing,
            area: 300.0,
            seconds: 10.0,
            height: 4.0,
            probe: false,
            against_baseline: false,
            in_play: false,
        }
    }
}

impl NavOverlay {
    fn saturation(&self) -> f32 {
        match self.shown {
            Shown::Distance(_) => self.seconds,
            Shown::Height => self.height,
            Shown::Danger => 1.0,
            Shown::Traffic => TRAFFIC_FULL,
            _ => self.area,
        }
    }

    /// The number a scalar overlay draws, `None` where it is not baked.
    fn value(&self, graph: &NavGraph, id: CellId) -> Option<f32> {
        let field = match self.shown {
            Shown::Sight => self.body.sight(),
            Shown::Exposure => self.body.exposure(),
            Shown::Advantage => self.body.advantage(),
            Shown::Cover => Field::Cover,
            Shown::Danger => Field::Knockback,
            Shown::Height => Field::Height,
            Shown::Traffic => Field::Traffic,
            Shown::Distance(pickup) => Field::Distance(pickup),
            _ => return None,
        };
        graph.field(id, field)
    }

    fn color(&self, graph: &NavGraph, id: CellId) -> Color {
        let full = self.saturation();
        let share = |value: f32| (value / full).clamp(0.0, 1.0);
        let (sight, exposure) = (graph.field(id, self.body.sight()), graph.field(id, self.body.exposure()));
        let drawn = match self.shown {
            Shown::Off | Shown::CellsAndLinks => None,
            Shown::Sight => self.value(graph, id).map(|value| ramp(share(value), SEES)),
            Shown::Exposure => self.value(graph, id).map(|value| ramp(share(value), SEEN)),
            Shown::SightExposure => sight.zip(exposure).map(|(sight, exposure)| bivariate(share(sight), share(exposure))),
            Shown::Advantage => self.value(graph, id).map(|value| diverging(value / full, SEES, SEEN)),
            Shown::Cover => graph.field(id, Field::Sight).zip(self.value(graph, id)).map(|(sight, cover)| ramp(share(sight) * (1.0 - share(cover)), PEEK)),
            Shown::Danger if graph.cells()[id as usize].hurt > 0 => Some(HURT_CELL),
            Shown::Danger => self.value(graph, id).map(|value| ramp(share(value), SEEN)),
            Shown::Height => self.value(graph, id).map(|value| ramp(share(value), HIGH)),
            Shown::Traffic => self.value(graph, id).map(|value| ramp(share(value), BUSY)),
            Shown::Distance(_) => self.value(graph, id).filter(|value| value.is_finite()).map(|value| ramp(1.0 - share(value), NEAR)),
            Shown::Rating(rating) => rating.holds(graph, id).map(|holds| if holds { RATED } else { DARK }),
        };
        drawn.unwrap_or(UNKNOWN)
    }
}

fn ramp(share: f32, hue: Color) -> Color {
    DARK.mix(&hue, share)
}

/// Sight on green and exposure on red, so sees-and-is-seen is yellow and neither is dark.
fn bivariate(sight: f32, exposure: f32) -> Color {
    let low = DARK.to_srgba();
    Color::srgb(low.red.max(exposure), low.green.max(sight), low.blue)
}

/// `value` from -1 to 1: `up` above zero, `down` below.
fn diverging(value: f32, up: Color, down: Color) -> Color {
    let value = value.clamp(-1.0, 1.0);
    if value >= 0.0 { ramp(value, up) } else { ramp(-value, down) }
}

/// A cell the way a baseline names it: by place, since cell ids change with every bake.
fn cell_key(graph: &NavGraph, id: CellId) -> (IVec2, i32) {
    let cell = &graph.cells()[id as usize];
    (cell.column, (cell.feet.y * 10.0).round() as i32)
}

/// One overlay's numbers as they stood when pinned.
#[derive(Resource, Default)]
pub struct NavBaseline {
    pinned: Option<(Shown, Body, HashMap<(IVec2, i32), f32>)>,
    /// Bumped on every pin and clear, for what draws it.
    version: u64,
}

impl NavBaseline {
    fn pin(&mut self, graph: &NavGraph, overlay: &NavOverlay) {
        let values = (0..graph.cells().len() as CellId).filter_map(|id| Some((cell_key(graph, id), overlay.value(graph, id)?))).collect();
        self.pinned = Some((overlay.shown, overlay.body, values));
        self.version += 1;
    }

    fn clear(&mut self) {
        self.pinned = None;
        self.version += 1;
    }

    /// The pinned numbers, if they are of what `overlay` draws.
    fn of(&self, overlay: &NavOverlay) -> Option<&HashMap<(IVec2, i32), f32>> {
        let (shown, body, values) = self.pinned.as_ref()?;
        (*shown == overlay.shown && *body == overlay.body).then_some(values)
    }
}

/// The cell the pointer is over while probing.
#[derive(Resource, Default, PartialEq)]
pub struct NavProbe(pub Option<CellId>);

#[derive(Component)]
struct NavOverlayMesh;

/// A quad per cell, in whatever color `color` gives it.
fn cell_mesh(graph: &NavGraph, mut color: impl FnMut(CellId) -> Color) -> Mesh {
    let half = SPACING * CELL_SIZE / 2.0;
    let cells = graph.cells();
    let mut positions = Vec::with_capacity(cells.len() * 4);
    let mut colors = Vec::with_capacity(cells.len() * 4);
    let mut indices = Vec::with_capacity(cells.len() * 6);
    for (id, cell) in cells.iter().enumerate() {
        let center = cell.feet + Vec3::Y * CELL_LIFT;
        let color = color(id as CellId).to_linear().to_f32_array();
        let first = positions.len() as u32;
        for (x, z) in [(-half, -half), (-half, half), (half, half), (half, -half)] {
            positions.push((center + Vec3::new(x, 0.0, z)).to_array());
            colors.push(color);
        }
        indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(indices))
}

/// A line per pair of linked cells, drawn once whichever way it was found, with an arrowhead at the
/// far end if it goes only one way. On a flat floor a walk two columns long only repeats two short
/// ones, so those are left out.
fn link_mesh(graph: &NavGraph) -> Mesh {
    let cells = graph.cells();
    let mut positions = Vec::new();
    let mut colors = Vec::new();
    for (from, cell) in cells.iter().enumerate() {
        for link in graph.links(from as u32) {
            let to = &cells[link.to as usize];
            let reverse = graph.links(link.to).iter().any(|back| back.to as usize == from);
            if reverse && link.to as usize <= from {
                continue;
            }
            let span = (to.column - cell.column).abs().max_element();
            if span > 1 && link.kind == LinkKind::Walk {
                continue;
            }
            let color = match link.kind {
                LinkKind::Walk => WALK,
                LinkKind::Step => STEP,
                LinkKind::Drop => DROP,
                LinkKind::Jump => JUMP,
            }
            .to_linear()
            .to_f32_array();
            let (start, end) = (cell.feet + Vec3::Y * LINK_LIFT, to.feet + Vec3::Y * LINK_LIFT);
            let mut segments = vec![(start, end)];
            if !reverse {
                let back = (start - end).normalize_or_zero() * ARROWHEAD;
                let side = back.cross(Vec3::Y).normalize_or_zero() * ARROWHEAD * 0.5;
                segments.extend([(end, end + back + side), (end, end + back - side)]);
            }
            for (a, b) in segments {
                positions.extend([a.to_array(), b.to_array()]);
                colors.extend([color, color]);
            }
        }
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
}

/// The probed cell in white, what it sees in green, what sees it in red, both in yellow, from the
/// sightlines the bake stored. `None` until they are baked.
fn probe_colors(graph: &NavGraph, probed: CellId, body: Body) -> Option<Vec<Color>> {
    let lines = &graph.fields().sightlines;
    let here = *lines.viewpoint_of.get(probed as usize)? as usize;
    let (sees, seen_by) = match body {
        Body::Crouched => (&lines.crouched_eye, &lines.crouched_body),
        Body::Standing | Body::Feet => (&lines.standing, &lines.standing),
    };
    let counts = |seen: &&nav_field::Seen| if body == Body::Feet { seen.feet } else { seen.samples > 0 };
    let mut by_viewpoint = vec![(false, false); lines.viewpoints.len()];
    for seen in sees[here].iter().filter(counts) {
        by_viewpoint[seen.viewpoint as usize].0 = true;
    }
    for (eye, row) in seen_by.iter().enumerate() {
        if row.iter().filter(counts).any(|seen| seen.viewpoint as usize == here) {
            by_viewpoint[eye].1 = true;
        }
    }
    let colors = lines
        .viewpoint_of
        .iter()
        .enumerate()
        .map(|(id, viewpoint)| match by_viewpoint[*viewpoint as usize] {
            _ if id as CellId == probed => PROBED,
            (true, true) => bivariate(1.0, 1.0),
            (true, false) => SEES,
            (false, true) => SEEN,
            (false, false) => DARK,
        })
        .collect();
    Some(colors)
}

fn layers(overlay: &NavOverlay, mode: &AppMode) -> RenderLayers {
    if overlay.in_play && *mode == AppMode::Play { editor_only().with(render_layers::WORLD) } else { editor_only() }
}

#[derive(SystemParam)]
struct Drawing<'w, 's> {
    existing: Query<'w, 's, Entity, With<NavOverlayMesh>>,
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

/// Rebuilt only when what is drawn changes.
fn draw_nav_overlay(
    overlay: Res<NavOverlay>,
    nav: Res<NavBake>,
    baseline: Res<NavBaseline>,
    probe: Res<NavProbe>,
    mode: Res<State<AppMode>>,
    mut drawn: Local<Option<(NavOverlay, u64, u64, Option<CellId>, AppMode)>>,
    mut drawing: Drawing,
) {
    let probed = probe.0.filter(|_| overlay.probe);
    let wanted = (overlay.clone(), nav.generation(), baseline.version, probed, *mode.get());
    if drawn.as_ref() == Some(&wanted) {
        return;
    }
    *drawn = Some(wanted);
    for entity in &drawing.existing {
        drawing.commands.entity(entity).despawn();
    }
    let graph = nav.graph();
    if overlay.shown == Shown::Off && probed.is_none() || graph.cells().is_empty() {
        return;
    }
    let mut meshes = Vec::new();
    if let Some(colors) = probed.and_then(|probed| probe_colors(graph, probed, overlay.body)) {
        meshes.push(cell_mesh(graph, |id| colors[id as usize]));
    } else if overlay.shown == Shown::CellsAndLinks {
        meshes.push(cell_mesh(graph, |id| if graph.cells()[id as usize].hurt > 0 { HURT_CELL } else { CELL }));
        meshes.push(link_mesh(graph));
    } else if let Some(pinned) = baseline.of(&overlay).filter(|_| overlay.against_baseline) {
        let full = overlay.saturation();
        meshes.push(cell_mesh(graph, |id| {
            let before = pinned.get(&cell_key(graph, id)).copied();
            let now = overlay.value(graph, id);
            match before.zip(now) {
                Some((before, now)) if before.is_finite() && now.is_finite() => diverging((now - before) / full, MORE, LESS),
                _ => UNKNOWN,
            }
        }));
    } else if overlay.shown != Shown::Off {
        meshes.push(cell_mesh(graph, |id| overlay.color(graph, id)));
    }
    let material = drawing.materials.add(StandardMaterial { unlit: true, cull_mode: None, ..default() });
    for mesh in meshes {
        let mesh = drawing.meshes.add(mesh);
        drawing.commands.spawn((
            Name::new("Nav overlay"),
            NavOverlayMesh,
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            NotShadowCaster,
            layers(&overlay, mode.get()),
        ));
    }
}

/// The cell under the pointer in a viewport, found where the pointer's ray first meets a wall.
fn probe_under_the_pointer(
    overlay: Res<NavOverlay>,
    mouse: Res<CurrentMouseInput>,
    world: Res<CollisionWorld>,
    nav: Res<NavBake>,
    mut probe: ResMut<NavProbe>,
) {
    let under = mouse.world_pos.filter(|_| overlay.probe).and_then(|ray| {
        let distance = world.ray_distance(&ray, 500.0)?;
        nav.graph().cell_at(ray.get_point(distance))
    });
    probe.set_if_neq(NavProbe(under));
}

/// A debug key: the overlay over the floor while playing, for tuning bots against.
fn show_in_play(keys: Res<ButtonInput<KeyCode>>, mut overlay: ResMut<NavOverlay>) {
    if keys.just_pressed(KeyCode::F7) {
        overlay.in_play = !overlay.in_play;
    }
}

/// What the nav section asked for this frame.
pub struct NavCommands {
    pub auto: bool,
    pub overlay: NavOverlay,
    pub bake: bool,
    pub clear: bool,
    pin: bool,
    unpin: bool,
    pinned: bool,
    pickups: Vec<String>,
    cells: usize,
    links: usize,
    statistics: MapStatistics,
}

/// The nav section's reads and writes, bundled to stay inside the panels' parameter limit.
#[derive(SystemParam)]
pub struct NavPanel<'w> {
    nav: ResMut<'w, NavBake>,
    overlay: ResMut<'w, NavOverlay>,
    baseline: ResMut<'w, NavBaseline>,
    bake: MessageWriter<'w, BakeNav>,
    clear: MessageWriter<'w, ClearNav>,
}

impl NavPanel<'_> {
    pub fn commands(&self) -> NavCommands {
        let graph = self.nav.graph();
        let links = (0..graph.cells().len() as u32).map(|id| graph.links(id).len()).sum();
        NavCommands {
            auto: self.nav.auto(),
            overlay: self.overlay.clone(),
            bake: false,
            clear: false,
            pin: false,
            unpin: false,
            pinned: self.baseline.pinned.is_some(),
            pickups: graph.fields().pickups.iter().map(|pickup| pickup.label.clone()).collect(),
            cells: graph.cells().len(),
            links,
            statistics: self.nav.statistics().clone(),
        }
    }

    pub fn apply(&mut self, commands: &NavCommands) {
        if commands.auto != self.nav.auto() {
            self.nav.set_auto(commands.auto);
        }
        if commands.overlay != *self.overlay {
            *self.overlay = commands.overlay.clone();
        }
        if commands.pin {
            self.baseline.pin(self.nav.graph(), &commands.overlay);
        }
        if commands.unpin {
            self.baseline.clear();
        }
        if commands.bake {
            self.bake.write(BakeNav);
        }
        if commands.clear {
            self.clear.write(ClearNav);
        }
    }
}

/// The nav graph's part of the Bakes panel.
pub fn section(ui: &mut egui::Ui, commands: &mut NavCommands) {
    ui.separator();
    ui.strong(get!("nav.title"));
    ui.vertical(|ui| {
        let overlay = &mut commands.overlay;
        for shown in Shown::LISTED {
            ui.radio_value(&mut overlay.shown, shown, shown.label());
        }
        if !commands.pickups.is_empty() {
            ui.horizontal(|ui| {
                let picked = match overlay.shown {
                    Shown::Distance(pickup) => pickup.min(commands.pickups.len() - 1),
                    _ => 0,
                };
                if ui.radio(matches!(overlay.shown, Shown::Distance(_)), Shown::Distance(picked).label()).clicked() {
                    overlay.shown = Shown::Distance(picked);
                }
                egui::ComboBox::from_id_salt("nav_pickup").selected_text(commands.pickups[picked].clone()).show_ui(ui, |ui| {
                    for (index, label) in commands.pickups.iter().enumerate() {
                        ui.selectable_value(&mut overlay.shown, Shown::Distance(index), label);
                    }
                });
            });
        }
        ui.horizontal(|ui| {
            let picked = match overlay.shown {
                Shown::Rating(rating) => rating,
                _ => Rating::Cramped,
            };
            if ui.radio(matches!(overlay.shown, Shown::Rating(_)), Shown::Rating(picked).label()).clicked() {
                overlay.shown = Shown::Rating(picked);
            }
            egui::ComboBox::from_id_salt("nav_rating").selected_text(picked.label()).show_ui(ui, |ui| {
                for rating in Rating::ALL {
                    ui.selectable_value(&mut overlay.shown, Shown::Rating(rating), rating.label());
                }
            });
        });
        ui.horizontal(|ui| {
            for body in Body::ALL {
                ui.radio_value(&mut overlay.body, body, body.label());
            }
        });
        egui::Grid::new("nav_saturation").num_columns(2).show(ui, |ui| {
            ui.label(get!("nav.saturation.area"));
            ui.add(egui::DragValue::new(&mut overlay.area).range(1.0..=10_000.0).suffix(" m²"));
            ui.end_row();
            ui.label(get!("nav.saturation.seconds"));
            ui.add(egui::DragValue::new(&mut overlay.seconds).range(0.5..=120.0).speed(0.1).suffix(" s"));
            ui.end_row();
            ui.label(get!("nav.saturation.height"));
            ui.add(egui::DragValue::new(&mut overlay.height).range(0.5..=50.0).speed(0.1).suffix(" m"));
            ui.end_row();
        });
        ui.checkbox(&mut overlay.probe, get!("nav.probe"));
        ui.horizontal(|ui| {
            if ui.add_enabled(overlay.shown.scalar(), egui::Button::new(get!("nav.baseline.pin"))).clicked() {
                commands.pin = true;
            }
            if commands.pinned && ui.small_button("x").clicked() {
                commands.unpin = true;
            }
            ui.checkbox(&mut overlay.against_baseline, get!("nav.baseline.difference"));
        });
        ui.separator();
        ui.checkbox(&mut commands.auto, get!("nav.auto"));
        ui.horizontal(|ui| {
            if ui.add_enabled(!commands.auto, egui::Button::new(get!("nav.bake"))).clicked() {
                commands.bake = true;
            }
            if ui.small_button("x").clicked() {
                commands.clear = true;
            }
        });
        ui.label(get!("nav.summary", "cells", commands.cells, "links", commands.links));
        statistics(ui, &commands.statistics);
    });
}

fn statistics(ui: &mut egui::Ui, statistics: &MapStatistics) {
    egui::CollapsingHeader::new(get!("nav.statistics.title")).id_salt("nav_statistics").show(ui, |ui| {
        egui::Grid::new("nav_statistics_grid").num_columns(2).show(ui, |ui| {
            ui.label(get!("nav.statistics.floor"));
            ui.label(format!("{:.0} m²", statistics.floor));
            ui.end_row();
            ui.label(get!("nav.statistics.rise"));
            ui.label(format!("{:.1} m", statistics.rise));
            ui.end_row();
            for (rating, share) in Rating::ALL.iter().zip(&statistics.shares) {
                ui.label(rating.label());
                ui.label(share.map_or_else(|| "–".to_string(), |share| format!("{:.0}%", share * 100.0)));
                ui.end_row();
            }
        });
    });
}

pub struct NavOverlayPlugin;

impl Plugin for NavOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavOverlay>()
            .init_resource::<NavBaseline>()
            .init_resource::<NavProbe>()
            // Ungated: the overlay is on a layer only the editor viewports draw, unless asked for in Play.
            .add_systems(Update, draw_nav_overlay)
            .add_systems(Update, probe_under_the_pointer.before(draw_nav_overlay).run_if(in_state(AppMode::Editor)))
            .add_systems(Update, show_in_play.run_if(in_state(AppMode::Play)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::movement_rules::MovementRules;
    use crate::common::world_surface::Volume;
    use crate::game::{nav, nav_field};
    use crate::tool::room::Room;

    fn graph_of(volumes: &[Volume]) -> NavGraph {
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(volumes);
        let rules = MovementRules::default();
        let mut graph = nav::graph(&world, &faces, &[], &rules, &[]);
        let fields = nav_field::bake(&world, &graph, &[], &rules, Vec::new(), Vec::new());
        graph.set_fields(fields);
        graph
    }

    fn rooms(rooms: &[Room]) -> NavGraph {
        graph_of(&rooms.iter().cloned().map(Volume::air).collect::<Vec<_>>())
    }

    fn vertex_count(mesh: &Mesh) -> usize {
        mesh.attribute(Mesh::ATTRIBUTE_POSITION).map_or(0, |positions| positions.len())
    }

    fn colors(mesh: &Mesh) -> &[[f32; 4]] {
        let Some(bevy::mesh::VertexAttributeValues::Float32x4(colors)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR) else {
            panic!("no colors");
        };
        colors
    }

    /// Breaks if a cell is drawn twice or not at all.
    #[test]
    fn every_cell_is_one_quad() {
        let graph = rooms(&[Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))]);
        let mesh = cell_mesh(&graph, |_| CELL);
        assert_eq!(vertex_count(&mesh), graph.cells().len() * 4);
        assert_eq!(mesh.indices().map_or(0, |indices| indices.len()), graph.cells().len() * 6);
    }

    /// Breaks if a two-way link is drawn from both ends, or a flat floor shows its long links.
    #[test]
    fn a_flat_floor_draws_each_neighbor_once() {
        let graph = rooms(&[Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))]);
        let cells = graph.cells();
        let neighbors = (0..cells.len())
            .flat_map(|a| (a + 1..cells.len()).map(move |b| (a, b)))
            .filter(|&(a, b)| (cells[a].column - cells[b].column).abs().max_element() == 1)
            .count();
        assert_eq!(vertex_count(&link_mesh(&graph)), neighbors * 2);
    }

    /// Breaks if a step across a lip is hidden with the long walks.
    #[test]
    fn a_step_is_drawn() {
        let graph = rooms(&[Room::new(Vec3::ZERO, Vec3::new(3.0, 3.0, 3.0)), Room::new(Vec3::new(3.0, 0.4, 0.0), Vec3::new(6.0, 3.4, 3.0))]);
        let step = STEP.to_linear().to_f32_array();
        assert!(colors(&link_mesh(&graph)).contains(&step), "no step link drawn across a 0.4 m lip");
    }

    /// Breaks if a one-way drop loses its arrowhead: a line and two strokes.
    #[test]
    fn a_drop_is_an_arrow() {
        let graph = rooms(&[Room::new(Vec3::ZERO, Vec3::new(3.0, 4.0, 3.0)), Room::new(Vec3::new(3.0, -1.5, 0.0), Vec3::new(6.0, 4.0, 3.0))]);
        let drop = DROP.to_linear().to_f32_array();
        let drops = colors(&link_mesh(&graph)).iter().filter(|color| **color == drop).count();
        assert!(drops > 0, "no drop drawn off a 1.5 m ledge");
        assert_eq!(drops % 6, 0, "a drop drawn without its arrowhead");
    }

    /// Breaks if a scale is normalized to the map: a room added far away must not recolor a cell
    /// it cannot see.
    #[test]
    fn a_room_out_of_sight_does_not_recolor_a_cell() {
        let here = Room::new(Vec3::ZERO, Vec3::new(6.0, 3.0, 6.0));
        let alone = rooms(&[here.clone()]);
        let beside = rooms(&[here, Room::new(Vec3::new(100.0, 0.0, 0.0), Vec3::new(120.0, 3.0, 20.0))]);
        let overlay = NavOverlay { shown: Shown::Exposure, ..default() };
        let feet = Vec3::new(2.25, 0.0, 2.25);
        let color = |graph: &NavGraph| overlay.color(graph, graph.cell_at(feet).unwrap());
        assert_eq!(color(&alone), color(&beside));
    }

    /// Breaks if the bivariate scale's corners are not the four the plan names.
    #[test]
    fn sight_and_exposure_read_as_green_red_yellow_and_dark() {
        let rgb = |color: Color| color.to_srgba();
        let (seeing, seen, both, neither) = (rgb(bivariate(1.0, 0.0)), rgb(bivariate(0.0, 1.0)), rgb(bivariate(1.0, 1.0)), rgb(bivariate(0.0, 0.0)));
        assert!(seeing.green > 0.9 && seeing.red < 0.1);
        assert!(seen.red > 0.9 && seen.green < 0.1);
        assert!(both.red > 0.9 && both.green > 0.9);
        assert!(neither.red < 0.1 && neither.green < 0.1);
    }

    /// Breaks if the probe reads the sightlines backwards: behind a pillar is neither seen nor seeing.
    #[test]
    fn the_probe_lights_what_a_cell_sees_and_not_what_is_behind_a_pillar() {
        let volumes = [
            Volume::air(Room::new(Vec3::ZERO, Vec3::new(12.0, 4.0, 6.0))),
            Volume::solid(Room::new(Vec3::new(5.0, 0.0, 2.0), Vec3::new(7.0, 4.0, 4.0))),
        ];
        let graph = graph_of(&volumes);
        let probed = graph.cell_at(Vec3::new(1.25, 0.0, 3.25)).unwrap();
        let colors = probe_colors(&graph, probed, Body::Standing).unwrap();
        let color = |feet| colors[graph.cell_at(feet).unwrap() as usize];
        assert_eq!(color(Vec3::new(1.25, 0.0, 3.25)), PROBED);
        assert_eq!(color(Vec3::new(10.75, 0.0, 3.25)), DARK, "a cell behind the pillar is lit");
        assert_ne!(color(Vec3::new(3.25, 0.0, 0.75)), DARK, "a cell in plain view is dark");
    }
}
