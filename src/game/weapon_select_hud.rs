//! The weapon selector: a row of buckets across the top, the open one a card
//! per weapon with its thumbnail. It draws [`WeaponSelection`] and picks
//! nothing.
//!
//! Rebuilt rather than patched when what it shows changes: a handful of nodes,
//! changing only on input or a new picture.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::fonts::DisplayFont;
use crate::common::swatch::{Ink, Swatch};
use crate::common::team::Team;
use crate::common::ui_shape::{Corner, ShapeMaterial, ShapeStyle};
use crate::common::weapon::{Equipped, Slot, WeaponCatalog};
use crate::game::hud::{hud_text, leaning_panel, panel_style, ZONE_DROP, ZONE_STROKE};
use crate::game::player::LocalPlayer;
use crate::game::weapon_select::{carried, WeaponSelection};
use crate::get;
use crate::prop::thumbnail::Thumbnails;

/// Logical pixels.
const CARD: Vec2 = Vec2::new(176.0, 116.0);
const CLOSED: Vec2 = Vec2::new(64.0, 50.0);
const PICTURE: f32 = 72.0;
const GAP: f32 = 4.0;
const TOP: f32 = 20.0;
const NAME_SIZE: f32 = 15.0;
const DIGIT_SIZE: f32 = 20.0;

pub struct WeaponSelectHudPlugin;

impl Plugin for WeaponSelectHudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, show_the_selector.run_if(in_state(AppMode::Play)))
            .add_systems(OnExit(AppMode::Play), despawn_the_selector);
    }
}

#[derive(Component)]
struct SelectorRoot;

#[derive(Debug, Clone, PartialEq)]
struct Shown {
    team: Option<Team>,
    columns: Vec<Column>,
}

#[derive(Debug, Clone, PartialEq)]
struct Column {
    bucket: u8,
    /// Empty for a closed bucket.
    cards: Vec<Card>,
}

#[derive(Debug, Clone, PartialEq)]
struct Card {
    name: String,
    /// `None` for an unmodeled weapon, or one not photographed yet.
    picture: Option<Handle<Image>>,
    highlighted: bool,
}

impl Shown {
    fn of(
        catalog: &WeaponCatalog,
        equipped: &Equipped,
        highlight: Option<Slot>,
        thumbnails: Option<&Thumbnails>,
        team: Option<Team>,
    ) -> Self {
        let order = carried(equipped, catalog);
        let open = order.iter().find(|(_, slot)| Some(*slot) == highlight).map(|(at, _)| *at);

        let mut columns: Vec<Column> = Vec::new();
        for (bucket, slot) in order {
            if columns.last().is_none_or(|column| column.bucket != bucket) {
                columns.push(Column { bucket, cards: Vec::new() });
            }
            if Some(bucket) != open {
                continue;
            }
            let Some(weapon) = equipped.get(slot).and_then(|id| catalog.get(id)) else {
                continue;
            };
            let picture = weapon
                .model
                .as_deref()
                .and_then(|model| thumbnails?.prop(model))
                .cloned();
            let card = Card { name: get!(weapon.name_key), picture, highlighted: highlight == Some(slot) };
            columns.last_mut().expect("pushed above").cards.push(card);
        }
        Self { team, columns }
    }
}

fn dimmed(swatch: &Swatch) -> ShapeStyle {
    ShapeStyle {
        fill: swatch.deep.with_alpha(0.7),
        highlight: Color::NONE,
        shadow: Color::NONE,
        drop: swatch.deep.with_alpha(0.35),
        ..panel_style(swatch)
    }
}

/// Compared first, so [`Shown`] and its lang lookups are not rebuilt every
/// frame.
type ShownFrom = (Option<Slot>, Equipped, Option<Team>, usize);

/// Made once per team rather than per card.
struct Panels {
    team: Option<Team>,
    lit: Handle<ShapeMaterial>,
    dim: Handle<ShapeMaterial>,
    closed: Handle<ShapeMaterial>,
}

impl Panels {
    fn new(team: Option<Team>, materials: &mut Assets<ShapeMaterial>) -> Self {
        let swatch = Swatch::of(team);
        let mut make = |size: Vec2, style: ShapeStyle| {
            let outline = leaning_panel(size, true, Corner::squircle(18.0), Corner::squircle(6.0));
            materials.add(ShapeMaterial::from_path(&outline, size, style))
        };
        Self {
            team,
            lit: make(CARD, panel_style(&swatch)),
            dim: make(CARD, dimmed(&swatch)),
            closed: make(CLOSED, dimmed(&swatch)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn show_the_selector(
    mut commands: Commands,
    selection: Res<WeaponSelection>,
    catalog: Res<WeaponCatalog>,
    thumbnails: Option<Res<Thumbnails>>,
    font: Res<DisplayFont>,
    ours: Option<Single<(&Equipped, Option<&Team>), With<LocalPlayer>>>,
    mut roots: Query<(Entity, &mut Node), With<SelectorRoot>>,
    materials: Option<ResMut<Assets<ShapeMaterial>>>,
    mut built_from: Local<Option<ShownFrom>>,
    mut panels: Local<Option<Panels>>,
) {
    let Ok((root, mut node)) = roots.single_mut() else {
        commands.spawn((
            SelectorRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(TOP),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::FlexStart,
                column_gap: Val::Px(GAP),
                display: Display::None,
                ..default()
            },
            Pickable::IGNORE,
        ));
        *built_from = None;
        return;
    };

    let ours = ours.filter(|_| selection.is_open()).map(|ours| ours.into_inner());
    let display = if ours.is_some() { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }

    // Kept while closed, so reopening onto the same thing rebuilds nothing.
    let Some((equipped, team)) = ours else { return };
    let team = team.copied();
    let from = (selection.highlight, *equipped, team, thumbnails.as_ref().map_or(0, |t| t.len()));
    if built_from.as_ref() == Some(&from) && !catalog.is_changed() {
        return;
    }
    *built_from = Some(from);
    let wanted = Shown::of(&catalog, equipped, selection.highlight, thumbnails.as_deref(), team);

    let swatch = Swatch::of(team);
    // Absent in a test app with no renderer; the text still lays out.
    if let Some(mut materials) = materials
        && panels.as_ref().is_none_or(|panels| panels.team != team)
    {
        *panels = Some(Panels::new(team, &mut materials));
    }
    let panels = panels.as_ref();
    let text = |words: String, size: f32, color: Color| hud_text(&font.0, words, size, color);

    let mut columns = Vec::new();
    for column in &wanted.columns {
        let digit = column.bucket.to_string();
        let mut boxes = Vec::new();

        if column.cards.is_empty() {
            let backing = panel(&mut commands, panels.map(|p| p.closed.clone()), CLOSED);
            let label = commands.spawn(text(digit.clone(), DIGIT_SIZE, Ink::White.on(&swatch))).id();
            boxes.push(
                commands
                    .spawn((
                        Node {
                            width: Val::Px(CLOSED.x),
                            height: Val::Px(CLOSED.y),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            padding: UiRect { right: Val::Px(ZONE_DROP.x), bottom: Val::Px(ZONE_DROP.y), ..default() },
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .add_children(&[backing, label])
                    .id(),
            );
        }

        for (i, card) in column.cards.iter().enumerate() {
            let (material, ink) = if card.highlighted {
                (panels.map(|p| p.lit.clone()), swatch.text())
            } else {
                (panels.map(|p| p.dim.clone()), Ink::White.on(&swatch))
            };
            let mut parts = vec![panel(&mut commands, material, CARD)];
            if i == 0 {
                parts.push(
                    commands
                        .spawn((
                            text(digit.clone(), DIGIT_SIZE, ink),
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(ZONE_STROKE + 12.0),
                                top: Val::Px(ZONE_STROKE + 2.0),
                                ..default()
                            },
                        ))
                        .id(),
                );
            }
            if let Some(picture) = &card.picture {
                let tint = if card.highlighted { Color::WHITE } else { Color::srgba(1.0, 1.0, 1.0, 0.6) };
                parts.push(
                    commands
                        .spawn((
                            ImageNode { color: tint, ..ImageNode::new(picture.clone()) },
                            Node { width: Val::Px(PICTURE), height: Val::Px(PICTURE), ..default() },
                            Pickable::IGNORE,
                        ))
                        .id(),
                );
            }
            parts.push(commands.spawn(text(card.name.clone(), NAME_SIZE, ink)).id());

            boxes.push(
                commands
                    .spawn((
                        Node {
                            width: Val::Px(CARD.x),
                            height: Val::Px(CARD.y),
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            padding: UiRect { right: Val::Px(ZONE_DROP.x), bottom: Val::Px(ZONE_DROP.y), ..default() },
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .add_children(&parts)
                    .id(),
            );
        }

        columns.push(
            commands
                .spawn((
                    Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(GAP), ..default() },
                    Pickable::IGNORE,
                ))
                .add_children(&boxes)
                .id(),
        );
    }

    commands.entity(root).despawn_children().add_children(&columns);
}

/// First among its box's children, so it is under them.
fn panel(commands: &mut Commands, material: Option<Handle<ShapeMaterial>>, size: Vec2) -> Entity {
    let mut panel = commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Px(size.x),
            height: Val::Px(size.y),
            ..default()
        },
        Pickable::IGNORE,
    ));
    if let Some(material) = material {
        panel.insert(MaterialNode(material));
    }
    panel.id()
}

fn despawn_the_selector(mut commands: Commands, roots: Query<Entity, With<SelectorRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon_file::default_catalog;

    fn mercenary() -> (WeaponCatalog, Equipped) {
        let catalog = default_catalog();
        let (equipped, _) = catalog.starting_equipment(crate::common::class::Class::Mercenary, None);
        (catalog, equipped)
    }

    #[test]
    fn only_the_bucket_being_chosen_from_is_open() {
        let (catalog, equipped) = mercenary();
        let order = carried(&equipped, &catalog);
        let (bucket, slot) = *order.iter().rev().find(|(at, _)| order.iter().filter(|(b, _)| b == at).count() > 1).unwrap();

        let shown = Shown::of(&catalog, &equipped, Some(slot), None, None);

        for column in &shown.columns {
            let inside = order.iter().filter(|(at, _)| *at == column.bucket).count();
            if column.bucket == bucket {
                assert_eq!(column.cards.len(), inside, "the open bucket is missing a weapon");
                assert_eq!(column.cards.iter().filter(|card| card.highlighted).count(), 1);
            } else {
                assert!(column.cards.is_empty(), "bucket {} opened too", column.bucket);
            }
        }
        let buckets: Vec<u8> = shown.columns.iter().map(|column| column.bucket).collect();
        let mut sorted = buckets.clone();
        sorted.dedup();
        assert_eq!(buckets, sorted, "a bucket appears twice");
    }

    /// The catalog arrives on a different channel from `Equipped`.
    #[test]
    fn weapons_nobody_can_name_yet_show_nothing() {
        let (_, equipped) = mercenary();
        let shown = Shown::of(&WeaponCatalog::default(), &equipped, Some(equipped.held), None, None);
        assert!(shown.columns.is_empty(), "{shown:?}");
    }
}
