# Rigs: a generic animator

The plan, and the checklist, for making the animator about *a* rig rather than
*the* humanoid: data-named bones, rig-declared regions and limbs, IK beyond two
bones, parametric controllers and scripted IK sequences in clips, prop rigs,
and taunts editable from the animation editor. It is written for non-humanoid
characters first, and to make editing taunts complete. Tick items as they land.

## Where things stand

**A prop can be animated, but only rigidly, and only while it is held.** A
`.gpp` carries a `ClipSet` (`PropDoc.animations`). The game plays `deploy`,
`fire`/`alt_fire` (viewmodel), `recoil`/`alt_recoil`, `reload` and
`body_reload` from it. Those clips move the weapon *as a whole* (`Channel::Weapon`,
`WeaponRotation`, as a deviation from the carry) and the arms (`Space::Grip`,
effectors). Nothing on a prop moves by itself: `baked::parts` makes one mesh
per body per surface and keeps no part identity, and `place_held_weapons`,
`float_pickups` and hats write one transform to every part. A magazine, a hinge
or a spinning barrel is impossible. `Channel::Feature` is a reserved comment.

**The pipeline is already layered.** The `Update` chain in
`src/game/skeleton.rs` runs: `advance_animators` (it overwrites the whole pose:
state clip or pose function, then `duck_under` and `plant_feet`, then
`stand_where_claimed`), `look_with_the_head`, `place_the_weapon`,
`perform_weapon_clips`, `perform_conditions`, the flinches, `fade_the_layers`,
`play_the_clips` (`apply_stack`), and `grip_the_weapon` last. `Layer` is
Locomotion < Weapon < Humiliation < Taunt < Flinch. Each one claims `Regions`,
a `u8` of legs, torso, head, arm.l, arm.r and weapon. Locomotion never goes on
the `ClipStack` in a match; it is the animator's pose underneath.

**IK is one analytic two-bone solver** (`ik::solve`, law of cosines plus a
pole). There are no N-bone chains, no look-at and no aim constraint. "Look" is
a fixed neck/head share of the pitch. The parametric passes are foot planting
(gait), duck depth, look, the weapon's placement and the grip. Everything else
is keyed: `Rotation`, `Root` and `Effector` tracks, with an effector's target in
one `Space` for the whole track.

**Humanoid is baked in everywhere**, and the root of it is
`Bone.name: &'static str`, so a bone's name cannot come from a file. Above it:

| Assumption | Where |
| --- | --- |
| The bone names, one builder, nine `Proportions` | `rig.rs` `bone`, `humanoid`, `Proportions` |
| `Pose` keyed by `&'static str` | `rig.rs` |
| Four limbs | `ChainRef`, `ik::{LEFT,RIGHT}_{ARM,LEG}`, `elbow_pole`, `default_pole` |
| Six regions, bone→region by name | `play.rs` `Regions`, `of_bone`, `of_track`, `Layer::claims` |
| Units read off particular bones | `Unit::{LegLengths,ArmLengths,HipHeights}`, `ik::leg_length` |
| Frames hung off particular bones | `Space::{Carry,Shoulders}` (chest tail), `Space::Grip` (right hand = trigger), `Space::Eye` |
| Two feet half a cycle apart | `gait::foot_offsets` → `[FootOffset; 2]`, `footfalls`, `Timing::{LeftFoot,RightFoot}`, `plant_feet` |
| Joint limits and mesh profiles by name | `joints::limit_of`, `mesh::profile` |
| Neck/head look | `game/skeleton.rs` `look` |
| Head hitbox, crown, eye, chest carry, hands | `hitbox.rs`, `archetype.rs` `crown_frame`, `viewmodel.rs`, `hold.rs` |
| A corpse and a mesh cache are a function of `Proportions` | `CorpseBuild`, `body_mesh.rs` `BuildKey` |
| Pose functions per state | `state.rs`, `gait.rs` (`idle_pose`, `gait_pose`, …) |
| One hull and one eye height for every class | `class.rs` |

Bots, name tags and the PBD ragdoll solver itself are already rig-agnostic.

**Taunts.** `taunts.toml` already states `movement` (free, stopped, slowed),
`turning` (free, locked, at most N°/s), `weapon`, `claims` and `length`, and
`step` enforces all of them. What is missing is the editor:

- No UI touches `taunts.toml`.
- The animation editor cannot even open `taunt_wave.gpa`. `read_the_packs`
  loads only the 13 `AnimationState` clips, so taunt, humiliation, flinch and
  the shared recoil/reload clips are unreachable.
- A clip's `blend_in` and `blend_out` have no fields.
- Previews run only on Locomotion or Weapon, never under a taunt's claims.
- Humiliation's claims are code (`Layer::Humiliation.claims`).
- A taunt's `length` and its clip's `length` are two numbers kept equal only
  by a test.

## The shape it is going to

### A rig is a vocabulary, not a bone list

A `Skeleton` keeps its bones, but each one is named by a `String`, addressed by
a `BoneIndex`, and the rig **publishes** what the rest of the game asks for by
name:

- **Chains**: a name, a run of bones from root to tip, a role (`leg`, `arm`,
  `neck`, `tail`, `spine`) and a default pole. Two-bone chains keep the
  analytic solver; longer ones get the N-bone solver below.
- **Regions**: a name, the bones it covers and a bit. A rig has at most 32. The
  humanoid's six are just its own.
- **Sockets**: a name, a bone and an offset frame. `eye`, `crown`, `carry` (now
  the chest tail), `shoulders`, `grip.trigger`, `grip.support`, `head` (for the
  head hitbox), `pelvis` (for the death cam). A `Space` that names a socket
  replaces the closed `Space` variants that hang off particular bones.
- **Measures**: a name and a length in meters for this build: `leg`, `arm`,
  `hip`. `Unit::LegLengths` becomes `Unit::Measure("leg")`, and the existing
  spellings stay as aliases so no shipped file changes.
- **Feet and a gait pattern**: which chains plant, and each foot's phase offset
  in the cycle (biped 0, ½; quadruped trot 0, ½, ½, 0). `[FootOffset; 2]`
  becomes one per foot. `Timing::LeftFoot`/`RightFoot` becomes
  `Timing::Foot(name)`.
- **Look**: the bones that share the aim's pitch (and yaw, for a long neck),
  with shares that sum to one and limits.
- **Per bone**: joint limit and mesh profile, which today are name-keyed tables.

The humanoid builder publishes exactly today's vocabulary, so the first stage
changes no behavior. This is the rule `surfaces.md` already states for rooms:
**a new shape is a new producer, not a rewrite of every consumer.** Every site
in the table above stops naming bones and asks the rig.

**`Pose` stays keyed by name.** A pose is built without a skeleton — by the
humanoid's pose functions and by clips — so that one pose fits every build;
keying it by index would tie it to one rig. What a data-defined rig needs is
only that a bone's name can come from a file, and a file's names can be
interned to `&'static str` where the rig is read (Stage 2). Per-index storage
is a speed-up to take if profiling asks for it, not a step toward other
anatomy. A name the rig lacks is a Problem in the editor and is skipped at
runtime, never a panic, as the prop evaluator does.

### Where a rig comes from

There are two producers:

1. **`humanoid(Proportions)`**, kept as code. Its geometry has rules that are
   not just scaling (the shoulders are floored by the chest's width, the ankle
   is a fraction of height), and `seed.rs` and every shipped clip are tuned
   against it. Its vocabulary moves into the `Skeleton` it returns.
2. **A rig file**, `rigs/<name>.toml` in a pack. It holds bones in meters at a
   reference build, each tagged with a measure group, plus the vocabulary
   above. A class build is then a height, a scale per measure group and a
   girth, so it is still "nine numbers, not a model".

`classes.toml` gains `rig = "humanoid"` (the default) per class.
`CorpseBuild` and `BuildKey` become *(rig name, build)*. Rig files sync like
`ClassBuilds` (`pack_sync`, JSON bytes), so a server can ship a creature
nobody else has.

The rig editor itself (drawing bones in the viewport) is out of scope. A rig
file is text, as a blueprint is, and the figure in the animation editor shows
it. Make the editor once there are two rigs to compare.

### Clips are bound to a rig

A clip names a `rig` (absent means `humanoid`). Body state clips are looked up
per rig, at `animations/<rig>/<state>.gpa`, with the humanoid's left where
they are. The pose functions stay a humanoid fallback; any other rig ships
clips, or stands in rest until it does. `AnimationState` and `BodyRequests`
stay shared: a creature runs, strafes, crouches and falls with the same
inputs. What a state *looks like* is the rig's.

Channel changes:

- `Effector { chain: String }`. `"right_arm"` and its siblings are already
  snake_case strings on disk, so the files do not change.
- `Space::Socket(name)` beside the old variants, which become aliases.
- **`LookAt { chain, target }`**: aim a chain's tip axis at a target. Covers a
  head turning to a point, a tail pointing, a turret.
- **`Weight { controller }`**: a keyed 0–1 on a parametric controller (below),
  so a clip can let go of foot planting during a leap, or of the grip during a
  taunt.
- **Per-key target space.** Today the `Space` belongs to the track. A key may
  name its own, and a span between two keys in different spaces blends in
  world space. This is what makes a **scripted IK sequence**: hand to the
  weapon's magazine well, then to the belt, then to the crown, as one track.
  The frames are resolved at sample time, so the sequence fits every build and
  follows a moving weapon.
- `Channel::PropBone` and `PropBoneOffset` for prop rigs (below), replacing
  the reserved `Feature` comment.

### IK: two kinds, one solver family

**Parametric** IK is a *controller*: a rig-declared constraint the game feeds
each frame from state. Today's are foot planting, look, place-the-weapon and
grip. They become instances of one `Controller` trait-free enum (closed set,
for the reasons the prop feature list gives), declared on the rig with their
parameters:

| Controller | Parameters | Fed by |
| --- | --- | --- |
| `Plant` | feet, gait pattern, ground probe | `Gait`, the floor raycast |
| `Look` | chain, shares, limits | `Player.pitch`/`yaw` |
| `Hold` | carry socket, grip sockets | `HoldSpec`, the aim |
| `Aim` | chain, axis | a target (turret, a non-humanoid's weapon mount) |

**Scripted** IK is a keyed effector or `LookAt` track in a clip, with per-key
spaces as above.

Both go through the **same solvers**:

- `solve` (analytic, two bones), kept, since it is exact and the hands depend
  on it.
- **FABRIK with joint limits** for chains of three or more bones (tails,
  necks, insect legs, digitigrade legs, which are three-bone). FABRIK because
  it converges in a few iterations, handles limits by projection the way the
  ragdoll already relaxes toward them, and has no Jacobian to get wrong. A
  fixed iteration count keeps it deterministic, which hitboxes need, since
  they rebuild the pose on the tick.
- The `reach_or_leave` rule carries over: an arm that cannot reach does not
  stretch.

**Layering IK** follows the rule `animation-layers.md` was built against: a
controller drives only bones whose region it *won*. `grip_the_weapon` already
does this for the arms. `Plant` and `Look` join it, so a taunt claiming the
legs owns them outright and the feet stop being planted under it unless its
clip keys `Weight { Plant }` back on.

### Props get rigs

A prop may have **bones**: a pivot frame on a modeling feature, parented to
the prop's root or to another bone. A part (a body, after evaluation) is bound
to a bone. Then:

- `baked::parts` bakes **per bone, then per surface**, and `BakedProp.parts`
  carries a bone index. This is the blocker the reserved channel names.
- A held prop's clips may key its bones: a magazine drops, a cylinder swings,
  a barrel spins. A placed prop has an animator too: an idle loop (a fan), or
  a clip played on an event (a door, a breakable's hit).
- A prop's pose is a `PropPose`, not a body `Pose`, since a prop bone moves
  as well as turns. Its tracks are ordinary keyed tracks, so the editor's
  strip, keys and preview need nothing new.
- On the wire: still state, never poses. A held prop's clip is chosen by the
  same replicated facts (`Ammo`, `Equipped`, `Effect::Fired`) that choose the
  body's. A placed prop's looping clip is derived on every machine. An evented
  one rides the event.

### Taunts are edited where clips are

- The Library loads **every** clip under `animations/`, not only the states,
  grouped: states, taunts, conditions, flinches, shared weapon clips, others.
- A **Taunts** section lists `taunts.toml`'s entries. Picking one opens its
  clip, and the Inspector gets a Taunt section: clip, claims (one checkbox per
  region the rig publishes), movement, turning, weapon, and length, which may
  be **"the clip's"**. That becomes the default, which removes the second
  number. It also has new/rename/duplicate/delete. Saved with `toml_edit`, so
  comments and order survive, as the weapon editor plans.
- A taunt previews **on the Taunt layer with its claims**, over a chosen body
  state, so what shows through the unclaimed regions is visible. A **Move**
  toggle walks the figure at the taunt's speed factor and turns it at the
  taunt's turning limit against a swept aim. That is the only honest preview
  of `movement` and `turning`.
- Humiliation's claims move to data, beside its blend in `layers.toml`
  (`claims = [...]`), so every condition is described the same way.
- `blend_in` and `blend_out` get fields on the clip.
- Per rig: a taunt's `clip` may be a table by rig (`clip = { humanoid =
  "taunt_wave", hound = "taunt_howl" }`). A class whose rig has no clip for a
  taunt is not offered it.

### Non-humanoid bodies, the rest of the way

None of these is animation, but a creature is not playable without them:

- **A hull per class.** Today every class shares one 2.0 m box and a 1.8 m
  eye. The movement step, `fits`, spawns, the nav bake and bots all read
  `class.rs`. Per-class hulls mean a nav graph per hull size. That is a real
  cost, so a first creature should fit the shared hull.
- **Holding.** A rig without `grip.trigger` and `grip.support` sockets holds a
  weapon by a `mount` socket instead (a back turret, a jaw), with no grip
  solve. Or it holds nothing, and its loadout says so.
- **Hitboxes**: the head box from the `head` socket. It is already
  `Option`, so a headless rig is fine.
- **Viewmodel**: the rig names its viewmodel bones (`ARM_BONES` today) and
  `eye` socket.
- **Hats**: the `crown` socket. A rig without one wears none.

## Decisions to make before stage 2

1. **The first creature.** It should be different enough to break every
   assumption, and small enough to fit the hull. I suggest a **digitigrade
   biped** (three-bone legs, a tail, no arms, a jaw mount): it exercises FABRIK,
   N-bone chains, a non-humanoid gait, a mount instead of a grip, and a tail
   `LookAt`, while still walking the biped gait. A quadruped is the stronger
   test of the gait pattern, but it is hard in a 0.7 m-wide hull.
2. **Code or file for the humanoid.** The plan keeps it as code. Porting it to
   a rig file is possible once the file format can say "at least the chest's
   width", but it would re-tune every shipped clip.
3. **Per-class hulls**: now, or not until a creature needs one.

## Checklist

### Stage 0: taunts and clips in the editor (no rig work; ships alone)
- [x] Library loads every clip under `animations/`, grouped; saving a non-state
      clip writes its own file. Edits reach the game through `NamedClips`.
- [x] `blend_in`/`blend_out` fields on the clip Inspector.
- [x] Taunts section: list, new, rename, duplicate, delete; Inspector fields
      for clip, claims, movement, turning, weapon, length; `toml_edit` save;
      undo with the body clips' snapshots.
- [x] `length = "clip"`, the default for new taunts; the test that keeps the
      two equal goes. Absent still means until `G` again, so no file changed
      meaning.
- [x] Taunt preview on `Layer::Taunt` with its claims over a chosen state;
      a Move toggle that walks and turns the figure under its limits. Every
      clip above locomotion previews this way (`FigureBeneath`), weapon clips
      included, instead of as locomotion clips plus corrections.
- [x] Humiliation's claims in `layers.toml` (`LayerClaims`); `Layer::claims`
      is the fallback.
- [ ] Choosing a taunt: G opens a small picker of the class's taunts (the
      open question in `animation-layers.md`).

### Stage 1: the rig's vocabulary (no behavior change)
- [x] `Skeleton::anatomy()` ([`anatomy.rs`](../src/common/skeleton/anatomy.rs)):
      limbs (name, chain, side, pole, region, whether it straightens), feet
      (limb, phase), the two hands, the carry bone, the head, the look shares,
      the viewmodel's bones, a region per bone, and leg, arm and hip in meters,
      all published by `humanoid()`. A `Bone` carries its joint limit and mesh
      profile.
- [x] Every site in the table above asks the rig: clip effectors and their
      poles and regions, `meters_per`, layer masking, the grip, the carry
      frame, the crown, the head hitbox, the viewmodel's arms and eye, the
      look pass, the ragdoll's limits, the mesh profiles, duck depth. `bone::`
      constants survive only in the humanoid builder, its limit and profile
      tables, its pose functions, `seed.rs` and tests.
- [x] Gait and `plant_feet` over the rig's feet (`PerFoot`, at most
      `MOST_FEET`), each at its own phase and side. `Timing::LeftFoot` and
      `RightFoot` name the rig's first two feet.
- [x] `cargo check --all-targets`, `cargo test`; corpses still settle.
- Moved to Stage 2, since only a rig file can need them: `Pose` keyed by
  interned names from a file, `ChainRef` as a free chain name, and
  `Space::Socket`, `Unit::Measure` and `Timing::Foot` beside the old
  spellings. `Regions` stays six bits until a rig names other regions.

### Stage 2: rig files
- [x] `rigs/<name>.toml` ([`rig_file.rs`](../src/common/skeleton/rig_file.rs)):
      bones at a reference height (parent, head, tail, thickness, group,
      region, and a joint kind and shape by the humanoid's names), limbs,
      feet, hands, carry, head, look, viewmodel, and which limb and bone the
      measures are read off. Names are interned (`rig::intern`). A file is
      checked by building it: a bad one is dropped with a line, and the
      classes on it are built as humanoids. The default pack ships
      `strider.toml`, two legs, a neck and a tail, which no class uses yet.
- [x] `classes.toml` `rig =`, with `height`, `girth` and `[scales]` per bone
      group; a build resizes the rig to its height, lengthens each group, and
      puts the feet back on the floor.
- [x] A `Build` (rig, proportions, scales) is what a `Skeleton`, a
      `CorpseBuild` and the body mesh cache are keyed by.
- [x] Rigs ride in `ClassBuilds`, which `pack_sync` already sends; a body is
      re-rigged when its rig file changes under an unchanged build.
- [x] Another rig's state clips are `animations/<rig>/<state>.gpa`, read as
      its builds arrive (`load_clips_for_new_rigs`); a state with no clip
      stands in rest rather than in the humanoid's pose functions.
- [x] The editor's figure, bone menu, pole and Problems come from the
      class's own rig.
- [x] `every_corpse_settles_however_it_lands` drops striders too, with 16 s
      rather than 12: their neck and tail can roll after resting.

### Stage 2b: what only a rig with other names needs
- [x] `ChainRef` is a limb's name, interned so it stays `Copy`; the
      humanoid's four are constants. The editor's Reach menu lists the
      previewed rig's limbs, and a track naming a limb the rig lacks is a
      Problem.
- [x] `Space::Bone(name)`: a bone's tail, turned with the bone as the pose
      stands — the generic socket. `Timing::Foot(n)` for any foot, with
      `LeftFoot` and `RightFoot` kept as feet 0 and 1. Every shipped `.gpa`
      and `.gpp` loads unchanged.
- [x] `Unit::Measure` was not needed: every rig answers leg, arm and hip
      (arm falls back to leg), which are what the units already name.
- [x] Per-rig regions: `Regions` is 32 bits, every rig's six first and a
      rig's own names after (`Anatomy::region_names`). The strider's tail is
      `tail`. A taunt's claims are names, resolved against each body's rig
      as it taunts; the Taunt inspector offers the previewed rig's regions
      and shows a claim it lacks in orange. Humiliation's claims stay the
      humanoid's names.
- [x] The animation editor lists every rig on disk with a group of its
      states, saves them under `animations/<rig>/`, publishes them to the
      game, and stands the preview figure up as that rig (`FigureRig`).

### Stage 3: IK
- [x] A `Chain` may have `middle` bones (a rig file's limb `between`), and
      `ik::solve` hands a chain of three or more to FABRIK: positions pulled
      to the target and back, then each bone turned within its joint limit,
      four rounds of eight passes, fixed so every machine agrees. Tested for
      reach, limits, determinism and pointing at what it cannot reach. The
      strider's tail is a three-bone limb.
- [x] Controllers are the passes the anatomy already feeds — `Plant` (its
      feet), `Look` (its look shares), `Hold` (its hands) — and a clip's
      `Weight` track turns each down (`play::controller_weight`), masked by a
      claim above like any track. Look now yields the head to a layer above
      the weapon that claims it. `Aim`, for a weapon mounted on a body, waits
      for Stage 4's creature.
- [x] `LookAt { bone }` turns a bone's length toward a target, within its
      joint; `Weight { controller }` keys how much a pass acts.
- [x] Per-key target spaces (`Key::space`): keys in different spaces are
      each placed in their own and blended in world space
      (`play::sample_point`), for effectors, look-ats and the weapon.
- [x] Editor: Look at and Weight tracks, a key's own space, and every
      effector and look-at target drawn on the figure, the picked track's
      larger (`FigureTargets`).

### Stage 4: the first creature
- [x] `Class::Strider`, built on `rigs/strider.toml` (two two-bone legs, a
      long neck, a three-bone tail, no hands), with a loadout of its own.
      The digitigrade three-bone legs the decisions above suggested wait:
      a zigzag knee and hock needs per-joint poles, not one. The humanoid's
      own tests loop over `ClassBuilds::humanoids()`.
- [x] State clips under `animations/strider/` (an idle that sways the tail)
      and the wave's strider clip, `strider/taunt_wag`, named in the
      taunt's `rigs`. A body is offered only taunts it has a clip for
      (`Taunts::first_for`); the Taunt inspector picks each rig's clip.
- [x] A rig file's `mount` holds a weapon on a body with no hands
      (`weapon_transform`); nothing grips. Hitboxes, the ragdoll, the death
      cam and hats come from the anatomy already; the viewmodel draws no
      arms where the rig names none.
- [x] `a_strider_plays_a_round`: spawns as its rig, walks, shoots, is
      offered its taunt, and dies into a strider corpse. `--class strider`
      plays every body as one until a class can be picked.
- Not done: the play camera's eye is still the shared hull's 1.8 m, above
  a strider's head; a per-class eye is the hull question from the
  decisions above.

### Stage 5: prop rigs
- [x] Bones on a prop (pivot feature, parent, bound parts), edited in Model.
  A `[[bones]]` section in the `.gpp` rather than a feature in the list:
  a bone makes no body and consumes none, so it has no place in the
  evaluation order. Each binds features; a body takes the bone of its own
  feature or of the first operand it ate that has one, so a bound barrel
  stays bound after the bore is cut. Edited in the **Bones** tab beside the
  Inspector; pivots are drawn as crosses.
- [x] Bake per bone; `BakedProp.parts` carries the bone.
  `BakedPart { mesh, material, bone }`. Meshes stay in prop space; a part
  is moved by its bone's change from rest, so nothing is re-centered.
- [x] `Channel::Bone` keyed in prop clips; held props play them.
  Two channels, `PropBone` (a turn about the pivot) and `PropBoneOffset`
  (a move, meters in the parent's frame), since a magazine slides. Posed
  by `prop::rig::apply_stack` into a `PropPose`, one transform per bone,
  not a body `Pose`: a prop bone has no length or limit, and it moves.
  The body, the viewmodel and the editor's stage all pose from the stack
  they already play, masked where a layer above claims the weapon.
- [x] Placed props get an animator: idle loop, evented clip.
  `idle` loops on each machine's clock; `hit` plays on every
  `DamageDealt` naming the instance, which is relayed, so every machine
  sees it. Drawn only.
- [x] The rocket launcher, or the pistol, ships a moving part.
  The pistol's slide kicks back on `fire` and its magazine drops and is
  seated in both reloads; the crate jolts on `hit`.
