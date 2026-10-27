//! Choosing a weapon, Source-style: a digit picks a bucket and pressing it
//! again moves along the weapons in it; the wheel steps through everything
//! carried. The switch is immediate, and the selector stays up a moment to
//! show where you landed (see [`crate::game::weapon_select_hud`]).
//!
//! Decided here, on the machine with the keyboard, and sent as a plain
//! [`Slot`]: "the next weapon in bucket 2" depends on what is held, and a
//! replayed tick has to reach the same loadout from the same input, which a
//! slot does and "next" does not.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;

use crate::common::weapon::{Equipped, Slot, WeaponCatalogue, BUCKETS};
use crate::game::pause_menu::PauseMenu;
use crate::game::player::{InputLatch, LocalPlayer};

/// How long the selector stays up after the last choice.
pub const OPEN_SECONDS: f32 = 1.5;

/// Wheel travel in pixels that counts as one notch, for a trackpad, which
/// reports pixels rather than lines.
const PIXELS_PER_NOTCH: f32 = 40.0;

const DIGITS: [KeyCode; BUCKETS as usize] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

/// What this machine last asked for. While the selector is open a second
/// press moves on from here rather than from what the server says is held,
/// which is a round trip behind.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq)]
pub struct WeaponSelection {
    pub highlight: Option<Slot>,
    /// Seconds left showing; zero is closed.
    pub open_for: f32,
}

impl WeaponSelection {
    pub fn is_open(&self) -> bool {
        self.open_for > 0.0
    }

    fn choose(&mut self, slot: Slot) {
        self.highlight = Some(slot);
        self.open_for = OPEN_SECONDS;
    }
}

/// Every carried weapon as `(bucket, slot)`, in the order the selector shows
/// them: by bucket, then by loadout slot. A weapon the catalogue cannot name
/// yet has no bucket and is left out.
pub fn carried(equipped: &Equipped, catalogue: &WeaponCatalogue) -> Vec<(u8, Slot)> {
    let mut order: Vec<(u8, Slot)> = Slot::ALL
        .into_iter()
        .filter_map(|slot| Some((catalogue.get(equipped.get(slot)?)?.bucket, slot)))
        .collect();
    order.sort();
    order
}

/// The first weapon in `bucket`, or the one after `from` if it is in there
/// already, wrapping.
pub fn press(order: &[(u8, Slot)], from: Option<Slot>, bucket: u8) -> Option<Slot> {
    let inside: Vec<Slot> =
        order.iter().filter(|(at, _)| *at == bucket).map(|(_, slot)| *slot).collect();
    let next = match from.and_then(|from| inside.iter().position(|slot| *slot == from)) {
        Some(i) => i + 1,
        None => 0,
    };
    inside.get(next % inside.len().max(1)).copied()
}

/// `notches` along everything carried, wrapping; positive is further along.
pub fn scroll(order: &[(u8, Slot)], from: Option<Slot>, notches: i32) -> Option<Slot> {
    if order.is_empty() {
        return None;
    }
    let at = from.and_then(|from| order.iter().position(|(_, slot)| *slot == from)).unwrap_or(0);
    let to = (at as i32 + notches).rem_euclid(order.len() as i32) as usize;
    Some(order[to].1)
}

/// Read the digits and the wheel into the latch's `select`.
///
/// Not gated on the pause menu, for the reason `gather_aim` is not: the wheel
/// is a `MessageReader`, so it drains first and answers the pause itself.
pub fn choose_weapons(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    paused: Option<Res<PauseMenu>>,
    catalogue: Res<WeaponCatalogue>,
    ours: Option<Single<&Equipped, With<LocalPlayer>>>,
    mut selection: ResMut<WeaponSelection>,
    mut travel: Local<f32>,
    mut latch: ResMut<InputLatch>,
) {
    for event in wheel.read() {
        *travel += match event.unit {
            MouseScrollUnit::Line => event.y,
            MouseScrollUnit::Pixel => event.y / PIXELS_PER_NOTCH,
        };
    }

    let (Some(equipped), false) = (ours, paused.is_some_and(|menu| menu.open)) else {
        *travel = 0.0;
        selection.open_for = 0.0;
        return;
    };

    selection.open_for = (selection.open_for - time.delta_secs()).max(0.0);
    let order = carried(&equipped, &catalogue);
    // Something no longer carried (a respawn into another loadout) is not
    // somewhere to move on from.
    let carrying = |slot: &Slot| order.iter().any(|(_, at)| at == slot);
    if !selection.is_open() || !selection.highlight.is_some_and(|slot| carrying(&slot)) {
        selection.highlight = Some(equipped.held).filter(carrying);
    }

    let mut chosen = None;
    for (bucket, key) in (1..=BUCKETS).zip(DIGITS) {
        if keys.just_pressed(key) {
            chosen = press(&order, chosen.or(selection.highlight), bucket).or(chosen);
        }
    }

    let notches = travel.trunc();
    *travel -= notches;
    if notches != 0.0 {
        // Wheel up is back towards bucket 1, as Source binds it.
        chosen = scroll(&order, chosen.or(selection.highlight), -(notches as i32)).or(chosen);
    }

    if let Some(slot) = chosen {
        selection.choose(slot);
        latch.0.select = Some(slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon::WeaponId;
    use crate::common::weapon_file::default_catalogue;

    /// Bucket 1 twice over, bucket 2 once and bucket 3 thrice, out of slot
    /// order on purpose.
    fn order() -> Vec<(u8, Slot)> {
        let mut order = vec![
            (3, Slot::Melee),
            (1, Slot::Primary),
            (2, Slot::Secondary),
            (1, Slot::Utility),
            (3, Slot::Gadget),
            (3, Slot::Special),
        ];
        order.sort();
        order
    }

    #[test]
    fn a_digit_draws_the_first_weapon_in_its_bucket() {
        assert_eq!(press(&order(), Some(Slot::Secondary), 3), Some(Slot::Melee));
        assert_eq!(press(&order(), None, 1), Some(Slot::Primary));
    }

    #[test]
    fn pressing_it_again_moves_along_the_bucket_and_wraps() {
        let order = order();
        let mut at = press(&order, None, 3);
        let mut seen = vec![at];
        for _ in 0..3 {
            at = press(&order, at, 3);
            seen.push(at);
        }
        assert_eq!(
            seen,
            [Some(Slot::Melee), Some(Slot::Gadget), Some(Slot::Special), Some(Slot::Melee)]
        );
    }

    #[test]
    fn a_bucket_with_one_weapon_stays_on_it() {
        assert_eq!(press(&order(), Some(Slot::Secondary), 2), Some(Slot::Secondary));
    }

    #[test]
    fn an_empty_bucket_chooses_nothing() {
        assert_eq!(press(&order(), Some(Slot::Primary), 9), None);
    }

    #[test]
    fn the_wheel_runs_through_every_bucket_and_wraps() {
        let order = order();
        assert_eq!(scroll(&order, Some(Slot::Utility), 1), Some(Slot::Secondary));
        assert_eq!(scroll(&order, Some(Slot::Primary), -1), Some(Slot::Special));
        assert_eq!(scroll(&order, Some(Slot::Special), 1), Some(Slot::Primary));
        assert_eq!(scroll(&[], Some(Slot::Primary), 1), None);
    }

    #[test]
    fn carried_weapons_are_ordered_by_bucket_then_slot() {
        let catalogue = default_catalogue();
        let (equipped, _) = catalogue.starting_equipment(crate::common::class::Class::Mercenary);
        let order = carried(&equipped, &catalogue);

        assert_eq!(order.len(), equipped.slots.iter().flatten().count());
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
        for (bucket, slot) in &order {
            let id: WeaponId = equipped.get(*slot).unwrap();
            assert_eq!(catalogue.get(id).unwrap().bucket, *bucket);
        }
    }

    /// The shipped loadout is meant to exercise cycling; a pack that stops
    /// sharing a bucket stops testing it by hand.
    #[test]
    fn the_placeholder_class_shares_a_bucket() {
        let catalogue = default_catalogue();
        let (equipped, _) = catalogue.starting_equipment(crate::common::class::Class::Mercenary);
        let order = carried(&equipped, &catalogue);
        assert!(order.windows(2).any(|pair| pair[0].0 == pair[1].0), "{order:?}");
    }

    #[test]
    fn a_second_press_moves_on_from_what_was_asked_not_what_is_held() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let catalogue = default_catalogue();
        let (equipped, _) = catalogue.starting_equipment(crate::common::class::Class::Mercenary);
        let order = carried(&equipped, &catalogue);
        let (bucket, first) = order
            .windows(2)
            .find(|pair| pair[0].0 == pair[1].0)
            .map(|pair| pair[0])
            .unwrap();
        let second = press(&order, Some(first), bucket).unwrap();

        world.insert_resource(catalogue);
        world.init_resource::<Time>();
        world.init_resource::<WeaponSelection>();
        world.init_resource::<InputLatch>();
        world.init_resource::<Messages<MouseWheel>>();
        world.init_resource::<ButtonInput<KeyCode>>();
        // Holding something outside the bucket, which the server never
        // changes: this machine is a round trip ahead of it.
        let mut holding = equipped;
        holding.held = order.iter().find(|(at, _)| *at != bucket).unwrap().1;
        world.spawn((holding, LocalPlayer));

        let digit = DIGITS[bucket as usize - 1];
        for wanted in [first, second] {
            let mut keys = world.resource_mut::<ButtonInput<KeyCode>>();
            keys.release(digit);
            keys.clear();
            keys.press(digit);
            world.run_system_once(choose_weapons).unwrap();
            assert_eq!(world.resource::<InputLatch>().0.select, Some(wanted));
        }
    }
}
