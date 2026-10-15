//! The crosshair, and what you are holding. Plain Bevy UI, not egui, so it
//! exists in a build with no editor.
//!
//! Ids are resolved through the catalogue whenever either changes, never cached:
//! `Equipped` and the catalogue arrive on different channels, and an id that
//! does not resolve yet draws blank.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::weapon::{Ammo, Equipped, WeaponCatalogue};
use crate::game::player::LocalPlayer;
use crate::get;

/// Half the gap in the middle of the crosshair, in logical pixels.
const CROSSHAIR_GAP: f32 = 4.0;
/// Logical pixels.
const CROSSHAIR_ARM: f32 = 7.0;
/// Logical pixels. Even, so an arm straddles the centre line cleanly.
const CROSSHAIR_THICKNESS: f32 = 2.0;

#[derive(Component)]
struct HudRoot;

#[derive(Component)]
struct HeldWeaponText;

#[derive(Component)]
struct AmmoText;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (spawn_the_hud, name_the_held_weapon)
                .chain()
                .run_if(in_state(AppMode::Play)),
        )
        .add_systems(OnExit(AppMode::Play), despawn_the_hud);
    }
}

/// Put the HUD up once there is a body for it to describe. Keyed on
/// `Added<LocalPlayer>` because a client's body arrives from the server.
/// No camera target: the play camera is `IsDefaultUiCamera`.
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
            ..default()
        },
        // A full-screen node would otherwise swallow the pause menu's clicks.
        Pickable::IGNORE,
        children![crosshair(), weapon_readout()],
    ));
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

fn weapon_readout() -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(24.0),
            bottom: Val::Px(20.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::End,
            row_gap: Val::Px(2.0),
            ..default()
        },
        Pickable::IGNORE,
        children![
            (
                HeldWeaponText,
                Text::new(""),
                TextFont { font_size: bevy::text::FontSize::Px(20.0), ..default() },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
            ),
            (
                AmmoText,
                Text::new(""),
                TextFont { font_size: bevy::text::FontSize::Px(32.0), ..default() },
                TextColor(Color::WHITE),
            ),
        ],
    )
}

/// Compares before writing: a written `Text` counts as changed and would be
/// re-laid-out every frame.
fn name_the_held_weapon(
    catalogue: Res<WeaponCatalogue>,
    body: Option<Single<(Ref<Equipped>, Ref<Ammo>, Ref<LocalPlayer>)>>,
    fresh: Query<(), Or<(Added<HeldWeaponText>, Added<AmmoText>)>>,
    mut named: Query<&mut Text, (With<HeldWeaponText>, Without<AmmoText>)>,
    mut counted: Query<&mut Text, (With<AmmoText>, Without<HeldWeaponText>)>,
) {
    let (name, count) = match body.as_deref() {
        Some((equipped, ammo, ours)) => {
            // Formatting is the cost; skip it when nothing it reads has moved.
            // `ours`, because a client is handed a body whose loadout arrived
            // before it was marked.
            let stale = catalogue.is_changed()
                || equipped.is_changed()
                || ammo.is_changed()
                || ours.is_changed();
            if !stale && fresh.is_empty() {
                return;
            }
            readout(&catalogue, equipped, ammo)
        }
        None => (String::new(), String::new()),
    };

    for mut text in &mut named {
        if text.0 != name {
            text.0 = name.clone();
        }
    }
    for mut text in &mut counted {
        if text.0 != count {
            text.0 = count.clone();
        }
    }
}

/// What the two lines should say.
fn readout(catalogue: &WeaponCatalogue, equipped: &Equipped, ammo: &Ammo) -> (String, String) {
    let Some(weapon) = catalogue.held_by(equipped) else {
        return (String::new(), String::new());
    };

    let state = ammo.at(equipped.held);
    let count = if state.reloading > 0.0 {
        get!("hud.reloading")
    } else {
        match weapon.magazine.clip {
            // No clip reads `200`, not `200 / 0`.
            None => get!("hud.ammo_no_clip", "reserve", state.reserve),
            Some(_) => {
                get!("hud.ammo", "clip", state.clip, "reserve", state.reserve)
            }
        }
    };

    (get!(weapon.name_key), count)
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

    /// Breaks if the lookup panics when `Equipped` arrives before the
    /// catalogue, which is the ordinary order on a client.
    #[test]
    fn a_body_whose_weapon_has_not_arrived_yet_is_drawn_blank() {
        let catalogue = default_catalogue();
        let (equipped, ammo) =
            catalogue.starting_equipment(crate::common::class::Class::Mercenary);

        let (name, count) = readout(&WeaponCatalogue::default(), &equipped, &ammo);
        assert_eq!(name, "", "a weapon nobody can name was named anyway");
        assert_eq!(count, "");
    }

    #[test]
    fn a_weapon_with_no_clip_does_not_read_as_zero_of_something() {
        let catalogue = default_catalogue();
        let (mut equipped, ammo) =
            catalogue.starting_equipment(crate::common::class::Class::Mercenary);

        equipped.select(Slot::Secondary);
        let (_, with_clip) = readout(&catalogue, &equipped, &ammo);
        assert!(with_clip.contains('/'), "a weapon with a clip read as {with_clip}");

        equipped.select(Slot::Utility);
        let (_, no_clip) = readout(&catalogue, &equipped, &ammo);
        assert!(!no_clip.contains('/'), "a weapon with no clip read as {no_clip}");
        assert!(!no_clip.is_empty());
    }

    #[test]
    fn a_reloading_weapon_says_so() {
        let catalogue = default_catalogue();
        let (equipped, mut ammo) =
            catalogue.starting_equipment(crate::common::class::Class::Mercenary);
        ammo.at_mut(equipped.held).reloading = 0.5;

        let (_, count) = readout(&catalogue, &equipped, &ammo);
        assert_eq!(count, get!("hud.reloading"));
    }
}
