//! The shape a body is drawn as: one mesh per bone, built from the bone's own
//! length and thickness plus a [`Profile`] of cross-sections down it. Built
//! rather than imported so each class's [`Proportions`] reach its silhouette.
//!
//! Meshes are in bone space (local `+Y` head to tail, as
//! [`crate::common::skeleton::rig::Bone`] describes), so placing one is a
//! single transform; see [`crate::game::body_mesh`].

use bevy::prelude::*;

use crate::common::mesh::{rect_ring, rounded_ring, MeshBuilder};
use crate::common::skeleton::rig::{bone, Bone, Skeleton};

/// One cross-section across a bone.
#[derive(Clone, Copy, Debug)]
pub struct Ring {
    /// Fraction of the bone's length. Unclamped: a foot's heel is behind its
    /// head, at negative `t`.
    pub t: f32,
    /// Width and depth as a fraction of the bone's `thickness`.
    pub scale: Vec2,
    /// Center offset, as a fraction of `thickness`.
    pub offset: Vec2,
}

impl Ring {
    /// A centered section.
    pub const fn at(t: f32, x: f32, z: f32) -> Ring {
        Ring { t, scale: Vec2::new(x, z), offset: Vec2::ZERO }
    }

    /// A section whose underside stays where a full-size one's would be, so a
    /// tapering foot keeps its sole on the floor.
    pub const fn flat_bottomed(t: f32, x: f32, z: f32) -> Ring {
        Ring { t, scale: Vec2::new(x, z), offset: Vec2::new(0.0, -0.5 * (1.0 - z)) }
    }
}

/// The shape one bone is drawn as.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    /// How much of each corner is cut off. `0` builds a four-point square,
    /// since an eight-point ring chamfered by nothing has degenerate quads.
    pub chamfer: f32,
    /// Head to tail, in order. At least two.
    pub rings: &'static [Ring],
}

/// For a bone with no profile of its own.
const DEFAULT: Profile = Profile {
    chamfer: 0.3,
    rings: &[Ring::at(0.0, 1.0, 1.0), Ring::at(1.0, 0.85, 0.85)],
};

// Pelvis and chest both narrow where they meet, so the solids do not step.
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
const HEAD: Profile = Profile {
    chamfer: 0.28,
    rings: &[
        Ring::at(0.0, 0.74, 0.82),
        Ring::at(0.18, 1.0, 1.0),
        Ring::at(0.8, 1.0, 0.98),
        Ring::at(1.0, 0.72, 0.74),
    ],
};

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
const HAND: Profile = Profile {
    chamfer: 0.2,
    rings: &[Ring::at(0.0, 0.9, 0.95), Ring::at(0.25, 1.0, 1.0), Ring::at(1.0, 0.72, 0.8)],
};

const THIGH: Profile = Profile {
    chamfer: 0.35,
    rings: &[Ring::at(0.0, 1.0, 1.0), Ring::at(0.45, 0.9, 0.9), Ring::at(1.0, 0.74, 0.78)],
};
const SHIN: Profile = Profile {
    chamfer: 0.35,
    rings: &[Ring::at(0.0, 0.82, 0.86), Ring::at(0.3, 0.95, 1.0), Ring::at(1.0, 0.54, 0.6)],
};
/// Local `+Y` runs ankle to toe and `+Z` is up, so the heel is a negative `t`.
const FOOT: Profile = Profile {
    chamfer: 0.18,
    rings: &[
        Ring::flat_bottomed(-0.34, 0.82, 0.86),
        Ring::at(0.0, 1.0, 1.0),
        Ring::flat_bottomed(0.7, 0.98, 0.82),
        Ring::flat_bottomed(1.0, 0.82, 0.6),
    ],
};

/// The shape of each bone in the humanoid rig. Kept off [`Bone`], which says
/// how a body moves rather than how it is drawn.
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

/// One mesh per bone, in [`Skeleton::bones`] order, which forward kinematics
/// also uses, so parts are matched to bones by index.
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

    /// An empty or flat mesh is an invisible limb rather than a failure.
    #[test]
    fn every_bone_of_every_class_has_a_solid_mesh() {
        for class in [Class::Scout, Class::Heavy, Class::Civilian, Class::Sniper] {
            let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
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

    /// A part is placed by its bone's transform alone, so it must span the
    /// bone along `+Y`. The foot is exempt.
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

    /// Without the heel every body stands on the balls of its feet.
    #[test]
    fn a_foot_has_a_heel_behind_its_ankle() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let (min, _) = bounds(&named(&skeleton, bone::FOOT_L));
        assert!(min.y < -0.01, "no heel: the foot starts at {}", min.y);
    }

    /// A class's proportions must reach its silhouette.
    #[test]
    fn a_heavier_class_comes_out_heavier() {
        let heavy = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Heavy));
        let sniper = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Sniper));

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
