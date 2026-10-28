# Blocks

A map feature that is solid where a room is air: a crate, a pillar, a ledge,
a wall standing in the middle of a hall. Hammer's brushes turned inside out —
Hammer's world is void with brushes in it, so a gap is a leak; here the world
is sealed air with blocks in it, so a gap is more wall. This is the plan and a
checklist; the checklist is the source of truth for what is done.

## Where it starts from (October 2026)

- **The map is the union of its rooms** ([collision.md](collision.md)):
  [`world_surface`](../src/common/world_surface.rs) cuts each room's walls
  against every room touching it, faces pointing into the air, and the bake,
  collision, picking and the sun all read those faces.
- **A `Room` is a prism with holes** ([`src/tool/room.rs`](../src/tool/room.rs),
  [footprint-rooms.md](footprint-rooms.md)). A hole is a full-height pillar
  belonging to one room: the only solid inside a room there is.
- **Ties go by box.** Where two rooms' faces coincide facing the same way,
  `precedes` picks the lower bounding box, so a share does not depend on
  which room is being rebuilt or on query order.

## Decisions

- **The map is a sequence of volumes, last writer wins.** Every room and
  block is a volume in feature order; a point is air when the *last* volume
  containing it is a room. So a block cuts the rooms above it, and a room
  below a block tunnels back through it. A block before every room is
  nothing.
- **It cannot leak.** Air is only ever made by a room, so every point of air
  is inside some room's closed volume, and the boundary of air is made of
  room and block faces. A block outside every room adds nothing; one
  crossing a wall is cut off at the wall, sealed.
- **A face is drawn where the air changes across it, by its volume — unless a
  later volume's face lies on it.** For a fragment of volume `V`'s boundary,
  inside `V` on one side and outside on the other: it is dropped if it lies
  inside or on the boundary of any later volume (that volume is the last
  writer there, and owns any face on it); otherwise it is drawn if `V`'s fill
  differs from the last earlier writer outside it (void counts as solid),
  facing the air. That makes coincident faces exactly one owner — the later
  feature — and replaces `precedes`.
- **A block is the `Room` prism with a different fill**, so footprint
  blocks, holes and angled sides come for free. It is its own component
  (`Block`), never a `Room`: everything that queries `&Room` today means
  air, and a block found there would be air.
- **Order is a key, not a query's order.** Every volume carries its place in
  the timeline; `Query` iteration order is archetype order and would make
  a share depend on which room was last touched. A Mirror's copies take
  their **source's** place, not the Mirror's — a copy placed at the Mirror
  would come after a room that tunnels through the original, and the two
  halves would differ.
- **Containment is one function.** `world_surface::is_air` is the volume
  truth: `inside_map`, the spawn fallback and depenetration's "which way out
  is into the level" all ask it, and a future BSP's leaf classification must
  agree with it.
- **Nothing treats a room as a cell.** A block can split one room into two
  disconnected pockets. Zones, navigation and visibility are derived from
  the final air, never from rooms.

## Later, and how this stays open to it

- **BSP and visibility.** The surface is closed and faces into the air,
  which is exactly what a leaf/portal BSP is built from, and sealed by
  construction, which is what a flood fill needs. Whole-map, baked on
  entering Play like the sun, not incremental.
- **Zones and area portals** from the air volume (BSP leaves or a coarser
  partition of them), never from rooms.
- **Navigation** from the walkable faces of the surface — a block's top is a
  floor with nothing more said.

## Checklist

### Stage 1 — the volume expression

- [x] `csg::partition`: a polygon set split into the parts inside a solid
      and the parts outside, coincident faces by facing.
- [x] `world_surface::Volume` (a prism and a `Fill`) and `share`, by the
      rule above. `room_share`/`of_rooms` order rooms by the old box key, so
      a rooms-only map is unchanged; every existing surface test passes.
- [x] `world_surface::is_air`.
- [x] Tests: a block on a floor (no floor under it, no face on its bottom),
      a block crossing a wall (sealed, nothing outside), a corridor after a
      block tunneling through it, a block before every room adding nothing,
      a block filling a room, a block flush in a corner.

### Stage 2 — a feature, in order

- [ ] A timeline place on every volume entity; reordering re-queues what the
      moved feature touches, as an edit does.
- [ ] `EditorBlock` feature standing up a `Block` component, drawn by the
      Room tool under its own `Tools` state as triggers are; gizmos, handles,
      `show.rs`, `find_nearest_feature_hit`, `mirror`, the save test.
- [ ] The bake, auto-bake, collision rebuild, picking and the sun gather
      rooms and blocks in timeline order and call `share`; the box-key
      wrappers go.
- [ ] A block's share is baked onto its own entity, with `RoomSurfaces` and
      a `Side`, so it takes materials and palette roles; `RoomSlot::of`
      already sorts a block's top as floor.

### Stage 3 — collision

- [ ] `CollisionWorld` keeps the volumes, and `inside_map` is `is_air`.
- [ ] Spawn fallback: the largest room's `floor_point` may be in a block;
      take a room point that is air.
- [ ] Wall slabs no deeper than the solid behind them: a 0.5 m slab behind
      one side of a 0.2 m block stands 0.3 m out of the other.

### Stage 4 — light

- [ ] The sun bake keys light by the share's entity rather than by `Room`,
      so a block gets a lightmap and casts its shadow.

### Open

- A block touching a room's wall from outside, flush, owns that patch of
  wall by the later-wins rule and paints it with its own material. Correct
  by the rule, odd to look at; revisit if anybody hits it.
