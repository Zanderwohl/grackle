//! A HUD panel sized to its text: the leaning quadrilateral of [`leaning_panel`] behind a column
//! of lines, painted in a [`Swatch`], each [`PlateText`] line in whichever of black and white
//! stands out on it ([`Swatch::text`]).
//!
//! Spawn [`plate`] with [`plate_text`] children and any [`PlateIcon`] images, and set
//! [`Plate::swatch`] to recolor it. After layout, [`paint_the_plates`] gives the plate its panel
//! (the first child) and repaints it when the plate's size or swatch changes. Its lines and panel stay hidden until the panel has been
//! through layout, so a new plate never shows text with no panel behind it. The plate's own
//! `Visibility` is never touched: it belongs to whoever spawned it.

use bevy::prelude::*;

use crate::common::swatch::Swatch;
use crate::common::ui_shape::{Corner, ShapeMaterial};
use crate::game::hud::{hud_text, leaning_panel, panel_style, LEAN, ZONE_DROP, ZONE_STROKE};

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
        app.add_systems(PostUpdate, (paint_the_plates, follow_the_lean).after(bevy::ui::UiSystems::Layout));
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq)]
#[require(Node, Pickable = Pickable::IGNORE)]
pub struct Plate {
    pub swatch: Swatch,
    pub leans_right: bool,
    /// The size and colors the panel was built for.
    drawn: Option<(Vec2, Swatch)>,
    shown: bool,
}

impl Plate {
    pub fn new(swatch: Swatch) -> Self {
        Self { swatch, leans_right: true, drawn: None, shown: false }
    }
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
        let fits = plate.drawn.is_some_and(|(drawn, painted)| painted == swatch && (drawn - size).abs().max_element() <= 0.5);
        if !fits && let Some(materials) = materials.as_deref_mut() {
            let outline = leaning_panel(size, plate.leans_right, Corner::squircle(16.0), Corner::squircle(6.0));
            let material = materials.add(ShapeMaterial::from_path(&outline, size, panel_style(&swatch)));
            commands.entity(panel).insert(MaterialNode(material));
            plate.drawn = Some((size, swatch));
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

fn follow_the_lean(
    plates: Query<(&Plate, &UiGlobalTransform, &ComputedNode, &Children)>,
    mut followers: Query<(&UiGlobalTransform, &mut UiTransform), With<FollowsTheLean>>,
) {
    for (plate, plate_at, computed, children) in &plates {
        let lean = if plate.leans_right { -LEAN } else { LEAN };
        for &child in children {
            let Ok((child_at, mut transform)) = followers.get_mut(child) else { continue };
            // Physical pixels, and y grows downward.
            let below = (child_at.translation.y - plate_at.translation.y) * computed.inverse_scale_factor();
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
