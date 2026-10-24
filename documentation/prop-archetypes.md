# Prop archetypes

A prop used to be one thing: something a body holds. This is the plan for the
other things a prop can be, and the checklist of what has landed.

## The decision

A prop names an **archetype**, which decides what the *game* builds from it.
The archetype is a preset over components, not a class hierarchy: what a
breakable crate is, in the ECS, is a placed model with a `Damageable`,
`Hitboxes` and a collider; a health pack is a placed model with a `Pickup`.

| Archetype | Where it goes | What it carries in a match |
| --- | --- | --- |
| `held` | a body's hands | — (the default; every prop before this was one) |
| `hat` | a body's head | — (cosmetic) |
| `obstacle` | placed in a map | colliders that block movement |
| `breakable` | placed in a map | colliders, `Damageable`, `Hitboxes` |
| `pickup` | placed in a map | a `Pickup` and a trigger volume |

The data each archetype reads lives in its own section of the `.gpp` —
`colliders`, `[breakable]`, `[pickup]`, `[hat]` — and every section is kept
whatever the archetype, so flipping archetype in the editor and back loses
nothing.

### Colliders

A collider is a **rounded box**: a core box plus a radius. A box has radius
zero, a sphere a zero core, an upright capsule a core that is only tall. That is
one shape for the collision world to answer for rather than three, and the
per-axis sweep `move_and_slide` already does turns out to need only one extra
number per shape (how far the rounding reaches given where the body is on the
other two axes).

A placed prop's origin is where it stands on the floor, so a mapper puts the
feature's point on the floor.

Colliders are axis-aligned in the prop's frame. A placed prop that is turned
collides as the world-space bounding box of its turned core, keeping its
radius — exact for spheres and quarter turns, a little generous otherwise.

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

Health and ammo are real; armour and the flag are kinds with no behaviour yet,
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

- [x] `archetype`, `colliders`, `[breakable]`, `[pickup]`, `[hat]` in the `.gpp`
- [x] Rounded-box colliders in `CollisionWorld` (sweep, `fits`, overlap)
- [x] `Hitboxes::head` optional, so a crate has no head
- [x] Map `Prop` feature names a model; authority stands `WorldProp`s up
- [x] Obstacle and breakable colliders block movement on every machine
- [x] Breakables take damage, break, stay broken for the round
- [x] Health and ammo pickups, respawning on a timer
- [x] Pickup style: Quake (sits) or TF2 (hovers and turns)
- [x] Hats on heads, from `loadouts.toml`, hidden in first person
- [x] Prop editor: archetype, colliders (drawn), breakable, pickup, hat panels
- [x] Example props: a hat, a crate, a health pack, an ammo pack
- [ ] Colliders stopping shots and projectiles for obstacles
- [ ] Viewport handles for colliders and the hat fit
- [ ] Armour: a pool in the damage layer
- [ ] Flag: carried, dropped on death, returned
- [ ] Picking a map prop's model from a list rather than typing its name
