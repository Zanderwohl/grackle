# Collision and the world surface

The plan for replacing the box-only collision in `src/game/collision/`, and
the box-only room bake beside it, with something that can hold an angled wall,
a footprint room and a Source-style displacement. Tick items off as they land;
the checklist is the source of truth for what is done.

## Where it starts from (October 2026)

Every layer between a room and a collision is an axis-aligned box:

- **`Room` is `{ min, max }`** ([`src/tool/room.rs`](../src/tool/room.rs)).
  Openings are carved per face with `rect_subtract`, and `bake_faces`,
  `collision_slabs` and `ray_hit_from_inside` all walk the same rectangles.
- **A wall is a `WallSlab`**, an AABB half a metre deep behind each solid
  rectangle. `CollisionWorld` is a flat `Vec` of them, plus the placed props'
  `RoundedBox`es, searched linearly on every query.
- **`move_and_slide` is swept one axis at a time**, which is exact for boxes
  against boxes and meaningless for anything else. A `RoundedBox` is reduced to
  a per-axis slab (`slab_along`) so it can go through the same loop.
- **Contact is a `BVec3`.** `Player.blocked` (replicated, read by the animator
  for `wall_ahead`), `on_ground = blocked.y`, the ragdoll's per-point
  `touching`, and its friction — which zeroes velocity per axis — all assume a
  contact normal is a coordinate axis.
- **`inside_map` is point-in-any-room-box**, used as the tie-breaker in
  `depenetrate`, by `fits`, and by the third-person camera test.

Callers, which is the whole surface to migrate:

| Query | Callers |
| --- | --- |
| `move_and_slide` | `step_player`, `third_person_camera`, ragdoll points |
| `depenetrate` | `step_player`, ragdoll `clear_walls` |
| `fits` | `usable_spawns`, `change_stance`, the template test in `save.rs` |
| `inside_map` | `fits`, `depenetrate`, third-person camera test |
| `ray_distance` | hitscan, laser, flame, explosion sight, projectile range |
| `ray_hit` (with normal) | projectile bounces, held-weapon ground probe |
| `rebuild(&[Room])` | `enter_play`, `rebuild_collision_when_rooms_change`, net bodies, many tests |
| `props` | `world_prop` (placed colliders) |

Boxes stay right for plenty: hitboxes, the movement hull, broadphase bounds,
the editor's resize handles, auto-bake's "which rooms touch". They stop being
the shape of the world.

## Decisions

- **One surface, two consumers.** The map bakes to a set of convex polygons —
  the *world surface* — and both the render mesh and the collision shapes are
  built from it. That is the rule `for_each_solid_rect` already keeps ("an
  opening you can see through is an opening you can walk through"), lifted off
  rectangles. A source of geometry (a box room, a footprint room, a
  displacement, a brush later) contributes polygons and never talks to
  collision directly.
- **The body stays an axis-aligned box.** Quake and Source both do this: the
  hull never turns, so a sweep is a ray against the world grown by the hull.
  Hitboxes are a separate volume already and are untouched.
- **Every sweep is a ray against a shape grown by the body.** For a convex
  hull that is the hull's planes pushed out by the box's support, plus bevel
  planes on edges and corners (without bevels a box catches on the air past a
  sharp corner). For a `RoundedBox` it is a rounded box with a bigger core,
  which is analytic. One idea, so `fits`, `sweep`, `ray` and `depenetrate`
  cannot disagree about where a wall is.
- **Two shape kinds: convex hulls and triangles.** A wall, a floor piece, a
  prop collider and a brush are convex hulls with volume. A displacement is
  triangles, each treated as a zero-thickness hull, two-sided. Volume matters
  for depenetration — a hull knows which way is out, a triangle only knows its
  two sides — which is why solids stay hulls rather than being triangulated.
- **Hand-written, not `parry3d`.** Parry is pure Rust and would build for
  wasm, but a client and server must reach the same position from the same
  inputs, and parry's determinism is a feature flag over SIMD paths we do not
  control. The shape set is two kinds, the queries are four, and the repo
  already hand-writes its CSG kernel and its ragdoll for the same reasons. No
  SIMD, no `mul_add`, no threads: the same scalar code on both ends.
- **Contacts are normals, not axes.** A query returns the plane it hit.
  `on_ground` is a hit whose normal is within `MAX_WALKABLE_SLOPE` of up;
  `wall_ahead` is a hit whose normal opposes the movement; ragdoll friction
  removes the velocity along the normal and damps the rest. `BVec3` leaves the
  contact path entirely.
- **Movement is a slide move over planes**, Quake's `PM_SlideMove`: sweep,
  clip velocity against the hit plane, repeat with every plane touched so far,
  and stop in the crease where two planes meet. Bounded at four bumps, as a
  projectile's bounces already are. Step-up for stairs is a separate pass
  after it, not a change to it.
- **A broadphase is required, not an optimisation.** Today every query is
  linear in walls, and the ragdoll solver runs `depenetrate` per point per
  iteration per corpse. A uniform grid over static shape bounds, rebuilt with
  the map; props in their own small set because they change on a different
  trigger, as now.
- **The map is the union of its room volumes.** A room is a volume of air; the
  surface is the boundary of the union, faces pointing in. Two rooms sharing a
  wall lose it, a room crossing another's wall opens a hole in it — what the
  rectangles do today. **One behaviour changes:** a room wholly inside another
  currently keeps all six faces (`bake_engulfed_room_keeps_all_faces`), and in
  a union it disappears. Nobody builds a pillar that way on purpose; a solid
  inside a room is a hole in a footprint, or a subtractive brush later.
- **Per-room baking survives.** A room's share of the surface is its own
  boundary clipped to outside every other room it touches, so auto-bake's
  "rebuild what a change touched" keeps working, and one room's mesh is one
  entity, as now.
- **The CSG kernel moves to `src/common/`** with `Surface` generalised, since
  the map now depends on it. Its `EPSILON` is an absolute `1e-5` m, which is
  at the edge of `f32` precision a hundred metres from the origin. Measured on
  the largest map before trusting it: room-local coordinates or `f64`
  internally if it is not enough. The editor's 0.1 m snap helps and must not
  be relied on.
- **Containment stays a question about volumes**, not about the surface:
  `inside_map` is "inside some room volume", now a prism rather than a box. It
  is only ever a preference (which way out of a wall is into the level), so a
  displaced floor above a room's own floor need not change the answer.
- **The `Room` component stops being what collision reads.** `CollisionWorld`
  is rebuilt from the baked world surface, which removes the reason the dead
  `Room` type could not be deleted. The editor keeps whatever it needs for
  handles.

## The shapes and queries

```
Hull { planes: Vec<(normal, reach)>, bounds }  // convex; Hull::triangle is flat
RoundedBox                                     // props, kept in their own list

Hit { t: f32 /* 0..1 of the move */, normal: Vec3, plane: f32 /* normal · centre */ }

sweep(centre, half, delta)  -> Option<Hit>     // first contact of a moving box
ray_hit(ray, range)         -> Option<(f32, Vec3)>
overlaps(centre, half)      -> bool            // strict: touching is not overlapping
escapes(centre, half, skin) -> positions       // one per grown plane
inside_map(point)           -> bool            // room volumes, not shapes
```

A triangle is not its own shape: it is a hull whose two faces point opposite
ways, so one grown-plane routine answers both. Props stay `RoundedBox`es in
their own list, swept as a rounded box grown by the body.

**Bits against box walls are pinned.** A corpse is a replicated seed that
every machine simulates, and
`the_solver_reaches_the_same_bits_it_always_has` holds the arithmetic — with a
release-profile number that cannot be re-recorded without a release build. So a
contact or an escape against an axis-aligned plane is written as the plane's
own number (`onto_plane`), not as a step from where the box started; any other
way round is a different rounding and a different corpse. Angled planes have no
history to match and use the step.

`slide(centre, half, delta) -> (end, contacts)` is built on `sweep` and is what
`step_player`, the camera and the ragdoll call; `fits` is `inside_map` and no
`overlap`.

## Checklist

### Stage 1 — shapes and queries, same world

- [x] `src/game/collision/`: `Hull` (`hull.rs`) with grown planes and bevels;
      `sweep`, `ray`, `overlaps`, `escapes`; `Hull::triangle`. Rounded-box
      sweep in `rounded.rs`. Unit tested shape by shape.
- [x] Uniform-grid broadphase over static bounds (`grid.rs`), answering in
      index order so "first overlapped" is unchanged.
- [x] `CollisionWorld` holds hulls; `rebuild` turns today's `WallSlab`s into
      box hulls, so the geometry is identical and only the code is new.
      `set_hulls` takes any hulls, which is what Stage 3 will feed.
- [x] Every existing collision, player, ragdoll and projectile test passes
      unchanged, including the ragdoll's pinned bits.
- [x] Angled hull and triangle tests for each query, in `hull.rs` and as a
      ramp in the world (landing, a bounce normal, depenetration).

### Stage 2 — contacts as normals

- [ ] `slide` over planes replaces `move_and_slide`; returns contact normals.
- [ ] `Player.blocked: BVec3` replaced by what the animator and step need
      (`ground_normal: Option<Vec3>`, `wall_ahead`); `on_ground` by slope.
- [ ] Ragdoll contacts and friction by normal.
- [ ] `depenetrate` on `overlap`, keeping the into-the-map preference.
- [ ] Third-person camera on `sweep`.
- [ ] A body standing on a slope it can walk does not slide; one too steep
      slides off. A body in a V-shaped crease stops rather than jitters.

### Stage 3 — the world surface

- [ ] CSG kernel to `src/common/`; tolerance measured at map scale.
- [ ] A room's share of the surface: its boundary clipped against every room
      it touches. The `room.rs` bake tests ported as area tests (a shared full
      wall leaves 5 faces' area, a doorway leaves the frame's).
- [ ] Render mesh and collision shapes both from it; `rect_subtract`,
      `RoomFace`, `WallSlab` and `collision_slabs` deleted.
- [ ] Editor picking (`ray_hit_from_inside`) against the same surface.
- [ ] `CollisionWorld` rebuilt from the surface, not `Query<&Room>`; removed
      rooms detected (today's `rebuild_collision_when_rooms_change` misses
      them).
- [ ] Engulfed-room change noted in CLAUDE.md.

### Stage 4 — shapes that are not boxes

- [ ] Room volumes as prisms (a polygon and a height range); `inside_map` and
      `fallback_spawn`'s "largest room" over them.
- [ ] Footprint room: a sketch (`src/prop/sketch.rs`, `region.rs` moved to
      `src/common/`) on a floor plane, extruded between two heights; holes
      are pillars. Its own plan once Stage 3 lands.
- [ ] Step-up for stairs.

### Stage 5 — displacements

- [ ] Its own plan. What this plan owes it: triangles are a first-class
      shape in collision and a first-class contribution to the surface, and
      nothing between the two assumes a face is flat.

### Noticed along the way

- Rays ignore props, as they always have: a crate does not stop a bullet or a
  blast. Kept for Stage 1; whether it should is a weapons question.
- `Grid::near` allocates a `Vec` per query. Cheap next to the linear scan it
  replaced; worth a scratch buffer if ragdolls show up in a profile.
