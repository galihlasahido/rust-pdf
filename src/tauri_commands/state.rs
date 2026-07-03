//! Tauri-managed application state: the registry of currently-open
//! documents plus the [`WorkerPool`] every command (including page
//! rasterization) dispatches its actual work to.
//!
//! An [`AppState`] is created once at application startup and registered
//! with `tauri::Builder::manage` (see the [module docs](super)); every
//! command receives a `tauri::State<'_, AppState>` referring to that same
//! instance.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::editor::EditableDocument;

use super::error::CommandError;
use super::worker::{default_worker_thread_count, WorkerPool};

/// Opaque identifier for a document opened via
/// [`super::commands::open_document`]. Allocated sequentially starting at
/// 1 for the lifetime of one [`AppState`] (i.e. one running application
/// instance) -- never reused, so a stale handle from an already-closed
/// document reliably reports [`crate::tauri_commands::error::ErrorCode::NotFound`]
/// rather than silently referring to a different, later document.
///
/// A plain `u64` (rather than e.g. a `Uuid`) serializes as a JSON number,
/// which is simplest for a Tauri frontend to hold onto and pass back
/// verbatim; sequential allocation starting at 1 keeps it comfortably
/// below JavaScript's `Number.MAX_SAFE_INTEGER` for any realistic
/// session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocumentHandle(pub u64);

/// One document currently open for structural editing (parsing,
/// text/annotation/form operations, saving, signing) *and* rendering:
/// [`commands::render_page`](super::commands::render_page) locks this same
/// [`EditableDocument`] (briefly, just to read a page's content/resources)
/// rather than opening a second, independent copy of the file through
/// [`crate::render::PdfRenderer`] the way an earlier, FFI-backed rendering
/// engine's `Send`-related constraints once required (see
/// [`crate::render`]'s module docs for that migration history).
pub(crate) struct DocumentEntry {
    /// Path the document was opened from, kept so `save_document`/
    /// `sign_document` can default their output path to it.
    pub(crate) path: PathBuf,
    pub(crate) doc: Mutex<EditableDocument>,
}

/// Shared handle to one registered [`DocumentEntry`], as returned by
/// [`AppState::get_document`]. Cloning is cheap (an `Arc` bump); commands
/// clone it out of the registry lock before doing any actual (possibly
/// slow) work, so the registry lock is only ever held for a lookup.
pub(crate) type DocumentEntryHandle = Arc<DocumentEntry>;

/// Application state shared across every Tauri command in this crate.
///
/// `Send + Sync + 'static` (required by `tauri::Manager::manage`): the
/// registry is a `Mutex<HashMap<..>>` of `Arc<DocumentEntry>`s, so
/// looking a document up only holds the registry lock briefly (to clone
/// an `Arc`) rather than for the duration of a whole (potentially
/// multi-second) operation on it.
pub struct AppState {
    documents: Mutex<HashMap<DocumentHandle, Arc<DocumentEntry>>>,
    next_handle: AtomicU64,
    pub(crate) pool: WorkerPool,
}

impl AppState {
    /// Creates a new, empty registry with a worker-thread count chosen
    /// from the host machine (see [`default_worker_thread_count`]).
    pub fn new() -> Self {
        Self::with_worker_threads(default_worker_thread_count())
    }

    /// Like [`AppState::new`], with an explicit worker-thread count (at
    /// least 1). Mainly useful for tests exercising concurrency with a
    /// small, deterministic thread count.
    pub fn with_worker_threads(thread_count: usize) -> Self {
        Self {
            documents: Mutex::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
            pool: WorkerPool::new(thread_count),
        }
    }

    /// Registers a newly-opened document and returns its handle.
    pub(crate) fn insert_document(&self, path: PathBuf, doc: EditableDocument) -> DocumentHandle {
        let handle = DocumentHandle(self.next_handle.fetch_add(1, Ordering::Relaxed));
        let entry = Arc::new(DocumentEntry {
            path,
            doc: Mutex::new(doc),
        });
        // Poisoned-lock recovery: a panic while some *other* document's
        // registry access held this lock must not permanently break
        // every future command for the whole app session. Rule 2 forbids
        // `unwrap`/`expect` on data *from a file*, which this is not --
        // it is this module's own in-memory registry -- but we still
        // avoid poisoning taking the app down, since a poisoned
        // `std::sync::Mutex` otherwise makes every subsequent
        // `documents.lock()` call panic too.
        let mut documents = self
            .documents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        documents.insert(handle, entry);
        handle
    }

    /// Looks up an open document by handle.
    pub(crate) fn get_document(&self, handle: DocumentHandle) -> Result<DocumentEntryHandle, CommandError> {
        let documents = self
            .documents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        documents.get(&handle).cloned().ok_or_else(|| {
            CommandError::not_found(format!("no open document with handle {}", handle.0))
        })
    }

    /// Removes a document from the registry (used by a future
    /// `close_document` command / on save-to-a-new-path flows).
    #[allow(dead_code)]
    pub(crate) fn remove_document(&self, handle: DocumentHandle) {
        let mut documents = self
            .documents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        documents.remove(&handle);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// Compile-time guarantee that `AppState` can be registered as Tauri
// managed state (`tauri::Manager::manage` requires `Send + Sync +
// 'static`). If a future field addition silently broke this, it would
// otherwise only surface as a confusing error at the `.manage(...)` call
// site in a downstream application, far from the actual cause.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<AppState>();
};

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    /// Builds (once per test process) a small, valid single-page PDF file
    /// on disk and returns its path, for every test in this module (and
    /// sibling `tauri_commands` test modules) that needs a real file to
    /// open/render/edit/sign.
    pub(crate) fn sample_pdf_path() -> PathBuf {
        static PATH: OnceLock<PathBuf> = OnceLock::new();
        PATH.get_or_init(|| {
            use crate::prelude::*;

            let content =
                ContentBuilder::new().text("F1", 24.0, 72.0, 700.0, "Hello, rust-pdf tests!");
            let page = PageBuilder::a4()
                .font("F1", Standard14Font::Helvetica)
                .content(content)
                .build();
            let doc = DocumentBuilder::new()
                .title("rust-pdf tauri_commands test fixture")
                .page(page)
                .build()
                .expect("building the test fixture PDF must not fail");
            let bytes = doc
                .save_to_bytes()
                .expect("serializing the test fixture PDF must not fail");

            let path = std::env::temp_dir().join(format!(
                "rust_pdf_tauri_commands_test_{}.pdf",
                std::process::id()
            ));
            std::fs::write(&path, &bytes).expect("writing the test fixture PDF must not fail");
            path
        })
        .clone()
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_has_no_open_documents() {
        let state = AppState::with_worker_threads(1);
        // `DocumentEntry` (via `EditableDocument`) does not implement
        // `Debug`, so `Result::unwrap_err` (which requires the `Ok` side
        // to be `Debug`) can't be used here; match instead.
        match state.get_document(DocumentHandle(1)) {
            Err(err) => assert_eq!(err.code, crate::tauri_commands::error::ErrorCode::NotFound),
            Ok(_) => panic!("expected no document to be registered yet"),
        }
    }

    #[test]
    fn insert_then_get_round_trips() {
        let state = AppState::with_worker_threads(1);
        let path = test_support::sample_pdf_path();
        let doc = EditableDocument::open(&path).expect("open fixture PDF");
        let handle = state.insert_document(path, doc);
        let entry = state.get_document(handle).expect("just-inserted document");
        assert_eq!(entry.doc.lock().unwrap().page_count().unwrap(), 1);
    }

    #[test]
    fn handles_are_never_reused() {
        let state = AppState::with_worker_threads(1);
        let path = test_support::sample_pdf_path();
        let doc_a = EditableDocument::open(&path).expect("open fixture PDF");
        let handle_a = state.insert_document(path.clone(), doc_a);
        state.remove_document(handle_a);

        let doc_b = EditableDocument::open(&path).expect("open fixture PDF");
        let handle_b = state.insert_document(path, doc_b);

        assert_ne!(handle_a.0, handle_b.0);
        assert!(state.get_document(handle_a).is_err());
    }
}
