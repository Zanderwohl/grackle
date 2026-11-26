//! How a bot fights: whom it shoots at, where it aims, when it pulls the trigger and with what.
//! The one writer of [`BotIntent`]'s `look_at`, `fire`, `alt_fire`, `select`, `reload` and
//! `strafe`; [`think`](crate::game::bot_think::think) decides where it goes meanwhile. It aims
//! only at what its [`BotSenses`] have in sight. See "Fight" in documentation/bot-brain.md.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::round_settings::{Better, BotSkill};
use crate::common::flame::FlameSpec;
use crate::common::hitbox::Hitboxes;
use crate::common::movement_rules::MovementRules;
use crate::common::net::has_authority;
use crate::common::projectile::ProjectileSpec;
use crate::common::rotation::wrap_angle;
use crate::common::weapon::{Ammo, Equipped, HitscanSpec, Mounted, Weapon, WeaponAction, WeaponCatalog, WeaponId};
use crate::game::bot::{aim_at, BotIntent, BotOf};
use crate::game::bot_sense::{in_view, look, BotMind, BotSenses, Sense};
use crate::game::bot_think::think;
use crate::game::collision::CollisionWorld;
use crate::game::death::Dying;
use crate::game::player::{PhysicsBody, Player};
use crate::game::projectile::Projectile;
use crate::game::sights::Sights;
use crate::common::team::Allegiances;
use crate::tool::nav_bake::NavBake;

/// Meters a body is wide, for how much of a pellet pattern lands on it and how close an aim
/// must be to hit.
const BODY_WIDTH: f32 = 0.8;
/// Seconds of flight at which a projectile is half as likely to land, counted squared: the longer
/// it flies, the further a body can step aside.
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
/// Radians per second the direction of a bot's aim error drifts round. It starts along the flick,
/// so a bot overshoots what it turns onto and comes back.
const ERROR_DRIFT: f32 = 0.5;
/// Meters out from the eye `look_at` is put along the aim.
const LOOK_AHEAD: f32 = 10.0;
/// Meters to the side a bot checks there is floor before strafing that way.
const STRAFE_CHECK: f32 = 1.0;
/// Meters from which a bot raises a scope at what it fights. Nearer, a target crosses the narrow
/// view too quickly, and the scoped primary is counted only from here out.
pub const SCOPE_FROM: f32 = 10.0;
/// Seconds a scoped bot keeps watching after it last saw what it fights, before it is done and
/// lowers the scope.
const SCOPE_LINGER: f32 = 3.0;
/// Seconds a bot leaves its scope down after a hit lowered it, so it can turn on whoever hit it.
const SCOPE_REST: f32 = 1.5;
/// Seconds of tracking a bot goes for the head before it settles for the body.
const HEAD_PATIENCE: f32 = 1.5;
/// Meters.
const HEAD_WIDTH: f32 = 0.3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Preference {
    Nearest,
    /// Whoever hurt it last.
    Attacker,
    /// Whoever is looking its way: the one about to shoot.
    Facing,
}

/// How well a bot shoots, rolled per seat inside its [`BotMind`].
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Aim {
    /// Radians per second its aim turns at most.
    pub turn_rate: f32,
    /// Radians off it starts on a target it has just found.
    pub error: f32,
    /// Seconds of tracking for the error to fall to a third of that.
    pub settle: f32,
    /// Seconds for the aim to close most of a small gap, so it slows as it arrives rather than
    /// stopping dead.
    pub ease: f32,
    /// The share of a target's motion across the view the aim follows by itself, 0 to 1; the
    /// rest it catches up on, so a bot tracking a runner trails them a little.
    pub tracking: f32,
    pub preference: Preference,
    /// The chance, per target, that it goes for the head with a weapon that pays for one, 0 to 1.
    pub headhunting: f32,
}

impl Default for Aim {
    fn default() -> Self {
        Aim { turn_rate: 6.0, error: 0.08, settle: 0.5, ease: 0.1, tracking: 0.75, preference: Preference::Nearest, headhunting: 0.5 }
    }
}

// Hard rolls aim error at or just under 0.09 to 0.11 rad, where results drop off
// (documentation/bot-brain.md).
const TURN_RATE: std::ops::Range<f32> = 3.0..8.0;
const AIM_ERROR: std::ops::Range<f32> = 0.06..0.17;
const SETTLE: std::ops::Range<f32> = 0.5..1.1;
const EASE: std::ops::Range<f32> = 0.06..0.16;
const TRACKING: std::ops::Range<f32> = 0.45..0.8;
/// Not narrowed by skill: no round has measured what it is worth.
const HEADHUNTING: std::ops::Range<f32> = 0.2..0.8;

impl Aim {
    pub fn temperament(skill: BotSkill) -> Self {
        let preference = [Preference::Nearest, Preference::Attacker, Preference::Facing][rand::random_range(0..3)];
        Aim {
            turn_rate: rand::random_range(skill.within(TURN_RATE, Better::Higher)),
            error: rand::random_range(skill.within(AIM_ERROR, Better::Lower)),
            settle: rand::random_range(skill.within(SETTLE, Better::Lower)),
            ease: rand::random_range(skill.within(EASE, Better::Lower)),
            tracking: rand::random_range(skill.within(TRACKING, Better::Higher)),
            preference,
            headhunting: rand::random_range(HEADHUNTING),
        }
    }
}

/// How a bot moves while it fights, rolled per seat inside its [`BotMind`]. A fight is legs: a
/// strafe one way, or drifting slowly while it aims, each held for a while. A full reversal takes a
/// body about a fifth of a second, so legs much shorter than a second read as jittering, not
/// dodging.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Footwork {
    /// The share of legs spent strafing rather than drifting, 0 to 1.
    pub restlessness: f32,
    /// Seconds a leg lasts on average; each is rolled between 0.6 and 1.4 times it.
    pub stride: f32,
    /// The chance that a strafe goes back the way the last one came, rather than on.
    pub reversal: f32,
    /// The share of the held weapon's best damage a bot insists on, 0 to 1: it keeps as far off as
    /// that allows (see [`comfortable_range`]), so a high one fights close and a low one from afar.
    pub comfort: f32,
    /// Seconds for its legs to go from still to a full run, or from a run to still.
    pub ramp: f32,
    /// How hard a drifting leg pushes sideways, 0 to 1.
    pub drift: f32,
}

impl Default for Footwork {
    fn default() -> Self {
        Footwork { restlessness: 0.6, stride: 1.3, reversal: 0.6, comfort: 0.7, ramp: 0.2, drift: 0.3 }
    }
}

const RESTLESSNESS: std::ops::Range<f32> = 0.3..0.85;
const STRIDE: std::ops::Range<f32> = 0.9..2.0;
const REVERSAL: std::ops::Range<f32> = 0.4..0.8;
const COMFORT: std::ops::Range<f32> = 0.55..0.9;
const RAMP: std::ops::Range<f32> = 0.12..0.3;
const DRIFT: std::ops::Range<f32> = 0.15..0.45;
/// Seconds of tracking a target before the first strafe: nobody starts dodging the instant they
/// see somebody.
const FIRST_STEP: f32 = 0.35;

impl Footwork {
    pub fn temperament(skill: BotSkill) -> Self {
        Footwork {
            restlessness: rand::random_range(RESTLESSNESS),
            stride: rand::random_range(STRIDE),
            reversal: rand::random_range(REVERSAL),
            comfort: rand::random_range(skill.within(COMFORT, Better::Higher)),
            ramp: rand::random_range(RAMP),
            drift: rand::random_range(skill.within(DRIFT, Better::Lower)),
        }
    }

    /// The next leg after one toward `last` (`0` for none yet): how hard sideways, `-1` to `1`,
    /// and its seconds. A leg that does not strafe drifts at [`Footwork::drift`].
    pub fn next_leg(&self, last: f32) -> (f32, f32) {
        let seconds = self.stride * rand::random_range(0.6..1.4);
        let side = if last == 0.0 {
            if rand::random::<bool>() { 1.0 } else { -1.0 }
        } else if rand::random::<f32>() < self.reversal {
            -last.signum()
        } else {
            last.signum()
        };
        let push = if rand::random::<f32>() < self.restlessness { 1.0 } else { self.drift };
        (side * push, seconds)
    }
}

/// Meters between the distances [`comfortable_range`] tries, and the furthest it tries.
const RANGE_STEP: f32 = 0.5;
const FURTHEST_TRIED: f32 = 60.0;
/// Meters a bot keeps from whoever it fights at least, whatever it holds, and at most.
pub const CLOSEST_STANDOFF: f32 = 3.0;
pub const FURTHEST_STANDOFF: f32 = 25.0;
/// For a scoped weapon.
pub const SCOPED_FURTHEST_STANDOFF: f32 = 40.0;

/// The furthest a weapon still does `comfort` of its best damage from, within
/// [`CLOSEST_STANDOFF`] and [`FURTHEST_STANDOFF`], or [`SCOPED_FURTHEST_STANDOFF`] for a scoped one. Every weapon's [`worth`] peaks close in, so its
/// best distance alone walks a bot onto its target. `None` for a weapon that reaches nobody.
pub fn comfortable_range(weapon: &Weapon, comfort: f32) -> Option<f32> {
    let worth_at = |distance: f32| weapon_worth(weapon, distance).1;
    let distances = (2..=(FURTHEST_TRIED / RANGE_STEP) as u32).map(|step| step as f32 * RANGE_STEP);
    let best = distances.clone().map(worth_at).fold(0.0, f32::max);
    if best <= 0.0 {
        return None;
    }
    let furthest = distances.filter(|distance| worth_at(*distance) >= comfort * best).last()?;
    let furthest_allowed = if scoped(weapon).is_some() { SCOPED_FURTHEST_STANDOFF } else { FURTHEST_STANDOFF };
    Some(furthest.clamp(CLOSEST_STANDOFF, furthest_allowed))
}

/// How a bot meets a projectile coming at it with an airblast in hand, rolled per seat inside its
/// [`BotMind`]. Each projectile it sees coming is judged once: whether to try, and how early or
/// late to press.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Reflexes {
    /// The chance it tries at all, 0 to 1.
    pub attempt: f32,
    /// Seconds its press may come early or late by, at most.
    pub timing: f32,
}

impl Default for Reflexes {
    fn default() -> Self {
        Reflexes { attempt: 0.5, timing: 0.15 }
    }
}

const ATTEMPT: std::ops::Range<f32> = 0.15..0.85;
const TIMING: std::ops::Range<f32> = 0.04..0.35;
/// The share of an airblast's range a bot means to meet a projectile at: inside the reach, with
/// room for a tick's lag before it is pressed.
const REFLECT_AT: f32 = 0.6;

impl Reflexes {
    pub fn temperament(skill: BotSkill) -> Self {
        Reflexes { attempt: rand::random_range(skill.within(ATTEMPT, Better::Higher)), timing: rand::random_range(TIMING) }
    }

    /// Judges a projectile first seen at `now`: whether it is tried for, and seconds early, a
    /// difference of two rolls so a small error is likelier than a large one.
    pub fn judge(&self, projectile: Entity, now: f32) -> Incoming {
        let early = (rand::random::<f32>() - rand::random::<f32>()) * self.timing;
        Incoming { projectile, seen_at: now, tries: rand::random::<f32>() < self.attempt, early }
    }
}

/// A projectile coming at a bot, as it judged it when it first saw it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Incoming {
    pub projectile: Entity,
    pub seen_at: f32,
    pub tries: bool,
    /// Seconds before the right moment it presses; negative is late.
    pub early: f32,
}

/// Seconds until a projectile at `at` moving at `velocity` reaches `eye`, and how fast it closes,
/// if it is coming within `near` meters of it.
pub fn arriving(eye: Vec3, at: Vec3, velocity: Vec3, near: f32) -> Option<(f32, f32)> {
    let offset = at - eye;
    let distance = offset.length();
    let closing = -velocity.dot(offset) / distance.max(1e-3);
    if closing <= 1.0 {
        return None;
    }
    let soonest = (-offset.dot(velocity) / velocity.length_squared().max(1e-6)).max(0.0);
    ((offset + velocity * soonest).length() <= near).then_some((distance / closing, closing))
}

/// Whether a bot that judged a projectile `early` presses now, `seconds` before it arrives
/// closing at `closing`, with an airblast reaching `range`.
pub fn presses(seconds: f32, closing: f32, range: f32, early: f32) -> bool {
    seconds <= REFLECT_AT * range / closing + early
}

/// Kept between ticks, so the aim turns at a rate and its error settles.
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
    /// `-1` left to `1` right, and seconds until the leg ends.
    strafe: f32,
    strafe_for: f32,
    /// The side of the last strafing leg, for whether the next goes back or on.
    last_side: f32,
    /// The projectile it is meeting with an airblast, or deciding not to.
    incoming: Option<Incoming>,
    /// The weapon whose [`comfortable_range`] was last worked out, and that range.
    standoff: Option<(WeaponId, Option<f32>)>,
    /// Wants its sights up.
    scoping: bool,
    /// Seconds of game time: the scope last asked up, what it fights last seen through it, and
    /// until when a hit keeps it down.
    scoped_at: f32,
    scoped_seen: f32,
    rest_until: f32,
    /// Goes for the head this target, rolled from [`Aim::headhunting`] as it is taken.
    head: bool,
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

impl Aim {
    /// Turns `from` toward `to` over `dt`: at the turn rate while far, and the last of the way by
    /// `ease`, so it never closes a gap outright.
    pub fn ease_toward(&self, from: (f32, f32), to: (f32, f32), dt: f32) -> (f32, f32) {
        let gap = Vec2::new(wrap_angle(to.0 - from.0), to.1 - from.1).length();
        let eased = gap * (1.0 - (-dt / self.ease.max(1e-3)).exp());
        turn_toward(from, to, eased.min(self.turn_rate * dt))
    }
}

fn angle_between(a: (f32, f32), b: (f32, f32)) -> f32 {
    look(a.0, a.1).angle_between(look(b.0, b.1))
}

/// How much one button of a weapon is worth against a body `distance` meters away, as rough
/// damage per second. Zero for what cannot reach or would hurt the bot more than its target.
pub fn worth(mounted: &Mounted, distance: f32) -> f32 {
    worth_aimed(mounted, distance, AIM_FALLOFF)
}

/// [`worth`], with a hitscan shot half as likely to land at `aim_falloff` meters.
fn worth_aimed(mounted: &Mounted, distance: f32, aim_falloff: f32) -> f32 {
    let interval = mounted.cadence.interval().max(1.0 / 64.0);
    match mounted.action {
        WeaponAction::Hitscan(spec @ HitscanSpec { range_m, damage, pellets, .. }) => {
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
            damage as f32 * spec.scale_at(distance) * landed / (1.0 + distance / aim_falloff) / interval
        }
        WeaponAction::Projectile(spec) => {
            // Inside its own blast the shooter takes the splash too.
            if distance < spec.explosion_radius * 1.2 {
                return 0.0;
            }
            let flight = distance / spec.initial_speed.max(1e-3);
            spec.damage as f32 * spec.scale_at(distance) / (1.0 + (flight / DODGE_SECONDS).powi(2)) / interval
        }
        WeaponAction::Flame(spec @ FlameSpec { range_m, damage, afterburn_damage, afterburn_interval_s, .. }) => {
            if distance > range_m {
                return 0.0;
            }
            damage as f32 * spec.scale_at(distance) / interval + afterburn_damage as f32 / afterburn_interval_s.max(1e-3)
        }
        _ => 0.0,
    }
}

/// What the primary fires once sighted, when [`Sights::mounted`] says that differs from the
/// unsighted primary, and how many times the scope magnifies against the open view.
fn scoped(weapon: &Weapon) -> Option<(Mounted, f32)> {
    let scope = weapon.scope()?;
    let up = Sights { up: true, raised: 1.0, weapon: Some(weapon.id), button_down: false };
    let mounted = up.mounted(weapon, crate::game::weapon::Button::Primary);
    (mounted != weapon.primary).then_some((mounted, (2.0 * crate::game::bot_sense::HALF_VIEW / scope.fov.max(1e-3)).max(1.0)))
}

/// Whether to press the scope's button: while the sights are not as wanted, and only after a tick
/// with it up, since they toggle as it goes down and a press lost while held is never seen.
fn scope_press(scoped_weapon: bool, scoping: bool, sights: &Sights) -> bool {
    scoped_weapon && scoping != sights.up && !sights.button_down
}

/// `true` for the secondary. The scoped primary counts from [`SCOPE_FROM`] out, a far shot through
/// it as much likelier to land as the scope magnifies.
pub fn weapon_worth(weapon: &Weapon, distance: f32) -> (bool, f32) {
    let through_scope = scoped(weapon)
        .filter(|_| distance >= SCOPE_FROM)
        .map_or(0.0, |(mounted, zoom)| worth_aimed(&mounted, distance, AIM_FALLOFF * zoom));
    let (primary, secondary) = (worth(&weapon.primary, distance).max(through_scope), worth(&weapon.secondary, distance));
    if secondary > primary { (true, secondary) } else { (false, primary) }
}

/// A carried weapon against a body `distance` away: its better button and what that is worth,
/// nothing with no ammo at all, and less while its clip is empty by the reload it needs first.
fn armed(weapon: WeaponId, ammo: &Ammo, catalog: &WeaponCatalog, distance: f32) -> Option<(WeaponId, bool, f32)> {
    let state = ammo.at(weapon).filter(|state| state.clip + state.reserve > 0)?;
    let spec = catalog.get(weapon)?;
    let (alt, value) = weapon_worth(spec, distance);
    let reload = if spec.magazine.clip.is_some() && state.clip == 0 { spec.magazine.first_reload.unwrap_or(spec.magazine.reload) } else { 0.0 };
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

/// The angle off a body `distance` away that still hits it, for this action; off its head with
/// `head`.
fn tolerance(action: &WeaponAction, distance: f32, head: bool) -> f32 {
    let radius = match action {
        _ if head => HEAD_WIDTH * 0.5,
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
    mut bots: Query<(Entity, &BotSenses, &BotMind, Option<(&Equipped, &Ammo)>, Option<&Sights>, &mut BotIntent, &mut BotAim), With<BotOf>>,
    projectiles: Query<(Entity, &Projectile, &PhysicsBody)>,
    allegiances: Allegiances,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let retuned = catalog.as_ref().is_some_and(|catalog| catalog.is_changed());
    let gravity = rules.as_deref().map_or(MovementRules::default().gravity, |rules| rules.gravity);
    for (bot, senses, mind, arms, sights, mut intent, mut aim) in &mut bots {
        let Ok((body, stance, player, _)) = bodies.get(bot) else { continue };
        let eye = body.current + Vec3::Y * player.duck.eye_from_center(*stance, player.eye);
        let sights = sights.copied().unwrap_or_default();
        let rifle = arms.zip(catalog.as_deref()).and_then(|((equipped, _), catalog)| catalog.held_by(equipped)).filter(|weapon| scoped(weapon).is_some());
        let scope = rifle.and_then(|weapon| weapon.scope());
        if rifle.is_none() {
            aim.scoping = false;
        }
        if aim.scoping && senses.hurt.is_some_and(|hurt| hurt >= aim.scoped_at) {
            aim.scoping = false;
            aim.rest_until = now + SCOPE_REST;
        }
        let sighted = sights.sighted() && scope.is_some();
        let skill = match scope.filter(|_| sighted) {
            // The mouse turns slower through a scope.
            Some(scope) => Aim { turn_rate: mind.aim.turn_rate * scope.look_speed, ..mind.aim },
            None => mind.aim,
        };
        let airblast = arms.zip(catalog.as_deref()).and_then(|((equipped, _), catalog)| match catalog.get(equipped.held()?)?.secondary.action {
            WeaponAction::Airblast(spec) if spec.reflect => Some(spec),
            _ => None,
        });
        let reflecting = airblast.is_some_and(|spec| {
            let coming = |entity: Entity, projectile: &Projectile, at: Vec3| {
                let hostile = projectile.shooter != Some(bot) && allegiances.may_hurt(projectile.source(), bot);
                hostile.then(|| arriving(eye, at, projectile.velocity, projectile.spec.explosion_radius.max(1.0))).flatten().map(|arrival| (entity, at, arrival))
            };
            let tracked = aim.incoming.and_then(|incoming| {
                let (entity, projectile, at) = projectiles.get(incoming.projectile).ok()?;
                coming(entity, projectile, at.current).map(|coming| (incoming, coming))
            });
            // Only something nearer than what it is already meeting is worth a ray.
            let nearest = tracked.map_or(f32::INFINITY, |(_, (_, at, _))| eye.distance_squared(at));
            let nearer = projectiles
                .iter()
                .filter(|(_, _, at)| eye.distance_squared(at.current) < nearest)
                .filter_map(|(entity, projectile, at)| coming(entity, projectile, at.current))
                .filter(|(_, at, _)| in_view(eye, player.yaw, player.pitch, *at) && world.segment_clear(eye, *at))
                .min_by(|a, b| eye.distance_squared(a.1).total_cmp(&eye.distance_squared(b.1)));
            let meeting = match nearer {
                Some(seen) => Some((mind.reflexes.judge(seen.0, now), seen)),
                None => tracked,
            };
            aim.incoming = meeting.map(|(incoming, _)| incoming);
            meeting.is_some_and(|(incoming, (_, _, (seconds, closing)))| {
                incoming.tries && now - incoming.seen_at >= mind.reaction && presses(seconds, closing, spec.range, incoming.early)
            })
        });

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
            aim.head = rand::random::<f32>() < skill.headhunting;
            aim.tracked = 0.0;
            aim.last_bearing = None;
            (aim.yaw, aim.pitch) = (player.yaw, player.pitch);
            (aim.strafe, aim.strafe_for) = (0.0, FIRST_STEP);
        }

        let Some((them, their_stance, their, boxes)) = target.and_then(|target| bodies.get(target).ok()) else {
            if let Some(watch) = intent.watch {
                if rifle.is_some() && !aim.scoping && now >= aim.rest_until {
                    aim.scoping = true;
                    aim.scoped_at = now;
                }
                if aim.scoping {
                    aim.scoped_seen = now;
                }
                (aim.yaw, aim.pitch) = skill.ease_toward((player.yaw, player.pitch), aim_at(eye, watch), dt);
                intent.look_at = Some(eye + look(aim.yaw, aim.pitch) * LOOK_AHEAD);
            } else {
                if aim.scoping && now - aim.scoped_seen > SCOPE_LINGER {
                    aim.scoping = false;
                }
                intent.look_at = aim.scoping.then(|| eye + look(player.yaw, player.pitch) * LOOK_AHEAD);
                (aim.yaw, aim.pitch) = (player.yaw, player.pitch);
            }
            // Lingering after a camp is left, it walks on to whatever `think` wants now.
            intent.hold = aim.scoping && intent.watch.is_some();
            intent.fire = false;
            intent.alt_fire = false;
            intent.strafe = 0.0;
            intent.advance = 0.0;
            intent.select = None;
            intent.standoff = standoff(&mut aim, arms.and_then(|(equipped, _)| equipped.held()), catalog.as_deref(), mind.footwork.comfort, retuned);
            intent.reload = arms.is_some_and(|(equipped, ammo)| {
                equipped.held().and_then(|held| Some((catalog.as_deref()?.get(held)?.magazine.clip?, ammo.at(held)?))).is_some_and(|(clip, state)| state.clip * 2 < clip && state.reserve > 0)
            });
            intent.alt_fire = reflecting || scope_press(rifle.is_some(), aim.scoping, &sights);
            continue;
        };
        intent.reload = false;
        aim.tracked += dt;
        let center = boxes.body.center;
        let feet = them.current - Vec3::Y * their_stance.half_extents().y;
        let distance = eye.distance(center);

        let choice = arms.zip(catalog.as_deref()).and_then(|((equipped, ammo), catalog)| {
            let best = best_weapon(equipped.carried(), ammo, catalog, distance)?;
            let held = equipped.held().and_then(|held| armed(held, ammo, catalog, distance));
            let pick = match held {
                // Scoped, it keeps the rifle.
                Some(held) if aim.scoping || best.2 <= held.2 * SWITCH_GAIN => held,
                _ => best,
            };
            let action = if pick.1 { catalog.get(pick.0)?.secondary.action } else { catalog.get(pick.0)?.primary.action };
            Some((pick.0, pick.1, action, equipped.held() == Some(pick.0)))
        });
        let rifle_ready = rifle.is_some() && choice.is_some_and(|(weapon, alt, _, holding)| holding && !alt && rifle.is_some_and(|rifle| rifle.id == weapon));
        if !rifle_ready {
            aim.scoping = false;
        } else if !aim.scoping && distance >= SCOPE_FROM && now >= aim.rest_until && !reflecting {
            aim.scoping = true;
            aim.scoped_at = now;
        }
        if aim.scoping {
            aim.scoped_seen = now;
        }
        // Until the sights are up, nothing fires.
        let choice = choice.map(|(weapon, alt, action, holding)| match rifle.filter(|_| aim.scoping && !alt).and_then(scoped) {
            Some((mounted, _)) => (weapon, alt, mounted.action, holding),
            None => (weapon, alt, action, holding),
        });
        // Drawing another weapon would put the airblast away before it goes off.
        intent.select = choice.filter(|(_, _, _, holding)| !holding && !reflecting).map(|(weapon, ..)| weapon);

        let action = choice.map_or(WeaponAction::None, |(_, _, action, _)| action);
        let pays_for_heads = matches!(action, WeaponAction::Hitscan(spec) if spec.headshot_multiplier > 1);
        let head = boxes.head.filter(|_| aim.head && pays_for_heads && aim.tracked < HEAD_PATIENCE).map(|head| head.center);
        let point = head.unwrap_or_else(|| aim_point(&world, eye, center, feet, their.velocity, &action, gravity));
        let true_aim = aim_at(eye, point);
        // Off the body rather than the point, which jumps between feet and chest as the feet go
        // in and out of view.
        let bearing = aim_at(eye, center);
        let crossing = aim.last_bearing.map_or(0.0, |last| angle_between(last, bearing) / dt.max(1e-3));
        let moved = aim.last_bearing.map_or((0.0, 0.0), |last| (wrap_angle(bearing.0 - last.0), bearing.1 - last.1));
        if aim.last_bearing.is_none() {
            aim.lean = (true_aim.1 - aim.pitch).atan2(wrap_angle(true_aim.0 - aim.yaw));
        }
        aim.last_bearing = Some(bearing);
        aim.lean += ERROR_DRIFT * dt;
        // A scope magnifies a hand's error down with the view.
        let zoom = rifle.and_then(scoped).filter(|_| sighted).map_or(1.0, |(_, zoom)| zoom);
        let error = skill.error * ((-aim.tracked / skill.settle).exp() + CROSSING_ERROR * crossing) / zoom;
        let off = (true_aim.0 + error * aim.lean.cos(), true_aim.1 + error * aim.lean.sin());
        let followed = (aim.yaw + moved.0 * skill.tracking, aim.pitch + moved.1 * skill.tracking);
        (aim.yaw, aim.pitch) = skill.ease_toward(followed, off, dt);
        intent.look_at = Some(eye + look(aim.yaw, aim.pitch) * LOOK_AHEAD);

        let ready = choice.is_some_and(|(.., holding)| holding) && (!aim.scoping || sighted);
        let lands = ready && would_land(&world, eye, (aim.yaw, aim.pitch), point, tolerance(&action, distance, head.is_some()), splash_of(&action));
        let alt = choice.is_some_and(|(_, alt, ..)| alt);
        intent.fire = lands && !alt && !reflecting;
        intent.alt_fire = (lands && alt) || reflecting || scope_press(rifle.is_some(), aim.scoping, &sights);
        intent.hold = aim.scoping;

        aim.strafe_for -= dt;
        if aim.strafe_for <= 0.0 {
            (aim.strafe, aim.strafe_for) = mind.footwork.next_leg(aim.last_side);
            aim.last_side = aim.strafe;
        }
        let right = Vec3::new(aim.yaw.cos(), 0.0, -aim.yaw.sin());
        let forward = Vec3::new(-aim.yaw.sin(), 0.0, -aim.yaw.cos());
        let standing = body.current - Vec3::Y * stance.half_extents().y;
        let safe = |toward: Vec3| {
            nav.as_deref().is_some_and(|nav| {
                nav.graph().cell_at(standing + toward * STRAFE_CHECK).is_some_and(|cell| nav.graph().cells()[cell as usize].hurt == 0)
            })
        };
        // Turned back from an edge, the new side is a leg of its own, so it is not flipped
        // again the next time the aim swings the check across the edge.
        if aim.strafe != 0.0 && !safe(right * aim.strafe.signum()) {
            aim.strafe = if safe(-right * aim.strafe.signum()) { -aim.strafe } else { 0.0 };
            aim.last_side = aim.strafe;
            aim.strafe_for = mind.footwork.stride;
        }
        intent.strafe = if aim.scoping { 0.0 } else { aim.strafe };

        intent.standoff = standoff(&mut aim, choice.map(|(weapon, ..)| weapon), catalog.as_deref(), mind.footwork.comfort, retuned);
        let advance = intent.standoff.filter(|_| !aim.scoping).map_or(0.0, |range| advance_toward(standing.distance(feet), range));
        intent.advance = if advance != 0.0 && safe(forward * advance.signum()) { advance } else { 0.0 };
    }
}

/// The [`comfortable_range`] of `weapon`, worked out again only when the weapon changes or the
/// catalog was `retuned`.
fn standoff(aim: &mut BotAim, weapon: Option<WeaponId>, catalog: Option<&WeaponCatalog>, comfort: f32, retuned: bool) -> Option<f32> {
    if retuned || aim.standoff.map(|(held, _)| held) != weapon {
        let range = weapon.zip(catalog).and_then(|(weapon, catalog)| comfortable_range(catalog.get(weapon)?, comfort));
        aim.standoff = weapon.map(|weapon| (weapon, range));
    }
    aim.standoff.and_then(|(_, range)| range)
}

/// Of a bot's standoff, the meters off it that push it at full speed toward it, and the share of
/// that within which it does not bother.
const STANDOFF_SLACK: f32 = 0.5;
const STANDOFF_IGNORED: f32 = 0.2;

/// How hard to walk forward, `-1` to `1`, at `distance` from a target to hold `standoff`.
pub fn advance_toward(distance: f32, standoff: f32) -> f32 {
    let push = ((distance - standoff) / (standoff * STANDOFF_SLACK).max(1.0)).clamp(-1.0, 1.0);
    if push.abs() < STANDOFF_IGNORED { 0.0 } else { push }
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
        catalog.weapons().iter().find(|weapon| weapon.key == key).unwrap().id
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

    /// Breaks if a bot's aim stops dead or crawls: a quarter turn away it starts at its turn rate,
    /// close to the target it slows, and it is near within a few of its ease times.
    #[test]
    fn a_bot_eases_onto_a_target() {
        let (skill, dt) = (Aim { ease: EASE.end, ..default() }, 1.0 / 64.0);
        let wanted = (std::f32::consts::FRAC_PI_2, 0.0);
        let first = skill.ease_toward((0.0, 0.0), wanted, dt);
        assert!((first.0 - skill.turn_rate * dt).abs() < 1e-5, "{first:?}");
        let near = (wanted.0 - 0.05, 0.0);
        let slowed = skill.ease_toward(near, wanted, dt).0 - near.0;
        assert!(slowed > 0.0 && slowed < 0.05 * 0.2, "{slowed}");
        let mut current = (0.0, 0.0);
        let ticks = ((wanted.0 / skill.turn_rate + 4.0 * skill.ease) / dt) as usize;
        for _ in 0..ticks {
            current = skill.ease_toward(current, wanted, dt);
        }
        assert!(angle_between(current, wanted) < 0.03, "still off by {}", angle_between(current, wanted));
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
        let rifle = WeaponAction::Hitscan(HitscanSpec { range_m: 100.0, damage: 10, headshot_multiplier: 1, shove_per_damage: 0.0, shove_radius_m: 0.0, pellets: None, kill_shove: None, ramp_up: None, falloff: None, trail: crate::common::weapon::HitscanTrail::Streak });
        let tolerance = tolerance(&rifle, 10.0, false);
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
        let (mut current, mut lean) = ((0.0, 0.0), wanted.1.atan2(wanted.0));
        let dt = 1.0 / 64.0;
        for tick in 0..(3.0 * skill.settle / dt) as usize {
            let tracked = tick as f32 * dt;
            lean += ERROR_DRIFT * dt;
            let error = skill.error * (-tracked / skill.settle).exp();
            current = skill.ease_toward(current, (wanted.0 + error * lean.cos(), wanted.1 + error * lean.sin()), dt);
        }
        let rifle = WeaponAction::Hitscan(HitscanSpec { range_m: 100.0, damage: 10, headshot_multiplier: 1, shove_per_damage: 0.0, shove_radius_m: 0.0, pellets: None, kill_shove: None, ramp_up: None, falloff: None, trail: crate::common::weapon::HitscanTrail::Streak });
        assert!(angle_between(current, wanted) <= tolerance(&rifle, eye.distance(point), false), "still off by {}", angle_between(current, wanted));
    }

    /// Breaks if weapon choice ignores range: the shotgun close, the sniper rifle far, and the
    /// rocket launcher between but never inside its own blast.
    #[test]
    fn a_bot_picks_its_weapon_by_range() {
        let catalog = catalog();
        let (shotgun, rifle, rockets) = (key(&catalog, "shotgun"), key(&catalog, "sniper_rifle"), key(&catalog, "rocket_launcher"));
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
        let rifle = key(&catalog, "sniper_rifle");
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

    /// The scope comes up and down in the tick it is pressed.
    fn fight_for(world: &mut World, seconds: f32) -> bool {
        fight_scoped_for(world, seconds).0
    }

    fn fight_scoped_for(world: &mut World, seconds: f32) -> (bool, bool) {
        use bevy::ecs::system::RunSystemOnce;
        let (mut fired, mut sighted_shot) = (false, false);
        for _ in 0..(seconds * 64.0) as usize {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(1.0 / 64.0));
            world.run_system_once(fight).unwrap();
            for (intent, equipped, mut sights) in world.query::<(&BotIntent, &Equipped, &mut Sights)>().iter_mut(world) {
                if intent.alt_fire && !sights.button_down {
                    sights.up = !sights.up;
                    sights.raised = if sights.up { 1.0 } else { 0.0 };
                    sights.weapon = equipped.held();
                }
                sights.button_down = intent.alt_fire;
                fired |= intent.fire;
                sighted_shot |= intent.fire && sights.sighted();
            }
        }
        (fired, sighted_shot)
    }

    /// Breaks if a bot shoots far off without its scope, moves while scoped, or stays scoped when hit.
    #[test]
    fn a_bot_scopes_at_range_and_comes_out_when_hurt() {
        let open = Room::new(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 6.0, 20.0));
        let (mut world, bot) = a_fight(&[Volume::air(open)]);
        let (fired, sighted_shot) = fight_scoped_for(&mut world, 2.0);
        assert!(fired && sighted_shot, "fired {fired}, through the scope {sighted_shot}");
        let unsighted_shots = {
            let mut shots = 0;
            for _ in 0..64 {
                let (fired, sighted) = fight_scoped_for(&mut world, 1.0 / 64.0);
                shots += (fired && !sighted) as u32;
            }
            shots
        };
        assert_eq!(unsighted_shots, 0, "shot without the scope at 16 m");
        let intent = world.get::<BotIntent>(bot).unwrap();
        assert!(intent.hold && intent.strafe == 0.0, "scoped but still moving: {intent:?}");

        let now = world.resource::<Time>().elapsed_secs();
        world.get_mut::<BotSenses>(bot).unwrap().hurt = Some(now + 1.0 / 64.0);
        fight_scoped_for(&mut world, 2.0 / 64.0);
        assert!(!world.get::<Sights>(bot).unwrap().up, "kept the scope up through a hit");
        fight_scoped_for(&mut world, SCOPE_REST * 0.5);
        assert!(!world.get::<Sights>(bot).unwrap().up, "raised the scope again at once");
        fight_scoped_for(&mut world, SCOPE_REST);
        assert!(world.get::<Sights>(bot).unwrap().up, "never raised the scope again");
    }

    /// Breaks if a camping bot does not watch through its scope, or keeps it up after camping.
    #[test]
    fn a_camping_bot_watches_through_its_scope() {
        let open = Room::new(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 6.0, 20.0));
        let (mut world, bot) = a_fight(&[Volume::air(open)]);
        world.get_mut::<BotSenses>(bot).unwrap().known.clear();
        let watch = Vec3::new(-15.0, 1.4, 0.0);
        world.get_mut::<BotIntent>(bot).unwrap().watch = Some(watch);
        // The fight never writes the body's facing; the step does, from the look.
        for _ in 0..64 {
            fight_scoped_for(&mut world, 1.0 / 64.0);
            let look = world.get::<BotIntent>(bot).unwrap().look_at.unwrap();
            let eye = world.get::<PhysicsBody>(bot).unwrap().current + Vec3::Y * 0.7;
            let (yaw, pitch) = aim_at(eye, look);
            let mut player = world.get_mut::<Player>(bot).unwrap();
            (player.yaw, player.pitch) = (yaw, pitch);
        }
        assert!(world.get::<Sights>(bot).unwrap().sighted(), "the scope is down at camp");
        let look = world.get::<BotIntent>(bot).unwrap().look_at.unwrap();
        assert!(look.x < -5.0 && look.z.abs() < 2.0, "not watching the camp's floor: {look}");
        assert!(world.get::<BotIntent>(bot).unwrap().hold);

        world.get_mut::<BotIntent>(bot).unwrap().watch = None;
        fight_scoped_for(&mut world, SCOPE_LINGER + 0.2);
        assert!(!world.get::<Sights>(bot).unwrap().up, "kept the scope up after camping");
    }

    /// Breaks if a bot set on headshots aims no higher than one that is not.
    #[test]
    fn a_headhunting_bot_aims_at_the_head() {
        use crate::common::hitbox::Box3;
        let open = Room::new(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 6.0, 20.0));
        let mut pitches = Vec::new();
        for headhunting in [0.0, 1.0] {
            let (mut world, bot) = a_fight(&[Volume::air(open.clone())]);
            world.get_mut::<BotMind>(bot).unwrap().aim.headhunting = headhunting;
            let enemy = world.query_filtered::<Entity, (With<Hitboxes>, Without<BotOf>)>().single(&world).unwrap();
            let mut boxes = world.get_mut::<Hitboxes>(enemy).unwrap();
            boxes.head = Some(Box3 { center: boxes.body.center + Vec3::Y * 0.75, half_extents: Vec3::splat(0.15) });
            fight_scoped_for(&mut world, 1.0);
            let look = world.get::<BotIntent>(bot).unwrap().look_at.unwrap();
            let eye = world.get::<PhysicsBody>(bot).unwrap().current + Vec3::Y * Player::default().duck.eye_from_center(Stance::Standing, Player::default().eye);
            pitches.push(aim_at(eye, look).1);
        }
        assert!(pitches[1] > pitches[0] + 0.02, "aimed no higher going for the head: {pitches:?}");
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

    /// The share of rockets a bot with these reflexes meets inside the gust: a 20 m/s rocket seen
    /// 20 m out, its press acted on a tick later, with a 5 m airblast. Caught is in reach and not
    /// yet against the body.
    fn reflected(reflexes: Reflexes) -> f32 {
        let (eye, dt, range, speed) = (Vec3::ZERO, 1.0 / 64.0, 5.0, 20.0);
        let velocity = Vec3::Z * speed;
        let tries = 4000;
        let caught = (0..tries)
            .filter(|_| {
                let incoming = reflexes.judge(Entity::PLACEHOLDER, 0.0);
                if !incoming.tries {
                    return false;
                }
                (0..64).find_map(|tick| {
                    let now = tick as f32 * dt;
                    let at = Vec3::NEG_Z * (20.0 - speed * now);
                    let (seconds, closing) = arriving(eye, at, velocity, 1.0)?;
                    let pressed = now >= BotMind::default().reaction && presses(seconds, closing, range, incoming.early);
                    pressed.then(|| (20.0 - speed * (now + dt)).clamp(-1.0, 20.0))
                })
                .is_some_and(|distance| distance > 0.6 && distance <= range + 0.42)
            })
            .count();
        caught as f32 / tries as f32
    }

    /// Breaks if a quick bot stops reflecting nearly every rocket, or a sloppy one stops missing
    /// a good share, or one that rarely tries reflects often.
    #[test]
    fn reflexes_decide_how_often_a_rocket_comes_back() {
        let sharp = reflected(Reflexes { attempt: 1.0, timing: TIMING.start });
        let sloppy = reflected(Reflexes { attempt: 1.0, timing: TIMING.end });
        let reluctant = reflected(Reflexes { attempt: ATTEMPT.start, timing: TIMING.start });
        assert!(sharp > 0.95, "sharp {sharp}");
        assert!((0.3..0.75).contains(&sloppy), "sloppy {sloppy}");
        assert!(reluctant < 0.25, "reluctant {reluctant}");
    }

    /// Breaks if a rocket passing wide or going away reads as coming.
    #[test]
    fn only_a_projectile_coming_close_is_arriving() {
        let eye = Vec3::ZERO;
        assert!(arriving(eye, Vec3::NEG_Z * 10.0, Vec3::Z * 20.0, 1.0).is_some_and(|(seconds, _)| (seconds - 0.5).abs() < 1e-4));
        assert!(arriving(eye, Vec3::new(3.0, 0.0, -10.0), Vec3::Z * 20.0, 1.0).is_none(), "passes 3 m wide");
        assert!(arriving(eye, Vec3::NEG_Z * 10.0, Vec3::NEG_Z * 20.0, 1.0).is_none(), "going away");
    }

    /// Breaks if legs get short enough to jitter, a leg plants the bot, footwork stops drifting
    /// between strafes, or it always turns back.
    #[test]
    fn a_fight_is_legs_long_enough_to_read() {
        let footwork = Footwork::default();
        let (mut last, mut drifting, mut reversals) = (0.0_f32, 0, 0);
        for _ in 0..4000 {
            let (side, seconds) = footwork.next_leg(last);
            assert!(seconds >= footwork.stride * 0.6 && seconds > 0.5, "a leg of {seconds} s");
            assert!(side.abs() == 1.0 || side.abs() == footwork.drift, "a leg of {side}");
            drifting += (side.abs() < 1.0) as u32;
            if last != 0.0 {
                reversals += (side.signum() == -last.signum()) as u32;
            }
            last = side;
        }
        let drifted = drifting as f32 / 4000.0;
        assert!((drifted - (1.0 - footwork.restlessness)).abs() < 0.05, "drifted {drifted}");
        let reversed = reversals as f32 / 3999.0;
        assert!((reversed - footwork.reversal).abs() < 0.05, "reversed {reversed}");
    }

    /// Breaks if every weapon's standoff is point blank, which walks a bot onto its target, or a
    /// standoff lies past where its weapon reaches.
    #[test]
    fn a_bot_keeps_back_as_far_as_its_weapon_allows() {
        let catalog = catalog();
        for weapon in catalog.weapons() {
            let Some(range) = comfortable_range(weapon, Footwork::default().comfort) else { continue };
            let key = &weapon.key;
            assert!((CLOSEST_STANDOFF..=SCOPED_FURTHEST_STANDOFF).contains(&range), "{key} keeps {range} m");
            assert!(range <= FURTHEST_STANDOFF || scoped(weapon).is_some(), "{key} keeps {range} m without a scope");
            let reach = (worth(&weapon.primary, range) + worth(&weapon.secondary, range)) > 0.0;
            assert!(reach || range == CLOSEST_STANDOFF, "{key} keeps {range} m, out of its own reach");
        }
        let far = |comfort| catalog.weapons().iter().filter_map(|weapon| comfortable_range(weapon, comfort)).fold(0.0, f32::max);
        assert!(far(0.7) > 8.0, "nothing is fought from further than {} m", far(0.7));
        assert!(far(0.6) >= far(0.85), "a bot insisting on more damage keeps further off");
        let rifle = catalog.get(WeaponId::of("sniper_rifle")).unwrap();
        let rifle_range = comfortable_range(rifle, COMFORT.end).unwrap();
        assert!(rifle_range > FURTHEST_STANDOFF, "the sniper rifle is fought from {rifle_range} m");
    }

    /// Breaks if a bot inside its standoff walks in, outside it walks off, or twitches back and
    /// forth about it.
    #[test]
    fn a_bot_walks_to_its_standoff_and_rests_there() {
        assert!(advance_toward(2.0, 10.0) < -0.9, "too close, it does not back off");
        assert!(advance_toward(25.0, 10.0) > 0.9, "too far, it does not close");
        assert_eq!(advance_toward(10.4, 10.0), 0.0, "at its standoff it still walks");
    }
}
