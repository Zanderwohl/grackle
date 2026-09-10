//! What a hit leads to: a red number where it landed, and — once there is no
//! health left — the body going away. The numbers are here; the reaping is in
//! [`crate::game::death`], wired up from this plugin because this is where
//! [`DamageDealt`] is registered and where the ordering against it belongs.
//!
//! Reads [`DamageDealt`] rather than being called by the thing that shot, so
//! the day a rocket, a fall or a raised floor deals damage they get numbers
//! without knowing this module exists.
//!
//! The number is anchored to a **world point** and projected every frame, not
//! pinned to a screen position when it was born. A number frozen in screen
//! space slides off the body that took the hit the moment the camera moves,
//! which is exactly when you are looking at it.
//!
//! Everything here works in **logical points**: that is what egui lays out in,
//! and it is also what `world_to_viewport` gives back, since it maps onto the
//! camera's logical viewport rect. There is no scale factor to apply, and
//! applying one anyway is invisible until somebody runs it on a retina display.
//!
//! It is drawn at a fixed size in logical points, so a hit reads the same
//! whether it landed on a body across the room or one in your face — distance
//! is what the body behind it is for. The drift upwards is screen-space for
//! the same reason: a fifth of the screen is a fifth of the screen at any
//! range, where a metre of world rise would be invisible at fifty of them.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::AppMode;
use crate::common::net::has_authority;
use crate::common::damage::{Damage, DamageDealt, Damageable, Died};
use crate::common::team::Allegiances;
use crate::game::death::{announce_deaths, announce_the_dead, reap_the_dead, record_damage};
use crate::game::player::PlayerCamera;

/// How long a damage number lasts, in seconds.
pub const DAMAGE_NUMBER_LIFETIME: f32 = 2.0;

/// How big the number is drawn, in logical points.
const DAMAGE_NUMBER_SIZE: f32 = 48.0;

/// How far a number climbs over its whole life, as a fraction of the window
/// height. It travels at a constant speed and expires at the top of that
/// climb, so lifetime and this together are the velocity.
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

/// The order a tick's damage goes through, as sets rather than as named
/// systems.
///
/// This is what makes damage sources extensible. Before it, everything that
/// could hurt something had to be named by `DamagePlugin` so the resolution
/// could be ordered after it, which meant adding a weapon meant editing this
/// module — and forgetting to produced no error, just a kill credited to
/// nobody every so often. A new source now joins [`DamageSystems::Deal`] and
/// says nothing about what happens next.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DamageSystems {
    /// Everything that writes [`Damage`]: a shot fired, an explosion, a fall,
    /// a floor raised onto somebody.
    Deal,
    /// [`apply_damage`] — requests become health taken off and records
    /// written.
    Apply,
    /// What the records lead to: the log, and then the reaping.
    Resolve,
}

pub struct DamagePlugin;

impl Plugin for DamagePlugin {
    fn build(&self, app: &mut App) {
        app
            // Registered here, by the reader, because this is the module that
            // outlives any one thing that writes it.
            .add_message::<Damage>()
            // Beside the damage records and for the same reason: the queue
            // belongs to the module that outlives the things writing into it.
            // Every visual reads this one, filled locally or off the wire.
            .add_message::<crate::common::effects::Effect>()
            .add_message::<DamageDealt>()
            .add_message::<Died>()
            // On the tick, in this order, in the same tick as the shot. A
            // body reaped before its log is written is a kill credited to
            // nobody — see [`crate::game::death`].
            // **Only where this process is believed.** Damage is decided
            // once, by the server, and every client is told the result:
            // `Damageable` is replicated. A client that resolved damage for
            // itself would take a body to zero on a shot the server never
            // agreed landed, drop a corpse, and then be told the body is
            // alive — and the two answers would differ on every client.
            //
            // Nothing here is predicted, deliberately. Guessing a kill and
            // being wrong is a body that falls over and stands back up, which
            // is worse to watch than a kill that arrives a round-trip late.
            .configure_sets(
                FixedUpdate,
                (DamageSystems::Deal, DamageSystems::Apply, DamageSystems::Resolve)
                    .chain()
                    .run_if(in_state(AppMode::Play))
                    .run_if(has_authority),
            )
            .add_systems(FixedUpdate, apply_damage.in_set(DamageSystems::Apply))
            .add_systems(
                FixedUpdate,
                // `announce_the_dead` marks and says so; `reap_the_dead`
                // removes what was marked on an *earlier* tick. The gap is
                // what gives replication a tick to carry the zero health that
                // every other machine raises its own corpse from.
                (record_damage, announce_the_dead, reap_the_dead)
                    .chain()
                    .in_set(DamageSystems::Resolve),
            )
            .add_systems(Update, (spawn_damage_numbers, fade_damage_numbers, announce_deaths).chain())
            .add_systems(
                EguiPrimaryContextPass,
                draw_damage_numbers.run_if(in_state(AppMode::Play)),
            )
            // Numbers belong to the match, not to the editor: F5 out and the
            // last shot's feedback should not still be hanging in a viewport.
            .add_systems(OnExit(AppMode::Play), clear_damage_numbers)
        ;
    }
}

/// Turn every request to hurt something into health taken off and a record of
/// it.
///
/// The one place that touches a health pool. Every weapon used to do this for
/// itself — look the target up, apply, remember to report the *clamped* number
/// rather than the one it swung — and the interesting failure was never a
/// crash: it was the second weapon reporting overkill as damage dealt, and a
/// scoreboard adding up to more health than the map contains.
///
/// A target with no [`Damageable`] is skipped rather than an error. A shot
/// that stopped on a wall dressing still stopped, and an explosion sweeps up
/// whatever is nearby without asking first whether it bleeds.
///
/// **This is also where teams are enforced**, for the same reason: a request
/// to hurt a teammate is dropped here rather than never written, so a weapon
/// does not have to know whose side anything is on and cannot get it wrong on
/// its own. Dropped means dropped entirely — no health comes off and no
/// [`DamageDealt`] is written, so there is no number over a teammate's head
/// reading zero. See [`crate::common::team`] for the rule, self-damage
/// included.
pub fn apply_damage(
    mut requests: MessageReader<Damage>,
    mut health: Query<&mut Damageable>,
    allegiances: Allegiances,
    mut dealt: MessageWriter<DamageDealt>,
) {
    for request in requests.read() {
        if !allegiances.may_hurt(request.source, request.target) {
            continue;
        }
        let Ok(mut target) = health.get_mut(request.target) else { continue };

        // Nothing happens at zero — see `Damageable::apply`. The record is
        // still written, worth nothing, which is what lets a debug harness
        // keep shooting a body it has already emptied.
        let amount = target.apply(request.amount);
        dealt.write(DamageDealt {
            target: request.target,
            source: request.source,
            amount,
            remaining: target.health(),
            point: request.point,
        });
    }
}

fn spawn_damage_numbers(mut commands: Commands, mut hits: MessageReader<DamageDealt>) {
    for hit in hits.read() {
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

/// How opaque a number is, given how much of its life is left.
///
/// Linear, and its own function so that a curve — a hold and then a fall, a
/// drift upwards — is a change here rather than a change threaded through the
/// drawing.
fn opacity(remaining: f32) -> f32 {
    (remaining / DAMAGE_NUMBER_LIFETIME).clamp(0.0, 1.0)
}

/// How far above its hit point a number has climbed, in logical points, given
/// how much of its life is left and how tall the window is.
///
/// Linear in age: zero the frame it appears, the full rise the frame it goes.
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

    // `world_to_viewport` maps onto the camera's *logical* viewport rect, which
    // is the same space egui lays out in — so the projection is already in
    // points and must not be divided by the scale factor again. Doing that put
    // every number in the top-left quadrant of a retina display.
    let height = window.height();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("damage_numbers"),
    ));

    for number in &numbers {
        // Behind the camera, or outside the viewport, and there is nowhere to
        // draw it — a hit you have already turned away from.
        let Ok(screen) = camera.world_to_viewport(view, number.at) else { continue };

        let alpha = (opacity(number.remaining) * 255.0) as u8;
        painter.text(
            egui::pos2(screen.x, screen.y - rise(number.remaining, height)),
            egui::Align2::CENTER_CENTER,
            number.amount.to_string(),
            egui::FontId::proportional(DAMAGE_NUMBER_SIZE),
            egui::Color32::from_rgba_unmultiplied(255, 40, 40, alpha),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::damage::{DamageSource, PlayerId};

    fn a_hit(amount: u32, point: Vec3) -> DamageDealt {
        DamageDealt {
            target: Entity::from_raw_u32(1).unwrap(),
            source: DamageSource::Player(PlayerId(0)),
            amount,
            remaining: 70,
            point,
        }
    }

    /// The clamp lives in `Damageable`; this is the check that one place
    /// applies it and reports the number that actually came off rather than
    /// the one that was swung.
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

    /// The team gate, through the system that owns it. A teammate is skipped
    /// **entirely** — no health off and no record — because a `DamageDealt` of
    /// zero would put a number reading nothing over a friend's head every time
    /// you clipped them.
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
        });

        world.run_system_once(apply_damage).unwrap();

        assert_eq!(world.get::<Damageable>(ally).unwrap().health(), 100);
        let messages = world.resource::<Messages<DamageDealt>>();
        let mut cursor = messages.get_cursor();
        assert_eq!(cursor.read(messages).count(), 0, "a number over a teammate");
    }

    /// And the exception the rule exists alongside: your own blast is yours,
    /// even though you are trivially on your own team. Without this there is
    /// no rocket jumping.
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
        });

        world.run_system_once(apply_damage).unwrap();

        assert_eq!(world.get::<Damageable>(me).unwrap().health(), 60);
    }

    /// A target with no health is skipped rather than an error: an explosion
    /// sweeps up whatever is nearby without asking first whether it bleeds.
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
        });

        world.run_system_once(apply_damage).unwrap();

        let messages = world.resource::<Messages<DamageDealt>>();
        let mut cursor = messages.get_cursor();
        assert_eq!(cursor.read(messages).count(), 0);
    }

    /// The set ordering, end to end and in one tick: an explosion is dealt,
    /// applied, logged and reaped, and the kill is credited to whoever set it
    /// off. Nothing in the type system holds that order together — see
    /// [`DamageSystems`] — so it is worth a test that assembles the plugins
    /// and takes a step.
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
        // The system rather than `ExplosionPlugin`, which also draws — and
        // drawing wants a renderer this headless app has not got. What is
        // under test is the order, and `explode` is the part that is in it.
        app.add_message::<Explosion>();
        // Written by `explode` and read by `RagdollPlugin`, which is not here.
        app.add_message::<crate::game::ragdoll::RagdollShove>();
        app.add_systems(FixedUpdate, explode.in_set(DamageSystems::Deal));
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();

        let centre = Vec3::new(0.0, 1.0, 0.0);
        let victim = app
            .world_mut()
            .spawn((
                Damageable::with_health(20),
                DamageLog::default(),
                Hitboxes {
                    body: Box3 { centre, half_extents: Vec3::splat(0.5) },
                    head: Box3 { centre, half_extents: Vec3::splat(0.15) },
                },
                Name::new("Bystander"),
            ))
            .id();
        app.world_mut().write_message(Explosion {
            at: centre,
            radius: 4.0,
            damage: 60,
            knockback: 10.0,
            source: DamageSource::Player(PlayerId(2)),
            direct: None,
        });

        // One whole fixed step, driven by hand: what is under test is the
        // order inside the tick, not how the fixed loop decides to take one.
        app.world_mut().run_schedule(FixedUpdate);

        assert!(app.world().get_entity(victim).is_err(), "the blast never reaped it");
        let messages = app.world().resource::<Messages<Died>>();
        let mut cursor = messages.get_cursor();
        let deaths: Vec<Died> = cursor.read(messages).cloned().collect();
        assert_eq!(deaths.len(), 1, "{deaths:?}");
        assert_eq!(deaths[0].killer, DamageSource::Player(PlayerId(2)));
    }

    /// A hit puts a number where it landed, in the world rather than on the
    /// screen — the projection happens at drawing time, every frame.
    #[test]
    fn a_hit_leaves_a_number_at_the_hit_point() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageDealt>>();
        let point = Vec3::new(1.0, 2.0, 3.0);
        world.write_message(a_hit(30, point));

        world.run_system_once(spawn_damage_numbers).unwrap();

        let number = world.query::<&DamageNumber>().single(&world).unwrap();
        assert_eq!(number.amount, 30);
        assert_eq!(number.at, point);
        assert_eq!(number.remaining, DAMAGE_NUMBER_LIFETIME);
    }

    /// And it is gone by the time its two seconds are up, rather than sitting
    /// invisible in the world forever.
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

    /// It starts at the hit point and ends a fifth of the screen above it,
    /// having covered the ground at a constant speed rather than easing.
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
