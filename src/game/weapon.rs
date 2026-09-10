//! What a body is holding, and the moment it pulls the trigger.
//!
//! Two things live here, and they exist to keep the weapons themselves from
//! having to know about each other.
//!
//! **The trigger is taken once, in one place.** [`PlayerInput::attack`] is a
//! latch — one press is one shot however many frames pass before a fixed step
//! reads it — and a latch is only correct while exactly one system consumes
//! it. With one weapon that was the weapon's own business; with two it is a
//! bug waiting to be written, because whichever firing system happened to run
//! first would eat the press and the other would never fire.
//! [`pull_trigger`] takes it and writes a [`TriggerPulled`] per shooter, and
//! each weapon reads that.
//!
//! **A weapon is a value, not a system.** [`Weapon`] is either a trace or a
//! [`ProjectileSpec`], so the rocket launcher, the pipe bomb launcher and the
//! TF2C-style RPG are three constants and one firing system rather than three
//! of anything. Adding a fourth is adding a line to [`Loadout::default`].
//!
//! Selection goes through [`PlayerInput`] like every other input rather than
//! being read off the keyboard where it is used, and for the same reason: the
//! step has to be a function of `(state, input, dt)` if a client and a server
//! are ever to agree about it. The input carries a **slot number** and not the
//! weapon, so what travels stays small and what a slot holds stays a property
//! of the body.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::projectile::ProjectileSpec;
use crate::game::damage::{DamagePlugin, DamageSystems};
use crate::game::explosion::ExplosionPlugin;
use crate::game::hitscan::HitscanPlugin;
use crate::game::player::{Player, PlayerInput};
use crate::game::projectile::ProjectilePlugin;

/// One thing you can shoot with.
///
/// The whole taxonomy: a shot either lands the instant it is fired or it
/// travels. Everything else about a weapon — how much it hurts, how far it
/// reaches, how it arcs — is numbers inside one of these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Weapon {
    /// Lands the instant it is fired. See [`crate::game::hitscan`].
    Hitscan,
    /// Leaves the muzzle and has to get there. See
    /// [`crate::game::projectile`].
    Projectile(ProjectileSpec),
}

/// What a body is carrying, and which of it is in its hands.
///
/// A `Vec` and a cursor rather than a fixed primary/secondary pair: what a
/// class carries is a loadout question and this layer should not answer it.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct Loadout {
    slots: Vec<Weapon>,
    held: usize,
}

impl Default for Loadout {
    /// The debug loadout: one of each kind of thing, on keys 1 to 4.
    ///
    /// Not a class. There are no classes with weapons yet, and this is what a
    /// body gets so that the layer below it can be shot with and looked at.
    fn default() -> Self {
        Self {
            slots: vec![
                Weapon::Hitscan,
                Weapon::Projectile(ProjectileSpec::ROCKET),
                Weapon::Projectile(ProjectileSpec::PIPE_BOMB),
                Weapon::Projectile(ProjectileSpec::RPG),
            ],
            held: 0,
        }
    }
}

impl Loadout {
    pub fn new(slots: Vec<Weapon>) -> Self {
        Self { slots, held: 0 }
    }

    /// What is in its hands. A loadout with nothing in it holds nothing, which
    /// is a body that cannot shoot rather than a panic.
    pub fn held(&self) -> Option<Weapon> {
        self.slots.get(self.held).copied()
    }

    /// Switch to a slot. A slot that is not carried is ignored, so pressing 4
    /// with three weapons leaves you holding what you had.
    pub fn select(&mut self, slot: usize) {
        if slot < self.slots.len() {
            self.held = slot;
        }
    }
}

/// A trigger was pulled by this body, on this tick.
///
/// One per shooter per press. Every weapon reads it and answers for the
/// shooters holding it, which is what lets a weapon be added without anything
/// already here being touched.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct TriggerPulled {
    pub shooter: Entity,
}

pub struct WeaponPlugin;

impl Plugin for WeaponPlugin {
    fn build(&self, app: &mut App) {
        app
            // The damage layer everything here writes into, and the two things
            // that read it. Brought in together because a weapon that dealt
            // damage nobody could see or be killed by is not a weapon.
            .add_plugins((DamagePlugin, ExplosionPlugin))
            .add_plugins((HitscanPlugin, ProjectilePlugin))
            .add_message::<TriggerPulled>()
            // Ahead of every weapon, and ahead of the damage they deal: the
            // press has to be turned into messages before anything can read
            // them, and a switch has to land before the shot it was made for.
            .add_systems(
                FixedUpdate,
                (select_weapons, pull_trigger)
                    .chain()
                    .before(DamageSystems::Deal)
                    .run_if(in_state(AppMode::Play)),
            )
        ;
    }
}

/// Put the latched press in front of every weapon, once.
pub fn pull_trigger(
    mut input: ResMut<PlayerInput>,
    shooters: Query<Entity, With<Player>>,
    mut pulled: MessageWriter<TriggerPulled>,
) {
    // Taken, not read: leaving it set would fire again next tick, and taking
    // it here rather than in a weapon is what stops two weapons racing for it.
    if !std::mem::take(&mut input.attack) {
        return;
    }

    for shooter in &shooters {
        pulled.write(TriggerPulled { shooter });
    }
}

/// Act on a switch made since the last step.
pub fn select_weapons(mut input: ResMut<PlayerInput>, mut carriers: Query<&mut Loadout, With<Player>>) {
    let Some(slot) = std::mem::take(&mut input.select) else { return };
    for mut loadout in &mut carriers {
        loadout.select(slot);
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn a_world() -> World {
        let mut world = World::new();
        world.init_resource::<PlayerInput>();
        world.init_resource::<Messages<TriggerPulled>>();
        world
    }

    fn pulls(world: &mut World) -> Vec<TriggerPulled> {
        let messages = world.resource::<Messages<TriggerPulled>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    /// One press is one shot: the latch is taken by the tick that read it,
    /// and the next tick sees nothing.
    #[test]
    fn a_press_is_announced_once() {
        let mut world = a_world();
        let shooter = world.spawn(Player::default()).id();
        world.resource_mut::<PlayerInput>().attack = true;

        world.run_system_once(pull_trigger).unwrap();
        assert_eq!(pulls(&mut world), vec![TriggerPulled { shooter }]);
        assert!(!world.resource::<PlayerInput>().attack);

        world.resource_mut::<Messages<TriggerPulled>>().clear();
        world.run_system_once(pull_trigger).unwrap();
        assert!(pulls(&mut world).is_empty(), "the same press fired twice");
    }

    /// A body that is not holding anything cannot be shot with, and a slot it
    /// does not carry is not a slot.
    #[test]
    fn selecting_a_slot_that_is_not_carried_changes_nothing() {
        let mut loadout = Loadout::new(vec![Weapon::Hitscan, Weapon::Projectile(ProjectileSpec::ROCKET)]);
        loadout.select(1);
        assert_eq!(loadout.held(), Some(Weapon::Projectile(ProjectileSpec::ROCKET)));

        loadout.select(7);
        assert_eq!(loadout.held(), Some(Weapon::Projectile(ProjectileSpec::ROCKET)), "an empty slot was equipped");

        assert_eq!(Loadout::new(vec![]).held(), None);
    }

    /// A switch travels through the input like everything else, and is
    /// consumed by the step that acted on it.
    #[test]
    fn a_switch_is_taken_by_the_step_that_acts_on_it() {
        let mut world = a_world();
        let player = world.spawn((Player::default(), Loadout::default())).id();
        world.resource_mut::<PlayerInput>().select = Some(2);

        world.run_system_once(select_weapons).unwrap();

        assert_eq!(
            world.get::<Loadout>(player).unwrap().held(),
            Some(Weapon::Projectile(ProjectileSpec::PIPE_BOMB))
        );
        assert_eq!(world.resource::<PlayerInput>().select, None);
    }
}
