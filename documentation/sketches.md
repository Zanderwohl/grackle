# Sketches

A 2D drawing on a plane, which Extrude and Revolve turn into solids. This is
the plan for adding them to the prop editor (`src/prop/`). It is deliberately a
small tool inside a small tool: lines first, the few checks a sweep needs to be
honest, and nothing a full CAD sketcher has that a weapon modeller would not
miss. Tick items off as they land; the checklist is the source of truth for
what is done.

## Where it starts from (October 2026)

- **Planes exist.** `FeatureOp::Plane` is made through corners, edges or a flat
  face, resolved where it sits in the list (`src/prop/plane.rs`). There are no
  origin planes, so an empty prop has nothing to draw on.
- **A sweep carries its own drawing.** Extrude and Revolve each hold a
  `Profile` (Rect, N-gon or a typed list of points) and a `Placement`. Two
  sweeps cannot share a drawing, and the points are edited as numbers.
- **Picking is ordered and toggling**, over bodies, named corners, edges and
  faces, and planes (`Pick`). A face stands in for its body.

## Decisions

- **A sketch is one feature.** `FeatureOp::Sketch` sits in the prop's list
  like any other, makes no body, and is consumed by nothing: two sweeps may
  use one sketch.
- **Inside, a sketch is a flat set, not a feature list.** Nothing in a drawing
  depends on the order it was drawn in. Two lines are tied together by
  *sharing a point*, which is identity rather than history: drag the point and
  both follow because they hold its id. A timeline inside would add ordering
  bugs and buy nothing.
- **No undo stack of its own.** A sketch is part of `PropDoc`, so the prop's
  whole-document snapshots already undo it, and `begin_gesture`/`end_gesture`
  already makes one drag one step. A second stack is a second history to keep
  in step with the first.
- **Points are first-class and shared.** A line holds two point ids, never two
  positions. Whether a loop is closed is then a question about ids, not about
  two coordinates agreeing to within a tolerance. Snapping an endpoint onto
  another *merges* the two points; that is how a loop gets closed.
- **Ids are never reused**, like `PropFeatureId`, because regions and later
  constraints name them.
- **Construction geometry** is a flag on an entity. It is drawn, snapped to and
  usable as a revolve's axis, and never bounds a region.
- **Several shapes on one sketch is normal**, and a sweep picks which regions
  it uses. So **self-intersection is not an error**: crossing lines make more
  regions, not an invalid sketch.
- **Regions are derived, and named by their boundary.** A sweep cannot store
  "region 3": the numbering is an accident of the arrangement. It stores the
  set of (entity id, side) pairs enclosing the region. Moving points keeps the
  name; deleting a bounding line breaks it, which is a Problem like a missing
  body. Naming a region by a point inside it is the tempting alternative and is
  wrong the moment the region moves away from the point.
- **Nothing refuses**, as everywhere else in the evaluator. An open chain, a
  zero-length line or a revolve axis through its region is a note in Problems,
  not a blocked edit.
- **Curves are faceted**, as an N-gon already is. When arcs and circles come,
  they are flattened to polylines before the arrangement, each piece
  remembering the entity it came from, so region names still hold.
- **Extrude goes along the plane's normal.** `end` is an enum from the first
  day — `Distance` now, `UpTo(GeomRef)` and `ThroughAll` later — so adding them
  does not change the file again. `symmetric` replaces `midplane`. A guide
  line for a slanted extrude is a later optional field.
- **Revolve turns about a line in the sketch**, construction or not, by an
  angle, optionally symmetric. A model edge as the axis can come later.
- **A region with holes is extruded as its outer loop minus its inner loops**,
  inside the evaluator, with the CSG that already exists, rather than teaching
  the ear-clipper about holes.
- **Editing a sketch is a third workspace**, beside Model and Fit, which is
  already the mechanism for swapping the toolbar and panels. In it, picking
  works on sketch points, lines and regions; the model shows through for
  snapping.
- **No migration.** `Profile` leaves Extrude and Revolve; the shipped props are
  rebuilt by hand.

## The model

```
Sketch {
    on: PlaneRef,                  // a plane feature, or an origin plane
    points:   Vec<SketchPoint>,    // id, position in the plane's x/y, metres
    entities: Vec<SketchEntity>,   // id, kind, construction
    next_id: u32,
}

SketchEntity::kind = Line { a: PointId, b: PointId }   // Arc, Circle, Bezier later

Extrude { sketch, regions: Vec<RegionRef>, end: End, symmetric, surface }
Revolve { sketch, regions: Vec<RegionRef>, axis: EntityId, degrees, symmetric, surface }

RegionRef = the set of (EntityId, Side) round the region's outer loop
```

Positions are in the plane's own frame (`Plane::x`, `Plane::y`), so moving the
plane moves the drawing and everything swept from it.

## The region toolkit

Pure functions over 2D data, in the style of `plane.rs`: no `World`, unit
tested.

1. **Flatten** every non-construction entity into segments, each tagged with
   its entity.
2. **Arrange**: find every crossing (with a stated tolerance), split segments
   there, and build a planar graph over shared and crossing points.
3. **Faces**: walk the graph for its bounded faces. Each is a region, with
   inner loops for any holes inside it.
4. **Name** each region by its boundary, and **find** one from a `RegionRef`.
5. **Report**: open chains, zero-length entities, coincident duplicates, and
   regions too thin to sweep, as notes rather than failures.
6. **Pick**: point-in-region against the faces, so a click inside a shape
   picks that region.
7. **Revolve check**: the axis lies in the plane (true of a sketch line by
   construction) and every chosen region is entirely on one side of it,
   touching allowed.

## Checklist

### Foundations

- [ ] Origin planes — XY, YZ, XZ through the prop's origin — always present,
      not deletable, pickable like plane features. A `PlaneRef` that names
      either kind.
- [ ] `FeatureOp::Sketch` with points and lines, saved to the file, with a
      round-trip test. No body; drawn on its plane in the viewport.
- [ ] The region toolkit, as above, with tests: a square, two overlapping
      squares (three regions), a figure-eight (two), a square with a square
      hole, an open chain (none), and naming that survives moving a point.

### Sweeping

- [ ] Extrude on regions, `end: Distance`, `symmetric`. Holes subtracted.
- [ ] Revolve on regions about a sketch line, with the one-side check.
- [ ] Region picks in the Model workspace, so a sweep is select-then-act like
      every other verb.
- [ ] Outlines and faces of swept regions feed `reference.rs`, so corners,
      edges and faces of a swept body are named and pickable as they are now.
- [ ] Retire `Profile` from Extrude and Revolve; rebuild the shipped props by
      hand.

### The sketch workspace

- [ ] Enter from a selected sketch (double-click, or the right-click menu);
      leave back to Model.
- [ ] One viewport straight down the plane; the others stay as they were.
- [ ] Line tool: click to place, chaining from the last point; landing on an
      existing point uses it.
- [ ] Drag points; delete points and lines; toggle construction.
- [ ] Snap to a grid, to sketch points, and to model corners seen from the
      plane (copied positions, not references).
- [ ] Regions shaded as they form, so a closed shape is visibly closed.

### Later

- [ ] `End::UpTo(GeomRef)` and `End::ThroughAll`.
- [ ] A guide line for a slanted extrude.
- [ ] A model edge as a revolve's axis.
- [ ] Arcs and circles, then Béziers, faceted before the arrangement.
- [ ] Projected model geometry that follows the model, rather than copied
      positions.
- [ ] Constraints and dimensions, named by the same ids.
- [ ] Offset and flip on planes; a plane from a plane.
