//! RGB Color handling.

use crate::error::ContentError;

/// An RGB color with components in the range [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RgbColor {
    /// Red component (0.0 to 1.0).
    pub r: f64,
    /// Green component (0.0 to 1.0).
    pub g: f64,
    /// Blue component (0.0 to 1.0).
    pub b: f64,
}

impl RgbColor {
    /// Creates a new RGB color.
    ///
    /// Returns an error if any component is outside [0.0, 1.0].
    pub fn new(r: f64, g: f64, b: f64) -> Result<Self, ContentError> {
        Self::validate_component(r)?;
        Self::validate_component(g)?;
        Self::validate_component(b)?;
        Ok(Self { r, g, b })
    }

    /// Creates a new RGB color without validation.
    ///
    /// # Safety
    /// The caller must ensure components are in [0.0, 1.0].
    pub fn new_unchecked(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b }
    }

    /// Creates an RGB color from 8-bit components (0-255).
    pub fn from_u8(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f64 / 255.0,
            g: g as f64 / 255.0,
            b: b as f64 / 255.0,
        }
    }

    /// Creates an RGB color from a hex string (e.g., "#FF0000" or "FF0000").
    pub fn from_hex(hex: &str) -> Result<Self, ContentError> {
        let hex = hex.trim_start_matches('#');

        if hex.len() != 6 {
            return Err(ContentError::InvalidColorValue(-1.0));
        }

        let r = u8::from_str_radix(&hex[0..2], 16)
            .map_err(|_| ContentError::InvalidColorValue(-1.0))?;
        let g = u8::from_str_radix(&hex[2..4], 16)
            .map_err(|_| ContentError::InvalidColorValue(-1.0))?;
        let b = u8::from_str_radix(&hex[4..6], 16)
            .map_err(|_| ContentError::InvalidColorValue(-1.0))?;

        Ok(Self::from_u8(r, g, b))
    }

    fn validate_component(value: f64) -> Result<(), ContentError> {
        if !(0.0..=1.0).contains(&value) {
            return Err(ContentError::InvalidColorValue(value));
        }
        Ok(())
    }

    /// Returns the color as a tuple.
    pub fn as_tuple(&self) -> (f64, f64, f64) {
        (self.r, self.g, self.b)
    }

    // Predefined colors

    /// Black (0, 0, 0).
    pub const BLACK: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
    };

    /// White (1, 1, 1).
    pub const WHITE: Self = Self {
        r: 1.0,
        g: 1.0,
        b: 1.0,
    };

    /// Red (1, 0, 0).
    pub const RED: Self = Self {
        r: 1.0,
        g: 0.0,
        b: 0.0,
    };

    /// Green (0, 1, 0).
    pub const GREEN: Self = Self {
        r: 0.0,
        g: 1.0,
        b: 0.0,
    };

    /// Blue (0, 0, 1).
    pub const BLUE: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 1.0,
    };
}

impl Default for RgbColor {
    fn default() -> Self {
        Self::BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_valid() {
        let color = RgbColor::new(0.5, 0.5, 0.5).unwrap();
        assert_eq!(color.r, 0.5);
    }

    #[test]
    fn test_new_invalid() {
        assert!(RgbColor::new(1.5, 0.0, 0.0).is_err());
        assert!(RgbColor::new(0.0, -0.1, 0.0).is_err());
    }

    #[test]
    fn test_from_u8() {
        let color = RgbColor::from_u8(255, 128, 0);
        assert_eq!(color.r, 1.0);
        assert!((color.g - 0.502).abs() < 0.01);
        assert_eq!(color.b, 0.0);
    }

    #[test]
    fn test_from_hex() {
        let color = RgbColor::from_hex("#FF0000").unwrap();
        assert_eq!(color.r, 1.0);
        assert_eq!(color.g, 0.0);
        assert_eq!(color.b, 0.0);

        let color2 = RgbColor::from_hex("00FF00").unwrap();
        assert_eq!(color2.g, 1.0);
    }

    #[test]
    fn test_predefined_colors() {
        assert_eq!(RgbColor::BLACK.r, 0.0);
        assert_eq!(RgbColor::WHITE.r, 1.0);
        assert_eq!(RgbColor::RED.r, 1.0);
    }
}
