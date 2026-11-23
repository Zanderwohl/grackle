pub mod app_mode;
pub mod assets;
pub mod cache_path;
pub mod bot_names;
pub mod class;
pub mod round_settings;
pub mod clip_file;
pub mod dock;
pub mod edit_menu;
pub mod file_menu;
pub mod recent_files;
pub mod fonts;
pub mod attribution;
pub mod credits;
pub mod music;
pub mod pack_fonts;
pub mod swatch;
pub mod ui_shape;
pub mod damage;
pub mod team;
pub mod weapon;
pub mod weapon_file;
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
pub mod log_file;
pub mod map_sync;
pub mod match_state;
pub mod match_phase;
pub mod effects;
pub mod net;
pub mod lan_discovery;
pub mod net_events;
pub mod net_transport;
pub mod protocol;
// `pub` because the `grackle` binary wires `PerfPlugin` up from outside the library.
pub mod path_open;
pub mod perf;
pub mod screenshot;
pub mod picker;
pub mod preview_grid;
pub mod library;
pub mod list_row;
pub mod trash;
pub(crate) mod painter;
pub(crate) mod ray;
pub mod item;
pub mod csg;
pub mod world_surface;
pub mod decal;
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
pub mod postcard_print;
pub mod postcard_sync;
pub mod sky;
pub mod stable_hash;
pub mod resource;
pub mod sunlight;
pub mod lightmap;
pub mod air_light;
pub mod air_lit;
pub mod render_layers;
pub mod shortcuts;
pub mod keybind;
pub mod settings;
pub mod reorder;
pub mod toolbar;
pub mod icons;
pub mod trigger;
pub mod stairs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointResolutionError {
    NoSuchPoint,
    NoSuchReferent,
    PropagatedError,
    Other,
}
pub mod wizard;
