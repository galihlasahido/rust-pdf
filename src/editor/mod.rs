//! Editing an already-existing PDF document in place.
//!
//! This module complements [`crate::document::Document`] (which only
//! *builds* brand-new PDFs) with an [`EditableDocument`] that loads an
//! existing file via [`crate::parser::PdfReader`], lets a caller mutate its
//! object graph (page content streams, page tree structure, bookmarks,
//! links, form fields, ...), and writes the result back out either as:
//!
//! - an **incremental update** ([`EditableDocument::save_incremental`],
//!   ISO 32000-1:2008 Section 7.5.6): the original file's bytes are left
//!   untouched and only the new/changed objects plus a new
//!   cross-reference section and trailer (chained via `/Prev`) are
//!   appended. This is the fast path used by interactive editors (e.g. a
//!   "save" after a single text edit) because cost is proportional to the
//!   size of the *change*, not the size of the document; or
//! - a **full rewrite** ([`EditableDocument::save_full_rewrite`]): only
//!   the objects still reachable from the trailer's `/Root` are written
//!   (so anything orphaned by an edit - e.g. a deleted page's content
//!   stream - is dropped instead of accumulating as dead weight), object
//!   numbers are compacted, and objects are packed into compressed object
//!   streams with a single compressed cross-reference stream (ISO
//!   32000-1:2008 Section 7.5.7 / 7.5.8). This is the path to use before
//!   long-term storage or distribution, where minimizing file size matters
//!   more than save latency.
//!
//! # What is (and isn't) implemented
//!
//! Page structure operations ([`EditableDocument::insert_blank_page`],
//! [`EditableDocument::delete_page`], [`EditableDocument::move_page`],
//! [`EditableDocument::rotate_page`], [`EditableDocument::extract_pages`],
//! [`EditableDocument::append_document`]) keep the page tree, the document
//! outline (bookmarks, ISO 32000-1 12.3.3) and `Link` annotations (ISO
//! 32000-1 12.5.6.5) internally consistent for the common case of
//! *direct-array* destinations (`[page /XYZ ...]`, which is what every
//! PDF producer this crate is aware of - including `Document` itself -
//! emits). Named destinations resolved through a document-level `/Names`
//! tree (ISO 32000-1 7.7.4) are not rewritten by `delete_page`; a bookmark
//! or link relying on one that pointed at a deleted page will therefore
//! keep pointing at the (now removed) page number rather than being
//! cleaned up. This is called out explicitly rather than silently
//! producing an inconsistent document.
//!
//! Content-stream editing ([`EditableDocument::replace_page_text`]) does a
//! byte-level substring replace inside `Tj`/`TJ`/`'`/`"` string operands.
//! It does not re-flow text, does not understand multi-byte/CID text
//! strings, and is only meaningful for simple (single-byte, Latin-text)
//! fonts. Rewriting arbitrary glyph runs with correct width metrics is a
//! much larger feature (effectively a text layout engine) and is out of
//! scope here; see `ARCHITECTURE.md`/the task report for an effort
//! estimate.
//!
//! [`EditableDocument::apply_redaction`]/[`EditableDocument::redact_text`]
//! build on top of the above for *permanent* redaction (text, images and
//! metadata actually removed from the object graph - not just visually
//! covered - plus an audit trail via [`RedactionAuditEntry`]), gated on
//! always finishing with [`EditableDocument::save_full_rewrite`] rather
//! than `save_incremental`; see `src/editor/redact.rs`'s module docs for
//! the full algorithm and its disclosed limitations.

mod annotations;
mod audit;
mod content_ops;
#[cfg(feature = "encryption")]
mod encrypt;
// `pub(crate)` (not private) so `crate::render::native` (feature
// `native-render`) can reuse this generic content-stream tokenizer instead
// of re-implementing its own ISO 32000-1 7.8.2 operand grammar. The
// individual items were already `pub(crate)`; only the module path itself
// needed widening. No behavior change.
pub(crate) mod content_stream;
mod forms;
mod graph;
pub mod icc;
mod outline;
mod pages;
pub mod pdfa;
pub mod pdfua;
pub mod pdfx;
mod redact;
mod save;
mod structure;
mod text_extract;
mod util;
mod watermark;
pub mod xmp;

pub use watermark::WatermarkOptions;

pub use annotations::{AnnotationInfo, AnnotationKind};
pub use audit::RedactionAuditEntry;
pub use graph::EditableDocument;
pub use icc::{IccColorSpace, IccError, OutputIntentInfo, OutputIntentSubtype};
pub use outline::{BookmarkNode, Destination};
pub use pdfa::{PdfAConversionOptions, PdfAConversionSummary, PdfAFlavor, PdfAReport, PdfAViolation};
pub use pdfua::{PdfUaReport, PdfUaViolation};
pub use pdfx::{PdfXColorReport, PdfXViolation};
pub use structure::{StructNode, StructType};
pub use xmp::{build_xmp_packet, read_pdfaid, read_pdfuaid, XmpFields};
