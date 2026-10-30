//! The nav graph drawn over the floor for mappers, and the Bakes panel section that picks what is
//! drawn.
//! See "The overlays" in documentation/bots.md.

use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::SystemParam;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy_egui::egui;

use crate::editor::multicam::editor_only;
use crate::game::nav::{LinkKind, NavGraph, SPACING};
use crate::get;
use crate::tool::nav_bake::{BakeNav, ClearNav, NavBake};

/// Above the floor, in meters, so the overlay does not fight the floor for depth.
const CELL_LIFT: f32 = 0.03;
const LINK_LIFT: f32 = 0.05;
/// Of [`SPACING`], so neighboring cells read as separate squares.
const CELL_SIZE: f32 = 0.8;

const CELL: Color = Color::srgb(0.2, 0.6, 0.9);
const HURT_CELL: Color = Color::srgb(1.0, 0.45, 0.1);
const WALK: Color = Color::srgb(0.85, 0.9, 0.95);
const STEP: Color = Color::srgb(1.0, 0.85, 0.2);

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavOverlay {
    #[default]
    Off,
    CellsAndLinks,
}

impl NavOverlay {
    const ALL: [NavOverlay; 2] = [NavOverlay::Off, NavOverlay::CellsAndLinks];

    fn label(self) -> String {
        match self {
            NavOverlay::Off => get!("nav.overlay.off"),
            NavOverlay::CellsAndLinks => get!("nav.overlay.cells_and_links"),
        }
    }
}

#[derive(Component)]
struct NavOverlayMesh;

/// A quad per cell, colored by whether it hurts.
fn cell_mesh(graph: &NavGraph) -> Mesh {
    let half = SPACING * CELL_SIZE / 2.0;
    let cells = graph.cells();
    let mut positions = Vec::with_capacity(cells.len() * 4);
    let mut colors = Vec::with_capacity(cells.len() * 4);
    let mut indices = Vec::with_capacity(cells.len() * 6);
    for cell in cells {
        let center = cell.feet + Vec3::Y * CELL_LIFT;
        let color = if cell.hurt > 0 { HURT_CELL } else { CELL }.to_linear().to_f32_array();
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

/// A line per pair of linked cells, drawn once whichever way it was found. On a flat floor a link
/// two columns long only repeats two short ones, so those are left out unless they are steps.
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
            }
            .to_linear()
            .to_f32_array();
            for feet in [cell.feet, to.feet] {
                positions.push((feet + Vec3::Y * LINK_LIFT).to_array());
                colors.push(color);
            }
        }
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
}

/// Rebuilt only when the choice or the graph changes.
fn draw_nav_overlay(
    overlay: Res<NavOverlay>,
    nav: Res<NavBake>,
    mut drawn: Local<Option<(NavOverlay, u64)>>,
    existing: Query<Entity, With<NavOverlayMesh>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let wanted = (*overlay, nav.generation());
    if *drawn == Some(wanted) {
        return;
    }
    *drawn = Some(wanted);
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    let graph = nav.graph();
    if *overlay == NavOverlay::Off || graph.cells().is_empty() {
        return;
    }
    let material = materials.add(StandardMaterial { unlit: true, cull_mode: None, ..default() });
    for mesh in [cell_mesh(graph), link_mesh(graph)] {
        commands.spawn((
            Name::new("Nav overlay"),
            NavOverlayMesh,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            NotShadowCaster,
            editor_only(),
        ));
    }
}

/// What the nav section asked for this frame.
pub struct NavCommands {
    pub auto: bool,
    pub overlay: NavOverlay,
    pub bake: bool,
    pub clear: bool,
    cells: usize,
    links: usize,
}

/// The nav section's reads and writes, bundled to stay inside the panels' parameter limit.
#[derive(SystemParam)]
pub struct NavPanel<'w> {
    nav: ResMut<'w, NavBake>,
    overlay: ResMut<'w, NavOverlay>,
    bake: MessageWriter<'w, BakeNav>,
    clear: MessageWriter<'w, ClearNav>,
}

impl NavPanel<'_> {
    pub fn commands(&self) -> NavCommands {
        let graph = self.nav.graph();
        let links = (0..graph.cells().len() as u32).map(|id| graph.links(id).len()).sum();
        NavCommands { auto: self.nav.auto(), overlay: *self.overlay, bake: false, clear: false, cells: graph.cells().len(), links }
    }

    pub fn apply(&mut self, commands: &NavCommands) {
        if commands.auto != self.nav.auto() {
            self.nav.set_auto(commands.auto);
        }
        if commands.overlay != *self.overlay {
            *self.overlay = commands.overlay;
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
        for overlay in NavOverlay::ALL {
            ui.radio_value(&mut commands.overlay, overlay, overlay.label());
        }
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
    });
}

pub struct NavOverlayPlugin;

impl Plugin for NavOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavOverlay>()
            // Ungated: the overlay is on a layer only the editor viewports draw.
            .add_systems(Update, draw_nav_overlay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::world_surface::Volume;
    use crate::game::collision::CollisionWorld;
    use crate::game::nav::{self, CellId};
    use crate::tool::room::Room;

    fn graph_of(rooms: &[Room]) -> NavGraph {
        let mut world = CollisionWorld::default();
        let volumes: Vec<Volume> = rooms.iter().cloned().map(Volume::air).collect();
        let faces = world.rebuild_volumes(&volumes);
        let cells = nav::floor_cells(&world, &faces, &[]);
        let columns = nav::columns(&cells);
        let links = (0..cells.len()).map(|id| nav::walk_links(&world, &cells, &columns, id as CellId, 7.0)).collect();
        let start = cells[0].feet;
        NavGraph::reachable(cells, links, &[start])
    }

    fn vertex_count(mesh: &Mesh) -> usize {
        mesh.attribute(Mesh::ATTRIBUTE_POSITION).map_or(0, |positions| positions.len())
    }

    /// Breaks if a cell is drawn twice or not at all.
    #[test]
    fn every_cell_is_one_quad() {
        let graph = graph_of(&[Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))]);
        let mesh = cell_mesh(&graph);
        assert_eq!(vertex_count(&mesh), graph.cells().len() * 4);
        assert_eq!(mesh.indices().map_or(0, |indices| indices.len()), graph.cells().len() * 6);
    }

    /// Breaks if a two-way link is drawn from both ends, or a flat floor shows its long links.
    #[test]
    fn a_flat_floor_draws_each_neighbor_once() {
        let graph = graph_of(&[Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))]);
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
        let graph = graph_of(&[
            Room::new(Vec3::ZERO, Vec3::new(3.0, 3.0, 3.0)),
            Room::new(Vec3::new(3.0, 0.4, 0.0), Vec3::new(6.0, 3.4, 3.0)),
        ]);
        let mesh = link_mesh(&graph);
        let Some(bevy::mesh::VertexAttributeValues::Float32x4(colors)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR) else {
            panic!("no colors");
        };
        let step = STEP.to_linear().to_f32_array();
        assert!(colors.contains(&step), "no step link drawn across a 0.4 m lip");
    }
}
