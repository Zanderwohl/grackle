use bevy::prelude::*;
use strum_macros::EnumIter;

use crate::common::skeleton::Proportions;
use crate::get;

/// The body metrics of the tallest class.
///
/// One place, because three things have to agree and are written in three
/// different modules: the body the game actually moves, the clearance a spawn
/// point is checked for before anyone is put there, and the gizmo the editor
/// draws so a mapper can see that clearance. If those drift, a spawn point
/// passes in the editor and wedges a player's head in the ceiling at runtime.
///
/// The maximum over every [`Class`] — a spawn point has to fit whoever might
/// use it, so the tallest class is what the map is checked against. Every
/// class is the same shape today, which makes it trivially the maximum; when
/// proportions diverge this has to stay at the maximum, and there is a test at
/// the bottom of this file that says so.
pub const TALLEST_CLASS_HEIGHT: f32 = 2.0;

/// Where the camera sits on the tallest class, measured from the feet.
pub const TALLEST_CLASS_EYE_HEIGHT: f32 = 1.8;

/// How wide a body is, as a half-extent on the two ground axes.
pub const CLASS_RADIUS: f32 = 0.35;

/// Half-extents of a body box, for collision.
pub const CLASS_HALF_EXTENTS: Vec3 =
    Vec3::new(CLASS_RADIUS, TALLEST_CLASS_HEIGHT * 0.5, CLASS_RADIUS);

/// How tall a crouched body is.
///
/// A little over half standing, which is about what the games this is modelled
/// on use. The number matters twice over: it is the gap a body can get through
/// only by ducking, and it is the height of the box that can be shot at.
pub const CROUCH_HEIGHT: f32 = 1.1;

/// Where the camera sits on a crouched body, measured from the feet.
pub const CROUCH_EYE_HEIGHT: f32 = 0.88;

/// How much of a body is standing up.
///
/// A component, because it is a fact about a body that several unrelated
/// things need and none of them owns: the step moves a hull this size, the
/// camera sits at this eye height, the hitboxes are this tall, and the
/// animation crouches to match. Kept apart from *asking* to crouch, which is a
/// [`crate::common::skeleton::BodyRequests`] field — a body under a low
/// ceiling is crouched whether it still wants to be or not.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stance {
    #[default]
    Standing,
    Crouched,
}

impl Stance {
    pub fn height(&self) -> f32 {
        match self {
            Stance::Standing => TALLEST_CLASS_HEIGHT,
            Stance::Crouched => CROUCH_HEIGHT,
        }
    }

    pub fn eye_height(&self) -> f32 {
        match self {
            Stance::Standing => TALLEST_CLASS_EYE_HEIGHT,
            Stance::Crouched => CROUCH_EYE_HEIGHT,
        }
    }

    /// Half-extents of this stance's box, for collision and for hitboxes.
    pub fn half_extents(&self) -> Vec3 {
        Vec3::new(CLASS_RADIUS, self.height() * 0.5, CLASS_RADIUS)
    }

    /// Where the camera sits relative to the body's *centre*, since that is
    /// what the body's transform is and what the box is measured from.
    pub fn eye_offset(&self) -> f32 {
        self.eye_height() - self.height() * 0.5
    }

    /// How far a body's centre moves when it changes stance.
    ///
    /// Which way it moves depends on what is being kept still: the feet, for a
    /// body on the ground, or the head, for one in the air.
    pub fn centre_shift() -> f32 {
        (Stance::Standing.height() - Stance::Crouched.height()) * 0.5
    }
}

/// The centre of a body standing with its feet at `feet`.
///
/// A spawn point marks the feet, because that is the thing a mapper can see
/// and place against the floor. Collision works in centres. This is the one
/// conversion between them, so nobody has to remember which end a position
/// refers to.
pub fn body_centre_from_feet(feet: Vec3) -> Vec3 {
    feet + Vec3::Y * (TALLEST_CLASS_HEIGHT * 0.5)
}

/// The ten bodies a player can be.
///
/// They all have the same proportions for now — the roster exists so that
/// everything downstream (a rig, a spawn, an animation display) can be asked
/// *per class* before there is anything to tell them apart. Giving them
/// distinct silhouettes is a design decision, and making it here would be
/// making it by accident.
///
/// The order is the order they are displayed in and nothing more; nothing
/// persists a class by index.
#[derive(EnumIter, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Scout,
    Soldier,
    Pyro,
    Demoman,
    Heavy,
    Engineer,
    Medic,
    Sniper,
    Spy,
    /// The tenth body: the one being escorted rather than doing the escorting.
    Civilian,
}

impl Class {
    /// The shape of this class's body.
    ///
    /// Built by shape rather than by height: the eye and the collision hull
    /// are one size for everybody, so a class much shorter than the rest would
    /// be looking out of a point above its own head. Everything stands between
    /// 1.70 and 2.00 m and reads by build instead, which is also how the games
    /// this is modelled on do it.
    ///
    /// One consequence is worth expecting rather than being surprised by: a
    /// stride is measured in leg-lengths, so at the same speed the Engineer's
    /// legs turn over about a third faster than the Sniper's. That is the
    /// whole point of poses being rotations, finally visible.
    pub fn proportions(&self) -> Proportions {
        match self {
            // Small and wiry: a short body on long legs.
            Class::Scout => Proportions {
                height: 1.78,
                hip_height: 0.57,
                torso_length: 0.25,
                arm_length: 0.30,
                head_length: 0.135,
                shoulder_half_width: 0.100,
                hip_half_width: 0.048,
                girth: 0.78,
                belly: 1.0,
            },
            // Broad and blocky, square across the shoulders.
            Class::Soldier => Proportions {
                height: 1.90,
                hip_height: 0.52,
                torso_length: 0.29,
                arm_length: 0.31,
                head_length: 0.125,
                shoulder_half_width: 0.135,
                hip_half_width: 0.058,
                girth: 1.25,
                belly: 1.0,
            },
            // Bulky and round in a suit, with a big masked head on stubby legs.
            Class::Pyro => Proportions {
                height: 1.80,
                hip_height: 0.50,
                torso_length: 0.30,
                arm_length: 0.30,
                head_length: 0.150,
                shoulder_half_width: 0.125,
                hip_half_width: 0.062,
                girth: 1.35,
                belly: 1.08,
            },
            // An ordinary build, thickening round the middle.
            Class::Demoman => Proportions {
                height: 1.86,
                hip_height: 0.52,
                torso_length: 0.29,
                arm_length: 0.32,
                head_length: 0.130,
                shoulder_half_width: 0.120,
                hip_half_width: 0.060,
                girth: 1.12,
                belly: 1.15,
            },
            // The biggest of them: a barrel of a torso, short legs, no neck.
            Class::Heavy => Proportions {
                height: 2.00,
                hip_height: 0.47,
                torso_length: 0.34,
                arm_length: 0.32,
                head_length: 0.125,
                shoulder_half_width: 0.155,
                hip_half_width: 0.075,
                girth: 1.75,
                // Big all over rather than paunchy: what makes him enormous is
                // girth and shoulders, which leaves being *round* to somebody
                // else.
                belly: 1.0,
            },
            // The shortest, and wide for his height.
            Class::Engineer => Proportions {
                height: 1.76,
                hip_height: 0.50,
                torso_length: 0.30,
                arm_length: 0.30,
                head_length: 0.140,
                shoulder_half_width: 0.130,
                hip_half_width: 0.062,
                girth: 1.20,
                belly: 1.0,
            },
            // Tall, upright and narrow.
            Class::Medic => Proportions {
                height: 1.88,
                hip_height: 0.55,
                torso_length: 0.27,
                arm_length: 0.31,
                head_length: 0.125,
                shoulder_half_width: 0.108,
                hip_half_width: 0.050,
                girth: 0.88,
                belly: 1.0,
            },
            // Lanky: the longest legs and the longest arms.
            Class::Sniper => Proportions {
                height: 1.95,
                hip_height: 0.58,
                torso_length: 0.25,
                arm_length: 0.34,
                head_length: 0.120,
                shoulder_half_width: 0.105,
                hip_half_width: 0.048,
                girth: 0.82,
                belly: 1.0,
            },
            // Slim and narrow, and otherwise unremarkable — which is the point
            // of him.
            Class::Spy => Proportions {
                height: 1.86,
                hip_height: 0.55,
                torso_length: 0.27,
                arm_length: 0.31,
                head_length: 0.120,
                shoulder_half_width: 0.105,
                hip_half_width: 0.048,
                girth: 0.85,
                belly: 1.0,
            },
            // Short, round and soft: narrow sloping shoulders over a belly
            // that is the widest part of him, on legs set wide to clear it.
            Class::Civilian => Proportions {
                height: 1.70,
                hip_height: 0.48,
                torso_length: 0.32,
                arm_length: 0.29,
                head_length: 0.140,
                shoulder_half_width: 0.100,
                hip_half_width: 0.075,
                girth: 1.40,
                belly: 1.60,
            },
        }
    }

    pub fn name(&self) -> String {
        match self {
            Class::Scout => get!("class.names.scout"),
            Class::Soldier => get!("class.names.soldier"),
            Class::Pyro => get!("class.names.pyro"),
            Class::Demoman => get!("class.names.demoman"),
            Class::Heavy => get!("class.names.heavy"),
            Class::Engineer => get!("class.names.engineer"),
            Class::Medic => get!("class.names.medic"),
            Class::Sniper => get!("class.names.sniper"),
            Class::Spy => get!("class.names.spy"),
            Class::Civilian => get!("class.names.civilian"),
        }
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;
    use crate::common::skeleton::Proportions;

    /// [`TALLEST_CLASS_HEIGHT`] is what a spawn point's clearance is checked
    /// against, so a class taller than it would pass the editor's check and
    /// wedge its head in the ceiling at runtime. Trivially true while every
    /// class is the same shape; the point is that it stops being trivial the
    /// moment somebody gives one class longer legs.
    #[test]
    fn no_class_is_taller_than_the_clearance_a_spawn_is_checked_for() {
        for class in Class::iter() {
            let height = class.proportions().height;
            assert!(
                height <= TALLEST_CLASS_HEIGHT,
                "{class:?} is {height} m tall, taller than the {TALLEST_CLASS_HEIGHT} m a spawn point is checked for"
            );
        }
    }

    /// The roster is ten, and the display grid is laid out on that count.
    #[test]
    fn there_are_ten_classes() {
        assert_eq!(Class::iter().count(), 10);
    }

    /// Ten classes that looked the same would be one class listed ten times.
    /// No two share a build, and the extremes are far enough apart to tell at
    /// a glance rather than with a ruler.
    #[test]
    fn every_class_has_a_silhouette_of_its_own() {
        let builds: Vec<Proportions> = Class::iter().map(|class| class.proportions()).collect();

        for (index, build) in builds.iter().enumerate() {
            for other in &builds[index + 1..] {
                assert_ne!(build, other, "two classes are built identically");
            }
        }

        let spread = |of: fn(&Proportions) -> f32| {
            let values: Vec<f32> = builds.iter().map(of).collect();
            let low = values.iter().copied().fold(f32::MAX, f32::min);
            let high = values.iter().copied().fold(f32::MIN, f32::max);
            high / low
        };

        // Legs, shoulders and girth are what a silhouette is read by.
        assert!(spread(|p| p.height * (p.hip_height - 0.04)) > 1.2, "the legs are all alike");
        assert!(spread(|p| p.height * p.shoulder_half_width) > 1.5, "the shoulders are all alike");
        assert!(spread(|p| p.girth) > 2.0, "the builds are all alike");
    }

    /// Fat is not the same as big. The Civilian is the widest thing on the
    /// roster around the middle without being the biggest overall, which only
    /// works because a belly is its own number.
    #[test]
    fn the_civilian_is_rotund_rather_than_merely_large() {
        let civilian = Class::Civilian.proportions();
        let heavy = Class::Heavy.proportions();

        assert!(civilian.height < heavy.height, "the Civilian is not the smaller of the two");
        assert!(civilian.girth < heavy.girth, "the Civilian is built bigger than the Heavy");

        // Widest at the belly and narrow at the shoulders, which is the shape.
        let belly = |p: &Proportions| p.height * p.girth * p.belly * 0.20;
        let shoulders = |p: &Proportions| p.height * p.shoulder_half_width * 2.0;
        assert!(
            belly(&civilian) > shoulders(&civilian),
            "the Civilian's shoulders are wider than his middle"
        );
        // Wider round the middle than the Heavy in plain metres, while being
        // thirty centimetres shorter and built smaller everywhere else. That
        // is the difference between fat and big, and it is only expressible
        // because the belly is its own number.
        assert!(
            belly(&civilian) > belly(&heavy),
            "the Civilian's middle is {:.2} m against the Heavy's {:.2} m",
            belly(&civilian),
            belly(&heavy)
        );
    }
}

