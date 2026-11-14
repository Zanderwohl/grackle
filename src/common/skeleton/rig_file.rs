//! A rig read from a pack's `rigs/<name>.toml`: its bones at a reference build, and the
//! [`Anatomy`] that says what they are for. A class names one in `classes.toml`; see
//! `documentation/rigs.md`.
//!
//! Bones are stated where they rest in a body `height` meters tall, feet on the floor, facing
//! `-Z`. A build resizes the whole rig to its own height, then lengthens each bone by its group's
//! scale; a child keeps its place along its parent, and the feet are put back on the floor.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::anatomy::{Anatomy, Foot, Limb};
use super::ik::Chain;
use super::play::Regions;
use super::rig::{intern, intern_list, Build, Builder, Skeleton};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigFile {
    /// Ground to crown at the reference build, meters.
    pub height: f32,
    /// Parents before children.
    pub bones: Vec<BoneEntry>,
    #[serde(default)]
    pub limbs: Vec<LimbEntry>,
    #[serde(default)]
    pub feet: Vec<FootEntry>,
    /// Limb names: the trigger hand, then the support hand.
    #[serde(default)]
    pub hands: Vec<String>,
    /// The bone whose tail a weapon hangs from.
    #[serde(default)]
    pub carry: Option<String>,
    /// The bone the head hitbox is built from.
    #[serde(default)]
    pub head: Option<String>,
    /// Where a weapon is fixed on a body with no hands.
    #[serde(default)]
    pub mount: Option<MountEntry>,
    #[serde(default)]
    pub look: Vec<LookEntry>,
    #[serde(default)]
    pub viewmodel: Vec<ViewmodelEntry>,
    pub measures: Measures,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoneEntry {
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
    /// Meters, at the reference build.
    pub head: [f32; 3],
    pub tail: [f32; 3],
    /// Full width on the bone's own X and Z, meters.
    pub thickness: [f32; 2],
    /// Which of a build's `scales` lengthens it.
    #[serde(default)]
    pub group: Option<String>,
    /// One of `Regions::NAMED`; absent is never masked.
    #[serde(default)]
    pub region: Option<String>,
    /// A joint kind from `joints::limit_named`; absent is narrow.
    #[serde(default)]
    pub limit: Option<String>,
    /// A shape from `mesh::profile_named`.
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimbEntry {
    pub name: String,
    pub upper: String,
    /// Bones between `upper` and `lower`, root first: a chain of three or more is solved by
    /// FABRIK, within each bone's joint limit.
    #[serde(default)]
    pub between: Vec<String>,
    pub lower: String,
    #[serde(default)]
    pub tip: Option<String>,
    /// -1 left, 1 right.
    pub side: f32,
    /// In the body's frame. Absent is forward for a limb that straightens, else an elbow's.
    #[serde(default)]
    pub pole: Option<[f32; 3]>,
    #[serde(default)]
    pub straightens: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FootEntry {
    pub limb: String,
    /// Fraction of a stride.
    pub phase: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountEntry {
    pub bone: String,
    /// Meters at the reference build, from the bone's tail, in the aim's frame (`-Z` along it).
    #[serde(default)]
    pub offset: [f32; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookEntry {
    pub bone: String,
    pub share: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewmodelEntry {
    pub bone: String,
    /// 0 the trigger hand, 1 the support hand.
    pub hand: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measures {
    /// The limb whose length is a leg-length.
    pub leg: String,
    /// The limb whose length is an arm-length; absent is a leg-length.
    #[serde(default)]
    pub arm: Option<String>,
    /// The bone whose head's height off the floor is a hip-height.
    pub hip: String,
}

impl RigFile {
    /// Builds it as the file states it, as a check: anything [`build_rig`] or
    /// [`Anatomy::problems`] objects to.
    pub fn check(&self, name: &str) -> Result<(), String> {
        let build = Build {
            rig: Some(name.to_string()),
            proportions: super::rig::Proportions { height: self.height, girth: 1.0, ..default() },
            scales: BTreeMap::new(),
        };
        let skeleton = build_rig(self, &build)?;
        match skeleton.anatomy().problems(skeleton.bones()).as_slice() {
            [] => Ok(()),
            problems => Err(problems.join("; ")),
        }
    }
}

/// `file` sized by `build`. Fails on a file that names a bone, limb, region, joint or shape it
/// does not define, or lists a child before its parent.
pub fn build_rig(file: &RigFile, build: &Build) -> Result<Skeleton, String> {
    if !(file.height.is_finite() && file.height > 0.0) {
        return Err(format!("a rig is some meters tall, not {}", file.height));
    }
    let index_of = |name: &str| file.bones.iter().position(|bone| bone.name == name);
    let uniform = build.proportions.height / file.height;
    let girth = build.proportions.girth;
    let scale_of = |bone: &BoneEntry| {
        bone.group.as_ref().and_then(|group| build.scales.get(group)).copied().unwrap_or(1.0)
    };

    // Placed parents first, each child offset from its parent's head by the parent's scale.
    let mut placed: Vec<(Vec3, Vec3)> = Vec::with_capacity(file.bones.len());
    for (index, bone) in file.bones.iter().enumerate() {
        let (head, tail) = (Vec3::from_array(bone.head), Vec3::from_array(bone.tail));
        let new_head = match &bone.parent {
            None => head * uniform,
            Some(parent) => {
                let parent_index = index_of(parent)
                    .filter(|at| *at < index)
                    .ok_or_else(|| format!("bone {} hangs from {parent}, which is not listed before it", bone.name))?;
                let parent_entry = &file.bones[parent_index];
                let offset = head - Vec3::from_array(parent_entry.head);
                placed[parent_index].0 + offset * uniform * scale_of(parent_entry)
            }
        };
        placed.push((new_head, new_head + (tail - head) * uniform * scale_of(bone)));
    }
    let lowest = |points: &mut dyn Iterator<Item = Vec3>| points.map(|point| point.y).fold(f32::MAX, f32::min);
    let reference_floor = lowest(&mut file.bones.iter().flat_map(|bone| [bone.head[1], bone.tail[1]].map(|y| Vec3::Y * y)));
    let floor = lowest(&mut placed.iter().flat_map(|(head, tail)| [*head, *tail]));
    let lift = Vec3::Y * (reference_floor * uniform - floor);

    let mut builder = Builder::default();
    for (bone, (head, tail)) in file.bones.iter().zip(&placed) {
        let limit = match &bone.limit {
            Some(kind) => super::joints::limit_named(kind).ok_or_else(|| format!("bone {}: no joint called {kind}", bone.name))?,
            None => super::joints::limit_named("unlisted").unwrap_or(super::joints::JointLimit::Fixed),
        };
        let profile = match &bone.profile {
            Some(kind) => super::mesh::profile_named(kind).ok_or_else(|| format!("bone {}: no shape called {kind}", bone.name))?,
            None => super::mesh::profile_named("default").ok_or("the default shape is missing")?,
        };
        builder.bone_shaped(
            intern(&bone.name),
            bone.parent.as_deref().map(intern),
            *head + lift,
            *tail + lift,
            Vec2::from_array(bone.thickness) * uniform * girth,
            (limit, profile),
        );
    }
    let bones = builder.bones;

    let bone = |name: &str| bones.iter().position(|bone| bone.name == name).ok_or_else(|| format!("no bone called {name}"));
    // Every rig's six, then a bit for each name of the rig's own, in the order its bones name them.
    let mut region_names: Vec<(&'static str, Regions)> =
        Regions::NAMED.into_iter().map(|(region, name)| (name, region)).collect();
    let mut regions = Vec::with_capacity(file.bones.len());
    for entry in &file.bones {
        let Some(name) = &entry.region else {
            regions.push(Regions::NONE);
            continue;
        };
        let region = match region_names.iter().find(|(named, _)| named == name) {
            Some((_, region)) => *region,
            None => {
                let index = Regions::FIRST_OWN + (region_names.len() - Regions::NAMED.len()) as u32;
                if index >= u32::BITS {
                    return Err(format!("bone {}: more regions than a rig may name", entry.name));
                }
                region_names.push((intern(name), Regions::bit(index)));
                Regions::bit(index)
            }
        };
        regions.push(region);
    }
    let limbs = file
        .limbs
        .iter()
        .map(|limb| {
            let upper = bone(&limb.upper)?;
            bone(&limb.lower)?;
            for name in &limb.between {
                bone(name)?;
            }
            if let Some(tip) = &limb.tip {
                bone(tip)?;
            }
            let pole = match (limb.pole, limb.straightens) {
                (Some(pole), _) => Vec3::from_array(pole),
                (None, true) => Vec3::NEG_Z,
                (None, false) => super::ik::elbow_pole(limb.side),
            };
            Ok(Limb {
                name: intern(&limb.name),
                chain: Chain {
                    upper: intern(&limb.upper),
                    middle: intern_list(&limb.between),
                    lower: intern(&limb.lower),
                    tip: limb.tip.as_deref().map(intern),
                },
                side: limb.side,
                pole,
                region: regions[upper],
                straightens: limb.straightens,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let limb = |name: &str| limbs.iter().position(|limb| limb.name == name).ok_or_else(|| format!("no limb called {name}"));
    let feet = file
        .feet
        .iter()
        .map(|foot| Ok(Foot { limb: limb(&foot.limb)?, phase: foot.phase }))
        .collect::<Result<Vec<_>, String>>()?;
    let mut hands = [None; 2];
    for (slot, name) in hands.iter_mut().zip(&file.hands) {
        *slot = Some(limb(name)?);
    }
    let reach = |limb: usize| {
        let chain = &limbs[limb].chain;
        chain.bones().filter_map(|name| bones.iter().find(|bone| bone.name == name)).map(|bone| bone.length).sum::<f32>()
    };
    let leg = reach(limb(&file.measures.leg)?);
    let arm = match &file.measures.arm {
        Some(name) => reach(limb(name)?),
        None => leg,
    };
    let hip = bone(&file.measures.hip)?;
    let hip = placed[hip].0.y + lift.y;

    let anatomy = Anatomy {
        feet,
        hands,
        carry: file.carry.as_deref().map(bone).transpose()?,
        head: file.head.as_deref().map(bone).transpose()?,
        mount: file
            .mount
            .as_ref()
            .map(|mount| Ok::<_, String>((bone(&mount.bone)?, Vec3::from_array(mount.offset) * uniform)))
            .transpose()?,
        look: file.look.iter().map(|look| Ok((bone(&look.bone)?, look.share))).collect::<Result<_, String>>()?,
        viewmodel: file.viewmodel.iter().map(|entry| Ok((bone(&entry.bone)?, entry.hand))).collect::<Result<_, String>>()?,
        regions,
        region_names,
        leg,
        arm,
        hip,
        limbs,
    };
    Ok(Skeleton::from_parts(build.clone(), bones, anatomy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::assets::{default_packs, Assets};
    use crate::common::skeleton::ik::{finish_pose, plant_feet, Reach};
    use crate::common::skeleton::rig::Pose;
    use crate::common::skeleton::state::{AnimationState, PoseInputs};

    fn strider() -> RigFile {
        crate::common::class_file::load_rig(&Assets::default(), &default_packs(), "strider")
            .expect("the default pack's strider should load")
    }

    fn sized(height: f32, scales: &[(&str, f32)]) -> Skeleton {
        let build = Build {
            rig: Some("strider".into()),
            proportions: crate::common::skeleton::Proportions { height, ..default() },
            scales: scales.iter().map(|(group, scale)| (group.to_string(), *scale)).collect(),
        };
        build_rig(&strider(), &build).unwrap()
    }

    fn lowest(skeleton: &Skeleton) -> f32 {
        skeleton
            .posed_bones(&Pose::rest(), &Transform::IDENTITY)
            .iter()
            .map(|bone| bone.head.y.min(bone.tail.y))
            .fold(f32::MAX, f32::min)
    }

    /// Fails if the shipped example stops building, or names something it does not define.
    #[test]
    fn the_strider_builds_with_nothing_to_complain_about() {
        let rig = sized(1.6, &[]);
        assert_eq!(rig.kind(), "strider");
        assert_eq!(rig.anatomy().problems(rig.bones()), Vec::<String>::new());
        assert_eq!(rig.anatomy().feet.len(), 2);
        assert!(rig.anatomy().hand(0).is_none(), "the strider has no hands");
        assert!(lowest(&rig).abs() < 1e-4, "the strider's feet are off the floor at {}", lowest(&rig));
    }

    /// A build resizes the whole rig to its height and each group by its scale, feet on the floor.
    #[test]
    fn a_build_sizes_the_rig_and_keeps_its_feet_down() {
        let reference = sized(1.6, &[]);
        let short = sized(0.8, &[]);
        assert!((short.anatomy().leg - reference.anatomy().leg * 0.5).abs() < 1e-4);

        let long_legged = sized(1.6, &[("leg", 1.25)]);
        assert!((long_legged.anatomy().leg - reference.anatomy().leg * 1.25).abs() < 1e-4);
        assert!(lowest(&long_legged).abs() < 1e-4, "longer legs put the feet at {}", lowest(&long_legged));
        assert!(long_legged.anatomy().hip > reference.anatomy().hip, "longer legs did not lift the hips");
    }

    #[test]
    fn a_file_naming_what_it_does_not_define_is_refused() {
        let mut child_first = strider();
        child_first.bones.swap(0, 1);
        assert!(child_first.check("strider").is_err(), "a child listed before its parent built");

        let mut bad_limb = strider();
        bad_limb.limbs[0].lower = "elbow".into();
        assert!(bad_limb.check("strider").is_err(), "a limb on a missing bone built");

        let mut bad_joint = strider();
        bad_joint.bones[1].limit = Some("swivel".into());
        assert!(bad_joint.check("strider").is_err(), "an unknown joint built");
    }

    /// The feet plant and the walk reaches, through the same path a match poses a body by; with no
    /// clip of its own a state stands in rest, never in the humanoid's pose functions.
    #[test]
    fn a_strider_stands_and_walks_on_its_own_feet() {
        let rig = sized(1.6, &[]);
        let root = Transform::IDENTITY;
        let mut pose = Pose::rest();
        let reached = plant_feet(&rig, &mut pose, &root, &[Default::default(); 2]);
        assert_eq!(reached[..], [Reach::Reached; 2]);

        for step in 0..16 {
            let inputs = PoseInputs { seconds: 0.0, stride: step as f32 / 16.0, speed: 4.0, travel: Vec2::Y, ..Default::default() };
            let pose = finish_pose(&rig, AnimationState::RunForward, &inputs, &root, None);
            assert!(
                pose.joint("chest") == Quat::IDENTITY && pose.joint("upper_arm.l") == Quat::IDENTITY,
                "the strider was posed by the humanoid's functions",
            );
            assert!(pose.joint("thigh.l") != Quat::IDENTITY, "the strider's legs did not walk at stride {step}");
        }
    }

    /// A clip for another rig names its limbs, its bones as frames and its feet by number, and
    /// survives being written and read back.
    #[test]
    fn a_clip_can_name_the_rigs_own_parts() {
        use crate::common::skeleton::clip::{Clip, Space, Timing};
        use crate::common::skeleton::play::{apply, Frames};

        let text = r#"
length = 1.0

[[tracks]]
combine = "replace"
unit = "meters"
enabled = true
timing = { foot = 1 }

[tracks.space]
bone = "pelvis"

[tracks.channel]
channel = "effector"
chain = "left_leg"

[[tracks.channel.keys]]
at = 0.0
to = [0.0, -0.6, 0.1]
"#;
        let clip: Clip = toml::from_str(text).expect("the clip should read");
        assert_eq!(clip.tracks[0].space, Space::Bone(crate::common::skeleton::clip::BoneRef::named("pelvis")));
        assert_eq!(clip.tracks[0].timing, Timing::Foot(1));
        let written = toml::to_string(&clip).unwrap();
        assert_eq!(toml::from_str::<Clip>(&written).unwrap(), clip, "{written}");

        let rig = sized(1.6, &[]);
        let root = Transform::IDENTITY;
        let mut pose = Pose::rest();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(root, root, &ground);
        let reaches = apply(&clip, 0.0, &rig, &mut pose, &frames, None);
        assert_eq!(reaches, [Reach::Reached], "the strider's own leg did not reach");

        let pelvis = rig.posed_bone(&Pose::rest(), &root, rig.index_of("pelvis").unwrap());
        let wanted = pelvis.tail + pelvis.rotation * Vec3::new(0.0, -0.6, 0.1);
        let ankle = rig.posed_bone(&pose, &root, rig.index_of("shin.l").unwrap()).tail;
        assert!(ankle.distance(wanted) < 1e-3, "the ankle is {} m off a target on the pelvis", ankle.distance(wanted));
    }

    /// A rig names regions of its own after every rig's six, and a claim by that name finds them.
    #[test]
    fn a_rig_names_its_own_regions() {
        let rig = sized(1.6, &[]);
        let anatomy = rig.anatomy();
        let tail = anatomy.region_named("tail").expect("the strider names its tail");
        assert_eq!(tail, Regions::bit(Regions::FIRST_OWN));
        assert_eq!(anatomy.region_of(rig.index_of("tail.2").unwrap()), tail);
        assert_eq!(anatomy.claims(["tail", "arm.l"]), tail.union(Regions::ARM_L));
        assert_eq!(anatomy.names_of(tail).collect::<Vec<_>>(), ["tail"]);
        assert!(crate::common::skeleton::default_humanoid().anatomy().region_named("tail").is_none());
    }

    /// A chain of three bones reaches what it can, stays inside every joint's limit, comes out the
    /// same every time, and points straight at what it cannot reach.
    #[test]
    fn a_longer_limb_reaches_within_its_joints() {
        use crate::common::skeleton::ik::solve;
        let rig = sized(1.6, &[]);
        let tail = *rig.anatomy().limb("tail").expect("the strider's tail is a limb");
        assert_eq!(tail.chain.middle, ["tail.2"]);
        let root = Transform::IDENTITY;
        let base = rig.posed_bone(&Pose::rest(), &root, rig.index_of("tail.1").unwrap()).head;
        let pole = Vec3::Y;

        let target = base + Vec3::new(0.15, 0.25, 0.45);
        let mut pose = Pose::rest();
        assert_eq!(solve(&rig, &mut pose, &root, &tail.chain, target, pole), Reach::Reached);
        let tip = rig.posed_bone(&pose, &root, rig.index_of("tail.3").unwrap()).tail;
        assert!(tip.distance(target) < 1e-3, "the tail tip is {} m off", tip.distance(target));

        for name in tail.chain.bones() {
            let index = rig.index_of(name).unwrap();
            let bone = &rig.bones()[index];
            let parent = rig.posed_bone(&pose, &root, bone.parent.unwrap()).rotation;
            let along = (parent * bone.rest).inverse() * rig.posed_bone(&pose, &root, index).rotation * Vec3::Y;
            assert!(bone.limit.nearest(along).distance(along) < 1e-3, "{name} is outside its joint");
        }

        let mut again = Pose::rest();
        solve(&rig, &mut again, &root, &tail.chain, target, pole);
        for name in tail.chain.bones() {
            assert_eq!(again.joint(name), pose.joint(name), "{name} came out different the second time");
        }

        let far = base + Vec3::new(0.0, 0.0, 5.0);
        let mut stretched = Pose::rest();
        assert_eq!(solve(&rig, &mut stretched, &root, &tail.chain, far, pole), Reach::Short);
        let tip = rig.posed_bone(&stretched, &root, rig.index_of("tail.3").unwrap()).tail;
        assert!(tip.is_finite() && (tip - base).normalize().dot(Vec3::Z) > 0.99, "a short tail does not point at its target");
    }

    /// Feet let go by a clip's plant weight are where the walk put them, not on the floor.
    #[test]
    fn feet_let_go_are_not_planted() {
        let rig = sized(1.6, &[]);
        let root = Transform::IDENTITY;
        let inputs = PoseInputs { seconds: 0.0, stride: 0.3, speed: 4.0, travel: Vec2::Y, feet_free: 0.0 };
        let planted = finish_pose(&rig, AnimationState::RunForward, &inputs, &root, None);
        let free = finish_pose(&rig, AnimationState::RunForward, &PoseInputs { feet_free: 1.0, ..inputs }, &root, None);
        assert_ne!(planted.joint("thigh.l"), free.joint("thigh.l"), "letting the feet go changed nothing");
        assert_eq!(free.joint("thigh.l"), Quat::IDENTITY, "a let-go strider leg was posed anyway");
    }

    /// Keys in different spaces are each placed in their own and blended in world space.
    #[test]
    fn a_track_blends_between_keys_in_different_spaces() {
        use crate::common::skeleton::clip::{BoneRef, Channel, ChainRef, Clip, Key, Space, Track};
        use crate::common::skeleton::play::{apply, Frames};
        use crate::common::skeleton::Interpolation;

        let rig = sized(1.6, &[]);
        let root = Transform::IDENTITY;
        let in_body = Vec3::new(-0.12, 0.2, -0.15);
        let on_pelvis = Vec3::new(0.0, -0.55, 0.0);
        let key = |at: f32, to: Vec3, space| Key { space, ..Key::all(at, to.to_array(), Interpolation::Linear) };
        let mut track = Track::rotation("thigh.l");
        track.channel = Channel::Effector {
            chain: ChainRef::LEFT_LEG,
            pole: None,
            keys: vec![key(0.0, in_body, None), key(1.0, on_pelvis, Some(Space::Bone(BoneRef::named("pelvis"))))],
        };
        let clip = Clip { length: 1.0, tracks: vec![track], ..Clip::default() };
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(root, root, &ground);
        let mut pose = Pose::rest();
        assert_eq!(apply(&clip, 0.5, &rig, &mut pose, &frames, None), [Reach::Reached]);

        let pelvis = rig.posed_bone(&Pose::rest(), &root, rig.index_of("pelvis").unwrap());
        let wanted = in_body.lerp(pelvis.tail + pelvis.rotation * on_pelvis, 0.5);
        let ankle = rig.posed_bone(&pose, &root, rig.index_of("shin.l").unwrap()).tail;
        assert!(ankle.distance(wanted) < 1e-3, "the ankle is {} m off the halfway point", ankle.distance(wanted));
    }
}
