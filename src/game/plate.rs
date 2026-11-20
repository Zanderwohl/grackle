//! A HUD panel sized to its text: the leaning quadrilateral of [`leaning_panel`](crate::game::hud::leaning_panel) behind a column
//! of lines, painted in a [`Swatch`], each [`PlateText`] line in whichever of black and white
//! stands out on it ([`Swatch::text`]).
//!
//! Spawn [`plate`] with [`plate_text`] children and any [`PlateIcon`] images, and set
//! [`Plate::swatch`] to recolor it. Plates (buttons among them) that are children of a
//! [`plate_row`] are joined side to side into one group. After layout, [`paint_the_plates`] gives the plate its panel
//! (the first child) and repaints it when the plate's size or swatch changes. Its lines and panel stay hidden until the panel has been
//! through layout, so a new plate never shows text with no panel behind it. The plate's own
//! `Visibility` is never touched: it belongs to whoever spawned it.

use bevy::prelude::*;

use crate::common::swatch::Swatch;
use crate::common::ui_shape::{Corner, ShapeMaterial};
use crate::game::hud::{hud_text, lean_across, leaning_corners, leaning_panel_each, panel_style, LEAN, ZONE_DROP, ZONE_STROKE};

/// Logical pixels inside the panel's box. The far sides are wider by the drop shadow, which
/// centers the text on the panel rather than on panel and shadow.
pub const PLATE_PADDING: UiRect = padding_with_room(1.0);

/// [`PLATE_PADDING`] with `room` logical pixels between the text and the stroke above and below.
pub const fn padding_with_room(room: f32) -> UiRect {
    UiRect {
        left: Val::Px(ZONE_STROKE + 15.0),
        right: Val::Px(ZONE_STROKE + ZONE_DROP.x + 15.0),
        top: Val::Px(ZONE_STROKE + room),
        bottom: Val::Px(ZONE_STROKE + ZONE_DROP.y + room),
    }
}

pub struct PlatePlugin;

impl Plugin for PlatePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, (join_the_rows, (paint_the_plates, follow_the_lean)).chain().after(bevy::ui::UiSystems::Layout));
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq)]
#[require(Node, Pickable = Pickable::IGNORE)]
pub struct Plate {
    pub swatch: Swatch,
    pub leans_right: bool,
    /// Written by [`join_the_rows`] for a plate in a [`plate_row`].
    joins: Joins,
    /// The size, colors and joins the panel was built for.
    drawn: Option<(Vec2, Swatch, Joins)>,
    shown: bool,
}

impl Plate {
    pub fn new(swatch: Swatch) -> Self {
        Self { swatch, leans_right: true, joins: Joins::default(), drawn: None, shown: false }
    }
}

/// Which slanted sides a plate shares with a neighbor, whose corners are left square.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Joins {
    left: bool,
    right: bool,
}

/// Its neighboring plate children share their slanted sides, with square corners there. They must
/// lean the same way; the row stretches them to one height, which keeps the sides parallel.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[require(Node)]
pub struct PlateRow;

/// Give it plate or plate button children. To place it, spawn `(PlateRow, Node { right, bottom, ..plate_row_node() })`.
pub fn plate_row() -> impl Bundle {
    (PlateRow, plate_row_node())
}

pub fn plate_row_node() -> Node {
    Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Stretch, ..default() }
}

/// How far a plate in a row is pulled left over the plate before it, in logical pixels, for plates
/// `height` tall: past its own stroke, the other's stroke and drop, and the slant across it.
fn overlap(height: f32) -> f32 {
    2.0 * ZONE_STROKE + ZONE_DROP.x + lean_across(height)
}

/// A line whose color follows its plate's swatch.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(Visibility = Visibility::Hidden)]
pub struct PlateText;

/// An image inked as a [`PlateText`] is. Drawn white, so its `ImageNode::color` is its ink.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(Visibility = Visibility::Hidden)]
pub struct PlateIcon;

/// A plate's child moved sideways to keep the distance from the slanted sides it has at the
/// plate's middle. Applied as a [`UiTransform`] after layout, so it lands a frame late, while
/// the plate is still hidden.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(UiTransform)]
pub struct FollowsTheLean;

/// The node a plate's panel is drawn on, put first among its children.
#[derive(Component)]
struct PlatePanel;

/// A plate in a column, its lines centered.
pub fn plate(swatch: Swatch) -> impl Bundle {
    (Plate::new(swatch), plate_node())
}

/// [`plate`]'s node, for a plate placed some other way: `Node { left, ..plate_node() }`.
pub fn plate_node() -> Node {
    Node { padding: PLATE_PADDING, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, ..default() }
}

pub fn plate_text(font: &Handle<Font>, words: String, size: f32) -> impl Bundle {
    (PlateText, hud_text(font, words, size, Color::WHITE))
}

fn panel_node() -> impl Bundle {
    (
        PlatePanel,
        // Left at its static position, it would start inside the padding.
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        Pickable::IGNORE,
        Visibility::Hidden,
    )
}

#[allow(clippy::type_complexity)]
fn paint_the_plates(
    mut commands: Commands,
    mut plates: Query<(Entity, &mut Plate, &ComputedNode, Option<&Children>)>,
    panels: Query<(), With<PlatePanel>>,
    mut lines: Query<&mut TextColor, With<PlateText>>,
    mut icons: Query<&mut ImageNode, With<PlateIcon>>,
    mut hidden: Query<&mut Visibility, Or<(With<PlatePanel>, With<PlateText>, With<PlateIcon>)>>,
    mut materials: Option<ResMut<Assets<ShapeMaterial>>>,
) {
    for (entity, mut plate, computed, children) in &mut plates {
        let ink = TextColor(plate.swatch.text());
        for &child in children.into_iter().flatten() {
            if let Ok(mut color) = lines.get_mut(child) {
                color.set_if_neq(ink);
            }
            if let Ok(mut icon) = icons.get_mut(child)
                && icon.color != ink.0
            {
                icon.color = ink.0;
            }
        }

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

        // Within half a pixel the panel already drawn still fits; layout's sizes jitter by fractions.
        let swatch = plate.swatch;
        let joins = plate.joins;
        let fits = plate.drawn.is_some_and(|(drawn, painted, joined)| painted == swatch && joined == joins && (drawn - size).abs().max_element() <= 0.5);
        if !fits && let Some(materials) = materials.as_deref_mut() {
            let mut corners = leaning_corners(plate.leans_right, Corner::squircle(16.0), Corner::squircle(6.0));
            // Clockwise from the top left, so the left side is the first and last.
            if joins.left {
                corners[0] = Corner::SHARP;
                corners[3] = Corner::SHARP;
            }
            if joins.right {
                corners[1] = Corner::SHARP;
                corners[2] = Corner::SHARP;
            }
            let outline = leaning_panel_each(size, plate.leans_right, corners);
            let material = materials.add(ShapeMaterial::from_path(&outline, size, panel_style(&swatch)));
            commands.entity(panel).insert(MaterialNode(material));
            plate.drawn = Some((size, swatch, joins));
        }

        // The panel spawned this frame has yet to be laid out.
        if !plate.shown && panels.contains(panel) {
            plate.shown = true;
            for &child in children.into_iter().flatten() {
                if let Ok(mut visibility) = hidden.get_mut(child) {
                    *visibility = Visibility::Inherited;
                }
            }
        }
    }
}

/// Measured after layout, since the slant depends on each plate's height; the margins land in the
/// next frame's layout, while a new plate is still hidden. Only children next to each other in the
/// row join, so anything else between two plates keeps them apart.
fn join_the_rows(
    rows: Query<&Children, With<PlateRow>>,
    mut plates: Query<(&mut Plate, &mut Node, &ComputedNode)>,
) {
    for children in &rows {
        let children: Vec<Entity> = children.iter().collect();
        for (index, &child) in children.iter().enumerate() {
            let is_plate = |at: Option<&Entity>| at.is_some_and(|entity| plates.contains(*entity));
            let joins = Joins {
                left: index > 0 && is_plate(children.get(index - 1)),
                right: is_plate(children.get(index + 1)),
            };
            let Ok((mut plate, mut node, computed)) = plates.get_mut(child) else { continue };
            if plate.joins != joins {
                plate.joins = joins;
            }
            let height = computed.size().y * computed.inverse_scale_factor();
            let left = match joins.left {
                true => Val::Px(-overlap(height).round()),
                false => Val::ZERO,
            };
            if node.margin.left != left {
                node.margin.left = left;
            }
        }
    }
}

fn follow_the_lean(
    plates: Query<(&Plate, &UiGlobalTransform, &ComputedNode, &Children)>,
    mut followers: Query<(&UiGlobalTransform, &mut UiTransform), With<FollowsTheLean>>,
) {
    for (plate, plate_at, computed, children) in &plates {
        let lean = if plate.leans_right { -LEAN } else { LEAN };
        // Measured on screen but applied inside the plate, so a plate scaled down (the class
        // picker's) would otherwise lean its lines by its scale twice over.
        let scale = plate_at.to_scale_angle_translation().0.y.abs().max(f32::EPSILON);
        for &child in children {
            let Ok((child_at, mut transform)) = followers.get_mut(child) else { continue };
            // Physical pixels, and y grows downward.
            let below = (child_at.translation.y - plate_at.translation.y) * computed.inverse_scale_factor() / scale;
            let x = Val::Px((lean * below).round());
            if transform.translation.x != x {
                transform.translation.x = x;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::team::Team;
    use bevy::ecs::system::RunSystemOnce;

    /// Breaks if a plate shows before it has a panel, never shows, or its text ignores the swatch.
    #[test]
    fn a_plate_gets_a_panel_then_shows_and_inks_its_text_for_its_swatch() {
        let mut world = World::new();
        let swatch = Swatch::team(Team::Yellow);
        let plate = world
            .spawn((plate(swatch), ComputedNode { size: Vec2::new(120.0, 40.0), ..default() }))
            .with_child(plate_text(&Handle::default(), "Somebody".into(), 18.0))
            .id();

        world.run_system_once(paint_the_plates).unwrap();
        let children = world.get::<Children>(plate).unwrap().to_vec();
        assert!(world.get::<PlatePanel>(children[0]).is_some(), "the panel is not first");
        assert_eq!(world.get::<TextColor>(children[1]), Some(&TextColor(swatch.text())));
        let shown = |world: &World| children.iter().map(|child| *world.get::<Visibility>(*child).unwrap()).collect::<Vec<_>>();
        assert_eq!(shown(&world), [Visibility::Hidden; 2], "shown with a panel not yet laid out");

        world.run_system_once(paint_the_plates).unwrap();
        assert_eq!(shown(&world), [Visibility::Inherited; 2]);
        assert_eq!(world.get::<Visibility>(plate), Some(&Visibility::Inherited), "the plate's own visibility was touched");
        assert_eq!(world.get::<Children>(plate).unwrap().len(), 2, "a second panel");
    }

    /// Breaks if plates in a row are not pulled together by the slant and both strokes, keep round
    /// corners where they meet, or the ends lose theirs.
    #[test]
    fn plates_in_a_row_are_joined_at_their_slanted_sides() {
        let mut world = World::new();
        let height = 60.0;
        let sized = ComputedNode { size: Vec2::new(100.0, height), inverse_scale_factor: 1.0, ..default() };
        let members: Vec<Entity> = (0..3).map(|_| world.spawn((plate(Swatch::neutral()), sized)).id()).collect();
        world.spawn(plate_row()).add_children(&members);

        world.run_system_once(join_the_rows).unwrap();
        let joins = |entity| world.get::<Plate>(entity).unwrap().joins;
        let margin = |entity| world.get::<Node>(entity).unwrap().margin.left;
        assert_eq!(joins(members[0]), Joins { left: false, right: true });
        assert_eq!(joins(members[1]), Joins { left: true, right: true });
        assert_eq!(joins(members[2]), Joins { left: true, right: false });
        assert_eq!(margin(members[0]), Val::ZERO, "the first plate moved");
        // The next plate's top left lands on this one's top right.
        let expected = 2.0 * ZONE_STROKE + ZONE_DROP.x + LEAN * (height - 2.0 * ZONE_STROKE - ZONE_DROP.y);
        assert_eq!(margin(members[1]), Val::Px(-expected.round()));
        assert_eq!(margin(members[2]), Val::Px(-expected.round()));
    }

    /// Breaks if two plates with something else between them are joined or pulled together.
    #[test]
    fn only_neighboring_plates_in_a_row_join() {
        let mut world = World::new();
        let sized = ComputedNode { size: Vec2::new(100.0, 60.0), inverse_scale_factor: 1.0, ..default() };
        let first = world.spawn((plate(Swatch::neutral()), sized)).id();
        let spacer = world.spawn(Node::default()).id();
        let second = world.spawn((plate(Swatch::neutral()), sized)).id();
        world.spawn(plate_row()).add_children(&[first, spacer, second]);

        world.run_system_once(join_the_rows).unwrap();
        assert_eq!(world.get::<Plate>(first).unwrap().joins, Joins::default());
        assert_eq!(world.get::<Plate>(second).unwrap().joins, Joins::default());
        assert_eq!(world.get::<Node>(second).unwrap().margin.left, Val::ZERO);
    }

    /// Breaks if a child above the middle of a right-leaning plate is not moved right, one below
    /// it not moved left, or the middle one moved at all.
    #[test]
    fn children_follow_a_right_leaning_plate() {
        let mut world = World::new();
        let at = |y: f32| UiGlobalTransform::from_translation(Vec2::new(100.0, y));
        let row = |world: &mut World, y: f32| world.spawn((FollowsTheLean, at(y))).id();
        let rows = [row(&mut world, 0.0), row(&mut world, 100.0), row(&mut world, 200.0)];
        world.spawn((plate(Swatch::neutral()), at(100.0), ComputedNode { inverse_scale_factor: 1.0, ..default() })).add_children(&rows);

        world.run_system_once(follow_the_lean).unwrap();
        let x = |entity| world.get::<UiTransform>(entity).unwrap().translation.x;
        assert_eq!([x(rows[0]), x(rows[1]), x(rows[2])], [Val::Px(12.0), Val::Px(0.0), Val::Px(-12.0)]);
    }
}
