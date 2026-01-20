//! PDF Rectangle type.

/// A PDF rectangle defined by lower-left and upper-right coordinates.
///
/// Used for page MediaBox, CropBox, BleedBox, TrimBox, ArtBox,
/// and other rectangular areas in PDF.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rectangle {
    /// Lower-left X coordinate.
    pub llx: f64,
    /// Lower-left Y coordinate.
    pub lly: f64,
    /// Upper-right X coordinate.
    pub urx: f64,
    /// Upper-right Y coordinate.
    pub ury: f64,
}

impl Rectangle {
    /// Creates a new rectangle from coordinates.
    pub fn new(llx: f64, lly: f64, urx: f64, ury: f64) -> Self {
        Self { llx, lly, urx, ury }
    }

    /// Creates a rectangle with origin at (0, 0) with given width and height.
    pub fn from_dimensions(width: f64, height: f64) -> Self {
        Self {
            llx: 0.0,
            lly: 0.0,
            urx: width,
            ury: height,
        }
    }

    /// Returns the width of the rectangle.
    #[inline]
    pub fn width(&self) -> f64 {
        self.urx - self.llx
    }

    /// Returns the height of the rectangle.
    #[inline]
    pub fn height(&self) -> f64 {
        self.ury - self.lly
    }

    // Standard paper sizes in points (72 points per inch)

    /// A4 paper size (210mm x 297mm = 595 x 842 points).
    pub fn a4() -> Self {
        Self::from_dimensions(595.0, 842.0)
    }

    /// A3 paper size (297mm x 420mm = 842 x 1191 points).
    pub fn a3() -> Self {
        Self::from_dimensions(842.0, 1191.0)
    }

    /// A5 paper size (148mm x 210mm = 420 x 595 points).
    pub fn a5() -> Self {
        Self::from_dimensions(420.0, 595.0)
    }

    /// US Letter paper size (8.5" x 11" = 612 x 792 points).
    pub fn letter() -> Self {
        Self::from_dimensions(612.0, 792.0)
    }

    /// US Legal paper size (8.5" x 14" = 612 x 1008 points).
    pub fn legal() -> Self {
        Self::from_dimensions(612.0, 1008.0)
    }

    /// Converts the rectangle to a PDF array representation [llx lly urx ury].
    pub fn to_array(&self) -> [f64; 4] {
        [self.llx, self.lly, self.urx, self.ury]
    }

    /// Returns a rectangle with the same dimensions but positioned at origin (0, 0).
    ///
    /// This is useful for creating appearance streams where the coordinate system
    /// starts at (0, 0).
    pub fn with_origin(&self) -> Self {
        Self::from_dimensions(self.width(), self.height())
    }
}

impl Default for Rectangle {
    fn default() -> Self {
        Self::a4()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let rect = Rectangle::new(10.0, 20.0, 100.0, 200.0);
        assert_eq!(rect.llx, 10.0);
        assert_eq!(rect.lly, 20.0);
        assert_eq!(rect.urx, 100.0);
        assert_eq!(rect.ury, 200.0);
    }

    #[test]
    fn test_from_dimensions() {
        let rect = Rectangle::from_dimensions(100.0, 200.0);
        assert_eq!(rect.llx, 0.0);
        assert_eq!(rect.lly, 0.0);
        assert_eq!(rect.urx, 100.0);
        assert_eq!(rect.ury, 200.0);
    }

    #[test]
    fn test_width_height() {
        let rect = Rectangle::new(10.0, 20.0, 110.0, 220.0);
        assert_eq!(rect.width(), 100.0);
        assert_eq!(rect.height(), 200.0);
    }

    #[test]
    fn test_a4() {
        let rect = Rectangle::a4();
        assert_eq!(rect.width(), 595.0);
        assert_eq!(rect.height(), 842.0);
    }

    #[test]
    fn test_letter() {
        let rect = Rectangle::letter();
        assert_eq!(rect.width(), 612.0);
        assert_eq!(rect.height(), 792.0);
    }

    #[test]
    fn test_to_array() {
        let rect = Rectangle::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(rect.to_array(), [1.0, 2.0, 3.0, 4.0]);
    }
}
