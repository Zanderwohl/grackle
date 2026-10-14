//! The feature list a prop is, and what evaluating one means.
//!
//! The same idea as the map's [`FeatureTimeline`] — an ordered list replayed
//! from the top — with one deliberate difference: **a modelling feature is an
//! enum, not a `typetag` trait object.** A boolean has to know what it is
//! subtracting, so there is no writing "cut B out of A" against a trait object
//! without the kernel answering for every shape anyway; the openness would be a
//! costume. Closing the set buys exhaustive matching and plain serde.
//!
//! **One rule runs the evaluator**: a feature that consumes bodies produces one
//! under its own id. What is left at the end is what gets drawn. Without the
//! consuming half, subtracting a bore from a barrel would draw the barrel, the
//! bore *and* the result.
//!
//! Nothing panics or refuses. A feature naming a body that is not there —
//! deleted, disabled, or already eaten — is **skipped with a note** that
//! reaches the panel; stopping at the first dangling reference would blank the
//! viewport every time somebody deleted a feature.
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
/// Handed out by the document and never reused: features get reordered and
/// deleted, and a reference meaning "the third one" would come to mean
/// something else.
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
    /// A body reflected in an axis-aligned plane. An axis and an offset rather
    /// than a full [`Placement`]: every mirror on a weapon is across the
    /// centreline, and a dropdown is a thing you get right without thinking
    /// about Euler order.
    Mirror {
        target: PropFeatureId,
        axis: Axis,
        #[serde(default, with = "crate::prop::nice_f32::scalar")]
        offset: f32,
        /// Keep the original too, which makes this symmetry rather than a flip.
        #[serde(default)]
        keep_original: bool,
    },
    /// Repaint a body. Its own feature because what gets repainted is usually
    /// the *result* of a boolean, which no earlier feature named.
    Paint { target: PropFeatureId, surface: Surface },
    /// Copies of a body, in a row or around an axis, joined into one — vents
    /// along a shroud, flutes round a cylinder, rungs on a rail. The original
    /// is the first copy, so a count of one changes nothing.
    Pattern {
        target: PropFeatureId,
        count: u32,
        along: PatternKind,
    },
}

/// How a pattern's copies are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PatternKind {
    /// Each copy `step` further on than the last.
    Linear {
        #[serde(with = "crate::prop::nice_f32::array")]
        step: [f32; 3],
    },
    /// Turned about a line through `centre` parallel to `axis`, spread across
    /// `degrees`. A whole turn spaces them evenly round it; anything less puts
    /// the first copy at one end and the last at the other.
    Radial {
        axis: Axis,
        #[serde(default, with = "crate::prop::nice_f32::array")]
        centre: [f32; 3],
        #[serde(with = "crate::prop::nice_f32::scalar")]
        degrees: f32,
    },
}

impl PatternKind {
    /// Where copy `index` of `count` goes, as a transform of the original.
    pub fn copy(&self, index: u32, count: u32) -> Transform {
        match *self {
            PatternKind::Linear { step } => {
                Transform::from_translation(Vec3::from_array(step) * index as f32)
            }
            PatternKind::Radial { axis, centre, degrees } => {
                // A whole turn would put the last copy on top of the first.
                let gaps = match degrees.abs() >= 360.0 - 1e-3 {
                    true => count,
                    false => count.saturating_sub(1).max(1),
                };
                let angle = (degrees / gaps as f32).to_radians() * index as f32;
                let centre = Vec3::from_array(centre);
                let turn = Quat::from_axis_angle(axis.normal(), angle);
                Transform::from_translation(centre - turn * centre).with_rotation(turn)
            }
        }
    }
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
            FeatureOp::Pattern { .. } => get!("prop.features.pattern"),
        }
    }

    /// The bodies this feature eats. One place answers it, so the evaluator,
    /// the delete check and the panel's dropdowns cannot disagree.
    pub fn consumes(&self) -> Vec<PropFeatureId> {
        match self {
            FeatureOp::Primitive { .. }
            | FeatureOp::Extrude { .. }
            | FeatureOp::Revolve { .. } => vec![],
            FeatureOp::Boolean { target, tool, .. } => vec![*target, *tool],
            FeatureOp::Mirror { target, .. }
            | FeatureOp::Paint { target, .. }
            | FeatureOp::Pattern { target, .. } => vec![*target],
        }
    }

    /// Where this feature's sketch or shape sits, if it has one.
    pub fn placement_mut(&mut self) -> Option<&mut Placement> {
        match self {
            FeatureOp::Primitive { placement, .. }
            | FeatureOp::Extrude { placement, .. }
            | FeatureOp::Revolve { placement, .. } => Some(placement),
            FeatureOp::Boolean { .. }
            | FeatureOp::Mirror { .. }
            | FeatureOp::Paint { .. }
            | FeatureOp::Pattern { .. } => None,
        }
    }

    /// The local axis this feature's solid runs *along*, which is what
    /// "point it along +Z" turns.
    ///
    /// Read off how each solid is built rather than chosen: a prism and a cone
    /// stand on `+Y`, an extrude runs down `+Z`, a revolve spins about `+Y`. A
    /// box, wedge or sphere has no length of its own, so it takes `+Z`, which
    /// is where a new box's longest side already is.
    pub fn main_axis(&self) -> Vec3 {
        match self {
            FeatureOp::Primitive { shape: Shape::Prism { .. } | Shape::Cone { .. }, .. } => Vec3::Y,
            FeatureOp::Revolve { .. } => Vec3::Y,
            _ => Vec3::Z,
        }
    }
}

/// One entry in the list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropFeature {
    pub id: PropFeatureId,
    /// Free text and **not** a lang key: content somebody typed, like a map's
    /// name, rather than interface text.
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
    /// Which bodies were live just before each feature ran.
    ///
    /// Recorded here rather than worked out again by the panel: "what can this
    /// boolean point at" is the consuming rule read backwards, and two copies
    /// of it drift.
    pub live_before: Vec<Vec<PropFeatureId>>,
}

impl Evaluated {
    pub fn triangle_count(&self) -> usize {
        self.bodies.iter().map(|(_, solid)| solid.triangle_count()).sum()
    }

    /// The box everything sits in.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.bounds_of(|_| true)
    }

    /// The box the standing bodies among `ids` sit in. A feature already eaten
    /// has no body of its own, so it contributes nothing.
    pub fn bounds_of_bodies(&self, ids: &[PropFeatureId]) -> Option<(Vec3, Vec3)> {
        self.bounds_of(|id| ids.contains(&id))
    }

    fn bounds_of(&self, wanted: impl Fn(PropFeatureId) -> bool) -> Option<(Vec3, Vec3)> {
        self.bodies
            .iter()
            .filter(|(id, _)| wanted(*id))
            .filter_map(|(_, solid)| solid.bounds())
            .reduce(|(min_a, max_a), (min_b, max_b)| (min_a.min(min_b), max_a.max(max_b)))
    }

    /// The standing body a ray meets first, if any.
    ///
    /// Against the faces rather than the bounding boxes: a bore runs through a
    /// barrel, and a click down it should reach whatever is behind rather than
    /// select the barrel for having a box around the hole. Faces are convex,
    /// so a fan is an exact triangulation of each.
    pub fn body_under(&self, origin: Vec3, direction: Vec3) -> Option<PropFeatureId> {
        self.bodies
            .iter()
            .filter_map(|(id, solid)| {
                solid
                    .polygons()
                    .iter()
                    .flat_map(|polygon| {
                        let v = &polygon.vertices;
                        (1..v.len().saturating_sub(1))
                            .filter_map(move |i| ray_triangle(origin, direction, v[0], v[i], v[i + 1]))
                    })
                    .min_by(f32::total_cmp)
                    .map(|distance| (*id, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }
}

/// How far along a ray it meets a triangle, from either side — Möller–Trumbore.
/// Both sides, because an inside-out face is still something somebody can see
/// and click.
fn ray_triangle(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let ab = b - a;
    let ac = c - a;
    let p = direction.cross(ac);
    let det = ab.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let t_vec = origin - a;
    let u = t_vec.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = t_vec.cross(ab);
    let v = direction.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = ac.dot(q) * inv;
    (t > 0.0).then_some(t)
}

/// Replay a feature list into the bodies it describes. A pure function of the
/// list, so it can be tested without a `World` and later run on a client that
/// was handed a prop rather than shown one.
pub fn evaluate(features: &[PropFeature]) -> Evaluated {
    let mut live: Vec<(PropFeatureId, Solid)> = Vec::new();
    let mut problems = Vec::new();
    let mut live_before = Vec::with_capacity(features.len());

    for feature in features {
        live_before.push(live.iter().map(|(id, _)| *id).collect());

        if !feature.enabled {
            continue;
        }

        // Finding an operand and taking it are two steps on purpose: one that
        // took its first and then failed to find its second would delete a body
        // it never used, and the half that vanishes is not the broken feature.
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
                // Refused explicitly: taking the same body twice would read as
                // a missing one, which is a confusing way to say this.
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
                // Higher index first, or removing one shifts the other.
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
            FeatureOp::Pattern { target, count, along } => {
                let Some(solid) = find(*target, &live).map(|at| live.remove(at).1) else {
                    problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.missing_body"),
                    });
                    continue;
                };
                let count = (*count).max(1);
                // Unioned rather than concatenated: copies close enough to
                // touch have to become one closed surface, or the inside of
                // the overlap is drawn and a later boolean keeps the wrong half.
                let result = (1..count).fold(solid.clone(), |joined, index| {
                    joined.union(&solid.clone().transformed(along.copy(index, count)))
                });
                live.push((feature.id, result));
            }
        }
    }

    Evaluated { bodies: live, problems, live_before }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row of three ten-centimetre cubes twenty centimetres apart is three
    /// times the volume of one; four round a whole turn are four, none on top
    /// of another — and a count of one is the body untouched.
    #[test]
    fn a_pattern_makes_that_many_separate_copies() {
        let cube = primitive(0, Shape::Box { size: [0.1, 0.1, 0.1] }, Vec3::new(0.3, 0.0, 0.0));
        let one = evaluate(std::slice::from_ref(&cube)).bodies[0].1.volume();
        let pattern = |count: u32, along: PatternKind| {
            let evaluated = evaluate(&[
                cube.clone(),
                PropFeature::new(PropFeatureId(1), FeatureOp::Pattern { target: PropFeatureId(0), count, along }),
            ]);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            assert_eq!(evaluated.bodies.len(), 1, "the copies are one body");
            evaluated.bodies[0].1.volume()
        };

        let row = pattern(3, PatternKind::Linear { step: [0.0, 0.0, 0.2] });
        assert!((row - 3.0 * one).abs() < 1e-6, "{row} vs {}", 3.0 * one);
        let round = pattern(4, PatternKind::Radial { axis: Axis::Y, centre: [0.0; 3], degrees: 360.0 });
        assert!((round - 4.0 * one).abs() < 1e-6, "{round} vs {}", 4.0 * one);
        let alone = pattern(1, PatternKind::Linear { step: [0.0, 0.0, 0.2] });
        assert!((alone - one).abs() < 1e-9);
    }

    /// A click lands on the nearer of two bodies in a line, and passes
    /// straight down a hole drilled through the first to reach the second.
    #[test]
    fn a_ray_picks_the_nearest_face_not_the_nearest_box() {
        let cube = |id: u32, z: f32| primitive(id, Shape::Box { size: [0.1, 0.1, 0.1] }, Vec3::new(0.0, 0.0, z));
        let near = cube(0, 0.0);
        let far = cube(1, -0.5);
        let evaluated = evaluate(&[near.clone(), far.clone()]);
        let from_front = evaluated.body_under(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z);
        assert_eq!(from_front, Some(PropFeatureId(0)));
        let beside = evaluated.body_under(Vec3::new(1.0, 0.0, 1.0), Vec3::NEG_Z);
        assert_eq!(beside, None);

        // A bore down the middle of the near cube, subtracted.
        let bore = primitive(2, Shape::Box { size: [0.04, 0.04, 0.3] }, Vec3::ZERO);
        let drilled = PropFeature::new(
            PropFeatureId(3),
            FeatureOp::Boolean { op: BooleanOp::Subtract, target: PropFeatureId(0), tool: PropFeatureId(2) },
        );
        let evaluated = evaluate(&[near, far, bore, drilled]);
        assert_eq!(
            evaluated.body_under(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z),
            Some(PropFeatureId(1)),
            "the ray goes down the hole",
        );
        assert_eq!(
            evaluated.body_under(Vec3::new(0.04, 0.0, 1.0), Vec3::NEG_Z),
            Some(PropFeatureId(3)),
            "beside the hole is the drilled body, under its own id",
        );
    }

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

    /// The rule the evaluator is built on: what is left to draw is the result,
    /// not the ingredients. The failure is a barrel with its own bore drawn
    /// inside it, visible only where the two disagree.
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

    /// Deleting something another feature pointed at must not blank the
    /// viewport: everything that still makes sense is still built.
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

    /// A feature that fails must put back whatever it took.
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

    /// Turning a feature off takes its body with it; dependants then read as
    /// problems, which is honest — turning it back on fixes them.
    #[test]
    fn disabling_a_feature_removes_its_body() {
        let mut features = vec![unit_box(0, Vec3::ZERO), unit_box(1, Vec3::new(3.0, 0.0, 0.0))];
        features[0].enabled = false;
        let result = evaluate(&features);
        assert_eq!(result.bodies.len(), 1);
        assert_eq!(result.bodies[0].0, PropFeatureId(1));
    }

    /// One body out, twice the volume, and no seam down the middle.
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

    /// What the panel offers has to be what the evaluator will find, which is
    /// why the evaluator records it: a second implementation of the consuming
    /// rule would offer bodies that are already eaten.
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
