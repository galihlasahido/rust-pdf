//! Structured, `Serialize`able errors for the Tauri command layer.
//!
//! Every command in [`super::commands`] returns `Result<T, CommandError>`
//! rather than panicking or returning a bare `String`: `CommandError`
//! carries a machine-readable [`ErrorCode`] a frontend can branch on
//! (e.g. redirect to a "enter password" dialog on
//! [`ErrorCode::PasswordRequired`]) plus a human-readable `message` for
//! display/logging. `tauri::command`'s macro-generated glue accepts any
//! error type implementing `Into<tauri::ipc::InvokeError>`, and `tauri`
//! provides a blanket `impl<T: Serialize> From<T> for InvokeError`, so
//! deriving `Serialize` here is all that is required to use this type
//! directly as a command's error type.

use serde::Serialize;

/// Machine-readable category of a [`CommandError`], stable across app
/// versions so a Tauri frontend can safely match on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// A caller-supplied argument was malformed or out of range (bad
    /// path, empty query, out-of-range page index, ...).
    InvalidArgument,
    /// The referenced document handle, page, field or annotation does
    /// not exist.
    NotFound,
    /// The document is encrypted and requires a (correct) password that
    /// was not supplied, or the supplied one was wrong.
    PasswordRequired,
    /// The requested operation is not supported by this document/build
    /// (e.g. a feature not compiled in, or a PDF construct this crate's
    /// editor does not model).
    Unsupported,
    /// The file could not be parsed as a valid PDF (ISO 32000-1 structure
    /// error), or is corrupt/truncated.
    ParseFailed,
    /// Rasterization failed (the content-stream interpreter reported a
    /// hard failure, or requested output would exceed this crate's pixel
    /// budget).
    RenderFailed,
    /// The rendering engine could not be initialized. Reserved for a
    /// future rendering backend with its own initialization step; this
    /// build's pure-Rust renderer has none (no native library to load), so
    /// no command in this build currently emits this code.
    RenderEngineUnavailable,
    /// A filesystem I/O operation (open/read/write) failed.
    IoFailed,
    /// A digital-signature operation (certificate/key loading, signing)
    /// failed.
    SignatureFailed,
    /// An internal invariant was violated (worker pool/render actor
    /// unavailable, a background task panicked, ...). Distinguished from
    /// the caller-facing categories above because it indicates a bug or
    /// resource-exhaustion condition in the host application rather than
    /// bad input.
    Internal,
}

/// A structured error returned by every command in [`super::commands`].
#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    /// Machine-readable category; see [`ErrorCode`].
    pub code: ErrorCode,
    /// Human-readable detail, safe to display to an end user or log.
    pub message: String,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

impl CommandError {
    /// Builds a new error with an explicit code and message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Shorthand for [`ErrorCode::InvalidArgument`].
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    /// Shorthand for [`ErrorCode::NotFound`].
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// Shorthand for [`ErrorCode::Unsupported`].
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }

    /// Shorthand for [`ErrorCode::RenderEngineUnavailable`].
    pub fn render_engine_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::RenderEngineUnavailable, message)
    }

    /// Shorthand for [`ErrorCode::Internal`].
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

impl From<std::io::Error> for CommandError {
    fn from(err: std::io::Error) -> Self {
        let code = match err.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::NotFound,
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                ErrorCode::InvalidArgument
            }
            _ => ErrorCode::IoFailed,
        };
        Self::new(code, err.to_string())
    }
}

/// Maps a [`crate::error::RenderError`] (borrowed, since callers may need
/// to classify one nested inside a [`crate::error::PdfError::Render`]
/// without consuming it) to the [`ErrorCode`] a frontend should see.
#[cfg(feature = "render")]
fn render_error_code(err: &crate::error::RenderError) -> ErrorCode {
    use crate::error::RenderError;
    match err {
        RenderError::PasswordRequired => ErrorCode::PasswordRequired,
        RenderError::InvalidPageIndex { .. }
        | RenderError::OutputTooLarge { .. }
        | RenderError::ViewportOutOfBounds { .. }
        | RenderError::EmptyViewport
        | RenderError::InvalidDpi(_) => ErrorCode::InvalidArgument,
        RenderError::DocumentLoad(_) | RenderError::PageRender { .. } => ErrorCode::RenderFailed,
    }
}

/// Maps an [`crate::error::EditorError`] (borrowed; same rationale as
/// [`render_error_code`]) to the [`ErrorCode`] a frontend should see.
#[cfg(feature = "parser")]
fn editor_error_code(err: &crate::error::EditorError) -> ErrorCode {
    use crate::error::EditorError;
    match err {
        EditorError::InvalidPageIndex { .. }
        | EditorError::InvalidArgument(_)
        | EditorError::WrongFieldType { .. }
        | EditorError::PageTreeTooLarge(_)
        | EditorError::ResourceLimitExceeded(_) => ErrorCode::InvalidArgument,
        EditorError::FieldNotFound(_) => ErrorCode::NotFound,
        EditorError::MissingCatalog
        | EditorError::MalformedPageTree(_)
        | EditorError::MissingBaseXref
        | EditorError::UnresolvedObject(_, _) => ErrorCode::ParseFailed,
        _ => ErrorCode::Unsupported,
    }
}

impl From<crate::error::PdfError> for CommandError {
    fn from(err: crate::error::PdfError) -> Self {
        use crate::error::PdfError;
        let code = match &err {
            PdfError::Io(io_err) => {
                return CommandError::from(std::io::Error::new(io_err.kind(), err.to_string()));
            }
            #[cfg(feature = "parser")]
            PdfError::Parser(crate::error::ParserError::EncryptedPdf) => ErrorCode::PasswordRequired,
            #[cfg(feature = "parser")]
            PdfError::Parser(_) => ErrorCode::ParseFailed,
            #[cfg(feature = "parser")]
            PdfError::Editor(editor_err) => editor_error_code(editor_err),
            #[cfg(feature = "render")]
            PdfError::Render(render_err) => render_error_code(render_err),
            #[cfg(feature = "signatures")]
            PdfError::Signature(_) => ErrorCode::SignatureFailed,
            PdfError::Form(_) => ErrorCode::InvalidArgument,
            _ => ErrorCode::Internal,
        };
        CommandError::new(code, err.to_string())
    }
}

#[cfg(feature = "render")]
impl From<crate::error::RenderError> for CommandError {
    fn from(err: crate::error::RenderError) -> Self {
        let code = render_error_code(&err);
        CommandError::new(code, err.to_string())
    }
}

#[cfg(feature = "signatures")]
impl From<crate::error::SignatureError> for CommandError {
    fn from(err: crate::error::SignatureError) -> Self {
        CommandError::new(ErrorCode::SignatureFailed, err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_not_found_maps_to_not_found_code() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let cmd_err = CommandError::from(io_err);
        assert_eq!(cmd_err.code, ErrorCode::NotFound);
        assert!(cmd_err.message.contains("no such file"));
    }

    #[test]
    fn serializes_as_snake_case_code_plus_message() {
        let err = CommandError::invalid_argument("bad page index");
        let json = serde_json::to_string(&err).expect("serialize CommandError");
        assert!(json.contains("\"invalid_argument\""));
        assert!(json.contains("bad page index"));
    }

    #[cfg(feature = "render")]
    #[test]
    fn render_password_required_maps_to_password_required_code() {
        let err = crate::error::RenderError::PasswordRequired;
        let cmd_err = CommandError::from(err);
        assert_eq!(cmd_err.code, ErrorCode::PasswordRequired);
    }

    #[cfg(feature = "render")]
    #[test]
    fn render_output_too_large_maps_to_invalid_argument() {
        let err = crate::error::RenderError::OutputTooLarge {
            width: 100_000,
            height: 100_000,
            pixels: 10_000_000_000,
            max_pixels: 1,
        };
        let cmd_err = CommandError::from(err);
        assert_eq!(cmd_err.code, ErrorCode::InvalidArgument);
    }
}
