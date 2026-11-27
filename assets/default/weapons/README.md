# Weapons

One file per weapon. Every pack's `weapons/*.toml` is read at startup, lowest
pack first, so a later pack's file of the same name retunes that weapon.

**The file's name is the weapon's key**, without `.toml`. It is hashed into a
`WeaponId` at load and that number is what travels, and it is what a map's
weapon pickup and `loadouts.toml` name the weapon by. Renaming a file renames
the weapon everywhere; two keys that hash alike are caught at load rather than
in a fight. A key is lowercase letters, digits and underscores.

`name_key` is a lang key and not a name. Display text lives in
`lang/<code>.toml` like every other string in the game.

Each of `primary` and `secondary` is one mouse button: what it does, how fast
it answers being held, and what it costs. A button that does nothing says so
with `kind = "none"` rather than being left out. A `semi` cadence fires once
per press and an `automatic` one while held; both fire at most once every
`interval` seconds (absent on `semi` is no limit), and a press inside that is
dropped.

`model` names a prop under `props/`, without its extension: the geometry the
weapon is drawn as, modeled in the prop editor. It is a name rather than a
path because a client is told which prop to build and builds it from its own
pack. Omitted means nobody has modeled this weapon yet.

`icon` names an SVG under `icons/`, without its extension, that stands for the
weapon in the kill feed. Omitted draws the skull.

`bucket` is the digit that selects it, 1 to 9, as in Source. Weapons may share
a bucket: pressing its digit again moves along them, in the order they were
carried. `deploy` is how many seconds after being drawn it may fire; its
`deploy` clip, if the model has one, is stretched to fit.

`magazine.clip` omitted means the weapon has no clip at all: its whole supply
is loaded, each shot comes straight out of the reserve, and it never reloads.

A hitscan action's `trail` is how its shot is drawn, and every one must say:
`"streak"` flies a short white streak down each pellet's path, and `"beam"`
stands a line in the shooter's color over the whole path, fading in over a
quarter second and out over the next second.

The weapon editor (Mode → Weapon) writes these files in place with
`toml_edit`, so comments survive a save.

A `melee` action hits the nearest body it may hurt within `range` meters of
the eye, on the tick it is fired, across five rays spread `width` meters
across at full range (absent is 0.6); a teammate in the way is swung through.
It deals `damage`, three times that on a
crit, and is stopped by walls. `kill_shove`, as on a hitscan, throws a body it
kills away from the swing, level with the floor plus its `lift`. What the swing looks like is the model's own
`recoil` clip on the body and `fire` clip in first person.
