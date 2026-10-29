//! The solid world, and the questions movement, weapons and corpses ask it.
//! See `documentation/collision.md`.

mod contact;
pub mod hull;
mod rounded;

use bevy::prelude::*;

use crate::common::world_surface::{self, Face, Volume};
use crate::prop::archetype::RoundedBox;
use crate::tool::room::Room;
use crate::common::grid::Grid;
pub use contact::{Touched, MOST_TOUCHED};
pub use hull::{onto_plane, Hit, Hull};

/// How far each wall stands out behind its face, in meters. Only the face
/// matters to a sweep; the depth gives depenetration somewhere to measure from.
const WALL_THICKNESS: f32 = 0.5;

/// Clearance left when a body is pushed off a wall, so the next overlap test
/// does not find it touching again.
const SKIN: f32 = 0.001;

/// Built from the rooms' surface directly rather than from the baked meshes,
/// so an edit lands without waiting for a re-bake. Same faces either way.
#[derive(Resource, Default)]
pub struct CollisionWorld {
    hulls: Vec<Hull>,
    grid: Grid,
    /// The map's rooms and blocks, in order. [`CollisionWorld::depenetrate`]
    /// needs them to tell which way out of a wall is into the level.
    volumes: Vec<Volume>,
    /// Placed props that block movement. Kept apart from `hulls` because they
    /// are rebuilt on a different trigger: rooms change with the map, props
    /// with what the authority has standing.
    pub props: Vec<RoundedBox>,
}

fn face_bounds(face: &Face) -> (Vec3, Vec3) {
    let min = face.vertices.iter().copied().fold(Vec3::INFINITY, Vec3::min);
    let max = face.vertices.iter().copied().fold(Vec3::NEG_INFINITY, Vec3::max);
    (min, max)
}

/// The prism a face sweeps back, but for its back: its own plane and one
/// through each edge, as (outward normal, reach).
fn prism(face: &Face) -> Vec<(Vec3, f32)> {
    let n = face.plane.normal;
    let count = face.vertices.len();
    let mut planes = vec![(n, n.dot(face.vertices[0]))];
    planes.extend((0..count).filter_map(|i| {
        let (a, b) = (face.vertices[i], face.vertices[(i + 1) % count]);
        let out = (b - a).cross(n).try_normalize()?;
        Some((out, out.dot(a)))
    }));
    planes
}

/// How far behind `face` the nearest part of `other` is, if any of it is
/// strictly inside the face's `prism` swept back `depth` deep; else `depth`.
/// Every face bounds air, so the solid behind `face` ends there. Strictly,
/// by `INSIDE`: a neighbor sharing an edge or a plane is on the prism, not
/// in it, and would thin a slab to nothing.
fn depth_behind(prism: &[(Vec3, f32)], other: &Face, depth: f32) -> f32 {
    const INSIDE: f32 = 1.0e-4;
    let (n, front) = prism[0];
    let mut kept = other.vertices.clone();
    for (normal, reach) in prism.iter().copied().chain([(-n, depth - front)]) {
        kept = clip(&kept, normal, reach - INSIDE);
        if kept.is_empty() {
            return depth;
        }
    }
    kept.iter().map(|v| front - n.dot(*v)).fold(depth, f32::min)
}

/// The part of a convex polygon where `normal · x <= reach`.
fn clip(polygon: &[Vec3], normal: Vec3, reach: f32) -> Vec<Vec3> {
    let mut kept = Vec::with_capacity(polygon.len() + 1);
    for (i, &a) in polygon.iter().enumerate() {
        let b = polygon[(i + 1) % polygon.len()];
        let (da, db) = (normal.dot(a) - reach, normal.dot(b) - reach);
        if da <= 0.0 {
            kept.push(a);
        }
        if (da < 0.0) != (db < 0.0) && da != db {
            kept.push(a + (b - a) * (da / (da - db)));
        }
    }
    kept
}

impl CollisionWorld {
    /// The map's volumes, and the surface they make — the same one that is
    /// drawn — as the walls.
    pub fn rebuild_volumes(&mut self, volumes: &[Volume]) {
        self.volumes = volumes.to_vec();
        self.set_surface(&world_surface::of_volumes(volumes));
    }

    /// A map of rooms alone.
    pub fn rebuild(&mut self, rooms: &[Room]) {
        self.rebuild_volumes(&world_surface::in_box_order(rooms).0);
    }

    /// Each face of the surface stood out into the solid behind it as a hull,
    /// never further than that solid goes: a slab through a thin block or a
    /// thin floor would stand out into the air on its far side.
    pub fn set_surface(&mut self, faces: &[Face]) {
        let grid = Grid::build(faces.iter().map(face_bounds));
        let hulls = faces
            .iter()
            .enumerate()
            .map(|(index, face)| {
                let (min, max) = face_bounds(face);
                let behind = -face.plane.normal * WALL_THICKNESS;
                let others = grid.near(min.min(min + behind), max.max(max + behind)).into_iter().filter(|other| *other as usize != index);
                let prism = prism(face);
                let depth = others.fold(WALL_THICKNESS, |depth, other| depth.min(depth_behind(&prism, &faces[other as usize], depth)));
                Hull::slab(&face.vertices, face.plane.normal, depth)
            })
            .collect();
        self.set_hulls(hulls);
    }

    /// The map's rooms and blocks, in order.
    pub fn volumes(&self) -> &[Volume] {
        &self.volumes
    }

    /// Replace the static world, leaving the room interiors as they were.
    pub fn set_hulls(&mut self, hulls: Vec<Hull>) {
        self.grid = Grid::build(hulls.iter().map(Hull::bounds));
        self.hulls = hulls;
    }

    /// Add to the static world without rebuilding what is there.
    pub fn add_hulls(&mut self, more: impl IntoIterator<Item = Hull>) {
        let mut hulls = std::mem::take(&mut self.hulls);
        hulls.extend(more);
        self.set_hulls(hulls);
    }

    fn hulls_near(&self, min: Vec3, max: Vec3) -> impl Iterator<Item = &Hull> {
        self.grid.near(min, max).into_iter().map(|index| &self.hulls[index as usize])
    }

    /// The first thing a box moving by `delta` touches: a wall or a solid prop.
    pub fn sweep(&self, center: Vec3, half: Vec3, delta: Vec3) -> Option<Hit> {
        let (min, max) = ((center - half).min(center + delta - half), (center + half).max(center + delta + half));
        let walls = self.hulls_near(min, max).filter_map(|hull| hull.sweep(center, half, delta));
        let props = self.props.iter().filter_map(|prop| rounded::sweep(prop, center, half, delta));
        walls.chain(props).fold(None, |best: Option<Hit>, hit| match best {
            Some(best) if best.t <= hit.t => Some(best),
            _ => Some(hit),
        })
    }

    /// Move a box by `delta`, one axis at a time so a diagonal run into a wall
    /// Move a box by `delta`, sliding along whatever it meets: sweep, stop
    /// `SKIN` short, take the part of the rest that goes into what was hit out,
    /// and go again. Returns where it ended and what it touched.
    ///
    /// Swept, so a body moving further than a wall is thick cannot tunnel
    /// through. A motion turned back against where it started is dropped
    /// rather than followed, which is what stops a box rattling in a corner.
    pub fn slide(&self, center: Vec3, half: Vec3, delta: Vec3) -> (Vec3, Touched) {
        let mut at = center;
        let mut remaining = delta;
        let mut touched = Touched::default();

        for _ in 0..MOST_TOUCHED {
            if remaining == Vec3::ZERO {
                break;
            }
            let Some(hit) = self.sweep(at, half, remaining) else {
                at += remaining;
                break;
            };
            at = onto_plane(at + remaining * hit.t, hit.normal, hit.plane + SKIN);
            touched.add(hit.normal);
            remaining = touched.clip(remaining * (1.0 - hit.t));
            if remaining.dot(delta) <= 0.0 {
                break;
            }
        }

        (at, touched)
    }

    /// How far a box gets along `delta` before it touches anything, `SKIN`
    /// short of it. No sliding: for something that should stay on its line,
    /// like a camera.
    pub fn travel(&self, center: Vec3, half: Vec3, delta: Vec3) -> Vec3 {
        match self.sweep(center, half, delta) {
            Some(hit) => onto_plane(center + delta * hit.t, hit.normal, hit.plane + SKIN),
            None => center + delta,
        }
    }

    /// [`CollisionWorld::slide`] taken over a ledge up to `height` high: lift,
    /// slide the level part of `delta`, and come back down onto a walkable
    /// floor. `None` if there is no room to lift or nowhere to land, which the
    /// caller reads as "walk normally".
    pub fn step_up(&self, center: Vec3, half: Vec3, delta: Vec3, height: f32, walkable: f32) -> Option<(Vec3, Touched)> {
        let raised = self.travel(center, half, Vec3::Y * height);
        let lift = raised.y - center.y;
        if lift <= SKIN {
            return None;
        }
        let (across, touched) = self.slide(raised, half, Vec3::new(delta.x, 0.0, delta.z));
        let (landed, _) = self.ground(across, half, lift + SKIN, walkable)?;
        Some((landed, touched))
    }

    /// The walkable surface right under a box, if there is one within
    /// `reach` — a normal no further from up than `walkable`, a cosine — as
    /// where the box rests on it and its normal.
    pub fn ground(&self, center: Vec3, half: Vec3, reach: f32, walkable: f32) -> Option<(Vec3, Vec3)> {
        let delta = Vec3::NEG_Y * reach;
        let hit = self.sweep(center, half, delta).filter(|hit| hit.normal.y >= walkable)?;
        let touching = center + delta * hit.t;
        // Backed off straight up, not along the normal: along it, a body
        // settled each tick on a slope creeps down it.
        let resting = if hit.normal.y == 1.0 {
            onto_plane(touching, hit.normal, hit.plane + SKIN)
        } else {
            touching + Vec3::Y * ((hit.plane + SKIN - hit.normal.dot(touching)) / hit.normal.y)
        };
        Some((resting, hit.normal))
    }

    pub fn inside_map(&self, point: Vec3) -> bool {
        world_surface::is_air(&self.volumes, point)
    }

    /// Whether a body stands clear: in the map and touching no wall. A low
    /// ceiling is a slab the body overlaps, so this covers headroom too.
    pub fn fits(&self, center: Vec3, half: Vec3) -> bool {
        self.inside_map(center)
            && !self.hulls_near(center - half, center + half).any(|hull| hull.overlaps(center, half))
            && !self.props.iter().any(|prop| prop.overlaps(center, half))
    }

    /// How far along the ray the first wall is, or `None` if nothing solid is
    /// in the way within `range`.
    pub fn ray_distance(&self, ray: &Ray3d, range: f32) -> Option<f32> {
        self.ray_hit(ray, range).map(|(distance, _)| distance)
    }

    /// The first wall along the ray, as a distance and the normal of the face
    /// crossed. The crossed face, not the nearest: a bounce off the wrong
    /// normal goes through the floor.
    pub fn ray_hit(&self, ray: &Ray3d, range: f32) -> Option<(f32, Vec3)> {
        let end = ray.origin + *ray.direction * range;
        self.hulls_near(ray.origin.min(end), ray.origin.max(end))
            .filter_map(|hull| hull.ray(ray, range))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// Shove a body that is already inside a wall or a solid prop back out of
    /// it, for when the world moves instead (a floor raised mid-match, a crate
    /// dragged or stood back up onto somebody).
    ///
    /// Escapes that land inside a room beat shorter ones that do not; least
    /// depth alone pushes a body under a raised floor down through the map.
    pub fn depenetrate(&self, center: Vec3, half: Vec3) -> Vec3 {
        /// Large enough that any escape staying in the map wins.
        const OUTSIDE_MAP_PENALTY: f32 = 1.0e6;

        let mut center = center;

        // Clearing one wall can bury the body in another. Bounded, for a body
        // wedged with nowhere to go.
        for _ in 0..4 {
            let Some(escapes) = self.escapes(center, half) else { break };

            let mut best: Option<(f32, Vec3)> = None;
            for moved in escapes {
                let mut score = (moved - center).length();
                if !self.inside_map(moved) {
                    score += OUTSIDE_MAP_PENALTY;
                }
                if best.is_none_or(|(best_score, _)| score < best_score) {
                    best = Some((score, moved));
                }
            }

            match best {
                Some((_, moved)) => center = moved,
                None => break,
            }
        }

        center
    }

    /// The ways out of the first wall or solid prop the box is inside. A
    /// rounded prop is escaped along the axes, by its extent on each as the
    /// body would meet it moving along that axis from where it is.
    fn escapes(&self, center: Vec3, half: Vec3) -> Option<Vec<Vec3>> {
        if let Some(hull) = self.hulls_near(center - half, center + half).find(|hull| hull.overlaps(center, half)) {
            return Some(hull.escapes(center, half, SKIN).collect());
        }
        let prop = self.props.iter().find(|prop| prop.overlaps(center, half))?;
        let mut escapes = Vec::with_capacity(6);
        for axis in 0..3 {
            let (min, max) = prop.slab_along(axis, center, half)?;
            for value in [max + half[axis] + SKIN, min - half[axis] - SKIN] {
                let mut moved = center;
                moved[axis] = value;
                escapes.push(moved);
            }
        }
        Some(escapes)
    }
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

    /// Whether something facing along `axis`, either way, was touched.
    fn touched_on(touched: &Touched, axis: Vec3) -> bool {
        touched.iter().any(|n| n.dot(axis).abs() > 0.99)
    }

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
    fn a_crate_stops_a_body_walking_into_it() {
        let mut world = one_room();
        world.props.push(RoundedBox { center: Vec3::new(2.0, 0.5, 0.0), core: Vec3::splat(0.5), radius: 0.0 });

        let start = Vec3::new(0.0, HALF.y + 0.01, 0.0);
        let (end, touched) = world.slide(start, HALF, Vec3::new(3.0, 0.0, 0.0));
        assert!(touched_on(&touched, Vec3::X), "walked straight through the crate");
        assert!((end.x - (1.5 - HALF.x)).abs() < 0.01, "stopped at {}", end.x);
        assert!(!world.fits(Vec3::new(2.0, HALF.y, 0.0), HALF));
    }

    /// Fails if a crate that lands on a body leaves it inside, where a sweep
    /// lets it walk out through the far side.
    #[test]
    fn a_body_inside_a_crate_is_pushed_out_of_it() {
        let mut world = one_room();
        world.props.push(RoundedBox { center: Vec3::new(0.2, 0.5, 0.0), core: Vec3::splat(0.5), radius: 0.0 });

        let inside = Vec3::new(0.0, HALF.y + 0.01, 0.0);
        let out = world.depenetrate(inside, HALF);
        assert!(world.fits(out, HALF), "still inside the crate at {out}");
    }

    /// Head on, a pillar stops a body; off-center, its round side turns the
    /// body aside and lets it past, which a square one would not.
    #[test]
    fn a_round_pillar_is_round() {
        let mut world = one_room();
        world.props.push(RoundedBox { center: Vec3::new(2.0, 2.0, 0.0), core: Vec3::new(0.0, 2.0, 0.0), radius: 0.5 });

        let head_on = world.slide(Vec3::new(0.0, HALF.y + 0.01, 0.0), HALF, Vec3::X * 3.0).0;
        assert!((head_on.x - (1.5 - HALF.x)).abs() < 0.01, "head on stopped at {}", head_on.x);

        let glancing = world.slide(Vec3::new(0.0, HALF.y + 0.01, 0.8), HALF, Vec3::X * 3.0).0;
        assert!(glancing.z > 0.8 + 0.01, "the round side did not turn the body aside: {glancing}");
        assert!(glancing.x > head_on.x + 0.1, "head on {} glancing {}", head_on.x, glancing.x);
        assert!(world.fits(glancing, HALF), "glancing ended inside the pillar at {glancing}");
    }

    #[test]
    fn a_lone_room_is_walled_on_all_six_sides() {
        assert_eq!(one_room().hulls.len(), 6);
    }

    #[test]
    fn walking_into_a_wall_stops_at_it() {
        let world = one_room();
        let start = Vec3::new(0.0, 0.9, 0.0);
        let (end, touched) = world.slide(start, HALF, Vec3::new(100.0, 0.0, 0.0));

        assert!(touched_on(&touched, Vec3::X));
        assert!((end.x - (5.0 - HALF.x)).abs() < 0.01, "stopped at {}", end.x);
    }

    #[test]
    fn running_diagonally_into_a_wall_slides_along_it() {
        let world = one_room();
        let start = Vec3::new(0.0, 0.9, 0.0);
        let (end, touched) = world.slide(start, HALF, Vec3::new(100.0, 0.0, 2.0));

        assert!(touched_on(&touched, Vec3::X));
        assert!(!touched_on(&touched, Vec3::Z));
        assert!((end.z - 2.0).abs() < 0.01, "lost the free axis: z = {}", end.z);
    }

    #[test]
    fn falling_lands_on_the_floor() {
        let world = one_room();
        let (end, touched) = world.slide(Vec3::new(0.0, 3.0, 0.0), HALF, Vec3::new(0.0, -10.0, 0.0));

        assert!(touched_on(&touched, Vec3::Y));
        assert!((end.y - HALF.y).abs() < 0.01, "landed at {}", end.y);
    }

    /// Breaks if a slab stands out further than the solid behind it: the
    /// near face's slab would reach 0.3 m past the far face of the block.
    #[test]
    fn a_thin_blocks_walls_stand_no_further_than_the_block() {
        let mut world = CollisionWorld::default();
        world.rebuild_volumes(&[
            Volume::air(Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0))),
            Volume::solid(Room::new(Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.2, 4.0, 5.0))),
        ]);
        let ray = Ray3d::new(Vec3::new(-2.0, 1.0, 0.0), Dir3::X);
        assert!((world.ray_distance(&ray, 10.0).unwrap() - 2.0).abs() < 1e-3, "the block is not there");
        assert!(world.fits(Vec3::new(0.2 + HALF.x + 0.01, HALF.y + 0.01, 0.0), HALF), "beyond the block is walled");
        assert!(world.fits(Vec3::new(-HALF.x - 0.01, HALF.y + 0.01, 0.0), HALF), "before the block is walled");
    }

    /// The same for rooms: a floor 0.2 m thick between two stacked rooms.
    #[test]
    fn a_thin_floor_does_not_stand_up_into_the_room_above() {
        let mut world = CollisionWorld::default();
        world.rebuild(&[
            Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 3.0, 5.0)),
            Room::new(Vec3::new(-5.0, 3.2, -5.0), Vec3::new(5.0, 6.2, 5.0)),
        ]);
        assert!(world.fits(Vec3::new(0.0, 3.2 + HALF.y + 0.01, 0.0), HALF));
        assert!(world.fits(Vec3::new(0.0, 3.0 - HALF.y - 0.01, 0.0), HALF));
    }

    /// Breaks if the test stops being swept: the body lands past the slab,
    /// overlapping nothing.
    #[test]
    fn a_fast_mover_does_not_pass_through_a_wall() {
        let world = one_room();
        let start = Vec3::new(0.0, 0.9, 0.0);
        let (end, _) = world.slide(start, HALF, Vec3::new(1000.0, 0.0, 0.0));

        assert!(end.x < 5.0, "tunneled through to {}", end.x);
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
        let (end, touched) = world.slide(start, HALF, Vec3::new(4.0, 0.0, 0.0));

        assert!(!touched_on(&touched, Vec3::X), "the shared wall was solid");
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

        let (end, touched) = world.slide(Vec3::new(-2.0, 0.9, 0.0), HALF, Vec3::new(100.0, 0.0, 0.0));
        assert!(touched_on(&touched, Vec3::X));
        assert!((end.x - (5.0 - HALF.x)).abs() < 0.01, "stopped at {}", end.x);
    }

    /// Breaks if walls stop coming from the surface: a box-only bake has no
    /// diagonal to slide along.
    #[test]
    fn walking_into_a_rooms_diagonal_wall_slides_along_it() {
        let mut world = CollisionWorld::default();
        world.rebuild(&[Room::prism(vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(0.0, 10.0)], 0.0, 4.0)]);
        let start = Vec3::new(2.0, HALF.y + 0.01, 4.0);
        let (end, touched) = world.slide(start, HALF, Vec3::X * 6.0);
        assert!(touched.iter().any(|n| n.x < -0.5 && n.z < -0.5), "touched {touched:?}");
        assert!(end.z < start.z - 0.5, "did not slide along the wall: {end}");
        // The box's far corner is on the wall, x + z = 10.
        assert!((end.x + HALF.x + end.z + HALF.z - 10.0).abs() < 0.01, "{end}");
        assert!(!world.inside_map(Vec3::new(8.0, 1.0, 8.0)), "the far side of the diagonal is map");
    }

    /// A floor that is not a box: two triangles, tilted up toward `+X`.
    fn a_ramp() -> CollisionWorld {
        let mut world = one_room();
        let (a, b) = (Vec3::new(-4.0, 0.0, -4.0), Vec3::new(-4.0, 0.0, 4.0));
        let (c, d) = (Vec3::new(4.0, 2.0, 4.0), Vec3::new(4.0, 2.0, -4.0));
        let mut hulls = world.hulls.clone();
        hulls.extend([Hull::triangle(a, b, c), Hull::triangle(a, c, d)]);
        world.set_hulls(hulls);
        world
    }

    #[test]
    fn a_body_falling_onto_a_ramp_lands_on_it_and_not_on_the_floor_under_it() {
        let world = a_ramp();
        // The uphill edge, at x + 0.35, meets the slope first.
        let (end, touched) = world.slide(Vec3::new(0.0, 3.5, 0.0), HALF, Vec3::NEG_Y * 5.0);
        assert!(touched.iter().any(|n| n.y > 0.9), "touched {touched:?}");
        let surface_under_the_corner = (end.x + HALF.x + 4.0) / 4.0;
        assert!((end.y - HALF.y - surface_under_the_corner).abs() < 0.01, "landed at {end}");
    }

    #[test]
    fn a_shot_down_at_a_ramp_bounces_off_its_slope() {
        let world = a_ramp();
        let (distance, normal) = world.ray_hit(&Ray3d::new(Vec3::new(0.0, 3.0, 0.0), Dir3::NEG_Y), 10.0).unwrap();
        assert!((distance - 2.0).abs() < 1e-4, "{distance}");
        assert!(normal.abs_diff_eq(Vec3::new(-1.0, 4.0, 0.0).normalize(), 1e-5), "{normal}");
    }

    #[test]
    fn a_body_buried_in_a_ramp_is_lifted_out_of_it() {
        let world = a_ramp();
        let buried = Vec3::new(0.0, 1.2, 0.0);
        assert!(!world.fits(buried, HALF));
        let freed = world.depenetrate(buried, HALF);
        assert!(world.fits(freed, HALF), "still inside at {freed}");
        assert!(freed.y > buried.y, "pushed down through the ramp to {freed}");
    }
}
