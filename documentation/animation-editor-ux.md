# The animation editor's layout, revised

The first cut of the animation editor (`src/anim/`) had every piece of data
reachable, and almost none of it reachable from where you were already
looking. This is the plan for the second cut, and the checklist for it. Tick
items as they land; anything struck out was decided against, with why.

## What was wrong

1. **One row, two selections, two places.** A track was picked in the right
   dock's Track tab; its keys on the bottom strip. The strip's row labels did
   nothing, and picking a key did not pick its track.
2. **Things read together were on sibling tabs.** A key's numbers mean nothing
   without its track's space and unit, but Track and Key were two tabs of one
   dock. The clip's length, clock and looping — which set the ruler — were in
   the left panel, as far from the ruler as the window allows.
3. **The source toggle is global, drawn as local.** It decides what Save,
   Undo and the dirty marker mean, and sat at the top of one tab.
4. **Problems are a tab you open once you already suspect something**, and
   name tracks by index while the strip names them by bone.
5. **Scene was nearly empty, and there was no transport** —
   `TimelineCursor::playing` existed and nothing read it.

## The shape it is going to

**One selection, one inspector, and the strip as the main surface.**

```
┌ File  Mode │ deploy — rocket_launcher.gpp* │ Preview: [Mercenary ▾] ─────────┐
├──────────────┬──────────────────────────────────────────┬────────────────────┤
│ LIBRARY      │                                          │ INSPECTOR          │
│ ▾ Weapon     │            four viewports                │ ▸ Clip   deploy    │
│   deploy  ●  │                                          │ ▾ Track  Weapon    │
│   + fire     │                                          │     space · unit   │
│ ▾ Body       │                                          │ ▾ Key @ 0.000      │
│   idle    ●  │                                          │     axes, ease     │
│   run_fwd ○  │                                          │                    │
├──────────────┴──────────────────────────────────────────┴────────────────────┤
│ ▶ ⏸  0.213 / 0.45 s   Length · Clock · Loops   Snap        [+ Track ▾]      │
│ ● ⚠ Weapon       │──◆──────────────◆───────────────────────────────────────  │
│ ●   Weapon turn  │──◆───────◆──────────────────────────────────────────────  │
└──────────────────────────────────────────────────────────────────────────────┘
```

- **Left — Library.** Replaces the Clips tab *and* the source toggle: one tree
  grouped by where a clip is saved. Picking a clip picks its source. Body
  states all listed, with whether each is data or still a built-in function;
  the names the game asks for (`deploy`) offered as placeholders. Right-click:
  rename, duplicate, delete, revert-to-built-in.
- **Bottom — the strip is the track editor.** A transport row (play, pause,
  time, the clip's length/clock/looping, snap). Track headers are controls:
  click selects the track, a toggle for `enabled`, a badge when a problem
  names that track. Reordering, because order matters for `Compose`.
- **Right — one Inspector**, stacked Clip → Track → Key sections, shown as deep
  as the selection reaches. A key is always shown under its track's space and
  unit. Fields with no home before live here: `note`, `pole`, a key's time.
  Problems shown inline in the section they belong to.
- **Scene goes away**: the figure picker moves onto the menu bar, because
  checking a parametric clip on another build should be one click.

## Rules that fall out of it

- **Picking a key picks its track.** The selection is one thing; the track is
  the key selection's first track whenever there are keys picked, and only
  free-standing when none are.
- **A multi-key edit writes only what changed**, never the first key's whole
  `to` across the others — a selection spanning a rotation and a position
  would otherwise write degrees into meters. Only keys of the same kind
  (angles or places) as the first are written to.
- Display text through `get!` — including track row names and Problems.
- **The UI is not a tutorial.** It shows current state ("No track picked."),
  never how to use it ("Click a track's name…"). Menu items and buttons name
  the action.

## Notes from implementation

- `follow_the_document` used to re-frame the strip whenever `AnimEditor`
  changed — and the panels' mutable borrow marks it changed every frame. It
  now re-frames when *which clip is picked* changes, tracked in a `Local`.
- `K` with no keys picked keys the picked track; with no track picked either,
  every track, as before.
- The Track tab's list of tracks is gone: the strip's headers are the list.
- **Track headers are egui, painted over the Bevy strip's names column.** A
  header is a checkbox, a name, a warning and a context menu — controls, which
  Bevy UI has none of. The strip's own input never reads the names column, so
  the two cannot both answer a click. The Bevy row labels were removed.
- **The transport is egui too**, framed at the top of the reserved bottom
  panel; the Bevy strip is whatever is left under it.
- `AnimPanels::egui_wants_pointer` is now "egui is using the pointer, or a
  non-background layer (menu, combo list, context menu) is under it". It was
  `egui_wants_pointer_input`, which is true over any panel while no button is
  held — including the strip — so the wheel never reached the timeline.
- `Clip::problems` returns `ClipProblem { track, message }`, with messages
  from `[anim.problem]`, so the strip can badge the row a problem is about.
- Playback: `Space` or ▶. A one-shot holds its last frame and stops; play on a
  finished one restarts it; scrubbing stops playback. A stride clip plays at
  one stride a second.
- Library: picking a clip picks its document (`AnimEditor::source`). Body
  states with no clip are listed as built-in with a `+` to animate one;
  right-click a body clip to revert it. Prop clips: rename, duplicate, delete;
  the game's played names (`game::held::DEPLOY`) offered until they exist.
- The Scene tab is gone; the figure picker is on the menu bar, after a label
  naming the clip and the document Save writes.

- **Add track** is a menu with submenus: *Bone* lists every bone of the rig
  and *Reach* every limb, so a track is made already pointing at what it
  drives. Body, Weapon and Weapon turn are plain items. `ChainRef::ALL` is the
  one list of limbs.

- **Check a symbol is really in egui's fonts before using it.** The Library
  first marked rows ● and ○; `emoji-icon-font` maps U+25CF to glyph 0, so ●
  drew as the missing-glyph box while ○ was fine. Having an entry in a font's
  character map is not the same as having a glyph. The markers were dropped; a
  built-in state is told apart by being dimmed and carrying a `+`.

- **Staging the figure** (`prop::figure::FigureStage`, menu bar, animation
  editor only): *Holding* off hides the prop and leaves the hands to the
  pose — a body holding nothing. *Grid Frame* chooses what stays still: the
  **Weapon** (grip at the origin, body hung off it — right for weapon clips)
  or the **Floor** (feet at the origin; the weapon placed off the animated
  chest, then moved by whatever the clip did to it — what a body clip looks
  like in a match). The modeling surface always reads the default.

- **A body clip is previewed through its state's corrections**
  (`ik::correct_for_state`, shared with `finish_pose`): duck, then plant the
  feet. Without them the idle's settle — which lowers the hips — moved the
  whole body, feet and all, and read as the feet floating. The port itself was
  exact: the ported idle and `idle_pose` give the same root offset at the same
  time. `FigureCorrections` carries the state; only the Body source sets it.

## Follow-ups, not in this revision

- Stride clips are scrubbed and played against a seconds ruler.
- A solo control on track headers (switch off every other track).
- The strip does not scroll vertically; headers stop at its bottom edge.
- ~~The walking states are not ported.~~ They are, through foot-timed tracks
  and heading blends — see [animation-blending.md](animation-blending.md).

## Checklist

### Stage 1 — cheap, no new kinds of widget
- [x] Picking a key on the strip picks its track (`AnimEditor::track` follows
      the key selection).
- [x] Clicking a track's row label on the strip picks that track (and clears
      the key selection).
- [x] Track and Key merged into one stacked **Inspector** tab (Clip → Track →
      Key sections, collapsible).
- [x] Clip length / clock / looping moved out of the left panel into the
      Inspector's Clip section (stage 2 moves them onto the transport row).
- [x] Multi-key edits write only the changed axis / ease, and only to keys of
      the same kind.
- [x] Picked track drawn highlighted on the strip.

### Stage 2 — the strip as the track editor
- [x] Transport row on the strip: play/pause (Space), time readout, loop.
      `TimelineCursor::playing` actually advances the playhead.
- [x] Length / clock / looping on the transport row.
- [x] `+ Track` on the strip.
- [x] Track header `enabled` toggle; a disabled track's keys drawn dimmed.
- [x] Track reorder (move up / down) and duplicate, on a right-click menu.
- [x] Problem badge on a track header; Problems carry which track they name.

### Stage 3 — Library and inspector completeness
- [x] Library tree replaces the Clips tab and the source toggle; picking a
      clip picks its source.
- [x] Body states listed with data/built-in status; revert one to built-in.
- [x] Prop: the names the game plays (`deploy`) offered as placeholders.
- [x] Rename and duplicate a clip.
- [x] Inspector: `note`, `pole`, key time as a typed number.
- [x] Menu bar names the document Save will write.
- [x] Figure picker on the menu bar; Scene tab removed.

### Strings
- [x] Track row names through `get!` (no `{:?}`).
- [x] `Clip::problems` text through the lang layer.

### Done means
- [x] `cargo check --all-targets` and `cargo test` pass.
- [x] CLAUDE.md's animation-editor notes (if any) agree with the layout — it has none; this file is the record.
