//! C-compatible FFI interface for rust-pdf library.
//!
//! This module provides functions that can be called from C, Python, or other languages
//! via FFI (Foreign Function Interface).

use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;

use crate::content::ContentBuilder;
use crate::document::DocumentBuilder;
use crate::font::Standard14Font;
use crate::page::PageBuilder;

/// Opaque handle to a PDF document
pub struct PdfHandle {
    data: Vec<u8>,
}

/// Create a simple PDF with text and return a handle.
/// Returns null on failure.
///
/// # Safety
/// `text` must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn pdf_create_simple(
    text: *const c_char,
    font_size: f64,
) -> *mut PdfHandle {
    if text.is_null() {
        return ptr::null_mut();
    }

    // SAFETY: `text` is non-null (checked above) and, per this function's
    // `# Safety` contract, points to a valid null-terminated C string owned
    // by the caller for at least the duration of this call. `CStr::from_ptr`
    // itself only requires the pointer be valid up to and including its
    // first NUL byte, which that contract guarantees; `.to_str()` right
    // after independently validates the bytes as UTF-8 before we ever treat
    // them as a Rust `&str`, so a non-UTF-8-but-otherwise-valid C string
    // cannot cause anything worse than the `Err` branch below.
    let c_str = match CStr::from_ptr(text).to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    let content = ContentBuilder::new()
        .text("F1", font_size, 72.0, 750.0, c_str);

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = match DocumentBuilder::new().page(page).build() {
        Ok(d) => d,
        Err(_) => return ptr::null_mut(),
    };

    let data = match doc.save_to_bytes() {
        Ok(d) => d,
        Err(_) => return ptr::null_mut(),
    };

    Box::into_raw(Box::new(PdfHandle { data }))
}

/// Get the PDF data from a handle.
/// Returns the length of the data, or 0 on failure.
/// The data pointer is written to `out_data`.
///
/// # Safety
/// `handle` must be a valid pointer returned by `pdf_create_*` functions.
/// `out_data` must be a valid pointer to a `*const u8`.
#[no_mangle]
pub unsafe extern "C" fn pdf_get_data(
    handle: *const PdfHandle,
    out_data: *mut *const u8,
) -> usize {
    if handle.is_null() || out_data.is_null() {
        return 0;
    }

    // SAFETY: `handle`/`out_data` are non-null (checked above). Per this
    // function's `# Safety` contract, `handle` was returned by a
    // `pdf_create_*` call and not yet freed via `pdf_free` (so it points to
    // a live, exclusively-owned `PdfHandle` -- this module never hands out
    // more than one reference to the same handle concurrently), and
    // `out_data` points to valid, correctly-aligned storage for one
    // `*const u8` that this call may write through.
    let pdf = &*handle;
    *out_data = pdf.data.as_ptr();
    pdf.data.len()
}

/// Save the PDF to a file.
/// Returns 0 on success, -1 on failure.
///
/// # Safety
/// `handle` must be a valid pointer returned by `pdf_create_*` functions.
/// `path` must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn pdf_save_to_file(
    handle: *const PdfHandle,
    path: *const c_char,
) -> i32 {
    if handle.is_null() || path.is_null() {
        return -1;
    }

    // SAFETY: `handle` is non-null (checked above) and, per this function's
    // `# Safety` contract, was returned by a `pdf_create_*` call and not yet
    // freed, so it points to a live `PdfHandle` we may borrow immutably.
    let pdf = &*handle;
    // SAFETY: `path` is non-null (checked above) and, per the same
    // contract, points to a valid null-terminated C string for the
    // duration of this call; see `pdf_create_simple`'s matching comment for
    // why a non-UTF-8 string is still handled safely (`Err` branch below).
    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    match std::fs::write(path_str, &pdf.data) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// Free a PDF handle.
///
/// # Safety
/// `handle` must be a valid pointer returned by `pdf_create_*` functions,
/// or null (which is safely ignored).
#[no_mangle]
pub unsafe extern "C" fn pdf_free(handle: *mut PdfHandle) {
    if !handle.is_null() {
        // SAFETY: `handle` is non-null (checked above) and, per this
        // function's `# Safety` contract, was returned by a `Box::into_raw`
        // call in `pdf_create_*` (the only place a `PdfHandle` is
        // constructed) and has not already been freed -- so it is exactly
        // the pointer `Box::from_raw` requires, reconstructing the `Box`
        // that owns it and dropping it exactly once here. Callers must not
        // use `handle` again after this call (also documented above); this
        // module cannot enforce that itself since the pointer crosses the
        // FFI boundary into caller-controlled code.
        drop(Box::from_raw(handle));
    }
}

/// Get the library version as a string.
/// The returned string is statically allocated and should not be freed.
#[no_mangle]
pub extern "C" fn pdf_version() -> *const c_char {
    static VERSION: &[u8] = b"0.1.0\0";
    VERSION.as_ptr() as *const c_char
}
