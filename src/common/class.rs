use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

use crate::common::skeleton::Proportions;
use crate::get;

/// Metres. The moving body, spawn clearance and the editor's spawn gizmo all
/// read these constants, so they cannot drift apart. Must stay at least the
/// maximum [`Class`] height; a test checks it.
pub const TALLEST_CLASS_HEIGHT: f32 = 2.0;

/// From the feet.
pub const TALLEST_CLASS_EYE_HEIGHT: f32 = 1.8;

/// Half-extent on the two ground axes.
pub const CLASS_RADIUS: f32 = 0.35;

pub const CLASS_HALF_EXTENTS: Vec3 =
    Vec3::new(CLASS_RADIUS, TALLEST_CLASS_HEIGHT * 0.5, CLASS_RADIUS);

/// Both the gap a body fits through only by ducking and the height of the
/// shootable box.
pub const CROUCH_HEIGHT: f32 = 1.1;

/// From the feet.
pub const CROUCH_EYE_HEIGHT: f32 = 0.88;

/// What the body is, not what it asked for (that is
/// [`crate::common::skeleton::BodyRequests`]): under a low ceiling a body stays
/// crouched whether it wants to or not.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Reflect, Serialize, Deserialize)]
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

    pub fn half_extents(&self) -> Vec3 {
        Vec3::new(CLASS_RADIUS, self.height() * 0.5, CLASS_RADIUS)
    }

    /// Relative to the body's centre, which is what its transform is.
    pub fn eye_offset(&self) -> f32 {
        self.eye_height() - self.height() * 0.5
    }

    /// The sign is the caller's: down keeping the feet still on the ground,
    /// up keeping the head still in the air.
    pub fn centre_shift() -> f32 {
        (Stance::Standing.height() - Stance::Crouched.height()) * 0.5
    }
}

/// Spawn points mark the feet; collision works in centres.
pub fn body_centre_from_feet(feet: Vec3) -> Vec3 {
    feet + Vec3::Y * (TALLEST_CLASS_HEIGHT * 0.5)
}

/// Declaration order is display order only; nothing persists a class by
/// index. A component because a viewer builds a remote body's rig from it.
#[derive(
    Component, EnumIter, Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect,
    Serialize, Deserialize,
)]
pub enum Class {
    #[default]
    Scout,
    Soldier,
    Pyro,
    Demoman,
    Heavy,
    Engineer,
    Medic,
    Sniper,
    Spy,
    /// The one being escorted.
    Civilian,
    /// The placeholder everybody plays as until a class can be picked. Its
    /// loadout carries one of every kind of action.
    ///
    /// A corpse's build is read off its rig, never keyed on a class; a body
    /// need not have one.
    Mercenary,
}

impl Class {
    /// The data-file key, derived from the variant (`snake_case`) so no table
    /// can fall out of step with the enum.
    pub fn key(class: Class) -> String {
        let name = format!("{class:?}");
        let mut key = String::with_capacity(name.len() + 2);
        for (index, character) in name.char_indices() {
            if character.is_uppercase() && index > 0 {
                key.push('_');
            }
            key.extend(character.to_lowercase());
        }
        key
    }

    pub fn named(key: &str) -> Option<Class> {
        Class::iter().find(|class| Self::key(*class) == key)
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
            Class::Mercenary => get!("class.names.mercenary"),
            Class::Civilian => get!("class.names.civilian"),
        }
    }
}

/// What each class is built like, read from a pack's `classes.toml` by
/// [`crate::common::class_file`]. Held by the authority and sent to clients
/// with the weapon catalogue, so every machine builds the same rigs.
///
/// Keyed by [`Class::key`] in a `BTreeMap` so two equal tables compare equal;
/// see [`crate::common::pack_sync`].
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClassBuilds(pub BTreeMap<String, Proportions>);

impl ClassBuilds {
    /// A class the pack does not describe is built as [`Proportions::DEFAULT`]
    /// rather than not at all.
    pub fn of(&self, class: Class) -> Proportions {
        self.0.get(&Class::key(class)).copied().unwrap_or(Proportions::DEFAULT)
    }

    /// The default pack's builds. What tests and tools use; there is
    /// deliberately no table written in Rust to drift from the file.
    pub fn shipped() -> Self {
        crate::common::class_file::load_builds(
            &crate::common::assets::Assets::default(),
            &crate::common::assets::default_packs(),
        )
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;
    use crate::common::skeleton::Proportions;

    /// Fails if a class is taller than spawn clearance is checked for, which
    /// would pass the editor and wedge a head in the ceiling at runtime.
    #[test]
    fn no_class_is_taller_than_the_clearance_a_spawn_is_checked_for() {
        for class in Class::iter() {
            let height = ClassBuilds::shipped().of(class).height;
            assert!(
                height <= TALLEST_CLASS_HEIGHT,
                "{class:?} is {height} m tall, taller than the {TALLEST_CLASS_HEIGHT} m a spawn point is checked for"
            );
        }
    }

    /// Ten plus the `Mercenary` placeholder, which is in the iterator on
    /// purpose so the animation grid shows it.
    #[test]
    fn there_are_ten_classes_and_the_placeholder() {
        assert_eq!(Class::iter().count(), 11);
    }

    /// Fails if two classes share a build or the extremes are too close to
    /// tell apart at a glance.
    #[test]
    fn every_class_has_a_silhouette_of_its_own() {
        let builds: Vec<Proportions> = Class::iter().map(|class| ClassBuilds::shipped().of(class)).collect();

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

        assert!(spread(|p| p.height * (p.hip_height - 0.04)) > 1.2, "the legs are all alike");
        assert!(spread(|p| p.height * p.shoulder_half_width) > 1.5, "the shoulders are all alike");
        assert!(spread(|p| p.girth) > 2.0, "the builds are all alike");
    }

    /// Fails if the Civilian stops being wider at the middle than the Heavy
    /// while smaller overall, which needs `belly` separate from `girth`.
    #[test]
    fn the_civilian_is_rotund_rather_than_merely_large() {
        let civilian = ClassBuilds::shipped().of(Class::Civilian);
        let heavy = ClassBuilds::shipped().of(Class::Heavy);

        assert!(civilian.height < heavy.height, "the Civilian is not the smaller of the two");
        assert!(civilian.girth < heavy.girth, "the Civilian is built bigger than the Heavy");

        // Measured off the built rig: on a build this wide
        // `shoulder_half_width` is only a floor.
        let belly = |p: &Proportions| p.height * p.girth * p.belly * 0.20;
        let shoulders = |p: &Proportions| {
            let skeleton = crate::common::skeleton::humanoid(*p);
            let bones = skeleton.posed_bones(
                &crate::common::skeleton::Pose::rest(),
                &bevy::prelude::Transform::IDENTITY,
            );
            let shoulder = bones
                .iter()
                .find(|bone| bone.name == crate::common::skeleton::bone::UPPER_ARM_R)
                .expect("the rig has an arm");
            shoulder.head.x * 2.0
        };
        assert!(
            belly(&civilian) > shoulders(&civilian),
            "the Civilian's shoulders are {:.2} m across and his middle {:.2} m",
            shoulders(&civilian),
            belly(&civilian),
        );
        assert!(
            belly(&civilian) > belly(&heavy),
            "the Civilian's middle is {:.2} m against the Heavy's {:.2} m",
            belly(&civilian),
            belly(&heavy)
        );
    }
}

