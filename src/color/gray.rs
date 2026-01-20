//! Grayscale Color handling.

use crate::error::ContentError;

/// A grayscale color with value in the range [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrayColor {
    /// Gray level (0.0 = black, 1.0 = white).
    pub level: f64,
}

impl GrayColor {
    /// Creates a new grayscale color.
    ///
    /// Returns an error if the level is outside [0.0, 1.0].
    pub fn new(level: f64) -> Result<Self, ContentError> {
        if !(0.0..=1.0).contains(&level) {
            return Err(ContentError::InvalidColorValue(level));
        }
        Ok(Self { level })
    }

    /// Creates a new grayscale color without validation.
    ///
    /// # Safety
    /// The caller must ensure level is in [0.0, 1.0].
    pub fn new_unchecked(level: f64) -> Self {
        Self { level }
    }

    /// Returns the gray level.
    pub fn level(&self) -> f64 {
        self.level
    }

    // Predefined colors

    /// Black (0.0).
    pub const BLACK: Self = Self { level: 0.0 };

    /// White (1.0).
    pub const WHITE: Self = Self { level: 1.0 };

    /// 50% gray (0.5).
    pub const GRAY_50: Self = Self { level: 0.5 };

    /// Light gray (0.75).
    pub const LIGHT_GRAY: Self = Self { level: 0.75 };

    /// Dark gray (0.25).
    pub const DARK_GRAY: Self = Self { level: 0.25 };
}

impl Default for GrayColor {
    fn default() -> Self {
        Self::BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_valid() {
        let color = GrayColor::new(0.5).unwrap();
        assert_eq!(color.level, 0.5);
    }

    #[test]
    fn test_new_invalid() {
        assert!(GrayColor::new(1.5).is_err());
        assert!(GrayColor::new(-0.1).is_err());
    }

    #[test]
    fn test_predefined_colors() {
        assert_eq!(GrayColor::BLACK.level, 0.0);
        assert_eq!(GrayColor::WHITE.level, 1.0);
        assert_eq!(GrayColor::GRAY_50.level, 0.5);
    }
}
