# rust-pdf

A comprehensive Rust library for creating, manipulating, and securing PDF documents.

## Features

- **PDF Creation**: Generate valid PDF 1.7 and 2.0 documents
- **Text Support**: All 14 standard PDF fonts with positioning and styling
- **Graphics**: Draw shapes, lines, curves, and apply colors (RGB, CMYK, Grayscale)
- **Images**: Embed JPEG and PNG images with alpha channel support
- **Compression**: Flate/zlib compression for reduced file sizes
- **PDF Parsing**: Read and extract content from existing PDFs
- **Encryption**: AES-256 password protection with permission controls
- **Digital Signatures**: Sign PDFs with X.509 certificates (RSA/ECDSA)

## Installation

### From crates.io (when published)

Add to your `Cargo.toml`:

```toml
[dependencies]
rust-pdf = "0.1.0"
```

### From Git Repository

```toml
[dependencies]
rust-pdf = { git = "https://github.com/galihlasahido/rust-pdf.git" }

# Or with a specific branch/tag
rust-pdf = { git = "https://github.com/galihlasahido/rust-pdf.git", branch = "main" }
rust-pdf = { git = "https://github.com/galihlasahido/rust-pdf.git", tag = "v0.1.0" }
```

### From Local Path

If you have the library locally (e.g., cloned or downloaded):

```toml
[dependencies]
rust-pdf = { path = "../rust-pdf" }

# With specific features
rust-pdf = { path = "../rust-pdf", features = ["encryption", "compression"] }
```

### Feature Flags

Enable only the features you need:

```toml
[dependencies]
# Basic PDF creation (no optional features)
rust-pdf = "0.1.0"

# With specific features
rust-pdf = { version = "0.1.0", features = ["compression", "images"] }

# All features
rust-pdf = { version = "0.1.0", features = ["full"] }
```

| Feature | Description | Dependencies |
|---------|-------------|--------------|
| `compression` | Flate/zlib stream compression | `flate2` |
| `images` | JPEG/PNG image embedding | `image` |
| `parser` | Read existing PDFs | `nom` |
| `encryption` | AES-256 password protection | `aes`, `sha2`, `rand` |
| `signatures` | Digital signatures | `rsa`, `x509-cert`, `cms` |
| `full` | All features enabled | All above |

## Quick Start

### Hello World

```rust
use rust_pdf::prelude::*;

fn main() -> Result<(), PdfError> {
    // Create content with text
    let content = ContentBuilder::new()
        .text("F1", 24.0, 72.0, 750.0, "Hello, World!");

    // Build a page
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    // Create and save document
    let doc = DocumentBuilder::new()
        .title("Hello World")
        .page(page)
        .build()?;

    doc.save_to_file("hello_world.pdf")?;
    Ok(())
}
```

### Drawing Graphics

```rust
use rust_pdf::prelude::*;

let content = ContentBuilder::new()
    // Red filled rectangle
    .save_state()
    .fill_color(Color::rgb(1.0, 0.0, 0.0))
    .rect(100.0, 100.0, 200.0, 150.0)
    .fill()
    .restore_state()

    // Blue stroked line
    .save_state()
    .stroke_color(Color::BLUE)
    .line_width(2.0)
    .move_to(100.0, 300.0)
    .line_to(300.0, 500.0)
    .stroke()
    .restore_state();
```

### Working with Images

```rust
use rust_pdf::prelude::*;

// Load an image
let image = Image::from_file("photo.jpg")?;

// Create page with image
let page = PageBuilder::a4()
    .image("Img1", image)
    .content(
        ContentBuilder::new()
            .save_state()
            .transform(Matrix::translate(100.0, 500.0).scale(200.0, 150.0))
            .draw_image("Img1")
            .restore_state()
    )
    .build();
```

### Compression

```rust
use rust_pdf::prelude::*;

// Compress all streams in the document
let doc = DocumentBuilder::new()
    .compress_streams(true)
    .page(page)
    .build()?;
```

### Password Protection (Encryption)

```rust
use rust_pdf::prelude::*;

let doc = DocumentBuilder::new()
    .encrypt(
        EncryptionConfig::aes256()
            .user_password("user123")      // Password to open
            .owner_password("owner456")    // Password for full access
            .permissions(
                Permissions::default()
                    .allow_printing(true)
                    .allow_copying(false)
            )
    )
    .page(page)
    .build()?;
```

### Digital Signatures

```rust
use rust_pdf::prelude::*;
use rust_pdf::signatures::{Certificate, PrivateKey, DocumentSigner};

// Load certificate and private key
let cert = Certificate::from_pem_file("cert.pem")?;
let key = PrivateKey::from_pem_file("key.pem")?;

// Create document
let doc = DocumentBuilder::new()
    .page(page)
    .build()?;

// Sign the document
let signed_pdf = DocumentSigner::new(doc)
    .certificate(cert)
    .private_key(key)
    .reason("Document approval")
    .location("San Francisco")
    .sign()?;

std::fs::write("signed.pdf", signed_pdf)?;
```

### Reading Existing PDFs

```rust
use rust_pdf::prelude::*;

let reader = PdfReader::from_file("document.pdf")?;

println!("Pages: {}", reader.page_count());
println!("Version: {:?}", reader.version());

// Access trailer and catalog
let trailer = reader.trailer();
let catalog = reader.catalog()?;
```

## Standard Fonts

The library includes all 14 PDF standard fonts:

| Font Family | Variants |
|-------------|----------|
| Helvetica | Regular, Bold, Oblique, BoldOblique |
| Times | Roman, Bold, Italic, BoldItalic |
| Courier | Regular, Bold, Oblique, BoldOblique |
| Symbol | - |
| ZapfDingbats | - |

```rust
use rust_pdf::prelude::*;

// Use any standard font
let page = PageBuilder::a4()
    .font("F1", Standard14Font::Helvetica)
    .font("F2", Standard14Font::TimesBold)
    .font("F3", Standard14Font::CourierOblique)
    .content(content)
    .build();
```

## Color Spaces

```rust
use rust_pdf::prelude::*;

// RGB colors
let red = Color::rgb(1.0, 0.0, 0.0);
let custom = Color::rgb(0.2, 0.5, 0.8);

// Predefined colors
let blue = Color::BLUE;
let black = Color::BLACK;

// Grayscale
let gray = Color::gray(0.5);

// CMYK (for print)
let cyan = Color::cmyk(1.0, 0.0, 0.0, 0.0);
```

## Page Sizes

```rust
use rust_pdf::prelude::*;

// Standard sizes
let a4 = PageBuilder::a4();
let letter = PageBuilder::letter();

// Custom size (width x height in points, 72 points = 1 inch)
let custom = PageBuilder::new(400.0, 600.0);
```

## Running Tests

```bash
# Run all tests (requires all features)
cargo test --all-features

# Run tests for specific feature
cargo test --features compression
cargo test --features images
cargo test --features parser
cargo test --features encryption
cargo test --features signatures

# Run only unit tests
cargo test --all-features --lib

# Run only integration tests
cargo test --all-features --test integration_tests
```

## Test Output

The test suite generates example PDFs in `tests/output/`. These serve as visual verification of library capabilities.

### Encrypted PDF Passwords

The following test PDFs are password-protected:

| File | User Password | Owner Password |
|------|---------------|----------------|
| `encrypted_minimal.pdf` | `user123` | `owner456` |
| `encrypted_with_content.pdf` | `secret` | `admin` |
| `encrypted_permissions.pdf` | *(empty)* | `owner` |
| `encrypted_multipage.pdf` | `multi` | `pages` |

**Note:** The user password opens the document. An empty user password means the PDF opens without prompting. The owner password grants full access including permission changes.

## API Reference

### Core Types

| Type | Description |
|------|-------------|
| `DocumentBuilder` | Constructs PDF documents |
| `PageBuilder` | Configures individual pages |
| `ContentBuilder` | Builds page content (text, graphics) |
| `GraphicsBuilder` | Drawing operations |
| `TextBuilder` | Text rendering |

### Document Metadata

```rust
let doc = DocumentBuilder::new()
    .version(PdfVersion::V2_0)
    .title("My Document")
    .author("John Doe")
    .subject("Example PDF")
    .keywords("rust, pdf, example")
    .creator("My Application")
    .producer("rust-pdf")
    .page(page)
    .build()?;
```

## Project Structure

```
rust-pdf/
├── src/
│   ├── lib.rs           # Library entry point
│   ├── color/           # Color types (RGB, CMYK, Gray)
│   ├── content/         # Content streams and operators
│   ├── document/        # Document and page structures
│   ├── encryption/      # AES-256 encryption
│   ├── error.rs         # Error types
│   ├── font/            # Font handling
│   ├── image/           # Image embedding
│   ├── object/          # PDF object types
│   ├── page/            # Page builder
│   ├── parser/          # PDF reader
│   ├── signatures/      # Digital signatures
│   ├── types/           # Common types
│   └── writer/          # PDF serialization
├── tests/
│   ├── integration_tests.rs
│   └── output/          # Generated test PDFs
└── Cargo.toml
```

## Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Build with all features
cargo build --all-features

# Check without building
cargo check

# Format code
cargo fmt

# Run linter
cargo clippy --all-features
```

## Dynamic Library Distribution (FFI)

The library can be built as a dynamic library (`.dylib`/`.so`/`.dll`) for use from C, Python, Ruby, or other languages without requiring Rust source code.

### Building the Dynamic Library

```bash
cargo build --release
```

This produces:
- **macOS**: `target/release/librust_pdf.dylib`
- **Linux**: `target/release/librust_pdf.so`
- **Windows**: `target/release/rust_pdf.dll`

### Distribution Files

To distribute without source code, include:

```
rust-pdf-dist/
├── lib/
│   └── librust_pdf.dylib   # (or .so/.dll)
└── include/
    └── rust_pdf.h          # C header file
```

### C API

```c
#include "rust_pdf.h"

int main(void) {
    // Create a simple PDF
    PdfHandle* pdf = pdf_create_simple("Hello from C!", 24.0);

    // Save to file
    pdf_save_to_file(pdf, "output.pdf");

    // Free resources
    pdf_free(pdf);

    return 0;
}
```

Compile with:
```bash
# macOS
clang -o myapp myapp.c -L/path/to/lib -lrust_pdf -I/path/to/include
DYLD_LIBRARY_PATH=/path/to/lib ./myapp

# Linux
gcc -o myapp myapp.c -L/path/to/lib -lrust_pdf -I/path/to/include
LD_LIBRARY_PATH=/path/to/lib ./myapp
```

### Using from Rust (via FFI)

If you want to use the pre-built dynamic library from another Rust project (without source code):

1. Create FFI bindings in your project:

```rust
// src/rust_pdf_sys.rs
use std::os::raw::c_char;

#[link(name = "rust_pdf")]
extern "C" {
    pub fn pdf_create_simple(text: *const c_char, font_size: f64) -> *mut PdfHandle;
    pub fn pdf_get_data(handle: *const PdfHandle, out_data: *mut *const u8) -> usize;
    pub fn pdf_save_to_file(handle: *const PdfHandle, path: *const c_char) -> i32;
    pub fn pdf_free(handle: *mut PdfHandle);
    pub fn pdf_version() -> *const c_char;
}

#[repr(C)]
pub struct PdfHandle {
    _private: [u8; 0],
}
```

2. Configure `build.rs` to find the library:

```rust
// build.rs
fn main() {
    println!("cargo:rustc-link-search=native=/path/to/lib");
    println!("cargo:rustc-link-lib=dylib=rust_pdf");
}
```

3. Use it in your code:

```rust
use std::ffi::CString;

mod rust_pdf_sys;

fn main() {
    unsafe {
        let text = CString::new("Hello from Rust FFI!").unwrap();
        let pdf = rust_pdf_sys::pdf_create_simple(text.as_ptr(), 24.0);

        if !pdf.is_null() {
            let path = CString::new("output.pdf").unwrap();
            rust_pdf_sys::pdf_save_to_file(pdf, path.as_ptr());
            rust_pdf_sys::pdf_free(pdf);
        }
    }
}
```

4. Run with the library path:

```bash
# macOS
DYLD_LIBRARY_PATH=/path/to/lib cargo run

# Linux
LD_LIBRARY_PATH=/path/to/lib cargo run
```

### Available C Functions

| Function | Description |
|----------|-------------|
| `pdf_create_simple(text, font_size)` | Create a PDF with text |
| `pdf_get_data(handle, out_data)` | Get PDF bytes (returns length) |
| `pdf_save_to_file(handle, path)` | Save PDF to file (returns 0 on success) |
| `pdf_free(handle)` | Free PDF handle |
| `pdf_version()` | Get library version string |

### Using from Other Languages

The dynamic library works with any language that supports FFI. See the `examples/` folder for complete examples:

#### Python (ctypes)

```python
import ctypes

lib = ctypes.CDLL("librust_pdf.dylib")  # or .so / .dll
lib.pdf_version.restype = ctypes.c_char_p

print(lib.pdf_version().decode())  # "0.1.0"

# Create PDF
pdf = lib.pdf_create_simple(b"Hello from Python!", 24.0)
lib.pdf_save_to_file(pdf, b"output.pdf")
lib.pdf_free(pdf)
```

#### Node.js (ffi-napi)

```javascript
const ffi = require('ffi-napi');

const lib = ffi.Library('librust_pdf', {
    'pdf_version': ['string', []],
    'pdf_create_simple': ['pointer', ['string', 'double']],
    'pdf_save_to_file': ['int', ['pointer', 'string']],
    'pdf_free': ['void', ['pointer']]
});

console.log(lib.pdf_version());  // "0.1.0"

const pdf = lib.pdf_create_simple("Hello from Node!", 24.0);
lib.pdf_save_to_file(pdf, "output.pdf");
lib.pdf_free(pdf);
```

#### Go (cgo)

```go
/*
#cgo LDFLAGS: -L./lib -lrust_pdf
#include <stdlib.h>
typedef struct PdfHandle PdfHandle;
PdfHandle* pdf_create_simple(const char* text, double font_size);
int pdf_save_to_file(const PdfHandle* handle, const char* path);
void pdf_free(PdfHandle* handle);
const char* pdf_version(void);
*/
import "C"

func main() {
    text := C.CString("Hello from Go!")
    pdf := C.pdf_create_simple(text, 24.0)
    C.pdf_save_to_file(pdf, C.CString("output.pdf"))
    C.pdf_free(pdf)
}
```

#### Ruby (ffi gem)

```ruby
require 'ffi'

module RustPdf
  extend FFI::Library
  ffi_lib 'librust_pdf.dylib'
  attach_function :pdf_version, [], :string
  attach_function :pdf_create_simple, [:string, :double], :pointer
  attach_function :pdf_save_to_file, [:pointer, :string], :int
  attach_function :pdf_free, [:pointer], :void
end

puts RustPdf.pdf_version  # "0.1.0"

pdf = RustPdf.pdf_create_simple("Hello from Ruby!", 24.0)
RustPdf.pdf_save_to_file(pdf, "output.pdf")
RustPdf.pdf_free(pdf)
```

### Supported Languages

| Language | FFI Method | Example File |
|----------|------------|--------------|
| C/C++ | Native | `examples/c_example.c` |
| Rust | FFI bindings | `examples/rust_ffi_example/` |
| Python | ctypes | `examples/python_example.py` |
| Node.js | ffi-napi | `examples/node_example.js` |
| Go | cgo | `examples/go_example.go` |
| Ruby | ffi gem | `examples/ruby_example.rb` |
| Java | JNA/JNI | (similar pattern) |
| C# | P/Invoke | (similar pattern) |
| Swift | C interop | (similar pattern) |
| PHP | FFI extension | (similar pattern) |

## License

MIT

## Contributing

Contributions are welcome! Please ensure:

1. All tests pass: `cargo test --all-features`
2. Code is formatted: `cargo fmt`
3. No clippy warnings: `cargo clippy --all-features`
