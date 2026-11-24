//! Gizmos drawn into one editor viewport only, for lines whose look depends
//! on that view's zoom: a grid that fades by how far apart its lines are on
//! screen cannot share one alpha across four views at four scales.
//!
//! Each viewport's camera draws its own layer ([`own_layer`]) and each layer
//! has its own gizmo group, so drawing is a macro over the four groups:
//! closures cannot be generic over the group.

use bevy::camera::visibility::RenderLayers;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::common::render_layers;

/// How many editor viewports there are, each with its own layer.
pub const VIEWPORTS: u32 = 4;

/// The layer only viewport `id` draws.
pub fn own_layer(id: u32) -> usize {
    render_layers::VIEWPORT_FIRST + id as usize
}

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct Viewport0;
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct Viewport1;
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct Viewport2;
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct Viewport3;

#[derive(SystemParam)]
pub struct ViewportGizmos<'w, 's> {
    pub v0: Gizmos<'w, 's, Viewport0>,
    pub v1: Gizmos<'w, 's, Viewport1>,
    pub v2: Gizmos<'w, 's, Viewport2>,
    pub v3: Gizmos<'w, 's, Viewport3>,
}

/// Runs `$body` once per viewport, with `$id` its `Multicam::id` and `$g` its
/// `&mut Gizmos<_>`.
#[macro_export]
macro_rules! for_each_viewport {
    ($gizmos:expr, |$id:ident, $g:ident| $body:block) => {{
        { let $id: u32 = 0; let $g = &mut $gizmos.v0; $body }
        { let $id: u32 = 1; let $g = &mut $gizmos.v1; $body }
        { let $id: u32 = 2; let $g = &mut $gizmos.v2; $body }
        { let $id: u32 = 3; let $g = &mut $gizmos.v3; $body }
    }};
}

/// Registers the four groups once; every plugin drawing with them calls it,
/// since plugins stand alone in tests.
pub fn ensure(app: &mut App) {
    if app.world().contains_resource::<ViewportGizmosRegistered>() {
        return;
    }
    app.insert_resource(ViewportGizmosRegistered);
    let config = |id: u32| GizmoConfig { render_layers: RenderLayers::layer(own_layer(id)), ..default() };
    app.insert_gizmo_config(Viewport0, config(0))
        .insert_gizmo_config(Viewport1, config(1))
        .insert_gizmo_config(Viewport2, config(2))
        .insert_gizmo_config(Viewport3, config(3));
}

#[derive(Resource)]
struct ViewportGizmosRegistered;

/// Pixels of `camera`'s viewport per meter, measured across the view at
/// `point`. `None` for a point behind a perspective camera.
pub fn pixels_per_meter(camera: &Camera, projection: &Projection, at: &GlobalTransform, point: Vec3) -> Option<f32> {
    let height = camera.logical_viewport_size()?.y;
    let meters_tall = match projection {
        Projection::Perspective(perspective) => {
            let depth = (point - at.translation()).dot(*at.forward());
            if depth <= perspective.near {
                return None;
            }
            2.0 * (perspective.fov / 2.0).tan() * depth
        }
        Projection::Orthographic(orthographic) => orthographic.area.height(),
        Projection::Custom(_) => return None,
    };
    (meters_tall > 0.0).then(|| height / meters_tall)
}

/// On-screen pixels between a decade's lines at which it starts and finishes
/// fading out, and at which the next decade up starts and finishes fading in.
const FADE_OUT: (f32, f32) = (15.0, 7.0);
const FADE_IN: (f32, f32) = (500.0, 400.0);

/// How opaque a decade whose unit is `pixels` long on screen is drawn, 0 to 1:
/// a lerp in pixels, never in time.
pub fn decade_alpha(pixels: f32) -> f32 {
    let fade_in = ((FADE_IN.0 - pixels) / (FADE_IN.0 - FADE_IN.1)).clamp(0.0, 1.0);
    fade_out(pixels).min(fade_in)
}

/// [`decade_alpha`] for the smallest decade there is, which has none below it
/// to hand over to and so stays as it is zoomed in.
pub fn fade_out(pixels: f32) -> f32 {
    ((pixels - FADE_OUT.1) / (FADE_OUT.0 - FADE_OUT.1)).clamp(0.0, 1.0)
}

/// Every unit `base` × 10ᵏ with a nonzero alpha at `pixels_per_meter`, with
/// that alpha.
pub fn decades(base: f32, pixels_per_meter: f32) -> impl Iterator<Item = (f32, f32)> {
    let base_pixels = base * pixels_per_meter;
    let (low, high) = match base_pixels > 0.0 && base_pixels.is_finite() {
        true => ((FADE_OUT.1 / base_pixels).log10().floor() as i32, (FADE_IN.0 / base_pixels).log10().ceil() as i32),
        false => (0, -1),
    };
    (low..=high).filter_map(move |k| {
        let unit = base * 10f32.powi(k);
        let alpha = decade_alpha(unit * pixels_per_meter);
        (alpha > 0.0).then_some((unit, alpha))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decade_fades_by_its_size_on_screen() {
        assert_eq!(decade_alpha(100.0), 1.0);
        assert_eq!(decade_alpha(15.0), 1.0);
        assert_eq!(decade_alpha(7.0), 0.0);
        assert!((decade_alpha(11.0) - 0.5).abs() < 1e-6);
        assert_eq!(decade_alpha(400.0), 1.0);
        assert_eq!(decade_alpha(500.0), 0.0);
        assert!((decade_alpha(450.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn some_decade_is_always_fully_shown() {
        for step in 0..2000 {
            let pixels_per_meter = 10f32.powf(step as f32 / 200.0 - 2.0);
            let shown: Vec<(f32, f32)> = decades(0.005, pixels_per_meter).collect();
            assert!(shown.iter().any(|(_, alpha)| *alpha == 1.0), "{pixels_per_meter}: {shown:?}");
            assert!(shown.len() <= 3, "{pixels_per_meter}: {shown:?}");
        }
    }
}
