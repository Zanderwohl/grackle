# Logging cleanup

Plan for cleaning up what Grackle logs and where it goes. The audit it was built on, kept out of
the repo, listed every `error!`/`warn!`/`info!`/`debug!`/`println!`/`eprintln!`
under `src/`, with a purpose and a note per row. Line numbers below are from commit `e7dd4c8`.

## Rules

These are the rules the work below applies, and the rules new code should follow.

1. **Once logging is up, everything goes through the log macros**, so it reaches the file in
   `logs/`. `eprintln!` is only for failures before `LogPlugin` has installed the subscriber
   (bad command line, a lang file that will not load, a log file that will not open).
2. **Levels:**
   - `error!`: something the user asked for did not happen (a save, an open, a connect), or a
     bug in our code.
   - `warn!`: content was skipped or defaulted (a pack file that will not parse, a loadout naming
     a missing weapon), something the user typed was refused (a host name that does not resolve, a
     malformed weapon key), a cache write failed, or a system ran without something it needs (the
     egui context).
   - `info!`: milestones and state changes, at most a few per second at the busiest.
   - `debug!`: anything that may repeat per frame or per tick.
3. **A failure names the file and the situation.** `error!("{e}")` alone is not enough; write
   `error!("Could not read {path}: {e}")`.
4. **Nothing logs every frame**, except a missing egui context (see below). A failure that can
   repeat warns once, then stays silent until the condition clears, as the LAN broadcast already
   does with `broadcast_failing`.
5. **Game events are JSON lines with a fixed prefix**, so they can be grepped and parsed apart from
   prose (see "Game events").
6. **One layer logs each failure.** When a function returns an error to a caller that logs it,
   the function does not also log it.

## Settings: level and retention

Two new fields on `Settings` (`assets/user/settings.toml`, machine-wide, read before the app
is built), shown on the **Developer** page of the Settings window, above the icon grid:

- **Log level**: Error, Warn, Info, Debug or Trace. Default **Info**.
- **Keep logs for**: a number of days, 1 to 365. Default **7**.

Both use `#[serde(default)]`, so an existing `settings.toml` keeps working.

**The level applies to both drains, the console and the file, and is read once at startup**
(`LogPlugin.level`). Bevy builds its `EnvFilter` itself with no reload handle, so a change takes
effect at the next launch: while the chosen level differs from the one the process started with
(`log_file::startup_level`), the page says so in yellow under the dropdown. `RUST_LOG`, when set,
still overrides it. `Settings` is inserted before `DefaultPlugins` in `launch::run`, so the file
layer can read the retention days.

**Retention by age replaces the 30-file cap.** `log_file::prune` deletes `grackle_*` files in
`logs/` older than the setting, by the file's modified time. It covers `.log` files and the ECS dumps (below). Splitting
a launch's log at 32 MiB stays. Errors from `prune` are collected and logged with `warn!` from a
`Startup` system, because by then the subscriber exists.

**The file drain records each line's source**: `.with_target(true).with_file(true).with_line_number(true)`
on the file's fmt layer. Then every line in `logs/` says which file and line wrote it, which also
tells the repeated egui-context warnings apart. The console stays as it is.

## Game events

Every game event in the log is one line, `GAME_EVENT {json}`, under the log target
`grackle::event`. Filter with `grep '^.*GAME_EVENT ' logs/*.log | sed 's/.*GAME_EVENT //'`, or by
target with `RUST_LOG`.

- One helper, `telemetry::log_event(&Event)`, writes it: `info!(target: "grackle::event", "GAME_EVENT {}", serde_json::to_string(event))`.
  The JSON is the same `Event` shape the round's `.jsonl` file holds, so one parser reads both.
- `telemetry.rs:498` (`kill {ron}`) becomes `log_event(&Event::Kill(..))`.
- `telemetry.rs:622` (`round {pretty ron}`, many lines) becomes one `log_event` of the
  `RoundRecord` as a JSON line. This needs a variant (`Event::RoundRecord`) or a sibling type; it
  is never written to the `.jsonl`.
- `death.rs:216-217` (`X killed by Y`) is dropped. On the authority it duplicates the kill event.
  On a client, which keeps no telemetry, it is replaced by `log_event` of a small `Event::Death`
  built from `Died`, so a client's log still has its deaths.
- `net_bodies.rs:229, 261, 322` (a body stood up for us, a client, a bot) go to `debug!`: the
  round's `Event::Spawn` already records each spawn.
- Choices of team and class (`team_choice.rs`, `class_choice.rs`) become `Event::Seat` and
  `Event::Class`, from the authority only. The class picker's "You will spawn as the X" stays, as
  the local player's feedback until there are toasts.
- Not events: connection, mode changes, bakes, saves. Those stay prose at `info!`.

## LAN discovery

`lan_discovery.rs:369, 382, 417, 433` log at `debug!` on every frame a socket keeps failing.
Give each its own latch, as `ServerList::broadcast_failing` does:

- `answer_questions` read failure (369) and `read_replies` read failure (433): `warn!` once,
  set a flag, and clear it on the next successful read or `WouldBlock`.
- Reply send failure (382): `warn!` once per address per 60 s at most, through the existing
  `Throttle` or a second one.
- Broadcast send failure (417): drop the `debug!`; the `warn!` at 414 already covers it.
- The bind retry (311-312): the `debug!` on each retry every 5 s is fine to drop as well.

## ECS dump

Remove the `LogECS` logging (`bakes.rs:570-592`). Instead:

- A **Developer** section at the bottom of the Bakes panel, with one button, **Dump ECS**.
- It writes `logs/grackle_ecsdump_{%Y-%m-%d_%H-%M-%S}.txt` (same stamp format as the log file) on
  the IO pool, and logs one `info!` line with the path, or a `warn!` if the write fails.
- The dump covers every entity, not only `FeatureTag` ones: entity, name if any, and its
  component names (from `World::inspect_entity`), with the feature fields the current dump prints
  where present. Being a file, it can be as long as it needs to be.
- Rename `LogECS` to `DumpEcs` and move the button out of the bakes row in `panels.rs:680`.

## Missing egui context

Keep every one of these as `warn!` (they find scheduling-order bugs). Give each the name of its
system, so the console says which system ran without a context:
`perf.rs:57`, `input.rs:177`, `multicam.rs:592, 654`, `panels.rs:350`, `movement.rs:141`,
`anim/timeline.rs:232`, `bin/crate_drop.rs:67`. Wording: `"{system}: no egui context: {error}"`.

## Level changes

**Bad content, `error!` → `warn!`:**
`class_file.rs:66, 77`, `game_rules.rs:156, 167`, `layer_file.rs:55, 66`, `taunt.rs:364, 382`,
`weapon_file.rs:147, 168, 225, 241, 343, 352, 386, 399`, `editable.rs:817`,
`material/library.rs:102, 154`, `prop/sync.rs:205`.

**User input, `error!` → `warn!`:** `net_transport.rs:117` (host name does not resolve),
`weapon_editor/ui.rs:343` (malformed weapon key).

**Failed saves, → `error!`:** `editor/save.rs:183` (`info!`, and it should say which file),
`anim/ui.rs:1918`, `player_settings.rs:209`, `round_settings.rs:428`, `screenshot.rs:82`.
Cache writes stay `warn!`, since nothing the user asked for is lost: `prop/thumbnail.rs:296`,
`tool/postcard_bake.rs:224`, the bake caches.

**Too loud at `info!`, → `debug!`:**
- `map_sync.rs:172` (each map resend, up to 4 per second while dragging). Keep the per-client
  send at 195 as `info!`.
- `map_sync.rs:276` (a client adopting each resend) → `debug!`, except the first adoption of a
  new map (`adoptions` changed), which stays `info!`.
- `prop/thumbnail.rs:295` (one line per thumbnail saved).

## Leftovers to delete

- `editor/multicam.rs:485`: `info!("?")` in a fallthrough arm. Delete the log; leave the arm empty.
- `unlock/series_entry.rs:68-101`: seven `info!` lines tracing each crate roll. Delete, or make
  them `trace!` if the prototype is still being tuned.
- `unlock/series_entry.rs:112`: the lang key `create_drop.error.no_odds` is misspelled
  (`crate_drop`); check which spelling the lang file has.

## Context for vague messages

Each of these logs the error alone. Add what was being read or written, and the path:

- `assets.rs:107`, `bot_names.rs:65`, `class_file.rs:69`, `game_rules.rs:159`, `layer_file.rs:58`,
  `taunt.rs:367`, `weapon_file.rs:336`, `lang.rs:98`, `prop/document.rs:802`, `prop/mod.rs:129`,
  `weapon_editor/ui.rs:755`, `bin/crate_drop.rs:97`.
  Several follow a line that already names the file (`class_file.rs:66` then `:69`); there the
  fix is one line with both, not two.
- `anim/body.rs:392`: names the clip but not the pack path.
- Map save and open, `editor/panels.rs:544, 554, 587, 596` (`Save failed: {e}`, no path) and
  `editor/editable.rs:123, 128`: one message per failure, with the path, logged in one place.
- Prop, lang and weapon editors, where the same failure is logged twice with the same text:
  `prop/ui.rs:3147/3249, 3161/3163`, `lang_editor/ui.rs:716/783`, `weapon_editor/mod.rs:556/563`.
- The "Opening the X editor, as asked" and "Quitting, as asked" lines repeated across the editors:
  name the file being opened, or drop them where the open is logged elsewhere.
- `launch.rs:68`: "Editor Startup Error" is printed by the `grackle` binary too. Use
  `binary.title()` or plain "Startup error".

## Command-line tools

The `println!` calls in `src/bin/*` (29 rows) are each tool's output (`map_stats`' tables and
charts, the generators' "wrote X" lines). These tools build no Bevy app and have no subscriber,
so they keep `println!` for output and `eprintln!` for failures. `crate_drop` does build an app;
its `eprintln!` at line 14 runs before `LogPlugin` and stays.

## Checklist

- [x] `Settings`: `log_level` and `log_days`, serde defaults, Developer page fields, lang strings.
- [x] Insert `Settings` before `DefaultPlugins`; the level read at startup, with the restart note.
- [x] File drain: target, file and line on each line.
- [x] `prune` by age from the setting, covering ECS dumps; its errors logged after startup.
- [x] `log_event` helper and `GAME_EVENT` lines for kills, round records, client deaths, spawns
      and choices; drop `death.rs`' prose kill line and the RON lines.
- [x] LAN discovery latches; no per-frame logging.
- [x] Bakes panel Developer section; `DumpEcs` writes to `logs/`; old `LogECS` logging removed.
- [x] System names on the egui-context warnings.
- [x] Level changes listed above.
- [x] Delete `info!("?")` and the crate-roll tracing; fix the `create_drop` lang key.
- [x] Context and de-duplication for the vague and doubled messages.
- [x] Update CLAUDE.md: the `logs/` row (retention by days, ECS dumps) and a short "Logging"
      section with the rules above.
