# Prop modeling: references, named geometry, curves and sweeps

A plan for the next stretch of the prop editor (`src/prop/`). It makes the
editor behave more like feature-based CAD: any point, curve, path or plane can
be picked wherever one is wanted, every edge on a body has a name, sketches
have curves, and the sweeps share one core that Extrude, Revolve, Sweep and
Loft all use. The checklist is at the bottom.

It builds on [sketches.md](sketches.md) and
[prop-editor-ux.md](prop-editor-ux.md), and replaces the parts of them that
assumed lines only and two sweeps.

## Where things stood

Stage 1 (references by kind) is built; this section describes the editor
before it.

- `Pick` (`document.rs`) is a closed enum of *where a click landed*: `Body`,
  `Geom`, `Plane`, `Region`, `SketchLine`, `Axis`. Each reference type has its
  own hand-written conversion (`Pick::line`, `Pick::body`, `plane_pick`).
- There is `LineRef` and `PlaneRef` but no point type. A point is only ever
  `Element::Point` inside a `GeomRef`, which means a body corner. A plane is
  made `through: Vec<GeomRef>`, so it cannot use a sketch's points or lines.
- `LineRef::resolve` returns two points. That is enough for an axis and
  cannot describe an arc to sweep along.
- Each re-pickable field is a `Field` variant with arms in `Task::slots`,
  `Task::name`, `repick` and `set_line`. `fits` has Revolve-only cases
  (`sweep_plane`, `picked_line`).
- `FeatureOp::consumes` and `references` are hand-written lists beside the
  fields they describe. Nothing checks they agree.
- Each evaluator arm passes its own lookups (`edge_of(&live, ..)`,
  `plane_of(.., &planes)`, `locate_in(&live, ..)`).
- A `GeomRef` names an element of its generator's `Outline` by index. An edge
  a boolean makes has no name, and a revolve's faces are named per facet
  (`k*n + i`), so a cylinder is twelve faces and a circle is twelve edges.
- Sketches hold `SketchEntity::Line` only. `Bound` already names an entity and
  a side, so regions are ready for other entity kinds.
- Extrude and Revolve each repeat: find the sketch, resolve its regions, sweep
  each, union, check for an empty result. Primitives have a third set of
  sweeps behind `Shape::sweep`.

No shipped prop under `assets/default/props/` names a corner, edge, face,
plane or axis; they are primitives, booleans, mirrors and patterns. Changing
the reference format therefore rewrites no shipped file, and there is no
migration (as before: `every_prop_the_default_pack_ships_loads_and_builds` is
the guard).

## Decisions

### 1. Reference types by kind

A reference is typed by **what it is geometrically**, not by where it came
from. New module `src/prop/refs.rs`:

| Kind | Variants | Resolves to |
| --- | --- | --- |
| `PointRef` | `Origin`; `Corner(GeomRef)`; `Sketch { sketch, point }`; `Datum(id)` | `Vec3` |
| `CurveRef` | `Axis(Axis)`; `Edge(GeomRef)` (a generator edge or a seam); `Sketch { sketch, entity }`; `DatumAxis(id)` | `Curve3` |
| `PathRef` | `Vec<CurveRef>`, in order | `Path3` (curves joined end to end) |
| `PlaneRef` | `Origin(OriginPlane)`; `Feature(id)` | `Plane` |
| `DirectionRef` | `Along(CurveRef)` (straight only); `Normal(PlaneRef)`; `Face(GeomRef)` (flat only) | `Dir3` |
| `BodyRef` | a feature id | a live body |
| `RegionsRef` | a sketch and entries in its region list | `Vec<Region>` |

- **What was built differs from the first draft in three places.** A curve's
  end or middle is not a `PointRef` variant; a datum point at a curve's middle
  (`PointDef::Midpoint`) covers it, and names a feature you can see. A face is
  not a `PlaneRef`: `PlaneRef` stays `Copy`, and a face picked where a plane is
  wanted is given a plane feature in the same undo step (`PlaneSource`), as
  Mirror and Sketch already did. A reversed direction is a negative spacing,
  not a `DirectionRef` variant.
- **One resolve context, `Scope`**: the live bodies and their wires, the
  planes, datum axes and points, and placed sketches *as they stand at this
  point in the list*. The evaluator builds it as it walks and every reference
  resolves against it through one trait,
  `Resolve { type Out; fn resolve(&self, &Scope) -> Option<Self::Out> }`.
  The ad hoc lookups (`edge_of`, `plane_of`, `locate_in`) become its methods.
- **`AnyRef`** is the sum of the kinds, so slots, re-picks and the inspector
  can hold any of them without a match per feature.
- **`Pick` stays as what is under the pointer**, with variants added for what
  can now be picked (`SketchPoint`, `SketchCurve` replacing `SketchLine`,
  `DatumAxis`, `DatumPoint`). It gets one conversion per kind:
  `pick.as_point()`, `as_curve()`, `as_plane()`, `as_direction()`,
  `as_body()`. A new pickable thing is one `Pick` variant and an arm in each
  conversion that it can stand for, all in one file.
- **A face stands for its plane, its body and its normal**, as a face stands
  for its body today. An edge stands for its curve and, if straight, its
  direction. A corner stands for its point.
- **`Plane { through }` takes `Vec<AnyRef>`**: three points, a curve and a
  point, two coplanar curves, or one flat face — from any source. Offset, angle
  about an edge and midplane between two planes are further `FeatureOp::Plane`
  variants (`PlaneDef`), since each is a different construction.
- **Datum axis and datum point features** join beside Plane: they make no body
  and are referenced by `CurveRef::DatumAxis` and `PointRef::Datum`.
- **Serde**: every kind is internally tagged, scalars before tables, floats
  through `nice_f32`, as the rest of the file is. A prop crosses the wire as
  its text, so the postcard limit on tagged enums does not apply.

### 2. Slots declare a kind and checks

- A `Slot` holds a `Kind` (`Body`, `Point`, `Curve`, `Path`, `Plane`,
  `Direction`, `Regions`), a count (one, or open until Done), a prompt key, and
  a list of checks.
- **A check is a function** `fn(&SlotCx, &AnyRef) -> bool`, where `SlotCx`
  holds the `Scope` and the other slots' current values. "The axis lies in the
  regions' sketch plane", "the direction is not parallel to the sketch plane"
  and "the path starts on the profile's plane" are each one named check,
  written once and reused by any tool.
- **Each feature lists its reference fields** through one method on
  `FeatureOp`: `fields(&self) -> Vec<Field>` and
  `fields_mut(&mut self) -> Vec<FieldMut>`, where a field is a lang key, a
  slot (kind, count, checks), and the value as `&AnyRef` / `&mut AnyRef`.
  From that one list:
  - `consumes` is every `BodyRef` field the feature eats and `references` is
    every other field's features. The hand-written lists go.
  - **Re-picking is generic.** `Task::Repick { feature, field: usize }` takes
    its slot from the field, and fills `SlotCx` from the feature's other
    fields, so a check that compares slots works the same on a new feature and
    on a re-pick. `Field` (the enum), `set_line`, `sweep_plane` and
    `picked_line` go.
  - The inspector draws a **Pick** row per field with no per-feature code.
  - A test walks one of every feature and checks that every reference in its
    serialized form is listed by `fields` (as `every_point_ref_is_listed` does
    on the map side).
- **A tool is a table row**: `ToolSpec { name, icon, slots, build }`, where
  `build(&[Vec<AnyRef>]) -> FeatureOp` makes the feature with defaults. `fill`,
  `is_open`, `is_ready`, `prompt` and `accepts` read the slots and never match
  on a tool.
- **Picking a path**: a `Path` slot takes curves until Done, as `Regions` does.
  A pick is accepted when it joins either end of what is there. A **Chain**
  button in the toolbar extends the last curve picked to every curve that
  joins it end to end without a corner (tangent within a tolerance), along
  seams and generator edges on a body, or along entities in a sketch. The
  result is stored as the plain list of curves, so the file never holds a rule
  that could chain differently later.

`FeatureOp` stays an enum, for the reasons in CLAUDE.md ("A modeling feature
is an `enum`"). What changes is that the per-variant facts a tool, the
inspector and the delete check need are read from `fields`, so adding a feature
is a variant, its `fields`, its arm in `evaluate`, and a `ToolSpec` row.
CLAUDE.md's sentence about this is updated when the stage lands.

### 3. Every edge and face has a name

**Faces are tagged through the kernel.** `csg::Polygon<S>` already carries an
`S: Copy` through every split. The prop's `S` becomes `(Surface, FaceTag)`,
where `FaceTag(u32)` indexes a table of face names in `Evaluated`, interned
per evaluation because `GeomRef` holds a `Vec` and is not `Copy`. Every
polygon of every body then says which named face it is part of, after any
number of booleans. `subtract` and `intersect` repaint the tool's surface and
keep its tag.

**Faces are logical, not facets.** A face is named by the profile entity and
the path segment that swept it (see section 5), so every facet a revolve or an
arc makes is one face: a cylinder is one side face, a sphere one face. The
`Outline` keeps facet geometry for drawing and hit tests, and gains a face
table mapping logical faces to facets. An edge between two facets of one face
is not an edge and is never offered.

**A seam is an edge between two different faces**, found on the final solid:

- Collect every polygon boundary segment, and pair segments that overlap with
  the opposite direction on a polygon of a different tag. The BSP leaves
  T-junctions, so segments are matched by overlap within the kernel's
  tolerance, never by shared vertex.
- Join the matched pieces between the same two faces into chains.
- Name each chain `Element::Seam { faces: [FaceName; 2], index }`, the two
  faces sorted so the name does not depend on which polygon was seen first.
  `index` separates two disjoint chains between the same two faces, ordered by
  their centroids along the first face's generator frame. That ordering can
  swap when geometry moves across, which is accepted and noted on the type: two
  separate seams between the same two faces are uncommon.
- A vertex where three or more faces meet is
  `Element::Junction { faces: [FaceName; 3] }`, the three smallest names.
- A seam that coincides with a generator edge is offered once, under the
  generator's name, so existing references and a seam never disagree.

**A seam resolves when its two faces still meet**, wherever they now meet. A
generator corner or edge still resolves after it is cut away, as now, but only
elements on the surface are offered (`pickables`).

**Every way of making a solid names its faces the same way.** Primitives are
built through the sweep core (section 5) — a box is a rectangle extruded, a
prism an n-gon extruded, a cone and a sphere a profile revolved — so no
generator has naming of its own to drift. Mirror and Pattern copies carry
`CopyOf` on each face name, so a seam on a copy names the copied faces.

Tests: the bore through a barrel offers its two rims as seams, each a circle
whose curve resolves; a seam survives lengthening the bore; every seam on every
shipped prop resolves to a closed chain or a chain with two ends on junctions;
a mirrored body's seams resolve to the reflection of the original's.

### 4. Curves in sketches

New `SketchEntity` variants, each naming its points by id so a shared end is
one point:

| Entity | Fields |
| --- | --- |
| `Arc` | `center`, `start`, `end` (point ids), `clockwise` |
| `Circle` | `center`, `radius` (meters) |
| `Bezier` | `points` (ids: start, controls, end), degree from the count |
| `Nurbs` | `points` (ids), `weights`, `knots`, `degree` |

- **Curves are evaluated as curves, and faceted before the arrangement.** A
  `Curve2` type (`point(t)`, `tangent(t)`, `length`, `facets(tolerance)`) is
  what an entity resolves to. `arrange` cuts facets, not curves, as it cuts
  lines today, but **each facet carries its entity id and parameter span**, so
  a region's outline is tagged per segment with the entity it came from. That
  tag is what names a swept face per entity rather than per facet, and `Bound`
  already names an entity.
- **Faceting is by chord tolerance**, the same for every machine (a constant,
  later a per-prop setting). Facet count changes as a curve is dragged; nothing
  is named by facet, so nothing renames.
- An arc's end point is moved onto the arc's radius when either end or the
  center moves, so the three points stay consistent without constraints.
- Bezier and NURBS control points are drawn as construction (a dashed hull) and
  snapped to like any point. NURBS editing in the sketch is points and weights;
  knots and degree are inspector fields.
- **`CurveRef::Sketch` resolves to a `Curve3`** (the `Curve2` lifted by the
  sketch's plane), and `CurveRef::Edge` resolves a generator edge or seam to a
  `Curve3` too: a line, an arc, or a polyline for a faceted seam. A path is
  then made of real curves.
- **3D paths** come from two places at first: edges and seams on bodies, and
  curves in different sketches joined end to end. A 3D sketch and a helix
  feature are left for later.

### 5. One sweep core

New module `src/prop/skin.rs`:

```rust
struct Section { loops: Vec<TaggedLoop>, frame: Affine3A }
fn skin(sections: &[Section], closed: bool, caps: Caps) -> Result<Swept, SkinProblem>
struct Swept { solid: Solid, outline: Outline }
```

- A `TaggedLoop` is a 2D loop whose segments carry the entity they came from
  (section 4), outer counter-clockwise, holes clockwise.
- Consecutive sections must have the same loops with the same segment counts.
  Side faces between them are emitted as **triangles**, since a quad between
  two frames is not planar in general and the kernel needs planar convex
  polygons. Caps are ear-clipped, as now. `closed` joins the last section to
  the first (a full revolve) and drops the caps.
- **Naming is done once, here.** A side face is `(entity, span)` where `span`
  is the path segment it lies on; a cap is `Start` or `End`. Generator edges
  and corners are numbered from the same tags, so every sweep names its
  geometry by the same rule.
- **Problems the core reports**: sections that do not match, a frame that
  flips between sections, and a path whose radius of curvature is smaller than
  the profile's reach from the path (the skin would fold through itself). Each
  is a `Problem` on the feature, not a panic.
- `Solid::oriented_outward` still decides the winding; the core does not try
  to get it right by reasoning.

Each sweep is a way of producing sections:

| Feature | Sections |
| --- | --- |
| **Extrude** | The profile at the sketch frame and at the frame moved along the direction: two sections. |
| **Revolve** | The profile at frames turned about the axis, `steps + 1` of them, or `steps` and `closed` for a full turn. |
| **Sweep** | The profile at frames along a `PathRef`, one per facet of the path. |
| **Loft** | A different profile per section, each on its own plane. |

#### Extrude, refactored

- Onto the core with no change in behavior first; the shipped props must build
  identically (volume and triangle count compared before and after).
- Then: `direction: Option<DirectionRef>` (none is the sketch normal; a
  slanted extrude replaces the planned guide line), `End` gains `UpTo(AnyRef)`
  (a point, a flat face or a plane) and `ThroughAll`, a second `End` for the
  other side in place of `symmetric`, and a taper angle (scale per section).
- `op: Join | Cut | Intersect | NewBody` with a `target: Option<BodyRef>`, so a
  cut is one feature. With `NewBody` it behaves as today. Join, Cut and
  Intersect consume the target, through `fields`, as a Boolean does.

#### Revolve

- Onto the core. `axis` becomes a `CurveRef` with a check that it is straight
  and in the sketch plane (both checks from section 2). Two-sided angles in
  place of `symmetric`. Faces are one per profile entity, so a revolved circle
  is one torus face.

#### Sweep

- `FeatureOp::Sweep { sketch, regions, path: PathRef, orient, twist, scale, op, target }`.
- `orient`: **`Minimal`** (rotation-minimizing frames by the double-reflection
  method — the default, since it does not spin on straight stretches),
  **`Frenet`** (follows the curvature, undefined on a straight path and
  reported as a problem there), **`Fixed`** (the profile keeps its own
  orientation, translated only).
- `twist` in degrees over the whole path and `scale` at the far end, both
  linear in path length.
- The path must start on the profile's plane (a check). Rotation-minimizing
  frames on a closed path do not return to the start orientation; the leftover
  angle is spread over the path so a closed sweep meets itself.

#### Loft

- `FeatureOp::Loft { profiles: Vec<RegionsRef>, guides: Vec<PathRef>, closed, op, target }`.
- **Correspondence**: each profile must have the same number of loops. Loops
  with the same number of entities are matched entity to entity; otherwise
  every loop is resampled by arc length to the largest facet count. The start
  of each loop is the rotation of its points that minimizes total distance to
  the previous loop's, with an optional picked `PointRef` per profile to set it
  by hand.
- Sections between profiles are interpolated (linearly first; guide curves,
  then a smooth interpolation through the profiles, later). Faces are named by
  the first profile's entities and the span between profiles.

## Order of work

Each stage is one PR, and each leaves `cargo test` passing.

1. **References by kind.** `refs.rs`, `Scope`, `AnyRef`, `Pick` conversions;
   `LineRef` becomes `CurveRef`, `Plane { through }` takes `AnyRef`. Evaluator
   resolves through `Scope`. No new features.
2. **Slots by kind and checks.** `fields`/`fields_mut`, derived
   `consumes`/`references`, generic re-pick, `ToolSpec` table, generic
   inspector Pick rows.
3. **The sweep core**, with Extrude, Revolve and the primitives moved onto it
   unchanged in behavior. Logical faces named per entity.
4. **Face tags and seams.** `FaceTag` through the kernel, seam and junction
   naming, `pickables` offering seams, `CurveRef::Edge` resolving seams.
5. **Arcs and circles**, then **Bezier**, then **NURBS**; `Curve2`/`Curve3`;
   tagged facets in the arrangement.
6. **Sweep**, with path picking and the Chain button.
7. **Loft.**
8. **Extrude options**: direction, `UpTo`/`ThroughAll`, two sides, taper,
   Join/Cut/Intersect.

Stage 3 comes before stage 4 so that seams are named from one generator rule,
not from three that are about to be replaced. Fillet and chamfer are not in
this plan; they need stage 4 and adjacency on the result, and get their own.

## Checklist

### References by kind
- [x] `src/prop/refs.rs`: `PointRef`, `CurveRef`, `PathRef`, `DirectionRef`, `AnyRef` (`PlaneRef` stays in `plane.rs`)
- [x] `BodyRef` and `RegionsRef` — built as `AnyRef::Body` and `AnyRef::Regions` in stage 2
- [x] `Scope` built by `evaluate`; `edge_of`, `plane_of`, `locate_in` become its methods
- [x] `Resolve` trait implemented for every kind
- [x] `Pick` gains `SketchPoint`, `SketchCurve`, `DatumAxis`, `DatumPoint`; one `as_*` conversion per kind
- [x] Sketch points pickable from outside the sketch
- [x] `LineRef` replaced by `CurveRef` in Revolve and the circle pattern; the row pattern runs along a `DirectionRef`
- [x] `Plane { def }`: through any `AnyRef`s, offset, angle and midplane
- [x] Datum axis and datum point features, with Axis and Point tools (`construct.rs`)
- [x] Round-trip tests for every kind's serde
- [ ] A handle to drag an offset plane's distance; today it is a number in the Inspector

### Slots and fields
- [x] `Slot { kind, prompt, checks }` in `slot.rs`; named checks shared between tools (a kind's count is its `open`)
- [x] `FeatureOp::fields` / `set_field` (one reads, one writes by the same index; `every_field_writes_back_what_it_reads` holds them together)
- [x] `consumes` and `references` derived from `fields`
- [x] `every_reference_is_a_field` test over one of every feature
- [x] `Task::Repick { feature, field: usize }`; `Field`, `set_line`, `sweep_plane`, `picked_line` removed
- [x] `ToolSpec` table; `fill`/`accepts`/`prompt` with no per-tool match
- [x] Inspector Pick rows drawn from `fields`, in a References section; datums and a sketch's plane can be re-picked
- [ ] `Path` slot and the Chain button — moved to stage 6, where Sweep is the first thing to take a path
- [x] CLAUDE.md's "adding a feature" sentence updated

### Sweep core
- [ ] `skin.rs`: `Section`, `TaggedLoop`, `skin`, `Swept`
- [ ] Naming by `(entity, span)` and `Start`/`End` caps
- [ ] Problems: mismatched sections, flipped frame, folding path
- [ ] Extrude on the core, shipped props unchanged (volume and triangle count)
- [ ] Revolve on the core; one face per profile entity
- [ ] Primitives on the core; `Shape::sweep` removed
- [ ] `every_sweep_is_wound_consistently` covers every section producer

### Named edges
- [ ] `FaceTag` carried by `csg::Polygon`; face table in `Evaluated`
- [ ] `subtract`/`intersect` repaint the tool but keep its tag
- [ ] Seams found by segment overlap; `Element::Seam`, `Element::Junction`
- [ ] A seam on a generator edge offered under the generator's name
- [ ] Mirror and Pattern copies carry `CopyOf` on face names
- [ ] `pickables` offers seams; edges between facets of one face never offered
- [ ] Tests: bore rims, rims survive lengthening the bore, every shipped seam resolves, mirrored seams

### Sketch curves
- [ ] `Curve2` and `Curve3`
- [ ] `SketchEntity::Arc` and `Circle`, with draw tools in the sketch workspace
- [ ] Facets tagged with entity and span in `arrange`
- [ ] `SketchEntity::Bezier`
- [ ] `SketchEntity::Nurbs`, weights and knots in the inspector
- [ ] `CurveRef` resolves arcs and seams to curves, not two points

### Sweeps
- [ ] `Path` slot (a pick joins either end) and the Chain button
- [ ] Sweep feature: `Minimal`, `Frenet`, `Fixed`; twist and scale
- [ ] Closed-path sweep spreads the leftover angle
- [ ] Loft feature: correspondence by entity, resampling, start alignment, picked start points
- [ ] Loft guide curves
- [ ] Extrude: direction, `UpTo`, `ThroughAll`, two sides, taper
- [ ] Extrude, Revolve, Sweep and Loft: Join / Cut / Intersect / New body
