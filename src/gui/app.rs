//! The `eframe::App` implementation: an MVP PDF viewer (open, page through,
//! zoom, jump via bookmarks) built directly on [`PdfRenderer`].

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, RwLock};

use eframe::{App, Frame};
use egui::{CentralPanel, Color32, Context, Panel, ScrollArea, Ui};

use crate::editor::{BookmarkNode, Destination, EditableDocument};
use crate::error::{PdfResult, RenderError};
use crate::render::PdfRenderer;
use crate::types::Rectangle;
use crate::Color;

use super::actions;
use super::coords::PageCoords;
use super::forms::FormOverlayState;
use super::search::SearchState;
use super::sign::{self, SignDialogState};
use super::thumbnails::{self, ThumbnailStrip};
use super::tools::{self, FinishedGesture, Tool, ToolState};
use super::viewer::PageViewer;

const DEFAULT_DPI: f32 = 96.0;
const MIN_DPI: f32 = 24.0;
const MAX_DPI: f32 = 600.0;

/// A page-management action requested from the thumbnail panel this
/// frame, applied after the panel's closure ends (see
/// [`PdfViewerApp::apply_page_action`]) since it needs `&mut self` for
/// `mark_dirty`/re-clamping `current_page`, which the closure -- already
/// borrowing `self.doc` -- can't grant alongside.
enum PageAction {
    RotateLeft(usize),
    RotateRight(usize),
    MoveUp(usize),
    MoveDown(usize),
    Delete(usize),
    InsertBlankAfter(usize),
}

/// Shared handle to the open document. A `RwLock` (rather than a bare
/// `Arc<PdfRenderer>`) because editing features need exclusive `&mut`
/// access to the underlying `EditableDocument` (see
/// `PdfRenderer::edit_document`) while background threads may still be
/// mid-render against the same document -- `.read()` for rendering (many
/// pages can render concurrently, same as before), `.write()` for edits
/// (naturally serializes against in-flight reads instead of racing them).
pub(super) type SharedRenderer = Arc<RwLock<PdfRenderer>>;

enum DocState {
    Empty,
    Loading,
    Loaded {
        renderer: SharedRenderer,
        page_count: usize,
        bookmarks: Vec<BookmarkNode>,
    },
    Error(String),
}

pub struct PdfViewerApp {
    doc: DocState,
    opening: Option<mpsc::Receiver<Result<PdfRenderer, RenderError>>>,
    current_page: usize,
    dpi: f32,
    viewer: PageViewer,
    thumbnails: ThumbnailStrip,
    search: SearchState,
    forms: FormOverlayState,
    tools: ToolState,

    /// Path the current document was opened from (or last saved to via
    /// "Save As…"). `None` when nothing is open.
    open_path: Option<PathBuf>,
    /// Set on every successful edit, cleared on a successful save.
    dirty: bool,
    /// Set once a redaction (a future pass) runs this session: redaction
    /// removes underlying bytes rather than just hiding them, which
    /// `save_incremental` cannot express -- `save`/`save_as` must route
    /// to `save_full_rewrite` once this is set, for the rest of the
    /// session. Threaded through now so the redaction feature doesn't
    /// need to also teach `save` about this later.
    needs_full_rewrite: bool,
    saving: Option<mpsc::Receiver<PdfResult<()>>>,
    /// Most recent edit/save failure, shown as a dismissable banner.
    last_error: Option<String>,
    /// The page index a delete has been armed for (first click), waiting
    /// on a second click to actually delete -- a lightweight two-step
    /// confirm instead of a modal dialog.
    delete_confirm: Option<usize>,
    /// Whether the annotations/redaction-log panel is toggled open.
    show_annotations_panel: bool,
    sign_dialog: SignDialogState,
}

impl Default for PdfViewerApp {
    fn default() -> Self {
        Self {
            doc: DocState::Empty,
            opening: None,
            current_page: 0,
            dpi: DEFAULT_DPI,
            viewer: PageViewer::default(),
            thumbnails: ThumbnailStrip::default(),
            search: SearchState::default(),
            forms: FormOverlayState::default(),
            tools: ToolState::default(),
            open_path: None,
            dirty: false,
            needs_full_rewrite: false,
            saving: None,
            last_error: None,
            delete_confirm: None,
            show_annotations_panel: false,
            sign_dialog: SignDialogState::default(),
        }
    }
}

impl PdfViewerApp {
    /// Creates the app, applying [`super::theme`]'s visual polish to the
    /// context before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        super::theme::apply(&cc.egui_ctx);
        Self::default()
    }

    fn open_file(&mut self, path: PathBuf) {
        self.doc = DocState::Loading;
        self.viewer.reset();
        self.thumbnails.reset();
        self.search.reset();
        self.forms = FormOverlayState::default();
        self.tools.set_active(Tool::None);
        self.sign_dialog = SignDialogState::default();
        self.current_page = 0;
        self.dirty = false;
        self.needs_full_rewrite = false;
        self.last_error = None;
        self.open_path = Some(path.clone());
        self.opening = Some(actions::spawn(move || PdfRenderer::open_file(&path)));
    }

    fn poll_open(&mut self, ctx: &Context) {
        let Some(rx) = &self.opening else { return };
        match rx.try_recv() {
            Ok(Ok(renderer)) => {
                let page_count = renderer.page_count();
                let bookmarks = renderer.document().list_bookmarks().unwrap_or_default();
                self.doc = DocState::Loaded {
                    renderer: Arc::new(RwLock::new(renderer)),
                    page_count,
                    bookmarks,
                };
                self.opening = None;
            }
            Ok(Err(RenderError::PasswordRequired)) => {
                self.doc = DocState::Error(
                    "This PDF is password-protected. Encrypted PDFs aren't supported yet."
                        .to_string(),
                );
                self.open_path = None;
                self.opening = None;
            }
            Ok(Err(err)) => {
                self.doc = DocState::Error(err.to_string());
                self.open_path = None;
                self.opening = None;
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint(),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.doc = DocState::Error(
                    "failed to open file: worker thread ended unexpectedly".to_string(),
                );
                self.open_path = None;
                self.opening = None;
            }
        }
    }

    /// Marks the document as having unsaved changes and drops the page
    /// caches so the next render/thumbnail reflects the edit -- call this
    /// after any successful mutation (form fill, page rotate/delete/
    /// insert/reorder, and later annotate/redact/sign).
    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.viewer.reset();
        self.thumbnails.reset();
    }

    fn save(&mut self) {
        self.trigger_save(self.needs_full_rewrite);
    }

    fn save_as(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).save_file() else {
            return;
        };
        self.open_path = Some(path);
        // Always a full rewrite for an arbitrary new path -- the simplest
        // safe default (an incremental update assumes it's appending to
        // bytes it already wrote, which isn't true when the target is a
        // fresh file).
        self.trigger_save(true);
    }

    fn trigger_save(&mut self, full_rewrite: bool) {
        let (DocState::Loaded { renderer, .. }, Some(path)) = (&self.doc, &self.open_path) else {
            return;
        };
        let renderer = renderer.clone();
        let path = path.clone();
        self.saving = Some(actions::spawn(move || {
            let renderer = renderer.read().unwrap_or_else(|p| p.into_inner());
            let doc = renderer.document();
            if full_rewrite {
                doc.save_full_rewrite(&path)
            } else {
                doc.save_incremental(&path)
            }
        }));
    }

    fn poll_save(&mut self, ctx: &Context) {
        let Some(rx) = &self.saving else { return };
        match rx.try_recv() {
            Ok(Ok(())) => {
                self.dirty = false;
                self.last_error = None;
                self.saving = None;
            }
            Ok(Err(err)) => {
                self.last_error = Some(format!("Save failed: {err}"));
                self.saving = None;
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint(),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.last_error =
                    Some("Save failed: worker thread ended unexpectedly".to_string());
                self.saving = None;
            }
        }
    }

    fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    fn page_count(&self) -> usize {
        match &self.doc {
            DocState::Loaded { page_count, .. } => *page_count,
            _ => 0,
        }
    }

    fn go_to_page(&mut self, page_index: usize) {
        let page_count = self.page_count();
        if page_count > 0 {
            self.current_page = page_index.min(page_count - 1);
            self.forms = FormOverlayState::default();
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let renderer_and_count = match &self.doc {
            DocState::Loaded {
                renderer,
                page_count,
                ..
            } => Some((Arc::clone(renderer), *page_count)),
            _ => None,
        };

        Panel::top("toolbar")
            .frame(surface_frame(ui))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Open PDF…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("PDF", &["pdf"])
                            .pick_file()
                        {
                            self.open_file(path);
                        }
                    }

                    let page_count = self.page_count();
                    let has_doc = page_count > 0;

                    if ui
                        .add_enabled(has_doc && self.dirty, egui::Button::new("Save"))
                        .clicked()
                    {
                        self.save();
                    }
                    if ui
                        .add_enabled(has_doc, egui::Button::new("Save As…"))
                        .clicked()
                    {
                        self.save_as();
                    }
                    if self.dirty {
                        ui.colored_label(Color32::YELLOW, "●").on_hover_text("Unsaved changes");
                    }

                    ui.separator();

                    if ui
                        .add_enabled(
                            has_doc && self.current_page > 0,
                            egui::Button::new("< Prev"),
                        )
                        .clicked()
                    {
                        self.go_to_page(self.current_page.saturating_sub(1));
                    }
                    ui.label(if has_doc {
                        format!("Page {} / {}", self.current_page + 1, page_count)
                    } else {
                        "No document".to_string()
                    });
                    if ui
                        .add_enabled(
                            has_doc && self.current_page + 1 < page_count,
                            egui::Button::new("Next >"),
                        )
                        .clicked()
                    {
                        self.go_to_page(self.current_page + 1);
                    }

                    ui.separator();

                    if ui
                        .add_enabled(has_doc, egui::Button::new("Zoom -"))
                        .clicked()
                    {
                        self.dpi = (self.dpi * 0.8).clamp(MIN_DPI, MAX_DPI);
                    }
                    ui.label(format!("{:.0}%", self.dpi / DEFAULT_DPI * 100.0));
                    if ui
                        .add_enabled(has_doc, egui::Button::new("Zoom +"))
                        .clicked()
                    {
                        self.dpi = (self.dpi * 1.25).clamp(MIN_DPI, MAX_DPI);
                    }
                    if ui
                        .add_enabled(has_doc, egui::Button::new("Reset"))
                        .clicked()
                    {
                        self.dpi = DEFAULT_DPI;
                    }

                    ui.separator();

                    let text_response = ui.add_enabled(
                        has_doc,
                        egui::TextEdit::singleline(&mut self.search.query)
                            .hint_text("Search text…")
                            .desired_width(160.0),
                    );
                    let search_clicked = ui
                        .add_enabled(has_doc, egui::Button::new("Search"))
                        .clicked();
                    let submitted = text_response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if let Some((renderer, page_count)) = &renderer_and_count {
                        if search_clicked || submitted {
                            self.search.run(renderer, *page_count);
                        }
                    }

                    if self.opening.is_some()
                        || self.viewer.is_loading()
                        || self.search.is_searching()
                        || self.is_saving()
                    {
                        ui.spinner();
                    }
                });

                let has_doc = self.page_count() > 0;
                ui.add_enabled_ui(has_doc, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Tool:");
                        for tool in tools::ALL_TOOLS {
                            if ui
                                .selectable_label(self.tools.active == tool, tool.label())
                                .clicked()
                            {
                                self.tools.set_active(tool);
                            }
                        }
                        ui.separator();
                        if ui
                            .selectable_label(self.show_annotations_panel, "Annotations")
                            .clicked()
                        {
                            self.show_annotations_panel = !self.show_annotations_panel;
                        }
                        if ui.button("Sign Document…").clicked() {
                            self.begin_sign(None);
                        }
                    });
                });
            });
    }

    fn bookmarks_panel(&mut self, ui: &mut Ui) {
        let DocState::Loaded { bookmarks, .. } = &self.doc else {
            return;
        };
        if bookmarks.is_empty() {
            return;
        }

        let mut jump_to = None;
        Panel::left("bookmarks")
            .frame(surface_frame(ui))
            .show(ui, |ui| {
                ui.heading("Bookmarks");
                ui.separator();
                ScrollArea::vertical().show(ui, |ui| {
                    for bookmark in bookmarks {
                        show_bookmark(ui, bookmark, &mut jump_to);
                    }
                });
            });
        if let Some(page_index) = jump_to {
            self.go_to_page(page_index);
        }
    }

    fn search_results_panel(&mut self, ui: &mut Ui) {
        if self.search.results().is_empty() && !self.search.is_searching() {
            return;
        }

        let mut jump_to = None;
        Panel::bottom("search_results")
            .frame(surface_frame(ui))
            .default_size(160.0)
            .show(ui, |ui| {
                if self.search.is_searching() {
                    ui.label("Searching…");
                    return;
                }
                let results = self.search.results();
                ui.label(format!("{} match(es)", results.len()));
                ScrollArea::vertical().show(ui, |ui| {
                    for hit in results {
                        let label = format!("Page {}: {}", hit.page_index + 1, hit.snippet);
                        if ui.selectable_label(false, label).clicked() {
                            jump_to = Some(hit.page_index);
                        }
                    }
                });
            });
        if let Some(page_index) = jump_to {
            self.go_to_page(page_index);
        }
    }

    fn annotations_panel(&mut self, ui: &mut Ui) {
        if !self.show_annotations_panel {
            return;
        }
        let DocState::Loaded { renderer, .. } = &self.doc else {
            return;
        };
        let renderer = Arc::clone(renderer);
        let current_page = self.current_page;

        let mut delete_id = None;
        Panel::bottom("annotations")
            .frame(surface_frame(ui))
            .default_size(180.0)
            .show(ui, |ui| {
                let (annotations, redactions) = {
                    let guard = renderer.read().unwrap_or_else(|p| p.into_inner());
                    let doc = guard.document();
                    (
                        doc.list_annotations(current_page).unwrap_or_default(),
                        doc.audit_log().to_vec(),
                    )
                };

                ScrollArea::vertical().show(ui, |ui| {
                    ui.heading(format!("Annotations on page {}", current_page + 1));
                    if annotations.is_empty() {
                        ui.label("No annotations on this page.");
                    }
                    for annot in &annotations {
                        ui.horizontal(|ui| {
                            let author = annot.author.as_deref().unwrap_or("(unknown)");
                            let contents = annot.contents.as_deref().unwrap_or("");
                            ui.label(format!("{:?} — {author}: {contents}", annot.kind));
                            if ui.small_button("🗑").clicked() {
                                delete_id = Some(annot.id);
                            }
                        });
                    }

                    ui.separator();
                    ui.heading("Redaction log");
                    if redactions.is_empty() {
                        ui.label("No redactions applied.");
                    }
                    for entry in &redactions {
                        let where_ = entry
                            .page_index
                            .map(|p| format!("page {}", p + 1))
                            .unwrap_or_else(|| "document".to_string());
                        ui.label(format!(
                            "{} — {where_} — {}: {}",
                            entry.timestamp, entry.actor, entry.reason
                        ));
                    }
                });
            });

        if let Some(id) = delete_id {
            let mut renderer = renderer.write().unwrap_or_else(|p| p.into_inner());
            match renderer.edit_document(|doc| doc.delete_annotation(current_page, id)) {
                Ok(()) => self.mark_dirty(),
                Err(err) => self.last_error = Some(format!("Delete annotation failed: {err}")),
            }
        }
    }

    fn thumbnails_panel(&mut self, ui: &mut Ui) {
        let DocState::Loaded {
            renderer,
            page_count,
            ..
        } = &self.doc
        else {
            return;
        };
        let renderer = Arc::clone(renderer);
        let page_count = *page_count;
        let current_page = self.current_page;

        let mut jump_to = None;
        let mut action = None;
        Panel::right("thumbnails")
            .frame(surface_frame(ui))
            .default_size(160.0)
            .show(ui, |ui| {
                const ROW_HEIGHT: f32 = thumbnails::MAX_DIMENSION as f32 + 48.0;
                ScrollArea::vertical().show_rows(ui, ROW_HEIGHT, page_count, |ui, row_range| {
                    for page_index in row_range {
                        self.thumbnails.request(&renderer, page_index);

                        ui.vertical_centered(|ui| {
                            let is_current = page_index == current_page;
                            if let Some(texture) = self.thumbnails.texture(page_index) {
                                let response = ui.add(
                                    egui::Button::new(egui::Image::new(texture))
                                        .selected(is_current),
                                );
                                if response.clicked() {
                                    jump_to = Some(page_index);
                                }
                            } else {
                                ui.add_sized(
                                    [
                                        thumbnails::MAX_DIMENSION as f32,
                                        thumbnails::MAX_DIMENSION as f32 * 0.75,
                                    ],
                                    egui::Spinner::new(),
                                );
                            }
                            ui.label(format!("{}", page_index + 1));

                            ui.horizontal(|ui| {
                                if ui.small_button("⟲").on_hover_text("Rotate left").clicked() {
                                    action = Some(PageAction::RotateLeft(page_index));
                                }
                                if ui.small_button("⟳").on_hover_text("Rotate right").clicked() {
                                    action = Some(PageAction::RotateRight(page_index));
                                }
                                if ui
                                    .add_enabled(page_index > 0, egui::Button::new("↑").small())
                                    .on_hover_text("Move up")
                                    .clicked()
                                {
                                    action = Some(PageAction::MoveUp(page_index));
                                }
                                if ui
                                    .add_enabled(
                                        page_index + 1 < page_count,
                                        egui::Button::new("↓").small(),
                                    )
                                    .on_hover_text("Move down")
                                    .clicked()
                                {
                                    action = Some(PageAction::MoveDown(page_index));
                                }
                                if ui
                                    .small_button("+")
                                    .on_hover_text("Insert blank page after")
                                    .clicked()
                                {
                                    action = Some(PageAction::InsertBlankAfter(page_index));
                                }
                                let armed = self.delete_confirm == Some(page_index);
                                let delete_response = ui
                                    .small_button(if armed { "Confirm?" } else { "🗑" })
                                    .on_hover_text(if armed {
                                        "Click again to permanently delete this page"
                                    } else {
                                        "Delete page"
                                    });
                                if delete_response.clicked() {
                                    if armed {
                                        action = Some(PageAction::Delete(page_index));
                                        self.delete_confirm = None;
                                    } else {
                                        self.delete_confirm = Some(page_index);
                                    }
                                }
                            });
                        });
                    }
                });
            });
        if let Some(page_index) = jump_to {
            self.go_to_page(page_index);
        }
        if let Some(action) = action {
            self.apply_page_action(&renderer, action);
        }
    }

    fn apply_page_action(&mut self, renderer: &SharedRenderer, action: PageAction) {
        let result: PdfResult<()> = {
            let mut renderer = renderer.write().unwrap_or_else(|p| p.into_inner());
            match action {
                PageAction::RotateLeft(idx) => {
                    renderer.edit_document(|doc| doc.rotate_page(idx, -90))
                }
                PageAction::RotateRight(idx) => {
                    renderer.edit_document(|doc| doc.rotate_page(idx, 90))
                }
                PageAction::MoveUp(idx) => {
                    renderer.edit_document(|doc| doc.move_page(idx, idx.saturating_sub(1)))
                }
                PageAction::MoveDown(idx) => {
                    renderer.edit_document(|doc| doc.move_page(idx, idx + 1))
                }
                PageAction::Delete(idx) => renderer.edit_document(|doc| doc.delete_page(idx)),
                PageAction::InsertBlankAfter(idx) => {
                    let (width, height) = renderer.page_size_pt(idx).unwrap_or((612.0, 792.0));
                    renderer
                        .edit_document(|doc| doc.insert_blank_page(idx + 1, width, height))
                        .map(|_| ())
                }
            }
        };

        match result {
            Ok(()) => {
                self.mark_dirty();
                if let DocState::Loaded {
                    renderer,
                    page_count,
                    ..
                } = &mut self.doc
                {
                    if let Ok(guard) = renderer.read() {
                        *page_count = guard.page_count();
                    }
                }
                let count = self.page_count();
                if count > 0 {
                    self.current_page = self.current_page.min(count - 1);
                }
            }
            Err(err) => self.last_error = Some(format!("Page action failed: {err}")),
        }
    }

    fn page_panel(&mut self, ui: &mut Ui) {
        CentralPanel::default().show(ui, |ui| match &self.doc {
            DocState::Empty => {
                ui.centered_and_justified(|ui| ui.label("Open a PDF to get started."));
            }
            DocState::Loading => {
                ui.centered_and_justified(|ui| ui.label("Opening…"));
            }
            DocState::Error(message) => {
                ui.centered_and_justified(|ui| ui.colored_label(Color32::RED, message));
            }
            DocState::Loaded {
                renderer,
                page_count,
                ..
            } => {
                let renderer = Arc::clone(renderer);
                let page_count = *page_count;
                self.viewer.show(&renderer, self.current_page, self.dpi);
                if self.current_page + 1 < page_count {
                    self.viewer
                        .prefetch(&renderer, self.current_page + 1, self.dpi);
                }
                if self.current_page > 0 {
                    self.viewer
                        .prefetch(&renderer, self.current_page - 1, self.dpi);
                }

                let mut committed = false;
                let mut finished_gesture = None;
                let current_page = self.current_page;
                let dpi = self.dpi;
                ScrollArea::both().show(ui, |ui| {
                    if let Some(texture) = self.viewer.texture() {
                        let image_response = ui.image(texture);
                        let page_size_pt = {
                            let guard = renderer.read().unwrap_or_else(|p| p.into_inner());
                            guard.page_size_pt(current_page)
                        };
                        if let Some(page_size_pt) = page_size_pt {
                            let coords =
                                PageCoords::for_page(page_size_pt, dpi, image_response.rect);
                            if self.tools.active == Tool::None {
                                if self.forms.show(ui, &renderer, current_page, &coords) {
                                    committed = true;
                                }
                            } else {
                                finished_gesture = self.tools.handle(
                                    ui,
                                    image_response.rect,
                                    &coords,
                                    current_page,
                                );
                            }
                        }
                    } else if let Some(error) = &self.viewer.error {
                        ui.colored_label(Color32::RED, error);
                    } else {
                        ui.label("Rendering…");
                    }
                });
                if let Some(gesture) = finished_gesture {
                    if gesture.tool == Tool::SignPlace {
                        let rect = tools::bounding_rect(&gesture.points);
                        self.begin_sign(Some(rect));
                    } else {
                        self.commit_gesture(&renderer, gesture, None);
                    }
                }
                if committed {
                    self.mark_dirty();
                }
            }
        });
    }
}

impl PdfViewerApp {
    /// Commits a finished annotation gesture (see `tools.rs`) into the
    /// document. `text` is the prompt text for tools that needed one
    /// (`FreeText`/`Stamp`/`Comment`) -- `None` for the immediate-commit
    /// tools (`Highlight`/`Underline`/`Strikeout`/`Ink`).
    fn commit_gesture(
        &mut self,
        renderer: &SharedRenderer,
        gesture: FinishedGesture,
        text: Option<&str>,
    ) {
        let page_index = gesture.page_index;
        let result: PdfResult<()> = {
            let mut renderer = renderer.write().unwrap_or_else(|p| p.into_inner());
            match gesture.tool {
                Tool::Highlight => {
                    let quads = selected_text_quads(&renderer, page_index, &gesture.points);
                    renderer
                        .edit_document(|doc| {
                            doc.add_highlight_annotation(page_index, &quads, Color::rgb(1.0, 1.0, 0.0))
                        })
                        .map(|_| ())
                }
                Tool::Underline => {
                    let quads = selected_text_quads(&renderer, page_index, &gesture.points);
                    renderer
                        .edit_document(|doc| {
                            doc.add_underline_annotation(page_index, &quads, Color::rgb(0.9, 0.1, 0.1))
                        })
                        .map(|_| ())
                }
                Tool::Strikeout => {
                    let quads = selected_text_quads(&renderer, page_index, &gesture.points);
                    renderer
                        .edit_document(|doc| {
                            doc.add_strikeout_annotation(page_index, &quads, Color::rgb(0.9, 0.1, 0.1))
                        })
                        .map(|_| ())
                }
                Tool::Ink => {
                    let stroke = gesture.points.clone();
                    renderer
                        .edit_document(|doc| {
                            doc.add_ink_annotation(page_index, &[stroke], Color::rgb(0.1, 0.3, 0.9), 2.0)
                        })
                        .map(|_| ())
                }
                Tool::Stamp => {
                    let (llx, lly, urx, ury) = tools::bounding_rect(&gesture.points);
                    let rect = Rectangle::new(llx, lly, urx, ury);
                    let label = text.unwrap_or_default();
                    renderer
                        .edit_document(|doc| {
                            doc.add_stamp_annotation(page_index, rect, label, Color::rgb(0.9, 0.5, 0.1))
                        })
                        .map(|_| ())
                }
                Tool::FreeText => {
                    let (llx, lly, urx, ury) = tools::bounding_rect(&gesture.points);
                    let rect = Rectangle::new(llx, lly, urx, ury);
                    let content = text.unwrap_or_default();
                    renderer
                        .edit_document(|doc| {
                            doc.add_freetext_annotation(page_index, rect, content, 12.0, Color::BLACK)
                        })
                        .map(|_| ())
                }
                Tool::Comment => {
                    let (x, y) = gesture.points[0];
                    let contents = text.unwrap_or_default();
                    renderer
                        .edit_document(|doc| doc.add_comment(page_index, (x, y), contents, None))
                        .map(|_| ())
                }
                Tool::Redact => {
                    let (llx, lly, urx, ury) = tools::bounding_rect(&gesture.points);
                    let rect = Rectangle::new(llx, lly, urx, ury);
                    let reason = text.unwrap_or_default();
                    let actor = current_actor();
                    renderer
                        .edit_document(|doc| doc.apply_redaction(page_index, rect, &actor, reason))
                        .map(|_| ())
                }
                // Signature placement doesn't create an annotation at all
                // -- it just records the drawn rect for the signing
                // dialog (see `sign.rs`) to place the visible signature
                // at, so it's not part of this commit path.
                Tool::SignPlace | Tool::None => Ok(()),
            }
        };
        let was_redaction = gesture.tool == Tool::Redact;

        match result {
            Ok(()) => {
                if was_redaction {
                    // Redaction removes underlying bytes rather than just
                    // hiding them -- `save_incremental` can't express
                    // that, so every save for the rest of this session
                    // must be a full rewrite.
                    self.needs_full_rewrite = true;
                }
                self.mark_dirty();
            }
            Err(err) => self.last_error = Some(format!("Annotation failed: {err}")),
        }
    }

    /// Shows the small text prompt for a gesture that needs one before
    /// it's committed (see `Tool::needs_prompt`). A no-op if nothing is
    /// pending.
    fn tool_prompt(&mut self, ctx: &Context) {
        let Some(tool) = self.tools.pending.as_ref().map(|g| g.tool) else {
            return;
        };
        let DocState::Loaded { renderer, .. } = &self.doc else {
            self.tools.cancel_pending();
            return;
        };
        let renderer = renderer.clone();

        let title = match tool {
            Tool::FreeText => "Add Text Box",
            Tool::Stamp => "Add Stamp",
            Tool::Comment => "Add Comment",
            Tool::Redact => "Redact",
            _ => "Add Annotation",
        };
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.text_edit_multiline(&mut self.tools.prompt_text);
                ui.horizontal(|ui| {
                    if ui.button("Add").clicked() {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if submit {
            if let Some(gesture) = self.tools.take_pending() {
                let text = std::mem::take(&mut self.tools.prompt_text);
                self.commit_gesture(&renderer, gesture, Some(&text));
            }
        } else if cancel {
            self.tools.cancel_pending();
        }
    }

    /// Opens the "Sign Document" dialog. `visible_rect` is the PDF-space
    /// rectangle captured via `Tool::SignPlace`, if signing was triggered
    /// that way rather than from the toolbar button directly.
    fn begin_sign(&mut self, visible_rect: Option<(f64, f64, f64, f64)>) {
        self.sign_dialog.open = true;
        self.sign_dialog.visible_rect = visible_rect;
        self.sign_dialog.error = None;
        self.tools.set_active(Tool::None);
    }

    fn poll_signing(&mut self, ctx: &Context) {
        let Some(rx) = &self.sign_dialog.signing else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(output_path)) => {
                self.sign_dialog = SignDialogState::default();
                self.open_file(output_path);
            }
            Ok(Err(err)) => {
                self.sign_dialog.error = Some(err);
                self.sign_dialog.signing = None;
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint(),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.sign_dialog.error =
                    Some("Signing failed: worker thread ended unexpectedly".to_string());
                self.sign_dialog.signing = None;
            }
        }
    }

    fn sign_dialog_window(&mut self, ctx: &Context) {
        if !self.sign_dialog.open {
            return;
        }
        let DocState::Loaded { renderer, .. } = &self.doc else {
            self.sign_dialog.open = false;
            return;
        };
        let renderer = Arc::clone(renderer);

        let mut close = false;
        let mut do_sign = false;
        egui::Window::new("Sign Document")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Certificate (PEM):");
                    if ui.button("Browse…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Certificate", &["pem", "crt", "cer"])
                            .pick_file()
                        {
                            self.sign_dialog.cert_path = Some(path);
                        }
                    }
                });
                ui.label(
                    self.sign_dialog
                        .cert_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(none selected)".to_string()),
                );

                ui.horizontal(|ui| {
                    ui.label("Private key (PEM):");
                    if ui.button("Browse…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Private key", &["pem", "key"])
                            .pick_file()
                        {
                            self.sign_dialog.key_path = Some(path);
                        }
                    }
                });
                ui.label(
                    self.sign_dialog
                        .key_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(none selected)".to_string()),
                );

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.sign_dialog.name);
                });
                ui.horizontal(|ui| {
                    ui.label("Reason:");
                    ui.text_edit_singleline(&mut self.sign_dialog.reason);
                });
                ui.horizontal(|ui| {
                    ui.label("Location:");
                    ui.text_edit_singleline(&mut self.sign_dialog.location);
                });
                ui.horizontal(|ui| {
                    ui.label("Contact info:");
                    ui.text_edit_singleline(&mut self.sign_dialog.contact_info);
                });

                ui.horizontal(|ui| {
                    ui.label("Algorithm:");
                    egui::ComboBox::new("sign_algorithm", "")
                        .selected_text(sign::algorithm_label(self.sign_dialog.algorithm))
                        .show_ui(ui, |ui| {
                            for algo in sign::ALGORITHMS {
                                ui.selectable_value(
                                    &mut self.sign_dialog.algorithm,
                                    algo,
                                    sign::algorithm_label(algo),
                                );
                            }
                        });
                });
                ui.checkbox(&mut self.sign_dialog.pades_b, "PAdES-B (CAdES baseline)");

                if self.sign_dialog.visible_rect.is_some() {
                    ui.label(
                        "A visible signature widget will be drawn on the document's first page.",
                    );
                } else {
                    ui.label("This will be an invisible signature (no visible widget).");
                }

                if let Some(error) = &self.sign_dialog.error {
                    ui.colored_label(Color32::RED, error);
                }

                ui.horizontal(|ui| {
                    let ready = self.sign_dialog.cert_path.is_some()
                        && self.sign_dialog.key_path.is_some()
                        && !self.sign_dialog.is_signing();
                    if ui.add_enabled(ready, egui::Button::new("Sign…")).clicked() {
                        do_sign = true;
                    }
                    if ui
                        .add_enabled(!self.sign_dialog.is_signing(), egui::Button::new("Cancel"))
                        .clicked()
                    {
                        close = true;
                    }
                    if self.sign_dialog.is_signing() {
                        ui.spinner();
                    }
                });
            });

        if do_sign {
            let Some(output_path) = rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .set_file_name("signed.pdf")
                .save_file()
            else {
                return;
            };
            let pdf_bytes = {
                let guard = renderer.read().unwrap_or_else(|p| p.into_inner());
                match guard.document().save_full_rewrite_to_bytes() {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        self.sign_dialog.error =
                            Some(format!("Failed to prepare document: {err}"));
                        return;
                    }
                }
            };
            let cert_path = self.sign_dialog.cert_path.clone().unwrap();
            let key_path = self.sign_dialog.key_path.clone().unwrap();
            let name = self.sign_dialog.name.clone();
            let reason = self.sign_dialog.reason.clone();
            let location = self.sign_dialog.location.clone();
            let contact_info = self.sign_dialog.contact_info.clone();
            let algorithm = self.sign_dialog.algorithm;
            let pades_b = self.sign_dialog.pades_b;
            let visible_rect = self.sign_dialog.visible_rect;
            self.sign_dialog.error = None;
            self.sign_dialog.signing = Some(actions::spawn(move || {
                sign::sign_in_background(
                    pdf_bytes,
                    cert_path,
                    key_path,
                    name,
                    reason,
                    location,
                    contact_info,
                    algorithm,
                    pades_b,
                    visible_rect,
                    output_path,
                )
            }));
        }
        if close {
            self.sign_dialog = SignDialogState::default();
        }
    }

    fn error_banner(&mut self, ui: &mut Ui) {
        let Some(message) = self.last_error.clone() else {
            return;
        };
        Panel::top("error_banner")
            .frame(egui::Frame::side_top_panel(ui.style()).fill(Color32::from_rgb(120, 30, 30)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::WHITE, &message);
                    if ui.small_button("✕").clicked() {
                        self.last_error = None;
                    }
                });
            });
    }
}

impl App for PdfViewerApp {
    fn logic(&mut self, ctx: &Context, _frame: &mut Frame) {
        self.poll_open(ctx);
        self.poll_save(ctx);
        if self.viewer.poll(ctx) {
            ctx.request_repaint();
        }
        if self.thumbnails.poll(ctx) {
            ctx.request_repaint();
        }
        if self.search.poll() {
            ctx.request_repaint();
        }
        self.tool_prompt(ctx);
        self.poll_signing(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut Frame) {
        self.toolbar(ui);
        self.error_banner(ui);
        self.search_results_panel(ui);
        self.annotations_panel(ui);
        self.bookmarks_panel(ui);
        self.thumbnails_panel(ui);
        self.page_panel(ui);
        self.sign_dialog_window(ui.ctx());
    }
}

/// A bookmark with children only expands/collapses on click here (a leaf
/// bookmark still navigates); jumping via a parent's own destination is
/// deferred to a follow-up pass since it needs a distinct click target from
/// the expand arrow.
fn show_bookmark(ui: &mut Ui, node: &BookmarkNode, jump_to: &mut Option<usize>) {
    if node.children.is_empty() {
        if ui.selectable_label(false, &node.title).clicked() {
            if let Some(page_index) = node.dest.as_ref().and_then(destination_page_index) {
                *jump_to = Some(page_index);
            }
        }
    } else {
        egui::CollapsingHeader::new(&node.title)
            .default_open(false)
            .show(ui, |ui| {
                for child in &node.children {
                    show_bookmark(ui, child, jump_to);
                }
            });
    }
}

/// Maps a Highlight/Underline/Strikeout drag gesture's PDF-space
/// rectangle to the actual text runs it overlaps (via
/// `extract_page_text_layout`), producing one quad per overlapping run
/// rather than a single coarse bounding box -- this is what makes the
/// markup track the underlying text instead of an approximate
/// hand-drawn rectangle. Falls back to the gesture's own bounding box if
/// the page has no extractable text under the drag, so the tool still
/// places *something* visible rather than silently adding an empty
/// annotation.
fn selected_text_quads(
    renderer: &PdfRenderer,
    page_index: usize,
    points: &[(f64, f64)],
) -> Vec<(f64, f64, f64, f64)> {
    let drag_rect = tools::bounding_rect(points);
    let quads = text_run_quads(renderer.document(), page_index, drag_rect);
    if quads.is_empty() {
        vec![drag_rect]
    } else {
        quads
    }
}

fn text_run_quads(
    document: &EditableDocument,
    page_index: usize,
    drag_rect: (f64, f64, f64, f64),
) -> Vec<(f64, f64, f64, f64)> {
    let Ok(page_id) = document.page_id_at(page_index) else {
        return Vec::new();
    };
    let Ok(runs) = document.extract_page_text_layout(page_id) else {
        return Vec::new();
    };
    let (dllx, dlly, durx, dury) = drag_rect;
    runs.into_iter()
        .filter_map(|run| {
            let (rllx, rlly, rurx, rury) = (run.x, run.y, run.x + run.width, run.y + run.height);
            let intersects = rllx < durx && rurx > dllx && rlly < dury && rury > dlly;
            intersects.then_some((rllx, rlly, rurx, rury))
        })
        .collect()
}

/// A best-effort "who did this" for the redaction audit trail
/// (`RedactionAuditEntry::actor`) -- the OS account name, since this app
/// has no user-identity system of its own.
fn current_actor() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn destination_page_index(dest: &Destination) -> Option<usize> {
    match dest {
        Destination::FitPage { page_index } | Destination::Xyz { page_index, .. } => {
            Some(*page_index)
        }
    }
}

/// A frame for the toolbar/bookmark chrome that's subtly offset from the
/// document canvas's own background -- just enough to read as a distinct
/// surface, not a hard line.
fn surface_frame(ui: &Ui) -> egui::Frame {
    let visuals = ui.visuals();
    let base = visuals.panel_fill;
    let delta: i16 = if visuals.dark_mode { 10 } else { -6 };
    let shift = |c: u8| (i16::from(c) + delta).clamp(0, 255) as u8;
    let fill = Color32::from_rgb(shift(base.r()), shift(base.g()), shift(base.b()));

    egui::Frame::side_top_panel(ui.style()).fill(fill)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_run_quads_finds_overlapping_runs_and_ignores_far_away_drags() {
        let renderer =
            PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");
        let document = renderer.document();

        let whole_page = text_run_quads(document, 0, (0.0, 0.0, 1000.0, 1000.0));
        assert!(
            !whole_page.is_empty(),
            "a page-covering drag should overlap at least one text run"
        );

        let nowhere = text_run_quads(document, 0, (100_000.0, 100_000.0, 100_001.0, 100_001.0));
        assert!(nowhere.is_empty());
    }

    #[test]
    fn selected_text_quads_falls_back_to_the_drag_rect_when_nothing_overlaps() {
        let renderer =
            PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");
        let points = [(100_000.0, 100_000.0), (100_001.0, 100_001.0)];

        let quads = selected_text_quads(&renderer, 0, &points);

        assert_eq!(quads, vec![(100_000.0, 100_000.0, 100_001.0, 100_001.0)]);
    }
}
