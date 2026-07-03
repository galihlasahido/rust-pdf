//! Error and warning types for the native (pure-Rust) content-stream
//! interpreter.
//!
//! Per this crate's mandatory rules, content streams are untrusted input:
//! this module distinguishes between
//!
//! - [`NativeRenderError`]: a *hard* failure that aborts the render before
//!   (or instead of) producing a pixel buffer -- always a structured,
//!   `Display`-able error, never a panic; and
//! - [`RenderWarning`]: a *soft*, recoverable condition (an operator or
//!   color space this phase doesn't implement, a malformed operand, a
//!   dangling resource reference, ...) that is recorded and skipped so the
//!   rest of the content stream still renders, rather than the whole page
//!   silently coming out blank or the interpreter panicking.

use thiserror::Error;

/// Hard failures that abort content-stream interpretation.
#[derive(Debug, Error, PartialEq)]
pub enum NativeRenderError {
    /// The requested output raster has a zero or otherwise invalid
    /// dimension.
    #[error("invalid output dimensions: {width}x{height}")]
    InvalidDimensions {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },

    /// `tiny_skia::Pixmap::new` failed to allocate the requested raster
    /// (e.g. `width * height` overflowed or the allocator refused).
    #[error("failed to allocate a {width}x{height} px raster")]
    PixmapAllocationFailed {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },

    /// The page's `/MediaBox` (ISO 32000-1 7.7.3.3) has zero or negative
    /// width/height, so no page-space-to-device-space mapping exists.
    #[error(
        "media box has zero or negative extent: [{llx} {lly} {urx} {ury}]"
    )]
    DegenerateMediaBox {
        /// Lower-left X.
        llx: f64,
        /// Lower-left Y.
        lly: f64,
        /// Upper-right X.
        urx: f64,
        /// Upper-right Y.
        ury: f64,
    },

    /// The `q` (save graphics state) operator nesting depth exceeded
    /// [`super::MAX_GRAPHICS_STATE_DEPTH`]. A well-formed content stream
    /// never needs anywhere near this many nested saves; this bounds a
    /// crafted content stream's ability to force unbounded `Vec` growth
    /// (a memory-exhaustion attempt via a `q` flood), per this crate's
    /// mandatory untrusted-input rules.
    #[error(
        "graphics state stack depth exceeded {max} (crafted/corrupt content stream?)"
    )]
    GraphicsStateStackOverflow {
        /// The configured maximum depth.
        max: usize,
    },
}

/// A recoverable condition encountered while interpreting a content
/// stream: something this phase of the native renderer does not (yet)
/// implement, or a malformed-but-not-fatal construct in the input. The
/// affected operator/construct is skipped (treated as a no-op) and
/// interpretation continues with the rest of the stream.
///
/// This type exists so callers/tests can assert *why* a render might not
/// look like a full-fidelity PDF viewer's output, instead of the gap being
/// silent. See `src/render/native/mod.rs` module docs for the full list of
/// gaps this phase is known to have (text, images/XObjects, non-Device
/// color spaces, shading, JBIG2/JPX/Type1-CFF, ICC color management).
#[derive(Debug, Clone, PartialEq)]
pub enum RenderWarning {
    /// A content-stream operator this phase of the interpreter does not
    /// implement (e.g. text-showing, `Do`, `sh`, non-Device color space
    /// selection). Its operands are discarded and it is treated as a
    /// no-op; painting/graphics-state operators elsewhere in the stream
    /// are unaffected.
    UnsupportedOperator {
        /// The operator keyword (e.g. `"Tj"`, `"Do"`, `"scn"`).
        operator: String,
    },
    /// An inline image (`BI`...`ID`...`EI`, ISO 32000-1 8.9.7) was
    /// encountered. Image/XObject painting is out of scope for this
    /// phase; the inline image is skipped in its entirety.
    InlineImageUnsupported,
    /// A `gs` operator named an `ExtGState` resource that was not found in
    /// `/Resources/ExtGState` (or no `resources` dictionary was supplied
    /// at all). Treated as a no-op: the graphics state is left unchanged.
    MissingExtGState {
        /// The resource name that could not be resolved.
        name: String,
    },
    /// A `d` (set dash pattern) operator supplied an array tiny-skia
    /// rejects (e.g. all-zero lengths, a negative length, or a
    /// non-finite phase). Falls back to a solid (non-dashed) stroke.
    InvalidDashPattern,
    /// The content stream ended partway through a syntactically malformed
    /// statement (e.g. an unterminated string or array). Bytes from that
    /// point to the end of the stream are not interpreted; everything
    /// parsed before it still rendered normally.
    TruncatedContentStream,
    /// The graphics-state-stack "restore" operator (`Q`) was invoked with
    /// no matching `q` (stack already at its initial depth). Ignored
    /// rather than treated as an error, since unbalanced `q`/`Q` is a
    /// (surprisingly common) real-world producer bug and not something
    /// that should block an otherwise-renderable page.
    UnbalancedRestore,
}

impl std::fmt::Display for RenderWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderWarning::UnsupportedOperator { operator } => {
                write!(f, "unsupported operator: {operator}")
            }
            RenderWarning::InlineImageUnsupported => {
                write!(f, "inline image (BI/ID/EI) unsupported this phase")
            }
            RenderWarning::MissingExtGState { name } => {
                write!(f, "ExtGState resource not found: /{name}")
            }
            RenderWarning::InvalidDashPattern => {
                write!(f, "invalid dash pattern, falling back to solid stroke")
            }
            RenderWarning::TruncatedContentStream => {
                write!(f, "content stream truncated at a malformed statement")
            }
            RenderWarning::UnbalancedRestore => {
                write!(f, "Q with no matching q, ignored")
            }
        }
    }
}
