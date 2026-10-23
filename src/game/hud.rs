//! The crosshair, health and what you are holding. Plain Bevy UI, not egui,
//! so it exists in a build with no editor.
//!
//! Everything but the crosshair sits in the **safe zone**: a 4:3 box the
//! height of the window, centred. On a 4:3 window it is the whole screen; on
//! an ultrawide the readouts stay near the crosshair rather than out at the
//! edges where reading them means looking away from the fight. Narrower than
//! 4:3 it is clamped to the window. `F9` outlines it.
//!
//! The HUD has a camera of its own, drawing nothing but UI, a child of the
//! play camera and ordered above the viewmodel's. On the play camera it was
//! painted over by the arms; the viewmodel camera cannot carry it either,
//! being switched off in third person.
//!
//! Ids are resolved through the catalogue every frame rather than cached:
//! `Equipped` and the catalogue arrive on different channels, and an id that
//! does not resolve yet draws blank.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::Hdr;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::Damageable;
use crate::common::weapon::{Ammo, Equipped, WeaponCatalogue};
use crate::game::player::LocalPlayer;
use crate::game::viewmodel::VIEWMODEL_CAMERA_ORDER;
use crate::get;

/// Half the gap in the middle of the crosshair, in logical pixels.
const CROSSHAIR_GAP: f32 = 4.0;
/// Logical pixels.
const CROSSHAIR_ARM: f32 = 7.0;
/// Logical pixels. Even, so an arm straddles the centre line cleanly.
const CROSSHAIR_THICKNESS: f32 = 2.0;

pub const HUD_CAMERA_ORDER: isize = VIEWMODEL_CAMERA_ORDER + 1;

const SAFE_ZONE_ASPECT: f32 = 4.0 / 3.0;
/// Logical pixels in from the safe zone's edges.
const MARGIN_X: f32 = 24.0;
const MARGIN_Y: f32 = 20.0;

const BIG: f32 = 44.0;
const SMALL: f32 = 22.0;

/// Whether the safe zone is outlined. Survives rounds: it is a debug view,
/// and somebody laying out the HUD wants it to stay on.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SafeZoneOutline(pub bool);

#[derive(Component)]
pub struct HudCamera;

#[derive(Component)]
struct HudRoot;

#[derive(Component)]
struct SafeZone;

/// Which readout a `Text` is, so one system can write them all.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum HudText {
    Health,
    WeaponName,
    Reloading,
    /// What is in the clip; for a weapon with no clip, what is left at all.
    Loaded,
    Reserve,
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SafeZoneOutline>()
            .add_systems(
                Update,
                (
                    spawn_the_hud,
                    toggle_safe_zone_outline,
                    (outline_the_safe_zone, fill_in_the_readouts),
                )
                    .chain()
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), despawn_the_hud);
    }
}

/// For the play camera's children. Every in-game UI node lands here, the
/// pause menu included, because it carries `IsDefaultUiCamera`.
pub fn hud_camera() -> impl Bundle {
    (
        HudCamera,
        bevy::ui::IsDefaultUiCamera,
        Camera2d,
        Camera {
            order: HUD_CAMERA_ORDER,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        // Must match the cameras under it, or it composites as nothing.
        Hdr,
        // It draws no scene; tonemapping again would grade the world twice.
        Tonemapping::None,
        RenderLayers::none(),
        Name::new("HUD view"),
    )
}

/// Put the HUD up once there is a body for it to describe. Keyed on
/// `Added<LocalPlayer>` because a client's body arrives from the server.
/// No camera target: the HUD camera is `IsDefaultUiCamera`.
fn spawn_the_hud(
    mut commands: Commands,
    bodies: Query<Entity, Added<LocalPlayer>>,
    existing: Query<(), With<HudRoot>>,
) {
    if bodies.is_empty() || !existing.is_empty() {
        return;
    }

    commands.spawn((
        HudRoot,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
        // A full-screen node would otherwise swallow the pause menu's clicks.
        Pickable::IGNORE,
        children![crosshair(), safe_zone()],
    ));
}

/// Full height, and a width stated in viewport heights rather than through
/// `aspect_ratio`: with that, Taffy re-derived the height from a clamped width,
/// and on a portrait window the readouts floated halfway up the screen.
/// Narrower than 4:3, `max_width` makes it the window.
fn safe_zone() -> impl Bundle {
    (
        SafeZone,
        Node {
            height: Val::Percent(100.0),
            width: Val::Vh(100.0 * SAFE_ZONE_ASPECT),
            max_width: Val::Percent(100.0),
            ..default()
        },
        // Drawn inside the box, so it is on screen when the box is the window.
        Outline::new(Val::Px(2.0), Val::Px(-2.0), Color::NONE),
        Pickable::IGNORE,
        children![health_readout(), weapon_readout()],
    )
}

/// Four bars around a gap, dead centre. The arms hang off a zero-size node
/// centred by flexbox, so each is a pixel offset from the exact middle rather
/// than a percentage that lands half a pixel out.
fn crosshair() -> impl Bundle {
    fn arm(node: Node) -> impl Bundle {
        (
            Node { position_type: PositionType::Absolute, ..node },
            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
        )
    }

    let gap = Val::Px(CROSSHAIR_GAP);
    let long = Val::Px(CROSSHAIR_ARM);
    let thick = Val::Px(CROSSHAIR_THICKNESS);
    let centred = Val::Px(-CROSSHAIR_THICKNESS * 0.5);

    (
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        },
        Pickable::IGNORE,
        children![(
            Node::default(),
            children![
                arm(Node { right: gap, top: centred, width: long, height: thick, ..default() }),
                arm(Node { left: gap, top: centred, width: long, height: thick, ..default() }),
                arm(Node { bottom: gap, left: centred, width: thick, height: long, ..default() }),
                arm(Node { top: gap, left: centred, width: thick, height: long, ..default() }),
            ],
        )],
    )
}

fn text(kind: HudText, size: f32, alpha: f32) -> impl Bundle {
    (
        kind,
        Text::new(""),
        TextFont { font_size: bevy::text::FontSize::Px(size), ..default() },
        TextColor(Color::srgba(1.0, 1.0, 1.0, alpha)),
        Pickable::IGNORE,
    )
}

fn health_readout() -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(MARGIN_X),
            bottom: Val::Px(MARGIN_Y),
            ..default()
        },
        Pickable::IGNORE,
        children![text(HudText::Health, BIG, 1.0)],
    )
}

/// The name, a reload notice when there is one, then the loaded count large
/// and the reserve small beside it, sharing a baseline.
fn weapon_readout() -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(MARGIN_X),
            bottom: Val::Px(MARGIN_Y),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::End,
            row_gap: Val::Px(2.0),
            ..default()
        },
        Pickable::IGNORE,
        children![
            text(HudText::WeaponName, SMALL, 0.85),
            (text(HudText::Reloading, SMALL, 0.85), Node { display: Display::None, ..default() }),
            (
                Node {
                    align_items: AlignItems::Baseline,
                    column_gap: Val::Px(8.0),
                    ..default()
                },
                Pickable::IGNORE,
                children![
                    text(HudText::Loaded, BIG, 1.0),
                    text(HudText::Reserve, SMALL, 0.85),
                ],
            ),
        ],
    )
}

fn toggle_safe_zone_outline(
    keys: Res<ButtonInput<KeyCode>>,
    mut outline: ResMut<SafeZoneOutline>,
) {
    if keys.just_pressed(KeyCode::F9) {
        outline.0 = !outline.0;
    }
}

/// Colour rather than insert-and-remove, which would move tables each toggle.
/// Every frame rather than on change, so a HUD spawned while it is on starts
/// outlined.
fn outline_the_safe_zone(
    shown: Res<SafeZoneOutline>,
    mut zones: Query<&mut Outline, With<SafeZone>>,
) {
    let color = if shown.0 { Color::srgba(1.0, 0.8, 0.0, 0.8) } else { Color::NONE };
    for mut outline in &mut zones {
        if outline.color != color {
            outline.color = color;
        }
    }
}

/// What each readout should say.
#[derive(Debug, Default, PartialEq)]
struct Readout {
    health: String,
    name: String,
    reloading: bool,
    loaded: String,
    reserve: String,
}

impl Readout {
    fn of(
        catalogue: &WeaponCatalogue,
        health: &Damageable,
        equipped: &Equipped,
        ammo: &Ammo,
    ) -> Self {
        let health = health.health().to_string();
        let Some(weapon) = catalogue.held_by(equipped) else {
            return Self { health, ..default() };
        };

        let state = ammo.at(equipped.held);
        let (loaded, reserve) = match weapon.magazine.clip {
            // Nothing is loaded, so the reserve is the number that matters.
            None => (state.reserve.to_string(), String::new()),
            Some(_) => (state.clip.to_string(), state.reserve.to_string()),
        };

        Self {
            health,
            name: get!(weapon.name_key),
            reloading: state.reloading > 0.0,
            loaded,
            reserve,
        }
    }

    fn text(&self, kind: HudText) -> &str {
        match kind {
            HudText::Health => &self.health,
            HudText::WeaponName => &self.name,
            HudText::Reloading => "",
            HudText::Loaded => &self.loaded,
            HudText::Reserve => &self.reserve,
        }
    }
}

/// Compares before writing: a written `Text` or `Node` counts as changed and
/// would be re-laid-out every frame.
fn fill_in_the_readouts(
    catalogue: Res<WeaponCatalogue>,
    body: Option<Single<(&Damageable, &Equipped, &Ammo), With<LocalPlayer>>>,
    mut texts: Query<(&HudText, &mut Text, &mut Node)>,
) {
    let readout = match body.as_deref() {
        Some((health, equipped, ammo)) => Readout::of(&catalogue, health, equipped, ammo),
        None => Readout::default(),
    };

    for (&kind, mut text, mut node) in &mut texts {
        if kind == HudText::Reloading {
            // Looked up while showing rather than once, so a change of
            // language reaches it.
            if readout.reloading {
                let label = get!("hud.reloading");
                if text.0 != label {
                    text.0 = label;
                }
            }
            let display = if readout.reloading { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
            continue;
        }

        let wanted = readout.text(kind);
        if text.0 != wanted {
            text.0 = wanted.to_owned();
        }
    }
}

fn despawn_the_hud(mut commands: Commands, roots: Query<Entity, With<HudRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon::Slot;
    use crate::common::weapon_file::default_catalogue;

    fn mercenary() -> (WeaponCatalogue, Equipped, Ammo) {
        let catalogue = default_catalogue();
        let (equipped, ammo) =
            catalogue.starting_equipment(crate::common::class::Class::Mercenary);
        (catalogue, equipped, ammo)
    }

    /// Breaks if the lookup panics when `Equipped` arrives before the
    /// catalogue, which is the ordinary order on a client.
    #[test]
    fn a_body_whose_weapon_has_not_arrived_yet_is_drawn_blank() {
        let (_, equipped, ammo) = mercenary();
        let health = Damageable::with_health(125);

        let readout = Readout::of(&WeaponCatalogue::default(), &health, &equipped, &ammo);
        assert_eq!(
            readout,
            Readout { health: "125".into(), ..default() },
            "a weapon nobody can name was described anyway",
        );
    }

    #[test]
    fn a_weapon_with_a_clip_shows_both_counts() {
        let (catalogue, mut equipped, ammo) = mercenary();
        equipped.select(Slot::Secondary);
        let state = ammo.at(Slot::Secondary);

        let readout = Readout::of(&catalogue, &Damageable::default(), &equipped, &ammo);
        assert_eq!(readout.loaded, state.clip.to_string());
        assert_eq!(readout.reserve, state.reserve.to_string());
    }

    #[test]
    fn a_weapon_with_no_clip_does_not_read_as_zero_of_something() {
        let (catalogue, mut equipped, ammo) = mercenary();
        equipped.select(Slot::Utility);
        let state = ammo.at(Slot::Utility);

        let readout = Readout::of(&catalogue, &Damageable::default(), &equipped, &ammo);
        assert_eq!(readout.loaded, state.reserve.to_string());
        assert_eq!(readout.reserve, "", "a weapon with no clip showed a second count");
    }

    #[test]
    fn a_reloading_weapon_says_so_and_keeps_its_counts() {
        let (catalogue, equipped, mut ammo) = mercenary();
        ammo.at_mut(equipped.held).reloading = 0.5;

        let readout = Readout::of(&catalogue, &Damageable::default(), &equipped, &ammo);
        assert!(readout.reloading);
        assert!(!readout.loaded.is_empty(), "the count vanished during a reload");
    }
}
