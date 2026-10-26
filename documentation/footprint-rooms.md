# Footprint rooms

A map feature that is rooms drawn as a floor plan — any outline, notches,
angled walls and holes included — stood between two heights: the Doom/Build
editors' sector, in a map whose rooms are a union of volumes. This is the
plan for it and a checklist; the checklist is the source of truth for what is
done.

## Where it starts from (October 2026)

Everything underneath is built ([collision.md](collision.md), Stages 1–4):

- **`Room` is a prism** ([`src/tool/room.rs`](../src/tool/room.rs)): a
  footprint in `x`/`z`, any winding, not necessarily convex, between `floor`
  and `ceiling`. `Room::solid` triangulates a notched plan's caps. It has no
  holes.
- **The surface, collision and picking take any `Room`**
  ([`world_surface.rs`](../src/common/world_surface.rs)). An angled wall two
  rooms share opens, measured ~170 m from the origin. Bodies slide along
  angled walls and walk up steps.
- **A feature stands a room up** by inserting a `Room` on its entity in
  `apply_to_entity`, as `EditorRoom` does. The bake, collision and auto-bake
  read `Room` components and need nothing new.
- **The prop editor has sketches** ([`sketch.rs`](../src/prop/sketch.rs),
  [`region.rs`](../src/prop/region.rs), [sketches.md](sketches.md)): points
  by id, lines between them, and regions derived by arranging the lines —
  crossing lines split, an island is a hole, and a region is named by the
  lines round it so it survives edits.
- **What does not take any `Room`**: `find_nearest_feature_hit` rebuilds the
  room list from features whose `type_key` is `"editor_room"`, via
  `drag_handle_bounds`.

## Decisions

- **A footprint is a sketch whose points are `PointRef`s.** Region entries
  and ids are the prop sketch's own types (`SketchRegion`, `SketchId`); the
  points differ, because a map point
  can be measured from another feature's. At resolve time each corner is
  resolved, a `Sketch` is assembled from the results, and
  `region::arrange` derives the regions — the prop editor's region rules
  (split-tolerant, merge-strict, islands are holes) are the map's too, with no
  second implementation. Lines are the map's own `Line`, not `SketchEntity`:
  that is internally tagged, and RON writes it with its struct name and then
  cannot read it back.
- **Every corner is a `PointRef`**, read for `x` and `z`; its `y` is ignored.
  Drawn corners are absolute; Shift-picking another feature's point while
  drawing references it, as the Room tool does. A corner cannot reference the
  feature it is on — that is a cycle — so "relative to my own first corner"
  is not offered.
- **The floor is a `PointRef` too**, read for `y`, and the height above it a
  number. A floor anchored to another room's floor rides with it.
- **The feature names the regions it stands up**, as an extrude does
  (`rooms: Vec<SketchId>`, entries in the region list). One sketch can be
  several rooms at one floor height; each named region is one `Room`, on a
  child entity of the feature's, re-applied rather than respawned. Closing a
  loop while drawing names the region it closed.
- **Holes from the start.** A region's holes — whatever is drawn inside it —
  are holes in its `Room`: `Room` gains inner loops, `solid` subtracts their
  prisms (cut a little past the floor and ceiling, so no cap is coplanar, as
  `extrude_region` does), and `contains`, `volume` and `floor_point` leave
  them out. A pillar is a loop drawn inside a room.
- **Arranged in local coordinates.** `region::TOLERANCE` is `1e-6`,
  absolute, and chosen for a prop centimetres across, so the corners are
  arranged relative to the footprint's own first corner. Measured: slanted,
  crossing loops ~170 m out arrange correctly *without* it, unlike the CSG
  kernel; it is kept because it costs nothing, not because a test needs it.
- **Nothing refuses.** An open chain, a region whose line was deleted or a
  named region that no longer resolves stands up no room for it, and the
  inspector says which in orange, as the Mirror lists what it cannot copy.
- **Imported from `src/prop/`, not moved.** The map editor already depends on
  the prop module (placed props), and moving `sketch.rs` and `region.rs` to
  `src/common/` is churn with no behaviour in it. Left for when a third
  consumer appears.
- **Drawn on the floor plane, in any viewport.** Each click lands where the
  pointer's ray meets the horizontal plane through the floor, snapped to the
  grid and to existing corners; Shift picks a feature's point. The line tool
  chains, as the prop sketch's does, and closing on an existing corner merges
  the two.
- **Edited with handles.** Under the Select tool a selected footprint shows a
  handle per corner, dragged in the floor plane (`PointRef::drag_axis` on `x`
  and `z`), and floor and ceiling handles.
- **Mirroring reflects the resolved rooms** and lets `Room::prism` fix the
  winding; a mirror copy is not a feature, so its sketch is never edited.
- **Picking asks features for their rooms**: a new
  `FeatureTrait::rooms(&self) -> Vec<Room>` that `EditorRoom` and
  `FootprintRoom` both answer, used by `find_nearest_feature_hit`, so the
  next kind of room is pickable the day it is written.

## The model

```
FootprintRoom {
    floor: PointRef,                 // its y is the floor
    height: f32,                     // ceiling above the floor, metres
    next_id: u32,                    // shared by corners, lines and regions
    corners: Vec<Corner>,
    lines: Vec<Line>,                // { id, a, b, construction }
    regions: Vec<SketchRegion>,      // hints in world x/z
    rooms: Vec<SketchId>,            // region entries stood up as rooms
    #[serde(skip)] resolved: Vec<Room>,
    #[serde(skip)] problems: Vec<String>,
    #[serde(skip)] entity, room_entities
}

Corner { id: SketchId, at: PointRef }
```

Published points: `floor`, and `corner#{id}` / `corner#{id}.ceiling` for
every corner, so another feature can reference one.

## Checklist

### Stage 1 — the feature

- [x] `Room` with holes (`Room::with_holes`): `solid` subtracts each hole's
      prism, cut past the floor and ceiling; `contains` counts a pillar's face
      as room and its inside as not; `volume`, `floor_point` and `edges` know.
- [x] `FootprintRoom` in `src/editor/footprint_room.rs`: serde, resolving
      corners and floor, arranging relative to the first corner, one `Room`
      per named region on child entities re-applied by index, published
      points, mirror. `name_region_at` is what a closed loop will call.
- [x] Problems in the inspector, in orange: a lost region, and the
      arrangement's notes (open lines, zero length, coincident, missing
      corners).
- [x] Inspector: floor, height, corners as `PointRef` widgets, each
      retargetable by its label.
- [x] `FeatureTrait::rooms`, answered by both kinds; picking, the hover
      highlight and gizmo visibility all go through it rather than
      `drag_handle_bounds` and `"editor_room"`.
- [x] In `every_feature_type_survives_a_save_and_load`, with a corner
      measured from another feature.
- [x] Tests: an L, a pillar, two crossing squares (three regions), crossing
      slanted loops ~170 m out, a lost region, published points, mirroring,
      a corner following the point it references, and a map where the room
      entities, collision and picking agree on an L's notch.

### Stage 2 — drawing one

- [x] `Tools::Footprint`, a `Plugin` in `src/tool/footprint.rs`. The first
      click lands where the Room tool's would and sets the floor; every
      later one is where the pointer's ray meets that plane (a side view
      keeps the axis it cannot see from the last corner), snapped to 0.1 m.
      Shift-click on a feature's point adds a corner measured from it.
      Clicking within 0.3 m of the first corner, with three or more down,
      closes the loop; `Backspace` takes the last corner back and `Escape`
      drops the drawing.
- [x] Closing makes the feature with its region named, selects it and hands
      back to Select, as the Room tool does. The drawing is a plain `Draft`,
      so what a click does is tested without a mouse.
- [x] Drawn as placed: the lines so far, a dashed line to the pointer and a
      dashed closing edge, the first corner lit when a click would close,
      and the loop it would make hatched (the prop sketch's `hatch`).
- [x] `tools.footprint` in `en-US`.
- [x] Tried by hand in the editor.
- Only one loop per drawing: a pillar, or a second loop crossing the first,
  is adding lines to an existing footprint, which is Stage 3.

### Stage 3 — editing one

- [ ] Corner handles, dragged in the floor plane.
- [ ] Floor and ceiling handles.
- [ ] Adding lines to an existing footprint (the draw tool on a selected one),
      erasing a line, toggling which regions are rooms.
- [ ] Every edit one undo step (free from `checkpoint_when_idle`; checked).

### Later, not in this plan

- Moving `sketch.rs` and `region.rs` to `src/common/`.
- A floor or ceiling that slopes, which `Room` would need first.
- Per-region heights.
