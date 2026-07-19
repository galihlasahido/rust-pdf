//! Screen <-> PDF user-space point coordinate mapping for the currently
//! displayed page. Used by interactive overlays (form field editing this
//! pass; annotation/redaction/signature-placement tools in follow-ups),
//! all of which need to translate between where the user clicked/dragged
//! on screen and the PDF user-space points `EditableDocument`'s own APIs
//! (field/annotation rects, redaction areas, signature placement) expect.
//!
//! **Known limitation, stated rather than silently wrong**: this mapper
//! assumes the page's effective `/Rotate` is 0. `EditableDocument`'s own
//! field/annotation/text-layout rectangles are documented as being in
//! *unrotated* page space, with rotation left to the caller to apply --
//! this mapper doesn't yet apply it, so overlays on a rotated page will be
//! visually offset until a follow-up teaches it the rotation transform.

use egui::{Pos2, Rect};

use crate::types::Rectangle;

/// Maps between PDF user-space points (origin bottom-left, Y up) and
/// on-screen egui points (origin top-left, Y down) for one displayed page.
pub struct PageCoords {
    page_height_pt: f64,
    /// Screen points per PDF point (`dpi / 72`, since `egui::Image`
    /// displays a texture at one screen point per texture pixel by
    /// default, and the page was rasterized at `dpi` dots per 72-point
    /// inch).
    scale: f64,
    /// Top-left of the rendered page image on screen.
    origin: Pos2,
}

impl PageCoords {
    /// `page_size_pt`: the page's raw, unrotated width/height in points
    /// (from [`crate::render::PdfRenderer::page_size_pt`]). `dpi`: the DPI
    /// the page was rendered at. `image_rect`: the screen rect the
    /// rendered page image occupies (a page image widget's
    /// `Response::rect`).
    pub fn for_page(page_size_pt: (f64, f64), dpi: f32, image_rect: Rect) -> Self {
        Self {
            page_height_pt: page_size_pt.1,
            scale: f64::from(dpi) / 72.0,
            origin: image_rect.min,
        }
    }

    /// Maps a PDF user-space point to a screen position.
    pub fn to_screen(&self, pdf_x: f64, pdf_y: f64) -> Pos2 {
        Pos2::new(
            self.origin.x + (pdf_x * self.scale) as f32,
            self.origin.y + ((self.page_height_pt - pdf_y) * self.scale) as f32,
        )
    }

    /// Maps a PDF user-space `Rectangle` to a screen `Rect`.
    pub fn rect_to_screen(&self, rect: &Rectangle) -> Rect {
        Rect::from_two_pos(
            self.to_screen(rect.llx, rect.lly),
            self.to_screen(rect.urx, rect.ury),
        )
    }

    /// Maps a screen position back to a PDF user-space point. Not used by
    /// the form-filling overlay (this pass only ever maps rects forward,
    /// to position overlays); kept for the drag-based tools (annotate,
    /// redact, place a signature) that are the natural next passes on top
    /// of this mapper -- see its own unit tests for coverage in the
    /// meantime.
    #[allow(dead_code)]
    pub fn to_page(&self, screen: Pos2) -> (f64, f64) {
        let x = f64::from(screen.x - self.origin.x) / self.scale;
        let y = self.page_height_pt - f64::from(screen.y - self.origin.y) / self.scale;
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_point() {
        let coords = PageCoords::for_page(
            (612.0, 792.0),
            96.0,
            Rect::from_min_size(Pos2::new(10.0, 20.0), egui::vec2(816.0, 1056.0)),
        );
        let screen = coords.to_screen(100.0, 700.0);
        let (x, y) = coords.to_page(screen);
        // Tolerance sized for f32 (Pos2's precision), not f64 -- the
        // round trip goes through an f32 screen position in between.
        assert!((x - 100.0).abs() < 1e-3, "x = {x}");
        assert!((y - 700.0).abs() < 1e-3, "y = {y}");
    }

    #[test]
    fn origin_maps_to_top_left_of_page_top_left() {
        // PDF (0, page_height) is the page's top-left corner -- should
        // land exactly at the image rect's top-left corner on screen.
        let image_rect = Rect::from_min_size(Pos2::new(10.0, 20.0), egui::vec2(816.0, 1056.0));
        let coords = PageCoords::for_page((612.0, 792.0), 96.0, image_rect);
        let screen = coords.to_screen(0.0, 792.0);
        assert!((screen.x - image_rect.min.x).abs() < 1e-3);
        assert!((screen.y - image_rect.min.y).abs() < 1e-3);
    }

    #[test]
    fn rect_to_screen_preserves_area_ordering() {
        let coords = PageCoords::for_page(
            (612.0, 792.0),
            96.0,
            Rect::from_min_size(Pos2::ZERO, egui::vec2(816.0, 1056.0)),
        );
        let rect = Rectangle::new(100.0, 100.0, 200.0, 150.0);
        let screen = coords.rect_to_screen(&rect);
        assert!(screen.min.x < screen.max.x);
        assert!(screen.min.y < screen.max.y);
    }
}
