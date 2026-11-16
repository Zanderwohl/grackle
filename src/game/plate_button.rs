//! A [`Plate`] that is a button: lighter while hovered, darker while held down, and a
//! [`ButtonActivated`] on the entity when the primary button is let go over it.
//!
//! Spawn [`plate_button`] with [`plate_text`](crate::game::plate::plate_text) children and
//! observe [`ButtonActivated`], on the button or globally.

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::picking::events::{PointerClick, PointerPress};
use bevy::picking::hover::Hovered;
use bevy::picking::pointer::{PointerButton, PointerPressState};
use bevy::prelude::*;

use crate::common::swatch::Swatch;
use crate::game::hud::{ZONE_DROP, ZONE_STROKE};
use crate::game::plate::Plate;

/// OKLCH lightness added while hovered.
const HOVER_LIGHTEN: f32 = 0.08;
/// OKLCH lightness taken away while held down.
const HELD_DARKEN: f32 = -0.12;

pub struct PlateButtonPlugin;

impl Plugin for PlateButtonPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, light_the_buttons);
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq)]
#[require(Hovered)]
#[component(on_add = listen_for_the_pointer)]
pub struct PlateButton {
    pub swatch: Swatch,
    /// Pressed on this button and not yet let go, wherever the pointer went since.
    held: bool,
}

impl PlateButton {
    pub fn new(swatch: Swatch) -> Self {
        Self { swatch, held: false }
    }

    fn shown(&self, hovered: bool) -> Swatch {
        match (hovered, self.held) {
            (true, true) => self.swatch.lightened(HELD_DARKEN),
            (true, false) => self.swatch.lightened(HOVER_LIGHTEN),
            (false, _) => self.swatch,
        }
    }
}

/// The primary button was pressed and let go over a [`PlateButton`].
#[derive(EntityEvent, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonActivated {
    pub entity: Entity,
}

/// A button in a column, its lines centered. `Pickable::default()` overrides the plate's
/// `Pickable::IGNORE`; its lines and panel stay ignored, so the button is always what is hit.
pub fn plate_button(swatch: Swatch) -> impl Bundle {
    (
        PlateButton::new(swatch),
        Plate::new(swatch),
        Pickable::default(),
        plate_button_node(),
    )
}

/// [`plate_button`]'s node, for a button sized some other way: `Node { min_width, ..plate_button_node() }`.
pub fn plate_button_node() -> Node {
    Node {
        padding: UiRect {
            left: Val::Px(ZONE_STROKE + 28.0),
            right: Val::Px(ZONE_STROKE + ZONE_DROP.x + 28.0),
            top: Val::Px(ZONE_STROKE + 8.0),
            bottom: Val::Px(ZONE_STROKE + ZONE_DROP.y + 8.0),
        },
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        ..default()
    }
}

/// Kept when `PlateButton` is removed, so adding it again does not observe twice.
#[derive(Component)]
struct ListeningForThePointer;

fn listen_for_the_pointer(mut world: DeferredWorld, context: HookContext) {
    let button = context.entity;
    if world.entity(button).contains::<ListeningForThePointer>() {
        return;
    }
    world
        .commands()
        .entity(button)
        .insert(ListeningForThePointer)
        .observe(move |press: On<PointerPress>, mut buttons: Query<&mut PlateButton>| {
            if press.button == PointerButton::Primary
                && let Ok(mut state) = buttons.get_mut(button)
            {
                state.held = true;
            }
        })
        .observe(move |click: On<PointerClick>, mut commands: Commands| {
            if click.button == PointerButton::Primary {
                commands.trigger(ButtonActivated { entity: button });
            }
        });
}

fn light_the_buttons(
    pointers: Query<&PointerPressState>,
    mut buttons: Query<(&mut PlateButton, &Hovered, &mut Plate)>,
) {
    let down = pointers.iter().any(PointerPressState::is_primary_pressed);
    for (mut button, hovered, mut plate) in &mut buttons {
        if !down && button.held {
            button.held = false;
        }
        let shown = button.shown(hovered.get());
        if plate.swatch != shown {
            plate.swatch = shown;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Its fields are private; picking writes them from pointer input.
    fn press(world: &mut World, pointer: Entity, down: bool) {
        let mut state = world.get_mut::<PointerPressState>(pointer).unwrap();
        *state.get_field_mut::<bool>("primary").unwrap() = down;
    }

    fn lightness(swatch: Swatch) -> f32 {
        Oklcha::from(swatch.base).lightness
    }

    /// Breaks if hovering or holding a button does not show on its plate, or a release leaves it held.
    #[test]
    fn a_button_lights_while_hovered_and_darkens_while_held() {
        let mut world = World::new();
        let pointer = world.spawn(PointerPressState::default()).id();
        let idle = Swatch::neutral();
        let button = world.spawn(plate_button(idle)).id();
        let plate = |world: &World| world.get::<Plate>(button).unwrap().swatch;

        world.run_system_once(light_the_buttons).unwrap();
        assert_eq!(plate(&world), idle);

        world.entity_mut(button).insert(Hovered(true));
        world.run_system_once(light_the_buttons).unwrap();
        assert!(lightness(plate(&world)) > lightness(idle), "hovering did not light it");

        world.get_mut::<PlateButton>(button).unwrap().held = true;
        press(&mut world, pointer, true);
        world.run_system_once(light_the_buttons).unwrap();
        assert!(lightness(plate(&world)) < lightness(idle), "holding did not darken it");

        press(&mut world, pointer, false);
        world.run_system_once(light_the_buttons).unwrap();
        assert!(!world.get::<PlateButton>(button).unwrap().held, "letting go left it held");
    }
}
