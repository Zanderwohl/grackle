//! Pulling a trigger, switching a slot, and paying for the shot.
//!
//! What a weapon *is* lives in [`crate::common::weapon`]; this is the layer
//! that turns a body's input into a thing happening. Three rules hold it
//! together, and each has a failure that announces itself as something else.
//!
//! **The trigger is taken once, in one place.** `attack` is a latch — one
//! press is one shot however many frames pass before a fixed step reads it —
//! and a latch is only correct while exactly one system consumes it. With one
//! weapon that was the weapon's own business; with three firing systems and
//! two buttons it is a race where whichever ran first ate the press.
//! [`pull_trigger`] takes it and writes a [`WeaponActionFired`] per shot.
//!
//! **The action travels by value.** [`pull_trigger`] is the only thing that
//! asks what a body is holding, and what it found is what everything
//! downstream answers for. That is the rule
//! [`launch`](crate::game::projectile::launch) already follows when it copies
//! a spec onto a projectile, so that switching weapons cannot reach back and
//! change a shot already in the air — and it means a firing system needs no
//! access to a body's loadout at all.
//!
//! **Ammo is spent here and nowhere else.** This is already the one place that
//! decides a shot happens, so it is the one place that can decline for want of
//! a round. A firing system that debited its own pool would be several writers
//! on one number, and a reserve that depends on which system ran first.
//!
//! One thing to expect and not be alarmed by: everything in this module
//! consumes mutable state inside `FixedUpdate`, which the rest of the fixed
//! loop is forbidden from doing. It is correct here because the whole plugin
//! is `run_if(has_authority)` and a rollback replays only a client's own
//! movement — nothing in here is ever replayed. Anything that stops being
//! authority-only stops being allowed to spend.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::net::has_authority;
use crate::common::weapon::{
    Ammo, Cadence, Cost, Equipped, Magazine, Mounted, WeaponAction, WeaponCatalogue,
};
use crate::game::damage::{DamagePlugin, DamageSystems};
use crate::game::explosion::ExplosionPlugin;
use crate::game::flame::FlamePlugin;
use crate::game::hitscan::HitscanPlugin;
use crate::game::player::{Inputs, Player};
use crate::game::projectile::ProjectilePlugin;

/// Which half of the weapon went off.
///
/// Not a [`Slot`]: a slot says which weapon is in your hands, a button says
/// which of its two things you asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Primary,
    Secondary,
}

impl Button {
    pub const BOTH: [Button; 2] = [Button::Primary, Button::Secondary];

    fn index(self) -> usize {
        self as usize
    }

    /// The half of a weapon this button is mounted on.
    fn of(self, weapon: &crate::common::weapon::Weapon) -> Mounted {
        match self {
            Button::Primary => weapon.primary,
            Button::Secondary => weapon.secondary,
        }
    }
}

/// A body's weapon did something, on this tick.
///
/// The action rides along **by value** — see the module docs. Every weapon
/// system reads this and answers for the actions it knows, which is what lets
/// a kind of weapon be added without anything already here being touched.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct WeaponActionFired {
    pub shooter: Entity,
    pub button: Button,
    pub action: WeaponAction,
}

/// How long each of this body's buttons has to wait before it may fire again.
///
/// **Two clocks, because there are two buttons.** One would let a flamethrower's
/// stream eat its own alt-fire's cooldown, and the symptom is a weapon that
/// stutters under a right-click, which reads as lag rather than as a bug.
///
/// Per button rather than per slot, and still not reset by a switch:
/// quick-switching to dodge a cooldown stays a decision to make deliberately
/// rather than one to fall into.
///
/// Counting *down* rather than up so that the default — zero — is a body that
/// may fire immediately; counting up would make a freshly spawned player wait
/// out an interval before their first shot and read as a misfire. It floors at
/// zero rather than running further negative, which quantises the rate of fire
/// to whole ticks: carrying the remainder would let a weapon that had been idle
/// empty several shots on consecutive ticks.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Trigger {
    ready_in: [f32; 2],
}

impl Trigger {
    fn tick(&mut self, dt: f32) {
        for remaining in &mut self.ready_in {
            *remaining = (*remaining - dt).max(0.0);
        }
    }

    fn ready(&self, button: Button) -> bool {
        self.ready_in[button.index()] <= 0.0
    }

    fn arm(&mut self, button: Button, interval: f32) {
        self.ready_in[button.index()] = interval;
    }
}

pub struct WeaponPlugin;

impl Plugin for WeaponPlugin {
    fn build(&self, app: &mut App) {
        app
            // The damage layer everything here writes into, and the two things
            // that read it. Brought in together because a weapon that dealt
            // damage nobody could see or be killed by is not a weapon.
            .add_plugins((DamagePlugin, ExplosionPlugin))
            .add_plugins((HitscanPlugin, ProjectilePlugin, FlamePlugin))
            .add_message::<WeaponActionFired>()
            // Ahead of every weapon, and ahead of the damage they deal. The
            // order inside is the order a player experiences: a switch lands
            // before the shot it was made for, a reload finishes before the
            // trigger asks whether there is a round.
            .add_systems(
                FixedUpdate,
                (select_weapons, reload_weapons, pull_trigger)
                    .chain()
                    .before(DamageSystems::Deal)
                    .run_if(in_state(AppMode::Play))
                    .run_if(has_authority),
            )
            .add_systems(
                Update,
                note_unimplemented_actions.run_if(in_state(AppMode::Play)),
            );
    }
}

/// Put a shot in front of every weapon, when the weapon in hand says there is
/// one and there is a round to pay for it.
///
/// Runs only where this process is believed. A client fires nothing locally;
/// what its trigger does is travel as an input and come back as a consequence.
///
/// Both halves of each trigger are read here: the latched *press*, which a
/// semi-automatic button answers, and the *held* state, which an automatic one
/// answers on its own interval. Doing it in one place is what stops the two
/// disagreeing about whether a press that was also a hold is one shot or two.
pub fn pull_trigger(
    time: Res<Time<Fixed>>,
    catalogue: Res<WeaponCatalogue>,
    mut shooters: Query<(Entity, &Equipped, &mut Ammo, &mut Trigger, &Inputs), With<Player>>,
    mut fired: MessageWriter<WeaponActionFired>,
) {
    let dt = time.delta_secs();

    for (shooter, equipped, mut ammo, mut trigger, input) in &mut shooters {
        trigger.tick(dt);

        let held = equipped.held;
        let Some(weapon) = catalogue.held_by(equipped) else { continue };

        // A copy, written back only if it actually changed. `Ammo` is
        // replicated and a `Mut` counts as changed the moment it is
        // dereferenced, so touching it every tick would put every body's ammo
        // on the wire sixty-four times a second for no reason.
        let mut state = *ammo.at(held);

        for button in Button::BOTH {
            // Read, never taken: this tick's input has to survive being read.
            // The edge was already consumed when the tick's input was written —
            // see `write_client_inputs`.
            let (pressed, down) = match button {
                Button::Primary => (input.attack, input.attack_held),
                Button::Secondary => (input.alt_attack, input.alt_attack_held),
            };

            let mounted = button.of(weapon);
            if matches!(mounted.action, WeaponAction::None) {
                continue;
            }

            let asked = match mounted.cadence {
                // No cooldown: as fast as you can click. The seam for a real
                // rate of fire is here, and it is the same array.
                Cadence::Semi => pressed,
                // `pressed` as well as `down`, because a click made and
                // released between two ticks was never held on any tick that
                // ran, and a flamethrower that ignored a tap would feel broken.
                Cadence::Automatic { .. } => (down || pressed) && trigger.ready(button),
            };
            if !asked {
                continue;
            }

            // A shot that cannot be paid for writes no message *and* arms no
            // cooldown, so an empty weapon is silent and instantly ready
            // rather than dry-firing on a timer. Starting a reload is not this
            // system's business — `reload_weapons` owns that, so that spending
            // and refilling have one writer each.
            if !spend(&mut state, &weapon.magazine, mounted.cost) {
                continue;
            }

            if let Cadence::Automatic { interval } = mounted.cadence {
                trigger.arm(button, interval);
            }

            fired.write(WeaponActionFired { shooter, button, action: mounted.action });
        }

        if state != *ammo.at(held) {
            *ammo.at_mut(held) = state;
        }
    }
}

/// Take what a pull costs, or say it cannot be taken.
///
/// The one place the clip-versus-reserve question is answered, so that a
/// weapon with no clip is a weapon that spends its reserve rather than a
/// weapon with a clip the size of its reserve — which is a lie the reload
/// system would then have to keep believing.
fn spend(
    state: &mut crate::common::weapon::AmmoState,
    magazine: &Magazine,
    cost: Cost,
) -> bool {
    match cost {
        Cost::Free => true,
        Cost::Ammo { rounds } => match magazine.clip {
            Some(_) => {
                if state.clip < rounds {
                    return false;
                }
                state.clip -= rounds;
                true
            }
            None => {
                if state.reserve < rounds {
                    return false;
                }
                state.reserve -= rounds;
                true
            }
        },
        Cost::Charge { fraction } => {
            if state.charge < fraction {
                return false;
            }
            state.charge -= fraction;
            true
        }
    }
}

/// Start a reload, if there is one to start.
///
/// Declines on a weapon with no clip — a minigun draws from its reserve and
/// has nothing to fill — on a clip that is already full, and on an empty
/// reserve. Declining rather than starting a reload that cannot finish is what
/// keeps `reloading` meaning "a reload is running".
fn begin_reload(state: &mut crate::common::weapon::AmmoState, magazine: &Magazine) {
    let Some(size) = magazine.clip else { return };
    if state.reloading > 0.0 || state.clip >= size || state.reserve == 0 {
        return;
    }
    state.reloading = magazine.reload;
}

/// Start reloads, run them down, and move the rounds across.
///
/// **The only thing that starts a reload**, asked for or not, so that spending
/// a round and replacing one have one writer each. Ordered before
/// [`pull_trigger`] so that the reload finishing on this tick is available to
/// the shot asked for on it.
///
/// An empty clip reloads without being asked, because a weapon that needed a
/// keypress to become useful again reads as broken before it reads as empty.
/// A clip that is merely short does not: topping up after every shot would
/// make the magazine size mean nothing.
pub fn reload_weapons(
    time: Res<Time<Fixed>>,
    catalogue: Res<WeaponCatalogue>,
    mut carriers: Query<(&Equipped, &mut Ammo, &Inputs), With<Player>>,
) {
    let dt = time.delta_secs();

    for (equipped, mut ammo, input) in &mut carriers {
        let held = equipped.held;
        let Some(weapon) = catalogue.held_by(equipped) else { continue };
        let Some(size) = weapon.magazine.clip else { continue };

        let mut state = *ammo.at(held);

        if input.reload || state.clip == 0 {
            begin_reload(&mut state, &weapon.magazine);
        }

        if state.reloading > 0.0 {
            state.reloading = (state.reloading - dt).max(0.0);

            if state.reloading <= 0.0 {
                // Clamped against both the room in the clip and what is left
                // in the reserve — the second is where a `u32` would otherwise
                // wrap round and hand somebody four billion rockets.
                let wanted = if weapon.magazine.reload_one_at_a_time {
                    1
                } else {
                    size.saturating_sub(state.clip)
                };
                let moved = wanted.min(state.reserve);
                state.clip += moved;
                state.reserve -= moved;

                // A shotgun goes round again; a rocket launcher is done. That
                // one difference is the whole of `reload_one_at_a_time`.
                if weapon.magazine.reload_one_at_a_time {
                    begin_reload(&mut state, &weapon.magazine);
                }
            }
        }

        if state != *ammo.at(held) {
            *ammo.at_mut(held) = state;
        }
    }
}

/// Act on a switch made since the last step.
pub fn select_weapons(mut carriers: Query<(&mut Equipped, &mut Ammo, &Inputs), With<Player>>) {
    for (mut equipped, mut ammo, input) in &mut carriers {
        let Some(slot) = input.select else { continue };
        if equipped.held == slot || equipped.get(slot).is_none() {
            continue;
        }

        // A reload in progress does not survive being put in a holster. The
        // alternative is a clip that fills while the weapon is not in anybody's
        // hands, which is a weapon reloading with nobody holding it.
        let was = equipped.held;
        if ammo.at(was).reloading > 0.0 {
            ammo.at_mut(was).reloading = 0.0;
        }

        // Compared above rather than written blindly: `Equipped` is
        // replicated, so a write is a packet, and selecting the slot already
        // held is not a switch. Idempotence is also what lets the input be
        // read rather than taken — a replayed tick asking for it again reaches
        // the same loadout.
        equipped.select(slot);
    }
}

/// Say, once in a while, that a button did nothing because nothing answers for
/// it yet.
///
/// A log line rather than silence and rather than a `todo!()`. Silence is a
/// weapon that reads as jammed; a panic is a listen server dropping everybody
/// because somebody right-clicked. Rate-limited because an automatic stub would
/// otherwise say so ten times a second.
fn note_unimplemented_actions(
    time: Res<Time>,
    mut fired: MessageReader<WeaponActionFired>,
    mut quiet_until: Local<f32>,
) {
    /// Seconds between complaints.
    const SAY_EVERY: f32 = 1.0;

    let now = time.elapsed_secs();
    for shot in fired.read() {
        if shot.action.is_implemented() || now < *quiet_until {
            continue;
        }
        *quiet_until = now + SAY_EVERY;
        info!("{:?} is not implemented yet; the button did nothing", shot.action);
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::weapon::{HitscanSpec, Magazine, Slot, Weapon, WeaponId, SLOTS};
    use crate::common::weapon_file::default_catalogue;

    /// One fixed step at Bevy's 64 Hz default.
    const STEP: std::time::Duration = std::time::Duration::from_micros(15625);

    const LASER: HitscanSpec = HitscanSpec {
        range: 100.0,
        damage: 30,
        headshot_multiplier: 3,
        shove_per_damage: 0.15,
        shove_radius: 0.4,
    };

    fn a_world(catalogue: WeaponCatalogue) -> World {
        let mut world = World::new();
        world.init_resource::<Messages<WeaponActionFired>>();
        world.insert_resource(catalogue);

        let mut fixed = Time::<Fixed>::default();
        fixed.advance_by(STEP);
        world.insert_resource(fixed);
        world
    }

    /// A catalogue of exactly one weapon, in the primary slot of one class.
    fn one_weapon(weapon: Weapon) -> WeaponCatalogue {
        let id = weapon.id;
        let mut catalogue = WeaponCatalogue::default();
        catalogue.insert(weapon);

        let mut loadout = crate::common::weapon::Loadout::default();
        loadout.set(
            Slot::Primary,
            crate::common::weapon::SlotChoices { permitted: vec![id], default: id },
        );
        catalogue.set_loadout(crate::common::class::Class::Mercenary, loadout);
        catalogue
    }

    fn weapon(primary: Mounted, secondary: Mounted, magazine: Magazine) -> Weapon {
        Weapon {
            id: WeaponId::of("test"),
            name_key: "weapon.names.test".into(),
            primary,
            secondary,
            magazine,
        }
    }

    fn semi(action: WeaponAction, rounds: u32) -> Mounted {
        Mounted { action, cadence: Cadence::Semi, cost: Cost::Ammo { rounds } }
    }

    fn auto(action: WeaponAction, interval: f32, rounds: u32) -> Mounted {
        Mounted {
            action,
            cadence: Cadence::Automatic { interval },
            cost: Cost::Ammo { rounds },
        }
    }

    /// A body carrying whatever the catalogue says its class carries.
    fn a_shooter(world: &mut World) -> Entity {
        let catalogue = world.resource::<WeaponCatalogue>().clone();
        let (equipped, ammo) =
            catalogue.starting_equipment(crate::common::class::Class::Mercenary);
        world.spawn((Player::default(), equipped, ammo, Trigger::default())).id()
    }

    fn input(world: &mut World, body: Entity) -> Mut<'_, Inputs> {
        world.get_mut::<Inputs>(body).expect("body has no input")
    }

    fn ammo(world: &World, body: Entity, slot: Slot) -> crate::common::weapon::AmmoState {
        *world.get::<Ammo>(body).expect("body has no ammo").at(slot)
    }

    fn shots(world: &mut World) -> Vec<WeaponActionFired> {
        let messages = world.resource::<Messages<WeaponActionFired>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    fn step(world: &mut World) {
        world.resource_mut::<Messages<WeaponActionFired>>().clear();
        world.run_system_once(reload_weapons).unwrap();
        world.run_system_once(pull_trigger).unwrap();
    }

    /// One press is one shot for a semi-automatic weapon: the latch is taken
    /// by the tick that wrote it, and holding the button changes nothing.
    #[test]
    fn a_press_is_announced_once() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted::EMPTY,
            Magazine { clip: Some(12), reserve: 12, reload: 1.0, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack = true;
        input(&mut world, shooter).attack_held = true;

        step(&mut world);
        let fired = shots(&mut world);
        assert_eq!(fired.len(), 1, "{fired:?}");
        assert_eq!(fired[0].shooter, shooter);
        assert_eq!(fired[0].button, Button::Primary);

        // The next tick's input: still held, no longer a fresh press, because
        // the edge was consumed when that tick's input was written. Reading it
        // here would fire again, which is the bug this pins.
        input(&mut world, shooter).attack = false;
        step(&mut world);
        assert!(shots(&mut world).is_empty(), "holding the button fired a second time");
    }

    /// An automatic weapon keeps firing while held, on its own interval and no
    /// faster — the rate of fire is the weapon's, not the tick rate's.
    #[test]
    fn an_automatic_weapon_fires_on_its_interval_while_held() {
        let mut world = a_world(one_weapon(weapon(
            auto(WeaponAction::Hitscan(LASER), 0.1, 0),
            Mounted::EMPTY,
            Magazine::BOTTOMLESS,
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack_held = true;

        // One second of holding it down, in 64 Hz steps.
        let mut fired = 0;
        for _ in 0..64 {
            step(&mut world);
            fired += shots(&mut world).len();
        }

        // Ten, quantised to whole ticks: a tenth of a second is seven ticks at
        // 64 Hz, so the exact count depends on where the second is cut.
        assert!((9..=11).contains(&fired), "a second of holding it down fired {fired} times");
    }

    /// And it stops when the button comes up, rather than running on.
    #[test]
    fn an_automatic_weapon_stops_when_the_trigger_is_released() {
        let mut world = a_world(one_weapon(weapon(
            auto(WeaponAction::Hitscan(LASER), 0.1, 0),
            Mounted::EMPTY,
            Magazine::BOTTOMLESS,
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack_held = false;

        for _ in 0..64 {
            step(&mut world);
            assert!(shots(&mut world).is_empty(), "it fired with nobody holding it");
        }
    }

    /// A tap made and released between two ticks was never held on any tick
    /// that ran, and a flamethrower that ignored it would feel broken.
    #[test]
    fn a_tap_too_short_to_be_held_still_fires_an_automatic_weapon() {
        let mut world = a_world(one_weapon(weapon(
            auto(WeaponAction::Hitscan(LASER), 0.1, 0),
            Mounted::EMPTY,
            Magazine::BOTTOMLESS,
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack = true;

        step(&mut world);
        assert_eq!(shots(&mut world).len(), 1);
    }

    /// The reason the input lives on the body: two bodies in one world are
    /// asked for different things in the same tick. With a single global
    /// input, whoever wrote it last fired everybody's weapon.
    #[test]
    fn one_body_firing_does_not_fire_the_others() {
        let mut world = a_world(default_catalogue());
        let shooting = a_shooter(&mut world);
        let idle = a_shooter(&mut world);
        input(&mut world, shooting).attack = true;

        step(&mut world);

        let fired = shots(&mut world);
        assert_eq!(fired.len(), 1, "{fired:?}");
        assert_eq!(fired[0].shooter, shooting);
        assert!(!input(&mut world, idle).attack, "the idle body's latch was taken too");
    }

    /// A switch travels through the input like everything else, and acting on
    /// it twice is the same as acting on it once — which is what lets it be
    /// read rather than taken, so a replayed tick reaches the same loadout.
    #[test]
    fn acting_on_a_switch_twice_is_the_same_as_once() {
        let mut world = a_world(default_catalogue());
        let body = a_shooter(&mut world);
        input(&mut world, body).select = Some(Slot::Secondary);

        world.run_system_once(select_weapons).unwrap();
        world.run_system_once(select_weapons).unwrap();

        assert_eq!(world.get::<Equipped>(body).unwrap().held, Slot::Secondary);
        assert_eq!(
            input(&mut world, body).select,
            Some(Slot::Secondary),
            "the tick's input was emptied by the system that read it"
        );
    }

    /// A slot the class does not carry is not a slot: pressing `6` on a class
    /// with one weapon leaves you holding what you had.
    ///
    /// Against a class that really does leave slots empty, rather than against
    /// the placeholder — which fills all six, so it cannot say anything about
    /// this and would pass by accident if the guard were removed.
    #[test]
    fn selecting_an_empty_slot_changes_nothing() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted::EMPTY,
            Magazine::BOTTOMLESS,
        )));
        let body = a_shooter(&mut world);
        let held = world.get::<Equipped>(body).unwrap().held;
        assert_eq!(held, Slot::Primary, "the fixture stopped leaving slots empty");

        input(&mut world, body).select = Some(Slot::Special);
        world.run_system_once(select_weapons).unwrap();

        assert_eq!(world.get::<Equipped>(body).unwrap().held, held, "an empty slot was equipped");
    }

    /// Two buttons are two clocks. One would let an alt-fire's cooldown eat
    /// the stream the primary is meant to be putting out, which reads as lag
    /// rather than as a bug.
    #[test]
    fn each_button_has_its_own_cooldown() {
        let mut world = a_world(one_weapon(weapon(
            auto(WeaponAction::Hitscan(LASER), 0.1, 0),
            auto(WeaponAction::Flame(crate::common::flame::FlameSpec::FLAMETHROWER), 0.5, 0),
            Magazine::BOTTOMLESS,
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack_held = true;
        input(&mut world, shooter).alt_attack_held = true;

        let (mut primary, mut secondary) = (0, 0);
        for _ in 0..64 {
            step(&mut world);
            for shot in shots(&mut world) {
                match shot.button {
                    Button::Primary => primary += 1,
                    Button::Secondary => secondary += 1,
                }
            }
        }

        assert!((9..=11).contains(&primary), "the primary fired {primary} times in a second");
        assert!((1..=3).contains(&secondary), "the secondary fired {secondary} times in a second");
    }

    /// What was fired is what the message says, not what the body is holding
    /// by the time somebody reads it. This is the rule `launch` already
    /// follows when it copies a spec onto a projectile.
    #[test]
    fn the_action_that_was_fired_is_the_one_the_message_carries() {
        let mut world = a_world(default_catalogue());
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack = true;

        step(&mut world);
        let fired = shots(&mut world);
        assert_eq!(fired.len(), 1, "{fired:?}");

        // Switch after the trigger was pulled, exactly as a tick with a switch
        // and a click in it would.
        world.get_mut::<Equipped>(shooter).unwrap().select(Slot::Secondary);

        assert!(
            matches!(fired[0].action, WeaponAction::Hitscan(_)),
            "the shot changed when the weapon did: {:?}",
            fired[0].action
        );
    }

    /// A button with nothing on it writes nothing and arms nothing, which is
    /// what makes `WeaponAction::None` a statement rather than an oversight.
    #[test]
    fn an_unbound_button_fires_nothing() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 0),
            Mounted::EMPTY,
            Magazine::BOTTOMLESS,
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).alt_attack = true;
        input(&mut world, shooter).alt_attack_held = true;

        step(&mut world);
        assert!(shots(&mut world).is_empty(), "an unbound button fired");
    }

    /// An empty weapon is silent *and* instantly ready. Arming the cooldown on
    /// a shot that never happened would be a dry-fire on a timer.
    #[test]
    fn an_empty_weapon_fires_nothing_and_arms_nothing() {
        let mut world = a_world(one_weapon(weapon(
            auto(WeaponAction::Hitscan(LASER), 0.1, 1),
            Mounted::EMPTY,
            // A clip of one and no reserve: one shot, and then nothing ever.
            Magazine { clip: Some(1), reserve: 0, reload: 1.0, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack_held = true;

        step(&mut world);
        assert_eq!(shots(&mut world).len(), 1, "the one round it had did not fire");

        // Long enough for the cooldown that shot *did* arm to run out, so what
        // is asserted below is about the shot that could not be paid for and
        // not about the one that could.
        for _ in 0..16 {
            step(&mut world);
            assert!(shots(&mut world).is_empty(), "it fired a round it did not have");
        }
        assert!(
            world.get::<Trigger>(shooter).unwrap().ready(Button::Primary),
            "an empty weapon armed its cooldown"
        );
    }

    /// The minigun case: no clip, so a shot comes straight out of the reserve.
    #[test]
    fn a_weapon_with_no_clip_spends_its_reserve() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted::EMPTY,
            Magazine { clip: None, reserve: 10, reload: 0.0, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack = true;

        step(&mut world);

        let state = ammo(&world, shooter, Slot::Primary);
        assert_eq!(state.reserve, 9, "a clipless weapon did not spend its reserve");
        assert_eq!(state.clip, 0, "a clipless weapon loaded a clip it does not have");
    }

    /// And it never reloads, rather than topping up a clip the size of its
    /// reserve on every tick.
    #[test]
    fn a_weapon_with_no_clip_never_reloads() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted::EMPTY,
            Magazine { clip: None, reserve: 10, reload: 0.0, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).reload = true;

        for _ in 0..8 {
            step(&mut world);
        }

        let state = ammo(&world, shooter, Slot::Primary);
        assert_eq!(state.reserve, 10);
        assert_eq!(state.clip, 0);
        assert_eq!(state.reloading, 0.0, "a weapon with no clip started a reload");
    }

    /// A free action costs nothing, so a scope toggle works on an empty gun.
    #[test]
    fn a_free_action_costs_nothing() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted {
                action: WeaponAction::Scope(crate::common::weapon::ScopeSpec { zoom: 2.0 }),
                cadence: Cadence::Semi,
                cost: Cost::Free,
            },
            Magazine { clip: Some(0), reserve: 0, reload: 1.0, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack = true;
        input(&mut world, shooter).alt_attack = true;

        step(&mut world);

        let fired = shots(&mut world);
        assert_eq!(fired.len(), 1, "{fired:?}");
        assert_eq!(fired[0].button, Button::Secondary, "the empty primary fired");
    }

    /// A reload moves rounds across, and cannot move more than there are.
    ///
    /// The clamp is where an unsigned subtraction would otherwise wrap round
    /// and hand somebody four billion rockets.
    #[test]
    fn a_reload_moves_rounds_from_the_reserve_to_the_clip() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted::EMPTY,
            Magazine { clip: Some(4), reserve: 8, reload: 0.1, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);

        // Two rounds gone, then a reload asked for.
        for _ in 0..2 {
            input(&mut world, shooter).attack = true;
            step(&mut world);
            input(&mut world, shooter).attack = false;
        }
        assert_eq!(ammo(&world, shooter, Slot::Primary).clip, 2);

        input(&mut world, shooter).reload = true;
        step(&mut world);
        input(&mut world, shooter).reload = false;
        for _ in 0..8 {
            step(&mut world);
        }

        let state = ammo(&world, shooter, Slot::Primary);
        assert_eq!(state.clip, 4, "the clip did not fill");
        assert_eq!(state.reserve, 6, "the reserve did not pay for it");
    }

    #[test]
    fn a_reload_cannot_take_more_than_the_reserve_holds() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted::EMPTY,
            Magazine { clip: Some(4), reserve: 1, reload: 0.1, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);

        for _ in 0..4 {
            input(&mut world, shooter).attack = true;
            step(&mut world);
            input(&mut world, shooter).attack = false;
        }

        // Empty, so a reload starts by itself; let it finish.
        for _ in 0..16 {
            step(&mut world);
        }

        let state = ammo(&world, shooter, Slot::Primary);
        assert_eq!(state.clip, 1, "the clip took more than the reserve had");
        assert_eq!(state.reserve, 0);
    }

    /// A weapon that needed a keypress to become useful again would read as
    /// broken before it read as empty.
    #[test]
    fn an_empty_clip_reloads_without_being_asked() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted::EMPTY,
            Magazine { clip: Some(1), reserve: 4, reload: 0.1, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);

        // The round it had, then a click on an empty clip.
        input(&mut world, shooter).attack = true;
        step(&mut world);
        step(&mut world);

        assert!(
            ammo(&world, shooter, Slot::Primary).reloading > 0.0,
            "an empty weapon sat there rather than reloading"
        );
    }

    /// A reload does not survive being put in a holster: a clip that filled
    /// while nobody was holding the weapon would be a reload with no reloader.
    #[test]
    fn switching_weapons_cancels_a_reload() {
        let mut world = a_world(default_catalogue());
        let shooter = a_shooter(&mut world);
        let held = world.get::<Equipped>(shooter).unwrap().held;

        world.get_mut::<Ammo>(shooter).unwrap().at_mut(held).reloading = 5.0;

        input(&mut world, shooter).select = Some(Slot::Secondary);
        world.run_system_once(select_weapons).unwrap();

        assert_eq!(
            ammo(&world, shooter, held).reloading,
            0.0,
            "the holstered weapon went on reloading"
        );
    }

    /// Every slot is reachable, so growing the array is the only thing that
    /// ever has to change to add one.
    #[test]
    fn every_slot_has_a_home() {
        assert_eq!(Slot::ALL.len(), SLOTS);
        for (index, slot) in Slot::ALL.into_iter().enumerate() {
            assert_eq!(slot.index(), index);
            assert_eq!(Slot::from_index(index), Some(slot));
        }
        assert_eq!(Slot::from_index(SLOTS), None, "a digit past the last slot found one");
    }
}
