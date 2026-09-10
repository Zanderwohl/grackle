use bevy::prelude::*;

use crate::common::ray::{ray_aabb_distance, ray_aabb_entry};
use crate::tool::room::{Room, WallSlab};

/// How far each wall slab extends behind its face.
///
/// Only the near side does any work: [`CollisionWorld::move_and_slide`] tests
/// whether the body's leading edge *crosses* that plane, so depth cannot be
/// tunnelled through however fast the body is moving. The thickness is here so
/// a slab is a well-formed box for the lateral overlap test, and the space it
/// occupies is outside the room either way.
const WALL_THICKNESS: f32 = 0.5;

/// A hair of clearance left when a body is pushed off a wall, so the next
/// frame's overlap test does not immediately find it touching again.
const SKIN: f32 = 0.001;

/// The solid world, as boxes.
///
/// Rebuilt from the `Room` components rather than from the baked meshes: same
/// geometry either way (see [`Room::collision_slabs`]), but the components are
/// what the feature timeline keeps current, so an edit lands here without
/// waiting for a re-bake.
#[derive(Resource, Default)]
pub struct CollisionWorld {
    pub slabs: Vec<WallSlab>,
    /// The room interiors themselves, as (min, max).
    ///
    /// Kept alongside the walls because "inside the map" is a question the
    /// walls alone cannot answer, and [`CollisionWorld::depenetrate`] needs it
    /// to know which way *out* of a wall is also *into* the level.
    interiors: Vec<(Vec3, Vec3)>,
}

impl CollisionWorld {
    pub fn rebuild(&mut self, rooms: &[Room]) {
        self.slabs.clear();
        self.interiors.clear();
        for room in rooms {
            self.interiors.push((room.min.min(room.max), room.min.max(room.max)));
        }
        for (i, room) in rooms.iter().enumerate() {
            let others: Vec<Room> = rooms
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, r)| r.clone())
                .collect();
            self.slabs.extend(room.collision_slabs(&others, WALL_THICKNESS));
        }
    }

    /// Move a box by `delta`, one axis at a time, stopping against walls.
    ///
    /// Axis-at-a-time is what produces sliding: a diagonal run into a wall
    /// loses the blocked component and keeps the other, rather than stopping
    /// dead. Returns the new centre and which axes were blocked — the caller
    /// needs the latter to zero velocity and to know it is standing on
    /// something.
    ///
    /// Each axis is a *swept* test rather than a move-then-push-out: it asks
    /// whether the leading edge crosses a wall plane over the course of the
    /// step, not whether it happens to be inside a wall when the step lands.
    /// The difference matters at speed — a body moving further in one tick
    /// than a slab is thick lands beyond it, overlapping nothing, and a
    /// push-out test waves it through.
    pub fn move_and_slide(&self, centre: Vec3, half: Vec3, delta: Vec3) -> (Vec3, BVec3) {
        let mut centre = centre;
        let mut blocked = BVec3::FALSE;

        for axis in 0..3 {
            let step = delta[axis];
            if step == 0.0 {
                continue;
            }

            let mut target = centre[axis] + step;

            for slab in &self.slabs {
                // Only walls the body actually passes beside can stop it. The
                // other two axes are read from `centre`, which already holds
                // the resolved value for any axis handled before this one.
                if !overlaps_other_axes(centre, half, slab, axis) {
                    continue;
                }

                if step > 0.0 {
                    let plane = slab.min[axis];
                    let leading = centre[axis] + half[axis];
                    if leading <= plane && target + half[axis] > plane {
                        target = target.min(plane - half[axis] - SKIN);
                        blocked = set_axis(blocked, axis, true);
                    }
                } else {
                    let plane = slab.max[axis];
                    let trailing = centre[axis] - half[axis];
                    if trailing >= plane && target - half[axis] < plane {
                        target = target.max(plane + half[axis] + SKIN);
                        blocked = set_axis(blocked, axis, true);
                    }
                }
            }

            centre[axis] = target;
        }

        (centre, blocked)
    }

    /// Whether a point is inside any room.
    pub fn inside_map(&self, point: Vec3) -> bool {
        self.interiors.iter().any(|(min, max)| {
            (0..3).all(|axis| point[axis] >= min[axis] && point[axis] <= max[axis])
        })
    }

    /// Whether a body of `half` extents centred at `centre` stands clear: in
    /// the map, and touching no wall.
    ///
    /// What decides whether a spawn point is usable. A ceiling two metres
    /// above the feet leaves the head inside the ceiling slab, and this says
    /// so — no separate notion of "headroom" is needed, because the geometry
    /// already knows.
    pub fn fits(&self, centre: Vec3, half: Vec3) -> bool {
        self.inside_map(centre) && !self.slabs.iter().any(|slab| overlaps(centre, half, slab))
    }

    /// How far along the ray the first wall is, or `None` if nothing solid is
    /// in the way within `range`.
    ///
    /// The unswept counterpart to [`CollisionWorld::move_and_slide`]: a shot
    /// has no volume, so it needs the plane it crosses and nothing else. What
    /// it is for is stopping a hitscan at the surface a player can see, so
    /// that a body behind a wall is behind it for shooting as well as for
    /// looking.
    pub fn ray_distance(&self, ray: &Ray3d, range: f32) -> Option<f32> {
        self.slabs
            .iter()
            .filter_map(|slab| ray_aabb_distance(ray, slab.min, slab.max))
            .filter(|distance| *distance <= range)
            .min_by(f32::total_cmp)
    }

    /// The first wall along the ray, as a distance and the face it presents.
    ///
    /// [`CollisionWorld::ray_distance`] with the half a *bounce* needs. A
    /// hitscan shot stops at a wall and never asks which way it was facing; a
    /// pipe bomb has to be mirrored about it, and a normal taken off the
    /// nearest face rather than the crossed one sends it through the floor.
    pub fn ray_hit(&self, ray: &Ray3d, range: f32) -> Option<(f32, Vec3)> {
        self.slabs
            .iter()
            .filter_map(|slab| ray_aabb_entry(ray, slab.min, slab.max))
            .filter(|(distance, _)| *distance <= range)
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// Shove a body that is *already* inside a wall back out of it.
    ///
    /// [`CollisionWorld::move_and_slide`] answers "may I cross this plane",
    /// which is the right question while the world holds still and the wrong
    /// one when the world moves instead. Editing a room mid-match does exactly
    /// that: raise a floor and the body it was resting on is suddenly enclosed
    /// by it, having crossed nothing.
    ///
    /// The usual fix — push out along whichever axis is least deep — puts a
    /// body under a rising floor straight through it, because down is the
    /// shorter way out of a floor slab you have just been swallowed by. So
    /// candidates that land inside a room are preferred over shorter ones that
    /// do not, and the level's own volume decides which way is out.
    pub fn depenetrate(&self, centre: Vec3, half: Vec3) -> Vec3 {
        /// Added to a candidate's distance when it would leave the body
        /// outside every room: any escape that stays in the map wins.
        const OUTSIDE_MAP_PENALTY: f32 = 1.0e6;

        let mut centre = centre;

        // Pushing clear of one wall can bury the body in another, so iterate —
        // but bounded, because a body wedged in a corner with nowhere to go
        // must not spin here forever.
        for _ in 0..4 {
            let Some(slab) = self.slabs.iter().find(|slab| overlaps(centre, half, slab)) else {
                break;
            };

            let mut best: Option<(f32, Vec3)> = None;
            for axis in 0..3 {
                let candidates = [
                    slab.max[axis] + half[axis] + SKIN,
                    slab.min[axis] - half[axis] - SKIN,
                ];
                for value in candidates {
                    let mut moved = centre;
                    moved[axis] = value;

                    let mut score = (value - centre[axis]).abs();
                    if !self.inside_map(moved) {
                        score += OUTSIDE_MAP_PENALTY;
                    }
                    if best.is_none_or(|(best_score, _)| score < best_score) {
                        best = Some((score, moved));
                    }
                }
            }

            match best {
                Some((_, moved)) => centre = moved,
                None => break,
            }
        }

        centre
    }
}

/// Whether the body overlaps the slab on all three axes.
fn overlaps(centre: Vec3, half: Vec3, slab: &WallSlab) -> bool {
    (0..3).all(|axis| {
        centre[axis] - half[axis] < slab.max[axis] && centre[axis] + half[axis] > slab.min[axis]
    })
}

/// Whether the body's cross-section overlaps the slab on the two axes that are
/// not the one being moved along.
fn overlaps_other_axes(centre: Vec3, half: Vec3, slab: &WallSlab, axis: usize) -> bool {
    (0..3).filter(|other| *other != axis).all(|other| {
        centre[other] - half[other] < slab.max[other] && centre[other] + half[other] > slab.min[other]
    })
}

fn set_axis(mut mask: BVec3, axis: usize, value: bool) -> BVec3 {
    match axis {
        0 => mask.x = value,
        1 => mask.y = value,
        2 => mask.z = value,
        _ => unreachable!(),
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10×4×10 room with its floor at y = 0.
    fn one_room() -> CollisionWorld {
        let mut world = CollisionWorld::default();
        world.rebuild(&[Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0))]);
        world
    }

    const HALF: Vec3 = Vec3::new(0.35, 0.9, 0.35);

    /// A shot fired across the room stops at the far wall, at the wall.
    #[test]
    fn a_ray_stops_at_the_wall_it_crosses() {
        let world = one_room();
        let ray = Ray3d::new(Vec3::new(0.0, 1.5, 0.0), Dir3::X);

        let distance = world.ray_distance(&ray, 100.0).expect("the ray left the room unstopped");
        assert!((distance - 5.0).abs() < 1e-3, "stopped at {distance} m");
    }

    /// And a range shorter than the room reports nothing, which is what lets
    /// a weapon's reach be the only thing deciding how far it carries.
    #[test]
    fn a_wall_beyond_the_range_is_not_reported() {
        let world = one_room();
        let ray = Ray3d::new(Vec3::new(0.0, 1.5, 0.0), Dir3::X);
        assert_eq!(world.ray_distance(&ray, 2.0), None);
    }


    #[test]
    fn a_lone_room_is_walled_on_all_six_sides() {
        assert_eq!(one_room().slabs.len(), 6);
    }

    #[test]
    fn walking_into_a_wall_stops_at_it() {
        let world = one_room();
        let start = Vec3::new(0.0, 0.9, 0.0);
        let (end, blocked) = world.move_and_slide(start, HALF, Vec3::new(100.0, 0.0, 0.0));

        assert!(blocked.x);
        assert!((end.x - (5.0 - HALF.x)).abs() < 0.01, "stopped at {}", end.x);
    }

    /// The reason resolution is per-axis: a diagonal run into a wall has to
    /// keep the component that is not blocked, or movement feels like glue.
    #[test]
    fn running_diagonally_into_a_wall_slides_along_it() {
        let world = one_room();
        let start = Vec3::new(0.0, 0.9, 0.0);
        let (end, blocked) = world.move_and_slide(start, HALF, Vec3::new(100.0, 0.0, 2.0));

        assert!(blocked.x);
        assert!(!blocked.z);
        assert!((end.z - 2.0).abs() < 0.01, "lost the free axis: z = {}", end.z);
    }

    #[test]
    fn falling_lands_on_the_floor() {
        let world = one_room();
        let (end, blocked) = world.move_and_slide(Vec3::new(0.0, 3.0, 0.0), HALF, Vec3::new(0.0, -10.0, 0.0));

        assert!(blocked.y);
        assert!((end.y - HALF.y).abs() < 0.01, "landed at {}", end.y);
    }

    /// A move-then-push-out resolver waves this through: the body lands well
    /// past the slab, overlapping nothing, and reports no collision. This is
    /// the case the swept test exists for.
    #[test]
    fn a_fast_mover_does_not_pass_through_a_wall() {
        let world = one_room();
        let start = Vec3::new(0.0, 0.9, 0.0);
        // Far past the wall, and past the far side of the slab behind it.
        let (end, _) = world.move_and_slide(start, HALF, Vec3::new(1000.0, 0.0, 0.0));

        assert!(end.x < 5.0, "tunnelled through to {}", end.x);
    }

    /// The whole point of building collision from the same subtraction the
    /// mesh uses: where two rooms meet there is no wall, and you can walk
    /// between them.
    #[test]
    fn a_shared_wall_between_two_rooms_is_open() {
        let mut world = CollisionWorld::default();
        world.rebuild(&[
            Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(0.0, 4.0, 5.0)),
            Room::new(Vec3::new(0.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0)),
        ]);

        let start = Vec3::new(-2.0, 0.9, 0.0);
        let (end, blocked) = world.move_and_slide(start, HALF, Vec3::new(4.0, 0.0, 0.0));

        assert!(!blocked.x, "the shared wall was solid");
        assert!((end.x - 2.0).abs() < 0.01, "stopped at {}", end.x);
    }

    /// A spawn point under a low ceiling has to be rejected rather than used
    /// and then resolved by shoving the body somewhere unexpected.
    #[test]
    fn a_body_does_not_fit_under_a_low_ceiling() {
        let mut world = CollisionWorld::default();
        // 1.5m of headroom for a 2m body.
        world.rebuild(&[Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 1.5, 5.0))]);

        assert!(!world.fits(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.35, 1.0, 0.35)));
    }

    #[test]
    fn a_body_fits_in_a_room_tall_enough_for_it() {
        let world = one_room();
        assert!(world.fits(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.35, 1.0, 0.35)));
    }

    #[test]
    fn a_body_outside_the_map_does_not_fit() {
        let world = one_room();
        assert!(!world.fits(Vec3::new(50.0, 1.0, 0.0), Vec3::new(0.35, 1.0, 0.35)));
    }

    /// A floor raised onto a standing body must lift it, not drop it through
    /// the map — the shorter way out of that slab is downwards.
    #[test]
    fn a_body_swallowed_by_a_raised_floor_is_pushed_up() {
        let mut world = CollisionWorld::default();
        world.rebuild(&[Room::new(Vec3::new(-5.0, 2.0, -5.0), Vec3::new(5.0, 6.0, 5.0))]);

        // Standing where the old floor was, now inside the new floor slab.
        let freed = world.depenetrate(Vec3::new(0.0, 0.9, 0.0), HALF);

        assert!(freed.y > 2.0, "pushed to y = {}, which is under the floor", freed.y);
        assert!(world.inside_map(freed), "pushed out of the map entirely");
    }

    #[test]
    fn a_body_standing_clear_of_the_walls_is_left_alone() {
        let world = one_room();
        let resting = Vec3::new(0.0, HALF.y + SKIN, 0.0);
        assert_eq!(world.depenetrate(resting, HALF), resting);
    }

    /// ...and the outer walls of that pair still hold.
    #[test]
    fn two_joined_rooms_are_still_closed_at_the_ends() {
        let mut world = CollisionWorld::default();
        world.rebuild(&[
            Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(0.0, 4.0, 5.0)),
            Room::new(Vec3::new(0.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0)),
        ]);

        let (end, blocked) = world.move_and_slide(Vec3::new(-2.0, 0.9, 0.0), HALF, Vec3::new(100.0, 0.0, 0.0));
        assert!(blocked.x);
        assert!((end.x - (5.0 - HALF.x)).abs() < 0.01, "stopped at {}", end.x);
    }
}
