//! Error types for the rust-pdf library.

use thiserror::Error;

/// The main error type for PDF operations.
#[derive(Debug, Error)]
pub enum PdfError {
    /// Error during object serialization.
    #[error("Object error: {0}")]
    Object(#[from] ObjectError),

    /// Error during document building.
    #[error("Document error: {0}")]
    Document(#[from] DocumentError),

    /// Error during content stream building.
    #[error("Content error: {0}")]
    Content(#[from] ContentError),

    /// Error during PDF writing.
    #[error("Writer error: {0}")]
    Writer(#[from] WriterError),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Error during compression.
    #[cfg(feature = "compression")]
    #[error("Compression error: {0}")]
    Compression(#[from] CompressionError),

    /// Error during image handling.
    #[cfg(feature = "images")]
    #[error("Image error: {0}")]
    Image(#[from] ImageError),

    /// Error during PDF parsing.
    #[cfg(feature = "parser")]
    #[error("Parser error: {0}")]
    Parser(#[from] ParserError),

    /// Error during encryption.
    #[cfg(feature = "encryption")]
    #[error("Encryption error: {0}")]
    Encryption(#[from] EncryptionError),

    /// Error during digital signature operations.
    #[cfg(feature = "signatures")]
    #[error("Signature error: {0}")]
    Signature(#[from] SignatureError),

    /// Error during form field operations.
    #[error("Form error: {0}")]
    Form(#[from] FormError),

    /// Error during page rasterization/rendering.
    #[cfg(feature = "render")]
    #[error("Render error: {0}")]
    Render(#[from] RenderError),

    /// Error during in-place document editing (content-stream edits, page
    /// tree surgery, incremental or full-rewrite save).
    #[cfg(feature = "parser")]
    #[error("Editor error: {0}")]
    Editor(#[from] EditorError),

    /// Error during embedded TrueType/OpenType font loading, subsetting, or
    /// CID/Type0 composite font construction.
    #[cfg(feature = "fonts")]
    #[error("Font error: {0}")]
    Font(#[from] FontError),
}

/// Errors related to PDF object handling.
#[derive(Debug, Error)]
pub enum ObjectError {
    /// Invalid PDF name (contains invalid characters).
    #[error("Invalid PDF name: {0}")]
    InvalidName(String),

    /// Invalid PDF string encoding.
    #[error("Invalid PDF string: {0}")]
    InvalidString(String),

    /// Invalid object reference.
    #[error("Invalid object reference: ({0}, {1})")]
    InvalidReference(u32, u16),

    /// Stream without required Length key.
    #[error("Stream missing required Length key")]
    StreamMissingLength,
}

/// Errors related to document building.
#[derive(Debug, Error)]
pub enum DocumentError {
    /// Document has no pages.
    #[error("Document must have at least one page")]
    NoPages,

    /// Invalid PDF version.
    #[error("Invalid PDF version: {0}")]
    InvalidVersion(String),

    /// Missing required resource.
    #[error("Missing required resource: {0}")]
    MissingResource(String),
}

/// Errors related to content stream building.
#[derive(Debug, Error)]
pub enum ContentError {
    /// Unbalanced graphics state (save/restore).
    #[error("Unbalanced graphics state: {0} unmatched save operations")]
    UnbalancedState(i32),

    /// Text operation outside BT/ET block.
    #[error("Text operation outside text block")]
    TextOutsideBlock,

    /// Invalid color value (must be 0.0 to 1.0).
    #[error("Invalid color value: {0} (must be 0.0 to 1.0)")]
    InvalidColorValue(f64),

    /// Font not set before text operation.
    #[error("Font must be set before text operations")]
    FontNotSet,
}

/// Errors related to PDF writing.
#[derive(Debug, Error)]
pub enum WriterError {
    /// Failed to write PDF structure.
    #[error("Failed to write PDF structure: {0}")]
    Structure(String),

    /// Invalid byte offset.
    #[error("Invalid byte offset: {0}")]
    InvalidOffset(u64),
}

/// Errors related to compression operations.
#[cfg(feature = "compression")]
#[derive(Debug, Error)]
pub enum CompressionError {
    /// Failed to compress data.
    #[error("Failed to compress data: {0}")]
    CompressionFailed(String),

    /// Failed to decompress data.
    #[error("Failed to decompress data: {0}")]
    DecompressionFailed(String),

    /// Invalid compressed data.
    #[error("Invalid compressed data")]
    InvalidData,
}

/// Errors related to image handling.
#[cfg(feature = "images")]
#[derive(Debug, Error)]
pub enum ImageError {
    /// Failed to load image from file.
    #[error("Failed to load image: {0}")]
    LoadFailed(String),

    /// Unsupported image format.
    #[error("Unsupported image format: {0}")]
    UnsupportedFormat(String),

    /// Invalid image dimensions.
    #[error("Invalid image dimensions: {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },

    /// Failed to decode image data.
    #[error("Failed to decode image: {0}")]
    DecodeFailed(String),

    /// Failed to encode image data.
    #[error("Failed to encode image: {0}")]
    EncodeFailed(String),
}

/// Errors related to PDF parsing.
#[cfg(feature = "parser")]
#[derive(Debug, Error)]
pub enum ParserError {
    /// Failed to find PDF header.
    #[error("Invalid PDF: missing or invalid header")]
    InvalidHeader,

    /// Failed to find trailer.
    #[error("Invalid PDF: missing or invalid trailer")]
    InvalidTrailer,

    /// Failed to parse xref table.
    #[error("Invalid PDF: failed to parse xref table")]
    InvalidXref,

    /// Object not found.
    #[error("Object not found: {0} {1} R")]
    ObjectNotFound(u32, u16),

    /// Failed to parse object.
    #[error("Failed to parse object at offset {0}: {1}")]
    ParseFailed(u64, String),

    /// Unexpected end of file.
    #[error("Unexpected end of file")]
    UnexpectedEof,

    /// Invalid object stream.
    #[error("Invalid object stream: {0}")]
    InvalidObjectStream(String),

    /// Unsupported PDF feature.
    #[error("Unsupported PDF feature: {0}")]
    UnsupportedFeature(String),

    /// Encrypted PDF requires password.
    #[error("Encrypted PDF requires password")]
    EncryptedPdf,

    /// Invalid cross-reference stream.
    #[error("Invalid cross-reference stream")]
    InvalidXrefStream,

    /// Decompression error (when using parser with compression).
    #[cfg(feature = "compression")]
    #[error("Decompression failed: {0}")]
    Decompression(#[from] CompressionError),
}

/// Errors related to editing an already-loaded document
/// ([`crate::editor::EditableDocument`]): content-stream rewriting, page
/// tree surgery (insert/delete/move/rotate/split/merge) and incremental /
/// full-rewrite saving.
#[cfg(feature = "parser")]
#[derive(Debug, Error)]
pub enum EditorError {
    /// The document's trailer `/Root` does not resolve to a dictionary,
    /// or that dictionary has no usable `/Pages` entry (ISO 32000-1
    /// 7.7.2, Table 28).
    #[error("document catalog is missing or malformed (no usable /Pages)")]
    MissingCatalog,

    /// The page tree could not be walked: a node was neither a `/Pages`
    /// node (has `/Kids`) nor a `/Page` leaf, or referenced an object
    /// that does not resolve to a dictionary (ISO 32000-1 7.7.3).
    #[error("malformed page tree: {0}")]
    MalformedPageTree(String),

    /// Page tree recursion/fan-out exceeded the safety bound while
    /// walking a (possibly adversarial/corrupt) `/Kids` structure.
    #[error("page tree exceeds the maximum supported depth/size ({0})")]
    PageTreeTooLarge(&'static str),

    /// `index` was out of range for the document's current page count.
    #[error("page index {index} out of range (document has {count} pages)")]
    InvalidPageIndex {
        /// The requested zero-based page index.
        index: usize,
        /// The document's actual page count.
        count: usize,
    },

    /// A content stream (or the reachable-object graph during a
    /// full-rewrite save) exceeded a sanity size bound, most likely
    /// because of a corrupt or adversarial `/Length`/`/Kids`/`/Count`
    /// value in the source file.
    #[error("{0}")]
    ResourceLimitExceeded(String),

    /// Incremental save requires the byte offset of the base file's
    /// existing final cross-reference section (to populate `/Prev`), but
    /// it could not be located.
    #[error("could not locate base file's startxref offset for incremental save")]
    MissingBaseXref,

    /// A referenced indirect object could not be resolved (dangling
    /// reference, or object not present in this document's object
    /// space).
    #[error("object {0} {1} R could not be resolved")]
    UnresolvedObject(u32, u16),

    /// A caller-supplied argument was rejected (e.g. a rotation not a
    /// multiple of 90 degrees, ISO 32000-1 Table 30).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// No AcroForm field (ISO 32000-1 12.7.3) with the given fully
    /// qualified name (12.7.3.2) exists in the document.
    #[error("no form field named {0:?}")]
    FieldNotFound(String),

    /// A form field exists but is not of the type the operation requires
    /// (e.g. calling a checkbox setter on a text field).
    #[error("form field {name:?} is not a {expected} field")]
    WrongFieldType {
        /// The fully qualified field name.
        name: String,
        /// The field type the operation required (e.g. "text", "checkbox").
        expected: &'static str,
    },

    /// A new form field was requested with a name that already identifies
    /// an existing field (ISO 32000-1 12.7.3.2 requires field names be
    /// unique among siblings; this crate requires full-document
    /// uniqueness, which is always sufficient and simpler to reason
    /// about).
    #[error("a form field named {0:?} already exists")]
    DuplicateFieldName(String),

    /// No outline (bookmark) item with the given object id exists.
    #[error("no outline item {0} {1} R in the document")]
    OutlineItemNotFound(u32, u16),

    /// No annotation with the given object id exists on the expected page.
    #[error("no annotation {0} {1} R found")]
    AnnotationNotFound(u32, u16),

    /// [`crate::editor::EditableDocument::save_incremental`] (or
    /// `save_incremental_to_bytes`) was called after a `redact_*`/
    /// `strip_document_metadata` call this session. An incremental
    /// update only *appends* bytes (ISO 32000-1 7.5.6), so the
    /// pre-redaction content those calls removed from the object graph
    /// would still be fully recoverable in the file's earlier bytes -
    /// exactly the "hidden revision" problem redaction must not
    /// reintroduce. Use `save_full_rewrite`/`save_full_rewrite_to_bytes`
    /// instead.
    #[error(
        "cannot save incrementally after redaction: the pre-redaction content would remain \
         recoverable in the file's earlier bytes; use save_full_rewrite/save_full_rewrite_to_bytes instead"
    )]
    RedactionRequiresFullRewrite,
}

/// Errors related to PDF encryption.
#[cfg(feature = "encryption")]
#[derive(Debug, Error)]
pub enum EncryptionError {
    /// Invalid password.
    #[error("Invalid password")]
    InvalidPassword,

    /// Encryption key generation failed.
    #[error("Key generation failed: {0}")]
    KeyGenerationFailed(String),

    /// AES encryption/decryption failed.
    #[error("Cipher operation failed: {0}")]
    CipherFailed(String),

    /// Invalid encryption parameters.
    #[error("Invalid encryption parameters: {0}")]
    InvalidParameters(String),

    /// Unsupported encryption algorithm.
    #[error("Unsupported encryption algorithm: {0}")]
    UnsupportedAlgorithm(String),

    /// Missing file ID.
    #[error("File ID required for encryption")]
    MissingFileId,
}

/// Errors related to digital signatures.
#[cfg(feature = "signatures")]
#[derive(Debug, Error)]
pub enum SignatureError {
    /// Failed to load certificate.
    #[error("Failed to load certificate: {0}")]
    CertificateLoadFailed(String),

    /// Failed to load private key.
    #[error("Failed to load private key: {0}")]
    PrivateKeyLoadFailed(String),

    /// Signing operation failed.
    #[error("Signing failed: {0}")]
    SigningFailed(String),

    /// Verification failed.
    #[error("Signature verification failed: {0}")]
    VerificationFailed(String),

    /// Invalid signature format.
    #[error("Invalid signature format: {0}")]
    InvalidFormat(String),

    /// Certificate chain validation failed.
    #[error("Certificate chain validation failed: {0}")]
    CertificateChainInvalid(String),

    /// Unsupported algorithm.
    #[error("Unsupported signature algorithm: {0}")]
    UnsupportedAlgorithm(String),

    /// ByteRange calculation error.
    #[error("ByteRange calculation error: {0}")]
    ByteRangeError(String),

    /// PKCS#7 encoding error.
    #[error("PKCS#7 encoding error: {0}")]
    Pkcs7Error(String),

    /// RFC 3161 timestamp request/response error (malformed request,
    /// malformed/untrusted TSA response, or a transport error surfaced by
    /// the caller-supplied [`crate::signatures::TimestampAuthorityClient`]).
    #[error("Timestamp error: {0}")]
    TimestampError(String),
}

/// Errors related to form fields.
#[derive(Debug, Error)]
pub enum FormError {
    /// Invalid field name.
    #[error("Invalid field name: {0}")]
    InvalidFieldName(String),

    /// Duplicate field name.
    #[error("Duplicate field name: {0}")]
    DuplicateFieldName(String),

    /// Invalid field configuration.
    #[error("Invalid field configuration: {0}")]
    InvalidConfiguration(String),

    /// Missing required property.
    #[error("Missing required property: {0}")]
    MissingProperty(String),

    /// Invalid option index.
    #[error("Invalid option index: {0}")]
    InvalidOptionIndex(usize),
}

/// Errors related to page rasterization via this crate's pure-Rust
/// rendering pipeline (`render` feature: [`crate::render::PdfRenderer`],
/// built on [`crate::editor::EditableDocument`] for document structure and
/// [`crate::render::native`] for content-stream interpretation/
/// rasterization).
///
/// See the [`crate::render`] module documentation for this backend's
/// explicitly-documented scope and gaps (this type intentionally does not
/// wrap a third-party FFI error type -- there is no native/FFI dependency
/// anywhere in this rendering pipeline).
#[cfg(feature = "render")]
#[derive(Debug, Error)]
pub enum RenderError {
    /// Failed to load/parse the document's structure (corrupt file,
    /// unsupported/malformed cross-reference table, missing catalog,
    /// truncated data, etc). See [`crate::error::PdfError`]'s `Display`
    /// for the underlying cause.
    #[error("failed to open PDF document: {0}")]
    DocumentLoad(#[source] Box<PdfError>),

    /// The document is encrypted (ISO 32000-1 §7.6). This crate's
    /// pure-Rust parser ([`crate::parser::PdfReader`]) does not implement
    /// any decryption filter (RC4/AES) at all -- a pre-existing limitation
    /// of this crate's whole structural-editing pipeline, not something
    /// newly introduced by this rendering backend -- so *no* password,
    /// correct or not, allows opening an encrypted document here. (The
    /// previous, now-removed renderer (an FFI binding to a third-party
    /// native rendering engine) *could* decrypt a password-protected
    /// document; this is a real, accepted compatibility trade-off of the
    /// migration to a pure-Rust engine, not an oversight.)
    #[error(
        "document is encrypted; decryption is not supported by this pure-Rust engine \
         (no password can open it)"
    )]
    PasswordRequired,

    /// `page_index` was out of range for the document's page count.
    #[error("page index {index} out of range (document has {page_count} pages)")]
    InvalidPageIndex {
        /// The requested zero-based page index.
        index: usize,
        /// The document's actual page count.
        page_count: usize,
    },

    /// The native content-stream interpreter reported a hard failure while
    /// rendering a specific page (see
    /// [`crate::render::native::NativeRenderError`] for the exhaustive list
    /// -- all structurally-impossible-request cases, never a panic).
    #[error("failed to render page {page_index}: {source}")]
    PageRender {
        /// The zero-based page index that failed to render.
        page_index: usize,
        /// Underlying interpreter error.
        #[source]
        source: crate::render::native::NativeRenderError,
    },

    /// The requested output raster would exceed the configured maximum
    /// pixel budget. This guards against unbounded allocation driven by a
    /// (possibly adversarial) page `/MediaBox` combined with caller-supplied
    /// DPI -- untrusted-input input sizes must be bounds-checked before
    /// allocating, per this crate's mandatory rules.
    #[error(
        "requested render of {width}x{height} px ({pixels} px total) exceeds the \
         maximum of {max_pixels} px; lower the DPI or request a smaller viewport"
    )]
    OutputTooLarge {
        /// Requested output width in pixels.
        width: u32,
        /// Requested output height in pixels.
        height: u32,
        /// `width * height` as `u64` (computed with widening to avoid overflow).
        pixels: u64,
        /// The configured maximum number of pixels per render call.
        max_pixels: u64,
    },

    /// The requested viewport (tile) rectangle is not fully contained
    /// within the full-page raster it would be cropped from.
    #[error(
        "viewport at ({x},{y}) of size {width}x{height} is out of bounds for a \
         page rendered at {page_width}x{page_height} px"
    )]
    ViewportOutOfBounds {
        /// Requested viewport left offset, in device pixels.
        x: u32,
        /// Requested viewport top offset, in device pixels.
        y: u32,
        /// Requested viewport width, in device pixels.
        width: u32,
        /// Requested viewport height, in device pixels.
        height: u32,
        /// Full-page raster width, in device pixels, at the requested DPI.
        page_width: u32,
        /// Full-page raster height, in device pixels, at the requested DPI.
        page_height: u32,
    },

    /// The requested viewport had a zero width or height.
    #[error("viewport width and height must both be greater than zero")]
    EmptyViewport,

    /// `dpi` was not a finite, positive number.
    #[error("invalid DPI value: {0} (must be finite and > 0)")]
    InvalidDpi(f32),
}

/// Errors related to embedded TrueType/OpenType font loading, subsetting,
/// and CID/Type0 composite font construction (`fonts` feature). See
/// [`crate::font::truetype`], [`crate::font::subset`], and
/// [`crate::font::cid`].
#[cfg(feature = "fonts")]
#[derive(Debug, Error)]
pub enum FontError {
    /// Failed to load/validate a TrueType/OpenType font program.
    #[error("failed to load font: {0}")]
    Load(#[from] crate::font::truetype::FontLoadError),

    /// Failed to subset a font program to its used glyphs.
    #[error("failed to subset font: {0}")]
    Subset(#[from] crate::font::subset::SubsetError),
}

/// A specialized Result type for PDF operations.
pub type PdfResult<T> = Result<T, PdfError>;
