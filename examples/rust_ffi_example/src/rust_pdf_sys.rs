//! FFI bindings for rust-pdf dynamic library.
//!
//! This module provides raw FFI bindings to the rust-pdf C API.
//! Use these functions to create PDFs without needing the rust-pdf source code.

use std::os::raw::c_char;

/// Opaque handle to a PDF document.
/// The internal structure is hidden - only use through the provided functions.
#[repr(C)]
pub struct PdfHandle {
    _private: [u8; 0],
}

#[link(name = "rust_pdf")]
extern "C" {
    /// Create a simple PDF with text.
    ///
    /// # Parameters
    /// - `text`: Null-terminated UTF-8 string for the PDF content
    /// - `font_size`: Font size in points (e.g., 12.0, 24.0)
    ///
    /// # Returns
    /// A handle to the PDF document, or null on failure.
    /// Must be freed with `pdf_free()`.
    pub fn pdf_create_simple(text: *const c_char, font_size: f64) -> *mut PdfHandle;

    /// Get the PDF data from a handle.
    ///
    /// # Parameters
    /// - `handle`: PDF handle from `pdf_create_*` functions
    /// - `out_data`: Pointer to receive the data pointer
    ///
    /// # Returns
    /// The length of the data in bytes, or 0 on failure.
    /// The data pointer is valid until `pdf_free()` is called.
    pub fn pdf_get_data(handle: *const PdfHandle, out_data: *mut *const u8) -> usize;

    /// Save the PDF to a file.
    ///
    /// # Parameters
    /// - `handle`: PDF handle from `pdf_create_*` functions
    /// - `path`: Null-terminated file path string
    ///
    /// # Returns
    /// 0 on success, -1 on failure.
    pub fn pdf_save_to_file(handle: *const PdfHandle, path: *const c_char) -> i32;

    /// Free a PDF handle.
    ///
    /// # Parameters
    /// - `handle`: PDF handle to free (null is safely ignored)
    pub fn pdf_free(handle: *mut PdfHandle);

    /// Get the library version.
    ///
    /// # Returns
    /// A static string containing the version (e.g., "0.1.0").
    /// Do not free this string.
    pub fn pdf_version() -> *const c_char;
}
