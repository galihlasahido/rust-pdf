//! Click-to-fill form field overlay: draws an interactive region over
//! each widget on the current page (from `EditableDocument::
//! list_form_fields`), positioned via [`super::coords::PageCoords`], and
//! commits edits synchronously through [`crate::render::PdfRenderer::
//! edit_document`] (these are plain in-memory document mutations, not
//! rendering, so unlike page/thumbnail images they don't need a
//! background thread).

use egui::{Checkbox, Color32, TextEdit, Ui};

use crate::editor::FormFieldWidget;

use super::app::SharedRenderer;
use super::coords::PageCoords;

/// Per-field UI state that must survive across frames while a text field
/// is being edited (the in-progress, not-yet-committed draft text).
#[derive(Default)]
pub struct FormOverlayState {
    editing: Option<EditingField>,
}

struct EditingField {
    name: String,
    draft: String,
}

impl FormOverlayState {
    /// Draws every form field on `page_index` as an interactive overlay
    /// atop the already-rendered page image, and commits any edit made
    /// this frame straight into the document. Returns `true` if an edit
    /// was committed (caller should mark the document dirty and refresh
    /// the page/thumbnail caches, since the page's appearance changed).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        renderer: &SharedRenderer,
        page_index: usize,
        coords: &PageCoords,
    ) -> bool {
        let fields = {
            let renderer = renderer.read().unwrap_or_else(|p| p.into_inner());
            renderer
                .document()
                .list_form_fields(page_index)
                .unwrap_or_default()
        };

        let mut committed = false;
        for field in &fields {
            if self.show_field(ui, renderer, field, coords) {
                committed = true;
            }
        }
        committed
    }

    fn show_field(
        &mut self,
        ui: &mut Ui,
        renderer: &SharedRenderer,
        field: &FormFieldWidget,
        coords: &PageCoords,
    ) -> bool {
        let screen_rect = coords.rect_to_screen(&field.rect);
        if !ui.clip_rect().intersects(screen_rect) {
            return false;
        }

        match field.field_type.as_str() {
            "Tx" => self.show_text_field(ui, renderer, field, screen_rect),
            "Btn" if field.is_radio => show_radio_field(ui, renderer, field, screen_rect),
            "Btn" => show_checkbox_field(ui, renderer, field, screen_rect),
            "Ch" => show_choice_field(ui, renderer, field, screen_rect),
            _ => false,
        }
    }

    fn show_text_field(
        &mut self,
        ui: &mut Ui,
        renderer: &SharedRenderer,
        field: &FormFieldWidget,
        screen_rect: egui::Rect,
    ) -> bool {
        if field.read_only {
            return false;
        }

        let is_editing_this = self
            .editing
            .as_ref()
            .is_some_and(|e| e.name == field.name);

        if !is_editing_this {
            // Not being edited: an invisible clickable region that starts
            // editing on click, matching Acrobat's own "click to fill"
            // affordance -- the field's own baked-in appearance (already
            // painted into the page raster) is what's visible.
            let response = ui.put(
                screen_rect,
                egui::Button::new("").fill(Color32::TRANSPARENT).frame(false),
            );
            if response.clicked() {
                self.editing = Some(EditingField {
                    name: field.name.clone(),
                    draft: field.value.clone().unwrap_or_default(),
                });
            }
            return false;
        }

        let Some(editing) = &mut self.editing else {
            return false;
        };
        let response = ui.put(screen_rect, TextEdit::singleline(&mut editing.draft));
        let committed = response.lost_focus();
        if committed {
            let name = editing.name.clone();
            let value = editing.draft.clone();
            self.editing = None;
            let mut renderer = renderer.write().unwrap_or_else(|p| p.into_inner());
            return renderer
                .edit_document(|doc| doc.set_text_value(&name, &value))
                .is_ok();
        }
        response.request_focus();
        false
    }
}

fn show_checkbox_field(
    ui: &mut Ui,
    renderer: &SharedRenderer,
    field: &FormFieldWidget,
    screen_rect: egui::Rect,
) -> bool {
    if field.read_only {
        return false;
    }
    let mut checked = {
        let renderer = renderer.read().unwrap_or_else(|p| p.into_inner());
        renderer
            .document()
            .get_checkbox_checked(&field.name)
            .unwrap_or(false)
    };
    let response = ui.put(screen_rect, Checkbox::without_text(&mut checked));
    if response.changed() {
        let mut renderer = renderer.write().unwrap_or_else(|p| p.into_inner());
        return renderer
            .edit_document(|doc| doc.set_checkbox_checked(&field.name, checked))
            .is_ok();
    }
    false
}

fn show_radio_field(
    ui: &mut Ui,
    renderer: &SharedRenderer,
    field: &FormFieldWidget,
    screen_rect: egui::Rect,
) -> bool {
    if field.read_only {
        return false;
    }
    let Some(export_value) = &field.export_value else {
        return false;
    };
    let is_selected = {
        let renderer = renderer.read().unwrap_or_else(|p| p.into_inner());
        renderer
            .document()
            .get_radio_value(&field.name)
            .ok()
            .flatten()
            .as_deref()
            == Some(export_value.as_str())
    };
    let response = ui.put(screen_rect, egui::RadioButton::new(is_selected, ""));
    if response.clicked() && !is_selected {
        let mut renderer = renderer.write().unwrap_or_else(|p| p.into_inner());
        return renderer
            .edit_document(|doc| doc.set_radio_value(&field.name, export_value))
            .is_ok();
    }
    false
}

/// Clicking a choice field cycles to the next option rather than opening a
/// real dropdown -- a deliberate simplification for this pass (an overlay
/// positioned `egui::ComboBox` is a reasonable follow-up, not attempted
/// here). Still a working "click to fill" affordance, just not the final
/// polish.
fn show_choice_field(
    ui: &mut Ui,
    renderer: &SharedRenderer,
    field: &FormFieldWidget,
    screen_rect: egui::Rect,
) -> bool {
    if field.read_only || field.options.is_empty() {
        return false;
    }
    let response = ui.put(
        screen_rect,
        egui::Button::new("").fill(Color32::TRANSPARENT).frame(false),
    );
    if !response.clicked() {
        return false;
    }

    let current = {
        let renderer = renderer.read().unwrap_or_else(|p| p.into_inner());
        renderer.document().get_choice_value(&field.name).ok().flatten()
    };
    let next_index = current
        .and_then(|v| field.options.iter().position(|o| *o == v))
        .map(|i| (i + 1) % field.options.len())
        .unwrap_or(0);
    let next_value = field.options[next_index].clone();

    let mut renderer = renderer.write().unwrap_or_else(|p| p.into_inner());
    renderer
        .edit_document(|doc| doc.set_choice_value(&field.name, &next_value))
        .is_ok()
}
