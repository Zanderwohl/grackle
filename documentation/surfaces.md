# Surfaces: colouring and texturing a map

How walls, floors and ceilings get a material, and why a map ships as its
blueprint rather than as a compiled file. Tick items off as they land; the
checklist is the source of truth.

## Where it starts (October 2026)

Every room is baked with one shared `StandardMaterial` (`RoomBake::material`),
and `bake_faces` emits UVs nothing reads. Props colour themselves with a style
and a tint, four numbers on a `StandardMaterial`.

## What is wrong with the two usual answers

Quake and Hammer both have the mapper author **texture coordinates per face**.
Quake's are cheap — a world-axis projection with a shift, scale and rotation —
so everything lines up and nothing varies. Hammer's are expressive — a U and V
axis per face — which is exactly why curves hurt: the mapper is unwrapping by
hand, a face at a time.

The fix is not a better UV editor. **Nobody authors UVs.** It is also the only
answer that survives this engine: a room's faces are re-cut by face subtraction
on every bake, so a face has no stable name to hang a setting on, for the same
reason a BSP polygon in the prop kernel does not.

## Decisions

### Materials are texture-graphs, in metres

A material is a `.tgraph` ([texture-graph](../../texture-graph)) under a pack's
`materials/`, beside `weapons.toml`, named rather than embedded. Its scale is
stated in metres — a brick is 0.215 m by 0.065 m — never in texels per unit, so
there is no scale to retune per face.

### The system projects; the mapper does not

texture-graph samples `(u, v, w)`, so a surface samples its material at its
**world position**: a wall is cut out of a block of concrete. No seams, nothing
to align, and an odd shape costs nothing extra.

- Rooms are axis-aligned boxes today, so projecting along a face's own axis is
  exact. They will not stay boxes (see "Rooms will not stay boxes"), so the
  projection is written as triplanar from the start, with blending that
  collapses to one axis on an axis-aligned face — not as a per-axis special
  case that a sloped wall later has to be bolted onto.
- A material with a direction — planks, brick courses, siding — declares a
  **grain axis**, and its pattern's origin is a `PointRef`, so courses start at
  a room's `bottom_plane_center` and follow the floor when the room is resized.
  Alignment is anchored to features, not typed as numbers.
- **Noise is sampled in world space, never in the grain frame.** Only the
  structured pattern follows an anchor. A mirrored copy therefore samples
  different noise from its source, because it stands somewhere else, and never
  shows the source's rust pattern reflected — which reads as copy-paste at a
  glance.

### Materials are assigned to volumes, not faces

- **A room has three slots**: floor, walls, ceiling. Most maps are this.
- **A `Paint` feature** is a box of `PointRef`s and what to put inside it:
  everything within is that material. Later paints win, in timeline order. It
  survives resizing and re-cutting, works with the rollback bar, and saves,
  undoes and syncs through its own serde like every feature.
- **Trim, signs and stripes are decals** anchored to features, not UV nudges.

### Team-coded paint is a role; plain paint is a material

What a `Paint` (or a room slot) holds is one of two things:

- **A material**, applied directly. Side is not an input to it: a Mirror may
  copy the paint's box across, and the copy is the same material.
- **A role** — "Thin Material A", "Heavy wall" — resolved through the map's
  **palette**, a table in `MapMetadata` of what each role is on each side:

  | Role | Red | Blue | No side |
  | --- | --- | --- | --- |
  | Thin Material A | barn-red wood | corrugated blue steel | galvanised sheet |

  Which column applies is the paint's `Side`, and `assign_sides` is already its
  only writer: a role paint listed as a Mirror's source takes the first team,
  its copy the second, and deleting the Mirror puts both back on "No side". One
  half authored, two looks.

The palette lives in the metadata, not in the paint: a paint holding a
material per team would be a second writer of team identity. It also makes
reskinning a map one table.

A role changes *which* material; a material that merely leans towards the team
colour does that with a `side` parameter. Those are different jobs and stay
different mechanisms.

### Variation is inputs, not more tiles

A tile repeats because it knows nothing about where it is. A material's graph
gets named inputs, bound per fragment from geometry:

- height above the floor
- distance to the nearest edge or corner
- facing (how far a surface faces up)
- baked ambient occlusion
- a per-room seed
- the surface's `Side`

Each is a texture-graph parameter, so a graph that reads none of them is an
ordinary tiling material.

### A map ships as its blueprint; the bake is a cache

There is **no compiled map file**. What it was for, and where that went:

- *Flat runtime geometry* — an in-memory resolve: flat meshes and material
  assignments the game reads, so nothing at runtime walks features.
- *Baked textures and lighting* — a cache keyed by content hash, like
  `cache/thumbnail/`: derived, safe to delete, never authoritative. IndexedDB in
  a browser.
- *Art-heavy maps* — their weight is meshes and textures, which are pack
  content and travel as packs do; the map only names them.

Shipping graphs rather than pixels keeps a map small, which matters to a
browser tab, and it is the kkrieger trade: a few kilobytes of description, a
second of GPU on arrival.

`map_sync`'s JSON was knowingly temporary until "the compiled format". It now
waits on a **compact binary encoding of the blueprint** instead, parsed as
defensively as [sketch.md](sketch.md) asks of anything a server sends.

### Evaluator versions

A shipped graph's look is whatever the evaluator makes of it, so texture-graph
carries an **evaluator version**, separate from its file format version, and it
is part of every bake's cache key.

**It changes only at a major release.** Between majors, a change to the
evaluator must not change what any existing graph produces; a change that would
waits for the next major. A map then looks the same for the life of a major
version, and a cached bake is never shown for an evaluator that would no
longer produce it.

CPU and GPU results, and two vendors' GPUs, may differ slightly. That is
accepted: bakes are for drawing, and **nothing gameplay-related ever reads a
baked texel**.

### Live and baked paths

- **Live** (editor and between rounds): each material is baked once, by
  `texture-graph-gpu` on Bevy's own device, to a small tileable set — albedo,
  roughness, normal — keyed by hash. A room material samples it by the
  projection above and blends clean and worn per fragment from the geometry
  inputs, carried as vertex attributes from `bake_faces`. An edit changes
  vertices, not textures, so `RoomBake`'s incremental rebake stays cheap.
- **Full bake** (on load, cached): unwrap the resolved geometry and evaluate
  every graph per texel with its inputs, AO and decals into atlas pages.
  Today's geometry is axis-aligned rectangles, which chart trivially; the
  charting is still written against arbitrary planar polygons and grids, so it
  is not the thing that breaks when rooms stop being boxes.
- On wasm a bake cannot block on a device poll. A surface is drawn in its
  material's flat colour until its bake lands.

Props use the same materials, projected in object space; today's styles and
tints become the fallback for a prop that names none.

### Sky

**Sky** is a material a slot or a `Paint` can hold, like any other, and it is
how a map fakes being outdoors: a ceiling painted sky is open air. It is the
same idea as Quake's sky texture and Source's `toolsskybox`, and it comes last
on the checklist.

- **It is sampled by direction, not position.** The fragment shader samples
  the sky along the line from the camera to the fragment, so what a sky
  surface shows does not move as you walk — that is what reads as infinitely
  far away. Nothing is unwrapped and nothing is in the atlas.
- **The surface is still real.** It writes its own depth, so it hides what is
  behind it; that is the illusion. It still collides like any wall — a sky
  ceiling is a ceiling you cannot jump through.
- **One sky per map**, in `MapMetadata`, named like any material. It is a
  texture-graph baked to a cubemap — texture-graph already samples cube faces
  (`cube_sample`) — and cached like every other bake. A sky paint holds no
  graph of its own, so changing the sky changes every sky surface at once.
- **Sky is unlit.** It receives no lighting, AO or geometry inputs, and casts
  no shadow on itself. A sun derived from the sky is a later question.
- **Sky is never team-coded by itself.** A role may resolve to sky on one side
  and to a roof on the other, so it is just one more entry in the palette.
- **Every camera sees it the same way**, because the direction comes from
  whichever camera is drawing: the four editor viewports and the play camera
  need nothing per camera. In an orthographic viewport every pixel looks the
  same way, so a sky surface there is one flat colour. That is correct, not a
  bug to work round.
- **What hits sky is lost.** A projectile reaching a sky surface is removed
  without exploding, and a tracer ends there, as in Quake: a rocket going off
  against a sky reads as hitting an invisible wall. That is the one place game
  logic asks what material a surface is. It reads the *assignment*, never a
  baked texel, so it is as deterministic as collision is.

Out of scope: Source's 3D skybox (a scaled-down scene drawn behind the sky,
with parallax). A sky that is only a cubemap is the version that costs a
browser nothing.

### Rooms will not stay boxes

Not in this version, but planned: **Doom-style polygon rooms** (a floor
outline extruded between a floor and a ceiling height, possibly sloped) and
**Source-style displacements** (a face subdivided into a grid of offset
vertices), and likely other shapes after those. Nothing here may assume an
AABB where the assumption would have to be dug out later:

- **Projection** is triplanar from day one (above).
- **Assignment is by volume**, which is why `Paint` is a box in the world
  rather than a face index: a polygon room's walls and a displacement's
  vertices fall inside a box the same way a box room's faces do. A `Paint`'s
  own volume may later grow beyond a box; its contents rule does not change.
- **Room slots** are floor, walls and ceiling, which a polygon room still has.
  Which a surface counts as is decided by its **normal** — up-facing is floor,
  down-facing is ceiling, the rest is wall — not by which side of a box it is.
- **Geometry inputs** are stated per vertex from the mesh, not from a room's
  extents: "height above the floor" is height above the floor beneath that
  point, and "distance to an edge" is measured on the surface. A displacement's
  vertices then get the same inputs any vertex does.
- **Displacements want one more input**: a per-vertex blend weight the mapper
  paints (Source's alpha), for grass into dirt across a hillside. It is a vertex
  attribute like the others and needs nothing new from the material.

`bake_faces` is the single place that turns a room into triangles and inputs,
so a new room shape is a new producer of that mesh, and the material side
should not need to know it exists.

## Checklist

- [ ] texture-graph: an evaluator version, separate from `CURRENT_FORMAT_VERSION`
- [ ] Materials under a pack's `materials/`, loaded through `AssetSource`
- [ ] Bake a material to a tileable set on Bevy's device, cached by hash and evaluator version
- [ ] Room floor / wall / ceiling slots (classified by normal), triplanar world-space projection
- [ ] Grain axis with a `PointRef` anchor
- [ ] Geometry inputs as vertex attributes; clean/worn blend
- [ ] `Paint` feature: a box and a material, timeline-ordered
- [ ] Roles and the palette in `MapMetadata`; role paint resolved by `Side`
- [ ] Decals anchored to features
- [ ] Props: materials in object space, style and tint as fallback
- [ ] Full bake to atlases, cached on disk / in IndexedDB
- [ ] Non-blocking bake on wasm, flat colour until ready
- [ ] Binary blueprint encoding for `map_sync`, parsed defensively
- [ ] Sky: the map's sky graph baked to a cubemap; a sky material sampled by view direction, unlit
- [ ] Sky: projectiles and tracers lost against a sky surface
- [ ] Later, not this version: polygon rooms, displacements and their blend weight
