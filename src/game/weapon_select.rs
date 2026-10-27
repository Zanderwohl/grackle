//! Choosing a weapon, Source-style: a digit picks a bucket, pressing it again
//! moves along it, and the wheel steps through everything carried.
//!
//! Choosing moves only [`WeaponSelection::highlight`]. One switch is sent when
//! it settles or on a left click; a switch per notch would have the server
//! draw, and cancel the reload of, every weapon scrolled past. A bucket of one
//! weapon switches at once.
//!
//! Sent as a plain [`Slot`] rather than "next in bucket 2", so a replayed tick
//! reaches the same loadout.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;

use crate::common::weapon::{Equipped, Slot, WeaponCatalogue, BUCKETS};
use crate::game::pause_menu::PauseMenu;
use crate::game::player::{InputLatch, LocalPlayer};

/// How long the selector stays up after the last choice.
pub const OPEN_SECONDS: f32 = 1.5;

/// Quiet before a choice is sent. Shorter than [`OPEN_SECONDS`], so nothing is
/// left pending behind a closed selector.
pub const SETTLE_SECONDS: f32 = 0.35;

/// For a trackpad, which reports pixels rather than lines.
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

/// What this machine has chosen. A second press moves on from here, not from
/// `Equipped`, which is a round trip behind.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq)]
pub struct WeaponSelection {
    pub highlight: Option<Slot>,
    /// Seconds left showing; zero is closed.
    pub open_for: f32,
    /// Seconds of quiet left before the highlight is sent.
    pending: Option<f32>,
    /// By [`Button`](crate::game::weapon::Button) index: a click that
    /// confirmed or cancelled, still held, is not a shot until released.
    swallowing: [bool; 2],
    /// Sent and not yet seen held: the old weapon is being put away.
    sent: Option<Slot>,
    /// A new body, from a respawn or a new round, starts from nothing.
    body: Option<Entity>,
}

impl WeaponSelection {
    pub fn is_open(&self) -> bool {
        self.open_for > 0.0
    }

    /// Nothing should guess a shot from the mouse buttons.
    pub fn holds_the_buttons(&self) -> bool {
        self.pending.is_some() || self.sent.is_some() || self.swallowing.contains(&true)
    }

    fn choose(&mut self, slot: Slot) {
        self.highlight = Some(slot);
        self.open_for = OPEN_SECONDS;
    }

    fn send(&mut self, latch: &mut InputLatch) {
        self.pending = None;
        self.sent = self.highlight;
        latch.0.select = self.highlight;
    }

    fn forget(&mut self) {
        *self = Self { highlight: self.highlight, body: self.body, ..Self::default() };
    }
}

/// Sorted by bucket, then slot. A weapon the catalogue cannot name yet is left
/// out.
pub fn carried(equipped: &Equipped, catalogue: &WeaponCatalogue) -> Vec<(u8, Slot)> {
    let mut order: Vec<(u8, Slot)> = Slot::ALL
        .into_iter()
        .filter_map(|slot| Some((catalogue.get(equipped.get(slot)?)?.bucket, slot)))
        .collect();
    order.sort();
    order
}

/// The first weapon in `bucket`, or the one after `from` if `from` is in it.
pub fn press(order: &[(u8, Slot)], from: Option<Slot>, bucket: u8) -> Option<Slot> {
    let inside: Vec<Slot> =
        order.iter().filter(|(at, _)| *at == bucket).map(|(_, slot)| *slot).collect();
    let next = match from.and_then(|from| inside.iter().position(|slot| *slot == from)) {
        Some(i) => i + 1,
        None => 0,
    };
    inside.get(next % inside.len().max(1)).copied()
}

/// Wraps; positive is further along.
pub fn scroll(order: &[(u8, Slot)], from: Option<Slot>, notches: i32) -> Option<Slot> {
    if order.is_empty() {
        return None;
    }
    let at = from.and_then(|from| order.iter().position(|(_, slot)| *slot == from)).unwrap_or(0);
    let to = (at as i32 + notches).rem_euclid(order.len() as i32) as usize;
    Some(order[to].1)
}

/// Not gated on the pause menu: like `gather_aim`, it drains a
/// `MessageReader` first. After `gather_input`, whose clicks it may take back.
pub fn choose_weapons(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    paused: Option<Res<PauseMenu>>,
    catalogue: Res<WeaponCatalogue>,
    ours: Option<Single<(Entity, &Equipped), With<LocalPlayer>>>,
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

    let (Some(ours), false) = (ours, paused.is_some_and(|menu| menu.open)) else {
        *travel = 0.0;
        selection.forget();
        return;
    };
    let (body, equipped) = ours.into_inner();

    if selection.body != Some(body) {
        *selection = WeaponSelection { body: Some(body), ..default() };
    }
    let dt = time.delta_secs();
    selection.open_for = (selection.open_for - dt).max(0.0);
    if selection.sent == Some(equipped.held) {
        selection.sent = None;
    }
    if !selection.is_open() {
        *selection = WeaponSelection {
            highlight: Some(equipped.held),
            swallowing: selection.swallowing,
            body: selection.body,
            ..default()
        };
    }

    let pressed: Vec<u8> =
        (1..=BUCKETS).zip(DIGITS).filter(|(_, key)| keys.just_pressed(*key)).map(|(b, _)| b).collect();
    let notches = travel.trunc();
    *travel -= notches;

    let mut chosen = None;
    let mut at_once = false;
    if !pressed.is_empty() || notches != 0.0 {
        let order = carried(equipped, &catalogue);
        for bucket in pressed {
            if let Some(slot) = press(&order, chosen.or(selection.highlight), bucket) {
                chosen = Some(slot);
                at_once = order.iter().filter(|(at, _)| *at == bucket).count() == 1;
            }
        }
        // Wheel up is back towards bucket 1, as Source binds it.
        if notches != 0.0
            && let Some(slot) = scroll(&order, chosen.or(selection.highlight), -(notches as i32))
        {
            chosen = Some(slot);
            at_once = false;
        }
    }

    if let Some(slot) = chosen {
        selection.choose(slot);
        if at_once {
            selection.send(&mut latch);
        } else {
            selection.pending = Some(SETTLE_SECONDS);
        }
    } else if let Some(left) = selection.pending {
        let left = left - dt;
        selection.pending = Some(left);
        if left <= 0.0 {
            selection.send(&mut latch);
        }
    }

    if selection.pending.is_some() {
        if latch.0.attack {
            selection.send(&mut latch);
            selection.swallowing[0] = true;
        } else if latch.0.alt_attack {
            selection.highlight = Some(equipped.held);
            selection.forget();
            selection.swallowing[1] = true;
        }
    }

    // `gather_input` rewrites the held flags every frame.
    let input = &mut latch.0;
    let buttons = [
        (&mut input.attack, &mut input.attack_held),
        (&mut input.alt_attack, &mut input.alt_attack_held),
    ];
    for ((pressed, held), swallowing) in buttons.into_iter().zip(&mut selection.swallowing) {
        if !*swallowing {
            continue;
        }
        if *held || *pressed {
            *pressed = false;
            *held = false;
        } else {
            *swallowing = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon::WeaponId;
    use crate::common::weapon_file::default_catalogue;

    /// Out of slot order on purpose.
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

    /// Cycling can only be tried by hand if the shipped loadout shares a bucket.
    #[test]
    fn the_placeholder_class_shares_a_bucket() {
        let catalogue = default_catalogue();
        let (equipped, _) = catalogue.starting_equipment(crate::common::class::Class::Mercenary);
        let order = carried(&equipped, &catalogue);
        assert!(order.windows(2).any(|pair| pair[0].0 == pair[1].0), "{order:?}");
    }

    use bevy::ecs::system::RunSystemOnce;

    /// Holding something outside the first shared bucket, which nothing here
    /// changes. Returns that bucket and its slots.
    fn a_chooser() -> (World, u8, Vec<Slot>) {
        let mut world = World::new();
        let catalogue = default_catalogue();
        let (mut equipped, _) =
            catalogue.starting_equipment(crate::common::class::Class::Mercenary);
        let order = carried(&equipped, &catalogue);
        let bucket = order.windows(2).find(|pair| pair[0].0 == pair[1].0).unwrap()[0].0;
        let inside = order.iter().filter(|(at, _)| *at == bucket).map(|(_, slot)| *slot).collect();
        equipped.held = order.iter().find(|(at, _)| *at != bucket).unwrap().1;

        world.insert_resource(catalogue);
        world.init_resource::<Time>();
        world.init_resource::<WeaponSelection>();
        world.init_resource::<InputLatch>();
        world.init_resource::<Messages<MouseWheel>>();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.spawn((equipped, LocalPlayer));
        (world, bucket, inside)
    }

    fn frame(world: &mut World, seconds: f32, key: Option<KeyCode>) {
        world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(seconds));
        let mut keys = world.resource_mut::<ButtonInput<KeyCode>>();
        keys.reset_all();
        if let Some(key) = key {
            keys.press(key);
        }
        world.run_system_once(choose_weapons).unwrap();
    }

    fn sent(world: &World) -> Option<Slot> {
        world.resource::<InputLatch>().0.select
    }

    fn highlight(world: &World) -> Option<Slot> {
        world.resource::<WeaponSelection>().highlight
    }

    #[test]
    fn a_second_press_moves_on_from_what_was_chosen_not_what_is_held() {
        let (mut world, bucket, inside) = a_chooser();
        let digit = DIGITS[bucket as usize - 1];

        frame(&mut world, 0.0, Some(digit));
        assert_eq!(highlight(&world), Some(inside[0]));
        frame(&mut world, 0.01, Some(digit));
        assert_eq!(highlight(&world), Some(inside[1]));
    }

    #[test]
    fn moving_through_a_bucket_sends_one_switch_once_it_settles() {
        let (mut world, bucket, inside) = a_chooser();
        let digit = DIGITS[bucket as usize - 1];

        frame(&mut world, 0.0, Some(digit));
        frame(&mut world, 0.05, Some(digit));
        assert_eq!(sent(&world), None, "a weapon passed through was sent");

        frame(&mut world, SETTLE_SECONDS * 0.5, None);
        assert_eq!(sent(&world), None, "sent before it settled");
        frame(&mut world, SETTLE_SECONDS, None);
        assert_eq!(sent(&world), Some(inside[1]));
    }

    #[test]
    fn a_bucket_of_one_switches_at_once() {
        let (mut world, _, _) = a_chooser();
        let catalogue = world.resource::<WeaponCatalogue>().clone();
        let equipped = *world.query::<&Equipped>().single(&world).unwrap();
        let order = carried(&equipped, &catalogue);
        let (alone, slot) = *order
            .iter()
            .find(|(at, _)| order.iter().filter(|(b, _)| b == at).count() == 1)
            .expect("the placeholder class has no bucket of one");

        frame(&mut world, 0.0, Some(DIGITS[alone as usize - 1]));
        assert_eq!(sent(&world), Some(slot));
    }

    #[test]
    fn a_click_confirms_the_choice_and_is_not_a_shot() {
        let (mut world, bucket, inside) = a_chooser();
        frame(&mut world, 0.0, Some(DIGITS[bucket as usize - 1]));

        for held_down in [true, true, false] {
            let mut latch = world.resource_mut::<InputLatch>();
            latch.0.attack |= held_down;
            latch.0.attack_held = held_down;
            frame(&mut world, 0.01, None);
            let input = world.resource::<InputLatch>().0;
            assert!(!input.attack && !input.attack_held, "the confirming click fired");
        }
        assert_eq!(sent(&world), Some(inside[0]));
        world.query::<&mut Equipped>().single_mut(&mut world).unwrap().held = inside[0];
        frame(&mut world, 0.01, None);
        assert!(!world.resource::<WeaponSelection>().holds_the_buttons(), "never let go");

        world.resource_mut::<InputLatch>().0.attack = true;
        frame(&mut world, 0.01, None);
        assert!(world.resource::<InputLatch>().0.attack, "the next click was swallowed too");
    }

    #[test]
    fn a_right_click_cancels() {
        let (mut world, bucket, _) = a_chooser();
        let held = world.query::<&Equipped>().single(&world).unwrap().held;
        frame(&mut world, 0.0, Some(DIGITS[bucket as usize - 1]));

        world.resource_mut::<InputLatch>().0.alt_attack = true;
        frame(&mut world, 0.01, None);
        frame(&mut world, SETTLE_SECONDS * 2.0, None);

        assert_eq!(sent(&world), None, "a cancelled choice was sent");
        assert_eq!(highlight(&world), Some(held));
        assert!(!world.resource::<InputLatch>().0.alt_attack, "the cancelling click fired");
    }

    #[test]
    fn a_sent_switch_holds_the_buttons_until_it_is_seen_held() {
        let (mut world, _, _) = a_chooser();
        let catalogue = world.resource::<WeaponCatalogue>().clone();
        let equipped = *world.query::<&Equipped>().single(&world).unwrap();
        let order = carried(&equipped, &catalogue);
        let (alone, slot) = *order
            .iter()
            .find(|(at, _)| order.iter().filter(|(b, _)| b == at).count() == 1)
            .unwrap();

        frame(&mut world, 0.0, Some(DIGITS[alone as usize - 1]));
        frame(&mut world, 0.01, None);
        assert!(
            world.resource::<WeaponSelection>().holds_the_buttons(),
            "the weapon being put away could be guessed to fire"
        );

        world.query::<&mut Equipped>().single_mut(&mut world).unwrap().held = slot;
        frame(&mut world, 0.01, None);
        assert!(!world.resource::<WeaponSelection>().holds_the_buttons());
    }

    /// A choice made before a death is not sent to the next body.
    #[test]
    fn a_new_body_starts_from_nothing() {
        let (mut world, bucket, _) = a_chooser();
        frame(&mut world, 0.0, Some(DIGITS[bucket as usize - 1]));

        let old = world.query_filtered::<Entity, With<LocalPlayer>>().single(&world).unwrap();
        let equipped = *world.get::<Equipped>(old).unwrap();
        world.despawn(old);
        world.spawn((equipped, LocalPlayer));
        frame(&mut world, SETTLE_SECONDS * 2.0, None);

        assert_eq!(sent(&world), None, "the dead body's choice reached the new one");
        assert_eq!(highlight(&world), Some(equipped.held));
    }
}
