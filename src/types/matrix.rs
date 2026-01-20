//! PDF Transformation Matrix.

/// A 2D transformation matrix used in PDF for coordinate transformations.
///
/// The matrix represents the transformation:
/// ```text
/// | a  b  0 |
/// | c  d  0 |
/// | e  f  1 |
/// ```
///
/// Where (x', y') = (a*x + c*y + e, b*x + d*y + f)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    /// Creates a new transformation matrix.
    pub fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }

    /// Creates an identity matrix (no transformation).
    pub fn identity() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Creates a translation matrix.
    pub fn translate(tx: f64, ty: f64) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: tx,
            f: ty,
        }
    }

    /// Creates a scaling matrix.
    pub fn scale(sx: f64, sy: f64) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Creates a uniform scaling matrix.
    pub fn scale_uniform(s: f64) -> Self {
        Self::scale(s, s)
    }

    /// Creates a rotation matrix (angle in radians).
    pub fn rotate(angle: f64) -> Self {
        let cos = angle.cos();
        let sin = angle.sin();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Creates a rotation matrix (angle in degrees).
    pub fn rotate_degrees(degrees: f64) -> Self {
        Self::rotate(degrees.to_radians())
    }

    /// Creates a skew matrix (angles in radians).
    pub fn skew(alpha: f64, beta: f64) -> Self {
        Self {
            a: 1.0,
            b: alpha.tan(),
            c: beta.tan(),
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Multiplies this matrix by another matrix (self * other).
    pub fn multiply(&self, other: &Matrix) -> Matrix {
        Matrix {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }

    /// Transforms a point (x, y) using this matrix.
    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// Converts the matrix to an array for PDF output.
    pub fn to_array(&self) -> [f64; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }
}

impl Default for Matrix {
    fn default() -> Self {
        Self::identity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f64 = 1e-10;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < EPSILON
    }

    #[test]
    fn test_identity() {
        let m = Matrix::identity();
        assert_eq!(m.a, 1.0);
        assert_eq!(m.d, 1.0);
        assert_eq!(m.e, 0.0);
        assert_eq!(m.f, 0.0);
    }

    #[test]
    fn test_translate() {
        let m = Matrix::translate(10.0, 20.0);
        let (x, y) = m.transform_point(0.0, 0.0);
        assert_eq!(x, 10.0);
        assert_eq!(y, 20.0);
    }

    #[test]
    fn test_scale() {
        let m = Matrix::scale(2.0, 3.0);
        let (x, y) = m.transform_point(5.0, 10.0);
        assert_eq!(x, 10.0);
        assert_eq!(y, 30.0);
    }

    #[test]
    fn test_rotate_90_degrees() {
        let m = Matrix::rotate_degrees(90.0);
        let (x, y) = m.transform_point(1.0, 0.0);
        assert!(approx_eq(x, 0.0));
        assert!(approx_eq(y, 1.0));
    }

    #[test]
    fn test_multiply_identity() {
        let m = Matrix::translate(10.0, 20.0);
        let result = m.multiply(&Matrix::identity());
        assert_eq!(result.e, 10.0);
        assert_eq!(result.f, 20.0);
    }

    #[test]
    fn test_to_array() {
        let m = Matrix::new(1.0, 2.0, 3.0, 4.0, 5.0, 6.0);
        assert_eq!(m.to_array(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }
}
