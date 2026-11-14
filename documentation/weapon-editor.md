# Weapon editor

Today `weapons.toml` is edited only as text, and seeing what an edit does means
restarting, pressing F5 and finding somebody to shoot. This is the plan for an
authoring mode that edits the catalog. Its centerpiece is a **range**: a small
staged fight that loops while you tune, so the effect of a number is on screen
a second after you type it. The plan ends with a checklist.

The prop (geometry, hold, muzzle, viewmodel framing, clips) stays the prop
editor's and the animation editor's. This mode edits what the weapon *does*
and which prop it is drawn as, and links across to the prop for the rest.

## Layout

A new `AppMode::Weapon`, reached from the Mode menu (no shortcut, for the same
reason the prop editor has none). It has its own `DockState`, as the prop
editor does, with three areas:

| Left: **Weapons** | Middle: **Range** | Right: **Weapon** |
| --- | --- | --- |
| Every weapon in every pack, grouped by bucket, with the model's thumbnail and the pack it comes from. New, duplicate, rename and delete. Which classes' loadouts permit it (read-only). | Two views of one staged fight: first person (the real play camera with the viewmodel) and third person (an orbit camera). Under them, a strip of the loop's beats, with play/pause, restart, step to a beat and a speed control. | The config: name, model, bucket, deploy, magazine, then **Primary** and **Secondary**, each a cadence, a cost and an action. The action's kind is a combo, and its fields follow the kind. Problems are listed at the bottom. |

File, Edit and Mode menus come from the shared ones. Ctrl+S saves the catalog
and Ctrl+Z/Ctrl+Y undo (`src/common/shortcuts.rs`, `src/common/edit_menu.rs`).
Revert to Saved is one undo step, as in the prop editor. The UI shows what is;
it does not explain what to click.

## The decisions

### The document is the whole catalog, edited in memory

The editor holds a `WeaponDraft`: every weapon of every pack, each tagged with
the pack file it lives in. **Undo is whole-draft snapshots taken when idle**,
the model the map, the prop editor and the material editor all use. A catalog
is a few kilobytes, so cloning it per step is free, and a gesture (one drag on
a number) is one step.

**Saving is publishing**, as with props (`publish_the_saved_prop`): the saved
draft replaces `WeaponCatalog`. On a listen server, `pack_sync` already resends
the catalog whenever it changes. **While editing, the range plays the draft,
not the published catalog**: entering the mode stashes the published catalog
and puts the draft in its place, and leaving puts the stashed copy back unless
it was saved. That is the same "reversible on the way in" pattern the prop mode
follows when it hides the world. Every firing system reads `WeaponCatalog`, so
the range needs no second code path to show an unsaved edit.

The mode is offered **only when not networked** (`offered_networked` false),
as the prop and animation editors are. That also means the swapped catalog is
never broadcast mid-edit.

### Saving edits the file in place, with `toml_edit`

`weapons.toml` is mostly comments. They are the reasoning behind each weapon's
numbers, and the file's header documents its own format. Re-serializing the
file from serde would throw all of that away, and so would the "animations go
at the bottom of an entry" ordering the header asks for. So the saver opens the
file with `toml_edit` (already in the lock through `toml`; make it a direct
dependency) and edits it in place:

- An edited weapon has its values replaced **inside its existing table**, so
  the comment above it stays, and so does the inline-table style
  (`primary = { cadence = …, action = { … } }`).
- A new weapon is appended at the end of its file, written in the shipped
  style.
- A deleted weapon takes its leading comment with it.
- Numbers are written the shortest way that reads back exactly, as
  `src/prop/nice_f32.rs` does for props: `0.8`, not `0.800000011920929`.

The test that guards this is **`saving_an_untouched_catalog_changes_no_bytes`**
against the shipped file. A second test edits one field and checks that the
diff is one line.

Each weapon is saved to the pack file it came from. A new one goes to the pack
selected in the browser, `assets/user/` by default. Editing a default-pack
weapon writes `assets/default/weapons.toml`, which is intended, as the prop
editor writes the shipped rocket launcher. Nothing here needs a file dialog,
so the `rfd` problem on wasm does not grow.

Note the loader's rule: **a key two packs both define is an override when both
give the same `name_key`**, the later pack winning, and a hash collision (dropped)
when they do not. So the browser offers two things that look alike and are
not: **Duplicate**, which asks for a new key and refuses one any pack already
uses, and **Override in pack**, which copies a weapon into a later pack under the
same key and `name_key`, so it can be retuned there without touching the shipped
file. The draft keeps both entries, and only the later one is drawn in the range.

### A key is an identity, so renaming and deleting say what depends on it

The table key is hashed into the `WeaponId` that travels. Renaming it renames
the weapon everywhere, and a `loadouts.toml` entry, a mode's loadout or a
map's `WeaponPickup` naming the old key silently stops resolving. Rename and
delete therefore list what names the weapon before they act, as the prop
editor's delete lists dependants: every loadout in every pack, and every
`WeaponPickup` in the open map. Rename offers to rewrite the loadouts (with
`toml_edit` again). Blueprints on disk other than the open map are not scanned,
and the dialog says so.

`name_key` is a lang key. The panel shows the key and what it resolves to. A
new weapon gets `weapon.names.<key>`. Writing the display string itself is the
lang editor's job (being built in its own session), and a button that opens it
at that key is a follow-up once it lands.

### The form comes from reflection, so a new field appears without UI code

Every spec type already derives `Reflect`, and `WeaponAction` and `Cadence` are
enums. The config panel is **one generic reflected form**, not a hand-written
panel per action:

- A struct is a row per field. An enum is a combo of its variants, with the
  chosen variant's fields beneath.
- An `Option<T>` (ramp-up, falloff, pellets, kill shove, clip) is a checkbox
  that adds or removes its `T`. Turning one off and on again restores the
  previous value for the rest of the session, as flipping a prop's archetype
  does.
- Switching an action's kind keeps the old kind's values for the session, so
  trying projectile and going back to hitscan loses nothing.
- **Units, ranges and labels are custom reflect attributes on the fields**
  (`#[reflect(@Meters)]`, `#[reflect(@0.0..=1.0)]`), which Bevy supports. That
  puts the unit next to the field it describes, not in a table in the UI that
  can drift. Radians are shown and typed in degrees.
- Labels are lang keys derived from the type and field (`weapon.fields.projectile.gravity_factor`).
  The test **`every_weapon_field_has_a_label`** walks the reflected types, so a
  field added to `ProjectileSpec` fails the test until it has a label, and then
  it is in the editor.

This is what "the whole space of possibilities" means in practice: whatever
serde reads from the file, the form can show, and nothing is missing because
somebody forgot to write a widget. The stubs (`heal_beam`, `invulnerable`,
`scope`) are edited like the rest and marked as having no behavior yet,
using `WeaponAction::is_implemented`.

Two fields get a hand-written widget on top of the generic one, because a
number alone does not show what it means:

- **`DistanceScale` pairs (ramp-up and falloff)** are drawn together as one
  damage-over-distance graph per button. It is the curve bots weigh weapons by
  (`bot_fight::worth`), and it is the same curve the range's damage numbers
  should match.
- **`PelletGrid`** gets a small preview of the pattern at 10 m and at 30 m
  (`PelletGrid::offsets`).

**Problems** reuse the loader's checks rather than restating them. Split
`load_weapons` into parse, validate and insert steps, so the editor runs the
same `validate` on the draft. A bucket outside 1–9, an id collision, or a model
that names no prop gets a red row, and the save goes ahead anyway, as a broken
prop feature does not stop a save either.

### The range is the real game, played by puppets

The middle area does not animate a weapon. **It plays one**: a few real
`Player` bodies on a small stage, driven by a director that writes their
`Inputs`. `steer_bots` already proves that nothing downstream can tell a filled
`Inputs` from a keyboard. Recoil, reloads, the deploy clip, the hold, the
viewmodel's arms, tracers, blasts, afterburn, flinches, damage numbers,
knockback and ragdolls are therefore the match's own systems, not a mock. A
mock would be a second description of the weapon, and the editor would show
something the game does not do.

What that takes:

- **A run condition `AppMode::simulates()`**, true in `Play` and `Weapon`, as
  `authoring` and `shows_the_world` are named questions. The systems the range
  needs swap `in_state(AppMode::Play)` for it: `step_player`, `face_bodies`,
  `place_camera`, `WeaponPlugin`, `DamageSystems`, projectiles, explosions,
  flame, airblast, recoil, reloading, the viewmodel, held weapons, flinch,
  damage numbers and ragdolls. Each `OnExit(Play)` cleanup that also applies
  (`clear_projectiles`, `clear_damage_numbers`, `clear_ragdolls`) runs on
  leaving `Weapon` too.
- **Nothing that seats, scores or respawns is widened**:
  `give_bodies_to_whoever_needs_one`, seats and the team picker, `Standing`,
  rounds, telemetry and bots. Puppets have no `Identity` and are owed nothing.
  The director stands them up and takes them down itself. Test:
  `the_range_seats_nobody_and_scores_nothing`.
- **The map is hidden, not unloaded**, by the prop mode's own rule (anything
  that is not the mode's scene is hidden, with the previous visibility restored
  on the way out). The stage is a floor, a back wall and a side wall,
  built as `Volume`s and handed to `CollisionWorld::rebuild_volumes`.
  `rebuild_collision_when_rooms_change` stays gated to `Play`, so the map's
  rooms never merge into the range. Leaving the mode rebuilds from the map, and
  entering Play rebuilds anyway.
- **Puppets carry teams** (the shooter Red, the rest Red or Blue per beat)
  whatever the map's mode, because a beat like "extinguish a teammate" needs
  allegiance. `Allegiances` reads `Team`, so no rule changes. Puppet
  `PlayerId`s come from a range reserved for the director, so a damage record
  never names a real player.
- **The shooter is `Watched`**: spectating is already "playing without the
  controls", so the first-person view uses the existing `place_camera`
  path, the viewmodel's arms and kicks, and the hiding of the shooter's own
  body. No `LocalPlayer` is marked, so `write_client_inputs` has nothing to
  overwrite.
- **Two cameras render into the middle area's rect** (`Camera::viewport`):
  the play camera with its viewmodel and forward-camera children, and an orbit
  camera reusing `prop::camera`'s orbit and zoom. Each carries its marker
  (`PlayerCamera`, and a new `RangeCamera`), per the one-marker-per-camera
  rule, and only the bottom one clears. `shows_the_viewports` is false for this
  mode, so the four editor viewports stay off.
- **Speed** is `Time<Virtual>`'s relative speed, which the fixed loop follows,
  so 0.25× shows a shotgun's pellets landing without a second simulation rate.

### The loop is beats, and each button contributes its own

A **beat** sets up a situation, has the shooter do one thing, and records what
happened. The loop is the beats in order, repeated. Each beat starts by putting
every puppet back where its setup says, at full health, so each pass is
comparable with the last after an edit.

The beats come from the weapon, not from a script somebody maintains.
`beats_for(button, &Mounted) -> Vec<Beat>` gives each action kind its own,
and the weapon as a whole adds the beats that are about the weapon rather
than a button:

| Source | Beats |
| --- | --- |
| hitscan | shoot an enemy at near, mid and far (the ramp-up and falloff ends, when set); with pellets, mid range only, where the spread shows |
| projectile | shoot an enemy standing in the open at mid range (feet, as `bot_fight::aim_point` aims); one off the side wall when it bounces; one on a strafing enemy, led; a rocket jump when it has knockback |
| flame | flame an enemy until it burns, then stop and let afterburn run out |
| airblast | push an enemy, `push`/`lift` (when non-zero); reflect a rocket an enemy fires at the shooter (`reflect`); put out a burning teammate (`extinguish`, which is implemented) |
| scope, heal beam, invulnerable | one beat marked "no behavior yet": the button is pressed, and the beat shows that nothing happens |
| none | nothing |
| the weapon | draw it from the pistol (deploy); fire until the clip is empty and watch the reload (or one shell at a time when `reload_one_at_a_time`); nothing for a clipless weapon |

For the flamethrower, that loop is: flame the enemy, airblast the enemy,
reflect a rocket, extinguish a teammate, draw, which is the sequence asked
for. Each beat can be switched off in the strip, so a loop can be narrowed to
the one thing being tuned. That is view state: not saved, not undone.

**The director aims, it does not think.** It uses `bot_fight`'s pure helpers
(`aim_point`, `turn_toward`, `would_land`, `arriving`) for exact aim, leads and
feet shots, and none of the bot's senses, temperament or error. A loop that
plays out differently each pass would hide the effect of the edit being made.
The enemy puppets have simple scripted moves (stand, strafe, walk in), and
the rocket shooter in the reflect beat is a puppet firing the rocket launcher,
read from the catalog.

**Every beat logs its outcome** under the strip: each hit's damage and distance,
time to kill, knockback speed, and whether the reflect or the extinguish
happened. That is how you check a falloff edit as numbers rather than by eye,
and it shows ramp-up and falloff on the same rows as the graph in the config
panel.

### The prop and its clips are linked, not edited here

`model` is picked with the prop picker (`PickerKind::Prop`, thumbnails,
filtered to `held`). Under it, the panel lists what the model ships against
what the game will play: `fire`, `alt_fire`, `reload`, `deploy`, `recoil`,
`alt_recoil` and `body_reload`, each "own" or "falls back to the shared clip".
It also says whether the model has a `[muzzle]`. **Open in prop editor** and
**Open in animation editor** switch mode with that prop loaded. Both modes are
local, so the switch is one transition.

## Checklist

### Stage 1: the mode and the document
- [x] `AppMode::Weapon`: in the Mode menu, `is_authoring`, not offered while networked, `shows_the_viewports` false
- [x] Split `weapon_file::load_weapons` into parse (`parse_weapons`, `read_pack_weapons`), validate (`problems`) and insert (`catalog_of`) steps; the loader's behavior is unchanged
- [x] `WeaponEditor` draft (`PackWeapon`s tagged with their pack), snapshot undo when idle, the Edit menu, chords, Revert to Saved
- [x] Stash the published catalog on entry, put the draft in place, restore on exit unless saved
- [x] `toml_edit` saver (`weapon_editor::save`): in place, comments and inline style kept, shortest-decimal numbers; `saving_an_untouched_catalog_changes_no_bytes`, `one_edit_changes_one_line`
- [x] Publish on save
- [x] CLAUDE.md section
- [x] Plain egui panels for now: the list on the left, a handful of fields on the right; the `DockState` and the form are stage 2

### Stage 2: browser and form
- [x] Own `DockState`s: Weapons on the left, Weapon on the right; the middle waits for the range
- [x] Weapons tab: grouped by bucket, thumbnails (`PropPreviews`), pack; new, duplicate (with the any-pack key refusal), override in pack, rename, delete
- [x] Rename and delete list dependants (loadouts in every pack, the open map's pickups through `FeatureTrait::weapon_named`); a rename staged with "in loadouts too" rides in the undo snapshot and rewrites each pack's `loadouts.toml` (`toml_edit`) once the weapons are saved
- [x] Generic reflected form (`weapon_editor::form`): structs, enums as combos, `Option` checkboxes, kind switches that keep the old values for the session (`FormMemory`, per weapon)
- [x] `Unit` attributes on every spec float; degrees for radians; `Default` on every spec so a variant never held starts from something
- [x] `every_weapon_field_has_a_label`, `every_number_states_its_unit`, `every_variant_can_be_switched_to`
- [x] Damage-over-distance graph, pellet pattern preview (`weapon_editor::preview`)
- [x] Problems from `problems`, at the top of the Weapon tab and in orange in the list
- [x] Model picker (the prop picker, offered in this mode too) and the clip/muzzle summary, with the open-in-prop-editor and open-in-animation-editor buttons, held back while the prop editor has another prop unsaved

### Stage 3: the range
- [ ] `AppMode::simulates()`; widen exactly the systems listed above, and their `OnExit` cleanups
- [ ] Stage volumes into `CollisionWorld::rebuild_volumes`; the map hidden by the prop mode's rule; collision rebuilt from the map on exit
- [ ] Puppets: real `Player` bodies, reserved `PlayerId`s, teams, stood up and taken down by the director; `the_range_seats_nobody_and_scores_nothing`
- [ ] Shooter `Watched`; the play camera and an orbit `RangeCamera` in the middle rect, bottom one clearing
- [ ] Beats: `beats_for` per action kind plus the weapon's deploy and reload beats; reset per beat; the director aiming with `bot_fight`'s helpers
- [ ] Strip: beats with on/off toggles, play/pause, restart, step, speed
- [ ] Beat log: hits, distances, time to kill, knockback, reflect and extinguish outcomes
- [ ] `every_action_kind_contributes_a_beat`, so a new `WeaponAction` variant fails until it has one

### Later
- [ ] Open the lang editor at a weapon's `name_key` (after the lang editor lands)
- [ ] Editing `loadouts.toml` in the same mode, as its own tab
- [ ] A beat per stub action once it has behavior (heal a teammate, invulnerability, scope zoom)

## Open questions

- **Whether the draft should stay applied after leaving the mode unsaved.**
  The plan restores the published catalog, so F5 plays what is on disk. The
  other choice is to keep the draft live, so F5 tests the edit in the real
  map, at the cost of the game playing something the files do not say.
  Restoring matches "saving is publishing".
- **Whether the HUD appears in the first-person view.** The plan leaves it out
  and shows clip and reserve in the beat log, because the HUD's camera is
  sized to the window, not a viewport. Constraining it to the rect is
  probably small.
