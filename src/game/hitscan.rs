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
use crate::common::damage::{Damageable, DamageDealt, DamageSource, PlayerId};
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::{trace, HitZone};
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamagePlugin;
use crate::game::hitbox::update_hitboxes;
use crate::game::player::{step_player, PhysicsBody, Player, PlayerInput};

/// How far the debug laser reaches, in metres.
///
/// Long enough to cross any room somebody is going to build by hand, short
/// enough that it is a line and not a claim about a weapon's range. A real
/// weapon brings its own.
pub const LASER_RANGE: f32 = 200.0;

/// Radius of the ball drawn at a hit, in metres. Twenty centimetres across.
pub const HIT_BALL_RADIUS: f32 = 0.1;

/// What one shot takes off a body.
///
/// A single number for a weapon that does not exist: there is no falloff, no
/// spread and no reload, so this is the whole of the damage model. A real
/// weapon brings its own, and this constant is what gets deleted rather than
/// generalised.
pub const HITSCAN_DAMAGE: u32 = 30;

/// What a head is worth, as a multiple of [`HITSCAN_DAMAGE`].
///
/// The reason the zone is carried out of the trace at all: without it the head
/// box is an elaborate way of computing the same number as the body box.
pub const HEADSHOT_MULTIPLIER: u32 = 3;

pub struct HitscanPlugin;

impl Plugin for HitscanPlugin {
    fn build(&self, app: &mut App) {
        // With the other gizmos, after the transforms they are placed from
        // have been propagated.
        app
            // The feedback the shot produces. Brought in here because a shot
            // that dealt damage nobody could see would be a debug tool with
            // its output switched off.
            .add_plugins(DamagePlugin)
            // On the tick, after the boxes it tests against have been moved
            // to where this step left them.
            .add_systems(
                FixedUpdate,
                fire_hitscan
                    .after(update_hitboxes)
                    .after(step_player)
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(
                PostUpdate,
                draw_debug_laser
                    .after(TransformSystems::Propagate)
                    .run_if(in_state(AppMode::Play)),
            )
        ;
    }
}

/// Which way the player is looking, as a direction in world space.
///
/// Yaw and pitch rather than the camera's transform: in third person the
/// camera has swung behind the body and is no longer standing where the eye
/// is, and the eye is what a shot comes out of in both views.
fn aim(player: &Player) -> Dir3 {
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
fn eye(centre: Vec3, stance: Stance) -> Vec3 {
    centre + Vec3::Y * stance.eye_offset()
}

/// What a hit on `zone` is worth.
fn damage_for(zone: HitZone) -> u32 {
    match zone {
        HitZone::Body => HITSCAN_DAMAGE,
        HitZone::Head => HITSCAN_DAMAGE * HEADSHOT_MULTIPLIER,
    }
}

/// Pull the trigger: one shot, on the tick, for each latched press.
///
/// Nothing happens at zero health — see [`Damageable::apply`]. A body shot to
/// nothing keeps standing there and keeps taking hits worth zero, which is
/// exactly what a debug harness should do until a gamemode has an opinion
/// about death.
pub fn fire_hitscan(
    mut input: ResMut<PlayerInput>,
    world: Res<CollisionWorld>,
    shooters: Query<(Entity, &PhysicsBody, &Player, &Stance, &PlayerId)>,
    targets: Query<(Entity, &Hitboxes)>,
    mut health: Query<&mut Damageable>,
    mut dealt: MessageWriter<DamageDealt>,
) {
    // Taken once, whoever ends up firing: the latch is a record that the
    // trigger was pulled, and leaving it set would fire again next tick.
    if !std::mem::take(&mut input.attack) {
        return;
    }

    for (shooter, body, player, stance, id) in &shooters {
        // The step's own position, not the drawn one. `Transform` is written
        // by `interpolate_bodies` at frame rate, and a shot fired from it
        // would come from a slightly different place on every machine.
        let ray = Ray3d::new(eye(body.current, *stance), aim(player));
        let range = world.ray_distance(&ray, LASER_RANGE).unwrap_or(LASER_RANGE);

        let others = targets
            .iter()
            .filter(|(entity, _)| *entity != shooter)
            .map(|(entity, boxes)| (entity, *boxes));

        let Some(hit) = trace(&ray, range, others) else { continue };
        // Hit something with no health — a wall dressing, a display body
        // somebody has not given HP to. It stopped the shot all the same.
        let Ok(mut target) = health.get_mut(hit.target) else { continue };

        let amount = target.apply(damage_for(hit.zone));
        dealt.write(DamageDealt {
            target: hit.target,
            source: DamageSource::Player(*id),
            amount,
            remaining: target.health(),
            point: hit.point,
        });
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

    /// A shooter standing at the origin looking down -Z, and a target box
    /// hanging in front of it.
    ///
    /// The boxes are placed by hand rather than posed off a rig: what is under
    /// test is the trigger, and a rig would make the test fail when a head
    /// moved rather than when the firing did.
    fn a_shooter_and_a_target(boxes: Hitboxes, health: u32) -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<PlayerInput>();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<DamageDealt>>();

        let centre = body_centre_from_feet(Vec3::ZERO);
        let shooter = world
            .spawn((
                Player::default(),
                PlayerId(7),
                Stance::Standing,
                PhysicsBody { previous: centre, current: centre },
                Hitboxes::default(),
                Damageable::default(),
            ))
            .id();
        let target = world.spawn((boxes, Damageable::with_health(health))).id();

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

    fn fire(world: &mut World) {
        world.resource_mut::<PlayerInput>().attack = true;
        world.run_system_once(fire_hitscan).unwrap();
    }

    fn records(world: &mut World) -> Vec<DamageDealt> {
        let messages = world.resource::<Messages<DamageDealt>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    /// The whole path: a click takes health off the thing in front of you and
    /// leaves a record of who did it.
    #[test]
    fn a_shot_takes_health_off_and_says_who_took_it() {
        let (mut world, _, target) = a_shooter_and_a_target(target_boxes(5.0), 100);
        fire(&mut world);

        assert_eq!(world.get::<Damageable>(target).unwrap().health(), 100 - HITSCAN_DAMAGE);

        let record = records(&mut world);
        assert_eq!(record.len(), 1);
        assert_eq!(record[0].target, target);
        assert_eq!(record[0].source, DamageSource::Player(PlayerId(7)));
        assert_eq!(record[0].amount, HITSCAN_DAMAGE);
        assert_eq!(record[0].remaining, 100 - HITSCAN_DAMAGE);
    }

    /// A head is worth more, which is the only reason the trace distinguishes
    /// the two boxes.
    #[test]
    fn a_head_shot_is_worth_the_multiplier() {
        let mut boxes = target_boxes(5.0);
        // Aimed level, so only a head box at eye height is in the way.
        boxes.head.centre.y = TALLEST_CLASS_EYE_HEIGHT;
        let (mut world, _, target) = a_shooter_and_a_target(boxes, 500);

        fire(&mut world);
        assert_eq!(
            world.get::<Damageable>(target).unwrap().health(),
            500 - HITSCAN_DAMAGE * HEADSHOT_MULTIPLIER
        );
    }

    /// Overkill is recorded as what was there. The clamp lives in
    /// `Damageable`; this is the check that the shot reports the clamped
    /// number rather than the one it swung.
    #[test]
    fn a_shot_into_a_body_with_less_left_records_what_it_took() {
        let (mut world, _, _) = a_shooter_and_a_target(target_boxes(5.0), 12);
        fire(&mut world);

        let record = records(&mut world);
        assert_eq!(record[0].amount, 12);
        assert_eq!(record[0].remaining, 0);
    }

    /// One press is one shot. The latch is consumed by the step that used it,
    /// or a click held across two ticks would fire twice.
    #[test]
    fn the_trigger_is_consumed_by_the_shot_it_fires() {
        let (mut world, _, target) = a_shooter_and_a_target(target_boxes(5.0), 500);

        fire(&mut world);
        assert!(!world.resource::<PlayerInput>().attack);

        world.run_system_once(fire_hitscan).unwrap();
        assert_eq!(
            world.get::<Damageable>(target).unwrap().health(),
            500 - HITSCAN_DAMAGE,
            "the second tick fired again on the same press"
        );
    }

    /// A wall in the way stops the shot, so the body behind it is safe from a
    /// weapon that only tests bodies.
    #[test]
    fn a_wall_between_you_and_a_body_stops_the_shot() {
        let (mut world, _, target) = a_shooter_and_a_target(target_boxes(20.0), 100);
        // A room ending well short of the target: its far wall is between the
        // two.
        world.resource_mut::<CollisionWorld>().rebuild(&[crate::tool::room::Room::new(
            Vec3::new(-5.0, 0.0, -10.0),
            Vec3::new(5.0, 4.0, 5.0),
        )]);

        fire(&mut world);
        assert_eq!(world.get::<Damageable>(target).unwrap().health(), 100);
    }

    /// And you cannot shoot yourself, which you would do at zero metres every
    /// time otherwise.
    #[test]
    fn a_shot_never_lands_on_the_body_that_fired_it() {
        let (mut world, shooter, _) = a_shooter_and_a_target(target_boxes(5.0), 100);
        // The shooter's own boxes, around its own eye.
        let centre = body_centre_from_feet(Vec3::ZERO);
        world.entity_mut(shooter).insert(Hitboxes {
            body: Box3 { centre, half_extents: Vec3::new(0.4, 0.9, 0.4) },
            head: Box3 { centre: centre + Vec3::Y * 0.7, half_extents: Vec3::splat(0.15) },
        });

        fire(&mut world);
        assert_eq!(world.get::<Damageable>(shooter).unwrap().health(), 100);
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
