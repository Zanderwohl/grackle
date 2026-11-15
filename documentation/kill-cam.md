# Kill cam: showing who killed you

When another player or a bot kills you, the view slows the match down, swings out to show your
body falling, glides over to frame your killer, stops time on them, and then hands over to the
spectator cycle until you respawn. A death at your own hand or the map's keeps the orbiting death
cam. This is the plan; the checklist is at the bottom.

## Where it starts (November 2026)

- `apply_damage` writes a `DamageDealt` per hit, `record_damage` keeps a `DamageLog` per body
  (one entry per source, at its latest hit, inside `ASSIST_WINDOW`), and `announce_the_dead`
  writes `Died { killer, assist }`: the killer is the last hit, the assist the one before. `Died`
  is relayed to every machine.
- The death cam (`DeathCam`, `start_the_death_cam`, `follow_our_corpse` in
  [`src/game/player.rs`](../src/game/player.rs)) orbits our corpse under the mouse until
  `decide_the_view` sees our next body.
- Spectating ([`src/game/spectate.rs`](../src/game/spectate.rs)) steps through bodies' eyes,
  spectator cameras and spawns with the mouse buttons, for `Seat::Spectating` only.
- Nothing keeps past frames. `interpolate_bodies` draws between the last two ticks, despawns land
  as they arrive, and on a listen host the drawn state is the simulated state: hitboxes are built
  from a body's `GlobalTransform`.

## Blame

- **The killer is the last hit**, as before. The kill cam runs when that is another player;
  `World` and our own `PlayerId` keep the orbit cam. There is no fall damage.
- **Finishing somebody off is an assist.** `DamageLog::assist` is the most recent other
  *player* inside the window who is not the victim, so a body hurt by somebody and then killed by
  a pit or by its own rocket credits that somebody with an assist (1 point). Before, the victim's
  own hit or the map could take the assist slot and credit nobody.

## Sequence

All times are real seconds from the moment the kill cam starts.

| From | To | Camera | Match time |
| --- | --- | --- | --- |
| 0 | 0.5 | The death cam: from our eye out to third person over our corpse, as now | Slowing from full speed |
| 0.5 | 1.0 | Glides from wherever it was to the killer's framing | Still slowing; stopped at 1.0 |
| 1.0 | 2.0 | Holds on the killer | Stopped |
| 2.0 | respawn | The spectator cycle, starting on the killer's eye | Live |

- **The rate falls linearly** from 1 to 0 over the first second, so the view sees half a second
  of match time and ends half a second behind it.
- **A respawn cuts all of it short**, at any point: the replay ends, history is dropped, and the
  view is our new body's eye.
- **The kill cam starts when the death cam does and `Died` names another player as our killer.**
  On a client `Died` and the despawn arrive in either order, so a `Died` read up to
  `KILL_CAM_GRACE` after the death cam started still starts it, from the death cam's start.

## Framing the killer

- The killer's body is a vertical segment from its feet to its crown (the hull's bottom and top).
  The framing is the point on the line from the killer to our death pivot at the distance where
  that segment fills `FRAME_FILL` of the vertical field of view.
- **Never further away than we were**: if we died closer than that distance, the camera stays at
  our pivot and only turns to the killer.
- Pulled in short of walls by sweeping from the killer's eye toward the framing with
  `CollisionWorld::travel`, so a wall between never blocks the shot.
- Taken from the killer's **replayed** position, at the view time the glide is heading for, so the
  killer is framed where the frozen frame shows them.

## Record and replay

The match is slowed by drawing it late, never by slowing the simulation. Everything the play
camera draws is a mesh entity, and a mesh is drawn where its `GlobalTransform` is, so the replay
records drawn `GlobalTransform`s and puts old ones back for the render.

- **`Replay` exists only while a kill cam runs.** Its absence is the off switch: no system
  records, and `ReplayHistory` is empty. Nothing is kept while alive.
- **Recorded after `TransformSystems::Propagate`, written before
  `VisibilitySystems::CheckVisibility`.** The recorder reads the live `GlobalTransform` and
  `InheritedVisibility` of every mesh that moved or appeared since the replay began; the player
  writes the sample at the view time in its place, so frustum culling and extraction see the
  replayed one.
- **Put back in `First`.** Every overridden `GlobalTransform` gets its live value back before
  anything simulates, so a host's hitboxes, bots and shots never see a replayed body. Nothing
  touches `Transform` or `Pose`.
- **Tweened**: a sample between two frames lerps translation and scale and slerps rotation.
- **A mesh that appeared after the view time is drawn out of sight** (far below the map), so a
  respawned body, a new rocket or a streak waits for the view to reach it.
- **A mesh despawned while the view still has frames of it is drawn by a ghost**: an entity of the
  same mesh, material and render layers, played from the history and despawned once the view
  passes its last frame. Materials known to the ghost: `StandardMaterial` and `LitMaterial`;
  anything else is not ghosted and vanishes early.
- **Recording stops once time has stopped**: only the frame at the frozen view time is needed.
- What is not replayed: UI (the HUD, name tags, damage numbers), gizmos, lights, and material
  changes such as a fading decal. All of it is drawn live.

## Checklist

- [x] `DamageLog::assist` passes over the victim and the map, so finishing somebody off is an
      assist
- [x] Learn our killer from `Died` (`OurKiller`), cleared on respawn and leaving Play
- [x] `KillCam` resource and its phases; started with the death cam when the killer is another
      player
- [x] Killer framing, pulled short of walls; never further than our pivot
- [x] `place_camera` glides to the framing and holds there
- [x] Spectator cycle while dead after the hold, starting on the killer's eye
- [x] `Replay` + `ReplayHistory`: record, play, restore in `First`
- [x] Hide meshes born after the view time; ghost meshes despawned before it
- [x] The death cam's pivot follows the replayed corpse
- [x] Respawn cuts the kill cam short at any phase
- [x] Tests: framing, the rate curve, replay restore leaves live transforms untouched, respawn cut
- [ ] Watched in a round against bots: the swing, the glide, ghosts of rockets, the hand-off
