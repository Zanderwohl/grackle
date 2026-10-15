# Bevy systems audit

A pass over every system for query filters that belong in the query, value
checks that want a marker, iteration the ECS could do better, and work redone
every frame. Read-only findings; nothing benchmarked.

The headline: filters and markers are already in good shape. The real cost is
**writes that change nothing** — a `&mut` deref or an unconditional assignment
marks a component or resource changed, which re-propagates transforms, re-sends
replicated components, and wakes anything keyed on `Changed`/`is_changed`.

## Bugs

- [x] **B1** `src/common/map_sync.rs` `broadcast_map_changes` — `is_changed()`
  is relative to the system's last run, and it runs every frame including
  inside `RESEND_INTERVAL`. An edit landing inside the window has aged out by
  the time the window closes: the last frame of a drag can go unsent. Fix:
  latch it in a `Local<bool>`.
- [x] **B2** `src/common/net_events.rs` `replicate_corpses` /
  `replicate_projectiles` / `replicate_display_bodies` — the `Listen` check is
  an early return, so `Added<_>` keeps advancing while Solo; anything spawned
  before picking Host (the grid's bodies) is never replicated. Fix: make it a
  `run_if`.
- [x] **B3** `src/game/skeleton.rs` `advance_animation_clock`, `advance_gaits`;
  `src/game/ragdoll.rs` `step_ragdolls` (and `retire_ragdolls`) — in
  `FixedUpdate`, which Lightyear re-runs on rollback, with none of their state
  rolled back. A client re-advances the clock, strides and corpses on every
  replayed tick. Fix: `run_if(not(is_in_rollback))`.
- [x] **B4** `src/game/skeleton.rs` `phase_bodies_by_id` — keyed on
  `Added<PlayerId>` with `With<Skeleton>`; a `PlayerId` that arrives before the
  rig is never phased. Fix: `Or<(Added<PlayerId>, Added<Skeleton>)>`.
- [x] **B5** `src/tool/movement.rs` `MovementPlugin::handle` — scroll messages
  are not drained while not dragging, so they land on the first drag frame.

## HIGH

- [x] **H1** `src/editor/panels.rs` `EditorPanels::ui` — `&mut *editor_features`
  marked `FeatureTimeline` changed every frame. `checkpoint_when_idle`
  serialized the whole map every idle frame, and `broadcast_map_changes`'
  pre-filter never filtered. Done: the viewer holds it through
  `bypass_change_detection()`, `FeatureTimeline::ui` returns whether it
  edited, selected, deleted or rolled back, and the panel `set_changed()`s on
  that. Guarded by `the_history_panel_left_alone_changes_nothing`.
  - [ ] Same pattern, still open and cheaper since nothing reads their change
    ticks today: `map_metadata` (also `sync_authors_ui_buffer_from_authors`
    every frame), `multicam_state`, `gizmo_visibility`, `next_tool`,
    `current_file.requested_op.take()`.
- [x] **H2** `src/common/skeleton/rig.rs` `Skeleton::posed_bones` — full FK +
  two `Vec`s, called to read one or two bones by `head_box`, `chain_tip`,
  `set_world_rotation`, `ik::solve`, `plant_feet` (which also re-FKs the rest
  pose every call). ~12–30 FKs per body per frame across `advance_animators`
  and `update_hitboxes`. Fix: cache rest tails on `Skeleton`; add
  `posed_bone(pose, root, index)` walking only the ancestor chain; a
  non-allocating variant for `duck_under`. Done: `posed_bone` and
  `each_posed_bone`, identical to the bit (root-first products); pose plus
  hitboxes for 60 bodies went 0.225 → 0.100 ms/frame in release. Rest tails
  were not cached — rig-local tails reassociate the products, so only close.
  Callers that want every bone (`body_mesh`, ragdoll, viewmodel, drawing)
  still collect, now with one allocation rather than two.
- [x] **H3** `src/game/player.rs` `interpolate_bodies` — writes `Transform` on
  every `PhysicsBody` every frame even when still; re-propagates every bone
  mesh. Fix: compare first.
- [x] **H4** identical writes to replicated components: `drive_carousels`
  (`ForcedAnimation`, `DisplaySpeed`), `describe_player_bodies`
  (`BodyRequests`), `step_player` (`Stance` via `change_stance`, also wakes
  `follow_stance`; `Player`/`PhysicsBody` on idle bodies). Fix: `set_if_neq`.
- [x] **H5** `src/prop/document.rs` `doc_mut` bumps the one `generation` for any
  write, so dragging a grip/carry/viewmodel/animation key re-runs CSG and
  re-uploads meshes every frame (`src/prop/view.rs` `rebuild_the_prop`,
  `ghost_the_operands`). Fix: a geometry generation, or compare `shown()` to
  the features last built.
- [x] **H6** `src/anim/timeline.rs` `lay_out_the_timeline` — writes ~700 pooled
  `Node`s every frame, hidden ones too; full UI re-layout while paused. Fix:
  compare before writing.

## MEDIUM

- [x] Drags held still write every frame: `drag_the_selected`, `drag_the_hold`
  (`src/prop/gizmo.rs`), the timeline key nudge (`src/anim/mod.rs`);
  `BodyClips::clip_mut` calls `changed()` before finding the clip.
- [x] Per-frame `Arc::new(clip.clone())`: `perform_weapon_clips`
  (`src/game/held.rs`), `drive_the_preview` (`src/anim/mod.rs`). **Not** by
  spreading `Arc<Clip>` further — the `Arc`s already there are the cause.
  Clips become an asset: `Clip: Asset`, `Playing.clip: Handle<Clip>`,
  sampling reads `Res<Assets<Clip>>`, the way `BakedProp.parts` already holds
  `Handle<Mesh>`. Replace `Arc<Clip>` (`play.rs` `Playing`),
  `Arc<StateAnimation>` (`state.rs` `ClipLibrary`) and `Arc<ClipSet>`
  (`baked.rs`, `held.rs`). A save or an edit re-inserts under the same handle
  (`AssetEvent::Modified`) instead of every frame re-cloning to catch one.
  Derived clips (`at_heading` blend previews) are added once per change, not
  per frame. Done: weapon and preview clips are `Handle<Clip>`; `ClipLibrary`
  holds its animations by value (one resource is the single owner);
  `ClipAssetsPlugin` requires `AssetPlugin` rather than falling back.
- [x] Gizmo material churn: `point_drag`, `rotate_drag`, `room` remove +
  re-insert `MeshMaterial3d` on every handle every frame. Add a
  `GizmoPart { axis }` marker, assign only when different; drop the child
  `Vec` and linear scan.
- [x] `src/tool/tool_helpers.rs` `find_nearest_feature_hit` / `rooms_but` —
  O(n²) room clones per frame. Pass a slice and skip an index. Done for
  picking; `CollisionWorld::rebuild` and the room bake still clone the other
  rooms, but run on rebuild rather than per frame.
- [x] `src/tool/movement.rs` writes `CursorOptions` every frame (winit re-grabs).
  Compare first; `cameras.get_mut(cam_id)`.
- [x] `place_the_viewmodel_arms` (`src/game/viewmodel.rs`) re-solves IK every
  frame on inputs that change only on respawn/weapon/FOV/aspect. Key it.
- [x] `pose_body_meshes` (`src/game/body_mesh.rs`) re-poses sleeping corpses and
  static previews. Filter `Or<(Changed<Pose>, Changed<BodyMesh>,
  Changed<SkeletonRoot>)>`, `set_if_neq`.
- [x] `draw_debug_laser` (`src/game/hitscan.rs`) traces every body for every
  player every frame, not tied to F4. Done: gated on `ShowHitboxes` (F4),
  now off by default.
- [ ] `Ragdoll::step` ~12 allocations per corpse per tick; scratch buffers,
  `par_iter_mut`.
- [ ] `advance_animators` recomputes identical poses in the editor (clock
  frozen); `par_iter_mut` or skip when inputs unchanged.

## LOW

- [ ] `Option<&LocalPlayer>` as a bool → `Has<LocalPlayer>`: `draw_hitboxes`,
  `draw_skeletons`, `dress_held_weapons`.
- [ ] Unused fetches: `step_projectiles` `&mut Transform`; `&Multicam` in
  `navigate`, `mouse_input`, `draw_camera_gizmos` → `With<Multicam>`;
  `draw_room_bounds` `Entity`.
- [ ] Early returns that want `run_if`: F4/F6 draws, `debug_boxes` (also `ResMut`
  → `Res`), `start_in_play`, adopt/send in `map_sync`, `weapon_sync`,
  `prop::sync`, `announce_mode_change`.
- [ ] `give_bodies_to_whoever_needs_one` builds rooms and spawns before checking
  anyone needs a body.
- [ ] `sync_animation_grids` allocates and scans per grid every frame.
- [ ] `sync_entities` collects the whole feature order every frame.
- [ ] `name_the_held_weapon` formats strings every frame.
- [ ] `keep_a_clip_selected`, anim panels: clone clip names / every clip per
  frame.
- [ ] `Pose::lerp` slerps shared joints twice; `Skeleton::index_of` linear
  string scan on hot paths.
- [ ] `build_body_meshes` rebuilds all parts on `Team`/`BodyTint` change; only
  the material differs.
- [ ] `announce_the_dead` could filter `Changed<Damageable>`.
- [ ] `broadcast_catalogue_changes` deep-compares the catalogue every frame; add
  an `is_changed` latch in front.
- [ ] `fire_flame` traces every ray against every body; broad-phase first.
- [ ] `retarget` builds `prior_features` twice a frame with `contains` checks;
  iterate a prefix.
- [ ] `mouse_input` computes an unused ray and scans every camera.
