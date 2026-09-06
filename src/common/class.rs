use bevy::prelude::Vec3;

/// The body metrics of the tallest class.
///
/// One place, because three things have to agree and are written in three
/// different modules: the body the game actually moves, the clearance a spawn
/// point is checked for before anyone is put there, and the gizmo the editor
/// draws so a mapper can see that clearance. If those drift, a spawn point
/// passes in the editor and wedges a player's head in the ceiling at runtime.
///
/// Classes do not exist yet. When they do, these become the maximum over all
/// of them — a spawn point has to fit whoever might use it, so the tallest
/// class is what the map is checked against.
pub const TALLEST_CLASS_HEIGHT: f32 = 2.0;

/// Where the camera sits on the tallest class, measured from the feet.
pub const TALLEST_CLASS_EYE_HEIGHT: f32 = 1.8;

/// How wide a body is, as a half-extent on the two ground axes.
pub const CLASS_RADIUS: f32 = 0.35;

/// Half-extents of a body box, for collision.
pub const CLASS_HALF_EXTENTS: Vec3 =
    Vec3::new(CLASS_RADIUS, TALLEST_CLASS_HEIGHT * 0.5, CLASS_RADIUS);

/// The centre of a body standing with its feet at `feet`.
///
/// A spawn point marks the feet, because that is the thing a mapper can see
/// and place against the floor. Collision works in centres. This is the one
/// conversion between them, so nobody has to remember which end a position
/// refers to.
pub fn body_centre_from_feet(feet: Vec3) -> Vec3 {
    feet + Vec3::Y * (TALLEST_CLASS_HEIGHT * 0.5)
}
