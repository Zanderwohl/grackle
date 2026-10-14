# Blending body animations

The plan, and the checklist, for porting the walking states to clips. Tick
items as they land.

## Why a walk is not one clip

`gait_pose` reads two inputs a clip has no axis for:

- **Speed.** The stance fraction slides from a walk's 0.62 to a run's 0.32,
  which moves *when* each foot lifts and lands — and an arm's swing follows its
  foot. A fixed clip was 33° out sprinting straight.
- **Heading.** A diagonal tips the chest and mixes the swing. Up to 72° out.

Plain pose blending by speed does not fix the first: two arms that reverse at
different moments average to an arm doing neither. Measured on a Mercenary, a
grid of 9 speeds × 5 headings was still 4.7° out at worst.

## The design

**1. A track can be timed by a foot** (`Track::timing`: `Clip`, `LeftFoot`,
`RightFoot`). Its time is that foot's own cycle, normalised so **0–½ is the
foot on the ground and ½–1 is the foot in the air**, whatever the stance
fraction is. An arm keyed this way is right at every speed — measured 0.06° —
because what made the function speed-dependent was only *where* in the stride
the foot changes over, and this timing moves the keys with it. That is the
parametric idea one axis further: a key holds a moment of the footfall, not a
moment of the stride.

- The caller states where each foot is (`Frames::footfalls`), the way it
  already states where the floor is. `ik::clip_pose` computes it from the
  state's gait shape; the editor's figure from the preview inputs.
- With nothing stated, a foot-timed track reads the stride as if the stance
  were one half: left at `t`, right at `t + ½`.
- Only meaningful on the stride clock; `Clip::problems` says so otherwise.

**2. A state can blend variants by heading** — the signed angle from the
state's own direction (`direction_of`) to where the body is actually going.
Measured worst joint between headings: an upright gait is 4.5° out at three
headings (−45°, 0, +45°) and 1.35° at five (adding ±22.5°), so the runs and
pushing-a-wall get five; a crouched gait is 1.5° at three, so it gets three.
Strafes get none — pressing a diagonal is running, not strafing.

- On disk a blended `.gpa` is `[[blend]]` tables, each a clip with a
  `heading` (radians). A file without `blend` is a plain clip, as today.
  Parsed by looking for the key rather than by `#[serde(untagged)]`, so a bad
  file still says what is wrong with it.
- At run time a blend is two neighbouring variants as a two-layer
  `ClipStack`, the upper at the blend weight — `apply_stack` already lerps a
  partial layer. Outside the range, the nearest variant.
- **Speed is not a blend axis**, because footfall timing already answers it.
  A future axis would join the variant's coordinates, not replace this.

**3. Seeding.** Gait states bake one variant per heading at walking pace. A
bone on a side (`.l`, `.r`) is timed by the foot on its side; everything else
by the stride. Keys are chosen by Ramer–Douglas–Peucker over the actual key
times, which for a foot-timed track are not evenly spaced.

**4. The editor.**

- The Library shows a blended state with its variants beneath it, by heading.
  A variant is addressed as `state#index` wherever a clip name goes.
- Right-click a plain state: *Blend by heading* (three copies at −45°, 0,
  +45°). Right-click a variant: duplicate (halfway to the next heading, or
  15° past the last), remove. Right-click a blended state: revert.
- The Inspector's Clip section edits a variant's heading; the Track section
  edits *Timed by* on a stride clip.
- Preview: a variant plays alone, at its own heading. A *Blend* toggle on the
  transport previews the whole state at a chosen heading instead.

## Checklist

- [x] `Track::timing` and `Frames::footfalls`; `apply` reads a foot-timed
      track at its foot's phase.
- [x] `StateAnimation` (plain or blend), parsed by key; `ClipLibrary` and
      `finish_pose` play a blend.
- [x] Seeding: foot timing per side, RDP over real times, a variant per
      heading; `GAIT_SEEDS` folded into `SEEDS` and the files written.
- [x] Fidelity test through the real path, across speeds and headings.
- [x] Editor: `BodyClips` holds `StateAnimation`; `state#i` addressing;
      Library rows; blend/duplicate/remove; heading and timing in the
      Inspector; preview footfalls and the blend toggle.
- [x] Problems: a foot-timed track off the stride clock; variants whose
      tracks differ.
- [x] `cargo check --all-targets`, `cargo test`.

## What landed

- All thirteen states ship as clips; `GAIT_SEEDS` is gone and every seed is
  in `SEEDS`. About thirteen keys a track.
- The fidelity test goes through `ik::animation_pose` — the path a match
  plays by — at speeds 0, 2.8, 3.6, 4.5 and 7.5 leg-lengths a second and
  across each blend's headings: within 0.2° on a variant, 2° between.
- The lurch test also goes through it, at a sprint.
- The figure's preview reads footfalls from the preview's state and inputs, at
  walking pace. A preview speed control is not built; footfall timing makes
  the clip correct at every speed, but the editor only shows one.
