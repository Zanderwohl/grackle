# Animation layers

The plan, and the checklist, for ranking everything that moves a body so that
two things wanting the same bones always have one answer. Tick items as they
land.

## The bug this is shaped against

TF2 has one: a humiliated Medic's hand stays IK-pinned to the medigun's handle
though the medigun is hidden, so the hand twists. The humiliation animation is
fine. A **procedural pass never asks whether it still owns the hand**.

Nothing here has that bug yet, because nothing here has humiliation. But the
pass that would have it exists: `grip_the_weapon` runs unconditionally after
the clips, so the first clip to raise the hands would be undone by it in the
same frame. The ranking therefore covers the procedural passes
(`look_with_the_head`, `plant_feet`, `place_the_weapon`, `grip_the_weapon`,
the viewmodel arms) as well as clips, or it ranks the wrong things.

## The hierarchy

A **total order**, resolved **per region** rather than per body. Each layer
claims some regions. Each region is driven by the highest active layer that
claims it. A layer that wins nothing is not stopped, only unseen.

**A claim above the weapon owns the region outright**, including what its
clip leaves unkeyed: `stand_where_claimed` replaces the animator's pose there
with the pose the body would hold standing still (crouched if it is
crouched), by the layer's weight, before the stack plays. Without it a taunt
claiming the legs and keying none of them would show the run cycle through
itself. The weapon layer's claims stand nothing, since the grip owns the arms
there.

Per body would make a moving taunt impossible, and firing while running too:
firing outranks locomotion only because it leaves the legs alone.

| Rank | Layer | Claims | First person | On the wire |
| --- | --- | --- | --- | --- |
| — | Death | — | — | already a separate entity (`Ragdoll`) |
| 1 | Taunt | per taunt | forced third person | state: `Condition` |
| 2 | Humiliation, stun | arms, torso, head, weapon | forced third person | state: `Condition` |
| 3 | Carry (flag, building) | arms, weapon | yes | state: what is carried |
| 4 | Deploy / holster | arms, weapon | yes | emergent from `Equipped` |
| 5 | Fire (incl. melee, spin-up, held beams) | arms, weapon; torso for melee | yes | event: `Effect` |
| 6 | Reload | arms, weapon | yes | state: the reload in progress |
| 7 | Gesture (voice command) | one arm | no | event |
| 8 | Inspect | arms, weapon | yes | local only |
| 9 | Hold (`place_the_weapon` + `grip_the_weapon`) | arms, weapon | yes | emergent from `Player.pitch` + `Equipped` |
| 10 | Look (`look_with_the_head`) | head | — | emergent from `Player.pitch` |
| 11 | Locomotion (+ `plant_feet`) | everything | — | emergent from `PhysicsBody` + `BodyRequests` |
| + | Flinch (built), landing squash | additive | no | emergent from `DamageDealt`, landing |

**Additive layers sit outside the order.** A flinch composes
(`Combine::Compose`) onto whatever won, including a taunt. A body shot
mid-taunt that does not react reads as invulnerable.

**Deploy is above fire** because it is the one that gates the other: gameplay
does not fire during a deploy, so they only overlap during a blend.

**Hold is a layer, not plumbing.** It is the bottom of the weapon family, and
`grip_the_weapon` solves an arm only when that arm's winning layer is in the
family (hold, inspect, reload, fire, deploy, carry). That one check is what
keeps the TF2 bug out. `test: a_hand_a_higher_layer_owns_is_not_gripped`.

**Humiliation leaves the legs to locomotion**, so a humiliated body runs with
its hands up rather than sliding across the floor in a standing pose.

## Decisions

### Conditions are gameplay; their animation only draws them

Humiliation, taunting and stuns are **one replicated `Condition` on the body**,
written by the simulation on the authority. Everything reads it and nobody
else writes it. It is a struct, not an enum — `humiliated` and an optional
`taunt` — because a humiliated body may still taunt, and the taunt's layer
outranks humiliation's while it lasts:

- `pull_trigger` refuses to fire.
- `HeldModel` hides the weapon. Whether a condition shows the weapon is a
  property of the condition (a taunt kill would show it; humiliation and the
  wave do not), not a side effect of its clip.
- `step_player` slows or stops the body, as the condition says.
- The animation layer plays its clip.
- The view is forced to third person.

An animation layer deciding whether a body may shoot would be a second writer,
and a client predicting its own movement would disagree with the server about
it.

### `H` toggles humiliation and `G` toggles the taunt

Debug keys, standing in for a round structure that could humiliate anybody and
a menu that could pick a taunt. Both are **edges in `PlayerInput`**
(`humiliate`, `taunt`), latched in `gather_input` and consumed in
`write_client_inputs` like `jump` and `reload`. The condition is written by
the authority, so a client's key has to reach the server as input; a key read
straight off the keyboard would toggle the condition on the client alone.

`G` starts the wave and it ends by itself after its `length` (1.2 s, two
sweeps); `G` again ends it early. A taunt with no `length` lasts until `G`.
The end is the authority's — `end_taunts` counts a `TauntClock` and clears
`Condition::taunt` — and the length is a field on the taunt rather than read
off the clip, since each machine has its own clip. A test pins the two
together.

**A raised hand has to be keyed.** The arm solver keeps the tip's world
rotation, which is right for a planted foot and leaves a hand that has been
lifted over the head pointing at the floor. Both the wave and the hands-up
clip key the hand straight along the forearm after the arm is solved.

### One taunt: a wave above the head

Like the Sniper's: one arm up over the head, waving side to side, the other
arm loose. A taunt is pack content, a clip plus what it does to the body:

```toml
# assets/default/taunts.toml
[wave]
clip = "taunt_wave"      # assets/default/animations/taunt_wave.gpa
claims = ["arm.l", "arm.r", "torso", "head", "weapon"]
movement = "stopped"     # "free" | "stopped" | { slowed = 0.5 }
turning = "locked"       # "free" | "locked" | { at_most = 90 }, degrees a second
weapon = "hidden"        # "hidden" | "shown"
length = 1.2             # seconds; absent lasts until G again
```

**Movement is configured per taunt.** `slowed` multiplies the speed
`step_player` would have reached; `stopped` is zero horizontal input (a body
still falls). A taunt that leaves the legs unclaimed and its movement free is
a walking taunt; one that claims the legs and stops the body is a standing
one, and its legs stand however hard the player presses forward. A body is
also *described* from the movement its taunt allows rather than from the keys
held (`describe_player_bodies`), so a stopped body is not told it is running
and frozen mid-stride for want of ground covered. The two settings are independent on purpose: a taunt could claim the legs
for a dance and still be shuffled across the floor slowly. **Nobody crouches
while taunting** (`Condition::may_crouch`): a taunt's clip is authored
standing, so a crouched body stands up as soon as there is room. Humiliation
still crouches, as in TF2.

**So is turning.** `turning` is how far the *body* follows the aim: `locked`
keeps the facing it started with, `at_most` turns it no faster than so many
degrees a second, yaw and pitch alike, and the short way round. The view is
aimed straight from the input and is never held back — the wave lets you look
around at a body standing still, which is the point. Movement is measured from
the body's yaw, so a conga (`at_most` turning, `slowed` movement) shuffles the
way the body is facing and swings round after the camera. When a taunt ends
the body snaps back to the aim.

Movement and turning are part of the predicted step, so `Condition` is read by
`step_player` on the client too. The tick the condition flips is a rollback,
which is correct and rare.

### Interrupting is gameplay; masking is presentation

Two different things, and only one belongs to the layer stack:

- **Masking**: a higher layer takes a region. The lower one keeps its clock
  and shows again when the region is handed back. Firing over running.
- **Interrupting**: the lower one's action is canceled and its clip ends.
  **A reload is interrupted by firing** with a round in the clip, TF2-style.
  That is a rule in `pull_trigger`, on the authority — the one place a shot
  is paid for — and the reload clip ends because the reload did.

The stack never cancels anything on its own; it only ranks what is playing.

### Blend times are per layer, and tunable

Each layer has a **blend in** and a **blend out** in seconds, ramping
`Playing::weight`. Handing a region up or down fades over the outgoing layer's
blend out, so nothing pops. A finished one-shot fades out rather than being
dropped, which replaces "seamless only because a deploy ends at zero offset".

Defaults per layer are pack content, `layers.toml` beside `classes.toml`, so
they are tuned without a rebuild. A clip may override its layer's numbers;
absence means the layer's. The weapon layer fades in at zero, since a deploy
starts below the frame and fading it in would show the hold first.

A layer fading in masks nothing beneath it, so it blends over what is there.
The grip is shared the same way: solved, then pulled back toward the clip by
the weight of whatever is fading over the arm.

### Some layers reach the viewmodel, some do not

"First person performs; third person indicates"
([weapons-in-hand.md](weapons-in-hand.md)) still holds, so a layer states
which rigs it drives:

- The weapon family drives **both**, from separate clips where they differ:
  a first-person reload is per-weapon and authored, a third-person one is the
  generic busy indication or nothing. Fire is a kick per weapon in both:
  `fire` in first person and `recoil` in third, each falling back on its own
  to the shared `animations/recoil.gpa`, so a weapon nobody has animated yet
  still kicks.
- Gestures and flinches are **third person only**. A flinch in first person,
  if ever, is a camera punch, not an arm clip.
- Taunt and humiliation never reach the viewmodel because they force third
  person.

The viewmodel arms are solved from the rest pose in eye space today, so a
first-person layer plays onto that rig (`Space::Eye`), not onto the body's.

### Forced third person does not overwrite the player's choice

`ViewPreference` is the player's choice, toggled by `F`. What is drawn is the
**effective** view: third person if a condition forces it, otherwise the
preference. Writing the preference would need remembering what it was and
putting it back, and forgetting leaves somebody in third person after a
taunt.

**The camera swings rather than cuts.** `CameraPull` runs 0 (the eye) to 1
(where third person puts it) and is eased toward the wanted view over
`CAMERA_PULL_SECONDS`; `place_camera` works out both positions every frame
and blends them, so the view not in use is always current and a swing starts
from where it really is. `ViewMode` — what everything that hides your own
body reads — is third person whenever the pull is above zero, so the body is
drawn for the whole swing back in rather than vanishing as it starts. `F`
swings too, since it is the same transition.

### Your own recoil starts on your own machine

Firing is decided on the authority, so waiting for the relayed `Effect` would
kick a round trip late, on the one body whose recoil is felt rather than
watched. So a client starts its own fire clip **optimistically** from its own
input, when what it can see says a shot would happen: the trigger pulled, a
round in the clip as last replicated, and its own guess at the cooldown.

That is presentation, not simulation — the same arrangement as an own tracer
drawn from the muzzle — so it breaks no rule about what a client may predict.
The cost is accepted: a shot the server refused still kicks. The relayed
`Effect` for a shot of our own is ignored by its `PlayerId`, or every shot
would kick twice.

### What crosses the wire

- **State, replicated:** `Condition`, what is carried, the reload in
  progress. The taunt table is sent like the weapon catalog, since its
  movement is part of the predicted step. `Condition::taunt_started` is the
  tick the taunt began on, so a body seen mid-taunt plays it from where it is.
  Only viewers read it: the authority ends a taunt on its own `TauntClock`, so
  that gameplay does not hang off the network layer's timeline.
- **Input:** the `humiliate` and `taunt` toggles.
- **Events, relayed:** firing, as an `Effect` carrying the shooter and the
  button, so every other machine starts the recoil; gestures.
- **Emergent:** locomotion, look, hold and deploy are already derived from
  replicated state on every machine. Flinch is derived from the relayed
  `DamageDealt`.

## Regions

Regions are sets of bones by name: **legs**, **torso** (spine, chest),
**head** (neck, head), **arm.l**, **arm.r**, and **weapon** (which is
`WeaponInHand` and the weapon's visibility, not a bone). A track on a bone
outside its layer's claim is a problem `Clip::problems` reports.

## Checklist

- [x] Regions, and a rank and claim on each `Playing`; `apply_stack` resolves
      per region. (`Layer` has only `Locomotion`, `Weapon` and `Humiliation`
      so far; the rest join as they are built.)
- [x] `grip_the_weapon` drives only the arms its layer won, with the test
      above. `look_with_the_head` and `plant_feet` run before the stack, so a
      higher layer's `Replace` track already overrides them; neither needed a
      check of its own.
- [x] Blend in and out per layer from `layers.toml`, overridable per clip
      (`blend_in`/`blend_out` on the clip, in the file only — no editor field
      yet); one-shots fade out instead of being dropped. Owners send clips
      out with `ClipStack::keep`, and `fade_the_layers` takes them down and
      off. The grip shares an arm with a layer fading over it.
- [x] `Condition` component, replicated: humiliated. `pull_trigger` refuses;
      `show_held_weapons` hides the weapon per condition.
- [x] Taunting as a condition: `Condition::taunt`, beside `humiliated`.
- [x] A taunt ends by itself after its `length`, on the authority.
- [x] A taunt's start tick on the wire (`Condition::taunt_started`, from
      Lightyear's `LocalTimeline`), so a late joiner sees it partway through.
      The clip is lined up once as it arrives and then ticked by the frame,
      since the timeline steps at the tick rate.
- [x] `humiliate` edge in `PlayerInput` on `H`; the authority toggles it.
- [x] `taunt` edge on `G`; the authority toggles the first taunt.
- [x] Humiliation: a hands-up clip (`animations/humiliated.gpa`) on the arms,
      legs left to locomotion. Keyed in `Space::Shoulders`, the carry point
      turned with the body rather than the aim, so looking up does not tilt
      it.
- [x] `taunts.toml` read into `Taunts`, synced like the other pack tables;
      the wave clip (`animations/taunt_wave.gpa`); `step_player` applies the
      taunt's movement. Clips are not sent: each machine reads its own, as
      with body animations.
- [x] Effective view mode: forced third person under a condition.
      `ViewPreference` is what `F` toggles; `decide_the_view` is the only
      writer of `ViewMode`.
- [x] `Effect::Fired`, written by `announce_shots` from every shot and
      relayed like a tracer; `see_shots` turns it into a `ShotSeen` on every
      machine, and `guess_our_shots` makes our own from the mouse on a
      client, which then skips its own `Fired` (`src/game/recoil.rs`).
- [x] Reload interrupted by firing, in `pull_trigger`.
- [x] First and third person play different things. A body plays the held
      prop's `recoil` or `alt_recoil`; the viewmodel has a `Performance` and
      `ClipStack` of its own and plays its `deploy` and `fire` or `alt_fire`.
      Either falls back to the shared `animations/recoil.gpa`, and the arms are
      solved onto wherever the clips put the weapon. In the viewmodel
      `Space::Carry` is the framed weapon and `Space::Eye` the camera.
- [x] A fire clip on the rocket launcher.
- [x] The apparent muzzle reads the weapon as drawn, kick and all (a frame
      old, since it runs before `Update` places this frame's).
- [x] Animation editor: a weapon clip previews on the weapon layer over a
      body state picked under **Over** (with that state's corrections, as a
      match layers it); the Inspector lists what the clip's layer claims, and
      a track moving anything outside that claim is a Problem.

### Reloads are read off the ammo, not sent

A reload already crosses the wire: `Ammo.reloading` counts down, replicated.
So the clip is driven from that state on every machine — a rise starts it
(again, for each shell of a one-at-a-time reload), a drop to zero before it
finishes ends it, and a body first seen mid-reload starts it as far in as
its remaining time says. The clip is fitted to the weapon's `reload` seconds,
so one authored at any length plays across a quick pistol and a slow
launcher alike. The body plays `body_reload`, the viewmodel `reload`, and
either falls back to the shared `animations/reload.gpa`.

- [x] Reload clips from `Ammo`, fitted, joined partway, cut short; the shared
      reload and the rocket launcher's own first-person one.
- [x] The support hand leaves the weapon. A hand is moved **by its grip, not
      past it**: an arm effector in `Space::Grip` is an offset from that
      hand's own grip, in the weapon's axes, which `grip_offsets` reads off
      the stack and `grip_hands` solves onto — so zero is holding, the
      per-class support grip is honored, and fading the clip in or out slides
      the hand rather than popping it. A clip that keys no hand leaves the
      hand where the clip beneath put it, so a kick over a reload does not
      snap it back. The body, the viewmodel and the editor's figure all read
      it. The shared reload slides the hand back under the receiver as the
      weapon drops, dips to the magazine well and returns, on a path every
      build reaches: further out, the Heavy and the Civilian already lost the
      support grip under the reload's drop.

### A flinch composes; it does not rank

A flinch is on `Layer::Flinch`, last in the order and claiming nothing: it
masks nothing, owns nothing, stands nothing, and its tracks compose onto
whatever won, so a body shot mid-taunt still jolts. It is started by every
`DamageDealt` — local on the authority, relayed elsewhere — and restarted from
the top by the next hit. Which of four clips (`flinch_back`, `_forward`,
`_left`, `_right`) is picked by the way the hit traveled in the body's own
frame: from the attacker's body where there is one, otherwise away from where
it landed. Chest and neck only, so a flinch never lifts a planted foot.

- [x] Flinch layer, four directional clips, restarted per hit.
- [x] Strength by damage: `Playing::strength`, beside the weight, is how far
      a clip moves what it plays and not how faded in it is, so it masks no
      less and `fade` leaves it alone. A hit of `FULL_FLINCH_DAMAGE` (60) or
      more flinches fully, the least is `SLIGHTEST_FLINCH`, and a hit
      landing mid-flinch takes the larger of its own strength and what is
      left of the one it interrupts, so an afterburn tick does not shrink a
      rocket's jolt.

## Open questions

- **Gestures**: one arm, or the arm the weapon is not in? Which arm a weapon
  is in is a hold question.
- **Where humiliation comes from** once there are rounds. `H` stands in until
  then.
- **Choosing a taunt.** `G` plays the first in `taunts.toml` until there is a
  menu.
