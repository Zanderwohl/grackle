# Weapons in hand

> **Status — none of this is built.** `Weapon.model` exists, names a prop, and
> crosses the wire with the catalogue; nothing reads it. This is the shape the
> work is aiming at, written down before any of it is written.
>
> Where this says a thing *is* a certain way, that is a decision already made.
> Open questions are collected at the end rather than answered inline.
>
> See [CLAUDE.md](../CLAUDE.md) for what is in the tree, and
> ["Modelling a prop"](../CLAUDE.md) for the prop editor these models come out
> of.

## The one idea

**The weapon is placed first, and the hands follow it.**

The obvious arrangement is the other one — parent the model to `hand.r` and let
the animation carry it — and it is a dead end. It makes a weapon's hold a
property of the *body's* animation, so every weapon needs its own set of poses
per class, and a weapon held while running needs a different pose again. That
is a combinatorial explosion authored by hand, and it is why the request for
"any weapon looks good with anything the body is doing" cannot be met that way.

Inverted, it collapses:

1. The body animates as it does today — legs, spine, the lot.
2. The weapon is aimed from the chest, along the player's true look direction.
3. The arms are **solved** to two grip points in the weapon's own space.

A weapon's hold is then *two points and an aim*, not a pose. It is the same
data for every class, because an arm is solved rather than posed, and it is the
same data at every moment of the walk cycle, because the lower body never
enters into it.

The machinery for step 3 already exists: `ik::Chain { upper, lower, tip }` with
`ik::solve` is what plants feet today, and `upper_arm.r / forearm.r / hand.r` is
a chain of exactly that shape.

## Decisions

### The grip convention is the reference figure's, read backwards

A prop is authored with **its grip at the origin and its axis along `-Z`** —
that is what `figure::place` already establishes, by standing the reference
figure so its right hand is at the origin and the line between its hands runs
down `-Z`. Attaching in-game is the inverse of the same rule.

One rule, stated once, or a weapon looks right in the prop editor and sits
wrong in the hand, and somebody tunes two numbers against each other forever.

### A hold is two grip points, and it lives on the prop

`HoldSpec`: where the trigger hand goes and where the support hand goes, in the
weapon's own space, plus a rest offset from the shoulder.

**On the prop, not on the weapon.** A hold is a fact about the model's *shape*,
it wants authoring next to the geometry, and the prop editor is already the
place where a reference figure is standing there to judge it against. A weapon
that wanted a different hold for the same model would be the exception, and
exceptions can name their own.

A missing support point means a one-handed weapon and the off hand is free.

### Looking and aiming are two pitches

Today `look_with_the_head` bends the neck and head to `Player.pitch`, split
`0.4 / 0.6`, with no limit — so a body looking straight down folds its head
through its own chest.

- **The head clamps to 45° up and 70° down** from horizontal, keeping the
  existing neck/head split within that range.
- **The weapon takes the full range.** Past the clamp the head stops and the
  arms keep going, which is what a person does and what every shooter draws.

So one look direction produces two values, and the thing that must not happen
is a second opinion about which is which. `Player.pitch` stays the truth; the
head's is derived from it by clamping, at the one place that bends the neck.

Note that this makes the *visible* aim more honest rather than less: shots
already originate at the eye along `Player.pitch`
(`hitscan::eye`, and a projectile at `eye + direction * MUZZLE_OFFSET`), so a
weapon that visibly points where the shot goes is a weapon that stopped lying.

### The arms are their own animation layer

`AnimationState` stays what it is — a whole-body state machine over Idle, Run,
Strafe, Crouch, Airborne, PushingWall. The arms come **after** it and compose
onto the pose it produced, which is the arrangement `look_with_the_head`
already uses and the reason it runs after `advance_animators`.

That is what lets a reload play over a sprint without either knowing about the
other.

### Models cross the wire as documents, not as meshes

After the catalogue, on connect, the way `weapon_sync` sends the catalogue and
`map_sync` sends the feature timeline.

- **The `PropDoc`, not the baked geometry.** A prop is parametric and small,
  the client already has the whole kernel, and sending features rather than
  triangles is the same choice the map layer made for the same reason.
- **After the catalogue**, because a model is named by a weapon: a client handed
  a model for a weapon it has not heard of has nothing to attach it to.
- **Only the props the catalogue names**, which is enumerable from the
  catalogue — so this needs no directory listing, and the rule in
  [`assets.rs`](../src/common/assets.rs) survives.

### `model: None` aims an empty hand

Not an error and not a placeholder model. The hand aims as though it held
something, and nothing is drawn. Most weapons will be in this state for a
while, and the failure mode of the alternative — a stand-in cube in everybody's
hands — is worse than empty hands.

### The viewmodel is two arms, not a floating weapon

First person gets a pair of arms holding the weapon, normally out of frame,
carrying the holding, firing and reload animations.

Drawn by a **second camera on its own `RenderLayers`**, with a small near plane
and its own field of view, ordered after the main camera and clearing nothing.
That is the standard arrangement and it exists for one reason: a weapon drawn
by the main camera clips through walls. Only `multicam` uses render layers
today (layer 31, for the UI painter), so there is no infrastructure and no
conflict.

`hide_own_body` already hides your own body in first person; the viewmodel arms
are what it has to stop hiding.

## Stages

Each stands alone and each is worth having on its own.

| # | Stage | Proves |
| --- | --- | --- |
| 1 | **Hold the thing.** Extract the prop cache out of the prop editor; write the grip transform down once; attach the held model to `hand.r`. | `Weapon.model` end to end. Will look wrong while walking — no upper-body layer yet, and that is expected. |
| 2 | **Split the aim.** Clamp the head, leave `Player.pitch` alone. | Independent of the rest; visible immediately. |
| 3 | **The upper-body layer.** `HoldSpec` on the prop; aim the weapon from the chest; solve both arms to its grip points. | The idea at the top of this document. This is where it starts looking right. |
| 4 | **Prop sync.** Documents on connect, after the catalogue. | A server can ship a weapon nobody else has. |
| 5 | **The viewmodel pipeline.** Second camera, layer, near plane. | First person. Needs 3: the arms have to be posed before they are worth drawing close up. |
| 6 | **Weapon animations.** Fire, reload, deploy, as parametric poses over the hold. An editor for them, someday. | |

Stage 1 comes before stage 2 only because the aim split is easier to judge with
something in the hand; they do not depend on each other.

**Stage 1 has one rule that is not negotiable**: the prop cache is *extracted*
from `prop/view.rs`, not copied. `SurfaceMaterials` and the mesh grouping are
private to the prop editor today, and two bakers for one format is the failure
this codebase keeps naming.

## Open questions

**Do the first-person arms share the third-person pose?** Sharing is the
obvious pick — the same arm bones, the same `HoldSpec`, so a reload is authored
once — and it is what the "one description" rule would say. But viewmodels are
conventionally exaggerated: bigger, closer, with motion a third-person arm
would never make. If that turns out to be wanted, the shared pose becomes a
base with a viewmodel-specific offset on top, which is a different design than
starting from one.

**Where does a shot come from once the weapon is visible?** From the eye today,
which nobody can see. A muzzle flash at the eye would be obviously wrong, and
moving the *origin* to the muzzle changes hit registration and what a corner
peek can do. The likely answer is that the shot keeps coming from the eye and
only the flash moves, but that is a decision and not an oversight.

**What happens to the hold during a crouch, a sprint or a jump?** The lower
body has states for all three and the arms currently have none. A sprint that
lowers the weapon is a real design choice with real gameplay weight, and it is
the first thing that will want an arm state that is not "holding".

**Do weapon animations live in the prop file?** The hold does, by the decision
above. A reload is a sequence rather than a pose, and the prop format has no
notion of time in it. That may argue for a sibling file, or for the prop format
growing one.

**Is a hold mirrored for a left-handed body?** Nothing is left-handed today and
nothing asks to be.
