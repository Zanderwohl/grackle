//! A prop as a document: the feature list, the selection, and undo.
//!
//! Undo is whole-document snapshots; a prop is a couple of kilobytes, so
//! cloning per edit is cheap. History does not survive closing the file.
//!
//! The game reads props, so reading goes through
//! [`AssetSource`](crate::common::assets::AssetSource) and never `std::fs`,
//! which a browser tab does not have.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::skeleton::clip::ClipSet;
use crate::common::assets::{AssetError, Assets};
use crate::prop::feature::{evaluate, Evaluated, FeatureOp, PropFeature, PropFeatureId};
use crate::prop::archetype::{Archetype, BreakableSpec, Collider, HatSpec, PickupSpec};
use crate::prop::hold::{HoldSpec, MuzzleSpec, ViewmodelSpec};
use crate::prop::reference::{GeomRef, Element};
use crate::prop::plane::PlaneRef;

/// Grackle Prop: TOML, one prop per file.
pub const PROP_EXTENSION: &str = "gpp";

/// Where props live inside a pack.
pub const PROPS_DIR: &str = "props";

/// Something picked: a whole body, a named corner, edge or face of one, or a
/// plane feature.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Pick {
    Body(PropFeatureId),
    Geom(GeomRef),
    Plane(PlaneRef),
}

impl Pick {
    /// The standing body this pick stands for, if any. A face stands for the
    /// body it is on, so picking a face on the barrel and one on the bore still
    /// offers Combine; a corner or an edge does not, since one on a seam
    /// belongs to two bodies' worth of meaning.
    pub fn body(&self, evaluated: &Evaluated) -> Option<PropFeatureId> {
        match self {
            Pick::Body(id) => evaluated.bodies.iter().any(|(body, _)| body == id).then_some(*id),
            Pick::Geom(reference @ GeomRef { element: Element::Face(_), .. }) => evaluated.body_of(reference),
            Pick::Geom(_) | Pick::Plane(_) => None,
        }
    }

    /// See [`PropEditor::primary_feature`].
    pub fn feature(&self, evaluated: &Evaluated) -> Option<PropFeatureId> {
        match self {
            Pick::Body(id) | Pick::Plane(PlaneRef::Feature(id)) => Some(*id),
            Pick::Plane(PlaneRef::Origin(_)) => None,
            Pick::Geom(_) => self.body(evaluated),
        }
    }

    /// Every feature the pick names; if any goes, so does the pick.
    pub fn features(&self) -> Box<dyn Iterator<Item = PropFeatureId> + '_> {
        match self {
            Pick::Body(id) | Pick::Plane(PlaneRef::Feature(id)) => Box::new(std::iter::once(*id)),
            Pick::Plane(PlaneRef::Origin(_)) => Box::new(std::iter::empty()),
            Pick::Geom(reference) => Box::new(reference.source.features()),
        }
    }
}

/// Bounded because each step is a whole document.
const UNDO_DEPTH: usize = 256;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PropDoc {
    /// A lang key, not display text.
    #[serde(default)]
    pub name_key: String,
    #[serde(default)]
    pub features: Vec<PropFeature>,
    #[serde(default)]
    pub hold: HoldSpec,
    /// First-person framing; not the same as [`HoldSpec`]'s carry.
    #[serde(default)]
    pub viewmodel: ViewmodelSpec,
    /// Absent: shots are drawn from where they really leave, the eye.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muzzle: Option<MuzzleSpec>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub archetype: Archetype,
    /// What a placed prop collides as, or is picked up by. Authored apart from
    /// the geometry: a crate with a handle cut in it is still a box to walk into.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colliders: Vec<Collider>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub breakable: BreakableSpec,
    #[serde(default, skip_serializing_if = "is_default")]
    pub pickup: PickupSpec,
    #[serde(default, skip_serializing_if = "is_default")]
    pub hat: HatSpec,
    /// Stored rather than derived from the highest id in use, so deleting the
    /// last feature cannot make the next one reuse its id.
    #[serde(default)]
    next_id: u32,
    /// Fire, reload, deploy. Here rather than in `weapons.toml` because a clip
    /// is stated relative to the hold.
    ///
    /// Must stay the last field: a TOML array-of-tables header captures every
    /// key after it, so a field serialised below `[[features]]` would be read
    /// back into the last feature.
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
            muzzle: None,
            archetype: Archetype::default(),
            colliders: vec![],
            breakable: BreakableSpec::default(),
            pickup: PickupSpec::default(),
            hat: HatSpec::default(),
            next_id: 0,
            animations: ClipSet::default(),
        }
    }

    fn take_id(&mut self) -> PropFeatureId {
        let id = PropFeatureId(self.next_id);
        self.next_id += 1;
        id
    }

    /// `at` is clamped to the end.
    pub fn insert_at(&mut self, at: usize, op: FeatureOp) -> PropFeatureId {
        let id = self.take_id();
        let at = at.min(self.features.len());
        self.features.insert(at, PropFeature::new(id, op));
        id
    }

    pub fn push(&mut self, op: FeatureOp) -> PropFeatureId {
        let id = self.take_id();
        self.features.push(PropFeature::new(id, op));
        id
    }

    /// Copy a feature in directly after itself. `None` for a feature that
    /// consumes anything: its copy would find its operands already eaten.
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

    /// Everything that names `id`, as an operand or a reference.
    pub fn dependants(&self, id: PropFeatureId) -> Vec<PropFeatureId> {
        self.features
            .iter()
            .filter(|feature| feature.op.depends_on().contains(&id))
            .map(|feature| feature.id)
            .collect()
    }

    /// Where a feature may be moved to. Always contiguous, since moving one
    /// item leaves the others' order alone. Direct relations are enough: an
    /// indirect operand is already above a direct one.
    ///
    /// Indices count the list with this feature taken out, which is the index
    /// it will end up at.
    pub fn legal_range(&self, id: PropFeatureId) -> Option<std::ops::RangeInclusive<usize>> {
        let at = self.index_of(id)?;
        let feature = self.feature(id)?;

        let without = |i: usize| if i < at { i } else { i - 1 };

        let mut low = 0;
        for operand in feature.op.depends_on() {
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

        // An out-of-order document would give an inverted range, and `clamp`
        // panics on one.
        Some(low..=high.max(low))
    }

    /// Clamped into [`Self::legal_range`] rather than refused, so a drag stops
    /// at the last legal place. Returns whether anything moved.
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

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

#[derive(Resource, Debug)]
pub struct PropEditor {
    doc: PropDoc,
    undo: Vec<PropDoc>,
    redo: Vec<PropDoc>,
    /// The document as a gesture began. egui reports a drag as an edit per
    /// frame, so this makes one drag one undo step.
    pending: Option<PropDoc>,
    /// Oldest first; the last is the primary, which the inspector and the
    /// gizmos show. Ordered because a boolean's first pick is its target and
    /// a plane's points are taken in the order they were picked.
    picks: Vec<Pick>,
    /// The rollback bar: the last feature evaluated, `None` for all. An id
    /// rather than a count so edits above it cannot move it. View state: not
    /// saved, undone or dirtying.
    rollback: Option<PropFeatureId>,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    /// Bumped on every change, as a cue to look; the viewport compares the
    /// features it built from before re-running the kernel.
    generation: u64,
    /// Bumped on every save; `publish_the_saved_prop` watches it.
    saves: u64,
}

impl Default for PropEditor {
    fn default() -> Self {
        PropEditor {
            doc: PropDoc::new("prop.untitled"),
            undo: vec![],
            redo: vec![],
            pending: None,
            picks: vec![],
            rollback: None,
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

    pub fn saves(&self) -> u64 {
        self.saves
    }

    /// The file stem, which is the name [`load`] takes.
    pub fn saved_name(&self) -> Option<&str> {
        self.path.as_ref()?.file_stem()?.to_str()
    }

    /// Records an undo step if anything changed.
    pub fn edit<T>(&mut self, change: impl FnOnce(&mut PropDoc) -> T) -> T {
        let before = self.doc.clone();
        let result = change(&mut self.doc);
        if self.doc != before {
            self.push_undo(before);
        }
        result
    }

    /// Until [`PropEditor::end_gesture`], writes through [`Self::doc_mut`]
    /// make one undo step.
    pub fn begin_gesture(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(self.doc.clone());
        }
    }

    /// Only inside a gesture. Marks the document changed even if nothing is.
    pub fn doc_mut(&mut self) -> &mut PropDoc {
        self.doc_changed();
        &mut self.doc
    }

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
        self.forget_missing();
        self.doc_changed();
    }

    pub fn rollback(&self) -> Option<PropFeatureId> {
        self.rollback
    }

    /// A bar at the last feature is stored as `None`, so "everything" has one
    /// representation. Does not dirty the document.
    pub fn roll_back_to(&mut self, at: Option<PropFeatureId>) {
        let at = at.filter(|id| {
            self.doc.features.last().is_some_and(|last| last.id != *id) && self.doc.feature(*id).is_some()
        });
        if at != self.rollback {
            self.rollback = at;
            self.generation += 1;
        }
    }

    /// The features down to the rollback bar.
    pub fn shown(&self) -> &[PropFeature] {
        match self.rollback.and_then(|id| self.doc.index_of(id)) {
            Some(at) => &self.doc.features[..=at],
            None => &self.doc.features,
        }
    }

    /// Inserts at the rollback bar and moves the bar onto the new feature, or
    /// appends with no bar.
    pub fn add(&mut self, op: FeatureOp) -> PropFeatureId {
        let anchor = self.rollback;
        let id = self.edit(|doc| match anchor.and_then(|anchor| doc.index_of(anchor)) {
            Some(at) => doc.insert_at(at + 1, op),
            None => doc.push(op),
        });
        if anchor.is_some() {
            self.roll_back_to(Some(id));
        }
        id
    }

    /// In pick order; the primary last.
    pub fn picks(&self) -> &[Pick] {
        &self.picks
    }

    pub fn primary(&self) -> Option<&Pick> {
        self.picks.last()
    }

    /// The feature the primary is about, for the inspector, the handles and
    /// the feature list: a body or plane itself, eaten or not, or the body a
    /// face is on.
    pub fn primary_feature(&self, evaluated: &Evaluated) -> Option<PropFeatureId> {
        self.primary().and_then(|pick| pick.feature(evaluated))
    }

    /// [`Self::primary_feature`] for every pick, in pick order.
    pub fn picked_features(&self, evaluated: &Evaluated) -> Vec<PropFeatureId> {
        self.picks.iter().filter_map(|pick| pick.feature(evaluated)).collect()
    }

    /// The body the primary stands for; see [`Pick::body`].
    pub fn primary_body(&self, evaluated: &Evaluated) -> Option<PropFeatureId> {
        self.primary()?.body(evaluated)
    }

    /// The bodies the picks stand for, in pick order, repeats kept.
    pub fn picked_bodies(&self, evaluated: &Evaluated) -> Vec<PropFeatureId> {
        self.picks.iter().filter_map(|pick| pick.body(evaluated)).collect()
    }

    pub fn select(&mut self, id: PropFeatureId) {
        self.picks = vec![Pick::Body(id)];
    }

    pub fn select_pick(&mut self, pick: Pick) {
        self.picks = vec![pick];
    }

    pub fn clear_selection(&mut self) {
        self.picks.clear();
    }

    /// Picked again, it is unpicked; the pick before it becomes the primary.
    pub fn toggle(&mut self, pick: Pick) {
        match self.picks.iter().position(|picked| *picked == pick) {
            Some(at) => {
                self.picks.remove(at);
            }
            None => self.picks.push(pick),
        }
    }

    pub fn toggle_selected(&mut self, id: PropFeatureId) {
        self.toggle(Pick::Body(id));
    }

    pub fn unpick(&mut self, pick: &Pick) {
        self.picks.retain(|picked| picked != pick);
    }

    /// Drop picks and the rollback bar that name features no longer here. A
    /// reference whose features are all here stays picked even if it does not
    /// resolve, since rolling forward again may bring it back.
    pub fn forget_missing(&mut self) {
        if self.rollback.is_some_and(|id| self.doc.feature(id).is_none()) {
            self.rollback = None;
            self.generation += 1;
        }
        let doc = &self.doc;
        self.picks.retain(|pick| pick.features().all(|id| doc.feature(id).is_some()));
    }

    /// `(target, tool)` when exactly two picks stand for two different
    /// standing bodies; an eaten body would make a boolean broken on arrival.
    pub fn pair_to_combine(&self, evaluated: &Evaluated) -> Option<(PropFeatureId, PropFeatureId)> {
        match self.picks[..] {
            [ref target, ref tool] => {
                let (target, tool) = (target.body(evaluated)?, tool.body(evaluated)?);
                (target != tool).then_some((target, tool))
            }
            _ => None,
        }
    }

    /// Single-operand verbs take the body the primary stands for.
    pub fn body_to_act_on(&self, evaluated: &Evaluated) -> Option<PropFeatureId> {
        self.primary_body(evaluated)
    }

    /// Go back to `saved`, the file's contents, as one undo step. Unlike
    /// [`Self::open`], which clears history.
    pub fn revert_to(&mut self, saved: PropDoc) {
        // An open gesture would fold the revert into it.
        self.end_gesture();
        self.edit(|doc| *doc = saved);
        self.forget_missing();
        self.dirty = false;
    }

    /// Replace the document and clear the history.
    pub fn open(&mut self, doc: PropDoc, path: Option<PathBuf>) {
        self.doc = doc;
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        self.clear_selection();
        self.rollback = None;
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

/// Through [`Assets`] rather than `std::fs` because the game reads props.
/// The editor's file dialog goes through [`parse`].
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

/// Authoring side only; the one `std::fs` in this file.
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
    use crate::prop::feature::{Axis, BooleanOp, PatternKind};
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

    /// A field that silently defaults changes the prop's shape on load.
    #[test]
    fn a_document_round_trips_through_its_file_format() {
        let doc = sample();
        let text = to_text(&doc).expect("a prop serialises");
        let back = parse(&text).unwrap_or_else(|e| panic!("{e}\n---\n{text}"));
        assert_eq!(doc, back);
    }

    /// The nested tag is the kind of thing TOML gets wrong quietly.
    #[test]
    fn a_pattern_round_trips() {
        let mut doc = PropDoc::new("x");
        let a = doc.push(FeatureOp::default());
        let row = doc.push(FeatureOp::Pattern {
            target: a,
            count: 3,
            along: PatternKind::Linear { step: [0.0, 0.0, 0.05] },
        });
        doc.push(FeatureOp::Pattern {
            target: row,
            count: 6,
            along: PatternKind::Radial { axis: Axis::Z, centre: [0.0, 0.1, 0.0], degrees: 360.0 },
        });
        let text = to_text(&doc).unwrap();
        assert_eq!(parse(&text).unwrap(), doc, "{text}");
    }

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

    /// Fails if the animations are serialised under `[[features]]` and
    /// swallowed by the last feature; the load still succeeds.
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
                    timing: Default::default(),
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

    #[test]
    fn deleting_the_last_feature_does_not_free_its_id() {
        let mut doc = PropDoc::new("x");
        let first = doc.push(FeatureOp::default());
        doc.remove(first);
        let second = doc.push(FeatureOp::default());
        assert_ne!(first, second);
    }

    /// Counted by who names it, not by who comes after it.
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

    #[test]
    fn what_a_feature_may_be_moved_to_is_one_unbroken_range() {
        let mut doc = PropDoc::new("x");
        let a = doc.push(FeatureOp::default());
        let b = doc.push(FeatureOp::default());
        let cut = doc.push(FeatureOp::Boolean { op: BooleanOp::Subtract, target: a, tool: b });
        let spare = doc.push(FeatureOp::default());

        // Indices count the list with the moved feature lifted out.
        assert_eq!(doc.legal_range(cut), Some(2..=3));
        assert_eq!(doc.legal_range(a), Some(0..=1));
        assert_eq!(doc.legal_range(spare), Some(0..=3));
    }

    #[test]
    fn a_drag_past_a_dependency_stops_against_it() {
        let mut doc = PropDoc::new("x");
        let a = doc.push(FeatureOp::default());
        let b = doc.push(FeatureOp::default());
        let cut = doc.push(FeatureOp::Boolean { op: BooleanOp::Subtract, target: a, tool: b });
        doc.push(FeatureOp::default());

        doc.move_to(cut, 0);
        assert_eq!(doc.index_of(cut), Some(2), "the boolean got above its operands");
        assert_eq!(doc.index_of(a), Some(0));
        assert_eq!(doc.index_of(b), Some(1));

        doc.move_to(b, 3);
        assert_eq!(doc.index_of(b), Some(1));
        assert_eq!(doc.index_of(cut), Some(2));
    }

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

    /// "Legal" means the evaluator finds everything it needs, so re-evaluate.
    #[test]
    fn no_sequence_of_drags_can_break_a_prop() {
        // Offset, because subtracting identical boxes leaves nothing, which
        // is a problem this test is not looking for.
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

    /// Fails if a placed or worn prop's sections stop reading back: the
    /// shipped examples are what the game is walked up to and checked with.
    #[test]
    fn the_shipped_examples_are_what_they_say_they_are() {
        use crate::prop::archetype::{Archetype, PickupKind};
        let read = |name: &str| {
            let path = std::path::Path::new("assets/default").join(PROPS_DIR).join(format!("{name}.{PROP_EXTENSION}"));
            std::fs::read_to_string(&path).ok().map(|text| parse(&text).unwrap())
        };
        // Not run from the repo root.
        let Some(hat) = read("top_hat") else { return };
        assert_eq!(hat.archetype, Archetype::Hat);
        assert!(hat.hat.offset[1] < 0.0, "the hat floats above the head");

        let crate_ = read("crate").unwrap();
        assert_eq!(crate_.archetype, Archetype::Breakable);
        assert_eq!(crate_.colliders.len(), 1);
        assert_eq!(crate_.breakable.health, 75);

        for (name, kind) in [("health_pack", PickupKind::Health), ("ammo_pack", PickupKind::Ammo)] {
            let pack = read(name).unwrap();
            assert_eq!(pack.archetype, Archetype::Pickup, "{name}");
            assert_eq!(pack.pickup.kind, kind, "{name}");
            assert!(!pack.colliders.is_empty(), "{name} has nothing to walk into");
        }

        let rifle = read("sniper_rifle").unwrap();
        assert_eq!(rifle.archetype, Archetype::Held, "a weapon written before archetypes is not held");
    }

    /// The new sections round-trip, and a held prop's file gains none of them.
    #[test]
    fn archetype_sections_round_trip_and_a_held_prop_writes_none() {
        use crate::prop::archetype::*;
        let held = PropDoc::new("prop.untitled");
        let text = to_text(&held).unwrap();
        for section in ["archetype", "colliders", "breakable", "pickup", "hat"] {
            assert!(!text.contains(section), "a held prop wrote {section}:\n{text}");
        }

        let mut doc = PropDoc::new("prop.untitled");
        doc.push(FeatureOp::Primitive {
            shape: crate::prop::solid::Shape::Box { size: [0.5; 3] },
            placement: Default::default(),
            surface: Default::default(),
        });
        doc.archetype = Archetype::Pickup;
        doc.colliders = vec![
            Collider { origin: [0.0, 0.1, 0.0], shape: ColliderShape::Capsule { radius: 0.2, height: 1.0 } },
            Collider { origin: [0.0; 3], shape: ColliderShape::Sphere { radius: 0.05 } },
        ];
        doc.breakable.health = 30;
        doc.pickup = PickupSpec { kind: PickupKind::Ammo, amount: 0.25, respawn: 15.0 };
        doc.hat.offset = [0.0, -0.05, 0.01];
        doc.hat.per_class.insert("heavy".into(), HatOverride { scale: Some(1.1), ..default() });

        let text = to_text(&doc).unwrap();
        assert_eq!(parse(&text).unwrap(), doc, "did not round-trip:\n{text}");
        assert!(text.contains("0.05"), "numbers were not written as typed:\n{text}");
    }

    /// There is no schema version, so a field renamed without updating the
    /// shipped files fails here instead of as an invisible weapon. `read_dir`
    /// is fine in a test; the no-enumeration rule is for the runtime.
    #[test]
    fn every_prop_the_default_pack_ships_loads_and_builds() {
        let directory = std::path::Path::new("assets/default").join(PROPS_DIR);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            // Not run from the repo root.
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

    /// Fails on a misspelled bone or a renamed unit in a shipped clip, neither
    /// of which errors at runtime. Sampled across the whole length, not just
    /// at the keys.
    #[test]
    fn every_clip_the_default_pack_ships_samples_on_every_class() {
        use crate::common::class::Class;
        use crate::common::skeleton::play::{apply, Frames};
        use crate::common::skeleton::rig::humanoid;
        use crate::common::skeleton::Pose;
        use strum::IntoEnumIterator;

        // Past the end, because a one-shot holds there.
        const OVERSHOOT: f32 = 1.2;
        const STEPS: u32 = 120;

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
                    let rig = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
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
                        footfalls: None,
                    };

                    for step in 0..=STEPS {
                        let time = clip.length * OVERSHOOT * step as f32 / STEPS as f32;
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

        // Zero is a pass: not every prop has clips.
        let _ = checked;
    }

    /// A read that bumps the generation re-runs the kernel every frame and
    /// marks the document unsaved.
    #[test]
    fn looking_at_the_document_does_not_change_it() {
        let mut editor = PropEditor::default();
        editor.edit(|doc| doc.push(FeatureOp::default()));
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

    #[test]
    fn undoing_past_the_selected_feature_clears_the_selection() {
        let mut editor = PropEditor::default();
        let id = editor.edit(|doc| doc.push(FeatureOp::default()));
        editor.select(id);
        editor.undo();
        assert_eq!(editor.primary(), None);
    }

    /// Pick order decides target and tool, not list order.
    #[test]
    fn a_pair_is_taken_in_the_order_it_was_picked() {
        let mut editor = PropEditor::default();
        let barrel = editor.edit(|doc| doc.push(FeatureOp::default()));
        let bore = editor.edit(|doc| doc.push(FeatureOp::default()));
        let evaluated = editor.doc().evaluate();

        editor.select(bore);
        editor.toggle_selected(barrel);
        assert_eq!(editor.pair_to_combine(&evaluated), Some((bore, barrel)));

        editor.select(barrel);
        editor.toggle_selected(bore);
        assert_eq!(editor.pair_to_combine(&evaluated), Some((barrel, bore)));
        assert_eq!(editor.primary_body(&evaluated), Some(bore), "the last pick is the one the inspector shows");
    }

    #[test]
    fn a_pair_needs_exactly_two_standing_bodies() {
        let mut editor = PropEditor::default();
        let a = editor.edit(|doc| doc.push(FeatureOp::default()));
        let b = editor.edit(|doc| doc.push(FeatureOp::default()));
        let c = editor.edit(|doc| doc.push(FeatureOp::default()));
        let standing = editor.doc().evaluate();
        let eats_b = editor.edit(|doc| doc.push(FeatureOp::Mirror {
            target: b,
            axis: crate::prop::feature::Axis::X,
            offset: 0.0,
            keep_original: false,
        }));
        let b_eaten = editor.doc().evaluate();
        assert!(standing.bodies.iter().any(|(id, _)| *id == b) && eats_b != b);

        editor.select(a);
        assert_eq!(editor.pair_to_combine(&standing), None);
        editor.toggle_selected(b);
        assert_eq!(editor.pair_to_combine(&b_eaten), None, "b has been eaten");
        assert!(editor.pair_to_combine(&standing).is_some());
        editor.toggle_selected(c);
        assert_eq!(editor.pair_to_combine(&standing), None);
    }

    /// A face stands for the body it is on; a pair of faces on one body is
    /// no pair.
    #[test]
    fn a_face_stands_for_its_body() {
        use crate::prop::reference::Source;
        let mut editor = PropEditor::default();
        let barrel = editor.edit(|doc| doc.push(FeatureOp::default()));
        let bore = editor.edit(|doc| doc.push(FeatureOp::default()));
        let evaluated = editor.doc().evaluate();
        let face = |feature, index| {
            Pick::Geom(GeomRef { element: Element::Face(index), source: Source { feature, copies: vec![] } })
        };

        editor.toggle(face(barrel, 0));
        editor.toggle(face(bore, 2));
        assert_eq!(editor.pair_to_combine(&evaluated), Some((barrel, bore)));

        editor.select_pick(face(barrel, 0));
        editor.toggle(face(barrel, 1));
        assert_eq!(editor.pair_to_combine(&evaluated), None);
        assert_eq!(editor.body_to_act_on(&evaluated), Some(barrel));
    }

    #[test]
    fn unpicking_the_primary_promotes_the_one_before() {
        let mut editor = PropEditor::default();
        let a = editor.edit(|doc| doc.push(FeatureOp::default()));
        let b = editor.edit(|doc| doc.push(FeatureOp::default()));

        editor.select(a);
        editor.toggle_selected(b);
        editor.toggle_selected(b);
        assert_eq!(editor.picks(), &[Pick::Body(a)]);
    }

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

    #[test]
    fn adding_while_rolled_back_builds_at_the_bar() {
        let mut editor = PropEditor::default();
        let a = editor.add(FeatureOp::default());
        let b = editor.add(FeatureOp::default());
        editor.dirty = false;
        editor.roll_back_to(Some(a));
        assert_eq!(editor.shown().len(), 1);
        assert!(!editor.dirty, "rolling back is not an edit");

        let c = editor.add(FeatureOp::default());
        let order: Vec<_> = editor.doc().features.iter().map(|f| f.id).collect();
        assert_eq!(order, vec![a, c, b]);
        assert_eq!(editor.rollback(), Some(c));
        assert_eq!(editor.shown().len(), 2);
    }

    #[test]
    fn the_bar_never_points_at_nothing() {
        let mut editor = PropEditor::default();
        let a = editor.add(FeatureOp::default());
        let b = editor.add(FeatureOp::default());
        editor.roll_back_to(Some(b));
        assert_eq!(editor.rollback(), None);

        editor.roll_back_to(Some(a));
        editor.edit(|doc| doc.remove(a));
        editor.forget_missing();
        assert_eq!(editor.rollback(), None);
        assert_eq!(editor.shown().len(), 1);
    }

    #[test]
    fn reverting_is_one_undoable_step() {
        let mut editor = PropEditor::default();
        editor.edit(|doc| doc.push(FeatureOp::default()));
        let saved = editor.doc().clone();
        editor.dirty = false;

        editor.edit(|doc| doc.push(FeatureOp::default()));
        let edited = editor.doc().clone();
        assert!(editor.dirty);

        editor.revert_to(saved.clone());
        assert_eq!(editor.doc(), &saved);
        assert!(!editor.dirty, "reverted is the file");

        editor.undo();
        assert_eq!(editor.doc(), &edited, "the revert itself can be undone");
        assert!(editor.dirty);
    }

    #[test]
    fn undo_drops_every_pick_it_takes_away() {
        let mut editor = PropEditor::default();
        let a = editor.edit(|doc| doc.push(FeatureOp::default()));
        let b = editor.edit(|doc| doc.push(FeatureOp::default()));
        editor.select(b);
        editor.toggle_selected(a);
        editor.undo();
        assert_eq!(editor.picks(), &[Pick::Body(a)]);
    }

    /// Fails if a reference outlives a feature it is named through.
    #[test]
    fn undo_drops_a_reference_through_a_feature_it_takes_away() {
        use crate::prop::reference::{CopyOf, Element, Source};
        let mut editor = PropEditor::default();
        let a = editor.edit(|doc| doc.push(FeatureOp::default()));
        let mirror = editor.edit(|doc| {
            doc.push(FeatureOp::Mirror { target: a, axis: crate::prop::feature::Axis::X, offset: 0.0, keep_original: true })
        });
        let corner = |copies| Pick::Geom(GeomRef { element: Element::Point(0), source: Source { feature: a, copies } });
        editor.toggle(corner(vec![]));
        editor.toggle(corner(vec![CopyOf { by: mirror, index: 1 }]));
        editor.undo();
        assert_eq!(editor.picks(), &[corner(vec![])]);
    }

    #[test]
    fn picking_something_again_unpicks_it() {
        use crate::prop::reference::{Element, Source};
        let mut editor = PropEditor::default();
        let a = editor.edit(|doc| doc.push(FeatureOp::default()));
        let edge = Pick::Geom(GeomRef { element: Element::Edge(3), source: Source { feature: a, copies: vec![] } });
        editor.toggle(Pick::Body(a));
        editor.toggle(edge.clone());
        assert_eq!(editor.primary(), Some(&edge));
        assert_eq!(editor.primary_body(&editor.doc().evaluate()), None, "an edge does not stand for its body");
        editor.toggle(edge);
        assert_eq!(editor.primary(), Some(&Pick::Body(a)));
    }
}
