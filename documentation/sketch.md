# Service sketch

> **Status — nothing here is built.** This is the shape the project is aiming
> at, written down so the pieces that *do* exist can be pointed at the right
> place. Today the repo is one binary: a level editor. See
> [CLAUDE.md](../CLAUDE.md) for what is actually in the tree.
>
> Where this document says a thing *is* a certain way, that is a decision
> already made. Where it asks a question, the question is genuinely open —
> those are collected at the end rather than answered inline.

Five processes, plus the client. The dividing line running through all of them
is that **three of them are ours and two of them are not**: the accounts server,
the item server and the matchmaking server are operated by whoever runs the
game, while play servers are anybody's, and the client is on the player's
machine. Everything interesting about the design falls out of that line.

| Process | Owns | Trusts |
| --- | --- | --- |
| Accounts | Identities, credentials, sessions | Nobody |
| Items | Backpacks, crate series, unlock rolls | Accounts, for who a player is |
| Matchmaking | The server list | Play servers, for what they claim to be |
| Play server | One lobby: a map, a gamemode, a match | Accounts, for who joined |
| Client | Rendering, input, prediction | Everything it is told, provisionally |

## Accounts

Identity and auth, and nothing else. Every other service asks it "who is this
connection", and it answers with an account id or refuses.

It should stay the smallest of the five and the least interesting. The reason to
split it out at all rather than folding it into items is that play servers need
to authenticate joiners without being handed anything that would let them touch
a backpack — a play server run by a stranger has to be able to prove *who* is
playing without being able to give them anything.

The shape that makes that work is a token the client gets from Accounts and
hands to the play server, which the play server verifies against Accounts rather
than reading. The play server ends up knowing the account id and nothing else.

## Items

Backpacks, and the crate-unlock roll.

**This one already half-exists.** [`src/unlock/`](../src/unlock/) is the roll:
`CrateSeries` holds weighted `CrateSeriesEntry` entries, each of which can have
its own weighted tables of particle effects and stat trackers, and
`unlock(series) -> Result<Item, UnlockProblem>` does the draw.
[`src/common/item/`](../src/common/item/) is the item model — a `Prototype`
plus optional name, description, stat tracker and particle effect, with
`trade_restriction` / `crafting_restriction` / `destroyed` flags already on it.
[`src/bin/crate_drop.rs`](../src/bin/crate_drop.rs) is a Bevy GUI harness for
watching drops happen.

So the item server is largely that logic with the GUI removed, a database
underneath, and a network boundary in front. Three things have to change on the
way:

- **The catalog is hardcoded.** `PROTOTYPES` and `PARTICLES` are `lazy_static`
  maps built in code, and `init_crate_series()` builds the two series inline.
  Adding an item currently means a recompile and a redeploy of the authoritative
  service. These need to be data the server loads.
- **`Item` is a value, not a record.** It has no id and no owner. A backpack
  needs both, plus whatever a trade or a craft would need to reference an
  instance rather than describe one.
- **The roll has to happen server-side, and only there.** It is the one piece of
  logic in the project where the client having a copy is a problem rather than
  an optimization.

Crates and keys are **awarded by play, never bought** — on a win *or* a loss.
That removes the payment surface entirely, which is a large simplification.

It does not remove the fraud surface: play servers are run by strangers, and a
play server is the thing that knows a match happened, so anyone running one can
mint items by reporting matches that never were. **This is accepted, on
purpose.** It is a casual browser game, and at the scale where item fraud would
matter there is a straightforward answer: stand up official servers, stop
awarding items on unofficial ones, and retroactively relabel everything granted
before the cutoff as *Antique*. That turns a fraud problem into a cosmetic tier
rather than a rollback, and it costs nothing until it is needed. Do not build a
trust tier for play servers before then.

The one part of that plan which is expensive to retrofit is the provenance it
sorts on. **Record where an item came from at grant time** — granting server,
timestamp, match — from the very first item. With those columns the Antique
partition is a query; without them every pre-cutoff item is indistinguishable
from every other, which is precisely the moment the label is wanted. Two or
three columns now, and no other machinery.

Series differ in which items and which effects they can produce, exactly as
`CrateSeriesEntry` already models — series 0 in the current code is plain stock
items, series 1 is the same items with stat trackers and, for the hat,
unusual-style particle effects.

## Matchmaking

A list, and the thing that keeps it honest.

Play servers **report in** — the registration is push, from the server, not a
scan from us. A server that wants to be listed says so and keeps saying so;
one that stops saying so falls off the list. That is the whole protocol in
outline, and it means matchmaking never has to reach *into* a play server, which
matters because play servers are behind other people's NAT and firewalls.

What a server reports is its address, its gamemode, its map, and how full it is.
All four are claims, and the client will discover for itself whether they were
true the moment it connects. The list is advertising, not truth, and the design
should not lean on it for anything else.

### Matchmaking is one of three ways in

The list is the convenient path, not the only one, and deliberately not a
chokepoint:

- **The list.** Servers that opt in, for players who want to browse.
- **Direct connection.** A player with an address connects to it. No listing, no
  matchmaking involvement, nothing to opt into. This has to work on its own —
  it is what makes a private game among friends possible, and it is the fallback
  when the list is down or the game is old enough that nobody is running one.
- **LAN discovery.** A broadcast on the local network so servers turn up without
  anyone typing an address, for playing offline together.

LAN discovery carries a constraint the other two do not: **a browser cannot send
a UDP broadcast**, so it cannot work in the wasm client. It is a native-build
feature. That is worth knowing before it is designed rather than after, and it
is an argument for the native client being a real target rather than a debug
convenience — the same argument the editor already makes.

Direct connection, by contrast, works everywhere, which is why it is the one
that has to be solid.

## Play servers

One lobby each. No server browser inside a server, no map rotation across
lobbies, no queue — a play server is one map, one gamemode, one set of players.
That is a real constraint and it simplifies a lot: the server's state *is* the
match.

Two things make this harder than a normal shooter server:

**The map changes during the match.** Teams edit it between rounds. So the
server is authoritative over map state the way it is authoritative over player
state, and map edits have to replicate — this is not a static level loaded once
at join. The editor's feature model in
[`src/editor/editable.rs`](../src/editor/editable.rs) is already close to the
right shape for that: `FeatureTimeline` is an ordered list of features with
before/after `FeatureDelta` snapshots, which is very nearly a replication log
already. Undo/redo and "apply the edits that happened while I was loading" are
the same problem.

**Maps transfer over the wire**, the way Source does it: a client that does not
have the server's map is sent it on join, rather than being turned away or sent
somewhere to go download it. This falls out of the rest of the design more or
less by force — between-round editing means the map a server is running is one
nobody else has ever seen, so there is no repository it could have come from.

Two mechanisms, one pipe at different granularities:

- **Full transfer at join.** The compiled runtime format, sent whole.
- **Incremental edits during play.** The `FeatureDelta` log described below,
  which is the same content expressed as changes.

A client that joins mid-match needs both — the map as of some point, then the
edits since — which is the same problem the editor's rollback bar already
solves, and a reason to keep the two representations convertible rather than
letting the runtime format drift into something the timeline cannot produce.

Being sent executable-ish content by a stranger's server is a real attack
surface, and Source's history with it is not encouraging. The compiled format
should be designed to be parsed defensively by a client that assumes the sender
is hostile: bounded sizes, no dynamic allocation driven by attacker-controlled
counts, and nothing that reaches the filesystem. This is much easier to get
right while designing the format than to add to it afterwards, and it is a
second argument against shipping raw `.gmb` SQLite over the wire — handing a
stranger's bytes to a C SQLite parser is not a boundary anyone wants to defend.

**The client is a browser.** A wasm client cannot open a UDP socket, which rules
out the transport every shooter of this kind normally uses. WebTransport is the
closest thing (unreliable datagrams over QUIC), WebSockets are the fallback and
are TCP-shaped, with the head-of-line blocking that implies for a twitch game.
Whichever it is, it constrains the netcode design rather than being a detail
bolted on at the end, and it should be decided before much of the runtime is
written.

Gamemodes exist as an enum today —
[`src/common/mode.rs`](../src/common/mode.rs) has Arena, CTF, PL, PLR, KOTH, CP
and SD — and maps carry one in their metadata. Nothing consumes it yet.

## Client

The editor and the game are the same binary, and swapping between them is fast
and seamless because that *is* the between-round gameplay. That constraint is
documented in [CLAUDE.md](../CLAUDE.md) and it is the reason the client cannot
simply be "the game, plus a separate editor tool".

Practically it means the client talks to all four services: Accounts to log in,
Items to show a backpack, Matchmaking to find a game, and a play server to play
one.

## What this does to the repo

Six binaries — editor/game client, accounts, items, matchmaking, play server,
plus the existing `ensure_lang` — argues for a Cargo workspace with the shared
model in library crates, rather than the current single crate with `[[bin]]`
entries. Two specific pressures:

- `src/main.rs` re-declares `mod common; mod editor; mod tool;` instead of using
  the `grackle` library, so the editor binary compiles a second copy of the tree.
  With one extra binary that is a build-time annoyance; with five it is not.
- The item model and crate series have to be shared between the item server and
  the client that displays a backpack, but the *roll* must exist only on the
  server. That is a crate boundary, and it is easier to enforce as one than as a
  convention.

The server binaries also want to be headless. `crate_drop` currently pulls in
`DefaultPlugins` and opens a window to test the unlock logic; the item server
must not.

## Open questions

**Are trading and crafting in scope?** `Item` already carries
`trade_restriction`, `crafting_restriction` and `destroyed`, which suggests yes.
Both are item-server features with meaningful fraud surface, and both are much
easier to design for now than to retrofit.

**What is the transport?** WebTransport versus WebSockets, per the play-server
section. Constrains netcode; should not be deferred.

**Can a play server run without the other three services?** Direct connection
and LAN discovery both imply playing with no matchmaking, but a play server
still authenticates joiners against Accounts and a match still wants to award
items. An offline server presumably does neither — no accounts, no drops — which
is coherent, but it means "who is this player" needs an answer that does not go
through Accounts. Running a play server inside the client's own process is the
same question one step further, and would make testing the game loop possible
without standing up anything.
