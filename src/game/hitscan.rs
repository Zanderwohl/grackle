//! A debug laser out of the player's eye, and a ball where it lands.
//!
//! The first half of a weapon: aiming and hit registration with no damage, no
//! ammunition and no firing. It is on all the time while playing rather than
//! bound to a trigger, because what it is for is *seeing* the hitbox layer
//! answer — stand in front of the animation grid, sweep across sixty bodies,
//! and watch the ball turn red exactly where a head shot would be.
//!
//! The beam starts at the eye, so in first person there is nothing to see but
//! the ball; press `F` and watch it from behind the body.
//!
//! Firing is the other half, and it lives on the tick rather than beside the
//! drawing: a shot is a fact the world has to agree on, and it is taken from
//! the step's own position against the step's own boxes. The beam you see and
//! the shot you fire therefore start a fraction apart — the beam from the
//! interpolated body, the shot from the fixed one — and that is the right way
//! round. The picture may be smooth; the shot must be reproducible.
//!
//! Two things are deliberately *not* here:
//!
//! - **This runs at frame rate, not on the tick.** It is a drawing of where
//!   you are aiming, and aim is a frame-rate quantity in this codebase — see
//!   `mouse_look`. The boxes it tests against are the tick's, which is the
//!   half that has to be reproducible. Real hit registration moves the trace
//!   itself onto the tick; [`crate::common::hitscan::trace`] is a function of
//!   values so that it can be called from either.
//! - **The trace knows nothing about walls.** [`CollisionWorld::ray_distance`]
//!   shortens the range before the bodies are asked, so whether a shot goes
//!   through a wall is one line here rather than a rule buried in the test.

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::effects::Effect;
use crate::common::damage::{Damage, DamageSource, PlayerId};
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::{trace, HitZone};
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::hitbox::update_hitboxes;
use crate::game::player::{step_player, PhysicsBody, Player};
use crate::game::ragdoll::{Push, RagdollShove};
use crate::common::weapon::{HitscanSpec, WeaponAction};
use crate::game::weapon::WeaponActionFired;

/// How far the debug laser reaches, in metres.
///
/// Long enough to cross any room somebody is going to build by hand, short
/// enough that it is a line and not a claim about a weapon's range. A real
/// weapon brings its own.
pub const LASER_RANGE: f32 = 200.0;

/// How long a tracer is drawn for, in seconds. Long enough to see, short
/// enough that a stream of them does not read as one continuous beam.
const TRACER_LIFETIME: f32 = 0.12;

/// Radius of the ball drawn at a hit, in metres. Twenty centimetres across.
pub const HIT_BALL_RADIUS: f32 = 0.1;

// The numbers a shot is worth used to be four constants here, marked as
// placeholders to be deleted rather than generalised once a real weapon stated
// its own. That is [`HitscanSpec`]: range, damage, headshot multiplier, and
// how hard the shot shoves a corpse. Two things about the last pair are worth
// keeping said now that they live in a data file.
//
// The shove is off what the shot was *worth* rather than what it took off, so
// the round that kills somebody with twenty health left shoves as hard as the
// one before it did — a bullet's momentum is not a question about the target.
//
// And its radius is tight, about the length of a forearm, so that a shot moves
// the bone it hit and the joints drag the rest of the body after it. That is
// the whole difference between a body spun by a headshot and a body slid
// sideways as a lump. An explosion writes the same message with a radius that
// covers all of it; see [`RagdollShove`].

pub struct HitscanPlugin;

impl Plugin for HitscanPlugin {
    fn build(&self, app: &mut App) {
        // With the other gizmos, after the transforms they are placed from
        // have been propagated.
        app
            // On the tick, after the boxes it tests against have been moved
            // to where this step left them, and in the set every damage
            // source shares — see [`DamageSystems`].
            .add_systems(
                FixedUpdate,
                fire_hitscan
                    .after(update_hitboxes)
                    .after(step_player)
                    .in_set(DamageSystems::Deal),
            )
            // Drawing, so everywhere: a client fires nothing and has to show
            // every shot anybody took.
            .add_systems(Update, (mark_tracers, fade_tracers))
            .add_systems(
                PostUpdate,
                (draw_debug_laser, draw_tracers)
                    .after(TransformSystems::Propagate)
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), clear_tracers)
        ;
    }
}

/// Which way the player is looking, as a direction in world space.
///
/// Yaw and pitch rather than the camera's transform: in third person the
/// camera has swung behind the body and is no longer standing where the eye
/// is, and the eye is what a shot comes out of in both views.
pub(crate) fn aim(player: &Player) -> Dir3 {
    let looking = Quat::from_rotation_y(player.yaw) * Quat::from_rotation_x(player.pitch);
    // A rotation of a unit vector is a unit vector; the fallback is unreachable
    // arithmetic rather than a case worth handling.
    Dir3::new(looking * Vec3::NEG_Z).unwrap_or(Dir3::NEG_Z)
}

/// Where the eye is, given the centre of a body and how much of it is
/// standing.
///
/// A centre rather than a `Transform`, because the two callers hold different
/// ones: the drawing uses the interpolated transform so the beam does not
/// stutter, and the shot uses the fixed step's own position so two machines
/// agree on where it came from.
///
/// The same offset the first-person camera is placed at, from the same class
/// metric, so the beam comes out of the lens rather than out of somewhere near
/// it.
pub(crate) fn eye(centre: Vec3, stance: Stance) -> Vec3 {
    centre + Vec3::Y * stance.eye_offset()
}

/// What a hit on `zone` is worth, to this weapon.
///
/// The reason the zone is carried out of the trace at all: without a
/// multiplier the head box is an elaborate way of computing the same number as
/// the body box.
fn damage_for(zone: HitZone, spec: &HitscanSpec) -> u32 {
    match zone {
        HitZone::Body => spec.damage,
        HitZone::Head => spec.damage * spec.headshot_multiplier,
    }
}

/// Pull the trigger: one shot, on the tick, for each latched press.
///
/// Nothing happens at zero health — see [`Damageable::apply`]. A body shot to
/// nothing keeps standing there and keeps taking hits worth zero, which is
/// exactly what a debug harness should do until a gamemode has an opinion
/// about death.
pub fn fire_hitscan(
    mut fired: MessageReader<WeaponActionFired>,
    world: Res<CollisionWorld>,
    shooters: Query<(&PhysicsBody, &Player, &Stance, Option<&PlayerId>)>,
    targets: Query<(Entity, &Hitboxes)>,
    mut damage: MessageWriter<Damage>,
    mut shoves: MessageWriter<RagdollShove>,
    mut effects: MessageWriter<Effect>,
) {
    for shot in fired.read() {
        // Somebody else's action. Each weapon system answers for the actions
        // it knows and ignores the rest, which is what lets a new kind be
        // added without this system being touched.
        //
        // Read out of the message rather than looked up on the shooter: what
        // `pull_trigger` found is what this answers for, so a weapon switched
        // in the same tick cannot reach back and change the shot.
        let WeaponAction::Hitscan(spec) = shot.action else { continue };
        let Ok((body, player, stance, id)) = shooters.get(shot.shooter) else { continue };

        // The step's own position, not the drawn one. `Transform` is written
        // by `interpolate_bodies` at frame rate, and a shot fired from it
        // would come from a slightly different place on every machine.
        let ray = Ray3d::new(eye(body.current, *stance), aim(player));
        let range = world.ray_distance(&ray, spec.range).unwrap_or(spec.range);

        let others = targets
            .iter()
            .filter(|(entity, _)| *entity != shot.shooter)
            .map(|(entity, boxes)| (entity, *boxes));

        let hit = trace(&ray, range, others);

        // Written before the `continue` below, because a shot that hit nothing
        // is still a shot somebody saw. The tracer is where the bullet went,
        // which is a fact about the world and not about whether it found
        // anybody.
        effects.write(Effect::Tracer {
            from: ray.origin,
            to: hit.map_or(ray.origin + *ray.direction * range, |hit| hit.point),
            hit: hit.is_some(),
        });

        let Some(hit) = hit else { continue };

        // Asked for, not applied: what a shot is worth is this module's
        // business and what it does to a health pool is the damage layer's.
        // A target with no health is skipped there — a wall dressing stopped
        // the shot all the same.
        let worth = damage_for(hit.zone, &spec);
        damage.write(Damage {
            target: hit.target,
            source: id.map_or(DamageSource::World, |id| DamageSource::Player(*id)),
            amount: worth,
            point: hit.point,
        });

        // Written whether or not that was fatal, and read a moment later by
        // corpses that may not have existed when this ran. A shot that kills
        // therefore throws the body it just made, and one that does not is a
        // shove aimed at a body still standing up, which nothing answers.
        // Neither case is tested for here, which is the point: this knows it
        // fired a bullet and nothing else.
        shoves.write(RagdollShove {
            at: hit.point,
            push: Push::Along(*ray.direction * (worth as f32 * spec.shove_per_damage)),
            radius: spec.shove_radius,
        });
    }
}

/// A tracer, for the fraction of a second it is worth seeing.
///
/// An entity rather than a gizmo call at the moment of firing, because firing
/// happens on the tick and drawing happens at frame rate — a gizmo written
/// inside `fire_hitscan` would be drawn for one frame if the frame rate
/// happened to line up, and not at all if it did not.
#[derive(Component)]
pub struct Tracer {
    from: Vec3,
    to: Vec3,
    hit: bool,
    remaining: f32,
}

fn mark_tracers(mut commands: Commands, mut effects: MessageReader<Effect>) {
    for effect in effects.read() {
        let Effect::Tracer { from, to, hit } = *effect else { continue };
        commands.spawn((
            Tracer { from, to, hit, remaining: TRACER_LIFETIME },
            Name::new("Tracer"),
        ));
    }
}

fn fade_tracers(mut commands: Commands, time: Res<Time>, mut tracers: Query<(Entity, &mut Tracer)>) {
    for (entity, mut tracer) in &mut tracers {
        tracer.remaining -= time.delta_secs();
        if tracer.remaining <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

fn draw_tracers(mut gizmos: Gizmos, tracers: Query<&Tracer>) {
    for tracer in &tracers {
        // Fading out rather than vanishing, so a burst reads as several shots
        // rather than as one line that flickers.
        let fade = (tracer.remaining / TRACER_LIFETIME).clamp(0.0, 1.0);
        gizmos.line(
            tracer.from,
            tracer.to,
            Color::srgba(1.0, 0.85, 0.4, fade),
        );
        if tracer.hit {
            gizmos.sphere(
                Isometry3d::from_translation(tracer.to),
                HIT_BALL_RADIUS,
                Color::srgba(1.0, 0.4, 0.2, fade),
            );
        }
    }
}

/// Tracers belong to the match, like the corpses and the blast marks.
fn clear_tracers(mut commands: Commands, tracers: Query<Entity, With<Tracer>>) {
    for tracer in &tracers {
        commands.entity(tracer).despawn();
    }
}

fn draw_debug_laser(
    mut gizmos: Gizmos,
    world: Res<CollisionWorld>,
    shooters: Query<(Entity, &Transform, &Player, &Stance)>,
    targets: Query<(Entity, &Hitboxes)>,
) {
    let beam_colour = Color::srgb(0.35, 0.8, 1.0);
    let body_colour = Color::srgb(0.2, 0.95, 0.35);
    let head_colour = Color::srgb(1.0, 0.15, 0.15);

    for (shooter, transform, player, stance) in &shooters {
        let ray = Ray3d::new(eye(transform.translation, *stance), aim(player));

        // A wall stops the shot at its surface, so a body behind it is behind
        // it for shooting as well as for looking.
        let range = world.ray_distance(&ray, LASER_RANGE).unwrap_or(LASER_RANGE);

        // Your own boxes are around your own eye, and every shot would land on
        // them at zero metres.
        let others = targets
            .iter()
            .filter(|(entity, _)| *entity != shooter)
            .map(|(entity, boxes)| (entity, *boxes));

        let hit = trace(&ray, range, others);

        let end = match hit {
            Some(hit) => hit.point,
            None => ray.origin + *ray.direction * range,
        };
        gizmos.line(ray.origin, end, beam_colour);

        if let Some(hit) = hit {
            let colour = match hit.zone {
                HitZone::Body => body_colour,
                HitZone::Head => head_colour,
            };
            gizmos.sphere(Isometry3d::from_translation(hit.point), HIT_BALL_RADIUS, colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::class::{body_centre_from_feet, TALLEST_CLASS_EYE_HEIGHT};
    use crate::common::hitbox::Box3;
    use crate::common::projectile::ProjectileSpec;

    /// A shooter standing at the origin looking down -Z, and a target box
    /// hanging in front of it.
    ///
    /// The boxes are placed by hand rather than posed off a rig: what is under
    /// test is the trigger, and a rig would make the test fail when a head
    /// moved rather than when the firing did.
    fn a_shooter_and_a_target(boxes: Hitboxes) -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<WeaponActionFired>>();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<RagdollShove>>();
        world.init_resource::<Messages<Effect>>();

        let centre = body_centre_from_feet(Vec3::ZERO);
        let shooter = world
            .spawn((
                Player::default(),
                PlayerId(7),
                Stance::Standing,
                PhysicsBody { previous: centre, current: centre },
                Hitboxes::default(),
            ))
            .id();
        let target = world.spawn(boxes).id();

        (world, shooter, target)
    }

    /// Boxes for a target `away` metres down -Z, at the shooter's eye height,
    /// with the head box the top of it.
    fn target_boxes(away: f32) -> Hitboxes {
        let eye = TALLEST_CLASS_EYE_HEIGHT;
        Hitboxes {
            body: Box3 {
                centre: Vec3::new(0.0, eye, -away),
                half_extents: Vec3::new(0.4, 0.9, 0.4),
            },
            head: Box3 {
                centre: Vec3::new(0.0, eye + 0.7, -away),
                half_extents: Vec3::splat(0.15),
            },
        }
    }

    /// The weapon under test. Its numbers are the ones this module used to
    /// keep as constants, so the assertions below still read as the same
    /// facts about the same shot.
    const LASER: HitscanSpec = HitscanSpec {
        range: LASER_RANGE,
        damage: 30,
        headshot_multiplier: 3,
        shove_per_damage: 0.15,
        shove_radius: 0.4,
    };

    fn fire(world: &mut World, shooter: Entity) {
        fire_action(world, shooter, WeaponAction::Hitscan(LASER));
    }

    fn fire_action(world: &mut World, shooter: Entity, action: WeaponAction) {
        world.write_message(WeaponActionFired {
            shooter,
            button: crate::game::weapon::Button::Primary,
            action,
        });
        world.run_system_once(fire_hitscan).unwrap();
    }

    fn asked_for(world: &mut World) -> Vec<Damage> {
        let messages = world.resource::<Messages<Damage>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    fn effects_of(world: &mut World) -> Vec<Effect> {
        let messages = world.resource::<Messages<Effect>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    /// Every shot leaves a tracer, including one that hit nobody.
    ///
    /// A tracer is where the bullet went, which is a fact about the world and
    /// not about whether it found anybody — written before the early return
    /// that skips the damage. Miss that and a player firing into open space
    /// sees no feedback at all and reads the weapon as jammed.
    #[test]
    fn a_shot_that_hits_nothing_still_leaves_a_tracer() {
        let (mut world, shooter, target) = a_shooter_and_a_target(target_boxes(5.0));
        // Nothing in front of the muzzle at all.
        world.entity_mut(target).despawn();
        fire(&mut world, shooter);

        assert!(asked_for(&mut world).is_empty(), "it hit something after all");

        let effects = effects_of(&mut world);
        assert_eq!(effects.len(), 1, "{effects:?}");
        assert!(matches!(effects[0], Effect::Tracer { .. }));
    }

    /// And a shot that lands says where it landed, so the mark is drawn on the
    /// body rather than at the end of the weapon's reach.
    #[test]
    fn a_tracer_stops_where_the_shot_did() {
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(5.0));
        fire(&mut world, shooter);

        let effects = effects_of(&mut world);
        let Some(Effect::Tracer { from, to, hit }) = effects.first().copied() else {
            panic!("no tracer: {effects:?}");
        };
        assert!(hit, "the shot landed but the tracer says it did not");
        assert!((to - from).length() < LASER.range, "the tracer ran to full range");
        assert!((to.z - from.z).abs() > 1.0, "the tracer went nowhere");
    }

    /// The whole path: a click asks for damage on the thing in front of you
    /// and says who is asking. What that does to a health pool is the damage
    /// layer's business — see [`crate::game::damage::apply_damage`].
    #[test]
    fn a_shot_asks_for_damage_on_what_it_hit_and_says_who_fired() {
        let (mut world, shooter, target) = a_shooter_and_a_target(target_boxes(5.0));
        fire(&mut world, shooter);

        let asked = asked_for(&mut world);
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].target, target);
        assert_eq!(asked[0].source, DamageSource::Player(PlayerId(7)));
        assert_eq!(asked[0].amount, LASER.damage);
    }

    /// A head is worth more, which is the only reason the trace distinguishes
    /// the two boxes.
    #[test]
    fn a_head_shot_is_worth_the_multiplier() {
        let mut boxes = target_boxes(5.0);
        // Aimed level, so only a head box at eye height is in the way.
        boxes.head.centre.y = TALLEST_CLASS_EYE_HEIGHT;
        let (mut world, shooter, _) = a_shooter_and_a_target(boxes);

        fire(&mut world, shooter);
        assert_eq!(asked_for(&mut world)[0].amount, LASER.damage * LASER.headshot_multiplier);
    }

    /// A shot fired with somebody else's action does nothing here, which is
    /// the whole of how several weapons share one message.
    ///
    /// Stated against the *action* rather than against what the shooter is
    /// holding, because that is what this system now reads — and it is what
    /// stops a weapon switched in the same tick changing a shot already
    /// decided on.
    #[test]
    fn a_bullet_is_only_fired_for_a_hitscan_action() {
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(5.0));

        fire_action(
            &mut world,
            shooter,
            WeaponAction::Projectile(ProjectileSpec::ROCKET),
        );
        assert!(asked_for(&mut world).is_empty(), "the launcher fired a bullet");
    }

    /// A shot fired by something that is not a player still fires, credited to
    /// the world.
    ///
    /// `fire_hitscan` used to require a `PlayerId` outright, unlike the other
    /// two firing systems, so a turret's shot would have vanished with no
    /// error at all.
    #[test]
    fn a_shot_fired_by_nobody_is_the_worlds() {
        let (mut world, shooter, target) = a_shooter_and_a_target(target_boxes(5.0));
        world.entity_mut(shooter).remove::<PlayerId>();

        fire(&mut world, shooter);

        let asked = asked_for(&mut world);
        assert_eq!(asked.len(), 1, "a shot with no shooter id vanished");
        assert_eq!(asked[0].target, target);
        assert_eq!(asked[0].source, DamageSource::World);
    }

    /// A wall in the way stops the shot, so the body behind it is safe from a
    /// weapon that only tests bodies.
    #[test]
    fn a_wall_between_you_and_a_body_stops_the_shot() {
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(20.0));
        // A room ending well short of the target: its far wall is between the
        // two.
        world.resource_mut::<CollisionWorld>().rebuild(&[crate::tool::room::Room::new(
            Vec3::new(-5.0, 0.0, -10.0),
            Vec3::new(5.0, 4.0, 5.0),
        )]);

        fire(&mut world, shooter);
        assert!(asked_for(&mut world).is_empty());
    }

    /// And you cannot shoot yourself, which you would do at zero metres every
    /// time otherwise.
    #[test]
    fn a_shot_never_lands_on_the_body_that_fired_it() {
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(5.0));
        // The shooter's own boxes, around its own eye.
        let centre = body_centre_from_feet(Vec3::ZERO);
        world.entity_mut(shooter).insert(Hitboxes {
            body: Box3 { centre, half_extents: Vec3::new(0.4, 0.9, 0.4) },
            head: Box3 { centre: centre + Vec3::Y * 0.7, half_extents: Vec3::splat(0.15) },
        });

        fire(&mut world, shooter);
        let asked = asked_for(&mut world);
        assert!(asked.iter().all(|hit| hit.target != shooter), "{asked:?}");
    }

    /// Looking up sends the beam up. The sign of the pitch is the one thing
    /// here that can be silently backwards, and a laser that dips when you
    /// raise the mouse would be found by hand rather than by the compiler.
    #[test]
    fn pitching_up_aims_up() {
        let level = aim(&Player { pitch: 0.0, ..default() });
        let up = aim(&Player { pitch: 0.5, ..default() });

        assert!(level.y.abs() < 1e-6);
        assert!(up.y > 0.0, "looking up aimed at {up:?}");
    }

    /// And the beam leaves the body the way the body is facing — `mouse_look`
    /// writes the same yaw into both, so a laser that disagreed would come out
    /// of the player's ear.
    #[test]
    fn the_beam_goes_the_way_the_body_faces() {
        let player = Player { yaw: 1.1, pitch: 0.4, ..default() };
        let facing = Quat::from_rotation_y(player.yaw) * Vec3::NEG_Z;

        let aimed = aim(&player);
        assert!(
            (aimed.xz().normalize() - facing.xz().normalize()).length() < 1e-5,
            "aimed {aimed:?} while facing {facing:?}"
        );
    }

    /// The eye is the camera's, not the body's centre and not its feet.
    #[test]
    fn the_beam_starts_at_eye_height() {
        use crate::common::class::body_centre_from_feet;

        let feet = Vec3::new(2.0, 3.0, -1.0);
        let origin = eye(body_centre_from_feet(feet), Stance::Standing);

        assert!((origin.y - feet.y - TALLEST_CLASS_EYE_HEIGHT).abs() < 1e-5, "at {origin:?}");
        assert!((origin.xz() - feet.xz()).length() < 1e-5);
    }
}
