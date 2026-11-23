# Prop archetypes

A prop used to be one thing: something a body holds. This is the plan for the
other things a prop can be, and the checklist of what has landed.

## The decision

A prop names an **archetype**, which decides what the *game* builds from it.
The archetype is a preset over components, not a class hierarchy: what a
breakable crate is, in the ECS, is a placed model with a `Damageable`,
`Hitboxes` and colliders; a health pack is a placed model with a `Pickup`.

| Archetype | Where it goes | What it carries in a match |
| --- | --- | --- |
| `held` | a body's hands | — (the default; every prop before this was one) |
| `hat` | a body's head | — (cosmetic) |
| `obstacle` | placed in a map | colliders |
| `breakable` | placed in a map | colliders, `Damageable`, `Hitboxes` |
| `pickup` | placed in a map | a `Pickup`, taken through its trigger colliders |

The data each archetype reads lives in its own section of the `.gpp` —
`[breakable]`, `[pickup]`, `[hat]` — and every section is kept whatever the
archetype, so flipping archetype in the editor and back loses nothing.
Colliders are features, so they are kept too.

### Colliders

A collider is a **feature** in the prop's list (`FeatureOp::Collider`,
[`src/prop/collider.rs`](../src/prop/collider.rs)) that makes no body: a box, a
sphere or a capsule, each with a `Placement` (origin and turn, edited as
"points along" and "spin" as a primitive's is) and any number of **roles**:

| Role | What touching it does | Read by |
| --- | --- | --- |
| `trigger` | sets off the prop's own effect: a pickup is taken through it | `take_pickups` |
| `shots` | stops bullets, projectiles, blasts and sight, and is what a breakable is shot through | `CollisionWorld::shots`, `box_breakables` |
| `solid` | bodies walk into it and stand on it | `CollisionWorld::props` |
| `ignites` | sets a body alight with the flamethrower's afterburn, credited to the map | `act_on_bodies_touching_props` |
| `kills` | kills a body outright, as a Kill trigger does | `act_on_bodies_touching_props` |

A new collider plays `trigger` on a pickup and `shots` and `solid` on anything
else. They are made from the toolbar's collider group (Box, Sphere, Capsule,
the last one used on the button), resized and moved by the same handles as a
primitive, and drawn in the color of their most drastic role. A collider
suppressed or past the rollback bar is not there, as any feature is not.

A capsule lies along its placement's `Y`; its `height` is end to end, caps
included, and is never less than the two caps.

**In the world a collider is exact however it is turned**
([`src/game/collision/shape.rs`](../src/game/collision/shape.rs)): a box is a
`Hull`, so the body's box sweeps against it as against a turned wall, and a
capsule (a sphere is one whose ends meet) is swept as its segment grown by the
body's box and then by its radius. Each instance's colliders are placed once
per move or bake, by role, in `PlacedColliders`, which everything above reads.
A taken pickup has none.

A ray that starts inside or on a shot collider is not stopped by it, so a shot
from inside a bush, or a blast on a crate's face, goes on out.

A breakable is still *damaged* through the box round its shot colliders
(`Hitboxes`), while the shapes themselves stop the shot. A shot through the
corner of that box that misses a turned or round collider counts as a hit.

A placed prop's origin is where it stands on the floor, so a mapper puts the
feature's point on the floor.

Files from before colliders were features have a `colliders` section; it is
read as collider features at the end of the list (`read_collider_section`).

### Who builds what

Same rule as carousel bodies: **features describe the map; the authority builds
the bodies.** A map `Prop` feature names a model. On the authority,
`sync_world_props` stands one replicated `WorldProp` entity up per feature
that names a model, and keeps it where the feature is. Every machine draws
those entities and builds collision from them, so a client predicting its
movement walks into the same crate the server does.

A breakable that is broken stays broken for the round: the authority
remembers on the feature that its instance went away, and only entering or
leaving Play forgets.

### Pickups

Health and ammo are real; armor and the flag are kinds with no behavior yet,
the way the stub weapon actions are. A taken pickup is hidden and comes back
after its own number of seconds; that countdown is replicated so clients stop
drawing it too. Taking one is authority-only.

A pickup is not taken by somebody who would get nothing from it — full health
walks past a health pack.

A pickup has a `style`: `quake` sits on the floor, still; `tf2` is drawn
15 cm up, turning at 90°/s. Only the drawing moves, so the placement and the
trigger are the same either way and flipping it needs nothing re-placed.

### Hats

A hat sits on the head bone, at the **crown** (the head bone's tail), facing
the way the body does: `-Z` forward and `+Y` up, whatever the head bone's own
rest rotation is. A `[hat]` section offsets and turns it from there, with
per-class overrides in the same shape as a hold's.

Which hat a class wears is a `hat` key on its entry in `loadouts.toml`. Items,
ownership and choosing one come later; this is the anchor they will name.

In the prop editor's Fit workspace a hat is fitted the way a weapon is: the
hat stays at the origin and the figure is placed under it.

## Checklist

- [x] `archetype`, `[breakable]`, `[pickup]`, `[hat]` in the `.gpp`
- [x] Colliders as features: turned boxes, spheres and capsules with roles
- [x] Exact turned colliders in `CollisionWorld` (sweep, `fits`, overlap, rays)
- [x] `Hitboxes::head` optional, so a crate has no head
- [x] Map `Prop` feature names a model; authority stands `WorldProp`s up
- [x] Solid colliders block movement on every machine
- [x] Shot colliders stop shots, projectiles, blasts and sight
- [x] Igniting and killing colliders
- [x] Breakables take damage, break, stay broken for the round
- [x] Health and ammo pickups, respawning on a timer
- [x] Pickup style: Quake (sits) or TF2 (hovers and turns)
- [x] Hats on heads, from `loadouts.toml`, hidden in first person
- [x] Prop editor: archetype, breakable, pickup, hat panels; a collider tool group
- [x] Example props: a hat, a crate, a health pack, an ammo pack
- [x] Viewport handles for colliders
- [ ] Viewport handles for the hat fit
- [ ] Picking a collider in the viewport (today: from the feature list)
- [ ] A weapon pickup taken through its model's trigger colliders rather than a fixed sphere
- [ ] A breakable damaged through its exact shot colliders rather than their bounding box
- [ ] Armor: a pool in the damage layer
- [ ] Flag: carried, dropped on death, returned
- [ ] Picking a map prop's model from a list rather than typing its name
