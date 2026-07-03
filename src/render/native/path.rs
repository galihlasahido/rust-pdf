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

/// Hard cap on the number of points a *single* path object (ISO 32000-1
/// 8.5.2: everything between one path-painting operator and the next) may
/// accumulate, independent of the overall per-render operator budget (see
/// [`super::interpreter::MAX_OPERATOR_COUNT`]). The two limits guard
/// different attack shapes: the operator budget bounds *how many
/// path-construction operators* a content stream may issue in total, while
/// this bounds *how large the resulting geometry of one path object* is
/// allowed to grow to, which matters even when each construction operator
/// only ever adds a handful of points (as every one of `m`/`l`/`c`/`v`/`y`/
/// `re` does) -- a content stream that never calls a painting operator at
/// all (so the operator budget is the only other thing bounding it) can
/// still otherwise accumulate an unbounded `Vec<PathVerb/Point>` in
/// `tiny_skia::PathBuilder` purely through repeated `l`/`c` calls.
///
/// Chosen generously above what any legitimate page's single path object
/// plausibly needs (dense real-world vector art -- maps, technical
/// drawings -- rarely exceeds a few hundred thousand points in one path
/// object) while still bounding a crafted content stream's ability to grow
/// one path's memory footprint without limit.
pub(super) const MAX_PATH_POINTS_PER_PATH: usize = 1_000_000;

/// Outcome of [`PathAccumulator::reserve_points`]: whether the caller
/// should go ahead and add the geometry it was about to add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PathReserve {
    /// Under budget: go ahead and add the points.
    Ok,
    /// This reservation is the one that pushed the running total past
    /// [`MAX_PATH_POINTS_PER_PATH`] -- the caller should *not* add the
    /// points (the accumulator stops growing right here) and should emit
    /// [`super::error::RenderWarning::PathPointBudgetExceeded`] exactly
    /// once for this path object.
    JustExceeded,
    /// Already over budget from an earlier call on this same path object:
    /// go on silently dropping further construction for it (already
    /// warned once, via `JustExceeded`).
    AlreadyOver,
}

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
    /// Running count of points reserved via [`Self::reserve_points`] for
    /// this path object, since the last [`Self::clear`]. See
    /// [`MAX_PATH_POINTS_PER_PATH`].
    point_count: usize,
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

    /// Reserves `n` additional points against [`MAX_PATH_POINTS_PER_PATH`]
    /// for this path object, *without* adding any geometry itself -- call
    /// this before the corresponding `move_to`/`line_to`/`cubic_to`/`rect`
    /// call and only actually add the geometry if the result is
    /// [`PathReserve::Ok`]. Untrusted input: this is what stops a content
    /// stream consisting of millions of `l` operators inside one never-
    /// painted path object from growing `self.builder` without bound.
    pub(super) fn reserve_points(&mut self, n: usize) -> PathReserve {
        if self.point_count > MAX_PATH_POINTS_PER_PATH {
            return PathReserve::AlreadyOver;
        }
        self.point_count += n;
        if self.point_count > MAX_PATH_POINTS_PER_PATH {
            PathReserve::JustExceeded
        } else {
            PathReserve::Ok
        }
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
    /// construction operator starts a brand new one -- including its own,
    /// fresh [`MAX_PATH_POINTS_PER_PATH`] budget (`point_count` resets to
    /// 0 along with everything else, since `Self::default()` zero-inits
    /// it).
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Hard clamp on the magnitude of any single device-space coordinate this
/// interpreter will hand to `tiny-skia`.
///
/// **Found by fuzzing this phase's `render_interpreter` cargo-fuzz
/// target** (not a theoretical concern): a content stream as small as `0 0
/// m 0 1e11 l 10 10 l f` -- a path with one absurdly-large-but-*finite*
/// (not `NaN`/`Infinity`) `y` coordinate, reachable from an ordinary
/// content stream via a huge literal operand or a `cm` scale factor
/// applied to an ordinary one -- trips an internal `assert!` in
/// `tiny_skia::scan::path::fill_path_impl` (`edges[curr_idx].last_y >=
/// curr_y as i32`) and aborts the process. This directly contradicts what
/// this crate's own docs previously (incorrectly) claimed about
/// `tiny-skia`'s handling of extreme magnitudes (that it "refuses...none,
/// does not panic" -- see `native/mod.rs`'s "Untrusted input handling"
/// section, corrected alongside this fix); the true behavior is that
/// `tiny-skia`'s internal scanline conversion has its own, undocumented
/// magnitude limit and panics rather than erroring past it. Root cause is
/// inside `tiny-skia` (a dependency this crate does not maintain), not in
/// this crate's own code, so the fix here is defensive clamping at this
/// crate's boundary into `tiny-skia`, not a patch to `tiny-skia` itself --
/// same posture as the `ttf-parser` `catch_unwind` mitigation documented
/// in `docs/THREAT_MODEL.md` §4.3/§7.2 for an analogous fuzz-found panic
/// in a different rasterization dependency.
///
/// Empirically (see the regression test below), `1e10` still rendered
/// without panicking and `1e11` panicked, so this is set to `1_000_000.0`
/// (1 million) -- four orders of magnitude below the confirmed-safe value
/// and five below the confirmed-crashing one, while still being far larger
/// than any device-space coordinate a legitimate render could ever
/// plausibly produce (this crate's own `MAX_RENDER_PIXELS` bounds a
/// requested raster to 64,000,000 total *pixels*, orders of magnitude
/// below this clamp already). A legitimate render's geometry is therefore
/// never affected by this clamp; only a crafted/degenerate input's
/// coordinates are.
pub(super) const MAX_COORDINATE_MAGNITUDE: f32 = 1_000_000.0;

/// Converts a device-space `(f32, f32)` pair to a `tiny_skia::Point`,
/// substituting a finite fallback for non-finite input so a crafted
/// content stream cannot smuggle `NaN`/`Infinity` into the rasterizer via
/// an extreme/degenerate CTM, **and** clamping any finite-but-absurdly-
/// large magnitude to [`MAX_COORDINATE_MAGNITUDE`] so a crafted content
/// stream cannot instead smuggle in a value large enough to trip
/// `tiny-skia`'s own internal panic (see that constant's docs for the
/// fuzz-found crash this defends against).
pub(super) fn sanitize_point(x: f64, y: f64) -> (f32, f32) {
    let sx = if x.is_finite() { clamp_magnitude(x as f32) } else { 0.0 };
    let sy = if y.is_finite() { clamp_magnitude(y as f32) } else { 0.0 };
    (sx, sy)
}

fn clamp_magnitude(v: f32) -> f32 {
    v.clamp(-MAX_COORDINATE_MAGNITUDE, MAX_COORDINATE_MAGNITUDE)
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

    /// Adversarial: reserving points one at a time up to
    /// [`MAX_PATH_POINTS_PER_PATH`] stays `Ok`, the reservation that
    /// crosses the line reports `JustExceeded` exactly once, and every
    /// reservation after that reports `AlreadyOver` -- proving a crafted
    /// content stream of unbounded `l` operators inside one never-painted
    /// path object cannot grow `PathAccumulator` without bound.
    #[test]
    fn reserve_points_caps_and_reports_transition_once() {
        let mut acc = PathAccumulator::new();
        for _ in 0..MAX_PATH_POINTS_PER_PATH {
            assert_eq!(acc.reserve_points(1), PathReserve::Ok);
        }
        assert_eq!(acc.reserve_points(1), PathReserve::JustExceeded);
        for _ in 0..10 {
            assert_eq!(acc.reserve_points(1), PathReserve::AlreadyOver);
        }
    }

    /// Same as above but reserving in one large batch (as `re`, which
    /// reserves 4 points per call, would do near the boundary) instead of
    /// one point at a time.
    #[test]
    fn reserve_points_handles_multi_point_batches_crossing_the_limit() {
        let mut acc = PathAccumulator::new();
        assert_eq!(acc.reserve_points(MAX_PATH_POINTS_PER_PATH - 2), PathReserve::Ok);
        // This batch of 4 crosses the boundary partway through.
        assert_eq!(acc.reserve_points(4), PathReserve::JustExceeded);
        assert_eq!(acc.reserve_points(1), PathReserve::AlreadyOver);
    }

    /// `clear()` resets the point-count budget along with the rest of the
    /// accumulator, since it represents a brand new path object (ISO
    /// 32000-1 8.5.3: a path-painting operator terminates the current path
    /// object).
    #[test]
    fn clear_resets_point_budget() {
        let mut acc = PathAccumulator::new();
        assert_eq!(acc.reserve_points(MAX_PATH_POINTS_PER_PATH), PathReserve::Ok);
        assert_eq!(acc.reserve_points(1), PathReserve::JustExceeded);
        acc.clear();
        assert_eq!(acc.reserve_points(1), PathReserve::Ok);
    }

    /// Non-finite input still sanitizes to a finite fallback (pre-existing
    /// behavior, re-asserted here alongside the new magnitude clamp below).
    #[test]
    fn sanitize_point_replaces_non_finite_with_zero() {
        assert_eq!(sanitize_point(f64::NAN, f64::NAN), (0.0, 0.0));
        assert_eq!(sanitize_point(f64::INFINITY, f64::NEG_INFINITY), (0.0, 0.0));
    }

    /// Fuzz-found regression (see [`MAX_COORDINATE_MAGNITUDE`]'s docs): a
    /// finite-but-absurdly-large coordinate is clamped rather than passed
    /// through unchanged, which is what stops it from later reaching
    /// `tiny-skia`'s own internal panic on out-of-range magnitudes.
    #[test]
    fn sanitize_point_clamps_extreme_finite_magnitude() {
        let (x, y) = sanitize_point(1e11, -1e11);
        assert_eq!(x, MAX_COORDINATE_MAGNITUDE);
        assert_eq!(y, -MAX_COORDINATE_MAGNITUDE);

        // A moderate, plausible device-space coordinate passes through
        // unclamped.
        let (x, y) = sanitize_point(123.5, -45.25);
        assert_eq!(x, 123.5);
        assert_eq!(y, -45.25);
    }
}
