//! A line a feature turns about or runs along: an edge of a body, a line in a
//! sketch, or one of the three axes through the origin. One type, so wherever
//! a feature wants a line any of the three can be picked, rather than a
//! dropdown of axes or of one sketch's lines.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::get;
use crate::prop::feature::{Axis, PropFeatureId};
use crate::prop::reference::GeomRef;
use crate::prop::sketch::{PlacedSketch, SketchId};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LineRef {
    Origin { axis: Axis },
    /// A named edge, resolved like a plane's references: as the prop stood
    /// when the feature naming it runs.
    Edge { edge: GeomRef },
    Sketch { sketch: PropFeatureId, line: SketchId },
}

impl LineRef {
    pub fn features(&self) -> Vec<PropFeatureId> {
        match self {
            LineRef::Origin { .. } => vec![],
            LineRef::Edge { edge } => edge.source.features().collect(),
            LineRef::Sketch { sketch, .. } => vec![*sketch],
        }
    }

    /// Two points on the line, `a` to `b` being its direction. An origin axis
    /// is the unit segment from the origin.
    pub fn resolve(
        &self,
        edge: impl FnOnce(&GeomRef) -> Option<(Vec3, Vec3)>,
        sketches: &[PlacedSketch],
    ) -> Option<(Vec3, Vec3)> {
        let (a, b) = match self {
            LineRef::Origin { axis } => (Vec3::ZERO, axis.normal()),
            LineRef::Edge { edge: reference } => edge(reference)?,
            LineRef::Sketch { sketch, line } => {
                let placed = sketches.iter().find(|placed| placed.id == *sketch)?;
                let (a, b) = placed.sketch.line(*line)?;
                (placed.world(a), placed.world(b))
            }
        };
        (a.distance(b) > DEGENERATE).then_some((a, b))
    }
}

/// Metres; shorter has no direction worth trusting.
const DEGENERATE: f32 = 1e-6;

pub fn axis_name(axis: Axis) -> String {
    get!("prop.lines.axis", "axis", axis.name())
}
