//! What is left of a body once it stops holding itself up.
//!
//! A ragdoll is a **second entity**, spawned in the dying body's exact pose at
//! the exact place, and the original is reaped as usual — see
//! [`crate::game::death`], which is deliberately not told about any of this.
//! The alternative, keeping the corpse as the same entity with its animator
//! taken away, would mean everything that asks the world about players has to
//! ask whether each one is still alive: a corpse would still have hitboxes, an
//! id, a camera hanging off it and a health pool at zero waiting to be reaped
//! a second time. A corpse is a prop that used to be a person, so it is spawned
//! as a prop.
//!
//! Nothing is authored. The corpse is the same [`Skeleton`] the body had, so it
//! is dressed by the same [`BodyMeshPlugin`](crate::game::body_mesh) from the
//! same cached meshes, in the same colour — a class's corpse is its own build
//! without a second asset existing anywhere. What it does *not* get is a
//! [`SkeletonAnimator`](crate::common::skeleton::SkeletonAnimator): the pose on
//! a ragdoll is written by the solver here, and a body with both would have two
//! things arguing over one component.
//!
//! # How it is simulated
//!
//! Position-based dynamics over **two points per bone** — a head and a tail —
//! rather than rigid bodies with orientations and torques. The reasons are the
//! same ones that keep the rest of this codebase's physics hand-written:
//!
//! - It is a few hundred lines of `bevy_math` with no C library behind it, so
//!   it builds for wasm the way everything in `src/game` has to.
//! - Every constraint is a distance or an angle, which is exactly what a rig
//!   already states: a bone's length, a joint's offset from its parent, and
//!   the limits in [`crate::common::skeleton::joints`]. Nothing has to be
//!   authored twice, and a new class ragdolls correctly because its bones are
//!   already the right length.
//! - It is a function of `(state, dt)` and nothing else — no wall clock, no
//!   RNG — which is the same property [`crate::game::player`] keeps for the
//!   step, for the same eventual reason.
//!
//! Two points cannot express a twist, so the solver does not try: a bone's
//! orientation is reconstructed as the shortest rotation from where it rests
//! to where it now points. A forearm that rolls without either end moving is
//! motion this cannot represent, and a corpse is the one body nobody is
//! looking that closely at. The **root** is the exception that has to be
//! handled rather than shrugged off, because every limit on the body is
//! ultimately stated against its frame — see [`Ragdoll::root_seed`], which is
//! also the single subtlest thing in this file.
//!
//! Most of what is written down here is numerical rather than architectural,
//! and it is written down because none of it announces itself: a corpse that
//! crawls, buzzes, hovers, stretches or windmills one arm forever is what you
//! get, and each has a different cause. [`LIMIT_RELAXATION`], [`STICK_SPEED`],
//! [`SLEEP_DRIFT`], [`Ragdoll::root_seed`] and the ordering inside
//! [`Ragdoll::step`] each carry the symptom they exist to prevent. Change one
//! and run `every_corpse_settles_however_it_lands`, which drops forty bodies
//! precisely because any single drop settles or does not by luck.
//!
//! Momentum is **not** conserved, and [`RagdollShove`] says so in its own
//! units: a shove is a speed imparted at a point, not a quantity of momentum
//! divided by a mass. Mass is still real — it is a bone's own volume, so a
//! head weighs what a head weighs — and it decides how the rest of the body
//! resists being dragged along. What it deliberately does not decide is how
//! fast the bone that was hit leaves, because a rocket that flings a scout's
//! hand at two hundred metres per second is a correct simulation and a bad
//! game.
//!
//! # Where the force comes from
//!
//! [`RagdollShove`] is a message, so a rocket, a falling floor or a shotgun all
//! push a corpse without knowing this module exists. It names a **point and a
//! radius**, not a bone: the bones inside the radius are shoved and the joints
//! drag the rest along, which is what makes one message serve both a bullet
//! (a tight radius, one or two bones) and an explosion (a wide one, most of the
//! body). The killing shot is not a special case — [`fire_hitscan`] writes a
//! shove for *every* hit it lands, and the one that happens to be fatal lands
//! on a corpse that this module raised a moment earlier in the same tick.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::Damageable;
use crate::common::skeleton::joints::limit_of;
use crate::common::skeleton::{Bone, Pose, Skeleton};
use crate::game::body_mesh::BodyTint;
use crate::game::collision::CollisionWorld;
use crate::game::death::{reap_the_dead, record_damage};
use crate::game::hitscan::fire_hitscan;
use crate::game::player::{PhysicsBody, GRAVITY};
use crate::game::skeleton::{skeleton_root, SkeletonRoot};

/// How long a corpse lies there before it is taken away, in seconds of game
/// time.
///
/// Game time rather than wall time, so corpses do not quietly rot while
/// somebody is in the editor between rounds. A gamemode's number in the end,
/// like every other duration in the damage layer.
pub const RAGDOLL_LIFETIME: f32 = 20.0;

/// How many times the constraints are solved per tick.
///
/// Position-based dynamics converges rather than solving exactly, and the
/// stiffness of the whole body is this number: too few and the corpse is
/// rubbery, too many and a tick costs more than the rest of the frame. Eight
/// is enough that a leg does not visibly stretch.
const SOLVER_ITERATIONS: usize = 8;

/// Fraction of speed lost per tick to nothing in particular.
///
/// Not air resistance, which would be a function of speed. This is the
/// numerical damping every PBD solver needs so that constraint corrections do
/// not feed energy back in and shake a body apart.
const DRAG: f32 = 0.02;

/// Fraction of the sideways speed a contact point loses per tick to sliding.
///
/// Kinetic friction, and deliberately gentle: a body thrown hard at a floor
/// should skid before it stops. What stops it is [`STICK_SPEED`], not this.
const FRICTION: f32 = 0.12;

/// How slowly a contact point has to be sliding, in metres per second, before
/// it simply stops.
///
/// Static friction, and it is doing more work here than it looks. A
/// position-based solver leaks a little energy back in every tick — gravity
/// presses a foot into the floor, the floor pushes it out, and the rig
/// redistributes that through the body as a fraction of a millimetre of
/// sideways drift. Kinetic friction alone never quite kills it, because it
/// only ever removes a *fraction*, and the result is a corpse that creeps
/// across the room for as long as you watch it and never settles enough to
/// fall asleep.
///
/// A floor that holds anything slower than a stroll is also just true: a body
/// lying on the ground does not inch along.
const STICK_SPEED: f32 = 0.25;

/// How far a bone is moved towards what its joint allows, per pass.
///
/// Not all the way, and this is the difference between a corpse that lies
/// still and one that flaps a hand for as long as you watch it. A hinge
/// removes sideways motion outright — that is what a hinge *is* — and a bone
/// snapped exactly onto its plane on every pass is a hard projection fighting
/// the distance constraints that are pulling it off. The two overshoot each
/// other and the limb oscillates forever, at the elbows and knees especially,
/// because those are the joints with a plane to be snapped onto.
///
/// Moved most of the way instead, the two settle into an equilibrium where
/// each gives a little. Over [`SOLVER_ITERATIONS`] passes what is left of a
/// violation is a fraction of a percent, so nothing is meaningfully less
/// limited — a knee still does not bend forwards.
const LIMIT_RELAXATION: f32 = 0.5;

/// How far the furthest point may drift over [`SLEEP_AFTER`] before a corpse
/// counts as settled, in metres.
///
/// A distance travelled rather than a speed, and that is the whole point.
/// Position-based dynamics leaves a body at rest with a permanent buzz — the
/// two points that make up a joint chase each other by a few millimetres a
/// tick, forever — and measured as a speed that reads as motion. It is not:
/// the body is not going anywhere, and asking where it has *got to* over a
/// third of a second answers the question that sleeping is actually about.
const SLEEP_DRIFT: f32 = 0.02;

/// How long it has to stay that still before it stops being simulated.
///
/// A corpse that never slept would keep thirty-four points in a constraint
/// solver for as long as it lay there, and a round's worth of them is a cost
/// that only grows. Waking is on a shove, which is the only thing that can
/// disturb one.
const SLEEP_AFTER: f32 = 0.5;

/// The smallest and largest a bone's collision sphere may be, in metres.
///
/// Derived from the bone's own thickness between them, so a heavy's arm does
/// not sink into a floor a scout's rests on. Floored because a solver point
/// with no size tunnels, and capped because a torso the size of a torso wedges
/// in every doorway it is dropped near.
const POINT_RADIUS: (f32, f32) = (0.04, 0.14);

/// A body being pushed about, at a point.
///
/// The units are the honest ones: `push` is the speed the bone at `at` is
/// knocked to, in metres per second, and not an impulse to be divided by a
/// mass. See the module docs for why a corpse is deliberately not a momentum
/// simulation.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct RagdollShove {
    /// Where the push came from, in world space.
    pub at: Vec3,
    /// Which way, and how hard, at the centre.
    pub push: Vec3,
    /// How far the push reaches. Falls off linearly to nothing at the edge, so
    /// a bullet's tight radius moves one bone and lets the joints carry the
    /// rest, and an explosion's wide one moves the whole body at once.
    pub radius: f32,
}

/// A corpse.
#[derive(Component, Debug)]
pub struct Ragdoll {
    /// Two per bone, in [`Skeleton::bones`] order: `2i` is bone `i`'s head and
    /// `2i + 1` its tail. An index rather than a name because this is walked
    /// several times per tick and the two lists are the same list by
    /// construction — the same argument as
    /// [`BoneMesh`](crate::game::body_mesh::BoneMesh).
    points: Vec<Point>,
    /// The root bone's orientation at the moment of death.
    ///
    /// Two points per bone cannot express a roll, and for every other bone
    /// that does not matter — a rolled forearm is invisible. For the root it
    /// matters completely: it is the frame every limit on the body is
    /// ultimately stated against, so a root with the wrong roll is a corpse
    /// whose knees bend north whichever way it is facing.
    ///
    /// It is a **seed**, and that nothing ever writes to it is the point. The
    /// obvious alternative is to read the roll off the hips on every solve,
    /// since they hang off opposite sides of the pelvis and so say which way
    /// it is turned. That closes a loop: the roll sets the frame the legs'
    /// limits are stated in, the limits move the legs, and the legs move the
    /// hips the roll was read from. It has gain — the body picks up a small
    /// consistent bias, drag removes exactly as much per tick as the loop
    /// adds, and the corpse slides across the room at a slow walk forever
    /// without ever settling. Measured over forty drops, better than a third
    /// of them never came to rest at all.
    ///
    /// Seeded once and then swung onto wherever the pelvis currently points,
    /// there is no path back into it. What is given up is rotation about the
    /// pelvis's *own* axis after death: a corpse that rolls from its back onto
    /// its side keeps the knee plane it died with. Every other tumble is a
    /// change in the pelvis's direction, which the swing follows exactly.
    root_seed: Quat,
    /// Seconds of game time it has lain here.
    age: f32,
    /// How long every point has stayed within [`SLEEP_DRIFT`] of `anchor`.
    resting: f32,
    /// Where the points were when they were last judged to have moved. What
    /// drift is measured against.
    anchor: Vec<Vec3>,
    /// Settled, and no longer being solved.
    asleep: bool,
}

/// A body that has already had its corpse made.
///
/// [`raise_ragdolls`] runs immediately before the reaping that takes the body
/// away, so in the assembled game it could never see one twice. That ordering
/// is stated in [`crate::game::damage`] and could be changed there, and the
/// failure it would cause — a pile of corpses growing by one a tick — is the
/// kind that is noticed late. One marker is cheaper than that.
#[derive(Component, Debug)]
pub struct Ragdolled;

/// One end of one bone, under Verlet integration.
///
/// Velocity is the gap between `at` and `previous` rather than a field, which
/// is what makes a constraint that moves a point *be* a change in its velocity
/// — a leg straightened by its own length constraint pushes off the floor,
/// with nothing written to make it.
#[derive(Clone, Copy, Debug)]
struct Point {
    at: Vec3,
    previous: Vec3,
    /// Zero would mean a pinned point. Nothing is pinned today; the field is
    /// what a hanging corpse or a limb held by a hook would use.
    inverse_mass: f32,
    /// Half-extents of the box it collides as.
    half: Vec3,
}

impl Ragdoll {
    /// Take the pose a body is standing in and let go of it.
    ///
    /// `root` is the body's skeleton root in world space — its feet — and
    /// `carried` is the displacement the body covered on its last tick, so a
    /// corpse keeps the momentum of whoever was running when they died rather
    /// than dropping straight down out of a sprint.
    pub fn from_body(skeleton: &Skeleton, pose: &Pose, root: &Transform, carried: Vec3) -> Ragdoll {
        let posed = skeleton.posed_bones(pose, root);
        let mut points = Vec::with_capacity(posed.len() * 2);

        for bone in &posed {
            // A bone's own volume, halved between its two ends. Nothing here
            // is a second opinion about how big a body is: the prism this
            // measures is the one the mesh is built around.
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

        // Normalised so a whole body weighs one, whatever its build. Only the
        // ratios between points do any work, and leaving them in cubic metres
        // would make the shove constants mean something different for a heavy
        // than for a scout.
        let total: f32 = points.iter().map(|point| 1.0 / point.inverse_mass).sum();
        for point in &mut points {
            point.inverse_mass *= total;
        }

        Ragdoll {
            // The orientation the body actually died in, so a corpse's first
            // drawn frame faces the way the body did.
            root_seed: posed[0].rotation,
            anchor: points.iter().map(|point| point.at).collect(),
            points,
            age: 0.0,
            resting: 0.0,
            asleep: false,
        }
    }

    /// One tick: fall, hit things, and be pulled back into the shape of a
    /// body.
    ///
    /// The order is the standard position-based one, and the last step is the
    /// one that is easy to leave out: **velocity is read back off where the
    /// body actually ended up**, not written during the move. Everything in
    /// between — gravity, the joints, the walls — only moves points, and what
    /// that adds up to is what the body is doing next tick. A leg straightened
    /// by its own length constraint pushes off the floor with nothing written
    /// to make it, and a foot lifted back out of that floor does *not* pick up
    /// an upward velocity for having been lifted.
    pub fn step(&mut self, skeleton: &Skeleton, world: &CollisionWorld, dt: f32) {
        self.age += dt;
        if self.asleep {
            return;
        }

        // Where every point started, which is what the velocity at the end of
        // this is measured against.
        let started: Vec<Vec3> = self.points.iter().map(|point| point.at).collect();
        let mut touching = vec![BVec3::FALSE; self.points.len()];

        let fall = Vec3::Y * GRAVITY * dt * dt;
        for (point, contact) in self.points.iter_mut().zip(&mut touching) {
            let carried = (point.at - point.previous) * (1.0 - DRAG);
            // The same swept move the player takes, for the same reason: a
            // point thrown by an explosion covers more ground in a tick than a
            // wall is thick, and a push-out test would wave it through.
            let (moved, blocked) = world.move_and_slide(point.at, point.half, carried + fall);
            *contact = blocked;
            point.at = moved;
        }

        // The walls are solved *with* the rig rather than before or after it,
        // and both of the tempting orders are wrong in a way that takes a
        // while to see:
        //
        // - Rig last, and the floor is a suggestion. A foot pushed up out of
        //   the ground is put straight back through it by the bone that owns
        //   it, and it sinks a little further every tick. Once a point is past
        //   a plane the swept move cannot help — it only stops a *crossing* —
        //   so the foot stays under the floor, and the constant pushing and
        //   pulling makes the corpse crawl slowly across the room.
        // - Walls last, and the rig is a suggestion. Nothing puts a bone back
        //   to its own length after the world has moved one end of it, so a
        //   body lying on a floor stretches.
        //
        // Interleaved, each answers the other and both converge.
        //
        // Within a pass the walls go last, so nothing is left inside one at
        // the end of a tick. That does mean a bone can finish a tick slightly
        // the wrong length, which matters less than it sounds: the drawn body
        // comes from [`Ragdoll::pose`], and forward kinematics draws every
        // bone at its proper length whatever the solver's points are doing.
        let root = self.root_frame();
        for _ in 0..SOLVER_ITERATIONS {
            self.hold_together(skeleton);
            self.hold_joints(skeleton, root);
            self.clear_walls(world, &mut touching);
        }

        self.settle(dt);

        for ((point, was), contact) in self.points.iter_mut().zip(&started).zip(&touching) {
            let mut velocity = point.at - *was;
            for axis in 0..3 {
                // Held against something on this axis, so whatever it looks
                // like it did there, it is not going anywhere.
                if contact.test(axis) {
                    velocity[axis] = 0.0;
                }
            }
            // Friction, in two parts, and the reason a corpse that lands
            // spinning stops spinning rather than turning on the spot forever:
            // nothing else here can take angular momentum away.
            if contact.any() {
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
    /// `dt` because velocity here is the gap between two positions, so a speed
    /// only means anything against the length of a tick.
    pub fn shove(&mut self, at: Vec3, push: Vec3, radius: f32, dt: f32) {
        if radius <= 0.0 {
            return;
        }

        let mut touched = false;
        for point in &mut self.points {
            let reach = (1.0 - point.at.distance(at) / radius).clamp(0.0, 1.0);
            if reach <= 0.0 {
                continue;
            }
            // Backwards, because that is what a velocity is here.
            point.previous -= push * reach * dt;
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

    /// Where the solver has bone `index`'s two ends, in world space.
    ///
    /// **Not where the bone is drawn.** [`Ragdoll::pose`] hands the rig a
    /// direction and lets forward kinematics place the bone, so these two
    /// answers differ by however far the last solve was from converging.
    /// This is the solver's own state, for a debug view or a test that wants
    /// to know whether an arm is being stretched rather than whether it looks
    /// stretched.
    pub fn bone_points(&self, index: usize) -> (Vec3, Vec3) {
        (self.points[index * 2].at, self.points[index * 2 + 1].at)
    }

    pub fn is_asleep(&self) -> bool {
        self.asleep
    }

    /// Where the body is and what shape it is in, as something the rest of the
    /// skeleton layer can draw.
    ///
    /// The corpse's [`Transform`] is left unrotated and the whole of the
    /// body's orientation goes into the pose, so the entity's frame is the
    /// world's and a bone's rotation means the same thing here as it does on a
    /// body that is still standing.
    ///
    /// Note what this does **not** do: it does not place bones at the points.
    /// It reads a *direction* off each pair and hands the rig back a joint
    /// rotation, and forward kinematics puts the bone where the rig says it
    /// goes. A solve that has left a joint slightly pulled apart therefore
    /// draws as a body with a joint bent slightly too far, rather than as one
    /// with a gap in its arm.
    pub fn pose(&self, skeleton: &Skeleton) -> (Transform, Pose) {
        let bones = skeleton.bones();
        let mut frames: Vec<Quat> = Vec::with_capacity(bones.len());
        let mut pose = Pose::rest();
        let root = self.root_frame();

        for (index, bone) in bones.iter().enumerate() {
            let (rest, world, _) = self.frame(index, bone, &frames, root);
            // A bone's global rotation is `parent * bone.rest * pose`, and
            // `rest` here is the first two of those.
            pose.set(bone.name, rest.inverse() * world);
            frames.push(world);
        }

        // Forward kinematics puts the root bone's head at
        // `root.translation + root.rotation * bone.head`, and the rotation is
        // the identity, so this is that read backwards.
        (Transform::from_translation(self.points[0].at - bones[0].head), pose)
    }

    /// Bone lengths, and the offsets that hold one bone onto the next.
    ///
    /// A joint is two distances rather than one: a child's head is pinned to
    /// its parent's head *and* to its parent's tail, at whatever the rest
    /// offsets say. One distance alone would leave the child free to orbit its
    /// parent's head, and a hip — which hangs off the side of the pelvis
    /// rather than off its end — would swing round to the front of the body.
    fn hold_together(&mut self, skeleton: &Skeleton) {
        let bones = skeleton.bones();
        for (index, bone) in bones.iter().enumerate() {
            self.pull(index * 2, index * 2 + 1, bone.length);

            let Some(parent) = bone.parent else { continue };
            // `bone.head` is stated in the parent's rest frame, and a distance
            // is the same in every frame.
            self.pull(parent * 2, index * 2, bone.head.length());
            let from_tail = bone.head - Vec3::Y * bones[parent].length;
            self.pull(parent * 2 + 1, index * 2, from_tail.length());
        }
    }

    /// Bring every bone back inside what its joint allows.
    ///
    /// Parents first, because a limit is stated against where the parent
    /// actually is: an arm hanging off a chest that has folded over is not
    /// bent at the shoulder at all, and measuring it against the world would
    /// say it was.
    ///
    /// Only the tail moves. The head is where the joint constraints just put
    /// it, and a limit that shoved it too would be the two of them arguing —
    /// which shows up as a corpse that hums rather than one that lies still.
    fn hold_joints(&mut self, skeleton: &Skeleton, root: Quat) {
        let bones = skeleton.bones();
        let mut frames: Vec<Quat> = Vec::with_capacity(bones.len());

        for (index, bone) in bones.iter().enumerate() {
            let (_, world, direction) = self.frame(index, bone, &frames, root);
            let head = self.points[index * 2].at;

            // Part of the way towards what the joint allows, not all of it.
            let was = (self.points[index * 2 + 1].at - head).normalize_or(direction);
            let eased = was.lerp(direction, LIMIT_RELAXATION).normalize_or(direction);

            self.points[index * 2 + 1].at = head + eased * bone.length;
            frames.push(world);
        }
    }

    /// Where this bone would point if its joint were at rest, given where its
    /// parent has actually ended up.
    /// Where bone `index` has ended up: the frame it would be in with its
    /// joint at rest, the frame it is actually in, and the direction it
    /// points once its joint has had its say.
    ///
    /// `frames` holds the same answer for every bone before this one, which is
    /// why both callers walk the rig parents-first. A limit is stated against
    /// where the *parent* actually is: an arm hanging off a chest that has
    /// folded over is not bent at the shoulder at all, and measuring it
    /// against the world would say it was.
    fn frame(&self, index: usize, bone: &Bone, frames: &[Quat], root: Quat) -> (Quat, Quat, Vec3) {
        let along = self.points[index * 2 + 1].at - self.points[index * 2].at;

        let Some(parent) = bone.parent else {
            // A root has no joint, and so no limit — there is nothing for it
            // to be bent relative to. Pinning it instead, which is what a
            // limit against the world would do, welds the body upright and
            // leaves every other joint straining against a pelvis that will
            // not lie down.
            let direction = along.try_normalize().unwrap_or(root * Vec3::Y);
            return (bone.rest, root, direction);
        };

        // The limit is applied in the bone's own rest frame, which is the
        // frame [`crate::common::skeleton::joints`] states every limit in and
        // the reason one table covers both sides of the body.
        let rest = frames[parent] * bone.rest;
        let direction = rest * limit_of(bone.name).nearest(rest.inverse() * along);
        (rest, Quat::from_rotation_arc(rest * Vec3::Y, direction) * rest, direction)
    }

    /// Where the root bone is now: the frame it died in, swung onto wherever
    /// its two points currently point.
    ///
    /// Derived from [`Ragdoll::root_seed`] every time rather than from the
    /// last answer, so nothing accumulates and there is no state to drift.
    fn root_frame(&self) -> Quat {
        let along = self.points[1].at - self.points[0].at;
        let direction = along.try_normalize().unwrap_or(self.root_seed * Vec3::Y);
        Quat::from_rotation_arc(self.root_seed * Vec3::Y, direction) * self.root_seed
    }

    /// Lift every point back out of anything it has been pushed into.
    ///
    /// [`CollisionWorld::depenetrate`] rather than a plane test, because the
    /// question it answers is the one that matters here: the shortest way out
    /// of a floor is downwards, and the right way out is the one that leaves
    /// the point inside the level.
    fn clear_walls(&mut self, world: &CollisionWorld, touching: &mut [BVec3]) {
        for (point, contact) in self.points.iter_mut().zip(touching) {
            let out = world.depenetrate(point.at, point.half);

            // Having to be pushed out of a wall *is* touching it, and saying
            // so is what stops a corpse buzzing. A foot the rig keeps pressing
            // into the floor is put back every pass, and if that did not count
            // as contact it would never be given any friction — so it would
            // sit there being pushed out and pulled back in, a few millimetres
            // a tick, for as long as the body lay there. The swept move cannot
            // notice it, because a point that starts inside a wall never
            // *crosses* one.
            for axis in 0..3 {
                if (out[axis] - point.at[axis]).abs() > 1.0e-6 {
                    *contact = set_axis(*contact, axis);
                }
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
        // Two points on top of each other have no direction to be pushed
        // apart along. A later iteration, with gravity having moved one of
        // them, will find one.
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

/// `BVec3` has no per-index setter, so this is it.
fn set_axis(flags: BVec3, axis: usize) -> BVec3 {
    let mut set = [flags.x, flags.y, flags.z];
    set[axis] = true;
    BVec3::from_array(set)
}

pub struct RagdollPlugin;

impl Plugin for RagdollPlugin {
    fn build(&self, app: &mut App) {
        app
            .add_message::<RagdollShove>()
            // All on the tick, and the order is the whole of it:
            //
            // 1. `raise_ragdolls` has to see the body *before* `reap_the_dead`
            //    takes it away — a corpse is a copy of a pose, and by the time
            //    `Died` is written there is no pose left to copy.
            // 2. `shove_ragdolls` has to run after that, so the shot that did
            //    the killing lands on the corpse it just made. `fire_hitscan`
            //    writes the shove without knowing whether anything died.
            // 3. `step_ragdolls` last, so a shove is taken on the tick it
            //    arrived rather than the one after.
            .add_systems(FixedUpdate, (
                raise_ragdolls.after(record_damage).before(reap_the_dead),
                shove_ragdolls.after(fire_hitscan),
                step_ragdolls,
            ).chain().run_if(in_state(AppMode::Play)))
            // Corpses belong to the match. F5 back to the editor and a body
            // lying across the room you are trying to resize is in the way of
            // the thing you went there to do.
            .add_systems(OnExit(AppMode::Play), clear_ragdolls)
        ;
    }
}

/// Make a corpse of anything that has run out of health and had a body to
/// lose.
///
/// The query is the whole of the rule the user of this module cares about: a
/// [`Skeleton`] and a [`Damageable`] at zero. A crate has health and no
/// skeleton, so it simply vanishes as it always did; a spawn point's preview
/// has a skeleton and no health, so there is nothing that could kill it. The
/// two together are what "a humanoid died" means, with no marker component to
/// forget to add.
pub fn raise_ragdolls(
    mut commands: Commands,
    bodies: Query<
        (
            Entity,
            &Damageable,
            &Skeleton,
            &Pose,
            &GlobalTransform,
            Option<&SkeletonRoot>,
            Option<&BodyTint>,
            Option<&PhysicsBody>,
            Option<&Name>,
        ),
        Without<Ragdolled>,
    >,
) {
    for (entity, health, skeleton, pose, global, offset, tint, physics, name) in &bodies {
        if health.is_alive() {
            continue;
        }

        // The step's own displacement rather than a velocity in metres per
        // second, because that is the unit the solver keeps: how far a point
        // moved on the last tick.
        let carried = physics.map_or(Vec3::ZERO, |body| body.current - body.previous);
        let ragdoll = Ragdoll::from_body(skeleton, pose, &skeleton_root(global, offset), carried);

        // Posed now rather than left to the first solve, so the corpse is
        // never drawn for a frame standing at the origin in its rest pose.
        let (transform, pose) = ragdoll.pose(skeleton);

        let mut corpse = commands.spawn((
            ragdoll,
            // The same rig, so the same cached meshes dress it. Notably *not*
            // a `SkeletonAnimator`: the solver owns this body's pose.
            skeleton.clone(),
            pose,
            transform,
            Name::new(match name {
                Some(name) => format!("Ragdoll of {name}"),
                None => "Ragdoll".to_string(),
            }),
        ));
        if let Some(tint) = tint {
            // Whose body it was is still worth knowing once it is on the
            // floor — more so, if teams are ever a thing you check by looking.
            corpse.insert(*tint);
        }

        commands.entity(entity).insert(Ragdolled);
    }
}

/// Push every corpse each shove reaches.
///
/// Every corpse, not the one that was hit: a shove is a point in the world,
/// and an explosion between two bodies moves both. That is also what makes the
/// message usable by things that have no idea a body is there.
pub fn shove_ragdolls(
    time: Res<Time<Fixed>>,
    mut shoves: MessageReader<RagdollShove>,
    mut corpses: Query<&mut Ragdoll>,
) {
    let dt = time.delta_secs();
    // Read once even when there is nothing to push, or the queue backs up and
    // every corpse spawned later takes the whole round's shots at once.
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
/// `Time<Fixed>` rather than `Time`, so a corpse is simulated in the same
/// steps everything else that decides where a body ends up is, and so it does
/// not age while the game is paused in the editor.
pub fn step_ragdolls(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    world: Res<CollisionWorld>,
    mut corpses: Query<(Entity, &mut Ragdoll, &Skeleton, &mut Transform, &mut Pose)>,
) {
    let dt = time.delta_secs();
    for (entity, mut corpse, skeleton, mut transform, mut pose) in &mut corpses {
        if corpse.age() >= RAGDOLL_LIFETIME {
            commands.entity(entity).despawn();
            continue;
        }

        let settled = corpse.is_asleep();
        corpse.step(skeleton, &world, dt);
        // A settled corpse is not moving, and writing its pose anyway would
        // dirty change detection for every part of it every tick.
        if settled && corpse.is_asleep() {
            continue;
        }

        let (placed, shape) = corpse.pose(skeleton);
        *transform = placed;
        *pose = shape;
    }
}

/// Take every corpse away.
///
/// Called on leaving Play, and again from
/// [`reset_for_play`](crate::game::reset::reset_for_play) — the same belt and
/// braces the damage numbers get, and for the same reason: one covers the way
/// out and the other covers a match started by any other route.
pub fn clear_ragdolls(mut commands: Commands, corpses: Query<Entity, With<Ragdoll>>) {
    for corpse in &corpses {
        commands.entity(corpse).despawn();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::damage::{DamageLog, PlayerId};
    use crate::common::skeleton::rig::{bone, humanoid, PosedBone, Proportions};
    use crate::tool::room::Room;

    /// One fixed step at Bevy's 64 Hz default, as a number rather than a
    /// `Duration`: the solver takes seconds.
    const DT: f32 = 1.0 / 64.0;

    fn rig() -> Skeleton {
        humanoid(Proportions::DEFAULT)
    }

    /// A body standing on the floor of a room, freshly dead and not yet
    /// falling.
    fn stood_up(skeleton: &Skeleton, pose: &Pose) -> Ragdoll {
        Ragdoll::from_body(skeleton, pose, &Transform::IDENTITY, Vec3::ZERO)
    }

    /// A floor to land on, with walls far enough away to be irrelevant.
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
            corpse.step(skeleton, world, DT);
        }
    }

    /// Where a named bone is drawn, which is the answer that matters: it is
    /// what the mesh is placed from.
    fn drawn(corpse: &Ragdoll, skeleton: &Skeleton, name: &str) -> PosedBone {
        let (root, pose) = corpse.pose(skeleton);
        let index = skeleton.index_of(name).expect("no such bone");
        skeleton.posed_bones(&pose, &root)[index]
    }

    /// The whole promise of "spawn a ragdoll at the exact place in the exact
    /// pose": before a single step is taken, the corpse has to be drawn
    /// exactly where the body was. Anything less is a body that twitches at
    /// the moment of death, which is the one moment everybody is looking at
    /// it.
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

    /// A rotated body is the case the reconstruction can get wrong and still
    /// look plausible, because the corpse's own transform is left unrotated
    /// and the whole of the facing has to survive in the pose instead.
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

        // The right hand is out to the character's own right, which after a
        // quarter turn to the left is world -Z.
        let hand = drawn(&corpse, &skeleton, bone::HAND_R);
        assert!(hand.head.z < -0.2, "the facing was lost: right hand at {}", hand.head);
    }

    /// Nothing holds a corpse up, so it has to come down. With no map at all
    /// there is nothing to land on either, which is the version of this that
    /// cannot pass by accident.
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

    /// And with a floor under it, it lands on the floor rather than through
    /// it. The head is the part that has furthest to travel and so the one
    /// that would sail through a floor the solver was not consulting.
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

    /// A body left standing bolt upright is the failure this whole thing is
    /// judged on: a corpse standing to attention is worse than no ragdoll at
    /// all. Nothing pushes it over — it has to collapse under its own weight,
    /// because nothing is holding its knees straight any more.
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

    /// Bones are rigid. A solver that let them stretch would show up as a body
    /// that grows a little every time it is hit, and the drawn body would hide
    /// it — forward kinematics always draws a bone at its proper length, so
    /// this has to ask the solver directly.
    #[test]
    fn a_corpse_that_is_thrown_about_does_not_stretch() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        // At one wrist, which is the worst case: the lightest point on the
        // body, as far from the pelvis as anything gets. Hard, but within
        // reach — a headshot writes about 13 m/s, so this is a rocket.
        let (wrist, _) = corpse.bone_points(skeleton.index_of(bone::HAND_R).unwrap());
        corpse.shove(wrist, Vec3::new(18.0, 11.0, 0.0), 0.3, DT);
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

    /// And joints hold. The shin's head has to stay at the thigh's tail
    /// however hard the leg is pulled, or the body comes apart at the knee.
    #[test]
    fn a_corpse_that_is_thrown_about_stays_in_one_piece() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        let (ankle, _) = corpse.bone_points(skeleton.index_of(bone::FOOT_L).unwrap());
        corpse.shove(ankle, Vec3::new(-14.0, 14.0, 9.0), 0.3, DT);
        run(&mut corpse, &skeleton, &world, 64 * 4);

        for (index, bone) in skeleton.bones().iter().enumerate() {
            let Some(parent) = bone.parent else { continue };
            let (_, parent_tail) = corpse.bone_points(parent);
            let (head, _) = corpse.bone_points(index);

            // How far this bone's head sits from its parent's tail at rest.
            let rest = (bone.head - Vec3::Y * skeleton.bones()[parent].length).length();
            assert!(
                (head.distance(parent_tail) - rest).abs() < 0.05,
                "{} came off its parent",
                bone.name
            );
        }
    }

    /// The limits are not decoration: a knee that can go either way turns
    /// every corpse into a spider. Checked on the drawn body rather than on
    /// the solver's points, because the drawn body is what anyone would see.
    #[test]
    fn no_amount_of_throwing_bends_a_knee_forwards() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        // Straight up the front of the shin, which is exactly the direction
        // that would hyperextend the joint.
        let (knee, _) = corpse.bone_points(skeleton.index_of(bone::SHIN_L).unwrap());
        corpse.shove(knee, Vec3::new(0.0, 4.0, -35.0), 0.35, DT);

        for _ in 0..64 * 4 {
            corpse.step(&skeleton, &world, DT);

            let (root, pose) = corpse.pose(&skeleton);
            // The pose is the joint's own rotation, so this reads the bend
            // itself rather than where the leg happens to be pointing.
            let bend = pose.joint(bone::SHIN_L);
            let tail = bend * Vec3::Y;
            assert!(tail.z <= 0.1, "the knee bent forwards: {tail}");
            let _ = root;
        }
    }

    /// The shove is a point and a radius, and the point is the whole design:
    /// a tight one moves the bone it names and the joints drag the rest along.
    /// A solver that moved the body as a lump would pass a test that only
    /// looked at the pelvis.
    #[test]
    fn a_tight_shove_moves_the_bone_it_names_more_than_the_rest() {
        let skeleton = rig();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        let head_index = skeleton.index_of(bone::HEAD).unwrap();
        let (before_head, _) = corpse.bone_points(head_index);
        let (before_foot, _) = corpse.bone_points(skeleton.index_of(bone::FOOT_L).unwrap());

        corpse.shove(before_head, Vec3::new(0.0, 0.0, -12.0), 0.4, DT);
        // Two steps: enough for the shove to be taken and not enough for the
        // rest of the body to have caught up.
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

    /// And the joints do drag the rest along, given a moment. Otherwise a
    /// shove would tear a head off rather than spin a body round.
    #[test]
    fn what_the_shove_reached_pulls_the_rest_of_the_body_after_it() {
        let skeleton = rig();
        let mut corpse = stood_up(&skeleton, &Pose::rest());

        let head_index = skeleton.index_of(bone::HEAD).unwrap();
        let chest_index = skeleton.index_of(bone::CHEST).unwrap();
        let (at, _) = corpse.bone_points(head_index);
        let (before_chest, _) = corpse.bone_points(chest_index);

        corpse.shove(at, Vec3::new(0.0, 0.0, -12.0), 0.4, DT);
        run(&mut corpse, &skeleton, &CollisionWorld::default(), 30);

        let (after_chest, _) = corpse.bone_points(chest_index);
        assert!(
            (after_chest - before_chest).z < -0.05,
            "the chest was not dragged along: {before_chest} -> {after_chest}"
        );
    }

    /// A corpse that never stopped being solved would cost a round's worth of
    /// constraint solving for as long as the bodies lay there.
    ///
    /// **Forty drops, not one.** Whether any single body settles turns out to
    /// depend on exactly how it lands, so a test that drops one is a test that
    /// passes or fails on luck — an earlier version of this file settled the
    /// upright drop below perfectly and left better than a third of these
    /// crawling across the floor forever. If this starts failing, that is what
    /// it has found, and the cause is something feeding energy back into the
    /// solver rather than a constant that needs nudging.
    #[test]
    fn every_corpse_settles_however_it_lands() {
        let skeleton = rig();
        let world = a_room();

        let mut restless = Vec::new();
        for case in 0..40 {
            // Spread over poses, facings, places and speeds, deterministically
            // — a randomised sweep that failed one run in ten would be worse
            // than no sweep at all.
            let a = case as f32 * 0.9;
            let pose = Pose::rest()
                .with(bone::CHEST, Quat::from_rotation_x(a.sin() * 0.5))
                .with(bone::THIGH_L, Quat::from_rotation_x(a.cos() * 0.5));
            let root = Transform::from_translation(Vec3::new(a.cos(), 0.0, a.sin()))
                .with_rotation(Quat::from_rotation_y(a));

            let mut corpse =
                Ragdoll::from_body(&skeleton, &pose, &root, Vec3::new(a.sin(), 0.0, a.cos()) * 0.1);
            let (head, _) = corpse.bone_points(skeleton.index_of(bone::HEAD).unwrap());
            corpse.shove(head, Vec3::new(a.cos() * 9.0, 2.0, a.sin() * 9.0), 0.4, DT);

            run(&mut corpse, &skeleton, &world, 64 * 12);
            if !corpse.is_asleep() {
                restless.push(case);
            }
        }

        assert!(restless.is_empty(), "still moving after twelve seconds: {restless:?}");
    }

    /// And a shove has to wake it, or a corpse could be shot at and ignore it.
    #[test]
    fn a_shove_wakes_a_settled_corpse() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());
        run(&mut corpse, &skeleton, &world, 64 * 15);
        assert!(corpse.is_asleep());

        let (pelvis, _) = corpse.bone_points(0);
        corpse.shove(pelvis, Vec3::new(6.0, 3.0, 0.0), 1.0, DT);
        assert!(!corpse.is_asleep(), "slept through being shot");

        let before = corpse.bone_points(0).0;
        run(&mut corpse, &skeleton, &world, 8);
        assert!(corpse.bone_points(0).0.distance(before) > 0.05, "woke but did not move");
    }

    /// A shove that lands nowhere near a corpse leaves it alone, which is what
    /// makes the message safe to write for every shot rather than only for the
    /// fatal one.
    #[test]
    fn a_shove_out_of_range_is_ignored() {
        let skeleton = rig();
        let world = a_room();
        let mut corpse = stood_up(&skeleton, &Pose::rest());
        run(&mut corpse, &skeleton, &world, 64 * 15);

        corpse.shove(Vec3::new(50.0, 0.0, 0.0), Vec3::X * 100.0, 0.4, DT);
        assert!(corpse.is_asleep(), "woken by a shot on the other side of the map");
    }

    /// Mass is real, and this is what it buys. A shove is deliberately *not*
    /// divided by it — see the module docs, and [`RagdollShove`]'s units — so
    /// what mass decides is not how fast the bone that was hit leaves, but how
    /// much of the body it manages to drag with it.
    ///
    /// So the asymmetry is the assertion: a hand cannot move a pelvis, and a
    /// pelvis moves a hand easily. A solver with one mass for every point
    /// would find those two the same.
    #[test]
    fn a_light_bone_cannot_drag_a_heavy_one_the_way_a_heavy_one_drags_it() {
        let skeleton = rig();
        let world = CollisionWorld::default();

        // Where a bone ends up, with the shove and without it. The difference
        // between the two is the shove's doing and nothing else — a corpse
        // dropped in free space is collapsing all the while, and a bare
        // displacement would be mostly that.
        let settled = |pushed: Option<&str>, watched: usize| {
            let mut corpse = stood_up(&skeleton, &Pose::rest());
            if let Some(pushed) = pushed {
                let (at, _) = corpse.bone_points(skeleton.index_of(pushed).unwrap());
                corpse.shove(at, Vec3::X * 10.0, 0.25, DT);
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

    /// The rule the rest of the game sees: a skeleton plus health at zero.
    /// Everything below is that sentence taken apart.
    fn a_dead_body(world: &mut World, extras: impl Bundle) -> Entity {
        let mut health = Damageable::with_health(100);
        health.apply(100);
        world
            .spawn((
                health,
                DamageLog::default(),
                rig(),
                Pose::rest(),
                Transform::IDENTITY,
                GlobalTransform::default(),
                extras,
            ))
            .id()
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

    /// Health is not enough. A crate shot to pieces takes the same path
    /// through [`crate::game::death`] and must not leave a humanoid corpse
    /// behind it — which is the whole reason the query asks for a skeleton
    /// rather than for a marker somebody has to remember to add.
    #[test]
    fn a_dead_thing_with_no_body_leaves_nothing() {
        let mut world = World::new();
        let mut health = Damageable::with_health(20);
        health.apply(20);
        world.spawn((health, DamageLog::default(), Transform::IDENTITY, GlobalTransform::default()));
        world.run_system_once(raise_ragdolls).unwrap();

        assert_eq!(corpses(&mut world), 0);
    }

    /// And a body is not enough either: a spawn point's preview is a rig with
    /// no health, and there is nothing that could kill it.
    #[test]
    fn a_living_body_is_left_standing() {
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

    /// In the assembled game the reaping takes the body away on the same tick,
    /// so this could not happen. It is guarded anyway because the thing that
    /// guarantees it is an ordering stated in another module, and the failure
    /// — a corpse a tick, forever — is the kind that is noticed late.
    #[test]
    fn a_body_is_only_ever_made_into_one_corpse() {
        let mut world = World::new();
        a_dead_body(&mut world, ());

        for _ in 0..5 {
            world.run_system_once(raise_ragdolls).unwrap();
        }
        assert_eq!(corpses(&mut world), 1);
    }

    /// A corpse is not an animated body. Handing it a
    /// [`SkeletonAnimator`](crate::common::skeleton::SkeletonAnimator) — or
    /// leaving one on it — would put two writers on one [`Pose`], and the
    /// visible result is a corpse that lies there doing its idle animation.
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

    /// Whose body it was is still worth knowing once it is on the floor.
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

    /// The end of it: a body killed by a shot leaves a corpse where it was
    /// standing, and the shot that killed it throws that corpse. Everything
    /// between those two facts is the ordering in
    /// [`RagdollPlugin`], which is what this is really testing.
    #[test]
    fn the_shot_that_kills_a_body_throws_the_corpse_it_makes() {
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<RagdollShove>>();
        // A bare `World` has a fixed clock that has never ticked, and a step
        // of no time at all would pass this test by moving nothing.
        world.init_resource::<Time<Fixed>>();
        world
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_micros(15_625));

        let feet = Vec3::new(2.0, 0.0, 0.0);
        // `GlobalTransform` would be written by propagation, which is not
        // running here, so it is set alongside the `Transform` by hand.
        let placed = Transform::from_translation(feet);
        let body = a_dead_body(&mut world, ());
        world.entity_mut(body).insert((placed, GlobalTransform::from(placed)));

        world.run_system_once(raise_ragdolls).unwrap();

        let raised = {
            let mut corpses = world.query::<&Ragdoll>();
            corpses.single(&world).unwrap().bone_points(0).0
        };
        assert!((raised.x - feet.x).abs() < 0.01, "raised at {raised}, not over {feet}");

        // A shot arriving after the corpse exists, which is the order the
        // plugin guarantees and the reason `fire_hitscan` can write a shove
        // without knowing whether anything died.
        world.write_message(RagdollShove {
            at: raised,
            push: Vec3::X * 20.0,
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

    /// The ordering, assembled.
    ///
    /// Everything above drives one system at a time, which is exactly the way
    /// to miss the thing most likely to be wrong here: `raise_ragdolls` lives
    /// in this module and `reap_the_dead` lives in another, and the guarantee
    /// that the first sees a body before the second takes it away is a
    /// `.before()` spanning two plugins. Nothing in the type system holds that
    /// together, and if it comes apart the symptom is not an error — it is
    /// bodies that quietly stop leaving corpses.
    #[test]
    fn a_body_that_dies_in_the_running_game_leaves_a_corpse_behind_it() {
        use crate::game::damage::DamagePlugin;
        use crate::game::GamePlugin;

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::input::InputPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.add_plugins(GamePlugin);
        // Where the reaping lives, and so where the ordering this is about is
        // actually stated.
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

        // Killed outright, the way a shot with nothing left to take would.
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


    /// Corpses belong to the match. One lying across the room you went back to
    /// the editor to resize is in the way of the thing you went there to do.
    #[test]
    fn leaving_the_match_takes_every_corpse_with_it() {
        let mut world = World::new();
        a_dead_body(&mut world, ());
        world.run_system_once(raise_ragdolls).unwrap();
        assert_eq!(corpses(&mut world), 1);

        world.run_system_once(clear_ragdolls).unwrap();
        assert_eq!(corpses(&mut world), 0);
    }

    /// Radius zero is what a caller with nothing to say passes, and it must be
    /// nothing rather than a division by zero applied to the whole body.
    #[test]
    fn a_shove_with_no_reach_does_nothing() {
        let skeleton = rig();
        let mut corpse = stood_up(&skeleton, &Pose::rest());
        let before = corpse.bone_points(0).0;

        corpse.shove(before, Vec3::X * 100.0, 0.0, DT);
        run(&mut corpse, &skeleton, &CollisionWorld::default(), 1);

        // Gravity, and nothing sideways.
        assert!((corpse.bone_points(0).0.x - before.x).abs() < 1.0e-4);
    }
}


