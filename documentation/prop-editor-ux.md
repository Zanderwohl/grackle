# Prop editor UX

The prop editor (`src/prop/`) works, but its panels grew one feature at a time
and read that way. This is the plan for reorganising it around what somebody
modelling a weapon is actually doing. Tick items off as they land; the checklist
is the source of truth for what is done.

## Diagnosis (October 2026)

- **The Features panel does three jobs**: undo/redo buttons, the Add menu and
  the tree. Undo is not a property of the tree, and neither is creation.
- **Operations act on recency, not selection.** Boolean took the last two live
  bodies, Mirror and Paint the last one; operands were fixed up afterwards in
  dropdowns. Every CAD tool people know is select-then-act.
- **Hold is a different job hidden as a tab**, with a hidden mode switch (the
  "handles" checkbox silently turns the feature gizmos off).
- **Scene is one dropdown** — the reference figure, which matters almost only
  while authoring the hold.
- **The inspector is one flat scroll**, and rotations are bare Euler angles.
- **Two places to colour a body** — a surface on the producing feature, or a
  Paint feature — and nothing says which wins.
- **Consumption is invisible structure**: a boolean's operands are only dimmed
  rows, not shown as belonging to it.
- **No viewport picking.** A feature can only be selected from the tree.

## Target layout

Two workspaces over one viewport, picked by a segmented control at the start of
a toolbar under the menu bar.

```
┌ File  Edit  View  Mode ───────────────────────────── rocket_launcher.gpp * ┐
├ [Model | Fit]  Solid▾  Sweep▾  Combine▾  Mirror  │ Frame                   ┤
├──────────────┬───────────────────────────────────────┬────────────────────┤
│ HISTORY      │                                       │ PROPERTIES         │
│ ▸ Tube       │           4 viewports                 │ ▾ Feature          │
│ ▾ Drilled ⊖  │                                       │ ▾ Shape / Profile  │
│    Tube      │                                       │ ▾ Placement        │
│    Bore      │                                       │ ▾ Material         │
│ ━━ rollback  │                                       │ ▾ Operands         │
├──────────────┴───────────────────────────────────────┴────────────────────┤
│ Problems (0) · 3 bodies · 1,204 tris · selected: Drilled                  │
└───────────────────────────────────────────────────────────────────────────┘
```

- **Model**: toolbar of creation verbs enabled by selection; History on the
  left; Properties on the right in collapsible sections; with nothing selected,
  Properties shows the prop itself.
- **Fit**: the right panel becomes the hold editor (Grip, Carry, Viewmodel,
  Per-class); hold handles on and feature gizmos off *by being in Fit*; the
  reference figure shown; a class picker in the toolbar that is both "who is
  standing there" and "whose override am I editing".

## Decisions

- **Selection is ordered.** Ctrl/Cmd-click adds to it; the last picked is the
  primary (what the inspector and gizmos show). Combine takes the first picked
  as the target and the primary as the tool — "select the barrel, then the
  bore, Subtract" reads in the order it is said.
- **Paint becomes Repaint**: an action on a body's right-click menu rather than
  a peer of Extrude in the Add menu. Body-producing features keep their own
  surface as the main way to colour something. The on-disk tag stays `paint`.
- **Rotation fields** get a friendlier control (axis presets / "point this axis
  along…"), with the raw Euler angles behind a disclosure.
- **Profile points: deferred.** The numeric list stays until a 2D sketch overlay
  in the matching orthographic view is worth building.

## Checklist

### Easy

- [x] Operations act on the selection (ordered multi-select with Ctrl/Cmd-click;
      Combine, Mirror and Repaint disabled when the selection does not fit —
      no instructional hover text; the UI states what is, it is not a
      tutorial).
- [x] Paint → Repaint, moved to the tree's right-click menu.
- [x] Properties in collapsible sections: Feature, Shape/Profile, Placement,
      Material, Operands.
- [x] Empty selection shows the prop itself (name key, size, counts).
- [x] Undo/Redo out of the tree into an **Edit** menu.
- [x] Add menu out of the tree into a toolbar under the menu bar; Frame moves
      there too.

### Medium

- [x] Rotation control for placements: "Points along" ±X/±Y/±Z for the
      feature's own length axis (`FeatureOp::main_axis`), a Spin about it, and
      Euler behind a fold. Built on `rotation::aim_and_spin`.
- [ ] The same control for the hold's three rotations. Not done: a hand has
      no obvious "length axis" to aim, so it wants deciding with the Fit
      workspace rather than guessing now.
- [x] Duplicate on the right-click menu (not for features that consume —
      the copy would find its operands already eaten).
- [x] Frame selected (toolbar + `F`), beside Frame all.
- [x] Click a body in the viewport to select it; Ctrl/Cmd-click to extend;
      a plain click on nothing clears.
- [x] Status line at the bottom: selection, counts, size, settling.
- [x] Problems: count in the tab title.

### Larger

- [ ] **Model / Fit workspaces.** Fit absorbs the Hold and Scene tabs and the
      handles checkbox; class picker in the toolbar.
- [ ] Nested operands in History (a boolean shows what it consumed beneath it).
- [ ] Rollback bar: show the model as of feature *N* (`Evaluated::live_before`
      already records what each step saw).
- [ ] Ghost consumed tool bodies while a boolean is selected.
- [ ] Linear / radial pattern feature.
- [ ] Revert to saved.

### Deferred

- [ ] 2D sketch overlay for `Profile::Points`.
