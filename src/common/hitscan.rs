//! Firing a ray at bodies and finding out what it hit.
//!
//! Deliberately a function of values rather than a system: a shot is
//! `(ray, range, the boxes)` in and a hit out, with no `World` in sight. That
//! is the same reason the player's step takes a [`crate::game::player::PlayerInput`]
//! — once there is a server, hit registration has to be re-runnable against a
//! rewound set of boxes, and something that queried the live world could not
//! be.
//!
//! What it does *not* know about is walls. The caller shortens `range` to the
//! nearest solid surface before asking, which keeps this free of the collision
//! world and means "shoot through a wall" is one decision in one place rather
//! than a flag threaded through here.

use bevy::prelude::*;

use crate::common::hitbox::{Box3, Hitboxes};
use crate::common::ray::ray_aabb_distance;

/// Which box a shot landed on.
///
/// Two, because there are two — see [`crate::common::hitbox::Hitboxes`]. Limbs
/// arrive here when they arrive there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitZone {
    Body,
    Head,
}

/// What a ray found.
///
/// `target` is whatever the caller used to name its bodies — an `Entity`, in
/// the game; an index, in a test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit<T> {
    pub target: T,
    /// Where the ray entered the box it is reported against, in world space.
    pub point: Vec3,
    /// How far along the ray this body was first touched.
    ///
    /// The *body's* entry, not the reported zone's: a head shot enters the
    /// full-height body column a hair before the head box, and that hair is
    /// what decides which of two overlapping bodies is in front.
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

/// One body's answer: did the ray touch it, and did it touch the head.
///
/// The head box sits *inside* the body column rather than on top of it, so
/// "whichever box the ray reaches first" would report every head shot as a
/// body shot. The head wins whenever it is hit at all, which is also the rule
/// a player expects.
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

    /// A body standing with its feet at `feet`, boxed as it rests.
    fn body(feet: Vec3) -> Hitboxes {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::from_translation(feet);
        crate::common::hitbox::hitboxes(&skeleton, &Pose::rest(), &Pose::rest(), &root, Stance::Standing)
    }

    fn shoot_at(origin: Vec3, at: Vec3) -> Ray3d {
        Ray3d::new(origin, Dir3::new(at - origin).unwrap())
    }

    /// The point of the whole exercise: aim at the head and the head is what
    /// is reported, even though the ray crossed the body column to get there.
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

    /// Nearest wins, and it is the *body's* entry that decides — the near
    /// body is hit in the chest and the far one in the head, so ranking on
    /// the reported zone's distance would pick the wrong one only if the
    /// chest shot were ranked by the head box it never touched.
    #[test]
    fn the_nearer_body_is_the_one_that_is_hit() {
        let near = body(Vec3::new(0.0, 0.0, 0.0));
        let far = body(Vec3::new(0.0, 0.0, 10.0));
        let ray = shoot_at(Vec3::new(0.0, near.body.centre.y, -5.0), Vec3::new(0.0, near.body.centre.y, 0.0));

        let hit = trace(&ray, 100.0, [("near", near), ("far", far)]).unwrap();
        assert_eq!(hit.target, "near");
    }

    /// Range is a hard stop, which is how the caller keeps a shot from
    /// carrying through the wall in front of it.
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
