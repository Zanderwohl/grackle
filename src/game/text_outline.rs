//! An outline round a UI text's glyphs. Bevy's text has no stroke, and a bigger copy of the text
//! behind it would close up counters and miss thin strokes, so each glyph is drawn again in the
//! outline's color at [`SAMPLES`] points on a circle of the outline's width, under the text.
//!
//! Extracted beside Bevy's text shadows from the text's own layout. Bevy extracts only nodes it
//! marked changed, so changing or removing an outline shows only once the text changes.

use std::f32::consts::TAU;
use std::mem;

use bevy::math::Affine2;

use bevy::prelude::*;
use bevy::render::{Extract, ExtractSchedule, RenderApp};
use bevy::text::{PositionedGlyph, TextLayoutInfo};
use bevy::ui::{CalculatedClip, ComputedNode, ComputedStackIndex, ComputedUiTargetCamera, UiGlobalTransform};
use bevy::ui_render::{
    stack_z_offsets, ExtractedGlyph, ExtractedUiItem, ExtractedUiNode, ExtractedUiNodes, RenderUiSystems, UiCameraMap,
};

/// Enough that two stamps at the widest outline used are under a pixel apart.
const SAMPLES: usize = 16;
/// Under the text's own glyphs and over everything else the node draws.
const OUTLINE_Z: f32 = stack_z_offsets::TEXT - 0.002;

/// `width` is logical pixels out from every edge of every glyph.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct TextOutline {
    pub width: f32,
    pub color: Color,
}

/// Added where there is a renderer; without one the component is carried and drawn by nothing.
pub struct TextOutlinePlugin;

impl Plugin for TextOutlinePlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app.add_systems(ExtractSchedule, extract_text_outlines.in_set(RenderUiSystems::ExtractTextShadows));
    }
}

/// In logical pixels.
fn stamps(width: f32) -> impl Iterator<Item = Vec2> {
    (0..SAMPLES).map(move |i| Vec2::from_angle(TAU * i as f32 / SAMPLES as f32) * width)
}

#[allow(clippy::type_complexity)]
fn extract_text_outlines(
    mut commands: Commands,
    extracted: ResMut<ExtractedUiNodes>,
    nodes: Extract<
        Query<(
            Entity,
            &ComputedNode,
            &ComputedStackIndex,
            &UiGlobalTransform,
            &ComputedUiTargetCamera,
            &InheritedVisibility,
            Option<&CalculatedClip>,
            &TextLayoutInfo,
            &TextOutline,
        )>,
    >,
    camera_map: Extract<UiCameraMap>,
) {
    let extracted = extracted.into_inner();
    let mut cameras = camera_map.get_mapper();
    let mut glyphs = Vec::new();

    for (entity, node, stack, transform, target, visible, clip, layout, outline) in
        extracted.changed.iter().filter_map(|main| nodes.get(main.entity()).ok())
    {
        if !visible.get() || node.is_empty() || outline.width <= 0.0 {
            continue;
        }
        let Some(camera) = cameras.map(target) else { continue };
        let color: LinearRgba = outline.color.into();
        for offset in stamps(outline.width) {
            let placed = Affine2::from(*transform)
                * Affine2::from_translation(node.content_box().min + offset / node.inverse_scale_factor());
            for (i, PositionedGlyph { position, atlas_info, section_index, .. }) in layout.glyphs.iter().enumerate() {
                glyphs.push(ExtractedGlyph { color, translation: *position, rect: atlas_info.rect });
                // One node per run of glyphs sharing a section and an atlas page, as Bevy's does.
                let run_ends = layout
                    .glyphs
                    .get(i + 1)
                    .is_none_or(|next| next.section_index != *section_index || next.atlas_info.texture != atlas_info.texture);
                if run_ends {
                    extracted.uinodes.entry(entity.into()).or_insert_with(|| (camera, default())).1.insert(
                        commands.spawn_empty().id(),
                        ExtractedUiNode {
                            transform: placed,
                            z_order: stack.0 as f32 + OUTLINE_Z,
                            image: atlas_info.texture,
                            clip: clip.cloned(),
                            item: ExtractedUiItem::Glyphs { glyphs: mem::take(&mut glyphs) },
                        },
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Breaks if a stamp is not the outline's width out, or one side of the circle has none.
    #[test]
    fn the_stamps_ring_the_glyph_at_the_outline_width() {
        let ring: Vec<Vec2> = stamps(3.0).collect();
        assert_eq!(ring.len(), SAMPLES);
        assert!(ring.iter().all(|p| (p.length() - 3.0).abs() < 1e-4));
        assert!(ring.iter().map(|p| *p).sum::<Vec2>().length() < 1e-3, "lopsided");
        let gap = ring[0].distance(ring[1]);
        assert!(gap < 1.25, "{gap} pixels between stamps");
    }
}
