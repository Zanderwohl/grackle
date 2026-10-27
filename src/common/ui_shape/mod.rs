//! Arbitrary closed shapes as ordinary UI nodes: a [`Path`] is flattened to a
//! polygon and a fragment shader draws it by signed distance, so a shape is
//! laid out by flexbox like anything else and its edges are crisp at any
//! scale.
//!
//! Distance rather than a tessellated mesh because a mesh is not a UI node —
//! it would have to be positioned from the layout after the fact, and its
//! edges antialiased by hand. Distance also makes the outline, the rim light
//! and the drop shadow one comparison each.
//!
//! A shape is authored in logical pixels inside its own box (`size`), and the
//! node should be given that size. Drawing is proportional to the node, so a
//! different `UiScale` or a high-DPI window scales the stroke with it.

pub mod path;

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

pub use path::{Corner, Path};

/// The most points one shape can have. A uniform array, not a storage
/// buffer, because WebGL2 has none.
pub const MAX_POINTS: usize = 128;

pub struct UiShapePlugin;

impl Plugin for UiShapePlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "ui_shape.wesl");
        app.add_plugins(UiMaterialPlugin::<ShapeMaterial>::default());
    }
}

/// How a shape is painted. Widths and offsets are in the shape's logical
/// pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeStyle {
    pub fill: Color,
    /// A hard-edged band along the edges facing the light, top-left.
    pub highlight: Color,
    /// The same along the edges facing away.
    pub shadow: Color,
    /// How deep both bands reach in from the stroke. Zero paints it flat.
    pub rim: f32,
    pub stroke: Color,
    /// Centred on the path, so half of it is outside.
    pub stroke_width: f32,
    /// A copy of the silhouette, stroke included, drawn behind it.
    pub drop: Color,
    pub drop_offset: Vec2,
}

impl Default for ShapeStyle {
    fn default() -> Self {
        Self {
            fill: Color::WHITE,
            highlight: Color::NONE,
            shadow: Color::NONE,
            rim: 0.0,
            stroke: Color::NONE,
            stroke_width: 0.0,
            drop: Color::NONE,
            drop_offset: Vec2::ZERO,
        }
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[uniform(0, ShapeUniform)]
pub struct ShapeMaterial {
    points: Vec<Vec2>,
    size: Vec2,
    pub style: ShapeStyle,
}

impl ShapeMaterial {
    /// Past [`MAX_POINTS`] the outline is thinned evenly, with a warning:
    /// a shape that loses detail is better than one that is not drawn.
    pub fn new(outline: Vec<Vec2>, size: Vec2, style: ShapeStyle) -> Self {
        let points = if outline.len() > MAX_POINTS {
            warn!("A UI shape has {} points; keeping {MAX_POINTS}", outline.len());
            (0..MAX_POINTS).map(|i| outline[i * outline.len() / MAX_POINTS]).collect()
        } else {
            outline
        };
        Self { points, size, style }
    }

    pub fn from_path(path: &Path, size: Vec2, style: ShapeStyle) -> Self {
        Self::new(path.flatten(), size, style)
    }

    pub fn points(&self) -> &[Vec2] {
        &self.points
    }
}

impl UiMaterial for ShapeMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://grackle/common/ui_shape/ui_shape.wesl".into()
    }
}

/// Mirrors `Shape` in `ui_shape.wesl`. Scalars are packed into `Vec4`s so the
/// two layouts cannot disagree about padding.
#[derive(ShaderType, Debug, Clone)]
pub struct ShapeUniform {
    fill: Vec4,
    highlight: Vec4,
    shadow: Vec4,
    stroke: Vec4,
    drop: Vec4,
    /// Box width, box height, stroke width, rim.
    frame: Vec4,
    /// Drop offset x, y, point count, unused.
    extra: Vec4,
    /// Two points to an element.
    points: [Vec4; MAX_POINTS / 2],
}

impl From<&ShapeMaterial> for ShapeUniform {
    fn from(material: &ShapeMaterial) -> Self {
        let colour = |c: Color| Vec4::from_array(c.to_linear().to_f32_array());
        let style = &material.style;
        let mut points = [Vec4::ZERO; MAX_POINTS / 2];
        for (i, pair) in material.points.chunks(2).enumerate() {
            let second = pair.get(1).copied().unwrap_or(pair[0]);
            points[i] = Vec4::new(pair[0].x, pair[0].y, second.x, second.y);
        }
        Self {
            fill: colour(style.fill),
            highlight: colour(style.highlight),
            shadow: colour(style.shadow),
            stroke: colour(style.stroke),
            drop: colour(style.drop),
            frame: Vec4::new(material.size.x, material.size.y, style.stroke_width, style.rim),
            extra: Vec4::new(
                style.drop_offset.x,
                style.drop_offset.y,
                material.points.len() as f32,
                0.0,
            ),
            points,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_are_packed_two_to_an_element() {
        let outline = vec![Vec2::new(1.0, 2.0), Vec2::new(3.0, 4.0), Vec2::new(5.0, 6.0)];
        let uniform = ShapeUniform::from(&ShapeMaterial::new(outline, Vec2::ONE, default()));
        assert_eq!(uniform.points[0], Vec4::new(1.0, 2.0, 3.0, 4.0));
        assert_eq!(uniform.points[1].truncate().truncate(), Vec2::new(5.0, 6.0));
        assert_eq!(uniform.extra.z, 3.0);
    }

    #[test]
    fn too_many_points_are_thinned_to_fit() {
        let outline = path::superellipse(Vec2::ZERO, Vec2::splat(10.0), 4.0, 500);
        assert_eq!(ShapeMaterial::new(outline, Vec2::ONE, default()).points().len(), MAX_POINTS);
    }
}
