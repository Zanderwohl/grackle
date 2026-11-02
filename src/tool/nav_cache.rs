//! The nav bake on disk ([`bake_cache`]): `cache/nav/{hash}.bin`, the graph and, once they land,
//! its sightlines. After changing how something is baked, bump [`FORMAT`] or delete `cache/nav/`.

use crate::common::stable_hash;
use crate::game::nav::NavGraph;
use crate::tool::bake_cache;

pub const FORMAT: u32 = 7;

const FOLDER: &str = "nav";

pub fn stable_hash(text: &str) -> u64 {
    stable_hash::of_text(&format!("{FORMAT}:{text}"))
}

pub fn read(hash: u64) -> Option<NavGraph> {
    bake_cache::read(FOLDER, hash)
}

pub fn write(hash: u64, graph: NavGraph) {
    bake_cache::write(FOLDER, hash, graph);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    use crate::common::movement_rules::MovementRules;
    use crate::common::world_surface::Volume;
    use crate::game::collision::CollisionWorld;
    use crate::game::{nav, nav_field};
    use crate::tool::room::Room;

    /// Breaks if anything the bake produces fails to round-trip, the columns most of all.
    #[test]
    fn a_baked_graph_comes_back_from_the_cache_whole() {
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(&[Volume::air(Room::new(Vec3::ZERO, Vec3::new(6.0, 3.0, 4.0)))]);
        let rules = MovementRules::default();
        let mut graph = nav::graph(&world, &faces, &[], &rules, &[]);
        let pickup = nav_field::Pickup::weapon("rocket".into(), Vec3::new(1.25, 0.0, 1.25));
        graph.set_fields(nav_field::bake(&world, &graph, &[], &rules, vec![pickup], Vec::new()));

        let hash = 0x7a7;
        bake_cache::store(FOLDER, hash, graph.clone()).unwrap();
        let back = read(hash).unwrap();
        assert_eq!(back.cells(), graph.cells());
        assert_eq!(back.fields(), graph.fields());
        for id in 0..graph.cells().len() as nav::CellId {
            assert_eq!(back.links(id), graph.links(id));
        }
        assert_eq!(back.cell_at(Vec3::new(3.25, 0.0, 2.25)), graph.cell_at(Vec3::new(3.25, 0.0, 2.25)), "the columns were not rebuilt");
    }
}
