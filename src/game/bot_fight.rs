//! How a bot fights: whom it shoots at, where it aims, when it pulls the trigger and with what.
//! The one writer of [`BotIntent`]'s `look_at`, `fire`, `alt_fire`, `select`, `reload` and
//! `strafe`; [`think`](crate::game::bot_think::think) decides where it goes meanwhile. It aims
//! only at what its [`BotSenses`] have in sight, turns at a limited rate, and its aim settles
//! the longer it tracks. See "Fight" in documentation/bot-brain.md.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::flame::FlameSpec;
use crate::common::hitbox::Hitboxes;
use crate::common::movement_rules::MovementRules;
use crate::common::net::has_authority;
use crate::common::projectile::ProjectileSpec;
use crate::common::rotation::wrap_angle;
use crate::common::weapon::{Ammo, Equipped, HitscanSpec, Mounted, WeaponAction, WeaponCatalog, WeaponId};
use crate::game::bot::{aim_at, BotIntent, BotOf};
use crate::game::bot_sense::{look, BotMind, BotSenses, Sense};
use crate::game::bot_think::think;
use crate::game::collision::CollisionWorld;
use crate::game::death::Dying;
use crate::game::player::{PhysicsBody, Player};
use crate::tool::nav_bake::NavBake;

/// Meters a body is wide, for how much of a pellet pattern lands on it and how close an aim
/// must be to hit.
const BODY_WIDTH: f32 = 0.8;
/// Seconds of flight at which a projectile is half as likely to land: a body sees it coming, and
/// the longer it flies the more it can step aside, so it counts squared.
const DODGE_SECONDS: f32 = 0.8;
/// Seconds of reload at which a weapon with an empty clip is worth half as much as loaded.
const EMPTY_CLIP_SECONDS: f32 = 0.4;
/// Meters at which a hitscan shot from a bot is half as likely to land as point blank. A splash
/// lands near enough without a perfect aim, a bullet does not.
const AIM_FALLOFF: f32 = 40.0;
/// A weapon is switched to only when it is this much better than the one held, since drawing
/// one costs its deploy.
const SWITCH_GAIN: f32 = 1.3;
/// Another enemy is taken as the target only when it is this much more wanted.
const RETARGET_GAIN: f32 = 1.5;
/// Of the aim error, per radian per second the target crosses the view.
const CROSSING_ERROR: f32 = 0.5;
/// Radians per second the direction of a bot's aim error drifts round.
const ERROR_DRIFT: f32 = 1.3;
/// Meters out from the eye `look_at` is put along the aim.
const LOOK_AHEAD: f32 = 10.0;
/// Seconds between a strafing bot's changes of side, the range each is rolled in.
const STRAFE_SECONDS: std::ops::Range<f32> = 0.4..1.1;
/// Meters to the side a bot checks there is floor before strafing that way.
const STRAFE_CHECK: f32 = 1.0;

/// Which enemy a bot would rather shoot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preference {
    Nearest,
    /// Whoever hurt it last.
    Attacker,
    /// Whoever is looking its way: the one about to shoot.
    Facing,
}

/// How well a bot shoots, rolled per seat inside its [`BotMind`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    /// Radians per second its aim turns at most.
    pub turn_rate: f32,
    /// Radians off it starts on a target it has just found.
    pub error: f32,
    /// Seconds of tracking for the error to fall to a third of that.
    pub settle: f32,
    pub preference: Preference,
}

impl Default for Aim {
    fn default() -> Self {
        Aim { turn_rate: 6.0, error: 0.08, settle: 0.5, preference: Preference::Nearest }
    }
}

const TURN_RATE: std::ops::Range<f32> = 3.5..9.0;
const AIM_ERROR: std::ops::Range<f32> = 0.03..0.14;
const SETTLE: std::ops::Range<f32> = 0.3..0.9;

impl Aim {
    pub fn temperament() -> Self {
        let preference = [Preference::Nearest, Preference::Attacker, Preference::Facing][rand::random_range(0..3)];
        Aim {
            turn_rate: rand::random_range(TURN_RATE),
            error: rand::random_range(AIM_ERROR),
            settle: rand::random_range(SETTLE),
            preference,
        }
    }
}

/// Where a bot is aiming and what it is aiming at. Its own, so aiming turns at a rate rather than
/// jumping, and kept between ticks so the error settles as it tracks.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct BotAim {
    pub target: Option<Entity>,
    /// Radians, in the step's convention.
    yaw: f32,
    pitch: f32,
    /// Seconds the target has been tracked.
    tracked: f32,
    /// The direction the error leans, in radians round the aim.
    lean: f32,
    /// Which way the target's body lay last tick, for how fast it crosses the view.
    last_bearing: Option<(f32, f32)>,
    /// `-1` left, `1` right, and seconds until it changes.
    strafe: f32,
    strafe_for: f32,
}

/// Turns `(yaw, pitch)` toward `(to_yaw, to_pitch)` by at most `step` radians, the short way.
pub fn turn_toward(from: (f32, f32), to: (f32, f32), step: f32) -> (f32, f32) {
    let gap = Vec2::new(wrap_angle(to.0 - from.0), to.1 - from.1);
    let length = gap.length();
    if length <= step {
        return (from.0 + gap.x, to.1);
    }
    let moved = gap / length * step;
    (from.0 + moved.x, from.1 + moved.y)
}

/// Radians between two aims.
fn angle_between(a: (f32, f32), b: (f32, f32)) -> f32 {
    look(a.0, a.1).angle_between(look(b.0, b.1))
}

/// How much one button of a weapon is worth against a body `distance` meters away, as rough
/// damage per second. Zero for what cannot reach or would hurt the bot more than its target.
pub fn worth(mounted: &Mounted, distance: f32) -> f32 {
    let interval = mounted.cadence.interval().max(1.0 / 64.0);
    match mounted.action {
        WeaponAction::Hitscan(HitscanSpec { range_m, damage, pellets, .. }) => {
            if distance > range_m {
                return 0.0;
            }
            let landed = match pellets {
                None => 1.0,
                Some(grid) => {
                    let pattern = distance / grid.spread;
                    let share = (BODY_WIDTH / pattern.max(1e-3)).min(1.0);
                    (grid.across * grid.across) as f32 * share * share
                }
            };
            damage as f32 * landed / (1.0 + distance / AIM_FALLOFF) / interval
        }
        WeaponAction::Projectile(spec) => {
            // Inside its own blast the shooter takes the splash too.
            if distance < spec.explosion_radius * 1.2 {
                return 0.0;
            }
            let flight = distance / spec.initial_speed.max(1e-3);
            spec.damage_direct_hit.max(spec.damage_splash) as f32 / (1.0 + (flight / DODGE_SECONDS).powi(2)) / interval
        }
        WeaponAction::Flame(FlameSpec { range_m, damage, afterburn_damage, afterburn_interval_s, .. }) => {
            if distance > range_m {
                return 0.0;
            }
            damage as f32 / interval + afterburn_damage as f32 / afterburn_interval_s.max(1e-3)
        }
        _ => 0.0,
    }
}

/// A carried weapon against a body `distance` away: its better button and what that is worth,
/// nothing with no ammo at all, and less while its clip is empty by the reload it needs first.
fn armed(weapon: WeaponId, ammo: &Ammo, catalog: &WeaponCatalog, distance: f32) -> Option<(WeaponId, bool, f32)> {
    let state = ammo.at(weapon).filter(|state| state.clip + state.reserve > 0)?;
    let spec = catalog.get(weapon)?;
    let (primary, secondary) = (worth(&spec.primary, distance), worth(&spec.secondary, distance));
    let (alt, value) = if secondary > primary { (true, secondary) } else { (false, primary) };
    let reload = if spec.magazine.clip.is_some() && state.clip == 0 { spec.magazine.reload } else { 0.0 };
    Some((weapon, alt, value / (1.0 + reload / EMPTY_CLIP_SECONDS))).filter(|(.., value)| *value > 0.0)
}

/// The carried weapon best against a body `distance` away, and which button.
pub fn best_weapon(carried: &[WeaponId], ammo: &Ammo, catalog: &WeaponCatalog, distance: f32) -> Option<(WeaponId, bool, f32)> {
    carried.iter().filter_map(|weapon| armed(*weapon, ammo, catalog, distance)).max_by(|a, b| a.2.total_cmp(&b.2))
}

/// Where to put a shot at a body: led by the projectile's flight, lifted for its fall, and at
/// the feet when a splash will reach them and they can be seen.
#[allow(clippy::too_many_arguments)]
pub fn aim_point(world: &CollisionWorld, eye: Vec3, center: Vec3, feet: Vec3, velocity: Vec3, action: &WeaponAction, gravity: f32) -> Vec3 {
    let WeaponAction::Projectile(ProjectileSpec { initial_speed, gravity_factor, explosion_radius, .. }) = *action else {
        return center;
    };
    let splash = explosion_radius > 0.0 && world.segment_clear(eye, feet + Vec3::Y * 0.1);
    let at = if splash { feet + Vec3::Y * 0.1 } else { center };
    // Two rounds of where it will be when the shot gets there.
    let mut flight = eye.distance(at) / initial_speed.max(1e-3);
    let mut led = at;
    for _ in 0..2 {
        led = at + Vec3::new(velocity.x, 0.0, velocity.z) * flight;
        flight = eye.distance(led) / initial_speed.max(1e-3);
    }
    led + Vec3::Y * 0.5 * gravity * gravity_factor * flight * flight
}

/// Whether a shot along `aim` would hit what it is meant for: within `tolerance` of the true
/// aim, with nothing in the way, and a splash not going off on the shooter.
pub fn would_land(world: &CollisionWorld, eye: Vec3, aim: (f32, f32), point: Vec3, tolerance: f32, splash: f32) -> bool {
    let wanted = aim_at(eye, point);
    if angle_between(aim, wanted) > tolerance || !world.segment_clear(eye, point) {
        return false;
    }
    let ray = Ray3d::new(eye, Dir3::new(look(aim.0, aim.1)).unwrap_or(Dir3::NEG_Z));
    splash <= 0.0 || world.ray_distance(&ray, splash * 1.2).is_none()
}

/// The angle off a body `distance` away that still hits it, for this action.
fn tolerance(action: &WeaponAction, distance: f32) -> f32 {
    let radius = match action {
        WeaponAction::Projectile(spec) => (spec.explosion_radius * 0.5).max(BODY_WIDTH * 0.5),
        WeaponAction::Flame(spec) => return spec.spread,
        _ => BODY_WIDTH * 0.5,
    };
    (radius / distance.max(0.5)).atan()
}

fn splash_of(action: &WeaponAction) -> f32 {
    match action {
        WeaponAction::Projectile(spec) => spec.explosion_radius,
        _ => 0.0,
    }
}

#[allow(clippy::type_complexity)]
pub fn fight(
    time: Res<Time>,
    world: Res<CollisionWorld>,
    catalog: Option<Res<WeaponCatalog>>,
    rules: Option<Res<MovementRules>>,
    nav: Option<Res<NavBake>>,
    bodies: Query<(&PhysicsBody, &Stance, &Player, &Hitboxes), Without<Dying>>,
    mut bots: Query<(Entity, &BotSenses, &BotMind, Option<(&Equipped, &Ammo)>, &mut BotIntent, &mut BotAim), With<BotOf>>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let gravity = rules.as_deref().map_or(MovementRules::default().gravity, |rules| rules.gravity);
    for (bot, senses, mind, arms, mut intent, mut aim) in &mut bots {
        let Ok((body, stance, player, _)) = bodies.get(bot) else { continue };
        let eye = body.current + Vec3::Y * player.duck.eye_from_center(*stance);
        let skill = mind.aim;

        // Whom: an enemy in sight, by temperament, the one already tracked kept unless another
        // is much more wanted.
        let wanted = |entity: Entity, how: Sense| -> Option<f32> {
            let (them, _, their, _) = bodies.get(entity).ok()?;
            let distance = eye.distance(them.current).max(1.0);
            Some(match skill.preference {
                Preference::Nearest => 1.0 / distance,
                Preference::Attacker => (if how == Sense::Hurt { 2.0 } else { 1.0 }) / distance,
                Preference::Facing => {
                    let toward = aim_at(them.current, eye);
                    (1.0 + (1.0 - angle_between((their.yaw, their.pitch), toward) / std::f32::consts::PI)) / distance
                }
            })
        };
        let seen: Vec<(Entity, f32)> = senses
            .noticed(now, mind)
            .filter(|known| known.in_sight)
            .filter_map(|known| Some((known.body, wanted(known.body, known.how)?)))
            .collect();
        let held = aim.target.and_then(|target| seen.iter().find(|(entity, _)| *entity == target).copied());
        let best = seen.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1));
        let target = match (held, best) {
            (Some(held), Some(best)) if best.1 > held.1 * RETARGET_GAIN => Some(best.0),
            (Some(held), _) => Some(held.0),
            (None, best) => best.map(|best| best.0),
        };
        if target != aim.target {
            aim.target = target;
            aim.tracked = 0.0;
            aim.last_bearing = None;
            aim.lean = rand::random_range(0.0..std::f32::consts::TAU);
            (aim.yaw, aim.pitch) = (player.yaw, player.pitch);
        }

        let Some((them, their_stance, their, boxes)) = target.and_then(|target| bodies.get(target).ok()) else {
            // Nothing to shoot: look where it walks, keep its hands free, and top up a clip.
            intent.look_at = None;
            intent.fire = false;
            intent.alt_fire = false;
            intent.strafe = 0.0;
            intent.select = None;
            intent.reload = arms.is_some_and(|(equipped, ammo)| {
                equipped.held().and_then(|held| Some((catalog.as_deref()?.get(held)?.magazine.clip?, ammo.at(held)?))).is_some_and(|(clip, state)| state.clip * 2 < clip && state.reserve > 0)
            });
            (aim.yaw, aim.pitch) = (player.yaw, player.pitch);
            continue;
        };
        intent.reload = false;
        aim.tracked += dt;
        let center = boxes.body.center;
        let feet = them.current - Vec3::Y * their_stance.half_extents().y;
        let distance = eye.distance(center);

        // With what.
        let choice = arms.zip(catalog.as_deref()).and_then(|((equipped, ammo), catalog)| {
            let best = best_weapon(equipped.carried(), ammo, catalog, distance)?;
            let held = equipped.held().and_then(|held| armed(held, ammo, catalog, distance));
            let pick = match held {
                Some(held) if best.2 <= held.2 * SWITCH_GAIN => held,
                _ => best,
            };
            let action = if pick.1 { catalog.get(pick.0)?.secondary.action } else { catalog.get(pick.0)?.primary.action };
            Some((pick.0, pick.1, action, equipped.held() == Some(pick.0)))
        });
        intent.select = choice.filter(|(_, _, _, holding)| !holding).map(|(weapon, ..)| weapon);

        // Where: the point to put the shot, off by an error that settles as it tracks and grows
        // as the target crosses the view.
        let action = choice.map_or(WeaponAction::None, |(_, _, action, _)| action);
        let point = aim_point(&world, eye, center, feet, their.velocity, &action, gravity);
        let true_aim = aim_at(eye, point);
        // Off the body rather than the point, which jumps between feet and chest as the feet go
        // in and out of view.
        let bearing = aim_at(eye, center);
        let crossing = aim.last_bearing.map_or(0.0, |last| angle_between(last, bearing) / dt.max(1e-3));
        aim.last_bearing = Some(bearing);
        aim.lean += ERROR_DRIFT * dt;
        let error = skill.error * ((-aim.tracked / skill.settle).exp() + CROSSING_ERROR * crossing);
        let off = (true_aim.0 + error * aim.lean.cos(), true_aim.1 + error * aim.lean.sin());
        (aim.yaw, aim.pitch) = turn_toward((aim.yaw, aim.pitch), off, skill.turn_rate * dt);
        intent.look_at = Some(eye + look(aim.yaw, aim.pitch) * LOOK_AHEAD);

        // When.
        let ready = choice.is_some_and(|(.., holding)| holding);
        let lands = ready && would_land(&world, eye, (aim.yaw, aim.pitch), point, tolerance(&action, distance), splash_of(&action));
        let alt = choice.is_some_and(|(_, alt, ..)| alt);
        intent.fire = lands && !alt;
        intent.alt_fire = lands && alt;

        // Moving while fighting: side to side, onto floor the graph has and away from harm.
        aim.strafe_for -= dt;
        if aim.strafe_for <= 0.0 || aim.strafe == 0.0 {
            aim.strafe = if rand::random::<bool>() { 1.0 } else { -1.0 };
            aim.strafe_for = rand::random_range(STRAFE_SECONDS);
        }
        let right = Vec3::new(aim.yaw.cos(), 0.0, -aim.yaw.sin());
        let standing = body.current - Vec3::Y * stance.half_extents().y;
        let safe = |side: f32| {
            nav.as_deref().is_some_and(|nav| {
                nav.graph().cell_at(standing + right * side * STRAFE_CHECK).is_some_and(|cell| nav.graph().cells()[cell as usize].hurt == 0)
            })
        };
        if !safe(aim.strafe) {
            aim.strafe = -aim.strafe;
        }
        intent.strafe = if safe(aim.strafe) { aim.strafe } else { 0.0 };
    }
}

pub struct BotFightPlugin;

impl Plugin for BotFightPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, fight.after(think).run_if(has_authority).run_if(in_state(AppMode::Play)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon_file::default_catalog;
    use crate::common::world_surface::Volume;
    use crate::tool::room::Room;

    fn catalog() -> WeaponCatalog {
        default_catalog()
    }

    fn key(catalog: &WeaponCatalog, key: &str) -> WeaponId {
        catalog.weapons.iter().find(|weapon| weapon.key == key).unwrap().id
    }

    fn full(catalog: &WeaponCatalog, weapons: &[WeaponId]) -> Ammo {
        let mut ammo = Ammo::default();
        for weapon in weapons {
            let spec = catalog.get(*weapon).unwrap();
            let state = ammo.at_mut(*weapon);
            state.clip = spec.magazine.clip.unwrap_or(0);
            state.reserve = spec.magazine.reserve;
        }
        ammo
    }

    /// Breaks if a bot's aim jumps rather than turns: a quarter turn away, one tick moves it by
    /// its turn rate and no further, and a small gap closes outright.
    #[test]
    fn a_bot_turns_onto_a_target_at_its_turn_rate() {
        let step = Aim::default().turn_rate / 64.0;
        let (yaw, pitch) = turn_toward((0.0, 0.0), (std::f32::consts::FRAC_PI_2, 0.0), step);
        assert!((yaw - step).abs() < 1e-5 && pitch.abs() < 1e-6, "{yaw}");
        assert_eq!(turn_toward((0.0, 0.0), (step * 0.5, 0.0), step), (step * 0.5, 0.0));
        // The short way round across the back.
        let (yaw, _) = turn_toward((3.0, 0.0), (-3.0, 0.0), step);
        assert!(yaw > 3.0);
    }

    /// Breaks if a bot fires into a wall: with a pillar between it and the target it holds, and
    /// with the pillar gone it shoots.
    #[test]
    fn a_bot_holds_fire_with_a_wall_between() {
        let open = Room::new(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 6.0, 20.0));
        let mut walled = CollisionWorld::default();
        walled.rebuild_volumes(&[Volume::air(open.clone()), Volume::solid(Room::new(Vec3::new(-2.0, 0.0, -6.0), Vec3::new(2.0, 6.0, -5.0)))]);
        let mut clear = CollisionWorld::default();
        clear.rebuild_volumes(&[Volume::air(open)]);
        let (eye, point) = (Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.0, 1.0, -10.0));
        let aim = aim_at(eye, point);
        let rifle = WeaponAction::Hitscan(HitscanSpec { range_m: 100.0, damage: 10, headshot_multiplier: 1, shove_per_damage: 0.0, shove_radius_m: 0.0, pellets: None, kill_shove: None });
        let tolerance = tolerance(&rifle, 10.0);
        assert!(!would_land(&walled, eye, aim, point, tolerance, 0.0), "fired into the pillar");
        assert!(would_land(&clear, eye, aim, point, tolerance, 0.0));
        let wide = (aim.0 + 0.3, aim.1);
        assert!(!would_land(&clear, eye, wide, point, tolerance, 0.0), "fired well off the target");
    }

    /// Breaks if a bot never settles onto a still target at range: the worst aim there is, after
    /// three of its settle times, is inside the tolerance that hits.
    #[test]
    fn a_bot_settles_onto_a_still_target_at_range() {
        let skill = Aim { error: AIM_ERROR.end, settle: SETTLE.end, ..default() };
        let (eye, point) = (Vec3::new(0.0, 1.6, 0.0), Vec3::new(12.0, 1.0, -25.0));
        let wanted = aim_at(eye, point);
        let (mut current, mut lean) = ((0.0, 0.0), 1.0f32);
        let dt = 1.0 / 64.0;
        for tick in 0..(3.0 * skill.settle / dt) as usize {
            let tracked = tick as f32 * dt;
            lean += ERROR_DRIFT * dt;
            let error = skill.error * (-tracked / skill.settle).exp();
            current = turn_toward(current, (wanted.0 + error * lean.cos(), wanted.1 + error * lean.sin()), skill.turn_rate * dt);
        }
        let rifle = WeaponAction::Hitscan(HitscanSpec { range_m: 100.0, damage: 10, headshot_multiplier: 1, shove_per_damage: 0.0, shove_radius_m: 0.0, pellets: None, kill_shove: None });
        assert!(angle_between(current, wanted) <= tolerance(&rifle, eye.distance(point)), "still off by {}", angle_between(current, wanted));
    }

    /// Breaks if weapon choice ignores range: the shotgun close, the laser rifle far, and the
    /// rocket launcher between but never inside its own blast.
    #[test]
    fn a_bot_picks_its_weapon_by_range() {
        let catalog = catalog();
        let (shotgun, rifle, rockets) = (key(&catalog, "shotgun"), key(&catalog, "laser_rifle"), key(&catalog, "rocket_launcher"));
        let carried = [shotgun, rifle, rockets];
        let ammo = full(&catalog, &carried);
        let pick = |distance| best_weapon(&carried, &ammo, &catalog, distance).unwrap().0;
        assert_eq!(pick(2.0), shotgun);
        assert_eq!(pick(60.0), rifle);
        assert_eq!(pick(8.0), rockets);
        assert!(worth(&catalog.get(rockets).unwrap().primary, 2.0) == 0.0, "a rocket at its own feet");
    }

    /// Breaks if a bot holds on to a weapon it cannot fire, or swaps to one it must reload first:
    /// an empty weapon is never picked, and a launcher with an empty clip loses to a loaded pistol
    /// it beats when loaded.
    #[test]
    fn a_weapon_is_worth_only_the_ammo_it_has() {
        let catalog = catalog();
        let (shotgun, pistol, rockets) = (key(&catalog, "shotgun"), key(&catalog, "pistol"), key(&catalog, "rocket_launcher"));
        let mut ammo = full(&catalog, &[shotgun, pistol, rockets]);
        *ammo.at_mut(shotgun) = Default::default();
        assert_eq!(armed(shotgun, &ammo, &catalog, 10.0), None, "an empty shotgun is worth something");
        let carried = [pistol, rockets];
        assert_eq!(best_weapon(&carried, &ammo, &catalog, 8.0).unwrap().0, rockets);
        ammo.at_mut(rockets).clip = 0;
        assert_eq!(best_weapon(&carried, &ammo, &catalog, 8.0).unwrap().0, pistol, "swapped to a launcher it must reload");
    }

    /// Breaks if a rocket is aimed where its target is rather than where it will be, or if a
    /// falling shot is not lifted for its drop.
    #[test]
    fn a_projectile_is_led_and_lifted() {
        let mut world = CollisionWorld::default();
        world.rebuild_volumes(&[Volume::air(Room::new(Vec3::new(-40.0, 0.0, -40.0), Vec3::new(40.0, 6.0, 40.0)))]);
        let catalog = catalog();
        let rocket = catalog.get(key(&catalog, "rocket_launcher")).unwrap().primary.action;
        let rpg = catalog.get(key(&catalog, "rpg")).unwrap().primary.action;
        let (eye, center, feet) = (Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.0, 1.0, -20.0), Vec3::new(0.0, 0.0, -20.0));
        let running = Vec3::new(6.0, 0.0, 0.0);
        let led = aim_point(&world, eye, center, feet, running, &rocket, 9.8);
        assert!(led.x > 3.0, "not led: {led}");
        assert!(led.y < 0.5, "a splash not aimed at the feet: {led}");
        let lifted = aim_point(&world, eye, center, feet, Vec3::ZERO, &rpg, 9.8);
        assert!(lifted.y > 1.0, "a falling shot not lifted: {lifted}");
    }

    fn a_fight(volumes: &[Volume]) -> (World, Entity) {
        use crate::common::damage::PlayerId;
        use crate::common::hitbox::Box3;
        use crate::game::bot_sense::Known;
        let mut world = World::new();
        let mut collision = CollisionWorld::default();
        collision.rebuild_volumes(volumes);
        world.insert_resource(collision);
        world.init_resource::<Time>();
        let catalog = catalog();
        let rifle = key(&catalog, "laser_rifle");
        let ammo = full(&catalog, &[rifle]);
        world.insert_resource(catalog);
        let body = |at: Vec3| {
            let center = crate::common::class::body_center_from_feet(at);
            (
                PhysicsBody::at(center),
                Stance::Standing,
                Player::default(),
                Hitboxes { body: Box3 { center, half_extents: Stance::Standing.half_extents() }, head: None },
            )
        };
        let enemy = world.spawn(body(Vec3::new(6.0, 0.0, -15.0))).id();
        let mut senses = BotSenses::default();
        senses.known.push(Known { who: PlayerId(2), body: enemy, at: Vec3::new(6.0, 1.0, -15.0), how: Sense::Seen, last: 0.0, since: -1.0, in_sight: true });
        let bot = world
            .spawn((body(Vec3::ZERO), BotOf(Entity::PLACEHOLDER), senses, BotMind::default(), Equipped::carrying([rifle]), ammo, BotIntent::default(), BotAim::default()))
            .id();
        (world, bot)
    }

    fn fight_for(world: &mut World, seconds: f32) -> bool {
        use bevy::ecs::system::RunSystemOnce;
        let mut fired = false;
        for _ in 0..(seconds * 64.0) as usize {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(1.0 / 64.0));
            world.run_system_once(fight).unwrap();
            fired |= world.query::<&BotIntent>().iter(world).any(|intent| intent.fire);
        }
        fired
    }

    /// Breaks if the fight layer does not turn a bot onto somebody it sees and shoot, or shoots
    /// through a wall at them: in the open it looks their way and fires, with a pillar between
    /// it holds.
    #[test]
    fn a_bot_turns_on_what_it_sees_and_fires_only_with_a_clear_shot() {
        let open = Room::new(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 6.0, 20.0));
        let (mut world, bot) = a_fight(&[Volume::air(open.clone())]);
        assert!(fight_for(&mut world, 2.0), "never fired at somebody in the open");
        let look = world.get::<BotIntent>(bot).unwrap().look_at.unwrap();
        assert!(look.x > 0.0 && look.z < 0.0, "looked away from them: {look}");

        let pillar = Volume::solid(Room::new(Vec3::new(1.0, 0.0, -8.0), Vec3::new(4.0, 6.0, -7.0)));
        let (mut world, _) = a_fight(&[Volume::air(open), pillar]);
        assert!(!fight_for(&mut world, 2.0), "fired into the pillar");
    }

    /// Breaks if a bot fires a rocket into a wall right in front of it.
    #[test]
    fn a_splash_is_held_with_a_wall_inside_its_blast() {
        let mut world = CollisionWorld::default();
        world.rebuild_volumes(&[
            Volume::air(Room::new(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 6.0, 20.0))),
            Volume::solid(Room::new(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 6.0, -1.5))),
        ]);
        let eye = Vec3::new(0.0, 1.6, 0.0);
        // Aimed past the pillar's edge at somebody beyond, but the pillar is in the blast.
        let point = Vec3::new(6.0, 1.0, -10.0);
        assert!(!would_land(&world, eye, aim_at(eye, Vec3::new(1.5, 1.6, -2.0)), point, 1.0, 3.0));
    }
}
