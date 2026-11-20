//! Printing pictures onto a 3:2 card of off-white grained paper. Each layer is pressed into an
//! [`Area`] of the card, desaturated a little, speckled and unevenly inked, and multiplied onto
//! what is already there, so the paper's tint and grain show through every layer. [`postcard`] is the usual card: an inked
//! outline, then a 3:2 picture an eighth of the card in from every side.
//!
//! The noise is a hash of each pixel's place and the layer's number, so every machine prints the
//! same card.

use bevy::math::Vec2;

/// Pixels of a card: exactly 3:2 with an eighth of each side whole.
pub const WIDTH: u32 = 768;
pub const HEIGHT: u32 = WIDTH * 2 / 3;
/// Pixels of [`Area::PICTURE`]: the size the bake stores its pictures at, so the usual card's
/// picture is pressed in at its own size, never resampled.
pub const PICTURE_WIDTH: u32 = WIDTH * 3 / 4;
pub const PICTURE_HEIGHT: u32 = HEIGHT * 3 / 4;
/// Pixels of transparency round the paper, so a card drawn tilted is filtered at its edge rather
/// than cut.
pub const RIM: u32 = 3;
/// Pixels of the outline's width, inside the rim.
pub const OUTLINE: u32 = 8;

/// Paper's color before grain: a warm off-white.
const PAPER: [f32; 3] = [242.0, 236.0, 222.0];
/// Levels of 255 the paper's fibers vary by, pixel to pixel, and its blotches across [`BLOTCH`].
const FIBER: f32 = 4.0;
const BLOTCH_DEPTH: f32 = 4.0;
/// Pixels across a blotch.
const BLOTCH: f32 = 40.0;

/// Share of the way each layer's colors are moved toward their gray.
const DESATURATION: f32 = 0.15;
/// The darkest a layer's ink reaches, as a level of 255, before it is multiplied onto the paper:
/// ink is never black.
const DARKEST_INK: f32 = 45.0;
/// Share of coverage the ink loses where it took to the paper worst, and the pixels across a patch
/// of it.
const INK_SKIP: f32 = 0.07;
const SKIP_PATCH: f32 = 6.0;
/// Levels of 255 a printed pixel's color is speckled by.
const SPECKLE: f32 = 5.0;

/// A part of the card, as shares of its width and height from its top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub min: Vec2,
    pub max: Vec2,
}

impl Area {
    pub const WHOLE: Area = Area { min: Vec2::ZERO, max: Vec2::ONE };
    /// The middle three quarters, an eighth of the card in from every side: 3:2 like the card.
    pub const PICTURE: Area = Area { min: Vec2::splat(0.125), max: Vec2::splat(0.875) };

    /// In whole pixels: left, top, width, height.
    fn pixels(&self) -> (u32, u32, u32, u32) {
        let size = Vec2::new(WIDTH as f32, HEIGHT as f32);
        let min = (self.min * size).round();
        let max = (self.max * size).round();
        (min.x as u32, min.y as u32, (max.x - min.x).max(1.0) as u32, (max.y - min.y).max(1.0) as u32)
    }
}

pub struct Postcard {
    card: image::RgbaImage,
    layers: u32,
}

impl Postcard {
    pub fn blank() -> Self {
        let mut card = image::RgbaImage::new(WIDTH, HEIGHT);
        for y in RIM..HEIGHT - RIM {
            for x in RIM..WIDTH - RIM {
                card.put_pixel(x, y, to_pixel(paper_at(x, y)));
            }
        }
        Self { card, layers: 0 }
    }

    /// Prints `layer`, stretched to `area`, onto the card. Where it is transparent nothing is
    /// printed; the rim takes no ink.
    pub fn print(&mut self, layer: &image::RgbaImage, area: Area) {
        self.layers += 1;
        let salt = self.layers * 16;
        let (left, top, width, height) = area.pixels();
        let resized;
        let layer = if layer.dimensions() == (width, height) {
            layer
        } else {
            resized = image::imageops::resize(layer, width, height, image::imageops::FilterType::Triangle);
            &resized
        };
        for (lx, ly, ink) in layer.enumerate_pixels() {
            let (x, y) = (left + lx, top + ly);
            if x >= WIDTH || y >= HEIGHT || ink.0[3] == 0 || self.card.get_pixel(x, y).0[3] == 0 {
                continue;
            }
            let under = self.card.get_pixel(x, y).0;
            let color = printed_ink([ink.0[0], ink.0[1], ink.0[2]], x, y, salt);
            let coverage = ink.0[3] as f32 / 255.0 * (1.0 - INK_SKIP * smooth_noise(x, y, SKIP_PATCH, salt + 1).powi(3));
            let mixed: [f32; 3] = std::array::from_fn(|channel| {
                let paper = under[channel] as f32;
                paper + (paper * color[channel] / 255.0 - paper) * coverage
            });
            self.card.put_pixel(x, y, to_pixel(mixed));
        }
    }

    pub fn into_image(self) -> image::RgbaImage {
        self.card
    }
}

/// `ink` desaturated, lifted off black and speckled, ready to multiply onto paper.
fn printed_ink(ink: [u8; 3], x: u32, y: u32, salt: u32) -> [f32; 3] {
    let [r, g, b] = ink.map(f32::from);
    let gray = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let speckle = (noise(x, y, salt + 2) - 0.5) * SPECKLE;
    [r, g, b].map(|channel| {
        let toward_gray = channel + (gray - channel) * DESATURATION;
        (DARKEST_INK + toward_gray * (1.0 - DARKEST_INK / 255.0) + speckle).clamp(0.0, 255.0)
    })
}

/// A full-card layer: black where the outline goes, clear elsewhere.
pub fn outline() -> image::RgbaImage {
    image::RgbaImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let edge = x.min(y).min(WIDTH - 1 - x).min(HEIGHT - 1 - y);
        match edge < RIM + OUTLINE {
            true => image::Rgba([0, 0, 0, 255]),
            false => image::Rgba([0, 0, 0, 0]),
        }
    })
}

impl Postcard {
    /// Blank paper with the outline printed, the same for every card, so made once: it is a third
    /// of a card's printing.
    pub fn outlined() -> Self {
        static OUTLINED: std::sync::OnceLock<image::RgbaImage> = std::sync::OnceLock::new();
        let card = OUTLINED.get_or_init(|| {
            let mut card = Postcard::blank();
            card.print(&outline(), Area::WHOLE);
            card.card
        });
        Self { card: card.clone(), layers: 1 }
    }
}

/// The usual card: blank paper, the outline, then `picture` in [`Area::PICTURE`]. A picture that is
/// not 3:2 is stretched to it.
pub fn postcard(picture: &image::RgbaImage) -> image::RgbaImage {
    let mut card = Postcard::outlined();
    card.print(picture, Area::PICTURE);
    card.into_image()
}

/// A number in [0, 1) from a pixel's place and a `salt`.
fn noise(x: u32, y: u32, salt: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ salt.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xff_ffff) as f32 / 0x100_0000 as f32
}

/// Value noise across cells `cell` pixels wide, smoothly interpolated, in [0, 1).
fn smooth_noise(x: u32, y: u32, cell: f32, salt: u32) -> f32 {
    let (fx, fy) = (x as f32 / cell, y as f32 / cell);
    let (cx, cy) = (fx.floor() as u32, fy.floor() as u32);
    let ease = |t: f32| t * t * (3.0 - 2.0 * t);
    let (tx, ty) = (ease(fx.fract()), ease(fy.fract()));
    let top = noise(cx, cy, salt) * (1.0 - tx) + noise(cx + 1, cy, salt) * tx;
    let bottom = noise(cx, cy + 1, salt) * (1.0 - tx) + noise(cx + 1, cy + 1, salt) * tx;
    top * (1.0 - ty) + bottom * ty
}

fn paper_at(x: u32, y: u32) -> [f32; 3] {
    let grain = (noise(x, y, 1) - 0.5) * FIBER + (smooth_noise(x, y, BLOTCH, 2) - 0.5) * BLOTCH_DEPTH;
    PAPER.map(|channel| channel + grain)
}

fn to_pixel(color: [f32; 3]) -> image::Rgba<u8> {
    let [r, g, b] = color.map(|channel| channel.round().clamp(0.0, 255.0) as u8);
    image::Rgba([r, g, b, 255])
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARGIN_X: u32 = WIDTH / 8;
    const MARGIN_Y: u32 = HEIGHT / 8;

    fn is_paper(pixel: &image::Rgba<u8>) -> bool {
        let [r, _, b, a] = pixel.0;
        a == 255 && r > 220 && b > 200 && r < 255 && r >= b
    }

    fn is_ink(pixel: &image::Rgba<u8>) -> bool {
        let max = pixel.0[..3].iter().copied().max().unwrap();
        pixel.0[3] == 255 && max < 80 && max > 0
    }

    fn is_red(pixel: &image::Rgba<u8>) -> bool {
        let [r, g, b, _] = pixel.0;
        r as i32 > g as i32 + 80 && r as i32 > b as i32 + 80
    }

    #[test]
    fn the_picture_sits_an_eighth_in_from_every_side_of_a_paper_card_outlined_in_ink() {
        let card = postcard(&image::RgbaImage::from_pixel(300, 200, image::Rgba([255, 0, 0, 255])));
        assert_eq!(card.width() * 2, card.height() * 3);
        assert_eq!(card.get_pixel(0, 0).0[3], 0, "the rim is not transparent");
        assert!(is_ink(card.get_pixel(RIM, HEIGHT / 2)));
        assert!(is_paper(card.get_pixel(MARGIN_X - 1, HEIGHT / 2)));
        assert!(is_red(card.get_pixel(MARGIN_X, HEIGHT / 2)));
        assert!(is_red(card.get_pixel(WIDTH - MARGIN_X - 1, HEIGHT / 2)));
        assert!(is_paper(card.get_pixel(WIDTH / 2, MARGIN_Y - 1)));
        assert!(is_paper(card.get_pixel(WIDTH / 2, HEIGHT - MARGIN_Y)));
    }

    #[test]
    fn a_transparent_picture_leaves_paper_behind_it() {
        let card = postcard(&image::RgbaImage::new(300, 200));
        assert!(is_paper(card.get_pixel(WIDTH / 2, HEIGHT / 2)));
    }

    #[test]
    fn the_paper_has_grain() {
        let card = Postcard::blank().into_image();
        let row: Vec<_> = (40..80).map(|x| card.get_pixel(x, MARGIN_Y / 2).0).collect();
        assert!(row.iter().any(|pixel| *pixel != row[0]), "the paper is one flat color");
    }

    #[test]
    fn a_printed_layer_is_less_saturated_than_what_was_printed_and_not_flat() {
        let mut card = Postcard::blank();
        card.print(&image::RgbaImage::from_pixel(30, 20, image::Rgba([255, 0, 0, 255])), Area::WHOLE);
        let card = card.into_image();
        let pixel = card.get_pixel(WIDTH / 2, HEIGHT / 2).0;
        assert!(pixel[0] < 255 && pixel[1] > 0 && pixel[2] > 0, "printed red came out pure: {pixel:?}");
        let row: Vec<_> = (100..140).map(|x| card.get_pixel(x, HEIGHT / 2).0).collect();
        assert!(row.iter().any(|pixel| *pixel != row[0]), "a flat layer printed flat, with no noise");
    }

    #[test]
    fn a_later_layer_prints_over_an_earlier_one() {
        let mut card = Postcard::blank();
        card.print(&image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 255, 0, 255])), Area::WHOLE);
        let yellow = card.card.get_pixel(WIDTH / 2, HEIGHT / 2).0;
        card.print(&image::RgbaImage::from_pixel(3, 2, image::Rgba([0, 255, 255, 255])), Area::PICTURE);
        let both = card.card.get_pixel(WIDTH / 2, HEIGHT / 2).0;
        assert!(both[0] < yellow[0] / 2, "cyan over yellow did not take the red out");
        assert!(both[1] > 100, "cyan over yellow did not leave green");
    }

    #[test]
    fn an_outlined_card_is_the_blank_card_with_the_outline_printed() {
        let mut card = Postcard::blank();
        card.print(&outline(), Area::WHOLE);
        assert_eq!(Postcard::outlined().into_image(), card.into_image());
    }

    #[test]
    fn the_picture_area_is_the_picture_size() {
        let (_, _, width, height) = Area::PICTURE.pixels();
        assert_eq!((width, height), (PICTURE_WIDTH, PICTURE_HEIGHT));
        assert_eq!(PICTURE_WIDTH * 2, PICTURE_HEIGHT * 3);
    }

    #[test]
    fn the_same_picture_prints_the_same_card_every_time() {
        let picture = image::RgbaImage::from_fn(30, 20, |x, y| image::Rgba([(x * 8) as u8, (y * 12) as u8, 90, 255]));
        assert_eq!(postcard(&picture), postcard(&picture));
    }
}
