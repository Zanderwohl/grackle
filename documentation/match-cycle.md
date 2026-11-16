# The match cycle

A server in Play runs matches back to back: **pre-game**, **live**, **post-game** (humiliation),
**between rounds**, then pre-game again. It no longer drops back to the map editor when a limit runs out; F5 still
does.

## Phases

`MatchPhase` is a Bevy `SubState` of `AppMode::Play` (`src/common/match_phase.rs`), so it exists
only while playing and starts at `PreGame` every time Play is entered. The authority is its one
writer (`src/game/match_cycle.rs`); a client follows what the server states.

| Phase | Bodies | Scoring | Ends when |
| --- | --- | --- | --- |
| `PreGame` | nobody is given one | off | the countdown reaches zero |
| `Live` | everybody seated, as before | on | the frag limit or the time limit |
| `PostGame` | winners only (losers do not respawn) | off | no loser is left alive for 2 s, or the post-game length |
| `BetweenRounds` | nobody is given one; every body goes on entering | off | the between-rounds length |

Entering `PreGame` is what makes a new match: every body, corpse, projectile and standing goes,
and the respawn timers are cleared. Identities are kept: a player keeps their seat and team from
one match to the next. Only entering Play makes everybody a new player.

A match can be restarted in place by setting `PreGame` again, since `NextState::set` runs the
transition for the same state. The server numbers matches so a client can tell a restart from a
restatement.

## Who counts as a player

A **person playing** is a human (the host or a client) whose seat is `Seat::Playing`. Somebody
choosing a team or spectating does not count. Bots never count.

## Bot-only matches

`RoundSettings::bot_only_matches`, off by default, in the Round tab and `--server-config`
(`bot_only_matches`). The countdown runs when a person is playing, or when bot-only matches are
allowed, empty slots are filled with bots, and a bot is seated.

- A match that went live with no person playing is **bot-only**.
- When a person starts playing in a bot-only match, in any phase, the server restarts at a fresh
  `PreGame`, so nobody is dropped into a match that was only bots.
- When the last person stops playing in a match that is not bot-only and bot-only matches are not
  allowed, the server goes back to `PreGame` and waits.

`assets/default/servers/bot_arena.toml` turns it on, since an unattended `--spectate` round has
no person playing.

## Pre-game

- `RoundSettings::pre_game_length`, seconds (default 10).
- The server sends `Countdown { left }` on its own unreliable sequenced channel every tick of
  `PreGame` and `BetweenRounds`; `left` is `None` while it waits for players.
- Every machine draws the map's name (its `display_name`, else the file stem) as a large title a
  third of the way down, and the countdown two thirds of the way down: the seconds rounded to a
  whole number, "Start!" below half a second, and "Waiting for players" while waiting. "Start!"
  stays up for a second into `Live`.
- The view cycles through the spectator cameras the seat may use, five seconds each, for every
  seat. A map with no camera keeps the old views.

## Post-game (humiliation)

- `RoundSettings::post_game_length`, seconds (default 15), an upper bound.
- Winners are the players with the most frags; with teams, every player on the team with the most
  frags. Ties all win. A mode with no scoreboard has no winners and skips the post-game.
- Every body that is not a winner's has `Condition::humiliated` forced on, which already means
  third person, no firing, the `humiliated` clip, and running allowed.
- A bot's drives change for the phase (`Drives::in_the_post_game`): a winner's hunt is 1 and a
  loser's cover is 1, whatever its temperament rolled.
- `record_damage`, `record_frags` and `record_healing` run only in `Live`.
- The phase ends 2 s after the last loser body is gone (the last frag), or at the length.

## Between rounds

- `RoundSettings::between_rounds_length`, seconds (default 15).
- The standings stay until the next `PreGame` clears them, and the Tab scoreboard is up for the
  whole phase without Tab held (`hold_the_board`).
- The view cycles through the spectator cameras as in the pre-game (`ShotClock`, restarted on
  entering either phase).
- "Waiting for next round to start." sits just above the bottom of the window safe zone. No timer
  is shown, but the server sends the countdown as it does in the pre-game.
- This is where editing the map between matches will go.

## Rounds tab, `--rounds`, telemetry

- `RoundHistory` starts a round's clock on entering `Live` and records it on leaving `Live`, so
  pre-game and post-game time is not a round's length, and the time limit counts live time only.
- A match restarted from `Live` (a person joining bots, or everybody leaving) is not a round: it
  is neither recorded nor counted.
- `--rounds N` counts rounds recorded and, after the Nth, leaves Play (so telemetry closes its
  round) and quits, without waiting for its post-game.
- Telemetry still opens a file on entering Play and closes it on leaving, so one file can hold
  several matches. Splitting it per match is left to do.

## Checklist

- [x] Settings: `bot_only_matches`, `pre_game_length`, `post_game_length`, `frag_limit`, in the
  Round tab and `ServerConfig`.
- [x] `MatchPhase` sub-state, server-driven, with the match number.
- [x] Phase and countdown sent to clients.
- [x] No bodies in `PreGame`; losers not respawned in `PostGame`.
- [x] New-match reset on entering `PreGame`.
- [x] Limits end `Live` into `PostGame`, not the editor.
- [x] Bot-only rules and the first-person restart.
- [x] Pre-game title, countdown and camera cycle.
- [x] Humiliation forced on losers; scoring off outside `Live`.
- [x] Post-game ends at the last frag + 2 s, or its length.
- [x] `RoundHistory` and `--rounds` on the `Live` phase.
- [ ] Look at the banner and the camera cycle in a window; only their logic is tested.
- [ ] Play it with a client connected: phase and countdown sync have only a wire-encoding test.
- [ ] Stop a loser's `H` from letting go of humiliation for a frame (`toggle_conditions`).
- [ ] Frag limit and other rules per mode, once a mode other than arena plays; the server's
  `frag_limit` stands in for every mode.
- [ ] Telemetry per match rather than per Play.
- [x] Winning bots hunt at a drive of 1, losing bots take cover at 1.
- [ ] Losing bots flee at full health: a cover's score is `(1 - health) * cover`, so at full
  health a loser still does not take cover.
- [x] A between-rounds phase: the scoreboard over the camera cycle, sent its countdown.
- [ ] Look at the between-rounds scoreboard and line in a window.
- [ ] Map editing between matches, in `BetweenRounds` (the game's design calls for it).
- [ ] A post-game banner naming the winners.
