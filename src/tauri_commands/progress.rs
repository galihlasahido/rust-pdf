//! Progress reporting for long-running commands.
//!
//! Kept decoupled from the `tauri` crate's event/window types so the
//! actual PDF-processing logic in [`super::commands`] stays unit
//! testable without a running Tauri app: a [`ProgressReporter`] is just
//! `Arc<dyn Fn(ProgressEvent) + Send + Sync>`. The real
//! `#[tauri::command]` wrappers build one that calls
//! `tauri::Emitter::emit`; tests build one that pushes into a `Vec`.

use std::sync::Arc;

use serde::Serialize;

/// Tauri event name every [`ProgressEvent`] is emitted under. A frontend
/// listens for this once (`appWindow.listen("rust_pdf://progress", ...)`)
/// and dispatches on [`ProgressEvent::operation`]/`handle` itself, rather
/// than this crate registering one event name per operation.
pub const PROGRESS_EVENT_NAME: &str = "rust_pdf://progress";

/// One progress update for a long-running command.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProgressEvent {
    /// Machine-readable operation name (e.g. `"extract_text"`), stable
    /// across app versions.
    pub operation: &'static str,
    /// The document handle this progress belongs to, if the operation is
    /// scoped to one already-open document.
    pub handle: Option<u64>,
    /// Units of work completed so far (e.g. pages processed).
    pub current: u64,
    /// Total units of work, if known up front.
    pub total: Option<u64>,
    /// Optional human-readable detail (e.g. `"page 12 of 340"`).
    pub message: Option<String>,
}

/// Callback invoked with each [`ProgressEvent`] a long-running command
/// produces.
pub type ProgressReporter = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

/// A [`ProgressReporter`] that discards every event, for call sites that
/// don't need progress updates (e.g. a one-shot programmatic caller, or
/// a unit test only asserting on the return value).
pub fn no_progress() -> ProgressReporter {
    Arc::new(|_event| {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn no_progress_accepts_events_without_panicking() {
        let reporter = no_progress();
        reporter(ProgressEvent {
            operation: "test",
            handle: Some(1),
            current: 1,
            total: Some(10),
            message: None,
        });
    }

    #[test]
    fn reporter_closure_can_collect_events_for_assertions() {
        let events: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let reporter: ProgressReporter = Arc::new(move |event| {
            sink.lock().expect("test mutex poisoned").push(event);
        });

        reporter(ProgressEvent {
            operation: "extract_text",
            handle: Some(42),
            current: 1,
            total: Some(2),
            message: Some("page 1 of 2".to_string()),
        });
        reporter(ProgressEvent {
            operation: "extract_text",
            handle: Some(42),
            current: 2,
            total: Some(2),
            message: Some("page 2 of 2".to_string()),
        });

        let collected = events.lock().expect("test mutex poisoned");
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[1].current, 2);
    }
}
