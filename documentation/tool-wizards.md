# Tools as wizards

A proposal to rebuild the prop editor's tools and the map editor's tools as **wizards**: each tool
is a fixed list of steps, a selection that fits fills steps in advance, Escape takes back one
pick at a time, and a step may take any number of picks before Enter confirms. The model is the
one in `~/rust/century` (`crates/century-app/src/wizard/`). Taking more than one pick per step
in the prop editor needs features that make more than one body, so the proposal also separates
what a feature *makes* (its products) from the feature itself.

Nothing here is built. The checklist is at the bottom.

## How century does it

`century-app/src/wizard/mod.rs` is about 600 lines of driver and a file per wizard.

- **A wizard is a trait object** (`Wizard`): `update` gets one frame of pointer input and returns
  `Continue` or `Done(params)`; `back` steps back once and returns `false` on the first step;
  `hint` names the step under way as a verb and its object ("Place a point."); `complete` says
  whether what it has is enough without another click; `confirm` builds the feature from what it
  has. Optional methods draw guides, a ghost, a dial (a length typed on the toolbar or dragged
  from the press), and say what the map may do with the pointer meanwhile.
- **A registry row per wizard** (`WizardEntry`): key, icon, the feature kind it makes, and how
  many picks a selection may supply (`picks`, `usize::MAX` for any number).
- **Starting from a selection** (`wizard::start`): the selection's points are fed to the wizard as
  if clicked, in order. All of them makes the feature at once; some starts it partway; more than
  `picks` starts it empty. A wizard that takes any number says whether the selection was enough
  through `complete`.
- **Escape** calls `back`; on the first step it exits, making nothing. **Enter** calls `confirm`
  and exits whether or not a feature came of it.
- **A step may take many picks.** Line takes points in pairs and Confirm makes every pair a line
  of *one* Line feature, dropping a point left without a partner. Road takes any number of paths.
  B-spline takes control points until the first is picked again.
- **A pick names a product, not a feature.** `Anchor::Point(PointId { feature, index })` or
  `Anchor::End { path: PathId { feature, index, of }, end }`. A feature's params are separate from
  what evaluating it produces (`timeline::Product`: points, derived points, outlines, segments,
  curves, ways). `PathId::of` is how many paths the feature had; when that count changes the id
  names nothing, so what referred to it gets a Problem rather than silently pointing at another
  path.
- **Picks follow the map.** Each frame every pick is resolved again (`follow`); a pick that no
  longer resolves is dropped with every pick after it.
- **Points placed along the way** (a click on empty map) are kept pending, are pickable again,
  and are inserted with the feature in one undo step, owned by it. Escape drops a placed point it
  unchose.
- **Editing a made feature** (`wizard::edit`) starts the wizard again on that feature's paths or
  stops, with the map cut just after the feature, and on finishing replaces only that part of the
  feature's params.
- **A click the wizard takes is cleared** from `ButtonInput`, so nothing under the pointer also
  acts on it.

## Where Grackle stands (November 2026)

### The prop editor

[`src/prop/tool.rs`](../src/prop/tool.rs) and [`src/prop/slot.rs`](../src/prop/slot.rs) are
already most of a wizard, and declarative where century's are not:

- A tool is a `ToolSpec` row: name, icon, a list of `Slot`s (a `Kind`, a prompt key, and checks
  that see the other slots' values) and a `build` function from one resolved value per slot.
- Starting a tool fills slots from the selection (`fill`: each pick into the first open slot it
  fits, in any order), and finishes at once if that is enough (`start` checks `is_ready`).
- The picks a tool collects **are the selection**. A click toggles a pick in or out. The
  viewport only offers picks that fit (`accepts`).
- A tool finishes by itself when no slot could take more (`is_finished`); an open-ended last slot
  (regions, profiles, a path, a construction) waits for Enter or Done.
- Re-picking a feature's field is the same machinery with one slot (`Task::Repick`), limited to
  what stands before the feature.
- A face picked where a plane is wanted gets a plane feature in the same undo step
  (`PropEditor::add_chain`). This is century's placed points under another name.

What it lacks:

1. **Escape puts the whole tool down** (`tool::cancel`), dropping every pick.
2. **Every operand slot takes exactly one body.** `FeatureOp::Boolean` is one target and one
   tool, `Mirror`, `Pattern` and `Paint` one target each, and a sweep's join or cut one target.
   Uniting five parts is four Union features; drilling one hole through three parts is three
   Subtracts and three copies of the drill.
3. **A body is named by the feature that made it.** `Evaluated::bodies` is
   `Vec<(PropFeatureId, Solid)>`, `Pick::Body` holds a `PropFeatureId`, and the evaluator's rule is
   "a feature that consumes bodies produces one, under its own id". A Mirror of three bodies would
   have to make three bodies, and there is no way to name the second.
4. Two open-ended slots cannot follow one another, because nothing says when the first is done.
5. Body operands are not re-pickable from the viewport (`Field::picked` is false for them),
   because a body the feature has eaten cannot be clicked while the feature stands.

### The map editor

[`src/tool/`](../src/tool/) has one Bevy `State` per tool (`Tools`) and a hand-written state
machine per tool:

- **Point-like tools** (Point, Light, Spawn, Camera, Decal, Prop, Weapon pickup, Mirror) share
  `PointPlacementTool<F>`: one point, then back to Select.
- **Room, Block and Trigger** take a first corner then a far corner. Escape while placing the far
  corner drops the first.
- **Stairs** takes four points. Escape drops all of them.
- **Footprint** takes corners until the loop closes on the first. Escape pops one corner.
- Each point is picked by `PointPicker`: a click at the cursor (absolute), or Shift, a click on an
  existing named point and a second click for the offset (relative). Escape backs out of a
  relative pick first.
- **The selection is ignored.** `FeatureTimeline` keeps an ordered multi-selection of features,
  but no tool reads it.
- **No prompt is shown.** Nothing on the toolbar says what the running tool is waiting for.
- **A Mirror's sources are chosen in its panel** after it is placed, though choosing them is the
  main thing a mapper does with one.
- **Retarget** is a hidden tool that re-points one `PointRef`; it is the map's version of
  `Task::Repick`.
- What a map feature makes is spread over `FeatureTrait` methods (`available_point_keys`,
  `rooms`, `volumes`, `pick_boxes`) and ECS components, and showing or picking a new kind needs
  arms in `show.rs`, `tool_helpers.rs` and `point_drag.rs`.

## Decisions

### 1. One wizard model for both editors, with declarative steps

Keep the prop editor's declarative slots rather than century's free-form `update`. A step list
can be filled from a selection, re-picked, tested without input, and drawn as a prompt without
each tool writing that code. A step whose input is not a click on a product (a drag, a closing
loop, a value) is a step kind with its own handling, not a reason to hand-write the tool.

The shared part lives in `src/common/wizard.rs`, generic over each editor's pick and step kinds:

```rust
pub trait Steps {
    type Pick: Clone + PartialEq;
    fn count(&self) -> usize;
    /// Enough for the step to build from.
    fn answered(&self, step: usize, has: &[Self::Pick]) -> bool;
    /// Another pick may still go in.
    fn open(&self, step: usize, has: &[Self::Pick]) -> bool;
    /// Answered, but another pick is likely, so the wizard waits to be told it is done.
    fn waits(&self, step: usize, has: &[Self::Pick]) -> bool { self.open(step, has) }
    fn fits(&self, step: usize, pick: &Self::Pick, filled: &Filled<Self::Pick>) -> bool;
}
```

Each editor implements it for one wizard against its own document: the prop editor's
`PropSteps` holds a task's slots, the document and its evaluation, and its `fits` runs the
slot's kind and checks. Prompts stay with the editor, since each words "pick more" its own way.

`fill`, `is_ready`, `is_finished`, `under_way`, `accepts` and `back` live there (stage 1); the
seeding rule below joins them in stage 2. The toolbar's prompt widget, Escape and Enter handling,
and the Done and Next buttons are to be shared too once the map editor uses it.

### 2. Escape takes back the last pick

- Escape first backs out of a pick in progress (a relative point's offset, a drag).
- Otherwise it removes the **last pick in pick order**, wherever it went. Removing it may reopen
  an earlier step; the step under way is always the first one not answered.
- With no picks left, Escape exits the wizard. A re-pick then restores the selection it started
  from, as `tool::cancel` does today.
- A plane feature, placed point or other pending product that the removed pick needed is dropped
  with it.

Picks seeded from the selection are picks like any other, so Escape takes those back too, one at
a time. So are the curves **Chain** adds to a path: Escape takes them back one curve
per press, so a long chain is shortened from its newest end rather than dropped whole. Clicking a picked item still toggles it out, wherever it is in the list.

### 3. A step may take any number of picks

A step is **answered** when it has enough to build from, and **open** while it may take another
pick. A step that is full (a single body, a single axis) is not open, so the next pick goes on to
the step after it by itself.

- The wizard finishes by itself when every step is answered and the last does not wait for more
  (`Steps::waits`).
- When the last step holding picks is answered and open, and a later step is not answered,
  **Enter moves on** to the next step, and the toolbar shows a **Next** button in place of Done.
  A pick that fits only a later step also goes there, as `fill` allows.
- Enter finishes the wizard when every step is answered and there is nowhere to move on to.
- Moving on is recorded as a marker on the pick it followed (`ActiveTool::moved_on`), not as a
  step index, because the picks are the selection and are filled again every frame. A marker
  whose pick is clicked off is dropped.
- **Escape takes back moving on as its own step.** After Next and one more pick, the first Escape
  removes that pick and the second reopens the step before it. Undoing the last thing done is
  easier to predict than undoing two things at once.

Seeding from a selection fills steps in order. **An answered step yields a pick to a later step
that is not answered and would take it** (`wizard::seed`), and the move is recorded as a marker
as Next would be. A selection of bodies A, B and C fed to Subtract makes A the target and B and C
the tools, which matches what CAD programs do with "first picked is the target". If the selection
answers every step, the feature is made at once, as now.

### 4. Products are separate from features

A feature is what is stored and edited. A product is something evaluating it makes that another
feature can name: a body, a plane, a corner, a region, a named point on the map. Picks and
references name products. This is century's split between `Params` and `Product`.

**In the prop editor**, everything but bodies already works this way: corners, edges and faces are
`GeomRef`s carried through copies, regions are named by their bounding lines, seams by their two
faces, and planes, axes and points are datum features. Bodies change:

```rust
/// One body a feature made. `of` is how many bodies the feature made when this was taken: a
/// different count means its bodies were renumbered, so the reference names nothing.
pub struct BodyRef {
    pub feature: PropFeatureId,
    pub index: u32,
    pub of: u32,
}
```

- `Evaluated::bodies` becomes `Vec<(BodyRef, Solid)>`, `Pick::Body` holds a `BodyRef`, and
  `consumes` returns `BodyRef`s.
- The evaluator's rule becomes: **a feature that consumes bodies produces bodies under its own
  id, numbered in the order of the operands they came from.** A Mirror of three bodies makes three,
  index 0 to 2. A Union of five makes one.
- **A file writes a bare feature id for `index: 0, of: 1`** and reads one back the same way, so
  every shipped `.gpp` keeps loading unchanged and most references stay short. Only a feature
  with more than one body writes the long form.
- A reference whose `of` no longer matches is a Problem on the feature that holds it, under the
  same rule century applies to `PathId`. The alternative is to name an output by the operand it
  came from, so removing the second target of a Mirror does not renumber the third. That is more
  stable, but a reference then nests one level per feature it passed through. Start with
  `index`/`of`; see the open questions.
- A body's bone (`src/prop/rig.rs`) is inherited per output, from the operand it came from, rather
  than from the first operand of the whole feature.
- The feature list draws a row weak when **all** its bodies are eaten.

**On the map**, the products are named points, volumes and pick boxes, read today through
several `FeatureTrait` methods. Collect them into a `MapProducts` resource rebuilt on each resolve:
points keyed by `PointId { feature, key }`, the Mirror copies' points under their existing
`{source}@{image}/{key}` keys, volumes and pick boxes by feature. The wizard driver, hover,
picking and the selection read it. A new feature kind then joins by producing products, and the
`show.rs`, `tool_helpers.rs` and `point_drag.rs` arms become reads of `MapProducts`. This is the
largest map change and is not needed for the first map stage, which can read the existing
methods.

### 5. Prop features that take many operands

With `BodyRef` in place:

| Feature | Steps | Bodies made |
| --- | --- | --- |
| Union | bodies, 2 or more | 1 |
| Subtract | targets, 1 or more; then tools, 1 or more | one per target |
| Intersect | bodies, 2 or more | 1 |
| Mirror | bodies, 1 or more; then a plane | one per body |
| Linear / radial pattern | bodies, 1 or more; then a direction or axis | one per body |
| Paint | bodies, 1 or more | one per body |
| Extrude / Revolve / Sweep / Loft, joining or cutting | profile; axis or path; targets, 0 or more | one per target, or 1 |

`FeatureOp::Boolean { op, target, tool }` splits into `Union { bodies }`,
`Subtract { targets, tools }` and `Intersect { bodies }`, since the three no longer have the same
shape. A file holding the old form is read into the new one (one target and one tool), which is
the one piece of the change that reads an old form.

Two of these steps are followed by another step, so Mirror and Pattern need the **Next** rule
from decision 3. Their order puts the bodies first, which is what is usually selected before the
tool is clicked.

### 6. Editing a made feature is a wizard started on it

`Task::Repick` becomes "start the feature's wizard on one of its fields":

- The wizard is preloaded with the field's current picks, so Escape takes them back one at a
  time and a click adds to a list field.
- **The view is peeked to just before the feature** (`PropEditor::peek_at`), so the bodies it
  eats are standing and can be clicked. This removes the reason body operands are lists in the
  Inspector rather than Pick rows: every field of `FeatureOp::fields` gets a Pick row.
- Finishing replaces that field only, in one undo step; anything else changed meanwhile is kept,
  as century's `wizard::finish` does.

On the map, the same machinery replaces the hidden Retarget tool, and gives a Mirror's sources a
Pick row that starts its sources step.

### 7. The map's tools become wizard entries

`Tools` shrinks to `Select`, `Wizard` and `Spectate`; which wizard runs is a `MapWizard` resource,
and the toolbar's groups list wizard entries instead of states. One driver system runs every
wizard, gated on `Tools::Wizard` and `AppMode::Editor`, so the rule that every editor system is
gated still holds in one place.

Map step kinds:

| Kind | Answered by |
| --- | --- |
| `Point` | a click at the cursor (absolute), a click on a named point, or Shift's relative pick, all through `PointPicker` |
| `Corners { closes }` | points chained into a loop, closed by picking the first again |
| `Feature(filter)` | a feature in the viewport or a Features panel row, such as anything `mirror` accepts |
| `Value(dial)` | a number on the toolbar, or a drag from the press, such as a facing or a height |

| Wizard | Steps |
| --- | --- |
| Point, Light, Camera, Decal | a point |
| Spawn point | a point; then a facing (value, dragged from the press) |
| Prop, Weapon pickup | a point; the model or weapon is chosen in the picker shown on the toolbar during the step |
| Room, Block, Trigger | a corner; then the far corner |
| Footprint, Footprint block | corners, closing |
| Stairs | start; end; width point; height point |
| Mirror | a center; then sources, 1 or more; symmetry as a toolbar value |

- **Seeding**: a step that takes a point takes a selected point-like feature (a `GlobalPoint`) as
  `PointRef::reference(id)`; a `Feature` step takes selected features that pass its filter. A
  Mirror started with five features selected asks only for its center.
- **Prompt**: the map editor's toolbar shows the step's prompt, as the prop editor's does. This
  is the one kind of instruction text CLAUDE.md allows.
- **A point is one step or two, decided by the click.** The Room tool's picking stays as it is
  (`PointPicker`): a left click on empty space takes an absolute point and answers the step; a
  left click on a named point takes a point relative to it and adds an **offset** step, answered
  by a second left click (the offset to the cursor) or a right click (no offset). Escape during
  the offset step takes back the named point, which removes the offset step with it. So a Room
  is two steps when both corners are free and up to four when both are measured from points.
- **Absolute points need no pending products.** A map feature stores an absolute point inline in
  its `PointRef`, so the century rule for placed points does not apply here.
- **After finishing**, the made feature is selected and the editor returns to Select, as the
  point tools do now.

### 8. Previews

When every step is answered and the last still takes more (a Union of two or more bodies, a
Subtract's tools, an Extrude's regions, a Sweep's path), the view shows what Enter would make. A
Mirror finishes when its plane is picked, so it never waits. In the prop
editor this is an evaluation of the document with the would-be feature appended, built on the
debounced build path; in the map editor it is the would-be feature's gizmos, as the Room and
Stairs tools draw today.

## Order of work

The prop editor first, because it is closest. Escape and the shared module carry no data change.
`BodyRef` is the large step, and every later prop stage needs it. The map stages need none of
the prop work except the shared module.

## Checklist

### Stage 1: shared module and Escape (prop editor)

- [x] `src/common/wizard.rs`: `Steps`, `Filled`, `fill`, `is_ready`, `is_finished`,
      `under_way`, `accepts`, `back`; `prop/tool.rs` implements `Steps` for a task's slots
      (`PropSteps`) and calls it.
- [x] Per step: `answered` replaces `waiting`. `open` (may take another pick) and `waits` (an
      answered step expects another pick) stay apart, since a construction is complete with one
      face yet still takes a second for a midplane. Fixed least and most counts did not fit
      constructions or a region step closed by a face.
- [x] Escape removes the last pick, and exits with none left; a re-pick restores its selection.
      The trash button still puts the tool down at once.
- [x] Escape drops a pending plane feature its removed pick needed. Nothing to do: plane
      features for faces are made only when the tool finishes, so none is pending.
- [x] Test: Escape walks back through seeded picks one at a time, then exits.

### Stage 2: many picks per step and Next

- [x] Enter, and a Next button, close an answered open step that is not the last
      (`wizard::can_move_on`, `wizard::move_on`; markers in `ActiveTool::moved_on`).
- [x] Seeding: an answered step yields a pick to a later unanswered step that takes it
      (`wizard::seed`), recorded as a marker so filling again agrees.
- [x] Test: a selection of three into a two-step, two-list wizard fills 1 and 2 (in
      `wizard.rs`, since no prop tool has two body lists until stage 4).
- [x] Test: Escape after a step's first pick, then again, reopens the step before it; and
      Revolve moves on with Enter after its regions.

### Stage 3: products for bodies

- [x] `BodyRef { feature, index, of }` in `feature.rs`; `Evaluated::bodies`, `wires`,
      `live_before`, `body_of`, `body_under`, `Pick::Body`, slot values and every body operand take
      it. `consumes` still answers which *features* are eaten, for ordering and dependencies;
      `eats` answers which bodies.
- [x] Serde: a bare id reads and writes as `index: 0, of: 1`
      (`a_body_of_a_feature_that_makes_one_is_written_as_its_bare_id`).
- [x] A mismatched `of` is a Problem (`prop.problems.renumbered_body`), and the body is left
      standing.
- [x] Bones inherited per output (`rig::bone_of`); a feature row is weak once every body it made
      is eaten.
- [x] `every_prop_the_default_pack_ships_loads_and_builds` still passes with no file rewritten.
- Picking a feature as a whole (`PropEditor::select`) picks `BodyRef::whole`, since every feature
  still makes one body. Stage 4 has to pick its first standing body instead.

### Stage 4: prop features with many operands

- [x] `Union { bodies }`, `Subtract { targets, tools }`, `Intersect { bodies }` replace `Boolean`;
      `document::parse` reads the old `boolean` and every single `target` into lists
      (`a_file_naming_single_operands_reads_as_lists`). The shipped props are left in the old form:
      saving them also sorts their clips by name, so the diff would be mostly reordering.
- [x] Mirror, Pattern and Paint take a list of targets and make one body per target.
- [x] A sweep's join, cut or intersect takes a list of targets: a join makes one body, a cut or
      intersect one per target. A join with no target is still a Problem.
- [x] `TOOLS` rows: Union and Intersect take bodies, 2 or more; Subtract targets then tools;
      Mirror and the patterns bodies then a plane, direction or axis (`slot::Kind::Bodies`).
- A face picked while a body list is open joins the list as its body, since a flat face of a
  second body is as likely meant as a body as a plane. Enter or Next moves on to the plane.
- [x] One tool through several targets names its faces apart in each body (`own_copy`), so a
      click on the wall it cut in one body picks that body.
- [x] Tests by volume: one tool through three targets; a Union of four; a Mirror of two.
- [x] Test: Subtract started from three selected bodies gives target A and tools B and C.
- [x] `PropEditor::select` on a feature that makes several bodies picks body 0 of
      `FeatureOp::bodies_made`.
- [x] The Inspector edits each list (`body_list`), and the boolean's operation switch keeps its
      operands, the first as a subtraction's target.

### Stage 5: editing a made prop feature

- [x] Re-pick peeks to just before the feature (`tool::peeked`, written by `keep_the_session`
      beside the open sketch's peek). A list field (bodies, a path) is preloaded with what it
      names; a single field starts empty, since a preloaded single pick would answer its step and
      finish the re-pick at once.
- [x] Body operand fields are Pick rows (`Field::picked` true; a sweep's targets only while it
      joins, cuts or intersects). The Inspector's body dropdowns are gone; choosing a sweep
      operation with no targets starts picking them.
- [x] Finishing replaces only that field, in one undo step (`finish`, as before).
- [x] Tests: a re-picked Subtract's tools start from what they were, the eaten cubes stand, a
      click adds and Enter writes, one undo takes it back; Escape out of a preloaded re-pick
      leaves the field as it was.

### Stage 6: preview

- [x] With every step answered and the last still taking more, the view draws the would-be
      result as a wireframe (`tool::would_make` builds the feature on a copy of the document;
      `view::preview_the_result` evaluates it when the picks or the document change). Re-picks
      show no preview: the view is peeked before the feature, and the field's change is the
      feature's own rebuild once Enter lands. A datum tool waiting on Enter shows nothing, since
      it makes no body, and a prop whose rebuild is held back for cost shows no preview.

### Stage 7: map wizards

- [x] One driver system (`map_wizard::run_map_wizard`) and a `MapWizard` resource holding the
      picks. **`Tools` keeps a variant per tool** rather than shrinking to Select, Wizard and
      Spectate: the toolbar and `FeatureTrait::tool` name tools by it, and a second resource
      naming the running entry would be a second writer of the same fact. The driver runs under
      any variant with an entry.
- [x] Map step kinds `Point`, `Corners`, `Features` and `NamedPoint` (the re-target's), behind
      `MapSteps`. `Value` is not built: no tool has a value step yet.
- [x] Wizard entries for every tool (`ENTRIES`); `PointPlacementTool`, `RoomTool`, `StairsTool`,
      `FootprintTool` and `RetargetPlugin` are gone, and `PlaceablePoint` is only how a point
      feature is made and previewed.
- [x] Toolbar prompt, Next or Done, and Cancel, in the Tools tab under the buttons
      (`map_wizard::status_ui`).
- [x] A point step is picked by `PointPicker`, so a click on a named point (with Shift) grows it
      by an offset step: a second click, or a right click for none. Escape inside the offset step
      backs out of it first.
- [x] Seeding: a selected point-like feature is a point pick, any other a feature pick, through
      `wizard::seed`. A selection that would finish the tool gives it nothing: finishing selects
      what it made, so picking the same tool again would copy it.
- [x] Mirror: a center, then sources (one or more, Enter to finish). Its panel's **Pick** starts
      the hidden `Tools::MirrorSources` entry from its current sources, taking only features
      before it; Enter finishes it even with none.
- [x] Retarget is the hidden `Tools::Retarget` entry: one named point, from features before the
      one edited or its own bases.
- [x] Test: Escape walks back through a Stairs wizard one point at a time; also a Mirror's two
      steps, a far corner as a size, a loop closing only with three corners, and re-picked
      sources taken only from before the Mirror.
- Behavior that changed: Delete no longer takes back a footprint corner (Escape does); a Mirror
  needs a source to be made; a footprint corner on a named point now takes an offset like every
  other point; Enter places at the cursor only while the wizard cannot move on or finish.

### Stage 8: map products

- [x] `MapProducts` (`src/tool/map_products.rs`), gathered again when the timeline moves (the
      pick cache's key): each feature's volumes, pick boxes and main point, and the named points
      by `PointId { feature, key }`, gathered the first time one is asked for after a change, since
      a drag changes the map every frame and asks for none.
- [x] `FeatureTrait::picked_by` (`Walls`, `Outside`, `Marker`, `Nothing`) replaces the per-kind
      arms in `tool_helpers.rs` and the point-like list in `point_drag.rs`. Selection's hover and
      the wizard's feature step read `MapProducts::hit`. `show.rs` keeps its switch per kind: it
      maps the Show tab's checkboxes, which is a fact about the panel. The point picker still asks
      each feature for its points along the ray, which is how a room shows its corners only when
      the ray passes near it.
- [x] Shift-click in Select picks a named point (`SelectionState::point`), drawn beside the
      feature selection and cleared by a plain click; the next tool takes it as a point step's
      pick, ahead of the selected features, and uses it up. Unlike the feature selection, it may
      finish a tool: Shift-pick a corner, choose Spawn Point, and the spawn is made there.
- [x] Tests: picking a point by its point and a trigger by its box; finding a named point under
      the ray; a Shift-picked corner seeding a Room.

## Open questions

- **Naming an output by index or by operand.** `index`/`of` turns a removed operand into a
  Problem on everything downstream of the outputs after it. Naming by operand avoids that at the
  cost of nesting. Decide after stage 3 shows how often operand lists are edited in practice.
- **Several map features from one wizard.** Century's Line makes many lines in one feature. A
  Room wizard could take corners in pairs and make one room per pair in one undo step. Worth it
  only if mappers block out several rooms in a row often.
- **Staying in a tool after it finishes.** Both editors return to Select. Placing a row of
  pickups or spawns would want the tool to stay up; a held modifier on the finishing click is
  one way.
- **Values during a prop wizard.** Century's dial sets a river's width while the wizard runs. An
  extrude's distance or a pattern's count could be set the same way instead of in the Inspector
  afterward.
