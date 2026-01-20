//! Color types for PDF content streams.

mod cmyk;
mod gray;
mod rgb;

pub use cmyk::CmykColor;
pub use gray::GrayColor;
pub use rgb::RgbColor;

/// A color that can be used in PDF content streams.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    /// Grayscale color.
    Gray(GrayColor),
    /// RGB color.
    Rgb(RgbColor),
    /// CMYK color.
    Cmyk(CmykColor),
}

impl Color {
    /// Creates a grayscale color.
    pub fn gray(level: f64) -> Self {
        Color::Gray(GrayColor::new_unchecked(level))
    }

    /// Creates an RGB color.
    pub fn rgb(r: f64, g: f64, b: f64) -> Self {
        Color::Rgb(RgbColor::new_unchecked(r, g, b))
    }

    /// Creates a CMYK color.
    pub fn cmyk(c: f64, m: f64, y: f64, k: f64) -> Self {
        Color::Cmyk(CmykColor::new_unchecked(c, m, y, k))
    }

    /// Creates an RGB color from 8-bit components.
    pub fn rgb_u8(r: u8, g: u8, b: u8) -> Self {
        Color::Rgb(RgbColor::from_u8(r, g, b))
    }

    /// Returns the PDF operator string for setting stroke color.
    pub fn stroke_operator(&self) -> String {
        match self {
            Color::Gray(c) => format!("{} G", format_f64(c.level)),
            Color::Rgb(c) => format!(
                "{} {} {} RG",
                format_f64(c.r),
                format_f64(c.g),
                format_f64(c.b)
            ),
            Color::Cmyk(c) => format!(
                "{} {} {} {} K",
                format_f64(c.c),
                format_f64(c.m),
                format_f64(c.y),
                format_f64(c.k)
            ),
        }
    }

    /// Returns the PDF operator string for setting fill color.
    pub fn fill_operator(&self) -> String {
        match self {
            Color::Gray(c) => format!("{} g", format_f64(c.level)),
            Color::Rgb(c) => format!(
                "{} {} {} rg",
                format_f64(c.r),
                format_f64(c.g),
                format_f64(c.b)
            ),
            Color::Cmyk(c) => format!(
                "{} {} {} {} k",
                format_f64(c.c),
                format_f64(c.m),
                format_f64(c.y),
                format_f64(c.k)
            ),
        }
    }

    // Predefined colors

    /// Black.
    pub const BLACK: Self = Color::Gray(GrayColor::BLACK);

    /// White.
    pub const WHITE: Self = Color::Gray(GrayColor::WHITE);

    /// Red.
    pub const RED: Self = Color::Rgb(RgbColor::RED);

    /// Green.
    pub const GREEN: Self = Color::Rgb(RgbColor::GREEN);

    /// Blue.
    pub const BLUE: Self = Color::Rgb(RgbColor::BLUE);
}

impl Default for Color {
    fn default() -> Self {
        Color::BLACK
    }
}

impl From<GrayColor> for Color {
    fn from(c: GrayColor) -> Self {
        Color::Gray(c)
    }
}

impl From<RgbColor> for Color {
    fn from(c: RgbColor) -> Self {
        Color::Rgb(c)
    }
}

impl From<CmykColor> for Color {
    fn from(c: CmykColor) -> Self {
        Color::Cmyk(c)
    }
}

/// Formats a float for PDF output.
fn format_f64(v: f64) -> String {
    if v == 0.0 {
        "0".to_string()
    } else if v.fract() == 0.0 {
        (v as i64).to_string()
    } else {
        let s = format!("{:.4}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gray_operators() {
        let color = Color::gray(0.5);
        assert_eq!(color.fill_operator(), "0.5 g");
        assert_eq!(color.stroke_operator(), "0.5 G");
    }

    #[test]
    fn test_rgb_operators() {
        let color = Color::rgb(1.0, 0.0, 0.0);
        assert_eq!(color.fill_operator(), "1 0 0 rg");
        assert_eq!(color.stroke_operator(), "1 0 0 RG");
    }

    #[test]
    fn test_cmyk_operators() {
        let color = Color::cmyk(1.0, 0.0, 0.0, 0.0);
        assert_eq!(color.fill_operator(), "1 0 0 0 k");
        assert_eq!(color.stroke_operator(), "1 0 0 0 K");
    }

    #[test]
    fn test_predefined_colors() {
        assert_eq!(Color::BLACK.fill_operator(), "0 g");
        assert_eq!(Color::RED.fill_operator(), "1 0 0 rg");
    }

    #[test]
    fn test_rgb_u8() {
        let color = Color::rgb_u8(255, 0, 0);
        assert_eq!(color.fill_operator(), "1 0 0 rg");
    }
}
