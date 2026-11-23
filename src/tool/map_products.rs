//! What each map feature offers to a click or a name: its walls, pick boxes and
//! named points, gathered again when the timeline moves so hover, picking and
//! the map wizard do not ask every feature every frame. A new kind of feature
//! joins by answering [`FeatureTrait::picked_by`] and its point and box methods.

use bevy::prelude::*;

use crate::common::ray::PickBox;
use crate::editor::editable::{FeatureId, FeatureTimeline, PickedBy};
use crate::tool::show::GizmoVisibility;
use crate::tool::tool_helpers::{ray_point_distance, PickCache};

/// Meters from the ray within which a point-like feature's own point picks it.
const SELECT_POINT_RADIUS: f32 = 0.3;

/// A named point: the feature that names it and the key it names it by.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PointId {
    pub feature: FeatureId,
    pub key: String,
}

pub struct Product {
    id: FeatureId,
    type_key: &'static str,
    picked_by: PickedBy,
    boxes: Vec<PickBox>,
    /// Its main point, which picks a point-like feature.
    point: Option<Vec3>,
}

#[derive(Resource, Default)]
pub struct MapProducts {
    pick: Option<PickCache>,
    products: Vec<Product>,
    /// Every named point, gathered the first time one is asked for after a
    /// change: a drag changes the map every frame and asks for none.
    points: Option<Vec<(PointId, Vec3)>>,
}

type Key = (bevy::ecs::change_detection::Tick, u64);

impl MapProducts {
    /// Gathered now, for code with no resource to keep.
    pub fn of(features: &FeatureTimeline) -> Self {
        let mut products = MapProducts::default();
        products.gather(features, Default::default());
        products
    }

    /// Gathered again only if the timeline has moved since `key`
    /// ([`PickCache::key`]).
    pub fn refresh(&mut self, features: &FeatureTimeline, key: Key) {
        if self.pick.as_ref().is_none_or(|pick| pick.key_of() != key) {
            self.gather(features, key);
        }
    }

    fn gather(&mut self, features: &FeatureTimeline, key: Key) {
        self.pick = Some(PickCache::of(features, key));
        self.points = None;
        self.products = features
            .active_features()
            .map(|(id, feature)| {
                let object = feature.object();
                Product {
                    id,
                    type_key: object.type_key(),
                    picked_by: object.picked_by(),
                    boxes: object.pick_boxes(),
                    point: object.get_point("").ok(),
                }
            })
            .collect();
    }

    pub fn is_point_like(&self, feature: FeatureId) -> bool {
        self.products.iter().any(|product| product.id == feature && product.picked_by == PickedBy::Marker)
    }

    /// The nearest visible feature `ray` hits, and where.
    pub fn hit(&mut self, ray: &Ray3d, visibility: &GizmoVisibility) -> Option<(FeatureId, Vec3)> {
        let dir = Vec3::from(ray.direction);
        let mut best: Option<(f32, FeatureId, Vec3)> = None;
        let offer = |t: f32, id: FeatureId, at: Vec3, best: &mut Option<(f32, FeatureId, Vec3)>| {
            if best.as_ref().is_none_or(|(nearest, _, _)| t < *nearest) {
                *best = Some((t, id, at));
            }
        };
        let pick = self.pick.as_mut()?;
        for product in self.products.iter().filter(|product| visibility.shows(product.type_key)) {
            match product.picked_by {
                PickedBy::Walls => {
                    for index in pick.owned(product.id) {
                        if let Some((t, at)) = pick.wall_hit(index, ray) {
                            offer(t, product.id, at, &mut best);
                        }
                    }
                }
                PickedBy::Outside | PickedBy::Marker => {
                    // A marker is picked from inside its box too: a grid's point is between
                    // bodies, inside none.
                    let inside_too = product.picked_by == PickedBy::Marker;
                    for pick_box in &product.boxes {
                        if let Some(t) = pick_box.ray_distance(ray).filter(|t| inside_too || *t > 0.0) {
                            offer(t, product.id, ray.origin + dir * t, &mut best);
                        }
                    }
                    if let Some(at) = product.point.filter(|_| inside_too)
                        && ray_point_distance(ray, at) < SELECT_POINT_RADIUS
                    {
                        let t = (at - ray.origin).dot(dir);
                        if t > 0.0 {
                            offer(t, product.id, at, &mut best);
                        }
                    }
                }
                PickedBy::Nothing => {}
            }
        }
        best.map(|(_, id, at)| (id, at))
    }

    /// The named point nearest `ray` within `radius` meters of it, from the
    /// `features` these products were gathered from.
    pub fn point_under(&mut self, features: &FeatureTimeline, ray: &Ray3d, radius: f32) -> Option<(PointId, Vec3)> {
        let points = self.points.get_or_insert_with(|| {
            features
                .active_features()
                .flat_map(|(id, feature)| {
                    let object = feature.object();
                    object
                        .available_point_keys()
                        .into_iter()
                        .filter_map(move |(key, _)| object.get_point(&key).ok().map(|at| (PointId { feature: id, key }, at)))
                })
                .collect()
        });
        points
            .iter()
            .map(|(id, at)| (ray_point_distance(ray, *at), id, at))
            .filter(|(distance, _, _)| *distance < radius)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id, at)| (id.clone(), *at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::editable::PointRef;
    use crate::editor::global_point::GlobalPoint;
    use crate::editor::trigger::EditorTrigger;
    use crate::editor::editor_room::EditorRoom;

    fn down_at(x: f32, z: f32) -> Ray3d {
        Ray3d::new(Vec3::new(x, 10.0, z), Dir3::NEG_Y)
    }

    #[test]
    fn a_point_is_picked_near_its_point_and_a_trigger_by_its_box() {
        let mut features = FeatureTimeline::default();
        let point = features.apply_feature(Box::new(GlobalPoint::new(5.0, 0.0, 5.0)));
        let box_of = |min: f32, max: f32| EditorRoom::from_point_refs(PointRef::absolute(min, 0.0, min), PointRef::absolute(max, 3.0, max));
        let trigger = features.apply_feature(Box::new(EditorTrigger::from_room(box_of(-1.0, 1.0))));
        let visibility = GizmoVisibility::default();
        let mut products = MapProducts::of(&features);
        assert_eq!(products.hit(&down_at(5.05, 5.0), &visibility).map(|(id, _)| id), Some(point));
        assert_eq!(products.hit(&down_at(0.0, 0.0), &visibility).map(|(id, _)| id), Some(trigger));
        assert!(products.is_point_like(point) && !products.is_point_like(trigger));
    }

    #[test]
    fn a_named_point_is_found_under_the_ray() {
        let mut features = FeatureTimeline::default();
        let point = features.apply_feature(Box::new(GlobalPoint::new(4.0, 0.0, 4.0)));
        let mut products = MapProducts::of(&features);
        let (id, at) = products.point_under(&features, &down_at(4.0, 4.0), 0.1).expect("the point");
        assert_eq!(id.feature, point);
        assert_eq!(at, Vec3::new(4.0, 0.0, 4.0));
        assert!(products.point_under(&features, &down_at(5.0, 4.0), 0.1).is_none());
    }
}
