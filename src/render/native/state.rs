//! The graphics state (ISO 32000-1:2008 8.4 "Graphics State") and its
//! `q`/`Q` save/restore stack (8.4.2).

use std::rc::Rc;

use tiny_skia::{Color, LineCap, LineJoin, Mask};

use crate::types::Matrix;

use super::font::ResolvedFont;

/// The subset of ISO 32000-1 9.3 "Text State Parameters" this phase
/// implements. Per 9.3 these *are* graphics-state parameters (saved/
/// restored by `q`/`Q`, unlike the text object's `Tm`/`Tlm` matrices,
/// which live only on the interpreter itself and reset at every `BT` --
/// see `interpreter.rs`).
///
/// `Debug` is implemented by hand (rather than derived) so it doesn't
/// need to recursively dump `font`'s embedded font-program bytes (which
/// could be megabytes) just to debug-print an otherwise-small struct.
#[derive(Clone)]
pub(super) struct TextState {
    /// `Tc` (9.3.2): additional spacing added after every glyph, in
    /// unscaled text space units.
    pub char_spacing: f64,
    /// `Tw` (9.3.3): additional spacing added after single-byte code `32`
    /// only (ISO 32000-1 9.3.3's explicit carve-out for multi-byte
    /// codes), in unscaled text space units.
    pub word_spacing: f64,
    /// `Tz` (9.3.4), already converted from a percentage to a factor
    /// (`100 Tz` -> `1.0`).
    pub horizontal_scale: f64,
    /// `TL` (9.3.5): used by `T*`/`'`/`"`/`TD`.
    pub leading: f64,
    /// `Tf`'s font operand, resolved once and cached for the lifetime of
    /// this render (see `interpreter.rs`'s `font_cache`).
    pub font: Option<Rc<ResolvedFont>>,
    /// The resource name `Tf` selected `font` from, kept only so a
    /// warning about an unrenderable font can name it.
    pub font_resource_name: Option<String>,
    /// `Tf`'s size operand (`Tfs`, 9.3.1).
    pub font_size: f64,
    /// `Tr` (9.3.6): fill (0), stroke (1), fill+stroke (2), invisible
    /// (3), or one of the `+4` "also add to clip" variants (4-7). This
    /// phase paints modes 4-7 identically to their 0-3 counterpart but
    /// does **not** add the glyph outlines to the clipping path -- an
    /// intentional simplification, not a silent correctness bug, since
    /// clip-mode text is rare and the fallback (paint normally, clip
    /// unaffected) is closer to "render more than asked" than to
    /// "silently wrong".
    pub render_mode: i64,
    /// `Ts` (9.3.7): vertical displacement of the baseline.
    pub rise: f64,
}

impl std::fmt::Debug for TextState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextState")
            .field("char_spacing", &self.char_spacing)
            .field("word_spacing", &self.word_spacing)
            .field("horizontal_scale", &self.horizontal_scale)
            .field("leading", &self.leading)
            .field("font_resource_name", &self.font_resource_name)
            .field("font_size", &self.font_size)
            .field("render_mode", &self.render_mode)
            .field("rise", &self.rise)
            .field("font_is_set", &self.font.is_some())
            .finish()
    }
}

impl TextState {
    pub(super) fn initial() -> Self {
        Self {
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scale: 1.0,
            leading: 0.0,
            font: None,
            font_resource_name: None,
            font_size: 0.0,
            render_mode: 0,
            rise: 0.0,
        }
    }
}

/// One entry of the graphics-state stack.
///
/// Only the subset of ISO 32000-1 Table 52 ("Device-Independent Graphics
/// State Parameters") this phase implements is modeled: CTM, stroke/fill
/// color (Device color spaces only), line width/cap/join/miter/dash,
/// constant alpha (`ca`/`CA`), and the clipping path. Text state (Tc, Tw,
/// Tz, ...) doesn't exist yet because text-showing operators are out of
/// scope this phase (see [`super`] module docs).
#[derive(Debug, Clone)]
pub(super) struct GraphicsState {
    /// Current transformation matrix: maps user space to device
    /// (pixel-raster) space (ISO 32000-1 8.3.4). Initialized to the
    /// page's MediaBox-to-device mapping, then updated by `cm`.
    pub ctm: Matrix,
    pub fill_color: Color,
    pub stroke_color: Color,
    /// `ca` (ISO 32000-1 8.4.5 Table 58): constant, non-stroking alpha.
    pub fill_alpha: f32,
    /// `CA`: constant, stroking alpha.
    pub stroke_alpha: f32,
    /// Line width in *user space* units (ISO 32000-1 8.4.3.2); scaled by
    /// the CTM's approximate uniform scale factor at paint time to get a
    /// device-space stroke width (see `interpreter.rs`'s
    /// `effective_device_line_width`).
    pub line_width: f64,
    pub line_cap: LineCap,
    pub line_join: LineJoin,
    pub miter_limit: f64,
    /// User-space dash array and phase (ISO 32000-1 8.4.3.6). Empty array
    /// means a solid (non-dashed) stroke.
    pub dash_array: Vec<f64>,
    pub dash_phase: f64,
    /// The active clipping path as an alpha mask, or `None` meaning "no
    /// clip restriction" (the entire canvas is paintable). `Rc` so `q`
    /// (clone-on-push) is O(1) unless/until the clip is actually
    /// narrowed by a `W`/`W*` inside the saved state.
    pub clip: Option<Rc<Mask>>,
    /// ISO 32000-1 9.3 "Text State Parameters" -- part of the graphics
    /// state (see [`TextState`]'s own docs for why `Tm`/`Tlm` are *not*
    /// here).
    pub text: TextState,
}

impl GraphicsState {
    /// The initial graphics state at the start of content-stream
    /// interpretation (ISO 32000-1 8.4.1's implicit defaults), given the
    /// page-space-to-device-space matrix computed from the page's
    /// MediaBox and the requested raster size.
    pub(super) fn initial(page_to_device: Matrix) -> Self {
        Self {
            ctm: page_to_device,
            fill_color: Color::BLACK,
            stroke_color: Color::BLACK,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            // ISO 32000-1 Table 52 default: 1.0.
            line_width: 1.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            // ISO 32000-1 Table 52 default: 10.0 (tiny-skia's own
            // `Stroke::default()` uses 4.0, so this must be set
            // explicitly rather than relying on that default).
            miter_limit: 10.0,
            dash_array: Vec::new(),
            dash_phase: 0.0,
            clip: None,
            text: TextState::initial(),
        }
    }
}

/// The `q`/`Q` graphics-state stack.
pub(super) struct GraphicsStateStack {
    stack: Vec<GraphicsState>,
    max_depth: usize,
}

impl GraphicsStateStack {
    pub(super) fn new(initial: GraphicsState, max_depth: usize) -> Self {
        Self {
            stack: vec![initial],
            max_depth,
        }
    }

    pub(super) fn current(&self) -> &GraphicsState {
        // Invariant: `stack` is never empty -- constructed with one
        // element and `pop` refuses to remove the last one.
        self.stack.last().expect("graphics state stack is never empty")
    }

    pub(super) fn current_mut(&mut self) -> &mut GraphicsState {
        self.stack.last_mut().expect("graphics state stack is never empty")
    }

    /// `q`: pushes a copy of the current state. Returns `Err` if this
    /// would exceed `max_depth` (see
    /// [`super::error::NativeRenderError::GraphicsStateStackOverflow`]).
    pub(super) fn push(&mut self) -> Result<(), ()> {
        if self.stack.len() >= self.max_depth {
            return Err(());
        }
        let top = self.current().clone();
        self.stack.push(top);
        Ok(())
    }

    /// `Q`: pops the current state. Returns `false` (rather than
    /// erroring) if there is nothing to pop beyond the initial state --
    /// unbalanced `Q` is a real-world producer bug this interpreter
    /// tolerates (see
    /// [`super::error::RenderWarning::UnbalancedRestore`]).
    pub(super) fn pop(&mut self) -> bool {
        if self.stack.len() <= 1 {
            return false;
        }
        self.stack.pop();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_matches_iso_defaults() {
        let gs = GraphicsState::initial(Matrix::identity());
        assert_eq!(gs.line_width, 1.0);
        assert_eq!(gs.miter_limit, 10.0);
        assert_eq!(gs.fill_alpha, 1.0);
        assert!(gs.clip.is_none());
    }

    #[test]
    fn push_pop_restores_previous_state() {
        let mut stack = GraphicsStateStack::new(GraphicsState::initial(Matrix::identity()), 8);
        stack.current_mut().line_width = 1.0;
        stack.push().unwrap();
        stack.current_mut().line_width = 5.0;
        assert_eq!(stack.current().line_width, 5.0);
        assert!(stack.pop());
        assert_eq!(stack.current().line_width, 1.0);
    }

    #[test]
    fn pop_below_initial_is_rejected_not_panicking() {
        let mut stack = GraphicsStateStack::new(GraphicsState::initial(Matrix::identity()), 8);
        assert!(!stack.pop());
        // Stack still usable afterward.
        assert_eq!(stack.current().line_width, 1.0);
    }

    #[test]
    fn push_beyond_max_depth_errs() {
        let mut stack = GraphicsStateStack::new(GraphicsState::initial(Matrix::identity()), 2);
        stack.push().unwrap(); // depth 2, at max
        assert!(stack.push().is_err()); // would be depth 3
    }
}
