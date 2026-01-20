//! Font metrics and width tables.

use super::Standard14Font;

/// Font metrics for text measurement.
pub struct FontMetrics {
    /// Units per em (typically 1000).
    pub units_per_em: u16,
    /// Ascender height.
    pub ascender: i16,
    /// Descender depth (negative).
    pub descender: i16,
    /// Line gap.
    pub line_gap: i16,
    /// Average character width.
    pub avg_width: u16,
}

impl FontMetrics {
    /// Returns approximate metrics for a standard 14 font.
    ///
    /// These are simplified metrics suitable for basic text layout.
    pub fn for_standard14(font: Standard14Font) -> Self {
        match font {
            Standard14Font::TimesRoman
            | Standard14Font::TimesBold
            | Standard14Font::TimesItalic
            | Standard14Font::TimesBoldItalic => Self {
                units_per_em: 1000,
                ascender: 683,
                descender: -217,
                line_gap: 0,
                avg_width: 480,
            },
            Standard14Font::Helvetica
            | Standard14Font::HelveticaBold
            | Standard14Font::HelveticaOblique
            | Standard14Font::HelveticaBoldOblique => Self {
                units_per_em: 1000,
                ascender: 718,
                descender: -207,
                line_gap: 0,
                avg_width: 520,
            },
            Standard14Font::Courier
            | Standard14Font::CourierBold
            | Standard14Font::CourierOblique
            | Standard14Font::CourierBoldOblique => Self {
                units_per_em: 1000,
                ascender: 629,
                descender: -157,
                line_gap: 0,
                avg_width: 600,
            },
            Standard14Font::Symbol | Standard14Font::ZapfDingbats => Self {
                units_per_em: 1000,
                ascender: 800,
                descender: -200,
                line_gap: 0,
                avg_width: 500,
            },
        }
    }

    /// Calculates the line height for a given font size.
    pub fn line_height(&self, font_size: f64) -> f64 {
        let height = self.ascender - self.descender + self.line_gap;
        font_size * height as f64 / self.units_per_em as f64
    }

    /// Estimates the width of a string in points.
    ///
    /// This is a rough estimate using average character width.
    pub fn estimate_width(&self, text: &str, font_size: f64) -> f64 {
        let char_count = text.chars().count();
        font_size * self.avg_width as f64 * char_count as f64 / self.units_per_em as f64
    }
}

/// Simple width table for common ASCII characters in Helvetica.
///
/// Widths are in 1/1000 of a unit (scaled by 1000/units_per_em).
pub fn helvetica_char_width(c: char) -> u16 {
    match c {
        ' ' => 278,
        '!' => 278,
        '"' => 355,
        '#' => 556,
        '$' => 556,
        '%' => 889,
        '&' => 667,
        '\'' => 191,
        '(' | ')' => 333,
        '*' => 389,
        '+' => 584,
        ',' => 278,
        '-' => 333,
        '.' => 278,
        '/' => 278,
        '0'..='9' => 556,
        ':' | ';' => 278,
        '<' | '=' | '>' => 584,
        '?' => 556,
        '@' => 1015,
        'A' | 'B' | 'C' | 'D' | 'E' | 'F' | 'G' | 'H' | 'I' | 'J' | 'K' | 'L' | 'M' | 'N'
        | 'O' | 'P' | 'Q' | 'R' | 'S' | 'T' | 'U' | 'V' | 'W' | 'X' | 'Y' | 'Z' => {
            match c {
                'I' => 278,
                'J' => 500,
                'M' | 'W' => 833,
                'Q' | 'O' | 'G' | 'C' | 'D' => 722,
                'A' | 'B' | 'E' | 'F' | 'H' | 'K' | 'L' | 'N' | 'P' | 'R' | 'S' | 'T' | 'U'
                | 'V' | 'X' | 'Y' | 'Z' => 611,
                _ => 611,
            }
        }
        'a'..='z' => match c {
            'i' | 'l' => 222,
            'j' => 222,
            'm' => 833,
            'w' => 722,
            'f' | 't' => 278,
            'r' => 333,
            's' => 500,
            _ => 556,
        },
        '[' | ']' => 278,
        '\\' => 278,
        '^' => 469,
        '_' => 556,
        '`' => 333,
        '{' | '}' => 334,
        '|' => 260,
        '~' => 584,
        _ => 556, // Default width
    }
}

/// Calculates the exact width of text in Helvetica.
pub fn calculate_helvetica_width(text: &str, font_size: f64) -> f64 {
    let total_width: u32 = text.chars().map(|c| helvetica_char_width(c) as u32).sum();
    font_size * total_width as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_metrics() {
        let metrics = FontMetrics::for_standard14(Standard14Font::Helvetica);
        assert_eq!(metrics.units_per_em, 1000);
        assert!(metrics.ascender > 0);
        assert!(metrics.descender < 0);
    }

    #[test]
    fn test_line_height() {
        let metrics = FontMetrics::for_standard14(Standard14Font::Helvetica);
        let height = metrics.line_height(12.0);
        assert!(height > 10.0 && height < 15.0);
    }

    #[test]
    fn test_helvetica_char_width() {
        assert_eq!(helvetica_char_width(' '), 278);
        assert_eq!(helvetica_char_width('I'), 278);
        assert_eq!(helvetica_char_width('M'), 833);
    }

    #[test]
    fn test_calculate_width() {
        let width = calculate_helvetica_width("Hello", 12.0);
        assert!(width > 0.0);
    }
}
