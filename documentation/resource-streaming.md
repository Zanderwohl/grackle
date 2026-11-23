# Resource streaming

Plan for sending a server's content to its clients, Source-style: a client that lacks a file the
match uses, or has a different version of it, is sent the server's copy on join. Every file is
matched by **path and content hash**, never by name alone. The plan also covers where a client
keeps what it was sent, so that foreign content never lands in `assets/default` or `assets/user`.

## Where it starts (November 2026)

Three separate syncs exist, and none of them compares versions:

- [`map_sync`](../src/common/map_sync.rs) sends the feature timeline as JSON, on connect and on
  every change (at most four times a second). It sends nothing the map names: props, materials,
  decals and music are assumed to be on the client already.
- [`pack_sync`](../src/common/pack_sync.rs) sends four merged tables (weapons, class builds,
  taunts, modes) as JSON. It sends the table the server built, not the files, so a client
  rebuilds nothing with its own loader, and rigs and animations the builds name are not sent.
- [`prop::sync`](../src/prop/sync.rs) sends the text of every model the catalog and the placed
  props name, keyed by stem. A client adopts it into `PropCache` in memory and writes nothing.

Materials, decal pictures, rigs, clips, music, sounds and particles are not sent at all. A client
with a different `materials/brick.tgraph` draws its own brick and nothing reports it. Every
reference in the game is a name (`crate`, `rocket_launcher`, `music/Pump/Pump.mp3`) resolved by
"highest pack wins", so two different files with one name are indistinguishable.

## Decisions

### A resource is a path and a hash

A **`ResourceKey`** is a kind, a **logical path** inside a pack (`props/crate.gpp`,
`materials/brick.tgraph`, `music/Pump/Pump.mp3`) and a **BLAKE3-256 hash of the file's bytes**.
Two files with one path and different hashes are different resources, and nothing substitutes one
for the other.

- **BLAKE3, not FNV.** `stable_hash` is fine for cache keys a machine writes for itself. Here the
  hash comes from a stranger's server and decides which stored file a client uses, so it has to
  resist a crafted collision. BLAKE3 is pure Rust, builds for wasm and is already in `Cargo.lock`
  through a dependency. Logs and UI show the first 12 hex digits.
- **The hash is over the bytes on disk**, not over a parsed form. A client stores exactly the
  bytes it was sent and can check them. A cost: a text file with CRLF line endings hashes
  differently from the same file with LF, so two checkouts of the default pack could disagree and
  trigger a pointless download. Add `* text=auto eol=lf` to `.gitattributes` for `assets/`.
- **The logical path is the identity across machines; the pack is not.** The host's
  `assets/user/props/crate.gpp` and the client's `assets/user/props/crate.gpp` have the same
  logical path, and the hash tells them apart. A pack name from the server is a label for the UI
  ("the host's user pack") and never names a folder on the client.

### The server publishes a manifest; the manifest is the session's pack

On connect and whenever anything changes, the server sends a **`Manifest`**: every resource the
match needs, as `ResourceKey`, size, the label of the pack it came from, and its authors (below).
It is the **closure** of what the match uses: the map, every prop, weapon, material, decal,
spectator camera, music track and so on the map names, the weapon files and `loadouts.toml`,
`classes.toml`, every rig and clip those name, `taunts.toml`, `modes.toml`, `layers.toml`, and the
sounds and particles the weapons name. Each kind states what it references (`fn references(bytes)
-> Vec<LogicalPath>`), so the closure is computed by walking files, not by a list that has to be
kept in step with the formats.

On the client the manifest becomes a **session pack**: a virtual pack mounted **above every local
pack** for as long as the client is connected, whose files are the manifest's entries. Resolution
by name then finds the server's version first, through the code that resolves names today. When
the link goes, the session pack is unmounted and the client's own packs are back as they were.

- **Gameplay tables are closed.** The catalog, class builds, taunts and modes are built from the
  session pack's files alone, with the same loaders the server used (`read_pack_weapons` and the
  rest). A weapon file in the client's own user pack is not in the match. This replaces
  `pack_sync`'s JSON tables: one loader, run on files, on both machines.
- **Presentation stays local.** Lang files, UI icons, fonts, menu backgrounds, keybindings and
  settings are not in the manifest. A client reads its own.
- **Live map edits stay a stream.** The map the server is editing changes at frame rate, and
  `map_sync` keeps sending the timeline as it does now; the manifest carries the map's
  *dependencies*, and is re-sent when a dependency is added or a saved file changes.
- **Every round's map is stored.** When a round starts, the server serializes the map as it
  stands, unsaved edits included, into blueprint bytes, hashes them and adds them to the manifest
  as `blueprints/{map}.gmb` with that hash. Every client stores the object, and the session record
  lists it under that round with its start time. A client can then keep the exact map it played
  in round 3, not only the one the host last saved. A round whose map is unchanged from the last
  one has the same hash and adds no object.
- **Clients never send content to a server.** No message carries a resource from client to
  server, and the server registers no receiver for one. A client editing between rounds, when
  that exists, will send edits, not files.

### A client fetches only what it lacks

For each manifest entry the client looks, in order:

1. **Its own packs**, at that logical path, with that hash. A shipped default-pack file the
   server also ships unchanged is never downloaded. A local file is matched by hash wherever it
   is, so the client's own `assets/user` copy is used **only if it is byte-identical**, and is
   never written to.
2. **The foreign store** (below), by hash.
3. **The server**, by asking for the hash.

Hashing every local file on every join is too slow, so a **local hash index** in
`cache/hashes.bin` maps (path, size, modified time) to hash and is updated as files are seen. The
default pack also ships its own manifest of paths and hashes, which the browser build needs anyway:
[`assets.rs`](../src/common/assets.rs) already notes that a browser cannot list a folder.

**The comparison is local.** A client does not tell the server what it has. It reads the
manifest, works out what it lacks on its own, and sends a `Want` naming only those hashes. The
server answers a `Want` only for hashes in the manifest it last sent, so a client cannot use it to
ask for arbitrary files off the host's disk. The server sends each wanted file as chunks on a
`ResourceChannel` (ordered, reliable, with a byte budget per tick so a 5 MB song does not stall
the map stream). A finished file is hashed before it is stored; a mismatch is dropped and logged.

**Each manifest entry states its size in bytes.** The size is not part of the hash. The loading
screen uses it to weight the progress bar per file, and the receiver uses it as a cap: a transfer
that runs past its stated size is ended and the file is refused.

After joining, a changed manifest (the host saved a prop between rounds) is fetched in the
background, with no loading screen. A round start waits on those fetches the way it waits on
bakes in [`round_prep`](../src/tool/round_prep.rs).

### Joining is a loading state

Between the connection starting and the first screen of the server's mode (the map editor or
Play), the client is in **`AppMode::Joining`**. It is never in the Mode menu, like `Onboarding`.
The client leaves it for whatever `MatchMode` the server last stated, and only once everything
below is done. Two things must happen while it is up:

1. **Get the manifest, the map and every dependency** from the server.
2. **Build everything**, so that the first frame of the server's mode does not stall: parse and
   bake every prop, build the weapon catalog, class builds, taunts and modes from the session
   pack's files, bake every material the map names, adopt the map and bake its rooms, light it,
   and decode the postcards.

The progress bar runs from 0 to 100% in three fixed bands:

| Band | Phase | How progress moves within it |
| --- | --- | --- |
| 0–5% | Reading the manifest and comparing it with local files | By entries checked of entries in the manifest. Hashing an unindexed local file is the slow part. |
| 5–80% | Downloading | By bytes received of the total size of wanted files, from the manifest's sizes. With nothing to download it goes straight to 80%. |
| 80–100% | Building | By build steps done, each weighted by kind (a material bake or the sun bake weighs more than parsing a weapon file). |

**The window** is a small egui window, centered, over a background picture:

- "Joining {server name}", from the server's `ServerListing` name (the address if the name is
  blank).
- The progress bar.
- A phase line naming the phase and the asset it is on: "Comparing files (412 of 655)",
  "Downloading props/crate.gpp (2 of 7)", "Baking materials/brick.tgraph", "Lighting the map".
  Every string goes through the lang layer (`joining.*`).
- **Disconnect**, which disconnects as the pause menu's Disconnect does: the server is told, the
  client goes back to the mode it joined from (`JoinedFrom`), the session pack is unmounted, and
  everything held in memory for this server is dropped (the manifest, partial transfers, baked
  props and materials, the adopted map). Objects already verified and written to `downloads/`
  stay there until garbage collection, since they are correct and a rejoin would fetch them
  again.

**The background** is picked at random from every pack's `images/backgrounds/loading/`, a folder
separate from the title screen's, and never the same picture as the previous load. The picker in
[`title_background.rs`](../src/title_background.rs) is reused with the folder as a parameter.

**What the server sends and what the client builds.** Today the server sends postcards
(`postcard_sync`) and nothing derived from the map besides them; lightmaps and the nav graph are
baked by each machine and cached by input hash in `cache/lightmap/` and `cache/nav/`. Under this
plan:

- **Lightmaps are offered by the server** as derived resources, keyed by the sun bake's input
  hash, with the bake's `FORMAT` in the key. A client with a cached bake for that hash uses it; one
  without downloads the server's in the 5–80% band rather than baking in the 80–100% band. The
  client cannot check a derived file without baking it, so it trusts the server for it. That is
  acceptable because a wrong lightmap only draws wrong light, and nothing gameplay-related reads a
  lightmap texel. A client set never to download bakes it itself.
- **The nav graph is not needed on a client.** Bots run only on the authority, so a client neither
  downloads nor bakes one while joining.
- **Postcards** stay as `postcard_sync` sends them, and the join waits for them in the build band.

### Foreign content lives in `downloads/`, outside `assets/`

A new top-level folder, gitignored, beside `cache/` and `logs/`:

```
downloads/
  objects/ab/cdef0123....          one file per hash, named by the hash
  sessions/{server}/{started}.toml a session's manifest: what each path meant, and when
  index.bin                        hash -> size, kind, first and last seen
```

- **Objects are content-addressed and nothing else.** A file is stored under its hash, never
  under its logical path, so a hostile path such as `../../assets/user/props/crate.gpp` never
  becomes a filesystem path. Many iterations of one prop during a playtest are many small objects,
  and none of them shadows anything.
- **A session record is the map from names to hashes** for one connection. It is what lets the
  UI say "crate.gpp, version 7 of 9 seen from Alice's server" and what garbage collection reads.
  `{server}` is the server's listing name made safe as a folder name, plus its address.
- **It is not a pack.** `packs_on_disk` reads only `assets/`, so nothing in `downloads/` appears
  in the Library, the pickers or the File menu's Recent lists, and nothing in it overrides a local
  file outside a session.
- **It is not `cache/`.** `cache/` is "safe to delete, derived from the packs". Downloads are not
  derived from anything on this machine, and a server that has since gone cannot send them again.
  They are deleted on their own schedule.
- **Garbage collection** runs at startup, as the trash and the logs do: an object referenced by no
  session newer than **Settings → Developer → Keep downloads for** days (14 by default) is deleted,
  oldest first, and the folder is held under a size cap (1 GiB by default). Settings also offers
  "Delete all downloads".
- **In a browser** the same layout is an IndexedDB store, behind one `ObjectStore` trait with two
  implementations, as `AssetSource` already is.

Bevy's `AssetServer` also has to read some of these files (pictures, music). The session pack is
registered as a Bevy asset source (`session://props/crate.gpp`), whose reader looks the path up in
the mounted manifest and reads the object. A loader that resolves names asks the pack resolver for
the path, and the resolver returns `session://...` for a session file.

### Keeping foreign content is an explicit copy into a pack

Opening a foreign map or prop in the editor after the session is over is useful, but it must not
leave unsorted files in `assets/user`. So content stays in `downloads/` until the player chooses
**Keep a copy…** (from the map editor while connected, or from a downloads browser later). It asks
for a pack, defaulting to a **new** pack named after the source (`assets/alice-playtest/`), and
copies the map and its whole closure there at their logical paths. The downloads browser lists
each session's rounds, so the player can keep the map as it stood in any one of them; the file is
named `{map}_round{n}.gmb` when it would otherwise replace another round's copy.

- **A kept pack is marked as kept.** It gets a `pack.toml` with `origin = "kept"`, the server,
  the host, the date, and the hash of every file at the time it was kept. The editors open its
  files read-only; saving one is a Save As into a pack of your own. A kept pack can be deleted as a
  folder and nothing else breaks.
- **Kept files are found by hash, not by pack order.** The user pack is last in `default_packs()`,
  so a kept map naming `props/crate.gpp` would find the client's own `crate.gpp` first. That is why
  blueprints carry a lock (below).

### A blueprint locks what it was saved against

A saved `.gmb` gains a `requires` section: each dependency's logical path and hash as it resolved
when the map was saved. It has the same shape as a manifest entry, so a kept map's lock and a
session's manifest are the same data.

- **A map in a pack you author resolves by name**, as now. The lock is written but not enforced,
  because the host iterating on `assets/user/props/crate.gpp` wants the latest version, not the
  one from the map's last save. A dependency whose hash no longer matches is listed in the
  Problems panel as changed since the map was saved, as information.
- **A map in a kept pack, or one sent by a server, resolves by its lock**: for each dependency,
  the file with that hash wherever it is (any pack, the foreign store); only if none has it, the
  file at that path with a Problem saying it differs.
- **What counts as "a pack you author"** is any pack without `origin = "kept"` in its
  `pack.toml`, which includes `default`, `user` and packs made with the Library's New.

### Authors are per resource, not per map

`MapMetadata::authors` covers a map. Props, weapons, materials and pictures have no author field,
and a playtest session brings many of them from different people.

- **Each pack may have a `pack.toml`** with `authors`. The user pack's defaults to the player's
  name, filled in at startup if absent.
- **A manifest entry carries authors**: the file's own if its format has them (a map), else its
  pack's `pack.toml`, else the pack's `attribution.txt` (music and fonts already use one, see
  [`attribution.rs`](../src/common/attribution.rs)), else the host's player name.
- **A kept file keeps them.** Keeping writes a `pack.toml` whose `[files]` table records authors
  and origin per file, so a pack kept from one session can credit five people.
- Formats need not gain author fields of their own. A map's `authors` stays, and a prop that wants
  one can add it later; the manifest prefers a file's own.

### A hostile server is assumed

What [sketch.md](sketch.md) asks of the map encoding applies to every resource:

- **Paths are validated before anything else**: relative, `/`-separated, no `..`, under a folder
  the kind allows (`props/` for a prop), with an extension the kind allows. A manifest with a bad
  path is refused whole.
- **Sizes are bounded**: per kind (a prop 1 MiB, a picture 8 MiB, a track 32 MiB), per manifest
  (4,096 entries) and per session (512 MiB). A chunk past its stated size ends the transfer.
- **Nothing is executable.** Every kind is parsed by its existing loader, which treats bad input
  as a skipped file with a warning, and texture-graphs bake under the bounds
  [surfaces.md](surfaces.md) asks for.
- **A client may refuse downloads** (Settings → General → Download content from servers: Always,
  Ask over 50 MB, Never). With Never, a missing resource falls back to the client's own file at
  that path, or to nothing, and the round plays with a warning.

## Checklist

### Stage 1 — identity

- [x] `ResourceKey`, `LogicalPath` (validated on construction, and when deserialized) and
      `ContentHash` (BLAKE3) in [`src/common/resource/`](../src/common/resource/mod.rs), with a
      direct `blake3` dependency.
- [x] `ResourceKind` per file kind, each with its folder, extensions and size cap. What a file
      names is read by the loader that owns its format (`describe`, `Named`): a blueprint's
      features answer `FeatureTrait::resources_named`, a role brings every material it paints
      with, and every map brings the three slot defaults.
- [x] Local hash index (`HashIndex`, `cache/hashes.bin`, native only), which does not trust an
      entry modified within 2 s of being hashed. `.gitattributes` keeps `assets/` LF, fonts aside.
- [x] `closure(assets, packs, given, roots) -> Closure` (a manifest, what is missing, what was
      refused) and `match_roots(map)`. Every shipped map's closure is whole; the arena's holds its
      weapons, their models, the hat, the icons, the raptor's rig and clips, and the materials.

Left for later stages, noticed in this one:

- **Sounds and particles are not resources yet.** Nothing names a sound file, and particle
  pictures are read by fixed names; both join when a format names them.
- **`ctf_2fort.gmb` is a SQLite blueprint** from before the text format. Nothing reads it, and
  the closure test skips it by name until it is converted or deleted.
- **A map handed in by its bytes** (`given`) is how a round's map, serialized as it stands, will
  join the manifest; the hash index is what the client's comparison will use.

### Stage 2 — the session pack

- [x] `ObjectStore` with `DiskObjects` over `downloads/objects/` (native; the browser's comes
      later). An object is named by its hash and the extension of the path it was sent under
      (`ab/cdef….mp3`), checked when stored and again when read: a damaged file is deleted and
      reads as missing. Written beside and renamed into place.
- [x] `SessionPack` (a manifest over a store) mounted in the `Assets` resource with
      `Assets::mount`/`unmount`, listed as the pack `<server>` above every local pack.
      `Assets` is itself an `AssetSource`, so anything reading through it reads the server's
      files too. The `object://` Bevy asset source (`ObjectSourcePlugin`, added before
      `DefaultPlugins`) reads the store by `object_asset_path`, so a server's track plays;
      `Assets::asset_path` answers for either kind of pack.
- [x] One resolver, `Assets`, in place of `default_packs()` wherever the game reads content:
      `Assets::packs()` (local, then the server's) for props, materials, decals and music;
      `Assets::match_packs()` (the server's alone while mounted) for the gameplay tables and body
      clips, which are closed. The editors, pickers, Library, lang, fonts and menu backgrounds keep
      `default_packs()`, which never lists the server's pack.

Noticed in this stage:

- **Loaders run at startup.** Mounting a session changes what they would read, but nothing reads
  again yet; stage 3 reloads the tables and clears the prop and material caches on mount and
  unmount.
- **The app was not launched** with `ObjectSourcePlugin`; it is covered by the tests only.

### Stage 3 — the wire

- [x] `ManifestMessage` (with sizes and a generation) on connect and on change; `Want`, answered
      only for hashes in the current manifest; `Chunk`s on `ResourceChannel`, 32 KiB each, 128 KiB
      per client per frame. No message carries content from a client
      ([`resource::sync`](../src/common/resource/sync.rs)).
- [x] The manifest is rebuilt when what the live map names changes (read from the features
      without the closure), a prop is saved, the catalog changes, a round starts, the sun bake
      changes or the server starts, at most every 0.25 s. The live map is walked for its
      dependencies and not sent.
- [x] Each round's map serialized as it stands on entering Play, hashed and added to the
      manifest; every client stores it and lists it in `downloads/sessions/{server}/{started}.toml`.
- [x] The sun bake standing on the server offered as `lightmap/{input hash}.bin`; a client puts
      it where its sun bake looks and looks again.
- [x] A client compares by size then hash with its store and its local packs, copies a matching
      local file into the store, fetches the rest, mounts once all are stored, and writes
      `PacksChanged`: the tables, body and named clips, layers and music read again, and the prop
      cache and material library forget their bakes. Not being a client unmounts.
- [x] `pack_sync` and `prop::sync` removed; the client builds its tables from the server's files.
- [x] Real-socket tests: a client whose own `weapons/pistol.toml` differs ends up reading the
      host's, fetches only that file, and its own is untouched; a client keeps the map a round
      started on and records it.

Left for stage 4 or later:

- **A round started while a client is still fetching is not held back.** `round_prep` holds the
  authority's own round for its bakes; holding a client is the joining state's job.
- **The comparison is synchronous**, reading and hashing on the main thread when a manifest
  arrives, and does not use `HashIndex` yet. Stage 4 spreads it over frames for the 0–5% band.
- **A file whose bytes fail the hash is asked for again three times**, then given up on; it
  reads as missing, and nothing tells the player yet.

### Stage 4 — joining

- [x] `AppMode::Joining`, entered in the frame the role becomes a client (before
      `back_to_the_map_editor_when_networked` can move it), left for the mode the server last
      stated (`ServerMode`) once the files are mounted and built. `follow_the_server` records the
      mode instead of switching while joining. Losing the link, or Disconnect, goes back to where
      the client joined from.
- [x] The three bands (`joining::progress`): comparing by entries checked, now spread over frames
      at 8 MiB of local reads each; downloading by bytes received of the wanted files' manifest
      sizes; building by weighted steps (a prop 1, a material 2, rooms 2, light 4). The phase line
      names the file, through `joining.*` strings. The manifest carries the server's name.
- [x] The window ([`joining_screen`](../src/joining_screen.rs)): "Joining {server}" (its address
      while the manifest has not named it), the bar, the phase line and Disconnect.
- [x] Backgrounds from `images/backgrounds/loading/`, never the previous load's picture; the title
      screen's stand in while the folder holds none (no picture ships there yet).
- [x] The build band bakes every prop (`PropCache::ensure`) and material
      (`bake_a_material_for_joining`, once the device exists) the manifest lists, waits for the
      server's map to be adopted, and for the room and sun bakes to settle.

Not done in this stage:

- **No test proves the first frame after joining bakes nothing.** The socket tests run without a
  renderer, so props and materials are passed over there; the test covers the state flow (a
  client shows Joining, mounts, then goes to the server's mode).
- **Postcards are not waited for.** They arrive on their own channel and the HUD shows them when
  they do.
- **The joining screen has not been looked at in a running game.**

### Stage 5 — keeping and locking

- [ ] `pack.toml` (authors, origin, per-file provenance), read at startup.
- [ ] `requires` in the blueprint, written on save; resolution by lock for kept and sent maps.
- [ ] **Keep a copy…** into a new or chosen pack; kept packs open read-only.
- [ ] Downloads GC and the Settings entries.

## Settled questions

- **Every round's map is stored**, unsaved edits included (see "Every round's map is stored").
- **Clients never send content to a server.**
- **Clients do not announce what they have.** They compare locally and ask for what they lack.
