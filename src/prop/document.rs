//! A prop as a document: the feature list, what is selected, and how to get
//! back to what it looked like a moment ago.
//!
//! **Undo is whole-document snapshots**, not per-feature deltas like the map's
//! [`Action`](crate::editor::action::Action). That is a considered difference
//! rather than a corner cut. A map is thousands of features and a blueprint
//! somebody has kept for months, so its history is worth storing carefully and
//! worth persisting. A prop is a few dozen features and a couple of kilobytes:
//! cloning the whole thing per edit is free at that size, and it is correct by
//! construction — there is no before-and-after pair to get the wrong way
//! round, and no operation that can forget to record itself.
//!
//! The price is that history does not survive closing the file, which the map
//! deliberately pays for and this deliberately does not.
//!
//! **The file is flat text, not SQLite.** A map blueprint is an authoring
//! format the runtime never reads, so `rusqlite` costs it nothing. A prop is
//! read by the *game* — it is what a weapon is drawn as — and has to reach a
//! browser tab, where a bundled C library does not go. So a prop is read
//! through [`AssetSource`](crate::common::assets::AssetSource) like
//! `weapons.toml` is, and nothing in this module reaches for a filesystem on
//! the reading side.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::assets::{AssetError, Assets};
use crate::prop::feature::{evaluate, Evaluated, FeatureOp, PropFeature, PropFeatureId};

/// Grackle Prop. Flat text, one prop per file.
pub const PROP_EXTENSION: &str = "gpp";

/// Where props live inside a pack. Named here so the weapon loader and the
/// prop editor cannot disagree about it.
pub const PROPS_DIR: &str = "props";

/// How many edits back you can go.
///
/// Bounded because the stack is whole documents and an editor left open all
/// afternoon should not grow without limit; deep enough that nobody reaches
/// the end of it in practice.
const UNDO_DEPTH: usize = 256;

/// One prop: an ordered list of modelling features and a name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PropDoc {
    /// A lang key for what this prop is called, the way a weapon's is. Free
    /// text would be display text outside the lang layer.
    #[serde(default)]
    pub name_key: String,
    #[serde(default)]
    pub features: Vec<PropFeature>,
    /// The next id to hand out.
    ///
    /// Stored rather than derived from the highest id in use, so that deleting
    /// the last feature cannot make the next one reuse its number — and a
    /// reference held by something outside this file would then quietly point
    /// at a different shape.
    #[serde(default)]
    next_id: u32,
}

impl PropDoc {
    pub fn new(name_key: impl Into<String>) -> PropDoc {
        PropDoc { name_key: name_key.into(), features: vec![], next_id: 0 }
    }

    fn take_id(&mut self) -> PropFeatureId {
        let id = PropFeatureId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Add a feature at the end and return its id.
    pub fn push(&mut self, op: FeatureOp) -> PropFeatureId {
        let id = self.take_id();
        self.features.push(PropFeature::new(id, op));
        id
    }

    pub fn feature(&self, id: PropFeatureId) -> Option<&PropFeature> {
        self.features.iter().find(|feature| feature.id == id)
    }

    pub fn feature_mut(&mut self, id: PropFeatureId) -> Option<&mut PropFeature> {
        self.features.iter_mut().find(|feature| feature.id == id)
    }

    pub fn index_of(&self, id: PropFeatureId) -> Option<usize> {
        self.features.iter().position(|feature| feature.id == id)
    }

    pub fn remove(&mut self, id: PropFeatureId) {
        self.features.retain(|feature| feature.id != id);
    }

    /// Everything that names `id` as an operand — what deleting it would
    /// break, so the panel can say so before rather than after.
    pub fn dependants(&self, id: PropFeatureId) -> Vec<PropFeatureId> {
        self.features
            .iter()
            .filter(|feature| feature.op.consumes().contains(&id))
            .map(|feature| feature.id)
            .collect()
    }

    /// Where a feature may be moved to, as indices in the finished list.
    ///
    /// **Always a contiguous interval**, and that is what makes dragging one
    /// around tractable. The evaluator replays top to bottom, so a feature has
    /// to sit after everything it consumes and before everything that consumes
    /// it — and since moving *one* item leaves every other item's relative
    /// order alone, those two conditions are a single window: after the last
    /// operand, before the first dependant. There is no scattered set of legal
    /// slots to describe, so the panel can clamp a drag into the range rather
    /// than needing a way to refuse a drop.
    ///
    /// **Direct relations are enough.** An indirect operand is already above a
    /// direct one, so keeping clear of the direct ones keeps clear of all of
    /// them.
    ///
    /// Indices are counted in the list **with this feature taken out**, which
    /// is the same thing as the index it will end up at.
    pub fn legal_range(&self, id: PropFeatureId) -> Option<std::ops::RangeInclusive<usize>> {
        let at = self.index_of(id)?;
        let feature = self.feature(id)?;

        // Where an item at `i` sits once this feature is lifted out.
        let without = |i: usize| if i < at { i } else { i - 1 };

        let mut low = 0;
        for operand in feature.op.consumes() {
            if let Some(index) = self.index_of(operand) {
                low = low.max(without(index) + 1);
            }
        }

        let mut high = self.features.len().saturating_sub(1);
        for dependant in self.dependants(id) {
            if let Some(index) = self.index_of(dependant) {
                high = high.min(without(index));
            }
        }

        // A document whose features are already out of order would give an
        // inverted range, and an inverted range is a panic waiting in whatever
        // clamps with it. The lower bound wins: staying put is always legal.
        Some(low..=high.max(low))
    }

    /// Move a feature to `target`, or as close to it as the dependencies allow.
    ///
    /// **Clamped rather than refused.** A drag that would carry a boolean
    /// above its own operands stops at the last legal place instead of
    /// snapping back or landing somewhere broken — which is the whole of how
    /// the panel prevents an illegal order, and why it needs no way to mark a
    /// drop target as forbidden.
    ///
    /// Returns whether anything moved.
    pub fn move_to(&mut self, id: PropFeatureId, target: usize) -> bool {
        let Some(at) = self.index_of(id) else { return false };
        let Some(legal) = self.legal_range(id) else { return false };

        let target = target.clamp(*legal.start(), *legal.end());
        if target == at {
            return false;
        }

        let feature = self.features.remove(at);
        self.features.insert(target, feature);
        true
    }

    pub fn evaluate(&self) -> Evaluated {
        evaluate(&self.features)
    }
}

/// The prop editor's whole state: the document, the undo stacks, and what file
/// it came from.
#[derive(Resource, Debug)]
pub struct PropEditor {
    doc: PropDoc,
    undo: Vec<PropDoc>,
    redo: Vec<PropDoc>,
    /// What the document looked like when an edit began, waiting to be pushed
    /// onto the undo stack if and only if the edit changed anything.
    ///
    /// Deferred rather than pushed eagerly because an egui panel reports a
    /// drag as an edit per frame; without this, one drag of a slider would be
    /// forty undo steps.
    pending: Option<PropDoc>,
    pub selected: Option<PropFeatureId>,
    pub path: Option<PathBuf>,
    /// Whether the document differs from what is on disk.
    pub dirty: bool,
    /// Bumped whenever the document changes, so the viewport knows to rebuild
    /// without comparing two feature lists.
    generation: u64,
}

impl Default for PropEditor {
    fn default() -> Self {
        PropEditor {
            doc: PropDoc::new("prop.untitled"),
            undo: vec![],
            redo: vec![],
            pending: None,
            selected: None,
            path: None,
            dirty: false,
            generation: 0,
        }
    }
}

impl PropEditor {
    pub fn doc(&self) -> &PropDoc {
        &self.doc
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Edit the document, recording an undo step if anything actually changed.
    ///
    /// Everything that writes to the document goes through here. That is what
    /// makes "can this be undone" a property of the type rather than of
    /// whether each call site remembered — the same reason `NetRole` has
    /// exactly one writer.
    pub fn edit<T>(&mut self, change: impl FnOnce(&mut PropDoc) -> T) -> T {
        let before = self.doc.clone();
        let result = change(&mut self.doc);
        if self.doc != before {
            self.push_undo(before);
        }
        result
    }

    /// Begin an edit that may span several frames, such as a drag.
    ///
    /// Paired with [`PropEditor::end_gesture`]. Between the two, the document
    /// may be written to freely and only one undo step comes of it.
    pub fn begin_gesture(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(self.doc.clone());
        }
    }

    /// Write to the document as part of a gesture already begun.
    pub fn doc_mut(&mut self) -> &mut PropDoc {
        self.doc_changed();
        &mut self.doc
    }

    /// Close a gesture, recording one undo step for the whole of it.
    pub fn end_gesture(&mut self) {
        let Some(before) = self.pending.take() else { return };
        if before != self.doc {
            self.push_undo(before);
        }
    }

    fn push_undo(&mut self, before: PropDoc) {
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        // A fresh edit ends the redo branch, the same way the map's timeline
        // drops everything past the rollback bar.
        self.redo.clear();
        self.doc_changed();
    }

    fn doc_changed(&mut self) {
        self.dirty = true;
        self.generation += 1;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        let Some(previous) = self.undo.pop() else { return };
        self.redo.push(std::mem::replace(&mut self.doc, previous));
        self.after_history_move();
    }

    pub fn redo(&mut self) {
        let Some(next) = self.redo.pop() else { return };
        self.undo.push(std::mem::replace(&mut self.doc, next));
        self.after_history_move();
    }

    fn after_history_move(&mut self) {
        // A selection is an id, and undo can take the feature it names away.
        if self.selected.is_some_and(|id| self.doc.feature(id).is_none()) {
            self.selected = None;
        }
        self.doc_changed();
    }

    /// Replace the document wholesale — opening a file, or starting a new one.
    ///
    /// Clears the history rather than making the swap undoable: undoing back
    /// *through* a file open into a document you had closed is not something
    /// anybody means by Ctrl+Z.
    pub fn open(&mut self, doc: PropDoc, path: Option<PathBuf>) {
        self.doc = doc;
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        self.selected = None;
        self.path = path;
        self.generation += 1;
        self.dirty = false;
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.dirty = false;
    }
}

/// Why a prop could not be read or written.
#[derive(Debug)]
pub enum PropFileError {
    Asset(AssetError),
    Malformed(String),
    Unwritable(String),
}

impl std::fmt::Display for PropFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PropFileError::Asset(error) => write!(f, "{error}"),
            PropFileError::Malformed(why) => write!(f, "{why}"),
            PropFileError::Unwritable(why) => write!(f, "{why}"),
        }
    }
}

/// The path a prop named `name` has inside `pack`.
pub fn prop_path(pack: &Path, name: &str) -> PathBuf {
    pack.join(PROPS_DIR).join(format!("{name}.{PROP_EXTENSION}"))
}

/// Read a prop out of a pack.
///
/// Through [`Assets`] rather than `std::fs`, because the *game* does this: a
/// weapon's model is pack content that a browser build has to be able to
/// fetch. The editor's own file dialog goes through [`parse`] instead, since
/// it opens files from anywhere.
pub fn load(assets: &Assets, pack: &Path, name: &str) -> Result<PropDoc, PropFileError> {
    let relative = format!("{PROPS_DIR}/{name}.{PROP_EXTENSION}");
    let bytes = assets.0.read(pack, &relative).map_err(PropFileError::Asset)?;
    let text = String::from_utf8(bytes)
        .map_err(|e| PropFileError::Malformed(format!("{relative}: {e}")))?;
    parse(&text).map_err(|e| PropFileError::Malformed(format!("{relative}: {e}")))
}

pub fn parse(text: &str) -> Result<PropDoc, toml::de::Error> {
    toml::from_str(text)
}

pub fn to_text(doc: &PropDoc) -> Result<String, toml::ser::Error> {
    toml::to_string_pretty(doc)
}

/// Write a prop to disk.
///
/// The one `std::fs` in this module, and it is on the *authoring* side: saving
/// is something the native editor does, the way a blueprint is. Nothing the
/// game does at runtime comes through here.
pub fn save(path: &Path, doc: &PropDoc) -> Result<(), PropFileError> {
    let text = to_text(doc).map_err(|e| PropFileError::Unwritable(e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| PropFileError::Unwritable(format!("{}: {e}", parent.display())))?;
    }
    std::fs::write(path, text)
        .map_err(|e| PropFileError::Unwritable(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::feature::{Axis, BooleanOp};
    use crate::prop::profile::{Placement, Profile};
    use crate::prop::solid::Shape;
    use crate::prop::surface::{Style, Surface, Tint};

    fn sample() -> PropDoc {
        let mut doc = PropDoc::new("weapon.names.rocket_launcher");
        let tube = doc.push(FeatureOp::Primitive {
            shape: Shape::Prism { sides: 12, radius: 0.05, height: 0.9 },
            placement: Placement { origin: [0.0, 0.0, 0.0], rotation: [1.5708, 0.0, 0.0] },
            surface: Surface::new(Style::Metal, Tint([0.3, 0.32, 0.34])),
        });
        let bore = doc.push(FeatureOp::Extrude {
            profile: Profile::Ngon { sides: 12, radius: 0.038 },
            placement: Placement::at(0.0, 0.0, -0.5),
            depth: 1.0,
            midplane: false,
            surface: Surface::default(),
        });
        let drilled = doc.push(FeatureOp::Boolean {
            op: BooleanOp::Subtract,
            target: tube,
            tool: bore,
        });
        doc.push(FeatureOp::Mirror {
            target: drilled,
            axis: Axis::X,
            offset: 0.0,
            keep_original: true,
        });
        doc
    }

    /// The format is the whole storage layer, so this is the test that decides
    /// whether a prop survives being closed. Every variant that has ever been
    /// written has to come back as itself; a field that silently defaults is a
    /// prop that changes shape on load.
    #[test]
    fn a_document_round_trips_through_its_file_format() {
        let doc = sample();
        let text = to_text(&doc).expect("a prop serialises");
        let back = parse(&text).unwrap_or_else(|e| panic!("{e}\n---\n{text}"));
        assert_eq!(doc, back);
    }

    /// The numbers a solid is built from are the numbers in the file. A format
    /// that rounded them would move a bore off-centre by an amount too small
    /// to see and large enough to leave a sliver of metal inside the barrel.
    #[test]
    fn the_geometry_survives_the_round_trip() {
        let doc = sample();
        let back = parse(&to_text(&doc).unwrap()).unwrap();
        let (before, after) = (doc.evaluate(), back.evaluate());
        assert_eq!(before.bodies.len(), after.bodies.len());
        assert!((before.triangle_count() as i64 - after.triangle_count() as i64).abs() == 0);
        for ((_, a), (_, b)) in before.bodies.iter().zip(&after.bodies) {
            assert!((a.volume() - b.volume()).abs() < 1e-6);
        }
    }

    /// Ids are never reused, so a reference cannot come to mean a different
    /// shape than it did when it was written.
    #[test]
    fn deleting_the_last_feature_does_not_free_its_id() {
        let mut doc = PropDoc::new("x");
        let first = doc.push(FeatureOp::default());
        doc.remove(first);
        let second = doc.push(FeatureOp::default());
        assert_ne!(first, second);
    }

    /// What deleting a feature would break, which the tree's context menu
    /// warns about before the fact rather than leaving the evaluator to report
    /// four separate problems afterwards.
    ///
    /// Counted by **who names it**, not by who comes after it: two features
    /// can sit either side of a third without either caring about it.
    #[test]
    fn a_feature_knows_what_is_built_on_it() {
        let mut doc = PropDoc::new("x");
        let a = doc.push(FeatureOp::default());
        let b = doc.push(FeatureOp::default());
        let cut = doc.push(FeatureOp::Boolean { op: BooleanOp::Subtract, target: a, tool: b });
        let mirrored = doc.push(FeatureOp::Mirror {
            target: cut,
            axis: Axis::X,
            offset: 0.0,
            keep_original: true,
        });
        let unrelated = doc.push(FeatureOp::default());

        assert_eq!(doc.dependants(a), vec![cut]);
        assert_eq!(doc.dependants(b), vec![cut], "a tool is depended on like a target");
        assert_eq!(doc.dependants(cut), vec![mirrored]);
        assert!(doc.dependants(mirrored).is_empty(), "nothing is built on the last feature");
        assert!(
            doc.dependants(unrelated).is_empty(),
            "a feature later in the list is not a dependant of one earlier",
        );
    }

    /// The claim the whole drag interaction rests on: what a feature may do is
    /// one unbroken span, so a drag can be clamped into it rather than needing
    /// a way to refuse a drop.
    #[test]
    fn what_a_feature_may_be_moved_to_is_one_unbroken_range() {
        let mut doc = PropDoc::new("x");
        let a = doc.push(FeatureOp::default());
        let b = doc.push(FeatureOp::default());
        let cut = doc.push(FeatureOp::Boolean { op: BooleanOp::Subtract, target: a, tool: b });
        let spare = doc.push(FeatureOp::default());

        // The boolean eats the first two, so it can go anywhere after them:
        // with itself lifted out that is index 2 (the end) and nowhere else,
        // since `spare` is the only thing left to sit before.
        assert_eq!(doc.legal_range(cut), Some(2..=3));
        // `a` is consumed by the boolean at index 2, so with `a` lifted out the
        // boolean is at 1 and `a` must land before it.
        assert_eq!(doc.legal_range(a), Some(0..=1));
        // Nothing depends on `spare` and it depends on nothing.
        assert_eq!(doc.legal_range(spare), Some(0..=3));
    }

    /// The evaluator replays top to bottom, so a boolean above its operands can
    /// never find them. A drag that would do it stops at the last legal place
    /// rather than snapping back — you get the nearest thing you asked for.
    #[test]
    fn a_drag_past_a_dependency_stops_against_it() {
        let mut doc = PropDoc::new("x");
        let a = doc.push(FeatureOp::default());
        let b = doc.push(FeatureOp::default());
        let cut = doc.push(FeatureOp::Boolean { op: BooleanOp::Subtract, target: a, tool: b });
        doc.push(FeatureOp::default());

        // Dragged to the very top, it lands directly under its own operands.
        doc.move_to(cut, 0);
        assert_eq!(doc.index_of(cut), Some(2), "the boolean got above its operands");
        assert_eq!(doc.index_of(a), Some(0));
        assert_eq!(doc.index_of(b), Some(1));

        // And a tool dragged to the bottom stops above the boolean that eats it.
        doc.move_to(b, 3);
        assert_eq!(doc.index_of(b), Some(1));
        assert_eq!(doc.index_of(cut), Some(2));
    }

    /// A move that *is* legal has to actually happen, or the clamp is just a
    /// way of never moving anything.
    #[test]
    fn an_unconstrained_feature_goes_where_it_is_put() {
        let mut doc = PropDoc::new("x");
        let first = doc.push(FeatureOp::default());
        doc.push(FeatureOp::default());
        doc.push(FeatureOp::default());

        assert!(doc.move_to(first, 2));
        assert_eq!(doc.index_of(first), Some(2));
        assert_eq!(doc.features.len(), 3, "the move lost or duplicated a feature");

        assert!(!doc.move_to(first, 2), "moving to where it already is counted as a move");
    }

    /// Every feature in a document stays somewhere it is allowed to be, however
    /// it is dragged about — including the one being dragged. Checked by
    /// re-evaluating, because "legal" means exactly "the evaluator finds
    /// everything it needs".
    #[test]
    fn no_sequence_of_drags_can_break_a_prop() {
        // Offset from each other on purpose: `FeatureOp::default()` is the
        // same box at the same place every time, so subtracting one from
        // another leaves nothing — a real problem, and not the kind this test
        // is looking for.
        let solid = |x: f32| FeatureOp::Primitive {
            shape: Shape::Box { size: [0.2, 0.2, 0.2] },
            placement: Placement::at(x, 0.0, 0.0),
            surface: Surface::default(),
        };

        let mut doc = PropDoc::new("x");
        let a = doc.push(solid(0.0));
        let b = doc.push(solid(0.1));
        let cut = doc.push(FeatureOp::Boolean { op: BooleanOp::Subtract, target: a, tool: b });
        let c = doc.push(solid(0.05));
        let joined = doc.push(FeatureOp::Boolean { op: BooleanOp::Union, target: cut, tool: c });
        doc.push(FeatureOp::Mirror {
            target: joined,
            axis: Axis::X,
            offset: 0.0,
            keep_original: true,
        });

        let ids: Vec<PropFeatureId> = doc.features.iter().map(|feature| feature.id).collect();
        // Every feature dragged to every slot, in both directions, including
        // the ones a careful user would never try.
        for _ in 0..3 {
            for id in &ids {
                for target in 0..ids.len() {
                    doc.move_to(*id, target);
                    assert!(
                        doc.evaluate().problems.is_empty(),
                        "dragging {id} to {target} broke the prop: {:?}",
                        doc.evaluate().problems,
                    );
                }
            }
        }
    }

    /// A drag reports an edit every frame. One gesture is one undo step, or
    /// Ctrl+Z becomes a way to watch a slider move backwards.
    #[test]
    fn a_gesture_is_one_undo_step_however_many_frames_it_took() {
        let mut editor = PropEditor::default();
        editor.edit(|doc| doc.push(FeatureOp::default()));
        let after_push = editor.doc().clone();

        editor.begin_gesture();
        for depth in 1..20 {
            let doc = editor.doc_mut();
            doc.features[0].name = format!("frame {depth}");
        }
        editor.end_gesture();

        assert!(editor.can_undo());
        editor.undo();
        assert_eq!(*editor.doc(), after_push, "one drag took more than one undo");
    }

    /// An edit that changed nothing must not push a step, or clicking about in
    /// the panel fills the history with nothing.
    #[test]
    fn an_edit_that_changes_nothing_records_nothing() {
        let mut editor = PropEditor::default();
        editor.edit(|doc| doc.push(FeatureOp::default()));
        let before = editor.can_undo();
        editor.edit(|doc| {
            let _ = doc.features.len();
        });
        editor.undo();
        assert!(before);
        assert!(!editor.can_undo(), "a no-op edit left a step behind");
    }

    /// Every prop the default pack ships has to load and build cleanly.
    ///
    /// The file format has no schema version and no migration chain — that is
    /// the trade for being flat text a mapper can read — so the thing that
    /// keeps a shipped prop honest is this: open it, replay it, and insist it
    /// comes out as geometry with nothing to complain about. A field renamed
    /// in Rust without the files being brought along fails here rather than as
    /// a weapon that is invisible in somebody's hands.
    ///
    /// `read_dir` rather than a list of names, deliberately: a list is a
    /// second description of the directory, and the one that would be
    /// forgotten. The rule against enumerating a directory is about the
    /// *runtime*, which has to work behind a `fetch`; a test runs on a machine
    /// with the repo on it.
    #[test]
    fn every_prop_the_default_pack_ships_loads_and_builds() {
        let directory = std::path::Path::new("assets/default").join(PROPS_DIR);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            // A pack with no props is an ordinary pack, and this test running
            // from somewhere other than the repo root is not a prop being
            // broken.
            return;
        };

        let mut checked = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(PROP_EXTENSION) {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let doc = parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

            let built = doc.evaluate();
            assert!(
                built.problems.is_empty(),
                "{} does not build: {:?}",
                path.display(),
                built.problems,
            );
            assert!(
                !built.bodies.is_empty(),
                "{} builds nothing at all",
                path.display(),
            );
            for (id, solid) in &built.bodies {
                assert!(
                    solid.volume() > 0.0,
                    "{}: body {id} came out inside out, at {} cubic metres",
                    path.display(),
                    solid.volume(),
                );
            }
            checked += 1;
        }
        assert!(checked > 0, "{} has no props in it", directory.display());
    }

    /// Reading the document must not make the viewport think it has moved on.
    ///
    /// The generation is what the rebuild is keyed on, so a reader that
    /// bumped it would re-run the boolean kernel every frame — which is
    /// exactly what an inspector drawn against `doc_mut` did, and it showed up
    /// as the unsaved-work marker appearing from merely selecting something
    /// rather than as anything obviously wrong.
    #[test]
    fn looking_at_the_document_does_not_change_it() {
        let mut editor = PropEditor::default();
        editor.edit(|doc| doc.push(FeatureOp::default()));
        // Saved first, so "dirty" below means *reading* made it dirty rather
        // than the push that set the scene up.
        editor.mark_saved(PathBuf::from("somewhere.gpp"));
        let generation = editor.generation();

        for _ in 0..10 {
            let _ = editor.doc().features.len();
            editor.edit(|doc| {
                let _ = doc.features.first().map(|feature| feature.id);
            });
        }

        assert_eq!(editor.generation(), generation, "reading rebuilt the prop");
        assert!(!editor.dirty, "reading marked the document unsaved");
    }

    /// Undo can take away the feature the panel is showing.
    #[test]
    fn undoing_past_the_selected_feature_clears_the_selection() {
        let mut editor = PropEditor::default();
        let id = editor.edit(|doc| doc.push(FeatureOp::default()));
        editor.selected = Some(id);
        editor.undo();
        assert_eq!(editor.selected, None);
    }
}
