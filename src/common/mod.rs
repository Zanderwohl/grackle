pub mod app_mode;
pub mod class;
pub mod hitbox;
pub mod mesh;
pub mod skeleton;
pub mod lang;
pub(crate) mod perf;
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
