//! A prop as a document: the feature list, what is selected, and how to get
//! back to what it looked like a moment ago.
//!
//! **Undo is whole-document snapshots**, not per-feature deltas like the map's
//! [`Action`](crate::editor::action::Action). A prop is a few dozen features
//! and a couple of kilobytes, so cloning per edit is free and correct by
//! construction: no before-and-after pair to get the wrong way round, and no
//! operation that can forget to record itself. The price is that history does
//! not survive closing the file, which the map pays for and this does not.
//!
//! **The file is flat text, not SQLite.** A blueprint is an authoring format
//! the runtime never reads; a prop is read by the *game* and has to reach a
//! browser tab, where bundled C does not go. So it comes through
//! [`AssetSource`](crate::common::assets::AssetSource) like `weapons.toml`,
//! and nothing here reaches for a filesystem on the reading side.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::skeleton::clip::ClipSet;
use crate::common::assets::{AssetError, Assets};
use crate::prop::feature::{evaluate, Evaluated, FeatureOp, PropFeature, PropFeatureId};
use crate::prop::hold::{HoldSpec, ViewmodelSpec};

/// Grackle Prop. Flat text, one prop per file.
pub const PROP_EXTENSION: &str = "gpp";

/// Where props live inside a pack, so the weapon loader and the prop editor
/// cannot disagree about it.
pub const PROPS_DIR: &str = "props";

/// How many edits back you can go. Bounded because the stack is whole
/// documents; deep enough that nobody reaches the end in practice.
const UNDO_DEPTH: usize = 256;

/// One prop: an ordered list of modelling features and a name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PropDoc {
    /// A lang key, the way a weapon's is — free text would be display text
    /// outside the lang layer.
    #[serde(default)]
    pub name_key: String,
    #[serde(default)]
    pub features: Vec<PropFeature>,
    /// How this prop is held, if anything holds it. Every prop has one and
    /// almost every prop will keep the default — see [`HoldSpec`].
    #[serde(default)]
    pub hold: HoldSpec,
    /// Where it hangs in first person, which is a different question from how
    /// a body holds it — see [`ViewmodelSpec`].
    #[serde(default)]
    pub viewmodel: ViewmodelSpec,
    /// The next id to hand out. Stored rather than derived from the highest in
    /// use, so deleting the last feature cannot make the next reuse its number.
    #[serde(default)]
    next_id: u32,
    /// What this prop does over time — a fire, a reload, a deploy.
    ///
    /// Here rather than in `weapons.toml` because an animation is stated as a
    /// *deviation from the hold*, and a deviation wants to live beside the
    /// thing it deviates from. It also reaches clients free: a `.gpp` already
    /// crosses the wire as the text the file holds, which is the only way an
    /// internally tagged enum can cross at all.
    ///
    /// **Declared last, and it has to stay last.** A TOML array-of-tables
    /// header captures every key after it, so a field written out below
    /// `[[features]]` or `[[...tracks]]` would be swallowed by it — the rule
    /// `weapons.toml` already documents, and it applies to
    /// `toml::to_string_pretty`'s output exactly as it does to a hand-written
    /// file.
    #[serde(default, skip_serializing_if = "ClipSet::is_empty")]
    pub animations: ClipSet,
}

impl PropDoc {
    pub fn new(name_key: impl Into<String>) -> PropDoc {
        PropDoc {
            name_key: name_key.into(),
            features: vec![],
            hold: HoldSpec::default(),
            viewmodel: ViewmodelSpec::default(),
            next_id: 0,
            animations: ClipSet::default(),
        }
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

    /// Copy a feature in directly after itself and return the copy's id.
    ///
    /// Only something that consumes nothing can be copied: a boolean's copy
    /// would find its operands already eaten by the original and arrive
    /// broken. `None` for those, and for an id that is not here. `name` is
    /// what to call the copy — the caller's, since what "copy" is called is
    /// the lang layer's business, not the document's.
    pub fn duplicate(&mut self, id: PropFeatureId, name: String) -> Option<PropFeatureId> {
        let at = self.index_of(id)?;
        let original = &self.features[at];
        if !original.op.consumes().is_empty() {
            return None;
        }
        let mut copy = original.clone();
        let new = self.take_id();
        copy.id = new;
        copy.name = name;
        self.features.insert(at + 1, copy);
        Some(new)
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

    /// Everything that names `id` as an operand — what deleting it breaks, so
    /// the panel can say so beforehand.
    pub fn dependants(&self, id: PropFeatureId) -> Vec<PropFeatureId> {
        self.features
            .iter()
            .filter(|feature| feature.op.consumes().contains(&id))
            .map(|feature| feature.id)
            .collect()
    }

    /// Where a feature may be moved to, as indices in the finished list.
    ///
    /// **Always a contiguous interval**, which is what makes dragging tractable.
    /// Moving *one* item leaves every other item's relative order alone, so
    /// "after everything I consume" and "before everything that consumes me"
    /// collapse into one window — no scattered set of slots to describe, so the
    /// panel clamps into the range rather than needing to refuse a drop.
    ///
    /// **Direct relations are enough**: an indirect operand is already above a
    /// direct one. Indices count the list **with this feature taken out**,
    /// which is the index it will end up at.
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

        // An already-out-of-order document would give an inverted range, which
        // is a panic waiting in whatever clamps with it.
        Some(low..=high.max(low))
    }

    /// Move a feature to `target`, or as close as the dependencies allow.
    ///
    /// **Clamped rather than refused**, so a drag stops at the last legal place
    /// instead of snapping back or landing somewhere broken. That is the whole
    /// of how an illegal order is prevented.
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

/// The document, the undo stacks, and what file it came from.
#[derive(Resource, Debug)]
pub struct PropEditor {
    doc: PropDoc,
    undo: Vec<PropDoc>,
    redo: Vec<PropDoc>,
    /// What the document looked like when a gesture began, pushed onto the
    /// undo stack only if it changed anything. egui reports a drag as an edit
    /// per frame, so without this one drag would be forty undo steps.
    pending: Option<PropDoc>,
    /// The **primary** selection: what the inspector shows and the gizmos
    /// drag. The last thing picked.
    pub selected: Option<PropFeatureId>,
    /// Everything picked before the primary with Ctrl/Cmd held, oldest first.
    ///
    /// **Selection is ordered** because the verbs that take two operands care
    /// which is which: pick the barrel, then the bore, and *Subtract* cuts the
    /// bore out of the barrel — the order it is said in. Kept apart from
    /// `selected` rather than folded into one list so the many readers that
    /// only want "the thing being edited" go on reading one field.
    pub also_selected: Vec<PropFeatureId>,
    pub path: Option<PathBuf>,
    /// Whether the document differs from what is on disk.
    pub dirty: bool,
    /// Bumped on every change, so the viewport can tell without comparing two
    /// feature lists.
    generation: u64,
    /// Bumped on every successful save, which is what tells the *game* to pick
    /// the model up — see `publish_the_saved_prop`.
    saves: u64,
}

impl Default for PropEditor {
    fn default() -> Self {
        PropEditor {
            doc: PropDoc::new("prop.untitled"),
            undo: vec![],
            redo: vec![],
            pending: None,
            selected: None,
            also_selected: vec![],
            path: None,
            dirty: false,
            generation: 0,
            saves: 0,
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

    /// How many times this document has been written out.
    pub fn saves(&self) -> u64 {
        self.saves
    }

    /// The name a weapon would know this prop by, which is its file's stem —
    /// the same name [`load`] turns back into a path. A document that has
    /// never been saved has none, and so is nothing the game could be holding.
    pub fn saved_name(&self) -> Option<&str> {
        self.path.as_ref()?.file_stem()?.to_str()
    }

    /// Edit the document, recording an undo step if anything changed.
    ///
    /// Everything that writes goes through here, which makes "can this be
    /// undone" a property of the type rather than of what each call site
    /// remembered.
    pub fn edit<T>(&mut self, change: impl FnOnce(&mut PropDoc) -> T) -> T {
        let before = self.doc.clone();
        let result = change(&mut self.doc);
        if self.doc != before {
            self.push_undo(before);
        }
        result
    }

    /// Begin an edit spanning several frames, such as a drag. Between this and
    /// [`PropEditor::end_gesture`] the document may be written to freely and
    /// only one undo step comes of it.
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
        // A fresh edit ends the redo branch.
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
        // Undo can take the feature the selection names away.
        self.forget_missing();
        self.doc_changed();
    }

    /// Everything selected, in the order it was picked; the primary last.
    pub fn selection(&self) -> Vec<PropFeatureId> {
        self.also_selected.iter().copied().chain(self.selected).collect()
    }

    /// Select one thing and nothing else.
    pub fn select(&mut self, id: PropFeatureId) {
        self.also_selected.clear();
        self.selected = Some(id);
    }

    pub fn clear_selection(&mut self) {
        self.also_selected.clear();
        self.selected = None;
    }

    /// Ctrl/Cmd-click: add `id` as the new primary, or take it out if it was
    /// already picked. Taking out the primary promotes the most recent pick
    /// before it, so the inspector lands on something that is still selected.
    pub fn toggle_selected(&mut self, id: PropFeatureId) {
        if self.selected == Some(id) {
            self.selected = self.also_selected.pop();
        } else if let Some(at) = self.also_selected.iter().position(|picked| *picked == id) {
            self.also_selected.remove(at);
        } else {
            self.also_selected.extend(self.selected);
            self.selected = Some(id);
        }
    }

    /// Drop anything selected that the document no longer has.
    pub fn forget_missing(&mut self) {
        let doc = &self.doc;
        self.also_selected.retain(|id| doc.feature(*id).is_some());
        if self.selected.is_some_and(|id| doc.feature(id).is_none()) {
            self.selected = self.also_selected.pop();
        }
    }

    /// The two bodies a boolean would take from the selection, as
    /// `(target, tool)`: exactly two picked, both standing at the end of the
    /// list. A boolean is appended, so a body already eaten is not something
    /// it could take — offering it would add a feature that is broken on
    /// arrival.
    pub fn pair_to_combine(&self, live: &[PropFeatureId]) -> Option<(PropFeatureId, PropFeatureId)> {
        match self.selection()[..] {
            [target, tool] if live.contains(&target) && live.contains(&tool) => Some((target, tool)),
            _ => None,
        }
    }

    /// The one body a mirror or a repaint would take: the primary, if it is
    /// still standing. Single-operand verbs read the primary alone, so a
    /// selection of three still mirrors the one in the inspector.
    pub fn body_to_act_on(&self, live: &[PropFeatureId]) -> Option<PropFeatureId> {
        self.selected.filter(|id| live.contains(id))
    }

    /// Replace the document wholesale, clearing the history: undoing back
    /// *through* a file open is not what anybody means by Ctrl+Z.
    pub fn open(&mut self, doc: PropDoc, path: Option<PathBuf>) {
        self.doc = doc;
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        self.clear_selection();
        self.path = path;
        self.generation += 1;
        self.dirty = false;
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.dirty = false;
        self.saves += 1;
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

/// Read a prop out of a pack, through [`Assets`] rather than `std::fs` because
/// the *game* does this. The editor's file dialog goes through [`parse`].
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

/// Write a prop to disk — the one `std::fs` here, and on the *authoring* side.
/// Nothing the game does at runtime comes through it.
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

    /// Whether a prop survives being closed. A field that silently defaults is
    /// a prop that changes shape on load.
    #[test]
    fn a_document_round_trips_through_its_file_format() {
        let doc = sample();
        let text = to_text(&doc).expect("a prop serialises");
        let back = parse(&text).unwrap_or_else(|e| panic!("{e}\n---\n{text}"));
        assert_eq!(doc, back);
    }

    /// A format that rounded would move a bore off-centre by too little to see
    /// and enough to leave a sliver of metal inside the barrel.
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

    /// A clip lives in the same file as the geometry, below an
    /// `[[features]]` array of tables — and a TOML array-of-tables header
    /// captures **every key after it**, so a field written out in the wrong
    /// place is silently swallowed into the last feature.
    ///
    /// That failure does not error and does not even lose the file: it loads,
    /// the geometry is fine, and the animations are simply gone. Worth a test
    /// of its own rather than trusting the field order in the struct, because
    /// the thing that decides it is `toml::to_string_pretty`'s own idea of
    /// what to emit when.
    #[test]
    fn animations_survive_the_round_trip_beside_the_features() {
        use crate::common::skeleton::clip::{
            Axis, Channel, Clip, Clock, Combine, Key, Space, Track, Unit,
        };
        use crate::common::skeleton::Interpolation;

        let mut doc = sample();
        assert!(!doc.features.is_empty(), "the point is a clip *after* features");

        doc.animations.clips.insert(
            "deploy".into(),
            Clip {
                name_key: "prop.animations.deploy".into(),
                length: 0.45,
                clock: Clock::Seconds,
                looping: false,
                note: "the launcher swings up as it comes out".into(),
                tracks: vec![Track {
                    combine: Combine::Compose,
                    space: Space::Carry,
                    unit: Unit::ArmLengths,
                    enabled: true,
                    channel: Channel::Weapon {
                        keys: vec![
                            Key {
                                at: 0.0,
                                to: [Axis::Free, Axis::Value(-0.8), Axis::Free],
                                ease: Interpolation::CubicOut,
                            },
                            Key {
                                at: 0.45,
                                to: [Axis::Free, Axis::Value(0.0), Axis::Free],
                                ease: Interpolation::Linear,
                            },
                        ],
                    },
                }],
            },
        );

        let text = to_text(&doc).expect("a prop with a clip should serialise");
        let back = parse(&text).expect("a prop with a clip should come back");

        assert_eq!(
            doc.animations, back.animations,
            "the clip did not survive the file:\n{text}"
        );
        assert_eq!(doc.features.len(), back.features.len(), "the features changed");
    }

    /// A reused id would make a reference mean a different shape.
    #[test]
    fn deleting_the_last_feature_does_not_free_its_id() {
        let mut doc = PropDoc::new("x");
        let first = doc.push(FeatureOp::default());
        doc.remove(first);
        let second = doc.push(FeatureOp::default());
        assert_ne!(first, second);
    }

    /// Counted by **who names it**, not by who comes after it.
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

    /// The claim the drag interaction rests on: one unbroken span.
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

    /// A drag past a dependency stops at the last legal place rather than
    /// snapping back — you get the nearest thing you asked for.
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

    /// A legal move has to happen, or the clamp never moves anything.
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

    /// Checked by re-evaluating, because "legal" means exactly "the evaluator
    /// finds everything it needs".
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
        // Every feature to every slot, including the ones nobody would try.
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

    /// One gesture is one undo step, or Ctrl+Z becomes a way to watch a slider
    /// move backwards.
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

    /// An edit that changed nothing must not push a step.
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
    /// The format has no schema version and no migration chain — the trade for
    /// being text a mapper can read — so this is what keeps a shipped prop
    /// honest: open it, replay it, insist it comes out as geometry. A field
    /// renamed without the files brought along fails here rather than as a
    /// weapon invisible in somebody's hands.
    ///
    /// `read_dir` rather than a list of names, which would be a second
    /// description of the directory. The no-enumeration rule is about the
    /// *runtime*, which has to work behind a `fetch`.
    #[test]
    fn every_prop_the_default_pack_ships_loads_and_builds() {
        let directory = std::path::Path::new("assets/default").join(PROPS_DIR);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            // Running from somewhere other than the repo root is not a prop
            // being broken.
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

    /// The same guard, one layer up: every clip every shipped prop carries has
    /// to sample cleanly, on every build, all the way through.
    ///
    /// Here for exactly the reason the geometry guard is here. There is no
    /// schema version and no migration chain, so replaying the shipped files
    /// *is* the check — and a clip fails in a way nobody notices. A bone
    /// dropped for being misspelled is one hand that does not move; a unit
    /// renamed is a weapon somewhere it has never been. Neither errors.
    ///
    /// Sampled across the whole length rather than at the keys, because the
    /// keys are the part somebody looked at.
    #[test]
    fn every_clip_the_default_pack_ships_samples_on_every_class() {
        use crate::common::class::Class;
        use crate::common::skeleton::play::{apply, Frames};
        use crate::common::skeleton::rig::humanoid;
        use crate::common::skeleton::Pose;
        use strum::IntoEnumIterator;

        let directory = std::path::Path::new("assets/default").join(PROPS_DIR);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            return;
        };

        let mut checked = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(PROP_EXTENSION) {
                continue;
            }
            let doc = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let named = path.display();

            for (name, clip) in &doc.animations.clips {
                for class in Class::iter() {
                    let rig = humanoid(class.proportions());
                    let problems = clip.problems(|bone| rig.index_of(bone).is_some());
                    assert!(
                        problems.is_empty(),
                        "{named}: clip '{name}' on {class:?}: {problems:?}",
                    );

                    let ground = |_: Vec3| 0.0;
                    let frames = Frames {
                        body: Transform::IDENTITY,
                        carry: Transform::IDENTITY,
                        weapon: Some(Transform::IDENTITY),
                        eye: None,
                        ground: &ground,
                    };

                    // Past the end as well as through the middle: a one-shot
                    // holds there, and holding is where a clip that runs off
                    // the end of its own keys shows up.
                    for step in 0..=120 {
                        let time = clip.length * step as f32 / 100.0;
                        let mut pose = Pose::rest();
                        let mut weapon = Transform::IDENTITY;
                        apply(clip, time, &rig, &mut pose, &frames, Some(&mut weapon));

                        for (bone, rotation) in pose.posed_joints() {
                            assert!(
                                rotation.is_finite() && rotation.is_normalized(),
                                "{named}: clip '{name}' on {class:?} at {time}s left \
                                 '{bone}' at {rotation:?}",
                            );
                        }
                        assert!(
                            weapon.translation.is_finite() && weapon.rotation.is_finite(),
                            "{named}: clip '{name}' on {class:?} at {time}s put the \
                             weapon at {weapon:?}",
                        );
                    }
                }
                checked += 1;
            }
        }

        // Zero is a pass here rather than a failure: not every prop has to
        // move. It is the geometry guard above that insists the pack is not
        // empty.
        let _ = checked;
    }

    /// The generation is what the rebuild is keyed on, so a reader that bumped
    /// it would re-run the kernel every frame. An inspector drawn against
    /// `doc_mut` did exactly that, and it showed as the unsaved-work marker
    /// appearing from merely selecting something.
    #[test]
    fn looking_at_the_document_does_not_change_it() {
        let mut editor = PropEditor::default();
        editor.edit(|doc| doc.push(FeatureOp::default()));
        // Saved first, so "dirty" below means reading made it so.
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

    /// Picking the barrel then the bore subtracts the bore from the barrel —
    /// the order it was said in, whichever way round they sit in the list.
    #[test]
    fn a_pair_is_taken_in_the_order_it_was_picked() {
        let mut editor = PropEditor::default();
        let barrel = editor.edit(|doc| doc.push(FeatureOp::default()));
        let bore = editor.edit(|doc| doc.push(FeatureOp::default()));
        let live = [barrel, bore];

        editor.select(bore);
        editor.toggle_selected(barrel);
        assert_eq!(editor.pair_to_combine(&live), Some((bore, barrel)));

        editor.select(barrel);
        editor.toggle_selected(bore);
        assert_eq!(editor.pair_to_combine(&live), Some((barrel, bore)));
        assert_eq!(editor.selected, Some(bore), "the last pick is the one the inspector shows");
    }

    /// Only two standing bodies make a pair: one is not a boolean, three is a
    /// question about which two, and an eaten body cannot be taken again.
    #[test]
    fn a_pair_needs_exactly_two_standing_bodies() {
        let mut editor = PropEditor::default();
        let a = editor.edit(|doc| doc.push(FeatureOp::default()));
        let b = editor.edit(|doc| doc.push(FeatureOp::default()));
        let c = editor.edit(|doc| doc.push(FeatureOp::default()));

        editor.select(a);
        assert_eq!(editor.pair_to_combine(&[a, b, c]), None);
        editor.toggle_selected(b);
        assert_eq!(editor.pair_to_combine(&[a, c]), None, "b has been eaten");
        editor.toggle_selected(c);
        assert_eq!(editor.pair_to_combine(&[a, b, c]), None);
    }

    /// Ctrl-clicking the primary again takes it out and hands the inspector
    /// to the pick before it, rather than to nothing.
    #[test]
    fn unpicking_the_primary_promotes_the_one_before() {
        let mut editor = PropEditor::default();
        let a = editor.edit(|doc| doc.push(FeatureOp::default()));
        let b = editor.edit(|doc| doc.push(FeatureOp::default()));

        editor.select(a);
        editor.toggle_selected(b);
        editor.toggle_selected(b);
        assert_eq!(editor.selection(), vec![a]);
        assert_eq!(editor.selected, Some(a));
    }

    /// A copy lands right after the original, under a new id; a boolean
    /// cannot be copied, because its copy would have nothing left to eat.
    #[test]
    fn duplicating_copies_in_place_and_refuses_what_consumes() {
        let mut doc = PropDoc::new("test");
        let a = doc.push(FeatureOp::default());
        let b = doc.push(FeatureOp::default());
        let cut = doc.push(FeatureOp::Boolean { op: BooleanOp::Subtract, target: a, tool: b });
        let problems_before = doc.evaluate().problems.len();

        let copy = doc.duplicate(a, "A copy".into()).expect("a primitive copies");
        assert_eq!(doc.index_of(copy), Some(1));
        assert_ne!(copy, a);
        assert_eq!(doc.feature(copy).unwrap().op, doc.feature(a).unwrap().op);
        assert_eq!(doc.duplicate(cut, "no".into()), None);
        assert_eq!(doc.evaluate().problems.len(), problems_before, "the copy broke nothing");
    }

    /// Undo can take away a secondary pick as well as the primary.
    #[test]
    fn undo_drops_every_pick_it_takes_away() {
        let mut editor = PropEditor::default();
        let a = editor.edit(|doc| doc.push(FeatureOp::default()));
        let b = editor.edit(|doc| doc.push(FeatureOp::default()));
        editor.select(b);
        editor.toggle_selected(a);
        editor.undo();
        assert_eq!(editor.selection(), vec![a]);
    }
}
