//! The `eframe::App` implementation: an MVP PDF viewer (open, page through,
//! zoom, jump via bookmarks) built directly on [`PdfRenderer`].

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;

use eframe::{App, Frame};
use egui::{CentralPanel, Color32, Context, Panel, ScrollArea, Ui};

use crate::editor::{BookmarkNode, Destination};
use crate::error::RenderError;
use crate::render::PdfRenderer;

use super::actions;
use super::search::SearchState;
use super::thumbnails::{self, ThumbnailStrip};
use super::viewer::PageViewer;

const DEFAULT_DPI: f32 = 96.0;
const MIN_DPI: f32 = 24.0;
const MAX_DPI: f32 = 600.0;

enum DocState {
    Empty,
    Loading,
    Loaded {
        renderer: Arc<PdfRenderer>,
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
        self.current_page = 0;
        self.opening = Some(actions::spawn(move || PdfRenderer::open_file(&path)));
    }

    fn poll_open(&mut self, ctx: &Context) {
        let Some(rx) = &self.opening else { return };
        match rx.try_recv() {
            Ok(Ok(renderer)) => {
                let page_count = renderer.page_count();
                let bookmarks = renderer.document().list_bookmarks().unwrap_or_default();
                self.doc = DocState::Loaded {
                    renderer: Arc::new(renderer),
                    page_count,
                    bookmarks,
                };
                self.opening = None;
            }
            Ok(Err(err)) => {
                self.doc = DocState::Error(err.to_string());
                self.opening = None;
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint(),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.doc = DocState::Error(
                    "failed to open file: worker thread ended unexpectedly".to_string(),
                );
                self.opening = None;
            }
        }
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

                    ui.separator();

                    let page_count = self.page_count();
                    let has_doc = page_count > 0;

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
                    {
                        ui.spinner();
                    }
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
        Panel::right("thumbnails")
            .frame(surface_frame(ui))
            .default_size(140.0)
            .show(ui, |ui| {
                const ROW_HEIGHT: f32 = thumbnails::MAX_DIMENSION as f32 + 24.0;
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
                        });
                    }
                });
            });
        if let Some(page_index) = jump_to {
            self.go_to_page(page_index);
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

                ScrollArea::both().show(ui, |ui| {
                    if let Some(texture) = self.viewer.texture() {
                        ui.image(texture);
                    } else if let Some(error) = &self.viewer.error {
                        ui.colored_label(Color32::RED, error);
                    } else {
                        ui.label("Rendering…");
                    }
                });
            }
        });
    }
}

impl App for PdfViewerApp {
    fn logic(&mut self, ctx: &Context, _frame: &mut Frame) {
        self.poll_open(ctx);
        if self.viewer.poll(ctx) {
            ctx.request_repaint();
        }
        if self.thumbnails.poll(ctx) {
            ctx.request_repaint();
        }
        if self.search.poll() {
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut Frame) {
        self.toolbar(ui);
        self.search_results_panel(ui);
        self.bookmarks_panel(ui);
        self.thumbnails_panel(ui);
        self.page_panel(ui);
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
