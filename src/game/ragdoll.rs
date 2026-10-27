//! What is left of a body once it stops holding itself up.
//!
//! A ragdoll is a second entity spawned in the dying body's pose, and the
//! original is reaped as usual by [`crate::game::death`]. Keeping the same
//! entity would leave a corpse with hitboxes, an id and a camera. It carries
//! the same [`Skeleton`], so [`BodyMeshPlugin`](crate::game::body_mesh) dresses
//! it, and never a
//! [`SkeletonAnimator`](crate::common::skeleton::SkeletonAnimator): the solver
//! here owns its pose.
//!
//! # How it is simulated
//!
//! Position-based dynamics over two points per bone, head and tail. No C
//! library, so it builds for wasm; every constraint is a length, offset or
//! limit the rig already states ([`crate::common::skeleton::joints`]); and it
//! is a function of `(state, dt)` only.
//!
//! Two points cannot express a twist, so a bone's orientation is the shortest
//! rotation from rest to where it points. The root is the exception that
//! matters, because every limit is stated against its frame; see
//! [`Ragdoll::root_seed`].
//!
//! The failures here are silent: a corpse that crawls, buzzes, hovers,
//! stretches or windmills an arm. [`LIMIT_RELAXATION`], [`STICK_SPEED`],
//! [`SLEEP_DRIFT`], [`Ragdoll::root_seed`] and the order in [`Ragdoll::step`]
//! each name the symptom they prevent. After changing one, run
//! `every_corpse_settles_however_it_lands`, which drops forty bodies because
//! whether one settles depends on how it lands.
//!
//! Momentum is not conserved: a [`RagdollShove`] is a speed at a point, not an
//! impulse. Mass (a bone's volume) decides how much of the body a shove drags
//! along, not how fast the bone that was hit leaves.
//!
//! # Where the force comes from
//!
//! [`RagdollShove`] names a point and a radius, not a bone: the bones inside
//! the radius are shoved and the joints drag the rest, so one message serves a
//! bullet and an explosion. [`fire_hitscan`] writes one for every hit; the
//! fatal one lands on the corpse raised earlier in the same tick.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::net::has_authority;
use crate::common::skeleton::rig::{humanoid, Proportions, Scratch, BONES_ON_THE_STACK};
use crate::common::damage::{falloff, PlayerId};
use crate::common::movement_rules::MovementRules;
use crate::common::skeleton::joints::limit_of;
use crate::common::skeleton::{Bone, Pose, Skeleton};
use crate::common::team::Team;
use crate::game::body_mesh::BodyTint;
use crate::game::collision::{CollisionWorld, Touched};
use crate::game::damage::DamageSystems;
use crate::game::death::{announce_the_dead, reap_the_dead, Dying};
use crate::game::player::PhysicsBody;
use crate::game::skeleton::{skeleton_root, SkeletonRoot};

/// How long a corpse lies there, in seconds of game time, so corpses do not
/// age in the editor.
///
/// Not tied to the body that left it: respawning is immediate, so clearing the
/// corpse on respawn would mean nobody ever saw it. Only the round ends it
/// early.
pub const RAGDOLL_LIFETIME: f32 = 10.0;

/// Constraint passes per tick. PBD stiffness is this number; eight keeps a leg
/// from visibly stretching.
const SOLVER_ITERATIONS: usize = 8;

/// Fraction of speed lost per tick. Numerical damping, not air resistance, so
/// constraint corrections do not feed energy back in.
const DRAG: f32 = 0.02;

/// Fraction of a contact point's sideways speed lost per tick. Gentle, so a
/// body thrown at a floor skids; [`STICK_SPEED`] is what stops it.
const FRICTION: f32 = 0.12;

/// Below this sliding speed, in metres per second, a contact point stops.
///
/// The solver leaks a fraction of a millimetre of sideways drift every tick,
/// and kinetic friction only removes a fraction of it, so without this a
/// corpse creeps across the room and never sleeps.
const STICK_SPEED: f32 = 0.25;

/// How far a bone is moved towards what its joint allows, per pass.
///
/// A hard projection fights the distance constraints and the limb oscillates
/// forever, especially at hinges. Partway, the two settle; over
/// [`SOLVER_ITERATIONS`] passes the remaining violation is a fraction of a
/// percent.
const LIMIT_RELAXATION: f32 = 0.5;

/// How far, in metres, the furthest point may drift over [`SLEEP_AFTER`]
/// before a corpse counts as settled.
///
/// A distance rather than a speed: a PBD body at rest buzzes by a few
/// millimetres a tick forever, which reads as motion when measured as speed.
const SLEEP_DRIFT: f32 = 0.02;

/// Seconds it has to stay that still before it stops being simulated. A
/// shove wakes it.
const SLEEP_AFTER: f32 = 0.5;

/// Bounds on a bone's collision sphere radius, in metres, derived from its
/// thickness. Floored because a point with no size tunnels; capped because a
/// torso-sized sphere wedges in doorways.
const POINT_RADIUS: (f32, f32) = (0.04, 0.14);

/// Which way a shove pushes what it reaches.
///
/// A bullet pushes everything the same way; an explosion pushes each bone away
/// from it. An explosion written as `Along` slides a corpse sideways instead
/// of throwing it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Push {
    /// A fixed direction; the vector's length is the speed at the centre.
    Along(Vec3),
    /// Away from the shove's point, at this speed at the centre. A bone
    /// exactly at the centre is left alone.
    Outward(f32),
}

/// A body being pushed about, at a point.
///
/// `push` is the speed, in metres per second, the bone at `at` is knocked to,
/// not an impulse to be divided by a mass.
#[derive(Message, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RagdollShove {
    /// World space.
    pub at: Vec3,
    /// Direction and speed at the centre.
    pub push: Push,
    /// Falls off linearly to nothing at the edge.
    pub radius: f32,
}

/// What a corpse was built from.
/// What a corpse was built from: the [`Proportions`] taken off the dying
/// body's own [`Skeleton`], since a `Skeleton` never crosses the wire. Read off
/// the rig rather than a class, because not every body has a class.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CorpseBuild(pub Proportions);

/// Whose body this corpse was, so the death cam can find its own. Only a
/// player's corpse has one.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpseOf(pub PlayerId);

/// A corpse.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
pub struct Ragdoll {
    /// Two per bone, in [`Skeleton::bones`] order: `2i` is bone `i`'s head and
    /// `2i + 1` its tail.
    points: Vec<Point>,
    /// The root bone's orientation at the moment of death.
    ///
    /// Two points cannot express a roll, and for the root that roll is the
    /// frame every limit is stated against. Never written after death: reading
    /// the roll off the hips each solve closes a loop (roll sets the leg
    /// limits, the limits move the legs, the legs move the hips) with gain, and
    /// over forty drops more than a third of corpses slid forever.
    ///
    /// Swung onto wherever the pelvis points instead, it has no path back in.
    /// The cost is that a roll about the pelvis's own axis after death keeps
    /// the knee plane it died with.
    root_seed: Quat,
    /// Seconds of game time.
    age: f32,
    /// How long every point has stayed within [`SLEEP_DRIFT`] of `anchor`.
    resting: f32,
    /// Where the points were when last judged to have moved.
    anchor: Vec<Vec3>,
    /// Settled, and no longer being solved.
    asleep: bool,
}

/// A body that has already had its corpse made.
///
/// [`raise_ragdolls`] runs just before reaping, so it should never see a body
/// twice, but that order is set elsewhere and breaking it would add a corpse a
/// tick.
#[derive(Component, Debug)]
pub struct Ragdolled;

/// One end of one bone, under Verlet integration.
///
/// Velocity is the gap between `at` and `previous`, so a constraint that moves
/// a point changes its velocity with nothing written.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Point {
    at: Vec3,
    previous: Vec3,
    /// Zero would pin the point. Nothing is pinned today.
    inverse_mass: f32,
    /// Half-extents of the box it collides as.
    half: Vec3,
}

impl Ragdoll {
    /// Take the pose a body is standing in and let go of it.
    ///
    /// `root` is the skeleton root (the feet) in world space. `carried` is the
    /// last tick's displacement, so a corpse keeps a runner's momentum.
    pub fn from_body(skeleton: &Skeleton, pose: &Pose, root: &Transform, carried: Vec3) -> Ragdoll {
        let posed = skeleton.posed_bones(pose, root);
        let mut points = Vec::with_capacity(posed.len() * 2);

        for bone in &posed {
            // A bone's own volume, halved between its ends.
            let mass = (bone.thickness.x * bone.thickness.y * bone.length).max(1.0e-5) * 0.5;
            let radius = (bone.thickness.min_element() * 0.5).clamp(POINT_RADIUS.0, POINT_RADIUS.1);
            for at in [bone.head, bone.tail] {
                points.push(Point {
                    at,
                    previous: at - carried,
                    inverse_mass: 1.0 / mass,
                    half: Vec3::splat(radius),
                });
            }
        }

        // Normalised so a whole body weighs one, so the shove constants mean
        // the same for every build.
        let total: f32 = points.iter().map(|point| 1.0 / point.inverse_mass).sum();
        for point in &mut points {
            point.inverse_mass *= total;
        }

        Ragdoll {
            root_seed: posed[0].rotation,
            anchor: points.iter().map(|point| point.at).collect(),
            points,
            age: 0.0,
            resting: 0.0,
            asleep: false,
        }
    }

    /// One tick: fall, hit things, and be pulled back into shape.
    ///
    /// Velocity is read back off where the body ended up, not written during
    /// the move. Otherwise a foot lifted out of the floor picks up an upward
    /// velocity for having been lifted.
    /// `fall` is the vertical acceleration, negative down.
    pub fn step(&mut self, skeleton: &Skeleton, world: &CollisionWorld, fall: f32, dt: f32) {
        self.age += dt;
        if self.asleep {
            return;
        }

        let mut started = Scratch::<Vec3, POINTS_ON_THE_STACK>::new(self.points.len(), Vec3::ZERO);
        for (was, point) in started.iter_mut().zip(&self.points) {
            *was = point.at;
        }
        let mut touching = Scratch::<Touched, POINTS_ON_THE_STACK>::new(self.points.len(), Touched::default());

        let fall = Vec3::Y * fall * dt * dt;
        for (point, contact) in self.points.iter_mut().zip(touching.iter_mut()) {
            let carried = (point.at - point.previous) * (1.0 - DRAG);
            // Swept: a point thrown by an explosion can cover more than a wall's
            // thickness in a tick.
            let (moved, touched) = world.slide(point.at, point.half, carried + fall);
            *contact = touched;
            point.at = moved;
        }

        // The walls are solved with the rig, not before or after. Rig last,
        // and a foot is pushed back through the floor a little further each
        // tick (a swept move only stops crossings) and the corpse crawls.
        // Walls last, and nothing restores a bone's length, so it stretches.
        //
        // Walls go last within a pass so nothing ends a tick inside one. A
        // bone left slightly the wrong length does not show, because
        // [`Ragdoll::pose`] draws through forward kinematics.
        let root = self.root_frame();
        for _ in 0..SOLVER_ITERATIONS {
            self.hold_together(skeleton);
            self.hold_joints(skeleton, root);
            self.clear_walls(world, &mut touching);
        }

        self.settle(dt);

        for ((point, was), contact) in self.points.iter_mut().zip(started.iter()).zip(touching.iter()) {
            let mut velocity = point.at - *was;
            // Into the surface and off it alike: a bounce is not wanted.
            for normal in contact.iter() {
                velocity -= normal * velocity.dot(normal);
            }
            // Friction is the only thing that takes angular momentum away, so
            // a corpse that lands spinning stops.
            if !contact.is_empty() {
                velocity *= 1.0 - FRICTION;
                if velocity.length() < STICK_SPEED * dt {
                    velocity = Vec3::ZERO;
                }
            }
            point.previous = point.at - velocity;
        }
    }

    /// Knock the bones near `at` along `push`, falling off to nothing at
    /// `radius`.
    ///
    /// `dt` because velocity is the gap between two positions.
    pub fn shove(&mut self, at: Vec3, push: Push, radius: f32, dt: f32) {
        if radius <= 0.0 {
            return;
        }

        let mut touched = false;
        for point in &mut self.points {
            let reach = falloff(point.at.distance(at), radius);
            if reach <= 0.0 {
                continue;
            }
            let along = match push {
                Push::Along(direction) => direction,
                // A bone exactly at the centre is left for its neighbours to
                // drag out.
                Push::Outward(speed) => (point.at - at).normalize_or_zero() * speed,
            };
            // Subtracted from `previous`, which is how velocity is stored.
            point.previous -= along * reach * dt;
            touched = true;
        }

        if touched {
            self.asleep = false;
            self.resting = 0.0;
            for (point, anchor) in self.points.iter().zip(&mut self.anchor) {
                *anchor = point.at;
            }
        }
    }

    /// Seconds of game time this corpse has lain here.
    pub fn age(&self) -> f32 {
        self.age
    }

    /// The root bone's head, in world space.
    pub fn pelvis(&self) -> Vec3 {
        self.points[0].at
    }

    /// Where the solver has bone `index`'s two ends, in world space.
    ///
    /// Not where the bone is drawn: [`Ragdoll::pose`] goes through forward
    /// kinematics, so the two differ by how far the solve is from converging.
    pub fn bone_points(&self, index: usize) -> (Vec3, Vec3) {
        (self.points[index * 2].at, self.points[index * 2 + 1].at)
    }

    pub fn is_asleep(&self) -> bool {
        self.asleep
    }

    /// Where the body is and what shape it is in, as something the rest of the
    /// skeleton layer can draw.
    ///
    /// The corpse's [`Transform`] is unrotated and the orientation goes into
    /// the pose, so a bone's rotation means the same here as on a living body.
    ///
    /// Bones are not placed at the points. Each pair gives a direction and
    /// forward kinematics places the bone, so an unconverged solve draws as a
    /// joint bent slightly too far rather than a gap in an arm.
    pub fn pose(&self, skeleton: &Skeleton) -> (Transform, Pose) {
        let mut pose = Pose::rest();
        let placed = self.pose_into(skeleton, &mut pose);
        (placed, pose)
    }

    /// [`Ragdoll::pose`], written over a pose that already exists.
    ///
    /// What the tick uses: a corpse's `Pose` is rewritten whole every tick it
    /// is awake, and building a fresh one each time is a map grown from empty
    /// per corpse per tick. Every bone is set, so the result is the same pose
    /// whatever was there before.
    pub fn pose_into(&self, skeleton: &Skeleton, pose: &mut Pose) -> Transform {
        let bones = skeleton.bones();
        let mut frames = Scratch::<Quat, BONES_ON_THE_STACK>::new(bones.len(), Quat::IDENTITY);
        pose.reset_to_rest();
        let root = self.root_frame();

        for (index, bone) in bones.iter().enumerate() {
            let (rest, world, _) = self.frame(index, bone, &frames[..index], root);
            // A bone's global rotation is `parent * bone.rest * pose`; `rest`
            // here is the first two.
            pose.set(bone.name, rest.inverse() * world);
            frames[index] = world;
        }

        // Forward kinematics puts the root's head at
        // `root.translation + root.rotation * bone.head` with identity rotation.
        Transform::from_translation(self.points[0].at - bones[0].head)
    }

    /// Bone lengths, and the offsets that hold one bone onto the next.
    ///
    /// A joint is two distances: the child's head to both its parent's head
    /// and tail. One alone lets the child orbit, and a hip, which hangs off
    /// the side of the pelvis, would swing round to the front.
    fn hold_together(&mut self, skeleton: &Skeleton) {
        let bones = skeleton.bones();
        for (index, bone) in bones.iter().enumerate() {
            self.pull(index * 2, index * 2 + 1, bone.length);

            let Some(parent) = bone.parent else { continue };
            // `bone.head` is in the parent's rest frame; distance is frame-free.
            self.pull(parent * 2, index * 2, bone.head.length());
            let from_tail = bone.head - Vec3::Y * bones[parent].length;
            self.pull(parent * 2 + 1, index * 2, from_tail.length());
        }
    }

    /// Bring every bone back inside what its joint allows.
    ///
    /// Parents first (see [`Ragdoll::frame`]). Only the tail moves: moving the
    /// head too would fight the joint constraints and the corpse hums.
    fn hold_joints(&mut self, skeleton: &Skeleton, root: Quat) {
        let bones = skeleton.bones();
        let mut frames = Scratch::<Quat, BONES_ON_THE_STACK>::new(bones.len(), Quat::IDENTITY);

        for (index, bone) in bones.iter().enumerate() {
            let (_, world, direction) = self.frame(index, bone, &frames[..index], root);
            let head = self.points[index * 2].at;

            // Partway, per `LIMIT_RELAXATION`.
            let was = (self.points[index * 2 + 1].at - head).normalize_or(direction);
            let eased = was.lerp(direction, LIMIT_RELAXATION).normalize_or(direction);

            self.points[index * 2 + 1].at = head + eased * bone.length;
            frames[index] = world;
        }
    }

    /// Where bone `index` has ended up: its frame with the joint at rest, the
    /// frame it is actually in, and the direction it points once limited.
    ///
    /// `frames` holds the answer for every earlier bone, so callers walk the
    /// rig parents-first. A limit is stated against where the parent actually
    /// is, not the world.
    fn frame(&self, index: usize, bone: &Bone, frames: &[Quat], root: Quat) -> (Quat, Quat, Vec3) {
        let along = self.points[index * 2 + 1].at - self.points[index * 2].at;

        let Some(parent) = bone.parent else {
            // A root has no joint. A limit against the world would weld the
            // body upright.
            let direction = along.try_normalize().unwrap_or(root * Vec3::Y);
            return (bone.rest, root, direction);
        };

        // Limits are in the bone's own rest frame, which is why one table
        // covers both sides of the body.
        let rest = frames[parent] * bone.rest;
        let direction = rest * limit_of(bone.name).nearest(rest.inverse() * along);
        (rest, Quat::from_rotation_arc(rest * Vec3::Y, direction) * rest, direction)
    }

    /// The root bone's frame now: [`Ragdoll::root_seed`] swung onto wherever its
    /// two points point. Derived from the seed each time, so nothing drifts.
    fn root_frame(&self) -> Quat {
        let along = self.points[1].at - self.points[0].at;
        let direction = along.try_normalize().unwrap_or(self.root_seed * Vec3::Y);
        Quat::from_rotation_arc(self.root_seed * Vec3::Y, direction) * self.root_seed
    }

    /// Lift every point back out of anything it has been pushed into.
    /// [`CollisionWorld::depenetrate`], because the shortest way out of a floor
    /// is downwards, out of the level.
    fn clear_walls(&mut self, world: &CollisionWorld, touching: &mut [Touched]) {
        for (point, contact) in self.points.iter_mut().zip(touching) {
            let out = world.depenetrate(point.at, point.half);

            // Being pushed out counts as contact. Otherwise a foot the rig
            // keeps pressing into the floor never gets friction and buzzes
            // forever; the swept move cannot see it, since a point starting
            // inside a wall never crosses one.
            let pushed = out - point.at;
            if pushed.length_squared() > 1.0e-12 {
                contact.add(pushed.normalize());
            }
            point.at = out;
        }
    }

    fn pull(&mut self, a: usize, b: usize, rest: f32) {
        let (wa, wb) = (self.points[a].inverse_mass, self.points[b].inverse_mass);
        let share = wa + wb;
        if share <= 0.0 {
            return;
        }

        let apart = self.points[b].at - self.points[a].at;
        let distance = apart.length();
        // Coincident points have no direction; a later pass will.
        if distance < 1.0e-6 {
            return;
        }

        let correction = apart * ((distance - rest) / distance);
        self.points[a].at += correction * (wa / share);
        self.points[b].at -= correction * (wb / share);
    }

    fn settle(&mut self, dt: f32) {
        let drifted = self
            .points
            .iter()
            .zip(&self.anchor)
            .any(|(point, anchor)| point.at.distance_squared(*anchor) > SLEEP_DRIFT * SLEEP_DRIFT);

        if drifted {
            for (point, anchor) in self.points.iter().zip(&mut self.anchor) {
                *anchor = point.at;
            }
            self.resting = 0.0;
            return;
        }

        self.resting += dt;
        if self.resting >= SLEEP_AFTER {
            self.asleep = true;
        }
    }
}

/// Solver points kept on the stack: two per bone.
const POINTS_ON_THE_STACK: usize = BONES_ON_THE_STACK * 2;

pub struct RagdollPlugin;

impl Plugin for RagdollPlugin {
    fn build(&self, app: &mut App) {
        app
            .add_message::<RagdollShove>()
            // `add_message` is idempotent; registered here too so corpses can
            // be assembled without the damage layer, as tests do.
            //
            // On the tick, in order: `raise_ragdolls` before `reap_the_dead`,
            // or there is no pose left to copy; `shove_ragdolls` after all of
            // `DamageSystems::Deal`, so the killing shot lands on the corpse it
            // made; `step_ragdolls` last, so a shove takes effect the tick it
            // arrives.
            .add_systems(FixedUpdate, (
                // The authority makes the corpse; everybody else is sent it.
                raise_ragdolls
                    .after(announce_the_dead)
                    .before(reap_the_dead)
                    .run_if(has_authority),
                shove_ragdolls.after(DamageSystems::Deal),
                // Not on replayed ticks: a corpse is not predicted.
                step_ragdolls.run_if(not(lightyear::prelude::is_in_rollback)),
                retire_ragdolls.run_if(has_authority),
            ).chain().run_if(in_state(AppMode::Play)))
            // Ungated: a corpse arrives from the wire whenever, with no rig
            // until this runs.
            .add_systems(Update, dress_corpses_from_elsewhere)
            .add_systems(Update, hide_the_dying)
            .add_systems(OnExit(AppMode::Play), clear_ragdolls.run_if(has_authority))
        ;
    }
}

/// Make a corpse of anything that has run out of health and had a body to
/// lose.
///
/// The authority raises it and it is replicated like any prop. Every machine
/// raising its own from the death races the body's despawn, which arrives with
/// the news of the death; narrowing that window never closes it.
///
/// What crosses is the seed: [`Ragdoll`] is `replicate_once`, and each machine
/// runs the solver from there.
///
/// The rule is the query: a [`Skeleton`] and a [`Damageable`] at zero. A crate
/// has no skeleton, and a spawn point's preview has no health.
pub fn raise_ragdolls(
    mut commands: Commands,
    bodies: Query<
        (
            Entity,
            &Skeleton,
            &Pose,
            &GlobalTransform,
            Option<&SkeletonRoot>,
            Option<&BodyTint>,
            Option<&Team>,
            Option<&PhysicsBody>,
            Option<&Name>,
            Option<&PlayerId>,
        ),
        (Added<Dying>, Without<Ragdolled>),
    >,
) {
    for (entity, skeleton, pose, global, offset, tint, team, physics, name, id) in &bodies {
        // Per-tick displacement, which is the unit the solver keeps.
        let carried = physics.map_or(Vec3::ZERO, |body| body.current - body.previous);
        let ragdoll = Ragdoll::from_body(skeleton, pose, &skeleton_root(global, offset), carried);

        // Posed now, so the corpse is never drawn for a frame at the origin in
        // its rest pose.
        let (transform, pose) = ragdoll.pose(skeleton);

        let mut corpse = commands.spawn((
            ragdoll,
            // Not a `SkeletonAnimator`: the solver owns this pose.
            skeleton.clone(),
            pose,
            transform,
            Name::new(match name {
                Some(name) => format!("Ragdoll of {name}"),
                None => "Ragdoll".to_string(),
            }),
        ));
        if let Some(tint) = tint {
            corpse.insert(*tint);
        }
        if let Some(team) = team {
            // Corpses keep their team colour, so a pile of them shows which way
            // a fight went.
            corpse.insert(*team);
        }
        if let Some(id) = id {
            corpse.insert(CorpseOf(*id));
        }

        // Read off the rig the body actually had.
        corpse.insert(CorpseBuild(skeleton.proportions()));

        commands.entity(entity).insert(Ragdolled);
    }
}

/// Build the rig for a corpse that arrived from somewhere else.
///
/// Not [`dress_bodies_from_elsewhere`](crate::game::skeleton), which hands out
/// a `SkeletonAnimator`. Posed immediately, so the corpse does not appear at
/// the origin for a frame.
///
/// Keyed on the absence of a rig, not `Added<Ragdoll>`: the seed and the build
/// can arrive in different packets (three in eight, measured), and keyed on
/// the seed a late build leaves the corpse invisible forever.
pub fn dress_corpses_from_elsewhere(
    mut commands: Commands,
    corpses: Query<(Entity, &Ragdoll, &CorpseBuild), Without<Skeleton>>,
) {
    for (entity, ragdoll, build) in &corpses {
        let skeleton = humanoid(build.0);
        let (transform, pose) = ragdoll.pose(&skeleton);
        commands
            .entity(entity)
            .insert((skeleton, pose, transform, Visibility::default()));
    }
}

/// Hide a body that has died, on every machine alike.
///
/// The body outlives its death by a tick, so the corpse's spawn reaches the
/// wire before the despawn; without this it stands inside its own corpse.
/// Keyed on [`Dying`] because `raise_ragdolls` runs on the authority only.
pub fn hide_the_dying(mut commands: Commands, dying: Query<Entity, Added<Dying>>) {
    for body in &dying {
        commands.entity(body).insert(Visibility::Hidden);
    }
}

/// Push every corpse each shove reaches.
///
/// Every corpse in reach, not the one that was hit: an explosion between two
/// bodies moves both.
pub fn shove_ragdolls(
    time: Res<Time<Fixed>>,
    mut shoves: MessageReader<RagdollShove>,
    mut corpses: Query<&mut Ragdoll>,
) {
    let dt = time.delta_secs();
    // Read even when there is nothing to push, or a later corpse takes the
    // whole backlog at once.
    let shoves: Vec<RagdollShove> = shoves.read().copied().collect();
    if shoves.is_empty() {
        return;
    }

    for mut corpse in &mut corpses {
        for shove in &shoves {
            corpse.shove(shove.at, shove.push, shove.radius, dt);
        }
    }
}

/// Fall, settle, and eventually be taken away.
///
/// `Time<Fixed>`, so corpses step with everything else and do not age in the
/// editor. In parallel: a corpse reads only its own components and the walls,
/// so stepping order cannot change any result.
pub fn step_ragdolls(
    time: Res<Time<Fixed>>,
    world: Res<CollisionWorld>,
    rules: Res<MovementRules>,
    mut corpses: Query<(&mut Ragdoll, &Skeleton, &mut Transform, &mut Pose)>,
) {
    let dt = time.delta_secs();
    let fall = rules.fall();
    let world = &*world;
    corpses.par_iter_mut().for_each(|(mut corpse, skeleton, mut transform, mut pose)| {
        if corpse.age() >= RAGDOLL_LIFETIME {
            return;
        }

        let settled = corpse.is_asleep();
        corpse.step(skeleton, world, fall, dt);
        // Writing a settled pose would dirty change detection every tick.
        if settled && corpse.is_asleep() {
            return;
        }

        *transform = corpse.pose_into(skeleton, &mut pose);
    });
}

/// Take away every corpse that has lain there long enough.
///
/// Authority only: a corpse is replicated, so a client must not remove it,
/// and its copy started ageing later anyway.
pub fn retire_ragdolls(mut commands: Commands, corpses: Query<(Entity, &Ragdoll)>) {
    for (entity, corpse) in &corpses {
        if corpse.age() >= RAGDOLL_LIFETIME {
            commands.entity(entity).despawn();
        }
    }
}

/// Take every corpse away.
///
/// Called on leaving Play and from
/// [`reset_for_play`](crate::game::reset::reset_for_play), to cover a match
/// started by any route.
pub fn clear_ragdolls(mut commands: Commands, corpses: Query<Entity, With<Ragdoll>>) {
    for corpse in &corpses {
        commands.entity(corpse).despawn();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use crate::common::damage::Damageable;

    use super::*;
    use crate::common::damage::{DamageLog, PlayerId};
    use crate::common::skeleton::rig::{bone, humanoid, PosedBone, Proportions};
    use crate::tool::room::Room;

    /// One fixed step at 64 Hz, in seconds.
    const DT: f32 = 1.0 / 64.0;
    const FALL: f32 = -crate::common::movement_rules::DEFAULT_GRAVITY;

    fn rig() -> Skeleton {
        humanoid(Proportions::DEFAULT)
    }

    /// A body standing on the floor, freshly dead.
    fn stood_up(skeleton: &Skeleton, pose: &Pose) -> Ragdoll {
        Ragdoll::from_body(skeleton, pose, &Transform::IDENTITY, Vec3::ZERO)
    }

    fn a_room() -> CollisionWorld {
        let mut world = CollisionWorld::default();
        world.rebuild(&[Room::new(
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 8.0, 10.0),
        )]);
        world
    }

    fn run(corpse: &mut Ragdoll, skeleton: &Skeleton, world: &CollisionWorld, steps: usize) {
        for _ in 0..steps {
            corpse.step(skeleton, world, FALL, DT);
        }
    }

    /// Where a named bone is drawn, which is what the mesh is placed from.
    fn drawn(corpse: &Ragdoll, skeleton: &Skeleton, name: &str) -> PosedBone {
        let (root, pose) = corpse.pose(skeleton);
        let index = skeleton.index_of(name).expect("no such bone");
        skeleton.posed_bones(&pose, &root)[index]
    }

    /// Before any step, the corpse is drawn exactly where and how the body
    /// was; otherwise it twitches at the moment of death.
    #[test]
    fn a_corpse_starts_where_the_body_was_standing_and_in_its_pose() {
        let skeleton = rig();
        let pose = Pose::rest()
            .with(bone::CHEST, Quat::from_rotation_x(0.4))
            .with(bone::SHIN_L, Quat::from_rotation_x(-0.9))
            .with(bone::UPPER_ARM_R, Quat::from_rotation_z(0.7));

        let root = Transform::from_translation(Vec3::new(3.0, 0.0, -2.0))
            .with_rotation(Quat::from_rotation_y(1.1));
        let was = skeleton.posed_bones(&pose, &root);

        let corpse = Ragdoll::from_body(&skeleton, &pose, &root, Vec3::ZERO);
        let (placed, held) = corpse.pose(&skeleton);
        let now = skeleton.posed_bones(&held, &placed);

        for (before, after) in was.iter().zip(&now) {
            assert!(
                before.head.abs_diff_eq(after.head, 1.0e-3)
                    && before.tail.abs_diff_eq(after.tail, 1.0e-3),
                "{} moved: {} -> {}",
                before.name,
                before.head,
                after.head
            );
        }
    }

    /// A turned body's facing survives in the pose, since the corpse's own
    /// transform is unrotated.
    #[test]
    fn a_corpse_of_a_turned_body_faces_the_same_way() {
        let skeleton = rig();
        let root = Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));

        let corpse = Ragdoll::from_body(&skeleton, &Pose::rest(), &root, Vec3::ZERO);
        let (placed, _) = corpse.pose(&skeleton);
        assert!(
            placed.rotation.abs_diff_eq(Quat::IDENTITY, 1.0e-5),
            "the corpse's own frame should be the world's"
        );

        // After a quarter turn left, the right hand is at world -Z.
        let hand = drawn(&corpse, &skeleton, bone::HAND_R);
        assert!(hand.head.z < -0.2, "the facing was lost: right hand at {}", hand.head);
    }

    /// With no map at all, a corpse falls.
    #[test]
    fn a_corpse_with_nothing_under_it_falls() {
        let skeleton = rig();
        let mut corpse = stood_up(&skeleton, &Pose::rest());
        let start = drawn(&corpse, &skeleton, bone::PELVIS).head.y;

        run(&mut corpse, &skeleton, &CollisionWorld::default(), 64);

        assert!(
            drawn(&corpse, &skeleton, bone::PELVIS).head.y < start - 1.0,
            "did not fall"
        );
    }

    /// With a floor, it lands on it rather than through it.
    #[test]
    fn a_corpse_lands_on_the_floor_rather_than_through_it() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        run(&mut corpse, &skeleton, &world, 64 * 6);

        let (root, pose) = corpse.pose(&skeleton);
        for placed in skeleton.posed_bones(&pose, &root) {
            assert!(
                placed.head.y > -0.3,
                "{} went through the floor to {}",
                placed.name,
                placed.head
            );
        }
    }

    /// A corpse collapses under its own weight with nothing pushing it.
    #[test]
    fn a_corpse_falls_over_on_its_own() {
        let skeleton = rig();
        let world = a_room();
        let standing = skeleton.proportions().height;
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        run(&mut corpse, &skeleton, &world, 64 * 8);

        let head = drawn(&corpse, &skeleton, bone::HEAD).tail;
        assert!(
            head.y < standing * 0.5,
            "still standing: the crown is at {} of a {standing} m body",
            head.y
        );
    }

    /// Bones do not stretch. Asks the solver directly, since forward kinematics
    /// would hide it.
    #[test]
    fn a_corpse_that_is_thrown_about_does_not_stretch() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        // The wrist is the lightest point furthest from the pelvis. A headshot
        // writes about 13 m/s, so this is a rocket.
        let (wrist, _) = corpse.bone_points(skeleton.index_of(bone::HAND_R).unwrap());
        corpse.shove(wrist, Push::Along(Vec3::new(18.0, 11.0, 0.0)), 0.3, DT);
        run(&mut corpse, &skeleton, &world, 64 * 4);

        for (index, bone) in skeleton.bones().iter().enumerate() {
            let (head, tail) = corpse.bone_points(index);
            let length = head.distance(tail);
            assert!(
                (length - bone.length).abs() < bone.length * 0.02,
                "{} is {length} m long, not {}",
                bone.name,
                bone.length
            );
        }
    }

    /// Joints hold: each bone's head stays at its parent's.
    #[test]
    fn a_corpse_that_is_thrown_about_stays_in_one_piece() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        let (ankle, _) = corpse.bone_points(skeleton.index_of(bone::FOOT_L).unwrap());
        corpse.shove(ankle, Push::Along(Vec3::new(-14.0, 14.0, 9.0)), 0.3, DT);
        run(&mut corpse, &skeleton, &world, 64 * 4);

        for (index, bone) in skeleton.bones().iter().enumerate() {
            let Some(parent) = bone.parent else { continue };
            let (_, parent_tail) = corpse.bone_points(parent);
            let (head, _) = corpse.bone_points(index);

            let rest = (bone.head - Vec3::Y * skeleton.bones()[parent].length).length();
            assert!(
                (head.distance(parent_tail) - rest).abs() < 0.05,
                "{} came off its parent",
                bone.name
            );
        }
    }

    /// A knee never bends forwards, checked on the drawn pose.
    #[test]
    fn no_amount_of_throwing_bends_a_knee_forwards() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        // Up the front of the shin, the direction that would hyperextend it.
        let (knee, _) = corpse.bone_points(skeleton.index_of(bone::SHIN_L).unwrap());
        corpse.shove(knee, Push::Along(Vec3::new(0.0, 4.0, -35.0)), 0.35, DT);

        for _ in 0..64 * 4 {
            corpse.step(&skeleton, &world, FALL, DT);

            let (_, pose) = corpse.pose(&skeleton);
            // The joint's own rotation, so this reads the bend itself.
            let bend = pose.joint(bone::SHIN_L);
            let tail = bend * Vec3::Y;
            assert!(tail.z <= 0.1, "the knee bent forwards: {tail}");
        }
    }

    /// An explosion under a corpse lifts it; written as `Along` it would slide
    /// it sideways.
    #[test]
    fn a_blast_underneath_throws_a_corpse_upwards() {
        let skeleton = rig();
        let world = CollisionWorld::default();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        let (pelvis, _) = corpse.bone_points(0);
        let under = pelvis - Vec3::Y * 0.5;
        corpse.shove(under, Push::Outward(14.0), 2.0, DT);
        run(&mut corpse, &skeleton, &world, 2);

        let (after, _) = corpse.bone_points(0);
        assert!(after.y > pelvis.y + 0.005, "the blast did not lift it: {pelvis} -> {after}");
    }

    /// A tight shove moves the bone it reaches more than the rest. A solver
    /// moving the body as a lump would pass a test watching only the pelvis.
    #[test]
    fn a_tight_shove_moves_the_bone_it_names_more_than_the_rest() {
        let skeleton = rig();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        let head_index = skeleton.index_of(bone::HEAD).unwrap();
        let (before_head, _) = corpse.bone_points(head_index);
        let (before_foot, _) = corpse.bone_points(skeleton.index_of(bone::FOOT_L).unwrap());

        corpse.shove(before_head, Push::Along(Vec3::new(0.0, 0.0, -12.0)), 0.4, DT);
        // Enough steps for the shove to land, not for the body to catch up.
        run(&mut corpse, &skeleton, &CollisionWorld::default(), 2);

        let (after_head, _) = corpse.bone_points(head_index);
        let (after_foot, _) = corpse.bone_points(skeleton.index_of(bone::FOOT_L).unwrap());

        let head_moved = (after_head - before_head).z.abs();
        let foot_moved = (after_foot - before_foot).z.abs();
        assert!(head_moved > 0.02, "the head was not shoved at all");
        assert!(
            head_moved > foot_moved * 4.0,
            "the whole body moved as a lump: head {head_moved}, foot {foot_moved}"
        );
    }

    /// The joints drag the rest of the body after a shoved bone.
    #[test]
    fn what_the_shove_reached_pulls_the_rest_of_the_body_after_it() {
        let skeleton = rig();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        let head_index = skeleton.index_of(bone::HEAD).unwrap();
        let chest_index = skeleton.index_of(bone::CHEST).unwrap();
        let (at, _) = corpse.bone_points(head_index);
        let (before_chest, _) = corpse.bone_points(chest_index);

        corpse.shove(at, Push::Along(Vec3::new(0.0, 0.0, -12.0)), 0.4, DT);
        run(&mut corpse, &skeleton, &CollisionWorld::default(), 30);

        let (after_chest, _) = corpse.bone_points(chest_index);
        assert!(
            (after_chest - before_chest).z < -0.05,
            "the chest was not dragged along: {before_chest} -> {after_chest}"
        );
    }

    /// Every one of forty deterministic drops falls asleep. Whether a single
    /// drop settles depends on how it lands; a failure here means something is
    /// feeding energy back into the solver, not a constant to nudge.
    #[test]
    fn every_corpse_settles_however_it_lands() {
        let skeleton = rig();
        let world = a_room();

        let mut restless = Vec::new();
        for case in 0..40 {
            // Deterministic: a randomised sweep that failed one run in ten
            // would be worse than none.
            let a = case as f32 * 0.9;
            let pose = Pose::rest()
                .with(bone::CHEST, Quat::from_rotation_x(a.sin() * 0.5))
                .with(bone::THIGH_L, Quat::from_rotation_x(a.cos() * 0.5));
            let root = Transform::from_translation(Vec3::new(a.cos(), 0.0, a.sin()))
                .with_rotation(Quat::from_rotation_y(a));

            let mut corpse =
                Ragdoll::from_body(&skeleton, &pose, &root, Vec3::new(a.sin(), 0.0, a.cos()) * 0.1);
            let (head, _) = corpse.bone_points(skeleton.index_of(bone::HEAD).unwrap());
            corpse.shove(head, Push::Along(Vec3::new(a.cos() * 9.0, 2.0, a.sin() * 9.0)), 0.4, DT);

            run(&mut corpse, &skeleton, &world, 64 * 12);
            if !corpse.is_asleep() {
                restless.push(case);
            }
        }

        assert!(restless.is_empty(), "still moving after twelve seconds: {restless:?}");
    }

    /// A shove wakes a settled corpse.
    #[test]
    fn a_shove_wakes_a_settled_corpse() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());
        run(&mut corpse, &skeleton, &world, 64 * 15);
        assert!(corpse.is_asleep());

        let (pelvis, _) = corpse.bone_points(0);
        corpse.shove(pelvis, Push::Along(Vec3::new(6.0, 3.0, 0.0)), 1.0, DT);
        assert!(!corpse.is_asleep(), "slept through being shot");

        let before = corpse.bone_points(0).0;
        run(&mut corpse, &skeleton, &world, 8);
        assert!(corpse.bone_points(0).0.distance(before) > 0.05, "woke but did not move");
    }

    /// A shove out of range leaves a corpse alone, so every shot can write one.
    #[test]
    fn a_shove_out_of_range_is_ignored() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());
        run(&mut corpse, &skeleton, &world, 64 * 15);

        corpse.shove(Vec3::new(50.0, 0.0, 0.0), Push::Along(Vec3::X * 100.0), 0.4, DT);
        assert!(corpse.is_asleep(), "woken by a shot on the other side of the map");
    }

    /// Mass decides how much of the body a shove drags: a hand barely moves a
    /// pelvis, and a pelvis moves a hand easily.
    #[test]
    fn a_light_bone_cannot_drag_a_heavy_one_the_way_a_heavy_one_drags_it() {
        let skeleton = rig();
        let world = CollisionWorld::default();

        // Measured against the same drop without the shove, since a corpse in
        // free space is collapsing all the while.
        let settled = |pushed: Option<&str>, watched: usize| {
            let mut corpse = stood_up(&skeleton, &Pose::rest());
            if let Some(pushed) = pushed {
                let (at, _) = corpse.bone_points(skeleton.index_of(pushed).unwrap());
                corpse.shove(at, Push::Along(Vec3::X * 10.0), 0.25, DT);
            }
            run(&mut corpse, &skeleton, &world, 30);
            corpse.bone_points(watched).0
        };

        let carried = |pushed: &str, watched: &str| {
            let watched = skeleton.index_of(watched).unwrap();
            settled(Some(pushed), watched).distance(settled(None, watched))
        };

        let pelvis_pulled_by_a_hand = carried(bone::HAND_R, bone::PELVIS);
        let hand_pulled_by_the_pelvis = carried(bone::PELVIS, bone::HAND_R);

        assert!(
            hand_pulled_by_the_pelvis > pelvis_pulled_by_a_hand * 3.0,
            "the body has no sense of weight: a hand moved the pelvis \
             {pelvis_pulled_by_a_hand} m, the pelvis moved a hand \
             {hand_pulled_by_the_pelvis} m"
        );
    }

    /// A body with a skeleton, at zero health, marked `Dying`.
    fn a_dead_body(world: &mut World, extras: impl Bundle) -> Entity {
        let mut health = Damageable::with_health(100);
        health.apply(100);
        let body = world
            .spawn((
                health,
                DamageLog::default(),
                rig(),
                Pose::rest(),
                Transform::IDENTITY,
                GlobalTransform::default(),
                extras,
            ))
            .id();
        announce(world, body);
        body
    }

    /// As `announce_the_dead` would, or replication on a client.
    fn announce(world: &mut World, victim: Entity) {
        world.entity_mut(victim).insert(Dying);
    }

    fn corpses(world: &mut World) -> usize {
        world.query::<&Ragdoll>().iter(world).count()
    }

    #[test]
    fn a_humanoid_that_runs_out_of_health_leaves_a_body() {
        let mut world = World::new();
        a_dead_body(&mut world, PlayerId(3));
        world.run_system_once(raise_ragdolls).unwrap();

        assert_eq!(corpses(&mut world), 1);
    }

    /// A dead thing with no skeleton, such as a crate, leaves no corpse.
    #[test]
    fn a_dead_thing_with_no_body_leaves_nothing() {
        let mut world = World::new();
        let mut health = Damageable::with_health(20);
        health.apply(20);
        let crate_entity = world
            .spawn((health, DamageLog::default(), Transform::IDENTITY, GlobalTransform::default()))
            .id();
        announce(&mut world, crate_entity);
        world.run_system_once(raise_ragdolls).unwrap();

        assert_eq!(corpses(&mut world), 0);
    }

    /// A body nobody announced is left standing.
    #[test]
    fn a_body_nobody_announced_is_left_standing() {
        let mut world = World::new();
        world.spawn((
            Damageable::with_health(100),
            DamageLog::default(),
            rig(),
            Pose::rest(),
            Transform::IDENTITY,
            GlobalTransform::default(),
        ));
        world.run_system_once(raise_ragdolls).unwrap();

        assert_eq!(corpses(&mut world), 0);
    }

    /// A body yields only one corpse, even if the reaping order in another
    /// module breaks.
    #[test]
    fn a_body_is_only_ever_made_into_one_corpse() {
        let mut world = World::new();
        a_dead_body(&mut world, ());

        for _ in 0..5 {
            world.run_system_once(raise_ragdolls).unwrap();
        }
        assert_eq!(corpses(&mut world), 1);
    }

    /// A corpse has no `SkeletonAnimator`, or it lies there playing its idle.
    #[test]
    fn a_corpse_is_not_animated() {
        use crate::common::skeleton::SkeletonAnimator;

        let mut world = World::new();
        a_dead_body(&mut world, SkeletonAnimator::default());
        world.run_system_once(raise_ragdolls).unwrap();

        let mut corpses = world.query_filtered::<Entity, With<Ragdoll>>();
        let corpse = corpses.single(&world).unwrap();
        assert!(world.get::<Skeleton>(corpse).is_some(), "a corpse needs its rig to be drawn");
        assert!(world.get::<SkeletonAnimator>(corpse).is_none(), "the corpse animates itself");
    }

    /// A corpse keeps the body's tint.
    #[test]
    fn a_corpse_keeps_the_colour_the_body_was() {
        let mut world = World::new();
        let tint = BodyTint(Color::srgb(0.9, 0.1, 0.1));
        a_dead_body(&mut world, tint);
        world.run_system_once(raise_ragdolls).unwrap();

        let mut corpses = world.query_filtered::<&BodyTint, With<Ragdoll>>();
        let kept = corpses.single(&world).unwrap();
        assert_eq!(kept.0, tint.0);
    }

    /// The shot that kills a body throws the corpse it makes.
    #[test]
    fn the_shot_that_kills_a_body_throws_the_corpse_it_makes() {
        // `step_ragdolls` steps corpses in parallel, which needs the pool
        // `TaskPoolPlugin` stands up in the game. A bare `World` has none, and
        // the pool is global to the process — so without this the test passes
        // or panics depending on whether some other test happened to make one.
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<MovementRules>();
        world.init_resource::<Messages<RagdollShove>>();
        // An unticked fixed clock would step zero time and move nothing.
        world.init_resource::<Time<Fixed>>();
        world
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_micros(15_625));

        let feet = Vec3::new(2.0, 0.0, 0.0);
        // Propagation is not running, so `GlobalTransform` is set by hand.
        let placed = Transform::from_translation(feet);
        let body = a_dead_body(&mut world, ());
        world.entity_mut(body).insert((placed, GlobalTransform::from(placed)));

        world.run_system_once(raise_ragdolls).unwrap();

        let raised = {
            let mut corpses = world.query::<&Ragdoll>();
            corpses.single(&world).unwrap().bone_points(0).0
        };
        assert!((raised.x - feet.x).abs() < 0.01, "raised at {raised}, not over {feet}");

        world.write_message(RagdollShove {
            at: raised,
            push: Push::Along(Vec3::X * 20.0),
            radius: 1.0,
        });
        world.run_system_once(shove_ragdolls).unwrap();
        world.run_system_once(step_ragdolls).unwrap();

        let moved = {
            let mut corpses = world.query::<&Ragdoll>();
            corpses.single(&world).unwrap().bone_points(0).0
        };
        assert!(moved.x > raised.x + 0.001, "the shot did not move the corpse: {raised} -> {moved}");
    }

    /// A client makes no corpse of its own (it is sent one) and hides the body
    /// it is told died.
    #[test]
    fn a_client_makes_no_corpse_and_hides_the_body_it_is_told_died() {
        use crate::common::net::NetRole;
        use crate::game::damage::DamagePlugin;
        use crate::game::GamePlugin;

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::input::InputPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.add_plugins(GamePlugin);
        app.add_plugins(DamagePlugin);
        app.insert_resource(NetRole::Client { host: "elsewhere".into(), port: 27100 });
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();

        let placed = Transform::from_translation(Vec3::new(4.0, 0.0, 1.0));
        let body = app
            .world_mut()
            .spawn((
                rig(),
                Pose::rest(),
                placed,
                GlobalTransform::from(placed),
                Damageable::with_health(100),
                DamageLog::default(),
                PlayerId(11),
            ))
            .id();
        app.update();

        // As replication delivers it.
        app.world_mut().get_mut::<Damageable>(body).unwrap().apply(100);
        app.world_mut().entity_mut(body).insert(Dying);
        app.world_mut().run_schedule(FixedUpdate);
        app.update();

        let mut corpses = app.world_mut().query::<&Ragdoll>();
        assert_eq!(
            corpses.iter(app.world()).count(),
            0,
            "the client raised a corpse of its own; it is sent one"
        );
        assert!(
            app.world().get_entity(body).is_ok(),
            "the client removed a body that is the server's to remove"
        );
        assert_eq!(
            app.world().get::<Visibility>(body),
            Some(&Visibility::Hidden),
            "the body was left standing where its corpse is about to arrive"
        );
    }

    /// A corpse from the wire is dressed and posed from its build, and never
    /// given an animator.
    #[test]
    fn a_corpse_from_the_wire_is_dressed_by_its_build() {
        use crate::common::skeleton::SkeletonAnimator;

        let skeleton = humanoid(Proportions::DEFAULT);
        let placed = Transform::from_translation(Vec3::new(2.0, 1.0, -3.0));
        let seed = Ragdoll::from_body(&skeleton, &Pose::rest(), &placed, Vec3::ZERO);

        let mut app = App::new();
        app.add_systems(Update, dress_corpses_from_elsewhere);
        let corpse = app
            .world_mut()
            .spawn((seed, CorpseBuild(Proportions::DEFAULT)))
            .id();
        app.update();

        assert!(app.world().get::<Skeleton>(corpse).is_some(), "the corpse was never dressed");
        assert!(app.world().get::<Pose>(corpse).is_some(), "the corpse has no pose to draw");
        assert!(
            app.world().get::<SkeletonAnimator>(corpse).is_none(),
            "a corpse was given an animator to argue with the solver"
        );
        let at = app.world().get::<Transform>(corpse).unwrap().translation;
        assert!(
            at.distance(placed.translation) < 1.0,
            "the corpse was not posed where it died: {at} against {}",
            placed.translation
        );
    }

    /// A corpse carries the build read off the rig, so a body with no `Class`
    /// can still be dressed on other machines.
    #[test]
    fn a_classless_corpse_carries_the_build_it_was_rigged_from() {
        use crate::common::class::Class;
        use crate::game::damage::DamagePlugin;
        use crate::game::GamePlugin;

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::input::InputPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.add_plugins(GamePlugin);
        app.add_plugins(DamagePlugin);
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();

        let placed = Transform::from_translation(Vec3::new(1.0, 0.0, 2.0));
        let body = app
            .world_mut()
            .spawn((
                rig(),
                Pose::rest(),
                placed,
                GlobalTransform::from(placed),
                Damageable::with_health(100),
                DamageLog::default(),
                PlayerId(4),
            ))
            .id();
        app.update();
        assert!(
            app.world().get::<Class>(body).is_none(),
            "this test is worthless if a body has a class to fall back on"
        );

        app.world_mut().get_mut::<Damageable>(body).unwrap().apply(100);
        app.world_mut().run_schedule(FixedUpdate);

        let mut corpses = app.world_mut().query::<&CorpseBuild>();
        let build = corpses
            .single(app.world())
            .expect("no corpse, or a corpse nobody else could draw");
        assert_eq!(
            build.0,
            rig().proportions(),
            "the corpse was built from something other than the rig it came from"
        );
    }

    /// `raise_ragdolls` sees a body before `reap_the_dead` removes it, with
    /// both plugins assembled. If the cross-plugin `.before()` breaks, bodies
    /// silently stop leaving corpses.
    #[test]
    fn a_body_that_dies_in_the_running_game_leaves_a_corpse_behind_it() {
        use crate::game::damage::DamagePlugin;
        use crate::game::GamePlugin;

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::input::InputPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.add_plugins(GamePlugin);
        app.add_plugins(DamagePlugin);
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();

        let feet = Vec3::new(4.0, 0.0, 1.0);
        let placed = Transform::from_translation(feet);
        let body = app
            .world_mut()
            .spawn((
                rig(),
                Pose::rest(),
                placed,
                GlobalTransform::from(placed),
                Damageable::with_health(100),
                DamageLog::default(),
                PlayerId(11),
            ))
            .id();
        app.update();

        app.world_mut().get_mut::<Damageable>(body).unwrap().apply(100);
        app.world_mut().run_schedule(FixedUpdate);

        assert!(app.world().get_entity(body).is_err(), "the body was not reaped");

        let mut corpses = app.world_mut().query::<&Ragdoll>();
        let corpse = corpses.single(app.world()).expect("no corpse");
        assert!(
            corpse.bone_points(0).0.distance(feet + Vec3::Y * rig().proportions().hip_metres())
                < 0.05,
            "the corpse was not raised where the body was standing"
        );
    }


    /// Clearing takes every corpse away.
    #[test]
    fn leaving_the_match_takes_every_corpse_with_it() {
        let mut world = World::new();
        a_dead_body(&mut world, ());
        world.run_system_once(raise_ragdolls).unwrap();
        assert_eq!(corpses(&mut world), 1);

        world.run_system_once(clear_ragdolls).unwrap();
        assert_eq!(corpses(&mut world), 0);
    }

    /// Radius zero does nothing, rather than dividing by zero.
    #[test]
    fn a_shove_with_no_reach_does_nothing() {
        let skeleton = rig();
        let mut corpse = stood_up(&skeleton, &Pose::rest());
        let before = corpse.bone_points(0).0;

        corpse.shove(before, Push::Along(Vec3::X * 100.0), 0.0, DT);
        run(&mut corpse, &skeleton, &CollisionWorld::default(), 1);

        assert!((corpse.bone_points(0).0.x - before.x).abs() < 1.0e-4);
    }

    /// A handful of corpses dropped, shoved and stepped, folded down to one
    /// number over the bits of every point and every drawn joint.
    fn fingerprint() -> u64 {
        let skeleton = rig();
        let world = a_room();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut fold = |bits: u32| {
            hash ^= u64::from(bits);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        };

        for case in 0..6 {
            let a = case as f32 * 1.3;
            let pose = Pose::rest()
                .with(bone::CHEST, Quat::from_rotation_x(a.sin() * 0.5))
                .with(bone::THIGH_L, Quat::from_rotation_x(a.cos() * 0.5));
            let root = Transform::from_translation(Vec3::new(a.cos(), 0.0, a.sin()))
                .with_rotation(Quat::from_rotation_y(a));
            let mut corpse =
                Ragdoll::from_body(&skeleton, &pose, &root, Vec3::new(a.sin(), 0.0, a.cos()) * 0.1);

            for tick in 0..160 {
                if tick == 40 {
                    let (head, _) = corpse.bone_points(skeleton.index_of(bone::HEAD).unwrap());
                    corpse.shove(head, Push::Outward(7.0), 0.8, DT);
                }
                corpse.step(&skeleton, &world, FALL, DT);
            }

            for point in &corpse.points {
                for value in point.at.to_array().into_iter().chain(point.previous.to_array()) {
                    fold(value.to_bits());
                }
            }
            let (placed, drawn) = corpse.pose(&skeleton);
            placed.translation.to_array().into_iter().for_each(|value| fold(value.to_bits()));
            for bone in skeleton.bones() {
                drawn.joint(bone.name).to_array().into_iter().for_each(|value| fold(value.to_bits()));
            }
        }
        hash
    }

    /// The solver's arithmetic is pinned, not merely its behaviour.
    ///
    /// A corpse is replicated as a seed and every machine runs this solver
    /// from it, so a change that moved the result by one bit is a corpse that
    /// lies differently on two screens — and none of the settling tests would
    /// notice, since a corpse that settles a millimetre to the left still
    /// settles. The number was recorded from the solver before its working
    /// lists moved onto the stack. If a change *means* to alter the
    /// simulation, record it again; if it did not mean to, it has.
    ///
    /// **There are two numbers, one per build profile**, and that is a finding
    /// rather than a convenience: an optimised build reaches different bits
    /// from a debug build of the very same code, measured on the solver as it
    /// stood before this test existed. So "every machine runs the same
    /// solver" holds between two builds of the same profile and target, and
    /// not beyond — a debug client watching a release server already sees a
    /// corpse lie slightly differently.
    ///
    /// Both were measured on aarch64 macOS, and the bits depend on the target
    /// too (`acos` is the platform's), so they are only checked there.
    /// Everywhere, the same seed has to reach the same bits twice in one
    /// build, which is the property a replicated corpse actually leans on.
    #[test]
    fn the_solver_reaches_the_same_bits_it_always_has() {
        let reached = fingerprint();
        assert_eq!(reached, fingerprint(), "one seed reached two answers in one build");

        if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
            let expected = if cfg!(debug_assertions) {
                0x1cda_b6fd_79ed_e0ca
            } else {
                0x93d1_f1d3_b313_e938
            };
            assert_eq!(reached, expected);
        }
    }

    /// The tick writes over the pose the corpse already has, and what was
    /// there must not survive into the answer — a stale joint or root offset
    /// would draw on top of the solver's.
    #[test]
    fn posing_over_an_old_pose_is_posing_from_rest() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());
        run(&mut corpse, &skeleton, &world, 20);

        let (placed, fresh) = corpse.pose(&skeleton);
        let mut reused = Pose::rest().with(bone::HEAD, Quat::from_rotation_z(1.0));
        reused.root_offset = Vec3::ONE;
        reused.set("not a bone", Quat::from_rotation_x(0.5));
        assert_eq!(corpse.pose_into(&skeleton, &mut reused), placed);

        assert_eq!(reused.root_offset, Vec3::ZERO);
        assert_eq!(reused.joint("not a bone"), Quat::IDENTITY);
        for bone in skeleton.bones() {
            assert_eq!(reused.joint(bone.name), fresh.joint(bone.name), "{}", bone.name);
        }
    }
}


