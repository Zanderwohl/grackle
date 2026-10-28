//! What a hit leads to: a number where it landed, and the body going away once
//! health runs out. The reaping is in [`crate::game::death`], wired up here
//! because this is where [`DamageDealt`] is registered.
//!
//! A number is anchored to a world point and projected every frame, so it
//! stays on the body as the camera moves. Size is in logical points and rise
//! in screen fraction, so a hit reads the same at any range.
//! `world_to_viewport` already returns logical points: no scale factor applies.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::AppMode;
use crate::common::net::has_authority;
use crate::common::damage::{Armor, Damage, DamageDealt, Damageable, Died};
use crate::common::game_rules::GameRules;
use crate::common::team::Allegiances;
use crate::game::death::{announce_deaths, announce_the_dead, reap_the_dead, record_damage, suicide};
use crate::common::damage::{DamageSource, PlayerId};
use crate::game::player::{LocalPlayer, PlayerCamera};

/// How long a damage number lasts, in seconds.
pub const DAMAGE_NUMBER_LIFETIME: f32 = 2.0;

/// How big the number is drawn, in logical points.
const DAMAGE_NUMBER_SIZE: f32 = 48.0;

/// How far a number climbs over its life, as a fraction of window height, at
/// constant speed.
const DAMAGE_NUMBER_RISE: f32 = 0.2;

/// A number floating at the place a hit landed.
#[derive(Component, Debug)]
pub struct DamageNumber {
    pub amount: u32,
    /// Where the hit was, in world space.
    pub at: Vec3,
    /// Seconds of life left.
    pub remaining: f32,
}

/// The order a tick's damage goes through. A new source joins
/// [`DamageSystems::Deal`] and needs no mention elsewhere.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DamageSystems {
    /// Everything that writes [`Damage`].
    Deal,
    /// [`apply_damage`] and nothing else.
    Apply,
    /// What the records lead to: the log, and then the reaping.
    Resolve,
}

pub struct DamagePlugin;

impl Plugin for DamagePlugin {
    fn build(&self, app: &mut App) {
        app
            .add_message::<Damage>()
            // Every visual reads this queue, filled locally or off the wire.
            .add_message::<crate::common::effects::Effect>()
            .add_message::<DamageDealt>()
            .add_message::<Died>()
            // A body reaped before its log is written is a kill credited to
            // nobody. Authority only: `Damageable` is replicated, and a client
            // resolving damage itself would disagree with the server. Not
            // predicted, since a wrongly guessed kill is a body that falls
            // over and stands back up.
            // is worse to watch than a kill that arrives a round-trip late.
            .configure_sets(
                FixedUpdate,
                (DamageSystems::Deal, DamageSystems::Apply, DamageSystems::Resolve)
                    .chain()
                    .run_if(in_state(AppMode::Play))
                    .run_if(has_authority),
            )
            .add_systems(FixedUpdate, apply_damage.in_set(DamageSystems::Apply))
            .add_systems(FixedUpdate, suicide.in_set(DamageSystems::Deal))
            .add_systems(
                FixedUpdate,
                // `reap_the_dead` removes what was marked on an earlier tick,
                // giving replication a tick to carry the zero health.
                (record_damage, announce_the_dead, reap_the_dead)
                    .chain()
                    .in_set(DamageSystems::Resolve),
            )
            .add_systems(Update, (spawn_damage_numbers, fade_damage_numbers, announce_deaths).chain())
            .add_systems(
                EguiPrimaryContextPass,
                draw_damage_numbers.run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), clear_damage_numbers)
        ;
    }
}

/// The only system that touches a health pool or [`Armor`], and where teams
/// are enforced (see [`crate::common::team`]).
///
/// A target with no [`Damageable`] is skipped: an explosion sweeps up whatever
/// is nearby. A request at a teammate is dropped with no [`DamageDealt`], so
/// no zero is drawn over a friend. The record is the whole hit, armor's share
/// included: a number is what you did, not where it landed.
pub fn apply_damage(
    mut requests: MessageReader<Damage>,
    mut health: Query<(&mut Damageable, Option<&mut Armor>)>,
    allegiances: Allegiances,
    // Absent in tests that build no map.
    rules: Option<Res<GameRules>>,
    mut dealt: MessageWriter<DamageDealt>,
) {
    let self_damage = rules.map_or(1.0, |rules| rules.self_damage);
    for request in requests.read() {
        if !allegiances.may_hurt(request.source, request.target) {
            continue;
        }
        let Ok((mut target, armor)) = health.get_mut(request.target) else { continue };
        let requested = if allegiances.is_self(request.source, request.target) {
            (request.amount as f32 * self_damage).round() as u32
        } else {
            request.amount
        };

        // An emptied body's armor is not spent on it.
        let through = match armor {
            Some(mut armor) if target.is_alive() && armor.points() > 0 => armor.soak(requested),
            _ => requested,
        };
        // Written even at zero, so a debug harness can keep shooting an
        // emptied body.
        let amount = requested - through + target.apply(through);
        dealt.write(DamageDealt {
            target: request.target,
            source: request.source,
            amount,
            remaining: target.health(),
            point: request.point,
            from: request.from,
        });
    }
}

/// Only for hits we dealt, as in TF2: every machine is sent every hit. Who we
/// are is our body's id, else the host's own identity, else whose death the
/// camera is watching, so a rocket that lands after we died still counts.
/// Hurting ourselves draws nothing.
fn spawn_damage_numbers(
    mut commands: Commands,
    mut hits: MessageReader<DamageDealt>,
    us: Option<Single<(Entity, &PlayerId), With<LocalPlayer>>>,
    identity: Option<Res<crate::game::respawn::OurIdentity>>,
    death_cam: Option<Res<crate::game::player::DeathCam>>,
) {
    let body = us.map(|us| *us);
    let ours = body
        .map(|(_, &id)| id)
        .or(identity.map(|identity| identity.0.id))
        .or(death_cam.and_then(|cam| cam.whose));
    for hit in hits.read() {
        if ours.is_none_or(|ours| hit.source != DamageSource::Player(ours)) || Some(hit.target) == body.map(|(entity, _)| entity) {
            continue;
        }
        commands.spawn((
            DamageNumber {
                amount: hit.amount,
                at: hit.point,
                remaining: DAMAGE_NUMBER_LIFETIME,
            },
            Name::new("Damage number"),
        ));
    }
}

fn fade_damage_numbers(
    mut commands: Commands,
    time: Res<Time>,
    mut numbers: Query<(Entity, &mut DamageNumber)>,
) {
    for (entity, mut number) in &mut numbers {
        number.remaining -= time.delta_secs();
        if number.remaining <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

fn clear_damage_numbers(mut commands: Commands, numbers: Query<Entity, With<DamageNumber>>) {
    for number in &numbers {
        commands.entity(number).despawn();
    }
}

fn opacity(remaining: f32) -> f32 {
    (remaining / DAMAGE_NUMBER_LIFETIME).clamp(0.0, 1.0)
}

/// Logical points above the hit point, linear in age.
fn rise(remaining: f32, window_height: f32) -> f32 {
    let age = (1.0 - remaining / DAMAGE_NUMBER_LIFETIME).clamp(0.0, 1.0);
    age * DAMAGE_NUMBER_RISE * window_height
}

fn draw_damage_numbers(
    mut contexts: EguiContexts,
    numbers: Query<&DamageNumber>,
    camera: Query<(&Camera, &GlobalTransform), With<PlayerCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    if numbers.is_empty() {
        return;
    }
    let Ok((camera, view)) = camera.single() else { return };
    let Ok(window) = window.single() else { return };
    let Ok(ctx) = contexts.ctx_mut() else { return };

    let height = window.height();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("damage_numbers"),
    ));

    for number in &numbers {
        let Ok(screen) = camera.world_to_viewport(view, number.at) else { continue };

        let alpha = (opacity(number.remaining) * 255.0) as u8;
        painter.text(
            egui::pos2(screen.x, screen.y - rise(number.remaining, height)),
            egui::Align2::CENTER_CENTER,
            number.amount.to_string(),
            egui::FontId::new(DAMAGE_NUMBER_SIZE, crate::common::fonts::display_family()),
            egui::Color32::from_rgba_unmultiplied(255, 40, 40, alpha),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn a_hit(amount: u32, point: Vec3) -> DamageDealt {
        DamageDealt {
            target: Entity::from_raw_u32(1).unwrap(),
            source: DamageSource::Player(PlayerId(0)),
            amount,
            remaining: 70,
            point,
            from: None,
        }
    }

    /// Overkill is recorded as what actually came off, not what was swung.
    #[test]
    fn a_request_takes_health_off_and_records_what_it_took() {
        let mut world = World::new();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<DamageDealt>>();
        let target = world.spawn(Damageable::with_health(12)).id();
        world.write_message(Damage {
            target,
            source: DamageSource::Player(PlayerId(1)),
            amount: 90,
            point: Vec3::Y,
            from: None,
        });

        world.run_system_once(apply_damage).unwrap();

        assert_eq!(world.get::<Damageable>(target).unwrap().health(), 0);
        let messages = world.resource::<Messages<DamageDealt>>();
        let mut cursor = messages.get_cursor();
        let dealt: Vec<DamageDealt> = cursor.read(messages).copied().collect();
        assert_eq!(dealt.len(), 1);
        assert_eq!(dealt[0].amount, 12, "overkill was recorded as what was swung");
        assert_eq!(dealt[0].remaining, 0);
        assert_eq!(dealt[0].point, Vec3::Y);
    }

    /// A teammate takes no health and gets no record, so no zero is drawn.
    #[test]
    fn a_teammate_takes_nothing_and_is_not_even_recorded() {
        use crate::common::team::Team;

        let mut world = World::new();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<DamageDealt>>();
        let shooter = PlayerId(1);
        world.spawn((shooter, Team::Blue));
        let ally = world
            .spawn((Damageable::with_health(100), PlayerId(2), Team::Blue))
            .id();
        world.write_message(Damage {
            target: ally,
            source: DamageSource::Player(shooter),
            amount: 40,
            point: Vec3::ZERO,
            from: None,
        });

        world.run_system_once(apply_damage).unwrap();

        assert_eq!(world.get::<Damageable>(ally).unwrap().health(), 100);
        let messages = world.resource::<Messages<DamageDealt>>();
        let mut cursor = messages.get_cursor();
        assert_eq!(cursor.read(messages).count(), 0, "a number over a teammate");
    }

    /// Your own blast still hurts you; rocket jumping depends on it.
    #[test]
    fn your_own_splash_still_hurts_you() {
        use crate::common::team::Team;

        let mut world = World::new();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<DamageDealt>>();
        let id = PlayerId(1);
        let me = world.spawn((Damageable::with_health(100), id, Team::Blue)).id();
        world.write_message(Damage {
            target: me,
            source: DamageSource::Player(id),
            amount: 40,
            point: Vec3::ZERO,
            from: None,
        });

        world.run_system_once(apply_damage).unwrap();

        assert_eq!(world.get::<Damageable>(me).unwrap().health(), 60);
    }

    #[test]
    fn armor_soaks_most_of_a_hit_and_the_record_is_the_whole_hit() {
        let mut world = World::new();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<DamageDealt>>();
        let mut armor = Armor::default();
        armor.add(50);
        let target = world.spawn((Damageable::with_health(100), armor)).id();
        world.write_message(Damage { target, source: DamageSource::World, amount: 30, point: Vec3::ZERO, from: None });

        world.run_system_once(apply_damage).unwrap();

        assert_eq!(world.get::<Armor>(target).unwrap().points(), 30);
        assert_eq!(world.get::<Damageable>(target).unwrap().health(), 90);
        let messages = world.resource::<Messages<DamageDealt>>();
        let mut cursor = messages.get_cursor();
        let dealt: Vec<DamageDealt> = cursor.read(messages).copied().collect();
        assert_eq!((dealt[0].amount, dealt[0].remaining), (30, 90));
    }

    /// A target with no health is skipped, not an error.
    #[test]
    fn a_target_with_no_health_is_left_alone() {
        let mut world = World::new();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<DamageDealt>>();
        let dressing = world.spawn(Name::new("Scenery")).id();
        world.write_message(Damage {
            target: dressing,
            source: DamageSource::World,
            amount: 40,
            point: Vec3::ZERO,
            from: None,
        });

        world.run_system_once(apply_damage).unwrap();

        let messages = world.resource::<Messages<DamageDealt>>();
        let mut cursor = messages.get_cursor();
        assert_eq!(cursor.read(messages).count(), 0);
    }

    /// Deal, apply, log and reap in one tick, with the kill credited. Nothing
    /// in the types holds this order together.
    #[test]
    fn one_tick_carries_a_blast_all_the_way_to_a_credited_kill() {
        use crate::common::damage::{DamageLog, Explosion};
        use crate::common::hitbox::{Box3, Hitboxes};
        use crate::common::skeleton::AnimationClock;
        use crate::game::collision::CollisionWorld;
        use crate::game::explosion::explode;

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.init_state::<AppMode>();
        app.init_resource::<CollisionWorld>();
        app.init_resource::<AnimationClock>();
        app.add_plugins(DamagePlugin);
        // `explode` rather than `ExplosionPlugin`, which needs a renderer.
        app.add_message::<Explosion>();
        // Written by `explode` and read by `RagdollPlugin`, which is not here.
        app.add_message::<crate::game::ragdoll::RagdollShove>();
        app.add_systems(FixedUpdate, explode.in_set(DamageSystems::Deal));
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();

        let center = Vec3::new(0.0, 1.0, 0.0);
        let victim = app
            .world_mut()
            .spawn((
                Damageable::with_health(20),
                DamageLog::default(),
                Hitboxes {
                    body: Box3 { center, half_extents: Vec3::splat(0.5) },
                    head: Some(Box3 { center, half_extents: Vec3::splat(0.15) }),
                },
                Name::new("Bystander"),
            ))
            .id();
        app.world_mut().write_message(Explosion {
            at: center,
            radius: 4.0,
            damage: 60,
            knockback: 10.0,
            source: DamageSource::Player(PlayerId(2)),
            direct: None,
        });

        app.world_mut().run_schedule(FixedUpdate);

        assert!(app.world().get_entity(victim).is_err(), "the blast never reaped it");
        let messages = app.world().resource::<Messages<Died>>();
        let mut cursor = messages.get_cursor();
        let deaths: Vec<Died> = cursor.read(messages).cloned().collect();
        assert_eq!(deaths.len(), 1, "{deaths:?}");
        assert_eq!(deaths[0].killer, DamageSource::Player(PlayerId(2)));
    }

    /// A hit leaves a number at the world hit point.
    #[test]
    fn a_hit_leaves_a_number_at_the_hit_point() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageDealt>>();
        world.spawn((LocalPlayer, PlayerId(0)));
        let point = Vec3::new(1.0, 2.0, 3.0);
        world.write_message(a_hit(30, point));

        world.run_system_once(spawn_damage_numbers).unwrap();

        let number = world.query::<&DamageNumber>().single(&world).unwrap();
        assert_eq!(number.amount, 30);
        assert_eq!(number.at, point);
        assert_eq!(number.remaining, DAMAGE_NUMBER_LIFETIME);
    }

    #[test]
    fn somebody_elses_hit_and_our_own_hurt_leave_no_number() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageDealt>>();
        let us = world.spawn((LocalPlayer, PlayerId(5))).id();
        world.write_message(a_hit(30, Vec3::ZERO));
        world.write_message(DamageDealt { target: us, source: DamageSource::Player(PlayerId(5)), ..a_hit(30, Vec3::ZERO) });

        world.run_system_once(spawn_damage_numbers).unwrap();
        assert_eq!(world.query::<&DamageNumber>().iter(&world).count(), 0);
    }

    /// A faded number is despawned.
    #[test]
    fn a_number_is_despawned_once_it_has_faded() {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.spawn(DamageNumber { amount: 30, at: Vec3::ZERO, remaining: 0.05 });

        // Two ticks of a 60 Hz frame is more than the sliver left.
        for _ in 0..2 {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_millis(33));
            world.run_system_once(fade_damage_numbers).unwrap();
        }

        assert_eq!(world.query::<&DamageNumber>().iter(&world).count(), 0);
    }

    /// Full when it appears, gone when it expires.
    #[test]
    fn a_number_fades_over_its_lifetime() {
        assert_eq!(opacity(DAMAGE_NUMBER_LIFETIME), 1.0);
        assert_eq!(opacity(0.0), 0.0);
        assert!(opacity(DAMAGE_NUMBER_LIFETIME * 0.5) < opacity(DAMAGE_NUMBER_LIFETIME));
    }

    /// A number climbs a fifth of the screen at constant speed.
    #[test]
    fn a_number_climbs_a_fifth_of_the_screen_at_a_constant_speed() {
        let height = 1000.0;
        assert_eq!(rise(DAMAGE_NUMBER_LIFETIME, height), 0.0);
        assert!((rise(0.0, height) - 200.0).abs() < 1e-3);

        // Constant speed: equal slices of life are equal distances.
        let quarter = DAMAGE_NUMBER_LIFETIME * 0.25;
        let first = rise(DAMAGE_NUMBER_LIFETIME - quarter, height) - rise(DAMAGE_NUMBER_LIFETIME, height);
        let last = rise(0.0, height) - rise(quarter, height);
        assert!((first - last).abs() < 1e-3, "{first} != {last}");
    }
}
