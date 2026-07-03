//! The graphics state (ISO 32000-1:2008 8.4 "Graphics State") and its
//! `q`/`Q` save/restore stack (8.4.2).

use std::rc::Rc;

use tiny_skia::{Color, LineCap, LineJoin, Mask};

use crate::types::Matrix;

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
