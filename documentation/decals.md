# Decals: marks on the map's surface

Bullet holes, scorch marks, blood, sprays, and the grime and signs a mapper
puts down to break up a plain wall. This is the plan; the checklist is at the
bottom.

## Where it starts (November 2026)

- The map's surface is `world_surface` faces, baked per room into
  `BakedRoomGeometry`, with a `UV_1` lightmap chart per face and a lightmap
  image per room from the sun bake.
- Shots, blasts and hits reach every machine as `Effect`s and `DamageDealt`
  records. Nothing marks a wall.

## Why not Bevy's decals

Bevy 0.20 has two kinds:

- **`ClusteredDecal`** projects textures inside the clustered-forward pass. It
  needs binding arrays, which `clustered_decals_are_usable` turns off on Metal,
  and bindless textures, which WebGL 2 and WebGPU do not have. It does not run
  on macOS or in a browser.
- **`ForwardDecal`** is a quad that reads the depth prepass. Every camera that
  draws one needs a `DepthPrepass`: the play camera, the viewmodel camera, the
  four editor viewports and the forward camera. On wasm it needs WebGPU with
  MSAA off. It paints whatever is in the depth buffer, so a body walking past a
  scorch mark is scorched, and each decal is a sorted transparent draw.

## Decisions

### A decal is a mesh cut from the wall triangles

A decal is a box: a center, a direction it projects along (into the wall), an
up, a width and height, and a depth. Placing one takes every baked face that
overlaps the box and faces against the projection, cuts it to the box's six
planes with the CSG kernel's plane split, and keeps what is left. Texture
coordinates are the vertex's place across the box's face. Nothing here reads a
room's min and max, so a footprint room or a later displacement needs no
change (see [surfaces.md](surfaces.md)).

A decal is cut into one **piece** per room it lands on, so each piece can take
that room's lightmap: every cut vertex lies on a face, so its `UV_1` is that
face's chart at that point, exactly. A piece is lifted a millimeter off the
wall and drawn with a depth bias so it does not flicker against it.

### Two ways to draw

- **Multiply, unlit**: bullet holes, scorch and blood. The result is the wall
  as it was drawn, lit, times the decal's color, so the decal needs no light of
  its own and overlapping marks come out the same in any order. A multiply can
  only darken, which is all these do. No lightmap and no sorting.
- **Blend, lit**: sprays and the map's decals, which can be lighter than the
  wall. These take their room's lightmap the way a wall slot does, so a spray
  in a dark corner is dark.

### Three lifetimes, each a marker, and relations for what a decal depends on

| Lifetime | Marker | Examples | Ends when |
| --- | --- | --- | --- |
| Map | `MapDecal` | Grime, signs, team logos | Never: it is a feature of the map |
| Owner | `Spray` | Sprays | Its player disconnects, or the round ends (leaving Play) |
| Transient | `Transient` | Bullet holes, scorch, blood | Its kind's cap pushes it out, it fades out, or the round ends |

- A piece is `DecalPieceOf` its decal (`linked_spawn`, so a decal takes its
  pieces with it) and `PaintedOn` the room it was cut from, so a room's walls
  going away take the pieces on them.
- A room rebaked in place changes its `BakedRoomGeometry`: map decals and
  sprays touching it are cut again; transient ones there are dropped, which is
  cheaper and nobody misses a bullet hole on a wall that just moved.
- A slot painted a material that refuses decals (`decal::takes_decals`: only
  `skybox` so far) is left out of every cut. Each `BakedRoomGeometry` carries
  `TakesDecals`, written only when a repaint changes it, and that change recuts
  every decal on the room, transient ones too, so marks on its other slots
  stay. A mark whose ray first meets a refusing face is not made at all, so it
  cannot push a real one out of its ring.
- On the server a spray is `SprayedBy` its client's link entity, with
  `linked_spawn`, so a disconnect despawns it and the despawn replicates.

### Transient decals are drawn, not sent

Every machine already receives what it needs, so nothing new crosses the
wire, and each machine may keep a different number:

- **Bullet hole**: an `Effect::Tracer` that stopped on the world. The wall is
  found by casting a short ray from just before its end.
- **Scorch**: an `Effect::Blast`. The nearest surface within the radius that
  takes decals takes a scorch the size of the blast.
- **Blood**: a `DamageDealt` on a body from a shot or a blast, followed on
  along the shot's line (for a blast, out from its center) onto the first
  surface within 5 m, and nothing if there is none. A killing hit also leaves
  blood on the floor under where it landed, whatever killed.
  A piece of a body blown apart striking a surface faster than 2 m/s
  (`GibImpact`, from the ragdoll solver every machine runs) leaves blood
  there, larger the harder it struck, up to 10 m/s.

Each kind keeps a ring of the newest, as many as this machine's player
settings allow (`PlayerSettings::debris`, 0 to 512 each; 200 bullet holes, 64
blood and 32 scorch by default), oldest out first, and each decal fades
out over its last seconds. All of them are cleared on entering and on leaving
Play.

### Drawn once before it is needed

Bevy compiles a pipeline on the frame something is first drawn with it, and on
macOS and wasm it does so on the main thread: the first bullet hole of a
session was a visible hitch. `warm_up_decals` draws one zero-area piece of
every decal material, and a blended one with a lightmap, through a camera set
up as the play camera is, for a few frames at startup, as the lens warm-up in
`forward_camera` does.

### Sprays

- `T` places your spray, or moves it if you have one. Where is
  decided on the authority from the body's eye and aim, at most 2 m away.
- The picture is 512×512 RGBA, made by the client: a disk in its team's color
  (else its own color, else neutral) with its name over it in black. It is
  remade when either changes. It goes to the server as a PNG, and the server
  sends every picture it holds to every client, with an owner number it
  assigned. There is no way to choose another picture yet.
- A placed spray is a replicated entity carrying its owner number and where it
  is; every machine cuts it from its own walls once it has that owner's
  picture. Leaving Play clears every spray, as it does the transient marks.

### Textures

Bullet holes, blood and scorch are SVGs in `assets/default/decals/svg/`,
rasterized to PNGs beside them by `cargo run --bin decal_images` with `resvg`,
which is already in the tree through `egui_extras`. A map decal names a PNG
under a pack's `decals/`; a texture-graph material is to come.

## Checklist

- [x] Plan
- [x] SVGs for bullet holes, blood and scorch, and the bin that rasterizes them
- [x] Clipping a box against baked faces into pieces, with `UV_0` and `UV_1`
- [x] Relations and lifetime markers; pieces dropped or recut with their room
- [x] Transient decals from tracers, blasts and hits, with caps, fading and round reset
- [x] Lightmaps on blended pieces
- [x] Spray pictures: made by the client, sent to the server, relayed with an owner number
- [x] `T` places or moves a spray, 2 m at most, replicated, gone on disconnect
- [x] Map `EditorDecal` feature, its tool and mirroring; a picture dropdown of every pack's `decals/`
- [ ] Turn a new map decal onto the surface it was placed on (it starts pressed onto the floor)
- [ ] Map decals naming a palette role or a texture-graph material
- [x] The transient caps as a player setting (`DebrisDecals`, 0 to 512 each, in the Player tab)
