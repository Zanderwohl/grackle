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
| + | Flinch, landing squash | additive | no | emergent from `DamageDealt`, landing |

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
else writes it:

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

Toggled, not timed: `G` starts the wave and `G` again ends it, and the wave
loops until then. A taunt with a length of its own is a field on the taunt,
added when a taunt wants one.

### One taunt: a wave above the head

Like the Sniper's: one arm up over the head, waving side to side, the other
arm loose. A taunt is pack content, a clip plus what it does to the body:

```toml
# assets/default/taunts.toml
[wave]
clip = "taunt_wave"      # assets/default/animations/taunt_wave.gpa
claims = ["arm.l", "arm.r", "torso", "head", "weapon"]
movement = "stopped"     # "free" | "stopped" | { slowed = 0.5 }
weapon = "hidden"        # "hidden" | "shown"
```

**Movement is configured per taunt.** `slowed` multiplies the speed
`step_player` would have reached; `stopped` is zero horizontal input (a body
still falls). A taunt that leaves the legs unclaimed and its movement free is
a walking taunt; one that claims the legs and stops the body is a standing
one. The two settings are independent on purpose: a taunt could claim the legs
for a dance and still be shuffled across the floor slowly.

Movement is part of the predicted step, so `Condition` is read by
`step_player` on the client too. The tick the condition flips is a rollback,
which is correct and rare.

### Interrupting is gameplay; masking is presentation

Two different things, and only one belongs to the layer stack:

- **Masking**: a higher layer takes a region. The lower one keeps its clock
  and shows again when the region is handed back. Firing over running.
- **Interrupting**: the lower one's action is cancelled and its clip ends.
  **A reload is interrupted by firing** with a round in the clip, TF2-style.
  That is a rule in `reload_weapons`, on the authority, and the reload clip
  ends because the reload did.

The stack never cancels anything on its own; it only ranks what is playing.

### Blend times are per layer, and tunable

Each layer has a **blend in** and a **blend out** in seconds, ramping
`Playing::weight`. Handing a region up or down fades over the outgoing layer's
blend out, so nothing pops. A finished one-shot fades out rather than being
dropped, which replaces "seamless only because a deploy ends at zero offset".

Defaults per layer are pack content, beside `classes.toml`, so they are tuned
without a rebuild. A clip may override its layer's numbers; absence means the
layer's.

### Some layers reach the viewmodel, some do not

"First person performs; third person indicates"
([weapons-in-hand.md](weapons-in-hand.md)) still holds, so a layer states
which rigs it drives:

- The weapon family drives **both**, from separate clips where they differ:
  a first-person reload is per-weapon and authored, a third-person one is the
  generic busy indication or nothing. Fire is a per-weapon kick in first
  person and a generic recoil in third.
- Gestures and flinches are **third person only**. A flinch in first person,
  if ever, is a camera punch, not an arm clip.
- Taunt and humiliation never reach the viewmodel because they force third
  person.

The viewmodel arms are solved from the rest pose in eye space today, so a
first-person layer plays onto that rig (`Space::Eye`), not onto the body's.

### Forced third person does not overwrite the player's choice

`ViewMode` stays the player's preference. What is drawn is the **effective**
view: third person if a condition forces it, otherwise the preference. Writing
`ViewMode` would need remembering what it was and putting it back, and
forgetting leaves somebody in third person after a taunt.

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

- **State, replicated:** `Condition` (which one, and the tick it started, so
  a late joiner sees a taunt at the right point in it and every machine plays
  it in step), what is carried, the reload in progress.
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

- [ ] Regions, and a rank and claim on each `Playing`; `apply_stack` resolves
      per region.
- [ ] `grip_the_weapon`, `look_with_the_head` and `plant_feet` drive only the
      regions their layer won, with the test above.
- [ ] Blend in and out per layer from a pack file, overridable per clip;
      one-shots fade out instead of being dropped.
- [ ] `Condition` component, replicated: humiliated, or taunting (which taunt,
      start tick). `pull_trigger` refuses; `HeldModel` hides the weapon per
      condition.
- [ ] `humiliate` and `taunt` edges in `PlayerInput` on `H` and `G`; the
      authority toggles the condition.
- [ ] Humiliation: a hands-up clip on arms, torso and head, legs left to
      locomotion.
- [ ] `taunts.toml` read into a resource, synced like the other pack tables;
      the wave clip; `step_player` applies the taunt's movement.
- [ ] Effective view mode: forced third person under a condition.
- [ ] `Effect::Fired`; a fire clip started from it on every other machine, and
      optimistically from input on the shooter's.
- [ ] Reload interrupted by firing, in `reload_weapons`.
- [ ] Layers declare first person, third person or both; the viewmodel arms
      play first-person layers in eye space.
- [ ] A fire clip on the rocket launcher.
- [ ] Animation editor: preview a layer over a locomotion state, and show
      which regions a clip claims.

## Open questions

- **Gestures**: one arm, or the arm the weapon is not in? Which arm a weapon
  is in is a hold question.
- **Where humiliation comes from** once there are rounds. `H` stands in until
  then.
- **Choosing a taunt.** `G` plays the first in `taunts.toml` until there is a
  menu.
