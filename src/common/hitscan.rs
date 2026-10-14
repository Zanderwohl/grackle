//! Firing a ray at bodies and finding out what it hit.
//!
//! A function of values with no `World`, so hit registration can be re-run
//! against a rewound set of boxes. It knows nothing of walls: the caller
//! shortens `range` to the nearest solid surface first.

use bevy::prelude::*;

use crate::common::hitbox::{Box3, Hitboxes};
use crate::common::ray::ray_aabb_distance;

/// Which box a shot landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitZone {
    Body,
    Head,
}

/// `target` is whatever the caller names its bodies by: an `Entity` in the
/// game, an index in a test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit<T> {
    pub target: T,
    /// World space, where the ray entered the box it is reported against.
    pub point: Vec3,
    /// The body's first entry, not the reported zone's: a head shot enters the
    /// body column a hair before the head box, and that hair decides which of
    /// two overlapping bodies is in front.
    pub distance: f32,
    pub zone: HitZone,
}

/// The nearest body the ray meets within `range`, and where on it.
pub fn trace<T>(
    ray: &Ray3d,
    range: f32,
    targets: impl IntoIterator<Item = (T, Hitboxes)>,
) -> Option<Hit<T>> {
    targets
        .into_iter()
        .filter_map(|(target, boxes)| hit_body(ray, range, target, boxes))
        .min_by(|a, b| a.distance.total_cmp(&b.distance))
}

/// The head box sits inside the body column, so "first box reached" would make
/// every head shot a body shot. The head wins whenever it is hit at all.
fn hit_body<T>(ray: &Ray3d, range: f32, target: T, boxes: Hitboxes) -> Option<Hit<T>> {
    let reach = |hitbox: Box3| {
        ray_aabb_distance(ray, hitbox.min(), hitbox.max()).filter(|distance| *distance <= range)
    };

    let head = reach(boxes.head);
    let body = reach(boxes.body);

    let (zone, entry) = match (head, body) {
        (Some(head), _) => (HitZone::Head, head),
        (None, Some(body)) => (HitZone::Body, body),
        (None, None) => return None,
    };

    Some(Hit {
        target,
        point: ray.origin + *ray.direction * entry,
        distance: head.into_iter().chain(body).fold(f32::MAX, f32::min),
        zone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::class::Stance;
    use crate::common::skeleton::rig::Pose;
    use crate::common::skeleton::{humanoid, Proportions};

    fn body(feet: Vec3) -> Hitboxes {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::from_translation(feet);
        crate::common::hitbox::hitboxes(&skeleton, &Pose::rest(), &Pose::rest(), &root, Stance::Standing)
    }

    fn shoot_at(origin: Vec3, at: Vec3) -> Ray3d {
        Ray3d::new(origin, Dir3::new(at - origin).unwrap())
    }

    /// The ray crosses the body column to reach the head, and must still
    /// report the head.
    #[test]
    fn a_shot_through_the_head_is_a_head_shot() {
        let boxes = body(Vec3::ZERO);
        let ray = shoot_at(Vec3::new(0.0, boxes.head.centre.y, -5.0), boxes.head.centre);

        let hit = trace(&ray, 100.0, [((), boxes)]).expect("missed a head it was aimed at");
        assert_eq!(hit.zone, HitZone::Head);
        assert!(boxes.head.contains(hit.point + *ray.direction * 1e-3), "{:?}", hit.point);
    }

    #[test]
    fn a_shot_through_the_chest_is_a_body_shot() {
        let boxes = body(Vec3::ZERO);
        let chest = Vec3::new(0.0, boxes.body.centre.y, 0.0);
        let hit = trace(&shoot_at(Vec3::new(0.0, chest.y, -5.0), chest), 100.0, [((), boxes)]).unwrap();
        assert_eq!(hit.zone, HitZone::Body);
    }

    /// Ranking is by the body's entry, not the reported zone's.
    #[test]
    fn the_nearer_body_is_the_one_that_is_hit() {
        let near = body(Vec3::new(0.0, 0.0, 0.0));
        let far = body(Vec3::new(0.0, 0.0, 10.0));
        let ray = shoot_at(Vec3::new(0.0, near.body.centre.y, -5.0), Vec3::new(0.0, near.body.centre.y, 0.0));

        let hit = trace(&ray, 100.0, [("near", near), ("far", far)]).unwrap();
        assert_eq!(hit.target, "near");
    }

    /// Range is a hard stop; it is how the caller stops a shot at a wall.
    #[test]
    fn a_body_past_the_range_is_not_hit() {
        let boxes = body(Vec3::new(0.0, 0.0, 20.0));
        let ray = Ray3d::new(Vec3::new(0.0, boxes.body.centre.y, 0.0), Dir3::Z);

        assert!(trace(&ray, 5.0, [((), boxes)]).is_none());
        assert!(trace(&ray, 100.0, [((), boxes)]).is_some());
    }

    #[test]
    fn a_ray_pointed_elsewhere_hits_nothing() {
        let boxes = body(Vec3::ZERO);
        let ray = Ray3d::new(Vec3::new(0.0, boxes.body.centre.y, -5.0), Dir3::NEG_Z);
        assert!(trace(&ray, 100.0, [((), boxes)]).is_none());
    }
}
