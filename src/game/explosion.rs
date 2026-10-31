//! What an [`Explosion`] does to the things around it. Every explosive writes
//! the same message so they all agree about splash.
//!
//! A wall stops a blast (the same [`CollisionWorld::ray_distance`] a bullet
//! uses). A direct hit takes the center once rather than splash on top of it; others take a share
//! falling to the blast's edge share at the radius, scaled by how far the blast went off from
//! the shooter, whose own blast is not. Corpses are
//! thrown [`Push::Outward`] by the blast's `knockback`; a living body with an
//! [`Impulse`] is pushed by the damage it took, as TF2 does — see
//! [`knockback`]. A splash candidate is anything with [`Hitboxes`].

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::effects::Effect;
use crate::common::damage::{falloff, Damage, Explosion};
use crate::common::class::Stance;
use crate::common::hitbox::{Box3, Hitboxes};
use crate::common::team::Allegiances;
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::player::{Impulse, Player};
use crate::game::ragdoll::{Push, RagdollShove};

/// How long the debug sphere of a blast is drawn for, in seconds.
pub const BLAST_MARK_LIFETIME: f32 = 0.35;
/// How long an airblast's cone is drawn for, in seconds.
pub const GUST_MARK_LIFETIME: f32 = 0.2;

/// A drawing of a blast or a gust that has already happened. Feedback only.
#[derive(Component, Debug)]
pub struct EffectMark {
    pub shape: MarkShape,
    /// Seconds.
    pub remaining: f32,
    pub lifetime: f32,
}

#[derive(Clone, Copy, Debug)]
pub enum MarkShape {
    Blast { at: Vec3, radius: f32 },
    Gust { from: Vec3, toward: Dir3, range: f32, spread: f32 },
}

pub struct ExplosionPlugin;

impl Plugin for ExplosionPlugin {
    fn build(&self, app: &mut App) {
        app
            .add_message::<Explosion>()
            // After the projectiles, so a shot fired this tick goes off this tick, and after the
            // boxes, so it is measured from where this tick's step left each body.
            .add_systems(FixedUpdate, explode.in_set(DamageSystems::Deal).after(crate::game::projectile::step_projectiles).after(crate::game::hitbox::update_hitboxes))
            .add_systems(Update, (mark_blasts, fade_blast_marks))
            .add_systems(
                PostUpdate,
                draw_blast_marks.run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), clear_blast_marks)
        ;
    }
}

/// Knockback, as TF2 works it out: meters per second of push per point of
/// damage the blast deals the body, airborne and standing — TF2's ten units.
const KNOCKBACK_PER_DAMAGE: f32 = 0.254;
/// TF2's ceiling on one blast's push: a thousand units.
const MAX_KNOCKBACK: f32 = 25.4;
/// A crouched body is pushed this much harder: TF2's standing box over its
/// crouched one, from before the crouch was made taller.
const CROUCHED_KNOCKBACK: f32 = 82.0 / 55.0;
/// Your own blast pushes you this much while you stand on something; the
/// air takes it whole.
const GROUNDED_SELF_KNOCKBACK: f32 = 0.5;
/// Your own splash never falls below this fraction, however far out you are
/// caught.
const SELF_SPLASH_FLOOR: f32 = 0.5;
/// How far below the blast the push is aimed from, in meters — TF2's ten
/// units — so a rocket at your side still lifts you a little.
const KNOCKBACK_ORIGIN_DROP: f32 = 0.254;

#[allow(clippy::type_complexity)]
pub fn explode(
    world: Res<CollisionWorld>,
    mut blasts: MessageReader<Explosion>,
    mut bodies: Query<(Entity, &Hitboxes, Option<&mut Impulse>, Option<&Stance>, Option<&Player>)>,
    allegiances: Allegiances,
    mut damage: MessageWriter<Damage>,
    mut shoves: MessageWriter<RagdollShove>,
    mut effects: MessageWriter<Effect>,
) {
    for blast in blasts.read() {
        effects.write(Effect::Blast { at: blast.at, radius: blast.radius });

        // Something hit squarely is hurt whether or not it can be caught in
        // a blast; one that can is handled with the rest below.
        if let Some(target) = blast.direct
            && !bodies.contains(target)
        {
            let amount = scaled(blast, 1.0);
            damage.write(Damage { target, source: blast.source, amount, point: blast.at, from: Some(blast.at), kill_shove: None });
        }

        for (entity, boxes, impulse, stance, player) in &mut bodies {
            let center = boxes.body.center;
            let direct = blast.direct.filter(|target| *target == entity);
            let mine = allegiances.is_self(blast.source, entity);
            let amount = match direct {
                Some(_) => scaled(blast, 1.0),
                None if mine => self_splash(blast, &boxes.body),
                None => splash(blast, center),
            };
            // Before the ray, which is the expensive part: most bodies in a
            // map are nowhere near any one blast.
            if amount == 0 {
                continue;
            }
            // A direct hit is at the body's surface; a wall cannot be between.
            if direct.is_none() && !in_sight(&world, blast.at, center, center.distance(blast.at)) {
                continue;
            }

            if let Some(mut impulse) = impulse
                && allegiances.may_hurt(blast.source, entity)
            {
                let crouched = stance == Some(&Stance::Crouched);
                let grounded = player.is_some_and(Player::on_ground);
                impulse.0 += knockback(blast.at, center, amount, mine, crouched, grounded);
            }

            damage.write(Damage {
                target: entity,
                source: blast.source,
                amount,
                // Over the body: at the blast point every victim's number
                // would be drawn on top of the others. A direct hit is drawn
                // where it struck.
                point: if direct.is_some() { blast.at } else { center },
                from: Some(blast.at),
                kill_shove: None,
            });
        }

        // Corpses have no hitboxes, so this is the only thing that moves them.
        shoves.write(RagdollShove {
            at: blast.at,
            push: Push::Outward(blast.knockback),
            radius: blast.radius,
        });
    }
}

/// `blast`'s damage to somebody other than its source taking `share` of it.
fn scaled(blast: &Explosion, share: f32) -> u32 {
    (blast.damage as f32 * blast.scale * share).round() as u32
}

/// Falling off in a line from the blast to the body's center, down to the blast's edge share at
/// the radius, and nothing past it.
fn splash(blast: &Explosion, center: Vec3) -> u32 {
    let distance = center.distance(blast.at);
    if blast.radius <= 0.0 || distance > blast.radius {
        return 0;
    }
    let toward_edge = 1.0 - falloff(distance, blast.radius);
    scaled(blast, 1.0 - (1.0 - blast.at_edge.clamp(0.0, 1.0)) * toward_edge)
}

/// TF2's self-splash: caught if any of the box is inside the radius, measured
/// to the nearer of the box's center and its feet, and never below
/// [`SELF_SPLASH_FLOOR`]. Kinder than [`splash`] on purpose — it is what a
/// rocket jump costs and how hard it throws. Not scaled by distance, which is from the source.
fn self_splash(blast: &Explosion, body: &Box3) -> u32 {
    let nearest = blast.at.clamp(body.center - body.half_extents, body.center + body.half_extents);
    if blast.radius <= 0.0 || nearest.distance(blast.at) > blast.radius {
        return 0;
    }
    let feet = body.center - Vec3::Y * body.half_extents.y;
    let distance = body.center.distance(blast.at).min(feet.distance(blast.at));
    let share = 1.0 - (1.0 - SELF_SPLASH_FLOOR) * (distance / blast.radius).clamp(0.0, 1.0);
    (blast.damage as f32 * share).round() as u32
}

/// What a body's own blast of `damage` splash and `radius` at `at` does to it: the damage it
/// takes and the push it gets. The rules [`explode`] applies to the blast's own source, for the
/// nav bake's rocket jumps, which have no entities to send messages about.
#[allow(clippy::too_many_arguments)]
pub fn own_blast(world: &CollisionWorld, at: Vec3, radius: f32, damage: u32, body: &Box3, crouched: bool, grounded: bool) -> (u32, Vec3) {
    let blast = Explosion { at, radius, damage, at_edge: 0.0, scale: 1.0, knockback: 0.0, source: crate::common::damage::DamageSource::World, direct: None };
    let amount = self_splash(&blast, body);
    if amount == 0 || !in_sight(world, at, body.center, body.center.distance(at)) {
        return (0, Vec3::ZERO);
    }
    (amount, knockback(at, body.center, amount, true, crouched, grounded))
}

/// How fast a blast dealing `amount` throws a standing body, in meters per second.
pub fn push_speed(amount: u32) -> f32 {
    (KNOCKBACK_PER_DAMAGE * amount as f32).min(MAX_KNOCKBACK)
}

/// The push one blast gives a body it dealt `amount` to, before any scaling of
/// that damage: what a hit costs and how far it throws are separate questions.
fn knockback(at: Vec3, center: Vec3, amount: u32, mine: bool, crouched: bool, grounded: bool) -> Vec3 {
    let mut speed = KNOCKBACK_PER_DAMAGE * amount as f32;
    if crouched {
        speed *= CROUCHED_KNOCKBACK;
    }
    if mine && grounded {
        speed *= GROUNDED_SELF_KNOCKBACK;
    }
    let from = at - Vec3::Y * KNOCKBACK_ORIGIN_DROP;
    (center - from).try_normalize().unwrap_or(Vec3::Y) * speed.min(MAX_KNOCKBACK)
}

/// Meters short of its target a line of sight stops, because a body against a
/// wall has the wall a few centimeters behind its chest.
const SIGHT_CLEARANCE: f32 = 0.05;

/// Whether there is a clear line from the blast to a point `distance` away.
fn in_sight(world: &CollisionWorld, from: Vec3, to: Vec3, distance: f32) -> bool {
    let Ok(direction) = Dir3::new(to - from) else {
        return true;
    };
    let reach = (distance - SIGHT_CLEARANCE).max(0.0);
    world.ray_distance(&Ray3d::new(from, direction), reach).is_none()
}

/// Reads [`Effect`] rather than `Explosion`: `Explosion` exists only on the
/// authority, `Effect` is relayed to clients.
fn mark_blasts(mut commands: Commands, mut effects: MessageReader<Effect>) {
    for effect in effects.read() {
        let (shape, lifetime) = match *effect {
            Effect::Blast { at, radius } => (MarkShape::Blast { at, radius }, BLAST_MARK_LIFETIME),
            Effect::Gust { from, toward, range, spread, .. } => {
                let Ok(toward) = Dir3::new(toward) else { continue };
                (MarkShape::Gust { from, toward, range, spread }, GUST_MARK_LIFETIME)
            }
            _ => continue,
        };
        commands.spawn((EffectMark { shape, remaining: lifetime, lifetime }, Name::new("Effect mark")));
    }
}

fn fade_blast_marks(
    mut commands: Commands,
    time: Res<Time>,
    mut marks: Query<(Entity, &mut EffectMark)>,
) {
    for (entity, mut mark) in &mut marks {
        mark.remaining -= time.delta_secs();
        if mark.remaining <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// A blast as a sphere the size of the damage radius, a gust as its cone.
fn draw_blast_marks(mut gizmos: Gizmos, marks: Query<&EffectMark>) {
    for mark in &marks {
        let alpha = (mark.remaining / mark.lifetime).clamp(0.0, 1.0);
        match mark.shape {
            MarkShape::Blast { at, radius } => {
                gizmos.sphere(Isometry3d::from_translation(at), radius, Color::srgba(1.0, 0.55, 0.1, alpha));
            }
            MarkShape::Gust { from, toward, range, spread } => {
                let (cone, isometry) = crate::game::flame::cone_gizmo(from, toward, range, spread);
                gizmos.primitive_3d(&cone, isometry, Color::srgba(0.75, 0.9, 1.0, alpha * 0.6));
            }
        }
    }
}

fn clear_blast_marks(mut commands: Commands, marks: Query<Entity, With<EffectMark>>) {
    for mark in &marks {
        commands.entity(mark).despawn();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::damage::{DamageSource, PlayerId};
    use crate::common::hitbox::Box3;
    use crate::tool::room::Room;

    const SHOOTER: DamageSource = DamageSource::Player(PlayerId(3));

    fn a_world() -> World {
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<Explosion>>();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<RagdollShove>>();
        world.init_resource::<Messages<Effect>>();
        world
    }

    fn body(world: &mut World, center: Vec3) -> Entity {
        world
            .spawn(Hitboxes {
                body: Box3 { center, half_extents: Vec3::new(0.4, 0.9, 0.4) },
                head: Some(Box3 { center: center + Vec3::Y * 0.8, half_extents: Vec3::splat(0.15) }),
            })
            .id()
    }

    fn blast(at: Vec3, direct: Option<Entity>) -> Explosion {
        Explosion { at, radius: 4.0, damage: 60, at_edge: 0.0, scale: 1.0, knockback: 10.0, source: SHOOTER, direct }
    }

    fn damage_written(world: &mut World) -> Vec<Damage> {
        let messages = world.resource::<Messages<Damage>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    /// Breaks if the edge share stops holding the splash up to the radius, reaches past it, or the
    /// distance from the shooter scales the shooter's own blast.
    #[test]
    fn splash_falls_to_its_edge_share_and_only_others_are_scaled_by_range() {
        let floored = Explosion { at_edge: 0.5, ..blast(Vec3::ZERO, None) };
        assert_eq!(splash(&floored, Vec3::ZERO), 60);
        assert_eq!(splash(&floored, Vec3::new(2.0, 0.0, 0.0)), 45);
        assert_eq!(splash(&floored, Vec3::new(4.0, 0.0, 0.0)), 30);
        assert_eq!(splash(&floored, Vec3::new(4.1, 0.0, 0.0)), 0);

        let near = Explosion { scale: 1.25, ..floored };
        assert_eq!(splash(&near, Vec3::ZERO), 75);
        let body = Box3 { center: Vec3::new(0.0, 1.0, 0.0), half_extents: Vec3::new(0.4, 1.0, 0.4) };
        assert_eq!(self_splash(&near, &body), self_splash(&floored, &body));
    }

    #[test]
    fn splash_falls_off_with_distance_and_stops_at_the_radius() {
        let mut world = a_world();
        let near = body(&mut world, Vec3::new(1.0, 0.0, 0.0));
        let far = body(&mut world, Vec3::new(3.0, 0.0, 0.0));
        let _outside = body(&mut world, Vec3::new(9.0, 0.0, 0.0));
        world.write_message(blast(Vec3::ZERO, None));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        assert_eq!(hits.len(), 2, "the body outside the radius was caught: {hits:?}");
        let hit_on = |target| *hits.iter().find(|h| h.target == target).unwrap();
        assert_eq!(hit_on(near).amount, 45, "60 at three quarters reach");
        assert_eq!(hit_on(far).amount, 15, "60 at a quarter reach");
    }

    /// Breaks if splash numbers go at the blast point, where they overlap.
    #[test]
    fn a_blast_pushes_living_bodies_away_but_not_a_teammates() {
        use crate::common::team::Team;
        use crate::game::player::Impulse;

        let mut world = a_world();
        world.spawn((PlayerId(3), Team::Red));
        let enemy = body(&mut world, Vec3::new(1.0, 0.0, 0.0));
        world.entity_mut(enemy).insert((Impulse::default(), Team::Blue));
        let friend = body(&mut world, Vec3::new(-1.0, 0.0, 0.0));
        world.entity_mut(friend).insert((Impulse::default(), Team::Red));
        world.write_message(blast(Vec3::ZERO, None));

        world.run_system_once(explode).unwrap();

        let pushed = world.get::<Impulse>(enemy).unwrap().0;
        // 60 at a quarter of the way out is 45, and 45 points push 45 × 0.254.
        assert!(pushed.x > 0.0, "pushed toward the blast: {pushed}");
        assert!((pushed.length() - KNOCKBACK_PER_DAMAGE * 45.0).abs() < 1e-3, "pushed {pushed}");
        assert_eq!(world.get::<Impulse>(friend).unwrap().0, Vec3::ZERO, "a teammate's blast pushed");
    }

    #[test]
    fn crouching_is_pushed_harder_and_your_own_blast_on_the_ground_half_as_hard() {
        let at = Vec3::ZERO;
        let center = Vec3::new(0.0, 1.0, 0.0);
        let speed = |mine, crouched, grounded| knockback(at, center, 40, mine, crouched, grounded).length();
        let base = speed(false, false, false);
        assert!((base - KNOCKBACK_PER_DAMAGE * 40.0).abs() < 1e-4);
        assert!((speed(false, true, false) - base * CROUCHED_KNOCKBACK).abs() < 1e-4);
        assert_eq!(speed(false, false, true), base, "somebody else's blast was halved on the ground");
        assert!((speed(true, false, true) - base * GROUNDED_SELF_KNOCKBACK).abs() < 1e-4);
        assert_eq!(speed(true, false, false), base, "your own blast was halved in the air");
        assert_eq!(knockback(at, center, 10_000, false, false, false).length(), MAX_KNOCKBACK);
    }

    #[test]
    fn your_own_splash_never_falls_below_half_and_stops_past_the_radius() {
        let body = Box3 { center: Vec3::new(0.0, 0.9, 0.0), half_extents: Vec3::new(0.4, 0.9, 0.4) };
        let at = |x: f32| Explosion { at: Vec3::new(x, 0.9, 0.0), ..blast(Vec3::ZERO, None) };
        assert_eq!(self_splash(&at(0.0), &body), 60);
        // Its edge at 0.4, so caught right out to 4.4 — and at its weakest there.
        assert_eq!(self_splash(&at(4.39), &body), 30);
        assert_eq!(self_splash(&at(4.5), &body), 0);
    }

    #[test]
    fn a_splash_number_is_anchored_over_the_body_it_hurt() {
        let mut world = a_world();
        let left = body(&mut world, Vec3::new(-1.0, 0.0, 0.0));
        let right = body(&mut world, Vec3::new(1.0, 0.0, 0.0));
        world.write_message(blast(Vec3::ZERO, None));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        let point_on = |target| hits.iter().find(|h| h.target == target).unwrap().point;
        assert_eq!(point_on(left), Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(point_on(right), Vec3::new(1.0, 0.0, 0.0));
        assert_ne!(point_on(left), point_on(right), "two hits landed on one point");
    }

    /// A direct hit's number goes where the projectile struck, not the chest.
    #[test]
    fn a_direct_hit_on_something_with_no_hitboxes_still_hurts_it() {
        let mut world = a_world();
        let target = world.spawn_empty().id();
        world.write_message(blast(Vec3::ZERO, Some(target)));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        assert_eq!(hits.iter().filter(|h| h.target == target).map(|h| h.amount).collect::<Vec<_>>(), vec![60]);
    }

    #[test]
    fn a_direct_hit_is_anchored_where_it_struck() {
        let mut world = a_world();
        let target = body(&mut world, Vec3::new(0.0, 1.0, 0.0));
        let struck = Vec3::new(0.1, 1.6, 0.3);
        world.write_message(blast(struck, Some(target)));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        assert_eq!(hits.iter().find(|h| h.target == target).unwrap().point, struck);
    }

    /// Breaks if a direct hit also takes splash, or takes less than the center.
    #[test]
    fn a_direct_hit_takes_the_center_once() {
        let mut world = a_world();
        let hit = body(&mut world, Vec3::new(0.2, 0.0, 0.0));
        let bystander = body(&mut world, Vec3::new(2.0, 0.0, 0.0));
        world.write_message(blast(Vec3::ZERO, Some(hit)));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        assert_eq!(hits.iter().filter(|h| h.target == hit).count(), 1, "{hits:?}");
        assert_eq!(hits.iter().find(|h| h.target == hit).unwrap().amount, 60);
        assert!(hits.iter().any(|h| h.target == bystander), "the bystander was missed");
    }

    #[test]
    fn a_wall_between_them_takes_the_blast() {
        let mut world = a_world();
        // Two rooms side by side, with a solid wall between them.
        world.resource_mut::<CollisionWorld>().rebuild(&[
            Room::new(Vec3::new(-4.0, 0.0, -4.0), Vec3::new(-0.5, 4.0, 4.0)),
            Room::new(Vec3::new(0.5, 0.0, -4.0), Vec3::new(4.0, 4.0, 4.0)),
        ]);
        let sheltered = body(&mut world, Vec3::new(1.5, 2.0, 0.0));
        world.write_message(blast(Vec3::new(-1.5, 2.0, 0.0), None));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        assert!(
            !hits.iter().any(|h| h.target == sheltered),
            "the blast went through the wall: {hits:?}"
        );
    }

    /// Breaks if corpses stop being thrown: they are not in the body query.
    #[test]
    fn a_blast_shoves_outwards_once() {
        let mut world = a_world();
        world.write_message(blast(Vec3::new(1.0, 2.0, 3.0), None));

        world.run_system_once(explode).unwrap();

        let messages = world.resource::<Messages<RagdollShove>>();
        let mut cursor = messages.get_cursor();
        let shoves: Vec<RagdollShove> = cursor.read(messages).copied().collect();
        assert_eq!(shoves.len(), 1);
        assert_eq!(shoves[0].at, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(shoves[0].push, Push::Outward(10.0));
        assert_eq!(shoves[0].radius, 4.0);
    }
}
