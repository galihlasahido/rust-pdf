//! A dedicated single OS thread ("actor") that owns every open document's
//! [`PdfRenderer`], used exclusively by [`super::commands::render_page`].
//!
//! # Why this can't just use [`super::worker::WorkerPool`]
//!
//! `pdfium-render`'s `PdfDocument` (which [`PdfRenderer`] wraps) holds a
//! raw `FPDF_DOCUMENT` handle and is **not** `Send`. [`WorkerPool`]
//! dispatches each job to whichever of its N worker threads happens to be
//! free next, so a `PdfRenderer` created (lazily, on first
//! `render_page` call) on one worker thread could then need to be reused
//! -- for a *later* `render_page` call on the same document -- from a
//! *different* worker thread. The type system correctly refuses to
//! compile that. `pdfium-render`'s `thread_safe` Cargo feature (which
//! this crate enables by default -- see `Cargo.toml`) only makes the
//! top-level `Pdfium`/[`PdfiumLibrary`] binding itself `Send + Sync` (see
//! `src/render/renderer.rs`'s module docs and its own `ffi_lock`); it
//! does not extend that guarantee to `PdfDocument`/[`PdfRenderer`], and
//! this crate does not add an `unsafe impl Send` for a type it does not
//! own the FFI invariants of.
//!
//! Instead, every [`PdfRenderer`] this module creates lives and dies on
//! the single OS thread spawned by [`RenderActor::spawn`]: callers send
//! small, plain-data (`Send`) job descriptions in over a channel and get
//! `Send` results back over another channel; a `PdfRenderer` value itself
//! never crosses a thread boundary. This keeps rendering fully off both
//! the native UI event-loop thread and Tauri's async-command executor
//! threads, satisfying this phase's "worker thread pool separate from the
//! UI thread" requirement for the render path specifically, without
//! needing any `unsafe` code.
//!
//! One consequence worth calling out: because Pdfium itself is only
//! usable one call at a time regardless (see `ffi_lock` in
//! `src/render/renderer.rs`), funnelling every render through one actor
//! thread costs no real parallelism compared to a multi-threaded pool --
//! it just makes that pre-existing serialization explicit and gives each
//! open document's [`PdfRenderer`] a stable, long-lived home instead of
//! being reopened from scratch on every call.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::OnceLock;

use serde::Serialize;

use crate::render::{PdfRenderer, PdfiumLibrary, Viewport};

use super::error::CommandError;
use super::state::DocumentHandle;

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

/// A rectangular sub-region of a page-at-DPI raster to render (device
/// pixels), mirroring [`crate::render::Viewport`] as a plain,
/// IPC-`Deserialize`-able tuple. See [`super::commands::RenderPageRequest`].
pub type ViewportRequest = (u32, u32, u32, u32);

struct RenderPageJob {
    handle: DocumentHandle,
    path: PathBuf,
    password: Option<String>,
    page_index: usize,
    dpi: f32,
    viewport: Option<ViewportRequest>,
    respond: tokio::sync::oneshot::Sender<Result<RenderedPage, CommandError>>,
}

enum RenderJob {
    RenderPage(RenderPageJob),
    CloseDocument(DocumentHandle),
    /// Test-only seam: arms a one-shot panic that fires from *inside* the
    /// `catch_unwind`-wrapped body of the next `RenderPage` job this actor
    /// processes (see `run`), so tests can deterministically exercise "a
    /// render job panics" without depending on finding a real PDF/Pdfium
    /// input that happens to panic (which would be both fragile across
    /// Pdfium versions/platforms and outside this crate's control). Never
    /// constructed outside `#[cfg(test)]` code, so it compiles to nothing
    /// in release builds and cannot be reached from real IPC callers.
    #[cfg(test)]
    InjectPanicOnNextRender,
}

/// Handle to the background rendering actor thread. See the module docs
/// for why this exists instead of routing render work through
/// [`super::worker::WorkerPool`].
pub struct RenderActor {
    sender: Option<mpsc::Sender<RenderJob>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RenderActor {
    /// Spawns the actor thread. Cheap: no Pdfium document (or the Pdfium
    /// library binding itself) is loaded until the first `render_page`
    /// request arrives.
    pub fn spawn() -> Self {
        let (sender, receiver) = mpsc::channel::<RenderJob>();
        let thread = std::thread::Builder::new()
            .name("rust-pdf-render".to_string())
            .spawn(move || Self::run(receiver))
            .ok();
        Self {
            sender: Some(sender),
            thread,
        }
    }

    fn run(receiver: mpsc::Receiver<RenderJob>) {
        let mut renderers: HashMap<DocumentHandle, PdfRenderer<'static>> = HashMap::new();
        #[cfg(test)]
        let mut panic_next_render = false;
        while let Ok(job) = receiver.recv() {
            match job {
                RenderJob::RenderPage(job) => {
                    let RenderPageJob {
                        handle,
                        path,
                        password,
                        page_index,
                        dpi,
                        viewport,
                        respond,
                    } = job;
                    #[cfg(test)]
                    let fire_test_panic = std::mem::take(&mut panic_next_render);
                    // Mirrors `WorkerPool::worker_loop`'s per-job
                    // `catch_unwind` (see `worker.rs`): a single
                    // pathological input (e.g. a Pdfium call that hits an
                    // internal `unwrap`/assertion inside `pdfium-render`
                    // or this crate's own rendering code) must only fail
                    // *that* `render_page` call, not permanently kill this
                    // actor thread -- which, unlike a `WorkerPool` worker,
                    // is the sole owner of every open document's
                    // `PdfRenderer` and cannot simply be replaced by
                    // another thread (see the module docs). `renderers`
                    // is borrowed mutably across the `catch_unwind`
                    // boundary; `AssertUnwindSafe` is sound here for the
                    // same reason it is in `WorkerPool`: a panic
                    // unwinding out of `handle_render` cannot leave the
                    // `HashMap` itself in an invalid state (no partial
                    // mutation of its structure straddles the panic
                    // point), and `PdfRenderer`'s own FFI calls are
                    // serialized under `ffi_lock`, whose poisoning is
                    // already recovered from (see `ffi_lock`'s doc
                    // comment in `src/render/renderer.rs`) rather than
                    // left to propagate.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        #[cfg(test)]
                        if fire_test_panic {
                            panic!(
                                "render_actor test-injected panic (InjectPanicOnNextRender)"
                            );
                        }
                        Self::handle_render(
                            &mut renderers,
                            handle,
                            &path,
                            password.as_deref(),
                            page_index,
                            dpi,
                            viewport,
                        )
                    }))
                    .unwrap_or_else(|_| {
                        Err(CommandError::internal(
                            "internal error: render job panicked",
                        ))
                    });
                    let _ = respond.send(result);
                }
                RenderJob::CloseDocument(handle) => {
                    renderers.remove(&handle);
                }
                #[cfg(test)]
                RenderJob::InjectPanicOnNextRender => {
                    panic_next_render = true;
                }
            }
        }
    }

    /// Binds the native Pdfium library exactly once for the process
    /// lifetime (see [`PdfiumLibrary::bind`]), caching either the bound
    /// library or the (string-rendered, since [`crate::error::RenderError`]
    /// is not `Clone`) error so a missing/mismatched native library only
    /// pays the lookup cost once. Only ever called from the actor thread
    /// (`run`, via `ensure_renderer`), so no additional synchronization
    /// beyond `OnceLock` is needed.
    fn library() -> Result<&'static PdfiumLibrary, CommandError> {
        static LIB: OnceLock<Result<PdfiumLibrary, String>> = OnceLock::new();
        match LIB.get_or_init(|| PdfiumLibrary::bind().map_err(|e| e.to_string())) {
            Ok(lib) => Ok(lib),
            Err(message) => Err(CommandError::render_engine_unavailable(message.clone())),
        }
    }

    fn ensure_renderer<'a>(
        renderers: &'a mut HashMap<DocumentHandle, PdfRenderer<'static>>,
        handle: DocumentHandle,
        path: &Path,
        password: Option<&str>,
    ) -> Result<&'a PdfRenderer<'static>, CommandError> {
        match renderers.entry(handle) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let library = Self::library()?;
                let renderer = PdfRenderer::open_file(library, path, password)?;
                Ok(entry.insert(renderer))
            }
        }
    }

    fn handle_render(
        renderers: &mut HashMap<DocumentHandle, PdfRenderer<'static>>,
        handle: DocumentHandle,
        path: &Path,
        password: Option<&str>,
        page_index: usize,
        dpi: f32,
        viewport: Option<ViewportRequest>,
    ) -> Result<RenderedPage, CommandError> {
        let renderer = Self::ensure_renderer(renderers, handle, path, password)?;
        let viewport = viewport.map(|(x, y, width, height)| Viewport::new(x, y, width, height));
        let image = renderer.render_page(page_index, dpi, viewport)?;
        Ok(RenderedPage {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        })
    }

    /// Renders one page of `path` (opening/caching a [`PdfRenderer`] for
    /// `handle` on the actor thread if this is the first render request
    /// for it) without blocking the calling async task's own executor
    /// thread.
    pub async fn render_page(
        &self,
        handle: DocumentHandle,
        path: PathBuf,
        password: Option<String>,
        page_index: usize,
        dpi: f32,
        viewport: Option<ViewportRequest>,
    ) -> Result<RenderedPage, CommandError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(RenderJob::RenderPage(RenderPageJob {
            handle,
            path,
            password,
            page_index,
            dpi,
            viewport,
            respond: tx,
        }))?;
        rx.await
            .map_err(|_| CommandError::internal("render actor terminated before responding"))?
    }

    /// Drops any cached [`PdfRenderer`] for `handle`, releasing the
    /// native Pdfium document. Fire-and-forget: a document being closed
    /// that was never rendered is not an error.
    pub fn close_document(&self, handle: DocumentHandle) {
        let _ = self.send(RenderJob::CloseDocument(handle));
    }

    fn send(&self, job: RenderJob) -> Result<(), CommandError> {
        self.sender
            .as_ref()
            .ok_or_else(|| CommandError::internal("render actor has been shut down"))?
            .send(job)
            .map_err(|_| CommandError::internal("render actor has been shut down"))
    }
}

impl Drop for RenderActor {
    fn drop(&mut self) {
        // Dropping the sender first lets the actor thread's blocking
        // `recv()` return `Err` and exit `run`'s loop (which also runs
        // every cached `PdfRenderer`'s own `Drop`, closing its Pdfium
        // document under `ffi_lock`); only then do we join it, so this
        // never deadlocks and tests that create many short-lived
        // `AppState`s never leak a detached render thread holding open
        // Pdfium documents.
        self.sender = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_drop_joins_thread_without_hanging() {
        let actor = RenderActor::spawn();
        drop(actor);
    }

    /// Spawns a fresh actor and renders `path`'s page 0 under `handle`,
    /// returning `None` (after printing a skip message, matching the
    /// convention in `tests/render_tests.rs`) if the native Pdfium
    /// library isn't available in this environment.
    ///
    /// Deliberately does **not** pre-check availability via a separate
    /// `PdfiumLibrary::bind()` probe call: `pdfium-render` only permits
    /// binding the native library once per **process**, ever (see
    /// [`PdfiumLibrary::bind`]'s docs) -- a probe living anywhere other
    /// than [`RenderActor::library`]'s own call site would race it for
    /// which one gets to be that one-and-only bind, and whichever one
    /// loses would wrongly report "unavailable" even though Pdfium is
    /// genuinely available (just already bound via the other call site).
    /// Using this same, single, real code path as the availability check
    /// avoids that race entirely.
    async fn warm_up(handle: DocumentHandle, path: &Path) -> Option<(RenderActor, RenderedPage)> {
        let actor = RenderActor::spawn();
        match actor
            .render_page(handle, path.to_path_buf(), None, 0, 72.0, None)
            .await
        {
            Ok(page) => Some((actor, page)),
            Err(err) if err.code == super::super::error::ErrorCode::RenderEngineUnavailable => {
                eprintln!(
                    "skipping a render_actor test: Pdfium native library not available in this \
                     environment ({}). Run `scripts/fetch_pdfium.sh` and set \
                     RUST_PDF_PDFIUM_LIB_DIR, or install Pdfium system-wide.",
                    err.message
                );
                None
            }
            Err(err) => panic!("warm-up render of a valid single-page PDF failed unexpectedly: {err:?}"),
        }
    }

    #[tokio::test]
    async fn render_page_out_of_range_reports_invalid_argument() {
        let path = crate::tauri_commands::state::test_support::sample_pdf_path();
        let Some((actor, _)) = warm_up(DocumentHandle(2), &path).await else {
            return;
        };
        let result = actor
            .render_page(DocumentHandle(2), path, None, 99, 72.0, None)
            .await;
        let err = result.unwrap_err();
        assert_eq!(err.code, super::super::error::ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn render_page_succeeds_and_returns_nonempty_rgba() {
        let path = crate::tauri_commands::state::test_support::sample_pdf_path();
        let Some((_actor, page)) = warm_up(DocumentHandle(3), &path).await else {
            return;
        };
        assert!(page.width > 0 && page.height > 0);
        assert_eq!(page.rgba.len(), page.width as usize * page.height as usize * 4);
    }

    #[tokio::test]
    async fn close_document_then_reopen_still_works() {
        let path = crate::tauri_commands::state::test_support::sample_pdf_path();
        let handle = DocumentHandle(4);
        let Some((actor, _)) = warm_up(handle, &path).await else {
            return;
        };
        actor.close_document(handle);
        // Re-rendering after `close_document` must transparently reopen
        // the (still on-disk) file rather than erroring out.
        let page = actor.render_page(handle, path, None, 0, 72.0, None).await;
        assert!(page.is_ok());
    }

    /// Regression guard for the actor-thread panic-resilience gap: one
    /// `render_page` call whose underlying job panics (simulated here via
    /// `RenderJob::InjectPanicOnNextRender` -- see that variant's doc
    /// comment for why a synthetic trigger is used instead of a real
    /// pathological PDF) must only fail *that* call with a structured
    /// `CommandError`, not permanently kill the actor thread. Every
    /// subsequent `render_page` call -- including one for a page after the
    /// panicking one, and one for a brand new document handle -- must
    /// still succeed, exactly like `WorkerPool::survives_a_panicking_job`
    /// (see `worker.rs`) proves for the general worker pool.
    #[tokio::test]
    async fn render_page_survives_a_panicking_job() {
        let path = crate::tauri_commands::state::test_support::sample_pdf_path();
        let handle = DocumentHandle(5);
        let Some((actor, _)) = warm_up(handle, &path).await else {
            return;
        };

        // Arm the one-shot test panic, then immediately issue the render
        // call it should fire from. `send` (like `render_page`) goes
        // through the same `mpsc::Sender`, so FIFO ordering guarantees
        // the actor thread processes the arming job before this render
        // job.
        actor
            .send(RenderJob::InjectPanicOnNextRender)
            .expect("actor should still be accepting jobs");
        let panicked = actor
            .render_page(handle, path.clone(), None, 0, 72.0, None)
            .await;
        let err = panicked.expect_err("the injected panic must surface as an Err, not a crash");
        assert_eq!(err.code, super::super::error::ErrorCode::Internal);

        // The actor thread must still be alive and able to serve further
        // render requests -- both for the same document handle and for a
        // different one that has to be freshly opened.
        let same_handle_after = actor.render_page(handle, path.clone(), None, 0, 72.0, None).await;
        assert!(
            same_handle_after.is_ok(),
            "actor must keep serving the same document handle after a panicking job: {same_handle_after:?}"
        );

        let other_handle = DocumentHandle(6);
        let other_handle_after = actor.render_page(other_handle, path, None, 0, 72.0, None).await;
        assert!(
            other_handle_after.is_ok(),
            "actor must keep serving new document handles after a panicking job: {other_handle_after:?}"
        );
    }
}
