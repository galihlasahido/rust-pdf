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
    /// `Tf` named a resource not present in `/Resources /Font` (or no
    /// `resources` dictionary was supplied at all, or the entry wasn't a
    /// dereferenced dictionary -- see `native`'s module docs on the
    /// pre-resolved-`Resources` assumption). Text shown before the next
    /// (valid) `Tf` renders nothing.
    MissingFontResource {
        /// The resource name that could not be resolved.
        name: String,
    },
    /// A text-showing operator ran with no active font at all (`Tf` was
    /// never called, or only ever named a missing resource). The string
    /// operand is discarded; no glyphs are painted.
    MissingActiveFont,
    /// `Tf` selected a font resource this phase cannot rasterize --
    /// see `crate::render::native::font`'s module docs for the full list
    /// of reasons (no embedded program, Type1/bare-CFF, or an otherwise
    /// unparseable program) and which of those is the documented,
    /// structural gap versus an unexpected/adversarial input. Recorded
    /// once per resource name; subsequent text shown with it renders
    /// nothing (but still advances the pen using its declared widths).
    UnsupportedFontProgram {
        /// The resource name (`/Resources /Font /<name>`) this applies to.
        resource_name: String,
        /// Human-readable reason (see
        /// `font::UnsupportedFontReason`'s `Display` impl).
        reason: String,
    },
    /// A Type 3 glyph procedure (ISO 32000-1:2008 9.6.5) recursed past
    /// [`super::font::MAX_TYPE3_DEPTH`] -- a self-referential or
    /// mutually-recursive set of Type 3 fonts (untrusted/adversarial
    /// input). The glyph is skipped (nothing painted for it) rather than
    /// recursing further.
    Type3RecursionLimitExceeded,
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
            RenderWarning::MissingFontResource { name } => {
                write!(f, "Tf referenced a missing font resource: /{name}")
            }
            RenderWarning::MissingActiveFont => {
                write!(f, "text shown with no active font (Tf never succeeded)")
            }
            RenderWarning::UnsupportedFontProgram { resource_name, reason } => {
                write!(f, "font /{resource_name} cannot be rendered: {reason}")
            }
            RenderWarning::Type3RecursionLimitExceeded => {
                write!(f, "Type 3 glyph procedure recursion limit exceeded, glyph skipped")
            }
        }
    }
}
