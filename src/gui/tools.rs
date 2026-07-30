//! The active annotation/redaction/signature-placement tool and its
//! in-progress drag/click gesture. All of these tools reduce to the same
//! shape: capture geometry over the rendered page (a rectangle drag, a
//! freehand stroke, or a single click), then either commit an edit
//! straight away or, for tools that need a bit of text first (a stamp's
//! label, a comment's contents, a redaction's reason), stash the
//! captured geometry and let the caller show a small prompt before
//! committing.

use egui::{Color32, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};

use super::coords::PageCoords;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tool {
    #[default]
    None,
    Highlight,
    Underline,
    Strikeout,
    FreeText,
    Stamp,
    Ink,
    Comment,
    Redact,
    SignPlace,
}

pub const ALL_TOOLS: [Tool; 10] = [
    Tool::None,
    Tool::Highlight,
    Tool::Underline,
    Tool::Strikeout,
    Tool::FreeText,
    Tool::Stamp,
    Tool::Ink,
    Tool::Comment,
    Tool::Redact,
    Tool::SignPlace,
];

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::None => "Select",
            Tool::Highlight => "Highlight",
            Tool::Underline => "Underline",
            Tool::Strikeout => "Strikeout",
            Tool::FreeText => "Text Box",
            Tool::Stamp => "Stamp",
            Tool::Ink => "Draw",
            Tool::Comment => "Comment",
            Tool::Redact => "Redact",
            Tool::SignPlace => "Place Signature",
        }
    }

    /// Whether finishing this tool's gesture needs a small text prompt
    /// (label/contents/reason) before the edit is committed, rather than
    /// committing immediately.
    pub fn needs_prompt(self) -> bool {
        matches!(
            self,
            Tool::FreeText | Tool::Stamp | Tool::Comment | Tool::Redact
        )
    }

    /// Whether this tool captures a full freehand stroke (every sampled
    /// point matters) rather than just a rectangle's two corners.
    fn is_freehand(self) -> bool {
        matches!(self, Tool::Ink)
    }

    /// Whether this tool captures a single click rather than a drag.
    fn is_click(self) -> bool {
        matches!(self, Tool::Comment)
    }
}

/// A finished gesture, in PDF user-space points: 2 entries for a
/// rectangle drag (opposite corners, not necessarily min/max ordered), 1
/// for a click (`Tool::Comment`), many for a freehand stroke (`Tool::Ink`).
pub struct FinishedGesture {
    pub tool: Tool,
    pub page_index: usize,
    pub points: Vec<(f64, f64)>,
}

#[derive(Default)]
pub struct ToolState {
    pub active: Tool,
    drag_points: Vec<Pos2>,
    dragging: bool,
    /// Set once a gesture needing a text prompt finishes; cleared once
    /// the caller submits or cancels it via [`Self::take_pending`]/
    /// [`Self::cancel_pending`]. The caller (`app.rs`) owns showing the
    /// actual prompt UI.
    pub pending: Option<FinishedGesture>,
    pub prompt_text: String,
}

impl ToolState {
    /// Draws the interactive capture region over the page image and
    /// updates the in-progress gesture for this frame. Returns a
    /// finished gesture the moment one completes *and* doesn't need a
    /// prompt (the caller should commit it immediately); a gesture that
    /// does need a prompt is stashed in `self.pending` instead.
    pub fn handle(
        &mut self,
        ui: &mut Ui,
        image_rect: Rect,
        coords: &PageCoords,
        page_index: usize,
    ) -> Option<FinishedGesture> {
        if self.active == Tool::None || self.pending.is_some() {
            return None;
        }

        let id = Id::new("gui_tool_capture");
        let response = ui.interact(image_rect, id, Sense::click_and_drag());

        if self.active.is_click() {
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let point = coords.to_page(pos);
                    return self.finish(page_index, vec![point]);
                }
            }
            return None;
        }

        if response.drag_started() {
            self.dragging = true;
            self.drag_points.clear();
        }

        if self.dragging {
            if let Some(pos) = response.interact_pointer_pos() {
                if self.active.is_freehand() {
                    if self.drag_points.last() != Some(&pos) {
                        self.drag_points.push(pos);
                    }
                } else if self.drag_points.is_empty() {
                    self.drag_points.push(pos);
                } else {
                    let last = self.drag_points.len() - 1;
                    if last == 0 {
                        self.drag_points.push(pos);
                    } else {
                        self.drag_points[last] = pos;
                    }
                }
            }
            self.paint_preview(ui);
        }

        if response.drag_stopped() && self.dragging {
            self.dragging = false;
            let points = std::mem::take(&mut self.drag_points);
            if points.len() >= 2 {
                let pdf_points = points.into_iter().map(|p| coords.to_page(p)).collect();
                return self.finish(page_index, pdf_points);
            }
        }

        None
    }

    fn finish(&mut self, page_index: usize, points: Vec<(f64, f64)>) -> Option<FinishedGesture> {
        let gesture = FinishedGesture {
            tool: self.active,
            page_index,
            points,
        };
        if gesture.tool.needs_prompt() {
            self.prompt_text.clear();
            self.pending = Some(gesture);
            None
        } else {
            Some(gesture)
        }
    }

    fn paint_preview(&self, ui: &Ui) {
        let painter = ui.painter();
        let stroke = Stroke::new(2.0, Color32::from_rgb(220, 60, 60));
        if self.active.is_freehand() {
            if self.drag_points.len() >= 2 {
                painter.line(self.drag_points.clone(), stroke);
            }
        } else if self.drag_points.len() == 2 {
            let rect = Rect::from_two_pos(self.drag_points[0], self.drag_points[1]);
            painter.rect_stroke(rect, 0.0, stroke, StrokeKind::Middle);
        }
    }

    pub fn take_pending(&mut self) -> Option<FinishedGesture> {
        self.pending.take()
    }

    pub fn cancel_pending(&mut self) {
        self.pending = None;
        self.prompt_text.clear();
    }

    /// Switching tools (or documents/pages) mid-gesture would leave
    /// stale drag state around; reset when that happens.
    pub fn set_active(&mut self, tool: Tool) {
        self.active = tool;
        self.dragging = false;
        self.drag_points.clear();
        self.pending = None;
        self.prompt_text.clear();
    }
}

/// Reduces a rectangle gesture's two (unordered) corner points to a
/// PDF-space `(llx, lly, urx, ury)` tuple.
pub fn bounding_rect(points: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let (x0, y0) = points[0];
    let (x1, y1) = points[points.len() - 1];
    (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
}
