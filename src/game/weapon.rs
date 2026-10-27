//! Pulling a trigger, switching a slot, and paying for the shot. What a weapon
//! is lives in [`crate::common::weapon`].
//!
//! [`pull_trigger`] is the only reader of what a body holds, the only place
//! ammo is spent, and writes the action by value in [`WeaponActionFired`], so
//! a switch in the same tick cannot change a shot and firing systems need no
//! loadout access.
//!
//! These systems mutate state inside `FixedUpdate`, which the rest of the
//! fixed loop must not. It is safe only because the plugin is authority-only
//! and rollback replays only a client's own movement.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::net::has_authority;
use crate::common::weapon::{
    Ammo, Cadence, Cost, Equipped, Magazine, Mounted, Slot, WeaponAction, WeaponCatalog,
};
use crate::game::condition::Condition;
use crate::game::damage::{DamagePlugin, DamageSystems};
use crate::game::explosion::ExplosionPlugin;
use crate::game::flame::FlamePlugin;
use crate::game::hitscan::HitscanPlugin;
use crate::game::player::{Inputs, Player};
use crate::game::projectile::ProjectilePlugin;

/// Which half of the weapon went off. Not a [`Slot`]: a slot picks the weapon,
/// a button picks which of its two actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Button {
    Primary,
    Secondary,
}

impl Button {
    pub const BOTH: [Button; 2] = [Button::Primary, Button::Secondary];

    fn index(self) -> usize {
        self as usize
    }

    pub(crate) fn of(self, weapon: &crate::common::weapon::Weapon) -> Mounted {
        match self {
            Button::Primary => weapon.primary,
            Button::Secondary => weapon.secondary,
        }
    }
}

/// A body's weapon did something, on this tick. Each weapon system answers for
/// the actions it knows.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct WeaponActionFired {
    pub shooter: Entity,
    pub button: Button,
    pub action: WeaponAction,
}

/// Seconds until each button may fire again.
///
/// One clock per button, or an alt-fire's cooldown eats the primary's stream.
/// Not reset by a switch, so quick-switching past a cooldown is not free.
///
/// Counts down so the default, zero, may fire at once. Floors at zero, which
/// quantizes the rate to whole ticks: carrying the remainder would let an idle
/// weapon fire on several consecutive ticks.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Trigger {
    ready_in: [f32; 2],
    /// Seconds until the weapon just drawn may fire or reload.
    deploying: f32,
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

    pub fn deployed(&self) -> bool {
        self.deploying <= 0.0
    }
}

pub struct WeaponPlugin;

impl Plugin for WeaponPlugin {
    fn build(&self, app: &mut App) {
        app
            // Damage plus the two things that read it.
            .add_plugins((DamagePlugin, ExplosionPlugin))
            .add_plugins((HitscanPlugin, ProjectilePlugin, FlamePlugin))
            .add_message::<WeaponActionFired>()
            // Before every weapon and its damage. Switch, then reload, then
            // fire, so a reload finishing this tick feeds this tick's shot.
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

/// Fire every body's held weapon where its cadence asks and it can pay.
/// Reads both the latched press and the held state in one place, so a press
/// that was also a hold is one shot, not two.
pub fn pull_trigger(
    time: Res<Time<Fixed>>,
    catalog: Res<WeaponCatalog>,
    mut shooters: Query<
        (Entity, &Equipped, &mut Ammo, &mut Trigger, &Inputs, &Condition),
        With<Player>,
    >,
    mut fired: MessageWriter<WeaponActionFired>,
) {
    let dt = time.delta_secs();

    for (shooter, equipped, mut ammo, mut trigger, input, condition) in &mut shooters {
        trigger.tick(dt);
        // Dropped, not queued: a click while drawing is not a shot after it.
        if !condition.may_fire() || !trigger.deployed() {
            continue;
        }

        let held = equipped.held;
        let Some(weapon) = catalog.held_by(equipped) else { continue };

        // A copy, written back only on change: `Ammo` is replicated and a `Mut`
        // deref marks it changed, which would send it 64 times a second.
        let mut state = *ammo.at(held);

        for button in Button::BOTH {
            // Read, never taken: the edge was consumed in `write_client_inputs`
            // and rollback may read this input again.
            let (pressed, down) = match button {
                Button::Primary => (input.attack, input.attack_held),
                Button::Secondary => (input.alt_attack, input.alt_attack_held),
            };

            let mounted = button.of(weapon);
            if matches!(mounted.action, WeaponAction::None) {
                continue;
            }

            let asked = match mounted.cadence {
                // No cooldown: as fast as you can click.
                Cadence::Semi => pressed,
                // `pressed` too: a tap released between ticks was never held on
                // a tick that ran.
                Cadence::Automatic { .. } => (down || pressed) && trigger.ready(button),
            };
            if !asked {
                continue;
            }

            // Unpaid: no message and no cooldown, so an empty weapon is silent
            // and instantly ready. `reload_weapons` starts reloads, not this.
            if !spend(&mut state, &weapon.magazine, mounted.cost) {
                continue;
            }
            // A shot ends a reload, TF2-style: firing with a round left is
            // choosing to fire rather than to reload. Here rather than in
            // `reload_weapons` because this is the one place a shot happens.
            state.reloading = 0.0;

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

/// Take what a pull costs, or return false. A weapon with no clip spends its
/// reserve directly.
pub(crate) fn spend(
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

/// Start a reload unless there is no clip, the clip is full, or the reserve is
/// empty, so `reloading > 0` always means one that can finish.
fn begin_reload(state: &mut crate::common::weapon::AmmoState, magazine: &Magazine) {
    let Some(size) = magazine.clip else { return };
    if state.reloading > 0.0 || state.clip >= size || state.reserve == 0 {
        return;
    }
    state.reloading = magazine.reload;
}

/// Start reloads, run them down, and move the rounds across. The only thing
/// that starts a reload. An empty clip reloads unasked; a short one does not,
/// or the magazine size would mean nothing.
pub fn reload_weapons(
    time: Res<Time<Fixed>>,
    catalog: Res<WeaponCatalog>,
    mut carriers: Query<(&Equipped, &mut Ammo, &Trigger, &Inputs), With<Player>>,
) {
    let dt = time.delta_secs();

    for (equipped, mut ammo, trigger, input) in &mut carriers {
        if !trigger.deployed() {
            continue;
        }
        let held = equipped.held;
        let Some(weapon) = catalog.held_by(equipped) else { continue };
        let Some(size) = weapon.magazine.clip else { continue };

        let mut state = *ammo.at(held);

        if input.reload || state.clip == 0 {
            begin_reload(&mut state, &weapon.magazine);
        }

        if state.reloading > 0.0 {
            state.reloading = (state.reloading - dt).max(0.0);

            if state.reloading <= 0.0 {
                // Clamped to the reserve too, or the `u32` subtraction below
                // wraps.
                let wanted = if weapon.magazine.reload_one_at_a_time {
                    1
                } else {
                    size.saturating_sub(state.clip)
                };
                let moved = wanted.min(state.reserve);
                state.clip += moved;
                state.reserve -= moved;

                // `reload_one_at_a_time` goes round again until full.
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
pub fn select_weapons(
    time: Res<Time<Fixed>>,
    catalog: Res<WeaponCatalog>,
    mut carriers: Query<(&mut Equipped, &mut Ammo, &mut Trigger, &Inputs), With<Player>>,
) {
    let dt = time.delta_secs();

    for (mut equipped, mut ammo, mut trigger, input) in &mut carriers {
        trigger.deploying = (trigger.deploying - dt).max(0.0);

        let Some(slot) = input.select else { continue };
        if equipped.held == slot || equipped.get(slot).is_none() {
            continue;
        }

        // Guarded above rather than written blindly: `Equipped` is
        // replicated, so a write is a packet. Idempotence also lets a replayed
        // tick read the same input and reach the same loadout.
        draw(slot, &mut equipped, &mut ammo, &mut trigger, &catalog);
    }
}

/// Take up what is in `slot` and wait out its deploy, as a switch does. An
/// empty slot is ignored.
pub fn draw(slot: Slot, equipped: &mut Equipped, ammo: &mut Ammo, trigger: &mut Trigger, catalog: &WeaponCatalog) {
    if equipped.get(slot).is_none() {
        return;
    }
    // A reload does not continue in a holster.
    let was = equipped.held;
    if ammo.at(was).reloading > 0.0 {
        ammo.at_mut(was).reloading = 0.0;
    }
    equipped.select(slot);
    // A weapon nobody can name yet draws instantly rather than never.
    trigger.deploying = catalog.held_by(equipped).map_or(0.0, |weapon| weapon.deploy);
}

/// Log, at most once a second, that a button's action is not implemented.
/// Silence reads as jammed, and a `todo!()` would drop a listen server.
fn note_unimplemented_actions(
    time: Res<Time>,
    mut fired: MessageReader<WeaponActionFired>,
    mut quiet_until: Local<f32>,
) {
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
    use crate::common::weapon::{HitscanSpec, Magazine, Weapon, WeaponId, SLOTS};
    use crate::common::weapon_file::default_catalog;

    /// Bevy's default 64 Hz.

    const LASER: HitscanSpec = HitscanSpec {
        range_m: 100.0,
        damage: 30,
        headshot_multiplier: 3,
        shove_per_damage: 0.15,
        shove_radius_m: 0.4,
    };

    fn a_world(catalog: WeaponCatalog) -> World {
        let mut world = World::new();
        world.init_resource::<Messages<WeaponActionFired>>();
        world.insert_resource(catalog);

        let mut fixed = Time::<Fixed>::default();
        fixed.advance_by(crate::constants::one_tick());
        world.insert_resource(fixed);
        world
    }

    /// One weapon, in the Mercenary's primary slot.
    fn one_weapon(weapon: Weapon) -> WeaponCatalog {
        let id = weapon.id;
        let mut catalog = WeaponCatalog::default();
        catalog.insert(weapon);

        let mut loadout = crate::common::weapon::Loadout::default();
        loadout.set(
            Slot::Primary,
            crate::common::weapon::SlotChoices { permitted: vec![id], default: id },
        );
        catalog.set_loadout(crate::common::class::Class::Mercenary, loadout);
        catalog
    }

    fn weapon(primary: Mounted, secondary: Mounted, magazine: Magazine) -> Weapon {
        Weapon {
            key: String::new(),
            id: WeaponId::of("test"),
            name_key: "weapon.names.test".into(),
            model: None,
            bucket: 1,
            deploy: 0.0,
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

    fn a_shooter(world: &mut World) -> Entity {
        let catalog = world.resource::<WeaponCatalog>().clone();
        let (equipped, ammo) =
            catalog.starting_equipment(crate::common::class::Class::Mercenary, None);
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

    /// Holding a semi-automatic button after the press fires once.
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

        // Next tick: still held, no fresh press.
        input(&mut world, shooter).attack = false;
        step(&mut world);
        assert!(shots(&mut world).is_empty(), "holding the button fired a second time");
    }

    /// Firing with a round in the clip cancels the reload under way, and the
    /// rounds it had not yet moved stay in the reserve.
    #[test]
    fn a_shot_interrupts_a_reload() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted::EMPTY,
            Magazine { clip: Some(4), reserve: 8, reload: 1.0, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack = true;
        step(&mut world);
        input(&mut world, shooter).attack = false;
        input(&mut world, shooter).reload = true;
        step(&mut world);
        input(&mut world, shooter).reload = false;
        assert!(ammo(&world, shooter, Slot::Primary).reloading > 0.0, "the reload never started");

        input(&mut world, shooter).attack = true;
        step(&mut world);
        assert_eq!(shots(&mut world).len(), 1, "the shot did not go off");
        let state = ammo(&world, shooter, Slot::Primary);
        assert_eq!(state.reloading, 0.0, "the reload carried on after the shot");
        assert_eq!((state.clip, state.reserve), (2, 8), "rounds moved for a reload that was canceled");
    }

    /// A humiliated body's press goes nowhere, and costs nothing.
    #[test]
    fn a_humiliated_body_does_not_fire() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted::EMPTY,
            Magazine { clip: Some(12), reserve: 12, reload: 1.0, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);
        *world.get_mut::<Condition>(shooter).unwrap() = Condition::HUMILIATED;
        input(&mut world, shooter).attack = true;

        step(&mut world);
        assert!(shots(&mut world).is_empty(), "a humiliated body fired");
        assert_eq!(ammo(&world, shooter, Slot::Primary).clip, 12, "a refused shot was paid for");
    }

    /// An automatic weapon fires on its own interval while held, not per tick.
    #[test]
    fn an_automatic_weapon_fires_on_its_interval_while_held() {
        let mut world = a_world(one_weapon(weapon(
            auto(WeaponAction::Hitscan(LASER), 0.1, 0),
            Mounted::EMPTY,
            Magazine::BOTTOMLESS,
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack_held = true;

        let mut fired = 0;
        for _ in 0..64 {
            step(&mut world);
            fired += shots(&mut world).len();
        }

        // A tenth of a second rounds up to seven 64 Hz ticks, so the count
        // depends on where the second is cut.
        assert!((9..=11).contains(&fired), "a second of holding it down fired {fired} times");
    }

    /// It stops when the button comes up.
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

    /// A tap released between two ticks still fires an automatic weapon.
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

    /// Input lives on the body: one body firing does not fire the others.
    #[test]
    fn one_body_firing_does_not_fire_the_others() {
        let mut world = a_world(default_catalog());
        let shooting = a_shooter(&mut world);
        let idle = a_shooter(&mut world);
        input(&mut world, shooting).attack = true;

        step(&mut world);

        let fired = shots(&mut world);
        assert_eq!(fired.len(), 1, "{fired:?}");
        assert_eq!(fired[0].shooter, shooting);
        assert!(!input(&mut world, idle).attack, "the idle body's latch was taken too");
    }

    /// A replayed tick reading the same switch reaches the same loadout.
    #[test]
    fn acting_on_a_switch_twice_is_the_same_as_once() {
        let mut world = a_world(default_catalog());
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

    /// Selecting a slot the class leaves empty keeps the current weapon. Uses
    /// a one-weapon class, since the placeholder fills all six.
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

    /// One shared clock would let the alt-fire's cooldown eat the primary's.
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

    /// The message carries the action fired, not what is held when it is read.
    #[test]
    fn the_action_that_was_fired_is_the_one_the_message_carries() {
        let mut world = a_world(default_catalog());
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack = true;

        step(&mut world);
        let fired = shots(&mut world);
        assert_eq!(fired.len(), 1, "{fired:?}");

        world.get_mut::<Equipped>(shooter).unwrap().select(Slot::Secondary);

        assert!(
            matches!(fired[0].action, WeaponAction::Hitscan(_)),
            "the shot changed when the weapon did: {:?}",
            fired[0].action
        );
    }

    /// A button with `WeaponAction::None` writes nothing.
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

    /// An empty weapon writes nothing and arms no cooldown.
    #[test]
    fn an_empty_weapon_fires_nothing_and_arms_nothing() {
        let mut world = a_world(one_weapon(weapon(
            auto(WeaponAction::Hitscan(LASER), 0.1, 1),
            Mounted::EMPTY,
            // One shot, then nothing ever.
            Magazine { clip: Some(1), reserve: 0, reload: 1.0, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).attack_held = true;

        step(&mut world);
        assert_eq!(shots(&mut world).len(), 1, "the one round it had did not fire");

        // Let the first shot's cooldown run out first.
        for _ in 0..16 {
            step(&mut world);
            assert!(shots(&mut world).is_empty(), "it fired a round it did not have");
        }
        assert!(
            world.get::<Trigger>(shooter).unwrap().ready(Button::Primary),
            "an empty weapon armed its cooldown"
        );
    }

    /// No clip: a shot comes straight out of the reserve.
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

    /// No clip: it never reloads.
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

    /// A free action works on an empty gun.
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

    /// A reload moves rounds from the reserve into the clip.
    #[test]
    fn a_reload_moves_rounds_from_the_reserve_to_the_clip() {
        let mut world = a_world(one_weapon(weapon(
            semi(WeaponAction::Hitscan(LASER), 1),
            Mounted::EMPTY,
            Magazine { clip: Some(4), reserve: 8, reload: 0.1, reload_one_at_a_time: false },
        )));
        let shooter = a_shooter(&mut world);

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

        // Empty, so a reload starts by itself.
        for _ in 0..16 {
            step(&mut world);
        }

        let state = ammo(&world, shooter, Slot::Primary);
        assert_eq!(state.clip, 1, "the clip took more than the reserve had");
        assert_eq!(state.reserve, 0);
    }

    /// An empty clip reloads without a keypress.
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

    /// Switching weapons cancels a reload.
    #[test]
    fn switching_weapons_cancels_a_reload() {
        let mut world = a_world(default_catalog());
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

    /// A weapon just drawn fires once its deploy has run out, and not before.
    #[test]
    fn a_weapon_just_drawn_waits_out_its_deploy() {
        const DEPLOY: f32 = 0.25;
        let free = Mounted {
            action: WeaponAction::Hitscan(LASER),
            cadence: Cadence::Semi,
            cost: Cost::Free,
        };
        let mut catalog = one_weapon(weapon(free, Mounted::EMPTY, Magazine::BOTTOMLESS));
        let drawn = Weapon {
            key: String::new(),
            id: WeaponId::of("drawn"),
            deploy: DEPLOY,
            ..weapon(free, Mounted::EMPTY, Magazine::BOTTOMLESS)
        };
        let id = drawn.id;
        catalog.insert(drawn);
        let mut loadout = catalog.loadout(crate::common::class::Class::Mercenary).unwrap().clone();
        loadout.set(
            Slot::Secondary,
            crate::common::weapon::SlotChoices { permitted: vec![id], default: id },
        );
        catalog.set_loadout(crate::common::class::Class::Mercenary, loadout);

        let mut world = a_world(catalog);
        let shooter = a_shooter(&mut world);
        input(&mut world, shooter).select = Some(Slot::Secondary);

        let mut ticks = 0;
        loop {
            ticks += 1;
            assert!(ticks < 64, "the drawn weapon never fired");
            world.run_system_once(select_weapons).unwrap();
            input(&mut world, shooter).attack = true;
            step(&mut world);
            if !shots(&mut world).is_empty() {
                break;
            }
        }

        let expected = (DEPLOY / crate::constants::one_tick().as_secs_f32()).round() as i32;
        assert!(
            (expected..=expected + 1).contains(&ticks),
            "fired on tick {ticks}, deploy is {expected} ticks"
        );
    }

    /// `Slot::ALL`, `index` and `from_index` agree.
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
