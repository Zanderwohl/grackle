//! A round HUD panel: a disk painted as a [`Plate`](crate::game::plate::Plate) is, with its
//! bottom cut off flat, for a picture to stand on.
//!
//! Spawn a [`Medallion`] on a [`medallion_node`] and give it children; set [`Medallion::swatch`]
//! to recolor it. After layout, [`paint_the_medallions`] puts the panel in as the first child and
//! repaints it when the size, swatch or cut changes. Children are not clipped to the disk, so a
//! picture with a transparent background can stand out past its rim.

use bevy::prelude::*;

use crate::common::swatch::Swatch;
use crate::common::ui_shape::{Path, ShapeMaterial};
use crate::game::hud::{panel_style, ZONE_DROP, ZONE_STROKE};

/// Points on the arc; the shape holds at most [`crate::common::ui_shape::MAX_POINTS`].
const ARC_POINTS: usize = 72;

pub struct MedallionPlugin;

impl Plugin for MedallionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, paint_the_medallions.after(bevy::ui::UiSystems::Layout));
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq)]
#[require(Node, Pickable = Pickable::IGNORE)]
pub struct Medallion {
    pub swatch: Swatch,
    /// The share of the disk's height kept, from the top: 1 is a whole disk, 0.5 a half.
    pub kept: f32,
    drawn: Option<(Vec2, Swatch, f32)>,
}

impl Medallion {
    pub fn new(swatch: Swatch, kept: f32) -> Self {
        Self { swatch, kept, drawn: None }
    }
}

#[derive(Component)]
struct MedallionPanel;

/// A disk `size` logical pixels across, stroke included, with room for the drop shadow.
pub fn medallion_box(size: f32) -> Vec2 {
    Vec2::splat(size) + ZONE_DROP
}

/// A medallion `size` across. Place it with the node's other fields:
/// `Node { left, bottom, ..medallion_node(size) }`.
pub fn medallion_node(size: f32) -> Node {
    let size = medallion_box(size);
    Node { width: Val::Px(size.x), height: Val::Px(size.y), ..default() }
}

/// Center and radius, inset by the stroke and the drop.
fn disk(size: Vec2) -> (Vec2, f32) {
    let lo = Vec2::splat(ZONE_STROKE);
    let hi = size - Vec2::splat(ZONE_STROKE) - ZONE_DROP;
    ((lo + hi) * 0.5, ((hi - lo) * 0.5).min_element().max(0.0))
}

/// How far below the center of a disk of `radius` the chord runs when `kept` of its height is
/// kept: `radius` keeps the whole disk.
fn chord_below_center(radius: f32, kept: f32) -> f32 {
    radius * (2.0 * kept.clamp(0.0, 1.0) - 1.0)
}

/// Logical pixels from the top of a box of `size` down to the chord, for a picture that ends there.
pub fn cut_height(size: Vec2, kept: f32) -> f32 {
    let (center, radius) = disk(size);
    center.y + chord_below_center(radius, kept)
}

/// Cut flat at `kept` of its height. Y grows downward.
pub fn cut_disk(size: Vec2, kept: f32) -> Path {
    let (center, radius) = disk(size);
    let cut = chord_below_center(radius, kept);
    if kept.clamp(0.0, 1.0) >= 1.0 {
        let mut points = (0..ARC_POINTS).map(|i| center + radius * Vec2::from_angle(i as f32 / ARC_POINTS as f32 * std::f32::consts::TAU));
        let first = points.next().unwrap_or(center);
        return points.fold(Path::starting_at(first), Path::line_to);
    }
    // Angles from straight down; the arc runs from one end of the chord over the top (at π) to
    // the other, and closing the path draws the chord.
    let half = (cut / radius).clamp(-1.0, 1.0).acos();
    let (start, end) = (half, std::f32::consts::TAU - half);
    let mut arc = (0..=ARC_POINTS).map(|i| {
        let angle = start + (end - start) * i as f32 / ARC_POINTS as f32;
        center + radius * Vec2::new(angle.sin(), angle.cos())
    });
    let first = arc.next().unwrap_or(center);
    arc.fold(Path::starting_at(first), Path::line_to)
}

fn panel_node() -> impl Bundle {
    (
        MedallionPanel,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        Pickable::IGNORE,
        Visibility::Inherited,
    )
}

fn paint_the_medallions(
    mut commands: Commands,
    mut medallions: Query<(Entity, &mut Medallion, &ComputedNode, Option<&Children>)>,
    panels: Query<(), With<MedallionPanel>>,
    mut materials: Option<ResMut<Assets<ShapeMaterial>>>,
) {
    for (entity, mut medallion, computed, children) in &mut medallions {
        let size = computed.size() * computed.inverse_scale_factor();
        if size == Vec2::ZERO {
            continue;
        }
        let panel = children.into_iter().flatten().copied().find(|child| panels.contains(*child));
        let panel = panel.unwrap_or_else(|| {
            let panel = commands.spawn(panel_node()).id();
            commands.entity(entity).insert_children(0, &[panel]);
            panel
        });

        let wanted = (medallion.swatch, medallion.kept);
        // Within half a pixel the panel already drawn still fits; layout's sizes jitter by fractions.
        let fits = medallion.drawn.is_some_and(|(drawn, swatch, kept)| {
            (swatch, kept) == wanted && (drawn - size).abs().max_element() <= 0.5
        });
        if !fits && let Some(materials) = materials.as_deref_mut() {
            let outline = cut_disk(size, medallion.kept);
            let material = materials.add(ShapeMaterial::from_path(&outline, size, panel_style(&medallion.swatch)));
            commands.entity(panel).insert(MaterialNode(material));
            medallion.drawn = Some((size, wanted.0, wanted.1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ui_shape::path::area;
    use bevy::ecs::system::RunSystemOnce;

    /// Breaks if the disk is not cut where asked, spills out of its box, or keeps its bottom.
    #[test]
    fn a_cut_disk_keeps_its_top_and_loses_its_bottom() {
        let size = Vec2::splat(100.0) + ZONE_DROP;
        let whole = cut_disk(size, 1.0).flatten();
        let kept = cut_disk(size, 0.75).flatten();
        let radius = (100.0 - 2.0 * ZONE_STROKE) * 0.5;
        let center = Vec2::splat(50.0);
        let disk = std::f32::consts::PI * radius * radius;
        assert!((area(&whole) - disk).abs() / disk < 0.01, "the whole disk is {} of {disk}", area(&whole));

        let top = kept.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let bottom = kept.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        assert!((top - (center.y - radius)).abs() < 0.5, "the top is at {top}");
        assert!((bottom - (center.y + radius * 0.5)).abs() < 0.01, "the cut is at {bottom}");
        assert!((bottom - cut_height(size, 0.75)).abs() < 1e-3, "cut_height says {}", cut_height(size, 0.75));
        assert!(area(&kept) < area(&whole) * 0.85 && area(&kept) > area(&whole) * 0.75);
        for point in whole.iter().chain(&kept) {
            assert!(point.cmpge(Vec2::splat(ZONE_STROKE - 0.01)).all() && point.cmple(size - ZONE_DROP).all(), "{point} is outside");
        }
    }

    /// Breaks if a medallion gets no panel, a second one, or the panel is not under its children.
    #[test]
    fn a_medallion_gets_one_panel_under_its_children() {
        let mut world = World::new();
        let medallion = world
            .spawn((Medallion::new(Swatch::neutral(), 0.8), ComputedNode { size: Vec2::splat(120.0), inverse_scale_factor: 1.0, ..default() }))
            .with_child(Node::default())
            .id();
        world.run_system_once(paint_the_medallions).unwrap();
        world.run_system_once(paint_the_medallions).unwrap();
        let children = world.get::<Children>(medallion).unwrap().to_vec();
        assert_eq!(children.len(), 2);
        assert!(world.get::<MedallionPanel>(children[0]).is_some(), "the panel is not first");
    }
}
