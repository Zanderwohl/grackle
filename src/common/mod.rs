pub mod app_mode;
pub mod assets;
pub mod class;
pub mod damage;
pub mod team;
pub mod weapon;
pub mod weapon_file;
pub mod hitbox;
pub mod flame;
pub mod hitscan;
pub mod projectile;
pub mod mesh;
pub mod skeleton;
pub mod lang;
pub mod map_sync;
pub mod match_state;
pub mod effects;
pub mod net;
pub mod net_events;
pub mod net_transport;
pub mod protocol;
// `pub` rather than `pub(crate)`: the `editor` binary wires `PerfPlugin` up
// from outside the library now that it no longer compiles its own copy of the
// tree.
pub mod perf;
pub(crate) mod systems;
pub(crate) mod painter;
pub(crate) mod ray;
pub mod item;
pub mod cuboid;
pub mod rect_subtract;
pub mod rotation;
pub mod mode;

#[derive(Debug)]
pub enum PointResolutionError {
    NoSuchPoint,
    NoSuchReferent,
    PropagatedError,
    Other,
}
