# Sun: daylight that comes in through the sky

A map is outdoors where a slot is painted `skybox`. The sun should light what
it can see through those surfaces and nothing else, and the rooms around a
sunlit one should be brighter for it. This is the plan; the checklist is at the
bottom.

## Where it starts (October 2026)

- Rooms are lit by their `GracklePointLight`s and one `GlobalAmbientLight`
  (white, brightness 100, in `main.rs`) that is the same in every corner of
  every map.
- `skybox` slots draw `SkyMaterial`, colored by the map's `SkyTime`, and are
  `NotShadowCaster`. Nothing else knows they are sky.
- `BakeSystems::All` has one member, the room bake.

## Why not a real directional light

A Bevy `DirectionalLight` lights every surface facing it unless its shadow map
says something is in the way. The map's surface faces **into** the rooms, so
seen from the sun every wall and ceiling is a back face, culled from the shadow
pass, and the sun lights every floor in the map. Casting shadows from both
sides would fix that, at the price of acne and leaks at every corner of
zero-thickness walls, and of cascades that only reach so far from the camera.
On a big map, the distance is exactly where the sun would be wrong.

So the walls are **baked** and only things that move get a real light.

## Decisions

### The test is "is the first surface toward the sun sky"

The union of the rooms is closed, so a ray leaving any point inside it hits the
surface somewhere. A point is sunlit when the first face it hits going toward
the sun is a `skybox` face. That is the whole rule, and it is why light only
comes in through sky: there is no other way out.

Rays are cast against the **whole map's surface** (`world_surface::of_rooms`),
never one room's. A ray does not know which room it started in, so light comes
through a doorway, down a corridor and across a threshold with nothing to
stitch. That is what "correct across the joins" means here, and it costs
nothing extra.

A face needs to know whether it is sky, which is a fact about **paint**, not
geometry: the slot by normal, the room's `RoomSurfaces`, then the palette by
`Side`, the same resolution `paint_rooms` does. So repainting a ceiling
re-bakes the sun even though no geometry moved. That resolution has to be one
function both call, not two that agree today.

### What is baked, per texel

Bevy's `Lightmap` is a texture of diffuse indirect light on a mesh's second UV
set, and it **replaces** the ambient term on that mesh
(`GlobalAmbientLight::affects_lightmapped_meshes = false`). Point lights keep
lighting lightmapped meshes in real time (`affects_lightmapped_mesh_diffuse`
stays `true`). So the lightmap holds everything that is not a point light:

1. **Sun**: visibility × `N·L` × the sun's color. A few jittered rays per texel
   inside the sun's disc give a soft edge, and supersampling the texel keeps a
   shadow edge from stepping at texel size.
2. **Sky**: cosine-weighted rays over the hemisphere, each hitting sky adding
   the sky's own color in that direction (`SkyTime::color_at`, which already
   exists). This is ambient occlusion toward the sky: bright under an open
   roof, falling off into a building, and it uses the gradient instead of a
   guessed constant.
3. **One bounce**: those same hemisphere rays that hit a solid surface ask
   whether *that* point is sunlit (one more ray) and add a fraction of it, at a
   fixed gray albedo. That is what makes a room off a sunlit courtyard
   brighter. It stops there: one bounce, a fixed number of rays, no recursion.
   It is not path tracing.
4. **A floor**: the map's base ambient, so a room no light reaches is dim and
   not black. This is what `GlobalAmbientLight` becomes, and it moves into the
   map's settings.

**This is per texel, not per room.** A per-room ambient would be cheaper but
steps at every threshold, and a doorway between a bright room and a dim one is
exactly where a player looks. With per-texel rays the step does not exist to
hide. If the gather turns out too slow, the fallback is a per-room value
blended across openings, not a flat one.

### Two resolutions

Sun shadows need detail (a skylight's edge on a floor); sky and bounce are
smooth by nature.

- **Sun** at `SUN_TEXEL` = 0.25 m, a handful of rays per texel.
- **Sky and bounce** at 1 m, many rays per sample, interpolated up onto the sun
  lattice when the two are combined.

That is what keeps the bake affordable: the demo scene is about 1700 m² of
surface, so roughly 27 000 sun texels × 4 rays plus 1700 ambient samples × 64
rays, a couple of hundred thousand rays, against a grid. Native, that is well
under a second. Measure it before deciding anything about threads.

### The texel lattice is the world's

Each face is charted in its own plane, on a lattice fixed in **world space**
(as material UVs already are), not one starting at the face's corner. Two
rooms' coplanar floors meeting at a threshold then sample the same points, and
the seam disappears rather than being blended over. Charts get a texel of
padding, dilated from their edges, so bilinear filtering does not pull in a
neighboring chart, and a texel sample is pushed inside its face and off it
along the normal, so a corner texel does not start its rays inside a wall.

One atlas **per room**, packed from that room's faces. Each slot mesh of the
room gets a `Lightmap` pointing at it, with `ATTRIBUTE_UV_1` into it.

### The room bake owns the layout, the sun bake owns the pixels

A room's chart layout depends only on its own faces, so `bake_one` writes
`UV_1` as it builds the mesh, without knowing about the sun. What lands in the
atlas depends on every room, so it is the second bake.

### Light is replaced, never dropped

A room's light is kept by its **`Room` entity**, which outlives every rebuild
of its meshes, not on the mesh entities the room bake throws away. When a room
is rebuilt its UVs move, so the light it had is **carried onto the new layout**
(`Atlas::carry_over`): charts in one plane share a lattice, so each texel is
copied from the old chart in the same plane, or the nearest parallel one — a
dragged wall keeps its light — with no rays, the frame the room changes. Only
surface with nothing parallel before starts at the base. It stays until a bake
lands, and only the Bakes panel's clear takes it away. Clearing the rooms does
not: rebuilt, they come back lit.

### When it runs

Any change to a room's shape, a sky assignment, the sky time or the sun's
position can change light anywhere: open a skylight and a room down the corridor
gets brighter. So there is no incremental sun bake; a change invalidates the
whole map.

- **Entering Play always bakes**, in `BakeSystems::All`, after the room bake. A
  round is never played with stale light. Entering Play rebuilds every room, so
  a map unchanged since its last bake takes that bake back, matched room by
  room on a hash of its faces and sky, rather than paying for it again.
- **Auto-bake in the editor** is its own checkbox in the Bakes panel, waits
  until the map has been still for a moment rather than baking every frame of
  a drag, and is **off if the bake is slow**, decided by measuring it.
- **The result is a cache** keyed by a hash of what it read — faces, sky
  assignments, sky time, sun position, base ambient, the bake's own version — under
  `cache/lightmap/`, like thumbnails. A client handed a map the host already
  baked finds it on disk instead of baking it again. Never authoritative,
  never sent, safe to delete.

It runs on the `AsyncComputeTaskPool`, one bake at a time, from a copy of the
faces (`Job`), so the editor stays usable; a change made while one runs is baked
after it lands. Entering Play waits only when the light is stale — for the bake
already running if it is of this map, otherwise for its own. On wasm the pool
runs a task on the main thread in one go, so there it still hitches: chunking
it across frames is left to do.

### The sun is a map setting

**Altitude and azimuth** are map settings of their own (`SunPosition`), not
derived from `SkyTime`: matching the sun to the sky is the mapper's call, and
skies will grow beyond four presets. So is fairness — a low sun lights the
two halves of a mirrored map differently, and the mapper decides whether that
matters for their mode.

Azimuth is degrees clockwise from north (`-Z`) seen from above, so 90° is
east (`+X`); altitude is degrees above the horizon.

### Things that move

Bodies, corpses, projectiles and placed props are not lightmapped.

- **Sun** is a real `DirectionalLight` on its own render layer
  (`render_layers::SUNLIT`) that no wall is on, so it can never leak indoors
  or put a glint on a wall in the shade. A body joins the layer while a ray
  from its chest reaches sky, which is one ray per body per frame against the
  same accelerator. It is a switch, not a fade: a body half out of a doorway is
  one or the other. Bodies cast no shadow on a sunlit floor, since the floor is
  not on that layer.
- **Ambient** is a coarse grid of the same sky-and-bounce gather, a few meters a
  voxel per room, sampled on the CPU at each body and handed to its shader in a
  `MeshTag`. Bevy's irradiance volumes would do this, but they are disabled
  outright on wasm (`IRRADIANCE_VOLUMES_ARE_USABLE`), and a browser is the
  target. Until this lands, things that move keep `GlobalAmbientLight`.

### Nothing reads a texel

As [surfaces.md](surfaces.md) says of every bake: nothing gameplay-related reads
a baked texel. Whether a body is in the sun decides how it is drawn, never
whether it can be seen.

### Two things Bevy 0.20 does not do

Adding or removing a `Lightmap` does not re-specialize a mesh's pipeline: only
a changed mesh or material does, and a mismatch is a wgpu validation error that
stops the frame. And a lightmap's image is read only when the component first
arrives. So `attach_lightmaps` marks the material changed whenever it adds or
removes one, and a bake that lands on a room whose layout is unchanged writes
into the image already drawn rather than handing it a new one — Bevy writes a
same-sized image into its existing texture — so no slot goes a frame unlit.

## Checklist

- [x] One function resolving a slot to its material name (`Palette::paint`), shared by `paint_rooms` and the bake
- [x] A ray accelerator over the map's faces (`sunlight::LitSurface`, on the collision `Grid`, now in `common`)
- [x] Sun altitude and azimuth in the map settings
- [ ] Base ambient as a map setting, replacing `main.rs`'s fixed `GlobalAmbientLight`
- [x] Charting: faces onto a lattice fixed by their plane, packed per room, padded; `UV_1` written by `bake_one` (`common/lightmap.rs`)
- [x] Sun visibility per point, across the sun's disc (`LitSurface::sun_reaching`)
- [x] Sun visibility per texel, at its center; supersample if edges step
- [x] Sky gather and one bounce at 1 m, combined onto the sun lattice (`LitSurface::ambient`)
- [x] Entering Play on an unchanged map reuses the last bake rather than baking again (light is kept by room)
- [x] Lightmap images per room, `Lightmap` on each slot mesh, removed when a room is rebaked (`tool/sun_bake.rs`)
- [x] The sun bake in `BakeSystems::All`, after rooms; a Bakes-panel checkbox for auto, debounced
- [x] Measure it on the demo scene: 3 rooms, 34 583 texels, 240 ms in a debug build with the sun alone and 740 ms with sky and bounce, so auto-bake is on
- [x] Off the main thread, on the async compute pool; light kept and carried over until a bake replaces it; a clear beside the Sunlight button
- [ ] Chunked across frames on wasm, where the pool runs a task in one go: chunks of 8 ms through `BackgroundBake`, not yet run in a browser
- [ ] Cache under `cache/lightmap/` by content hash
- [ ] `DirectionalLight` on a `SUNLIT` layer; bodies join it by a ray from the chest
- [ ] Ambient grid per room, sampled per body into a `MeshTag`
- [ ] Tests: a closed room is unlit; a sky ceiling lights the floor under it and not the room next door, except by bounce; coplanar floors across a threshold bake the same values on both sides
