//! A [`ScopeSpec`] button raises and lowers a weapon's sights. The narrower
//! view, the slower mouse and [`Weapon::scoped_primary`] apply only once they
//! are all the way up, and stop as soon as they start down.
//!
//! [`Sights`] is predicted, as movement is: scoping is never refused, so a
//! client can work its own from its presses. The press is read from [`Inputs`]
//! because nothing that fires actions runs on a client.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::weapon::{Equipped, Mounted, ScopeSpec, Weapon, WeaponAction, WeaponCatalog, WeaponId};
use crate::game::condition::Condition;
use crate::game::player::{Inputs, Player, Simulated};
use crate::game::weapon::Button;

#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sights {
    /// Asked for: on the way up or up.
    pub up: bool,
    /// 0 down to 1 up, moving toward `up`.
    pub raised: f32,
    /// So that switching to another scoped weapon starts with its sights down.
    pub weapon: Option<WeaponId>,
    /// Last tick's. A press toggles only as the button goes down, so a server
    /// that repeats a late tick's input sees one press.
    pub button_down: bool,
}

impl Sights {
    pub fn sighted(&self) -> bool {
        self.up && self.raised >= 1.0
    }

    /// The scope in force: the held weapon's, while sighted on it.
    pub fn scope(&self, equipped: &Equipped, catalog: &WeaponCatalog) -> Option<ScopeSpec> {
        if !self.sighted() || equipped.held() != self.weapon {
            return None;
        }
        catalog.held_by(equipped)?.scope()
    }

    /// The scoped primary needs the scope on the secondary; a scope on the
    /// primary keeps the primary's clicks.
    pub fn mounted(&self, weapon: &Weapon, button: Button) -> Mounted {
        let scope_on_secondary = matches!(weapon.secondary.action, WeaponAction::Scope(_));
        match button {
            Button::Primary if scope_on_secondary && self.sighted() && self.weapon == Some(weapon.id) => {
                weapon.scoped_primary.unwrap_or(weapon.primary)
            }
            _ => button.of(weapon),
        }
    }
}

/// The viewed body's last sighted shot, timed from when this machine saw it, for
/// [`ScopeSpec::rechamber`]. Drawing only: [`Sights`] stays up, so the next shot
/// is still the scoped one.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct Rechamber {
    body: Option<Entity>,
    /// Seconds.
    since: f32,
    cooldown: f32,
}

impl Default for Rechamber {
    fn default() -> Self {
        Self { body: None, since: f32::INFINITY, cooldown: 0.0 }
    }
}

impl Rechamber {
    /// 0 to 1.
    pub fn down(&self, body: Entity, scope: &ScopeSpec) -> f32 {
        if self.body != Some(body) {
            return 0.0;
        }
        scope.rechamber.map_or(0.0, |after| rechamber_down(self.since, self.cooldown, after, scope))
    }
}

/// 0 to 1: lowered over `scope.lower` from `after`, raised over `scope.raise`
/// to be up as the cooldown ends. A cooldown too short for both comes only part
/// way down.
pub fn rechamber_down(since: f32, cooldown: f32, after: f32, scope: &ScopeSpec) -> f32 {
    if !(after..cooldown).contains(&since) {
        return 0.0;
    }
    let fall = match scope.lower > 0.0 {
        true => (since - after) / scope.lower,
        false => 1.0,
    };
    let rise = match scope.raise > 0.0 {
        true => (cooldown - since) / scope.raise,
        false => 1.0,
    };
    fall.min(rise).clamp(0.0, 1.0)
}

pub fn time_the_rechamber(
    time: Res<Time>,
    catalog: Res<WeaponCatalog>,
    mut seen: MessageReader<crate::game::recoil::ShotSeen>,
    viewed: Query<(Entity, &Equipped, &Sights), crate::game::player::Viewed>,
    mut rechamber: ResMut<Rechamber>,
) {
    let mut next = *rechamber;
    next.since += time.delta_secs();
    // Drained every frame, so a shot is never read a frame late.
    let shots: Vec<_> = seen.read().filter(|shot| shot.button == Button::Primary).map(|shot| shot.body).collect();
    if let Ok((body, equipped, sights)) = viewed.single()
        && sights.sighted()
        && shots.contains(&body)
        && let Some(weapon) = catalog.held_by(equipped)
    {
        next = Rechamber { body: Some(body), since: 0.0, cooldown: sights.mounted(weapon, Button::Primary).cadence.interval() };
    }
    if next != *rechamber {
        *rechamber = next;
    }
}

/// The sights as drawn and aimed with, a rechamber included.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shown {
    pub raised: f32,
    pub scope: Option<ScopeSpec>,
}

pub fn shown(body: Entity, sights: &Sights, equipped: &Equipped, catalog: &WeaponCatalog, rechamber: Option<&Rechamber>) -> Shown {
    let down = catalog
        .held_by(equipped)
        .and_then(|weapon| weapon.scope())
        .zip(rechamber)
        .map_or(0.0, |(scope, rechamber)| rechamber.down(body, &scope));
    Shown { raised: sights.raised * (1.0 - down), scope: sights.scope(equipped, catalog).filter(|_| down <= 0.0) }
}

/// A switch, a weapon with no scope, or a condition that forbids firing puts the
/// sights down at once. Never takes the input: a rollback replays it.
pub fn work_the_sights(
    time: Res<Time<Fixed>>,
    catalog: Res<WeaponCatalog>,
    mut bodies: Query<(&Equipped, &Inputs, &mut Sights, Option<&Condition>), (With<Player>, With<Simulated>)>,
) {
    let dt = time.delta_secs();

    for (equipped, input, mut sights, condition) in &mut bodies {
        let mut next = *sights;
        let weapon = catalog.held_by(equipped);
        // `pressed` too: a tap released between ticks was never held on a tick that ran.
        let down = weapon.is_some_and(|weapon| {
            Button::BOTH.into_iter().any(|button| {
                matches!(button.of(weapon).action, WeaponAction::Scope(_))
                    && match button {
                        Button::Primary => input.attack || input.attack_held,
                        Button::Secondary => input.alt_attack || input.alt_attack_held,
                    }
            })
        });
        let went_down = down && !next.button_down;
        next.button_down = down;
        if next.weapon != equipped.held() {
            next = Sights { weapon: equipped.held(), button_down: down, ..default() };
        }
        let scope = weapon.and_then(|weapon| weapon.scope());
        let allowed = condition.is_none_or(|condition| condition.may_fire());

        match (weapon, scope) {
            (Some(_), Some(scope)) if allowed => {
                if input.hold_sights {
                    next.up = down;
                } else if went_down {
                    next.up = !next.up;
                }
                next.raised = if next.up {
                    step_toward(next.raised, 1.0, dt, scope.raise)
                } else {
                    step_toward(next.raised, 0.0, dt, scope.lower)
                };
            }
            _ => next = Sights { weapon: next.weapon, button_down: next.button_down, ..default() },
        }

        // Replicated, so written only on change.
        if next != *sights {
            *sights = next;
        }
    }
}

/// At a rate covering 0 to 1 in `seconds`; all the way at once for zero.
fn step_toward(from: f32, to: f32, dt: f32, seconds: f32) -> f32 {
    if seconds <= 0.0 {
        return to;
    }
    let step = dt / seconds;
    if to > from { (from + step).min(to) } else { (from - step).max(to) }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::weapon::{Cadence, Cost, HitscanSpec, Magazine};
    use crate::common::weapon_file::default_catalog;

    fn a_world() -> (World, Entity) {
        let mut world = World::new();
        world.insert_resource(default_catalog());
        let mut time = Time::<Fixed>::from_hz(64.0);
        time.advance_by(time.timestep());
        world.insert_resource(time);
        let equipped = Equipped::carrying([WeaponId::of("sniper_rifle")]);
        let body = world.spawn((Player::default(), equipped, Simulated)).id();
        (world, body)
    }

    fn press(world: &mut World, body: Entity) {
        let mut input = world.get_mut::<Inputs>(body).unwrap();
        input.alt_attack = true;
        input.alt_attack_held = true;
    }

    fn tick(world: &mut World) -> Sights {
        world.run_system_once(work_the_sights).unwrap();
        let body = world.query_filtered::<Entity, With<Player>>().single(world).unwrap();
        let mut input = world.get_mut::<Inputs>(body).unwrap();
        input.alt_attack = false;
        input.alt_attack_held = false;
        let body = world.query_filtered::<Entity, With<Player>>().single(world).unwrap();
        *world.get::<Sights>(body).unwrap()
    }

    /// Fails if the scope takes effect before the raise lands, or lingers while lowering.
    #[test]
    fn the_sights_take_effect_only_once_all_the_way_up() {
        let (mut world, body) = a_world();
        let scope = world.resource::<WeaponCatalog>().get(WeaponId::of("sniper_rifle")).unwrap().scope().unwrap();
        press(&mut world, body);
        let ticks = (scope.raise * 64.0).ceil() as usize;
        for _ in 1..ticks {
            let sights = tick(&mut world);
            assert!(sights.up && !sights.sighted(), "{sights:?}");
        }
        assert!(tick(&mut world).sighted());

        press(&mut world, body);
        let sights = tick(&mut world);
        assert!(!sights.sighted() && sights.raised > 0.0, "{sights:?}");
    }

    /// Fails if, held to sight, the sights are not up while the button is held and lowering once it
    /// is let go.
    #[test]
    fn held_to_sight_the_sights_are_up_only_while_held() {
        let (mut world, body) = a_world();
        world.get_mut::<Inputs>(body).unwrap().hold_sights = true;
        for _ in 0..3 {
            press(&mut world, body);
            assert!(tick(&mut world).up, "the sights were down while held");
        }
        assert!(!tick(&mut world).up, "the sights stayed up once let go");
        assert!(!tick(&mut world).up, "the sights came back up with nothing held");
    }

    /// Fails if the scoped primary is not what fires while sighted, or is while not.
    #[test]
    fn the_scoped_primary_fires_only_while_sighted() {
        let laser = |damage| Mounted {
            action: WeaponAction::Hitscan(HitscanSpec { damage, ..default() }),
            cadence: Cadence::Semi { interval: 0.0 },
            cost: Cost::Free,
        };
        let weapon = Weapon {
            id: WeaponId::of("test"),
            key: "test".into(),
            name_key: String::new(),
            model: None,
            icon: None,
            bucket: 1,
            deploy: 0.0,
            primary: laser(1),
            secondary: Mounted { action: WeaponAction::Scope(ScopeSpec::default()), ..Mounted::EMPTY },
            scoped_primary: Some(laser(2)),
            magazine: Magazine::BOTTOMLESS,
        };
        let damage = |sights: Sights| match sights.mounted(&weapon, Button::Primary).action {
            WeaponAction::Hitscan(spec) => spec.damage,
            _ => 0,
        };
        let up = Sights { up: true, raised: 1.0, weapon: Some(weapon.id), button_down: false };
        assert_eq!(damage(Sights::default()), 1);
        assert_eq!(damage(Sights { raised: 0.5, ..up }), 1);
        assert_eq!(damage(up), 2);
        assert_eq!(damage(Sights { weapon: Some(WeaponId::of("other")), ..up }), 1);
    }

    /// Fails if a rechamber starts early, is still down as the cooldown ends, or never gets all
    /// the way down.
    #[test]
    fn a_rechamber_comes_down_after_its_delay_and_is_up_as_the_cooldown_ends() {
        let scope = ScopeSpec { raise: 0.25, lower: 0.2, rechamber: Some(0.5), ..default() };
        let down = |since| rechamber_down(since, 1.5, 0.5, &scope);
        assert_eq!(down(0.49), 0.0);
        assert!(down(0.6) > 0.0 && down(0.6) < 1.0);
        assert_eq!(down(1.0), 1.0);
        assert!(down(1.49) > 0.0);
        assert_eq!(down(1.5), 0.0);
    }

    /// Fails if one press seen on two ticks toggles twice.
    #[test]
    fn a_press_seen_twice_is_one_toggle() {
        let (mut world, body) = a_world();
        world.run_system_once(work_the_sights).unwrap();
        press(&mut world, body);
        world.run_system_once(work_the_sights).unwrap();
        world.run_system_once(work_the_sights).unwrap();
        assert!(world.get::<Sights>(body).unwrap().up, "the repeated press lowered the sights again");
    }

    /// Fails if a scope on the primary hands its clicks to the scoped primary.
    #[test]
    fn a_scope_on_the_primary_keeps_its_click() {
        let weapon = Weapon {
            id: WeaponId::of("test"),
            key: "test".into(),
            name_key: String::new(),
            model: None,
            icon: None,
            bucket: 1,
            deploy: 0.0,
            primary: Mounted { action: WeaponAction::Scope(ScopeSpec::default()), ..Mounted::EMPTY },
            secondary: Mounted::EMPTY,
            scoped_primary: Some(Mounted { action: WeaponAction::Hitscan(HitscanSpec::default()), ..Mounted::EMPTY }),
            magazine: Magazine::BOTTOMLESS,
        };
        let up = Sights { up: true, raised: 1.0, weapon: Some(weapon.id), button_down: false };
        assert!(matches!(up.mounted(&weapon, Button::Primary).action, WeaponAction::Scope(_)));
    }

    /// Fails if the sights stay up through a switch.
    #[test]
    fn a_switch_puts_the_sights_down() {
        let (mut world, body) = a_world();
        *world.get_mut::<Sights>(body).unwrap() = Sights { up: true, raised: 1.0, weapon: Some(WeaponId::of("sniper_rifle")), button_down: false };
        let mut equipped = world.get_mut::<Equipped>(body).unwrap();
        equipped.add(WeaponId::of("shotgun"));
        equipped.select(WeaponId::of("shotgun"));
        assert_eq!(tick(&mut world), Sights { weapon: Some(WeaponId::of("shotgun")), ..default() });
    }
}
