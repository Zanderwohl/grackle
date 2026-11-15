# Bot study, November 2026: temperament against outcome

A record of what 295 unattended bot rounds said about which parts of a bot's
temperament win. The conclusions, and what was changed because of them, are in
[bot-brain.md](bot-brain.md) under "What a temperament is worth". This page
keeps the data and the numbers behind them. The per-bot table, the scripts and
the full report are in [data/bot-study-2026-11/](data/bot-study-2026-11/).

## How it was run

- **Rounds**: 2026-11-15, 08:51 to 14:51, four `editor` processes at once
  (debug build), each round its own launch so every bot rolled a new
  temperament:
  `--map M --server-config C --play --spectate --rounds 1`.
- **Maps**: `arena.gmb` (Foundry) and `cloister.gmb`, alternating.
- **Sizes**: 2, 4 and 8 bots, weighted toward 1v1s and fours as 2, 2, 4, 2,
  4, 8. Every config was `fill_with_bots = true`, `bot_skill = "varied"`,
  `time_limit = 300`; the arena mode's frag limit of 20 ended most 8-bot
  rounds first.
- **Skill**: `Varied`, so every trait rolled across its whole range. The aim
  ranges then were wider and sharper than today's, which were moved one tier
  worse after the study:

  | Trait | Range in the study |
  | --- | --- |
  | Aim error | 0.03–0.14 rad |
  | Aim settle | 0.3–0.9 s |
  | Tracking | 55–90 % |
  | Turn rate | 3.5–9 rad/s |
  | Reaction | 0.20–0.40 s |

- **Game time kept real time** with four rounds at once and the screen
  locked: rounds that ran to the limit report 300.0 s.
- **Failures**: 3 of 299 launches hung at startup in a deadlock between
  `paint_rooms` and the GPU surface gate (fixed the same day; see
  `gpu::submitting`), and were killed at 15 minutes. The 295 analyzed are
  every finished round at `Varied` skill.

| Map | Bots | Rounds | Bot-rounds |
| --- | ---: | ---: | ---: |
| Cloister | 2 | 73 | 146 |
| Cloister | 4 | 49 | 196 |
| Cloister | 8 | 25 | 200 |
| Foundry | 2 | 75 | 150 |
| Foundry | 4 | 49 | 196 |
| Foundry | 8 | 24 | 192 |
| **All** | | **295** | **1,080** |

Every bot-round is a distinct temperament. Kills averaged 0.94 a minute per
bot in 1v1s, 2.30 with four and 3.72 with eight.

## How it was scored

- **Outcome**: kills minus deaths per minute (`kd_rate`), and placing (1 for
  first, 0 for last, ties sharing). Each is compared within its round,
  either centered on the round's mean or with the round as a fixed effect,
  since every bot in a round faces the same opponents.
- **Traits** are standardized (mean 0, standard deviation 1), so a
  coefficient is the change per standard deviation of the trait.
- **Models**: Spearman per trait; ordinary least squares with every trait at
  once, round fixed effects and errors clustered by bot; boosted trees
  scored out of sample by bot, with permutation importance and partial
  dependence; and for 1v1s, the trait difference against who won.

## Results

### All traits at once (OLS, kills minus deaths per minute)

Within-round R² 0.27, n = 1,080. Traits with p < 0.1:

| Trait | Coefficient | SE | p |
| --- | ---: | ---: | ---: |
| Aim error | −0.516 | 0.048 | < 0.0001 |
| Aim settle | −0.208 | 0.046 | < 0.0001 |
| Reaction | −0.131 | 0.043 | 0.0025 |
| Footwork comfort | +0.109 | 0.045 | 0.015 |
| Aim ease | −0.113 | 0.047 | 0.015 |
| Turn rate | +0.112 | 0.048 | 0.018 |
| Health drive | +0.109 | 0.048 | 0.024 |
| Footwork drift | −0.095 | 0.044 | 0.030 |
| Cover drive | −0.079 | 0.042 | 0.062 |
| Tracking | +0.084 | 0.046 | 0.066 |
| Route self-damage | +0.077 | 0.046 | 0.091 |

Nothing else came near: the hunt, ambush, roam, patience, armor, arms and
stubbornness drives, airblast attempt and timing, footwork restlessness,
stride, reversal and ramp, target preference, and the route's exposure,
danger, hurt, shortsightedness and misjudgment weights all had p > 0.12.

### By round size (OLS within each size)

| Trait | 1v1 (148 rounds) | 4 bots (98) | 8 bots (49) |
| --- | ---: | ---: | ---: |
| Aim error | −0.19 (p 0.008) | −0.42 (p < 0.0001) | −0.73 (p < 0.0001) |
| Aim settle | | −0.25 (p < 0.0001) | −0.24 (p 0.006) |
| Reaction | | | −0.26 (p 0.0005) |
| Arms drive | −0.19 (p 0.012) | | |
| Route self-damage | +0.18 (p 0.019) | | |
| Armor drive | −0.16 (p 0.025) | | |
| Turn rate | +0.16 (p 0.039) | | |
| Health drive | | | +0.18 (p 0.021) |
| Hunt drive | | | −0.19 (p 0.009) |

A blank is a trait outside that size's strongest eight. With an interaction
term, aim error's slope is −0.20 per deviation in 1v1s, −0.41 with four
and −0.73 with eight: the more bots, the more aim decides.

### Aim error in fifths

| Aim error (rad) | kd_rate vs. round | Placing | Damage dealt vs. round (/min) |
| --- | ---: | ---: | ---: |
| 0.029–0.053 | +0.44 | 0.60 | +47 |
| 0.053–0.076 | +0.28 | 0.59 | +28 |
| 0.076–0.097 | +0.13 | 0.53 | +5 |
| 0.097–0.117 | −0.34 | 0.44 | −28 |
| 0.117–0.140 | −0.51 | 0.34 | −52 |

The partial dependence (`data/bot-study-2026-11/partial_dependence.png`)
falls most steeply between 0.09 and 0.11 rad.

### Other traits in fifths

| Fifth | Aim settle (s) | kd_rate | Reaction (s) | kd_rate |
| --- | --- | ---: | --- | ---: |
| 1 | 0.30–0.44 | +0.19 | 0.20–0.24 | +0.21 |
| 2 | 0.44–0.55 | +0.13 | 0.24–0.28 | −0.10 |
| 3 | 0.55–0.67 | −0.02 | 0.28–0.32 | +0.12 |
| 4 | 0.67–0.80 | −0.20 | 0.32–0.36 | −0.07 |
| 5 | 0.80–0.90 | −0.10 | 0.36–0.40 | −0.16 |

Reaction's effect is real in eights and noisy in fifths.

### The skill traits together

Averaging the ten traits skill narrowed during the study (reaction, turn rate,
error, settle, tracking, airblast attempt and timing, and the cover, roam and
patience drives), each with its better end up, against kills minus deaths a
minute: ρ 0.19, p 1.5 × 10⁻¹⁰.

| Skill index fifth | kd_rate vs. round | Placing | Kill share (1 = fair) |
| --- | ---: | ---: | ---: |
| Lowest | −0.27 | 0.41 | 0.92 |
| Low | −0.20 | 0.45 | 0.96 |
| Middle | −0.04 | 0.48 | 0.98 |
| High | +0.23 | 0.56 | 1.04 |
| Highest | +0.29 | 0.61 | 1.08 |

### How much temperament explains

Boosted trees scored out of sample by bot explain R² 0.11 of a bot's
round-centered result. Permutation importance: aim error 0.31, aim settle
0.06, round size 0.06, route self-damage 0.05, reaction 0.04, aim ease 0.04,
comfort 0.03; every other trait 0.03 or less. The rest is the opponents, the
spawns and chance.

### 1v1s as paired contests

Of 148 1v1s, 133 were decided. The winner's trait minus the loser's, by
Spearman against who won: aim error −0.21 (p 0.016), arms drive −0.18
(p 0.035), route self-damage +0.18 (p 0.040). Nothing else reached p 0.1.

### What bots did, and what it was worth

Behavior is from the 1 Hz samples (share of time on each goal and holding
each weapon) and the pickups (per minute), centered per round, against kills
minus deaths per minute:

| Behavior | ρ | p |
| --- | ---: | ---: |
| Damage taken per minute | −0.55 | < 0.0001 |
| Damage dealt per minute | +0.53 | < 0.0001 |
| Health packs picked up | +0.37 | < 0.0001 |
| Time holding the shotgun | +0.29 | < 0.0001 |
| Time going for health | +0.25 | < 0.0001 |
| Time holding the pistol | −0.24 | < 0.0001 |
| Ammo packs picked up | +0.13 | < 0.0001 |
| Armor packs picked up | +0.13 | < 0.0001 |
| Flamethrowers picked up | −0.11 | 0.0002 |
| Time going for a weapon | −0.10 | 0.001 |
| Rocket launchers picked up | −0.09 | 0.004 |
| Time going for cover | −0.08 | 0.009 |
| Median kill distance | −0.00 | 0.94 |

Time on the pistol tracks deaths per minute at ρ 0.89 across all bots,
because every life starts on it, so it measures dying more than it causes
it. On average bots held the pistol 39 % of the time, the shotgun 14 %, the
rocket launcher 17 % and the pill launcher 16 %. The laser rifle is only on
the cloister.

Which traits drive which behavior (|ρ| ≥ 0.2):

| Trait | Behavior |
| --- | --- |
| Health drive | goes for health +0.51, health packs +0.40, goes for cover −0.24 |
| Armor drive | goes for armor +0.53, armor packs +0.44 |
| Arms drive | goes for weapons +0.65, shotguns picked up +0.51, pistol time −0.48, hunts −0.36 |
| Hunt drive | hunts +0.59, pistol time +0.29, damage dealt +0.23 |
| Cover drive | goes for cover +0.67 |
| Roam drive | roams +0.25 |
| Aim error | damage dealt −0.34 |

## Caveats

- One kind of opponent: bots only, no people.
- Two maps, both free-for-all, both small and close-range enough that the
  shotgun was the weapon to hold.
- One round per temperament, so each bot's result carries its round's luck;
  the round fixed effects remove the opponents' average, not the spawns.
- The aim ranges have since moved one tier worse, so the absolute numbers for
  aim describe the study's bots, not today's.
- Many traits were tested at once. A p of 0.03 among 30 traits is weak on its
  own; aim error, settle and reaction hold up under any correction.

## Reproducing it

`data/bot-study-2026-11/bots.csv` has one row per bot-round: every
temperament trait (`route.*`, `mind.*`), the round (`file`, `map`,
`players`, `minutes`), the scoreboard (`frags`, `kills`, `deaths`,
`assists`, `damage_dealt`, `damage_taken`), derived outcomes (`kd_rate`,
`damage_rate`, `kill_share`, `placing`, `won`, `dealt_rate`, `taken_rate`,
`kill_distance`), and behavior (`want.*` and `held.*` as shares of time,
`picked.*` per minute).

```bash
uv run documentation/data/bot-study-2026-11/analyze.py documentation/data/bot-study-2026-11/bots.csv /tmp/bot-study
```

writes `report.txt` and the partial-dependence plot. The table is rounded to
four significant digits, so a coefficient can differ from the committed
`report.txt` in its last place. `parse.py` builds the
same table from round files (`uv run parse.py cache/rounds out.csv`); the
round files themselves were not kept in the repo.
