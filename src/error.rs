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

/// A specialized Result type for PDF operations.
pub type PdfResult<T> = Result<T, PdfError>;
