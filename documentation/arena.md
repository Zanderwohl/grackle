# Arena

The plan for a Quake 3–style free-for-all, and the checklist of what has
landed. No teams, Mercenary only, weapons picked up off the floor and kept until
you die, first to the frag limit ends the round — and ending a round drops
everybody into the editor, so between-round map editing comes with it.

Two pieces are being built elsewhere, and this plan only states what it needs
from them:

- **Slots and selection** — weapons stating which slot they live in, several
  weapons per slot, Source style. See "What this needs from the slots work".
- **Respawn delay** — a gamemode number; it goes in the mode rules below
  when it lands.

## The mode has to mean something first

`MapMetadata.game_mode` is saved, shown in the settings panel, and read by
nothing. Everything below is "arena does X, other modes do what they do today",
so the first piece is a resource that answers for the mode.

**`GameRules` is a resource with one writer, `publish_game_rules`**, the same
shape as `publish_movement_rules`: it reads the map's `game_mode`, looks it up
in the modes table, and writes the result. Nothing else reads `MapMetadata` for
it. A mode with no entry gets `GameRules::default()`, which is today's
behavior exactly — teams, class loadouts, no scoring limit — so CTF and the
rest are unchanged by any of this.

**The modes table is pack content**, `assets/default/modes.toml`, keyed by
`GameMode::prefix`:

```toml
[arena]
teams = false
classes = ["mercenary"]
# Each lands in the slot the class's loadout permits it in, until weapons
# state their own slot.
loadout = ["medi_wrench", "laser_rifle"]
frag_limit = 20
# Planned, with the feature that reads it:
# self_damage = 0.5      # fraction of your own damage you take
```

An entry is `deny_unknown_fields`, so a rule written before the game has it is
dropped with a warning rather than silently read as nothing.

It is a `PackTable`, so `pack_sync` sends it to clients on connect with the
weapons and builds — a third table is an `impl` and a line in the plugin.
Settings that are per-map rather than per-mode (a map that wants a lower frag
limit) go on `MapMetadata` later as overrides; none are needed yet.

**When a rule takes effect.** Rules are read live, but a player's team is fixed
when their identity is made. The host's is made fresh each match; a client's
lives on its link entity and lasts the connection, so a client who joined a CTF
match keeps their side through a switch to Arena until they reconnect. Making
every identity fresh per match would fix it and is a small change to
`enter_play`, left until it matters.

The default template's mode is `arena`, so a new map plays free-for-all.

`classes` is trivially satisfied today: everybody is a Mercenary because there
is no class picker. It is stated anyway so the picker, when it comes, has
something to read rather than something to rediscover.

## No teams

**`Identity.team` becomes `Option<Team>`**, and so does a body's `Team`
component (absent rather than an `Option` field). `next_team` asks
`GameRules::teams`: Red/Blue alternating as now, or `None`.

The damage rule needs no change. `hostile` already reads `None` as hostile to
everybody — that is what a crate is — and the self exception is on `PlayerId`,
so your own rocket still hurts you.

**Spawns ignore `Side` with no team.** `choose_spawn` takes `Option<Team>`; with
none the pool is every spawn on the map. Avoiding the spawn nearest an enemy is
a later refinement, not a requirement.

### Body color

A body's color is its team, so with no teams every body would be the same gray
and nobody could tell who they were fighting. Each player gets a color of
their own instead:

- **OKLCH, fixed lightness and chroma, hue rotated per player.** Fixed L and C
  keep every player as readable under map lighting as the team colors are
  (L 0.62, C 0.10 — about the most chroma every hue holds inside sRGB, cyan
  running out first); only the hue says who you are.
- **The hue is `start + step × index`.** The first twelve players are 30°
  apart. The thirteenth onwards fill in at the 15° points between them, then
  7.5°, and so on — each round of filling halves the step and offsets by half
  of it, so the colors already handed out never move.
- **`start` is random per match**, chosen by the server when it makes the first
  identity, so the same player is not always the same red.
- **`index` is the lowest one not in use**, so a player who leaves frees their
  hue for the next to join rather than pushing everybody further round the
  wheel.

**The color is decided on the authority** — the identity holds an index on
the wheel and the match holds the start (`HueStart`, dropped with
`OurIdentity`), so a new match turns everybody's color together. The body gets
the resulting `PlayerColor` hue, which reaches everybody else as a
`replicate_once` component, the way `Team` does. No machine derives it: two
clients agreeing on join order is a coincidence, and a color derived from it
is a body that is purple on one screen and green on another.

`body_look` picks **team color, then player color, then tint, then the
default**. In a team mode a player still has a color; the team outranks it
because which side you are on is what you read across a room. Corpses keep
their `PlayerColor` as they keep their team. `BodyMaterials` is keyed by
color, so materials are per color, not per body — a dozen players is a
dozen materials.

## Weapon pickups

A weapon on the floor that a player walks over to add to what they carry, gone
when they die.

### The feature

**A new map feature, `WeaponPickup`**, not a kind on the `.gpp` `[pickup]`
section. A pickup prop is a model with a spec, and one `.gpp` per weapon would
be a second description of each weapon that has to agree with `weapons.toml`.
The weapon already names its own model, so the map names the weapon and the
model follows.

| Field | |
| --- | --- |
| `location` | `PointRef` — where it stands on the floor, like a `Prop` |
| `yaw` | radians |
| `weapon` | a catalog name, as written in `weapons.toml` |
| `respawn` | seconds of game time before a taken one is back |
| `ammo` | fraction of the weapon's reserve given to somebody who already has it |
| `style` | `PickupStyle`, defaulting to `Tf2` (hovers and turns) |

The weapon is a **name, not a `WeaponId`** on disk, because that is what the
mapper chose and what `weapons.toml` calls it; the id is a hash of it, taken
when the instance is stood up. A name nothing defines draws nothing and gives
nothing, the way a bad model name does.

The usual new-feature work, per CLAUDE.md: `show.rs`, `tool_helpers.rs`,
`every_feature_type_survives_a_save_and_load`, and `mirror` — carry the location
and reflect the yaw, so a symmetrical arena gets its pickups on both halves.

**The panel picks from the catalog**, a combo over `WeaponCatalog` sorted
by display name. Not a text field: the catalog is already loaded in the
editor, and a typed name is the thing that silently draws nothing.

### In the world

The authority stands one replicated **`WorldWeapon`** up per feature, the same
carousel rule `sync_world_props` follows — `StandsFor`/`OfPropFeature` for the
bookkeeping, and the instance follows its feature and goes with it. It carries
the `WeaponId`, its transform, and the existing `Pickup` and `Restock`, which
already mean "taken" and "coming back".

**Drawn from the weapon's model**, baked through `PropCache` by
`Weapon.model` — `prop::baked` stays the only baker. An id the catalog cannot
resolve yet draws nothing and is redressed when it can: key on absence, never
on arrival, as everything else that reads an id does. `float_pickups` gains a
`WorldWeapon` arm, or is generalised over both.

**The trigger is a fixed sphere**, a constant in `world_prop.rs`. A held prop
has no colliders, and a weapon's pickup volume is a game rule, not something a
modeler should have to author on every gun.

### Taking one

`take_pickups` generalises `give` over what a pickup gives:

```rust
enum Gift<'a> { Supply(&'a PickupSpec), Weapon { id: WeaponId, ammo: f32 } }
```

- **A weapon you do not have** is granted, with a full clip and the reserve
  `Ammo::full` gives. Whether it is switched to is the selection work's call.
- **A weapon you already have** tops up that weapon's reserve by `ammo`.
- **Full on that weapon too** — walked past, untaken, like a health pack at
  full health. A pickup that gave nothing is not spent.

**Losing them on death needs no code**, which is the point of checking it: a
body is a new entity each life, and its `Equipped` and `Ammo` are built by
`starting_equipment` on spawn. What does need code is that `starting_equipment`
reads `GameRules::loadout` when the mode has one, rather than the class's
six-weapon sampler — otherwise you spawn already holding everything the map is
offering.

Dropping the held weapon on death as a temporary pickup is Quake's, and later.

### What this needs from the slots work

Three things, stated as interface so the two threads can land in either order:

- **A weapon states its slot**, on `Weapon` in `weapons.toml`. Granting a
  weapon puts it there; the loadout file stops being where slots come from.
- **`Equipped` holds several weapons per slot**, so granting never has to
  decide what to throw away.
- **A grant that reports whether it was new** — something like
  `fn grant(&mut self, id, slot) -> bool` — paired with an `Ammo` entry per
  weapon rather than per slot, since ammo per slot stops meaning anything once
  a slot holds two guns.

Until then, a pickup can land in the 6-slot `Equipped` by the loadout's
`permitted` lists, which is enough to test the pickup on its own.

## Armor

**`Armor` is a component beside `Damageable`**: points and a max, replicated,
in the Consequence layer — the damage layer on the authority is its only
writer.

**`apply_damage` is where it is spent**, and nowhere else, for the reason
health is: two writers is a scoreboard that adds up to more than the map
contains. Of each hit, `ARMOR_ABSORBS` (Quake's two-thirds) comes off armor,
rounded up, until it runs out; the rest comes off health. Afterburn and your
own splash go through `apply_damage` like anything else, so armor soaks them
too. `DamageDealt` keeps reporting the whole hit, because a floating number is
what you did, not where it landed; the armor share can be added to the record
when something wants to draw it.

**The max is `DEFAULT_MAX_ARMOR`**, a constant beside `DEFAULT_HEALTH`, and
goes per class when health does: a Heavy and a Scout should not carry the same
plate, but giving armor a per-class table before health has one would be two
pools described in two places. Likewise the absorbed share is a constant until
a mode wants a different one. Every `Player` requires an `Armor`, and a body
starts with none.

A pickup's share of a maximum is rounded up past float noise (`share` in
`world_prop.rs`): `0.6 * 100.0` is `60.000002` in `f32`, which a plain `ceil`
made 61 — for health and ammo packs too, before armor found it. `PickupKind::Armor`
already exists and already has an amount as a fraction of the taker's max, so
its arm in `give` is a few lines; the default pack ships `armor_pack.gpp` next
to the health and ammo packs. Over-max armor that decays (Quake's red armor) is
later.

The HUD shows it over the health number while there is any.

## Knockback

Rocket jumping is the iconic arena move and needs a movement model with
momentum. Today it has none: `step_player` writes horizontal velocity outright
from the input every tick, so an impulse is gone by the next step. The
explosion already carries the number it would want.

### Momentum

**Movement becomes Quake-style acceleration and friction.** On the ground,
friction brings velocity down and acceleration brings it up toward the
wish direction and speed; in the air, acceleration is small and friction is
none. Velocity carries between ticks, so an impulse lives until friction or a
wall eats it. Tuned so walking feels as it does now — a ground acceleration
high enough that starting and stopping are nearly immediate.

It is the model, not an arena option. Two movement models is two sets of
prediction bugs, and everything here is about feel that ought to be the same
in every mode. The tuning numbers go in `MovementRules` beside gravity if a map
ever needs them, not before.

A consequence worth deciding on purpose rather than discovering: Quake's air
acceleration, clamped on the wish-direction component of velocity, is exactly
what makes strafe-jumping possible. The air acceleration constant decides how
much of that we get. Strafe-jumping is as iconic as rocket jumping, so the
proposal is to keep the Quake formulation and tune the constant, not to clamp it
away.

### Impulses

**`Impulse` is a component on a body: velocity to add on its next step.** The
explosion writes it — `Push::Outward` at the blast's falloff, the same number
corpses already get — and `step_player` folds it into `Player.velocity` at the
start of the step and clears it. `step_player` stays the one writer of
`Player.velocity`; an explosion writing velocity directly would be a second.

- **An upward impulse leaves the ground.** A body on the floor with an impulse
  carrying it up is treated as having jumped, or the ground snap puts it
  straight back.
- **Friction is off briefly after a knockback**, as Quake's knockback timer
  does. A rocket at your feet on flat ground is otherwise mostly eaten by
  friction before you leave the floor.
- **Self-damage is scaled by `GameRules::self_damage`, knockback is not.** That
  is what makes a rocket jump cost half and fly full. It lives in
  `apply_damage`, beside the self exception.
- **Mass, later.** Δv is the blast's number for every body today. A per-class
  knockback scale (heavier classes are shoved less) goes in `classes.toml` when
  there is more than one class to play.

Hitscan gets no knockback to begin with.

### Over the wire

`Impulse` is authority-only and not replicated: explosions only happen on the
authority, which never rolls back, so consuming it in the step is safe there.
A client predicting its own body learns about the launch through the
replicated `Player.velocity` and rolls back onto it — **a rocket jump starts a
round trip late on a client.** Fine on a LAN. Predicting your own self-knockback
would mean predicting your own rocket, which is a larger piece of work and not
part of this plan.

## Scoring

**A score belongs to the player, not the body** — the rule `Identity` already
follows. Each player gets a replicated **`Standing`** entity on the authority
([`src/game/score.rs`](../src/game/score.rs)): `PlayerId`, team, color, frags,
deaths. Replicated to everybody, because every machine draws the scoreboard.
`keep_standings` keys them on absence, the way bodies are: an identity with no
standing gets one, a standing whose identity has gone (a client who left) goes.
All of them are cleared on **entering** Play, not leaving it, so the final
standings are still in the world afterward — though nothing draws them in the
editor yet.

**`record_frags` runs in `DamageSystems::Resolve`**, after `announce_the_dead`,
reading `Died`:

| Who killed | Killer | Victim |
| --- | --- | --- |
| somebody else | +1 frag | +1 death |
| yourself | — | −1 frag, +1 death |
| nobody (the world) | — | −1 frag, +1 death |

`Died.assist` is recorded for when a mode wants it; arena scores none. In a
team mode this records too — the numbers are true whatever the mode, and what a
mode does with them is its own business.

**The frag limit ends the round.** When a `Standing` reaches
`GameRules::frag_limit`, the authority goes back to the editor through the same
transition F5 uses, so `match_state` carries everybody with it. No results
screen yet.

### The scoreboard

**Bevy UI, top-left, on the HUD camera** — part of the game, so not egui, the
same reasoning as the pause menu. A column of rows ranked by frags, ties broken
by fewer deaths and then by `PlayerId` so the order is the same on every
machine. Each row: a swatch in the player's color, their name, their frags.
Your own row is marked.

Players have no names. A row says "Player {id}" through the lang layer until
there is something better to say. Rows are rebuilt only when a standing
changes, comes or goes.

Up in Play in modes with a frag limit. A Tab scoreboard with deaths and pings,
a kill feed and a results screen are all later.

## Checklist

Mode rules
- [x] `modes.toml` and its `PackTable`, synced on connect
- [x] `GameRules` and `publish_game_rules`; default is today's behavior
- [x] `starting_equipment` reads the mode's loadout
- [x] The body's class is the first the mode allows
- [ ] A client's identity made fresh each match, as the host's is

No teams
- [x] `Identity.team: Option<Team>`, bodies with no `Team`
- [x] `next_team` asks the rules
- [x] `choose_spawn` with no team uses every spawn
- [x] `PlayerColor`: OKLCH, 30° then halving, random start, lowest free index
- [x] `PlayerColor` replicated on bodies and kept by corpses; `body_look` uses it
- [ ] HUD zones in the player's color when there is no team (neutral today)

Weapon pickups
- [ ] `WeaponPickup` feature: serde, save test, show/pick, mirror
- [ ] Panel combo over the catalog
- [ ] Authority stands `WorldWeapon`s up; replicated; drawn from the weapon's model
- [ ] `take_pickups` gives weapons: grant, or top up, or walk past
- [ ] An arena map in the default pack with pickups in it
- [ ] Lands in the new slots once the slots work is in

Armor
- [x] `Armor` component, replicated
- [x] `apply_damage` spends it at `ARMOR_ABSORBS`
- [ ] Max per class, when health is
- [x] Armor pickup gives it; example armor prop
- [x] HUD readout
- [ ] Armor packs on the template map

Knockback
- [ ] Acceleration and friction in `step_player`, tuned to feel as now
- [ ] `Impulse` written by explosions, folded in by the step
- [ ] Upward impulse leaves the ground; friction off briefly after
- [ ] `self_damage` scaling in `apply_damage`
- [ ] A test: a rocket at your feet lifts you higher than a jump

Scoring
- [x] `Standing` entity per player, replicated, cleared on entering Play
- [x] `record_frags` in `Resolve`
- [x] Frag limit ends the round
- [x] Top-left scoreboard in Bevy UI
- [ ] The last round's standings shown in the editor

Later
- [ ] Dropping the held weapon on death
- [ ] Spawn choice away from enemies
- [ ] Over-max armor and mega health that decay
- [ ] Per-class knockback scale
- [ ] Predicting your own rocket jump
- [ ] Tab scoreboard, kill feed, results screen, names
