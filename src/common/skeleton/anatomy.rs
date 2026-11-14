//! What a rig says about itself, so nothing outside its builder names one of its bones. See
//! `documentation/rigs.md`.

use std::ops::Deref;

use bevy::prelude::*;

use super::ik::Chain;
use super::play::Regions;

/// A two-bone chain, under the name a clip's `chain` uses.
#[derive(Clone, Copy, Debug)]
pub struct Limb {
    pub name: &'static str,
    pub chain: Chain,
    /// -1 on the left, 1 on the right: what a pole is mirrored by.
    pub side: f32,
    /// Which way the middle joint bends when nothing says, in the body's frame.
    pub pole: Vec3,
    pub region: Regions,
    /// Out of reach, straighten toward the target rather than stay put. False for an arm, since a
    /// straightened arm reads as broken.
    pub straightens: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Foot {
    /// Index into [`Anatomy::limbs`].
    pub limb: usize,
    /// Fraction of a stride.
    pub phase: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Anatomy {
    pub limbs: Vec<Limb>,
    /// The first two are what `Timing::LeftFoot` and `RightFoot` name.
    pub feet: Vec<Foot>,
    /// The trigger hand, then the support hand, as indices into `limbs`.
    pub hands: [Option<usize>; 2],
    /// The bone whose tail a weapon hangs from: the point between the shoulders.
    pub carry: Option<usize>,
    /// For a body with no hands: the bone a weapon is fixed to, and where on it, in meters along
    /// the aim from its tail. It replaces the hold's carry; nothing grips.
    pub mount: Option<(usize, Vec3)>,
    /// The bone the head hitbox is built from; its tail is the crown.
    pub head: Option<usize>,
    /// Bones that bend with the aim's pitch, each with its share. The shares sum to one.
    pub look: Vec<(usize, f32)>,
    /// Bones a first-person view draws, each with the hand (0 trigger, 1 support) it belongs to.
    pub viewmodel: Vec<(usize, usize)>,
    /// One per bone. `NONE` is never masked.
    pub regions: Vec<Regions>,
    /// What a claim may name on this rig: `Regions::NAMED`, then the rig's own.
    pub region_names: Vec<(&'static str, Regions)>,
    /// In meters: the units clips and carries are measured in.
    pub leg: f32,
    pub arm: f32,
    pub hip: f32,
}

impl Anatomy {
    pub fn limb(&self, name: &str) -> Option<&Limb> {
        self.limbs.iter().find(|limb| limb.name == name)
    }

    pub fn hand(&self, which: usize) -> Option<&Limb> {
        self.limbs.get(self.hands.get(which).copied().flatten()?)
    }

    pub fn region_of(&self, bone: usize) -> Regions {
        self.regions.get(bone).copied().unwrap_or(Regions::NONE)
    }

    pub fn region_named(&self, name: &str) -> Option<Regions> {
        self.region_names.iter().find(|(named, _)| *named == name).map(|(_, region)| *region)
    }

    /// What names claim on this rig. A name it does not have claims nothing here: it may be
    /// another rig's.
    pub fn claims<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Regions {
        names.into_iter().filter_map(|name| self.region_named(name)).fold(Regions::NONE, Regions::union)
    }

    /// The names of the regions in `claimed`, in `region_names` order.
    pub fn names_of(&self, claimed: Regions) -> impl Iterator<Item = &'static str> + '_ {
        self.region_names.iter().filter(move |(_, region)| claimed.overlaps(*region)).map(|(name, _)| *name)
    }

    /// Each problem makes something do nothing, with no error where it happens.
    pub fn problems(&self, bones: &[super::rig::Bone]) -> Vec<String> {
        let mut problems = Vec::new();
        let has = |name: &str| bones.iter().any(|bone| bone.name == name);
        for limb in &self.limbs {
            for name in limb.chain.bones().chain(limb.chain.tip) {
                if !has(name) {
                    problems.push(format!("limb {} names a bone it does not have, {name}", limb.name));
                }
            }
        }
        if self.feet.len() > MOST_FEET {
            problems.push(format!("{} feet, but only {MOST_FEET} are planted", self.feet.len()));
        }
        if self.feet.iter().any(|foot| foot.limb >= self.limbs.len()) {
            problems.push("a foot names no limb".into());
        }
        if self.hands.iter().flatten().any(|hand| *hand >= self.limbs.len()) {
            problems.push("a hand names no limb".into());
        }
        if self.regions.len() != bones.len() {
            problems.push(format!("{} regions for {} bones", self.regions.len(), bones.len()));
        }
        let shares: f32 = self.look.iter().map(|(_, share)| share).sum();
        if !self.look.is_empty() && (shares - 1.0).abs() > 1e-4 {
            problems.push(format!("the look shares come to {shares}, not 1"));
        }
        problems
    }

    /// Each region a bone is in, once, in bone order.
    pub fn bone_regions(&self) -> impl Iterator<Item = Regions> + '_ {
        self.regions
            .iter()
            .enumerate()
            .filter(|(index, region)| **region != Regions::NONE && !self.regions[..*index].contains(region))
            .map(|(_, region)| *region)
    }
}

/// Most feet a rig may plant, so a per-body list of them stays off the heap.
pub const MOST_FEET: usize = 8;

/// One value per foot, in [`Anatomy::feet`] order. Feet past [`MOST_FEET`] are dropped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerFoot<T> {
    values: [T; MOST_FEET],
    len: usize,
}

impl<T: Copy + Default> FromIterator<T> for PerFoot<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        let mut feet = PerFoot { values: [T::default(); MOST_FEET], len: 0 };
        for value in values.into_iter().take(MOST_FEET) {
            feet.values[feet.len] = value;
            feet.len += 1;
        }
        feet
    }
}

impl<T> Deref for PerFoot<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.values[..self.len]
    }
}
