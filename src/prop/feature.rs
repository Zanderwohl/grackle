//! The feature list a prop is, and what evaluating one means.
//!
//! This is the same idea as the map's [`FeatureTimeline`] — an ordered list of
//! operations, replayed from the top, so an edit halfway down is felt by
//! everything after it — with one deliberate difference: **a modelling feature
//! is an enum, not a `typetag` trait object.**
//!
//! The map's features are a trait because a map is open-ended: a new kind of
//! thing to place is a new implementor and nothing else needs to know. A
//! boolean, though, has to *know what it is subtracting*. There is no way to
//! write "cut B out of A" against a trait object without the kernel answering
//! for every shape anyway, so the openness would be a costume. Closing the set
//! buys exhaustive matching, plain serde, and an evaluator that cannot be
//! handed something it has never heard of.
//!
//! **One rule runs the whole evaluator**: a feature that consumes bodies
//! produces one body, under its own id. A boolean consumes two and leaves one;
//! a mirror consumes one and leaves one; a primitive consumes none and leaves
//! one. What is left at the end is what gets drawn. Without the consuming
//! half, subtracting a bore from a barrel would draw the barrel, the bore and
//! the result all at once.
//!
//! Nothing here panics or refuses. A feature referring to a body that is not
//! there — because it was deleted, disabled, or already eaten by an earlier
//! boolean — is **skipped with a note**, and the note reaches the panel. A
//! modelling tool that stopped evaluating at the first dangling reference
//! would be a tool where deleting a feature blanks the viewport.
//!
//! [`FeatureTimeline`]: crate::editor::editable::FeatureTimeline

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::get;
use crate::prop::profile::{Placement, Profile};
use crate::prop::solid::{Shape, Solid};
use crate::prop::surface::Surface;

/// What a feature is called on disk and by the features that refer to it.
///
/// A number handed out by the document and never reused, rather than a
/// position in the list: features get reordered and deleted, and a reference
/// that meant "the third one" would silently come to mean something else.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct PropFeatureId(pub u32);

impl std::fmt::Display for PropFeatureId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Which way round a boolean goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOp {
    #[default]
    Union,
    Subtract,
    Intersect,
}

impl BooleanOp {
    pub const ALL: [BooleanOp; 3] = [BooleanOp::Union, BooleanOp::Subtract, BooleanOp::Intersect];

    pub fn name(self) -> String {
        match self {
            BooleanOp::Union => get!("prop.boolean.union"),
            BooleanOp::Subtract => get!("prop.boolean.subtract"),
            BooleanOp::Intersect => get!("prop.boolean.intersect"),
        }
    }
}

/// One of the three world axes, as the normal of a plane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    #[default]
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    pub fn normal(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Axis::X => "X",
            Axis::Y => "Y",
            Axis::Z => "Z",
        }
    }
}

/// What a feature does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "feature", rename_all = "snake_case")]
pub enum FeatureOp {
    /// A ready-made shape, placed.
    Primitive { shape: Shape, placement: Placement, surface: Surface },
    /// A profile pushed along its sketch's normal.
    Extrude {
        profile: Profile,
        placement: Placement,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        depth: f32,
        /// Half each way rather than all of it forwards.
        #[serde(default)]
        midplane: bool,
        surface: Surface,
    },
    /// A profile spun about its sketch's vertical.
    Revolve {
        profile: Profile,
        placement: Placement,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        degrees: f32,
        segments: u32,
        surface: Surface,
    },
    /// Two bodies in, one out.
    Boolean { op: BooleanOp, target: PropFeatureId, tool: PropFeatureId },
    /// A body reflected in an axis-aligned plane.
    ///
    /// The plane is an axis and an offset rather than a full [`Placement`],
    /// because every mirror anybody has ever wanted on a weapon is across the
    /// centreline, and a dropdown and a number is a thing you can get right
    /// without thinking about Euler order.
    Mirror {
        target: PropFeatureId,
        axis: Axis,
        #[serde(default, with = "crate::prop::nice_f32::scalar")]
        offset: f32,
        /// Keep what was there as well as the reflection — the usual case, and
        /// what makes this a symmetry feature rather than a flip.
        #[serde(default)]
        keep_original: bool,
    },
    /// Repaint a body. Its own feature rather than a field on every other one
    /// because the thing being repainted is usually the *result* of a boolean,
    /// which no earlier feature had a name for.
    Paint { target: PropFeatureId, surface: Surface },
}

impl Default for FeatureOp {
    fn default() -> Self {
        FeatureOp::Primitive {
            shape: Shape::default(),
            placement: Placement::default(),
            surface: Surface::default(),
        }
    }
}

impl FeatureOp {
    /// What this kind of feature is called, for the tree.
    pub fn type_name(&self) -> String {
        match self {
            FeatureOp::Primitive { shape, .. } => shape.name(),
            FeatureOp::Extrude { .. } => get!("prop.features.extrude"),
            FeatureOp::Revolve { .. } => get!("prop.features.revolve"),
            FeatureOp::Boolean { op, .. } => op.name(),
            FeatureOp::Mirror { .. } => get!("prop.features.mirror"),
            FeatureOp::Paint { .. } => get!("prop.features.paint"),
        }
    }

    /// The bodies this feature eats, in the order it names them.
    ///
    /// One place that answers the question, so the evaluator, the delete
    /// check and the panel's dropdowns cannot disagree about what depends on
    /// what.
    pub fn consumes(&self) -> Vec<PropFeatureId> {
        match self {
            FeatureOp::Primitive { .. }
            | FeatureOp::Extrude { .. }
            | FeatureOp::Revolve { .. } => vec![],
            FeatureOp::Boolean { target, tool, .. } => vec![*target, *tool],
            FeatureOp::Mirror { target, .. } | FeatureOp::Paint { target, .. } => vec![*target],
        }
    }

    /// The surface this feature paints with, if it paints at all — so the
    /// panel can offer a style and tint without matching on the variant.
    pub fn surface_mut(&mut self) -> Option<&mut Surface> {
        match self {
            FeatureOp::Primitive { surface, .. }
            | FeatureOp::Extrude { surface, .. }
            | FeatureOp::Revolve { surface, .. }
            | FeatureOp::Paint { surface, .. } => Some(surface),
            FeatureOp::Boolean { .. } | FeatureOp::Mirror { .. } => None,
        }
    }

    /// Where this feature's sketch or shape sits, if it has one.
    pub fn placement_mut(&mut self) -> Option<&mut Placement> {
        match self {
            FeatureOp::Primitive { placement, .. }
            | FeatureOp::Extrude { placement, .. }
            | FeatureOp::Revolve { placement, .. } => Some(placement),
            FeatureOp::Boolean { .. } | FeatureOp::Mirror { .. } | FeatureOp::Paint { .. } => None,
        }
    }
}

/// One entry in the list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropFeature {
    pub id: PropFeatureId,
    /// What the mapper called it. Free text and **not** a lang key: this is
    /// content somebody typed, like a map's name, rather than interface text.
    pub name: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// A named field rather than a `#[serde(flatten)]`, which would read
    /// better and does not survive TOML: flattening emits a struct's fields
    /// through a map, and TOML wants every scalar in a table before any
    /// sub-table, which a flattened enum carrying nested tables cannot
    /// promise.
    pub op: FeatureOp,
}

fn enabled_by_default() -> bool {
    true
}

impl PropFeature {
    pub fn new(id: PropFeatureId, op: FeatureOp) -> PropFeature {
        PropFeature { id, name: op.type_name(), enabled: true, op }
    }
}

/// Something the evaluator could not do, said in a way a mapper can act on.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub feature: PropFeatureId,
    pub message: String,
}

/// What a feature list came out as.
#[derive(Clone, Debug, Default)]
pub struct Evaluated {
    /// The bodies left standing, in the order they were made.
    pub bodies: Vec<(PropFeatureId, Solid)>,
    pub problems: Vec<Problem>,
    /// Which bodies were live just before each feature ran, indexed the same
    /// way the feature list is.
    ///
    /// Recorded by the evaluator rather than worked out again by the panel:
    /// "what can this boolean point at" is the consuming rule read backwards,
    /// and two copies of that rule is two copies that drift.
    pub live_before: Vec<Vec<PropFeatureId>>,
}

impl Evaluated {
    pub fn triangle_count(&self) -> usize {
        self.bodies.iter().map(|(_, solid)| solid.triangle_count()).sum()
    }

    /// The box everything sits in.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.bodies.iter().filter_map(|(_, solid)| solid.bounds()).reduce(
            |(min_a, max_a), (min_b, max_b)| (min_a.min(min_b), max_a.max(max_b)),
        )
    }
}

/// Replay a feature list into the bodies it describes.
///
/// Deterministic and free of the `World`: this is a pure function of the list,
/// which is what lets it be tested without a `World` and, later, run on a
/// client that was handed a prop rather than shown one.
pub fn evaluate(features: &[PropFeature]) -> Evaluated {
    let mut live: Vec<(PropFeatureId, Solid)> = Vec::new();
    let mut problems = Vec::new();
    let mut live_before = Vec::with_capacity(features.len());

    for feature in features {
        live_before.push(live.iter().map(|(id, _)| *id).collect());

        if !feature.enabled {
            continue;
        }

        // Finding an operand and taking it are deliberately two steps. A
        // feature that takes its first operand and then fails to find its
        // second has *deleted a body it did not use*, which reads as the
        // surviving half of a broken boolean vanishing from the viewport —
        // and the feature that broke is not the one that looks wrong.
        let find = |id: PropFeatureId, live: &[(PropFeatureId, Solid)]| {
            live.iter().position(|(candidate, _)| *candidate == id)
        };

        match &feature.op {
            FeatureOp::Primitive { shape, placement, surface } => {
                let solid = shape.solid(*surface).transformed(placement.transform());
                live.push((feature.id, solid));
            }
            FeatureOp::Extrude { profile, placement, depth, midplane, surface } => {
                let solid = Solid::extrude(profile, *depth, *midplane, *surface)
                    .transformed(placement.transform());
                if solid.is_empty() {
                    problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.empty_sweep"),
                    });
                    continue;
                }
                live.push((feature.id, solid));
            }
            FeatureOp::Revolve { profile, placement, degrees, segments, surface } => {
                let solid = Solid::revolve(profile, *degrees, *segments, *surface)
                    .transformed(placement.transform());
                if solid.is_empty() {
                    problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.empty_sweep"),
                    });
                    continue;
                }
                live.push((feature.id, solid));
            }
            FeatureOp::Boolean { op, target, tool } => {
                // Refusing the self-reference explicitly: taking the same body
                // twice would give the second `take` nothing and read as a
                // missing body, which is a confusing way to say "you pointed
                // this at itself".
                if target == tool {
                    problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.self_boolean"),
                    });
                    continue;
                }
                let (Some(at_target), Some(at_tool)) = (find(*target, &live), find(*tool, &live))
                else {
                    problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.missing_body"),
                    });
                    continue;
                };
                // Higher index first: removing the earlier one would shift the
                // later one out from under its own index.
                let (first, second) = (at_target.max(at_tool), at_target.min(at_tool));
                let (a, b) = if at_target < at_tool {
                    let b = live.remove(first).1;
                    let a = live.remove(second).1;
                    (a, b)
                } else {
                    let a = live.remove(first).1;
                    let b = live.remove(second).1;
                    (a, b)
                };
                let result = match op {
                    BooleanOp::Union => a.union(&b),
                    BooleanOp::Subtract => a.subtract(&b),
                    BooleanOp::Intersect => a.intersect(&b),
                };
                if result.is_empty() {
                    problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.nothing_left"),
                    });
                    continue;
                }
                live.push((feature.id, result));
            }
            FeatureOp::Mirror { target, axis, offset, keep_original } => {
                let Some(solid) = find(*target, &live).map(|at| live.remove(at).1) else {
                    problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.missing_body"),
                    });
                    continue;
                };
                let origin = axis.normal() * *offset;
                let reflected = solid.clone().mirrored(origin, axis.normal());
                let result = if *keep_original { solid.union(&reflected) } else { reflected };
                live.push((feature.id, result));
            }
            FeatureOp::Paint { target, surface } => {
                let Some(solid) = find(*target, &live).map(|at| live.remove(at).1) else {
                    problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.missing_body"),
                    });
                    continue;
                };
                live.push((feature.id, solid.painted(*surface)));
            }
        }
    }

    Evaluated { bodies: live, problems, live_before }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn primitive(id: u32, shape: Shape, at: Vec3) -> PropFeature {
        PropFeature::new(
            PropFeatureId(id),
            FeatureOp::Primitive {
                shape,
                placement: Placement::at(at.x, at.y, at.z),
                surface: Surface::default(),
            },
        )
    }

    fn unit_box(id: u32, at: Vec3) -> PropFeature {
        primitive(id, Shape::Box { size: [1.0, 1.0, 1.0] }, at)
    }

    /// The rule the evaluator is built on. A boolean eats both its operands,
    /// so what is left to draw is the result and not the ingredients — the
    /// failure being a barrel with its own bore drawn inside it, visible only
    /// where the two disagree.
    #[test]
    fn a_boolean_consumes_both_of_its_operands() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            unit_box(1, Vec3::new(0.5, 0.0, 0.0)),
            PropFeature::new(
                PropFeatureId(2),
                FeatureOp::Boolean {
                    op: BooleanOp::Subtract,
                    target: PropFeatureId(0),
                    tool: PropFeatureId(1),
                },
            ),
        ];
        let result = evaluate(&features);
        assert!(result.problems.is_empty(), "{:?}", result.problems);
        assert_eq!(result.bodies.len(), 1, "the operands were left lying around");
        assert_eq!(result.bodies[0].0, PropFeatureId(2), "the result took the wrong id");
        assert!((result.bodies[0].1.volume() - 0.5).abs() < 1e-3);
    }

    /// Deleting a feature something else pointed at must not blank the
    /// viewport. Everything that still makes sense is still built, and the one
    /// broken feature says so.
    #[test]
    fn a_dangling_reference_is_a_note_rather_than_a_blank_prop() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            PropFeature::new(
                PropFeatureId(2),
                FeatureOp::Boolean {
                    op: BooleanOp::Subtract,
                    target: PropFeatureId(0),
                    // Never existed: the tool was deleted.
                    tool: PropFeatureId(9),
                },
            ),
        ];
        let result = evaluate(&features);
        assert_eq!(result.problems.len(), 1);
        assert_eq!(result.problems[0].feature, PropFeatureId(2));
        assert_eq!(result.bodies.len(), 1, "the surviving body went missing too");
        assert_eq!(result.bodies[0].0, PropFeatureId(0), "the target was eaten by a failed boolean");
    }

    /// A feature that fails must put back whatever it took. Left out, a
    /// self-referencing boolean would delete the body it named as well as
    /// doing nothing.
    #[test]
    fn a_boolean_pointed_at_itself_leaves_the_body_alone() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            PropFeature::new(
                PropFeatureId(1),
                FeatureOp::Boolean {
                    op: BooleanOp::Union,
                    target: PropFeatureId(0),
                    tool: PropFeatureId(0),
                },
            ),
        ];
        let result = evaluate(&features);
        assert_eq!(result.problems.len(), 1);
        assert_eq!(result.bodies.len(), 1);
        assert_eq!(result.bodies[0].0, PropFeatureId(0));
    }

    /// Turning a feature off has to take its dependants with it rather than
    /// leaving them building against a body that is not there. It reads as a
    /// problem, which is the honest answer: they are broken *because* of the
    /// toggle, and turning it back on fixes them.
    #[test]
    fn disabling_a_feature_removes_its_body() {
        let mut features = vec![unit_box(0, Vec3::ZERO), unit_box(1, Vec3::new(3.0, 0.0, 0.0))];
        features[0].enabled = false;
        let result = evaluate(&features);
        assert_eq!(result.bodies.len(), 1);
        assert_eq!(result.bodies[0].0, PropFeatureId(1));
    }

    /// Mirroring with the original kept is the symmetry feature: one body out,
    /// twice the volume, and no seam down the middle to draw.
    #[test]
    fn a_kept_mirror_doubles_the_body() {
        let features = vec![
            unit_box(0, Vec3::new(1.0, 0.0, 0.0)),
            PropFeature::new(
                PropFeatureId(1),
                FeatureOp::Mirror {
                    target: PropFeatureId(0),
                    axis: Axis::X,
                    offset: 0.0,
                    keep_original: true,
                },
            ),
        ];
        let result = evaluate(&features);
        assert!(result.problems.is_empty(), "{:?}", result.problems);
        assert_eq!(result.bodies.len(), 1);
        assert!((result.bodies[0].1.volume() - 2.0).abs() < 1e-3);
    }

    /// What the panel offers a boolean has to be exactly what the evaluator
    /// will find. Recorded by the evaluator itself for that reason — a second
    /// implementation of the consuming rule would offer bodies that are
    /// already eaten.
    #[test]
    fn the_live_set_is_recorded_as_each_feature_sees_it() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            unit_box(1, Vec3::new(0.5, 0.0, 0.0)),
            PropFeature::new(
                PropFeatureId(2),
                FeatureOp::Boolean {
                    op: BooleanOp::Subtract,
                    target: PropFeatureId(0),
                    tool: PropFeatureId(1),
                },
            ),
            unit_box(3, Vec3::new(9.0, 0.0, 0.0)),
        ];
        let result = evaluate(&features);
        assert_eq!(result.live_before[0], vec![]);
        assert_eq!(result.live_before[1], vec![PropFeatureId(0)]);
        assert_eq!(result.live_before[2], vec![PropFeatureId(0), PropFeatureId(1)]);
        assert_eq!(result.live_before[3], vec![PropFeatureId(2)]);
    }
}
