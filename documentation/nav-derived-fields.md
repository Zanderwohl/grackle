# Derived nav fields: engagement range, escape time, first weapon

Three fields worked out from what the nav bake already holds — the graph's links and the
sightlines in `nav_field.rs` — rather than from more rays. Each one is an overlay in the Bakes
panel, a section of `map_stats`, and something a bot reads. The fields and their reasons are
below; the general bake is [bots.md](bots.md) and the brain that reads them is
[bot-brain.md](bot-brain.md).

## Where it starts (November 2026)

The bot study ([bot-study-2026-11.md](bot-study-2026-11.md)) found that holding the shotgun
predicts doing well, and that time on the pistol mostly tracks dying. Two things about a map
follow from that and are not measured today: whether a place suits a short or a long weapon, and
how soon a body that spawns with the pistol can hold something better. A third gap is in cover:
`think` sends a threatened bot to the least exposed cell within 10 m, which may be across open
floor, when what it wants is the nearest place out of sight.

## Decisions

### All three are computed in the bake's last pass, from the sightlines

They need no rays: engagement range and escape time read `Sightlines::standing`, and the first
weapon reads the pickup distances and a route. So they are done at the end of
`SightBake::finish` (range, escape) or on demand from the graph (first weapon), stored per cell
like every other field where they are per cell, and cached with the graph. The cache's `FORMAT`
goes up once for the change.

A body sample counts as a sightline (`samples > 0`); a pair seen only at the feet does not, since
a rocket at the feet is splash and not a duel.

### Engagement range: the distances a cell's sightlines run

Per viewpoint, every viewpoint it sees or is seen from, once each, weighted by the floor each
stands for, by distance between their feet. Stored per cell: the median, and the 25th and 75th
percentiles, in meters. A cell that sees and is seen by nothing has all three at 0.

What a bot reads is the median against its weapon's standoff (`comfortable_range`): a place to
hunt from (`hunt_from`) costs a share of the meters between the two, and a route costs more
through exposed cells whose median is far from the standoff, scaled by how exposed the cell is,
so a closet costs a sniper nothing.

`map_stats` and the Bakes panel give the floor in each range band: under 8 m, 8–15 m, 15–25 m
and 25 m or more. The bands are close to the standoffs `comfortable_range` gives the shipped
weapons (shotgun, pistol and rockets, rifle).

### Escape time: seconds to the nearest cell out of every watcher's sight

For a cell, its **watchers** are the viewpoints that see it. A cell is out of their sight when no
watcher sees its viewpoint and it is not itself a watcher's square. Escape time is the walking
seconds over the links (each link's `seconds`) from the cell to the nearest such cell. That is
exact at the viewpoints' resolution, not an approximation: which cells the watchers see is a
union of rows, done as bitsets over the viewpoints (a few dozen words each), and each cell's
search stops at the first cell out of sight or at `ESCAPE_MOST` (4 s), past which the escape
is infinite. Viewers further than `SIGHT_RANGE` were never cast, so they count as not seeing.

The cell it escapes to is stored beside the time, since that is what a bot goes to.

A **death trap** (`Rating::DeathTrap`) is a cell that is open (exposure over 200 m²) with an escape
of 3 s or more. `map_stats` charts escape time and gives its share of the floor; the Bakes panel
draws it as an overlay, short escapes dark. The threshold was 2 s as planned, but Waterworks'
median escape is 2.0 s, so that flagged 17% of its floor. At 3 s, Waterworks has 0.4% and Cloister
13.4%, most of it Cloister's courtyard. Bot rounds already show Cloister as the busier and more
dangerous map of the two.

The bot's cover goes to the escape cell when there is one: a threatened bot heads for the nearest
place none of the floor that sees it can see, rather than the least exposed cell within 10 m,
which stays as the fallback where nothing within `ESCAPE_MOST` breaks the line.

### First weapon: from each spawn to the nearest weapon it does not start with

What a body starts with is the mode's loadout, else the class's defaults
(`WeaponCatalog::starting_equipment`), so a baked weapon pickup now records its `WeaponId`, and the
bake keeps the spawns it was given. For each spawn: the weapon pickup, not one it starts with,
that it reaches soonest (the pickup distances already hold that time), and the exposure along the
route to it — the mean of each cell's exposure weighted by the seconds spent in it, and the most.
It is computed when asked (`first_weapons`), not stored, since it depends on the loadout.

`map_stats` prints a row per spawn and the mean; the Bakes panel lists the same under the
statistics.

A bot carrying only what it spawned with (`Equipped` lists the starting weapons first, so this is
"carries no more weapons than the loadout gave it") wants an uncarried weapon pickup more the
nearer it is: its arms need is raised by `FIRST_WEAPON` times that pickup's nearness.

## Open questions

- Escape time treats every watcher as dangerous. A bot that knows where its enemies are could
  break only their lines, which is nearer; that needs a search per thought rather than a baked
  field.
- Engagement range weights every sightline alike, whether across a busy crossing or an empty
  corner. Weighting by traffic, as watch does, may read better.

## Checklist

### Engagement range
- [x] `range_median`, `range_low`, `range_high` per cell in `Fields`, from the sightlines in
  `SightBake::finish`; `Field::RangeMedian` and friends
- [x] Overlay: color by median range (Range), with its own saturation
- [x] `map_stats` and Bakes panel: floor by range band
- [x] Bots: `hunt_from` and `RouteCosts` weigh the median against the standoff
- [x] Tests: a long hall's cells range further than a closet's; the percentiles are ordered

### Escape time
- [x] `escape` (seconds) and `escape_to` (cell) per cell, from the sightlines' bitsets
- [x] Overlay: Escape
- [x] `Rating::DeathTrap`: open and a long escape
- [x] `map_stats`: escape time summary and chart; death trap share
- [x] Bots: cover goes to the escape cell, the old rule as fallback
- [x] Tests: behind a pillar escapes in no time; the middle of an open hall has a long or no
  escape; the escape cell is out of sight of every watcher

### First weapon
- [x] `Pickup::weapon` records its `WeaponId`; `Fields::spawns` keeps the bake's spawns
- [x] `first_weapons(graph, starting)`: per spawn, the pickup, seconds, mean and most exposure
- [x] `map_stats` per-spawn rows; the Bakes panel lists them
- [x] Bots: the arms need for an uncarried weapon rises with nearness while carrying only the
  starting weapons
- [x] Tests: the nearer of two weapons is picked; a starting weapon is skipped; exposure along a
  route through an open hall is higher than through a corridor

### Wrapping up
- [x] Bump `nav_cache::FORMAT`
- [x] CLAUDE.md's bot and nav paragraphs
- [ ] Optionally: check each against `cache/rounds/`
