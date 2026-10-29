//! What a [`TriggerVolume`] does to a body whose movement box overlaps it.
//! Authority only, in [`DamageSystems::Deal`]: hurting writes [`Damage`] and
//! killing writes [`Kill`], and neither touches health itself.

use bevy::prelude::*;

use crate::common::class::Stance;
use crate::common::damage::{Damage, DamageSource, Kill};
use crate::common::trigger::{TriggerEffect, TriggerVolume};
use crate::editor::trigger::TRIGGER_COLOR;
use crate::game::damage::DamageSystems;
use crate::game::death::Dying;
use crate::game::player::{PhysicsBody, Player};

/// Seconds between hurts, as with burning: a number a tick would be a blur.
const HURT_INTERVAL: f32 = 0.5;

pub struct TriggerPlugin;

impl Plugin for TriggerPlugin {
    fn build(&self, app: &mut App) {
        // Also `DamagePlugin`'s; here too for an app with no damage layer.
        app.add_message::<Damage>()
            .add_message::<Kill>()
            .add_systems(FixedUpdate, act_on_bodies_in_triggers.in_set(DamageSystems::Deal))
            // With the pickups' volumes on F6.
            .add_systems(
                PostUpdate,
                draw_triggers
                    .run_if(resource_exists::<GizmoConfigStore>)
                    .run_if(resource_exists::<crate::game::skeleton::ShowBones>)
                    .run_if(crate::common::app_mode::showing_the_world),
            );
    }
}

/// Counts down whether or not the body is in a trigger, so stepping out and
/// back in does not hurt again early, and entering hurts at once.
#[derive(Component, Debug, Default)]
pub struct Scalding {
    until_next: f32,
    /// The fraction of a point not yet dealt, so a rate that does not divide
    /// by the interval still comes out right over time.
    owed: f32,
}

impl Scalding {
    /// What to deal this tick for `per_second` of exposure.
    fn tick(&mut self, dt: f32, per_second: u32) -> u32 {
        self.until_next = (self.until_next - dt).max(0.0);
        if per_second == 0 || self.until_next > 0.0 {
            return 0;
        }
        self.until_next = HURT_INTERVAL;
        self.owed += per_second as f32 * HURT_INTERVAL;
        let due = self.owed.floor();
        self.owed -= due;
        due as u32
    }
}

#[allow(clippy::type_complexity)]
fn act_on_bodies_in_triggers(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    triggers: Query<&TriggerVolume>,
    mut bodies: Query<(Entity, &PhysicsBody, &Stance, Option<&mut Scalding>), (With<Player>, Without<Dying>)>,
    mut damage: MessageWriter<Damage>,
    mut kills: MessageWriter<Kill>,
) {
    let dt = time.delta_secs();
    for (body, placed, stance, scalding) in &mut bodies {
        let (center, half) = (placed.current, stance.half_extents());
        let mut per_second = 0;
        let mut killed = false;
        for trigger in triggers.iter().filter(|trigger| trigger.overlaps(center, half)) {
            match trigger.effect {
                TriggerEffect::Hurt { per_second: rate } => per_second += rate,
                TriggerEffect::Kill => killed = true,
            }
        }
        if killed {
            kills.write(Kill { target: body, source: DamageSource::World, point: center });
            continue;
        }
        let amount = match scalding {
            Some(mut scalding) => scalding.tick(dt, per_second),
            None if per_second > 0 => {
                let mut scalding = Scalding::default();
                let amount = scalding.tick(dt, per_second);
                commands.entity(body).insert(scalding);
                amount
            }
            None => 0,
        };
        if amount > 0 {
            damage.write(Damage { target: body, source: DamageSource::World, amount, point: center, from: None, kill_shove: None });
        }
    }
}

fn draw_triggers(show: Res<crate::game::skeleton::ShowBones>, triggers: Query<&TriggerVolume>, mut gizmos: Gizmos) {
    if !show.0 {
        return;
    }
    for trigger in &triggers {
        let size = trigger.max - trigger.min;
        gizmos.cube(Transform::from_translation(trigger.min + size / 2.0).with_scale(size), TRIGGER_COLOR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::app_mode::AppMode;
    use crate::common::damage::{Armor, DamageDealt, Damageable, Died};

    fn a_round() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::state::app::StatesPlugin, bevy::time::TimePlugin));
        app.init_state::<AppMode>();
        app.init_resource::<crate::common::skeleton::AnimationClock>();
        app.add_plugins((crate::game::damage::DamagePlugin, TriggerPlugin));
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        app
    }

    fn body_at(app: &mut App, center: Vec3) -> Entity {
        let mut armor = Armor::default();
        armor.add(50);
        app.world_mut().spawn((Player::default(), PhysicsBody::at(center), Stance::Standing, armor)).id()
    }

    fn volume(min: Vec3, effect: TriggerEffect) -> TriggerVolume {
        TriggerVolume { min, max: min + Vec3::splat(2.0), effect }
    }

    /// Breaks if a kill goes through `Damage`: armor would soak it.
    #[test]
    fn a_kill_trigger_kills_through_armor_and_the_map_is_credited() {
        let mut app = a_round();
        app.world_mut().spawn(volume(Vec3::ZERO, TriggerEffect::Kill));
        let inside = body_at(&mut app, Vec3::ONE);
        let outside = body_at(&mut app, Vec3::new(10.0, 1.0, 0.0));

        app.world_mut().run_schedule(FixedUpdate);

        let died: Vec<Died> = app.world_mut().resource_mut::<Messages<Died>>().drain().collect();
        assert_eq!(died.len(), 1, "{died:?}");
        assert_eq!(died[0].victim, inside);
        assert_eq!(died[0].killer, DamageSource::World);
        assert!(app.world().get::<Damageable>(outside).unwrap().is_alive());
    }

    #[test]
    fn a_hurt_trigger_deals_its_rate_on_entering() {
        let mut app = a_round();
        app.world_mut().spawn(volume(Vec3::ZERO, TriggerEffect::Hurt { per_second: 40 }));
        let body = body_at(&mut app, Vec3::new(2.3, 1.0, 1.0));

        app.world_mut().run_schedule(FixedUpdate);

        let dealt: Vec<DamageDealt> = app.world_mut().resource_mut::<Messages<DamageDealt>>().drain().collect();
        assert_eq!(dealt.len(), 1, "{dealt:?}");
        assert_eq!((dealt[0].target, dealt[0].amount), (body, 20));
    }

    #[test]
    fn entering_hurts_at_once_then_on_the_interval() {
        let mut scalding = Scalding::default();
        assert_eq!(scalding.tick(1.0 / 64.0, 20), 10);
        assert_eq!(scalding.tick(0.25, 20), 0);
        assert_eq!(scalding.tick(0.25, 20), 10);
    }

    #[test]
    fn a_rate_the_interval_does_not_divide_adds_up() {
        let mut scalding = Scalding::default();
        let dealt: u32 = (0..20).map(|_| scalding.tick(HURT_INTERVAL, 3)).sum();
        assert_eq!(dealt, 30);
    }

    #[test]
    fn stepping_out_and_back_in_does_not_hurt_early() {
        let mut scalding = Scalding::default();
        assert_eq!(scalding.tick(0.0, 10), 5);
        assert_eq!(scalding.tick(0.1, 0), 0);
        assert_eq!(scalding.tick(0.1, 10), 0);
    }
}
