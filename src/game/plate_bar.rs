//! A [`Plate`](crate::game::plate::Plate)'s leaning panel as a bar: the panel in its swatch over
//! the left [`PlateBar::t`] of its width, and the same panel at a fifth of the swatch's
//! chroma over the rest, on the right.
//!
//! The colored panel is drawn whole over the muted one, inside a node as wide as its share that
//! clips it, so the divide is a vertical line and the shape never changes with `t`. The panels
//! are made after layout, once the bar's size is known, and remade when its size, swatch or
//! corners change; `t` only moves the clip.

use bevy::prelude::*;

use crate::common::swatch::Swatch;
use crate::common::ui_shape::{Corner, ShapeMaterial};
use crate::game::hud::{leaning_corners, leaning_panel_each, panel_style};

/// Of the swatch's OKLCH chroma, on the empty part.
const EMPTY_CHROMA: f32 = 0.2;

pub struct PlateBarPlugin;

impl Plugin for PlateBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, (move_the_divides.before(bevy::ui::UiSystems::Layout), paint_the_bars.after(bevy::ui::UiSystems::Layout)));
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq)]
#[require(Node, Pickable = Pickable::IGNORE)]
pub struct PlateBar {
    pub swatch: Swatch,
    /// The share of the width, from the left, drawn in the swatch, 0 to 1.
    pub t: f32,
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
        Self { swatch, t, leans_right, corners, drawn: None }
    }

    /// `t` as a share of the width, a NaN counting as empty.
    fn share(&self) -> f32 {
        if self.t.is_nan() { 0.0 } else { self.t.clamp(0.0, 1.0) }
    }
}

/// The muted panel, under the clip.
#[derive(Component)]
struct EmptyPanel;

/// As wide as the colored share, on the left, clipping the colored panel inside it.
#[derive(Component)]
struct FullClip;

/// The colored panel, as wide as the whole bar and kept against the clip's left edge.
#[derive(Component)]
struct FullPanel;

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

fn clip(share: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        top: Val::Px(0.0),
        width: Val::Percent(share * 100.0),
        height: Val::Percent(100.0),
        overflow: Overflow::clip(),
        ..default()
    }
}

/// Before layout, so a new `t` is drawn in the frame it is set.
fn move_the_divides(bars: Query<(&PlateBar, &Children), Changed<PlateBar>>, mut clips: Query<&mut Node, With<FullClip>>) {
    for (bar, children) in &bars {
        let width = Val::Percent(bar.share() * 100.0);
        for &child in children {
            if let Ok(mut node) = clips.get_mut(child)
                && node.width != width
            {
                node.width = width;
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn paint_the_bars(
    mut commands: Commands,
    mut bars: Query<(Entity, &mut PlateBar, &ComputedNode, Option<&Children>)>,
    empties: Query<(), With<EmptyPanel>>,
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
        let empty = children.iter().copied().find(|child| empties.contains(*child));
        let clipping = children.iter().copied().find(|child| clips.contains(*child));
        let full = clipping
            .and_then(|clipping| clips.get(clipping).ok().flatten())
            .and_then(|inside| inside.iter().find(|child| fulls.contains(*child)));
        let (empty, full) = match (empty, clipping, full) {
            (Some(empty), Some(_), Some(full)) => (empty, full),
            _ => {
                for stale in [empty, clipping].into_iter().flatten() {
                    commands.entity(stale).despawn();
                }
                let full = commands.spawn((FullPanel, Node { width: Val::Px(size.x), ..whole() }, Pickable::IGNORE)).id();
                let clipping = commands.spawn((FullClip, clip(bar.share()), Pickable::IGNORE)).add_child(full).id();
                let empty = commands.spawn((EmptyPanel, whole(), Pickable::IGNORE)).id();
                commands.entity(entity).insert_children(0, &[empty, clipping]);
                bar.drawn = None;
                (empty, full)
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
        bar.drawn = Some((size, swatch, corners));
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
        assert!(world.get::<EmptyPanel>(children[0]).is_some(), "the muted panel is not first");
        let clipping = world.get::<Node>(children[1]).unwrap();
        assert_eq!((clipping.left, clipping.right, clipping.width), (Val::Px(0.0), Val::Auto, Val::Percent(75.0)));
        assert_eq!(clipping.overflow, Overflow::clip());
        let full = world.get::<Children>(children[1]).unwrap()[0];
        let node = world.get::<Node>(full).unwrap();
        assert_eq!((node.left, node.width), (Val::Px(0.0), Val::Px(200.0)));
        assert!(world.get::<MaterialNode<ShapeMaterial>>(full).is_some() && world.get::<MaterialNode<ShapeMaterial>>(children[0]).is_some());

        world.run_system_once(paint_the_bars).unwrap();
        assert_eq!(world.get::<Children>(bar).unwrap().len(), 2, "the panels were made again");
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

    /// Breaks if the empty part is not a fifth of the swatch's chroma at its lightness and hue.
    #[test]
    fn the_empty_part_is_the_swatch_at_a_fifth_of_its_chroma() {
        let swatch = Swatch::team(Team::Blue);
        let (full, empty) = (Oklcha::from(swatch.base), Oklcha::from(swatch.muted(1.0, EMPTY_CHROMA).base));
        assert!((empty.chroma - full.chroma * EMPTY_CHROMA).abs() < 0.01, "{full:?} {empty:?}");
        assert!((empty.lightness - full.lightness).abs() < 0.01);
    }
}
