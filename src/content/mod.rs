//! PDF Content Stream building.

mod graphics;
mod operator;
mod text;

pub use graphics::GraphicsBuilder;
pub use operator::{Operator, TextElement};
pub use text::{kern, text, TextBuilder};

use crate::color::Color;
use crate::error::PdfResult;
use crate::object::PdfStream;
use crate::types::Matrix;

/// Builder for PDF content streams.
///
/// Content streams contain operators that describe the appearance of a page.
#[derive(Debug, Default, Clone)]
pub struct ContentBuilder {
    operators: Vec<Operator>,
    state_depth: i32,
}

impl ContentBuilder {
    /// Creates a new content builder.
    pub fn new() -> Self {
        Self {
            operators: Vec::new(),
            state_depth: 0,
        }
    }

    // Graphics state

    /// Saves the current graphics state (q operator).
    pub fn save_state(mut self) -> Self {
        self.operators.push(Operator::SaveState);
        self.state_depth += 1;
        self
    }

    /// Restores the previous graphics state (Q operator).
    pub fn restore_state(mut self) -> Self {
        self.operators.push(Operator::RestoreState);
        self.state_depth -= 1;
        self
    }

    /// Concatenates the transformation matrix.
    pub fn transform(mut self, matrix: Matrix) -> Self {
        let [a, b, c, d, e, f] = matrix.to_array();
        self.operators.push(Operator::ConcatMatrix(a, b, c, d, e, f));
        self
    }

    /// Translates the coordinate system.
    pub fn translate(self, tx: f64, ty: f64) -> Self {
        self.transform(Matrix::translate(tx, ty))
    }

    /// Scales the coordinate system.
    pub fn scale(self, sx: f64, sy: f64) -> Self {
        self.transform(Matrix::scale(sx, sy))
    }

    /// Rotates the coordinate system (angle in degrees).
    pub fn rotate(self, degrees: f64) -> Self {
        self.transform(Matrix::rotate_degrees(degrees))
    }

    // Line style

    /// Sets the line width.
    pub fn line_width(mut self, width: f64) -> Self {
        self.operators.push(Operator::SetLineWidth(width));
        self
    }

    /// Sets the line cap style (0=butt, 1=round, 2=square).
    pub fn line_cap(mut self, cap: i32) -> Self {
        self.operators.push(Operator::SetLineCap(cap));
        self
    }

    /// Sets the line join style (0=miter, 1=round, 2=bevel).
    pub fn line_join(mut self, join: i32) -> Self {
        self.operators.push(Operator::SetLineJoin(join));
        self
    }

    /// Sets the miter limit.
    pub fn miter_limit(mut self, limit: f64) -> Self {
        self.operators.push(Operator::SetMiterLimit(limit));
        self
    }

    /// Sets the dash pattern.
    pub fn dash(mut self, array: Vec<f64>, phase: f64) -> Self {
        self.operators.push(Operator::SetDashPattern(array, phase));
        self
    }

    // Color

    /// Sets the stroke color.
    pub fn stroke_color(mut self, color: Color) -> Self {
        match color {
            Color::Gray(c) => self.operators.push(Operator::SetGrayStroke(c.level)),
            Color::Rgb(c) => self.operators.push(Operator::SetRgbStroke(c.r, c.g, c.b)),
            Color::Cmyk(c) => self.operators.push(Operator::SetCmykStroke(c.c, c.m, c.y, c.k)),
        }
        self
    }

    /// Sets the fill color.
    pub fn fill_color(mut self, color: Color) -> Self {
        match color {
            Color::Gray(c) => self.operators.push(Operator::SetGrayFill(c.level)),
            Color::Rgb(c) => self.operators.push(Operator::SetRgbFill(c.r, c.g, c.b)),
            Color::Cmyk(c) => self.operators.push(Operator::SetCmykFill(c.c, c.m, c.y, c.k)),
        }
        self
    }

    // Path construction

    /// Moves to a point.
    pub fn move_to(mut self, x: f64, y: f64) -> Self {
        self.operators.push(Operator::MoveTo(x, y));
        self
    }

    /// Draws a line to a point.
    pub fn line_to(mut self, x: f64, y: f64) -> Self {
        self.operators.push(Operator::LineTo(x, y));
        self
    }

    /// Draws a cubic Bezier curve.
    pub fn curve_to(mut self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) -> Self {
        self.operators.push(Operator::CurveTo(x1, y1, x2, y2, x3, y3));
        self
    }

    /// Closes the current subpath.
    pub fn close_path(mut self) -> Self {
        self.operators.push(Operator::ClosePath);
        self
    }

    /// Draws a rectangle.
    pub fn rect(mut self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.operators.push(Operator::Rectangle(x, y, width, height));
        self
    }

    // Path painting

    /// Strokes the current path.
    pub fn stroke(mut self) -> Self {
        self.operators.push(Operator::Stroke);
        self
    }

    /// Fills the current path.
    pub fn fill(mut self) -> Self {
        self.operators.push(Operator::Fill);
        self
    }

    /// Fills and strokes the current path.
    pub fn fill_and_stroke(mut self) -> Self {
        self.operators.push(Operator::FillAndStroke);
        self
    }

    /// Ends the path without painting.
    pub fn end_path(mut self) -> Self {
        self.operators.push(Operator::EndPath);
        self
    }

    // Clipping

    /// Sets clipping path (non-zero winding).
    pub fn clip(mut self) -> Self {
        self.operators.push(Operator::Clip);
        self
    }

    // Text

    /// Begins a text block with the given builder configuration.
    ///
    /// The text builder's operators are added to the content stream.
    pub fn text_block(mut self, builder: TextBuilder) -> Self {
        self.operators.extend(builder.end());
        self
    }

    /// Creates a simple text block.
    ///
    /// Convenience method for adding text at a position.
    pub fn text(self, font: &str, size: f64, x: f64, y: f64, text: &str) -> Self {
        let builder = TextBuilder::new()
            .font(font, size)
            .move_to(x, y)
            .show(text);
        self.text_block(builder)
    }

    // Graphics

    /// Adds operators from a graphics builder.
    pub fn graphics(mut self, builder: GraphicsBuilder) -> Self {
        self.operators.extend(builder.build());
        self
    }

    // XObject

    /// Paints an XObject.
    pub fn paint_xobject(mut self, name: impl Into<String>) -> Self {
        self.operators.push(Operator::PaintXObject(name.into()));
        self
    }

    /// Draws an image at the specified position and size.
    ///
    /// This is a convenience method that saves the graphics state, applies
    /// a transformation matrix to position and scale the image, paints the
    /// XObject, and restores the graphics state.
    ///
    /// # Arguments
    /// * `name` - The name of the image resource (e.g., "Img1")
    /// * `x` - X position of the lower-left corner
    /// * `y` - Y position of the lower-left corner
    /// * `width` - Display width of the image
    /// * `height` - Display height of the image
    pub fn draw_image(
        self,
        name: impl Into<String>,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Self {
        self.save_state()
            .transform(Matrix::new(width, 0.0, 0.0, height, x, y))
            .paint_xobject(name)
            .restore_state()
    }

    // Raw operator

    /// Adds a raw operator string.
    pub fn raw(mut self, op: impl Into<String>) -> Self {
        self.operators.push(Operator::Raw(op.into()));
        self
    }

    /// Adds operators from another content builder.
    pub fn extend(mut self, ops: impl IntoIterator<Item = Operator>) -> Self {
        self.operators.extend(ops);
        self
    }

    /// Builds the content stream as a string.
    pub fn build_string(&self) -> String {
        self.operators
            .iter()
            .map(|op| op.to_pdf_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Builds the content stream as bytes.
    pub fn build_bytes(&self) -> Vec<u8> {
        self.build_string().into_bytes()
    }

    /// Builds the content stream as a PDF stream object.
    pub fn build(&self) -> PdfResult<PdfStream> {
        Ok(PdfStream::new(self.build_bytes()))
    }

    /// Builds the content stream as a compressed PDF stream object.
    ///
    /// The stream is compressed using Flate compression (FlateDecode filter).
    #[cfg(feature = "compression")]
    pub fn build_compressed(&self) -> PdfResult<PdfStream> {
        let stream = PdfStream::new(self.build_bytes());
        Ok(stream.with_compression()?)
    }

    /// Returns the current state depth (for debugging).
    pub fn state_depth(&self) -> i32 {
        self.state_depth
    }

    /// Returns the operators.
    pub fn operators(&self) -> &[Operator] {
        &self.operators
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_content() {
        let content = ContentBuilder::new()
            .save_state()
            .fill_color(Color::RED)
            .rect(100.0, 100.0, 200.0, 150.0)
            .fill()
            .restore_state()
            .build_string();

        assert!(content.contains("q"));
        assert!(content.contains("1 0 0 rg"));
        assert!(content.contains("100 100 200 150 re"));
        assert!(content.contains("f"));
        assert!(content.contains("Q"));
    }

    #[test]
    fn test_text_content() {
        let content = ContentBuilder::new()
            .text("F1", 12.0, 72.0, 750.0, "Hello, World!")
            .build_string();

        assert!(content.contains("BT"));
        assert!(content.contains("/F1 12 Tf"));
        assert!(content.contains("(Hello, World!) Tj"));
        assert!(content.contains("ET"));
    }

    #[test]
    fn test_state_depth_tracking() {
        let builder = ContentBuilder::new()
            .save_state()
            .save_state();

        assert_eq!(builder.state_depth(), 2);

        let builder = builder.restore_state();
        assert_eq!(builder.state_depth(), 1);
    }

    #[test]
    fn test_graphics_builder_integration() {
        let graphics = GraphicsBuilder::new()
            .fill_color(Color::BLUE)
            .rect(0.0, 0.0, 100.0, 100.0)
            .fill();

        let content = ContentBuilder::new()
            .graphics(graphics)
            .build_string();

        assert!(content.contains("0 0 1 rg"));
        assert!(content.contains("100 100 re"));
    }

    #[test]
    fn test_build_stream() {
        let stream = ContentBuilder::new()
            .text("F1", 12.0, 72.0, 750.0, "Test")
            .build()
            .unwrap();

        assert!(!stream.is_empty());
    }
}
