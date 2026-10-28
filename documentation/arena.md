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
when their identity is made, and every identity is made fresh each match: the
host's by `enter_play` dropping `OurIdentity`, a client's by
`forget_client_identities` taking it off the link entity on entering Play. So
a switch from CTF to Arena takes effect for everybody at the next round.

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

**The HUD is painted in the same order**: `Swatch::of_body` gives the team's
swatch, else one built on the player's own hue (`Swatch::player`, as light as
a team's and as saturated as the hue holds in sRGB), else neutral. The health,
armor and ammo panels and the weapon selector's cards all ask it, so in arena
your HUD is your color.

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
editor, and a typed name is the thing that silently draws nothing. A
feature's panel has no world to read the catalog from, so
`offer_the_catalog` ([`src/tool/weapon_pickup.rs`](../src/tool/weapon_pickup.rs))
puts the names into egui's temporary data each frame and the panel reads them
back — rather than widening `FeatureTrait::editor_ui` for one feature, or
`EditorPanels::ui`, which is at Bevy's parameter limit. That needed the name
on the weapon: `Weapon.key` is what `weapons.toml` calls it, `id` its hash.

The **Weapon** tool places one; it is a point-placement tool like Prop's.

### In the world

The authority stands one up per feature with **`sync_weapon_pickups`**
([`src/game/world_prop.rs`](../src/game/world_prop.rs)), the same rule
`sync_world_props` follows — `StandsFor`/`OfPropFeature` for the bookkeeping,
the instance following its feature and going with it. The instance is an
ordinary **`WorldProp`** naming the weapon's model, plus a replicated
**`GivesWeapon`** and the existing `Pickup`/`Restock`. Reusing `WorldProp`
is what gets it placed, drawn, hidden while taken and stood back up at a
round's edge with no new code; the cost is three filters —
`sync_world_props` and `kit_world_props` leave `GivesWeapon` instances alone,
or the first would despawn them as orphans and the second would strip their
`Pickup` because a held prop is not a pickup archetype.

**Drawn from the weapon's model**, baked through `PropCache` by
`Weapon.model` — `prop::baked` stays the only baker. A weapon the catalog
cannot name stands nothing up until it can. **A weapon hovers with its lowest
point `WEAPON_HOVER` (1 m) above where it stands**, in either style — on the
floor it is lost against the floor. A held prop's origin is its grip, not its
base, so the lift is `RestsOn` — how far the lowest vertex is below the
origin, measured once when it is dressed — plus the hover. `float_pickups`
reads the style off `GivesWeapon`, and the style only decides whether it
turns. The editor's box is drawn at the same height, on a line down to the
point it stands on.

**A beam of light marks it** ([`pickup_beam.rs`](../src/game/pickup_beam.rs)):
a white truncated cone from the floor up, 0.3 m across at the floor to 0.7 m
at 2 m, 10% opaque at the bottom fading to clear at the top, and a spot light
at the floor pointing up into the weapon, reaching 1.5 m. Both drop to 20%
while the pickup restocks, so an empty pad still says where a weapon will be.
They are children of the replicated instance, made on every machine where an
instance has none, so they go with it and the prop editor hides them with it.

**The trigger is a fixed sphere** (`WEAPON_PICKUP_RADIUS`, centered at the
hover height). A held prop has no colliders, and a weapon's pickup volume is a game
rule, not something a modeler should have to author on every gun.

### Taking one

`take_pickups` asks `give_weapon` for a `GivesWeapon` and `give` otherwise.

- **A weapon you do not have** is added to the end of `Equipped::carried`
  with full ammo. There is no limit: the class's loadout slots decide what
  you spawn with, not what you can hold. **Which digit draws it is its
  bucket's business** — the selector lists everything carried by bucket.
  **It is drawn at once**, through the same `draw` a switch goes through, so
  it waits out its deploy and a reload in progress is put down.
- **A weapon you already have** tops up that weapon's reserve by `ammo`, and
  leaves your hands alone.
- **Full on that weapon too** — walked past, untaken, like a
  health pack at full health. A pickup that gave nothing is not spent.

**Losing them on death needs no code**, which is the point of checking it: a
body is a new entity each life, and its `Equipped` and `Ammo` are built by
`starting_equipment` on spawn from `GameRules::loadout`.

Dropping the held weapon on death as a temporary pickup is Quake's, and later.

### Carrying

`Equipped` is a list of carried weapon ids and the one held, and `Ammo` is
keyed by weapon id rather than lined up with that list: the two are separate
components, replicated in separate packets, and a list one longer than the
other would pair every weapon after the gap with the wrong count. A switch is
sent as the weapon's id, never its place in the list, which a pickup landing
between the press and the tick could shift. A loadout `Slot` is now only a
place in a class's definition.

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

**The max is per class**, `armor` beside `health` on a class's entry in
`loadouts.toml`, so the two pools are described in one place; absent is
`DEFAULT_MAX_ARMOR`, 50, which is also what the Mercenary ships with until it
is tuned. `spawn_player` gives a body an empty `Armor::with_max`, and a new
round empties it and keeps the max. The absorbed share is a constant until a
mode wants a different one. Every `Player` requires an `Armor`, and a body
starts with none.

A pickup's share of a maximum is rounded up past float noise (`share` in
`world_prop.rs`): `0.6 * 100.0` is `60.000002` in `f32`, which a plain `ceil`
made 61 — for health and ammo packs too, before armor found it. `PickupKind::Armor`
already exists and already has an amount as a fraction of the taker's max, so
its arm in `give` is a few lines; the default pack ships `armor_pack.gpp` next
to the health and ammo packs. Over-max armor that decays (Quake's red armor) is
later.

The HUD shows it right of the health panel, on a smaller panel of its own
(`HudZone::Armor`) leaning the same way, its number on the health number's
baseline and the word "armor" hanging under it in black; the panel and the number are one
node, hidden together while there is none.

## Knockback

Rocket jumping is the iconic arena move. It needed a movement model with
momentum and a way for a blast to push a living body; both are built.

### Momentum

**Movement is Quake's acceleration and friction, at TF2's numbers**
(`friction`, `accelerate` and `onto_floor` in
[`player.rs`](../src/game/player.rs)). On the ground, friction (4 a second,
acting as if at 2.54 m/s below that) brings velocity down and acceleration
(10 × the wished speed a second) brings it up toward the wished direction and
speed; in the air there is no friction and the keys push at most
`AIR_WISH_SPEED`, 0.76 m/s, along any one direction. Velocity carries between
ticks, so an impulse lives until friction or a wall eats it. A sprint takes
about a tenth of a second to reach and half a second to coast down from; the
top speeds are unchanged.

- **The tick that jumps feels no friction**, so a jump chained on landing
  keeps its speed. There is no bunny-hop cap, as in Quake and unlike TF2.
- **Speed on a slope is kept, not projected away**, as Quake does: walking
  up a 30° floor is as fast as walking on the flat.
- **A landing loses its fall**, including one the ground probe settles that
  the slide never touched; left in, it would turn into speed along the floor.
- **A taunt that stops you lets you coast to a stop** rather than freezing
  you, since freezing would also swallow a knockback.

It is the model, not an arena option. Two movement models is two sets of
prediction bugs, and everything here is about feel that ought to be the same
in every mode. The tuning numbers go in `MovementRules` beside gravity if a map
ever needs them, not before.

Air acceleration is clamped on the wish-direction component of velocity,
which is exactly what makes air strafing possible: holding a strafe key and
turning with the motion keeps adding speed. It is Source's formulation, kept
on purpose; `the_air_keeps_momentum_and_strafing_through_a_turn_adds_to_it`
pins it.

### Impulses

**`Impulse` is a component on a body: velocity to add on its next step.** The
explosion writes it and `step_player` folds it into `Player.velocity` at the
start of the step and clears it. `step_player` stays the one writer of
`Player.velocity`; an explosion writing velocity directly would be a second.

**How hard is TF2's formula** (`knockback` in
[`explosion.rs`](../src/game/explosion.rs)): the push is the damage the blast
deals that body, times 0.254 m/s a point (TF2's ten units), times 82/55 if
the body is crouched, halved if it is your own blast and you are standing on
something, capped at 25.4 m/s (a thousand units). It is aimed from 0.254 m
below the blast at the body's center. Corpses still go by the blast's
`knockback` number, which no longer touches a living body.

- **Your own splash is TF2's too** (`self_splash`): you are caught if any of
  your box is inside the radius, it is measured to the nearer of your box's
  center and your feet, and it never falls below half. That is both what a
  rocket jump costs and how hard it throws. Everybody else's splash still
  falls from their center to nothing at the radius.
- **The push uses the damage before the mode scales it**, so arena's
  `self_damage = 0.5` halves what a jump costs and not how far it flies.
  TF2's Soldier also takes 0.6 of his own damage while airborne; that is a
  class rule, not ported.
- **A blast pushes whoever it may hurt** (`Allegiances::may_hurt`), so a
  teammate's rocket does not shove you and your own does.
- **An impulse carrying a body off its floor faster than `LAUNCH_SPEED`
  (0.25 m/s, Quake's ten units) leaves the ground**, or the walk would lay it
  flat along the floor and the ground snap put the body straight back.
- **Friction is off for `KNOCKBACK_SLIDE` (0.2 s) after a knockback**
  (`Player.sliding`), as Quake's knockback timer does.
- **Self-damage is scaled by `ModeRules::self_damage`, knockback is not.**
  That is what makes a rocket jump cost half and fly full; arena ships 0.5.
  It lives in `apply_damage`, asking `Allegiances::is_self`.

The shipped rocket launcher splashes 90, as TF2's does, so a rocket at your
feet pushes 11.3 m/s standing and 16.8 m/s crouched, against a 7 m/s jump:
about 2.6 and 5.8 times a jump's height. Fired as you jump, airborne, it
pushes 22.6 m/s, and crouched it hits the 25.4 m/s cap. The launcher's
`damage_splash` is the dial, and it is also what an enemy's splash costs.
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
kills, assists, deaths, damage dealt and taken, and healing. Replicated to everybody, because every machine draws the scoreboard.
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

A kill also counts in `kills`, which a suicide never takes from, and
`Died.assist` is an assist to whoever it names, unless that is the victim or
the killer. In a team mode this records too — the numbers are true whatever the mode, and what a
mode does with them is its own business.

**`record_damage`** sits beside it, reading `DamageDealt` before the reaping,
while a hit's target still carries its `PlayerId`: the amount, armor
included, is damage taken by the target and dealt by the source — unless they
are the same person, and never against something with no `PlayerId`, so
shooting crates scores nothing. **Healing** is a `Healed` message, written by
whatever heals (a health pickup today, credited to whoever took it) and read
by `record_healing`.

**Points** are worked out from those counts, never stored or sent:
`Standing::points`, with the weights as constants beside it — 2 a kill, 1 an
assist, −1 a death, 1 per whole 150 healing and per whole 300 damage dealt.
They are ranked as they are, negatives included, and shown never below 0
(`shown_points`), so two players on 0 are still in the right order.
They are a first guess, to be tuned.

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

A row says the player's name, from their settings (see "Player settings"
below), or "Player {id}" through the lang layer for somebody unnamed. Rows are rebuilt only when a standing
changes, comes or goes.

Up in Play in modes with a frag limit.

### Holding Tab

[`src/game/tab_board.rs`](../src/game/tab_board.rs): while Tab is held the
play HUD — every root marked `GameHud`, the top-left board included — is
hidden and a table of everybody comes up in the middle, ranked by points
(then kills, fewer deaths, id): a squircle in the player's color, name,
points, kills, assists, deaths, healing, in JetBrains Mono (`MonoFont`) so
the numbers line up. In every mode, not only those with a frag limit.

**It cannot stick.** `TabBoard::up` is recomputed every frame as "Tab is held
now and the pause menu is not open", never toggled on a press and undone on a
release, since a release can land somewhere else: the pause menu opening over
it, or another window — and Bevy lets go of every key when the window loses
focus. Leaving Play takes it down and puts the HUD back.

### Round history

**The editor's Rounds tab** (bottom panel) lists the last 20 rounds' final
standings, newest first and open, each with its length and every player's
frags and deaths ([`rounds.rs`](../src/game/rounds.rs)). `RoundHistory` is
recorded on every machine as Play ends, from the replicated `Standing`s,
which are cleared only when the next round starts — so a client has them
however the end reached it. A mode with no scoreboard leaves no round. It is
not saved and not sent: a client that joined late has only the rounds it saw.

## Player settings

**What the person at a machine has chosen**, in the map editor's **Player**
tab: a name (anything, no uniqueness, cut to 24 characters) and auto-reload
(on by default). `PlayerSettings`
([`player_settings.rs`](../src/common/player_settings.rs)) is a resource kept
in `assets/user/player.toml` and saved on every change, natively only.
`--profile N` (1–9) reads and writes `assets/user/player-N.toml` instead,
so two copies on one machine can join as two named players.

**It is acted on by the authority.** A client sends its settings on
connecting and on every change (`SettingsChannel`, client to server); the
server keeps them as `ClientSettings` on the link entity, beside the
`Identity`, cleaning the name again since a client is not trusted to.
`settings_of` is the one answer to whose apply to whom: ours for the body
this machine plays, the link's for a client's, the defaults for anybody who
has not said.

- **The name** goes onto the player's `Standing` in `keep_standings`, so the
  scoreboard and the Rounds tab show it, and a rename mid-round shows at once.
- **Auto-reload** is `AutoReload` on the body, copied by
  `apply_player_settings`. With it on, a clip that is short reloads once
  neither button has been pressed or held for `AUTO_RELOAD_AFTER` (one
  second) — `Trigger::idle`, reset by any press, even one that fires nothing,
  and by drawing a weapon. An empty clip reloads either way.

## Checklist

Mode rules
- [x] `modes.toml` and its `PackTable`, synced on connect
- [x] `GameRules` and `publish_game_rules`; default is today's behavior
- [x] `starting_equipment` reads the mode's loadout
- [x] The body's class is the first the mode allows
- [x] A client's identity made fresh each match, as the host's is

No teams
- [x] `Identity.team: Option<Team>`, bodies with no `Team`
- [x] `next_team` asks the rules
- [x] `choose_spawn` with no team uses every spawn
- [x] `PlayerColor`: OKLCH, 30° then halving, random start, lowest free index
- [x] `PlayerColor` replicated on bodies and kept by corpses; `body_look` uses it
- [x] HUD zones in the player's color when there is no team

Weapon pickups
- [x] `WeaponPickup` feature: serde, save test, show/pick, mirror
- [x] Panel combo over the catalog
- [x] Authority stands them up as `WorldProp` + `GivesWeapon`; replicated; drawn from the weapon's model
- [x] `take_pickups` gives weapons: grant, or top up, or walk past
- [x] Pickups on the template map (an arena map of its own is still to make)
- [x] Selected by bucket
- [x] As many weapons carried as are picked up

Armor
- [x] `Armor` component, replicated
- [x] `apply_damage` spends it at `ARMOR_ABSORBS`
- [x] Max per class (`armor` in `loadouts.toml`, 50 for now)
- [x] Armor pickup gives it; example armor prop
- [x] HUD readout
- [x] Armor packs on the template map

Knockback
- [x] Acceleration and friction in `step_player`, at TF2's numbers
- [x] `Impulse` written by explosions, folded in by the step
- [x] Upward impulse leaves the ground; friction off briefly after
- [x] `self_damage` scaling in `apply_damage`
- [x] A test: a rocket at your feet lifts you higher than a jump

Scoring
- [x] `Standing` entity per player, replicated, cleared on entering Play
- [x] `record_frags` in `Resolve`
- [x] Frag limit ends the round
- [x] Top-left scoreboard in Bevy UI
- [x] The last 20 rounds' standings in the editor's Rounds tab

Later
- [ ] Dropping the held weapon on death
- [ ] Spawn choice away from enemies
- [ ] Over-max armor and mega health that decay
- [ ] Per-class knockback scale
- [ ] Predicting your own rocket jump
- [x] Tab scoreboard, ranked by points
- [ ] Kill feed, results screen, pings on the Tab board
