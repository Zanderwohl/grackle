//! The nav bake on disk: `cache/nav/{hash}.bin`, the graph and, once they land, its sightlines, as
//! postcard. Keyed by a hash of everything the bake reads, so an unchanged map opens with its graph
//! rather than baking it again. Derived and safe to delete; native only, like the thumbnails, and
//! never under test.
//!
//! The key is of the map, not of the code: after changing how something is baked, bump [`FORMAT`]
//! or delete `cache/nav/`.

use serde::{Deserialize, Serialize};

use crate::game::nav::NavGraph;

/// Folded into every key.
pub const FORMAT: u32 = 3;

pub fn stable_hash(text: &str) -> u64 {
    fnv(&format!("{FORMAT}:{text}"))
}

/// FNV-1a, 64 bits. Fixed, unlike `DefaultHasher`, whose output may change with the toolchain and
/// would quietly turn every file into a miss.
fn fnv(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[derive(Serialize, Deserialize)]
struct Cached {
    hash: u64,
    graph: NavGraph,
}

#[cfg(not(any(target_arch = "wasm32", test)))]
mod disk {
    use std::path::PathBuf;

    use bevy::prelude::*;
    use bevy::tasks::IoTaskPool;

    use super::*;

    const DIR: &str = "cache/nav";

    fn path(hash: u64) -> PathBuf {
        PathBuf::from(format!("{DIR}/{hash:016x}.bin"))
    }

    /// `None` on a miss, or a file that does not decode: one left half-written, or from an older
    /// format.
    pub fn read(hash: u64) -> Option<NavGraph> {
        let bytes = std::fs::read(path(hash)).ok()?;
        match postcard::from_bytes::<Cached>(&bytes) {
            Ok(cached) if cached.hash == hash => Some(cached.graph),
            Ok(_) => None,
            Err(e) => {
                warn!("Ignoring {}: {e}", path(hash).display());
                None
            }
        }
    }

    /// On the IO pool, through a temporary file renamed into place, so a reader never sees half.
    pub fn write(hash: u64, graph: NavGraph) {
        IoTaskPool::get()
            .spawn(async move {
                let written = postcard::to_allocvec(&Cached { hash, graph }).map_err(|e| e.to_string()).and_then(|bytes| {
                    let (path, partial) = (path(hash), path(hash).with_extension("partial"));
                    std::fs::create_dir_all(DIR).map_err(|e| e.to_string())?;
                    std::fs::write(&partial, bytes).map_err(|e| e.to_string())?;
                    std::fs::rename(&partial, &path).map_err(|e| e.to_string())
                });
                if let Err(e) = written {
                    warn!("Could not cache the nav graph: {e}");
                }
            })
            .detach();
    }
}

/// Tests bake small rooms that would otherwise be read back from the real cache by the next run.
#[cfg(any(target_arch = "wasm32", test))]
mod disk {
    use super::*;

    pub fn read(_: u64) -> Option<NavGraph> {
        None
    }

    pub fn write(_: u64, _: NavGraph) {}
}

pub use disk::{read, write};

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    use crate::common::movement_rules::MovementRules;
    use crate::common::world_surface::Volume;
    use crate::game::collision::CollisionWorld;
    use crate::game::{nav, nav_field};
    use crate::tool::room::Room;

    /// Breaks if the key's function changes, which would orphan every cached file. FNV-1a of
    /// "1:grackle" is fixed arithmetic, whatever the toolchain.
    #[test]
    fn the_key_is_the_same_on_every_build() {
        assert_eq!(fnv("1:grackle"), 0xb17f_e6b2_dd3c_1ffb);
    }

    /// Breaks if anything the bake produces fails to round-trip, the columns most of all.
    #[test]
    fn a_baked_graph_comes_back_from_the_wire_format_whole() {
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(&[Volume::air(Room::new(Vec3::ZERO, Vec3::new(6.0, 3.0, 4.0)))]);
        let rules = MovementRules::default();
        let mut graph = nav::graph(&world, &faces, &[], &rules, &[]);
        let pickup = nav_field::Pickup { label: "rocket".into(), feet: Vec3::new(1.25, 0.0, 1.25) };
        graph.set_fields(nav_field::bake(&world, &graph, &[], &rules, vec![pickup], Vec::new()));

        let bytes = postcard::to_allocvec(&Cached { hash: 7, graph: graph.clone() }).unwrap();
        let back = postcard::from_bytes::<Cached>(&bytes).unwrap().graph;
        assert_eq!(back.cells(), graph.cells());
        assert_eq!(back.fields(), graph.fields());
        for id in 0..graph.cells().len() as nav::CellId {
            assert_eq!(back.links(id), graph.links(id));
        }
        assert_eq!(back.cell_at(Vec3::new(3.25, 0.0, 2.25)), graph.cell_at(Vec3::new(3.25, 0.0, 2.25)), "the columns were not rebuilt");
    }
}

