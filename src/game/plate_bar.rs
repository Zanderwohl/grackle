//! A [`Plate`](crate::game::plate::Plate)'s leaning panel as a bar: the panel in its swatch over
//! [`PlateBar::t`] of its width, from the side its [`BarDirection`] fills from, and the same panel
//! at a fifth of the swatch's chroma over the rest.
//!
//! The colored panel is drawn whole over the muted one, inside a node as wide as its share that
//! clips it, so the divide is a vertical line and the shape never changes with `t`. The panels
//! are made after layout, once the bar's size is known, and remade when its size, swatch or
//! corners change; `t` only moves the clip.
//!
//! Below [`PlateBar::attention_below`] a red glow pulses under the panels: the panel's outline
//! stroked out past its edge, wider, more opaque and quicker the further `t` falls.

use std::f32::consts::TAU;

use bevy::prelude::*;

use crate::common::swatch::Swatch;
use crate::common::ui_shape::{Corner, ShapeMaterial, ShapeStyle};
use crate::game::hud::{leaning_corners, leaning_panel_each, panel_style};

/// Of the swatch's OKLCH chroma, on the empty part.
const EMPTY_CHROMA: f32 = 0.2;

/// The attention glow at the threshold and at empty: logical pixels past the panel's edge, peak
/// opacity, and pulses per second.
const ATTENTION_FROM: Attention = Attention { reach: 5.0, opacity: 0.2, hertz: 1.0 };
const ATTENTION_TO: Attention = Attention { reach: 30.0, opacity: 0.4, hertz: 3.0 };
/// Logical pixels the glow's node reaches past the bar on every side: the widest glow and room
/// for its soft edge.
const GLOW_MARGIN: f32 = ATTENTION_TO.reach + 4.0;
/// `#FF3333`.
const ATTENTION_RED: Color = Color::srgb(1.0, 0.2, 0.2);

pub struct PlateBarPlugin;

impl Plugin for PlateBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            (move_the_divides.before(bevy::ui::UiSystems::Layout), (paint_the_bars, pulse_for_attention).chain().after(bevy::ui::UiSystems::Layout)),
        );
    }
}

/// Which end of the bar the colored part grows from.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BarDirection {
    /// Colored on the left, emptying from the right.
    #[default]
    LeftToRight,
    /// Colored on the right, emptying from the left.
    RightToLeft,
}

#[derive(Component, Debug, Clone, Copy, PartialEq)]
#[require(Node, Pickable = Pickable::IGNORE)]
pub struct PlateBar {
    pub swatch: Swatch,
    /// The share of the width drawn in the swatch, 0 to 1.
    pub t: f32,
    pub direction: BarDirection,
    /// Below this `t` the attention glow is up. Zero never shows it.
    pub attention_below: f32,
    pub leans_right: bool,
    /// Clockwise from the top left.
    pub corners: [Corner; 4],
    /// The size, colors and corners the panels were built for.
    drawn: Option<(Vec2, Swatch, [Corner; 4])>,
}

impl PlateBar {
    /// With a plate's corners.
    pub fn new(swatch: Swatch, t: f32) -> Self {
        Self::with_corners(swatch, t, true, leaning_corners(true, Corner::squircle(16.0), Corner::squircle(6.0)))
    }

    pub fn with_corners(swatch: Swatch, t: f32, leans_right: bool, corners: [Corner; 4]) -> Self {
        Self { swatch, t, direction: BarDirection::default(), attention_below: 0.0, leans_right, corners, drawn: None }
    }

    pub fn toward(self, direction: BarDirection) -> Self {
        Self { direction, ..self }
    }

    pub fn attention_below(self, threshold: f32) -> Self {
        Self { attention_below: threshold, ..self }
    }

    /// `t` as a share of the width, a NaN counting as empty.
    fn share(&self) -> f32 {
        if self.t.is_nan() { 0.0 } else { self.t.clamp(0.0, 1.0) }
    }

    /// How far below the threshold `t` is, 0 at it to 1 at empty; `None` at or above it.
    fn sub_t(&self) -> Option<f32> {
        let share = self.share();
        (self.attention_below > 0.0 && share < self.attention_below).then(|| 1.0 - share / self.attention_below)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Attention {
    reach: f32,
    opacity: f32,
    hertz: f32,
}

impl Attention {
    fn at(sub_t: f32) -> Self {
        let lerp = |from: f32, to: f32| from + (to - from) * sub_t;
        Self {
            reach: lerp(ATTENTION_FROM.reach, ATTENTION_TO.reach),
            opacity: lerp(ATTENTION_FROM.opacity, ATTENTION_TO.opacity),
            hertz: lerp(ATTENTION_FROM.hertz, ATTENTION_TO.hertz),
        }
    }
}

/// The muted panel, under the clip.
#[derive(Component)]
struct EmptyPanel;

/// As wide as the colored share, at the end the bar fills from, clipping the colored panel inside it.
#[derive(Component)]
struct FullClip;

/// The colored panel, as wide as the whole bar and kept against the clip's outer edge.
#[derive(Component)]
struct FullPanel;

/// Under both panels, [`GLOW_MARGIN`] bigger on every side. The phase is kept, not read off the
/// clock, so a change of rate does not jump the pulse.
#[derive(Component, Default)]
struct AttentionGlow {
    phase: f32,
}

fn whole() -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        top: Val::Px(0.0),
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..default()
    }
}

/// Left and right of the clip, and of the colored panel inside it, for each way of filling.
fn anchored(direction: BarDirection) -> (Val, Val) {
    match direction {
        BarDirection::LeftToRight => (Val::Px(0.0), Val::Auto),
        BarDirection::RightToLeft => (Val::Auto, Val::Px(0.0)),
    }
}

fn clip(share: f32, direction: BarDirection) -> Node {
    let (left, right) = anchored(direction);
    Node { left, right, width: Val::Percent(share * 100.0), overflow: Overflow::clip(), ..whole() }
}

fn glow_node() -> Node {
    let reach = Val::Px(-GLOW_MARGIN);
    Node { position_type: PositionType::Absolute, left: reach, right: reach, top: reach, bottom: reach, ..default() }
}

/// Before layout, so a new `t` or direction is drawn in the frame it is set.
fn move_the_divides(
    bars: Query<(&PlateBar, &Children), Changed<PlateBar>>,
    mut clips: Query<(&mut Node, Option<&Children>), (With<FullClip>, Without<FullPanel>)>,
    mut fulls: Query<&mut Node, With<FullPanel>>,
) {
    for (bar, children) in &bars {
        let width = Val::Percent(bar.share() * 100.0);
        let (left, right) = anchored(bar.direction);
        for &child in children {
            let Ok((mut node, inside)) = clips.get_mut(child) else { continue };
            if node.width != width || node.left != left || node.right != right {
                node.width = width;
                node.left = left;
                node.right = right;
            }
            for &panel in inside.into_iter().flatten() {
                if let Ok(mut node) = fulls.get_mut(panel)
                    && (node.left != left || node.right != right)
                {
                    node.left = left;
                    node.right = right;
                }
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn paint_the_bars(
    mut commands: Commands,
    mut bars: Query<(Entity, &mut PlateBar, &ComputedNode, Option<&Children>)>,
    empties: Query<(), With<EmptyPanel>>,
    glows: Query<(), With<AttentionGlow>>,
    clips: Query<Option<&Children>, With<FullClip>>,
    mut fulls: Query<&mut Node, With<FullPanel>>,
    materials: Option<ResMut<Assets<ShapeMaterial>>>,
) {
    let Some(mut materials) = materials else { return };
    for (entity, mut bar, computed, children) in &mut bars {
        let size = computed.size() * computed.inverse_scale_factor();
        if size == Vec2::ZERO {
            continue;
        }
        let children: Vec<Entity> = children.into_iter().flatten().copied().collect();
        let glow = children.iter().copied().find(|child| glows.contains(*child));
        let empty = children.iter().copied().find(|child| empties.contains(*child));
        let clipping = children.iter().copied().find(|child| clips.contains(*child));
        let full = clipping
            .and_then(|clipping| clips.get(clipping).ok().flatten())
            .and_then(|inside| inside.iter().find(|child| fulls.contains(*child)));
        let (glow, empty, full) = match (glow, empty, clipping, full) {
            (Some(glow), Some(empty), Some(_), Some(full)) => (glow, empty, full),
            _ => {
                for stale in [glow, empty, clipping].into_iter().flatten() {
                    commands.entity(stale).despawn();
                }
                let (left, right) = anchored(bar.direction);
                let full = commands.spawn((FullPanel, Node { left, right, width: Val::Px(size.x), ..whole() }, Pickable::IGNORE)).id();
                let clipping = commands.spawn((FullClip, clip(bar.share(), bar.direction), Pickable::IGNORE)).add_child(full).id();
                let empty = commands.spawn((EmptyPanel, whole(), Pickable::IGNORE)).id();
                let glow = commands.spawn((AttentionGlow::default(), glow_node(), Pickable::IGNORE, Visibility::Hidden)).id();
                commands.entity(entity).insert_children(0, &[glow, empty, clipping]);
                bar.drawn = None;
                (glow, empty, full)
            }
        };

        // The colored panel is the bar's width in pixels, not the clip's share of it.
        let wide = Val::Px(size.x);
        if let Ok(mut node) = fulls.get_mut(full)
            && node.width != wide
        {
            node.width = wide;
        }

        // Within half a pixel the panels already drawn still fit; layout's sizes jitter by fractions.
        let swatch = bar.swatch;
        let corners = bar.corners;
        let fits = bar.drawn.is_some_and(|(drawn, painted, cornered)| painted == swatch && cornered == corners && (drawn - size).abs().max_element() <= 0.5);
        if fits {
            continue;
        }
        let outline = leaning_panel_each(size, bar.leans_right, corners);
        let muted = swatch.muted(1.0, EMPTY_CHROMA);
        commands.entity(empty).insert(MaterialNode(materials.add(ShapeMaterial::from_path(&outline, size, panel_style(&muted)))));
        commands.entity(full).insert(MaterialNode(materials.add(ShapeMaterial::from_path(&outline, size, panel_style(&swatch)))));
        // The same outline in the glow's bigger box; `pulse_for_attention` sets its stroke.
        let margin = Vec2::splat(GLOW_MARGIN);
        let shifted = outline.flatten().into_iter().map(|point| point + margin).collect();
        commands.entity(glow).insert(MaterialNode(materials.add(ShapeMaterial::new(shifted, size + 2.0 * margin, ShapeStyle { fill: Color::NONE, ..default() }))));
        bar.drawn = Some((size, swatch, corners));
    }
}

/// A stroke centered on the outline reaches past it by half its width; the half inside is under
/// the panels. Its opacity follows a sine from nothing to the peak, on real time, so a slowed
/// round does not slow it.
fn pulse_for_attention(
    time: Res<Time<Real>>,
    bars: Query<(&PlateBar, &Children)>,
    mut glows: Query<(&mut AttentionGlow, &mut Visibility, Option<&MaterialNode<ShapeMaterial>>)>,
    materials: Option<ResMut<Assets<ShapeMaterial>>>,
) {
    let Some(mut materials) = materials else { return };
    for (bar, children) in &bars {
        for &child in children {
            let Ok((mut glow, mut visibility, material)) = glows.get_mut(child) else { continue };
            let Some(sub_t) = bar.sub_t() else {
                glow.phase = 0.0;
                visibility.set_if_neq(Visibility::Hidden);
                continue;
            };
            let attention = Attention::at(sub_t);
            glow.phase = (glow.phase + TAU * attention.hertz * time.delta_secs()) % TAU;
            visibility.set_if_neq(Visibility::Inherited);
            let Some(material) = material else { continue };
            // From nothing, so a glow just come up does not flash on at its peak.
            let opacity = attention.opacity * 0.5 * (1.0 - glow.phase.cos());
            let (stroke, width) = (ATTENTION_RED.with_alpha(opacity), 2.0 * attention.reach);
            // Compared first, since `get_mut` marks the material changed.
            if materials.get(&material.0).is_none_or(|shape| shape.style.stroke == stroke && shape.style.stroke_width == width) {
                continue;
            }
            if let Some(mut shape) = materials.get_mut(&material.0) {
                shape.style.stroke = stroke;
                shape.style.stroke_width = width;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::team::Team;
    use bevy::color::Oklcha;
    use bevy::ecs::system::RunSystemOnce;

    fn a_bar(world: &mut World, t: f32) -> Entity {
        world.init_resource::<Assets<ShapeMaterial>>();
        let sized = ComputedNode { size: Vec2::new(200.0, 48.0), inverse_scale_factor: 1.0, ..default() };
        world.spawn((PlateBar::new(Swatch::team(Team::Red), t), sized)).id()
    }

    fn clip_width(world: &mut World) -> Val {
        world.query_filtered::<&Node, With<FullClip>>().single(world).unwrap().width
    }

    /// Breaks if the muted panel is not under the colored one, the colored one is not as wide as
    /// the bar, or the clip is not the colored share on the left.
    #[test]
    fn a_bar_shows_its_share_in_color_on_the_left() {
        let mut world = World::new();
        let bar = a_bar(&mut world, 0.75);
        world.run_system_once(paint_the_bars).unwrap();

        let children = world.get::<Children>(bar).unwrap().to_vec();
        assert!(world.get::<AttentionGlow>(children[0]).is_some(), "the glow is not under the panels");
        assert!(world.get::<EmptyPanel>(children[1]).is_some(), "the muted panel is not under the colored one");
        let clipping = world.get::<Node>(children[2]).unwrap();
        assert_eq!((clipping.left, clipping.right, clipping.width), (Val::Px(0.0), Val::Auto, Val::Percent(75.0)));
        assert_eq!(clipping.overflow, Overflow::clip());
        let full = world.get::<Children>(children[2]).unwrap()[0];
        let node = world.get::<Node>(full).unwrap();
        assert_eq!((node.left, node.width), (Val::Px(0.0), Val::Px(200.0)));
        assert!(world.get::<MaterialNode<ShapeMaterial>>(full).is_some() && world.get::<MaterialNode<ShapeMaterial>>(children[1]).is_some());

        world.run_system_once(paint_the_bars).unwrap();
        assert_eq!(world.get::<Children>(bar).unwrap().len(), 3, "the panels were made again");
    }

    /// Breaks if a new `t` does not move the divide, or one outside 0 to 1 is not held to it.
    #[test]
    fn the_divide_follows_t_and_stays_inside_the_bar() {
        let mut world = World::new();
        let bar = a_bar(&mut world, 1.0);
        world.run_system_once(paint_the_bars).unwrap();
        for (t, width) in [(0.25, 25.0), (1.7, 100.0), (-0.2, 0.0), (f32::NAN, 0.0)] {
            world.get_mut::<PlateBar>(bar).unwrap().t = t;
            world.run_system_once(move_the_divides).unwrap();
            assert_eq!(clip_width(&mut world), Val::Percent(width), "t = {t}");
        }
    }

    /// Breaks if a bar filling right to left does not keep its clip and colored panel on the right.
    #[test]
    fn a_bar_filling_right_to_left_is_colored_on_the_right() {
        let mut world = World::new();
        let bar = a_bar(&mut world, 0.5);
        world.run_system_once(paint_the_bars).unwrap();
        let turned = world.get::<PlateBar>(bar).unwrap().toward(BarDirection::RightToLeft);
        world.entity_mut(bar).insert(turned);
        world.run_system_once(move_the_divides).unwrap();
        let clipping = world.query_filtered::<&Node, With<FullClip>>().single(&world).unwrap().clone();
        let full = world.query_filtered::<&Node, With<FullPanel>>().single(&world).unwrap().clone();
        assert_eq!((clipping.left, clipping.right, clipping.width), (Val::Auto, Val::Px(0.0), Val::Percent(50.0)));
        assert_eq!((full.left, full.right), (Val::Auto, Val::Px(0.0)));
    }

    /// Breaks if the glow is not off at or above the threshold, or does not run from its least at
    /// the threshold to its most at empty.
    #[test]
    fn the_attention_glow_grows_as_t_falls_below_the_threshold() {
        let bar = |t| PlateBar::new(Swatch::neutral(), t).attention_below(0.2);
        assert_eq!(bar(0.2).sub_t(), None);
        assert_eq!(bar(0.5).sub_t(), None);
        assert_eq!(PlateBar::new(Swatch::neutral(), 0.0).sub_t(), None, "a bar with no threshold glowed");
        assert_eq!(Attention::at(bar(0.0).sub_t().unwrap()), ATTENTION_TO);
        let near = Attention::at(bar(0.199).sub_t().unwrap());
        assert!((near.reach - ATTENTION_FROM.reach).abs() < 0.2 && (near.hertz - ATTENTION_FROM.hertz).abs() < 0.02, "{near:?}");
        let half = Attention::at(bar(0.1).sub_t().unwrap());
        assert!((half.reach - 17.5).abs() < 1e-4 && (half.opacity - 0.3).abs() < 1e-4 && (half.hertz - 2.0).abs() < 1e-4, "{half:?}");
    }

    /// Breaks if the glow is not shown below the threshold with a stroke reaching its reach past
    /// the outline, or is not hidden again above it.
    #[test]
    fn the_glow_shows_and_strokes_below_the_threshold_only() {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        let bar = a_bar(&mut world, 0.0);
        world.entity_mut(bar).insert(PlateBar::new(Swatch::neutral(), 0.0).attention_below(0.2));
        world.run_system_once(paint_the_bars).unwrap();
        world.run_system_once(pulse_for_attention).unwrap();
        let glow = world.query_filtered::<Entity, With<AttentionGlow>>().single(&world).unwrap();
        assert_eq!(world.get::<Visibility>(glow), Some(&Visibility::Inherited));
        let handle = world.get::<MaterialNode<ShapeMaterial>>(glow).unwrap().0.clone();
        let stroke = world.resource::<Assets<ShapeMaterial>>().get(&handle).unwrap().style.stroke_width;
        assert_eq!(stroke, 2.0 * ATTENTION_TO.reach);

        world.get_mut::<PlateBar>(bar).unwrap().t = 0.6;
        world.run_system_once(pulse_for_attention).unwrap();
        assert_eq!(world.get::<Visibility>(glow), Some(&Visibility::Hidden));
    }

    /// Breaks if the empty part is not a fifth of the swatch's chroma at its lightness and hue.
    #[test]
    fn the_empty_part_is_the_swatch_at_a_fifth_of_its_chroma() {
        let swatch = Swatch::team(Team::Blue);
        let (full, empty) = (Oklcha::from(swatch.base), Oklcha::from(swatch.muted(1.0, EMPTY_CHROMA).base));
        assert!((empty.chroma - full.chroma * EMPTY_CHROMA).abs() < 0.01, "{full:?} {empty:?}");
        assert!((empty.lightness - full.lightness).abs() < 0.01);
    }
}
