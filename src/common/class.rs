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
    /// Deliberately the same for all ten. This is the seam the differences go
    /// through when they exist, so that a class's silhouette is one number
    /// changed here rather than a rig authored somewhere else.
    pub fn proportions(&self) -> Proportions {
        Proportions::DEFAULT
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
}
