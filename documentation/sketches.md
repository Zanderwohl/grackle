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
  "region 3": the numbering is an accident of the arrangement. The name is the
  set of (entity id, side) pairs enclosing the region. Moving points keeps the
  name. Naming a region by a point inside it is the tempting alternative and is
  wrong the moment the region moves away from the point.
- **A sweep names an entry in the sketch's region list**, not the boundary
  itself. The entry holds the boundary, an optional label and a hint point.
  Two sweeps of one region share the entry, so re-picking it once re-points
  both, and a Problem can say "needs *Grip*" rather than list line ids.
  Unlabelled entries nothing uses are dropped when a feature is removed.
- **A split keeps the region; a merge loses it.** Lines drawn across a named
  region cut it into pieces whose outline is still exactly its name, so the
  pieces are joined back up and the sweep does not change — picking one piece
  is a deliberate re-pick. Only pieces touching the outline join, so something
  drawn inside stays a hole.
- **A rerouted outline keeps the region.** Erasing part of an outline and
  drawing round it — a notch in or out — leaves one face that every named line
  still bounds on its named side; that face is the region. The erase tool keeps
  a trimmed line's id, which is what makes this work.
- **A lost line loses the region.** A region whose bounding line is deleted,
  as when a divider between two regions goes, is a Problem; the region now at
  its hint is offered as a fix and never taken silently, because it is bigger
  than what was meant.
- **The hint only breaks ties.** Once curves arrive two regions can share a
  name (an ellipse crossing a circle four times leaves two identical
  crescents); the one nearest the hint wins. It is never what a region is
  found by.
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

Extrude { sketch, regions, end: End, symmetric, surface }
Revolve { sketch, regions, axis: EntityId, degrees, symmetric, surface }

Sketch.regions: Vec<SketchRegion { id, label, hint, bounds: RegionRef }>
Extrude/Revolve.regions: Vec<SketchId>     // entries in that list

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

- [x] Origin planes — XY, YZ, XZ through the prop's origin — always present,
      not deletable, pickable like plane features. A `PlaneRef` that names
      either kind.
- [x] `FeatureOp::Sketch` with points and lines, saved to the file, with a
      round-trip test. No body; drawn on its plane in the viewport. Created
      empty from the toolbar's **Sketch** with one plane picked.
- [x] The region toolkit, as above, with tests: a square, two overlapping
      squares (three regions), a figure-eight (two), a square with a square
      hole, an open chain (none), and naming that survives moving a point
      (`src/prop/region.rs`). Also T-junctions, spurs, coincident points,
      three lines through one point, and an island bridged to its surround.
      Not yet: a note for regions too thin to sweep.

### Sweeping

- [x] Extrude on regions, `end: Distance`, `symmetric`. Holes subtracted.
      Several regions are unioned into one body.
- [x] Revolve on regions about a sketch line, with the one-side check. Steps
      are clamped to 3–24 (`revolve_steps`); the axis defaults to the first
      line clearing every region, construction lines first, and is drawn
      dashed while the revolve is selected. Holes are cut by a revolve one
      step longer each way, so its facets line up and its caps are clear.
- [x] Region picks in the Model workspace, so a sweep is select-then-act like
      every other verb. A region wins over the face it is drawn on, so a sketch
      on a face — or on the cap of what it extruded — picks as the region.
- [x] Outlines and faces of swept regions feed `reference.rs`, so corners,
      edges and faces of a swept body are named and pickable as they are now
      (Extrude). Numbered by the regions in pick order and each region's
      outline as the arrangement walks it: stable while the sketch's lines
      stay as they are, renumbered if they are redrawn.
- [x] Retire `Profile` from Extrude and Revolve; rebuild the shipped props by
      hand. Done: neither sweep has a profile or placement now, and the
      profile editor is gone. The shipped launcher's one extrude, the Bore, is
      now the equivalent prism.
- [x] Named regions: sweeps name entries on the sketch, labelled in the
      Inspector, re-pointed from a pick or from the region at the old spot.
      Split regions are joined back up; merged ones are a Problem.
- [ ] Hide a sketch once something sweeps it, as CAD tools do, so its regions
      stop sitting on top of the body it made.

### The sketch workspace

- [x] Enter from a selected sketch (double-click, or the right-click menu);
      leave back to Model. A sketch made from the toolbar is entered at once.
      `SketchSession::editing` is the one answer to "is a sketch open".
- [x] One viewport straight down the plane; the others stay as they were.
      The perspective view turns, and is put back on finishing.
- [x] Line tool: click to place, chaining from the last point; landing on an
      existing point uses it and carries on from there. A line that encloses
      a new region ends the chain; otherwise only `Escape` does.
- [x] The grid is drawn 0.1 mm behind the plane, away from the view under the
      pointer, so a line along a grid line does not depth-fight it.
- [x] `L` picks the line tool and `K` the eraser. The line tool shows where
      the next click lands, ringed when it lands on a point, and the line it
      would draw.
- [x] Erase tool: a click takes the piece of a line between the junctions
      and crossings either side of it, a drag every piece it passes through.
      What is left of the line keeps its id, so a region named by it holds;
      erasing a middle piece gives the far end a new id, and the named
      regions running along it are re-pointed at it.
- [x] While a sketch is open the view stops at it (`PropEditor::peek`), so
      what was built on it is not in the way. Not the rollback bar.
- [x] Drag points; delete points and lines; toggle construction. A click
      toggles a pick, as in Model; a point's position is editable in the
      Inspector.
- [x] Snap to a grid (5 mm), to sketch points, and to model corners seen from
      the plane (copied positions, not references).
- [x] Regions hatched as they form, so a closed shape is visibly closed.
- [ ] Pick a sketch in the Model viewport by its lines (today: from the
      feature list only).
- [ ] Grid spacing as a setting rather than a constant.

### Later

- [ ] `End::UpTo(GeomRef)` and `End::ThroughAll`.
- [ ] A guide line for a slanted extrude.
- [ ] A model edge as a revolve's axis.
- [ ] Arcs and circles, then Béziers, faceted before the arrangement.
- [ ] Projected model geometry that follows the model, rather than copied
      positions.
- [ ] Constraints and dimensions, named by the same ids.
- [ ] Offset and flip on planes; a plane from a plane.
