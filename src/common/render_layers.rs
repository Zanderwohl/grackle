//! Every render layer in use, so two features cannot quietly claim the same
//! one. A camera with no `RenderLayers` sees [`WORLD`] and nothing else.
//!
//! Two things about layers that fail silently:
//!
//! - **`RenderLayers` is not inherited.** Every mesh entity needs its own, not
//!   just the root of whatever it is part of.
//! - **A light only lights what shares a layer with it**, and a camera only
//!   gathers lights on its layers. That is what lets a camera on a layer of its
//!   own choose its lighting rather than taking the map's.

/// The map, bodies, props: what every ordinary camera draws.
pub const WORLD: usize = 0;

/// The first-person weapon and arms, drawn by the viewmodel camera over the
/// play camera.
pub const VIEWMODEL: usize = 1;

/// Handles a tool stands in the world, which only the editor viewports draw.
pub const EDITOR_ONLY: usize = 2;

/// The thumbnail studio: its camera, its three lights and the prop sitting for
/// it. See [`crate::prop::thumbnail`].
pub const THUMBNAIL: usize = 3;

/// Gizmos, which the editor viewports and the play camera draw and the
/// forward camera does not.
pub const GIZMOS: usize = 4;

/// What is drawn once, out of sight, so its pipeline is compiled before it is
/// needed. See [`crate::game::forward_camera`].
pub const WARM_UP: usize = 5;

/// The editor's 2D shape painter and the egui camera.
pub const PAINTER: usize = 31;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_two_features_share_a_layer() {
        let all = [WORLD, VIEWMODEL, EDITOR_ONLY, THUMBNAIL, GIZMOS, WARM_UP, PAINTER];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "two features claim layer {a}");
            }
        }
    }
}
