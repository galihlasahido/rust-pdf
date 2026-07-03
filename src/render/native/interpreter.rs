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
//! - Text state: `Tc Tw Tz TL Tf Tr Ts` (9.3), `BT ET` (9.4.1)
//! - Text positioning: `Td TD Tm T*` (9.4.2)
//! - Text showing: `Tj TJ '` `"` (9.4.3), including simple TrueType,
//!   composite (Type 0/CID) and Type 3 glyph rendering -- see
//!   [`super::font`] and [`super::glyph`].
//!
//! Everything else -- `Do` (XObjects/images), `sh` (shadings), `cs`/`CS`/
//! `sc`/`SC`/`scn`/`SCN` (non-Device color spaces), marked content -- is
//! recorded as [`RenderWarning::UnsupportedOperator`] and skipped as a
//! no-op; it does **not** abort the render or panic.

use std::collections::HashMap;
use std::rc::Rc;

use tiny_skia::{Color, FillRule, Mask, Paint, Pixmap, Stroke, StrokeDash, Transform};

use crate::editor::content_stream::{parse_content_stream, ContentItem};
use crate::object::{Object, PdfDictionary};
use crate::types::{Matrix, Rectangle};

use super::color as colorspace;
use super::error::{NativeRenderError, RenderWarning};
use super::font::{self, FontProgram, ResolvedFont};
use super::glyph::glyph_outline_path;
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
/// 7.8.3) if available -- consulted for `/ExtGState` and `/Font` lookups
/// (`gs`/`Tf`). Pass `None` if there are no resources (e.g. a synthetic
/// content stream with no `gs`/`Tf` operators); any reference then
/// degrades gracefully to a [`RenderWarning`] rather than failing the
/// render. See the [module docs](super) for the "already fully
/// dereferenced" assumption this makes about `resources`.
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
    let mut warnings = Vec::new();

    {
        let mut interp = Interpreter {
            pixmap: &mut pixmap,
            gs: GraphicsStateStack::new(GraphicsState::initial(page_to_device), MAX_GRAPHICS_STATE_DEPTH),
            path: PathAccumulator::new(),
            pending_clip: None,
            resources,
            warnings: &mut warnings,
            canvas_w: width,
            canvas_h: height,
            text_matrix: Matrix::identity(),
            text_line_matrix: Matrix::identity(),
            font_cache: HashMap::new(),
            type3_depth: 0,
        };

        for item in parse_content_stream(content) {
            match item {
                ContentItem::Op { operator, operands } => interp.exec(&operator, &operands)?,
                ContentItem::InlineImage(_) => interp.warn(RenderWarning::InlineImageUnsupported),
                ContentItem::Raw(_) => interp.warn(RenderWarning::TruncatedContentStream),
            }
        }
    }

    Ok(NativeRenderOutput { pixmap, warnings })
}

/// `'res` borrows the (pre-resolved, see [module docs](super)) resources
/// dictionary in scope for this interpreter instance -- a Type 3 glyph
/// procedure with its own `/Resources` runs a *different*-lifetimed
/// nested `Interpreter`, not this same value, so this never needs to
/// change mid-render. `'ctx` borrows the shared, render-wide pixmap and
/// warnings sink; a nested (Type 3) `Interpreter` reborrows both from the
/// parent so every glyph -- at any recursion depth -- paints onto the
/// same raster and reports into the same warning list.
struct Interpreter<'res, 'ctx> {
    pixmap: &'ctx mut Pixmap,
    gs: GraphicsStateStack,
    path: PathAccumulator,
    /// Set by `W`/`W*`; applied to the clip once the *next* path-painting
    /// operator runs (ISO 32000-1 8.5.4).
    pending_clip: Option<FillRule>,
    resources: Option<&'res PdfDictionary>,
    warnings: &'ctx mut Vec<RenderWarning>,
    canvas_w: u32,
    canvas_h: u32,
    /// Text matrix (ISO 32000-1 9.4.2): maps text space to the CTM in
    /// effect when the text object began. **Not** part of
    /// [`GraphicsState`] -- 9.4.1 defines `Tm`/`Tlm` as reset to identity
    /// at every `BT` and untouched by `q`/`Q`, unlike the text *state*
    /// parameters (`Tc`, `Tw`, ... in [`super::state::TextState`]), which
    /// genuinely are part of the graphics state.
    text_matrix: Matrix,
    /// Text line matrix (9.4.2): the `Tm`/`Tlm` value at the start of the
    /// current line, used by `Td`/`TD`/`T*` to compute the next line's
    /// origin without accumulating drift from intervening glyph
    /// advances.
    text_line_matrix: Matrix,
    /// Caches [`Tf`](Self::exec)'s font resolution by resource name for
    /// the lifetime of this `Interpreter` instance, since a content
    /// stream typically issues `Tf` far more often than it actually
    /// changes fonts (many producers re-select the current font before
    /// every run of text). Not shared across a Type 3 recursion boundary
    /// (each nested `Interpreter` gets its own, empty cache) -- simpler
    /// than plumbing a shared cache through, and Type 3 glyph procedures
    /// only rarely select fonts of their own.
    font_cache: HashMap<String, Rc<ResolvedFont>>,
    /// Current Type 3 glyph-procedure recursion depth (0 at the
    /// top-level content stream). See [`font::MAX_TYPE3_DEPTH`].
    type3_depth: usize,
}

impl<'res, 'ctx> Interpreter<'res, 'ctx> {
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

            "BT" => {
                self.text_matrix = Matrix::identity();
                self.text_line_matrix = Matrix::identity();
            }
            "ET" => {}
            "Tf" => {
                if let (Some(Object::Name(name)), Some(size)) = (operands.first(), operands.get(1).and_then(as_f64)) {
                    self.set_font(name.as_str(), size);
                }
            }
            "Tc" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().text.char_spacing = v[0];
                }
            }
            "Tw" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().text.word_spacing = v[0];
                }
            }
            "Tz" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().text.horizontal_scale = v[0] / 100.0;
                }
            }
            "TL" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().text.leading = v[0];
                }
            }
            "Ts" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().text.rise = v[0];
                }
            }
            "Tr" => {
                if let Some(v) = nums(operands, 1) {
                    self.gs.current_mut().text.render_mode = v[0] as i64;
                }
            }
            "Td" => {
                if let Some(v) = nums(operands, 2) {
                    self.text_translate(v[0], v[1]);
                }
            }
            "TD" => {
                if let Some(v) = nums(operands, 2) {
                    self.gs.current_mut().text.leading = -v[1];
                    self.text_translate(v[0], v[1]);
                }
            }
            "Tm" => {
                if let Some(v) = nums(operands, 6) {
                    let m = Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5]);
                    self.text_matrix = m;
                    self.text_line_matrix = m;
                }
            }
            "T*" => {
                let tl = self.gs.current().text.leading;
                self.text_translate(0.0, -tl);
            }
            "Tj" => {
                if let Some(Object::String(s)) = operands.last() {
                    self.show_text(s.as_bytes())?;
                }
            }
            "'" => {
                let tl = self.gs.current().text.leading;
                self.text_translate(0.0, -tl);
                if let Some(Object::String(s)) = operands.last() {
                    self.show_text(s.as_bytes())?;
                }
            }
            "\"" => {
                if let Some(aw) = operands.first().and_then(as_f64) {
                    self.gs.current_mut().text.word_spacing = aw;
                }
                if let Some(ac) = operands.get(1).and_then(as_f64) {
                    self.gs.current_mut().text.char_spacing = ac;
                }
                let tl = self.gs.current().text.leading;
                self.text_translate(0.0, -tl);
                if let Some(Object::String(s)) = operands.last() {
                    self.show_text(s.as_bytes())?;
                }
            }
            // Type 3 glyph metrics (ISO 32000-1:2008 9.6.5.3, Table 113):
            // valid, recognized operators whose operands (glyph width,
            // and -- for `d1` -- a bounding box) this phase intentionally
            // does not act on, since glyph advance is instead sourced
            // from the font dictionary's own `/Widths` array (see
            // `font.rs`'s module docs) and glyph-bbox-based clipping/
            // caching is an optional optimization this phase skips.
            // Recognized here (rather than falling through to
            // `UnsupportedOperator`) so a well-formed Type 3 `CharProc`
            // doesn't spuriously warn about operators the spec requires
            // it to contain.
            "d0" | "d1" => {}

            "TJ" => {
                if let Some(Object::Array(arr)) = operands.first() {
                    for elem in arr.iter() {
                        match elem {
                            Object::String(s) => self.show_text(s.as_bytes())?,
                            Object::Integer(n) => self.apply_tj_adjustment(*n as f64),
                            Object::Real(r) if r.is_finite() => self.apply_tj_adjustment(*r),
                            _ => {}
                        }
                    }
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

    /// Resolves and caches (see [`Self::font_cache`]) the `/Resources
    /// /Font /<name>` dictionary named by `Tf`, recording
    /// [`RenderWarning::MissingFontResource`] or
    /// [`RenderWarning::UnsupportedFontProgram`] (each at most once per
    /// name) rather than failing the render.
    fn set_font(&mut self, name: &str, size: f64) {
        let resolved = self.resolve_font_cached(name);
        let state = self.gs.current_mut();
        state.text.font = Some(resolved);
        state.text.font_resource_name = Some(name.to_string());
        state.text.font_size = size;
    }

    fn resolve_font_cached(&mut self, name: &str) -> Rc<ResolvedFont> {
        if let Some(cached) = self.font_cache.get(name) {
            return cached.clone();
        }

        let font_dict = self
            .resources
            .and_then(|r| r.get("Font"))
            .and_then(|o| match o {
                Object::Dictionary(d) => Some(d),
                _ => None,
            })
            .and_then(|fonts| fonts.get(name))
            .and_then(|o| match o {
                Object::Dictionary(d) => Some(d),
                _ => None,
            });

        let resolved = match font_dict {
            Some(dict) => {
                let resolved = font::resolve_font(dict);
                if let Some(reason) = resolved.unsupported_reason() {
                    self.warn(RenderWarning::UnsupportedFontProgram {
                        resource_name: name.to_string(),
                        reason: reason.to_string(),
                    });
                }
                resolved
            }
            None => {
                self.warn(RenderWarning::MissingFontResource { name: name.to_string() });
                ResolvedFont::missing_placeholder()
            }
        };

        let rc = Rc::new(resolved);
        self.font_cache.insert(name.to_string(), rc.clone());
        rc
    }

    /// `Td`/`TD`/`T*`/`'`/`"`: `Tlm_new = [1 0 0 1 tx ty] x Tlm_old`;
    /// `Tm` is reset to the same value (ISO 32000-1 9.4.2).
    fn text_translate(&mut self, tx: f64, ty: f64) {
        let translate = Matrix::translate(tx, ty);
        self.text_line_matrix = translate.multiply(&self.text_line_matrix);
        self.text_matrix = self.text_line_matrix;
    }

    /// A `TJ` array's numeric adjustment (ISO 32000-1 9.4.3): a positive
    /// number moves the next glyph *left* (for horizontal writing),
    /// expressed in thousandths of a text-space unit before scaling by
    /// `Tfs`/`Tz`.
    fn apply_tj_adjustment(&mut self, n: f64) {
        if !n.is_finite() {
            return;
        }
        let text = &self.gs.current().text;
        let tx = -(n / 1000.0) * text.font_size * text.horizontal_scale;
        self.text_matrix = Matrix::translate(tx, 0.0).multiply(&self.text_matrix);
    }

    /// `Tj`/each `TJ` string element/`'`/`"`: shows `bytes` (already the
    /// raw content-stream string bytes, ISO 32000-1 9.4.3), chunked per
    /// the active font's code width (1 byte for simple/Type 3, 2 bytes
    /// for composite -- see [`ResolvedFont::code_width_bytes`] and the
    /// [`super::font`] module docs on why composite is always assumed
    /// 2-byte). A trailing partial code (a malformed/truncated string for
    /// a 2-byte font) is silently dropped, matching this crate's general
    /// "tolerate truncation, don't lose the rest" untrusted-input stance.
    fn show_text(&mut self, bytes: &[u8]) -> Result<(), NativeRenderError> {
        let Some(font_rc) = self.gs.current().text.font.clone() else {
            self.warn(RenderWarning::MissingActiveFont);
            return Ok(());
        };

        let code_width = font_rc.code_width_bytes();
        if code_width == 0 {
            return Ok(());
        }

        let mut i = 0;
        while i + code_width <= bytes.len() {
            let chunk = &bytes[i..i + code_width];
            let code = chunk.iter().fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
            self.show_one_glyph(&font_rc, code, code_width == 1 && code == 32)?;
            i += code_width;
        }
        Ok(())
    }

    /// Paints (unless the render mode is invisible, or the glyph has no
    /// program/outline) glyph `code` at the current text position, then
    /// advances `text_matrix` by its displacement (ISO 32000-1 9.4.4).
    fn show_one_glyph(&mut self, font: &ResolvedFont, code: u32, is_word_space_code: bool) -> Result<(), NativeRenderError> {
        let (tfs, th, tc, tw, trise, render_mode) = {
            let text = &self.gs.current().text;
            (
                text.font_size,
                text.horizontal_scale,
                text.char_spacing,
                text.word_spacing,
                text.rise,
                text.render_mode,
            )
        };

        // Trm = [Tfs*Th 0 0 Tfs 0 Trise] x Tm x CTM (9.4.4).
        let text_scale = Matrix::new(tfs * th, 0.0, 0.0, tfs, 0.0, trise);
        let ctm = self.gs.current().ctm;
        let trm = text_scale.multiply(&self.text_matrix).multiply(&ctm);

        let visible = render_mode != 3 && render_mode != 7;
        if visible {
            self.paint_glyph(font, code, trm)?;
        }

        let w0 = font.width_1000(code) / 1000.0;
        let mut tx = (w0 * tfs + tc) * th;
        if is_word_space_code {
            tx += tw * th;
        }
        self.text_matrix = Matrix::translate(tx, 0.0).multiply(&self.text_matrix);
        Ok(())
    }

    /// Paints one glyph's shape given its already-computed text rendering
    /// matrix `trm` (glyph space's *text*-space origin -> device space).
    fn paint_glyph(&mut self, font: &ResolvedFont, code: u32, trm: Matrix) -> Result<(), NativeRenderError> {
        match font {
            ResolvedFont::Simple(simple) => {
                if let FontProgram::Loaded(ttf) = &simple.program {
                    let gid = font::simple_glyph_id(ttf, code);
                    self.paint_truetype_glyph(ttf, gid, trm);
                }
            }
            ResolvedFont::Composite(composite) => {
                if let FontProgram::Loaded(ttf) = &composite.program {
                    let gid = font::composite_glyph_id(composite, code);
                    self.paint_truetype_glyph(ttf, gid, trm);
                }
            }
            ResolvedFont::Type3(type3) => {
                let Some(name) = type3.glyph_name(code) else {
                    return Ok(());
                };
                let Some(proc_bytes) = type3.char_procs.get(name).cloned() else {
                    return Ok(());
                };
                // Glyph space -> text space (FontMatrix) -> device space (trm).
                let glyph_ctm = type3.font_matrix.multiply(&trm);
                let type3_resources = type3.resources.clone();
                self.run_type3_glyph(&proc_bytes, glyph_ctm, type3_resources.as_ref())?;
            }
        }
        Ok(())
    }

    fn paint_truetype_glyph(&mut self, ttf: &crate::font::truetype::TrueTypeFont, gid: u16, trm: Matrix) {
        let upm = f64::from(ttf.units_per_em().max(1));
        let glyph_to_device = Matrix::scale(1.0 / upm, 1.0 / upm).multiply(&trm);
        let Some(path) = glyph_outline_path(ttf, gid, glyph_to_device) else {
            return;
        };

        let state = self.gs.current();
        let render_mode = state.text.render_mode;
        let fill_color = paint_color(state.fill_color, state.fill_alpha);
        let stroke_color = paint_color(state.stroke_color, state.stroke_alpha);
        let clip: Option<Rc<Mask>> = state.clip.clone();
        let stroke_params = StrokeParams::from(state);

        // Render modes 4-7 add to the clip path in addition to painting
        // like 0-3; this phase paints identically but does not implement
        // the clip-add half (see `state::TextState::render_mode`'s docs).
        let mode = render_mode % 4;
        if mode == 0 || mode == 2 {
            let mut paint = Paint::default();
            paint.set_color(fill_color);
            paint.anti_alias = true;
            self.pixmap
                .fill_path(&path, &paint, FillRule::Winding, Transform::identity(), clip.as_deref());
        }
        if mode == 1 || mode == 2 {
            let mut paint = Paint::default();
            paint.set_color(stroke_color);
            paint.anti_alias = true;
            let (stroke, _dash_invalid) = stroke_params.build();
            self.pixmap
                .stroke_path(&path, &paint, &stroke, Transform::identity(), clip.as_deref());
        }
    }

    /// Runs a Type 3 glyph's `CharProc` (ISO 32000-1:2008 9.6.5.2)
    /// content stream recursively through this same interpreter, bounded
    /// by [`font::MAX_TYPE3_DEPTH`] (untrusted input: a crafted Type 3
    /// font can reference itself, directly or through a cycle of several
    /// fonts).
    ///
    /// `child_ctm` is the CTM the procedure's path/text operators see
    /// (glyph space is mapped directly to device space up front, per
    /// 9.6.5.2's "the CTM in effect at the time the glyph description is
    /// interpreted... includes the FontMatrix and any scaling implied by
    /// the font size"); the rest of the graphics state (color, clip,
    /// line style, ...) is inherited from the state in effect when the
    /// glyph was shown, since 9.6.5.3 has the procedure paint in the
    /// color active when it runs (unless it sets its own via `d0`,
    /// which -- see [`Self::exec`]'s fallthrough for `d0`/`d1` -- this
    /// phase does not specially distinguish from an ordinary color
    /// operator; both are simply executed like any other operator).
    fn run_type3_glyph(
        &mut self,
        proc_bytes: &[u8],
        child_ctm: Matrix,
        type3_resources: Option<&PdfDictionary>,
    ) -> Result<(), NativeRenderError> {
        if self.type3_depth >= font::MAX_TYPE3_DEPTH {
            self.warn(RenderWarning::Type3RecursionLimitExceeded);
            return Ok(());
        }

        let mut child_state = self.gs.current().clone();
        child_state.ctm = child_ctm;
        let resources = type3_resources.or(self.resources);

        let mut child = Interpreter {
            pixmap: &mut *self.pixmap,
            gs: GraphicsStateStack::new(child_state, MAX_GRAPHICS_STATE_DEPTH),
            path: PathAccumulator::new(),
            pending_clip: None,
            resources,
            warnings: &mut *self.warnings,
            canvas_w: self.canvas_w,
            canvas_h: self.canvas_h,
            text_matrix: Matrix::identity(),
            text_line_matrix: Matrix::identity(),
            font_cache: HashMap::new(),
            type3_depth: self.type3_depth + 1,
        };

        for item in parse_content_stream(proc_bytes) {
            match item {
                ContentItem::Op { operator, operands } => child.exec(&operator, &operands)?,
                ContentItem::InlineImage(_) => child.warn(RenderWarning::InlineImageUnsupported),
                ContentItem::Raw(_) => child.warn(RenderWarning::TruncatedContentStream),
            }
        }
        Ok(())
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
