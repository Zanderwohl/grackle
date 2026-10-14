use bevy::prelude::*;

use crate::common::ray::{ray_aabb_distance, ray_aabb_entry};
use crate::tool::room::{Room, WallSlab};

/// How far each wall slab extends behind its face, in metres. Only the face
/// matters to the swept test; the depth makes the slab a proper box for the
/// lateral overlap test.
const WALL_THICKNESS: f32 = 0.5;

/// Clearance left when a body is pushed off a wall, so the next overlap test
/// does not find it touching again.
const SKIN: f32 = 0.001;

/// The solid world, as boxes.
///
/// Built from the `Room` components rather than baked meshes, so an edit
/// lands without waiting for a re-bake. Same geometry either way.
#[derive(Resource, Default)]
pub struct CollisionWorld {
    pub slabs: Vec<WallSlab>,
    /// Room interiors as (min, max). [`CollisionWorld::depenetrate`] needs
    /// them to tell which way out of a wall is into the level.
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

    /// Move a box by `delta`, one axis at a time so a diagonal run into a wall
    /// slides along it. Returns the new centre and which axes were blocked.
    ///
    /// Swept per axis: it asks whether the leading edge crosses a wall plane,
    /// so a body moving further than a slab is thick cannot tunnel through.
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
                // `centre` already holds the resolved value for earlier axes.
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

    pub fn inside_map(&self, point: Vec3) -> bool {
        self.interiors.iter().any(|(min, max)| {
            (0..3).all(|axis| point[axis] >= min[axis] && point[axis] <= max[axis])
        })
    }

    /// Whether a body stands clear: in the map and touching no wall. A low
    /// ceiling is a slab the body overlaps, so this covers headroom too.
    pub fn fits(&self, centre: Vec3, half: Vec3) -> bool {
        self.inside_map(centre) && !self.slabs.iter().any(|slab| overlaps(centre, half, slab))
    }

    /// How far along the ray the first wall is, or `None` if nothing solid is
    /// in the way within `range`.
    pub fn ray_distance(&self, ray: &Ray3d, range: f32) -> Option<f32> {
        self.slabs
            .iter()
            .filter_map(|slab| ray_aabb_distance(ray, slab.min, slab.max))
            .filter(|distance| *distance <= range)
            .min_by(f32::total_cmp)
    }

    /// The first wall along the ray, as a distance and the normal of the face
    /// crossed. The crossed face, not the nearest: a bounce off the wrong
    /// normal goes through the floor.
    pub fn ray_hit(&self, ray: &Ray3d, range: f32) -> Option<(f32, Vec3)> {
        self.slabs
            .iter()
            .filter_map(|slab| ray_aabb_entry(ray, slab.min, slab.max))
            .filter(|(distance, _)| *distance <= range)
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// Shove a body that is already inside a wall back out of it, for when
    /// the world moves instead (a floor raised mid-match).
    ///
    /// Escapes that land inside a room beat shorter ones that do not; least
    /// depth alone pushes a body under a raised floor down through the map.
    pub fn depenetrate(&self, centre: Vec3, half: Vec3) -> Vec3 {
        /// Large enough that any escape staying in the map wins.
        const OUTSIDE_MAP_PENALTY: f32 = 1.0e6;

        let mut centre = centre;

        // Clearing one wall can bury the body in another. Bounded, for a body
        // wedged with nowhere to go.
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

fn overlaps(centre: Vec3, half: Vec3, slab: &WallSlab) -> bool {
    (0..3).all(|axis| {
        centre[axis] - half[axis] < slab.max[axis] && centre[axis] + half[axis] > slab.min[axis]
    })
}

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

    #[test]
    fn a_ray_stops_at_the_wall_it_crosses() {
        let world = one_room();
        let ray = Ray3d::new(Vec3::new(0.0, 1.5, 0.0), Dir3::X);

        let distance = world.ray_distance(&ray, 100.0).expect("the ray left the room unstopped");
        assert!((distance - 5.0).abs() < 1e-3, "stopped at {distance} m");
    }

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

    /// Breaks if the test stops being swept: the body lands past the slab,
    /// overlapping nothing.
    #[test]
    fn a_fast_mover_does_not_pass_through_a_wall() {
        let world = one_room();
        let start = Vec3::new(0.0, 0.9, 0.0);
        let (end, _) = world.move_and_slide(start, HALF, Vec3::new(1000.0, 0.0, 0.0));

        assert!(end.x < 5.0, "tunnelled through to {}", end.x);
    }

    /// Breaks if collision stops using the mesh's face subtraction.
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

    /// Breaks if depenetration prefers the shortest escape, which is down.
    #[test]
    fn a_body_swallowed_by_a_raised_floor_is_pushed_up() {
        let mut world = CollisionWorld::default();
        world.rebuild(&[Room::new(Vec3::new(-5.0, 2.0, -5.0), Vec3::new(5.0, 6.0, 5.0))]);

        // Inside the new floor slab.
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
