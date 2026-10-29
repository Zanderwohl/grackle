# Bots: where a body can go, and what each spot is worth

Bots need to know where they can stand, how to get from one place to another,
and what each place is worth: what you can see from it, who can see you, what
is there to pick up, and whether a blast there throws you into a pit. Mappers
need the same answers, drawn over the floor, so they can see what a wall they
just placed did to cover and sightlines, and build hot zones on purpose.

This plan covers the **nav graph and the fields baked onto it**, plus the
overlays that show them. The bot itself (the brain, aim, the loop that turns
fields into `PlayerInput`) comes after; only the interface it will read is
fixed here. The checklist is at the bottom.

## Where it starts (October 2026)

- No bot, nav or pathing code exists.
- `world_surface::of_volumes` is the boundary of the map's air: convex
  polygons facing into it. `CollisionWorld` stands each one out as a hull,
  and `fits(center, half)` answers "can a body stand here" (walls, low
  ceilings and solid props).
- `lightmap::Atlas` charts faces onto a lattice fixed by each face's plane,
  so floors meeting at a threshold sample the same points.
- `BakeSystems::All` holds the room bake and the sun bake. The sun bake runs
  on the async compute pool, keyed by a hash of the faces, and keeps the old
  result until a new one lands.
- Movement: `STEP_HEIGHT` 0.45 m, `WALKABLE` 0.7, `JUMP_SPEED` 7 m/s,
  sprint 7 m/s, gravity per map (`MovementRules`). The body box is
  0.7 × 2.0 × 0.7 m, with the eye at 1.8 m standing and 0.88 m crouched.
- `step_player` has its physics written inline in the system. Nothing outside
  the fixed loop can ask "where would this input take this body".
- The test map is `arena_halls.gmb` (user pack). It is one authored quarter
  turned four ways. It has a BFG platform ringed by a 5.7 m gap over an 8 m
  drop to a Kill trigger, a sunken area 1 m deep, a 0.4 m lip, weapon
  alcoves at −1 m, and health in each quarter.

## Decisions

### Floor cells, not navmesh polygons

The graph is a set of **cells**, each a point on a floor with a body's worth
of room above it, and **links** between them. There is no polygon navmesh.

Everything this plan bakes is a number per spot, and everything a bot does
with it is "which neighbor is better". A cell lattice carries that directly.
A polygon navmesh would need a second structure to hold the fields anyway,
and its polygons are too coarse for a sightline gradient across a room.

At 0.5 m spacing Halls is about 3 800 cells. A cell is a few dozen bytes plus
its links, so memory is not a concern at arena scale. If long paths on a big
map ever get slow, regions (connected groups of cells) can be layered on top
for hierarchical search. The cells stay the truth.

### Cells come from the surface, not from voxels

Recast voxelizes a triangle soup to find which surfaces are floors and to
discard the outside of the level. Neither problem exists here. The surface is
exact and every face of it bounds air, so **every face with
`normal.y >= WALKABLE` is a floor someone can stand on**, and there is no
outside to leak into.

A cell is a point on such a face, on the face's plane lattice
(`lightmap`'s charting, at nav spacing), where
`fits(body_center_from_feet(point), PLAYER_HALF)` holds. That one test is
the agent-radius erosion and the headroom check together, and it is the same
test spawn points use, so a cell and a spawn cannot disagree about whether a
body fits. The BSP pieces of a floor are sampled through rather than used as
cells: their shape is wherever the kernel happened to cut.

Floors are flat today. When displacements arrive, a sloped face is just
another face with `normal.y >= WALKABLE` and needs no new code here. That is
the rule from CLAUDE.md: consume faces and normals, never a room's box.

A cell whose body box overlaps a **Kill** trigger is not a cell. One that
overlaps a **Hurt** trigger is a cell carrying that damage rate. Cells no
spawn point can reach are dropped after linking. A sealed room, or the floor
of a pit, is not somewhere a bot plans to be.

### Links are what the physics allows, found by running the physics

| Link | Found by | Notes |
| --- | --- | --- |
| Walk | `slide` between lattice neighbors on one level | |
| Step | `step_up` (≤ `STEP_HEIGHT`) | Halls' 0.4 m lip |
| Drop | Walk off an edge, simulate the fall | One way. Lands on a cell or in a Kill volume (then no link) |
| Jump | Simulate a jump from edge cells, a few headings and run-ups | Halls' 1 m divot, up |
| Rocket jump | As a jump, with an `Impulse` at takeoff | Later. Halls' BFG needs it |

Jumps and drops must be **simulated with the real step**, not with a
ballistic formula. A second description of how a body moves is one that
drifts from the first, and a bot that plans a jump it then cannot make walks
into the pit. So the first piece of work is to pull the body of
`step_player` out into a pure function:

```rust
fn step(body: &mut Mover, input: &PlayerInput, world: &CollisionWorld, rules: &MovementRules, dt: f32)
```

The system calls it, the baker calls it, and later the bot's own lookahead
calls it. Its determinism is what prediction already depends on, so the bake
inherits it for free.

By that arithmetic, Halls' gap is about 0.8 m too wide for a sprinting jump
from rest. That is the right answer: the BFG should cost something. The bake
will say so rather than the plan.

Each link stores its kind, its travel time (simulated, not distance), and the
input sequence that makes it, so a bot replays it rather than re-deriving it.

### Fields, baked per cell

All fields are stored in **absolute units**, never normalized to the map.
Normalized, placing one wall recolors every cell in the map, and the overlay
stops answering "what did this wall do".

**Visibility** is the expensive one. For cell pairs within range, cast rays
from A's eye to sample points on B's body (head, chest, knees). Visibility is
symmetric for two eyes at the same height but not for eye-to-body, and that
asymmetry is where high ground comes from.

- **Sight**: the floor area (m²) whose standing bodies I can see from here,
  weighted by how much of each body.
- **Exposure**: the floor area (m²) from which my body can be seen, weighted
  by how much of it.
- **Advantage**: sight − exposure. High ground is not a separate input. It
  falls out here, because from above you see whole bodies while they see your
  head.
- **Crouched variants** of sight and exposure, at the crouched eye and box.
  A ledge that hides you crouched but not standing is a peek spot.
- **Cover nearby**: the lowest exposure among the cells reachable within
  about 1.5 s. A cell with high sight next to one with low exposure is a peek
  spot, which is what "hugging corners" means as a number.

Rays go against `CollisionWorld::ray_distance`, the same test hitscan uses,
so a sightline the overlay shows is a shot the game allows. Solid props
block sight. Pickups and bodies do not.

Range: rays beyond a cap (start at 40 m) are not cast. Long-range exposure
mattering less than short-range is a weighting question (see "Open
questions").

**Danger**:
- **Knockback**: from each cell, give the body a blast's push (a typical
  rocket's `knockback`, in 8 directions) and simulate with `step` until it
  lands. Danger is the fraction of directions that end in a Kill volume, a
  Hurt volume, or a fall to no cell. The ring around Halls' BFG platform
  lights up here.
- **Hurt**: the trigger's damage rate, for cells inside one.

**Pickups**: one shortest-path distance field per pickup, in travel time over
the links, so "go to the rocket launcher" is reading which neighbor is
closer. Pickups are map features and do not move. Whether one is *worth*
going for (taken, restock time, what the bot needs) is decided at runtime,
not baked.

**Height**: the cell's height above the lowest cell that can see it. This is
mostly redundant with advantage and is cheap. Keep it for the overlay, where
mappers will ask for it by name.

### What is baked, and when

`NavBake` is the third member of `BakeSystems::All`, after rooms. It is a
`BakeJob` run by `BackgroundBake` ([`src/tool/bake_job.rs`](../src/tool/bake_job.rs)),
as the sun is, and reports as `BakeKind::Nav`:

- The input is a snapshot (faces, volumes, solid props, triggers, pickups,
  spawns, `MovementRules`), hashed. The job runs on the async compute pool
  from the copy.
- **The old graph is kept until a new one lands.** Bots and the overlay use
  stale data rather than none, and an edit between rounds never leaves a bot
  standing still.
- Never incremental. Visibility is global: a wall changes sightlines across
  the whole map.
- Auto-bake is on, debounced, waiting for the map to be still, like the sun's.
  Entering Play waits only if the graph is stale.

Cells and links are cheap (seconds at most in a debug build). Visibility is
not: about 3 800 cells is about 7 M ordered pairs before the range cap. So
the bake **lands in two stages**. First cells, links, danger and pickup
fields. Then visibility. The overlay shows the first stage as soon as it is
ready and fills in sightlines after. Visibility at 1 m spacing (about 950
cells), interpolated onto the 0.5 m cells, is the fallback if 0.5 m is too
slow. That is measured before it is decided.

Do not use Halls' symmetry. Pickups are authored per quarter and mappers
break symmetry on purpose. The general bake is the only one.

Bots run on the authority, so the graph is built there. Clients build their
own copy only to draw the overlay. Nothing about it crosses the wire: every
machine has the map.

### The overlays

The overlays are the **mapper's tool first**. A wall placed while an overlay
is up should visibly change the floor around it within the debounce. That
loop is the feature.

Drawn as a **mesh** over the floor cells (a quad per cell, vertex-colored,
lifted a few millimeters, on `multicam::editor_only()`), not as gizmos:
thousands of cells redrawn as lines every frame is the wrong tool, and a mesh
can be swapped when a bake lands. The play camera gets a copy on a debug key
for tuning bots. The editor is where it lives.

One overlay at a time, picked from a **Nav** panel (an `egui_dock` tab):

| Overlay | Shows |
| --- | --- |
| Off | |
| Cells and links | Walkable cells, links colored by kind, one-way drops as arrows |
| Sight | Single hue, dark to bright |
| Exposure | Single hue, dark to bright |
| **Sight × exposure** | Bivariate: green = sees a lot and is seen little, red = sees little and is seen a lot, yellow = sees and is seen (contested), dark = sees and is seen little (hidden) |
| Advantage | Diverging: green positive, red negative |
| Cover nearby | Peek spots bright |
| Danger | Knockback and hurt, red |
| Distance to … | One pickup's field, picked from a list |

The panel also holds the Bake button, the stage the bake is at, a crouched
toggle for the visibility overlays, and the saturation for each scale.
Saturation is a fixed number in m², not a percentile, for the reason given
under "Fields".

Two tools beside the overlays:

- **Probe**: hover a cell to light up every cell that can see it (red) and
  every cell it can see (green), from what the bake stored. This answers
  "who can see me here", which is the question a mapper is actually asking
  when they look at an exposure number.
- **Baseline**: pin the current bake, then show the *difference* from it.
  "This wall cut exposure in the courtyard by 40%" as a color, rather than
  something to spot by flicking between two screenshots.

### What the bot will read

So the bot does not reach into the graph's layout, it gets a narrow
interface:

- `NavGraph::cell_at(feet) -> Option<CellId>`
- `neighbors(cell) -> links`, each with kind, time and input sequence
- `field(cell, Field) -> f32`, where `Field` is the enum the overlays are
  drawn from, so a bot and a mapper see the same numbers
- `path(from, to, cost)` with a caller-supplied cost per cell, so skill is
  weights (distance × (1 + exposure × w) + danger …), not a different
  pathfinder
- `distance_to(pickup, cell)` from the baked fields

Runtime costs that change during a round (where enemies were last seen or
heard, a breakable that broke, a prop that moved) are the bot's to add on
top. They are not baked.

### Wasm

Pure Rust over `CollisionWorld`, no threads assumed. The async compute pool
runs a task in one go on wasm, so the bake must be chunkable across frames,
as the sun bake must. Write the visibility stage as an iterator over cell
ranges from the start, so chunking is a scheduler change and not a rewrite.

## Open questions

- **Exposure weighting by range.** Being seen from 35 m by a rocket launcher
  is not being seen from 8 m by a shotgun. Start unweighted. Add range bands
  if the overlay reads wrong on Halls.
- **Cell spacing**: 0.5 m everywhere, or finer near edges? Decide on Halls'
  numbers.
- **Bunny-hop links.** Probably never baked: they depend on speed carried in,
  which a cell does not have. A high-skill bot can find them at runtime.
- **Breakables.** Bake as standing and let the bot cost them at runtime, or
  bake both states? Standing for now.

## Checklist

### Stage 1: cells and walking
- [ ] Pull `step_player`'s body out into a pure `step`, with the system
      calling it; a test that the system and the function agree
- [ ] Floor faces → lattice cells → `fits` → cells (`common/nav.rs`), Kill
      volumes excluded, Hurt rates kept
- [ ] Walk and step links; reachability from spawns prunes the rest
- [x] A shared runner and progress report for background bakes
      (`BackgroundBake`, `BakeProgress`), with the sun moved onto it
- [ ] `NavBake` as a `BakeJob` in `BakeSystems::All`, reporting as
      `BakeKind::Nav`; the old graph kept until replaced
- [ ] Nav panel with Bake and Cells and links overlay (floor mesh,
      `editor_only`)
- [ ] Tests: a lone room is one connected set; a 0.4 m lip is a step and a
      1 m lip is not; a Kill volume has no cells; a cell under a low ceiling
      is not one; floors across a threshold link

### Stage 2: jumps and drops
- [ ] Drop links by simulating off edges with `step`
- [ ] Jump links by simulating from edge cells; each link stores its input
      sequence
- [ ] Tests: a 1 m ledge is jumpable up and droppable down; a gap wider than
      a jump has no link; a drop into a Kill volume has no link

### Stage 3: fields and the mapper's overlays
- [ ] Knockback danger, Hurt danger
- [ ] Pickup distance fields (travel time)
- [ ] Visibility stage: sight, exposure, advantage, crouched variants;
      measured on Halls at 0.5 m and 1 m before choosing
- [ ] Cover nearby
- [ ] Overlays: Sight, Exposure, Sight × exposure (bivariate), Advantage,
      Cover, Danger, Distance to…; fixed saturation per scale
- [ ] Probe: who sees this cell, what it sees
- [ ] Baseline: pin a bake, show the difference
- [ ] Debug key to show the current overlay in Play
- [ ] Tests: a pillar between two cells hides them from each other; a ledge
      gives the high cell more sight than exposure against the low one; a
      cell beside a pit edge has knockback danger and one in the middle of a
      room has none
- [ ] Chunked across frames on wasm

### Stage 4: the bot's interface
- [ ] `NavGraph` query API (`cell_at`, `neighbors`, `field`, `path` with a
      caller cost, `distance_to`)
- [ ] A bot body: an entity holding an `Identity` that
      `give_bodies_to_whoever_needs_one` serves like a client's link
- [ ] Follow a path by writing `Inputs`, replaying a link's input sequence
      across jumps

### Later
- [ ] Rocket-jump links
- [ ] The brain: goals, utility scoring, hearing via `Effect`, aim and skill
      (its own plan)
