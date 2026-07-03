//! The content-stream interpreter loop: tokenizes a content stream (ISO
//! 32000-1:2008 7.8.2) and dispatches each operator to its effect on the
//! graphics state / raster, per ISO 32000-1 Table 51 ("Operator
//! Categories").
//!
//! # Implemented this phase
//!
//! - Graphics state stack: `q`, `Q` (8.4.2)
//! - CTM: `cm` (8.3.4)
//! - Path construction: `m l c v y h re` (8.5.2)
//! - Path painting: `f F f* S s B B* b b* n` (8.5.3)
//! - Clipping: `W W*` (8.5.4)
//! - Basic ExtGState: `gs` for `ca`/`CA`/`LW`/`D` only (8.4.5)
//! - Line style: `w J j M d` (8.4.3)
//! - Basic color: `g G rg RG k K` (8.6.3-8.6.5, Device color spaces only)
//!
//! Everything else -- text-showing operators, `Do` (XObjects/images),
//! `sh` (shadings), `cs`/`CS`/`sc`/`SC`/`scn`/`SCN` (non-Device color
//! spaces), marked content -- is recorded as
//! [`RenderWarning::UnsupportedOperator`] and skipped as a no-op; it does
//! **not** abort the render or panic.

use std::rc::Rc;

use tiny_skia::{Color, FillRule, Mask, Paint, Pixmap, Stroke, StrokeDash, Transform};

use crate::editor::content_stream::{parse_content_stream, ContentItem};
use crate::object::{Object, PdfDictionary};
use crate::types::{Matrix, Rectangle};

use super::color as colorspace;
use super::error::{NativeRenderError, RenderWarning};
use super::path::PathAccumulator;
use super::state::{GraphicsState, GraphicsStateStack};

/// Hard cap on `q` nesting depth. See
/// [`NativeRenderError::GraphicsStateStackOverflow`].
pub const MAX_GRAPHICS_STATE_DEPTH: usize = 4096;

/// Soft cap on the number of [`RenderWarning`]s collected, so a content
/// stream consisting of millions of unsupported operators can't be used
/// to force unbounded `Vec` growth (untrusted input).
const MAX_WARNINGS: usize = 1000;

/// The result of [`super::render_content_stream`]: the rasterized page
/// plus a (possibly empty) list of recoverable conditions encountered
/// along the way.
#[derive(Debug)]
pub struct NativeRenderOutput {
    /// The rendered raster. Use `.data()` for raw premultiplied RGBA8
    /// bytes, or `.pixel(x, y)` for a single pixel.
    pub pixmap: Pixmap,
    /// Operators/constructs this phase doesn't implement, or malformed
    /// constructs that were skipped rather than aborting the whole
    /// render. Empty for a content stream fully within this phase's
    /// scope.
    pub warnings: Vec<RenderWarning>,
}

/// Rasterizes `content` (a single page content stream, ISO 32000-1 7.8.2)
/// onto a `width`x`height` raster covering `media_box` in PDF user space.
///
/// `resources` should be the page's `/Resources` dictionary (ISO 32000-1
/// 7.8.3) if available -- currently only consulted for `/ExtGState`
/// lookups performed by the `gs` operator. Pass `None` if there are no
/// resources (e.g. a synthetic content stream with no `gs` operators);
/// any `gs` reference then degrades gracefully to
/// [`RenderWarning::MissingExtGState`] rather than failing the render.
///
/// The background is filled opaque white before interpretation starts
/// (a common rendering convention, not an ISO 32000 requirement -- a
/// content stream is not required to paint its own background).
pub fn render_content_stream(
    content: &[u8],
    width: u32,
    height: u32,
    media_box: Rectangle,
    resources: Option<&PdfDictionary>,
) -> Result<NativeRenderOutput, NativeRenderError> {
    if width == 0 || height == 0 {
        return Err(NativeRenderError::InvalidDimensions { width, height });
    }

    let mb_w = media_box.urx - media_box.llx;
    let mb_h = media_box.ury - media_box.lly;
    if !mb_w.is_finite() || !mb_h.is_finite() || mb_w <= 0.0 || mb_h <= 0.0 {
        return Err(NativeRenderError::DegenerateMediaBox {
            llx: media_box.llx,
            lly: media_box.lly,
            urx: media_box.urx,
            ury: media_box.ury,
        });
    }

    let scale_x = width as f64 / mb_w;
    let scale_y = height as f64 / mb_h;
    // Maps PDF user space (origin bottom-left, y-up) to device/raster
    // space (origin top-left, y-down): x' = scale_x*(x-llx),
    // y' = height - scale_y*(y-lly).
    let page_to_device = Matrix::new(
        scale_x,
        0.0,
        0.0,
        -scale_y,
        -media_box.llx * scale_x,
        height as f64 + media_box.lly * scale_y,
    );

    let mut pixmap = Pixmap::new(width, height).ok_or(NativeRenderError::PixmapAllocationFailed { width, height })?;
    pixmap.fill(Color::WHITE);

    let mut interp = Interpreter {
        pixmap,
        gs: GraphicsStateStack::new(GraphicsState::initial(page_to_device), MAX_GRAPHICS_STATE_DEPTH),
        path: PathAccumulator::new(),
        pending_clip: None,
        resources,
        warnings: Vec::new(),
        canvas_w: width,
        canvas_h: height,
    };

    for item in parse_content_stream(content) {
        match item {
            ContentItem::Op { operator, operands } => interp.exec(&operator, &operands)?,
            ContentItem::InlineImage(_) => interp.warn(RenderWarning::InlineImageUnsupported),
            ContentItem::Raw(_) => interp.warn(RenderWarning::TruncatedContentStream),
        }
    }

    Ok(NativeRenderOutput {
        pixmap: interp.pixmap,
        warnings: interp.warnings,
    })
}

struct Interpreter<'a> {
    pixmap: Pixmap,
    gs: GraphicsStateStack,
    path: PathAccumulator,
    /// Set by `W`/`W*`; applied to the clip once the *next* path-painting
    /// operator runs (ISO 32000-1 8.5.4).
    pending_clip: Option<FillRule>,
    resources: Option<&'a PdfDictionary>,
    warnings: Vec<RenderWarning>,
    canvas_w: u32,
    canvas_h: u32,
}

impl<'a> Interpreter<'a> {
    fn warn(&mut self, w: RenderWarning) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(w);
        }
    }

    fn exec(&mut self, operator: &str, operands: &[Object]) -> Result<(), NativeRenderError> {
        match operator {
            "q" => {
                if self.gs.push().is_err() {
                    return Err(NativeRenderError::GraphicsStateStackOverflow {
                        max: MAX_GRAPHICS_STATE_DEPTH,
                    });
                }
            }
            "Q" => {
                if !self.gs.pop() {
                    self.warn(RenderWarning::UnbalancedRestore);
                }
            }
            "cm" => {
                if let Some(v) = nums(operands, 6) {
                    let m = Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5]);
                    let state = self.gs.current_mut();
                    state.ctm = m.multiply(&state.ctm);
                }
            }

            "m" => {
                if let Some(v) = nums(operands, 2) {
                    let (x, y) = self.to_device(v[0], v[1]);
                    self.path.move_to(x, y);
                }
            }
            "l" => {
                if let Some(v) = nums(operands, 2) {
                    let (x, y) = self.to_device(v[0], v[1]);
                    self.path.line_to(x, y);
                }
            }
            "c" => {
                if let Some(v) = nums(operands, 6) {
                    let (x1, y1) = self.to_device(v[0], v[1]);
                    let (x2, y2) = self.to_device(v[2], v[3]);
                    let (x3, y3) = self.to_device(v[4], v[5]);
                    self.path.cubic_to(x1, y1, x2, y2, x3, y3);
                }
            }
            "v" => {
                if let Some(v) = nums(operands, 4) {
                    let (x2, y2) = self.to_device(v[0], v[1]);
                    let (x3, y3) = self.to_device(v[2], v[3]);
                    let (x1, y1) = self.path.current_point().unwrap_or((x2, y2));
                    self.path.cubic_to(x1, y1, x2, y2, x3, y3);
                }
            }
            "y" => {
                if let Some(v) = nums(operands, 4) {
                    let (x1, y1) = self.to_device(v[0], v[1]);
                    let (x3, y3) = self.to_device(v[2], v[3]);
                    self.path.cubic_to(x1, y1, x3, y3, x3, y3);
                }
            }
            "h" => self.path.close(),
            "re" => {
                if let Some(v) = nums(operands, 4) {
                    let (x, y, w, h) = (v[0], v[1], v[2], v[3]);
                    let corners = [
                        self.to_device(x, y),
                        self.to_device(x + w, y),
                        self.to_device(x + w, y + h),
                        self.to_device(x, y + h),
                    ];
                    self.path.rect(corners);
                }
            }

            "f" | "F" => self.finish_path(FillMode::Fill(FillRule::Winding)),
            "f*" => self.finish_path(FillMode::Fill(FillRule::EvenOdd)),
            "S" => self.finish_path(FillMode::Stroke),
            "s" => {
                self.path.close();
                self.finish_path(FillMode::Stroke);
            }
            "B" => self.finish_path(FillMode::FillStroke(FillRule::Winding)),
            "B*" => self.finish_path(FillMode::FillStroke(FillRule::EvenOdd)),
            "b" => {
                self.path.close();
                self.finish_path(FillMode::FillStroke(FillRule::Winding));
            }
            "b*" => {
                self.path.close();
                self.finish_path(FillMode::FillStroke(FillRule::EvenOdd));
            }
            "n" => self.finish_path(FillMode::None),

            "W" => self.pending_clip = Some(FillRule::Winding),
            "W*" => self.pending_clip = Some(FillRule::EvenOdd),

            "w" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().line_width = v[0].max(0.0);
                }
            }
            "J" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().line_cap = line_cap_from_i64(v[0] as i64);
                }
            }
            "j" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().line_join = line_join_from_i64(v[0] as i64);
                }
            }
            "M" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().miter_limit = v[0];
                }
            }
            "d" => {
                if operands.len() == 2 {
                    if let (Object::Array(arr), Some(phase)) = (&operands[0], as_f64(&operands[1])) {
                        let state = self.gs.current_mut();
                        state.dash_array = arr.iter().filter_map(as_f64).collect();
                        state.dash_phase = phase;
                    }
                }
            }

            "g" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().fill_color = colorspace::device_gray(v[0], 1.0);
                }
            }
            "G" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().stroke_color = colorspace::device_gray(v[0], 1.0);
                }
            }
            "rg" => {
                if let Some(v) = nums(operands, 3) {
                    self.gs.current_mut().fill_color = colorspace::device_rgb(v[0], v[1], v[2], 1.0);
                }
            }
            "RG" => {
                if let Some(v) = nums(operands, 3) {
                    self.gs.current_mut().stroke_color = colorspace::device_rgb(v[0], v[1], v[2], 1.0);
                }
            }
            "k" => {
                if let Some(v) = nums(operands, 4) {
                    self.gs.current_mut().fill_color = colorspace::device_cmyk(v[0], v[1], v[2], v[3], 1.0);
                }
            }
            "K" => {
                if let Some(v) = nums(operands, 4) {
                    self.gs.current_mut().stroke_color = colorspace::device_cmyk(v[0], v[1], v[2], v[3], 1.0);
                }
            }

            "gs" => {
                if let Some(Object::Name(name)) = operands.first() {
                    self.apply_ext_gstate(name.as_str());
                } else {
                    self.warn(RenderWarning::UnsupportedOperator {
                        operator: "gs (malformed operand)".to_string(),
                    });
                }
            }

            other => self.warn(RenderWarning::UnsupportedOperator {
                operator: other.to_string(),
            }),
        }
        Ok(())
    }

    /// Transforms a user-space point through the current CTM to device
    /// space, sanitizing non-finite results (see
    /// [`super::path::sanitize_point`]).
    fn to_device(&self, x: f64, y: f64) -> (f32, f32) {
        let (dx, dy) = self.gs.current().ctm.transform_point(x, y);
        super::path::sanitize_point(dx, dy)
    }

    fn apply_ext_gstate(&mut self, name: &str) {
        let dict = self
            .resources
            .and_then(|r| r.get("ExtGState"))
            .and_then(|o| match o {
                Object::Dictionary(d) => Some(d),
                _ => None,
            })
            .and_then(|extg| extg.get(name))
            .and_then(|o| match o {
                Object::Dictionary(d) => Some(d),
                _ => None,
            });

        let Some(dict) = dict else {
            self.warn(RenderWarning::MissingExtGState { name: name.to_string() });
            return;
        };

        let state = self.gs.current_mut();
        if let Some(v) = dict.get("ca").and_then(as_f64) {
            state.fill_alpha = v.clamp(0.0, 1.0) as f32;
        }
        if let Some(v) = dict.get("CA").and_then(as_f64) {
            state.stroke_alpha = v.clamp(0.0, 1.0) as f32;
        }
        if let Some(v) = dict.get("LW").and_then(as_f64) {
            state.line_width = v.max(0.0);
        }
        if let Some(Object::Array(d)) = dict.get("D") {
            if d.len() == 2 {
                if let (Some(Object::Array(arr)), Some(phase)) = (d.get(0), d.get(1).and_then(as_f64)) {
                    state.dash_array = arr.iter().filter_map(as_f64).collect();
                    state.dash_phase = phase;
                }
            }
        }
    }

    /// Finalizes the current path object (ISO 32000-1 8.5.3): paints it
    /// per `mode`, applies any pending clip (8.5.4) using the same path
    /// snapshot, then clears the path.
    fn finish_path(&mut self, mode: FillMode) {
        // Avoid cloning the path builder at all for the common "no path
        // constructed" case (e.g. a stray painting operator, or `n` used
        // purely to discharge a pending clip that turned out empty).
        let path = if self.path.is_empty() { None } else { self.path.to_path() };

        if let Some(path) = &path {
            // Snapshot everything needed from the graphics state as owned
            // values up front so this block doesn't hold a live borrow of
            // `self.gs` while later calling `self.pixmap.*`/`self.warn`
            // (which need their own, non-overlapping access to `self`).
            let state = self.gs.current();
            let fill_color = paint_color(state.fill_color, state.fill_alpha);
            let stroke_color = paint_color(state.stroke_color, state.stroke_alpha);
            let clip: Option<Rc<Mask>> = state.clip.clone();
            let stroke_params = StrokeParams::from(state);

            match mode {
                FillMode::Fill(rule) | FillMode::FillStroke(rule) => {
                    let mut paint = Paint::default();
                    paint.set_color(fill_color);
                    paint.anti_alias = true;
                    self.pixmap
                        .fill_path(path, &paint, rule, Transform::identity(), clip.as_deref());
                }
                _ => {}
            }
            match mode {
                FillMode::Stroke | FillMode::FillStroke(_) => {
                    let mut paint = Paint::default();
                    paint.set_color(stroke_color);
                    paint.anti_alias = true;
                    let (stroke, dash_invalid) = stroke_params.build();
                    self.pixmap
                        .stroke_path(path, &paint, &stroke, Transform::identity(), clip.as_deref());
                    if dash_invalid {
                        self.warn(RenderWarning::InvalidDashPattern);
                    }
                }
                _ => {}
            }
        }

        if let Some(rule) = self.pending_clip.take() {
            if let Some(path) = &path {
                self.apply_clip(path, rule);
            }
        }

        self.path.clear();
    }

    fn apply_clip(&mut self, path: &tiny_skia::Path, rule: FillRule) {
        let (w, h) = (self.canvas_w, self.canvas_h);
        let state = self.gs.current_mut();
        match state.clip.as_mut() {
            Some(rc) => {
                let mask = Rc::make_mut(rc);
                mask.intersect_path(path, rule, true, Transform::identity());
            }
            None => {
                if let Some(mut mask) = Mask::new(w, h) {
                    mask.fill_path(path, rule, true, Transform::identity());
                    state.clip = Some(Rc::new(mask));
                }
            }
        }
    }
}

enum FillMode {
    Fill(FillRule),
    Stroke,
    FillStroke(FillRule),
    None,
}

/// Owned snapshot of the graphics-state fields needed to build a
/// `tiny_skia::Stroke`, decoupled from any borrow of `GraphicsState` so it
/// can outlive the `self.warn(...)` call in `finish_path`.
struct StrokeParams {
    ctm: Matrix,
    line_width: f64,
    miter_limit: f64,
    line_cap: tiny_skia::LineCap,
    line_join: tiny_skia::LineJoin,
    dash_array: Vec<f64>,
    dash_phase: f64,
}

impl StrokeParams {
    fn from(state: &GraphicsState) -> Self {
        Self {
            ctm: state.ctm,
            line_width: state.line_width,
            miter_limit: state.miter_limit,
            line_cap: state.line_cap,
            line_join: state.line_join,
            dash_array: state.dash_array.clone(),
            dash_phase: state.dash_phase,
        }
    }

    /// Builds the effective device-space [`Stroke`], approximating the
    /// CTM's effect on user-space line width/dash lengths by its uniform
    /// scale factor `sqrt(|det(CTM)|)`. This is an intentional
    /// simplification (documented in the [`super`] module docs): a
    /// skewed or non-uniformly-scaled CTM should, per a fully spec-exact
    /// renderer, turn a round pen into an ellipse, which this phase does
    /// not attempt.
    ///
    /// Returns `true` as the second element if the dash pattern was
    /// rejected by `tiny-skia` (e.g. all-zero lengths) and fell back to a
    /// solid stroke.
    fn build(&self) -> (Stroke, bool) {
        let det = self.ctm.a * self.ctm.d - self.ctm.b * self.ctm.c;
        let mut scale = det.abs().sqrt();
        if !scale.is_finite() || scale == 0.0 {
            scale = 1.0;
        }

        let mut stroke = Stroke {
            width: (self.line_width * scale) as f32,
            miter_limit: self.miter_limit.max(1.0) as f32,
            line_cap: self.line_cap,
            line_join: self.line_join,
            dash: None,
        };

        let mut dash_invalid = false;
        if !self.dash_array.is_empty() {
            let mut scaled: Vec<f32> = self.dash_array.iter().map(|v| (v * scale).max(0.0) as f32).collect();
            if scaled.len() % 2 == 1 {
                // ISO 32000-1 8.4.3.6: an odd-length dash array is used
                // twice in succession (i.e. treated as if repeated).
                let dup = scaled.clone();
                scaled.extend(dup);
            }
            let phase = (self.dash_phase * scale) as f32;
            match StrokeDash::new(scaled, phase) {
                Some(d) => stroke.dash = Some(d),
                None => dash_invalid = true,
            }
        }

        (stroke, dash_invalid)
    }
}

fn paint_color(base: Color, alpha: f32) -> Color {
    Color::from_rgba(base.red(), base.green(), base.blue(), alpha.clamp(0.0, 1.0)).unwrap_or(Color::BLACK)
}

fn line_cap_from_i64(v: i64) -> tiny_skia::LineCap {
    match v {
        1 => tiny_skia::LineCap::Round,
        2 => tiny_skia::LineCap::Square,
        _ => tiny_skia::LineCap::Butt,
    }
}

fn line_join_from_i64(v: i64) -> tiny_skia::LineJoin {
    match v {
        1 => tiny_skia::LineJoin::Round,
        2 => tiny_skia::LineJoin::Bevel,
        _ => tiny_skia::LineJoin::Miter,
    }
}

fn as_f64(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) if r.is_finite() => Some(*r),
        _ => None,
    }
}

/// Extracts exactly `n` numeric operands, or `None` if the operand count
/// or types don't match -- a malformed/short operand list for a
/// known operator is silently skipped (treated as a no-op for that one
/// invocation) rather than erroring the whole render.
fn nums(operands: &[Object], n: usize) -> Option<Vec<f64>> {
    if operands.len() != n {
        return None;
    }
    operands.iter().map(as_f64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nums_rejects_wrong_count() {
        assert!(nums(&[Object::Integer(1)], 2).is_none());
    }

    #[test]
    fn nums_rejects_non_numeric() {
        assert!(nums(&[Object::Name(crate::object::PdfName::new_unchecked("X"))], 1).is_none());
    }

    #[test]
    fn nums_rejects_non_finite_real() {
        assert!(nums(&[Object::Real(f64::NAN)], 1).is_none());
    }

    #[test]
    fn line_cap_mapping() {
        assert_eq!(line_cap_from_i64(0), tiny_skia::LineCap::Butt);
        assert_eq!(line_cap_from_i64(1), tiny_skia::LineCap::Round);
        assert_eq!(line_cap_from_i64(2), tiny_skia::LineCap::Square);
        assert_eq!(line_cap_from_i64(99), tiny_skia::LineCap::Butt);
    }
}
