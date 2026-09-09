//! The shape a body is drawn as: one mesh per bone, built from the same
//! numbers the bone is.
//!
//! A bone carries a length and a cross-section. This turns each one into a
//! solid and adds the one thing a box is missing: a **profile**, a handful of
//! cross-sections down the bone saying how wide it is where. A thigh is wide
//! at the hip and narrow at the knee, a skull is chamfered top and bottom, a
//! foot has a heel behind its own joint.
//!
//! That is all the same [`Proportions`] read at more than two points along
//! each bone, which is the argument for building the mesh rather than
//! importing one: `girth` and `belly` already differ per class and the
//! thicknesses already derive from them, so the Heavy comes out heavy for
//! free and a new class is nine numbers rather than a model.
//!
//! **Meshes are built in bone space**, with local `+Y` running head to tail as
//! [`crate::common::skeleton::rig::Bone`] describes, so putting one in the
//! world is setting a single transform — see [`crate::game::body_mesh`]. When
//! vertices are warped across a joint rather than following one bone rigidly,
//! this stays the authoring space and only the skinning changes.

use bevy::prelude::*;

use crate::common::mesh::{rect_ring, rounded_ring, MeshBuilder};
use crate::common::skeleton::rig::{bone, Bone, Skeleton};

/// One cross-section across a bone.
#[derive(Clone, Copy, Debug)]
pub struct Ring {
    /// How far along the bone it sits, as a fraction of the bone's length.
    ///
    /// Unclamped, so that a foot's heel — geometry behind the ankle, which is
    /// the bone's own head — is `t: -0.3` rather than a bone nobody animates.
    pub t: f32,
    /// Width and depth here, as a fraction of the bone's `thickness`.
    pub scale: Vec2,
    /// Where the section's middle sits, in the same fractions of `thickness`.
    ///
    /// Keeps a taper from narrowing about its own centre line: a foot thins
    /// towards the toe from the top, because its underside is the floor.
    pub offset: Vec2,
}

impl Ring {
    /// A section at its full size, centred.
    pub const fn at(t: f32, x: f32, z: f32) -> Ring {
        Ring { t, scale: Vec2::new(x, z), offset: Vec2::ZERO }
    }

    /// A section that keeps its underside where a full-size one would be.
    ///
    /// The foot's rule: narrowing `z` about the centre lifts the sole, and a
    /// body whose toes float reads as hovering.
    pub const fn flat_bottomed(t: f32, x: f32, z: f32) -> Ring {
        Ring { t, scale: Vec2::new(x, z), offset: Vec2::new(0.0, -0.5 * (1.0 - z)) }
    }
}

/// The shape one bone is drawn as.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    /// How much of each corner is cut off, `0` for a square section.
    ///
    /// Zero is its own case rather than the low end of a range: an eight-point
    /// ring chamfered by nothing is four coincident pairs and four degenerate
    /// quads, so a square section is built as a square.
    pub chamfer: f32,
    /// Sections from the head end to the tail end, in order. At least two.
    pub rings: &'static [Ring],
}

/// A plain tapered box, for a bone with nothing said about it — a new bone
/// that nobody gave a shape, which is better noticed on screen than crashed
/// on.
const DEFAULT: Profile = Profile {
    chamfer: 0.3,
    rings: &[Ring::at(0.0, 1.0, 1.0), Ring::at(1.0, 0.85, 0.85)],
};

// The spine flares at the hips and again across the chest, pinching at the
// waist between them. Both bones narrow where they meet, so the solids do not
// step against each other.
const PELVIS: Profile = Profile {
    chamfer: 0.3,
    rings: &[Ring::at(0.0, 0.94, 0.94), Ring::at(0.35, 1.0, 1.0), Ring::at(1.0, 0.86, 0.9)],
};
const CHEST: Profile = Profile {
    chamfer: 0.3,
    rings: &[Ring::at(0.0, 0.84, 0.88), Ring::at(0.45, 1.0, 1.0), Ring::at(1.0, 0.96, 0.84)],
};
const NECK: Profile = Profile {
    chamfer: 0.35,
    rings: &[Ring::at(0.0, 1.0, 1.0), Ring::at(1.0, 0.88, 0.88)],
};
/// A skull: full width through the middle, cut back at the jaw and again at
/// the crown, so it is not a brick standing on a neck.
const HEAD: Profile = Profile {
    chamfer: 0.28,
    rings: &[
        Ring::at(0.0, 0.74, 0.82),
        Ring::at(0.18, 1.0, 1.0),
        Ring::at(0.8, 1.0, 0.98),
        Ring::at(1.0, 0.72, 0.74),
    ],
};

/// Wider than its own bone where it meets the chest, so an arm comes off a
/// slope rather than out of a corner.
const CLAVICLE: Profile = Profile {
    chamfer: 0.35,
    rings: &[Ring::at(0.0, 1.15, 1.15), Ring::at(1.0, 0.8, 0.8)],
};
const UPPER_ARM: Profile = Profile {
    chamfer: 0.35,
    rings: &[Ring::at(0.0, 1.0, 1.0), Ring::at(0.3, 0.98, 0.98), Ring::at(1.0, 0.8, 0.8)],
};
const FOREARM: Profile = Profile {
    chamfer: 0.35,
    rings: &[Ring::at(0.0, 1.0, 1.0), Ring::at(0.35, 0.95, 0.95), Ring::at(1.0, 0.68, 0.72)],
};
/// Squarer than the arm above it: a hand is a slab, and the change of section
/// is most of what makes it read as one.
const HAND: Profile = Profile {
    chamfer: 0.2,
    rings: &[Ring::at(0.0, 0.9, 0.95), Ring::at(0.25, 1.0, 1.0), Ring::at(1.0, 0.72, 0.8)],
};

const THIGH: Profile = Profile {
    chamfer: 0.35,
    rings: &[Ring::at(0.0, 1.0, 1.0), Ring::at(0.45, 0.9, 0.9), Ring::at(1.0, 0.74, 0.78)],
};
/// A calf: swells just below the knee and runs down to an ankle much narrower
/// than the shin bone's own thickness.
const SHIN: Profile = Profile {
    chamfer: 0.35,
    rings: &[Ring::at(0.0, 0.82, 0.86), Ring::at(0.3, 0.95, 1.0), Ring::at(1.0, 0.54, 0.6)],
};
/// The one bone with geometry behind its own head. Its local `+Y` runs ankle
/// to toe and its local `+Z` is up, so the heel is a negative `t` and the sole
/// is what the offsets hold level.
const FOOT: Profile = Profile {
    chamfer: 0.18,
    rings: &[
        Ring::flat_bottomed(-0.34, 0.82, 0.86),
        Ring::at(0.0, 1.0, 1.0),
        Ring::flat_bottomed(0.7, 0.98, 0.82),
        Ring::flat_bottomed(1.0, 0.82, 0.6),
    ],
};

/// The shape of each bone in the humanoid rig.
///
/// Read off a name rather than stored on the [`Bone`], because it says how a
/// body is *drawn* and a rig says how it moves. A second silhouette — armour,
/// a class that is not a person — is another table here, not another skeleton.
pub fn profile(name: &str) -> Profile {
    match name {
        bone::PELVIS => PELVIS,
        bone::CHEST => CHEST,
        bone::NECK => NECK,
        bone::HEAD => HEAD,
        bone::CLAVICLE_L | bone::CLAVICLE_R => CLAVICLE,
        bone::UPPER_ARM_L | bone::UPPER_ARM_R => UPPER_ARM,
        bone::FOREARM_L | bone::FOREARM_R => FOREARM,
        bone::HAND_L | bone::HAND_R => HAND,
        bone::THIGH_L | bone::THIGH_R => THIGH,
        bone::SHIN_L | bone::SHIN_R => SHIN,
        bone::FOOT_L | bone::FOOT_R => FOOT,
        _ => DEFAULT,
    }
}

/// The mesh for one bone, in that bone's own frame.
pub fn bone_mesh(bone: &Bone) -> Mesh {
    let shape = profile(bone.name);
    let rings: Vec<Vec<Vec3>> = shape
        .rings
        .iter()
        .map(|ring| {
            let half = bone.thickness * ring.scale * 0.5;
            let offset = bone.thickness * ring.offset;
            let y = ring.t * bone.length;
            if shape.chamfer > 0.0 {
                rounded_ring(y, half, offset, shape.chamfer)
            } else {
                rect_ring(y, half, offset)
            }
        })
        .collect();

    let mut builder = MeshBuilder::new();
    builder.hull(&rings);
    builder.build()
}

/// One mesh per bone, in [`Skeleton::bones`] order.
///
/// The order is the contract: forward kinematics returns bones in it too, so a
/// part and the bone that places it share an index and nothing matches names
/// at frame rate.
pub fn body_meshes(skeleton: &Skeleton) -> Vec<Mesh> {
    skeleton.bones().iter().map(bone_mesh).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::class::Class;
    use crate::common::skeleton::rig::{humanoid, Proportions};

    fn bounds(mesh: &Mesh) -> (Vec3, Vec3) {
        let positions = match mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("a bone mesh has positions")
        {
            bevy::mesh::VertexAttributeValues::Float32x3(values) => values.clone(),
            other => panic!("wrong attribute format: {other:?}"),
        };
        positions.into_iter().map(Vec3::from_array).fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), point| (min.min(point), max.max(point)),
        )
    }

    fn named(skeleton: &Skeleton, name: &str) -> Mesh {
        let bone = skeleton
            .bones()
            .iter()
            .find(|bone| bone.name == name)
            .expect("the humanoid rig has that bone");
        bone_mesh(bone)
    }

    /// Every bone gets geometry, and none of it is empty. A one-ring profile,
    /// or a bone with no length, is an invisible limb rather than a failure.
    #[test]
    fn every_bone_of_every_class_has_a_solid_mesh() {
        for class in [Class::Scout, Class::Heavy, Class::Civilian, Class::Sniper] {
            let skeleton = humanoid(class.proportions());
            for (bone, mesh) in skeleton.bones().iter().zip(body_meshes(&skeleton)) {
                assert!(
                    mesh.count_vertices() >= 12,
                    "{} of the {:?} is barely a mesh: {} vertices",
                    bone.name,
                    class,
                    mesh.count_vertices(),
                );
                let (min, max) = bounds(&mesh);
                assert!(
                    (max - min).min_element() > 0.0,
                    "{} of the {:?} is flat: {min} to {max}",
                    bone.name,
                    class,
                );
            }
        }
    }

    /// A bone's mesh runs the bone's length along local `+Y`, which is what
    /// lets a part be placed by the bone's transform and nothing else. The
    /// foot is the exception, and says so.
    #[test]
    fn a_bones_mesh_spans_its_own_length() {
        let skeleton = humanoid(Proportions::DEFAULT);
        for bone in skeleton.bones() {
            if bone.name == bone::FOOT_L || bone.name == bone::FOOT_R {
                continue;
            }
            let (min, max) = bounds(&bone_mesh(bone));
            assert!(min.y.abs() < 1e-4, "{} starts at {}, not its head", bone.name, min.y);
            assert!(
                (max.y - bone.length).abs() < 1e-4,
                "{} ends at {}, not its tail at {}",
                bone.name,
                max.y,
                bone.length,
            );
        }
    }

    /// The heel is why [`Ring::t`] is unclamped. Lose it and every body stands
    /// on the balls of its feet.
    #[test]
    fn a_foot_has_a_heel_behind_its_ankle() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let (min, _) = bounds(&named(&skeleton, bone::FOOT_L));
        assert!(min.y < -0.01, "no heel: the foot starts at {}", min.y);
    }

    /// The argument for building the mesh: a class's numbers reach its
    /// silhouette with nobody authoring a second model.
    #[test]
    fn a_heavier_class_comes_out_heavier() {
        let heavy = humanoid(Class::Heavy.proportions());
        let sniper = humanoid(Class::Sniper.proportions());

        let width = |skeleton: &Skeleton| {
            let (min, max) = bounds(&named(skeleton, bone::CHEST));
            max.x - min.x
        };

        assert!(
            width(&heavy) > width(&sniper) * 1.5,
            "the Heavy's chest is {} across and the Sniper's {}",
            width(&heavy),
            width(&sniper),
        );
    }
}
