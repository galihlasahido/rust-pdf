//! Font handling for PDF documents.

mod metrics;
mod standard14;

pub use metrics::{calculate_helvetica_width, helvetica_char_width, FontMetrics};
pub use standard14::Standard14Font;

use crate::object::PdfDictionary;

/// A font that can be used in a PDF document.
#[derive(Debug, Clone)]
pub enum Font {
    /// One of the 14 standard PDF fonts.
    Standard14(Standard14Font),
}

impl Font {
    /// Creates a Helvetica font.
    pub fn helvetica() -> Self {
        Font::Standard14(Standard14Font::Helvetica)
    }

    /// Creates a Helvetica Bold font.
    pub fn helvetica_bold() -> Self {
        Font::Standard14(Standard14Font::HelveticaBold)
    }

    /// Creates a Times Roman font.
    pub fn times_roman() -> Self {
        Font::Standard14(Standard14Font::TimesRoman)
    }

    /// Creates a Courier font.
    pub fn courier() -> Self {
        Font::Standard14(Standard14Font::Courier)
    }

    /// Returns the PostScript name of the font.
    pub fn postscript_name(&self) -> &str {
        match self {
            Font::Standard14(f) => f.postscript_name(),
        }
    }

    /// Converts the font to a PDF dictionary.
    pub fn to_dictionary(&self) -> PdfDictionary {
        match self {
            Font::Standard14(f) => f.to_dictionary(),
        }
    }

    /// Returns font metrics for this font.
    pub fn metrics(&self) -> FontMetrics {
        match self {
            Font::Standard14(f) => FontMetrics::for_standard14(*f),
        }
    }

    /// Estimates the width of text at a given font size.
    pub fn estimate_width(&self, text: &str, font_size: f64) -> f64 {
        self.metrics().estimate_width(text, font_size)
    }
}

impl From<Standard14Font> for Font {
    fn from(f: Standard14Font) -> Self {
        Font::Standard14(f)
    }
}

impl Default for Font {
    fn default() -> Self {
        Font::helvetica()
    }
}

/// A reference to a font within a page's resources.
#[derive(Debug, Clone)]
pub struct FontRef {
    /// The resource name (e.g., "F1").
    pub name: String,
    /// The font.
    pub font: Font,
}

impl FontRef {
    /// Creates a new font reference.
    pub fn new(name: impl Into<String>, font: Font) -> Self {
        Self {
            name: name.into(),
            font,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_creation() {
        let font = Font::helvetica();
        assert_eq!(font.postscript_name(), "Helvetica");
    }

    #[test]
    fn test_font_from_standard14() {
        let font: Font = Standard14Font::Courier.into();
        assert_eq!(font.postscript_name(), "Courier");
    }

    #[test]
    fn test_font_metrics() {
        let font = Font::helvetica();
        let metrics = font.metrics();
        assert_eq!(metrics.units_per_em, 1000);
    }

    #[test]
    fn test_estimate_width() {
        let font = Font::helvetica();
        let width = font.estimate_width("Hello", 12.0);
        assert!(width > 0.0);
    }
}
