pub mod app_mode;
pub mod assets;
pub mod class;
pub mod clip_file;
pub mod dock;
pub mod edit_menu;
pub mod file_menu;
pub mod fonts;
pub mod swatch;
pub mod ui_shape;
pub mod damage;
pub mod team;
pub mod weapon;
pub mod weapon_file;
pub mod pack_sync;
pub mod class_file;
pub mod layer_file;
pub mod taunt;
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
// `pub` because the `editor` binary wires `PerfPlugin` up from outside the library.
pub mod perf;
pub(crate) mod painter;
pub(crate) mod ray;
pub mod item;
pub mod csg;
pub mod world_surface;
pub mod cuboid;
pub mod rotation;
pub mod triangulate;
pub mod mode;
pub mod game_rules;
pub mod player_color;
pub mod player_settings;
pub mod movement_rules;
pub mod grid;
pub mod palette;
pub mod sky;
pub mod sunlight;
pub mod lightmap;
pub mod render_layers;
pub mod shortcuts;

#[derive(Debug)]
pub enum PointResolutionError {
    NoSuchPoint,
    NoSuchReferent,
    PropagatedError,
    Other,
}
