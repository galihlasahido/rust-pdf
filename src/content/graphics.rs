//! Graphics operations for content streams.

use super::operator::Operator;
use crate::color::Color;
use crate::types::Matrix;

/// Builder for graphics operations.
#[derive(Debug, Default)]
pub struct GraphicsBuilder {
    operators: Vec<Operator>,
}

impl GraphicsBuilder {
    /// Creates a new graphics builder.
    pub fn new() -> Self {
        Self {
            operators: Vec::new(),
        }
    }

    /// Creates a graphics builder with operators.
    pub fn with_operators(operators: Vec<Operator>) -> Self {
        Self { operators }
    }

    // State operations

    /// Saves the current graphics state.
    pub fn save_state(mut self) -> Self {
        self.operators.push(Operator::SaveState);
        self
    }

    /// Restores the previous graphics state.
    pub fn restore_state(mut self) -> Self {
        self.operators.push(Operator::RestoreState);
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

    // Line style operations

    /// Sets the line width.
    pub fn line_width(mut self, width: f64) -> Self {
        self.operators.push(Operator::SetLineWidth(width));
        self
    }

    /// Sets the line cap style.
    ///
    /// 0 = butt cap, 1 = round cap, 2 = projecting square cap
    pub fn line_cap(mut self, cap: i32) -> Self {
        self.operators.push(Operator::SetLineCap(cap));
        self
    }

    /// Sets the line join style.
    ///
    /// 0 = miter join, 1 = round join, 2 = bevel join
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
    pub fn dash_pattern(mut self, array: Vec<f64>, phase: f64) -> Self {
        self.operators.push(Operator::SetDashPattern(array, phase));
        self
    }

    /// Sets a solid line (no dash).
    pub fn solid_line(self) -> Self {
        self.dash_pattern(vec![], 0.0)
    }

    /// Sets a dashed line.
    pub fn dashed_line(self, on: f64, off: f64) -> Self {
        self.dash_pattern(vec![on, off], 0.0)
    }

    // Color operations

    /// Sets the stroke color.
    pub fn stroke_color(mut self, color: Color) -> Self {
        match color {
            Color::Gray(c) => self.operators.push(Operator::SetGrayStroke(c.level)),
            Color::Rgb(c) => self.operators.push(Operator::SetRgbStroke(c.r, c.g, c.b)),
            Color::Cmyk(c) => self
                .operators
                .push(Operator::SetCmykStroke(c.c, c.m, c.y, c.k)),
        }
        self
    }

    /// Sets the fill color.
    pub fn fill_color(mut self, color: Color) -> Self {
        match color {
            Color::Gray(c) => self.operators.push(Operator::SetGrayFill(c.level)),
            Color::Rgb(c) => self.operators.push(Operator::SetRgbFill(c.r, c.g, c.b)),
            Color::Cmyk(c) => self
                .operators
                .push(Operator::SetCmykFill(c.c, c.m, c.y, c.k)),
        }
        self
    }

    // Path construction

    /// Moves to a point (starts a new subpath).
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
        self.operators
            .push(Operator::CurveTo(x1, y1, x2, y2, x3, y3));
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

    /// Closes and strokes the current path.
    pub fn close_and_stroke(mut self) -> Self {
        self.operators.push(Operator::CloseAndStroke);
        self
    }

    /// Fills the current path using non-zero winding rule.
    pub fn fill(mut self) -> Self {
        self.operators.push(Operator::Fill);
        self
    }

    /// Fills the current path using even-odd rule.
    pub fn fill_even_odd(mut self) -> Self {
        self.operators.push(Operator::FillEvenOdd);
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

    /// Sets the clipping path using non-zero winding rule.
    pub fn clip(mut self) -> Self {
        self.operators.push(Operator::Clip);
        self
    }

    /// Sets the clipping path using even-odd rule.
    pub fn clip_even_odd(mut self) -> Self {
        self.operators.push(Operator::ClipEvenOdd);
        self
    }

    // Convenience shapes

    /// Draws a line from (x1, y1) to (x2, y2) and strokes it.
    pub fn line(self, x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        self.move_to(x1, y1).line_to(x2, y2).stroke()
    }

    /// Draws a filled rectangle.
    pub fn filled_rect(self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.rect(x, y, width, height).fill()
    }

    /// Draws a stroked rectangle.
    pub fn stroked_rect(self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.rect(x, y, width, height).stroke()
    }

    /// Draws a circle (approximated with Bezier curves).
    pub fn circle(self, cx: f64, cy: f64, r: f64) -> Self {
        // Magic number for approximating a circle with cubic Bezier curves
        let k = 0.5522847498;
        let kr = k * r;

        self.move_to(cx + r, cy)
            .curve_to(cx + r, cy + kr, cx + kr, cy + r, cx, cy + r)
            .curve_to(cx - kr, cy + r, cx - r, cy + kr, cx - r, cy)
            .curve_to(cx - r, cy - kr, cx - kr, cy - r, cx, cy - r)
            .curve_to(cx + kr, cy - r, cx + r, cy - kr, cx + r, cy)
            .close_path()
    }

    /// Draws a filled circle.
    pub fn filled_circle(self, cx: f64, cy: f64, r: f64) -> Self {
        self.circle(cx, cy, r).fill()
    }

    /// Draws a stroked circle.
    pub fn stroked_circle(self, cx: f64, cy: f64, r: f64) -> Self {
        self.circle(cx, cy, r).stroke()
    }

    // XObject

    /// Paints an XObject.
    pub fn paint_xobject(mut self, name: impl Into<String>) -> Self {
        self.operators.push(Operator::PaintXObject(name.into()));
        self
    }

    // Raw operator

    /// Adds a raw operator string.
    pub fn raw(mut self, op: impl Into<String>) -> Self {
        self.operators.push(Operator::Raw(op.into()));
        self
    }

    /// Returns the accumulated operators.
    pub fn build(self) -> Vec<Operator> {
        self.operators
    }

    /// Extends with more operators.
    pub fn extend(mut self, ops: impl IntoIterator<Item = Operator>) -> Self {
        self.operators.extend(ops);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graphics_state() {
        let ops = GraphicsBuilder::new()
            .save_state()
            .line_width(2.0)
            .restore_state()
            .build();

        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0], Operator::SaveState);
        assert_eq!(ops[2], Operator::RestoreState);
    }

    #[test]
    fn test_colors() {
        let ops = GraphicsBuilder::new()
            .fill_color(Color::RED)
            .stroke_color(Color::BLACK)
            .build();

        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_shapes() {
        let ops = GraphicsBuilder::new()
            .rect(100.0, 100.0, 200.0, 150.0)
            .fill()
            .build();

        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_line() {
        let ops = GraphicsBuilder::new()
            .line(0.0, 0.0, 100.0, 100.0)
            .build();

        assert_eq!(ops.len(), 3); // m, l, S
    }

    #[test]
    fn test_circle() {
        let ops = GraphicsBuilder::new()
            .filled_circle(100.0, 100.0, 50.0)
            .build();

        // move, 4 curves, close, fill
        assert_eq!(ops.len(), 7);
    }
}
