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
//! - ExtGState: `gs` for `ca`/`CA`/`LW`/`D` (8.4.5), plus `BM` (blend
//!   mode, §11.3.5 -- all 16 standard modes, see [`resolve_blend_mode`])
//!   and `SMask` (soft mask group, §11.6.4.3/§11.6.5.2 -- see
//!   [`Interpreter::apply_soft_mask_group`])
//! - Line style: `w J j M d` (8.4.3)
//! - Color: `g G rg RG k K` (Device spaces) plus `cs CS sc SC scn SCN`
//!   (8.6.6-8.6.8: Indexed, Separation, DeviceN, ICCBased-approximated --
//!   see [`super::colorspace`]). `scn`/`SCN` naming a Pattern colour is
//!   recorded as a warning and leaves the colour unchanged (Patterns
//!   remain out of scope).
//! - Text state: `Tc Tw Tz TL Tf Tr Ts` (9.3), `BT ET` (9.4.1)
//! - Text positioning: `Td TD Tm T*` (9.4.2)
//! - Text showing: `Tj TJ '` `"` (9.4.3), including simple TrueType,
//!   composite (Type 0/CID) and Type 3 glyph rendering -- see
//!   [`super::font`] and [`super::glyph`].
//! - Image XObjects (`Do`, §8.8) and inline images (`BI`/`ID`/`EI`, 8.9.7)
//!   -- see [`super::image`], including the documented JBIG2/JPX gap and
//!   the (now-implemented) per-image `/SMask` soft mask.
//! - Form XObjects (`Do`, §8.10), including transparency groups (§11.4)
//!   -- see [`Interpreter::do_form_xobject`].
//!
//! Everything else -- `sh` (shadings), Patterns, marked content -- is
//! recorded as [`RenderWarning::UnsupportedOperator`] (or a dedicated
//! variant) and skipped as a no-op; it does **not** abort the render or
//! panic.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tiny_skia::{BlendMode, Color, FillRule, Mask, MaskType, Paint, Pixmap, PixmapPaint, Stroke, StrokeDash, Transform};

use crate::editor::content_stream::{parse_content_stream, ContentItem};
use crate::object::{Object, PdfDictionary, PdfStream};
use crate::parser::InlineImage;
use crate::types::{Matrix, Rectangle};

use super::color as device_color;
use super::colorspace::{self, ColorSpace};
use super::error::{NativeRenderError, RenderWarning};
use super::font::{self, FontProgram, ResolvedFont};
use super::glyph::glyph_outline_path;
use super::image::{self, ImageResult};
use super::path::{PathAccumulator, PathReserve, MAX_PATH_POINTS_PER_PATH};
use super::state::{GraphicsState, GraphicsStateStack};

/// Hard cap on `q` nesting depth. See
/// [`NativeRenderError::GraphicsStateStackOverflow`].
pub const MAX_GRAPHICS_STATE_DEPTH: usize = 4096;

/// Hard cap on Form XObject recursion -- shared by a Form XObject painted
/// directly (`Do`), a transparency-group Form used the same way, and a
/// group referenced by an ExtGState `/SMask`. Guards against a
/// self-referential or mutually-recursive set of Form XObjects
/// (untrusted/adversarial input; ISO 32000-1 doesn't forbid a Form's own
/// content stream from invoking `Do` again, including -- maliciously --
/// on itself). Deliberately small: legitimate nesting (a group used as a
/// soft mask for another group, etc.) is rarely more than 2-3 deep.
pub const MAX_FORM_XOBJECT_DEPTH: usize = 12;

/// Soft cap on the number of [`RenderWarning`]s collected, so a content
/// stream consisting of millions of unsupported operators can't be used
/// to force unbounded `Vec` growth (untrusted input).
const MAX_WARNINGS: usize = 1000;

/// Hard cap on the *total* number of content-stream items (operators
/// *and* inline images) interpreted for one call to
/// [`render_content_stream`] -- summed across the top-level content stream
/// and every nested Form XObject / Type 3 glyph procedure /
/// transparency-group content stream it triggers, via one shared
/// [`RenderBudget`] threaded through every recursion level. This is a
/// distinct attack shape from the per-recursion-level depth caps above
/// ([`MAX_GRAPHICS_STATE_DEPTH`], [`MAX_FORM_XOBJECT_DEPTH`],
/// [`font::MAX_TYPE3_DEPTH`]): none of those bound a content stream that
/// is never nested/recursive at all but simply *very long* (e.g. millions
/// of flat, non-nested `q Q` pairs, or millions of path-construction
/// operators each individually cheap) -- exactly the "operator soup" input
/// class this crate's fuzzing of the interpreter directly (as opposed to
/// via full document parsing) is meant to explore. See
/// [`NativeRenderError::OperatorBudgetExceeded`].
pub const MAX_OPERATOR_COUNT: usize = 2_000_000;

/// Hard wall-clock budget for one call to [`render_content_stream`],
/// checked alongside [`MAX_OPERATOR_COUNT`] on every content-stream item.
/// This is the backstop for input whose *cost per operator* (rather than
/// operator *count*) is what makes it pathological -- e.g. a legally
/// modest operator count that nonetheless does expensive per-operator work
/// (very large paths approaching [`MAX_PATH_POINTS_PER_PATH`], deeply
/// layered-but-within-limits transparency groups each allocating a
/// canvas-sized offscreen buffer). See
/// [`NativeRenderError::RenderTimeBudgetExceeded`].
///
/// Deliberately generous: large enough that no legitimate page (even a
/// slow one on modest hardware) should ever hit it, small enough that an
/// adversarial input cannot hang a caller indefinitely.
pub const MAX_RENDER_DURATION: Duration = Duration::from_secs(20);

/// Tracks the resource budget shared by every [`Interpreter`] at every
/// recursion level (top-level content stream, and every nested Form
/// XObject / Type 3 glyph procedure / transparency-group render it
/// triggers) for a single call to [`render_content_stream`] --
/// analogous to how `pixmap`/`warnings` are reborrowed (not copied) across
/// that same recursion, so a crafted input alternating between "wide"
/// (many flat operators) and "deep" (nested recursion) attack shapes still
/// hits one combined bound rather than each recursion level getting its
/// own fresh budget.
pub(super) struct RenderBudget {
    max_operators: usize,
    op_count: usize,
    max_duration: Duration,
    deadline: Instant,
}

impl RenderBudget {
    fn new(max_operators: usize, max_duration: Duration) -> Self {
        Self {
            max_operators,
            op_count: 0,
            max_duration,
            deadline: Instant::now() + max_duration,
        }
    }

    /// Called once per content-stream item (operator or inline image),
    /// at every recursion level. Returns `Err` the *first* time either
    /// bound is crossed; the caller propagates that as a hard render
    /// failure (unlike the soft [`RenderWarning`]s elsewhere in this
    /// module, an exhausted budget aborts the whole render rather than
    /// skipping just the offending construct -- by this point the render
    /// has already proven itself pathological, so continuing to spend
    /// more operators/time on it serves no one).
    fn tick(&mut self) -> Result<(), NativeRenderError> {
        self.op_count += 1;
        if self.op_count > self.max_operators {
            return Err(NativeRenderError::OperatorBudgetExceeded { max: self.max_operators });
        }
        if Instant::now() >= self.deadline {
            return Err(NativeRenderError::RenderTimeBudgetExceeded {
                max_millis: self.max_duration.as_millis() as u64,
            });
        }
        Ok(())
    }
}

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
    render_content_stream_with_limits(content, width, height, media_box, resources, MAX_OPERATOR_COUNT, MAX_RENDER_DURATION)
}

/// As [`render_content_stream`], but with the operator-count/wall-clock
/// resource limits ([`MAX_OPERATOR_COUNT`]/[`MAX_RENDER_DURATION`])
/// overridable rather than fixed at their crate-wide defaults. `pub(super)`
/// only -- this exists so this module's own tests can exercise
/// [`NativeRenderError::OperatorBudgetExceeded`]/
/// [`NativeRenderError::RenderTimeBudgetExceeded`] deterministically (a
/// tiny `max_operators`, or a `max_duration` of zero) without either
/// waiting out the real, generous production defaults or lowering them for
/// everyone.
pub(super) fn render_content_stream_with_limits(
    content: &[u8],
    width: u32,
    height: u32,
    media_box: Rectangle,
    resources: Option<&PdfDictionary>,
    max_operators: usize,
    max_duration: Duration,
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
    let mut budget = RenderBudget::new(max_operators, max_duration);

    {
        let mut interp = Interpreter {
            pixmap: &mut pixmap,
            gs: GraphicsStateStack::new(GraphicsState::initial(page_to_device), MAX_GRAPHICS_STATE_DEPTH),
            path: PathAccumulator::new(),
            pending_clip: None,
            resources,
            warnings: &mut warnings,
            budget: &mut budget,
            canvas_w: width,
            canvas_h: height,
            text_matrix: Matrix::identity(),
            text_line_matrix: Matrix::identity(),
            font_cache: HashMap::new(),
            type3_depth: 0,
            form_depth: 0,
        };

        interp.run_content(content)?;
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
    /// Shared operator-count/wall-clock budget for this entire render call
    /// (see [`RenderBudget`]) -- reborrowed (not copied) across every
    /// recursion level exactly like `pixmap`/`warnings`, so nested Form
    /// XObject / Type 3 / transparency-group work all draws against the
    /// same combined limit.
    budget: &'ctx mut RenderBudget,
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
    /// Current Form XObject recursion depth (0 at the top-level content
    /// stream). See [`MAX_FORM_XOBJECT_DEPTH`]. Shared (not reset) across
    /// a Type 3 recursion boundary and vice versa, so a crafted input
    /// alternating between the two recursion kinds still hits a bound.
    form_depth: usize,
}

impl<'res, 'ctx> Interpreter<'res, 'ctx> {
    fn warn(&mut self, w: RenderWarning) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(w);
        }
    }

    /// Parses and interprets `content` end to end, ticking the shared
    /// [`RenderBudget`] once per item (operator or inline image) *before*
    /// dispatching it -- the single choke point every content-stream loop
    /// in this module goes through, whether it's the top-level page
    /// content stream, a Form XObject's, a transparency group's, or a
    /// Type 3 glyph procedure's (see [`Self::run_form_xobject_inline`],
    /// [`Self::render_group_to_pixmap`], [`Self::run_type3_glyph`]).
    fn run_content(&mut self, content: &[u8]) -> Result<(), NativeRenderError> {
        for item in parse_content_stream(content) {
            self.budget.tick()?;
            match item {
                ContentItem::Op { operator, operands } => self.exec(&operator, &operands)?,
                ContentItem::InlineImage(img) => self.show_inline_image(&img),
                ContentItem::Raw(_) => self.warn(RenderWarning::TruncatedContentStream),
            }
        }
        Ok(())
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
                    if self.check_path_budget(1) {
                        let (x, y) = self.to_device(v[0], v[1]);
                        self.path.move_to(x, y);
                    }
                }
            }
            "l" => {
                if let Some(v) = nums(operands, 2) {
                    if self.check_path_budget(1) {
                        let (x, y) = self.to_device(v[0], v[1]);
                        self.path.line_to(x, y);
                    }
                }
            }
            "c" => {
                if let Some(v) = nums(operands, 6) {
                    if self.check_path_budget(3) {
                        let (x1, y1) = self.to_device(v[0], v[1]);
                        let (x2, y2) = self.to_device(v[2], v[3]);
                        let (x3, y3) = self.to_device(v[4], v[5]);
                        self.path.cubic_to(x1, y1, x2, y2, x3, y3);
                    }
                }
            }
            "v" => {
                if let Some(v) = nums(operands, 4) {
                    if self.check_path_budget(2) {
                        let (x2, y2) = self.to_device(v[0], v[1]);
                        let (x3, y3) = self.to_device(v[2], v[3]);
                        let (x1, y1) = self.path.current_point().unwrap_or((x2, y2));
                        self.path.cubic_to(x1, y1, x2, y2, x3, y3);
                    }
                }
            }
            "y" => {
                if let Some(v) = nums(operands, 4) {
                    if self.check_path_budget(2) {
                        let (x1, y1) = self.to_device(v[0], v[1]);
                        let (x3, y3) = self.to_device(v[2], v[3]);
                        self.path.cubic_to(x1, y1, x3, y3, x3, y3);
                    }
                }
            }
            "h" => self.path.close(),
            "re" => {
                if let Some(v) = nums(operands, 4) {
                    if self.check_path_budget(4) {
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

            // `g`/`G`/`rg`/`RG`/`k`/`K` set *both* the current colour and
            // the current colour space (ISO 32000-1 8.6.8: these operators
            // implicitly select the corresponding Device colour space),
            // so a later `sc`/`scn` with no intervening `cs` interprets
            // its raw operands against the right space.
            "g" => {
                if let Some(v) = nums(operands, 1) {
                    let state = self.gs.current_mut();
                    state.fill_color = device_color::device_gray(v[0], 1.0);
                    state.fill_color_space = Rc::new(ColorSpace::DeviceGray);
                }
            }
            "G" => {
                if let Some(v) = nums(operands, 1) {
                    let state = self.gs.current_mut();
                    state.stroke_color = device_color::device_gray(v[0], 1.0);
                    state.stroke_color_space = Rc::new(ColorSpace::DeviceGray);
                }
            }
            "rg" => {
                if let Some(v) = nums(operands, 3) {
                    let state = self.gs.current_mut();
                    state.fill_color = device_color::device_rgb(v[0], v[1], v[2], 1.0);
                    state.fill_color_space = Rc::new(ColorSpace::DeviceRGB);
                }
            }
            "RG" => {
                if let Some(v) = nums(operands, 3) {
                    let state = self.gs.current_mut();
                    state.stroke_color = device_color::device_rgb(v[0], v[1], v[2], 1.0);
                    state.stroke_color_space = Rc::new(ColorSpace::DeviceRGB);
                }
            }
            "k" => {
                if let Some(v) = nums(operands, 4) {
                    let state = self.gs.current_mut();
                    state.fill_color = device_color::device_cmyk(v[0], v[1], v[2], v[3], 1.0);
                    state.fill_color_space = Rc::new(ColorSpace::DeviceCMYK);
                }
            }
            "K" => {
                if let Some(v) = nums(operands, 4) {
                    let state = self.gs.current_mut();
                    state.stroke_color = device_color::device_cmyk(v[0], v[1], v[2], v[3], 1.0);
                    state.stroke_color_space = Rc::new(ColorSpace::DeviceCMYK);
                }
            }

            "cs" => {
                if let Some(Object::Name(name)) = operands.first() {
                    self.set_color_space(name.as_str(), true);
                } else {
                    self.warn(RenderWarning::UnsupportedOperator {
                        operator: "cs (malformed operand)".to_string(),
                    });
                }
            }
            "CS" => {
                if let Some(Object::Name(name)) = operands.first() {
                    self.set_color_space(name.as_str(), false);
                } else {
                    self.warn(RenderWarning::UnsupportedOperator {
                        operator: "CS (malformed operand)".to_string(),
                    });
                }
            }
            "sc" => self.set_color_from_components(operands, true),
            "SC" => self.set_color_from_components(operands, false),
            "scn" => self.set_color_from_components_n(operands, true),
            "SCN" => self.set_color_from_components_n(operands, false),

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

            "Do" => {
                if let Some(Object::Name(name)) = operands.first() {
                    let name = name.as_str().to_string();
                    self.do_xobject(&name)?;
                } else {
                    self.warn(RenderWarning::UnsupportedOperator {
                        operator: "Do (malformed operand)".to_string(),
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

    /// Reserves `points` against the current path object's
    /// [`MAX_PATH_POINTS_PER_PATH`] budget (see
    /// [`super::path::PathAccumulator::reserve_points`]) and returns
    /// whether the caller should go ahead and actually add that geometry.
    /// Emits [`RenderWarning::PathPointBudgetExceeded`] exactly once, the
    /// first call that crosses the limit; every call after that for the
    /// same path object returns `false` silently (already warned).
    fn check_path_budget(&mut self, points: usize) -> bool {
        match self.path.reserve_points(points) {
            PathReserve::Ok => true,
            PathReserve::JustExceeded => {
                self.warn(RenderWarning::PathPointBudgetExceeded { max: MAX_PATH_POINTS_PER_PATH });
                false
            }
            PathReserve::AlreadyOver => false,
        }
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

        self.set_blend_mode_from_dict(dict);
        self.set_soft_mask_from_dict(dict);
    }

    /// `/BM` (ISO 32000-1 §11.3.5, Table 58): a bare blend-mode name, or an
    /// array of names tried in order until one is recognised. All 16
    /// standard names map onto `tiny_skia::BlendMode` -- a real
    /// implementation of the spec's compositing formulas, not an
    /// approximation. An unrecognised name (every entry of an array, or
    /// the single bare name) falls back to `Normal` per §11.3.5's own
    /// fallback rule, recording [`RenderWarning::UnsupportedBlendMode`].
    fn set_blend_mode_from_dict(&mut self, dict: &PdfDictionary) {
        let Some(bm_value) = dict.get("BM") else { return };
        let names: Vec<&str> = match bm_value {
            Object::Name(n) => vec![n.as_str()],
            Object::Array(arr) => arr
                .iter()
                .filter_map(|o| match o {
                    Object::Name(n) => Some(n.as_str()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };

        let resolved = names.iter().find_map(|n| resolve_blend_mode(n));
        match resolved {
            Some(bm) => self.gs.current_mut().blend_mode = bm,
            None => {
                self.gs.current_mut().blend_mode = BlendMode::SourceOver;
                if let Some(first) = names.first() {
                    self.warn(RenderWarning::UnsupportedBlendMode {
                        name: (*first).to_string(),
                    });
                }
            }
        }
    }

    /// `/SMask` (ISO 32000-1 §11.6.4.3): either `/None` (clears the
    /// active soft mask) or a soft-mask dictionary naming a transparency
    /// group (`/G`) to render and reduce to a mask (see
    /// [`Self::apply_soft_mask_group`]). Absent entirely (`dict` has no
    /// `/SMask` key at all): leaves the current soft mask untouched, since
    /// `gs` only updates the parameters actually present in the ExtGState
    /// dictionary (ISO 32000-1 §8.4.5).
    fn set_soft_mask_from_dict(&mut self, dict: &PdfDictionary) {
        match dict.get("SMask") {
            None => {}
            Some(Object::Name(n)) if n.as_str() == "None" => {
                self.gs.current_mut().soft_mask = None;
            }
            Some(Object::Dictionary(smask_dict)) => {
                let group = smask_dict.get("G").and_then(|o| match o {
                    Object::Stream(s) => Some(s),
                    _ => None,
                });
                let luminosity = !matches!(smask_dict.get("S"), Some(Object::Name(n)) if n.as_str() == "Alpha");
                if smask_dict.get("TR").is_some_and(|o| !matches!(o, Object::Name(n) if n.as_str() == "Identity")) {
                    self.warn(RenderWarning::SoftMaskParameterIgnored { parameter: "TR" });
                }
                if smask_dict.get("BC").is_some() {
                    self.warn(RenderWarning::SoftMaskParameterIgnored { parameter: "BC" });
                }
                match group {
                    Some(group_stream) => self.apply_soft_mask_group(group_stream, luminosity),
                    None => {
                        self.warn(RenderWarning::InvalidSoftMaskGroup);
                        self.gs.current_mut().soft_mask = None;
                    }
                }
            }
            Some(_) => {
                self.warn(RenderWarning::InvalidSoftMaskGroup);
                self.gs.current_mut().soft_mask = None;
            }
        }
    }

    /// Resolves an ExtGState `/SMask`'s transparency group (ISO 32000-1
    /// §11.6.5.2) into a canvas-sized 8-bit mask and installs it as the
    /// current graphics state's [`GraphicsState::soft_mask`]. Rendered
    /// using the CTM in effect *now* (at `gs` time), per §11.6.4.3 --
    /// separately from whatever CTM is active later when marks using this
    /// mask are actually painted.
    ///
    /// `luminosity == true` selects `/S /Luminosity` (the default): the
    /// group is rendered against an opaque **black** backdrop (the
    /// ISO 32000-1 default absent an explicit `/BC`, which this phase
    /// doesn't apply -- see [`RenderWarning::SoftMaskParameterIgnored`])
    /// and each pixel's mask value is that backdrop-composited result's
    /// luminance (Rec. 709 weights, via `tiny_skia::MaskType::Luminance`).
    /// `luminosity == false` selects `/S /Alpha`: the group is rendered
    /// against a fully transparent backdrop and each pixel's mask value is
    /// simply the resulting alpha.
    fn apply_soft_mask_group(&mut self, group_stream: &PdfStream, luminosity: bool) {
        let backdrop = if luminosity {
            Color::BLACK
        } else {
            Color::from_rgba(0.0, 0.0, 0.0, 0.0).expect("0,0,0,0 is a valid color")
        };
        let ctm = self.gs.current().ctm;
        let pixmap = match self.render_group_to_pixmap(group_stream, ctm, backdrop) {
            Ok(p) => p,
            Err(_) => {
                self.warn(RenderWarning::InvalidSoftMaskGroup);
                self.gs.current_mut().soft_mask = None;
                return;
            }
        };
        let mask_type = if luminosity { MaskType::Luminance } else { MaskType::Alpha };
        let mask = Mask::from_pixmap(pixmap.as_ref(), mask_type);
        self.gs.current_mut().soft_mask = Some(Rc::new(mask));
    }

    /// `cs`/`CS` (ISO 32000-1 8.6.8): resolves `name` (a Device name or a
    /// `/Resources /ColorSpace` entry) and resets the corresponding
    /// current colour to that space's initial value.
    fn set_color_space(&mut self, name: &str, fill: bool) {
        let resolved = self.resolve_color_space_operand(name);
        if resolved.is_unsupported() {
            self.warn(RenderWarning::UnsupportedColorSpace {
                reason: resolved.description(),
            });
        }
        if matches!(resolved, ColorSpace::IccApproximated { .. }) {
            self.warn(RenderWarning::IccColorApproximated);
        }
        let initial = resolved.initial_components();
        let color = resolved.to_rgba(&initial, 1.0);
        let rc = Rc::new(resolved);
        let state = self.gs.current_mut();
        if fill {
            state.fill_color_space = rc;
            if let Some(c) = color {
                state.fill_color = c;
            }
        } else {
            state.stroke_color_space = rc;
            if let Some(c) = color {
                state.stroke_color = c;
            }
        }
    }

    /// Resolves a `cs`/`CS` operand as either a bare Device name or a
    /// `/Resources /ColorSpace /<name>` lookup, sharing
    /// [`super::colorspace::resolve_color_space`]'s array/name grammar by
    /// wrapping `name` as an [`Object::Name`] first.
    fn resolve_color_space_operand(&self, name: &str) -> ColorSpace {
        let obj = Object::Name(crate::object::PdfName::new_unchecked(name));
        colorspace::resolve_color_space(&obj, self.resources)
    }

    /// `sc`/`SC` (ISO 32000-1 8.6.8): sets the current colour from raw
    /// numeric operands, interpreted against the current colour space (set
    /// by the most recent `cs`/`CS`, or a Device space's own operator).
    fn set_color_from_components(&mut self, operands: &[Object], fill: bool) {
        let comps: Vec<f64> = operands.iter().filter_map(as_f64).collect();
        self.apply_color_components(&comps, fill);
    }

    /// `scn`/`SCN`: as `sc`/`SC`, but tolerates (and, since Patterns are
    /// out of scope this phase, ignores-with-a-warning) a trailing Pattern
    /// name operand (ISO 32000-1 8.6.8, Table 74).
    fn set_color_from_components_n(&mut self, operands: &[Object], fill: bool) {
        if matches!(operands.last(), Some(Object::Name(_))) {
            self.warn(RenderWarning::PatternColorUnsupported);
            return;
        }
        let comps: Vec<f64> = operands.iter().filter_map(as_f64).collect();
        self.apply_color_components(&comps, fill);
    }

    fn apply_color_components(&mut self, comps: &[f64], fill: bool) {
        let space = if fill {
            self.gs.current().fill_color_space.clone()
        } else {
            self.gs.current().stroke_color_space.clone()
        };
        match space.to_rgba(comps, 1.0) {
            Some(color) => {
                let state = self.gs.current_mut();
                if fill {
                    state.fill_color = color;
                } else {
                    state.stroke_color = color;
                }
            }
            None => self.warn(RenderWarning::UnsupportedColorSpace {
                reason: space.description(),
            }),
        }
    }

    /// `Do` (ISO 32000-1 8.8): resolves `/Resources /XObject /<name>` and
    /// paints it -- both image (`/Subtype /Image`) and Form
    /// (`/Subtype /Form`) XObjects are painted this phase; anything else
    /// (e.g. `/PS`) is recorded as
    /// [`RenderWarning::UnsupportedXObjectSubtype`] and skipped.
    fn do_xobject(&mut self, name: &str) -> Result<(), NativeRenderError> {
        let xobject = self
            .resources
            .and_then(|r| r.get("XObject"))
            .and_then(|o| match o {
                Object::Dictionary(d) => Some(d),
                _ => None,
            })
            .and_then(|xo| xo.get(name));

        let Some(Object::Stream(stream)) = xobject else {
            self.warn(RenderWarning::MissingXObjectResource { name: name.to_string() });
            return Ok(());
        };

        match stream.dictionary.get("Subtype") {
            Some(Object::Name(n)) if n.as_str() == "Image" => {
                let fill_color = self.gs.current().fill_color;
                let result = image::decode_image_xobject(stream, self.resources, fill_color);
                self.paint_image_result(result, name);
                Ok(())
            }
            Some(Object::Name(n)) if n.as_str() == "Form" => self.do_form_xobject(stream),
            other => {
                let subtype = match other {
                    Some(Object::Name(n)) => n.as_str().to_string(),
                    _ => "(missing)".to_string(),
                };
                self.warn(RenderWarning::UnsupportedXObjectSubtype {
                    name: name.to_string(),
                    subtype,
                });
                Ok(())
            }
        }
    }

    /// Paints a Form XObject (ISO 32000-1 §8.10). If it declares a
    /// `/Group /S /Transparency` (§11.4), it is rendered *isolated* --
    /// contents composite among themselves at full opacity/Normal blend
    /// into an offscreen buffer, then the finished group as a whole is
    /// composited into the page using the *outer* `ca`/blend mode (this
    /// is what makes a semi-transparent overlapping group look right:
    /// otherwise each overlapping shape's alpha would compound instead of
    /// the group appearing "as one" at the given alpha). Every group is
    /// treated as isolated regardless of the actual `/I` entry, and
    /// knockout (`/K`) is not implemented -- a documented approximation
    /// (see [module docs](super)).
    ///
    /// A Form *without* a transparency `/Group` paints directly onto the
    /// current pixmap instead (no offscreen buffer/compositing step),
    /// inheriting the graphics state active at the point of invocation
    /// (colour, alpha, blend mode, clip, ...) exactly like `q ... Do ...
    /// Q` -- matching ISO 32000-1 §8.10.2's "the objects painted by the
    /// form shall be defined with respect to the graphics state in effect
    /// at the beginning of execution", with none of that state leaking
    /// back out afterward (a fresh nested `Interpreter`, discarded when
    /// this method returns, same as `Type3`'s recursion).
    fn do_form_xobject(&mut self, stream: &PdfStream) -> Result<(), NativeRenderError> {
        if self.form_depth >= MAX_FORM_XOBJECT_DEPTH {
            self.warn(RenderWarning::FormXObjectRecursionLimitExceeded);
            return Ok(());
        }

        let is_transparency_group = matches!(
            stream.dictionary.get("Group").and_then(|o| o.as_dictionary()).and_then(|g| g.get("S")),
            Some(Object::Name(n)) if n.as_str() == "Transparency"
        );

        if is_transparency_group {
            let state = self.gs.current();
            let (ctm, fill_alpha, blend_mode) = (state.ctm, state.fill_alpha, state.blend_mode);
            let mask = combined_mask(state);
            let transparent = Color::from_rgba(0.0, 0.0, 0.0, 0.0).expect("0,0,0,0 is a valid color");
            let group_pixmap = self.render_group_to_pixmap(stream, ctm, transparent)?;
            let paint = PixmapPaint {
                opacity: fill_alpha.clamp(0.0, 1.0),
                blend_mode,
                ..Default::default()
            };
            self.pixmap
                .draw_pixmap(0, 0, group_pixmap.as_ref(), &paint, Transform::identity(), mask.as_deref());
            Ok(())
        } else {
            self.run_form_xobject_inline(stream)
        }
    }

    /// Runs a non-group Form XObject's content stream directly against
    /// `self.pixmap` (no offscreen buffer), applying its `/Matrix` and
    /// clipping to its `/BBox` (both ISO 32000-1 §8.10.1), inheriting
    /// every other graphics-state parameter from the point of invocation.
    fn run_form_xobject_inline(&mut self, stream: &PdfStream) -> Result<(), NativeRenderError> {
        let base_ctm = self.gs.current().ctm;
        let form_ctm = form_matrix(&stream.dictionary).multiply(&base_ctm);

        let mut child_state = self.gs.current().clone();
        child_state.ctm = form_ctm;
        if let Some(bbox_mask) = form_bbox_mask(&stream.dictionary, form_ctm, self.canvas_w, self.canvas_h) {
            child_state.clip = Some(Rc::new(match &child_state.clip {
                Some(existing) => intersect_masks(existing, &bbox_mask),
                None => bbox_mask,
            }));
        }

        let resources = form_resources(&stream.dictionary).or(self.resources);
        let content = match image::decode_all(stream) {
            Ok(bytes) => bytes,
            Err(reason) => {
                self.warn(RenderWarning::ImageDecodeFailed {
                    name: "(form)".to_string(),
                    reason,
                });
                return Ok(());
            }
        };

        let mut child = Interpreter {
            pixmap: &mut *self.pixmap,
            gs: GraphicsStateStack::new(child_state, MAX_GRAPHICS_STATE_DEPTH),
            path: PathAccumulator::new(),
            pending_clip: None,
            resources,
            warnings: &mut *self.warnings,
            budget: &mut *self.budget,
            canvas_w: self.canvas_w,
            canvas_h: self.canvas_h,
            text_matrix: Matrix::identity(),
            text_line_matrix: Matrix::identity(),
            font_cache: HashMap::new(),
            type3_depth: self.type3_depth,
            form_depth: self.form_depth + 1,
        };

        child.run_content(&content)
    }

    /// Renders a transparency-group Form XObject's content into a fresh,
    /// canvas-sized offscreen [`Pixmap`], seeded with `backdrop` before
    /// painting -- shared by a genuine `/Group /S /Transparency` Form
    /// painted via `Do` (transparent backdrop) and by an ExtGState
    /// `/SMask` group (opaque black or transparent backdrop, depending on
    /// `/S`; see [`Self::apply_soft_mask_group`]). Group content paints at
    /// full opacity/Normal blend internally (isolated group semantics);
    /// the caller composites the *result* with the outer alpha/blend mode
    /// (for a visible group) or reduces it to a mask (for a soft mask).
    fn render_group_to_pixmap(
        &mut self,
        stream: &PdfStream,
        base_ctm: Matrix,
        backdrop: Color,
    ) -> Result<Pixmap, NativeRenderError> {
        let mut pixmap = Pixmap::new(self.canvas_w, self.canvas_h).ok_or(NativeRenderError::PixmapAllocationFailed {
            width: self.canvas_w,
            height: self.canvas_h,
        })?;
        pixmap.fill(backdrop);

        if self.form_depth >= MAX_FORM_XOBJECT_DEPTH {
            self.warn(RenderWarning::FormXObjectRecursionLimitExceeded);
            return Ok(pixmap);
        }

        let form_ctm = form_matrix(&stream.dictionary).multiply(&base_ctm);

        let mut child_state = self.gs.current().clone();
        child_state.ctm = form_ctm;
        child_state.fill_alpha = 1.0;
        child_state.stroke_alpha = 1.0;
        child_state.blend_mode = BlendMode::SourceOver;
        child_state.soft_mask = None;
        child_state.clip = form_bbox_mask(&stream.dictionary, form_ctm, self.canvas_w, self.canvas_h).map(Rc::new);

        let resources = form_resources(&stream.dictionary).or(self.resources);
        let content = match image::decode_all(stream) {
            Ok(bytes) => bytes,
            Err(reason) => {
                self.warn(RenderWarning::ImageDecodeFailed {
                    name: "(form group)".to_string(),
                    reason,
                });
                return Ok(pixmap);
            }
        };

        {
            let mut child = Interpreter {
                pixmap: &mut pixmap,
                gs: GraphicsStateStack::new(child_state, MAX_GRAPHICS_STATE_DEPTH),
                path: PathAccumulator::new(),
                pending_clip: None,
                resources,
                warnings: &mut *self.warnings,
                budget: &mut *self.budget,
                canvas_w: self.canvas_w,
                canvas_h: self.canvas_h,
                text_matrix: Matrix::identity(),
                text_line_matrix: Matrix::identity(),
                font_cache: HashMap::new(),
                type3_depth: self.type3_depth,
                form_depth: self.form_depth + 1,
            };

            child.run_content(&content)?;
        }

        Ok(pixmap)
    }

    /// `BI`/`ID`/`EI` (ISO 32000-1 8.9.7): decodes and paints an inline
    /// image using the same pipeline as an image XObject.
    fn show_inline_image(&mut self, img: &InlineImage) {
        let fill_color = self.gs.current().fill_color;
        let result = image::decode_inline_image(img, self.resources, fill_color);
        self.paint_image_result(result, "(inline)");
    }

    /// Shared paint step for both `Do` (image XObjects) and inline images:
    /// maps the decoded pixel grid's unit square into device space via the
    /// current CTM (ISO 32000-1 8.9.5.2: image space has its origin at the
    /// upper-left, `x` right, `y` *down*, mapped onto the CTM's unit
    /// square) and draws it, honoring the current clip and non-stroking
    /// constant alpha (`ca`). `JBIG2Decode`/`JPXDecode` (the documented
    /// hard gap) paint a flat mid-grey placeholder rectangle instead of
    /// the real image, so the render is never silently blank -- see
    /// [`super::image`]'s module docs.
    fn paint_image_result(&mut self, result: ImageResult, name: &str) {
        match result {
            ImageResult::Ok(decoded) => {
                if let Some(reason) = decoded.smask_warning.clone() {
                    self.warn(RenderWarning::ImageSoftMaskDecodeFailed {
                        name: name.to_string(),
                        reason,
                    });
                }
                self.draw_image_pixels(decoded);
            }
            ImageResult::UnsupportedFilter(filter) => {
                self.warn(RenderWarning::UnsupportedImageFilter {
                    name: name.to_string(),
                    filter,
                });
                self.paint_placeholder_rect();
            }
            ImageResult::Failed(reason) => {
                self.warn(RenderWarning::ImageDecodeFailed {
                    name: name.to_string(),
                    reason,
                });
            }
        }
    }

    /// Computes the pixel-space -> device-space transform (ISO 32000-1
    /// 8.9.5.2's fixed `[1/w 0 0 -1/h 0 1]` image matrix composed with the
    /// current CTM) and blits `decoded`'s RGBA buffer onto the canvas.
    fn draw_image_pixels(&mut self, decoded: super::image::DecodedImage) {
        let (width, height) = (decoded.width, decoded.height);
        let Some(size) = tiny_skia::IntSize::from_wh(width, height) else {
            return;
        };
        let Some(pixmap) = tiny_skia::Pixmap::from_vec(decoded.rgba, size) else {
            return;
        };

        let state = self.gs.current();
        let image_matrix = Matrix::new(1.0 / f64::from(width), 0.0, 0.0, -1.0 / f64::from(height), 0.0, 1.0);
        let pixel_to_device = image_matrix.multiply(&state.ctm);
        let transform = tiny_skia::Transform::from_row(
            pixel_to_device.a as f32,
            pixel_to_device.b as f32,
            pixel_to_device.c as f32,
            pixel_to_device.d as f32,
            pixel_to_device.e as f32,
            pixel_to_device.f as f32,
        );

        let paint = tiny_skia::PixmapPaint {
            opacity: state.fill_alpha.clamp(0.0, 1.0),
            blend_mode: state.blend_mode,
            // `PixmapPaint::default()`'s quality is `FilterQuality::Nearest`
            // (tiny-skia's own choice of fastest-over-best default), which
            // visibly aliases/stair-steps any image XObject that isn't
            // drawn at exactly 1:1 device-pixel scale -- the overwhelmingly
            // common case, since a page's declared image resolution rarely
            // matches its on-page display size at the render DPI. `Bicubic`
            // matches the visual quality a mainstream viewer (e.g. Preview,
            // Acrobat) produces for the same file; a single per-image
            // XObject draw is not a hot path the way per-pixel path-fill
            // is, so its extra cost over `Nearest` is not a real-world
            // concern.
            quality: tiny_skia::FilterQuality::Bicubic,
        };
        let clip = combined_mask(state);
        self.pixmap
            .draw_pixmap(0, 0, pixmap.as_ref(), &paint, transform, clip.as_deref());
    }

    /// Paints a flat, clearly-artificial mid-grey placeholder over the
    /// image's unit square -- used only for the documented JBIG2/JPX gap
    /// (see [`super::image`]), never for a genuine decode failure (which
    /// stays unpainted, matching this interpreter's general
    /// "record-a-warning-and-skip" convention for gaps that don't have a
    /// standard "broken image" visual convention).
    fn paint_placeholder_rect(&mut self) {
        let state = self.gs.current();
        let corners = [
            state.ctm.transform_point(0.0, 0.0),
            state.ctm.transform_point(1.0, 0.0),
            state.ctm.transform_point(1.0, 1.0),
            state.ctm.transform_point(0.0, 1.0),
        ]
        .map(|(x, y)| super::path::sanitize_point(x, y));

        let mut pb = PathAccumulator::new();
        pb.rect(corners);
        let Some(path) = pb.to_path() else { return };

        let mut paint = Paint::default();
        // A distinctive, unmistakably-artificial mid-grey: not white (the
        // page background), not black (plausible real content), matching
        // the "broken image" convention most browsers/viewers use.
        paint.set_color(Color::from_rgba8(160, 160, 160, 255));
        paint.blend_mode = state.blend_mode;
        paint.anti_alias = false;
        let clip = combined_mask(state);
        self.pixmap
            .fill_path(&path, &paint, FillRule::Winding, Transform::identity(), clip.as_deref());
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
        let blend_mode = state.blend_mode;
        let clip = combined_mask(state);
        let stroke_params = StrokeParams::from(state);

        // Render modes 4-7 add to the clip path in addition to painting
        // like 0-3; this phase paints identically but does not implement
        // the clip-add half (see `state::TextState::render_mode`'s docs).
        let mode = render_mode % 4;
        if mode == 0 || mode == 2 {
            let mut paint = Paint::default();
            paint.set_color(fill_color);
            paint.blend_mode = blend_mode;
            paint.anti_alias = true;
            self.pixmap
                .fill_path(&path, &paint, FillRule::Winding, Transform::identity(), clip.as_deref());
        }
        if mode == 1 || mode == 2 {
            let mut paint = Paint::default();
            paint.set_color(stroke_color);
            paint.blend_mode = blend_mode;
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
            budget: &mut *self.budget,
            canvas_w: self.canvas_w,
            canvas_h: self.canvas_h,
            text_matrix: Matrix::identity(),
            text_line_matrix: Matrix::identity(),
            font_cache: HashMap::new(),
            type3_depth: self.type3_depth + 1,
            form_depth: self.form_depth,
        };

        child.run_content(proc_bytes)
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
            let blend_mode = state.blend_mode;
            let clip = combined_mask(state);
            let stroke_params = StrokeParams::from(state);

            match mode {
                FillMode::Fill(rule) | FillMode::FillStroke(rule) => {
                    let mut paint = Paint::default();
                    paint.set_color(fill_color);
                    paint.blend_mode = blend_mode;
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
                    paint.blend_mode = blend_mode;
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
    ///
    /// `width` and each dash-array length are routed through
    /// [`super::path::sanitize_nonneg_magnitude`] rather than an unchecked
    /// `as f32` cast: `self.line_width` and `self.dash_array` both come
    /// straight from content-stream operands (`w`/`d`, ISO 32000-1:2008
    /// 8.4.3.2/8.4.3.6) via [`GraphicsState`] with only a "not negative"
    /// clamp applied at parse time, so an adversarial `w` with an
    /// astronomically large (or non-finite, via an `f64`-parse overflow)
    /// operand reaches this method otherwise unclamped. Unlike path
    /// coordinates, an extreme *width* doesn't need an extreme point
    /// anywhere on the path to trip `tiny-skia`'s internal scanline panic
    /// -- stroke-to-fill expansion alone is enough (see
    /// `sanitize_nonneg_magnitude`'s docs for the fuzz-found regression
    /// this closes: `path::sanitize_point`'s coordinate clamp alone did
    /// not cover this).
    ///
    /// `dash_phase` deliberately is *not* run through the same clamp: it
    /// is legitimately allowed to be negative (ISO 32000-1 8.4.3.6 puts no
    /// sign restriction on it) and `tiny_skia::StrokeDash::new` already
    /// guards it independently -- it rejects a non-finite phase outright
    /// (reported back here as `dash_invalid`, falling back to a solid
    /// stroke) and otherwise wraps any finite phase, however large, into
    /// `[0, interval_len)` *before* it can influence any path geometry, so
    /// it cannot reach `tiny-skia`'s rasterizer unnormalized the way an
    /// unclamped width could.
    fn build(&self) -> (Stroke, bool) {
        let det = self.ctm.a * self.ctm.d - self.ctm.b * self.ctm.c;
        let mut scale = det.abs().sqrt();
        if !scale.is_finite() || scale == 0.0 {
            scale = 1.0;
        }

        let mut stroke = Stroke {
            width: super::path::sanitize_nonneg_magnitude(self.line_width * scale),
            miter_limit: self.miter_limit.max(1.0) as f32,
            line_cap: self.line_cap,
            line_join: self.line_join,
            dash: None,
        };

        let mut dash_invalid = false;
        if !self.dash_array.is_empty() {
            let mut scaled: Vec<f32> = self
                .dash_array
                .iter()
                .map(|v| super::path::sanitize_nonneg_magnitude(v * scale))
                .collect();
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

/// Resolves a Form XObject's `/Matrix` (ISO 32000-1 §8.10.1, Table 95),
/// defaulting to the identity matrix if absent or malformed (untrusted
/// input: a non-6-element or non-numeric `/Matrix` is treated the same as
/// "not specified" rather than erroring the whole render).
fn form_matrix(dict: &PdfDictionary) -> Matrix {
    let Some(Object::Array(arr)) = dict.get("Matrix") else {
        return Matrix::identity();
    };
    let v: Vec<f64> = arr.iter().filter_map(as_f64).collect();
    if v.len() != 6 {
        return Matrix::identity();
    }
    Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5])
}

/// Resolves a Form XObject's own `/Resources` sub-dictionary (ISO 32000-1
/// §7.8.3), if present -- see the [module docs](super)'s "pre-resolved
/// `/Resources`" assumption for why this expects an already-dereferenced
/// dictionary rather than a dangling indirect reference.
fn form_resources(dict: &PdfDictionary) -> Option<&PdfDictionary> {
    match dict.get("Resources") {
        Some(Object::Dictionary(d)) => Some(d),
        _ => None,
    }
}

/// Builds a device-space clip mask from a Form XObject's `/BBox` (ISO
/// 32000-1 §8.10.1: required by the spec, but treated as "no additional
/// clip restriction" if absent/malformed rather than failing the render
/// -- untrusted input).
fn form_bbox_mask(dict: &PdfDictionary, form_ctm: Matrix, canvas_w: u32, canvas_h: u32) -> Option<Mask> {
    let Some(Object::Array(arr)) = dict.get("BBox") else {
        return None;
    };
    let v: Vec<f64> = arr.iter().filter_map(as_f64).collect();
    if v.len() != 4 {
        return None;
    }
    let corners = [
        form_ctm.transform_point(v[0], v[1]),
        form_ctm.transform_point(v[2], v[1]),
        form_ctm.transform_point(v[2], v[3]),
        form_ctm.transform_point(v[0], v[3]),
    ]
    .map(|(x, y)| super::path::sanitize_point(x, y));

    let mut pb = PathAccumulator::new();
    pb.rect(corners);
    let path = pb.to_path()?;
    let mut mask = Mask::new(canvas_w, canvas_h)?;
    mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
    Some(mask)
}

/// Combines the active clip and soft mask (ISO 32000-1 §11.6.4.3: a mark
/// is only painted where *both* the clip path and the soft mask allow
/// it) into a single mask reference for `tiny-skia`'s painting calls.
/// Allocates a combined buffer only when both are simultaneously active;
/// the common case (at most one set) is a cheap `Rc` clone.
fn combined_mask(state: &GraphicsState) -> Option<Rc<Mask>> {
    match (&state.clip, &state.soft_mask) {
        (None, None) => None,
        (Some(c), None) => Some(c.clone()),
        (None, Some(m)) => Some(m.clone()),
        (Some(c), Some(m)) => Some(Rc::new(intersect_masks(c, m))),
    }
}

/// Multiplies two same-size masks together (`a*b/255`), used to combine a
/// clip path with a soft mask, or a Form XObject's `/BBox` clip with
/// whatever clip was already active. Falls back to cloning `a` if the two
/// masks aren't the same size (defensive; every mask this interpreter
/// builds is canvas-sized, so this should never actually trigger).
fn intersect_masks(a: &Mask, b: &Mask) -> Mask {
    if a.width() != b.width() || a.height() != b.height() {
        return a.clone();
    }
    let data: Vec<u8> = a
        .data()
        .iter()
        .zip(b.data())
        .map(|(&x, &y)| ((u16::from(x) * u16::from(y)) / 255) as u8)
        .collect();
    let size = tiny_skia::IntSize::from_wh(a.width(), a.height()).expect("same size as an existing valid Mask");
    Mask::from_vec(data, size).unwrap_or_else(|| a.clone())
}

/// Maps a PDF blend-mode name (ISO 32000-1 §11.3.5, Table 57) onto
/// `tiny_skia::BlendMode`. All 16 standard names are supported directly
/// -- `tiny-skia` implements the same separable/non-separable compositing
/// formulas the spec defines, so this is a real mapping, not an
/// approximation. Returns `None` for anything else (a nonstandard/private
/// blend-mode name), which the caller falls back to `Normal` for.
fn resolve_blend_mode(name: &str) -> Option<BlendMode> {
    Some(match name {
        "Normal" | "Compatible" => BlendMode::SourceOver,
        "Multiply" => BlendMode::Multiply,
        "Screen" => BlendMode::Screen,
        "Overlay" => BlendMode::Overlay,
        "Darken" => BlendMode::Darken,
        "Lighten" => BlendMode::Lighten,
        "ColorDodge" => BlendMode::ColorDodge,
        "ColorBurn" => BlendMode::ColorBurn,
        "HardLight" => BlendMode::HardLight,
        "SoftLight" => BlendMode::SoftLight,
        "Difference" => BlendMode::Difference,
        "Exclusion" => BlendMode::Exclusion,
        "Hue" => BlendMode::Hue,
        "Saturation" => BlendMode::Saturation,
        "Color" => BlendMode::Color,
        "Luminosity" => BlendMode::Luminosity,
        _ => return None,
    })
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

    fn page() -> Rectangle {
        Rectangle::new(0.0, 0.0, 200.0, 200.0)
    }

    /// Adversarial: a content stream consisting of a long, *flat* (never
    /// nested) run of cheap operators exceeds a tiny operator budget and
    /// fails with a structured [`NativeRenderError::OperatorBudgetExceeded`]
    /// rather than running to completion (or, with the real production
    /// default, than being unbounded) -- exactly the "very long but never
    /// deeply nested" attack shape [`MAX_GRAPHICS_STATE_DEPTH`]/
    /// [`MAX_FORM_XOBJECT_DEPTH`]/[`font::MAX_TYPE3_DEPTH`] don't bound.
    #[test]
    fn operator_budget_exceeded_is_a_structured_error_not_a_hang() {
        let content = b"q Q ".repeat(1000);
        let err = render_content_stream_with_limits(&content, 16, 16, page(), None, 10, MAX_RENDER_DURATION)
            .expect_err("a content stream with 2000 operators must exceed a 10-operator budget");
        assert_eq!(err, NativeRenderError::OperatorBudgetExceeded { max: 10 });
    }

    /// A content stream comfortably inside the operator budget still
    /// renders normally (the budget is not so tight it breaks legitimate,
    /// modestly-sized content).
    #[test]
    fn operator_budget_not_exceeded_renders_normally() {
        let content = b"1 0 0 rg 10 10 20 20 re f";
        let out = render_content_stream_with_limits(content, 16, 16, page(), None, 10_000, MAX_RENDER_DURATION)
            .expect("well within budget");
        assert!(out.warnings.is_empty());
    }

    /// Adversarial: even a content stream that would stay under the
    /// operator-count budget can still be bounded by the wall-clock
    /// budget -- a near-zero time budget must trip
    /// [`NativeRenderError::RenderTimeBudgetExceeded`] rather than the
    /// render running unbounded.
    #[test]
    fn render_time_budget_exceeded_is_a_structured_error_not_a_hang() {
        let content = b"q Q ".repeat(100);
        let err = render_content_stream_with_limits(&content, 16, 16, page(), None, MAX_OPERATOR_COUNT, Duration::from_nanos(1))
            .expect_err("a near-zero time budget must be exceeded almost immediately");
        assert!(matches!(err, NativeRenderError::RenderTimeBudgetExceeded { .. }));
    }

    /// A self-referential Form XObject (see the dedicated definition-of-
    /// done test in `native::mod`'s test suite for the full scenario)
    /// hitting [`MAX_FORM_XOBJECT_DEPTH`] does not, by itself, exhaust the
    /// *operator* budget too -- the two limits are independent and this
    /// confirms the recursion-depth guard alone is what stops it well
    /// before millions of operators would need to run.
    #[test]
    fn form_xobject_recursion_guard_does_not_need_the_operator_budget_to_stop_it() {
        use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream};

        let mut form_dict = PdfDictionary::new();
        form_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Form")));
        form_dict.set(
            "BBox",
            Object::Array(PdfArray::from_objects(vec![
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(200.0),
                Object::Real(200.0),
            ])),
        );
        let form_stream = PdfStream::with_dictionary(form_dict, b"/Rec Do".to_vec());

        let mut xobjects = PdfDictionary::new();
        xobjects.set("Rec", Object::Stream(form_stream));
        let mut resources = PdfDictionary::new();
        resources.set("XObject", Object::Dictionary(xobjects));

        let content = b"/Rec Do";
        // A tight operator budget that would be exceeded if the
        // recursion guard *weren't* what stopped this first: each `Do`
        // invocation is one operator, and `MAX_FORM_XOBJECT_DEPTH` (12)
        // is far below this budget of 1000.
        let out = render_content_stream_with_limits(content, 16, 16, page(), Some(&resources), 1000, MAX_RENDER_DURATION)
            .expect("recursion depth guard must stop this well within the operator budget");
        assert!(out
            .warnings
            .iter()
            .any(|w| matches!(w, RenderWarning::FormXObjectRecursionLimitExceeded)));
    }

    /// Fuzz-found regression (`render_interpreter` cargo-fuzz target, see
    /// `path::MAX_COORDINATE_MAGNITUDE`'s docs): a path with one
    /// finite-but-absurdly-large coordinate no longer trips `tiny-skia`'s
    /// internal panic on out-of-range magnitude -- it renders successfully
    /// (the huge coordinate is clamped in device space, so the resulting
    /// shape is degenerate but the render itself completes normally).
    #[test]
    fn extreme_finite_path_coordinate_does_not_panic() {
        let content = b"0 0 m 0 16666666660 l 10 10 l f";
        let out = render_content_stream(content, 32, 32, page(), None).expect("must not panic on an extreme finite coordinate");
        assert!(out.warnings.is_empty());
    }

    /// Fuzz-found regression (`render_interpreter` cargo-fuzz target,
    /// crash artifact `crash-69d1f2a7ec9f0178343d175b1f9807f775e7709e`,
    /// see [`super::path::sanitize_nonneg_magnitude`]'s docs): an
    /// astronomically large `w` (line width) operand on an otherwise tiny,
    /// perfectly ordinary path used to reach `tiny_skia::Stroke::width`
    /// completely unclamped and trip the rasterizer's internal scanline
    /// panic during stroke-to-fill expansion -- even though every path
    /// *coordinate* here is well within
    /// [`super::path::MAX_COORDINATE_MAGNITUDE`], proving the coordinate
    /// clamp alone (exercised by the sibling
    /// `extreme_finite_path_coordinate_does_not_panic` test above) does
    /// not cover this attack shape. `StrokeParams::build` clamping `width`
    /// directly is what closes it.
    #[test]
    fn extreme_finite_stroke_width_does_not_panic() {
        // A 60-digit line width (~1e59): finite as `f64`, but many orders
        // of magnitude past any width a legitimate page could plausibly
        // use, and past `MAX_COORDINATE_MAGNITUDE`.
        let huge_width = "9".repeat(60);
        let content = format!("{huge_width} w 0 0 m 10 10 l s");
        let out =
            render_content_stream(content.as_bytes(), 32, 32, page(), None).expect("must not panic on an extreme finite stroke width");
        assert!(out.warnings.is_empty());
    }

    /// Same regression as above, but with a line-width literal long
    /// enough (well past `f64::MAX`'s ~309 decimal digits) that Rust's own
    /// `str::parse::<f64>` saturates it to `f64::INFINITY` rather than a
    /// large-but-finite value -- confirming the non-finite guard in
    /// `sanitize_nonneg_magnitude` (as opposed to only the magnitude
    /// clamp) is what stops this variant.
    #[test]
    fn infinite_stroke_width_does_not_panic() {
        let huge_width = "9".repeat(400);
        let content = format!("{huge_width} w 0 0 m 10 10 l s");
        let out = render_content_stream(content.as_bytes(), 32, 32, page(), None).expect("must not panic on an infinite stroke width");
        assert!(out.warnings.is_empty());
    }
}
