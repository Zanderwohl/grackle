# Grackle

A Bevy engine and editor for quake-style shooters, aimed at the browser.

## What this is for

The target game is a variation on TF2 crossed with Ultimate Chicken Horse: teams
modify the map between rounds. **The editor and the game are one binary**, and
swapping between them has to be seamless and fast — that is a gameplay feature,
not a convenience. Design accordingly:

- Editor state and game state live in the same process and the same `World`.
  Prefer a Bevy `State` transition over a process boundary, a reload, or a
  round-trip through a file.
- Anything that only works because the editor is native (a file dialog, a
  blocking call, a background thread, a C library) becomes a problem the moment
  editing happens mid-match in a browser tab. See "Targeting wasm" below.
- Two map formats are intended, and the split is deliberate:
  - **Blueprint (`.gmb`)** — the authoring format, a SQLite database. Holds the
    full feature timeline with undo history. This is what the editor saves.
  - **Compiled map** — not written yet. A simpler, flatter runtime format that a
    more traditional art-filled map can be baked into and shipped. The game
    loads this; it does not load blueprints in the general case.

  Between-round editing works on live features. Compilation is for the other
  direction: authored content that ships.

The intended service topology — accounts, items, matchmaking, play servers,
client — is sketched in [documentation/sketch.md](documentation/sketch.md).
None of it is built. Read it before adding anything networked. The one thing
in it that constrains code written today: items must record their provenance
(granting server, timestamp, match) from the first grant onward.

## State of the repo

The editor is real and works. **The game barely exists.** There is a damage
layer and three kinds of weapon — hitscan, projectile, flame — but no
networking, no ammo,
no reload, no teams and no respawn: a body that dies leaves a ragdoll and is
gone. There is also no wasm build yet. Adding the runtime is the current
frontier, not a finished thing to extend.

`src/unlock` and the `crate_drop` binary are a self-contained TF2-style
crate-unboxing prototype. It is orthogonal to both the editor and the game.
Don't wire new work into it.

There is a player body that walks, falls and jumps in `src/game/`, but it is
still a prototype for feeling out room sizes: it has health and can shoot, and
that is the whole of it.

`cargo check --all-targets` and `cargo test` both pass. The `Room` type is
still dead — superseded by `EditorRoom` for authoring — but it is what
`CollisionWorld` is rebuilt from, so deleting it is not the small change it
looks like.

## Layout

| Path | What lives there |
| --- | --- |
| `src/main.rs` | The `editor` binary — plugin wiring only. |
| `src/editor/` | The document model: features, timeline, save/load, panels, cameras. |
| `src/tool/` | One module per editor tool, each its own `Plugin` with its own `Tools` state. |
| `src/game/` | Playing the open map: `AppMode` swap, player body, collision, weapons, damage, ragdolls. |
| `src/common/` | Shared: i18n, geometry, rays, gamemodes, items. |
| `src/unlock/` | The crate-drop prototype. Orthogonal — see above. |
| `src/bin/` | `ensure_lang` (fills missing translation keys), `new_map_template` (writes the blueprint a new map starts from), `crate_drop`. |
| `assets/default/` | The default pack: `lang/`, `blueprints/`. |

## Spawn points and class metrics

A `SpawnPoint` ([`src/editor/spawn_point.rs`](src/editor/spawn_point.rs)) marks
where **the feet** go — that is the part a mapper lines up against a floor.
Everything above it comes from
[`src/common/class.rs`](src/common/class.rs), which is deliberately the single
home for body metrics: the body the game moves, the clearance a spawn is
checked for, and the gizmo the editor draws all read the same constants. Let
those drift and a spawn point passes in the editor while wedging a head in the
ceiling at runtime.

Usability is decided by `CollisionWorld::fits` — the body box must be in the
map and touching no wall. There is no separate notion of "headroom": a low
ceiling *is* a slab the body overlaps. Unusable spawns are skipped with a
warning, and a map with none falls back to the largest room rather than
refusing to start.

The new-map template (`assets/default/blueprints/new.gmb`) ships one, anchored
to the room's `bottom_plane_center` rather than placed at absolute coordinates,
so resizing the room takes the spawn with it. Most of what is in the template
is taste and will change; what a new map has to *provide* is a room, a light,
and a spawn point with headroom, which is what the test at the bottom of
[`save.rs`](src/editor/save.rs) checks. Don't pin its coordinates there.

The template is a database, so a diff shows it as "changed" and nothing more.
Don't edit it by saving over it from the editor — it is written by
`cargo run --bin new_map_template`
([`src/bin/new_map_template.rs`](src/bin/new_map_template.rs)), run from the
repo root, and that file is where its contents are reviewable. It currently
also ships an animation grid, which is why the room is 24 m square: the grid is
a testing convenience rather than something a new map owes you.

Choice is uniform random, and facing is the fixed `SPAWN_YAW`. Per-team spawns
and not dropping people on each other are gamemode questions, deliberately not
answered at this layer yet.

## Editor and game in one process

`AppMode` ([`src/common/app_mode.rs`](src/common/app_mode.rs)) is `Editor` or
`Play`, and **`F5` swaps between them** — the only way back to the editor, as
`Escape` belongs to the pause menu. No reload, no second process — that is the
between-round editing feature, so keep it that way.

**On a client the server owns the mode**, and F5 does nothing.
[`src/common/match_state.rs`](src/common/match_state.rs) sends a `MatchMode`
whenever the server's `AppMode` changes and again to each client as it
connects, and the client follows. Everyone drops into the editor between rounds
together and drops back into the round together, which only works if one
process owns the transition. Two details there are load-bearing: the message
states what the mode *is* rather than that it changed, so a client joining
mid-round is told where things stand and a dropped message is corrected by the
next one instead of leaving somebody inverted; and a restatement of the mode
already in force is ignored, because `NextState::set` runs the transition even
for the same state and `OnEnter(Play)` tears the match down and rebuilds it.
`toggle_mode` answers a client's F5 with a line in the log rather than
swallowing it — a key that silently does nothing reads as a bug.

**`Escape` opens the pause menu**
([`src/game/pause_menu.rs`](src/game/pause_menu.rs)), and releasing the mouse
is the point of it — a grabbed cursor is not something a player can get out of
by clicking elsewhere, so a window with no way to hand the pointer back is a
window you have to kill the process to leave. Plain Bevy UI, not `egui`: the
panels are egui because they are editor tools, and this is part of the game, so
it has to exist in a build with no editor and on a browser tab where the egui
layer is one more thing to have gone wrong.

Three things there are easy to get wrong and none of them errors:

- **`gather_input` is gated on `not_paused`, but `mouse_look` is not.** A
  `MessageReader` has its own cursor, so a frame the system does not run is a
  frame of mouse motion still waiting to be read — gate it and closing the menu
  applies every scrap of motion made while it was up, in one frame. It drains
  first and answers the pause question itself.
- **Opening the menu clears `InputLatch`.** A key held as it goes up would stay
  held, and `gather_input` is not running to correct it, so the body walks into
  a wall for as long as the menu is open.
- **The cursor is only touched while in Play.** Change detection counts a
  resource's insertion as a change, so a `resource_changed` system that sets
  the cursor unconditionally grabs it inside the editor on the first frame of
  the process.

The play camera carries `IsDefaultUiCamera`: the window has five cameras in it
and Bevy UI otherwise picks one by ambiguity rules rather than by being told.

Three more keys while playing: **`F`** swaps between first and third person
(`ViewMode` in [`src/game/player.rs`](src/game/player.rs) — your own body and
its hitboxes are hidden from inside your own head, nobody else's are),
**`F4`** toggles the hitbox gizmos, and **`F6`** toggles the bone gizmos, which
are off by default now that bodies have geometry. `F3` is the perf overlay in
both modes.

Weapons are on **`1`–`5`** (hitscan, rocket, pipe bomb, RPG, flamethrower),
the left mouse button fires — held, for the flamethrower — and **`G`** plants
an emitter firing whatever projectile you are holding every two seconds with
**`B`** to clear them. See "Damage, weapons and projectiles" below.

## Baking before a round

**Entering Play bakes the map first.** `BakeSystems::All`
([`src/tool/bakes.rs`](src/tool/bakes.rs)) runs on `OnEnter(AppMode::Play)`
and `reset_for_play`/`enter_play` are ordered `.after` it, so a round is set up
against a world that has been rebuilt rather than one that is about to be.

It is a **set, not a list of named systems**, for the same reason
`DamageSystems` is one: rooms are the only thing baked today, and a second kind
— navmesh, lightmap, whatever the compiled map format wants — joins by naming
the set. Name the systems instead and the ordering lives somewhere that does
not know about them, and forgetting one produces no error, just a round that
starts before its world is finished.

The bake is unconditional rather than message-driven, because the case it
exists for is a client: it is handed the server's map and enters Play in the
same breath, and the `CalculateRoomGeometry` that adoption writes is read a
frame later than the round starts. What a missed bake costs is *not* a body
falling through the world — collision is built from the `Room` components, so
the walls are all still there — it is a round played in a map nobody can see.
That is why it is easy to miss.

`bake_room_geometry` is also **no longer gated on `AppMode::Editor`**, unlike
the rest of `BakePlugin`. A map can change while a round is being played; that
is the whole between-round editing feature, and geometry that stopped being
rebuilt the moment somebody pressed F5 would leave the match looking at the map
as it was before the edit. Joining a round therefore bakes twice — once on
adoption, once entering Play — which is the right trade: the message covers a
map arriving mid-round, the set covers one arriving as the round starts.

The actual work is the free function `bake_rooms`, called by both paths, so
that "somebody pressed the button" and "a round is starting" cannot bake
slightly differently.

## What a body is drawn as

Three volumes, and they are kept apart on purpose — the movement hull, the
hitboxes and the bones are covered in
[`src/common/hitbox.rs`](src/common/hitbox.rs). The mesh is a fourth, and it is
the only one that is authoritative about nothing: it is built from the bones
and nothing tests against it.

One child entity per bone, carrying that bone's mesh, placed by that bone
([`src/game/body_mesh.rs`](src/game/body_mesh.rs)). Rigid parenting — a
shoulder does not stretch, and two parts meeting at a joint interpenetrate.
That is a first cut, and the shape of it is what a skinned version wants:
vertices are authored in bone space in
[`src/common/skeleton/mesh.rs`](src/common/skeleton/mesh.rs), so warping them
across a joint later replaces the placing system and leaves the geometry alone.

The geometry itself is quads built by hand
([`src/common/mesh.rs`](src/common/mesh.rs)) rather than Bevy primitives,
because two `Cuboid`s meeting at a knee are two closed surfaces with no shared
edge to sew. Everything is built from **rings** — a closed loop of points
across a shape — so a limb is a stack of cross-sections, and welding two parts
later means sharing a ring rather than rewriting a shape. Winding is
counter-clockwise seen from outside, everywhere; get it backwards and a part is
not missing, it is inside out.

A bone's shape is a `Profile`: a handful of rings down its length, each a
multiplier on the bone's own thickness. So nothing here is a second opinion
about how big a body is — `girth` and `belly` already differ per class, and a
new class is nine numbers rather than a model somebody authored. Meshes are
cached per build, not per body, which is what makes the sixty-body animation
grid ten meshes; materials are cached per colour, `BodyTint` overriding the
default on a body that wants its own.

**Hitboxes are asked for, not handed out.** Every rig gets a `Gait`, because
every body has a stride, but `Hitboxes` come from `#[require(Hitboxes)]` on the
markers that mean *this is a real body*: `Player`, `CarouselBody` and
`AnimationDisplayMarker`. A `SpawnPoint` carries none of them — its body is
a drawing of the space a body needs, so it is green, has no animator, has no
hit volumes, and is hidden while playing rather than left for a spawning player
to materialise inside.

One trap when a feature stands up a body: `apply_to_entity` runs on **every**
edit, and re-inserting a `Skeleton` reads as a changed one, so a plain `insert`
throws the mesh away and rebuilds it on every frame of a drag. Insert the rig
with `insert_if_new` and let the edit change only what it actually changes.

Every editor system is gated with `.run_if(in_state(AppMode::Editor))` at its
`add_systems` call, and `EditorInputPlugin` is gated too so the resources every
tool reads go quiet in Play. **Two systems are deliberately not gated**:
`FeatureTimeline::sync_entities` and `handle_edits` keep running while playing,
which is what lets a map edit land mid-match. If you add an editor system,
gate it; if you add something the game needs to see, do not.

**Physics runs on `FixedUpdate` at 64 Hz**, and the split across three
schedules is deliberate:

| Schedule | What runs there | Why |
| --- | --- | --- |
| `BeforeFixedMainLoop` | `gather_input`, `mouse_look` | Aim at 64 Hz is latency you can feel; input gathered here reaches the same frame's steps. |
| `FixedUpdate` | `step_player`, collision rebuild | Everything deciding where a body ends up, so the same inputs give the same trajectory at any frame rate. |
| `AfterFixedMainLoop` | `interpolate_bodies` | Draws between steps, so 64 Hz does not visibly step on a 144 Hz display. |

Two rules that follow, and both are easy to break by accident:

- **Never read `ButtonInput` from `FixedUpdate`.** It is cleared once per
  frame while `FixedUpdate` runs zero, one or several times, so edges get
  double-counted on a slow frame and dropped on a fast one. Latch into
  `InputLatch` in `gather_input` and let the tick take it.
- **Physics writes `PhysicsBody`, never `Transform`.** `interpolate_bodies`
  owns `Transform.translation`; writing it from a step would be overwritten and
  would skip interpolation. `mouse_look` owns `Transform.rotation`.

`PlayerInput` is also the seam prediction needs: the step is a function of
`(state, input, fixed dt)`, so a client and a server fed the same values reach
the same position. Keep it that way — a step that reads the keyboard, the wall
clock, or an RNG directly cannot be reconciled.

**`PlayerInput` is a component on the body, not a resource**, and that is what
makes the step a function of one body's inputs rather than of whatever the
last writer said. A server steps every player in the same tick from a
different set of inputs; a global input would walk them all the same way.
`Player` requires it, so no body can exist without one.

Two consequences worth knowing:

- **Aim travels in the input.** `mouse_look` accumulates `yaw`/`pitch` onto
  `PlayerInput` and the step copies them onto the body, rather than writing
  the body directly. Facing decides which way "forward" is, so a server
  handed movement without aim walks the body the wrong way. `mouse_look` does
  still write `Transform.rotation` itself — waiting for the next fixed step to
  see the view move is 15 ms of lag on the one thing that has to feel
  immediate — but that write is for the view, and the step is what turns the
  body.
- **`LocalPlayer` marks the one body this machine drives.** Everything that
  reads a keyboard or a mouse looks for it and nothing else; a system querying
  `With<Player>` would steer every body in the world at once.

### Three clocks, and what consumes an edge

Input crosses two clock boundaries and each has its own home:

| Where | What lives there |
| --- | --- |
| `InputLatch` (resource) | What this machine's keyboard has said since the last tick. A fact about the peripherals, not about a body, so it outlives every body — which is why `reset_for_play` clears it. |
| `Inputs` = `ActionState<PlayerInput>` (component) | What *this tick* asked one body to do. Written by `write_client_inputs`, buffered and sent by Lightyear, replayed by rollback. |
| `Player`, `PhysicsBody`, `Stance` | What the step made of it. |

**`write_client_inputs` is the only place an edge is consumed.** It runs in
`FixedPreUpdate` inside Lightyear's `WriteClientInputs` set, copies the latch
onto the local body's `Inputs`, and clears `jump`, `attack` and `select` from
the latch. Level fields — movement, sprint, aim — are carried across, since
`gather_input` reassigns them every frame and the latest reading is the right
one.

**Nothing downstream may consume what it reads.** `step_player`, `pull_trigger`
and `select_weapons` take `&Inputs`, never `&mut`. Rollback replays a tick from
its buffered input, so a step that took the jump out of it would replay as a
step that never jumped — and the symptom is not an error, it is a body that
lands somewhere slightly different every time the connection hiccups. That is
also why `select_weapons` selects rather than takes: selecting the slot already
held is not a second switch, so replaying it changes nothing.
`replaying_a_tick_reads_the_same_input_again` is the test that pins it.

`Inputs` is Lightyear's `ActionState` used directly as the storage rather than
a `PlayerInput` of our own kept beside it. The alternative is a bridge that has
to be exactly right about *when* it copies, and rollback replays ticks out of
order; one value written and read in place has no such window to get wrong.

One trap when adding a system that wants the local body's `Transform`
alongside the camera's: put `With<Player>` in the filter next to
`With<LocalPlayer>`, redundant as it looks. It is what proves the query
disjoint from the camera's `Without<Player>` one, and without it Bevy refuses
the system at runtime rather than at compile time.

Collision comes from the rooms, not from the baked meshes
([`src/game/collision.rs`](src/game/collision.rs)) — but from the *same* face
subtraction, via `Room::collision_slabs`, so an opening you can see through is
an opening you can walk through. Two operations, and they are not
interchangeable:

- `move_and_slide` is swept per axis. It asks whether the body's leading edge
  crosses a wall plane during the step, so speed cannot tunnel through a wall.
- `depenetrate` handles the world moving instead of the body: a floor raised
  onto a standing player encloses it having crossed nothing. It prefers escapes
  that land inside a room over shorter ones that do not, because the short way
  out of a floor is downwards, through the map.

## What a body does once it is dead

A corpse is a **second entity**, not the same one with its animator taken
away. [`src/game/ragdoll.rs`](src/game/ragdoll.rs) copies the dying body's
pose out and stands its own entity up in it; `reap_the_dead` then removes the
original as it always did. Keeping the same entity would mean everything that
asks the world about players has to ask whether each one is still alive — a
corpse would still have hitboxes, a `PlayerId`, a camera hanging off it and a
health pool at zero waiting to be reaped again.

**The rule is a query, not a marker**: a `Skeleton` and a `Damageable` at
zero. A crate has health and no skeleton, so it vanishes as before; a spawn
point's preview has a skeleton and no health, so nothing can kill it. The
corpse carries the *same* `Skeleton`, so `BodyMeshPlugin` dresses it from the
same cached meshes in the same colour — a class's corpse is its own build,
with no second asset anywhere. What it must never carry is a
`SkeletonAnimator`: the solver owns a corpse's `Pose`, and two writers on one
component is a body that lies there playing its idle.

The ordering spans two plugins and nothing in the type system holds it
together: `raise_ragdolls` has to run **before** `reap_the_dead`, because by
the time `Died` is written there is no pose left to copy. Break it and bodies
quietly stop leaving corpses — there is no error. There is a test at the
bottom of `ragdoll.rs` that assembles both plugins for exactly this reason.

Simulation is position-based dynamics over **two points per bone**, a head and
a tail, hand-written for the same reasons the rest of the physics is: no C
library, so it builds for wasm, and every constraint is something the rig
already states — a bone's length, a joint's offset from its parent, and the
limits in [`src/common/skeleton/joints.rs`](src/common/skeleton/joints.rs). A
new class ragdolls correctly because its bones are already the right length.
Four things about it are load-bearing and easy to undo by accident:

- **The walls are solved *with* the rig, not before or after.** Rig last and a
  foot is pushed back through the floor a little further every tick, until the
  corpse crawls across the room. Walls last and nothing restores a bone's
  length after the world moved one end of it, so the body stretches.
- **Velocity is read off where the body ended up**, at the end of the tick,
  not written during the move. Otherwise a foot lifted out of the floor picks
  up an upward velocity for having been lifted, hovers, never registers
  contact, and so never feels friction — and a corpse that lands spinning
  spins forever.
- **The root bone has no joint and must not be given one.** A limit needs a
  parent to be measured against; applied to the pelvis it is measured against
  the world, which welds the body upright with every other joint straining
  against it.
- **Nothing the solver moves may feed back into the root's frame.** Two points
  cannot express a twist, and the root's twist is the one that matters: it is
  the frame every limit on the body is ultimately stated against. Reading it
  off the hips looks obviously right and is a closed loop with gain — the roll
  sets the legs' limit frames, the limits move the legs, the legs move the
  hips. Bodies pick up a bias that drag exactly cancels and slide across the
  room forever. It is seeded from the pose the body died in and only ever
  swung onto wherever the pelvis now points.
- **Limits are relaxed towards, not snapped onto.** A hinge removes sideways
  motion outright, and a hard projection fighting the distance constraints
  overshoots — an arm flaps at the elbow for as long as you watch it.
- **Limits are stated in the bone's own rest frame.** That is what lets one
  table cover both sides of the body: the rig builds mirrored rest rotations,
  so "an elbow bends forwards" is the same local rotation on both arms.

Force arrives as `RagdollShove` — a **point, a `Push` and a radius**, not a
bone. The bones inside the radius are shoved and the joints drag the rest
along, which is what makes one message serve a bullet (a tight radius, one or
two bones) and an explosion (a wide one) without either knowing about the
other. The `Push` is the one thing they cannot share: a bullet is
`Push::Along` a fixed direction, an explosion is `Push::Outward` from its own
point. Write a blast as a directed shove and it slides every corpse in the
room the same way instead of throwing them off it. A weapon writes a shove for
*every* hit it lands and never asks whether anything died; the shot that happens to be fatal lands on a corpse
raised earlier in the same tick. Its units are honest: `push` is a speed at
the centre, not momentum to be divided by a mass. Mass is real — a bone's own
volume — and what it decides is how much of the body a shove drags with it,
not how fast the bone that was hit leaves, because a rocket that flings a
scout's hand at two hundred metres per second is a correct simulation and a
bad game.

None of that announces itself when it breaks — you get a corpse that crawls,
buzzes, stretches or windmills an arm, and each has a different cause. The
test that catches them is `every_corpse_settles_however_it_lands`, which drops
**forty** bodies rather than one, because whether any single body settles turns
out to depend on exactly how it lands. Run it after touching any constant in
that file.

Corpses belong to the match: cleared on leaving Play and again in
`reset_for_play`, and they age on game time, so nothing rots while somebody is
in the editor. They sleep once settled, and a shove wakes them.

## Damage, weapons and projectiles

**Nothing applies damage to a health pool except one system.** A weapon writes
a `Damage` message — target, source, amount, where — and is finished;
`apply_damage` ([`src/game/damage.rs`](src/game/damage.rs)) is the only thing
that touches a `Damageable`, clamps the overkill and writes the `DamageDealt`
record everything downstream reads. Two weapons each doing that for themselves
is how a scoreboard ends up adding to more health than the map contains.

The order inside a tick is stated as **sets, not as named systems**
(`DamageSystems`):

| Set | What is in it |
| --- | --- |
| `Deal` | Everything that writes `Damage`: `fire_hitscan`, `step_projectiles`, `explode`, `fire_flame`, `burn`. |
| `Apply` | `apply_damage`, and nothing else, ever. |
| `Resolve` | `record_damage` then `reap_the_dead`. |

A new damage source joins `Deal` and says nothing about what happens after it.
Before the sets, `DamagePlugin` had to name every weapon so it could order
itself after them, and forgetting one produced no error — just the occasional
kill credited to nobody.

**An explosion is a message too.** `Explosion` (a point, a radius, splash
damage, knockback, a source, and an optional direct hit) is written by anything
that goes off, and `src/game/explosion.rs` is the only thing that knows what
splash means. Three rules live there and a player notices immediately if any is
wrong: a wall stops a blast (the same `ray_distance` that shortens a bullet), a
direct hit takes the direct number *instead of* splash rather than on top of
it, corpses are thrown `Push::Outward` rather than along, and each damage
number is anchored **over the body it hurt** rather than over the blast —
every victim of one rocket shares the blast's centre exactly, so numbers put
there are superimposed rather than merely close and six hits read as one.
Anything that hurts several bodies at once from a single origin wants that
last one. Splash reaches anything with `Hitboxes` — being shootable and being
catchable in a blast are deliberately the same property.

**Knockback on a body that is still alive is missing on purpose.** `step_player`
writes horizontal velocity outright from the movement input every step, so an
impulse given to a standing player is gone by the next tick. Rocket jumping
needs a movement model with momentum; the explosion already carries the number
it would want.

### One projectile type, several weapons

`ProjectileSpec` ([`src/common/projectile.rs`](src/common/projectile.rs)) is
thirteen numbers, and the rocket launcher, the pipe bomb launcher and the
TF2C-style RPG are three `const`s of it. A grenade is a rocket that falls,
spins, bounces and goes off on a timer. Adding a fourth weapon is adding a
constant, not a type — and `Weapon` in [`src/game/weapon.rs`](src/game/weapon.rs)
is `Hitscan` or `Projectile(spec)`, which is the whole taxonomy.

Things worth knowing before touching `src/game/projectile.rs`:

- **The move is swept**, against the walls and the hitboxes at once, using the
  same two functions hit registration uses (`trace`, `CollisionWorld::ray_hit`).
  A rocket covers half a metre a tick and a wall slab is half a metre thick, so
  a teleport-then-overlap test waves it straight through.
- **Bounces are resolved inside the step**, bounded at four contacts. A pipe
  thrown into a corner meets two walls in the same 15 ms.
- **A projectile never direct-hits the body that fired it** — the muzzle is
  inside your own hitbox. That is not a friendly-fire rule: your own splash
  hurts you like anyone else's, because there are no teams.
- **The spec is copied onto the projectile**, so switching weapons cannot reach
  back and change a shot that has already left.
- Position goes through `PhysicsBody`, which buys interpolation for free —
  drawn at the tick, a 30 m/s rocket visibly stutters.

Every firing system reads `TriggerPulled`, which `pull_trigger` writes after
taking the `PlayerInput::attack` latch **once**. Several firing systems each
taking the latch for themselves is a race where whichever ran first ate the
press.

Whether holding the button keeps firing is `Weapon::cadence()` — `Semi` for
everything you click, `Automatic { interval }` for the flamethrower — and the
cooldown lives in a `Trigger` on the body. It counts *down*, so the default of
zero is a body that may fire immediately; counting up would make a freshly
spawned player wait out an interval before their first shot and read as a
misfire. The rate is quantised to whole ticks, deliberately: carrying the
remainder would let a weapon that had been idle empty several shots on
consecutive ticks.

### Fire

Fire is the first source whose interesting half happens *after* the hit, and
it needed no new machinery for that: `burn` sits in `Deal` and writes a
`Damage` every half second like anything else would.

- **`Burning` is the state and the tag.** It carries who lit it last, which is
  who afterburn is credited to — the shooter may be dead or gone by the time
  the last tick lands.
- **Re-lighting resets the timer, never extends or maxes it.** A short burn on
  top of a long one is a short burn. But re-lighting deliberately does *not*
  touch `until_next`: a flamethrower re-lights ten times a second, and pushing
  the next tick out each time is a burn that never ticks while somebody keeps
  burning you. That one is easy to "fix" into a bug and has a test.
- **The cone is sampled, and the sampling is worth nothing.** A puff is
  `FlameSpec::rays` traces and a body caught by four of them takes one puff. A
  weapon that hurt more because it was traced more finely would be a weapon
  whose damage is a fidelity setting. There is no RNG in the spread — a fixed
  pattern is what a server can re-run and what a player can learn to aim.
- **Burning is an input to a body's colour, not a `BodyTint` written onto it.**
  Overwriting the tint means remembering what was underneath and putting it
  back, and forgetting leaves a body scorched for the rest of the match.
  `body_colour` in [`body_mesh.rs`](src/game/body_mesh.rs) is the one place
  that decides, and `recolour_bodies` swaps materials on the existing parts
  rather than rebuilding a body because somebody set it on fire.

Testing all of this by hand is what `ProjectileEmitter` is for: `G` plants one
firing whatever you are holding every two seconds, `B` clears them. Plant one
in front of the animation grid and the sixty standing bodies become a firing
range. It reads the keyboard directly rather than going through `PlayerInput`,
which is fine precisely because planting a prop is not part of the simulation.

## Which end of the wire this is

`NetRole` ([`src/common/net.rs`](src/common/net.rs)) is `Solo`, `Listen { port }`
or `Client { host, port }`, and **the default is `Solo` — a closed game with no
socket at all**. Opening the editor to lay out a room must not involve the
network, so a solo game is a real role rather than a server nobody connected
to; making it a degenerate listen server would put a loopback connection in the
way of the one thing this editor exists to make fast.

`--serve [PORT]` is a **listen server, not a dedicated one**: the window still
opens, the editor is still there, and the host is a player. That is what makes
between-round editing testable with somebody else standing in the map.
`--connect HOST[:PORT]` joins one. The two conflict at the clap level. Bare
`--serve` takes `DEFAULT_PORT`. The same two choices are in the `Multiplayer`
menu, which opens the connect dialog in
[`src/editor/net_menu.rs`](src/editor/net_menu.rs) — a floating
`egui::Window` rather than an `egui_dock` tab on purpose: the panels rule is
about surfaces you work in, and this is a modal question with an answer.

Two rules, and both are easy to undo:

- **Ask `NetRole::is_authority()`, not which variant it is.** Solo and Listen
  both simulate and are believed; a client predicts and is corrected. Almost
  nothing cares about the difference between playing alone and hosting, and a
  system that matches on the variant has to be found again the first time a
  fourth role appears. `has_authority` and `is_remote_client` are run
  conditions over the same question.
- **`NetRole` is written in exactly one place**, `apply_net_requests`, which
  reads `NetRequest`. Everything else — the CLI, the menu, the dialog — asks.
  There is nothing to tear down today; the moment there is, every caller
  already routes through the place that will do it. It also declines to write a
  role equal to the one already set, because the transport will hang off
  `Changed<NetRole>` and re-picking "Host" must not drop everybody connected.

### What a networked body is

A body is one entity on every machine, in four layers. Which layer a component
belongs to decides who writes it and whether it crosses the wire, and almost
every bug in this area has been a component in the wrong layer.

| Layer | Components | Written by | On the wire |
| --- | --- | --- | --- |
| **State** | `PhysicsBody`, `Player`, `Stance` | the step, on whoever simulates | replicated, predicted |
| **Consequence** | `Damageable` (+`DamageLog`) | the damage layer, on the authority only | replicated, *not* predicted |
| **Identity** | `PlayerId` | the server, once | `replicate_once` |
| **Intent** | `Inputs` (`ActionState<PlayerInput>`) | the owning client | client → server only |
| **Description** | `BodyRequests` | `describe_player_bodies` on simulated bodies | replicated |
| **Presentation** | `Skeleton`, `Pose`, `SkeletonAnimator`, `Gait`, `AnimationPhase`, `SkeletonRoot`, `BodyMesh`, `Hitboxes`, `Ragdoll` | every process, locally | **never** |

**Poses are never transmitted.** A pose is twenty-odd quaternions per body per
tick and it is the *output* of a state machine that is deterministic and
present on every machine. What crosses is the machine's input — `BodyRequests`,
nine bools — and each process runs `AnimationState` over it. `Gait` is not sent
either: it is advanced from ground covered, and the ground covered is
`PhysicsBody`, which is. `AnimationPhase` is derived from `PlayerId` by
`phase_bodies_by_id`, so two clients put the same body at the same point in its
cycle; an `Entity` index or an RNG would give every viewer a different answer.

**Corpses are never transmitted either.** `raise_ragdolls` runs on every
process, because a corpse is a local reaction to a fact — this body's health
reached zero — and that fact *is* replicated. What differs between machines is
which way an arm flopped, and nobody can tell.

### Three markers, and none of them is a synonym

This is the distinction the whole layer rests on. Getting it wrong does not
error.

| Marker | Means | Who has it |
| --- | --- | --- |
| `Player` | this is a person's body | every body, everywhere |
| `Simulated` | **this process steps it** | all bodies on a server; only the predicted one on a client |
| `LocalPlayer` | **this machine's keyboard drives it** | exactly one body, or none |

`spawn_player` adds `Simulated` — whoever spawns a body steps it — and
deliberately **does not** add `LocalPlayer`. It used to, and that single line
produced a crop of symptoms that looked unrelated to each other and to their
cause: on a host, the second player to join made `gather_input` (a `Single`)
match two entities and silently stop running, so the host could not move;
`hide_own_body` hid every body in the match; and every body was given its own
camera and its own `IsDefaultUiCamera`. Whose body it is is answered by whoever
knows — `enter_play` for a solo game or a host, `claim_our_own_body` on a
client.

What each marker gates:

- `Simulated`: `step_player`, `pull_trigger`, `select_weapons`,
  `describe_player_bodies`. A body driven from the wire carries a `Loadout` and
  a `Trigger` like any other and an `Inputs` nobody ever fills, so an ungated
  system either fires somebody else's gun locally or flattens the server's
  description to "standing still" one frame after it arrives.
- `LocalPlayer`: `gather_input`, `mouse_look`, `place_camera`,
  `give_the_local_body_a_camera`, `hide_own_body`, and the "is this mine" test
  in `draw_hitboxes` and `draw_skeletons`.
- Authority (`has_authority`, which answers `true` with no network layer at
  all): all of `DamageSystems`, and the health restore in `reset_for_play`.
  Nothing about damage is predicted — guessing a kill and being wrong is a body
  that falls over and stands back up, which is worse to watch than a kill that
  arrives a round-trip late.

`reap_the_dead` therefore does not run on a client: the entity belongs to the
server, which despawns it and replicates that. `raise_ragdolls` hides the body
it made a corpse of, because on a client the despawn is a round-trip away and
the body would otherwise stand upright inside its own corpse.

### Who gets a body, and when it goes away

The server spawns every body, its own included. A client never spawns one.

| Moment | What happens |
| --- | --- |
| Client connects mid-round | `give_arriving_clients_a_body` |
| Round starts with clients already connected | `give_waiting_clients_a_body`, on `OnEnter(Play)` |
| Client's body reaches it | `claim_our_own_body` marks it `LocalPlayer` + `Simulated` + `InputMarker` |
| Client disconnects | `ControlledBy { lifetime: SessionBased }` despawns it on the server; the despawn replicates |
| Round ends | the server's `leave_play` despawns every body; a client's despawns none, because they are not its to despawn |

Prediction is **in place**: `PredictionTarget` is a replication target for the
`Predicted` marker, so a client gets one entity per body with `Predicted` on
its own — not a predicted copy beside a confirmed one. There is no ghost of
yourself to hide.

There is deliberately **no `InterpolationTarget`**. An interpolated body is a
second entity written by interpolation functions, and none are registered — so
targeting it produces a body nothing ever writes to, which is a player you
cannot see. Everybody else's body is the plain replicated entity, smoothed by
`interpolate_bodies` between the last two positions the server sent.

**Known gaps, all of them "not sent yet" rather than "broken":**

- `Loadout` is not replicated, so every remote body holds the default weapons.
- `DamageDealt` and `Died` are messages, not replicated, so a client sees no
  damage numbers and no kill feed — including for its own shots.
- Another player's shot has no visual on your machine at all: the server
  decides it and nothing carries the effect.
- No respawn. A body that dies is gone for the round.

### The map every client is standing in

A client predicting its own movement steps the same physics against its own
copy of the walls, so the two copies have to *be* the same walls. When they are
not, prediction does not fail loudly — the client walks through a doorway the
server stops it at, is corrected, walks into it again, and rubber-bands there
for as long as you watch it.

[`src/common/map_sync.rs`](src/common/map_sync.rs) sends the **feature
timeline**, not the baked geometry: features are what the editor edits, so
sending them is what will let an edit made between rounds reach everybody. The
snapshot goes on connect rather than on entering Play, because a client is in
the editor with everyone else between rounds and an empty editor is not one
anybody can work in.

- **`FeatureTimeline::adopt` is what replaces a map**, and it queues the old
  map's despawns rather than doing them, so `sync_entities` performs them
  alongside the incoming spawns. Skip it and the client stands in two maps at
  once with a `CollisionWorld` built from both — which looks like a working
  sync. The file-open path in `panels.rs` goes through the same function.
- **Re-bake room geometry after adopting.** The features have no entities until
  `sync_entities` has run, so a bake fired in the same frame bakes nothing;
  `CalculateRoomGeometry` is written and read a frame later, which is what
  makes it work.
- **`entity` is `#[serde(skip)]` on every feature, and has to stay that way.**
  A decoded feature that arrives believing it already has an entity is skipped
  by `sync_entities`, so the map is adopted and never appears.
- Only the client adopts, and the guard is on `NetRole::is_authority()` rather
  than on holding a receiver — a listen server has one too, and adopting its
  own map back would despawn the entities it is replicating.
- No history crosses: undo is a fact about whoever made the edits, and a client
  that could undo the server's map would be editing a map it does not own.

The wire format is JSON, which is the wrong choice for anything large and is
knowingly temporary — it is what `typetag` gives for free, and a map is sent
once per join. **Only the initial snapshot is sent.** Edits made after a client
joins do not reach it yet; that is the same channel and the next piece of work,
and it is the half that makes between-round editing real.

### The transport behind it

[`src/common/net_transport.rs`](src/common/net_transport.rs) is what makes a
role true, using **Lightyear** — chosen because WebTransport reaches a browser
(`renet` upstream has no transport that does) and because it ships prediction
and rollback rather than leaving them to us.

**Both `ClientPlugins` and `ServerPlugins` are always added, in every process,
including a closed solo one.** Bevy cannot add a plugin after `run()`, so
"start hosting" can never mean "add the server plugins now". What comes and
goes is a single entity, marked `NetLink`, and `Solo` holds none: nothing bound,
nothing polled, no loopback between two halves of one process. A role change
always despawns before it spawns, even from one `Listen` port to another —
rebinding in place is a half-applied state that only surfaces later.

Two things about assembling that entity are not optional and fail silently:

- **Every arriving connection needs a `ReplicationSender` on its `LinkOf`
  child**, which `dress_new_client` does. The server endpoint is one socket;
  the child entity per client is what replication is addressed through, and
  nothing inserts this for you. Without it the netcode handshake completes,
  nothing flows, and the client is dropped on the server's three-second
  timeout — after which the client panics deep inside
  `lightyear_interpolation` on a negative tick delta. It reads as a Lightyear
  bug and is a missing component.
- **The client needs `ReplicationReceiver`**, and `PredictionManager` has to
  exist as a resource. Prediction is gated on that resource, not on a plugin,
  so it is inserted once and left: a process hosting now may be a client after
  the next menu click.

`TICK_HZ` in `constants.rs` sets both `Time<Fixed>` and Lightyear's
`tick_duration`. They are the same number from one place on purpose — a tick is
the unit prediction reconciles in, and two clocks that agree by coincidence
will stop agreeing.

`NetStatus` is what the link is *doing*, as against what `NetRole` asked for.
Keep them apart: a role changes the instant a menu item is picked, whereas a
connection is attempted, takes time, and may fail. A UI reading the role would
cheerfully say "Connected" about a host that never answered.

`DEV_PRIVATE_KEY` is compiled into the binary, so anyone with the game can mint
a token for any client id. That is the right trade for a LAN listen server with
no accounts behind it, and it is exactly what the token backend in
`documentation/sketch.md` replaces.

**Nothing is replicated yet.** The link is real and stable; no game state
crosses it. `PhysicsBody`, the player body and `PlayerInput` are the next
things to go over, and what they plug into is the input latch in
`gather_input` and the 64 Hz `FixedUpdate` step.

## The feature model

The core abstraction is `FeatureTrait` in
[`src/editor/editable.rs`](src/editor/editable.rs) — a `#[typetag::serde]`
trait object. Implementors today: `GlobalPoint`, `GracklePointLight`,
`EditorRoom`.

A feature does not store bare coordinates. It stores `PointRef`s, which are
per-axis absolute-or-relative references to *another feature's named point*.
That is what makes edits cascade: move a point and every room anchored to it
follows. Consequences to respect when adding a feature type:

- Implement `parent_ids()` and `resolve_references()` honestly. `FeatureTimeline`
  resolves in dependency order and will silently produce stale geometry if a
  feature under-reports its parents.
- `available_point_keys()` is what the Retarget tool offers; a point you don't
  publish there cannot be referenced.
- Registering a type takes edits in **seven** places that do not reference
  each other, and missing one makes load half-work — the file saves and the
  feature is simply absent when it comes back. In
  [`editable.rs`](src/editor/editable.rs): `create_object_from_type_key`. In
  [`action.rs`](src/editor/action.rs): the `FeatureData` variant,
  `FeatureSnapshot::blank_object`, and `feature_data_kind`. In
  [`save.rs`](src/editor/save.rs): `snapshot_data_kind`, the save arm in
  `save_feature_snapshot`, and the load arm in `load_feature_snapshot`.
  Nothing in the type system catches an omission, so add a round-trip test
  next to the ones at the bottom of `save.rs` — that is what they are for.
- To make it visible and clickable, also add arms in
  [`show.rs`](src/tool/show.rs) (`GizmoVisibility` plus both matches),
  [`tool_helpers.rs`](src/tool/tool_helpers.rs) (`find_nearest_feature_hit`),
  and [`point_drag.rs`](src/tool/point_drag.rs) (`is_point_like`) if it is
  point-shaped.

Undo/redo is `FeatureTimeline` plus `Action`/`FeatureDelta`: before/after
snapshots per feature, with `try_coalesce_incoming` folding a drag into one
entry. History is persisted into the blueprint, so it survives save and load.

## Save format

`.gmb` is a SQLite database with a numbered migration chain in
[`src/editor/save.rs`](src/editor/save.rs) — currently `SCHEMA_VERSION = 3` in
`src/constants.rs`. **Never edit an existing migration.** Add a new numbered
entry to `migrations()` and bump `SCHEMA_VERSION`; files in the wild are at
older versions. Saves write through a `.bak` and restore it on failure.

## Strings

Every user-visible string goes through the lang layer in
[`src/common/lang.rs`](src/common/lang.rs). Never hardcode display text.

```rust
get!("tools.select")                          // plain lookup
get!("room.messages.ghost", "me", a, "other", b)   // fills { me } / { other }
```

Placeholder syntax in the TOML files is `{ name }`. A missing key renders as
`<tools.select>` rather than panicking, so a wrong key is visible on screen and
not a crash.

Files live at `assets/<pack>/lang/<code>.toml`. Loading merges two layers: every
pack's `en-US` first as a base, then the chosen language on top, each walking
packs lowest-priority-first. So **an incomplete translation degrades to English
per key**, not to `<key>`. `default_packs()` is the stand-in until Grackle has a
real pack list; when one arrives, pass it and call `remerge()` on changes.

`de-DE` is well behind `en-US`. Because of the base layer that is cosmetic
rather than broken, which is why there is no coverage test — only
`every_language_keeps_its_placeholders`, which checks the keys a translation
does cover. Once `de-DE` catches up, replace
`an_incomplete_translation_falls_back_key_by_key` with a real coverage
assertion.

`cargo run --bin ensure_lang` stubs missing keys into non-reference files. It
writes `MISSING <key>` as the value, which will trip the placeholder test on any
key that has slots — fill those in by hand.

## Conventions

- Each tool is a `Plugin` in `src/tool/`, added in `ToolPlugin::build`, gated on
  the `Tools` state. Follow that shape rather than adding systems to the editor
  directly.
- Panels are `egui_dock` tabs — add a `TabKinds` variant in
  [`src/editor/panels.rs`](src/editor/panels.rs), not a floating `egui::Window`.
  The commented-out `ToolPlugin::toolbar` is the old floating-window approach;
  don't revive it.
- Asset paths are relative to the working directory (`assets/default/...`), so
  binaries must run from the repo root.
- **Every binary links `lib.rs`; none re-declares the module tree.** `main.rs`
  is `use grackle::...` and plugin wiring, nothing else. This used to be
  `mod common; mod editor; mod tool;`, which compiled a second copy of the
  whole tree with its own `LANG` table and its own caches. That is not a build
  time problem to get round to — a client and a server that must reach the same
  position from the same inputs cannot be two copies of the simulation, and the
  moment a second copy exists there is no way to tell which one a bug is in.
  A new binary goes in `src/bin/` and imports; if something it needs is
  `pub(crate)`, widen it rather than reaching around it.

## Targeting wasm

Nothing here builds for `wasm32-unknown-unknown` today. Known blockers, roughly
in order of how load-bearing they are:

- **`rusqlite` with `bundled`** is a C library and will not compile to wasm. The
  blueprint format depends on it. This is the main argument for the compiled map
  format: let the editor stay native for authoring, and give the runtime a
  format it can read in a browser.
- **`rfd` + `pollster::block_on` inside `std::thread::spawn`**
  ([`panels.rs:402`](src/editor/panels.rs:402)) — no threads on wasm, and
  blocking the main thread deadlocks.
- **`lang.rs` reads `assets/` via `std::fs`** at first use, outside Bevy's
  `AssetServer`.

Three are already dealt with: `objc2` is scoped to macOS, `getrandom` has its
`wasm_js` feature plus the `getrandom_backend` rustflag in
[`.cargo/config.toml`](.cargo/config.toml) — both halves are required, the
feature alone selects nothing — and `iyes_perf_ui`, which was pinned to a git
branch rather than a release, is gone: the F3 overlay in
[`src/common/perf.rs`](src/common/perf.rs) is now a plain `bevy_egui` window
over `FrameTimeDiagnosticsPlugin`, so there is no per-Bevy-release fork to
chase.

`cargo check --target wasm32-unknown-unknown` is worth running as a probe: it
now gets all the way to `libsqlite3-sys` trying to compile bundled C, which is
the real blocker and the reason the compiled map format exists.

When adding runtime code, keep it free of these rather than porting them later.
