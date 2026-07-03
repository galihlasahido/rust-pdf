//! Font handling for PDF documents.
//!
//! Beyond the always-available [`Standard14Font`]s, the `fonts` feature adds
//! embedded TrueType/OpenType support:
//! - [`truetype`] loads/validates a font program via `ttf-parser`.
//! - [`cid`] builds Type 0/CIDFontType2 composite fonts from one (ISO
//!   32000-1:2008 9.7), the mechanism used for CJK and any other text whose
//!   character codes don't fit in a single byte.
//! - [`subset`] reduces an embedded font program to only the glyphs a
//!   document actually uses.
//! - [`tounicode`] generates and parses `/ToUnicode` CMaps (9.10.3) for
//!   text extraction.
//! - [`encoding`] provides a fallback code-to-Unicode table for simple
//!   fonts that ship no `/ToUnicode` CMap.

mod metrics;
mod standard14;

#[cfg(feature = "fonts")]
pub mod cid;
#[cfg(feature = "fonts")]
pub mod subset;
#[cfg(feature = "fonts")]
pub mod truetype;

pub mod encoding;
pub mod tounicode;

// Used only by `render::native::font` (under `native-render`) -
// `pub(crate)`, not part of this crate's public API. `system.rs` itself
// handles the `system-fonts` feature gate internally (its
// `load_system_font_bytes` always exists, just always returns `None`
// when the feature is off), so this declaration isn't feature-gated.
pub(crate) mod system;

pub use metrics::{calculate_helvetica_width, helvetica_char_width, FontMetrics};
pub use standard14::Standard14Font;

#[cfg(feature = "fonts")]
pub use cid::CompositeFont;

use crate::object::PdfDictionary;

/// A font that can be used in a PDF document.
#[derive(Debug, Clone)]
pub enum Font {
    /// One of the 14 standard PDF fonts.
    Standard14(Standard14Font),
    /// An embedded TrueType/OpenType font, written as a Type 0/CIDFontType2
    /// composite font (`fonts` feature). See [`crate::font::cid`].
    #[cfg(feature = "fonts")]
    Composite(CompositeFont),
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
            #[cfg(feature = "fonts")]
            Font::Composite(f) => f.base_font_name(),
        }
    }

    /// Converts the font to a PDF dictionary.
    ///
    /// # Composite fonts
    /// A Type 0 composite font is a graph of several indirect PDF objects
    /// (descendant CIDFont, FontDescriptor, embedded font program stream,
    /// `/ToUnicode` stream — ISO 32000-1 9.7), not a single dictionary, so
    /// this only returns the top-level `/Type0` dictionary *skeleton*
    /// (missing `/DescendantFonts` and `/ToUnicode`, which need allocated
    /// object IDs). Use [`crate::document::Document::write_to`] (via
    /// [`crate::page::Page::add_font`]) to actually embed a composite font;
    /// this method exists mainly for introspection/testing.
    pub fn to_dictionary(&self) -> PdfDictionary {
        match self {
            Font::Standard14(f) => f.to_dictionary(),
            #[cfg(feature = "fonts")]
            Font::Composite(f) => {
                let mut dict = PdfDictionary::new();
                dict.set("Type", crate::object::Object::Name(crate::object::PdfName::font()));
                dict.set(
                    "Subtype",
                    crate::object::Object::Name(crate::object::PdfName::new_unchecked("Type0")),
                );
                dict.set(
                    "BaseFont",
                    crate::object::Object::Name(crate::object::PdfName::new_unchecked(
                        f.base_font_name(),
                    )),
                );
                dict.set(
                    "Encoding",
                    crate::object::Object::Name(crate::object::PdfName::new_unchecked(
                        "Identity-H",
                    )),
                );
                dict
            }
        }
    }

    /// Returns font metrics for this font.
    pub fn metrics(&self) -> FontMetrics {
        match self {
            Font::Standard14(f) => FontMetrics::for_standard14(*f),
            #[cfg(feature = "fonts")]
            Font::Composite(f) => {
                let ttf = f.font();
                let (ascender, descender, _cap_height) = ttf.descriptor_metrics_1000();
                FontMetrics {
                    units_per_em: 1000,
                    ascender: ascender.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
                    descender: descender.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
                    line_gap: 0,
                    avg_width: 500,
                }
            }
        }
    }

    /// Estimates the width of text at a given font size.
    ///
    /// For a [`Font::Composite`], this uses the font's *actual* per-glyph
    /// advance widths (via its `cmap`/`hmtx`) rather than an average-width
    /// approximation, so it is exact (missing glyphs count as `.notdef`'s
    /// width, usually 0).
    pub fn estimate_width(&self, text: &str, font_size: f64) -> f64 {
        match self {
            Font::Standard14(_) => self.metrics().estimate_width(text, font_size),
            #[cfg(feature = "fonts")]
            Font::Composite(f) => {
                let ttf = f.font();
                let total: u32 = text
                    .chars()
                    .map(|c| u32::from(ttf.glyph_id(c).map(|g| ttf.glyph_advance(g)).unwrap_or(0)))
                    .sum();
                font_size * f64::from(total) / 1000.0
            }
        }
    }
}

impl From<Standard14Font> for Font {
    fn from(f: Standard14Font) -> Self {
        Font::Standard14(f)
    }
}

#[cfg(feature = "fonts")]
impl From<CompositeFont> for Font {
    fn from(f: CompositeFont) -> Self {
        Font::Composite(f)
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
