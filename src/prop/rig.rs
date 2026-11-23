//! Bones on a prop. See `documentation/rigs.md`, "Props get rigs".
//!
//! A prop bone is a pivot with no length or limit, and it may slide as well as turn, so it is
//! posed by [`PropPose`] rather than a body's [`Pose`]. Meshes stay in prop space and a bone moves
//! its parts by its change from rest, so baking re-centers nothing.
//!
//! [`Pose`]: crate::common::skeleton::Pose

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::rotation::quat_from_euler;
use crate::common::skeleton::clip::{Axis, Channel, Clip, Combine};
use crate::common::skeleton::play::{angles_for, angles_onto, sample_key, shown_layers, track_time, ClipStack, Regions};
use crate::get;
use crate::prop::feature::{BodyRef, Evaluated, PropFeature, PropFeatureId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropBone {
    pub name: String,
    /// A bone listed before this one; `None` is the prop itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// What it turns about, in the prop's own frame, in meters.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub pivot: [f32; 3],
    /// Features whose bodies ride on it, including the body of whatever later ate one; see
    /// [`bone_of_each_body`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PropFeatureId>,
    /// Its parts are the round ready to fire; see [`PropRig::hide_spent_rounds`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub round: bool,
}

impl PropBone {
    pub fn new(name: impl Into<String>, pivot: Vec3) -> PropBone {
        PropBone { name: name.into(), parent: None, pivot: pivot.to_array(), parts: vec![], round: false }
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
    pub round: bool,
}

impl PropRig {
    /// A parent not listed before its child is dropped; [`problems`] reports it.
    pub fn of(bones: &[PropBone]) -> PropRig {
        let mut rig = PropRig::default();
        for bone in bones {
            let parent = bone
                .parent
                .as_deref()
                .and_then(|parent| rig.bones.iter().position(|earlier| earlier.name == parent));
            rig.bones.push(RigBone {
                name: bone.name.clone(),
                parent,
                pivot: Vec3::from_array(bone.pivot),
                round: bone.round,
            });
        }
        rig
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|bone| bone.name == name)
    }

    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    /// Each bone's change from rest, in the prop's frame.
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
/// binding its own feature, else the bone of the target it was made from (`FeatureOp::source_of`),
/// else of the first operand it ate that has one.
pub fn bone_of_each_body(features: &[PropFeature], bones: &[PropBone], evaluated: &Evaluated) -> Vec<Option<usize>> {
    evaluated.bodies.iter().map(|(body, _)| bone_of(*body, features, bones, 0)).collect()
}

/// Deeper than any feature list has consumed, so a malformed one cannot loop.
const MOST_ANCESTORS: usize = 256;

fn bone_of(body: BodyRef, features: &[PropFeature], bones: &[PropBone], depth: usize) -> Option<usize> {
    if depth > MOST_ANCESTORS {
        return None;
    }
    if let Some(bone) = bones.iter().position(|bone| bone.parts.contains(&body.feature)) {
        return Some(bone);
    }
    let feature = features.iter().find(|feature| feature.id == body.feature && feature.enabled)?;
    let own = feature.op.source_of(body.index).and_then(|source| bone_of(source, features, bones, depth + 1));
    own.or_else(|| feature.op.eats().into_iter().find_map(|operand| bone_of(operand, features, bones, depth + 1)))
}

/// `id` and every feature whose body it ate, nearest first.
pub fn lineage(id: PropFeatureId, features: &[PropFeature]) -> Vec<PropFeatureId> {
    let mut found = vec![id];
    let mut next = 0;
    while next < found.len() && found.len() <= MOST_ANCESTORS {
        let at = found[next];
        next += 1;
        if let Some(feature) = features.iter().find(|feature| feature.id == at && feature.enabled) {
            found.extend(feature.op.consumes().into_iter().filter(|eaten| !found.contains(eaten)).collect::<Vec<_>>());
        }
    }
    found
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

/// Masked where a layer above claims the weapon, as the weapon is.
pub fn apply_stack(stack: &ClipStack, clips: &Assets<Clip>, rig: &PropRig, pose: &mut PropPose) {
    pose.0.clear();
    pose.0.resize(rig.bones.len(), Transform::IDENTITY);
    if rig.is_empty() {
        return;
    }
    let mut before: Option<PropPose> = None;
    for (playing, clip, shown, masked) in shown_layers(stack, clips) {
        if masked.overlaps(Regions::WEAPON) {
            continue;
        }
        if shown >= 1.0 {
            apply_clip(clip, playing.time, rig, pose);
            continue;
        }
        let before = before.get_or_insert_with(PropPose::default);
        before.0.clone_from(&pose.0);
        apply_clip(clip, playing.time, rig, pose);
        pose.lerp(before, shown);
    }
}

/// Whether anything [`apply_stack`] would apply keys a prop bone.
fn moves_prop_bones(stack: &ClipStack, clips: &Assets<Clip>) -> bool {
    shown_layers(stack, clips).any(|(_, clip, _, masked)| {
        !masked.overlaps(Regions::WEAPON)
            && clip.tracks.iter().any(|track| {
                track.enabled && matches!(track.channel, Channel::PropBone { .. } | Channel::PropBoneOffset { .. })
            })
    })
}

impl PropRig {
    /// Empty for a prop with no bones.
    pub fn deltas_for(&self, stack: &ClipStack, clips: &Assets<Clip>) -> Vec<Transform> {
        if self.is_empty() {
            return vec![];
        }
        // An empty pose reads as rest.
        if !moves_prop_bones(stack, clips) {
            return self.deltas(&PropPose::default());
        }
        let mut pose = PropPose::default();
        apply_stack(stack, clips, self, &mut pose);
        self.deltas(&pose)
    }
}

impl PropRig {
    /// [`PropRig::deltas_for`], with the rounds hidden as [`PropRig::hide_spent_rounds`] says.
    pub fn drawn_deltas(&self, stack: &ClipStack, clips: &Assets<Clip>, loaded: bool) -> Vec<Transform> {
        let mut deltas = self.deltas_for(stack, clips);
        self.hide_spent_rounds(&mut deltas, stack, clips, loaded);
        deltas
    }

    /// Scales each round bone, and the bones under it, to nothing while the weapon is not
    /// `loaded`. The ammo hides a fired round, since a clip ends. A reload's clip shows the next
    /// round from its first key on that bone once the clip is fully faded in, or once the round is
    /// seated: while fading in, it draws the bone partway from rest.
    pub fn hide_spent_rounds(&self, deltas: &mut [Transform], stack: &ClipStack, clips: &Assets<Clip>, loaded: bool) {
        if loaded || !self.bones.iter().any(|bone| bone.round) {
            return;
        }
        let mut hidden = vec![false; self.bones.len()];
        for (index, bone) in self.bones.iter().enumerate() {
            let under_hidden = bone.parent.is_some_and(|parent| hidden[parent]);
            let seated = deltas.get(index).is_some_and(|delta| {
                delta.translation.length() < 1e-4 && delta.rotation.angle_between(Quat::IDENTITY) < 1e-4
            });
            let shown = clip_places(stack, clips, &bone.name).is_some_and(|weight| weight >= 1.0 || seated);
            hidden[index] = under_hidden || (bone.round && !shown);
            if hidden[index]
                && let Some(delta) = deltas.get_mut(index)
            {
                delta.scale = Vec3::ZERO;
            }
        }
    }
}

/// The weight of the most shown unmasked clip that has reached its first key on `bone`.
fn clip_places(stack: &ClipStack, clips: &Assets<Clip>, bone: &str) -> Option<f32> {
    let placing = shown_layers(stack, clips).filter(|(playing, clip, _, masked)| {
        let time = clip.wrap(playing.time);
        !masked.overlaps(Regions::WEAPON)
            && clip.tracks.iter().filter(|track| track.enabled).any(|track| match &track.channel {
                Channel::PropBone { bone: keyed, keys } | Channel::PropBoneOffset { bone: keyed, keys } => {
                    keyed == bone
                        && keys.iter().map(|key| key.at).reduce(f32::min).is_some_and(|first| {
                            track_time(track.timing, time, None) >= first
                        })
                }
                _ => false,
            })
    });
    placing.map(|(_, _, shown, _)| shown).reduce(f32::max)
}

pub fn place_part(prop: Transform, deltas: &[Transform], bone: Option<&OnPropBone>) -> Transform {
    match bone.and_then(|bone| deltas.get(bone.0)) {
        Some(delta) => prop * *delta,
        None => prop,
    }
}

/// Which bone a drawn part rides on. Absent: the prop itself.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct OnPropBone(pub usize);

#[cfg(test)]
mod tests {
    use crate::prop::feature::BodyRef;
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

    #[test]
    fn each_body_a_subtraction_makes_rides_its_own_targets_bone() {
        let mut doc = PropDoc::new("");
        let barrel = primitive(&mut doc, 0.0);
        let slide = primitive(&mut doc, 1.0);
        let bore = primitive(&mut doc, 0.05);
        doc.push(crate::prop::feature::FeatureOp::Subtract {
            targets: vec![BodyRef::whole(barrel), BodyRef::whole(slide)],
            tools: vec![BodyRef::whole(bore)],
        });
        let bones = [
            PropBone { parts: vec![barrel], ..PropBone::new("barrel", Vec3::ZERO) },
            PropBone { parts: vec![slide], ..PropBone::new("slide", Vec3::ZERO) },
        ];
        let evaluated = doc.evaluate();
        assert_eq!(evaluated.bodies.len(), 2, "{:?}", evaluated.problems);
        assert_eq!(bone_of_each_body(&doc.features, &bones, &evaluated), vec![Some(0), Some(1)]);
    }

    #[test]
    fn a_body_keeps_the_bone_of_what_it_ate() {
        let mut doc = PropDoc::new("");
        let barrel = primitive(&mut doc, 0.0);
        let bore = primitive(&mut doc, 0.05);
        let drilled = doc.push(
            crate::prop::feature::FeatureOp::Subtract { targets: vec![BodyRef::whole(barrel)], tools: vec![BodyRef::whole(bore)] },
        );
        let stock = primitive(&mut doc, 5.0);
        doc.bones.push(PropBone { parts: vec![barrel], ..PropBone::new("spin", Vec3::ZERO) });
        let evaluated = doc.evaluate();
        let bones = bone_of_each_body(&doc.features, &doc.bones, &evaluated);
        let on = |id| evaluated.bodies.iter().position(|(body, _)| body.feature == id).map(|index| bones[index]);
        assert_eq!(on(drilled), Some(Some(0)));
        assert_eq!(on(stock), Some(None));
    }

    /// The slide is bound before the bore cuts it, so it stays bound only through the cut.
    #[test]
    fn the_shipped_pistols_slide_kicks_back_when_it_fires() {
        let doc = crate::prop::document::parse(include_str!("../../assets/default/props/pistol.gpp")).unwrap();
        assert_eq!(problems(&doc.bones), Vec::<String>::new());
        let evaluated = doc.evaluate();
        let bones = bone_of_each_body(&doc.features, &doc.bones, &evaluated);
        let rig = PropRig::of(&doc.bones);
        let slide = rig.index_of("slide").unwrap();
        let named = |name: &str| doc.features.iter().find(|feature| feature.name == name).unwrap().id;
        let on = |id| evaluated.bodies.iter().position(|(body, _)| body.feature == id).map(|index| bones[index]);
        assert_eq!(on(named("Slide")), Some(Some(slide)));
        assert!(bones.contains(&None), "every body moves with a bone");

        // In both views: `fire` in first person, `recoil` on the body.
        for clip in ["fire", "recoil"] {
            let mut pose = PropPose::rest(&rig);
            apply_clip(&doc.animations.clips[clip], 0.03, &rig, &mut pose);
            // Forward is -Z, so back is +Z.
            assert!(rig.deltas(&pose)[slide].transform_point(Vec3::ZERO).z > 0.02, "{clip}");
        }
    }

    fn round_rig() -> PropRig {
        let mut round = PropBone::new("round", Vec3::ZERO);
        round.round = true;
        let mut fins = PropBone::new("fins", Vec3::ZERO);
        fins.parent = Some("round".into());
        PropRig::of(&[round, fins, PropBone::new("slide", Vec3::ZERO)])
    }

    fn stack_of(clip: Clip, time: f32) -> (ClipStack, Assets<Clip>) {
        let mut clips = Assets::<Clip>::default();
        let handle = clips.add(clip);
        (ClipStack(vec![crate::common::skeleton::play::Playing::new(crate::common::skeleton::play::Layer::Weapon, handle, time)]), clips)
    }

    /// Fails if an empty weapon still draws its round, or a loaded one hides it.
    #[test]
    fn a_round_is_drawn_only_while_loaded() {
        let rig = round_rig();
        let (stack, clips) = (ClipStack::default(), Assets::<Clip>::default());
        let loaded = rig.drawn_deltas(&stack, &clips, true);
        assert!(loaded.iter().all(|delta| delta.scale == Vec3::ONE));
        let empty = rig.drawn_deltas(&stack, &clips, false);
        assert_eq!(empty.iter().map(|delta| delta.scale).collect::<Vec<_>>(), [Vec3::ZERO, Vec3::ZERO, Vec3::ONE]);
    }

    /// A reload shows the next round from the key that first places it, not from its start.
    #[test]
    fn a_clip_shows_an_empty_weapons_round_from_its_first_key() {
        let rig = round_rig();
        let keyed = clip(Channel::PropBoneOffset {
            bone: "round".into(),
            keys: vec![Key::all(0.3, [0.0, -0.4, 0.2], Interpolation::Linear)],
        });
        let (before, clips) = stack_of(keyed.clone(), 0.2);
        assert_eq!(rig.drawn_deltas(&before, &clips, false)[0].scale, Vec3::ZERO);
        let (after, clips) = stack_of(keyed.clone(), 0.4);
        let shown = rig.drawn_deltas(&after, &clips, false)[0];
        assert_eq!(shown.scale, Vec3::ONE);
        assert!(shown.translation.abs_diff_eq(Vec3::new(0.0, -0.4, 0.2), 1e-6));
        let mut clips = Assets::<Clip>::default();
        let handle = clips.add(keyed.clone());
        let half_in = ClipStack(vec![crate::common::skeleton::play::Playing {
            weight: 0.5,
            ..crate::common::skeleton::play::Playing::new(crate::common::skeleton::play::Layer::Weapon, handle, 0.4)
        }]);
        assert_eq!(rig.drawn_deltas(&half_in, &clips, false)[0].scale, Vec3::ZERO, "a round showed sliding out of the tube");
        let (other, clips) = stack_of(clip(Channel::PropBoneOffset { bone: "slide".into(), keys: vec![Key::all(0.0, [0.0; 3], Interpolation::Linear)] }), 0.4);
        assert_eq!(rig.drawn_deltas(&other, &clips, false)[0].scale, Vec3::ZERO, "a clip moving another bone showed the round");
    }

    /// Fails if the RPG's rocket stops riding its round bone, or the reloads stop placing it.
    #[test]
    fn the_shipped_rpgs_rocket_is_its_round() {
        let doc = crate::prop::document::parse(include_str!("../../assets/default/props/rpg.gpp")).unwrap();
        assert_eq!(problems(&doc.bones), Vec::<String>::new());
        let rig = PropRig::of(&doc.bones);
        let round = rig.index_of("round").unwrap();
        assert!(rig.bones[round].round);
        let evaluated = doc.evaluate();
        let bones = bone_of_each_body(&doc.features, &doc.bones, &evaluated);
        let rocket = doc.features.iter().find(|feature| feature.name == "Round").unwrap().id;
        let on = evaluated.bodies.iter().position(|(body, _)| body.feature == rocket).map(|index| bones[index]);
        assert_eq!(on, Some(Some(round)));
        assert!(bones.contains(&None), "the tube rides the round too");
        for name in ["reload", "body_reload"] {
            let reload = &doc.animations.clips[name];
            let (start, clips) = stack_of(reload.clone(), 0.0);
            assert_eq!(rig.drawn_deltas(&start, &clips, false)[round].scale, Vec3::ZERO, "{name} shows the round before the hand has it");
            let (end, clips) = stack_of(reload.clone(), reload.length);
            let seated = rig.drawn_deltas(&end, &clips, false)[round];
            assert_eq!(seated.scale, Vec3::ONE, "{name}");
            assert!(seated.translation.length() < 1e-5, "{name} leaves the round out of the tube");
        }
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
