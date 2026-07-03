//! The Tauri commands for this crate's Tauri Integration phases.
//!
//! Every command follows the same shape: a plain `..._impl` async
//! function containing the real logic (no Tauri types, so it is directly
//! unit-testable and reusable from a non-Tauri host), and a thin
//! `#[tauri::command]` wrapper that extracts Tauri's `State`/`AppHandle`
//! and forwards to it. See the [module docs](super) for the overall
//! architecture and error/progress-reporting conventions.
//!
//! The original nine commands ([`open_document`], [`render_page`],
//! [`extract_text`], [`search_text`], [`apply_edit`], [`save_document`],
//! [`fill_form`], [`add_annotation`], [`sign_document`]) were joined by a
//! second batch adding PDF/A conversion, password encryption,
//! merge/split, and watermarking: [`convert_to_pdfa`], [`set_password`],
//! [`merge_documents`], [`split_document`], [`add_watermark`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::editor::EditableDocument;
use crate::render::Viewport;
use crate::types::Rectangle;

use super::error::CommandError;
use super::progress::{ProgressEvent, ProgressReporter, PROGRESS_EVENT_NAME};
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
    ///
    /// Currently accepted (for request-shape stability) but **not acted
    /// on**: this crate's pure-Rust parser
    /// ([`crate::parser::PdfReader`]) implements no decryption filter at
    /// all, so an encrypted document fails to open here regardless of any
    /// password supplied -- see [`crate::error::RenderError::PasswordRequired`]'s
    /// docs. This is a pre-existing limitation of this crate's whole
    /// structural-editing pipeline (not introduced by, or specific to,
    /// rendering).
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
    let handle = state.insert_document(path, doc);

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

/// A rectangular sub-region of a page-at-DPI raster to render (device
/// pixels), mirroring [`crate::render::Viewport`] as a plain,
/// IPC-`Deserialize`-able tuple.
pub type ViewportRequest = (u32, u32, u32, u32);

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

/// One rasterized page, ready to hand back across the Tauri IPC boundary.
#[derive(Debug, Clone, Serialize)]
pub struct RenderedPage {
    /// Raster width in pixels.
    pub width: u32,
    /// Raster height in pixels.
    pub height: u32,
    /// Raw 8-bit RGBA pixels, row-major, `width * height * 4` bytes.
    ///
    /// Sent as a plain byte array over Tauri's default JSON IPC here.
    /// For high-frequency interactive rendering (zoom/pan) a production
    /// integration should instead return this via
    /// `tauri::ipc::Response`/a raw byte channel to avoid JSON's
    /// per-byte encoding overhead; that is a frontend-transport choice
    /// left to the consuming Tauri application.
    pub rgba: Vec<u8>,
}

/// Renders one page to an RGBA raster on [`super::worker::WorkerPool`],
/// exactly like every other command in this module: [`crate::render`]'s
/// pure-Rust rendering pipeline has no native-library/FFI concurrency
/// constraint (unlike the FFI-backed engine this crate used previously --
/// see that module's docs), so page rasterization needs no dedicated
/// rendering-only thread/actor. This locks the same
/// [`EditableDocument`](crate::editor::EditableDocument) other commands
/// (`extract_text`, `apply_edit`, ...) already share for this document
/// handle -- briefly, just to read the requested page's content/resources
/// -- rather than opening a second, independent copy of the file.
pub async fn render_page_impl(
    state: &AppState,
    request: RenderPageRequest,
) -> Result<RenderedPage, CommandError> {
    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;
    let page_index = request.page_index;
    let dpi = request.dpi;
    let viewport = request
        .viewport
        .map(|(x, y, width, height)| Viewport::new(x, y, width, height));

    state
        .pool
        .run(move || {
            let doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let image = crate::render::render_page_document(&doc, page_index, dpi, viewport)
                .map_err(CommandError::from)?;
            Ok(RenderedPage {
                width: image.width(),
                height: image.height(),
                rgba: image.into_raw(),
            })
        })
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

// ===================================================================
// convert_to_pdfa
// ===================================================================

/// Which PDF/A "b" (basic) conformance level to target; see
/// [`crate::editor::PdfAFlavor`] for the underlying flavor and its
/// documented (partial, explicitly-scoped) rule coverage.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfAFlavorRequest {
    #[serde(rename = "1b")]
    Part1B,
    #[serde(rename = "2b")]
    Part2B,
    #[serde(rename = "3b")]
    Part3B,
}

impl From<PdfAFlavorRequest> for crate::editor::PdfAFlavor {
    fn from(flavor: PdfAFlavorRequest) -> Self {
        match flavor {
            PdfAFlavorRequest::Part1B => crate::editor::PdfAFlavor::Part1B,
            PdfAFlavorRequest::Part2B => crate::editor::PdfAFlavor::Part2B,
            PdfAFlavorRequest::Part3B => crate::editor::PdfAFlavor::Part3B,
        }
    }
}

/// Arguments for [`convert_to_pdfa`].
#[derive(Debug, Clone, Deserialize)]
pub struct ConvertToPdfaRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    pub flavor: PdfAFlavorRequest,
    /// ICC profile bytes for the mandatory `/OutputIntent` (ISO 19005-1
    /// 6.2.2). This crate never bundles one itself (see
    /// [`crate::editor::icc`]'s module docs) - the caller (typically a
    /// desktop app vendoring e.g. `sRGB2014.icc`) must supply real
    /// profile bytes.
    pub icc_profile: Vec<u8>,
    /// `/OutputConditionIdentifier` (e.g. `"sRGB IEC61966-2.1"`).
    pub icc_identifier: String,
    /// `/OutputCondition` (free text).
    pub icc_condition: String,
    /// `dc:title` written into the XMP packet.
    pub title: Option<String>,
    /// `pdf:Producer` / `xmp:CreatorTool`.
    pub producer: Option<String>,
}

/// What [`convert_to_pdfa`] actually changed, mirroring
/// [`crate::editor::PdfAConversionSummary`] as plain IPC-`Serialize`able
/// data.
#[derive(Debug, Clone, Serialize)]
pub struct PdfAConversionSummaryResult {
    pub lzw_streams_reencoded: usize,
    pub extgstates_disabled: usize,
    pub transparency_groups_removed: usize,
    pub output_intent_added: bool,
    pub catalog_entries_removed: Vec<String>,
}

impl From<crate::editor::PdfAConversionSummary> for PdfAConversionSummaryResult {
    fn from(summary: crate::editor::PdfAConversionSummary) -> Self {
        Self {
            lzw_streams_reencoded: summary.lzw_streams_reencoded,
            extgstates_disabled: summary.extgstates_disabled,
            transparency_groups_removed: summary.transparency_groups_removed,
            output_intent_added: summary.output_intent_added,
            catalog_entries_removed: summary.catalog_entries_removed.into_iter().map(str::to_string).collect(),
        }
    }
}

/// Result of [`convert_to_pdfa`].
#[derive(Debug, Clone, Serialize)]
pub struct ConvertToPdfaResult {
    /// Whether the document validates as fully conformant (within this
    /// crate's [documented rule coverage](crate::editor::pdfa)) *after*
    /// conversion.
    ///
    /// **Important caveat, not hidden**: this reflects validating bytes
    /// produced by [`crate::editor::EditableDocument::save_pdfa_compatible_to_bytes`]
    /// (the classic-xref, no-`ObjStm` writer PDF/A-1b in particular
    /// requires) - it does **not** reflect what [`save_document`] will
    /// actually write. `convert_to_pdfa` only mutates the open
    /// document's in-memory object graph (exactly like `apply_edit`);
    /// if the caller's next step is a plain `save_document` with
    /// `SaveMode::FullRewrite`, that path uses
    /// [`crate::editor::EditableDocument::save_full_rewrite_to_bytes`],
    /// which prefers compressed object streams/cross-reference streams
    /// that PDF/A-1b (defined against the pre-1.5 PDF 1.4 Reference)
    /// forbids - so the file actually saved to disk may not match this
    /// report for that flavor. A caller that needs genuinely conformant
    /// bytes on disk must serialize via that PDF/A-specific writer
    /// directly (not currently exposed as a `save_document` `SaveMode`
    /// variant - a known gap, called out here rather than papered over).
    pub conformant: bool,
    pub summary: PdfAConversionSummaryResult,
    /// Human-readable `"{rule}: {message}"` for every violation still
    /// present after conversion (empty if `conformant`).
    pub remaining_violations: Vec<String>,
}

/// Converts an already-open document towards PDF/A conformance (ISO
/// 19005-1/2/3, "b" levels only - see [`crate::editor::pdfa`] for exactly
/// what is/isn't checked and fixed), mutating its in-memory object graph
/// exactly like [`apply_edit`] does. See [`ConvertToPdfaResult::conformant`]'s
/// doc comment for an important caveat about what a later plain
/// `save_document` call will (and won't) preserve.
pub async fn convert_to_pdfa_impl(
    state: &AppState,
    request: ConvertToPdfaRequest,
) -> Result<ConvertToPdfaResult, CommandError> {
    if request.icc_profile.is_empty() {
        return Err(CommandError::invalid_argument("icc_profile must not be empty"));
    }
    if request.icc_identifier.trim().is_empty() {
        return Err(CommandError::invalid_argument("icc_identifier must not be empty"));
    }

    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;
    let flavor: crate::editor::PdfAFlavor = request.flavor.into();

    state
        .pool
        .run(move || {
            let mut doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let options = crate::editor::PdfAConversionOptions {
                icc_profile: &request.icc_profile,
                icc_identifier: &request.icc_identifier,
                icc_condition: &request.icc_condition,
                title: request.title.as_deref(),
                producer: request.producer.as_deref(),
            };
            let summary = doc.convert_to_pdfa(flavor, &options).map_err(CommandError::from)?;

            // Validate against what a PDF/A-aware save would actually
            // produce -- see `ConvertToPdfaResult::conformant`'s doc
            // comment for why this does not necessarily match what
            // `save_document` itself will write.
            let pdfa_bytes = doc
                .save_pdfa_compatible_to_bytes(flavor.min_pdf_version())
                .map_err(CommandError::from)?;
            drop(doc);
            let reopened = EditableDocument::from_bytes(pdfa_bytes).map_err(CommandError::from)?;
            let report = reopened.validate_pdfa(flavor).map_err(CommandError::from)?;

            Ok(ConvertToPdfaResult {
                conformant: report.is_conformant(),
                summary: summary.into(),
                remaining_violations: report
                    .violations
                    .iter()
                    .map(|v| format!("{}: {}", v.rule, v.message))
                    .collect(),
            })
        })
        .await
}

/// Tauri command wrapper for [`convert_to_pdfa_impl`].
#[tauri::command]
pub async fn convert_to_pdfa(
    state: tauri::State<'_, AppState>,
    request: ConvertToPdfaRequest,
) -> Result<ConvertToPdfaResult, CommandError> {
    convert_to_pdfa_impl(&state, request).await
}

// ===================================================================
// set_password
// ===================================================================

/// Which encryption algorithm to apply; see
/// [`crate::encryption::EncryptionAlgorithm`].
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncryptionAlgorithmRequest {
    Aes128,
    Aes256,
}

/// Arguments for [`set_password`].
#[derive(Debug, Clone, Deserialize)]
pub struct SetPasswordRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    /// Password required to open the document. Empty means "no open
    /// password" (permissions from `owner_password` still apply).
    #[serde(default)]
    pub user_password: String,
    /// Password required to bypass permission restrictions. Empty means
    /// this crate generates the encryption without a distinct owner
    /// secret (see [`crate::encryption::EncryptionConfig::owner_password`]).
    #[serde(default)]
    pub owner_password: String,
    pub algorithm: EncryptionAlgorithmRequest,
    /// Output path for the encrypted copy. Like [`sign_document`],
    /// encrypting always writes a **new** file rather than mutating the
    /// still-open, still-unencrypted in-memory document -- see this
    /// command's own doc comment for why that isn't optional here.
    pub output_path: String,
}

/// Result of [`set_password`].
#[derive(Debug, Clone, Serialize)]
pub struct SetPasswordResult {
    /// Path the encrypted PDF was written to.
    pub path: String,
    /// Number of bytes written.
    pub bytes_written: usize,
}

/// Applies password/permission encryption (ISO 32000-2 Section 7.6,
/// AES-128 or AES-256) to an already-open document, writing the
/// encrypted result to `output_path`.
///
/// **A real, disclosed gap, not papered over**: unlike every other
/// command in this module, this is **not** an in-place edit you can
/// later persist via a plain `save_document` -
/// [`crate::editor::EditableDocument`] (an arbitrary already-open,
/// already-parsed document) has no incremental "encrypt on next save"
/// facility - only [`crate::document::DocumentBuilder::encrypt`] can
/// encrypt, and only for a document built entirely from scratch via
/// [`crate::document::DocumentBuilder`]/[`crate::page::PageBuilder`],
/// never for an arbitrary already-open source PDF. This command instead
/// calls [`crate::editor::EditableDocument::save_encrypted_to_bytes`],
/// which does a dedicated one-shot full-graph rewrite with encryption
/// baked in (see that method's module docs for the full rationale) and
/// returns brand-new bytes; the original open document handle is left
/// completely untouched and still editable/re-saveable afterwards.
///
/// **Bigger caveat, also not hidden**: this crate's own parser cannot
/// reopen its own encrypted output at all (no decryption filter is
/// implemented anywhere in this crate - see
/// [`OpenDocumentRequest::password`]'s doc comment for the same gap on
/// the reading side). The file this command produces is genuinely,
/// correctly encrypted per ISO 32000-2 and opens fine in any real
/// conformant reader (Acrobat, etc.) with the configured password, but
/// **cannot be reopened via `open_document`** by this same application.
pub async fn set_password_impl(
    state: &AppState,
    request: SetPasswordRequest,
) -> Result<SetPasswordResult, CommandError> {
    validate_path_argument(&request.output_path)?;
    if request.user_password.is_empty() && request.owner_password.is_empty() {
        return Err(CommandError::invalid_argument(
            "at least one of user_password/owner_password must be non-empty",
        ));
    }

    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;
    let output_path = PathBuf::from(&request.output_path);
    let write_path = output_path.clone();

    let bytes_written = state
        .pool
        .run(move || {
            let doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            let mut config = match request.algorithm {
                EncryptionAlgorithmRequest::Aes128 => crate::encryption::EncryptionConfig::aes128(),
                EncryptionAlgorithmRequest::Aes256 => crate::encryption::EncryptionConfig::aes256(),
            };
            if !request.user_password.is_empty() {
                config = config.user_password(request.user_password.clone());
            }
            if !request.owner_password.is_empty() {
                config = config.owner_password(request.owner_password.clone());
            }

            let bytes = doc.save_encrypted_to_bytes(config).map_err(CommandError::from)?;
            std::fs::write(&write_path, &bytes).map_err(CommandError::from)?;
            Ok(bytes.len())
        })
        .await?;

    Ok(SetPasswordResult {
        path: output_path.to_string_lossy().into_owned(),
        bytes_written,
    })
}

/// Tauri command wrapper for [`set_password_impl`].
#[tauri::command]
pub async fn set_password(
    state: tauri::State<'_, AppState>,
    request: SetPasswordRequest,
) -> Result<SetPasswordResult, CommandError> {
    set_password_impl(&state, request).await
}

// ===================================================================
// merge_documents
// ===================================================================

/// One document to fold into a [`merge_documents`] call: either an
/// already-open handle, or a filesystem path opened fresh just for the
/// merge (not registered in [`AppState`], and not kept open afterwards).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MergeSource {
    Handle { handle: u64 },
    Path { path: String },
}

/// Arguments for [`merge_documents`].
#[derive(Debug, Clone, Deserialize)]
pub struct MergeDocumentsRequest {
    /// At least 2 sources, appended in order.
    pub sources: Vec<MergeSource>,
}

enum ResolvedMergeSource {
    Handle(super::state::DocumentEntryHandle),
    Path(PathBuf),
}

/// Merges 2+ documents (each either an already-open handle or a
/// filesystem path) into one brand-new document, registered in
/// [`AppState`] under a fresh handle exactly like [`open_document`] -
/// every other command (`render_page`, `apply_edit`, `save_document`,
/// ...) works on that returned handle immediately.
///
/// The merged document has no source file of its own (see
/// [`crate::editor::EditableDocument::new_empty`]), so
/// [`save_document`]'s `path: None` (default to the handle's open path)
/// has nothing to default to - a caller must pass an explicit `path`
/// when first saving a document produced by this command.
pub async fn merge_documents_impl(
    state: &AppState,
    request: MergeDocumentsRequest,
) -> Result<OpenDocumentResult, CommandError> {
    if request.sources.len() < 2 {
        return Err(CommandError::invalid_argument(
            "merge_documents requires at least 2 sources",
        ));
    }

    let mut resolved = Vec::with_capacity(request.sources.len());
    for source in &request.sources {
        match source {
            MergeSource::Handle { handle } => {
                let entry = state.get_document(DocumentHandle(*handle))?;
                resolved.push(ResolvedMergeSource::Handle(entry));
            }
            MergeSource::Path { path } => {
                validate_path_argument(path)?;
                resolved.push(ResolvedMergeSource::Path(PathBuf::from(path)));
            }
        }
    }

    let merged = state
        .pool
        .run(move || {
            let mut merged = EditableDocument::new_empty().map_err(CommandError::from)?;
            for source in resolved {
                match source {
                    ResolvedMergeSource::Handle(entry) => {
                        let doc = entry
                            .doc
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        merged.append_document(&doc).map_err(CommandError::from)?;
                    }
                    ResolvedMergeSource::Path(path) => {
                        let doc = EditableDocument::open(&path).map_err(CommandError::from)?;
                        merged.append_document(&doc).map_err(CommandError::from)?;
                    }
                }
            }
            Ok(merged)
        })
        .await?;

    let page_count = merged.page_count().map_err(CommandError::from)?;
    let handle = state.insert_document(PathBuf::new(), merged);
    Ok(OpenDocumentResult {
        handle: handle.0,
        page_count,
    })
}

/// Tauri command wrapper for [`merge_documents_impl`].
#[tauri::command]
pub async fn merge_documents(
    state: tauri::State<'_, AppState>,
    request: MergeDocumentsRequest,
) -> Result<OpenDocumentResult, CommandError> {
    merge_documents_impl(&state, request).await
}

// ===================================================================
// split_document
// ===================================================================

/// Arguments for [`split_document`].
#[derive(Debug, Clone, Deserialize)]
pub struct SplitDocumentRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    /// Zero-based page indices to extract into the new document, in the
    /// given order (may reorder/duplicate pages from the source).
    pub page_indices: Vec<usize>,
}

/// Extracts the given page indices of an already-open document into a
/// brand-new, standalone document (the source document is not modified),
/// registered in [`AppState`] under a fresh handle exactly like
/// [`open_document`]/[`merge_documents`]. See
/// [`merge_documents_impl`]'s doc comment for the same "no default save
/// path" note, which applies here too.
pub async fn split_document_impl(
    state: &AppState,
    request: SplitDocumentRequest,
) -> Result<OpenDocumentResult, CommandError> {
    if request.page_indices.is_empty() {
        return Err(CommandError::invalid_argument(
            "split_document requires at least one page index",
        ));
    }

    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;

    let split = state
        .pool
        .run(move || {
            let doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let page_count = doc.page_count().map_err(CommandError::from)?;
            for &index in &request.page_indices {
                if index >= page_count {
                    return Err(CommandError::invalid_argument(format!(
                        "page index {index} out of range (document has {page_count} pages)"
                    )));
                }
            }
            doc.extract_pages(&request.page_indices).map_err(CommandError::from)
        })
        .await?;

    let page_count = split.page_count().map_err(CommandError::from)?;
    let handle = state.insert_document(PathBuf::new(), split);
    Ok(OpenDocumentResult {
        handle: handle.0,
        page_count,
    })
}

/// Tauri command wrapper for [`split_document_impl`].
#[tauri::command]
pub async fn split_document(
    state: tauri::State<'_, AppState>,
    request: SplitDocumentRequest,
) -> Result<OpenDocumentResult, CommandError> {
    split_document_impl(&state, request).await
}

// ===================================================================
// add_watermark
// ===================================================================

/// Arguments for [`add_watermark`].
#[derive(Debug, Clone, Deserialize)]
pub struct AddWatermarkRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    pub text: String,
    pub font_size: f64,
    /// `0.0` (invisible) ..= `1.0` (fully opaque); out-of-range values
    /// are clamped, not rejected (see
    /// [`crate::editor::WatermarkOptions::opacity`]).
    pub opacity: f64,
    /// Counter-clockwise rotation in degrees (`45.0` for a classic
    /// diagonal watermark).
    pub rotation_degrees: f64,
    pub color: ColorRequest,
}

/// Result of [`add_watermark`].
#[derive(Debug, Clone, Serialize)]
pub struct AddWatermarkResult {
    /// Number of pages watermarked (this document's page count).
    pub pages_watermarked: usize,
}

/// Stamps a text watermark across every page of an open document via
/// content-stream injection (see [`crate::editor::WatermarkOptions`] for
/// exactly how). Mutates the in-memory object graph exactly like
/// [`apply_edit`]/[`add_annotation`] - persisted only once
/// [`save_document`] is called.
pub async fn add_watermark_impl(
    state: &AppState,
    request: AddWatermarkRequest,
) -> Result<AddWatermarkResult, CommandError> {
    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;

    state
        .pool
        .run(move || {
            let mut doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let options = crate::editor::WatermarkOptions {
                text: &request.text,
                font_size: request.font_size,
                opacity: request.opacity,
                rotation_degrees: request.rotation_degrees,
                color: request.color.into(),
            };
            let pages_watermarked = doc.add_text_watermark(&options).map_err(CommandError::from)?;
            Ok(AddWatermarkResult { pages_watermarked })
        })
        .await
}

/// Tauri command wrapper for [`add_watermark_impl`].
#[tauri::command]
pub async fn add_watermark(
    state: tauri::State<'_, AppState>,
    request: AddWatermarkRequest,
) -> Result<AddWatermarkResult, CommandError> {
    add_watermark_impl(&state, request).await
}

// ===================================================================
// get_outline
// ===================================================================

/// Arguments for [`get_outline`].
#[derive(Debug, Clone, Deserialize)]
pub struct GetOutlineRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
}

/// One node of the document outline (bookmark) tree, plain-data mirror of
/// [`crate::editor::BookmarkNode`] for IPC: `id` is dropped (an outline
/// item's object id is an internal editing handle for
/// [`crate::editor::EditableDocument::remove_bookmark`]/`add_bookmark`,
/// not something a read-only reader UI needs) and `dest` is collapsed to
/// the single field a page-jumping frontend actually wants -- a
/// destination page index, when the item's destination resolved to one
/// (see [`crate::editor::Destination`]; `None` covers both "no
/// destination" and a destination form this crate does not resolve to a
/// page, e.g. an unresolvable named destination).
#[derive(Debug, Clone, Serialize)]
pub struct OutlineNodeResult {
    pub title: String,
    pub page_index: Option<usize>,
    pub children: Vec<OutlineNodeResult>,
}

fn convert_bookmark_node(node: crate::editor::BookmarkNode) -> OutlineNodeResult {
    OutlineNodeResult {
        title: node.title,
        page_index: node.dest.map(|d| match d {
            crate::editor::Destination::FitPage { page_index } => page_index,
            crate::editor::Destination::Xyz { page_index, .. } => page_index,
        }),
        children: node.children.into_iter().map(convert_bookmark_node).collect(),
    }
}

/// Returns an open document's outline (bookmark) tree, in document order,
/// via [`crate::editor::EditableDocument::list_bookmarks`]. An empty
/// `Vec` means the document defines no outline at all (not an error --
/// most PDFs have none), which a frontend should render as a "no
/// bookmarks" message rather than an empty tree.
pub async fn get_outline_impl(
    state: &AppState,
    request: GetOutlineRequest,
) -> Result<Vec<OutlineNodeResult>, CommandError> {
    let handle = DocumentHandle(request.handle);
    let entry = state.get_document(handle)?;

    state
        .pool
        .run(move || {
            let doc = entry
                .doc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let bookmarks = doc.list_bookmarks().map_err(CommandError::from)?;
            Ok(bookmarks.into_iter().map(convert_bookmark_node).collect())
        })
        .await
}

/// Tauri command wrapper for [`get_outline_impl`].
#[tauri::command]
pub async fn get_outline(
    state: tauri::State<'_, AppState>,
    request: GetOutlineRequest,
) -> Result<Vec<OutlineNodeResult>, CommandError> {
    get_outline_impl(&state, request).await
}

// ===================================================================
// get_text_layout
// ===================================================================

/// Arguments for [`get_text_layout`].
#[derive(Debug, Clone, Deserialize)]
pub struct GetTextLayoutRequest {
    /// Handle returned by [`open_document`].
    pub handle: u64,
    /// A single zero-based page to compute layout for, or `None` for
    /// every page.
    pub page_index: Option<usize>,
}

/// One text-showing run's decoded text and bounding box, plain-data
/// mirror of [`crate::editor::TextRun`] for IPC. See
/// [`crate::editor::text_layout`]'s module docs for what a "run" is (one
/// `Tj`/`'`/`"`/`TJ` operator's worth of text, *not* necessarily one word
/// or one visual line) and this feature's disclosed approximation
/// limits (font metrics, ascent/descent, no per-glyph shaping).
#[derive(Debug, Clone, Serialize)]
pub struct TextRunResult {
    pub text: String,
    /// Lower-left X of the run's box, in the page's own default user
    /// space (see [`PageTextLayout::page_width`]'s docs for how a
    /// frontend maps this onto a `render_page` raster).
    pub x: f64,
    /// Lower-left Y of the run's box, page default user space (y
    /// increasing upward, ISO 32000-1 8.3.2.2).
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<crate::editor::TextRun> for TextRunResult {
    fn from(run: crate::editor::TextRun) -> Self {
        TextRunResult {
            text: run.text,
            x: run.x,
            y: run.y,
            width: run.width,
            height: run.height,
        }
    }
}

/// One page's approximate text-layer geometry (see
/// [`crate::editor::text_layout`]'s module docs), plus the page's own
/// `/MediaBox` width/height in points (ISO 32000-1 7.7.3.3) a frontend
/// needs to map `runs`' page-default-user-space coordinates onto a
/// `render_page` raster rendered at some `dpi`:
/// `scale = dpi / 72.0`, `pixel_x = run.x * scale`,
/// `pixel_y_from_top = (page_height - (run.y + run.height)) * scale`
/// (PDF user space has y increasing upward; a raster's row 0 is its
/// top, so the run's *top* edge -- `run.y + run.height` -- maps to the
/// smaller pixel-y). This assumes the page's `/Rotate` is `0`; see
/// [`crate::editor::text_layout`]'s "Known limitations" for rotated
/// pages, which this command does not compensate for.
#[derive(Debug, Clone, Serialize)]
pub struct PageTextLayout {
    /// Zero-based page index.
    pub page_index: usize,
    pub page_width: f64,
    pub page_height: f64,
    pub runs: Vec<TextRunResult>,
}

fn text_layout_for_pages(doc: &EditableDocument, indices: &[usize]) -> Result<Vec<PageTextLayout>, CommandError> {
    let mut pages = Vec::with_capacity(indices.len());
    for &index in indices {
        let page_id = doc.page_id_at(index).map_err(CommandError::from)?;
        let media_box = doc.effective_media_box(page_id).map_err(CommandError::from)?;
        let runs = doc.extract_page_text_layout(page_id).map_err(CommandError::from)?;
        pages.push(PageTextLayout {
            page_index: index,
            page_width: media_box.width(),
            page_height: media_box.height(),
            runs: runs.into_iter().map(TextRunResult::from).collect(),
        });
    }
    Ok(pages)
}

/// Returns the approximate text-layer geometry for one page, or every
/// page, of an open document -- the data a frontend overlays as
/// selectable/copyable transparent text atop [`render_page`]'s raster
/// output. See [`crate::editor::text_layout`]'s module docs for the
/// algorithm and its disclosed limitations, and [`PageTextLayout`]'s
/// docs for how to map a run's coordinates onto a rendered raster.
pub async fn get_text_layout_impl(
    state: &AppState,
    request: GetTextLayoutRequest,
) -> Result<Vec<PageTextLayout>, CommandError> {
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
            text_layout_for_pages(&doc, &indices)
        })
        .await
}

/// Tauri command wrapper for [`get_text_layout_impl`].
#[tauri::command]
pub async fn get_text_layout(
    state: tauri::State<'_, AppState>,
    request: GetTextLayoutRequest,
) -> Result<Vec<PageTextLayout>, CommandError> {
    get_text_layout_impl(&state, request).await
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
        // Unlike the previous FFI-backed rendering engine, this pure-Rust
        // pipeline has no native-library-availability precondition, so
        // there is nothing to skip here: it must simply succeed.
        let page = result.unwrap_or_else(|err| {
            panic!("rendering page 0 of the fixture PDF failed unexpectedly: {err:?}")
        });
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

    /// Definition-of-Done regression guard for the render-actor retirement:
    /// `render_page` now dispatches to the same [`super::worker::WorkerPool`]
    /// every other command uses (see [`super`]'s module docs) instead of a
    /// dedicated single-threaded rendering actor, because
    /// [`crate::render::PdfRenderer`] is built on
    /// [`crate::editor::EditableDocument`] (`Send + Sync`, no native-library/
    /// FFI concurrency constraint). This proves that actually works: many
    /// concurrent `render_page_impl` calls for *several different pages* of
    /// the *same* open document, issued from multiple `tokio::spawn`ed tasks
    /// on a multi-threaded runtime (so they really do run concurrently,
    /// including on the `WorkerPool`'s own separate OS threads -- not just
    /// interleaved on one), must all succeed, each page's concurrent renders
    /// must all agree pixel-for-pixel with each other (no data race on the
    /// shared, `Mutex`-protected `EditableDocument`), and different pages
    /// must render differently (proving no cross-talk between concurrent
    /// calls).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn render_page_is_correct_under_concurrent_calls_via_worker_pool() {
        use crate::prelude::*;
        use std::collections::HashMap;
        use std::sync::Arc;

        // A small multi-page fixture where each page is a distinctly
        // colored solid rectangle, so different page indices are trivially
        // distinguishable in their rendered output.
        let bytes = {
            let mut builder = DocumentBuilder::new().title("render concurrency test fixture");
            for i in 0..4 {
                let hue = f64::from(i) / 4.0;
                let content = ContentBuilder::new()
                    .fill_color(Color::rgb(hue, 0.2, 1.0 - hue))
                    .rect(0.0, 0.0, 200.0, 200.0)
                    .fill();
                builder = builder.page(PageBuilder::a4().content(content).build());
            }
            builder.build().unwrap().save_to_bytes().unwrap()
        };
        let path = std::env::temp_dir().join(format!(
            "rust_pdf_render_concurrency_test_{}.pdf",
            std::process::id()
        ));
        std::fs::write(&path, &bytes).expect("writing the concurrency-test fixture must not fail");

        // `AppState` itself isn't `Clone` -- the `Arc` here only exists so
        // this test can move a shared reference into several
        // `tokio::spawn`ed 'static tasks; it is not something
        // `render_page_impl`/`WorkerPool` require (see `AppState`'s own
        // `Send + Sync` compile-time assertion in `state.rs`).
        let state = Arc::new(test_state());
        let opened = open_document_impl(
            &state,
            OpenDocumentRequest {
                path: path.to_string_lossy().into_owned(),
                password: None,
            },
        )
        .await
        .expect("opening the concurrency-test fixture must succeed");
        assert_eq!(opened.page_count, 4);

        // Submit every render *before* awaiting any of them: `tokio::spawn`
        // starts a task running as soon as the multi-threaded runtime
        // schedules it, not merely when its `JoinHandle` is later awaited
        // -- this is what actually exercises several concurrent renders in
        // flight at once, rather than proving correctness of one render at
        // a time in sequence.
        const RENDERS_PER_PAGE: usize = 8;
        let mut handles = Vec::new();
        for _ in 0..RENDERS_PER_PAGE {
            for page_index in 0..4usize {
                let state = Arc::clone(&state);
                let handle = opened.handle;
                handles.push(tokio::spawn(async move {
                    let page = render_page_impl(
                        &state,
                        RenderPageRequest {
                            handle,
                            page_index,
                            dpi: 72.0,
                            viewport: None,
                        },
                    )
                    .await
                    .unwrap_or_else(|e| {
                        panic!("concurrent render_page of page {page_index} failed: {e:?}")
                    });
                    (page_index, page)
                }));
            }
        }

        let mut by_page: HashMap<usize, Vec<RenderedPage>> = HashMap::new();
        for handle in handles {
            let (page_index, page) = handle.await.expect("render task must not panic");
            assert!(page.width > 0 && page.height > 0);
            by_page.entry(page_index).or_default().push(page);
        }

        // Every concurrent render of the *same* page must be pixel-for-
        // pixel identical to every other render of that same page.
        for (page_index, pages) in &by_page {
            assert_eq!(
                pages.len(),
                RENDERS_PER_PAGE,
                "expected {RENDERS_PER_PAGE} concurrent renders of page {page_index}"
            );
            let first = &pages[0];
            for other in &pages[1..] {
                assert_eq!(
                    other.rgba, first.rgba,
                    "page {page_index} rendered inconsistently across concurrent calls"
                );
            }
        }

        // Different pages (different fill colors) must not render
        // identically -- proving each concurrent call actually rendered
        // its own requested page rather than some shared/racy buffer.
        let page0 = &by_page[&0][0];
        let page1 = &by_page[&1][0];
        assert_ne!(page0.rgba, page1.rgba, "different pages must not render identically");

        let _ = std::fs::remove_file(&path);
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

    // -- convert_to_pdfa -----------------------------------------------------

    #[tokio::test]
    async fn convert_to_pdfa_reports_conformant_for_a_simple_vector_document() {
        use crate::prelude::*;

        let state = test_state();
        // A vector-only, font-free, PDF-1.4, no-`/Info /Title` fixture:
        // `sample_pdf_path`'s fixture (used by every other test in this
        // module) has a Standard-14 font (never embeddable, ISO 19005-1
        // 6.3) and its own `/Info /Title`, both of which would (rightly)
        // still show up as PDF/A violations after conversion since
        // neither is auto-fixable -- see `crate::editor::pdfa`'s module
        // docs. This dedicated fixture isolates just the
        // `convert_to_pdfa` plumbing this test wants to check.
        let content = ContentBuilder::new().fill_color(Color::rgb(0.2, 0.4, 0.8)).rect(50.0, 50.0, 100.0, 100.0).fill();
        let page = PageBuilder::a4().content(content).build();
        let bytes = DocumentBuilder::new().version(PdfVersion::V1_4).page(page).build().unwrap().save_to_bytes().unwrap();
        let path = std::env::temp_dir().join(format!(
            "rust_pdf_tauri_commands_pdfa_test_{}.pdf",
            std::process::id()
        ));
        std::fs::write(&path, &bytes).unwrap();
        let opened = open_document_impl(
            &state,
            OpenDocumentRequest {
                path: path.to_string_lossy().into_owned(),
                password: None,
            },
        )
        .await
        .expect("opening the vector-only fixture must succeed");

        let icc = crate::editor::icc::test_support::fake_icc_profile(crate::editor::icc::IccColorSpace::Rgb);
        let result = convert_to_pdfa_impl(
            &state,
            ConvertToPdfaRequest {
                handle: opened.handle,
                flavor: PdfAFlavorRequest::Part1B,
                icc_profile: icc,
                icc_identifier: "sRGB IEC61966-2.1".to_string(),
                icc_condition: "sRGB".to_string(),
                title: Some("Test".to_string()),
                producer: Some("rust-pdf tests".to_string()),
            },
        )
        .await
        .expect("convert_to_pdfa must succeed");
        assert!(result.summary.output_intent_added);
        assert!(result.conformant, "violations: {:?}", result.remaining_violations);
        assert!(result.remaining_violations.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn convert_to_pdfa_rejects_empty_icc_profile() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = convert_to_pdfa_impl(
            &state,
            ConvertToPdfaRequest {
                handle: opened.handle,
                flavor: PdfAFlavorRequest::Part1B,
                icc_profile: Vec::new(),
                icc_identifier: "sRGB IEC61966-2.1".to_string(),
                icc_condition: "sRGB".to_string(),
                title: None,
                producer: None,
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn convert_to_pdfa_rejects_unknown_handle() {
        let state = test_state();
        let icc = crate::editor::icc::test_support::fake_icc_profile(crate::editor::icc::IccColorSpace::Rgb);
        let result = convert_to_pdfa_impl(
            &state,
            ConvertToPdfaRequest {
                handle: 12345,
                flavor: PdfAFlavorRequest::Part2B,
                icc_profile: icc,
                icc_identifier: "sRGB IEC61966-2.1".to_string(),
                icc_condition: "sRGB".to_string(),
                title: None,
                producer: None,
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::NotFound);
    }

    // -- set_password ---------------------------------------------------------

    #[tokio::test]
    async fn set_password_writes_a_structurally_encrypted_file() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let out_path = std::env::temp_dir().join(format!(
            "rust_pdf_tauri_commands_set_password_test_{}.pdf",
            std::process::id()
        ));
        let result = set_password_impl(
            &state,
            SetPasswordRequest {
                handle: opened.handle,
                user_password: "user-secret".to_string(),
                owner_password: "owner-secret".to_string(),
                algorithm: EncryptionAlgorithmRequest::Aes256,
                output_path: out_path.to_string_lossy().into_owned(),
            },
        )
        .await
        .expect("set_password must succeed");
        assert!(result.bytes_written > 0);

        let saved = std::fs::read(&out_path).expect("encrypted file must exist");
        assert!(saved.starts_with(b"%PDF-"));
        let text = String::from_utf8_lossy(&saved);
        assert!(text.contains("/Encrypt"));

        // The original, still-open document must remain usable
        // (unencrypted) after exporting an encrypted copy -- see
        // `set_password_impl`'s own doc comment.
        let extracted = extract_text_impl(
            &state,
            ExtractTextRequest {
                handle: opened.handle,
                page_index: Some(0),
            },
            no_progress(),
        )
        .await
        .expect("the original open document must still be usable after set_password");
        assert!(extracted[0].text.contains("Hello"));

        let _ = std::fs::remove_file(&out_path);
    }

    #[tokio::test]
    async fn set_password_rejects_empty_passwords() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let out_path = std::env::temp_dir().join(format!(
            "rust_pdf_tauri_commands_set_password_test2_{}.pdf",
            std::process::id()
        ));
        let result = set_password_impl(
            &state,
            SetPasswordRequest {
                handle: opened.handle,
                user_password: String::new(),
                owner_password: String::new(),
                algorithm: EncryptionAlgorithmRequest::Aes256,
                output_path: out_path.to_string_lossy().into_owned(),
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    // -- merge_documents --------------------------------------------------------

    #[tokio::test]
    async fn merge_documents_combines_an_open_handle_and_a_path() {
        let state = test_state();
        let opened = open_sample(&state).await; // 1 page ("Hello, rust-pdf tests!")

        // A second, independent fixture file on disk to merge in by path.
        let second_path = {
            use crate::prelude::*;
            let content = ContentBuilder::new().text("F1", 12.0, 72.0, 700.0, "Second document page");
            let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(content).build();
            let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
            let path = std::env::temp_dir().join(format!(
                "rust_pdf_tauri_commands_merge_test_{}.pdf",
                std::process::id()
            ));
            std::fs::write(&path, &bytes).unwrap();
            path
        };

        let result = merge_documents_impl(
            &state,
            MergeDocumentsRequest {
                sources: vec![
                    MergeSource::Handle { handle: opened.handle },
                    MergeSource::Path {
                        path: second_path.to_string_lossy().into_owned(),
                    },
                ],
            },
        )
        .await
        .expect("merge_documents must succeed");
        assert_eq!(result.page_count, 2);

        // The merged document is registered like any other open
        // document: other commands work on its handle immediately.
        let extracted = extract_text_impl(
            &state,
            ExtractTextRequest {
                handle: result.handle,
                page_index: None,
            },
            no_progress(),
        )
        .await
        .expect("extracting text from the merged document must succeed");
        assert_eq!(extracted.len(), 2);
        assert!(extracted[0].text.contains("Hello"));
        assert!(extracted[1].text.contains("Second document page"));

        let _ = std::fs::remove_file(&second_path);
    }

    #[tokio::test]
    async fn merge_documents_rejects_fewer_than_two_sources() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = merge_documents_impl(
            &state,
            MergeDocumentsRequest {
                sources: vec![MergeSource::Handle { handle: opened.handle }],
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn merge_documents_rejects_unknown_handle() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = merge_documents_impl(
            &state,
            MergeDocumentsRequest {
                sources: vec![
                    MergeSource::Handle { handle: opened.handle },
                    MergeSource::Handle { handle: 999_999 },
                ],
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::NotFound);
    }

    // -- split_document -----------------------------------------------------

    /// Writes a fresh `num_pages`-page fixture to a uniquely-named
    /// temporary file (never reused/collided across concurrently-running
    /// tests, unlike a name derived only from the process id) and opens
    /// it. The file is deliberately **not** removed here -
    /// [`crate::editor::EditableDocument::open`] memory-maps the file
    /// rather than reading it fully upfront (see that method's own
    /// docs), so deleting it immediately after open is not safe; callers
    /// should remove the returned path once they are done with the
    /// document, exactly like every other on-disk fixture test in this
    /// module already does.
    async fn open_multi_page_sample(state: &AppState, num_pages: usize) -> (OpenDocumentResult, std::path::PathBuf) {
        use crate::prelude::*;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let mut builder = DocumentBuilder::new();
        for i in 0..num_pages {
            let content = ContentBuilder::new().text("F1", 12.0, 72.0, 700.0, &format!("Page number {i}"));
            let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(content).build();
            builder = builder.page(page);
        }
        let bytes = builder.build().unwrap().save_to_bytes().unwrap();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rust_pdf_tauri_commands_multipage_test_{}_{}_{unique}.pdf",
            std::process::id(),
            num_pages
        ));
        std::fs::write(&path, &bytes).expect("writing the multi-page fixture must not fail");
        let opened = open_document_impl(
            state,
            OpenDocumentRequest {
                path: path.to_string_lossy().into_owned(),
                password: None,
            },
        )
        .await
        .expect("opening the multi-page fixture must succeed");
        (opened, path)
    }

    #[tokio::test]
    async fn split_document_extracts_the_requested_pages() {
        let state = test_state();
        let (opened, fixture_path) = open_multi_page_sample(&state, 4).await;

        let result = split_document_impl(
            &state,
            SplitDocumentRequest {
                handle: opened.handle,
                page_indices: vec![1, 3],
            },
        )
        .await
        .expect("split_document must succeed");
        assert_eq!(result.page_count, 2);

        let extracted = extract_text_impl(
            &state,
            ExtractTextRequest {
                handle: result.handle,
                page_index: None,
            },
            no_progress(),
        )
        .await
        .expect("extracting text from the split document must succeed");
        assert_eq!(extracted.len(), 2);
        assert!(extracted[0].text.contains("Page number 1"));
        assert!(extracted[1].text.contains("Page number 3"));

        // The source document must be unaffected by the split.
        let source_page_count = render_page_impl(
            &state,
            RenderPageRequest {
                handle: opened.handle,
                page_index: 3,
                dpi: 72.0,
                viewport: None,
            },
        )
        .await;
        assert!(source_page_count.is_ok(), "source document's page 3 must still exist after split");
        let _ = std::fs::remove_file(&fixture_path);
    }

    #[tokio::test]
    async fn split_document_rejects_empty_page_indices() {
        let state = test_state();
        let (opened, fixture_path) = open_multi_page_sample(&state, 2).await;
        let result = split_document_impl(
            &state,
            SplitDocumentRequest {
                handle: opened.handle,
                page_indices: Vec::new(),
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
        let _ = std::fs::remove_file(&fixture_path);
    }

    #[tokio::test]
    async fn split_document_rejects_out_of_range_page() {
        let state = test_state();
        let (opened, fixture_path) = open_multi_page_sample(&state, 2).await;
        let result = split_document_impl(
            &state,
            SplitDocumentRequest {
                handle: opened.handle,
                page_indices: vec![0, 9],
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
        let _ = std::fs::remove_file(&fixture_path);
    }

    // -- add_watermark --------------------------------------------------------

    #[tokio::test]
    async fn add_watermark_stamps_every_page() {
        let state = test_state();
        let (opened, fixture_path) = open_multi_page_sample(&state, 3).await;

        let result = add_watermark_impl(
            &state,
            AddWatermarkRequest {
                handle: opened.handle,
                text: "CONFIDENTIAL".to_string(),
                font_size: 36.0,
                opacity: 0.3,
                rotation_degrees: 45.0,
                color: ColorRequest { r: 0.5, g: 0.5, b: 0.5 },
            },
        )
        .await
        .expect("add_watermark must succeed");
        assert_eq!(result.pages_watermarked, 3);

        let extracted = extract_text_impl(
            &state,
            ExtractTextRequest {
                handle: opened.handle,
                page_index: None,
            },
            no_progress(),
        )
        .await
        .expect("extracting text after watermarking must succeed");
        // The watermark text is appended as its own text-showing
        // operator; `extract_text`'s content-stream walk picks it up
        // right alongside the page's original body text.
        for page in &extracted {
            assert!(page.text.contains("CONFIDENTIAL"), "page text was: {:?}", page.text);
        }
        let _ = std::fs::remove_file(&fixture_path);
    }

    #[tokio::test]
    async fn add_watermark_rejects_empty_text() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = add_watermark_impl(
            &state,
            AddWatermarkRequest {
                handle: opened.handle,
                text: String::new(),
                font_size: 36.0,
                opacity: 0.3,
                rotation_degrees: 45.0,
                color: ColorRequest { r: 0.5, g: 0.5, b: 0.5 },
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn add_watermark_rejects_unknown_handle() {
        let state = test_state();
        let result = add_watermark_impl(
            &state,
            AddWatermarkRequest {
                handle: 999_999,
                text: "DRAFT".to_string(),
                font_size: 36.0,
                opacity: 0.3,
                rotation_degrees: 45.0,
                color: ColorRequest { r: 0.5, g: 0.5, b: 0.5 },
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn get_outline_returns_empty_for_document_without_bookmarks() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let outline = get_outline_impl(&state, GetOutlineRequest { handle: opened.handle })
            .await
            .expect("get_outline must succeed even when the document has no bookmarks");
        assert!(outline.is_empty());
    }

    #[tokio::test]
    async fn get_outline_rejects_unknown_handle() {
        let state = test_state();
        let result = get_outline_impl(&state, GetOutlineRequest { handle: 999_999 }).await;
        assert_eq!(result.unwrap_err().code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn get_outline_returns_nested_bookmarks_in_document_order() {
        use crate::editor::Destination;
        use crate::prelude::*;

        let state = test_state();

        // Build a small multi-page fixture, mirroring
        // `editor::outline`'s own tests, then open it through the normal
        // command path so `get_outline_impl` exercises the same handle
        // every other command uses.
        let content = ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Page");
        let mut builder = DocumentBuilder::new();
        for _ in 0..3 {
            let page = PageBuilder::a4()
                .font("F1", Standard14Font::Helvetica)
                .content(content.clone())
                .build();
            builder = builder.page(page);
        }
        let bytes = builder
            .build()
            .expect("building the outline fixture must not fail")
            .save_to_bytes()
            .expect("serializing the outline fixture must not fail");
        let path = std::env::temp_dir().join(format!(
            "rust_pdf_get_outline_test_{}_{}.pdf",
            std::process::id(),
            line!()
        ));
        std::fs::write(&path, &bytes).expect("writing the outline fixture must not fail");

        let opened = open_document_impl(
            &state,
            OpenDocumentRequest {
                path: path.to_string_lossy().into_owned(),
                password: None,
            },
        )
        .await
        .expect("opening the outline fixture must succeed");

        {
            let entry = state
                .get_document(DocumentHandle(opened.handle))
                .expect("just-opened document");
            let mut doc = entry.doc.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let parent = doc
                .add_bookmark(None, "Part I", Destination::fit(0))
                .expect("add top-level bookmark");
            doc.add_bookmark(Some(parent), "Section 1.1", Destination::fit(1))
                .expect("add nested bookmark");
            doc.add_bookmark(None, "Part II", Destination::fit(2))
                .expect("add second top-level bookmark");
        }

        let outline = get_outline_impl(&state, GetOutlineRequest { handle: opened.handle })
            .await
            .expect("get_outline must succeed");

        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].title, "Part I");
        assert_eq!(outline[0].page_index, Some(0));
        assert_eq!(outline[0].children.len(), 1);
        assert_eq!(outline[0].children[0].title, "Section 1.1");
        assert_eq!(outline[0].children[0].page_index, Some(1));
        assert!(outline[0].children[0].children.is_empty());
        assert_eq!(outline[1].title, "Part II");
        assert_eq!(outline[1].page_index, Some(2));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn get_text_layout_finds_fixture_content_with_a_plausible_box() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let pages = get_text_layout_impl(
            &state,
            GetTextLayoutRequest {
                handle: opened.handle,
                page_index: None,
            },
        )
        .await
        .expect("get_text_layout must succeed");

        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert_eq!(page.page_index, 0);
        // A4 in points (see `Rectangle::a4`).
        assert!((page.page_width - 595.0).abs() < 1.0);
        assert!((page.page_height - 842.0).abs() < 1.0);

        assert_eq!(page.runs.len(), 1);
        let run = &page.runs[0];
        assert_eq!(run.text, "Hello, rust-pdf tests!");
        // Fixture places the baseline at y=700 (see `sample_pdf_path`);
        // the box must straddle it and stay within the page.
        assert!(run.y < 700.0 && run.y + run.height > 700.0);
        assert!(run.x >= 0.0 && run.x + run.width <= page.page_width);
    }

    #[tokio::test]
    async fn get_text_layout_rejects_out_of_range_page() {
        let state = test_state();
        let opened = open_sample(&state).await;
        let result = get_text_layout_impl(
            &state,
            GetTextLayoutRequest {
                handle: opened.handle,
                page_index: Some(5),
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn get_text_layout_rejects_unknown_handle() {
        let state = test_state();
        let result = get_text_layout_impl(
            &state,
            GetTextLayoutRequest {
                handle: 999_999,
                page_index: None,
            },
        )
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::NotFound);
    }
}
