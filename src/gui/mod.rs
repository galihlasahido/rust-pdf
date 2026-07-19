//! Native desktop GUI shell (feature `native-gui`), built directly on
//! [`crate::render::PdfRenderer`] and [`crate::editor::EditableDocument`] --
//! both plain synchronous, `Send + Sync` APIs with no dependency on Tauri or
//! any async runtime (see those modules' own docs). Background work (opening
//! a file, rasterizing a page) runs on a plain OS thread via [`actions::spawn`]
//! rather than through [`crate::tauri_commands`]'s worker pool, since egui's
//! own per-frame polling loop is a simpler fit here than bridging through an
//! async executor.
//!
//! This is intentionally an MVP viewer (open a PDF, page through it, zoom):
//! editing/signing/forms/redaction panels are follow-up work, each wiring
//! one more already-Tauri-free `editor`/`signatures` API into a new panel
//! the same way [`app::PdfViewerApp`] wires up rendering and bookmarks.

mod actions;
mod app;
mod coords;
mod forms;
mod search;
mod theme;
mod thumbnails;
mod viewer;

pub use app::PdfViewerApp;
