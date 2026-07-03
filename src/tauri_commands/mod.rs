//! Async Tauri desktop-app command layer.
//!
//! This module is the "Tauri Integration" phase glue between the pure
//! `rust-pdf` document engine (parser/editor/render/signatures modules)
//! and a Tauri desktop application's IPC surface. It provides the nine
//! commands requested for the original phase:
//!
//! - [`commands::open_document`]
//! - [`commands::render_page`]
//! - [`commands::extract_text`]
//! - [`commands::search_text`]
//! - [`commands::apply_edit`]
//! - [`commands::save_document`]
//! - [`commands::fill_form`]
//! - [`commands::add_annotation`]
//! - [`commands::sign_document`]
//!
//! plus five more added in a later phase (PDF/A conversion, password
//! encryption, merge/split, watermarking):
//!
//! - [`commands::convert_to_pdfa`]
//! - [`commands::set_password`]
//! - [`commands::merge_documents`]
//! - [`commands::split_document`]
//! - [`commands::add_watermark`]
//!
//! plus one more added for a reader-UI phase (thumbnail/outline sidebar):
//!
//! - [`commands::get_outline`]
//!
//! plus two more added for a redaction-UI phase (draw-a-box-and-permanently-
//! remove-it, plus reading back the resulting audit trail):
//!
//! - [`commands::apply_redaction`]
//! - [`commands::get_redaction_log`]
//!
//! all `async fn`s registerable directly with `tauri::generate_handler!`,
//! backed by a single dedicated worker thread pool ([`worker::WorkerPool`])
//! so that none of them ever block Tauri's own async-command executor
//! threads.
//!
//! Rasterization ([`commands::render_page`]) used to require its own,
//! separate single-thread "actor" (rather than sharing this same
//! [`worker::WorkerPool`]) because the previous rendering backend's
//! per-document handle was not `Send` and its underlying native library's
//! C API was not safe to call concurrently. The rendering backend is now
//! [`rust_pdf::render::PdfRenderer`], built entirely on
//! [`rust_pdf::editor::EditableDocument`] (already `Send + Sync` -- the
//! same type every other command in this module already shares across
//! [`worker::WorkerPool`]'s threads), so that separate actor has been
//! retired: [`commands::render_page`] now dispatches to
//! [`worker::WorkerPool`] exactly like every other command, reusing the
//! same already-open document as [`commands::extract_text`]/
//! [`commands::apply_edit`]/etc. rather than opening a second, independent
//! copy of the file just to render it.
//!
//! # Architecture
//!
//! ```text
//!            (Tauri IPC, JSON args/return)
//!                       |
//!                       v
//!   #[tauri::command] async fn open_document(...)   <-- thin wrapper,
//!         |                                             registered with
//!         v                                             tauri::generate_handler!
//!   commands::open_document_impl(&state, ...)        <-- plain async fn,
//!         |                                              no Tauri types,
//!         v                                              directly unit-testable
//!   state.pool.run(|| EditableDocument::open(path))   <-- WorkerPool: N
//!         |                                               plain OS threads
//!         v
//!   AppState.documents: Mutex<HashMap<DocumentHandle, Arc<DocumentEntry>>>
//! ```
//!
//! [`commands::render_page`] follows the same shape, locking the same
//! `Arc<DocumentEntry>`'s `EditableDocument` (briefly, just to read the
//! page's content/resources) from within a [`worker::WorkerPool`] job
//! rather than going through a separate rendering-specific code path.
//!
//! # Error handling
//!
//! Every command returns `Result<T, error::CommandError>`.
//! [`error::CommandError`] is a plain `Serialize`able struct (an
//! `error::ErrorCode` plus a human-readable message) rather than a bare
//! `String` or a panic, so a Tauri frontend always receives a structured,
//! typed error it can branch on (e.g. `password_required` to prompt for a
//! password, vs. `invalid_argument` to show a form validation message) --
//! see the DoD in this phase's task description ("semua command punya
//! error handling yang mengembalikan pesan jelas ke frontend, bukan panic
//! yang crash app").
//!
//! # Progress reporting
//!
//! Long-running operations (`extract_text`/`search_text` over many pages,
//! `save_document`, `sign_document`) report progress via
//! [`progress::ProgressEvent`]s. The `#[tauri::command]` wrappers emit
//! these as a Tauri event (see [`progress::PROGRESS_EVENT_NAME`]);
//! `_impl` functions take a generic [`progress::ProgressReporter`] so
//! their logic is testable without any Tauri runtime at all.
//!
//! # Wiring this up in a Tauri application
//!
//! ```ignore
//! use rust_pdf::tauri_commands::{state::AppState, commands};
//!
//! fn main() {
//!     tauri::Builder::default()
//!         .manage(AppState::new())
//!         .invoke_handler(tauri::generate_handler![
//!             commands::open_document,
//!             commands::render_page,
//!             commands::extract_text,
//!             commands::search_text,
//!             commands::apply_edit,
//!             commands::save_document,
//!             commands::fill_form,
//!             commands::add_annotation,
//!             commands::sign_document,
//!             commands::convert_to_pdfa,
//!             commands::set_password,
//!             commands::merge_documents,
//!             commands::split_document,
//!             commands::add_watermark,
//!             commands::get_outline,
//!             commands::apply_redaction,
//!             commands::get_redaction_log,
//!         ])
//!         .run(tauri::generate_context!())
//!         .expect("error while running tauri application");
//! }
//! ```
//!
//! # What this phase deliberately does not include
//!
//! The wider multi-phase plan for this crate also lists document-convert
//! (`soffice --headless` subprocess) and OCR (Tesseract subprocess)
//! commands under the same "Tauri Integration" heading. Those need
//! subprocess sandboxing (path/argument validation, timeouts, resource
//! limits on a *child process*) that is a distinct, separately-scoped
//! piece of work from the in-process document-engine commands implemented
//! here, and is not implemented in this module.

pub mod commands;
pub mod error;
pub mod progress;
pub mod state;
mod worker;

pub use commands::RenderedPage;
pub use error::{CommandError, ErrorCode};
pub use progress::{ProgressEvent, ProgressReporter, PROGRESS_EVENT_NAME};
pub use state::{AppState, DocumentHandle};
