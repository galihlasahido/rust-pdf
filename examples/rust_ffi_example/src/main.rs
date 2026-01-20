//! Example: Using rust-pdf dynamic library from another Rust project.
//!
//! This demonstrates how to use the pre-built librust_pdf.dylib
//! without needing the rust-pdf source code.
//!
//! # Running
//!
//! First, build the rust-pdf library:
//! ```bash
//! cd ../..  # Go to rust-pdf root
//! cargo build --release
//! ```
//!
//! Then run this example:
//! ```bash
//! # macOS
//! DYLD_LIBRARY_PATH=../../target/release cargo run
//!
//! # Linux
//! LD_LIBRARY_PATH=../../target/release cargo run
//! ```

mod rust_pdf_sys;

use std::ffi::{CStr, CString};

fn main() {
    println!("rust-pdf FFI Example (Rust)");
    println!("===========================\n");

    // Get and print library version
    let version = unsafe {
        let version_ptr = rust_pdf_sys::pdf_version();
        CStr::from_ptr(version_ptr).to_string_lossy()
    };
    println!("Library version: {}\n", version);

    // Create a PDF with text
    let text = CString::new("Hello from Rust FFI!\n\nThis PDF was created using the rust-pdf dynamic library.").unwrap();

    println!("Creating PDF...");
    let pdf = unsafe { rust_pdf_sys::pdf_create_simple(text.as_ptr(), 18.0) };

    if pdf.is_null() {
        eprintln!("Error: Failed to create PDF");
        return;
    }

    // Get PDF data size
    let mut data_ptr: *const u8 = std::ptr::null();
    let size = unsafe { rust_pdf_sys::pdf_get_data(pdf, &mut data_ptr) };
    println!("PDF size: {} bytes", size);

    // Save to file
    let filename = CString::new("hello_from_rust_ffi.pdf").unwrap();
    println!("Saving to 'hello_from_rust_ffi.pdf'...");

    let result = unsafe { rust_pdf_sys::pdf_save_to_file(pdf, filename.as_ptr()) };

    if result == 0 {
        println!("Success! PDF saved.");
    } else {
        eprintln!("Error: Failed to save PDF");
    }

    // Free the handle
    unsafe { rust_pdf_sys::pdf_free(pdf) };

    println!("\nDone.");
}
