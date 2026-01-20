//! Text content building.

use super::operator::{Operator, TextElement};

/// Builder for text blocks within a content stream.
///
/// Text blocks are delimited by BT (Begin Text) and ET (End Text) operators.
#[derive(Debug, Default)]
pub struct TextBuilder {
    operators: Vec<Operator>,
    font_set: bool,
}

impl TextBuilder {
    /// Creates a new text builder.
    pub fn new() -> Self {
        Self {
            operators: vec![Operator::BeginText],
            font_set: false,
        }
    }

    /// Sets the font and size.
    pub fn font(mut self, name: impl Into<String>, size: f64) -> Self {
        self.operators
            .push(Operator::SetFont(name.into(), size));
        self.font_set = true;
        self
    }

    /// Moves the text position by (tx, ty) from the current position.
    pub fn move_to(mut self, tx: f64, ty: f64) -> Self {
        self.operators.push(Operator::MoveText(tx, ty));
        self
    }

    /// Sets the text matrix directly.
    pub fn matrix(mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        self.operators
            .push(Operator::SetTextMatrix(a, b, c, d, e, f));
        self
    }

    /// Sets the text position using a text matrix.
    pub fn position(self, x: f64, y: f64) -> Self {
        self.matrix(1.0, 0.0, 0.0, 1.0, x, y)
    }

    /// Shows a text string.
    pub fn show(mut self, text: impl Into<String>) -> Self {
        self.operators.push(Operator::ShowText(text.into()));
        self
    }

    /// Shows text with kerning/positioning adjustments.
    pub fn show_positioned(mut self, elements: Vec<TextElement>) -> Self {
        self.operators.push(Operator::ShowTextPositioned(elements));
        self
    }

    /// Moves to the next line (uses current leading).
    pub fn next_line(mut self) -> Self {
        self.operators.push(Operator::NextLine);
        self
    }

    /// Moves to the next line and shows text.
    pub fn next_line_show(mut self, text: impl Into<String>) -> Self {
        self.operators
            .push(Operator::NextLineShowText(text.into()));
        self
    }

    /// Sets the character spacing.
    pub fn character_spacing(mut self, spacing: f64) -> Self {
        self.operators
            .push(Operator::SetCharacterSpacing(spacing));
        self
    }

    /// Sets the word spacing.
    pub fn word_spacing(mut self, spacing: f64) -> Self {
        self.operators.push(Operator::SetWordSpacing(spacing));
        self
    }

    /// Sets the horizontal scaling (100 = normal).
    pub fn horizontal_scaling(mut self, scale: f64) -> Self {
        self.operators
            .push(Operator::SetHorizontalScaling(scale));
        self
    }

    /// Sets the leading (line height).
    pub fn leading(mut self, leading: f64) -> Self {
        self.operators.push(Operator::SetLeading(leading));
        self
    }

    /// Sets the text rendering mode.
    ///
    /// Modes: 0=fill, 1=stroke, 2=fill+stroke, 3=invisible,
    /// 4=fill+clip, 5=stroke+clip, 6=fill+stroke+clip, 7=clip
    pub fn rendering_mode(mut self, mode: i32) -> Self {
        self.operators
            .push(Operator::SetTextRenderingMode(mode));
        self
    }

    /// Sets the text rise (superscript/subscript offset).
    pub fn rise(mut self, rise: f64) -> Self {
        self.operators.push(Operator::SetTextRise(rise));
        self
    }

    /// Ends the text block and returns the operators.
    pub fn end(mut self) -> Vec<Operator> {
        self.operators.push(Operator::EndText);
        self.operators
    }

    /// Returns whether a font has been set.
    pub fn has_font(&self) -> bool {
        self.font_set
    }
}

/// Helper to create text positioned elements.
pub fn text(s: impl Into<String>) -> TextElement {
    TextElement::Text(s.into())
}

/// Helper to create positioning adjustment.
pub fn kern(amount: f64) -> TextElement {
    TextElement::Position(amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_text() {
        let ops = TextBuilder::new()
            .font("F1", 12.0)
            .move_to(72.0, 750.0)
            .show("Hello, World!")
            .end();

        assert_eq!(ops.len(), 5); // BT, Tf, Td, Tj, ET
        assert_eq!(ops[0], Operator::BeginText);
        assert_eq!(ops[4], Operator::EndText);
    }

    #[test]
    fn test_text_with_leading() {
        let ops = TextBuilder::new()
            .font("F1", 12.0)
            .position(72.0, 750.0)
            .leading(14.0)
            .show("Line 1")
            .next_line()
            .show("Line 2")
            .end();

        assert!(ops.len() > 5);
    }

    #[test]
    fn test_positioned_text() {
        let ops = TextBuilder::new()
            .font("F1", 12.0)
            .position(72.0, 750.0)
            .show_positioned(vec![text("A"), kern(-100.0), text("V")])
            .end();

        assert!(ops.iter().any(|op| matches!(op, Operator::ShowTextPositioned(_))));
    }

    #[test]
    fn test_has_font() {
        let builder = TextBuilder::new();
        assert!(!builder.has_font());

        let builder = builder.font("F1", 12.0);
        assert!(builder.has_font());
    }
}
