//! The 14 Standard PDF Fonts.

use crate::object::{Object, PdfDictionary, PdfName};

/// The 14 standard PDF fonts that are guaranteed to be available.
///
/// These fonts don't require embedding - PDF viewers must have them built-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Standard14Font {
    /// Times Roman
    TimesRoman,
    /// Times Bold
    TimesBold,
    /// Times Italic
    TimesItalic,
    /// Times Bold Italic
    TimesBoldItalic,
    /// Helvetica
    #[default]
    Helvetica,
    /// Helvetica Bold
    HelveticaBold,
    /// Helvetica Oblique
    HelveticaOblique,
    /// Helvetica Bold Oblique
    HelveticaBoldOblique,
    /// Courier
    Courier,
    /// Courier Bold
    CourierBold,
    /// Courier Oblique
    CourierOblique,
    /// Courier Bold Oblique
    CourierBoldOblique,
    /// Symbol
    Symbol,
    /// Zapf Dingbats
    ZapfDingbats,
}

impl Standard14Font {
    /// Returns the PostScript name of the font.
    pub fn postscript_name(&self) -> &'static str {
        match self {
            Standard14Font::TimesRoman => "Times-Roman",
            Standard14Font::TimesBold => "Times-Bold",
            Standard14Font::TimesItalic => "Times-Italic",
            Standard14Font::TimesBoldItalic => "Times-BoldItalic",
            Standard14Font::Helvetica => "Helvetica",
            Standard14Font::HelveticaBold => "Helvetica-Bold",
            Standard14Font::HelveticaOblique => "Helvetica-Oblique",
            Standard14Font::HelveticaBoldOblique => "Helvetica-BoldOblique",
            Standard14Font::Courier => "Courier",
            Standard14Font::CourierBold => "Courier-Bold",
            Standard14Font::CourierOblique => "Courier-Oblique",
            Standard14Font::CourierBoldOblique => "Courier-BoldOblique",
            Standard14Font::Symbol => "Symbol",
            Standard14Font::ZapfDingbats => "ZapfDingbats",
        }
    }

    /// Returns true if this is a fixed-width (monospace) font.
    pub fn is_monospace(&self) -> bool {
        matches!(
            self,
            Standard14Font::Courier
                | Standard14Font::CourierBold
                | Standard14Font::CourierOblique
                | Standard14Font::CourierBoldOblique
        )
    }

    /// Returns true if this is a serif font.
    pub fn is_serif(&self) -> bool {
        matches!(
            self,
            Standard14Font::TimesRoman
                | Standard14Font::TimesBold
                | Standard14Font::TimesItalic
                | Standard14Font::TimesBoldItalic
        )
    }

    /// Returns true if this is a symbol font.
    pub fn is_symbol(&self) -> bool {
        matches!(
            self,
            Standard14Font::Symbol | Standard14Font::ZapfDingbats
        )
    }

    /// Creates a PDF font dictionary for this font.
    pub fn to_dictionary(&self) -> PdfDictionary {
        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::font()));
        dict.set("Subtype", Object::Name(PdfName::type1()));
        dict.set(
            "BaseFont",
            Object::Name(PdfName::new_unchecked(self.postscript_name())),
        );
        dict
    }

    /// Returns the approximate average character width for 1000 units.
    ///
    /// This is useful for estimating text width.
    pub fn average_width(&self) -> f64 {
        match self {
            Standard14Font::TimesRoman
            | Standard14Font::TimesBold
            | Standard14Font::TimesItalic
            | Standard14Font::TimesBoldItalic => 480.0,
            Standard14Font::Helvetica
            | Standard14Font::HelveticaBold
            | Standard14Font::HelveticaOblique
            | Standard14Font::HelveticaBoldOblique => 520.0,
            Standard14Font::Courier
            | Standard14Font::CourierBold
            | Standard14Font::CourierOblique
            | Standard14Font::CourierBoldOblique => 600.0,
            Standard14Font::Symbol | Standard14Font::ZapfDingbats => 500.0,
        }
    }

    /// Returns all 14 standard fonts.
    pub fn all() -> [Standard14Font; 14] {
        [
            Standard14Font::TimesRoman,
            Standard14Font::TimesBold,
            Standard14Font::TimesItalic,
            Standard14Font::TimesBoldItalic,
            Standard14Font::Helvetica,
            Standard14Font::HelveticaBold,
            Standard14Font::HelveticaOblique,
            Standard14Font::HelveticaBoldOblique,
            Standard14Font::Courier,
            Standard14Font::CourierBold,
            Standard14Font::CourierOblique,
            Standard14Font::CourierBoldOblique,
            Standard14Font::Symbol,
            Standard14Font::ZapfDingbats,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postscript_names() {
        assert_eq!(Standard14Font::Helvetica.postscript_name(), "Helvetica");
        assert_eq!(Standard14Font::TimesRoman.postscript_name(), "Times-Roman");
        assert_eq!(Standard14Font::Courier.postscript_name(), "Courier");
    }

    #[test]
    fn test_font_properties() {
        assert!(Standard14Font::Courier.is_monospace());
        assert!(!Standard14Font::Helvetica.is_monospace());

        assert!(Standard14Font::TimesRoman.is_serif());
        assert!(!Standard14Font::Helvetica.is_serif());

        assert!(Standard14Font::Symbol.is_symbol());
        assert!(!Standard14Font::Helvetica.is_symbol());
    }

    #[test]
    fn test_to_dictionary() {
        let dict = Standard14Font::Helvetica.to_dictionary();
        assert!(dict.contains_key("Type"));
        assert!(dict.contains_key("Subtype"));
        assert!(dict.contains_key("BaseFont"));
    }

    #[test]
    fn test_all_fonts() {
        let fonts = Standard14Font::all();
        assert_eq!(fonts.len(), 14);
    }
}
