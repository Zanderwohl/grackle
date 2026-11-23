//! The feature list a prop is, and how it evaluates. Like the map's
//! [`FeatureTimeline`], but a feature is an enum rather than a `typetag` trait
//! object, because a boolean has to know what it is subtracting. See CLAUDE.md,
//! "Modeling a prop".
//!
//! A feature that consumes bodies produces one under its own id; what is left
//! at the end is drawn. A feature naming a body that is not there is skipped
//! with a note rather than stopping evaluation, which would blank the viewport
//! whenever a feature was deleted.
//!
//! [`FeatureTimeline`]: crate::editor::editable::FeatureTimeline

use bevy::math::Affine3A;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::get;
use crate::prop::profile::Placement;
use crate::prop::solid::Polygon;
use crate::prop::datum::{AxisDef, DatumAxis, DatumPoint, PointDef};
use crate::prop::plane::{Datum, OriginPlane, PlaneDef, PlaneRef, MIN_ORIGIN_HALF};
use crate::prop::plane::Plane;
use crate::prop::loft;
use crate::prop::refs::{AnyRef, CurveRef, DirectionRef, PathRef, PointRef, Resolve, Scope};
use crate::prop::slot::{self, Field, Resolved};
use crate::prop::profile::revolve_steps;
use crate::prop::region::{arrange, axis_clear, Region};
use crate::prop::sketch::SketchId;
use crate::prop::sketch::{PlacedSketch, Sketch};
use crate::prop::seam::{self, FaceTable, Junction, Seam};
use crate::prop::reference::{pickables, GeomRef, Located, Outline, OutlineFace, Pickable, Wire};
use crate::prop::path::{self, Orient, PathPoints};
use crate::prop::skin::{pushed, revolution, skin, tapers_within, Run, Section, SkinProblem, Swept, TaggedLoop};
use crate::prop::face_profile::face_profile;
use crate::prop::solid::NO_FACE;
use crate::prop::solid::{is_full_turn, Shape, Solid};
use crate::prop::surface::Surface;

/// Never reused, so a reference survives reordering and deletion.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct PropFeatureId(pub u32);

impl std::fmt::Display for PropFeatureId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// One of the bodies a feature made. `of` is how many it made when this was
/// named: a different count means they were renumbered, so the reference names
/// nothing rather than the wrong body. Written as the bare feature id for
/// `index: 0, of: 1`, so older files read unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BodyRef {
    pub feature: PropFeatureId,
    pub index: u32,
    pub of: u32,
}

impl BodyRef {
    /// The one body of a feature that makes one.
    pub fn whole(feature: PropFeatureId) -> Self {
        BodyRef { feature, index: 0, of: 1 }
    }
}

/// For logs; the editor names a body through `prop.inspector.body_of`.
impl std::fmt::Display for BodyRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.of {
            1 => write!(f, "{}", self.feature),
            of => write!(f, "{} ({}/{})", self.feature, self.index + 1, of),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum BodyRefForm {
    Whole(PropFeatureId),
    Numbered { feature: PropFeatureId, index: u32, of: u32 },
}

impl Serialize for BodyRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            BodyRef { feature, index: 0, of: 1 } => BodyRefForm::Whole(feature),
            BodyRef { feature, index, of } => BodyRefForm::Numbered { feature, index, of },
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BodyRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let form = BodyRefForm::deserialize(deserializer).map_err(|_| {
            serde::de::Error::custom("a body is a feature id, or { feature, index, of }")
        })?;
        Ok(match form {
            BodyRefForm::Whole(feature) => BodyRef::whole(feature),
            BodyRefForm::Numbered { feature, index, of } if index < of => BodyRef { feature, index, of },
            BodyRefForm::Numbered { feature, index, of } => {
                return Err(serde::de::Error::custom(format!("body {index} of {of} of feature {feature} is past the last")));
            }
        })
    }
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "feature", rename_all = "snake_case")]
pub enum FeatureOp {
    Primitive { shape: Shape, placement: Placement, surface: Surface },
    /// A profile pushed along a direction, its plane's normal unless one is
    /// named, joined into one body. Scalars first: TOML needs every scalar
    /// before any sub-table.
    Extrude {
        /// Degrees each wall leans in, going away from the sketch's plane.
        #[serde(default, with = "crate::prop::nice_f32::scalar")]
        taper: f32,
        #[serde(default)]
        op: SweepOp,
        /// The bodies `op` joins, cuts or intersects.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyRef>,
        /// Along the direction.
        end: End,
        /// Back against the direction; none is one side only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        other: Option<End>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<DirectionRef>,
        profile: ProfileSource,
        surface: Surface,
    },
    /// A profile turned about a line lying in its plane, in straight steps.
    /// Scalars first, as for [`FeatureOp::Extrude`].
    Revolve {
        #[serde(with = "crate::prop::nice_f32::scalar")]
        degrees: f32,
        /// Clamped by [`revolve_steps`].
        steps: u32,
        /// Half each way of the sketch's plane.
        #[serde(default)]
        symmetric: bool,
        #[serde(default)]
        op: SweepOp,
        /// As for [`FeatureOp::Extrude`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyRef>,
        axis: CurveRef,
        profile: ProfileSource,
        surface: Surface,
    },
    /// A profile carried along a path, which starts on the profile's plane.
    /// Scalars first, as for [`FeatureOp::Extrude`].
    Sweep {
        #[serde(default)]
        orient: Orient,
        /// Degrees about the path over its whole length.
        #[serde(default, with = "crate::prop::nice_f32::scalar")]
        twist: f32,
        /// The profile's size at the end against its size at the start.
        #[serde(default = "unit", with = "crate::prop::nice_f32::scalar")]
        scale: f32,
        #[serde(default)]
        op: SweepOp,
        /// As for [`FeatureOp::Extrude`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyRef>,
        path: PathRef,
        profile: ProfileSource,
        surface: Surface,
    },
    /// A region of one sketch joined to a region of the next, in order,
    /// through straight faces.
    Loft {
        /// The last profile joins the first, with no caps.
        #[serde(default)]
        closed: bool,
        #[serde(default)]
        op: SweepOp,
        /// As for [`FeatureOp::Extrude`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyRef>,
        profiles: Vec<Profile>,
        surface: Surface,
    },
    /// Every body joined into one.
    Union { bodies: Vec<BodyRef> },
    /// Every tool taken out of each target: one body per target.
    Subtract { targets: Vec<BodyRef>, tools: Vec<BodyRef> },
    /// What every body shares.
    Intersect { bodies: Vec<BodyRef> },
    /// Each target about a plane, as it stood when this runs: one body per
    /// target.
    Mirror {
        /// Meters along the plane's normal.
        #[serde(default, with = "crate::prop::nice_f32::scalar")]
        offset: f32,
        /// Union with the original rather than replacing it.
        #[serde(default)]
        keep_original: bool,
        targets: Vec<BodyRef>,
        about: PlaneRef,
    },
    /// Its own feature because what gets repainted is usually a boolean's
    /// result, which no earlier feature has a surface for.
    Paint { targets: Vec<BodyRef>, surface: Surface },
    /// Copies of each target joined into one body per target. The original
    /// is the first copy.
    Pattern {
        count: u32,
        targets: Vec<BodyRef>,
        along: PatternKind,
    },
    /// Not geometry: something to sketch on and measure from, made from what
    /// stood when this runs.
    Plane { def: PlaneDef },
    /// Not geometry: something to turn about or run along.
    Axis { def: AxisDef },
    /// Not geometry: something to pass through.
    Point { def: PointDef },
    /// A drawing on a plane, which makes no body; see [`crate::prop::sketch`].
    Sketch { on: PlaneRef, sketch: Sketch },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum ProfileSource {
    /// Entries in the sketch's own region list.
    Regions { sketch: PropFeatureId, regions: Vec<SketchId> },
    /// Read off the body as it stands when the feature runs.
    Face { face: GeomRef },
}

impl ProfileSource {
    pub fn sketch(&self) -> Option<PropFeatureId> {
        match self {
            ProfileSource::Regions { sketch, .. } => Some(*sketch),
            ProfileSource::Face { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub sketch: PropFeatureId,
    /// An entry in the sketch's own region list.
    pub region: SketchId,
    /// Where the outside loop starts; otherwise it starts lying nearest the
    /// profile before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<PointRef>,
}

/// What a sweep does with the body it makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepOp {
    #[default]
    NewBody,
    Join,
    /// Taken out of the target.
    Cut,
    /// Only what the target and the new body share is kept.
    Intersect,
}

impl SweepOp {
    pub const ALL: [SweepOp; 4] = [SweepOp::NewBody, SweepOp::Join, SweepOp::Cut, SweepOp::Intersect];

    pub fn name(self) -> String {
        match self {
            SweepOp::NewBody => get!("prop.sweep_op.new_body"),
            SweepOp::Join => get!("prop.sweep_op.join"),
            SweepOp::Cut => get!("prop.sweep_op.cut"),
            SweepOp::Intersect => get!("prop.sweep_op.intersect"),
        }
    }
}

/// How far an extrude goes on one side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum End {
    /// Meters along the direction; negative goes the other way.
    Distance {
        #[serde(with = "crate::prop::nice_f32::scalar")]
        distance: f32,
    },
    /// Until the profile's own origin meets a point's level, a plane or a
    /// flat face. The end stays parallel to the sketch.
    UpTo { to: AnyRef },
    /// Past every body standing when it runs.
    ThroughAll,
}

impl End {
    pub const DEFAULT_DISTANCE: f32 = 0.05;

    pub fn bound(&self) -> Option<&AnyRef> {
        match self {
            End::UpTo { to } => Some(to),
            End::Distance { .. } | End::ThroughAll => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PatternKind {
    /// Each copy `spacing` meters further along `direction` than the last;
    /// negative goes back down it.
    Linear {
        #[serde(with = "crate::prop::nice_f32::scalar")]
        spacing: f32,
        direction: DirectionRef,
    },
    /// About `axis`. A whole turn spaces copies evenly; less puts the first
    /// and last at the ends of `degrees`.
    Radial {
        #[serde(with = "crate::prop::nice_f32::scalar")]
        degrees: f32,
        axis: CurveRef,
    },
}

impl PatternKind {
    /// A point on the line copies are made along or about, and its direction.
    pub fn resolve(&self, scope: &Scope) -> Option<(Vec3, Dir3)> {
        match self {
            PatternKind::Linear { direction, .. } => Some((Vec3::ZERO, direction.resolve(scope)?)),
            PatternKind::Radial { axis, .. } => {
                let (a, b) = axis.resolve(scope)?.line()?;
                Some((a, Dir3::new(b - a).ok()?))
            }
        }
    }

    /// Copy `index` of `count`, along or about [`Self::resolve`]'s line.
    pub fn copy(&self, (a, along): (Vec3, Dir3), index: u32, count: u32) -> Transform {
        match *self {
            PatternKind::Linear { spacing, .. } => Transform::from_translation(*along * spacing * index as f32),
            PatternKind::Radial { degrees, .. } => {
                // A whole turn would put the last copy on top of the first.
                let gaps = match is_full_turn(degrees) {
                    true => count,
                    false => count.saturating_sub(1).max(1),
                };
                let angle = (degrees / gaps as f32).to_radians() * index as f32;
                let turn = Quat::from_axis_angle(*along, angle);
                Transform::from_translation(a - turn * a).with_rotation(turn)
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
    pub fn type_name(&self) -> String {
        match self {
            FeatureOp::Primitive { shape, .. } => shape.name(),
            FeatureOp::Extrude { .. } => get!("prop.features.extrude"),
            FeatureOp::Sweep { .. } => get!("prop.features.sweep"),
            FeatureOp::Loft { .. } => get!("prop.features.loft"),
            FeatureOp::Revolve { .. } => get!("prop.features.revolve"),
            FeatureOp::Union { .. } => BooleanOp::Union.name(),
            FeatureOp::Subtract { .. } => BooleanOp::Subtract.name(),
            FeatureOp::Intersect { .. } => BooleanOp::Intersect.name(),
            FeatureOp::Mirror { .. } => get!("prop.features.mirror"),
            FeatureOp::Paint { .. } => get!("prop.features.paint"),
            FeatureOp::Pattern { .. } => get!("prop.features.pattern"),
            FeatureOp::Plane { .. } => get!("prop.features.plane"),
            FeatureOp::Axis { .. } => get!("prop.features.axis"),
            FeatureOp::Point { .. } => get!("prop.features.point"),
            FeatureOp::Sketch { .. } => get!("prop.features.sketch"),
        }
    }

    /// Every reference the feature holds, in the order a tool asks for them.
    /// [`Self::set_field`] takes the same indices; `consumes` and
    /// `references` are read from this, so a reference left out here is one
    /// nothing knows the feature depends on.
    pub fn fields(&self) -> Vec<Field> {
        let field = |label, slot, refs: Vec<AnyRef>| Field { label, slot, consumed: false, picked: true, refs };
        let bodies = |label, slot, list: &[BodyRef]| Field {
            label,
            slot,
            consumed: true,
            picked: true,
            refs: list.iter().map(|body| AnyRef::Body { body: *body }).collect(),
        };
        // Regions have rows of their own; a face is a Pick row.
        let profiled = |profile: &ProfileSource| match profile {
            ProfileSource::Regions { sketch, regions } => Field {
                label: "prop.inspector.regions_field",
                slot: slot::REGIONS,
                consumed: false,
                picked: false,
                refs: vec![AnyRef::Regions { sketch: *sketch, entries: regions.clone() }],
            },
            ProfileSource::Face { face } => Field {
                label: "prop.inspector.profile_face",
                slot: slot::REGIONS,
                consumed: false,
                picked: true,
                refs: vec![AnyRef::Face { face: face.clone() }],
            },
        };
        // Drawn in the feature's own sections rather than as rows, each
        // empty while unset, so every field keeps its index.
        let unpicked = |label, slot, refs: Vec<AnyRef>| Field { label, slot, consumed: false, picked: false, refs };
        // The bodies a sweep joins, cuts or intersects, eaten and shown only
        // while it does.
        let operand = |op: &SweepOp, targets: &[BodyRef]| match op {
            SweepOp::NewBody => Field { picked: false, ..bodies("prop.inspector.targets", slot::TARGETS, &[]) },
            _ => bodies("prop.inspector.targets", slot::TARGETS, targets),
        };
        let bound = |end: Option<&End>| end.and_then(End::bound).into_iter().cloned().collect();
        match self {
            FeatureOp::Primitive { .. } => vec![],
            FeatureOp::Extrude { profile, direction, end, other, op, targets, .. } => vec![
                profiled(profile),
                unpicked("prop.inspector.direction", slot::DIRECTION, direction.iter().cloned().map(AnyRef::Direction).collect()),
                unpicked("prop.extrude.up_to", slot::BOUND, bound(Some(end))),
                unpicked("prop.extrude.up_to", slot::BOUND, bound(other.as_ref())),
                operand(op, targets),
            ],
            FeatureOp::Sweep { profile, path, op, targets, .. } => vec![
                profiled(profile),
                field("prop.inspector.path", slot::PATH, vec![AnyRef::Path(path.clone())]),
                operand(op, targets),
            ],
            FeatureOp::Revolve { profile, axis, op, targets, .. } => vec![
                profiled(profile),
                field("prop.inspector.axis", slot::AXIS, vec![AnyRef::Curve(axis.clone())]),
                operand(op, targets),
            ],
            FeatureOp::Loft { profiles, op, targets, .. } => std::iter::once(field(
                "prop.inspector.profiles",
                slot::PROFILES,
                profiles.iter().map(|p| AnyRef::Regions { sketch: p.sketch, entries: vec![p.region] }).collect(),
            ))
            .chain(profiles.iter().map(|p| unpicked("prop.loft.start", slot::POINT, p.start.iter().cloned().map(AnyRef::Point).collect())))
            .chain(std::iter::once(operand(op, targets)))
            .collect(),
            FeatureOp::Union { bodies: list } | FeatureOp::Intersect { bodies: list } => {
                vec![bodies("prop.inspector.bodies", slot::BODIES_2, list)]
            }
            FeatureOp::Subtract { targets, tools } => vec![
                bodies("prop.inspector.targets", slot::TARGETS, targets),
                bodies("prop.inspector.tools", slot::TOOLS, tools),
            ],
            FeatureOp::Mirror { targets, about, .. } => vec![
                bodies("prop.inspector.targets", slot::BODIES, targets),
                field("prop.inspector.plane", slot::PLANE, vec![AnyRef::Plane(*about)]),
            ],
            FeatureOp::Paint { targets, .. } => vec![bodies("prop.inspector.targets", slot::BODIES, targets)],
            FeatureOp::Pattern { targets, along, .. } => vec![
                bodies("prop.inspector.targets", slot::BODIES, targets),
                match along {
                    PatternKind::Linear { direction, .. } => {
                        field("prop.inspector.direction", slot::DIRECTION, vec![AnyRef::Direction(direction.clone())])
                    }
                    PatternKind::Radial { axis, .. } => {
                        field("prop.inspector.axis", slot::AXIS, vec![AnyRef::Curve(axis.clone())])
                    }
                },
            ],
            FeatureOp::Plane { def } => vec![field("prop.inspector.made_from", slot::MAKE_PLANE, def.refs())],
            FeatureOp::Axis { def } => vec![field("prop.inspector.made_from", slot::MAKE_AXIS, def.refs())],
            FeatureOp::Point { def } => vec![field("prop.inspector.made_from", slot::MAKE_POINT, def.refs())],
            FeatureOp::Sketch { on, .. } => vec![field("prop.inspector.on_plane", slot::PLANE, vec![AnyRef::Plane(*on)])],
        }
    }

    /// Writes field `index` of [`Self::fields`]; `false` if the value is not
    /// that field's kind. A construction's field takes the whole feature it
    /// made, keeping the distance or angle when it is made the same way.
    pub fn set_field(&mut self, index: usize, value: Resolved) -> bool {
        match (self, index, value) {
            (
                FeatureOp::Extrude { profile, .. } | FeatureOp::Revolve { profile, .. } | FeatureOp::Sweep { profile, .. },
                0,
                Resolved::Regions { sketch, entries },
            ) => *profile = ProfileSource::Regions { sketch, regions: entries },
            (
                FeatureOp::Extrude { profile, .. } | FeatureOp::Revolve { profile, .. } | FeatureOp::Sweep { profile, .. },
                0,
                Resolved::ProfileFace(face),
            ) => *profile = ProfileSource::Face { face },
            (FeatureOp::Sweep { path, .. }, 1, Resolved::Path(to)) => *path = to,
            (FeatureOp::Extrude { direction, .. }, 1, Resolved::Direction(to)) => *direction = Some(to),
            (FeatureOp::Extrude { end, .. }, 2, Resolved::Bound(to)) => *end = End::UpTo { to },
            (FeatureOp::Extrude { other, .. }, 3, Resolved::Bound(to)) => *other = Some(End::UpTo { to }),
            (FeatureOp::Extrude { targets, .. }, 4, Resolved::Bodies(to))
            | (FeatureOp::Revolve { targets, .. } | FeatureOp::Sweep { targets, .. }, 2, Resolved::Bodies(to)) => *targets = to,
            (FeatureOp::Loft { profiles, targets, .. }, at, Resolved::Bodies(to)) if at == profiles.len() + 1 => *targets = to,
            (FeatureOp::Loft { profiles, .. }, 0, Resolved::Profiles(to)) => {
                // A profile on a sketch it already had keeps its start.
                let kept = |sketch| profiles.iter().find(|p| p.sketch == sketch).and_then(|p| p.start.clone());
                *profiles = to.into_iter().map(|(sketch, region)| Profile { sketch, region, start: kept(sketch) }).collect();
            }
            (FeatureOp::Loft { profiles, .. }, at, Resolved::Point(to)) if at >= 1 && at <= profiles.len() => {
                profiles[at - 1].start = Some(to);
            }
            (FeatureOp::Revolve { axis, .. }, 1, Resolved::Curve(curve)) => *axis = curve,
            (
                FeatureOp::Union { bodies: list }
                | FeatureOp::Intersect { bodies: list }
                | FeatureOp::Subtract { targets: list, .. }
                | FeatureOp::Mirror { targets: list, .. }
                | FeatureOp::Paint { targets: list, .. }
                | FeatureOp::Pattern { targets: list, .. },
                0,
                Resolved::Bodies(to),
            ) => *list = to,
            (FeatureOp::Subtract { tools, .. }, 1, Resolved::Bodies(to)) => *tools = to,
            (FeatureOp::Mirror { about, .. }, 1, Resolved::Plane(on)) => *about = on,
            (FeatureOp::Pattern { along: PatternKind::Linear { direction, .. }, .. }, 1, Resolved::Direction(to)) => *direction = to,
            (FeatureOp::Pattern { along: PatternKind::Radial { axis, .. }, .. }, 1, Resolved::Curve(curve)) => *axis = curve,
            (FeatureOp::Sketch { on, .. }, 0, Resolved::Plane(to)) => *on = to,
            (FeatureOp::Plane { def }, 0, Resolved::Op(FeatureOp::Plane { def: mut made })) => {
                match (&*def, &mut made) {
                    (PlaneDef::Offset { distance, .. }, PlaneDef::Offset { distance: new, .. }) => *new = *distance,
                    (PlaneDef::Angle { degrees, .. }, PlaneDef::Angle { degrees: new, .. }) => *new = *degrees,
                    _ => {}
                }
                *def = made;
            }
            (FeatureOp::Axis { def }, 0, Resolved::Op(FeatureOp::Axis { def: made })) => *def = made,
            (FeatureOp::Point { def }, 0, Resolved::Op(FeatureOp::Point { def: made })) => *def = made,
            _ => return false,
        }
        true
    }

    /// The features whose bodies it eats; see [`Self::fields`].
    pub fn consumes(&self) -> Vec<PropFeatureId> {
        self.fields().iter().filter(|field| field.consumed).flat_map(|field| field.refs.iter().flat_map(AnyRef::features)).collect()
    }

    /// The bodies it eats, in operand order.
    pub fn eats(&self) -> Vec<BodyRef> {
        self.fields()
            .iter()
            .filter(|field| field.consumed)
            .flat_map(|field| field.refs.iter().filter_map(|reference| match reference {
                AnyRef::Body { body } => Some(*body),
                _ => None,
            }))
            .collect()
    }

    /// Features this one names without consuming: what a datum was made
    /// from, the plane or line something is about, and every copy that
    /// geometry went through.
    pub fn references(&self) -> Vec<PropFeatureId> {
        let mut named: Vec<PropFeatureId> = self
            .fields()
            .iter()
            .filter(|field| !field.consumed)
            .flat_map(|field| field.refs.iter().flat_map(AnyRef::features))
            .collect();
        named.sort();
        named.dedup();
        named
    }

    pub fn depends_on(&self) -> Vec<PropFeatureId> {
        let mut all = self.consumes();
        all.extend(self.references());
        all.sort();
        all.dedup();
        all
    }

    pub fn placement_mut(&mut self) -> Option<&mut Placement> {
        match self {
            FeatureOp::Primitive { placement, .. } => Some(placement),
            FeatureOp::Extrude { .. }
            | FeatureOp::Revolve { .. }
            | FeatureOp::Sweep { .. }
            | FeatureOp::Loft { .. }
            | FeatureOp::Union { .. }
            | FeatureOp::Subtract { .. }
            | FeatureOp::Intersect { .. }
            | FeatureOp::Mirror { .. }
            | FeatureOp::Paint { .. }
            | FeatureOp::Pattern { .. }
            | FeatureOp::Plane { .. }
            | FeatureOp::Axis { .. }
            | FeatureOp::Point { .. }
            | FeatureOp::Sketch { .. } => None,
        }
    }

    /// How many bodies it makes when it runs, which every [`BodyRef`] to them
    /// carries as its `of`.
    pub fn bodies_made(&self) -> u32 {
        let count = |list: &[BodyRef]| list.len() as u32;
        match self {
            FeatureOp::Plane { .. } | FeatureOp::Axis { .. } | FeatureOp::Point { .. } | FeatureOp::Sketch { .. } => 0,
            FeatureOp::Subtract { targets, .. }
            | FeatureOp::Mirror { targets, .. }
            | FeatureOp::Paint { targets, .. }
            | FeatureOp::Pattern { targets, .. } => count(targets),
            FeatureOp::Extrude { op, targets, .. }
            | FeatureOp::Revolve { op, targets, .. }
            | FeatureOp::Sweep { op, targets, .. }
            | FeatureOp::Loft { op, targets, .. } => match op {
                SweepOp::Cut | SweepOp::Intersect => count(targets),
                SweepOp::NewBody | SweepOp::Join => 1,
            },
            FeatureOp::Primitive { .. } | FeatureOp::Union { .. } | FeatureOp::Intersect { .. } => 1,
        }
    }

    /// The target body `index` came from; `None` where its one body joins
    /// them all.
    pub fn source_of(&self, index: u32) -> Option<BodyRef> {
        let targets = match self {
            FeatureOp::Subtract { targets, .. }
            | FeatureOp::Mirror { targets, .. }
            | FeatureOp::Paint { targets, .. }
            | FeatureOp::Pattern { targets, .. } => targets,
            FeatureOp::Extrude { op, targets, .. }
            | FeatureOp::Revolve { op, targets, .. }
            | FeatureOp::Sweep { op, targets, .. }
            | FeatureOp::Loft { op, targets, .. }
                if matches!(op, SweepOp::Cut | SweepOp::Intersect) =>
            {
                targets
            }
            _ => return None,
        };
        targets.get(index as usize).copied()
    }

    /// A sweep that joins, cuts or intersects with no target named.
    pub fn sweep_with_no_targets(&self) -> bool {
        match self {
            FeatureOp::Extrude { op, targets, .. }
            | FeatureOp::Revolve { op, targets, .. }
            | FeatureOp::Sweep { op, targets, .. }
            | FeatureOp::Loft { op, targets, .. } => *op != SweepOp::NewBody && targets.is_empty(),
            _ => false,
        }
    }

    pub fn sweep_op_mut(&mut self) -> Option<&mut SweepOp> {
        match self {
            FeatureOp::Extrude { op, .. } | FeatureOp::Revolve { op, .. } | FeatureOp::Sweep { op, .. } | FeatureOp::Loft { op, .. } => {
                Some(op)
            }
            _ => None,
        }
    }

    pub fn makes_body(&self) -> bool {
        !matches!(self, FeatureOp::Plane { .. } | FeatureOp::Axis { .. } | FeatureOp::Point { .. } | FeatureOp::Sketch { .. })
    }

    /// Each sketch the feature sweeps regions of, and the entries it names.
    pub fn sketch_regions(&self) -> Vec<(PropFeatureId, Vec<SketchId>)> {
        match self {
            FeatureOp::Extrude { profile: ProfileSource::Regions { sketch, regions }, .. }
            | FeatureOp::Revolve { profile: ProfileSource::Regions { sketch, regions }, .. }
            | FeatureOp::Sweep { profile: ProfileSource::Regions { sketch, regions }, .. } => vec![(*sketch, regions.clone())],
            FeatureOp::Loft { profiles, .. } => profiles.iter().map(|p| (p.sketch, vec![p.region])).collect(),
            _ => vec![],
        }
    }

    /// The local axis "points along" turns. Must match how each primitive is
    /// built: prism and cone on `+Y`. Shapes with no length of their own take
    /// `+Z`.
    pub fn main_axis(&self) -> Vec3 {
        match self {
            FeatureOp::Primitive { shape: Shape::Prism { .. } | Shape::Cone { .. }, .. } => Vec3::Y,
            _ => Vec3::Z,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropFeature {
    pub id: PropFeatureId,
    /// User-typed content, not a lang key.
    pub name: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// Not `#[serde(flatten)]`: TOML needs every scalar before any sub-table,
    /// which a flattened enum with nested tables cannot promise.
    pub op: FeatureOp,
}

fn unit() -> f32 {
    1.0
}

fn enabled_by_default() -> bool {
    true
}

impl PropFeature {
    pub fn new(id: PropFeatureId, op: FeatureOp) -> PropFeature {
        PropFeature { id, name: op.type_name(), enabled: true, op }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub feature: PropFeatureId,
    pub message: String,
}

impl Problem {
    fn missing_body(feature: PropFeatureId) -> Self {
        Self { feature, message: get!("prop.problems.missing_body") }
    }

    fn renumbered_body(feature: PropFeatureId) -> Self {
        Self { feature, message: get!("prop.problems.renumbered_body") }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Evaluated {
    /// The bodies left standing, in the order they were made.
    pub bodies: Vec<(BodyRef, Solid)>,
    pub problems: Vec<Problem>,
    /// Beside `bodies`, in the same order: the outlines each was made from.
    pub wires: Vec<(BodyRef, Vec<Wire>)>,
    /// The named corners and edges that can be seen; see [`pickables`].
    pub pickables: Vec<Pickable>,
    /// The origin planes, then plane features in list order.
    pub planes: Vec<Datum>,
    /// Sketches, in list order, placed where their planes were.
    pub sketches: Vec<PlacedSketch>,
    /// Datum axes and points, in list order.
    pub axes: Vec<DatumAxis>,
    pub points: Vec<DatumPoint>,
    /// Meters each way of the origin the origin planes and axes are drawn.
    pub origin_half: f32,
    /// What every polygon's face tag names.
    pub faces: FaceTable,
    /// Every body's seams and junctions, each marked whether it is offered,
    /// found the first time something asks: the game bakes props through
    /// [`evaluate`] and never does.
    found: std::sync::OnceLock<Found>,
}

/// Seams, junctions, and the generator edges a seam is offered in place of.
#[derive(Clone, Debug, Default)]
struct Found {
    seams: Vec<Seam>,
    junctions: Vec<Junction>,
    replaced: Vec<GeomRef>,
}

impl Evaluated {
    pub fn triangle_count(&self) -> usize {
        self.bodies.iter().map(|(_, solid)| solid.triangle_count()).sum()
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.bounds_of(|_| true)
    }

    /// A feature already eaten has no body, so contributes nothing.
    pub fn bounds_of_bodies(&self, ids: &[BodyRef]) -> Option<(Vec3, Vec3)> {
        self.bounds_of(|id| ids.contains(&id))
    }

    fn bounds_of(&self, wanted: impl Fn(BodyRef) -> bool) -> Option<(Vec3, Vec3)> {
        self.bodies
            .iter()
            .filter(|(id, _)| wanted(*id))
            .filter_map(|(_, solid)| solid.bounds())
            .reduce(|(min_a, max_a), (min_b, max_b)| (min_a.min(min_b), max_a.max(max_b)))
    }

    /// Tested against faces, not boxes, so a click down a bore reaches what is
    /// behind. Faces are convex, so a fan triangulates them exactly.
    pub fn body_under(&self, origin: Vec3, direction: Vec3) -> Option<BodyRef> {
        self.face_under(origin, direction).map(|(id, _, _)| id)
    }

    pub fn face_distance(&self, ray: Ray3d) -> Option<f32> {
        self.face_under(ray.origin, *ray.direction).map(|(_, _, distance)| distance)
    }

    /// The nearest face hit and how far down the ray: what [`Evaluated::face_distance`],
    /// [`Evaluated::surface_under`] and [`Evaluated::region_under`] each cast for, so one cast
    /// can serve all three.
    pub fn face_under(&self, origin: Vec3, direction: Vec3) -> Option<(BodyRef, &Polygon, f32)> {
        self.bodies
            .iter()
            .flat_map(|(id, solid)| solid.polygons().iter().map(move |polygon| (*id, polygon)))
            .filter_map(|(id, polygon)| {
                let v = &polygon.vertices;
                (1..v.len().saturating_sub(1))
                    .filter_map(|i| ray_triangle(origin, direction, v[0], v[i], v[i + 1]))
                    .min_by(f32::total_cmp)
                    .map(|distance| (id, polygon, distance))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
    }

    /// What a click on a body's surface lands on: the body, and the named
    /// face its polygon carries the tag of.
    pub fn surface_under(&self, ray: Ray3d) -> Option<(BodyRef, Option<GeomRef>)> {
        Some(self.surface_of_hit(self.face_under(ray.origin, *ray.direction)?))
    }

    /// [`Evaluated::surface_under`] for a hit already cast.
    pub fn surface_of_hit(&self, (body, polygon, _): (BodyRef, &Polygon, f32)) -> (BodyRef, Option<GeomRef>) {
        (body, self.faces.name(polygon.surface.tag).cloned())
    }

    /// The sketch region under the pointer, nearest first, unless a face is
    /// in front of it. A region on a face wins over that face: picking it is
    /// the reason it was drawn there.
    pub fn region_under(&self, ray: Ray3d) -> Option<(PropFeatureId, &Region)> {
        self.region_in_front_of(ray, self.face_distance(ray))
    }

    /// [`Evaluated::region_under`] with the nearest face's distance already cast for.
    pub fn region_in_front_of(&self, ray: Ray3d, face: Option<f32>) -> Option<(PropFeatureId, &Region)> {
        self.sketches
            .iter()
            .filter_map(|placed| {
                let hit = placed.plane.hit(ray)?;
                let distance = hit.distance(ray.origin);
                if face.is_some_and(|face| distance > face + 0.002 + face * 0.01) {
                    return None;
                }
                let region = placed.arrangement.region_at(placed.plane.flatten(hit))?;
                Some((distance, placed.id, region))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id, region)| (id, region))
    }

    pub fn sketch(&self, id: PropFeatureId) -> Option<&PlacedSketch> {
        self.sketches.iter().find(|placed| placed.id == id)
    }

    pub fn datum(&self, id: PlaneRef) -> Option<&Datum> {
        self.planes.iter().find(|datum| datum.id == id)
    }

    /// The finished prop, for resolving references against.
    pub fn scope(&self) -> Scope<'_> {
        Scope::new(
            self.wires.iter().flat_map(|(_, wires)| wires),
            &self.planes,
            &self.sketches,
            &self.axes,
            &self.points,
            self.origin_half,
        )
        .with_seams(self.seams(), self.junctions())
    }

    pub fn seams(&self) -> &[Seam] {
        &self.found().seams
    }

    pub fn junctions(&self) -> &[Junction] {
        &self.found().junctions
    }

    /// Whether an offered seam stands in for this generator edge.
    pub fn replaced(&self, edge: &GeomRef) -> bool {
        self.found().replaced.contains(edge)
    }

    fn found(&self) -> &Found {
        self.found.get_or_init(|| {
            let (mut seams, mut junctions) = (vec![], vec![]);
            for (body, solid) in &self.bodies {
                let (found, meeting) = seam::find(body.feature, solid, &self.faces);
                seams.extend(found);
                junctions.extend(meeting);
            }
            let replaced = seam::offer(&mut seams, &mut junctions, &self.pickables);
            Found { seams, junctions, replaced }
        })
    }

    /// Where a reference is now, whether or not it can be seen.
    pub fn locate(&self, reference: &GeomRef) -> Option<Located> {
        self.scope().locate(reference)
    }

    /// A curve as it is now, end to end as it is drawn: an origin axis runs
    /// across the origin planes.
    pub fn drawn_curve(&self, curve: &CurveRef) -> Option<(Vec3, Vec3)> {
        match curve {
            CurveRef::Axis { axis } => Some((-axis.normal() * self.origin_half, axis.normal() * self.origin_half)),
            _ => curve.resolve(&self.scope())?.line(),
        }
    }

    pub fn body_of(&self, reference: &GeomRef) -> Option<BodyRef> {
        self.wires
            .iter()
            .find(|(_, wires)| wires.iter().any(|wire| wire.source == reference.source))
            .map(|(id, _)| *id)
    }
}

/// Möller–Trumbore, from either side: an inside-out face can still be seen
/// and clicked.
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

/// Where a revolve turns its regions: local `+Y` along the axis from its first
/// point, local `+X` across it toward the regions — so every radius
/// [`Solid::revolve`] sees is positive — and local `Z` completing the frame.
struct AxisFrame {
    transform: Transform,
    origin: Vec2,
    along: Vec2,
    across: Vec2,
}

impl AxisFrame {
    /// `None` if the axis has no length or crosses any of the regions.
    fn new(plane: &Plane, a: Vec2, b: Vec2, regions: &[&Region]) -> Option<AxisFrame> {
        if !regions.iter().all(|region| axis_clear(region, a, b)) {
            return None;
        }
        let along = (b - a).try_normalize()?;
        // Either perpendicular in the plane; the one the regions are on.
        let mut across = Vec2::new(along.y, -along.x);
        let lean: f32 = regions.iter().flat_map(|region| &region.outer).map(|p| (*p - a).dot(across)).sum();
        if lean < 0.0 {
            across = -across;
        }
        let lift = |v: Vec2| *plane.x * v.x + *plane.y() * v.y;
        let (x, y) = (lift(across), lift(along));
        let basis = Mat3::from_cols(x, y, x.cross(y));
        let transform = Transform::from_translation(plane.lift(a)).with_rotation(Quat::from_mat3(&basis));
        Some(AxisFrame { transform, origin: a, along, across })
    }

    /// Sketch-plane points as (radius, height) about the axis.
    fn local(&self, points: &[Vec2]) -> Vec<Vec2> {
        points
            .iter()
            .map(|p| Vec2::new((*p - self.origin).dot(self.across), (*p - self.origin).dot(self.along)))
            .collect()
    }
}

/// Stands a sweep's body up on its own, or joins, cuts or intersects its
/// target with it under the sweep's id. A target that cannot be combined is
/// put back where it stood.
fn combine(
    body: Live,
    op: SweepOp,
    targets: &[BodyRef],
    live: &mut Vec<Live>,
    problems: &mut Vec<Problem>,
    faces: &mut FaceTable,
) {
    let feature = body.body.feature;
    if op == SweepOp::NewBody {
        live.push(body);
        return;
    }
    if targets.is_empty() {
        problems.push(Problem::missing_body(feature));
        return;
    }
    let taken = match take(targets, live, feature) {
        Ok(taken) => taken,
        Err(problem) => {
            problems.push(problem);
            return;
        }
    };
    let solids: Vec<Solid> = match op {
        SweepOp::Join => {
            let joined = taken[1..].iter().fold(taken[0].1.solid.clone(), |joined, (_, target)| joined.union(&target.solid));
            vec![joined.union(&body.solid)]
        }
        SweepOp::Cut => taken.iter().map(|(_, target)| target.solid.subtract(&body.solid)).collect(),
        SweepOp::Intersect | SweepOp::NewBody => taken.iter().map(|(_, target)| target.solid.intersect(&body.solid)).collect(),
    };
    if solids.iter().any(Solid::is_empty) {
        problems.push(Problem { feature, message: get!("prop.problems.nothing_left") });
        put_back(taken, live);
        return;
    }
    let of = solids.len() as u32;
    if op == SweepOp::Join {
        let wires = taken.into_iter().flat_map(|(_, target)| target.wires).chain(body.wires).collect();
        let solid = solids.into_iter().next().expect("one joined body");
        live.push(Live { body: body.body, solid, wires });
        return;
    }
    for (index, ((_, target), solid)) in taken.into_iter().zip(solids).enumerate() {
        let (solid, copied) = own_copy(solid, &[&body], feature, index as u32, faces);
        let wires = target.wires.into_iter().chain(copied).collect();
        live.push(Live { body: BodyRef { feature, index: index as u32, of }, solid, wires });
    }
}

/// The faces and outlines `shared` brought into body `index` of one per
/// target, named apart from the other bodies' copies so a click on that wall
/// names this body. The first keeps the plain names.
fn own_copy(solid: Solid, shared: &[&Live], feature: PropFeatureId, index: u32, faces: &mut FaceTable) -> (Solid, Vec<Wire>) {
    let wires = shared.iter().flat_map(|body| &body.wires);
    if index == 0 {
        return (solid, wires.cloned().collect());
    }
    let copied = wires.map(|wire| wire.copied(feature, index, wire.outline.clone())).collect();
    let tags: std::collections::HashSet<u32> =
        shared.iter().flat_map(|body| body.solid.polygons().iter().map(|polygon| polygon.surface.tag)).collect();
    (solid.retagged(|tag| if tags.contains(&tag) { faces.copied(tag, feature, index) } else { tag }), copied)
}

/// Takes `bodies` out of `live`, in the order named, each with where it stood
/// so [`put_back`] can return it. All are found before any is taken, so a
/// missing one takes nothing.
fn take(bodies: &[BodyRef], live: &mut Vec<Live>, feature: PropFeatureId) -> Result<Vec<(usize, Live)>, Problem> {
    if bodies.is_empty() {
        return Err(Problem::missing_body(feature));
    }
    let mut at = Vec::with_capacity(bodies.len());
    for body in bodies {
        let found = locate(*body, live, feature)?;
        if at.contains(&found) {
            return Err(Problem { feature, message: get!("prop.problems.self_boolean") });
        }
        at.push(found);
    }
    // Highest first, so removing one does not shift the rest.
    let mut order: Vec<usize> = (0..at.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(at[i]));
    let mut taken: Vec<Option<(usize, Live)>> = (0..at.len()).map(|_| None).collect();
    for i in order {
        taken[i] = Some((at[i], live.remove(at[i])));
    }
    Ok(taken.into_iter().flatten().collect())
}

fn put_back(mut taken: Vec<(usize, Live)>, live: &mut Vec<Live>) {
    taken.sort_by_key(|(at, _)| *at);
    for (at, body) in taken {
        live.insert(at.min(live.len()), body);
    }
}

/// Where `body` stands among the live bodies, or why `feature` cannot use it.
fn locate(body: BodyRef, live: &[Live], feature: PropFeatureId) -> Result<usize, Problem> {
    let at = live
        .iter()
        .position(|standing| standing.body.feature == body.feature && standing.body.index == body.index)
        .ok_or_else(|| Problem::missing_body(feature))?;
    match live[at].body.of == body.of {
        true => Ok(at),
        false => Err(Problem::renumbered_body(feature)),
    }
}

/// Meters an extrude through everything runs past the last body.
const THROUGH_MARGIN: f32 = 1e-3;
/// How far from lying in the sketch's plane an extrude's direction must be,
/// as the cosine of its angle to the plane's normal.
const LEAST_RISE: f32 = 1e-3;

/// Where an extrude's sides end, as signed meters along its direction.
struct Reach<'a> {
    origin: Vec3,
    normal: Vec3,
    along: Vec3,
    scope: &'a Scope<'a>,
    live: &'a [Live],
}

impl<'a> Reach<'a> {
    fn new(plane: &Plane, direction: Option<&DirectionRef>, scope: &'a Scope<'a>, live: &'a [Live]) -> Result<Reach<'a>, String> {
        let normal = *plane.normal;
        let along = match direction {
            None => normal,
            Some(direction) => *direction.resolve(scope).ok_or_else(|| get!("prop.problems.missing_direction"))?,
        };
        if along.dot(normal).abs() < LEAST_RISE {
            return Err(get!("prop.problems.direction_in_plane"));
        }
        Ok(Reach { origin: plane.origin, normal, along, scope, live })
    }

    /// How far along the direction the sketch's origin goes to reach the
    /// level of `point` or the plane through `point` with `normal`.
    fn level(&self, point: Vec3, normal: Vec3) -> Option<f32> {
        let rise = self.along.dot(normal);
        (rise.abs() >= LEAST_RISE).then(|| (point - self.origin).dot(normal) / rise)
    }

    /// `sign` is 1 for the side along the direction and -1 for the other.
    fn side(&self, end: &End, sign: f32) -> Result<f32, String> {
        match end {
            End::Distance { distance } => Ok(distance * sign),
            End::UpTo { to } => {
                let level = match to {
                    AnyRef::Point(point) => point.resolve(self.scope).and_then(|at| self.level(at, self.normal)),
                    AnyRef::Plane(on) => self.scope.plane(*on).and_then(|plane| self.level(plane.origin, *plane.normal)),
                    AnyRef::Face { face } => match crate::prop::refs::flat_face(face, self.scope) {
                        Some(Located::Face { facets, normal }) => facets.first().and_then(|f| f.first()).and_then(|at| self.level(*at, normal)),
                        _ => None,
                    },
                    _ => None,
                };
                level.ok_or_else(|| get!("prop.problems.missing_bound"))
            }
            End::ThroughAll => {
                let corners = self
                    .live
                    .iter()
                    .filter_map(|body| body.solid.bounds())
                    .flat_map(|(min, max)| (0..8).map(move |i| Vec3::select(BVec3::new(i & 1 != 0, i & 2 != 0, i & 4 != 0), max, min)));
                let levels = corners.filter_map(|corner| self.level(corner, self.normal));
                // Never past the sketch the wrong way: with nothing on this
                // side, the side stops at the sketch.
                let furthest = match sign > 0.0 {
                    true => levels.fold(f32::NEG_INFINITY, f32::max).max(0.0),
                    false => levels.fold(f32::INFINITY, f32::min).min(0.0),
                };
                match self.live.is_empty() {
                    false => Ok(furthest + THROUGH_MARGIN * sign),
                    true => Err(get!("prop.problems.through_nothing")),
                }
            }
        }
    }

    /// The direction in the sketch's own coordinates, `z` along its normal.
    fn local(&self, plane: &Plane) -> Vec3 {
        Vec3::new(self.along.dot(*plane.x), self.along.dot(*plane.y()), self.along.dot(*plane.normal))
    }
}

/// A body mid-evaluation, with the outlines of everything that went into it.
struct Live {
    body: BodyRef,
    solid: Solid,
    wires: Vec<Wire>,
}

/// How far the origin planes reach: the furthest any body reaches from the
/// origin, plus the same margin a plane feature's square has.
fn origin_half_of(live: &[Live]) -> f32 {
    let reach = live
        .iter()
        .filter_map(|body| body.solid.bounds())
        .map(|(min, max)| min.abs().max(max.abs()).max_element())
        .fold(0.0, f32::max);
    (reach * 1.25).max(MIN_ORIGIN_HALF)
}

/// Meters a revolve's axis may sit off its sketch's plane: the kernel's
/// rounding, not a modeling tolerance.
pub(crate) const IN_PLANE: f32 = 1e-4;

/// Each region's loops, in `local` coordinates, skinned by the run `run_of`
/// makes of them, moved by each of `places` in turn, and unioned into one
/// body — unioned rather than gathered, since regions sharing a line must
/// become one closed surface. `Err` is the Problem's message.
fn sweep(
    regions: &[Region],
    run_of: impl Fn(Vec<TaggedLoop>) -> Run,
    local: impl Fn(&[Vec2]) -> Vec<Vec2>,
    places: &[Transform],
    surface: Surface,
    by_across: bool,
) -> Result<(Solid, Outline), String> {
    let mut outline = Outline::default();
    let mut solid: Option<Solid> = None;
    for region in regions {
        let swept = skin(&run_of(tagged_loops(region, &local)), surface).map_err(SkinProblem::message)?;
        let swept = if by_across { numbered_by_tag(swept) } else { swept };
        // Each region numbers its faces from 0, but the outline lists them
        // after the earlier regions' faces.
        let offset = outline.faces.len() as u32;
        let solid_of_region = swept.solid.retagged(|tag| if tag == NO_FACE { tag } else { tag + offset });
        let piece = places.iter().fold(solid_of_region, |solid, place| solid.transformed(*place));
        solid = Some(match solid {
            Some(joined) => joined.union(&piece),
            None => piece,
        });
        outline.append(places.iter().fold(swept.outline, |outline, place| outline.transformed(*place)));
    }
    match solid {
        Some(solid) if !solid.is_empty() => Ok((solid, outline)),
        _ => Err(get!("prop.problems.empty_sweep")),
    }
}

/// Side faces in order of span, then segment tag, so a notch that adds pieces
/// to a face profile's boundary renumbers none of them. Caps keep 0 and 1.
fn numbered_by_tag(swept: Swept) -> Swept {
    let caps = (swept.outline.faces.len() - swept.sides.len()) as u32;
    let mut order: Vec<usize> = (0..swept.sides.len()).collect();
    order.sort_by_key(|&side| swept.sides[side]);
    let mut moved_to = vec![0u32; order.len()];
    for (new, &old) in order.iter().enumerate() {
        moved_to[old] = caps + new as u32;
    }
    let Swept { solid, mut outline, sides } = swept;
    let side_faces = outline.faces.split_off(caps as usize);
    let mut slots: Vec<Option<OutlineFace>> = vec![None; side_faces.len()];
    for (old, face) in side_faces.into_iter().enumerate() {
        slots[(moved_to[old] - caps) as usize] = Some(face);
    }
    outline.faces.extend(slots.into_iter().flatten());
    let solid = solid.retagged(|tag| match tag >= caps && tag != NO_FACE {
        true => moved_to.get((tag - caps) as usize).copied().unwrap_or(tag),
        false => tag,
    });
    let sides = order.iter().map(|&old| sides[old]).collect();
    Swept { solid, outline, sides }
}

/// The bool is whether side faces are numbered by the face across. `None`
/// once a Problem is pushed.
fn profile_of(
    feature: PropFeatureId,
    profile: &ProfileSource,
    scope: &Scope,
    problems: &mut Vec<Problem>,
) -> Option<(Plane, Vec<Region>, bool)> {
    match profile {
        ProfileSource::Regions { sketch, regions } => {
            let Some(placed) = scope.sketch(*sketch) else {
                problems.push(Problem { feature, message: get!("prop.problems.missing_sketch") });
                return None;
            };
            let found = resolve_regions(feature, placed, regions, problems)?;
            Some((placed.plane, found, false))
        }
        ProfileSource::Face { face } => match face_profile(face, scope) {
            Ok((plane, regions)) => Some((plane, regions, true)),
            Err(message) => {
                problems.push(Problem { feature, message });
                None
            }
        },
    }
}

/// The outside loop then the holes, each segment tagged by its entity so an
/// arc's many pieces sweep one face.
fn tagged_loops(region: &Region, local: impl Fn(&[Vec2]) -> Vec<Vec2>) -> Vec<TaggedLoop> {
    let tagged = |points: &Vec<Vec2>, entities: &Vec<SketchId>| TaggedLoop {
        points: local(points),
        tags: entities.iter().map(|entity| entity.0).collect(),
    };
    std::iter::once(tagged(&region.outer, &region.outer_entities))
        .chain(region.holes.iter().zip(&region.hole_entities).map(|(hole, entities)| tagged(hole, entities)))
        .collect()
}

/// Every region a sweep names, as the sketch now stands, or a Problem naming
/// the first that is not there.
fn resolve_regions(
    feature: PropFeatureId,
    placed: &PlacedSketch,
    regions: &[SketchId],
    problems: &mut Vec<Problem>,
) -> Option<Vec<Region>> {
    if regions.is_empty() {
        problems.push(Problem { feature, message: get!("prop.problems.no_regions") });
        return None;
    }
    let mut found = Vec::with_capacity(regions.len());
    for &id in regions {
        match placed.resolve(id) {
            Some(region) => found.push(region),
            None => {
                let region = placed.region_label(id);
                problems.push(Problem { feature, message: get!("prop.problems.missing_region", "region", region) });
                return None;
            }
        }
    }
    Some(found)
}

/// A pure function of the list, so it runs without a `World`.
pub fn evaluate(features: &[PropFeature]) -> Evaluated {
    let mut live: Vec<Live> = Vec::new();
    let mut problems = Vec::new();
    let mut planes = Vec::new();
    let mut sketches = Vec::new();
    let mut axes = Vec::new();
    let mut points = Vec::new();
    let mut faces = FaceTable::default();

    for feature in features {

        if !feature.enabled {
            continue;
        }

        // Find before taking: a boolean that took one operand and failed to
        // find the other would delete a body it never used.
        // A generator's polygons carry its own face numbers until here.
        let made = |solid: Solid, outline: Outline, faces: &mut FaceTable| Live {
            body: BodyRef::whole(feature.id),
            solid: solid.retagged(|face| faces.of_generator(feature.id, face)),
            wires: vec![Wire::new(feature.id, outline)],
        };
        // Resolved against before anything this feature makes or takes.
        let scope = Scope::new(
            live.iter().flat_map(|body| &body.wires),
            &planes,
            &sketches,
            &axes,
            &points,
            origin_half_of(&live),
        )
        .with_bodies(live.iter().map(|body| (body.body.feature, &body.solid)), &faces);
        let resolved_axis = |axis: &CurveRef| axis.resolve(&scope).and_then(|curve| curve.line());

        match &feature.op {
            FeatureOp::Primitive { shape, placement, surface } => {
                let solid = shape.solid(*surface).transformed(placement.transform());
                live.push(made(solid, shape.outline().transformed(placement.transform()), &mut faces));
            }
            FeatureOp::Extrude { profile, taper, op, targets, end, other, direction, surface } => {
                let Some((plane, found, by_across)) = profile_of(feature.id, profile, &scope, &mut problems) else { continue };
                let reach = Reach::new(&plane, direction.as_ref(), &scope, &live).and_then(|reach| {
                    let near = match other {
                        Some(other) => reach.side(other, -1.0)?,
                        None => 0.0,
                    };
                    let far = reach.side(end, 1.0)?;
                    Ok((reach, near.min(far), near.max(far)))
                });
                let (reach, from, to) = match reach {
                    Ok(reached) => reached,
                    Err(message) => {
                        problems.push(Problem { feature: feature.id, message });
                        continue;
                    }
                };
                let rise = reach.along.dot(*plane.normal).abs() * from.abs().max(to.abs());
                if found.iter().any(|region| !tapers_within(&region.outer, *taper, rise)) {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.taper_closes") });
                    continue;
                }
                let local = reach.local(&plane);
                let run = |loops| pushed(loops, from, to, local, *taper);
                match sweep(&found, run, |points| points.to_vec(), &[plane.transform()], *surface, by_across) {
                    Ok((solid, outline)) => combine(made(solid, outline, &mut faces), *op, targets, &mut live, &mut problems, &mut faces),
                    Err(message) => problems.push(Problem { feature: feature.id, message }),
                }
            }
            FeatureOp::Sweep { profile, orient, twist, scale, op, targets, path, surface } => {
                let Some((plane, found, by_across)) = profile_of(feature.id, profile, &scope, &mut problems) else { continue };
                let Some(curves) = path.resolve(&scope) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.missing_path") });
                    continue;
                };
                let Some(points) = PathPoints::of(&curves).from_plane(&plane, IN_PLANE) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.path_off_plane") });
                    continue;
                };
                let frames = match path::frames(&points, *plane.x, *orient, *twist, *scale) {
                    Ok(frames) => frames,
                    Err(problem) => {
                        problems.push(Problem { feature: feature.id, message: problem.message() });
                        continue;
                    }
                };
                // From the sketch's own coordinates, carried from where the
                // path starts to each of its points.
                let base = plane.transform().compute_affine();
                let start = points.points[0];
                let placements: Vec<Affine3A> = frames
                    .iter()
                    .zip(&points.points)
                    .map(|((turn, size), at)| {
                        Affine3A::from_translation(*at)
                            * Affine3A::from_scale_rotation_translation(Vec3::splat(*size), *turn, Vec3::ZERO)
                            * Affine3A::from_translation(-start)
                            * base
                    })
                    .collect();
                let run = |loops: Vec<TaggedLoop>| Run {
                    sections: placements.iter().map(|frame| Section { loops: loops.clone(), frame: *frame }).collect(),
                    spans: points.spans.clone(),
                    closed: points.closed,
                };
                match sweep(&found, run, |points| points.to_vec(), &[], *surface, by_across) {
                    Ok((solid, outline)) => combine(made(solid, outline, &mut faces), *op, targets, &mut live, &mut problems, &mut faces),
                    Err(message) => problems.push(Problem { feature: feature.id, message }),
                }
            }
            FeatureOp::Loft { closed, op, targets, profiles, surface } => {
                let mut placed_profiles = Vec::with_capacity(profiles.len());
                for profile in profiles {
                    let Some(placed) = sketches.iter().find(|placed: &&PlacedSketch| placed.id == profile.sketch) else {
                        problems.push(Problem { feature: feature.id, message: get!("prop.problems.missing_sketch") });
                        break;
                    };
                    let Some(found) = resolve_regions(feature.id, placed, &[profile.region], &mut problems) else { break };
                    // A start that no longer resolves is reported and the
                    // nearest turn used in its place.
                    let start = profile.start.as_ref().and_then(|start| {
                        let at = start.resolve(&scope);
                        if at.is_none() {
                            problems.push(Problem { feature: feature.id, message: get!("prop.problems.loft_start_missing") });
                        }
                        at
                    });
                    placed_profiles.push(loft::Profile {
                        frame: placed.plane.transform().compute_affine(),
                        loops: tagged_loops(&found[0], |points| points.to_vec()),
                        start,
                    });
                }
                if placed_profiles.len() < profiles.len() {
                    continue;
                }
                let swept = loft::run(placed_profiles, *closed)
                    .map_err(|problem| problem.message())
                    .and_then(|run| skin(&run, *surface).map_err(SkinProblem::message));
                match swept {
                    Ok(swept) if !swept.solid.is_empty() => {
                        combine(made(swept.solid, swept.outline, &mut faces), *op, targets, &mut live, &mut problems, &mut faces)
                    }
                    Ok(_) => problems.push(Problem { feature: feature.id, message: get!("prop.problems.empty_sweep") }),
                    Err(message) => problems.push(Problem { feature: feature.id, message }),
                }
            }
            FeatureOp::Revolve { profile, axis, degrees, steps, symmetric, op, targets, surface } => {
                let Some((plane, found, by_across)) = profile_of(feature.id, profile, &scope, &mut problems) else { continue };
                let Some((a, b)) = resolved_axis(axis) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.missing_axis") });
                    continue;
                };
                if [a, b].iter().any(|p| plane.distance(*p).abs() > IN_PLANE) {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.axis_off_plane") });
                    continue;
                }
                let (a, b) = (plane.flatten(a), plane.flatten(b));
                let Some(frame) = AxisFrame::new(&plane, a, b, &found.iter().collect::<Vec<_>>()) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.axis_crosses") });
                    continue;
                };
                let steps = revolve_steps(*steps);
                let spin = match *symmetric {
                    true => Transform::from_rotation(Quat::from_rotation_y(-degrees.to_radians() / 2.0)),
                    false => Transform::IDENTITY,
                };
                let run = |loops| revolution(loops, *degrees, steps);
                match sweep(&found, run, |points| frame.local(points), &[spin, frame.transform], *surface, by_across) {
                    Ok((solid, outline)) => combine(made(solid, outline, &mut faces), *op, targets, &mut live, &mut problems, &mut faces),
                    Err(message) => problems.push(Problem { feature: feature.id, message }),
                }
            }
            FeatureOp::Union { bodies } | FeatureOp::Intersect { bodies } => {
                let taken = match take(bodies, &mut live, feature.id) {
                    Ok(taken) => taken,
                    Err(problem) => {
                        problems.push(problem);
                        continue;
                    }
                };
                let union = matches!(feature.op, FeatureOp::Union { .. });
                let solid = taken[1..].iter().fold(taken[0].1.solid.clone(), |joined, (_, body)| match union {
                    true => joined.union(&body.solid),
                    false => joined.intersect(&body.solid),
                });
                if solid.is_empty() {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.nothing_left") });
                    put_back(taken, &mut live);
                    continue;
                }
                let wires = taken.into_iter().flat_map(|(_, body)| body.wires).collect();
                live.push(Live { body: BodyRef::whole(feature.id), solid, wires });
            }
            FeatureOp::Subtract { targets, tools } => {
                if targets.is_empty() || tools.is_empty() {
                    problems.push(Problem::missing_body(feature.id));
                    continue;
                }
                // Taken together, so a body named as both is a problem.
                let operands: Vec<BodyRef> = targets.iter().chain(tools).copied().collect();
                let mut taken = match take(&operands, &mut live, feature.id) {
                    Ok(taken) => taken,
                    Err(problem) => {
                        problems.push(problem);
                        continue;
                    }
                };
                let tools = taken.split_off(targets.len());
                let solids: Vec<Solid> = taken
                    .iter()
                    .map(|(_, target)| tools.iter().fold(target.solid.clone(), |left, (_, tool)| left.subtract(&tool.solid)))
                    .collect();
                if solids.iter().any(Solid::is_empty) {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.nothing_left") });
                    put_back(taken.into_iter().chain(tools).collect(), &mut live);
                    continue;
                }
                let of = solids.len() as u32;
                let shared: Vec<&Live> = tools.iter().map(|(_, tool)| tool).collect();
                for (index, ((_, target), solid)) in taken.into_iter().zip(solids).enumerate() {
                    let (solid, copied) = own_copy(solid, &shared, feature.id, index as u32, &mut faces);
                    let wires = target.wires.into_iter().chain(copied).collect();
                    live.push(Live { body: BodyRef { feature: feature.id, index: index as u32, of }, solid, wires });
                }
            }
            FeatureOp::Mirror { targets, about, offset, keep_original } => {
                let Some(plane) = scope.plane(*about) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.no_mirror_plane") });
                    continue;
                };
                let taken = match take(targets, &mut live, feature.id) {
                    Ok(taken) => taken,
                    Err(problem) => {
                        problems.push(problem);
                        continue;
                    }
                };
                let normal = *plane.normal;
                let origin = plane.origin + normal * *offset;
                let of = taken.len() as u32;
                for (index, (_, body)) in taken.into_iter().enumerate() {
                    let reflected = body.solid.clone().mirrored(origin, normal).retagged(|tag| faces.copied(tag, feature.id, 1));
                    let reflections = body
                        .wires
                        .iter()
                        .map(|wire| wire.copied(feature.id, 1, wire.outline.clone().mirrored(origin, normal)));
                    let (solid, wires) = if *keep_original {
                        (body.solid.union(&reflected), body.wires.iter().cloned().chain(reflections).collect())
                    } else {
                        (reflected, reflections.collect())
                    };
                    live.push(Live { body: BodyRef { feature: feature.id, index: index as u32, of }, solid, wires });
                }
            }
            FeatureOp::Paint { targets, surface } => {
                let taken = match take(targets, &mut live, feature.id) {
                    Ok(taken) => taken,
                    Err(problem) => {
                        problems.push(problem);
                        continue;
                    }
                };
                let of = taken.len() as u32;
                for (index, (_, body)) in taken.into_iter().enumerate() {
                    let made = BodyRef { feature: feature.id, index: index as u32, of };
                    live.push(Live { body: made, solid: body.solid.painted(*surface), wires: body.wires });
                }
            }
            FeatureOp::Plane { def } => match def.resolve(feature.id, &scope) {
                Some(datum) => planes.push(datum),
                None => problems.push(Problem { feature: feature.id, message: get!("prop.problems.no_plane") }),
            },
            FeatureOp::Axis { def } => match def.resolve(feature.id, &scope) {
                Some(axis) => axes.push(axis),
                None => problems.push(Problem { feature: feature.id, message: get!("prop.problems.no_axis") }),
            },
            FeatureOp::Point { def } => match def.resolve(feature.id, &scope) {
                Some(point) => points.push(point),
                None => problems.push(Problem { feature: feature.id, message: get!("prop.problems.no_point") }),
            },
            FeatureOp::Sketch { on, sketch } => {
                match scope.plane(*on) {
                    Some(plane) => sketches.push(PlacedSketch {
                        id: feature.id,
                        plane,
                        sketch: sketch.clone(),
                        arrangement: arrange(sketch),
                    }),
                    None => problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.no_sketch_plane"),
                    }),
                }
            }
            FeatureOp::Pattern { targets, count, along } => {
                let Some(line) = along.resolve(&scope) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.missing_line") });
                    continue;
                };
                let taken = match take(targets, &mut live, feature.id) {
                    Ok(taken) => taken,
                    Err(problem) => {
                        problems.push(problem);
                        continue;
                    }
                };
                let count = (*count).max(1);
                let of = taken.len() as u32;
                for (made, (_, body)) in taken.into_iter().enumerate() {
                    // Unioned, not concatenated: overlapping copies must become one
                    // closed surface or a later boolean keeps the wrong half.
                    let solid = (1..count).fold(body.solid.clone(), |joined, index| {
                        let copy = body.solid.clone().transformed(along.copy(line, index, count));
                        joined.union(&copy.retagged(|tag| faces.copied(tag, feature.id, index)))
                    });
                    let mut wires = body.wires.clone();
                    for index in 1..count {
                        let moved = along.copy(line, index, count);
                        wires.extend(
                            body.wires
                                .iter()
                                .map(|wire| wire.copied(feature.id, index, wire.outline.clone().transformed(moved))),
                        );
                    }
                    live.push(Live { body: BodyRef { feature: feature.id, index: made as u32, of }, solid, wires });
                }
            }
        }
    }

    let pickables: Vec<Pickable> = live.iter().flat_map(|body| pickables(body.body.feature, &body.solid, &body.wires)).collect();
    let origin_half = origin_half_of(&live);
    planes.splice(0..0, OriginPlane::ALL.map(|which| Datum::origin(which, origin_half)));
    let (bodies, wires) = live.into_iter().map(|body| ((body.body, body.solid), (body.body, body.wires))).unzip();
    Evaluated {
        bodies,
        wires,
        pickables,
        problems,
        planes,
        sketches,
        axes,
        points,
        origin_half,
        faces,
        found: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if copies land on top of each other, including the last of a
    /// whole radial turn.
    /// Fails if a datum is resolved against the end of the list, or a
    /// pattern cannot turn about one.
    /// One of every feature, each naming features 100 and up and nothing
    /// else in that range, so a feature id in its text is a reference.
    fn one_of_every_referencing_feature() -> Vec<FeatureOp> {
        use crate::prop::reference::{CopyOf, Element, Source};
        use crate::prop::refs::{AnyRef, PointRef};
        let id = PropFeatureId;
        let geom = |element, feature| GeomRef { element, source: Source { feature: id(feature), copies: vec![] } };
        let copied = GeomRef {
            element: Element::Edge(3),
            source: Source { feature: id(111), copies: vec![CopyOf { by: id(112), index: 1 }] },
        };
        let surface = Surface::default();
        vec![
            FeatureOp::Extrude {
                profile: ProfileSource::Regions { sketch: id(100), regions: vec![SketchId(1)] },
                taper: 5.0,
                op: SweepOp::Cut,
                targets: vec![BodyRef::whole(id(137))],
                end: End::UpTo { to: AnyRef::Face { face: geom(Element::Face(2), 138) } },
                other: Some(End::UpTo { to: AnyRef::Plane(PlaneRef::Feature(id(139))) }),
                direction: Some(DirectionRef::Normal { plane: PlaneRef::Feature(id(140)) }),
                surface,
            },
            FeatureOp::Sweep { op: SweepOp::NewBody, targets: vec![],
                profile: ProfileSource::Regions { sketch: id(131), regions: vec![SketchId(1)] },
                orient: Orient::Frenet,
                twist: 15.0,
                scale: 0.5,
                path: PathRef {
                    curves: vec![
                        CurveRef::Sketch { sketch: id(132), entity: SketchId(4) },
                        CurveRef::Edge { edge: geom(Element::Edge(2), 133) },
                    ],
                },
                surface,
            },
            FeatureOp::Revolve { op: SweepOp::NewBody, targets: vec![],
                profile: ProfileSource::Regions { sketch: id(101), regions: vec![SketchId(1)] },
                degrees: 90.0,
                steps: 12,
                symmetric: false,
                axis: CurveRef::Datum { feature: id(102) },
                surface,
            },
            FeatureOp::Revolve { op: SweepOp::Join, targets: vec![BodyRef::whole(id(143))],
                profile: ProfileSource::Face { face: geom(Element::Face(4), 141) },
                degrees: 45.0,
                steps: 6,
                symmetric: true,
                axis: CurveRef::Datum { feature: id(142) },
                surface,
            },
            FeatureOp::Loft { op: SweepOp::NewBody, targets: vec![],
                closed: false,
                profiles: vec![
                    Profile { sketch: id(134), region: SketchId(1), start: None },
                    Profile { sketch: id(135), region: SketchId(2), start: Some(PointRef::Datum { feature: id(136) }) },
                ],
                surface,
            },
            FeatureOp::Subtract { targets: vec![BodyRef::whole(id(103))], tools: vec![BodyRef::whole(id(104))] },
            FeatureOp::Mirror { targets: vec![BodyRef::whole(id(105))], offset: 0.0, keep_original: true, about: PlaneRef::Feature(id(106)) },
            FeatureOp::Paint { targets: vec![BodyRef::whole(id(107))], surface },
            FeatureOp::Pattern {
                targets: vec![BodyRef::whole(id(108))],
                count: 3,
                along: PatternKind::Linear { spacing: 0.05, direction: DirectionRef::Normal { plane: PlaneRef::Feature(id(109)) } },
            },
            FeatureOp::Pattern { targets: vec![BodyRef::whole(id(110))], count: 3, along: PatternKind::Radial { degrees: 90.0, axis: CurveRef::Edge { edge: copied } } },
            FeatureOp::Plane {
                def: PlaneDef::Through {
                    through: vec![
                        AnyRef::Point(PointRef::Corner { corner: geom(Element::Point(2), 113) }),
                        AnyRef::Curve(CurveRef::Sketch { sketch: id(114), entity: SketchId(3) }),
                        AnyRef::Face { face: geom(Element::Face(1), 115) },
                    ],
                },
            },
            FeatureOp::Plane { def: PlaneDef::Offset { distance: 0.05, from: PlaneRef::Feature(id(116)) } },
            FeatureOp::Plane {
                def: PlaneDef::Angle { degrees: 30.0, from: PlaneRef::Feature(id(117)), about: CurveRef::Datum { feature: id(118) } },
            },
            FeatureOp::Plane { def: PlaneDef::Midplane { a: PlaneRef::Feature(id(119)), b: PlaneRef::Feature(id(120)) } },
            FeatureOp::Axis {
                def: AxisDef::Through { a: PointRef::Datum { feature: id(121) }, b: PointRef::Sketch { sketch: id(122), point: SketchId(2) } },
            },
            FeatureOp::Axis { def: AxisDef::Planes { a: PlaneRef::Feature(id(123)), b: PlaneRef::Feature(id(124)) } },
            FeatureOp::Axis {
                def: AxisDef::Normal { plane: PlaneRef::Feature(id(125)), point: PointRef::Corner { corner: geom(Element::Point(0), 126) } },
            },
            FeatureOp::Point { def: PointDef::Midpoint { curve: CurveRef::Datum { feature: id(127) } } },
            FeatureOp::Point {
                def: PointDef::Intersection { curve: CurveRef::Edge { edge: geom(Element::Edge(1), 128) }, plane: PlaneRef::Feature(id(129)) },
            },
            FeatureOp::Sketch { on: PlaneRef::Feature(id(130)), sketch: Sketch::default() },
        ]
    }

    /// Fails if a reference is left out of [`FeatureOp::fields`]: the feature
    /// would not be pinned below it, nor told of its deletion.
    #[test]
    fn every_reference_is_a_field() {
        #[derive(Serialize)]
        struct Wrapper<'a> {
            op: &'a FeatureOp,
        }
        for op in one_of_every_referencing_feature() {
            let text = toml::to_string(&Wrapper { op: &op }).unwrap();
            let mut named: Vec<PropFeatureId> = text
                .split(|c: char| !c.is_ascii_digit() && c != '.')
                .filter_map(|word| word.parse::<u32>().ok())
                .filter(|n| (100..200).contains(n))
                .map(PropFeatureId)
                .collect();
            named.sort();
            named.dedup();
            assert_eq!(op.depends_on(), named, "{text}");
        }
    }

    /// Fails if [`FeatureOp::set_field`] and [`FeatureOp::fields`] disagree
    /// about what an index is.
    #[test]
    fn every_field_writes_back_what_it_reads() {
        use crate::prop::refs::AnyRef;
        use crate::prop::slot::Kind;
        for op in one_of_every_referencing_feature() {
            for (index, field) in op.fields().iter().enumerate() {
                let value = match (field.slot.kind, &field.refs[..]) {
                    (Kind::Construct(_), _) => Resolved::Op(op.clone()),
                    (Kind::Bound, [to]) => Resolved::Bound(to.clone()),
                    (Kind::Profiles, refs) => Resolved::Profiles(
                        refs.iter()
                            .map(|r| match r {
                                AnyRef::Regions { sketch, entries } => (*sketch, entries[0]),
                                other => panic!("a profile is a region, not {other:?}"),
                            })
                            .collect(),
                    ),
                    (Kind::Bodies(_), refs) => Resolved::Bodies(
                        refs.iter()
                            .map(|r| match r {
                                AnyRef::Body { body } => *body,
                                other => panic!("an operand is a body, not {other:?}"),
                            })
                            .collect(),
                    ),
                    (_, [AnyRef::Regions { sketch, entries }]) => Resolved::Regions { sketch: *sketch, entries: entries.clone() },
                    (Kind::Regions, [AnyRef::Face { face }]) => Resolved::ProfileFace(face.clone()),
                    (_, [AnyRef::Curve(curve)]) => Resolved::Curve(curve.clone()),
                    (_, [AnyRef::Direction(direction)]) => Resolved::Direction(direction.clone()),
                    (_, [AnyRef::Plane(on)]) => Resolved::Plane(*on),
                    (_, [AnyRef::Path(path)]) => Resolved::Path(path.clone()),
                    (_, [AnyRef::Point(point)]) => Resolved::Point(point.clone()),
                    // An unset field is not a value to write back.
                    (_, []) => continue,
                    other => panic!("field {index} of {op:?} holds {other:?}"),
                };
                let mut written = op.clone();
                assert!(written.set_field(index, value), "field {index} of {op:?}");
                assert_eq!(written, op);
            }
        }
    }

    #[test]
    fn datum_axes_and_points_stand_where_they_were_made() {
        use crate::prop::refs::PointRef;
        let cube = PropFeature::new(
            PropFeatureId(0),
            FeatureOp::Primitive {
                shape: Shape::Box { size: [0.2; 3] },
                placement: Placement::default(),
                surface: Surface::default(),
            },
        );
        let corner = |i| PointRef::Corner {
            corner: GeomRef { element: crate::prop::reference::Element::Point(i), source: crate::prop::reference::Source { feature: PropFeatureId(0), copies: vec![] } },
        };
        let axis = PropFeature::new(PropFeatureId(1), FeatureOp::Axis { def: AxisDef::Through { a: corner(0), b: corner(4) } });
        let middle = PropFeature::new(
            PropFeatureId(2),
            FeatureOp::Point { def: PointDef::Midpoint { curve: CurveRef::Datum { feature: PropFeatureId(1) } } },
        );
        let turned = PropFeature::new(
            PropFeatureId(3),
            FeatureOp::Pattern {
                targets: vec![BodyRef::whole(PropFeatureId(0))],
                count: 4,
                along: PatternKind::Radial { degrees: 360.0, axis: CurveRef::Datum { feature: PropFeatureId(1) } },
            },
        );
        let evaluated = evaluate(&[cube, axis.clone(), middle, turned]);
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        assert_eq!(evaluated.axes.len(), 1);
        let (a, b) = (evaluated.axes[0].a, evaluated.axes[0].b);
        let point = evaluated.points[0].at;
        assert!((point - (a + b) / 2.0).length() < 1e-6, "the middle of the drawn axis is the middle of its points");
        assert_eq!(evaluated.bodies.len(), 1);

        let before_its_cube = evaluate(&[axis]);
        assert!(before_its_cube.axes.is_empty());
        assert_eq!(before_its_cube.problems.len(), 1);
    }

    #[test]
    fn a_pattern_makes_that_many_separate_copies() {
        let cube = primitive(0, Shape::Box { size: [0.1, 0.1, 0.1] }, Vec3::new(0.3, 0.0, 0.0));
        let one = evaluate(std::slice::from_ref(&cube)).bodies[0].1.volume();
        let pattern = |count: u32, along: PatternKind| {
            let evaluated = evaluate(&[
                cube.clone(),
                PropFeature::new(PropFeatureId(1), FeatureOp::Pattern { targets: vec![BodyRef::whole(PropFeatureId(0))], count, along }),
            ]);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            assert_eq!(evaluated.bodies.len(), 1, "the copies are one body");
            evaluated.bodies[0].1.volume()
        };

        let z = DirectionRef::Along { curve: CurveRef::Axis { axis: Axis::Z } };
        let row = pattern(3, PatternKind::Linear { spacing: 0.2, direction: z.clone() });
        assert!((row - 3.0 * one).abs() < 1e-6, "{row} vs {}", 3.0 * one);
        let round = pattern(4, PatternKind::Radial { degrees: 360.0, axis: CurveRef::Axis { axis: Axis::Y } });
        assert!((round - 4.0 * one).abs() < 1e-6, "{round} vs {}", 4.0 * one);
        let alone = pattern(1, PatternKind::Linear { spacing: 0.2, direction: z });
        assert!((alone - one).abs() < 1e-9);
    }

    /// Fails if picking uses boxes: the ray must pass down the bore.
    #[test]
    fn a_ray_picks_the_nearest_face_not_the_nearest_box() {
        let cube = |id: u32, z: f32| primitive(id, Shape::Box { size: [0.1, 0.1, 0.1] }, Vec3::new(0.0, 0.0, z));
        let near = cube(0, 0.0);
        let far = cube(1, -0.5);
        let evaluated = evaluate(&[near.clone(), far.clone()]);
        let from_front = evaluated.body_under(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z);
        assert_eq!(from_front, Some(BodyRef::whole(PropFeatureId(0))));
        let beside = evaluated.body_under(Vec3::new(1.0, 0.0, 1.0), Vec3::NEG_Z);
        assert_eq!(beside, None);

        let bore = primitive(2, Shape::Box { size: [0.04, 0.04, 0.3] }, Vec3::ZERO);
        let drilled = PropFeature::new(
            PropFeatureId(3),
            FeatureOp::Subtract { targets: vec![BodyRef::whole(PropFeatureId(0))], tools: vec![BodyRef::whole(PropFeatureId(2))] },
        );
        let evaluated = evaluate(&[near, far, bore, drilled]);
        assert_eq!(
            evaluated.body_under(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z),
            Some(BodyRef::whole(PropFeatureId(1))),
            "the ray goes down the hole",
        );
        assert_eq!(
            evaluated.body_under(Vec3::new(0.04, 0.0, 1.0), Vec3::NEG_Z),
            Some(BodyRef::whole(PropFeatureId(3))),
            "beside the hole is the drilled body, under its own id",
        );
    }

    fn small_cube(id: u32, x: f32) -> PropFeature {
        primitive(id, Shape::Box { size: [0.1; 3] }, Vec3::new(x, 0.0, 0.0))
    }

    #[test]
    fn one_tool_cuts_through_three_targets_as_three_bodies() {
        let rod = primitive(3, Shape::Box { size: [3.0, 0.02, 0.02] }, Vec3::X);
        let cut = PropFeature::new(
            PropFeatureId(4),
            FeatureOp::Subtract {
                targets: (0..3).map(|id| BodyRef::whole(PropFeatureId(id))).collect(),
                tools: vec![BodyRef::whole(PropFeatureId(3))],
            },
        );
        let evaluated = evaluate(&[small_cube(0, 0.0), small_cube(1, 1.0), small_cube(2, 2.0), rod, cut]);
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        assert_eq!(evaluated.bodies.len(), 3);
        for (index, (body, solid)) in evaluated.bodies.iter().enumerate() {
            assert_eq!(*body, BodyRef { feature: PropFeatureId(4), index: index as u32, of: 3 });
            assert!((solid.volume() - (0.001 - 0.1 * 0.02 * 0.02)).abs() < 1e-7, "{}", solid.volume());
        }
    }

    #[test]
    fn a_union_of_four_overlapping_cubes_is_one_body() {
        let cubes: Vec<PropFeature> = (0..4).map(|id| small_cube(id, id as f32 * 0.05)).collect();
        let union = PropFeature::new(
            PropFeatureId(4),
            FeatureOp::Union { bodies: (0..4).map(|id| BodyRef::whole(PropFeatureId(id))).collect() },
        );
        let evaluated = evaluate(&[cubes, vec![union]].concat());
        assert_eq!(evaluated.bodies.len(), 1);
        assert_eq!(evaluated.bodies[0].0, BodyRef::whole(PropFeatureId(4)));
        assert!((evaluated.bodies[0].1.volume() - 0.25 * 0.1 * 0.1).abs() < 1e-7);
    }

    #[test]
    fn a_mirror_of_two_bodies_makes_two() {
        let mirror = PropFeature::new(
            PropFeatureId(2),
            FeatureOp::Mirror {
                offset: 0.0,
                keep_original: false,
                targets: vec![BodyRef::whole(PropFeatureId(0)), BodyRef::whole(PropFeatureId(1))],
                about: PlaneRef::Origin(OriginPlane::Yz),
            },
        );
        let evaluated = evaluate(&[small_cube(0, 0.2), small_cube(1, 0.5), mirror]);
        assert_eq!(evaluated.bodies.len(), 2);
        for (index, (body, solid)) in evaluated.bodies.iter().enumerate() {
            assert_eq!(*body, BodyRef { feature: PropFeatureId(2), index: index as u32, of: 2 });
            assert!((solid.volume() - 0.001).abs() < 1e-7);
            assert!(solid.bounds().unwrap().1.x < 0.0, "copied across the plane");
        }
    }

    #[test]
    fn a_body_of_a_feature_that_makes_one_is_written_as_its_bare_id() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Holder {
            target: BodyRef,
        }
        let whole = Holder { target: BodyRef::whole(PropFeatureId(3)) };
        let text = toml::to_string(&whole).unwrap();
        assert_eq!(text.trim(), "target = 3");
        assert_eq!(toml::from_str::<Holder>(&text).unwrap(), whole);
        let second = Holder { target: BodyRef { feature: PropFeatureId(3), index: 1, of: 2 } };
        let text = toml::to_string(&second).unwrap();
        assert!(text.contains("index = 1") && text.contains("of = 2"), "{text}");
        assert_eq!(toml::from_str::<Holder>(&text).unwrap(), second);
    }

    #[test]
    fn a_body_past_the_count_its_feature_made_does_not_load() {
        #[derive(Deserialize, Debug)]
        #[allow(dead_code)]
        struct Holder {
            target: BodyRef,
        }
        for text in ["target = { feature = 3, index = 2, of = 2 }", "target = { feature = 3, index = 0, of = 0 }"] {
            let error = toml::from_str::<Holder>(text).unwrap_err().to_string();
            assert!(error.contains("past the last"), "{text}: {error}");
        }
        let error = toml::from_str::<Holder>("target = { feature = 3, idx = 1, of = 2 }").unwrap_err().to_string();
        assert!(error.contains("{ feature, index, of }"), "{error}");
    }

    #[test]
    fn a_body_named_under_another_count_is_a_problem_and_is_left_standing() {
        let cube = primitive(0, Shape::Box { size: [0.1, 0.1, 0.1] }, Vec3::ZERO);
        let paint = PropFeature::new(
            PropFeatureId(1),
            FeatureOp::Paint { targets: vec![BodyRef { feature: PropFeatureId(0), index: 0, of: 2 }], surface: Surface::default() },
        );
        let evaluated = evaluate(&[cube, paint]);
        assert_eq!(evaluated.problems.len(), 1);
        assert_eq!(evaluated.problems[0].feature, PropFeatureId(1));
        assert_eq!(evaluated.problems[0].message, get!("prop.problems.renumbered_body"));
        assert_eq!(evaluated.bodies[0].0, BodyRef::whole(PropFeatureId(0)), "the cube was not eaten");
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

    /// Fails as a barrel drawn with its own bore inside it.
    #[test]
    fn a_boolean_consumes_both_of_its_operands() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            unit_box(1, Vec3::new(0.5, 0.0, 0.0)),
            PropFeature::new(
                PropFeatureId(2),
                FeatureOp::Subtract { targets: vec![BodyRef::whole(PropFeatureId(0))], tools: vec![BodyRef::whole(PropFeatureId(1))] },
            ),
        ];
        let result = evaluate(&features);
        assert!(result.problems.is_empty(), "{:?}", result.problems);
        assert_eq!(result.bodies.len(), 1, "the operands were left lying around");
        assert_eq!(result.bodies[0].0.feature, PropFeatureId(2), "the result took the wrong id");
        assert!((result.bodies[0].1.volume() - 0.5).abs() < 1e-3);
    }

    /// Fails if a dangling reference blanks the viewport or eats its target.
    #[test]
    fn a_dangling_reference_is_a_note_rather_than_a_blank_prop() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            PropFeature::new(
                PropFeatureId(2),
                FeatureOp::Subtract { targets: vec![BodyRef::whole(PropFeatureId(0))], tools: vec![BodyRef::whole(PropFeatureId(9))] },
            ),
        ];
        let result = evaluate(&features);
        assert_eq!(result.problems.len(), 1);
        assert_eq!(result.problems[0].feature, PropFeatureId(2));
        assert_eq!(result.bodies.len(), 1, "the surviving body went missing too");
        assert_eq!(result.bodies[0].0.feature, PropFeatureId(0), "the target was eaten by a failed boolean");
    }

    /// A feature that fails must put back whatever it took.
    #[test]
    fn a_boolean_pointed_at_itself_leaves_the_body_alone() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            PropFeature::new(
                PropFeatureId(1),
                FeatureOp::Union { bodies: vec![BodyRef::whole(PropFeatureId(0)), BodyRef::whole(PropFeatureId(0))] },
            ),
        ];
        let result = evaluate(&features);
        assert_eq!(result.problems.len(), 1);
        assert_eq!(result.bodies.len(), 1);
        assert_eq!(result.bodies[0].0.feature, PropFeatureId(0));
    }

    #[test]
    fn disabling_a_feature_removes_its_body() {
        let mut features = vec![unit_box(0, Vec3::ZERO), unit_box(1, Vec3::new(3.0, 0.0, 0.0))];
        features[0].enabled = false;
        let result = evaluate(&features);
        assert_eq!(result.bodies.len(), 1);
        assert_eq!(result.bodies[0].0.feature, PropFeatureId(1));
    }

    #[test]
    fn a_kept_mirror_doubles_the_body() {
        let features = vec![
            unit_box(0, Vec3::new(1.0, 0.0, 0.0)),
            PropFeature::new(
                PropFeatureId(1),
                FeatureOp::Mirror {
                    targets: vec![BodyRef::whole(PropFeatureId(0))],
                    about: crate::prop::plane::PlaneRef::Origin(crate::prop::plane::OriginPlane::Yz),
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

    mod extrude {
        use super::super::*;
        use crate::prop::document::{parse, to_text, PropDoc};
        use crate::prop::plane::OriginPlane;
        use crate::prop::region::{arrange, Region, RegionRef};
        use crate::prop::sketch::{Sketch, SketchId};

        fn polygon(sketch: &mut Sketch, corners: &[[f32; 2]]) -> Vec<SketchId> {
            let points: Vec<SketchId> = corners.iter().map(|c| sketch.add_point(Vec2::from_array(*c))).collect();
            (0..points.len()).map(|i| sketch.add_line(points[i], points[(i + 1) % points.len()])).collect()
        }

        fn square(sketch: &mut Sketch, min: f32, max: f32) -> Vec<SketchId> {
            polygon(sketch, &[[min, min], [max, min], [max, max], [min, max]])
        }

        /// A sketch on `on`, then an extrude of the regions `pick` chooses.
        fn swept(
            on: OriginPlane,
            sketch: Sketch,
            distance: f32,
            symmetric: bool,
            pick: impl Fn(&[Region]) -> Vec<RegionRef>,
        ) -> (Vec<PropFeature>, Evaluated) {
            let mut sketch = sketch;
            let picked = pick(&arrange(&sketch).regions);
            let regions = adopt(&mut sketch, picked);
            let features = vec![
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(on), sketch }),
                PropFeature::new(
                    PropFeatureId(1),
                    FeatureOp::Extrude {
                        taper: 0.0,
                        op: SweepOp::NewBody,
                        targets: vec![],
                        // Half each way, as both sides at half the distance.
                        end: End::Distance { distance: if symmetric { distance / 2.0 } else { distance } },
                        other: symmetric.then_some(End::Distance { distance: distance / 2.0 }),
                        direction: None,
                        surface: Surface::default(),
                        profile: ProfileSource::Regions { sketch: PropFeatureId(0), regions },
                    },
                ),
            ];
            let evaluated = evaluate(&features);
            (features, evaluated)
        }

        pub(super) fn adopt(sketch: &mut Sketch, names: Vec<RegionRef>) -> Vec<SketchId> {
            let arrangement = arrange(sketch);
            names.iter().map(|name| sketch.adopt_region(arrangement.find(name).unwrap())).collect()
        }

        pub(super) fn every(regions: &[Region]) -> Vec<RegionRef> {
            regions.iter().map(|region| region.name.clone()).collect()
        }

        #[test]
        fn a_square_extrudes_to_a_box_along_the_planes_normal() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (_, evaluated) = swept(OriginPlane::Xy, sketch.clone(), 0.1, false, every);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let (_, solid) = &evaluated.bodies[0];
            assert!((solid.volume() - 0.04 * 0.04 * 0.1).abs() < 1e-8, "{}", solid.volume());
            let (min, max) = solid.bounds().unwrap();
            assert!(min.z.abs() < 1e-6 && (max.z - 0.1).abs() < 1e-6, "{min} {max}");

            let (_, symmetric) = swept(OriginPlane::Xy, sketch.clone(), 0.1, true, every);
            let (min, max) = symmetric.bodies[0].1.bounds().unwrap();
            assert!((min.z + 0.05).abs() < 1e-6 && (max.z - 0.05).abs() < 1e-6, "{min} {max}");

            let (_, up) = swept(OriginPlane::Xz, sketch, 0.1, false, every);
            let (min, max) = up.bodies[0].1.bounds().unwrap();
            assert!(min.y.abs() < 1e-6 && (max.y - 0.1).abs() < 1e-6, "a floor sketch goes up: {min} {max}");
        }

        fn sketch_in(features: &mut [PropFeature]) -> &mut Sketch {
            match &mut features[0].op {
                FeatureOp::Sketch { sketch, .. } => sketch,
                _ => unreachable!(),
            }
        }

        /// Fails if drawing across a swept region changes, or loses, the body.
        #[test]
        fn a_line_drawn_across_a_swept_region_leaves_the_body_alone() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (mut features, before) = swept(OriginPlane::Xy, sketch, 0.1, false, every);
            let drawn = sketch_in(&mut features);
            let (a, b) = (drawn.add_point(Vec2::new(0.02, -0.01)), drawn.add_point(Vec2::new(0.02, 0.05)));
            drawn.add_line(a, b);

            let after = evaluate(&features);
            assert!(after.problems.is_empty(), "{:?}", after.problems);
            let (was, is) = (before.bodies[0].1.volume(), after.bodies[0].1.volume());
            assert!((was - is).abs() < 1e-9, "{was} vs {is}");
        }

        /// Fails if a merge is guessed at rather than reported by name.
        #[test]
        fn a_region_merged_away_is_a_problem_naming_its_label() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (a, b) = (sketch.add_point(Vec2::new(0.02, 0.0)), sketch.add_point(Vec2::new(0.02, 0.04)));
            let divider = sketch.add_line(a, b);
            let left = |regions: &[Region]| {
                vec![regions.iter().find(|r| r.contains(Vec2::new(0.01, 0.02))).unwrap().name.clone()]
            };
            let (mut features, _) = swept(OriginPlane::Xy, sketch, 0.1, false, left);
            let drawn = sketch_in(&mut features);
            drawn.regions[0].label = "Grip".into();
            drawn.remove(&[divider]);

            let after = evaluate(&features);
            assert!(after.bodies.is_empty());
            assert_eq!(after.problems.len(), 1);
            assert!(after.problems[0].message.contains("Grip"), "{}", after.problems[0].message);
        }

        /// Fails if two sweeps of one region get an entry each, so that
        /// re-picking it for one leaves the other behind.
        #[test]
        fn sweeps_of_one_region_share_its_entry() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let region = arrange(&sketch).regions[0].clone();
            let first = sketch.adopt_region(&region);
            assert_eq!(sketch.adopt_region(&region), first);
            assert_eq!(sketch.regions.len(), 1);
        }

        #[test]
        fn removing_a_sweep_forgets_its_region_unless_it_was_labeled() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            square(&mut sketch, 0.1, 0.14);
            let regions = arrange(&sketch).regions;
            let mut doc = PropDoc::new("test");
            let named = sketch.adopt_region(&regions[0]);
            sketch.adopt_region(&regions[1]);
            sketch.regions[0].label = "Kept".into();
            let on = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch });
            let ids = match &doc.features[0].op {
                FeatureOp::Sketch { sketch, .. } => sketch.regions.iter().map(|r| r.id).collect(),
                _ => unreachable!(),
            };
            let sweep = doc.push(FeatureOp::Extrude {
                profile: ProfileSource::Regions { sketch: on, regions: ids },
                taper: 0.0,
                op: SweepOp::NewBody,
                targets: vec![],
                end: End::Distance { distance: 0.1 },
                other: None,
                direction: None,
                surface: Surface::default(),
            });

            doc.remove(sweep);
            let FeatureOp::Sketch { sketch, .. } = &doc.features[0].op else { unreachable!() };
            assert_eq!(sketch.regions.iter().map(|r| r.id).collect::<Vec<_>>(), vec![named]);
        }

        #[test]
        fn a_hole_is_left_out() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            square(&mut sketch, 0.01, 0.02);
            let biggest = |regions: &[Region]| {
                let ring = regions.iter().max_by(|a, b| a.area().total_cmp(&b.area())).unwrap();
                vec![ring.name.clone()]
            };
            let (_, evaluated) = swept(OriginPlane::Xy, sketch, 0.1, false, biggest);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            let expected = (0.04 * 0.04 - 0.01 * 0.01) * 0.1;
            assert!((volume - expected).abs() < 1e-8, "{volume} vs {expected}");
        }

        /// Fails if regions sharing a line are gathered rather than joined.
        #[test]
        fn regions_side_by_side_become_one_body() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.02);
            square(&mut sketch, 0.01, 0.03);
            let (_, evaluated) = swept(OriginPlane::Xy, sketch, 0.05, false, every);
            assert_eq!(evaluated.bodies.len(), 1);
            let area = 2.0 * 0.02 * 0.02 - 0.01 * 0.01;
            assert!((evaluated.bodies[0].1.volume() - area * 0.05).abs() < 1e-8);
        }

        /// The extruded body is named like any other: its corners can be
        /// picked, and its far cap faces out along the plane's normal.
        #[test]
        fn an_extruded_body_has_named_corners_and_faces() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (_, evaluated) = swept(OriginPlane::Xy, sketch, 0.1, false, every);
            let corners = evaluated.pickables.iter().filter(|p| matches!(p.at, Located::Point(_))).count();
            assert_eq!(corners, 8);
            let far_cap = GeomRef {
                element: crate::prop::reference::Element::Face(1),
                source: crate::prop::reference::Source { feature: PropFeatureId(1), copies: vec![] },
            };
            let Some(face @ Located::Face { normal, .. }) = evaluated.locate(&far_cap) else { panic!() };
            assert!(normal.distance(Vec3::Z) < 1e-6);
            assert!(face.points().iter().all(|p| (p.z - 0.1).abs() < 1e-6));
        }

        #[test]
        fn a_region_is_picked_where_it_is_drawn() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (_, evaluated) = swept(OriginPlane::Xy, sketch, -0.1, false, every);
            // Down onto the sketch, which sits on the extrusion's top cap.
            let (id, region) = evaluated.region_under(Ray3d::new(Vec3::new(0.02, 0.02, 1.0), Dir3::NEG_Z)).unwrap();
            assert_eq!(id, PropFeatureId(0));
            assert!((region.area() - 0.0016).abs() < 1e-8);
            // From underneath, the body is in the way.
            assert!(evaluated.region_under(Ray3d::new(Vec3::new(0.02, 0.02, -1.0), Dir3::Z)).is_none());
        }

        #[test]
        fn an_extrude_survives_the_file() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (features, _) = swept(OriginPlane::Xy, sketch, 0.1, true, every);
            let mut doc = PropDoc::new("test");
            for feature in features {
                doc.push(feature.op);
            }
            let text = to_text(&doc).unwrap();
            assert_eq!(parse(&text).unwrap(), doc, "{text}");
        }

        /// A circle swept is a cylinder with one side face, whose rims are
        /// each one seam.
        #[test]
        fn an_extruded_circle_is_one_side_face_with_round_rims() {
            let mut sketch = Sketch::default();
            let center = sketch.add_point(Vec2::ZERO);
            sketch.add_circle(center, 0.03);
            let (_, evaluated) = swept(OriginPlane::Xy, sketch, 0.1, false, every);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let wire = &evaluated.wires[0].1[0];
            assert_eq!(wire.outline.faces.len(), 3, "two caps and one side");
            let volume = evaluated.bodies[0].1.volume();
            let round = std::f32::consts::PI * 0.03 * 0.03 * 0.1;
            assert!(volume < round && volume > round * 0.97, "{volume} against {round}");
            let rims: Vec<_> = evaluated.seams().iter().filter(|seam| seam.offered && seam.closed).collect();
            assert_eq!(rims.len(), 2);
        }

        /// A D: a line and an arc, swept, make a flat side and a curved one.
        #[test]
        fn a_line_and_an_arc_sweep_a_face_each() {
            let mut sketch = Sketch::default();
            let center = sketch.add_point(Vec2::ZERO);
            let a = sketch.add_point(Vec2::new(0.0, -0.03));
            let b = sketch.add_point(Vec2::new(0.0, 0.03));
            sketch.add_line(b, a);
            sketch.add_arc(center, a, b, false);
            let (_, evaluated) = swept(OriginPlane::Xy, sketch, 0.05, false, every);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let outline = &evaluated.wires[0].1[0].outline;
            assert_eq!(outline.faces.len(), 4);
            let flat = (0..4).filter(|&i| outline.locate(crate::prop::reference::Element::Face(i)).is_some_and(|face| face.is_flat())).count();
            assert_eq!(flat, 3, "the caps and the line's side");
        }

        #[test]
        fn every_curve_survives_the_file() {
            let mut sketch = Sketch::default();
            let p: Vec<SketchId> = [[0.0, 0.0], [0.03, 0.0], [0.0, 0.03], [0.05, 0.02], [0.07, 0.05]]
                .iter()
                .map(|at| sketch.add_point(Vec2::from_array(*at)))
                .collect();
            sketch.add_arc(p[0], p[1], p[2], true);
            sketch.add_circle(p[3], 0.0125);
            sketch.add_bezier(vec![p[1], p[3], p[4]]);
            sketch.add_nurbs(vec![p[0], p[1], p[3], p[4]], 3);
            let mut doc = PropDoc::new("test");
            doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch });
            let text = to_text(&doc).unwrap();
            assert_eq!(parse(&text).unwrap(), doc, "{text}");
        }

        #[test]
        fn an_arcs_end_stays_on_its_circle() {
            let mut sketch = Sketch::default();
            let center = sketch.add_point(Vec2::ZERO);
            let start = sketch.add_point(Vec2::new(0.04, 0.0));
            let end = sketch.add_point(Vec2::new(0.0, 0.01));
            sketch.add_arc(center, start, end, false);
            assert!((sketch.point(end).unwrap().length() - 0.04).abs() < 1e-6, "put on the circle when made");
            sketch.move_point(start, Vec2::new(0.06, 0.0));
            assert!((sketch.point(end).unwrap().length() - 0.06).abs() < 1e-6, "and kept there");
        }
    }

    mod sweep {
        use super::super::*;
        use super::extrude::{adopt, every};
        use crate::prop::plane::OriginPlane;
        use crate::prop::region::arrange;
        use crate::prop::sketch::Sketch;

        /// A square, side `side`, centered on the XY plane's origin.
        fn square(side: f32) -> Sketch {
            let mut sketch = Sketch::default();
            let h = side / 2.0;
            let corners: Vec<SketchId> =
                [[-h, -h], [h, -h], [h, h], [-h, h]].iter().map(|at| sketch.add_point(Vec2::from_array(*at))).collect();
            for i in 0..4 {
                sketch.add_line(corners[i], corners[(i + 1) % 4]);
            }
            sketch
        }

        /// The square swept along whatever `path` draws on the XZ plane.
        fn swept(side: f32, path: Sketch, orient: Orient) -> Evaluated {
            let mut profile = square(side);
            let names = every(&arrange(&profile).regions);
            let regions = adopt(&mut profile, names);
            let curves: Vec<CurveRef> =
                path.entities.iter().map(|entity| CurveRef::Sketch { sketch: PropFeatureId(1), entity: entity.id() }).collect();
            evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch: profile }),
                PropFeature::new(PropFeatureId(1), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xz), sketch: path }),
                PropFeature::new(
                    PropFeatureId(2),
                    FeatureOp::Sweep { op: SweepOp::NewBody, targets: vec![],
                        profile: ProfileSource::Regions { sketch: PropFeatureId(0), regions },
                        orient,
                        twist: 0.0,
                        scale: 1.0,
                        path: PathRef { curves },
                        surface: Surface::default(),
                    },
                ),
            ])
        }

        /// The XZ plane's `y` runs along the prop's `-Z`; its `x` along `X`.
        #[test]
        fn a_straight_sweep_is_an_extrusion() {
            let mut path = Sketch::default();
            let (a, b) = (path.add_point(Vec2::ZERO), path.add_point(Vec2::new(0.0, -0.2)));
            path.add_line(a, b);
            let evaluated = swept(0.02, path, Orient::Minimal);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            assert!((volume - 0.02 * 0.02 * 0.2).abs() < 1e-9, "{volume}");
        }

        /// Pappus: a profile swept round a circle of radius R encloses its
        /// area times the distance its middle travels.
        #[test]
        fn a_sweep_round_a_quarter_arc_holds_what_pappus_says() {
            let mut path = Sketch::default();
            let center = path.add_point(Vec2::new(0.1, 0.0));
            let start = path.add_point(Vec2::ZERO);
            let end = path.add_point(Vec2::new(0.1, -0.1));
            // Counter-clockwise from the leftmost point is the quarter below.
            path.add_arc(center, start, end, false);
            let evaluated = swept(0.02, path, Orient::Minimal);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            let pappus = 0.02 * 0.02 * 0.1 * std::f32::consts::FRAC_PI_2;
            assert!((volume - pappus).abs() < pappus * 0.01, "{volume} against {pappus}");
            let faces = &evaluated.wires[0].1[0].outline.faces;
            assert_eq!(faces.len(), 6, "two caps and four sides, each one face along the arc");
        }

        #[test]
        fn a_sweep_round_a_closed_circle_is_a_ring() {
            let mut path = Sketch::default();
            let center = path.add_point(Vec2::new(0.08, 0.0));
            path.add_circle(center, 0.08);
            let evaluated = swept(0.02, path, Orient::Minimal);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            let pappus = 0.02 * 0.02 * std::f32::consts::TAU * 0.08;
            assert!((volume - pappus).abs() < pappus * 0.01, "{volume} against {pappus}");
            assert_eq!(evaluated.wires[0].1[0].outline.faces.len(), 4, "no caps");
        }

        #[test]
        fn a_path_that_misses_the_plane_is_a_problem() {
            let mut path = Sketch::default();
            let (a, b) = (path.add_point(Vec2::new(0.0, -0.05)), path.add_point(Vec2::new(0.0, -0.2)));
            path.add_line(a, b);
            let evaluated = swept(0.02, path, Orient::Minimal);
            assert_eq!(evaluated.problems.len(), 1);
            assert!(evaluated.bodies.is_empty());
        }
    }

    mod revolve {
        use super::super::*;
        use super::extrude::{adopt, every};
        use crate::prop::document::{parse, to_text, PropDoc};
        use crate::prop::plane::OriginPlane;
        use crate::prop::region::arrange;
        use crate::prop::sketch::{Sketch, SketchId};

        /// A rectangle `r0..r1` across and `0..h` up, and a construction axis
        /// along the plane's `y` at `axis_x`.
        fn ring(r0: f32, r1: f32, h: f32, axis_x: f32) -> (Sketch, SketchId) {
            let mut sketch = Sketch::default();
            let corners = [[r0, 0.0], [r1, 0.0], [r1, h], [r0, h]];
            let points: Vec<SketchId> = corners.iter().map(|c| sketch.add_point(Vec2::from_array(*c))).collect();
            for i in 0..4 {
                sketch.add_line(points[i], points[(i + 1) % 4]);
            }
            let (a, b) = (sketch.add_point(Vec2::new(axis_x, -0.1)), sketch.add_point(Vec2::new(axis_x, 0.1)));
            let axis = sketch.add_line(a, b);
            sketch.set_construction(axis, true);
            (sketch, axis)
        }

        fn turned(on: OriginPlane, mut sketch: Sketch, axis: SketchId, degrees: f32, steps: u32, symmetric: bool) -> Evaluated {
            let names = every(&arrange(&sketch).regions);
            let regions = adopt(&mut sketch, names);
            evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(on), sketch }),
                PropFeature::new(
                    PropFeatureId(1),
                    FeatureOp::Revolve { op: SweepOp::NewBody, targets: vec![],
                        axis: CurveRef::Sketch { sketch: PropFeatureId(0), entity: axis },
                        degrees,
                        steps,
                        symmetric,
                        surface: Surface::default(),
                        profile: ProfileSource::Regions { sketch: PropFeatureId(0), regions },
                    },
                ),
            ])
        }

        /// The area of a regular `n`-gon with corners `r` from its middle.
        fn ngon(n: u32, r: f32) -> f32 {
            n as f32 / 2.0 * r * r * (std::f32::consts::TAU / n as f32).sin()
        }

        #[test]
        fn a_rectangle_off_the_axis_turns_into_a_faceted_ring() {
            let (sketch, axis) = ring(0.02, 0.05, 0.1, 0.0);
            let evaluated = turned(OriginPlane::Xy, sketch, axis, 360.0, 12, false);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            let expected = 0.1 * (ngon(12, 0.05) - ngon(12, 0.02));
            assert!((volume - expected).abs() < 1e-7, "{volume} vs {expected}");
            // Turned about the plane's y, which on XY is world Y.
            let (min, max) = evaluated.bodies[0].1.bounds().unwrap();
            assert!(min.y.abs() < 1e-6 && (max.y - 0.1).abs() < 1e-6, "{min} {max}");
        }

        /// Fails if the step count is not clamped to the low-poly range.
        #[test]
        fn steps_are_kept_between_three_and_twenty_four() {
            let volume = |steps| {
                let (sketch, axis) = ring(0.0, 0.05, 0.1, 0.0);
                turned(OriginPlane::Xy, sketch, axis, 360.0, steps, false).bodies[0].1.volume()
            };
            assert!((volume(100) - volume(24)).abs() < 1e-9);
            assert!((volume(1) - volume(3)).abs() < 1e-9);
            assert!((volume(24) - 0.1 * ngon(24, 0.05)).abs() < 1e-7);
        }

        #[test]
        fn a_part_turn_is_that_part_and_symmetric_straddles_the_plane() {
            let (sketch, axis) = ring(0.02, 0.05, 0.1, 0.0);
            let full = turned(OriginPlane::Xy, sketch.clone(), axis, 360.0, 12, false).bodies[0].1.volume();
            let quarter = turned(OriginPlane::Xy, sketch.clone(), axis, 90.0, 3, false);
            assert!((quarter.bodies[0].1.volume() * 4.0 - full).abs() < 1e-7);

            let both_ways = turned(OriginPlane::Xy, sketch, axis, 90.0, 3, true);
            let (min, max) = both_ways.bodies[0].1.bounds().unwrap();
            assert!((min.z + max.z).abs() < 1e-6, "even about the sketch's plane: {min} {max}");
        }

        /// Fails if the frame is fixed to one side: a region left of its axis
        /// turns as well as one to the right.
        #[test]
        fn a_region_either_side_of_its_axis_turns() {
            let (sketch, axis) = ring(-0.05, -0.02, 0.1, 0.0);
            let evaluated = turned(OriginPlane::Xy, sketch, axis, 360.0, 12, false);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let expected = 0.1 * (ngon(12, 0.05) - ngon(12, 0.02));
            assert!((evaluated.bodies[0].1.volume() - expected).abs() < 1e-7);
        }

        #[test]
        fn an_axis_through_the_region_or_missing_is_a_problem() {
            let (sketch, axis) = ring(0.02, 0.05, 0.1, 0.03);
            let crossed = turned(OriginPlane::Xy, sketch.clone(), axis, 360.0, 12, false);
            assert!(crossed.bodies.is_empty());
            assert_eq!(crossed.problems.len(), 1);

            let missing = turned(OriginPlane::Xy, sketch, SketchId(999), 360.0, 12, false);
            assert!(missing.bodies.is_empty());
            assert_eq!(missing.problems.len(), 1);
        }

        /// Fails if a line that is not the sketch's own cannot be turned
        /// about, or one off the sketch's plane quietly turns something.
        #[test]
        fn any_line_in_the_sketchs_plane_is_an_axis() {
            let (mut sketch, _) = ring(0.02, 0.05, 0.1, 0.0);
            let names = every(&arrange(&sketch).regions);
            let regions = adopt(&mut sketch, names);
            let about = |axis: Axis| {
                evaluate(&[
                    PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch: sketch.clone() }),
                    PropFeature::new(
                        PropFeatureId(1),
                        FeatureOp::Revolve { op: SweepOp::NewBody, targets: vec![],
                            profile: ProfileSource::Regions { sketch: PropFeatureId(0), regions: regions.clone() },
                            degrees: 360.0,
                            steps: 12,
                            symmetric: false,
                            axis: CurveRef::Axis { axis },
                            surface: Surface::default(),
                        },
                    ),
                ])
            };
            let turned = about(Axis::Y);
            assert!(turned.problems.is_empty(), "{:?}", turned.problems);
            let expected = 0.1 * (ngon(12, 0.05) - ngon(12, 0.02));
            assert!((turned.bodies[0].1.volume() - expected).abs() < 1e-7);

            let off = about(Axis::Z);
            assert!(off.bodies.is_empty());
            assert_eq!(off.problems[0].message, get!("prop.problems.axis_off_plane"));
        }

        /// A part turn of a region with a hole: the cutter's caps are a step
        /// beyond the body's, and the volume still comes out exact.
        #[test]
        fn a_hole_is_left_out_of_a_part_turn() {
            let (mut sketch, axis) = ring(0.02, 0.06, 0.1, 0.0);
            let corners = [[0.03, 0.03], [0.05, 0.03], [0.05, 0.07], [0.03, 0.07]];
            let points: Vec<SketchId> = corners.iter().map(|c| sketch.add_point(Vec2::from_array(*c))).collect();
            for i in 0..4 {
                sketch.add_line(points[i], points[(i + 1) % 4]);
            }
            let outer = arrange(&sketch).regions.into_iter().max_by(|a, b| a.area().total_cmp(&b.area())).unwrap();
            let outer = sketch.adopt_region(&outer);
            let evaluated = evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch }),
                PropFeature::new(
                    PropFeatureId(1),
                    FeatureOp::Revolve { op: SweepOp::NewBody, targets: vec![],
                        axis: CurveRef::Sketch { sketch: PropFeatureId(0), entity: axis },
                        degrees: 120.0,
                        steps: 4,
                        symmetric: false,
                        surface: Surface::default(),
                        profile: ProfileSource::Regions { sketch: PropFeatureId(0), regions: vec![outer] },
                    },
                ),
            ]);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            // A wedge of a 12-gon ring is a third of the whole ring; 4 steps
            // over 120 degrees is 12 per turn.
            let ring = |r0: f32, r1: f32, h: f32| h * (ngon(12, r1) - ngon(12, r0)) / 3.0;
            let expected = ring(0.02, 0.06, 0.1) - ring(0.03, 0.05, 0.04);
            let volume = evaluated.bodies[0].1.volume();
            assert!((volume - expected).abs() < 1e-7, "{volume} vs {expected}");
        }

        #[test]
        fn a_revolve_survives_the_file() {
            let (mut sketch, axis) = ring(0.02, 0.05, 0.1, 0.0);
            let names = every(&arrange(&sketch).regions);
            let regions = adopt(&mut sketch, names);
            let mut doc = PropDoc::new("test");
            let on = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xz), sketch });
            doc.push(FeatureOp::Revolve { op: SweepOp::NewBody, targets: vec![],
                axis: CurveRef::Sketch { sketch: on, entity: axis },
                degrees: 270.0,
                steps: 9,
                symmetric: true,
                surface: Surface::default(),
                profile: ProfileSource::Regions { sketch: on, regions },
            });
            let text = to_text(&doc).unwrap();
            assert_eq!(parse(&text).unwrap(), doc, "{text}");
        }
    }

    mod loft {
        use super::super::*;
        use super::extrude::{adopt, every};
        use crate::prop::document::{parse, to_text, PropDoc};
        use crate::prop::plane::OriginPlane;
        use crate::prop::region::arrange;
        use crate::prop::sketch::Sketch;

        /// Squares centered on the origin, the first the outside and the
        /// rest inside it.
        fn squares(widths: &[f32]) -> Sketch {
            let mut sketch = Sketch::default();
            for side in widths {
                let h = side / 2.0;
                let corners: Vec<SketchId> =
                    [[-h, -h], [h, -h], [h, h], [-h, h]].iter().map(|at| sketch.add_point(Vec2::from_array(*at))).collect();
                for i in 0..4 {
                    sketch.add_line(corners[i], corners[(i + 1) % 4]);
                }
            }
            sketch
        }

        /// A region of `near` on the XY plane lofted to one of `far` on the
        /// plane `height` along its normal; `region` chooses which region.
        fn lofted(near: Sketch, far: Sketch, height: f32, region: impl Fn(&Sketch) -> usize) -> Evaluated {
            let mut sketches = [near, far];
            let entries: Vec<SketchId> = sketches
                .iter_mut()
                .map(|sketch| {
                    let names = every(&arrange(sketch).regions);
                    let at = region(sketch);
                    adopt(sketch, vec![names[at].clone()])[0]
                })
                .collect();
            let [near, far] = sketches;
            evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch: near }),
                PropFeature::new(
                    PropFeatureId(1),
                    FeatureOp::Plane { def: PlaneDef::Offset { distance: height, from: PlaneRef::Origin(OriginPlane::Xy) } },
                ),
                PropFeature::new(PropFeatureId(2), FeatureOp::Sketch { on: PlaneRef::Feature(PropFeatureId(1)), sketch: far }),
                PropFeature::new(
                    PropFeatureId(3),
                    FeatureOp::Loft { op: SweepOp::NewBody, targets: vec![],
                        closed: false,
                        profiles: vec![
                            Profile { sketch: PropFeatureId(0), region: entries[0], start: None },
                            Profile { sketch: PropFeatureId(2), region: entries[1], start: None },
                        ],
                        surface: Surface::default(),
                    },
                ),
            ])
        }

        fn frustum(a: f32, b: f32, h: f32) -> f32 {
            h / 3.0 * (a * a + b * b + a * b)
        }

        #[test]
        fn two_squares_loft_to_a_frustum() {
            let evaluated = lofted(squares(&[0.06]), squares(&[0.02]), 0.1, |_| 0);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            let expected = frustum(0.06, 0.02, 0.1);
            assert!((volume - expected).abs() < expected * 1e-4, "{volume} against {expected}");
            assert_eq!(evaluated.wires[0].1[0].outline.faces.len(), 6, "two caps and a side per line");
        }

        /// The far plane's normal points back at the first, so its frame is
        /// turned round before the skin sees it.
        #[test]
        fn a_profile_below_the_first_lofts_the_same() {
            let evaluated = lofted(squares(&[0.06]), squares(&[0.02]), -0.1, |_| 0);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            let expected = frustum(0.06, 0.02, 0.1);
            assert!((volume - expected).abs() < expected * 1e-4, "{volume} against {expected}");
        }

        #[test]
        fn a_square_lofts_to_a_circle() {
            let mut far = Sketch::default();
            let center = far.add_point(Vec2::ZERO);
            far.add_circle(center, 0.02);
            let evaluated = lofted(squares(&[0.06]), far, 0.1, |_| 0);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            let (low, high) = (frustum(0.04, 0.04, 0.1), frustum(0.06, 0.04, 0.1));
            assert!(volume > low && volume < high, "{volume} not within {low}..{high}");
            assert_eq!(evaluated.wires[0].1[0].outline.faces.len(), 6, "faces are named by the first profile's lines");
        }

        #[test]
        fn holes_loft_with_their_profiles() {
            // The region between the two squares, not the hole's own.
            let ring = |sketch: &Sketch| {
                arrange(sketch).regions.iter().position(|region| !region.holes.is_empty()).expect("a region with a hole")
            };
            let evaluated = lofted(squares(&[0.06, 0.02]), squares(&[0.04, 0.01]), 0.1, ring);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            let expected = frustum(0.06, 0.04, 0.1) - frustum(0.02, 0.01, 0.1);
            assert!((volume - expected).abs() < expected * 1e-3, "{volume} against {expected}");
        }

        /// Three squares a third of a turn apart about the Y axis, joined
        /// round: a ring of three straight pieces with no caps. A plane
        /// turned about an axis keeps that axis as its own `x`, so the
        /// squares stand off it along `y`.
        #[test]
        fn a_closed_loft_joins_its_last_profile_to_its_first() {
            let mut features = Vec::new();
            let mut profiles = Vec::new();
            for (i, degrees) in [0.0f32, 120.0, 240.0].into_iter().enumerate() {
                let (plane, sketch) = (PropFeatureId(2 * i as u32), PropFeatureId(2 * i as u32 + 1));
                features.push(PropFeature::new(
                    plane,
                    FeatureOp::Plane {
                        def: PlaneDef::Angle {
                            degrees,
                            from: PlaneRef::Origin(OriginPlane::Xy),
                            about: CurveRef::Axis { axis: Axis::Y },
                        },
                    },
                ));
                let mut drawn = Sketch::default();
                let corners: Vec<SketchId> = [[-0.02, 0.08], [0.02, 0.08], [0.02, 0.12], [-0.02, 0.12]]
                    .iter()
                    .map(|at| drawn.add_point(Vec2::from_array(*at)))
                    .collect();
                for k in 0..4 {
                    drawn.add_line(corners[k], corners[(k + 1) % 4]);
                }
                let names = every(&arrange(&drawn).regions);
                let region = adopt(&mut drawn, names)[0];
                features.push(PropFeature::new(sketch, FeatureOp::Sketch { on: PlaneRef::Feature(plane), sketch: drawn }));
                profiles.push(Profile { sketch, region, start: None });
            }
            features.push(PropFeature::new(PropFeatureId(6), FeatureOp::Loft { op: SweepOp::NewBody, targets: vec![], closed: true, profiles, surface: Surface::default() }));
            let evaluated = evaluate(&features);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            assert!(evaluated.bodies[0].1.volume() > 0.0);
            assert_eq!(evaluated.wires[0].1[0].outline.faces.len(), 12, "four sides on each of three spans");
        }

        #[test]
        fn one_profile_is_a_problem() {
            let mut sketch = squares(&[0.06]);
            let names = every(&arrange(&sketch).regions);
            let region = adopt(&mut sketch, names)[0];
            let evaluated = evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch }),
                PropFeature::new(
                    PropFeatureId(1),
                    FeatureOp::Loft { op: SweepOp::NewBody, targets: vec![],
                        closed: false,
                        profiles: vec![Profile { sketch: PropFeatureId(0), region, start: None }],
                        surface: Surface::default(),
                    },
                ),
            ]);
            assert!(evaluated.bodies.is_empty());
            assert_eq!(evaluated.problems.len(), 1);
        }

        #[test]
        fn a_loft_survives_the_file() {
            let mut doc = PropDoc::new("test");
            let near = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch: squares(&[0.06]) });
            let far = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch: squares(&[0.02]) });
            doc.push(FeatureOp::Loft { op: SweepOp::NewBody, targets: vec![],
                closed: true,
                profiles: vec![
                    Profile { sketch: near, region: SketchId(5), start: None },
                    Profile { sketch: far, region: SketchId(6), start: Some(PointRef::Sketch { sketch: far, point: SketchId(1) }) },
                ],
                surface: Surface::default(),
            });
            let text = to_text(&doc).unwrap();
            assert_eq!(parse(&text).unwrap(), doc, "{text}");
        }
    }

    mod extrude_options {
        use super::super::*;
        use super::extrude::{adopt, every};
        use crate::prop::plane::OriginPlane;
        use crate::prop::region::arrange;
        use crate::prop::sketch::Sketch;

        const SIDE: f32 = 0.02;

        /// A square `SIDE` wide centered on the origin, its one region adopted.
        fn square() -> (Sketch, SketchId) {
            let mut sketch = Sketch::default();
            let h = SIDE / 2.0;
            let corners: Vec<SketchId> =
                [[-h, -h], [h, -h], [h, h], [-h, h]].iter().map(|at| sketch.add_point(Vec2::from_array(*at))).collect();
            for i in 0..4 {
                sketch.add_line(corners[i], corners[(i + 1) % 4]);
            }
            let names = every(&arrange(&sketch).regions);
            let region = adopt(&mut sketch, names)[0];
            (sketch, region)
        }

        fn extrude(sketch: u32, region: SketchId, end: End) -> FeatureOp {
            FeatureOp::Extrude {
                profile: ProfileSource::Regions { sketch: PropFeatureId(sketch), regions: vec![region] },
                taper: 0.0,
                op: SweepOp::NewBody,
                targets: vec![],
                end,
                other: None,
                direction: None,
                surface: Surface::default(),
            }
        }

        fn cube() -> FeatureOp {
            FeatureOp::Primitive { shape: Shape::Box { size: [0.1; 3] }, placement: Placement::default(), surface: Surface::default() }
        }

        /// A 0.1 m cube centered on the origin, the square sketched on the
        /// XY plane through its middle, and `op` made from them.
        fn on_the_cube(op: impl Fn(SketchId) -> FeatureOp) -> Evaluated {
            let (sketch, region) = square();
            evaluate(&[
                PropFeature::new(PropFeatureId(0), cube()),
                PropFeature::new(PropFeatureId(1), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch }),
                PropFeature::new(PropFeatureId(2), op(region)),
            ])
        }

        fn only_volume(evaluated: &Evaluated) -> f32 {
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            assert_eq!(evaluated.bodies.len(), 1);
            evaluated.bodies[0].1.volume()
        }

        fn near(volume: f32, expected: f32) {
            assert!((volume - expected).abs() < expected * 1e-3, "{volume} against {expected}");
        }

        /// A cube at the origin, a second at `far_size` 0.3 m up, the square
        /// sketched, and a cut through all both ways naming both cubes.
        fn cut_through_two(far_size: f32) -> Evaluated {
            let (sketch, region) = square();
            let far = FeatureOp::Primitive {
                shape: Shape::Box { size: [far_size; 3] },
                placement: Placement::at(0.0, 0.0, 0.3),
                surface: Surface::default(),
            };
            let mut cut = extrude(2, region, End::ThroughAll);
            if let FeatureOp::Extrude { op, targets, other, .. } = &mut cut {
                *op = SweepOp::Cut;
                *targets = vec![BodyRef::whole(PropFeatureId(0)), BodyRef::whole(PropFeatureId(1))];
                *other = Some(End::ThroughAll);
            }
            evaluate(&[
                PropFeature::new(PropFeatureId(0), cube()),
                PropFeature::new(PropFeatureId(1), far),
                PropFeature::new(PropFeatureId(2), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch }),
                PropFeature::new(PropFeatureId(3), cut),
            ])
        }

        #[test]
        fn a_cut_through_two_targets_bores_each_as_its_own_body() {
            let evaluated = cut_through_two(0.1);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            assert_eq!(evaluated.bodies.len(), 2);
            for (index, (body, solid)) in evaluated.bodies.iter().enumerate() {
                assert_eq!(*body, BodyRef { feature: PropFeatureId(3), index: index as u32, of: 2 });
                near(solid.volume(), 0.1f32.powi(3) - SIDE * SIDE * 0.1);
                // The bore's wall in this body names this body.
                let (_, wires) = &evaluated.wires[index];
                let wall = wires.iter().find(|wire| wire.source.feature == PropFeatureId(3)).expect("the cut's outline");
                let face = GeomRef { element: crate::prop::reference::Element::Face(0), source: wall.source.clone() };
                assert_eq!(evaluated.body_of(&face), Some(*body));
            }
        }

        #[test]
        fn a_cut_that_empties_one_target_leaves_both_standing() {
            let evaluated = cut_through_two(0.01);
            assert_eq!(evaluated.problems.len(), 1);
            let standing: Vec<BodyRef> = evaluated.bodies.iter().map(|(body, _)| *body).collect();
            assert_eq!(standing, vec![BodyRef::whole(PropFeatureId(0)), BodyRef::whole(PropFeatureId(1))], "in their order");
        }

        #[test]
        fn a_join_adds_to_its_target_under_its_own_id() {
            let evaluated = on_the_cube(|region| {
                let mut op = extrude(1, region, End::Distance { distance: 0.1 });
                if let FeatureOp::Extrude { op: how, targets, .. } = &mut op {
                    (*how, *targets) = (SweepOp::Join, vec![BodyRef::whole(PropFeatureId(0))]);
                }
                op
            });
            near(only_volume(&evaluated), 0.1f32.powi(3) + SIDE * SIDE * 0.05);
            assert_eq!(evaluated.bodies[0].0.feature, PropFeatureId(2));
        }

        #[test]
        fn a_cut_through_all_both_ways_bores_the_cube() {
            let evaluated = on_the_cube(|region| {
                let mut op = extrude(1, region, End::ThroughAll);
                if let FeatureOp::Extrude { op: how, targets, other, .. } = &mut op {
                    (*how, *targets, *other) = (SweepOp::Cut, vec![BodyRef::whole(PropFeatureId(0))], Some(End::ThroughAll));
                }
                op
            });
            near(only_volume(&evaluated), 0.1f32.powi(3) - SIDE * SIDE * 0.1);
        }

        #[test]
        fn a_join_with_no_target_is_a_problem_and_makes_nothing() {
            let evaluated = on_the_cube(|region| {
                let mut op = extrude(1, region, End::Distance { distance: 0.1 });
                if let FeatureOp::Extrude { op: how, .. } = &mut op {
                    *how = SweepOp::Join;
                }
                op
            });
            assert_eq!(evaluated.problems.len(), 1);
            assert_eq!(evaluated.bodies.len(), 1, "the cube stands as it was");
            assert_eq!(evaluated.bodies[0].0.feature, PropFeatureId(0));
        }

        #[test]
        fn an_extrude_stops_at_a_plane() {
            let (sketch, region) = square();
            let evaluated = evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch }),
                PropFeature::new(
                    PropFeatureId(1),
                    FeatureOp::Plane { def: PlaneDef::Offset { distance: 0.07, from: PlaneRef::Origin(OriginPlane::Xy) } },
                ),
                PropFeature::new(PropFeatureId(2), extrude(0, region, End::UpTo { to: AnyRef::Plane(PlaneRef::Feature(PropFeatureId(1))) })),
            ]);
            near(only_volume(&evaluated), SIDE * SIDE * 0.07);
        }

        /// Cavalieri: a slanted prism holds its base times its height.
        #[test]
        fn a_slanted_extrude_holds_its_base_times_its_height() {
            let (sketch, region) = square();
            let mut guide = Sketch::default();
            // On the XZ plane, whose y runs along the prop's -Z: up and
            // across at 30 degrees from the normal.
            let (a, b) = (guide.add_point(Vec2::ZERO), guide.add_point(Vec2::new(0.5, -(3.0f32).sqrt() / 2.0)));
            let line = guide.add_line(a, b);
            let evaluated = evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch }),
                PropFeature::new(PropFeatureId(1), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xz), sketch: guide }),
                PropFeature::new(PropFeatureId(2), {
                    let mut op = extrude(0, region, End::Distance { distance: 0.1 });
                    if let FeatureOp::Extrude { direction, .. } = &mut op {
                        *direction = Some(DirectionRef::Along { curve: CurveRef::Sketch { sketch: PropFeatureId(1), entity: line } });
                    }
                    op
                }),
            ]);
            let height = 0.1 * 30f32.to_radians().cos();
            near(only_volume(&evaluated).abs(), SIDE * SIDE * height);
        }

        /// The square shrinks about its middle by the lean at its
        /// equivalent radius, so it is a square frustum.
        #[test]
        fn a_tapered_extrude_on_both_sides_is_two_frustums() {
            let (sketch, region) = square();
            let (taper, h) = (10.0f32, 0.02);
            let evaluated = evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch }),
                PropFeature::new(PropFeatureId(1), {
                    let mut op = extrude(0, region, End::Distance { distance: h });
                    if let FeatureOp::Extrude { taper: lean, other, .. } = &mut op {
                        (*lean, *other) = (taper, Some(End::Distance { distance: h }));
                    }
                    op
                }),
            ]);
            let radius = (SIDE * SIDE / std::f32::consts::PI).sqrt();
            let k = 1.0 - h * taper.to_radians().tan() / radius;
            let area = SIDE * SIDE;
            let frustum = h / 3.0 * (area + area * k * k + area * k);
            near(only_volume(&evaluated), 2.0 * frustum);
            assert_eq!(evaluated.wires[0].1[0].outline.faces.len(), 10, "two caps and four sides on each half");
        }

        #[test]
        fn a_taper_that_closes_the_region_is_a_problem() {
            let (sketch, region) = square();
            let evaluated = evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch }),
                PropFeature::new(PropFeatureId(1), {
                    let mut op = extrude(0, region, End::Distance { distance: 0.1 });
                    if let FeatureOp::Extrude { taper, .. } = &mut op {
                        *taper = 45.0;
                    }
                    op
                }),
            ]);
            assert!(evaluated.bodies.is_empty());
            assert_eq!(evaluated.problems.len(), 1);
        }

        #[test]
        fn every_extrude_option_survives_the_file() {
            use crate::prop::document::{parse, to_text, PropDoc};
            use crate::prop::reference::{Element, Source};
            let mut doc = PropDoc::new("test");
            let body = doc.push(cube());
            let (sketch, region) = square();
            let on = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch });
            let face = GeomRef { element: Element::Face(1), source: Source { feature: body, copies: vec![] } };
            doc.push(FeatureOp::Extrude {
                profile: ProfileSource::Regions { sketch: on, regions: vec![region] },
                taper: 2.5,
                op: SweepOp::Cut,
                targets: vec![BodyRef::whole(body)],
                end: End::UpTo { to: AnyRef::Face { face } },
                other: Some(End::ThroughAll),
                direction: Some(DirectionRef::Normal { plane: PlaneRef::Origin(OriginPlane::Xz) }),
                surface: Surface::default(),
            });
            doc.push(FeatureOp::Revolve {
                profile: ProfileSource::Regions { sketch: on, regions: vec![region] },
                degrees: 90.0,
                steps: 6,
                symmetric: false,
                op: SweepOp::Intersect,
                targets: vec![BodyRef::whole(body)],
                axis: CurveRef::Axis { axis: Axis::Y },
                surface: Surface::default(),
            });
            let text = to_text(&doc).unwrap();
            assert_eq!(parse(&text).unwrap(), doc, "{text}");
        }

        #[test]
        fn a_revolve_can_cut_too() {
            // A ring of the square's cross-section turned about the Y axis
            // from x = 0.03, cut from the cube.
            let mut sketch = Sketch::default();
            let corners: Vec<SketchId> = [[0.03, -0.01], [0.05, -0.01], [0.05, 0.01], [0.03, 0.01]]
                .iter()
                .map(|at| sketch.add_point(Vec2::from_array(*at)))
                .collect();
            for i in 0..4 {
                sketch.add_line(corners[i], corners[(i + 1) % 4]);
            }
            let names = every(&arrange(&sketch).regions);
            let region = adopt(&mut sketch, names)[0];
            let evaluated = evaluate(&[
                PropFeature::new(PropFeatureId(0), cube()),
                PropFeature::new(PropFeatureId(1), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch }),
                PropFeature::new(
                    PropFeatureId(2),
                    FeatureOp::Revolve {
                        profile: ProfileSource::Regions { sketch: PropFeatureId(1), regions: vec![region] },
                        degrees: 360.0,
                        steps: 24,
                        symmetric: false,
                        op: SweepOp::Cut,
                        targets: vec![BodyRef::whole(PropFeatureId(0))],
                        axis: CurveRef::Axis { axis: Axis::Y },
                        surface: Surface::default(),
                    },
                ),
            ]);
            let volume = only_volume(&evaluated);
            assert!(volume < 0.1f32.powi(3) && volume > 0.1f32.powi(3) * 0.8, "{volume}");
        }
    }
}
