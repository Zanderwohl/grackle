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

A room rebaked by the room bake **loses its `Lightmap`** until the sun bake
catches up: its UVs moved, and an old atlas on new UVs is noise. It falls back
to the map's base ambient, which looks unfinished rather than wrong.

### When it runs

Any change to a room's shape, a sky assignment, the sky time or the sun's
heading can change light anywhere: open a skylight and a room down the corridor
gets brighter. So there is no incremental sun bake; a change invalidates the
whole map.

- **Entering Play always bakes**, in `BakeSystems::All`, after the room bake. A
  round is never played with stale light.
- **Auto-bake in the editor** is its own checkbox in the Bakes panel, waits
  until the map has been still for a moment rather than baking every frame of
  a drag, and is **off if the bake is slow**, decided by measuring it.
- **The result is a cache** keyed by a hash of what it read — faces, sky
  assignments, sky time, heading, base ambient, the bake's own version — under
  `cache/lightmap/`, like thumbnails. A client handed a map the host already
  baked finds it on disk instead of baking it again. Never authoritative,
  never sent, safe to delete.

Natively it runs on the `AsyncComputeTaskPool` so the editor stays usable. On
wasm there are no threads, so it is chunked across frames like the room bake.
That is why the work is a plain function over faces with nothing held across
it.

### The sun is a map setting

Its **elevation and color come from `SkyTime`**: high noon nearly overhead,
day at about 50°, evening low and orange (long shafts through windows), night a
dim blue moon. Its **heading** is a new map setting, in degrees, because which
way the shafts fall is a mapper's decision.

A low sun on a mirrored map lights the two halves differently, which is a
fairness question for CTF. Nothing enforces anything: high noon puts the sun
near the zenith, and that is the default to suggest for symmetric maps.

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

## Checklist

- [ ] One function resolving a face to its material name, shared by `paint_rooms` and the bake; "is sky" from it
- [ ] A ray accelerator over the map's faces, shared by the bake and the per-body sun ray (the collision `Grid` moved to `common`)
- [ ] Sun direction and color per `SkyTime`; a sun heading in the map settings
- [ ] Base ambient as a map setting, replacing `main.rs`'s fixed `GlobalAmbientLight`
- [ ] Charting: faces onto a world-aligned lattice, packed per room, padded; `UV_1` written by `bake_one`
- [ ] Sun visibility per texel, supersampled, soft edge
- [ ] Sky gather and one bounce at 1 m, combined onto the sun lattice
- [ ] Lightmap images per room, `Lightmap` on each slot mesh, removed when a room is rebaked
- [ ] The sun bake in `BakeSystems::All`, after rooms; a Bakes-panel checkbox for auto, debounced
- [ ] Measure it on the demo scene; decide auto-bake's default from the number
- [ ] Off the main thread natively, chunked per frame on wasm
- [ ] Cache under `cache/lightmap/` by content hash
- [ ] `DirectionalLight` on a `SUNLIT` layer; bodies join it by a ray from the chest
- [ ] Ambient grid per room, sampled per body into a `MeshTag`
- [ ] Tests: a closed room is unlit; a sky ceiling lights the floor under it and not the room next door, except by bounce; coplanar floors across a threshold bake the same values on both sides
