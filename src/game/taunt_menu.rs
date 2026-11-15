//! The taunt menu: `G` puts it up, `G` again plays the default taunt (the held weapon's own, else
//! the first the body's rig has a clip for) and a digit plays the one over it. Up, it owns the digits, so a press does not
//! also choose a weapon. `G` while taunting ends the taunt instead.
//!
//! What is on this machine's screen, so local: choosing writes a [`TauntRequest`] onto the latch
//! and the authority decides. [`answer_the_taunt_keys`] is the one writer of [`TauntMenu`]; it
//! takes the menu down when the body changes, a taunt is playing, or the mouse is held by
//! another menu, so no other system has to remember to.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::fonts::DisplayFont;
use crate::common::player_color::PlayerColor;
use crate::common::skeleton::rig::HUMANOID;
use crate::common::skeleton::Skeleton;
use crate::common::swatch::{Ink, Swatch};
use crate::common::taunt::{TauntId, TauntRequest, Taunts, WeaponTaunts};
use crate::common::weapon::Equipped;
use crate::common::team::Team;
use crate::common::ui_shape::{Corner, ShapeMaterial, ShapeStyle};
use crate::game::condition::Condition;
use crate::game::hud::{hud_text, leaning_panel, panel_style, SafeZone, ZONE_DROP};
use crate::game::kill_icon::KillIcons;
use crate::game::pause_menu::MouseHeld;
use crate::game::plate::{plate_node, plate_text, Plate};
use crate::game::player::{InputLatch, LocalPlayer};
use crate::game::weapon_select::DIGITS;
use crate::get;

/// Logical pixels.
const SQUARE: f32 = 68.0;
const ICON: f32 = 44.0;
const GAP: f32 = 8.0;
const TITLE_SIZE: f32 = 18.0;
const DIGIT_SIZE: f32 = 18.0;
/// How far up the safe zone the menu's bottom edge sits, percent.
const FROM_THE_BOTTOM: f32 = 26.0;

pub struct TauntMenuPlugin;

impl Plugin for TauntMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TauntMenu>()
            .add_systems(Update, show_the_taunt_menu.run_if(in_state(AppMode::Play)))
            .add_systems(OnExit(AppMode::Play), close_the_menu);
    }
}

#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct TauntMenu {
    open: bool,
    /// The menu took a key this frame, so nothing after it may read the same press.
    answered: bool,
    /// The body it was last answered for: a new one is a new life, and closes it.
    body: Option<Entity>,
}

impl TauntMenu {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the digits pressed this frame are the menu's: while it is up, and on the frame
    /// one of them took it down.
    pub fn holds_the_digits(&self) -> bool {
        self.open || self.answered
    }
}

fn offered(
    taunts: Option<&Taunts>,
    weapons: Option<&WeaponTaunts>,
    rig: Option<&Skeleton>,
    equipped: Option<&Equipped>,
) -> Vec<(TauntId, Option<String>)> {
    let kind = rig.map_or(HUMANOID, Skeleton::kind);
    let none = WeaponTaunts::default();
    let weapons = weapons.unwrap_or(&none);
    let held = equipped.and_then(Equipped::held);
    taunts.map_or_else(Vec::new, |taunts| {
        taunts.offered(kind, weapons, held).map(|taunt| (taunt.id, taunt.icon.clone())).collect()
    })
}

/// What a frame's keys do to the menu and the latch, given what was up and whether a taunt is
/// playing. Pure, so the rules are testable without a world.
fn answer(open: bool, taunting: bool, offered: &[TauntId], g: bool, digit: Option<usize>) -> (TauntMenu, Option<TauntRequest>) {
    let shut = |answered| TauntMenu { open: false, answered, body: None };
    if taunting {
        return match g {
            true => (shut(true), Some(TauntRequest::Stop)),
            false => (shut(false), None),
        };
    }
    if !open {
        return match g && !offered.is_empty() {
            true => (TauntMenu { open: true, answered: true, body: None }, None),
            false => (shut(false), None),
        };
    }
    let chosen = match (g, digit) {
        (true, _) => offered.first(),
        (false, Some(digit)) => offered.get(digit),
        (false, None) => None,
    };
    match chosen {
        Some(id) => (shut(true), Some(TauntRequest::Start(*id))),
        // A digit with no taunt over it is still the menu's.
        None => (TauntMenu { open: true, answered: digit.is_some(), body: None }, None),
    }
}

/// Before `gather_input` and `choose_weapons`, so the menu has the keys first.
#[allow(clippy::type_complexity)]
pub fn answer_the_taunt_keys(
    keys: Res<ButtonInput<KeyCode>>,
    held: Option<Res<MouseHeld>>,
    taunts: Option<Res<Taunts>>,
    weapons: Option<Res<WeaponTaunts>>,
    ours: Option<Single<(Entity, &Condition, Option<&Skeleton>, Option<&Equipped>), With<LocalPlayer>>>,
    mut menu: ResMut<TauntMenu>,
    mut latch: ResMut<InputLatch>,
) {
    let held = held.is_some_and(|held| held.any());
    let Some((body, condition, rig, equipped)) = ours.filter(|_| !held).map(|ours| ours.into_inner()) else {
        menu.set_if_neq(TauntMenu::default());
        return;
    };
    let open = menu.open && menu.body == Some(body);

    let digit = DIGITS.iter().position(|key| keys.just_pressed(*key));
    let offered: Vec<TauntId> = offered(taunts.as_deref(), weapons.as_deref(), rig, equipped).into_iter().map(|(id, _)| id).collect();
    let (next, request) = answer(open, condition.taunt.is_some(), &offered, keys.just_pressed(KeyCode::KeyG), digit);
    if request.is_some() {
        latch.0.taunt = request;
    }
    menu.set_if_neq(TauntMenu { body: Some(body), ..next });
}

fn close_the_menu(mut menu: ResMut<TauntMenu>) {
    menu.set_if_neq(TauntMenu::default());
}

#[derive(Component)]
struct TauntMenuRoot;

/// What the menu was built from: the taunts' ids and icons, and the colors.
type Built = (Vec<(TauntId, Option<String>)>, Swatch);

/// The squares' panel: the plate's deep color, see-through, as an unchosen card is.
fn square_style(swatch: &Swatch) -> ShapeStyle {
    ShapeStyle {
        fill: swatch.deep.with_alpha(0.7),
        highlight: Color::NONE,
        shadow: Color::NONE,
        drop: swatch.deep.with_alpha(0.35),
        ..panel_style(swatch)
    }
}

/// Rebuilt only when the taunts offered or the colors change; shown and hidden by
/// [`TauntMenu::is_open`] alone.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn show_the_taunt_menu(
    mut commands: Commands,
    menu: Res<TauntMenu>,
    taunts: Option<Res<Taunts>>,
    weapons: Option<Res<WeaponTaunts>>,
    font: Res<DisplayFont>,
    zone: Option<Single<Entity, With<SafeZone>>>,
    ours: Option<Single<(Option<&Skeleton>, Option<&Equipped>, Option<&Team>, Option<&PlayerColor>), With<LocalPlayer>>>,
    mut roots: Query<(Entity, &mut Node), With<TauntMenuRoot>>,
    mut icons: ResMut<KillIcons>,
    pack: Option<Res<crate::common::assets::Assets>>,
    mut images: Option<ResMut<Assets<Image>>>,
    mut materials: Option<ResMut<Assets<ShapeMaterial>>>,
    mut built_from: Local<Option<Built>>,
) {
    let Ok((root, mut node)) = roots.single_mut() else {
        // Under the safe zone, which goes with the HUD; put back with it.
        if let Some(zone) = zone {
            let root = commands
                .spawn((
                    TauntMenuRoot,
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        right: Val::Px(0.0),
                        bottom: Val::Percent(FROM_THE_BOTTOM),
                        justify_content: JustifyContent::Center,
                        display: Display::None,
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .id();
            commands.entity(*zone).add_child(root);
        }
        *built_from = None;
        return;
    };

    let ours = ours.filter(|_| menu.is_open()).map(|ours| ours.into_inner());
    let display = if ours.is_some() { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }
    // Kept while down, so putting it up again builds nothing.
    let Some((rig, equipped, team, color)) = ours else { return };

    let swatch = Swatch::of_body(team.copied(), color.copied());
    let wanted = (offered(taunts.as_deref(), weapons.as_deref(), rig, equipped), swatch);
    if built_from.as_ref() == Some(&wanted) {
        return;
    }

    let panel = materials.as_deref_mut().map(|materials| {
        let size = Vec2::splat(SQUARE);
        let outline = leaning_panel(size, true, Corner::squircle(14.0), Corner::squircle(5.0));
        materials.add(ShapeMaterial::from_path(&outline, size, square_style(&swatch)))
    });
    let read = |relative: &str| pack.as_ref().and_then(|pack| pack.read_from_packs(relative));

    let mut cells = Vec::new();
    for (index, (_, icon)) in wanted.0.iter().enumerate() {
        let mut square = vec![];
        let mut backing = commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(SQUARE),
                height: Val::Px(SQUARE),
                ..default()
            },
            Pickable::IGNORE,
        ));
        if let Some(panel) = &panel {
            backing.insert(MaterialNode(panel.clone()));
        }
        square.push(backing.id());
        let image = icon.as_deref().zip(images.as_deref_mut()).and_then(|(icon, images)| icons.get(icon, read, images));
        if let Some(image) = image {
            square.push(
                commands
                    .spawn((
                        ImageNode { color: Ink::White.on(&swatch), ..ImageNode::new(image) },
                        Node { width: Val::Px(ICON), height: Val::Px(ICON), ..default() },
                        Pickable::IGNORE,
                    ))
                    .id(),
            );
        }
        let square = commands
            .spawn((
                Node {
                    width: Val::Px(SQUARE),
                    height: Val::Px(SQUARE),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    padding: UiRect { right: Val::Px(ZONE_DROP.x), bottom: Val::Px(ZONE_DROP.y), ..default() },
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .add_children(&square)
            .id();
        let digit = commands.spawn(hud_text(&font.0, (index + 1).to_string(), DIGIT_SIZE, swatch.text())).id();
        cells.push(
            commands
                .spawn((
                    Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, ..default() },
                    Pickable::IGNORE,
                ))
                .add_children(&[square, digit])
                .id(),
        );
    }

    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                justify_content: JustifyContent::Center,
                column_gap: Val::Px(GAP),
                row_gap: Val::Px(GAP),
                margin: UiRect::top(Val::Px(6.0)),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .add_children(&cells)
        .id();
    let title = commands.spawn(plate_text(&font.0, get!("hud.taunt_default"), TITLE_SIZE)).id();
    let plate = commands
        .spawn((Plate::new(swatch), Node { max_width: Val::Percent(100.0), ..plate_node() }))
        .add_children(&[title, row])
        .id();
    commands.entity(root).despawn_children().add_child(plate);
    *built_from = Some(wanted);
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAVE: TauntId = TauntId::of("wave");
    const SPAR: TauntId = TauntId::of("spar");
    const BOTH: &[TauntId] = &[WAVE, SPAR];

    #[test]
    fn g_puts_the_menu_up_and_g_again_plays_the_default() {
        let (menu, request) = answer(false, false, BOTH, true, None);
        assert!(menu.is_open() && request.is_none(), "G did not put the menu up: {menu:?} {request:?}");
        let (menu, request) = answer(true, false, BOTH, true, None);
        assert!(!menu.is_open(), "the menu stayed up after a choice");
        assert_eq!(request, Some(TauntRequest::Start(WAVE)));
    }

    #[test]
    fn a_digit_plays_the_taunt_over_it_and_holds_the_key() {
        let (menu, request) = answer(true, false, BOTH, false, Some(1));
        assert_eq!(request, Some(TauntRequest::Start(SPAR)));
        assert!(!menu.is_open() && menu.holds_the_digits(), "the digit would also choose a weapon");

        let (menu, request) = answer(true, false, BOTH, false, Some(5));
        assert_eq!(request, None, "a digit with nothing over it played something");
        assert!(menu.is_open() && menu.holds_the_digits());
    }

    #[test]
    fn digits_are_the_weapons_while_the_menu_is_down() {
        let (menu, request) = answer(false, false, BOTH, false, Some(0));
        assert_eq!(request, None);
        assert!(!menu.holds_the_digits(), "a closed menu took a weapon's digit");
    }

    #[test]
    fn g_while_taunting_ends_it_and_the_menu_stays_down() {
        let (menu, request) = answer(true, true, BOTH, true, None);
        assert_eq!(request, Some(TauntRequest::Stop));
        assert!(!menu.is_open());
        let (menu, _) = answer(true, true, BOTH, false, None);
        assert!(!menu.is_open(), "the menu stayed up over a taunt");
    }

    #[test]
    fn a_body_offered_nothing_gets_no_menu() {
        let (menu, request) = answer(false, false, &[], true, None);
        assert!(!menu.is_open() && request.is_none());
    }

    /// Every shipped taunt names an icon a pack has, and it draws.
    #[test]
    fn every_shipped_taunt_has_an_icon_that_draws() {
        let taunts = Taunts::shipped();
        assert!(!taunts.0.is_empty());
        for taunt in &taunts.0 {
            let icon = taunt.icon.as_deref().unwrap_or_else(|| panic!("`{}` names no icon", taunt.key));
            let bytes = std::fs::read(format!("assets/default/icons/{icon}.svg")).unwrap_or_else(|_| panic!("no icons/{icon}.svg"));
            let image = crate::game::kill_icon::rasterize(&bytes).unwrap_or_else(|e| panic!("{icon}: {e}"));
            assert!(image.data.as_ref().unwrap().chunks(4).any(|pixel| pixel[3] > 0), "{icon} is empty");
        }
    }
}
