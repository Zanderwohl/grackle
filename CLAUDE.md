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

The editor is real and works. **The game does not exist yet** — there is no
player, weapon, projectile, physics, collision response, or networking code in
`src/`. There is also no wasm build yet. Adding the runtime is the current
frontier, not a finished thing to extend.

`src/unlock` and the `crate_drop` binary are a self-contained TF2-style
crate-unboxing prototype. It is orthogonal to both the editor and the game.
Don't wire new work into it.

There is a player body that walks, falls and jumps in `src/game/`, but it is a
prototype for feeling out room sizes — no health, no weapons, no networking.

`cargo check --all-targets` passes. `cargo test` has one known failure:
`tool::room::tests::test_ghost_message` asserts on a hardcoded `Entity` Display
string, and Bevy 0.17 changed the entity bit layout. The `Room` type it covers
is itself dead — superseded by `EditorRoom` — so the fix is probably deleting
both, but that call has not been made.

## Layout

| Path | What lives there |
| --- | --- |
| `src/main.rs` | The `editor` binary — plugin wiring only. |
| `src/editor/` | The document model: features, timeline, save/load, panels, cameras. |
| `src/tool/` | One module per editor tool, each its own `Plugin` with its own `Tools` state. |
| `src/game/` | Playing the open map: `AppMode` swap, player body, collision. |
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

Three more keys while playing: **`F`** swaps between first and third person
(`ViewMode` in [`src/game/player.rs`](src/game/player.rs) — your own body and
its hitboxes are hidden from inside your own head, nobody else's are),
**`F4`** toggles the hitbox gizmos, and **`F6`** toggles the bone gizmos, which
are off by default now that bodies have geometry. `F3` is the perf overlay in
both modes.

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
markers that mean *this is a real body*: `Player`, `Mannequin`, `CarouselBody`
and `AnimationDisplayMarker`. A `SpawnPoint` carries none of them — its body is
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
  `PlayerInput` in `gather_input` and consume it in the step.
- **Physics writes `PhysicsBody`, never `Transform`.** `interpolate_bodies`
  owns `Transform.translation`; writing it from a step would be overwritten and
  would skip interpolation. `mouse_look` owns `Transform.rotation`.

`PlayerInput` is also the seam prediction will need: the step is a function of
`(state, input, fixed dt)`, so a client and a server fed the same values reach
the same position. Keep it that way — a step that reads the keyboard, the wall
clock, or an RNG directly cannot be reconciled.

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
- `src/main.rs` re-declares `mod common; mod editor; mod tool;` instead of using
  the `grackle` library, so the `editor` binary compiles a second copy of the
  whole tree rather than linking `lib.rs`. Each copy gets its own `LANG` table
  and cache. Self-consistent today, but it doubles build time and means a
  process cannot share state between the two. Worth collapsing before the game
  runtime lands and there are more binaries. It is also why the bin targets emit
  dead-code warnings (`remerge`, `get_maybe`, `TEMPLATE_REGEX`) that
  `cargo check --lib` does not.

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
