//! Background-thread dispatch for calls into `render`/`editor`, keeping the
//! egui UI thread from ever blocking on file I/O or page rasterization.
//!
//! `PdfRenderer::open_file`/`render_page` are plain synchronous functions
//! (see their docs) -- no async runtime is needed to call them off-thread,
//! just a channel back to the UI thread.

use std::sync::mpsc;

/// Runs `f` on a new OS thread and returns a receiver for its result.
///
/// The caller should poll the receiver with `try_recv()` once per frame and
/// call [`egui::Context::request_repaint`] while it is still pending, so the
/// result is picked up promptly without the UI thread ever blocking on `f`.
pub fn spawn<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> mpsc::Receiver<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Best-effort: the receiver may already have been dropped if a
        // newer request superseded this one.
        let _ = tx.send(f());
    });
    rx
}
