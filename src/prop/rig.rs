//! Bones on a prop: a magazine that drops, a cylinder that swings, a fan. See
//! `documentation/rigs.md`, "Props get rigs".
//!
//! A prop bone is a pivot, not a body's bone: it has no length and no limit, and it may move as
//! well as turn, since a magazine slides out. So it is posed by [`PropPose`], one transform per
//! bone, rather than a body's [`Pose`]. Meshes stay in prop space; a bone moves its parts by its
//! *change from rest*, so nothing is re-centered when it is baked.
//!
//! [`Pose`]: crate::common::skeleton::Pose

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::rotation::quat_from_euler;
use crate::common::skeleton::clip::{Axis, Channel, Clip, Combine};
use crate::common::skeleton::play::{angles_for, angles_onto, sample_key, track_time, ClipStack, Regions};
use crate::get;
use crate::prop::feature::{Evaluated, PropFeature, PropFeatureId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropBone {
    pub name: String,
    /// A bone listed before this one; `None` is the prop itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// What it turns about, in the prop's own frame, in meters.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub pivot: [f32; 3],
    /// Features whose bodies ride on it: the body a feature made, or the body made by whatever
    /// later ate it, so drilling a bore through a bound barrel keeps it bound.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PropFeatureId>,
}

impl PropBone {
    pub fn new(name: impl Into<String>, pivot: Vec3) -> PropBone {
        PropBone { name: name.into(), parent: None, pivot: pivot.to_array(), parts: vec![] }
    }
}

/// A prop's bones, resolved: parents by index, always earlier in the list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PropRig {
    pub bones: Vec<RigBone>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RigBone {
    pub name: String,
    pub parent: Option<usize>,
    pub pivot: Vec3,
}

impl PropRig {
    /// A bone whose parent is not listed before it rides on the prop itself; [`problems`] says
    /// so.
    pub fn of(bones: &[PropBone]) -> PropRig {
        let mut rig = PropRig::default();
        for bone in bones {
            let parent = bone
                .parent
                .as_deref()
                .and_then(|parent| rig.bones.iter().position(|earlier| earlier.name == parent));
            rig.bones.push(RigBone { name: bone.name.clone(), parent, pivot: Vec3::from_array(bone.pivot) });
        }
        rig
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|bone| bone.name == name)
    }

    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    /// Each bone's change from rest in the prop's frame, parents first: what its parts are
    /// moved by.
    pub fn deltas(&self, pose: &PropPose) -> Vec<Transform> {
        let mut deltas: Vec<Transform> = Vec::with_capacity(self.bones.len());
        for (index, bone) in self.bones.iter().enumerate() {
            let local = pose.0.get(index).copied().unwrap_or_default();
            let about_pivot = Transform::from_translation(local.translation + bone.pivot)
                * Transform::from_rotation(local.rotation)
                * Transform::from_translation(-bone.pivot);
            let parent = bone.parent.and_then(|parent| deltas.get(parent).copied()).unwrap_or_default();
            deltas.push(parent * about_pivot);
        }
        deltas
    }
}

/// Each listed problem makes a bone or a binding do nothing, with no error where it happens.
pub fn problems(bones: &[PropBone]) -> Vec<String> {
    let mut problems = Vec::new();
    for (index, bone) in bones.iter().enumerate() {
        if bones[..index].iter().any(|earlier| earlier.name == bone.name) {
            problems.push(get!("prop.bones.problems.duplicate", "bone", bone.name));
        }
        if let Some(parent) = &bone.parent
            && !bones[..index].iter().any(|earlier| &earlier.name == parent)
        {
            problems.push(get!("prop.bones.problems.parent", "bone", bone.name, "parent", parent));
        }
        for part in &bone.parts {
            if bones[..index].iter().any(|earlier| earlier.parts.contains(part)) {
                problems.push(get!("prop.bones.problems.bound_twice", "bone", bone.name, "feature", part));
            }
        }
    }
    problems
}

/// The bone each of `evaluated.bodies` rides on, in the same order. A body takes the first bone
/// binding its own feature, else the bone of the first operand it ate that has one.
pub fn bone_of_each_body(features: &[PropFeature], bones: &[PropBone], evaluated: &Evaluated) -> Vec<Option<usize>> {
    evaluated.bodies.iter().map(|(id, _)| bone_of(*id, features, bones, 0)).collect()
}

/// Deeper than any feature list has consumed, so a malformed one cannot loop.
const MOST_ANCESTORS: usize = 256;

fn bone_of(id: PropFeatureId, features: &[PropFeature], bones: &[PropBone], depth: usize) -> Option<usize> {
    if depth > MOST_ANCESTORS {
        return None;
    }
    if let Some(bone) = bones.iter().position(|bone| bone.parts.contains(&id)) {
        return Some(bone);
    }
    let feature = features.iter().find(|feature| feature.id == id && feature.enabled)?;
    feature.op.consumes().into_iter().find_map(|eaten| bone_of(eaten, features, bones, depth + 1))
}

/// Where each bone is, as a turn about its pivot and a move in its parent's frame. Rest is
/// identity.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct PropPose(pub Vec<Transform>);

impl PropPose {
    pub fn rest(rig: &PropRig) -> PropPose {
        PropPose(vec![Transform::IDENTITY; rig.bones.len()])
    }

    fn lerp(&mut self, before: &PropPose, shown: f32) {
        for (now, was) in self.0.iter_mut().zip(&before.0) {
            now.translation = was.translation.lerp(now.translation, shown);
            now.rotation = was.rotation.slerp(now.rotation, shown);
        }
    }
}

/// The prop-bone tracks of one clip, onto `pose`. Body tracks are left to [`apply_stack`].
///
/// [`apply_stack`]: crate::common::skeleton::play::apply_stack
pub fn apply_clip(clip: &Clip, time: f32, rig: &PropRig, pose: &mut PropPose) {
    let time = clip.wrap(time);
    for track in clip.tracks.iter().filter(|track| track.enabled) {
        let (bone, keys, turns) = match &track.channel {
            Channel::PropBone { bone, keys } => (bone, keys, true),
            Channel::PropBoneOffset { bone, keys } => (bone, keys, false),
            _ => continue,
        };
        let Some(index) = rig.index_of(bone) else { continue };
        let time = track_time(track.timing, time, None);
        let Some(to) = sample_key(keys, time, clip.length, clip.looping) else { continue };
        let Some(local) = pose.0.get_mut(index) else { continue };
        if turns {
            local.rotation = match track.combine {
                Combine::Replace => quat_from_euler(angles_for(to, local.rotation)),
                Combine::Compose => local.rotation * quat_from_euler(angles_onto(to)),
            };
        } else {
            for axis in 0..3 {
                local.translation[axis] = match (to[axis], track.combine) {
                    (Axis::Value(at), Combine::Replace) => at,
                    (Axis::Value(at), Combine::Compose) => local.translation[axis] + at,
                    (Axis::Free | Axis::Ground { .. }, _) => local.translation[axis],
                };
            }
        }
    }
}

/// Every clip on `stack`, lowest layer first, each over what is beneath by how far it is shown.
/// A layer above that claims the weapon masks the prop's bones beneath it, as it masks the
/// weapon.
pub fn apply_stack(stack: &ClipStack, clips: &Assets<Clip>, rig: &PropRig, pose: &mut PropPose) {
    *pose = PropPose::rest(rig);
    if rig.is_empty() {
        return;
    }
    let mut ranked: Vec<_> = stack.0.iter().collect();
    ranked.sort_by_key(|playing| playing.layer);
    for playing in ranked {
        let shown = (playing.weight * playing.strength).clamp(0.0, 1.0);
        let Some(clip) = clips.get(&playing.clip).filter(|_| shown > 0.0) else { continue };
        if stack.claimed_above(playing.layer).overlaps(Regions::WEAPON) {
            continue;
        }
        let before = pose.clone();
        apply_clip(clip, playing.time, rig, pose);
        if shown < 1.0 {
            pose.lerp(&before, shown);
        }
    }
}

/// Which bone a drawn part rides on. Absent: the prop itself.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct OnPropBone(pub usize);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::clip::{Key, Track};
    use crate::common::skeleton::interpolation::Interpolation;
    use crate::prop::document::PropDoc;
    use crate::prop::profile::Placement;
    use crate::prop::solid::Shape;
    use crate::prop::surface::Surface;

    fn hinge() -> PropRig {
        PropRig::of(&[PropBone::new("lid", Vec3::new(0.0, 1.0, 0.0))])
    }

    fn clip(channel: Channel) -> Clip {
        let mut clip = Clip { length: 1.0, ..default() };
        clip.tracks.push(Track::of(channel));
        clip
    }

    /// A turn is about the pivot, so a point on the pivot stays put and one beside it swings.
    #[test]
    fn a_bone_turns_its_parts_about_its_pivot() {
        let rig = hinge();
        let mut pose = PropPose::rest(&rig);
        pose.0[0].rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let delta = rig.deltas(&pose)[0];
        assert!(delta.transform_point(Vec3::new(0.0, 1.0, 0.0)).abs_diff_eq(Vec3::new(0.0, 1.0, 0.0), 1e-5));
        assert!(delta.transform_point(Vec3::new(1.0, 1.0, 0.0)).abs_diff_eq(Vec3::new(0.0, 2.0, 0.0), 1e-5));
    }

    #[test]
    fn a_child_is_carried_by_its_parent() {
        let mut bones = vec![PropBone::new("arm", Vec3::ZERO), PropBone::new("hand", Vec3::X)];
        bones[1].parent = Some("arm".into());
        let rig = PropRig::of(&bones);
        let mut pose = PropPose::rest(&rig);
        pose.0[0].translation = Vec3::Y;
        assert!(rig.deltas(&pose)[1].transform_point(Vec3::X).abs_diff_eq(Vec3::new(1.0, 1.0, 0.0), 1e-5));
    }

    #[test]
    fn a_parent_listed_after_its_child_is_a_problem_and_carries_nothing() {
        let mut bones = vec![PropBone::new("hand", Vec3::X), PropBone::new("arm", Vec3::ZERO)];
        bones[0].parent = Some("arm".into());
        assert_eq!(problems(&bones).len(), 1);
        assert_eq!(PropRig::of(&bones).bones[0].parent, None);
    }

    #[test]
    fn a_clip_keys_a_turn_and_a_slide() {
        let rig = hinge();
        let mut pose = PropPose::rest(&rig);
        let key = |to| vec![Key::all(0.0, to, Interpolation::Linear)];
        apply_clip(&clip(Channel::PropBone { bone: "lid".into(), keys: key([0.5, 0.0, 0.0]) }), 0.0, &rig, &mut pose);
        apply_clip(
            &clip(Channel::PropBoneOffset { bone: "lid".into(), keys: key([0.0, -0.1, 0.0]) }),
            0.0,
            &rig,
            &mut pose,
        );
        assert!(pose.0[0].rotation.angle_between(quat_from_euler(Vec3::new(0.5, 0.0, 0.0))) < 1e-5);
        assert!(pose.0[0].translation.abs_diff_eq(Vec3::new(0.0, -0.1, 0.0), 1e-6));
    }

    fn primitive(doc: &mut PropDoc, x: f32) -> PropFeatureId {
        doc.push(
            crate::prop::feature::FeatureOp::Primitive {
                shape: Shape::default(),
                placement: Placement::at(x, 0.0, 0.0),
                surface: Surface::default(),
            },
        )
    }

    /// Drilling a bore through a bound barrel leaves the drilled barrel on the bone.
    #[test]
    fn a_body_keeps_the_bone_of_what_it_ate() {
        let mut doc = PropDoc::new("");
        let barrel = primitive(&mut doc, 0.0);
        let bore = primitive(&mut doc, 0.05);
        let drilled = doc.push(
            crate::prop::feature::FeatureOp::Boolean {
                op: crate::prop::feature::BooleanOp::Subtract,
                target: barrel,
                tool: bore,
            },
        );
        let stock = primitive(&mut doc, 5.0);
        doc.bones.push(PropBone { parts: vec![barrel], ..PropBone::new("spin", Vec3::ZERO) });
        let evaluated = doc.evaluate();
        let bones = bone_of_each_body(&doc.features, &doc.bones, &evaluated);
        let on = |id| evaluated.bodies.iter().position(|(body, _)| *body == id).map(|index| bones[index]);
        assert_eq!(on(drilled), Some(Some(0)));
        assert_eq!(on(stock), Some(None));
    }

    /// The pistol's slide is cut by the bore after it is bound, so it stays bound only through
    /// what ate it; the grip rides on nothing.
    #[test]
    fn the_shipped_pistols_slide_kicks_back_when_it_fires() {
        let doc = crate::prop::document::parse(include_str!("../../assets/default/props/pistol.gpp")).unwrap();
        assert_eq!(problems(&doc.bones), Vec::<String>::new());
        let evaluated = doc.evaluate();
        let bones = bone_of_each_body(&doc.features, &doc.bones, &evaluated);
        let rig = PropRig::of(&doc.bones);
        let slide = rig.index_of("slide").unwrap();
        let named = |name: &str| doc.features.iter().find(|feature| feature.name == name).unwrap().id;
        let on = |id| evaluated.bodies.iter().position(|(body, _)| *body == id).map(|index| bones[index]);
        assert_eq!(on(named("Slide")), Some(Some(slide)));
        assert!(bones.contains(&None), "every body moves with a bone");

        let mut pose = PropPose::rest(&rig);
        apply_clip(&doc.animations.clips["fire"], 0.03, &rig, &mut pose);
        // Forward is -Z, so back is +Z.
        assert!(rig.deltas(&pose)[slide].transform_point(Vec3::ZERO).z > 0.02);
    }

    #[test]
    fn bones_survive_the_round_trip() {
        let mut doc = PropDoc::new("");
        let mut bone = PropBone::new("fan", Vec3::new(0.0, 0.05, 0.0));
        bone.parts.push(PropFeatureId(3));
        doc.bones.push(bone);
        let mut child = PropBone::new("blade", Vec3::ZERO);
        child.parent = Some("fan".into());
        doc.bones.push(child);
        let text = crate::prop::document::to_text(&doc).unwrap();
        assert_eq!(crate::prop::document::parse(&text).unwrap().bones, doc.bones, "{text}");
    }
}
