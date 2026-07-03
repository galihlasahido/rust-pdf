//! Device-space path accumulation for the path-construction operators
//! (ISO 32000-1:2008 8.5.2 "Path Construction Operators", Table 59:
//! `m l c v y h re`).
//!
//! Path coordinates are transformed to device space *at the time each
//! construction operator is executed*, using whatever CTM is in effect
//! then (ISO 32000-1 8.3.4: the CTM is a graphics-state parameter, and
//! per 8.5.2.1 the points supplied to a path-construction operator are
//! interpreted in the user space established by the CTM in effect when
//! that operator is invoked). Concretely this means the accumulated
//! `tiny_skia::Path` is always already in device space, so painting
//! operators pass `Transform::identity()` to `fill_path`/`stroke_path`.
//!
//! This matches how most content-stream interpreters behave (e.g. mapping
//! each operator straight onto a stateful 2D canvas API) and correctly
//! handles the (rare, but spec-legal) case of `cm` appearing in between
//! path-construction operators of the same path object.

use tiny_skia::{Path, PathBuilder};

/// Accumulates one path object (ISO 32000-1 8.5.2) in device space across
/// possibly-multiple subpaths (`m` starts a new one), ready to be hand
/// over to `fill_path`/`stroke_path`/clip-mask intersection.
#[derive(Debug, Default)]
pub(super) struct PathAccumulator {
    builder: PathBuilder,
    /// Device-space current point, if a subpath is open.
    current: Option<(f32, f32)>,
    /// Device-space start point of the current subpath (needed by `h` /
    /// `s` / `b`* to close back to it).
    subpath_start: Option<(f32, f32)>,
}

impl PathAccumulator {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.builder.is_empty()
    }

    pub(super) fn current_point(&self) -> Option<(f32, f32)> {
        self.current
    }

    /// `m`: begins a new subpath at device-space point `(x, y)`.
    pub(super) fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(x, y);
        self.current = Some((x, y));
        self.subpath_start = Some((x, y));
    }

    /// `l`: appends a straight line segment to device-space point
    /// `(x, y)`. If no subpath is open yet (a malformed content stream
    /// invoking `l` before any `m`), starts one at `(x, y)` instead of
    /// panicking -- ISO 32000-1 doesn't define this case, and silently
    /// treating it as an implicit `m` is the least-surprising graceful
    /// fallback.
    pub(super) fn line_to(&mut self, x: f32, y: f32) {
        if self.current.is_none() {
            self.move_to(x, y);
            return;
        }
        self.builder.line_to(x, y);
        self.current = Some((x, y));
    }

    /// `c`/`v`/`y`: appends a cubic Bezier segment. Control points are
    /// already resolved to device space by the caller (`v`/`y` reuse the
    /// current point as one of the two control points per ISO 32000-1
    /// 8.5.2.2).
    pub(super) fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        if self.current.is_none() {
            self.move_to(x1, y1);
        }
        self.builder.cubic_to(x1, y1, x2, y2, x, y);
        self.current = Some((x, y));
    }

    /// `h`: closes the current subpath back to its starting point.
    pub(super) fn close(&mut self) {
        if self.current.is_none() {
            return;
        }
        self.builder.close();
        self.current = self.subpath_start;
    }

    /// `re`: appends a complete rectangular subpath. Per ISO 32000-1
    /// 8.5.2.1 this is exactly equivalent to
    /// `x y m (x+w) y l (x+w) (y+h) l x (y+h) l h`, and each of those 4
    /// corners is transformed independently by the caller so a rotated
    /// or skewed CTM correctly turns the rectangle into a parallelogram.
    pub(super) fn rect(&mut self, corners: [(f32, f32); 4]) {
        self.move_to(corners[0].0, corners[0].1);
        self.builder.line_to(corners[1].0, corners[1].1);
        self.builder.line_to(corners[2].0, corners[2].1);
        self.builder.line_to(corners[3].0, corners[3].1);
        self.builder.close();
        // `re` starts a new subpath and leaves the current point at the
        // start point (ISO 32000-1 8.5.2.1).
        self.current = self.subpath_start;
    }

    /// Produces a finished [`Path`] snapshot of the geometry accumulated
    /// so far, if any. Non-consuming (clones the builder) because a
    /// painting operator may need the same path both for painting
    /// (fill and/or stroke) *and* for a pending clip intersection (ISO
    /// 32000-1 8.5.4), and `W`/`W*`'s clip only takes effect once the
    /// following painting operator runs -- using the same snapshot for
    /// both keeps that ordering exact.
    pub(super) fn to_path(&self) -> Option<Path> {
        self.builder.clone().finish()
    }

    /// Resets the accumulator to empty. Path-painting operators (ISO
    /// 32000-1 8.5.3) terminate the current path object; the next
    /// construction operator starts a brand new one.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Converts a device-space `(f32, f32)` pair to a `tiny_skia::Point`,
/// substituting a finite fallback for non-finite input so a crafted
/// content stream cannot smuggle `NaN`/`Infinity` into the rasterizer via
/// an extreme/degenerate CTM.
pub(super) fn sanitize_point(x: f64, y: f64) -> (f32, f32) {
    let sx = if x.is_finite() { x as f32 } else { 0.0 };
    let sy = if y.is_finite() { y as f32 } else { 0.0 };
    (sx, sy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_accumulator_has_no_path() {
        let acc = PathAccumulator::new();
        assert!(acc.is_empty());
        assert!(acc.to_path().is_none());
    }

    #[test]
    fn move_line_close_produces_path() {
        let mut acc = PathAccumulator::new();
        acc.move_to(0.0, 0.0);
        acc.line_to(10.0, 0.0);
        acc.line_to(10.0, 10.0);
        acc.close();
        assert!(!acc.is_empty());
        let path = acc.to_path().expect("path should exist");
        assert!(path.bounds().width() > 0.0);
    }

    #[test]
    fn clear_resets_accumulator() {
        let mut acc = PathAccumulator::new();
        acc.move_to(0.0, 0.0);
        acc.line_to(10.0, 10.0);
        assert!(!acc.is_empty());
        acc.clear();
        assert!(acc.is_empty());
        assert!(acc.current_point().is_none());
    }

    #[test]
    fn line_without_move_starts_implicit_subpath() {
        let mut acc = PathAccumulator::new();
        acc.line_to(5.0, 5.0);
        assert_eq!(acc.current_point(), Some((5.0, 5.0)));
    }

    #[test]
    fn rect_produces_closed_subpath() {
        let mut acc = PathAccumulator::new();
        acc.rect([(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        let path = acc.to_path().expect("rect path");
        assert_eq!(path.bounds().width(), 10.0);
        assert_eq!(path.bounds().height(), 10.0);
    }
}
