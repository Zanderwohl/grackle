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

/// A body's parts, hat and held weapon while the view is inside its head. The play camera skips
/// it and the weapon editor's stage camera draws it; `Visibility::Hidden` would hide it from both.
pub const INSIDE_HEAD: usize = 6;

/// The class picker's portraits: each class's body, its camera and the lights they share. See
/// [`crate::game::class_picker`].
pub const PORTRAIT: usize = 7;

/// The HUD's portrait of the viewed body: its copy, camera and lights. See
/// [`crate::game::hud_portrait`].
pub const HUD_PORTRAIT: usize = 8;

/// The first of four layers each drawn by one editor viewport alone, for
/// gizmos faded by that view's zoom. See [`crate::editor::viewport_gizmos`].
pub const VIEWPORT_FIRST: usize = 9;

/// The editor's 2D shape painter and the egui camera.
pub const PAINTER: usize = 31;

/// [`INSIDE_HEAD`] when the view is inside the body these are drawn for, else [`WORLD`].
pub fn body_layers(inside_head: bool) -> bevy::camera::visibility::RenderLayers {
    bevy::camera::visibility::RenderLayers::layer(if inside_head { INSIDE_HEAD } else { WORLD })
}

/// A body part's, hat's or held weapon's layers: its body's own when the body is drawn somewhere
/// of its own (a portrait), else [`body_layers`].
pub fn part_layers(
    body: Option<&bevy::camera::visibility::RenderLayers>,
    inside_head: bool,
) -> bevy::camera::visibility::RenderLayers {
    body.cloned().unwrap_or_else(|| body_layers(inside_head))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Breaks if a part of a body drawn somewhere of its own is put on the world, or inside the head.
    #[test]
    fn a_part_takes_its_bodys_own_layers() {
        use bevy::camera::visibility::RenderLayers;
        let portrait = RenderLayers::layer(HUD_PORTRAIT);
        assert_eq!(part_layers(Some(&portrait), true), portrait);
        assert_eq!(part_layers(None, true), RenderLayers::layer(INSIDE_HEAD));
        assert_eq!(part_layers(None, false), RenderLayers::layer(WORLD));
    }

    #[test]
    fn no_two_features_share_a_layer() {
        let viewports = VIEWPORT_FIRST..VIEWPORT_FIRST + 4;
        let all: Vec<usize> = [WORLD, VIEWMODEL, EDITOR_ONLY, THUMBNAIL, GIZMOS, WARM_UP, INSIDE_HEAD, PORTRAIT, HUD_PORTRAIT, PAINTER]
            .into_iter()
            .chain(viewports)
            .collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "two features claim layer {a}");
            }
        }
    }
}
