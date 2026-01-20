//! CMYK Color handling.

use crate::error::ContentError;

/// A CMYK color with components in the range [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CmykColor {
    /// Cyan component (0.0 to 1.0).
    pub c: f64,
    /// Magenta component (0.0 to 1.0).
    pub m: f64,
    /// Yellow component (0.0 to 1.0).
    pub y: f64,
    /// Black (Key) component (0.0 to 1.0).
    pub k: f64,
}

impl CmykColor {
    /// Creates a new CMYK color.
    ///
    /// Returns an error if any component is outside [0.0, 1.0].
    pub fn new(c: f64, m: f64, y: f64, k: f64) -> Result<Self, ContentError> {
        Self::validate_component(c)?;
        Self::validate_component(m)?;
        Self::validate_component(y)?;
        Self::validate_component(k)?;
        Ok(Self { c, m, y, k })
    }

    /// Creates a new CMYK color without validation.
    ///
    /// # Safety
    /// The caller must ensure components are in [0.0, 1.0].
    pub fn new_unchecked(c: f64, m: f64, y: f64, k: f64) -> Self {
        Self { c, m, y, k }
    }

    fn validate_component(value: f64) -> Result<(), ContentError> {
        if !(0.0..=1.0).contains(&value) {
            return Err(ContentError::InvalidColorValue(value));
        }
        Ok(())
    }

    /// Returns the color as a tuple.
    pub fn as_tuple(&self) -> (f64, f64, f64, f64) {
        (self.c, self.m, self.y, self.k)
    }

    // Predefined colors

    /// Black (0, 0, 0, 1).
    pub const BLACK: Self = Self {
        c: 0.0,
        m: 0.0,
        y: 0.0,
        k: 1.0,
    };

    /// White (0, 0, 0, 0).
    pub const WHITE: Self = Self {
        c: 0.0,
        m: 0.0,
        y: 0.0,
        k: 0.0,
    };

    /// Cyan (1, 0, 0, 0).
    pub const CYAN: Self = Self {
        c: 1.0,
        m: 0.0,
        y: 0.0,
        k: 0.0,
    };

    /// Magenta (0, 1, 0, 0).
    pub const MAGENTA: Self = Self {
        c: 0.0,
        m: 1.0,
        y: 0.0,
        k: 0.0,
    };

    /// Yellow (0, 0, 1, 0).
    pub const YELLOW: Self = Self {
        c: 0.0,
        m: 0.0,
        y: 1.0,
        k: 0.0,
    };
}

impl Default for CmykColor {
    fn default() -> Self {
        Self::BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_valid() {
        let color = CmykColor::new(0.5, 0.5, 0.5, 0.5).unwrap();
        assert_eq!(color.c, 0.5);
    }

    #[test]
    fn test_new_invalid() {
        assert!(CmykColor::new(1.5, 0.0, 0.0, 0.0).is_err());
    }

    #[test]
    fn test_predefined_colors() {
        assert_eq!(CmykColor::BLACK.k, 1.0);
        assert_eq!(CmykColor::CYAN.c, 1.0);
    }
}
