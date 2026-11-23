# Round recording: what happened, drawn over the map

The nav bake says what each place *could* be like: who can see it, how far a
blast throws you from it, how busy the routes through it are. This plan is
about what each place *was* like across the rounds played on it: where bodies
spent their time, where they died, where kills were made from, and which
places win fights. A mapper reads it as heat over the floor in the editor and
as a replay of any one round. A bot reads it as remembered danger. The
checklist is at the bottom.

## Where it starts (November 2026)

- [`src/game/telemetry.rs`](../src/game/telemetry.rs) writes one JSON Lines
  file per round (a match's live phase) at `cache/rounds/{pack}/{map}/{started}.jsonl`, native only, when
  `launch::run` inserts `RecordRounds`. Events: `round_start`,
  `player` (with a bot's whole temperament), `spawn`, `damage` (target,
  attacker, amount, weapon, how, the hit point), `kill` (both parties'
  feet, health, armor and wants; the weapon; the distance; the life's
  length; every attacker's share), `pickup`, `goal`, `sight` (a bot's first
  sighting after 3 s), `sample` (every body once a second: feet, yaw,
  health, armor, weapon, want) and `round_end` (the scoreboard).
- A sample costs about 400 bytes for four bodies. A five-minute 4-bot round
  is 70 to 210 KB.
- The file names a map by its display name. Two versions of a map, before
  and after an edit, write to the same folder, and nothing in a file says
  which version it was played on.
- A shot that hits nothing is not recorded, and a `damage` has the hit point
  but not where the shot came from (`Damage::from` exists in the game and
  is dropped by telemetry).
- The nav graph has a cell for every square meter of standable floor, on
  every level, and `MapIndex::near` finds a point's cell. The fields
  (`nav_field.rs`) and ratings (`nav_rating.rs`) live on those cells, and
  `nav_overlay.rs` draws any per-cell value over the floor by `Shown`.
- `map_stats` bakes a blueprint headless and prints its statistics and
  top-down charts.
- A 6-hour bot study read these files with a Python script and found
  per-place effects (holding the pistol, picking up health) only as totals
  per bot, because a once-a-second sample smears a running body over 6 m.

## Decisions

### Record at 4 Hz, compactly

Bodies are sampled four times a second, so a running body moves about 1.5 m
between samples, close to a nav cell. The 4 Hz record is a new `track`
event holding only what changes quickly, in integers:

```json
{"event":"track","t":12.25,"b":[[3,1520,0,-2228,1571,125],[4,...]]}
```

Each body is `[id, x cm, y cm, z cm, yaw milliradians, health]`. The full
`sample` stays at 1 Hz, with armor, weapon and want, which change slowly. At
about 30 bytes per body per track, an 8-bot five-minute round adds about
300 KB. Ids, not names, since `player` already names each id.

Reading back: a body's place at any time is the track either side of it,
interpolated. A body that died between two tracks ends at its `kill`.

### Record every shot, and where it came from

A new `shot` event per trigger pull that the authority decides:
the shooter, the weapon, the button, the eye, the aim direction, and how many
pellets or the projectile spec's key. `damage` gains `from`, the point it came
from (`Damage::from`: a muzzle, a blast, a rocket's own position). That
gives:

- accuracy per weapon, per bot and per place (hits over shots, by pellet);
- damage dealt *from* a place, which the kill record alone gives only for
  the killing hit;
- the shot lines a replay draws.

A projectile's hit is matched to its shot by shooter and time. Nothing needs
an id carried through the projectile.

### Stamp the map's version

`round_start` gains `map_hash`: the nav bake's snapshot hash, which already
covers the volumes, props, triggers, spawns, pickups and movement rules,
and names the nav cache file. The folder stays the display name, so a
renamed map is a new folder and an edited one is the same folder with a new
hash. A file also gains `format: 2`. Version 1 files (no tracks, no hash)
still read, with tracks synthesized from the 1 Hz samples and the version
unknown.

### Bin onto nav cells, not a grid

Every heat layer is a value per nav cell. The nav graph separates stacked
floors (the bridge over the cloister's basin), which a top-down grid would
merge. It is also what every baked field is on, so a heat layer can be drawn
by the overlay that already exists and correlated against exposure or
sightlines cell by cell. A point is binned with `MapIndex::near`. A point
more than a cell from any cell (mid-air, mid-leap) is binned to the cell
below it, or dropped if none is within 3 m.

### A fifth bake, from play rather than geometry

The **Play** bake (`src/tool/play_bake.rs`, `BakeKind::Play`) reads the
round files for the open map and builds `PlayFields`: one array per layer,
indexed by `CellId`, plus the kill and shot lines. It reports through
`BakeProgress` like every bake (files read of files found). It is **not** in
`BakeSystems::All`, because a round never needs it, and it is never
automatic: a mapper presses Bake. It reads files with `std::fs`, so it is
authoring-side only, which CLAUDE.md's wasm notes allow. It runs on the
async compute pool from a copy of the graph.

**Which rounds.** By default, rounds whose `map_hash` is this map's current
hash. A checkbox includes rounds on older versions, binned by position. A
cell whose floor moved has no data, and nothing pretends it does. The panel
states how many rounds and player-minutes went in, and how many were left
out for being on another version.

**Filters**, applied before binning: the last N rounds, round size, people or
bots or both, and bot skill. Temperament ranges are left to the Python side.

### Rates, not counts, and shrunk where thin

A layer is a rate per player-minute spent in the cell, never a raw count: a
busy corridor has more deaths because it has more people. With few minutes
in a cell, a rate is noise, so each is shrunk toward the map's average:

```
rate = (events + k × map_rate) / (minutes + k)
```

with `k` about two player-minutes. Then each layer is smoothed over the
graph's own neighbors within 2 m of walking, so smoothing never crosses a
wall. The raw counts are kept beside the rates, and the probe shows both.

### The layers

| Layer | Per cell | From |
| --- | --- | --- |
| Occupancy | share of all player-time spent here | tracks |
| Deaths | deaths here per player-minute here | `kill` victim |
| Kills from | kills made from here per player-minute here | `kill` killer |
| Fights won | kills from here ÷ (kills from + deaths here), shrunk to 0.5 | both |
| Damage taken | per player-minute | `damage` target place |
| Damage dealt from | per player-minute | `damage.from` |
| Accuracy | hits ÷ pellets fired from here | `shot`, `damage` |
| Life expectancy | mean seconds from entering the cell to dying | tracks, `kill` |
| First contact | sightings per player-minute | `sight` |
| Flow | mean velocity, drawn as arrows | consecutive tracks |

Fights won and life expectancy are the two a mapper will look at first: one
says which places win, the other which places kill whoever stands in them.
Life expectancy counts only lives that ended in the round; a life still going
at the round's end is censored and left out of the mean, which biases it a
little toward short lives and is stated in the panel.

### Drawing it

The play layers join `Shown` in `nav_overlay.rs` as `Shown::Play(PlayLayer)`,
drawn with the same per-cell coloring, legend and probe as the baked fields,
in the Bakes panel's Play section and on `F7` in a round. Flow draws an arrow
per cell instead of a color.

**Lines** are their own overlay, since they belong to no cell: each kill as a
segment from the killer's feet to the victim's, colored by weapon, and
optionally each shot as a ray to where it stopped. Filtered by weapon. In a
busy map thousands of lines are one gray smear, so the panel caps them at the
latest 500 and offers a weapon and a cell filter ("kills on this cell").

### Replaying one round

A **Replay** panel in the map editor (named `RoundReplay` in code; the kill
cam already has `Replay`) lists the open map's round files and plays one
back over the map: each body as a capsule in its color, with a tick for its
yaw, a name tag, its health as a bar, shots as rays that fade, and kills as a
flash and a line. A timeline scrubs, with play and pause, 0.25× to 8×, and a
mark at each kill. All of it is gizmos and meshes on `multicam::editor_only()`,
in `AppMode::Editor` only, read from the file. It never touches the
simulation or stands a body up.

Clicking a cell in the Play overlay can jump to the rounds that put heat
there: the kills on it, each a link that opens the replay a few seconds
before.

### Headless

`map_stats --rounds DIR` reads round files against the blueprint it bakes
and prints the play layers as the same top-down charts, plus a table of the
hottest and deadliest places. The Python analysis (brains against outcomes)
stays outside the repo. It reads the same files.

### Bots

Later, and gated on enough data: `RouteCosts` gains a weight on the Deaths
layer, as remembered danger, and the bot's misjudgment applies to it as to
the baked fields. Each bot rolls its own weight, so some remember and some do
not. It reads the bake for the current hash only, and a cell with fewer than
a few player-minutes counts as average. This must never become a bot reading
where an enemy is now: it is history, the same for every bot, and known before
the round starts.

### Checking the static ratings

With fights won and deaths per cell, each `Rating` (Ambush, Vantage, Gauntlet
and the rest) can be checked: do Ambush cells make kills, do Gauntlet cells
see deaths? `map_stats --rounds` prints, per rating, the mean of each play
layer over its cells against the map's mean. A rating that predicts nothing
needs retuning or removing.

## Open questions

- **People.** Every round today is bots. Recording a person's play is the
  same data, but once servers host strangers it is personal data and belongs
  under the provenance and accounts story in
  [sketch.md](sketch.md). For now, only rounds this machine hosts are
  recorded, and files stay local.
- **Retention.** Every file is kept; the bake chooses the window. Revisit if
  `cache/rounds/` grows past a few hundred megabytes.
- **Caching the bake.** Reading a few hundred files of a few hundred KB is
  seconds, so the Play bake is not cached at first. If it grows slow, cache
  it under a hash of the map hash and the file list.

## Checklist

### Stage 1: recording, format 2
- [ ] `track` at 4 Hz: id, feet in cm, yaw in milliradians, health
- [ ] `shot` per authoritative trigger pull; `damage` gains `from`
- [ ] `round_start` gains `map_hash` (the nav snapshot hash) and `format: 2`
- [ ] Tests: a round's track reproduces a body's path within 2 m; a missed
      shot is recorded; two versions of a map write different hashes

### Stage 2: the Play bake and its layers
- [ ] `play_bake.rs`: read the open map's files, bin onto cells, rates
      shrunk and smoothed over the graph, `BakeKind::Play` progress
- [ ] Format 1 files read with tracks synthesized from samples
- [ ] Filters: last N rounds, round size, people or bots, skill, older versions
- [ ] Layers: occupancy, deaths, kills from, fights won, damage taken and
      dealt from, accuracy, life expectancy, first contact, flow
- [ ] `Shown::Play` in `nav_overlay.rs`, with the probe showing rate and count
- [ ] Tests: a hand-written set of rounds puts the deaths on the right cells,
      a thin cell shrinks to the mean, smoothing does not cross a wall

### Stage 3: lines
- [ ] Kill lines colored by weapon, shot rays, capped and filtered

### Stage 4: replay
- [ ] `RoundReplay` panel: list, timeline, play and scrub, bodies, shots, kills
- [ ] From a cell in the Play overlay to the kills on it, each opening the replay

### Stage 5: headless
- [ ] `map_stats --rounds DIR`: the play layers as charts, the hottest and
      deadliest places, and each rating's play layers against the map's

### Stage 6: bots remember
- [ ] A rolled `RouteCosts` weight on the Deaths layer, through misjudgment,
      gated on minutes per cell
