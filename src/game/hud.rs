//! The crosshair, and what you are holding.
//!
//! Plain Bevy UI rather than `egui`, for the reason
//! [`crate::game::pause_menu`] gives: the editor's panels are egui because
//! they are tools, and this is part of the game — it has to exist in a build
//! with no editor in it and work on a browser tab where the egui layer is one
//! more thing to have gone wrong.
//!
//! There are no weapon models yet, neither view models nor world models, so
//! the weapon is a line of text. That is a placeholder and looks like one on
//! purpose.
//!
//! **Nothing here decides anything.** The HUD reads [`Equipped`] and [`Ammo`]
//! off the local body and resolves ids through the catalogue, which is a read
//! a client may make: it settles what is on the screen, never what happened.
//! And it resolves **every time it draws** rather than caching when a body
//! arrives, because a body's ids and the catalogue travel on different
//! channels — a resolution cached on arrival is a body that is never named at
//! all. An id nobody can resolve draws as nothing.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::weapon::{Ammo, Equipped, WeaponCatalogue};
use crate::game::player::LocalPlayer;
use crate::get;

/// Half the gap in the middle of the crosshair, in logical pixels.
const CROSSHAIR_GAP: f32 = 4.0;
/// How long each arm is.
const CROSSHAIR_ARM: f32 = 7.0;
/// How thick each arm is. Odd numbers do not centre cleanly on an even
/// viewport, so this is even and the arms straddle the middle.
const CROSSHAIR_THICKNESS: f32 = 2.0;

/// Marks the HUD's root, so tearing it down is a despawn of one entity.
#[derive(Component)]
struct HudRoot;

/// Marks the line of text naming what is held.
#[derive(Component)]
struct HeldWeaponText;

/// Marks the ammo counter.
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

/// Put the HUD up once there is a body for it to describe.
///
/// Keyed on `Added<LocalPlayer>` and guarded on there not being one already,
/// exactly as [`spawn_the_view`](crate::game::player::spawn_the_view) is — and
/// for the same reason: on a client the body is not spawned locally at all, it
/// arrives from the server and is marked ours when it turns out to be.
///
/// It hangs off no camera. The play camera already carries `IsDefaultUiCamera`
/// because the window has five of them; adding a second answer here would put
/// the HUD on whichever one Bevy's ambiguity rules preferred.
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
        // The HUD is something to look at and never something to click. A
        // full-screen node that ate the pointer would swallow the pause menu's
        // buttons the moment there are any.
        Pickable::IGNORE,
        children![crosshair(), weapon_readout()],
    ));
}

/// Four bars around a gap, dead centre.
///
/// A UI node rather than a gizmo: gizmos are world space, drawn in
/// `PostUpdate` from a camera's projection, and a crosshair is a fact about
/// the screen. Not a font glyph either — there are no font assets in the repo,
/// and picking one out of the embedded default font would be a design decision
/// made by accident.
///
/// The arms hang off a node of no size, centred by the flexbox, so each one is
/// an offset from the exact middle rather than a percentage that lands half a
/// pixel out on an odd viewport.
fn crosshair() -> impl Bundle {
    /// One arm, as offsets from the centre point.
    fn arm(node: Node) -> impl Bundle {
        (
            Node { position_type: PositionType::Absolute, ..node },
            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
        )
    }

    let gap = Val::Px(CROSSHAIR_GAP);
    let long = Val::Px(CROSSHAIR_ARM);
    let thick = Val::Px(CROSSHAIR_THICKNESS);
    // Half the thickness, back the other way, so an arm straddles the centre
    // line rather than sitting to one side of it.
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
            // No size at all: this is the middle of the screen, and the four
            // arms are placed from it.
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

/// The bottom-right readout: what is held, and how much of it is left.
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

/// Say what is held and how much of it there is.
///
/// **Compares before it writes.** A written `Text` is a changed one, and ammo
/// changes on every shot — rewriting an identical string sixty times a second
/// would re-lay-out the text for nothing. Same trap as re-inserting a
/// `Skeleton`, one layer up.
fn name_the_held_weapon(
    catalogue: Res<WeaponCatalogue>,
    body: Option<Single<(&Equipped, &Ammo), With<LocalPlayer>>>,
    mut named: Query<&mut Text, (With<HeldWeaponText>, Without<AmmoText>)>,
    mut counted: Query<&mut Text, (With<AmmoText>, Without<HeldWeaponText>)>,
) {
    let (name, count) = match body.as_deref() {
        Some((equipped, ammo)) => readout(&catalogue, equipped, ammo),
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
///
/// A free function so the "a weapon we cannot name yet" case can be stated as
/// a test rather than as a screenshot.
fn readout(catalogue: &WeaponCatalogue, equipped: &Equipped, ammo: &Ammo) -> (String, String) {
    // Resolved here and not cached, which is what makes a body whose
    // `Equipped` outran the catalogue draw blank instead of never drawing.
    let Some(weapon) = catalogue.held_by(equipped) else {
        return (String::new(), String::new());
    };

    let state = ammo.at(equipped.held);
    let count = if state.reloading > 0.0 {
        get!("hud.reloading")
    } else {
        match weapon.magazine.clip {
            // A slot the weapon does not have is not a zero, so a flamethrower
            // reads `200` rather than `200 / 0`.
            None => get!("hud.ammo_no_clip", "reserve", state.reserve),
            Some(_) => {
                get!("hud.ammo", "clip", state.clip, "reserve", state.reserve)
            }
        }
    };

    (get!(weapon.name_key), count)
}

/// Take it down with the round, beside `despawn_the_view`.
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

    /// A body's weapon ids and the catalogue that describes them arrive on
    /// different channels, so a client really does hold one without the other.
    /// It draws blank.
    ///
    /// This is what stops somebody tidying the lookup into an `.expect()`,
    /// which would take a listen server's clients down for arriving in the
    /// ordinary order.
    #[test]
    fn a_body_whose_weapon_has_not_arrived_yet_is_drawn_blank() {
        let catalogue = default_catalogue();
        let (equipped, ammo) =
            catalogue.starting_equipment(crate::common::class::Class::Mercenary);

        let (name, count) = readout(&WeaponCatalogue::default(), &equipped, &ammo);
        assert_eq!(name, "", "a weapon nobody can name was named anyway");
        assert_eq!(count, "");
    }

    /// A weapon with a clip reads as two numbers; one without reads as one.
    /// A slot the weapon does not have is not a zero.
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

    /// Reloading says so rather than counting down a clip that is not filling.
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
