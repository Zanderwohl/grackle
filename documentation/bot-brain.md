# Bots: the brain

A bot can already get anywhere the nav graph reaches (see [bots.md](bots.md)):
it walks, jumps, crouches through vents and rocket-jumps, and its routes weigh
exposure and danger by a temperament of its own. What it cannot do is decide
where to go or fight anybody. This plan is that part: what a bot notices, what
it wants, and how it shoots. The checklist is at the bottom.

## Where it starts (October 2026)

- `BotIntent` (`src/game/bot.rs`) is the only thing between a brain and a
  body: a `Goal` (a place or a pickup), a point to look at, and buttons.
  `steer_bots` turns it into `Inputs` each tick and follows the route.
- `Roam` stands in for a brain: a random cell, or now and then a random
  pickup.
- `RouteCosts` is already a temperament rolled per seat and kept across lives:
  risk weights, a misjudgment, a shortsightedness.
- The bake gives each cell its exposure, cover, knockback danger, traffic,
  distance to every pickup, and which viewpoints see which
  (`Fields::sightlines`).

## Decisions

### Three layers, each with one writer

| Layer | Runs | Writes |
| --- | --- | --- |
| **Senses** | every tick | `BotSenses`: who it sees, hears and remembers |
| **Think** | a few times a second, and at once on an event | `BotIntent.goal` |
| **Fight** | every tick | `BotIntent.look_at`, `fire`, `alt_fire`, `select`, `crouch` |

`steer_bots` stays as it is and reads the intent. Fighting is not a goal: a
bot backing off toward health while shooting the bot chasing it is the think
layer saying "get health" and the fight layer saying "shoot that". A state
machine would need a fleeing-and-fighting state for that, and then one per
combination.

### Senses: nothing the bot could not have noticed

A bot never reads where an enemy is. It reads what its senses report, and
everything downstream works from that, so it cannot be cheap without that
being visible in one place.

- **Sight is a 90° cone.** An enemy is seen when it is within 45° of where the
  bot is looking (its `Player.yaw` and `pitch`, so turning its head is how it
  looks around), within range, and in line of sight. Vertically the cone is
  wider, about ±60°, since a ledge above is somewhere a person glances.
  Behind it, a bot is blind: what it notices there it notices by sound or by
  being hit.
- **Line of sight is two steps.** The baked rows from the bot's viewpoint to
  the enemy's say quickly whether there is any chance, and a ray from the
  bot's eye to the enemy's head and chest (`CollisionWorld::segment_clear`,
  the test hitscan uses) says whether there is one now. The rows are 1 m
  squares at fixed eye heights; the ray is the truth.
- **Hearing** is the `Effect`s every machine already reads: a shot or a blast
  within earshot gives a position, rough by distance, and no identity. It is
  what makes a bot turn round.
- **Being hurt** gives the direction the damage came from (`Damage::from`),
  as the damage indicator shows a player, and no distance: the bot guesses
  its attacker is 10 m that way, or at the first wall if nearer. A hit from
  somebody already in sight tells it nothing new.
- **Memory**: each enemy noticed is remembered with where and when, and
  forgotten after a few seconds unseen. A bot that loses sight of somebody
  goes to where they were, not to where they are.
- **Reaction time**: what is sensed reaches the think and fight layers after
  a delay, a quarter to 0.45 of a second by temperament. A bot that fires
  the frame an enemy rounds a corner reads as a cheat whatever its aim.

What the bot knows without sensing: the map, the bake, and its own state.
Whether a pickup is in stock is an open question (below).

### Think: utility scoring with commitment

Each candidate goal is scored from the bot's state, the highest wins, and the
current goal gets a **commitment bonus** that fades as it is achieved. A rival
has to beat the current goal by that margin, which is a thermostat's band for
every goal at once: no flip-flopping at 41 health, no threshold per pair of
states.

| Goal | Rises with | Falls with |
| --- | --- | --- |
| Get health | health missing | travel time to an in-stock health pack |
| Get armor | armor missing, health full | travel time |
| Get a weapon or ammo | ammo low on what it holds; a better weapon close | travel time |
| Hunt | an enemy remembered; health and ammo | how long since it was seen |
| Take cover | being seen while hurt | the cover field's distance |
| Hold an ambush | patience; traffic it can watch from a low-exposure cell (the Ambush rating) | being found there |
| Roam | nothing else wanted | |

Every score is a weight from the temperament times a response curve over a
fraction, so tuning is weights and curves, not code. Travel time comes from
the baked distance fields, so "closest health pack" is a field read, not a
search.

**When to think**: a few times a second, plus at once when something happens:
taking damage, first sighting an enemy, the goal's pickup being taken, or
arriving.

### Fight: aim and fire with human limits

- **Target**: the enemy most worth shooting among those seen: nearest, most
  hurt, or most dangerous, by temperament.
- **Aim** turns at a limited rate, with an error that shrinks the longer the
  target is tracked and grows when either moves fast. Projectiles are led;
  splash weapons aim at the floor by the feet, which the feet-sight field
  already says is visible.
- **Fire** only with a clear ray to the target, so a bot does not waste
  rockets into a wall, and only once aim is within the weapon's tolerance.
- **Weapon** by range and situation: the shotgun close, the laser rifle far,
  the rocket launcher when splash will reach.
- **Moving while fighting** is the think layer's goal, plus strafing from the
  fight layer within the route's next link, so a duel is not two statues.
- **Keeping its distance.** A hunt goes to a place at the range its weapon
  is comfortable at with a clear line to the quarry, never onto the quarry,
  and once there the fight layer walks in or backs off to hold that range.
  A route avoids the floor right beside any enemy it knows of.

### What a temperament is worth (November 2026)

The data, method and full tables are in
[bot-study-2026-11.md](bot-study-2026-11.md). In short, 295 rounds at `Varied` skill (148 1v1s, 98 of four bots and 49 of eight, on
the arena and the cloister; 1,080 bots, each with its own temperament) were
scored by kills minus deaths per minute against the round's mean, with every
trait standardized and the round as a fixed effect:

- **Aim error decides most of it**: −0.52 per standard deviation (p < 10⁻⁴),
  more than every other trait together. The best fifth beat the worst fifth by
  0.94 kills minus deaths a minute and placed 0.60 against 0.34 (1 first, 0
  last). The effect falls off between 0.09 and 0.11 rad and grows with the
  round: −0.20 per deviation in a 1v1, −0.73 with eight. It works through
  damage dealt (ρ −0.34) and, less, damage taken.
- Smaller and real: a slower aim settle (−0.21), a slower reaction (−0.13),
  a wider ease (−0.11), a faster turn (+0.11), a farther standoff
  (`comfort`, +0.11), a stronger health drive (+0.11, through health packs
  taken) and more drift (−0.10).
- No effect found: the hunt, ambush, roam, patience, armor and stubbornness
  drives, airblast timing, footwork `ramp`, `stride` and `reversal`, and every
  route weight but self-damage (+0.18 in 1v1s only). A stronger arms drive
  picks up more weapons and hunts less, and places a little lower.
- The cover drive leans negative (−0.08, p 0.06), though an earlier 72-round
  study had more cover going with a high score. `BotSkill` no longer narrows
  cover, roam, patience or airblast timing.
- Time on the pistol tracks deaths per minute (ρ 0.89), since every life
  starts on it, so it measures dying rather than causing it.
- Temperament explains about a tenth of a round's outcome out of sample
  (boosted trees, R² 0.11); the rest is the opponents, the spawns and chance.
- The ten traits `BotSkill` narrowed during the study, averaged with the
  better end up, sort bots cleanly: the top fifth placed 0.61, the bottom
  0.41.

What changed because of it:

- `BotSkill` narrows the traits with an effect (aim error, settle, ease,
  reaction, turn rate, tracking, airblast attempt, `comfort`, `drift` and the
  health drive) and no longer narrows cover, roam, patience or airblast
  timing.
- Every aim range moved one tier worse (error 0.06–0.17 rad, settle
  0.5–1.1 s, tracking 45–80 %, turn rate 3–8 rad/s, reaction 0.25–0.45 s),
  since the study's bots aimed too well to play against. Hard now rolls aim
  error at or just under the 0.09–0.11 rad drop-off.

For the next study:

- Run it again at the new ranges before reading the aim numbers above as
  today's.
- A stronger arms drive cut time on the pistol but did not win more, and
  the hunt drive raised placing overall (+0.04, p 0.009) but lowered kills
  minus deaths with eight bots (−0.19, p 0.009). Both are worth a targeted
  run, varying one drive with the rest fixed.
- The 1 Hz samples smear a running body over 6 m, so place-level questions
  (where fights are won) wait for the 4 Hz tracks in
  [round-recording.md](round-recording.md).
- Run each study with `--server-config` files like the study's, one round per
  launch, so every round rolls new temperaments; the scripts in
  `data/bot-study-2026-11/` read the round files directly.

### Temperament

`BotMind` holds the goal weights, reaction time, aim skill and targeting
preference, rolled per seat beside `RouteCosts` and kept across lives. Cowards,
campers and brawlers come out of one system with different numbers.

## Open questions

- **Pickup stock.** Decided for now: a bot knows whether a pickup is in
  stock. Players learn timers; revisit if bots look clairvoyant arriving at
  respawns.
- **Teamwork.** Nothing here coordinates bots on a team. Spreading out and
  focusing fire are later work.
- **Difficulty.** Decided: `BotSkill` (Easy, Normal, Hard) rolls each skill
  trait from the worse 40 %, the middle or the better 40 % of its range —
  aim error, settle, ease, reaction, turn rate, tracking, airblast attempt,
  footwork `comfort` and `drift`, and the health drive, the traits that went with a high
  score in the study above. Everything else rolls its whole range at every
  skill, and `Varied` rolls every trait across its whole range, for studying
  which traits win.

## Checklist

### Stage 5: senses
- [x] `BotSenses` on the body: seen, heard and remembered enemies, with
      positions and times
- [x] Sight: a 90° horizontal and ±60° vertical cone about the bot's look,
      range, the baked rows as a quick reject and an eye-to-body ray as the
      test
- [x] Hearing from `Effect`, and the direction of damage taken
- [x] Memory that forgets after a few seconds unseen
- [x] Reaction delay by temperament
- [x] Tests: an enemy behind a bot is not seen, one in front behind a wall is
      not seen, one in front in the open is; a shot behind is heard

### Stage 6: think
- [x] `BotMind` with weights rolled per seat (`Drives`)
- [x] Scored goals: health, armor, weapon or ammo, hunt, cover, ambush, roam
- [x] Commitment bonus, and thinking a few times a second plus on events
- [x] `Roam` retired as a goal of its own, kept as the fallback score
- [x] Tests: a hurt bot goes for health; it does not turn back for 1 more
      point; a bot that sees an enemy hunts it

Built as `src/game/bot_think.rs`. Each score is a drive times a need times
`1 / (1 + seconds / 5)` for anything travelled to; travel to pickups is the
bake's distance field, in seconds. A hunt goes to where an enemy was last
believed to be, a heard noise at half worth; cover is the least exposed cell
within 10 m, wanted while hurt and under fire; an ambush is the nearest
Ambush-rated cell, held for the bot's patience and then rested for 30 s. A
goal that cannot be reached is rested for 3 s, and a hunt or a roam that
arrives finds nothing and is rested until the memory is gone. A bot thinks
every quarter second, spread by id, and at once when hurt, when it notices
somebody new, when its route ends, or when its pickup is taken.

### Stage 7: fight
- [x] Target choice by temperament
- [x] Aim with a turn rate and an error that settles; leading projectiles;
      splash at the feet
- [x] Fire only on a clear ray and within tolerance; weapon by range
- [x] Strafing while fighting
- [x] Tests: a bot turns onto a target at its turn rate, holds fire with a
      wall between, and hits a still target at range

Built as `src/game/bot_fight.rs`, after `think` in the tick. A bot shoots
only what its senses have in sight, preferring the nearest, its last
attacker or whoever faces it by temperament (`Aim`, in `BotMind`), and keeps
a target until another is half as wanted again. Its aim (`BotAim`) turns at
3 to 8 rad/s while far and eases the last of the way over 0.06 to 0.16 s,
following 45 to 80 % of the target's motion across the view by itself and
catching up on the rest, so it trails a runner a little. It is off by 0.06 to
0.17 rad when it finds somebody, starting past them along the flick and
settling to a third of that over 0.5 to 1.1 s, the error drifting round at
0.5 rad/s and growing with how fast the target crosses the view. A weapon is worth its damage per second at the range,
hitscan scaled by its ramp-up and falloff and halved by 40 m of it, a projectile by its flight time squared
against 0.8 s, and nothing inside 1.2 times its own blast; it is switched
to only when 1.3 times better than the one held. A rocket is led, lifted for
its fall and put at the feet it can see, and never fired with a wall inside
its blast. While fighting a bot moves in legs (`Footwork`):
a strafe one way or a drift at 15 to 45 % of a strafe while it aims, each about 0.9 to 2 s, strafing 30 to
85 % of them and going back the way it came 40 to 80 % of the time, starting
0.35 s after it finds a target, onto floor the graph has and that does not
hurt — a reversal takes a fifth of a second, so shorter legs read as jitter.
It holds a **standoff**: the furthest the weapon it means to use still does
its `comfort` (55 to 90 %) of its best damage per second from, kept between
3 and 25 m (`comfortable_range`). Every weapon's worth is greatest close in,
so the best range alone walked bots onto their targets. With nothing left on
its route it walks in or backs off along its aim to hold it, at full speed
half the standoff off it and not at all within a fifth of that, onto floor
that does not hurt. A hunt's goal is the place nearest the bot and the
standoff, within 1.5 m or a fifth of it, that has a clear line to where the
quarry stands (`hunt_from`), keeping the place it holds while that still
qualifies; arriving with nobody in sight rests the quarry's spot as well. A
route pays 3 s per second spent beside an enemy it knows of, falling to
nothing at 3 m. **Its legs ease** (`BotStride`, in `steer_bots`): the route's
speed, the strafe and the walk in or out each move toward what is asked over
the footwork's `ramp` (0.12 to 0.3 s from still to a run), the route's speed
easing off over the last 1.5 m to a place, and with the route done or
between plans it carries on the way it was going while it slows. Leaps and
their run-ups are not eased, since they replay the bake's input.
Out of a fight it turns onto each link the way its aim does rather than
snapping onto it. Holding an airblast that reflects, it meets a
hostile projectile it sees coming within its blast radius (`Reflexes`): it
tries for 15 to 85 % of them, after its reaction time, pressing when the
projectile is 60 % of the gust's reach away, early or late by up to 0.04 to
0.35 s — a sharp bot reflects nearly every 20 m/s rocket it tries for, a
sloppy one about three in five, and on average a bot about two in five of
those it sees. It does not turn to meet one. Out of a fight it tops up a
clip less than half full.

### Stage 8: seeing what a bot thinks
- [x] `F8` also draws each bot's view and a line to each enemy it senses:
      solid while seen, dashed when only hurt by them, fading as it forgets
- [x] Every bot's name tag up while `F8` is on, its goal on a second line
- [ ] Noises heard, and a goal's score against the runner-up
