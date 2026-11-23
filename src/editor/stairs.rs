//! A flight of stairs or a ramp, placed by four points. The geometry is
//! [`crate::common::stairs`]; this is the feature that names the points and the style. Its walls
//! are baked with the rooms' (`BakePlugin`), so they wait for a bake as a room's do.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::PointResolutionError;
use crate::common::ray::PickBox;
use crate::common::stairs::{Degenerate, Frame, Stairs, StairsShape, StairsStyle};
use crate::editor::editable::{insert_if_changed, Feature, FeatureId, FeatureTrait, PointRef, BROKEN};
use crate::editor::mirror::Mirroring;
use crate::get;
use crate::material::picker::material_button;
use crate::tool::room::{RoomSlot, RoomSurfaces};

pub const STAIRS_COLOR: Color = Color::srgb(0.55, 0.85, 0.45);
const TOO_STEEP_COLOR: Color = Color::srgb(0.95, 0.35, 0.25);

/// The four points, in the order they are stored and listed.
const KEYS: [&str; 4] = ["start", "end", "width", "height"];

/// Each point's label, in [`KEYS`] order.
const LABELS: [&str; 4] = [
    "editor.features.stairs.start",
    "editor.features.stairs.end",
    "editor.features.stairs.width",
    "editor.features.stairs.height",
];

/// The derived points: key, label, and where each is in the flight's frame as `(along, across,
/// up)` in multiples of its run, width and height.
const DERIVED: [(&str, &str, (f32, f32, f32)); 6] = [
    ("", "editor.features.stairs.points.center", (0.5, 0.5, 0.0)),
    ("start_far", "editor.features.stairs.points.start_far", (0.0, 1.0, 0.0)),
    ("end_far", "editor.features.stairs.points.end_far", (1.0, 1.0, 0.0)),
    ("top_near", "editor.features.stairs.points.top_near", (1.0, 0.0, 1.0)),
    ("top_far", "editor.features.stairs.points.top_far", (1.0, 1.0, 1.0)),
    ("top", "editor.features.stairs.points.top", (1.0, 0.5, 1.0)),
];

#[derive(Serialize, Deserialize)]
pub struct EditorStairs {
    start: PointRef,
    end: PointRef,
    width: PointRef,
    height: PointRef,
    #[serde(default)]
    style: StairsStyle,
    #[serde(default, skip_serializing_if = "RoomSurfaces::is_empty")]
    surfaces: RoomSurfaces,
    /// In [`KEYS`] order.
    #[serde(skip)]
    resolved: [Vec3; 4],
    #[serde(skip)]
    entity: Option<Entity>,
}

impl EditorStairs {
    pub fn new(start: PointRef, end: PointRef, width: PointRef, height: PointRef) -> Self {
        Self { start, end, width, height, style: StairsStyle::default(), surfaces: RoomSurfaces::default(), resolved: [Vec3::ZERO; 4], entity: None }
    }

    /// A flight [`DEFAULT_SIZE`] long, wide and high from `start`, up along +X with its width
    /// toward +Z. Each point is `start` moved, so one naming a feature names the same one.
    #[cfg(test)]
    pub fn from_start(start: PointRef) -> Self {
        let moved = |by: Vec3| {
            let mut point = start.clone();
            for (axis, by) in [(&mut point.x, by.x), (&mut point.y, by.y), (&mut point.z, by.z)] {
                *axis.value_mut() += by;
            }
            point
        };
        let (end, width, height) = (moved(DEFAULT_SIZE * Vec3::X), moved(DEFAULT_SIZE * Vec3::Z), moved(DEFAULT_SIZE * Vec3::Y));
        Self::new(start, end, width, height)
    }

    pub fn with_style(mut self, style: StairsStyle) -> Self {
        self.style = style;
        self
    }

    pub fn with_surfaces(mut self, surfaces: RoomSurfaces) -> Self {
        self.surfaces = surfaces;
        self
    }

    pub fn shape(&self) -> StairsShape {
        let [start, end, width, height] = self.resolved;
        StairsShape { start, end, width, height, style: self.style }
    }

    fn frame(&self) -> Result<Frame, Degenerate> {
        self.shape().frame()
    }

    fn label(index: usize) -> String {
        get!(LABELS[index])
    }

    /// Which point `key` names: its key, or the label the inspector shows it under, which is what
    /// the retarget tool hands back.
    fn index_of(key: &str) -> Option<usize> {
        KEYS.iter().position(|k| *k == key).or_else(|| (0..KEYS.len()).find(|&index| key == Self::label(index)))
    }

    fn points(&self) -> [&PointRef; 4] {
        [&self.start, &self.end, &self.width, &self.height]
    }

    fn points_mut(&mut self) -> [&mut PointRef; 4] {
        [&mut self.start, &mut self.end, &mut self.width, &mut self.height]
    }

    fn resolve_points(&mut self, features: &HashMap<FeatureId, Feature>) {
        let mut resolved = self.resolved;
        for (point, slot) in self.points_mut().into_iter().zip(resolved.iter_mut()) {
            if let Ok(at) = point.resolve(features) {
                *slot = at;
            }
        }
        self.resolved = resolved;
    }

    fn derived(key: &str) -> Option<(f32, f32, f32)> {
        DERIVED.iter().find(|(k, _, _)| *k == key).map(|(_, _, at)| *at)
    }

    fn style_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let style = &mut self.style;
        ui.horizontal(|ui| {
            changed |= ui.selectable_value(&mut style.steps, true, get!("editor.features.stairs.steps")).changed();
            changed |= ui.selectable_value(&mut style.steps, false, get!("editor.features.stairs.ramp")).changed();
        });
        if style.steps {
            changed |= ui
                .add(egui::DragValue::new(&mut style.rise).speed(0.01).range(0.05..=2.0).suffix(get!("editor.features.stairs.meters")).prefix(format!("{}: ", get!("editor.features.stairs.step_height"))))
                .changed();
            if let Some(steps) = self.shape().steps() {
                ui.label(get!("editor.features.stairs.step_count", "count", steps.count, "rise", format!("{:.3}", steps.rise), "depth", format!("{:.3}", steps.depth)));
            }
        }
        let style = &mut self.style;
        ui.horizontal(|ui| {
            changed |= ui.selectable_value(&mut style.thick, true, get!("editor.features.stairs.thick")).changed();
            changed |= ui.selectable_value(&mut style.thick, false, get!("editor.features.stairs.thin")).changed();
        });
        if !style.thick {
            changed |= ui
                .add(egui::DragValue::new(&mut style.thickness).speed(0.01).range(0.01..=2.0).suffix(get!("editor.features.stairs.meters")).prefix(format!("{}: ", get!("editor.features.stairs.thickness"))))
                .changed();
        }
        changed
    }
}

/// The footprint and the collider's outline over it: up the ramp, flat to the end edge for steps,
/// and down the back.
pub fn draw_collider(gizmos: &mut Gizmos, shape: &StairsShape, color: Color) {
    let Ok(frame) = shape.frame() else { return };
    let (knee, top) = shape.collider_top(&frame);
    let (r, w) = (frame.run, frame.width);
    let base = [frame.at(0.0, 0.0, 0.0), frame.at(r, 0.0, 0.0), frame.at(r, w, 0.0), frame.at(0.0, w, 0.0)];
    for i in 0..4 {
        gizmos.line(base[i], base[(i + 1) % 4], color);
    }
    for side in [0.0, w] {
        let outline = [frame.at(0.0, side, 0.0), frame.at(knee, side, top), frame.at(r, side, top), frame.at(r, side, 0.0)];
        for pair in outline.windows(2) {
            gizmos.line(pair[0], pair[1], color);
        }
    }
    gizmos.line(frame.at(knee, 0.0, top), frame.at(knee, w, top), color);
    gizmos.line(frame.at(r, 0.0, top), frame.at(r, w, top), color);
}

/// Meters: each side of a flight [`EditorStairs::from_start`] makes.
#[cfg(test)]
const DEFAULT_SIZE: f32 = 2.0;

#[typetag::serde(name = "stairs")]
impl FeatureTrait for EditorStairs {
    fn resources_named(&self) -> Vec<crate::common::resource::Named> {
        self.surfaces.named()
    }

    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        if let Some(index) = KEYS.iter().position(|k| *k == key) {
            return Ok(self.resolved[index]);
        }
        let (u, w, h) = Self::derived(key).ok_or(PointResolutionError::NoSuchPoint)?;
        let frame = self.frame().map_err(|_| PointResolutionError::Other)?;
        Ok(frame.at(u * frame.run, w * frame.width, h * frame.height))
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        match self.frame() {
            Err(problem) => {
                let key = match problem {
                    Degenerate::NoRun => "editor.features.stairs.no_run",
                    Degenerate::NoWidth => "editor.features.stairs.no_width",
                    Degenerate::NoHeight => "editor.features.stairs.no_height",
                };
                ui.colored_label(BROKEN, get!(key));
            }
            Ok(frame) => {
                ui.label(get!(
                    "editor.features.stairs.size",
                    "run", format!("{:.2}", frame.run),
                    "width", format!("{:.2}", frame.width),
                    "height", format!("{:.2}", frame.height),
                    "slope", format!("{:.1}", frame.slope().to_degrees())
                ));
                if !self.shape().walkable() {
                    ui.colored_label(BROKEN, get!("editor.features.stairs.too_steep"));
                }
            }
        }
        ui.separator();
        // Scoped so the size and steepness lines above coming and going do not move focus.
        let mut changed = ui.push_id("stairs_style", |ui| self.style_ui(ui)).inner;
        for index in 0..KEYS.len() {
            ui.separator();
            let label = Self::label(index);
            changed |= self.points_mut()[index].editor_ui(ui, &label, features, prior_feature_order, retarget_request);
        }
        if changed {
            self.resolve_points(features);
        }
        ui.separator();
        ui.label(get!("editor.features.room.materials"));
        egui::Grid::new("stairs_materials").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            for slot in RoomSlot::ALL {
                ui.label(get!(slot.name_key()));
                changed |= material_button(ui, self.surfaces.slot_mut(slot), true);
                ui.end_row();
            }
        });
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.stairs.title")
    }

    fn tool(&self) -> Option<crate::tool::Tools> { Some(crate::tool::Tools::Stairs) }
    fn type_key(&self) -> &'static str { "stairs" }
    fn picked_by(&self) -> crate::editor::editable::PickedBy { crate::editor::editable::PickedBy::Outside }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        for (point, at) in self.points().into_iter().zip(self.resolved) {
            point.debug_gizmos(at, gizmos);
        }
        if self.frame().is_err() {
            for at in self.resolved {
                gizmos.sphere(Isometry3d::from_translation(at), 0.1, TOO_STEEP_COLOR);
            }
            return;
        }
        let color = if self.shape().walkable() { STAIRS_COLOR } else { TOO_STEEP_COLOR };
        draw_collider(gizmos, &self.shape(), color);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        let shape = self.shape();
        let center = shape.frame().map(|frame| frame.oriented_box().0).unwrap_or(shape.start);
        let mut entity = commands.entity(entity);
        insert_if_changed(&mut entity, Transform::from_translation(center));
        insert_if_changed(&mut entity, Stairs(shape));
        insert_if_changed(&mut entity, self.surfaces.clone());
    }

    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>) {
        self.resolve_points(features);
    }

    fn parent_ids(&self) -> Vec<FeatureId> {
        let mut ids = Vec::new();
        for id in self.points().into_iter().flat_map(PointRef::referenced_features) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        let mut keys: Vec<(String, String)> = (0..KEYS.len()).map(|index| (KEYS[index].to_string(), Self::label(index))).collect();
        keys.extend(DERIVED.iter().map(|(key, label, _)| (key.to_string(), get!(label))));
        keys
    }

    fn point_label(&self, key: &str) -> Option<String> {
        if let Some(index) = KEYS.iter().position(|k| *k == key) {
            return Some(Self::label(index));
        }
        DERIVED.iter().find(|(k, _, _)| *k == key).map(|(_, label, _)| get!(label))
    }

    fn point_count(&self) -> usize {
        KEYS.len() + DERIVED.len()
    }

    fn reference_points_for_ray(&self, ray: &Ray3d) -> Vec<(String, Vec3)> {
        let Ok(frame) = self.frame() else { return Vec::new() };
        let (center, rotation, half) = frame.oriented_box();
        let padded = PickBox { center, rotation, half_extents: half + Vec3::splat(0.25) };
        if padded.ray_distance(ray).is_none() {
            return Vec::new();
        }
        self.available_point_keys().into_iter().filter_map(|(key, _)| self.get_point(&key).ok().map(|at| (key, at))).collect()
    }

    fn pick_boxes(&self) -> Vec<PickBox> {
        let Ok(frame) = self.frame() else { return Vec::new() };
        let (center, rotation, half_extents) = frame.oriented_box();
        vec![PickBox { center, rotation, half_extents }]
    }

    fn point_refs(&self) -> Vec<&PointRef> {
        self.points().to_vec()
    }

    fn get_point_ref(&self, key: &str) -> Option<&PointRef> {
        Self::index_of(key).map(|index| self.points()[index])
    }

    fn get_point_ref_mut(&mut self, key: &str) -> Option<&mut PointRef> {
        let index = Self::index_of(key)?;
        self.points_mut().into_iter().nth(index)
    }

    /// Faces are wound from the frame, so a reflected flight is not drawn inside out.
    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        self.resolved = self.resolved.map(|at| mirroring.point(at));
        for point in self.points_mut() {
            point.resolved_reference = None;
        }
        true
    }

    fn stairs(&self) -> Option<StairsShape> {
        Some(self.shape())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::editable::FeatureTimeline;
    use crate::editor::global_point::GlobalPoint;

    #[test]
    fn a_flight_from_a_feature_point_names_it_four_times() {
        let mut timeline = FeatureTimeline::default();
        let point = timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 2.0, 3.0)));
        let id = timeline.apply_feature(Box::new(EditorStairs::from_start(PointRef::reference(point))));
        timeline.resolve_all();
        let stairs = timeline.get_feature(&id).unwrap().object();
        assert_eq!(stairs.parent_ids(), vec![point]);
        assert_eq!(stairs.get_point("top").unwrap(), Vec3::new(3.0, 4.0, 4.0));
        let shape = stairs.stairs().unwrap();
        assert!(shape.walkable());
        assert!(!timeline.get_feature(&id).unwrap().is_broken());
    }

    #[test]
    fn the_inspectors_labels_name_the_points() {
        let stairs = EditorStairs::from_start(PointRef::absolute(0.0, 0.0, 0.0));
        for index in 0..KEYS.len() {
            assert!(stairs.get_point_ref(&EditorStairs::label(index)).is_some(), "{}", KEYS[index]);
        }
    }

    /// Breaks if stairs start answering `rooms`: they would carve air.
    #[test]
    fn stairs_are_no_room() {
        assert!(EditorStairs::from_start(PointRef::absolute(0.0, 0.0, 0.0)).volumes().is_empty());
    }
}
