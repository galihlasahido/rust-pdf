//! Converts a `ttf-parser` glyph outline (ISO 32000-1:2008 9.6.5/9.7 glyph
//! shapes) into a device-space `tiny-skia` [`Path`], via
//! [`crate::font::truetype::TrueTypeFont::outline_glyph`].
//!
//! `ttf-parser`'s [`OutlineBuilder`] callback trait (`move_to`/`line_to`/
//! `quad_to`/`curve_to`/`close`) maps directly onto `tiny_skia::PathBuilder`'s
//! own methods of the same names/signatures -- the only real work this
//! adapter does is transform each incoming (font design unit) coordinate
//! through the caller-supplied glyph-space-to-device-space [`Matrix`]
//! before forwarding it, and sanitize the result (a crafted font combined
//! with an extreme CTM must not be able to smuggle `NaN`/`Infinity` into
//! the rasterizer, same rule [`super::path`] already enforces for regular
//! path-construction operators).

use tiny_skia::PathBuilder;

use crate::font::truetype::TrueTypeFont;
use crate::types::Matrix;

use super::path::sanitize_point;

/// Adapts a [`tiny_skia::PathBuilder`] to `ttf-parser`'s [`OutlineBuilder`]
/// trait, transforming every point through `transform` (glyph space ->
/// device space) on the way in.
struct GlyphOutlineSink<'a> {
    builder: &'a mut PathBuilder,
    transform: Matrix,
}

impl<'a> GlyphOutlineSink<'a> {
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        let (dx, dy) = self.transform.transform_point(f64::from(x), f64::from(y));
        sanitize_point(dx, dy)
    }
}

impl<'a> ttf_parser::OutlineBuilder for GlyphOutlineSink<'a> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.map(x, y);
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.map(x, y);
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.map(x1, y1);
        let (x, y) = self.map(x, y);
        self.builder.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.map(x1, y1);
        let (x2, y2) = self.map(x2, y2);
        let (x, y) = self.map(x, y);
        self.builder.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

/// Builds a device-space [`tiny_skia::Path`] for glyph `gid` of `ttf`,
/// mapping its font-design-unit outline through `glyph_to_device`
/// (typically `scale(1/units_per_em) * text-rendering-matrix * CTM`, see
/// `interpreter.rs`'s glyph-painting code).
///
/// Returns `None` if the glyph has no outline at all (e.g. `.notdef`, a
/// space, or a bitmap/color glyph this phase doesn't shape) -- callers
/// should treat that as "paint nothing for this glyph", not an error.
pub(super) fn glyph_outline_path(ttf: &TrueTypeFont, gid: u16, glyph_to_device: Matrix) -> Option<tiny_skia::Path> {
    let mut builder = PathBuilder::new();
    let mut sink = GlyphOutlineSink {
        builder: &mut builder,
        transform: glyph_to_device,
    };
    ttf.outline_glyph(gid, &mut sink)?;
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::truetype::test_support::build_test_font;

    #[test]
    fn empty_glyph_has_no_outline() {
        // `build_test_font`'s glyphs are all zero-contour (see its own
        // doc comment), so there is genuinely no outline to build.
        let bytes = build_test_font(&[('A', 1)]);
        let ttf = TrueTypeFont::load(bytes, 0).unwrap();
        assert!(glyph_outline_path(&ttf, 1, Matrix::identity()).is_none());
    }

    #[test]
    fn out_of_range_gid_does_not_panic() {
        let bytes = build_test_font(&[('A', 1)]);
        let ttf = TrueTypeFont::load(bytes, 0).unwrap();
        // gid 9999 is far beyond this fixture's glyph count.
        assert!(glyph_outline_path(&ttf, 9999, Matrix::identity()).is_none());
    }
}
