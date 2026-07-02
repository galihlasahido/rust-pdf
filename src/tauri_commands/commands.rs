//! The nine Tauri commands for this phase.
//!
//! Every command follows the same shape: a plain `..._impl` async
//! function containing the real logic (no Tauri types, so it is directly
//! unit-testable and reusable from a non-Tauri host), and a thin
//! `#[tauri::command]` wrapper that extracts Tauri's `State`/`AppHandle`
//! and forwards to it. See the [module docs](super) for the overall
//! architecture and error/progress-reporting conventions.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::editor::EditableDocument;
use crate::types::Rectangle;

use super::error::CommandError;
use super::progress::{ProgressEvent, ProgressReporter, PROGRESS_EVENT_NAME};
use super::render_actor::{RenderedPage, ViewportRequest};
use super::state::{AppState, DocumentHandle};

/// Upper bound on an `open_document`/`sign_document` output path length,
/// purely a sanity guard against a pathological/mistaken argument (e.g.
/// an entire file's contents accidentally passed as a "path") rather than
/// a real filesystem limit.
const MAX_PATH_LEN: usize = 4096;

/// Upper bound on a `search_text` query length: a substring search itself
/// can't runaway, but there is no reason a legitimate search query should
/// ever approach this, and rejecting early avoids doing a full per-page
/// scan for an obviously-wrong (e.g. an entire document accidentally
/// pasted in) argument.
const MAX_QUERY_LEN: usize = 10_000;

fn validate_path_argument(path: &str) -> Result<(), CommandError> {
    if path.is_empty() {
        return Err(CommandError::invalid_argument("path must not be empty"));
    }
    if path.len() > MAX_PATH_LEN {
        return Err(CommandError::invalid_argument(format!(
            "path exceeds the maximum supported length of {MAX_PATH_LEN} bytes"
        )));
    }
    Ok(())
}

fn emitting_progress_reporter<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> ProgressReporter {
    use tauri::Emitter;
    std::sync::Arc::new(move |event: ProgressEvent| {
        // Progress is a best-effort UX nicety: if no listener is
        // registered, or the app is shutting down, `emit` returning an
        // error is not something the operation itself should fail for.
        let _ = app.emit(PROGRESS_EVENT_NAME, event);
    })
}

// ===================================================================
// open_document
// ===================================================================

/// Arguments for [`open_document`].
#[derive(Debug, Clone, Deserialize)]
pub struct OpenDocumentRequest {
    /// Filesystem path to the PDF to open.
    pub path: String,
    /// Password to try (as both user and owner password) if the document
    /// turns out to be encrypted.
    pub password: Option<String>,
}

/// Result of a successful [`open_document`] call.
#[derive(Debug, Clone, Serialize)]
pub struct OpenDocumentResult {
    /// Handle to use for every subsequent command on this document.
    pub handle: u64,
    /// Number of pages in the document.
    pub page_count: usize,
}

/// Opens a PDF file for editing (parsing its structure via
/// [`EditableDocument::open`]) and registers it in `state`, returning a
/// handle for use by every other command.
///
/// Large files are handled efficiently: [`EditableDocument::open`] reads
/// through [`crate::parser::PdfReader::from_file`], which memory-maps the
/// file rather than loading it fully into the process heap (see that
/// type's docs), so opening a multi-gigabyte PDF does not itself require
/// gigabytes of RSS.
pub async fn open_document_impl(
    state: &AppState,
    request: OpenDocumentRequest,
) -> Result<OpenDocumentResult, CommandError> {
    validate_path_argument(&request.path)?;
    let path = PathBuf::from(&request.path);

    let metadata = std::fs::metadata(&path).map_err(CommandError::from)?;
    if !metadata.is_file() {
        return Err(CommandError::invalid_argument(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() == 0 {
        return Err(CommandError::invalid_argument("file is empty"));
    }

    let worker_path = path.clone();
    let doc = state
        .pool
        .run(move || EditableDocument::open(&worker_path).map_err(CommandError::from))
        .await?;

    let page_count = doc.page_count().map_err(CommandError::from)?;
    let handle = state.insert_document(path, request.password, doc);

    Ok(OpenDocumentResult {
        handle: handle.0,
        page_count,
    })
}

/// Tauri command wrapper for [`open_document_impl`].
#[tauri::command]
pub async fn open_document(
    state: tauri::State<'_, AppState>,
    request: OpenDocumentRequest,
) -> Result<OpenDocumentResult, CommandError> {
    open_document_impl(&state, request).await
}

// ===================================================================
// render_page
// ===================================================================

/// Arguments for [`render_page`].
#[derive(Debug, Clone, Deserialize)]
pub struct RenderPageRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    /// Zero-based page index.
    pub page_index: usize,
    /// Dots per inch to rasterize at (ISO 32000-1 §7.7.3.3: PDF user
    /// space is 1/72 inch).
    pub dpi: f32,
    /// Device-pixel sub-rectangle `(x, y, width, height)` of the
    /// full-page-at-`dpi` raster to render, for tiled zoom/pan. `None`
    /// renders the whole page.
    pub viewport: Option<ViewportRequest>,
}

/// Renders one page to an RGBA raster via the dedicated
/// [`super::render_actor::RenderActor`] (see that module's docs for why
/// rendering doesn't go through [`super::worker::WorkerPool`]).
pub async fn render_page_impl(
    state: &AppState,
    request: RenderPageRequest,
) -> Result<RenderedPage, CommandError> {
    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;
    state
        .render_actor
        .render_page(
            handle,
            entry.path.clone(),
            entry.password.clone(),
            request.page_index,
            request.dpi,
            request.viewport,
        )
        .await
}

/// Tauri command wrapper for [`render_page_impl`].
#[tauri::command]
pub async fn render_page(
    state: tauri::State<'_, AppState>,
    request: RenderPageRequest,
) -> Result<RenderedPage, CommandError> {
    render_page_impl(&state, request).await
}

// ===================================================================
// extract_text
// ===================================================================

/// Arguments for [`extract_text`].
#[derive(Debug, Clone, Deserialize)]
pub struct ExtractTextRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    /// A single zero-based page to extract, or `None` for every page.
    pub page_index: Option<usize>,
}

/// One page's extracted text.
#[derive(Debug, Clone, Serialize)]
pub struct PageText {
    /// Zero-based page index.
    pub page_index: usize,
    /// Extracted text (see [`EditableDocument::extract_page_text`] for
    /// what "extracted" means -- content-stream `Tj`/`TJ`/`'`/`"`
    /// operands in showing order, not a layout-aware reconstruction).
    pub text: String,
}

fn extract_pages_with_progress(
    doc: &EditableDocument,
    handle: DocumentHandle,
    indices: &[usize],
    progress: &ProgressReporter,
) -> Result<Vec<PageText>, CommandError> {
    let total = indices.len() as u64;
    let mut pages = Vec::with_capacity(indices.len());
    for (done, &index) in indices.iter().enumerate() {
        let page_id = doc.page_id_at(index).map_err(CommandError::from)?;
        let text = doc.extract_page_text(page_id).map_err(CommandError::from)?;
        pages.push(PageText {
            page_index: index,
            text,
        });
        progress(ProgressEvent {
            operation: "extract_text",
            handle: Some(handle.0),
            current: done as u64 + 1,
            total: Some(total),
            message: Some(format!("page {} of {}", done + 1, total)),
        });
    }
    Ok(pages)
}

/// Extracts text from one page, or every page, of an open document.
pub async fn extract_text_impl(
    state: &AppState,
    request: ExtractTextRequest,
    progress: ProgressReporter,
) -> Result<Vec<PageText>, CommandError> {
    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;

    state
        .pool
        .run(move || {
            let doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let page_count = doc.page_count().map_err(CommandError::from)?;
            let indices: Vec<usize> = match request.page_index {
                Some(index) => {
                    if index >= page_count {
                        return Err(CommandError::invalid_argument(format!(
                            "page index {index} out of range (document has {page_count} pages)"
                        )));
                    }
                    vec![index]
                }
                None => (0..page_count).collect(),
            };
            extract_pages_with_progress(&doc, handle, &indices, &progress)
        })
        .await
}

/// Tauri command wrapper for [`extract_text_impl`]; emits
/// [`ProgressEvent`]s under [`PROGRESS_EVENT_NAME`] as pages complete.
#[tauri::command]
pub async fn extract_text<R: tauri::Runtime>(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle<R>,
    request: ExtractTextRequest,
) -> Result<Vec<PageText>, CommandError> {
    let progress = emitting_progress_reporter(app);
    extract_text_impl(&state, request, progress).await
}

// ===================================================================
// search_text
// ===================================================================

/// Arguments for [`search_text`].
#[derive(Debug, Clone, Deserialize)]
pub struct SearchTextRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    /// Text to search for.
    pub query: String,
    /// Whether the match is case-sensitive.
    pub case_sensitive: bool,
}

/// One search hit.
#[derive(Debug, Clone, Serialize)]
pub struct SearchMatch {
    /// Zero-based page index the match was found on.
    pub page_index: usize,
    /// Byte offset of the match within that page's extracted text.
    pub offset: usize,
    /// A short excerpt of extracted text around the match, for display.
    pub snippet: String,
}

const SEARCH_SNIPPET_RADIUS: usize = 40;

fn find_matches_in_page(
    page_index: usize,
    text: &str,
    query: &str,
    case_sensitive: bool,
) -> Vec<SearchMatch> {
    let (haystack, needle): (String, String) = if case_sensitive {
        (text.to_string(), query.to_string())
    } else {
        (text.to_lowercase(), query.to_lowercase())
    };
    if needle.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut search_from = 0usize;
    while let Some(found_at) = haystack[search_from..].find(&needle) {
        let offset = search_from + found_at;
        let snippet_start = text
            .char_indices()
            .rev()
            .find(|&(i, _)| i <= offset.saturating_sub(SEARCH_SNIPPET_RADIUS))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let snippet_end = (offset + needle.len() + SEARCH_SNIPPET_RADIUS).min(text.len());
        // `snippet_end` may land mid-character; walk back to the nearest
        // char boundary so the slice below can't panic on untrusted
        // multi-byte extracted text.
        let mut snippet_end = snippet_end;
        while snippet_end > snippet_start && !text.is_char_boundary(snippet_end) {
            snippet_end -= 1;
        }
        let snippet = text[snippet_start..snippet_end].to_string();
        matches.push(SearchMatch {
            page_index,
            offset,
            snippet,
        });
        search_from = offset + needle.len().max(1);
        if search_from >= haystack.len() {
            break;
        }
    }
    matches
}

/// Searches for `query` across every page of an open document, reporting
/// progress as each page is scanned.
pub async fn search_text_impl(
    state: &AppState,
    request: SearchTextRequest,
    progress: ProgressReporter,
) -> Result<Vec<SearchMatch>, CommandError> {
    if request.query.is_empty() {
        return Err(CommandError::invalid_argument("query must not be empty"));
    }
    if request.query.len() > MAX_QUERY_LEN {
        return Err(CommandError::invalid_argument(format!(
            "query exceeds the maximum supported length of {MAX_QUERY_LEN} bytes"
        )));
    }

    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;

    state
        .pool
        .run(move || {
            let doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let page_count = doc.page_count().map_err(CommandError::from)?;
            let mut all_matches = Vec::new();
            for page_index in 0..page_count {
                let page_id = doc.page_id_at(page_index).map_err(CommandError::from)?;
                let text = doc.extract_page_text(page_id).map_err(CommandError::from)?;
                all_matches.extend(find_matches_in_page(
                    page_index,
                    &text,
                    &request.query,
                    request.case_sensitive,
                ));
                progress(ProgressEvent {
                    operation: "search_text",
                    handle: Some(handle.0),
                    current: page_index as u64 + 1,
                    total: Some(page_count as u64),
                    message: Some(format!("page {} of {}", page_index + 1, page_count)),
                });
            }
            Ok(all_matches)
        })
        .await
}

/// Tauri command wrapper for [`search_text_impl`].
#[tauri::command]
pub async fn search_text<R: tauri::Runtime>(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle<R>,
    request: SearchTextRequest,
) -> Result<Vec<SearchMatch>, CommandError> {
    let progress = emitting_progress_reporter(app);
    search_text_impl(&state, request, progress).await
}

// ===================================================================
// apply_edit
// ===================================================================

/// One structural or content edit understood by [`apply_edit`].
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EditOperation {
    /// Byte-level substring replace within a page's text-showing
    /// operators; see [`EditableDocument::replace_page_text`].
    ReplaceText {
        page_index: usize,
        find: String,
        replace: String,
    },
    /// Rotates a page by a multiple of 90 degrees; see
    /// [`EditableDocument::rotate_page`].
    RotatePage { page_index: usize, degrees: i64 },
    /// Deletes a page; see [`EditableDocument::delete_page`].
    DeletePage { page_index: usize },
    /// Inserts a blank page (PDF-point media box); see
    /// [`EditableDocument::insert_blank_page`].
    InsertBlankPage {
        index: usize,
        width: f64,
        height: f64,
    },
    /// Moves a page; see [`EditableDocument::move_page`].
    MovePage { from: usize, to: usize },
}

/// Arguments for [`apply_edit`].
#[derive(Debug, Clone, Deserialize)]
pub struct ApplyEditRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    pub operation: EditOperation,
}

/// Result of [`apply_edit`]: only [`EditOperation::ReplaceText`]
/// produces a meaningful count.
#[derive(Debug, Clone, Serialize)]
pub struct ApplyEditResult {
    /// Number of occurrences replaced, for [`EditOperation::ReplaceText`];
    /// `0` for every other operation kind.
    pub replacements: usize,
}

/// Applies one edit operation to an already-open document's in-memory
/// object graph. Changes are only persisted once [`save_document`] is
/// called.
pub async fn apply_edit_impl(
    state: &AppState,
    request: ApplyEditRequest,
) -> Result<ApplyEditResult, CommandError> {
    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;

    state
        .pool
        .run(move || {
            let mut doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let replacements = match request.operation {
                EditOperation::ReplaceText {
                    page_index,
                    find,
                    replace,
                } => {
                    let page_id = doc.page_id_at(page_index).map_err(CommandError::from)?;
                    doc.replace_page_text(page_id, &find, &replace)
                        .map_err(CommandError::from)?
                }
                EditOperation::RotatePage { page_index, degrees } => {
                    doc.rotate_page(page_index, degrees)
                        .map_err(CommandError::from)?;
                    0
                }
                EditOperation::DeletePage { page_index } => {
                    doc.delete_page(page_index).map_err(CommandError::from)?;
                    0
                }
                EditOperation::InsertBlankPage {
                    index,
                    width,
                    height,
                } => {
                    doc.insert_blank_page(index, width, height)
                        .map_err(CommandError::from)?;
                    0
                }
                EditOperation::MovePage { from, to } => {
                    doc.move_page(from, to).map_err(CommandError::from)?;
                    0
                }
            };
            Ok(ApplyEditResult { replacements })
        })
        .await
}

/// Tauri command wrapper for [`apply_edit_impl`].
#[tauri::command]
pub async fn apply_edit(
    state: tauri::State<'_, AppState>,
    request: ApplyEditRequest,
) -> Result<ApplyEditResult, CommandError> {
    apply_edit_impl(&state, request).await
}

// ===================================================================
// save_document
// ===================================================================

/// How [`save_document`] should persist pending edits; see the
/// [`crate::editor`] module docs for the incremental-vs-full-rewrite
/// tradeoff.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveMode {
    Incremental,
    FullRewrite,
}

/// Arguments for [`save_document`].
#[derive(Debug, Clone, Deserialize)]
pub struct SaveDocumentRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    /// Output path; `None` overwrites the path the document was opened
    /// from.
    pub path: Option<String>,
    pub mode: SaveMode,
}

/// Result of [`save_document`].
#[derive(Debug, Clone, Serialize)]
pub struct SaveDocumentResult {
    /// The path actually written to.
    pub path: String,
    /// Number of bytes written.
    pub bytes_written: usize,
}

/// Saves an open document's pending edits to disk.
pub async fn save_document_impl(
    state: &AppState,
    request: SaveDocumentRequest,
    progress: ProgressReporter,
) -> Result<SaveDocumentResult, CommandError> {
    if let Some(path) = &request.path {
        validate_path_argument(path)?;
    }

    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;
    let output_path = match &request.path {
        Some(path) => PathBuf::from(path),
        None => entry.path.clone(),
    };

    progress(ProgressEvent {
        operation: "save_document",
        handle: Some(handle.0),
        current: 0,
        total: Some(1),
        message: Some("saving".to_string()),
    });

    let save_path = output_path.clone();
    let bytes_written = state
        .pool
        .run(move || {
            let doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let bytes = match request.mode {
                SaveMode::Incremental => doc.save_incremental_to_bytes(),
                SaveMode::FullRewrite => doc.save_full_rewrite_to_bytes(),
            }
            .map_err(CommandError::from)?;
            std::fs::write(&save_path, &bytes).map_err(CommandError::from)?;
            Ok(bytes.len())
        })
        .await?;

    progress(ProgressEvent {
        operation: "save_document",
        handle: Some(handle.0),
        current: 1,
        total: Some(1),
        message: Some("done".to_string()),
    });

    Ok(SaveDocumentResult {
        path: output_path.to_string_lossy().into_owned(),
        bytes_written,
    })
}

/// Tauri command wrapper for [`save_document_impl`].
#[tauri::command]
pub async fn save_document<R: tauri::Runtime>(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle<R>,
    request: SaveDocumentRequest,
) -> Result<SaveDocumentResult, CommandError> {
    let progress = emitting_progress_reporter(app);
    save_document_impl(&state, request, progress).await
}

// ===================================================================
// fill_form
// ===================================================================

/// A value to assign to one AcroForm field (ISO 32000-1 §12.7.3/12.7.4),
/// dispatched by [`EditableDocument::field_type`].
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FormFieldValue {
    /// A text field's (`/FT /Tx`) value.
    Text { value: String },
    /// A checkbox field's (`/FT /Btn`, no radio flag) checked state.
    Checkbox { checked: bool },
    /// A radio-group field's (`/FT /Btn`, radio flag) selected export
    /// value.
    Radio { export_value: String },
    /// A choice field's (`/FT /Ch`) selected value.
    Choice { value: String },
}

/// Arguments for [`fill_form`].
#[derive(Debug, Clone, Deserialize)]
pub struct FillFormRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    /// Field name -> value to set. Applied in iteration (map) order;
    /// each is independently reported in [`FillFormResult::errors`]
    /// rather than aborting the whole request on the first bad field
    /// name, since a form-filling frontend typically wants to apply
    /// every field it can and show per-field errors rather than lose all
    /// other edits because of one typo'd field name.
    pub fields: std::collections::HashMap<String, FormFieldValue>,
}

/// Result of [`fill_form`].
#[derive(Debug, Clone, Serialize)]
pub struct FillFormResult {
    /// Number of fields successfully updated.
    pub updated: usize,
    /// `(field_name, error)` for every field that failed.
    pub errors: Vec<(String, CommandError)>,
}

fn set_form_field(doc: &mut EditableDocument, name: &str, value: FormFieldValue) -> Result<(), CommandError> {
    match value {
        FormFieldValue::Text { value } => doc.set_text_value(name, &value).map_err(CommandError::from),
        FormFieldValue::Checkbox { checked } => {
            doc.set_checkbox_checked(name, checked).map_err(CommandError::from)
        }
        FormFieldValue::Radio { export_value } => doc
            .set_radio_value(name, &export_value)
            .map_err(CommandError::from),
        FormFieldValue::Choice { value } => {
            doc.set_choice_value(name, &value).map_err(CommandError::from)
        }
    }
}

/// Sets one or more AcroForm field values on an open document.
pub async fn fill_form_impl(
    state: &AppState,
    request: FillFormRequest,
) -> Result<FillFormResult, CommandError> {
    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;

    state
        .pool
        .run(move || {
            let mut doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut updated = 0usize;
            let mut errors = Vec::new();
            for (name, value) in request.fields {
                match set_form_field(&mut doc, &name, value) {
                    Ok(()) => updated += 1,
                    Err(err) => errors.push((name, err)),
                }
            }
            Ok(FillFormResult { updated, errors })
        })
        .await
}

/// Tauri command wrapper for [`fill_form_impl`].
#[tauri::command]
pub async fn fill_form(
    state: tauri::State<'_, AppState>,
    request: FillFormRequest,
) -> Result<FillFormResult, CommandError> {
    fill_form_impl(&state, request).await
}

// ===================================================================
// add_annotation
// ===================================================================

/// Plain-data RGB color, `0.0..=1.0` per channel (ISO 32000-1 §8.6.3
/// DeviceRGB).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ColorRequest {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl From<ColorRequest> for Color {
    fn from(c: ColorRequest) -> Self {
        Color::rgb(c.r, c.g, c.b)
    }
}

/// Plain-data rectangle (PDF user-space points, lower-left origin; ISO
/// 32000-1 §7.9.5).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RectangleRequest {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

impl From<RectangleRequest> for Rectangle {
    fn from(r: RectangleRequest) -> Self {
        Rectangle::new(r.llx, r.lly, r.urx, r.ury)
    }
}

/// One annotation to create, dispatched to the matching
/// `EditableDocument::add_*_annotation`/`add_comment` method.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnnotationRequest {
    Highlight {
        page_index: usize,
        quads: Vec<(f64, f64, f64, f64)>,
        color: ColorRequest,
    },
    Underline {
        page_index: usize,
        quads: Vec<(f64, f64, f64, f64)>,
        color: ColorRequest,
    },
    StrikeOut {
        page_index: usize,
        quads: Vec<(f64, f64, f64, f64)>,
        color: ColorRequest,
    },
    FreeText {
        page_index: usize,
        rect: RectangleRequest,
        text: String,
        font_size: f64,
        color: ColorRequest,
    },
    Stamp {
        page_index: usize,
        rect: RectangleRequest,
        label: String,
        color: ColorRequest,
    },
    Ink {
        page_index: usize,
        strokes: Vec<Vec<(f64, f64)>>,
        color: ColorRequest,
        line_width: f64,
    },
    Comment {
        page_index: usize,
        at: (f64, f64),
        contents: String,
        author: Option<String>,
    },
}

/// Arguments for [`add_annotation`].
#[derive(Debug, Clone, Deserialize)]
pub struct AddAnnotationRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    pub annotation: AnnotationRequest,
}

/// A PDF indirect object reference (ISO 32000-1 §7.3.10), plain-data for
/// IPC.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ObjectIdResult {
    pub number: u32,
    pub generation: u16,
}

impl From<crate::types::ObjectId> for ObjectIdResult {
    fn from(id: crate::types::ObjectId) -> Self {
        Self {
            number: id.number,
            generation: id.generation,
        }
    }
}

/// Result of [`add_annotation`]: the new annotation's object id(s).
/// [`AnnotationRequest::Comment`] creates both a text-note and a popup
/// annotation, so `popup` is populated only for that case.
#[derive(Debug, Clone, Serialize)]
pub struct AddAnnotationResult {
    pub annotation: ObjectIdResult,
    pub popup: Option<ObjectIdResult>,
}

/// Adds one markup/comment annotation (ISO 32000-1 §12.5) to an open
/// document.
pub async fn add_annotation_impl(
    state: &AppState,
    request: AddAnnotationRequest,
) -> Result<AddAnnotationResult, CommandError> {
    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;

    state
        .pool
        .run(move || {
            let mut doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let result = match request.annotation {
                AnnotationRequest::Highlight {
                    page_index,
                    quads,
                    color,
                } => doc
                    .add_highlight_annotation(page_index, &quads, color.into())
                    .map(|id| (id, None)),
                AnnotationRequest::Underline {
                    page_index,
                    quads,
                    color,
                } => doc
                    .add_underline_annotation(page_index, &quads, color.into())
                    .map(|id| (id, None)),
                AnnotationRequest::StrikeOut {
                    page_index,
                    quads,
                    color,
                } => doc
                    .add_strikeout_annotation(page_index, &quads, color.into())
                    .map(|id| (id, None)),
                AnnotationRequest::FreeText {
                    page_index,
                    rect,
                    text,
                    font_size,
                    color,
                } => doc
                    .add_freetext_annotation(page_index, rect.into(), &text, font_size, color.into())
                    .map(|id| (id, None)),
                AnnotationRequest::Stamp {
                    page_index,
                    rect,
                    label,
                    color,
                } => doc
                    .add_stamp_annotation(page_index, rect.into(), &label, color.into())
                    .map(|id| (id, None)),
                AnnotationRequest::Ink {
                    page_index,
                    strokes,
                    color,
                    line_width,
                } => doc
                    .add_ink_annotation(page_index, &strokes, color.into(), line_width)
                    .map(|id| (id, None)),
                AnnotationRequest::Comment {
                    page_index,
                    at,
                    contents,
                    author,
                } => doc
                    .add_comment(page_index, at, &contents, author.as_deref())
                    .map(|(note_id, popup_id)| (note_id, Some(popup_id))),
            }
            .map_err(CommandError::from)?;

            Ok(AddAnnotationResult {
                annotation: result.0.into(),
                popup: result.1.map(Into::into),
            })
        })
        .await
}

/// Tauri command wrapper for [`add_annotation_impl`].
#[tauri::command]
pub async fn add_annotation(
    state: tauri::State<'_, AppState>,
    request: AddAnnotationRequest,
) -> Result<AddAnnotationResult, CommandError> {
    add_annotation_impl(&state, request).await
}

// ===================================================================
// sign_document
// ===================================================================

/// Arguments for [`sign_document`].
#[derive(Debug, Clone, Deserialize)]
pub struct SignDocumentRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    /// PEM-encoded signer certificate.
    pub certificate_pem: String,
    /// PEM-encoded additional certificates in the chain, root/issuer
    /// last.
    #[serde(default)]
    pub chain_pem: Vec<String>,
    /// PEM-encoded private key.
    pub private_key_pem: String,
    pub name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact_info: Option<String>,
    /// Output path for the signed copy. Signing always writes a new
    /// file (never overwrites the still-editable in-memory document),
    /// mirroring how [`crate::signatures::IncrementalSigner`] works from
    /// bytes rather than mutating an [`EditableDocument`] in place.
    pub output_path: String,
    /// How to serialize pending (unsigned) structural edits before
    /// signing; see [`SaveMode`].
    pub save_mode: SaveMode,
}

/// Result of [`sign_document`].
#[derive(Debug, Clone, Serialize)]
pub struct SignDocumentResult {
    /// Path the signed PDF was written to.
    pub path: String,
    /// Number of bytes written.
    pub bytes_written: usize,
}

/// Applies a detached PKCS#7/CMS digital signature (ISO 32000-1 §12.8) to
/// an open document, writing the signed result to `output_path`.
pub async fn sign_document_impl(
    state: &AppState,
    request: SignDocumentRequest,
    progress: ProgressReporter,
) -> Result<SignDocumentResult, CommandError> {
    validate_path_argument(&request.output_path)?;
    if request.certificate_pem.trim().is_empty() {
        return Err(CommandError::invalid_argument("certificate_pem must not be empty"));
    }
    if request.private_key_pem.trim().is_empty() {
        return Err(CommandError::invalid_argument("private_key_pem must not be empty"));
    }

    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;

    progress(ProgressEvent {
        operation: "sign_document",
        handle: Some(handle.0),
        current: 0,
        total: Some(1),
        message: Some("signing".to_string()),
    });

    let output_path = PathBuf::from(&request.output_path);
    let write_path = output_path.clone();
    let bytes_written = state
        .pool
        .run(move || sign_document_blocking(&entry, request, &write_path))
        .await?;

    progress(ProgressEvent {
        operation: "sign_document",
        handle: Some(handle.0),
        current: 1,
        total: Some(1),
        message: Some("done".to_string()),
    });

    Ok(SignDocumentResult {
        path: output_path.to_string_lossy().into_owned(),
        bytes_written,
    })
}

/// The actual (blocking, CPU-bound) signing work, run on
/// [`super::worker::WorkerPool`]. Split out from [`sign_document_impl`]
/// only so the `move ||` closure passed to `pool.run` stays readable.
fn sign_document_blocking(
    entry: &super::state::DocumentEntryHandle,
    request: SignDocumentRequest,
    output_path: &Path,
) -> Result<usize, CommandError> {
    use crate::signatures::{Certificate, IncrementalSigner, PrivateKey};

    let doc = entry
        .doc
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let bytes = match request.save_mode {
        SaveMode::Incremental => doc.save_incremental_to_bytes(),
        SaveMode::FullRewrite => doc.save_full_rewrite_to_bytes(),
    }
    .map_err(CommandError::from)?;
    drop(doc);

    let certificate = Certificate::from_pem(&request.certificate_pem).map_err(CommandError::from)?;
    let private_key = PrivateKey::from_pem(&request.private_key_pem).map_err(CommandError::from)?;

    let mut signer = IncrementalSigner::new(bytes)
        .certificate(certificate)
        .private_key(private_key);
    for chain_pem in &request.chain_pem {
        signer = signer.add_chain_certificate(Certificate::from_pem(chain_pem).map_err(CommandError::from)?);
    }
    if let Some(name) = request.name {
        signer = signer.name(name);
    }
    if let Some(reason) = request.reason {
        signer = signer.reason(reason);
    }
    if let Some(location) = request.location {
        signer = signer.location(location);
    }
    if let Some(contact_info) = request.contact_info {
        signer = signer.contact_info(contact_info);
    }

    let signed_bytes = signer.sign().map_err(CommandError::from)?;
    std::fs::write(output_path, &signed_bytes).map_err(CommandError::from)?;
    Ok(signed_bytes.len())
}

/// Tauri command wrapper for [`sign_document_impl`].
#[tauri::command]
pub async fn sign_document<R: tauri::Runtime>(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle<R>,
    request: SignDocumentRequest,
) -> Result<SignDocumentResult, CommandError> {
    let progress = emitting_progress_reporter(app);
    sign_document_impl(&state, request, progress).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::error::ErrorCode;
    use super::super::progress::no_progress;
    use crate::tauri_commands::state::test_support::sample_pdf_path;

    fn test_state() -> AppState {
        AppState::with_worker_threads(2)
    }

    async fn open_sample(state: &AppState) -> OpenDocumentResult {
        open_document_impl(
            state,
            OpenDocumentRequest {
                path: sample_pdf_path().to_string_lossy().into_owned(),
                password: None,
            },
        )
        .await
        .expect("opening the fixture PDF must succeed")
    }

    #[tokio::test]
    async fn open_document_rejects_missing_file() {
        let state = test_state();
        let result = open_document_impl(
            &state,
            OpenDocumentRequest {
                path: "/definitely/does/not/exist.pdf".to_string(),
                password: None,
            },
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn open_document_rejects_empty_path() {
        let state = test_state();
        let result = open_document_impl(
            &state,
            OpenDocumentRequest {
                path: String::new(),
                password: None,
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn open_document_rejects_a_directory() {
        let state = test_state();
        let dir = std::env::temp_dir();
        let result = open_document_impl(
            &state,
            OpenDocumentRequest {
                path: dir.to_string_lossy().into_owned(),
                password: None,
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn open_document_succeeds_on_valid_pdf() {
        let state = test_state();
        let opened = open_sample(&state).await;
        assert_eq!(opened.page_count, 1);
        assert!(opened.handle >= 1);
    }

    #[tokio::test]
    async fn render_page_end_to_end() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = render_page_impl(
            &state,
            RenderPageRequest {
                handle: opened.handle,
                page_index: 0,
                dpi: 72.0,
                viewport: None,
            },
        )
        .await;
        // No separate "is Pdfium available" probe: see
        // `render_actor`'s test module (`warm_up`) for why the
        // availability check has to be this exact call, not an
        // independent one -- `pdfium-render` only permits binding the
        // native library once per process, ever.
        let page = match result {
            Ok(page) => page,
            Err(err) if err.code == ErrorCode::RenderEngineUnavailable => {
                eprintln!("skipping render_page_end_to_end: Pdfium native library not available in this environment ({})", err.message);
                return;
            }
            Err(err) => panic!("rendering page 0 of the fixture PDF failed unexpectedly: {err:?}"),
        };
        assert!(page.width > 0 && page.height > 0);
    }

    #[tokio::test]
    async fn render_page_rejects_unknown_handle() {
        let state = test_state();
        let result = render_page_impl(
            &state,
            RenderPageRequest {
                handle: 999,
                page_index: 0,
                dpi: 72.0,
                viewport: None,
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn extract_text_finds_fixture_content() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let pages = extract_text_impl(
            &state,
            ExtractTextRequest {
                handle: opened.handle,
                page_index: None,
            },
            no_progress(),
        )
        .await
        .expect("extracting text must succeed");
        assert_eq!(pages.len(), 1);
        assert!(pages[0].text.contains("Hello"));
    }

    #[tokio::test]
    async fn extract_text_rejects_out_of_range_page() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = extract_text_impl(
            &state,
            ExtractTextRequest {
                handle: opened.handle,
                page_index: Some(5),
            },
            no_progress(),
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn search_text_finds_and_reports_progress() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&events);
        let reporter: ProgressReporter = std::sync::Arc::new(move |event| {
            sink.lock().unwrap().push(event);
        });

        let matches = search_text_impl(
            &state,
            SearchTextRequest {
                handle: opened.handle,
                query: "Hello".to_string(),
                case_sensitive: false,
            },
            reporter,
        )
        .await
        .expect("search must succeed");

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].page_index, 0);
        assert!(!events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_text_rejects_empty_query() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = search_text_impl(
            &state,
            SearchTextRequest {
                handle: opened.handle,
                query: String::new(),
                case_sensitive: false,
            },
            no_progress(),
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn apply_edit_rotate_then_extract_still_works() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = apply_edit_impl(
            &state,
            ApplyEditRequest {
                handle: opened.handle,
                operation: EditOperation::RotatePage {
                    page_index: 0,
                    degrees: 90,
                },
            },
        )
        .await
        .expect("rotate must succeed");
        assert_eq!(result.replacements, 0);
    }

    #[tokio::test]
    async fn apply_edit_replace_text_reports_count() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = apply_edit_impl(
            &state,
            ApplyEditRequest {
                handle: opened.handle,
                operation: EditOperation::ReplaceText {
                    page_index: 0,
                    find: "Hello".to_string(),
                    replace: "Goodbye".to_string(),
                },
            },
        )
        .await
        .expect("replace must succeed");
        assert_eq!(result.replacements, 1);
    }

    #[tokio::test]
    async fn apply_edit_rejects_out_of_range_page() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = apply_edit_impl(
            &state,
            ApplyEditRequest {
                handle: opened.handle,
                operation: EditOperation::DeletePage { page_index: 7 },
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn save_document_writes_a_valid_pdf() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let out_path = std::env::temp_dir().join(format!(
            "rust_pdf_tauri_commands_save_test_{}_{}.pdf",
            std::process::id(),
            opened.handle
        ));
        let result = save_document_impl(
            &state,
            SaveDocumentRequest {
                handle: opened.handle,
                path: Some(out_path.to_string_lossy().into_owned()),
                mode: SaveMode::FullRewrite,
            },
            no_progress(),
        )
        .await
        .expect("save must succeed");
        assert!(result.bytes_written > 0);
        let saved = std::fs::read(&out_path).expect("saved file must exist");
        assert!(saved.starts_with(b"%PDF-"));
        let _ = std::fs::remove_file(&out_path);
    }

    #[tokio::test]
    async fn fill_form_reports_missing_field_as_a_per_field_error() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let mut fields = std::collections::HashMap::new();
        fields.insert(
            "does_not_exist".to_string(),
            FormFieldValue::Text {
                value: "x".to_string(),
            },
        );
        let result = fill_form_impl(
            &state,
            FillFormRequest {
                handle: opened.handle,
                fields,
            },
        )
        .await
        .expect("fill_form itself must not error out");
        assert_eq!(result.updated, 0);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].1.code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn add_annotation_highlight_returns_object_id() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = add_annotation_impl(
            &state,
            AddAnnotationRequest {
                handle: opened.handle,
                annotation: AnnotationRequest::Highlight {
                    page_index: 0,
                    quads: vec![(72.0, 700.0, 300.0, 700.0), (72.0, 680.0, 300.0, 680.0)],
                    color: ColorRequest {
                        r: 1.0,
                        g: 1.0,
                        b: 0.0,
                    },
                },
            },
        )
        .await
        .expect("adding a highlight annotation must succeed");
        assert!(result.annotation.number > 0);
        assert!(result.popup.is_none());
    }

    #[tokio::test]
    async fn add_annotation_comment_returns_note_and_popup() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = add_annotation_impl(
            &state,
            AddAnnotationRequest {
                handle: opened.handle,
                annotation: AnnotationRequest::Comment {
                    page_index: 0,
                    at: (100.0, 100.0),
                    contents: "a note".to_string(),
                    author: Some("tester".to_string()),
                },
            },
        )
        .await
        .expect("adding a comment must succeed");
        assert!(result.popup.is_some());
    }

    #[tokio::test]
    async fn add_annotation_rejects_out_of_range_page() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = add_annotation_impl(
            &state,
            AddAnnotationRequest {
                handle: opened.handle,
                annotation: AnnotationRequest::Stamp {
                    page_index: 9,
                    rect: RectangleRequest {
                        llx: 0.0,
                        lly: 0.0,
                        urx: 10.0,
                        ury: 10.0,
                    },
                    label: "DRAFT".to_string(),
                    color: ColorRequest {
                        r: 1.0,
                        g: 0.0,
                        b: 0.0,
                    },
                },
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn sign_document_rejects_empty_certificate() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let out_path = std::env::temp_dir().join(format!(
            "rust_pdf_tauri_commands_sign_test_{}.pdf",
            std::process::id()
        ));
        let result = sign_document_impl(
            &state,
            SignDocumentRequest {
                handle: opened.handle,
                certificate_pem: String::new(),
                chain_pem: Vec::new(),
                private_key_pem: "irrelevant".to_string(),
                name: None,
                reason: None,
                location: None,
                contact_info: None,
                output_path: out_path.to_string_lossy().into_owned(),
                save_mode: SaveMode::Incremental,
            },
            no_progress(),
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn sign_document_rejects_malformed_certificate_pem() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let out_path = std::env::temp_dir().join(format!(
            "rust_pdf_tauri_commands_sign_test2_{}.pdf",
            std::process::id()
        ));
        let result = sign_document_impl(
            &state,
            SignDocumentRequest {
                handle: opened.handle,
                certificate_pem: "-----BEGIN CERTIFICATE-----\nnot valid\n-----END CERTIFICATE-----"
                    .to_string(),
                chain_pem: Vec::new(),
                private_key_pem: "-----BEGIN PRIVATE KEY-----\nnot valid\n-----END PRIVATE KEY-----"
                    .to_string(),
                name: None,
                reason: None,
                location: None,
                contact_info: None,
                output_path: out_path.to_string_lossy().into_owned(),
                save_mode: SaveMode::Incremental,
            },
            no_progress(),
        )
        .await;
        // Malformed PEM must surface as a structured `SignatureFailed`
        // error, not a panic.
        assert_eq!(result.unwrap_err().code, ErrorCode::SignatureFailed);
    }
}
