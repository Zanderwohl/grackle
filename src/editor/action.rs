use bevy::prelude::Vec3;
use serde::{Deserialize, Serialize};

use crate::editor::editable::{Feature, FeatureId, FeatureTrait, PointRef};
use crate::editor::editor_room::EditorRoom;
use crate::editor::global_point::GlobalPoint;
use crate::editor::grackle_point_light::GracklePointLight;
use crate::common::cuboid::GrackleCuboid;

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum FeatureData {
    GlobalPoint {
        location: PointRef,
    },
    PointLight {
        location: PointRef,
        intensity: f32,
        radius: f32,
        range: f32,
    },
    Room {
        min: PointRef,
        max: PointRef,
    },
    Cuboid {
        min: Vec3,
        max: Vec3,
    },
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct FeatureSnapshot {
    pub data: FeatureData,
    pub parents: Vec<FeatureId>,
    pub order_index: usize,
}

impl FeatureSnapshot {
    pub fn from_feature(feature: &Feature, order_index: usize) -> Self {
        Self {
            data: feature.object().snapshot(),
            parents: feature.parents().to_vec(),
            order_index,
        }
    }

    pub fn blank_object(&self) -> Box<dyn FeatureTrait> {
        let mut object: Box<dyn FeatureTrait> = match &self.data {
            FeatureData::GlobalPoint { .. } => Box::new(GlobalPoint::from_point_ref(PointRef::absolute(0.0, 0.0, 0.0))),
            FeatureData::PointLight { .. } => Box::new(GracklePointLight::from_point_ref(PointRef::absolute(0.0, 0.0, 0.0))),
            FeatureData::Room { .. } => Box::new(EditorRoom::from_point_refs(
                PointRef::absolute(0.0, 0.0, 0.0),
                PointRef::absolute(0.0, 0.0, 0.0),
            )),
            FeatureData::Cuboid { .. } => Box::new(GrackleCuboid::new(Vec3::ZERO, Vec3::ZERO)),
        };

        object.apply_snapshot(&self.data);
        object
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FeatureDelta {
    pub feature_id: FeatureId,
    pub before: Option<FeatureSnapshot>,
    pub after: Option<FeatureSnapshot>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Action {
    pub deltas: Vec<FeatureDelta>,
}

fn feature_data_kind(data: &FeatureData) -> &'static str {
    match data {
        FeatureData::GlobalPoint { .. } => "Global Point",
        FeatureData::PointLight { .. } => "Point Light",
        FeatureData::Room { .. } => "Room",
        FeatureData::Cuboid { .. } => "Cuboid",
    }
}

impl FeatureDelta {
    /// Short description for the History panel (no live feature lookup).
    pub fn label(&self) -> String {
        match (&self.before, &self.after) {
            (None, Some(a)) => format!(
                "Create {} #{}",
                feature_data_kind(&a.data),
                self.feature_id
            ),
            (Some(b), None) => format!(
                "Delete {} #{}",
                feature_data_kind(&b.data),
                self.feature_id
            ),
            (Some(_), Some(a)) => format!(
                "Modify {} #{}",
                feature_data_kind(&a.data),
                self.feature_id
            ),
            (None, None) => "(empty action)".to_owned(),
        }
    }
}

impl Action {
    pub fn label(&self) -> String {
        if self.deltas.len() == 1 {
            self.deltas[0].label()
        } else {
            format!("{} changes", self.deltas.len())
        }
    }

    /// If `next` is a single-delta edit to the same feature and its `before` snapshot matches
    /// this action's `after` (including create-then-modify: create `after` == modify `before`),
    /// folds `next` into `self` by updating `self`'s `after` only. Returns `true` when merged.
    ///
    /// Does not merge create-then-delete into a single delta (would be ambiguous for undo).
    pub fn try_coalesce_incoming(&mut self, next: &Action) -> bool {
        if self.deltas.len() != 1 || next.deltas.len() != 1 {
            return false;
        }
        let a = &mut self.deltas[0];
        let b = &next.deltas[0];
        if a.feature_id != b.feature_id {
            return false;
        }
        let Some(before_b) = &b.before else {
            return false;
        };
        let Some(after_a) = &a.after else {
            return false;
        };
        if after_a != before_b {
            return false;
        }
        if a.before.is_none() && b.after.is_none() {
            return false;
        }
        a.after = b.after.clone();
        true
    }
}

