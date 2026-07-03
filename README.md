# rust-pdf

An enterprise-oriented, mostly-pure-Rust PDF engine: create documents from
scratch, edit and redact existing ones, render pages to raster images with
no native/FFI dependency, fill and flatten forms, sign and encrypt, check
PDF/A / PDF/UA / PDF/X conformance, and drive all of it from a Tauri
desktop app's async command layer.

This README describes what is **actually implemented in `src/` today** —
every function/type named below was verified against the source while
writing this document, and every code block is either a real `cargo run`
example checked into `examples/` or a compiled doctest. For the full,
warts-and-all technical account (module-by-module inventory, migration
history, measured performance, `cargo audit`/`clippy` results, and an
exhaustive "known gaps" list), see **[ARCHITECTURE.md](ARCHITECTURE.md)**.
For the security trust-boundary analysis (what an attacker who controls a
PDF/font/ICC-profile can do, and what mitigates it), see
**[docs/THREAT_MODEL.md](docs/THREAT_MODEL.md)**.

## What this crate is (and isn't)

- A PDF **generation** library (`document`/`page`/`content`/`font`/`forms`),
  the original core.
- An **in-place editor** for existing PDFs (`editor::EditableDocument`):
  page structure, form fill/flatten, annotations, outline/bookmarks,
  Tagged-PDF logical structure, permanent redaction, PDF/A-PDF/UA-PDF/X
  conversion and validation, incremental or full-rewrite saving.
- A **pure-Rust page rasterizer** (`render`/`native-render`): no bundled
  native binary, no FFI — a from-scratch content-stream interpreter over
  `tiny-skia` (rasterization) and `ttf-parser` (font outlines). It has
  honestly-documented gaps (JBIG2/JPX images, Type1/bare-CFF fonts, true
  ICC color management, Patterns/shadings — see "Known limitations"
  below and ARCHITECTURE.md §8d) that fail closed with a structured
  warning, never a silent mis-render.
- A **font-embedding pipeline** (`fonts` feature): embedded TrueType/
  OpenType fonts, Type 0/CIDFontType2 composite fonts for CJK and other
  non-Latin text, and automatic glyph subsetting.
- A **Tauri desktop-app command layer** (`tauri` feature): nine async
  commands (`open_document`, `render_page`, `extract_text`, `search_text`,
  `apply_edit`, `save_document`, `fill_form`, `add_annotation`,
  `sign_document`) on a shared worker-thread pool, with structured
  (never-panicking) errors and progress events.
- **Not** a general-purpose arbitrary-PDF consumer: reading an
  **encrypted** PDF is not supported anywhere in the read/render/edit
  path (`ParserError::EncryptedPdf`/`RenderError::PasswordRequired`
  unconditionally) — the `encryption` feature is write-only (it produces
  encrypted output for documents this crate creates, it does not open
  password-protected input). `html` and `office` are Cargo features that
  exist in `Cargo.toml` but have **no corresponding module** — declared,
  aspirational surface area, not implemented functionality.

## Features

- **PDF creation** — PDF 1.7/2.0 documents, all 14 standard fonts, vector
  graphics (paths, curves, RGB/CMYK/Gray colors), JPEG/PNG image
  embedding, Flate compression.
- **Embedded/CJK fonts** — TrueType/OpenType embedding, Type 0/CIDFontType2
  composite fonts for CJK (and any non-Latin script), automatic glyph
  subsetting to shrink embedded font programs to only the glyphs used.
- **PDF parsing** — structural reader with memory-mapped file I/O
  (`PdfReader::from_file`), so opening a multi-gigabyte PDF and reading a
  handful of pages does not pull the whole file into process memory
  (measured: see `tests/large_file_rss_bench.rs`).
- **Editing an existing PDF** (`editor::EditableDocument`) — insert/
  delete/move/rotate/extract pages, append documents, byte-level content
  replace, incremental save (ISO 32000-1 §7.5.6, appends only the delta)
  or full rewrite with compacted/compressed object streams.
- **Redaction** — permanently removes text runs and images intersecting a
  page area (or a literal text needle) from the object graph — not just a
  visual overlay — prunes now-unused `/ToUnicode` entries, and records an
  audit trail. Always finishes with a full rewrite so the pre-redaction
  bytes can't linger recoverable in an incremental update.
- **Annotations** — highlight, underline, strikeout, free text, stamp,
  ink, and comment/popup, each with a generated appearance stream.
- **Outline / bookmarks / named destinations** — read/write the document
  outline tree (ISO 32000-1 §12.3.3) and the `/Names` destination tree.
- **Tagged structure (Tagged PDF)** — headings, paragraphs, tables (rows/
  cells) and figures with alt text, tied to real marked-content sequences
  in the page's content stream (ISO 32000-1 §14.7/14.8) — the building
  block PDF/UA tagging needs.
- **AcroForm fields** — create text fields, checkboxes, radio groups,
  combo/list boxes, push buttons and signature fields; read/set values;
  **flatten** a filled form into non-interactive, visually-identical
  content.
- **PDF/A, PDF/UA, PDF/X** — validate and (where automatically safe)
  convert to PDF/A-1b/2b/3b; prepare and validate PDF/UA-1 tagging
  (document language, `/DisplayDocTitle`, structure tree); validate
  PDF/X-related output-intent/color rules. Each is an explicitly-scoped
  subset of its ISO spec, not full veraPDF parity — see
  `src/editor/pdfa.rs`'s module docs for the exact rule list.
- **Rendering** — rasterize a page to an `image::RgbaImage` at any DPI,
  full-page or a device-pixel tile/viewport (for zoom/pan), plus cached
  thumbnails. 100% pure Rust, no native binary/FFI at all.
- **Encryption** — AES-256 password protection with permission flags,
  for documents this crate creates (write-only, see above).
- **Digital signatures** — sign with X.509 certificates (RSA/ECDSA),
  verify signatures, incremental (add-a-signature-without-invalidating-
  earlier-ones) signing, PAdES timestamping.
- **Tauri integration** — an async command layer wired directly to
  `tauri::generate_handler!`, backed by a plain worker-thread pool (no
  native-library-driven "actor" thread needed, since the renderer itself
  has no FFI handle to serialize access to).
- **Security hardening** — resource limits throughout the parser,
  redaction and rendering paths (bounded allocation from untrusted
  `/MediaBox`, `/W` width arrays, outline/structure-tree traversal, etc.)
  and fuzz-tested entry points; see
  [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) for the full attacker
  model and mitigation register.

## Known limitations

Read [ARCHITECTURE.md §8d](ARCHITECTURE.md) for the complete, re-verified
list. The headline ones:

| Gap | What actually happens |
|-----|------------------------|
| JBIG2 / JPX (JPEG2000) images | No pure-Rust decoder exists for either; the renderer paints a documented mid-grey placeholder and records a warning — never a crash or silent blank. |
| Type1 / bare (unwrapped) CFF font programs | `ttf-parser` cannot parse these; no glyph ink is painted for that font (text position still advances correctly), with a recorded warning. Real `sfnt`/OpenType containers with CFF-flavored outlines *are* supported — only bare/unwrapped Type1/CFF is affected. |
| ICC color management | `ICCBased` color spaces are approximated (resolved to `/Alternate` or guessed from `/N`), not truly color-managed; `Lab` is unsupported. Every use records a warning. |
| Encrypted PDFs | Cannot be opened for reading, editing, or rendering at all — no password path exists anywhere in the read pipeline. |
| Patterns / shadings | Not painted; selecting a Pattern color space leaves the current color unchanged and records a warning. |
| Tiled rendering | Still rasterizes the whole page internally before cropping the requested tile, so memory use is bounded by full-page size, not tile size. |

None of these fail silently: every gap raises a structured
`RenderWarning`/`NativeRenderError` (or, outside rendering, a documented
`PdfResult` error) that a caller can branch on.

## Installation

### From crates.io (when published)

```toml
[dependencies]
rust-pdf = "0.1.0"
```

### From Git Repository

```toml
[dependencies]
rust-pdf = { git = "https://github.com/galihlasahido/rust-pdf.git" }
```

### From Local Path

```toml
[dependencies]
rust-pdf = { path = "../rust-pdf", features = ["parser", "render"] }
```

### Feature flags

`default = []` — the bare crate builds with only document/page/content/
font/forms/object/types/writer (no compression, no image/parsing/
rendering/crypto). Enable what you need:

| Feature | What it adds | Key dependencies |
|---|---|---|
| `compression` | Flate/zlib stream compression | `flate2` |
| `images` | JPEG/PNG image embedding | `image` |
| `parser` | Read existing PDFs (`PdfReader`), memory-mapped for large files, plus the `editor::EditableDocument` in-place editing API | `nom`, `memmap2`, `jpeg-decoder` |
| `encryption` | AES-256 password protection (write-only, see "Known limitations") | `aes`, `cbc`, `sha2`, `md-5`, `rand`, `zeroize` |
| `signatures` | Digital signatures (RSA/ECDSA), verification, PAdES timestamping | `cms`, `x509-cert`, `rsa`, `p256`, `ecdsa`, `der`, `spki` (pulls in `encryption`) |
| `fonts` | Embedded TrueType/OpenType fonts, Type 0/CID composite fonts (CJK), subsetting | `ttf-parser`, `subsetter` |
| `native-render` | Content-stream interpreter + 2D rasterizer, single content stream in (no document/page-tree access of its own) | `tiny-skia`, `ttf-parser` (pulls in `parser`, `fonts`) |
| `render` | Whole-document page rasterization (`render::PdfRenderer`): resolves the page tree/`/MediaBox`/`/Resources` via this crate's own parser/editor, then rasterizes via `native-render` | `native-render` + `images` |
| `tauri` | Async Tauri desktop command layer (nine commands) on a shared worker-thread pool | `tauri`, `tokio`, `serde` (pulls in `parser`, `render`, `signatures`) |
| `html`, `office` | **Declared only — no implementation exists.** Do not enable expecting functionality. | — |
| `full` | `compression` + `images` + `parser` + `encryption` + `signatures` + `fonts` (deliberately **excludes** `render`/`native-render`/`tauri` — a structural/generation/signing consumer shouldn't have to pull in a rasterizer or the Tauri runtime) | — |

```toml
# Document creation + reading + editing + CJK fonts
rust-pdf = { version = "0.1.0", features = ["parser", "fonts", "compression"] }

# Everything a desktop viewer/editor app needs, including rendering
rust-pdf = { version = "0.1.0", features = ["render", "signatures", "fonts"] }

# A Tauri application
rust-pdf = { version = "0.1.0", features = ["tauri"] }
```

## Quick Start

### Hello World

```rust
use rust_pdf::prelude::*;

fn main() -> Result<(), PdfError> {
    let content = ContentBuilder::new()
        .text("F1", 24.0, 72.0, 750.0, "Hello, World!");

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Hello World")
        .page(page)
        .build()?;

    doc.save_to_file("hello_world.pdf")?;
    Ok(())
}
```

### Editing, redaction, annotation and rendering an existing PDF

Requires `features = ["parser", "fonts", "render"]`. Full, run source at
[`examples/render_and_edit_demo.rs`](examples/render_and_edit_demo.rs)
(verified with `cargo run --features "parser fonts render" --example render_and_edit_demo`):

```rust
use rust_pdf::editor::{Destination, StructType};
use rust_pdf::prelude::*;

let mut doc = EditableDocument::from_bytes(source_pdf_bytes)?;

// Outline/bookmark (ISO 32000-1 12.3.3).
doc.add_bookmark(None, "Quarterly Report", Destination::fit(0))?;

// Tagged structure: a /H1 structure element (ISO 32000-1 14.7/14.8).
doc.add_tagged_content(
    0, None, StructType::Heading(1),
    &ContentBuilder::new().text("F1", 18.0, 72.0, 760.0, "Quarterly Report"),
    None,
)?;

// Permanent redaction: text/images inside the rect are actually removed
// from the object graph, not just visually covered.
doc.apply_redaction(
    0,
    Rectangle::new(72.0, 690.0, 400.0, 712.0),
    "compliance-bot",
    "PII removed before external distribution",
)?;

// Highlight annotation (ISO 32000-1 12.5.6.10).
doc.add_highlight_annotation(0, &[(72.0, 755.0, 300.0, 772.0)], Color::rgb(1.0, 1.0, 0.0))?;

// Redaction requires a full rewrite (an incremental update would leave
// the pre-redaction bytes recoverable in the file's earlier revision).
doc.save_full_rewrite("edited_report.pdf")?;

// Render page 0 with the pure-Rust rasterizer (no native/FFI dependency).
let renderer = rust_pdf::render::PdfRenderer::open_file("edited_report.pdf")?;
let image = renderer.render_page(0, 150.0, None)?;
image.save("edited_report_page0.png")?;
```

### Forms: fill and flatten

Requires `features = ["parser"]`. Full, run source at
[`examples/form_fill_flatten_demo.rs`](examples/form_fill_flatten_demo.rs)
(verified with `cargo run --features parser --example form_fill_flatten_demo`):

```rust
use rust_pdf::forms::{CheckBox, TextField};
use rust_pdf::prelude::*;

let page = PageBuilder::a4()
    .font("F1", Standard14Font::Helvetica)
    .form_field(TextField::new("full_name").rect(100.0, 700.0, 250.0, 20.0))
    .form_field(CheckBox::new("subscribe").rect(100.0, 660.0, 18.0, 18.0))
    .build();
let bytes = DocumentBuilder::new().page(page).build()?.save_to_bytes()?;

let mut doc = EditableDocument::from_bytes(bytes)?;
doc.set_text_value("full_name", "Andi Wijaya")?;
doc.set_checkbox_checked("subscribe", true)?;
doc.save_incremental("signup_filled.pdf")?;   // fields still interactive

doc.flatten_form()?;                          // bake appearances, drop /AcroForm
doc.save_full_rewrite("signup_flattened.pdf")?;
assert!(doc.field_names()?.is_empty());
```

### CJK text via embedded/subsetted composite fonts

Requires `features = ["fonts", "parser"]`. Full, run source at
[`examples/cjk_font_demo.rs`](examples/cjk_font_demo.rs) (verified with
`cargo run --features "fonts parser" --example cjk_font_demo`):

```rust
use rust_pdf::font::CompositeFont;
use rust_pdf::prelude::*;

let font_bytes = std::fs::read("NotoSansSC.ttf")?;
let font = CompositeFont::new(font_bytes, "NotoSansSC")?.subset(true);
let cid_bytes = font.encode("中文测试");

let page = PageBuilder::a4()
    .font("F1", font)
    .content(ContentBuilder::new().text_block(
        TextBuilder::new().font("F1", 24.0).position(72.0, 700.0).show_bytes(cid_bytes),
    ))
    .build();

let bytes = DocumentBuilder::new().page(page).build()?.save_to_bytes()?;
```

### PDF/A conversion

Requires `features = ["parser"]`. Full, run source at
[`examples/pdfa_quickstart_demo.rs`](examples/pdfa_quickstart_demo.rs)
(verified with `cargo run --features parser --example pdfa_quickstart_demo`
— see [`examples/pdfa_convert_demo.rs`](examples/pdfa_convert_demo.rs) for a
fuller demo cross-checked against the real veraPDF CLI):

```rust
use rust_pdf::editor::{PdfAConversionOptions, PdfAFlavor};
use rust_pdf::prelude::*;

let mut doc = EditableDocument::from_bytes(source_pdf_bytes)?;
let options = PdfAConversionOptions {
    icc_profile,       // real ICC profile bytes -- this crate never bundles one
    icc_identifier: "sRGB IEC61966-2.1",
    icc_condition: "sRGB",
    title: Some("Archival copy"),
    producer: Some("rust-pdf"),
};
let summary = doc.convert_to_pdfa(PdfAFlavor::Part2B, &options)?;

let pdfa_bytes = doc.save_pdfa_compatible_to_bytes(PdfAFlavor::Part2B.min_pdf_version())?;
let reopened = EditableDocument::from_bytes(pdfa_bytes)?;
let report = reopened.validate_pdfa(PdfAFlavor::Part2B)?;
assert!(report.is_conformant() || !report.violations.is_empty()); // see docs for caveats
```

Note the honest caveat this example surfaces when actually run with a
Standard-14 (non-embedded) font: `validate_pdfa` correctly reports
`is_conformant() == false` with an "font resource is not embedded"
violation, because PDF/A mandates every font be embedded — conversion
does not fabricate a fix for a font this crate has no embeddable source
for. Using an embedded (`fonts`-feature) font instead avoids this.

### Password protection (encryption)

Requires `features = ["encryption"]`. Full, run source at
[`examples/encryption_demo.rs`](examples/encryption_demo.rs) (verified with
`cargo run --features encryption --example encryption_demo`). Write-only,
see "Known limitations":

```rust
use rust_pdf::prelude::*;

let doc = DocumentBuilder::new()
    .encrypt(
        EncryptionConfig::aes256()
            .user_password("user123")
            .owner_password("owner456")
            .permissions(
                Permissions::default()
                    .allow_printing(true)
                    .allow_copying(false)
            )
    )
    .page(page)
    .build()?;
```

### Digital signatures

Requires `features = ["signatures"]`. Verified against the real, running
[`examples/digital_signature_example.rs`](examples/digital_signature_example.rs)
(`cargo run --example digital_signature_example --features signatures`):

```rust
use rust_pdf::prelude::*;
use rust_pdf::signatures::{Certificate, PrivateKey, DocumentSigner};

let cert = Certificate::from_pem_file("cert.pem")?;
let key = PrivateKey::from_pem_file("key.pem")?;

let doc = DocumentBuilder::new().page(page).build()?;

let signed_pdf = DocumentSigner::new(doc)
    .certificate(cert)
    .private_key(key)
    .reason("Document approval")
    .location("San Francisco")
    .sign()?;

std::fs::write("signed.pdf", signed_pdf)?;
```

### Tauri desktop-app integration

Requires the `tauri` feature (pulls in `parser`, `render`, `signatures`).
See [`src/tauri_commands/mod.rs`](src/tauri_commands/mod.rs) for full
module docs, architecture diagram, and error-handling/progress-reporting
conventions.

```rust
use rust_pdf::tauri_commands::{state::AppState, commands};

fn main() {
    tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::open_document,
            commands::render_page,
            commands::extract_text,
            commands::search_text,
            commands::apply_edit,
            commands::save_document,
            commands::fill_form,
            commands::add_annotation,
            commands::sign_document,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

Every command returns a structured, serializable `CommandError` (never a
panic), and long-running commands (`extract_text`/`search_text` over
many pages, `save_document`, `sign_document`) emit Tauri progress events.
Rendering shares the same worker-thread pool as every other command —
no separate single-thread "actor" is needed, since `PdfRenderer` (built
on `EditableDocument`) is a plain `Send + Sync` value rather than a
handle into a native library.

### Reading existing PDFs

Requires `features = ["parser"]`. Full, run source at
[`examples/read_existing_pdf_demo.rs`](examples/read_existing_pdf_demo.rs)
(verified with `cargo run --features parser --example read_existing_pdf_demo`):

```rust
use rust_pdf::prelude::*;

// `from_file` memory-maps the file (see the `parser` feature above) --
// opening a multi-gigabyte PDF does not require multi-gigabyte RSS.
let reader = PdfReader::from_file("document.pdf")?;

println!("Pages: {}", reader.page_count());
println!("Version: {:?}", reader.version());

let trailer = reader.trailer();
let catalog = reader.catalog().ok_or("document has no /Root catalog")?;
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

let page = PageBuilder::a4()
    .font("F1", Standard14Font::Helvetica)
    .font("F2", Standard14Font::TimesBold)
    .font("F3", Standard14Font::CourierOblique)
    .content(content)
    .build();
```

For CJK or other non-Latin text, use an embedded `CompositeFont` instead
(see the CJK quick start above) — the Standard 14 fonts only cover
Latin/WinAnsi-range glyphs.

## Color Spaces

```rust
use rust_pdf::prelude::*;

let red = Color::rgb(1.0, 0.0, 0.0);
let blue = Color::BLUE;
let gray = Color::gray(0.5);
let cyan = Color::cmyk(1.0, 0.0, 0.0, 0.0);
```

## Page Sizes

```rust
use rust_pdf::prelude::*;

let a4 = PageBuilder::a4();
let letter = PageBuilder::letter();
let custom = PageBuilder::new(400.0, 600.0); // points, 72pt = 1 inch
```

## Running Tests

```bash
# Run everything (721+ unit tests plus integration suites)
cargo test --all-features

# Feature-scoped
cargo test --features parser
cargo test --features render
cargo test --features signatures

# Doctests only
cargo test --doc --all-features
```

Two integration tests build/read a real ~2 GB, 10,000-page fixture to
measure large-file behavior (render latency, resident memory) and are
`#[ignore]`d by default — run explicitly with `-- --ignored`; see
`tests/large_file_render_bench.rs`/`tests/large_file_rss_bench.rs`.

## Project Structure

```
rust-pdf/
├── src/
│   ├── lib.rs           # Library entry point
│   ├── color/           # Color types (RGB, CMYK, Gray)
│   ├── content/         # Content streams and operators
│   ├── document/        # Document and page structures (creation)
│   ├── editor/          # In-place editing of existing PDFs: pages, forms,
│   │                     annotations, outline, tagged structure, redaction,
│   │                     PDF/A, PDF/UA, PDF/X, ICC/XMP metadata, save
│   ├── encryption/       # AES-256 encryption (write-only)
│   ├── font/             # Standard 14 + embedded TrueType/CID/subsetting
│   ├── forms/            # AcroForm field types (creation-time)
│   ├── image/            # Image embedding
│   ├── object/           # PDF object types
│   ├── page/              # Page builder
│   ├── parser/            # PDF reader (memory-mapped for large files)
│   ├── render/             # Pure-Rust content-stream interpreter + rasterizer
│   │   └── native/          # No native/FFI dependency at all
│   ├── signatures/         # Digital signatures (sign/verify/timestamp)
│   ├── tauri_commands/      # Async Tauri desktop-app command layer
│   ├── types/               # Common types
│   └── writer/               # PDF serialization
├── tests/                     # Integration tests (see "Running Tests")
├── examples/                  # Runnable examples (see below)
├── ARCHITECTURE.md            # Full technical/audit account
└── docs/THREAT_MODEL.md       # Security trust-boundary analysis
```

## Building

```bash
cargo build                       # debug, no optional features
cargo build --release
cargo build --all-features
cargo check
cargo fmt
cargo clippy --all-features --all-targets -- -D warnings
```

## Examples

Runnable, `cargo run`-verified examples in [`examples/`](examples/):

| Example | Features | What it shows |
|---|---|---|
| `forms_example` | `full` | Text fields, checkboxes, radio groups, combo/list boxes, push buttons |
| `render_and_edit_demo` | `parser fonts render` | Outline, tagged structure, redaction, annotation, rendering |
| `form_fill_flatten_demo` | `parser` | Fill an AcroForm, save incrementally, then flatten |
| `cjk_font_demo` | `fonts parser` | Embedded/subsetted CJK composite font + text-extraction round-trip |
| `pdfa_quickstart_demo` | `parser` | Minimal PDF/A-2b conversion + validation |
| `pdfa_convert_demo` | `full` | Fuller PDF/A-1b/2b/3b + PDF/UA-1 + PDF/X sample generation for external (veraPDF) validation |
| `digital_signature_example` | `signatures` | Signing with a certificate/private key, single and multi-signature |
| `encryption_demo` | `encryption` | AES-256 password protection |
| `read_existing_pdf_demo` | `parser` | Read-only `PdfReader` API (page count, version, trailer, catalog) |
| `c_example.c`, `rust_ffi_example/`, `python_example.py`, `node_example.js`, `go_example.go`, `ruby_example.rb` | (dylib) | FFI usage from other languages (see below) |

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
    PdfHandle* pdf = pdf_create_simple("Hello from C!", 24.0);
    pdf_save_to_file(pdf, "output.pdf");
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

### Available C Functions

| Function | Description |
|----------|-------------|
| `pdf_create_simple(text, font_size)` | Create a PDF with text |
| `pdf_get_data(handle, out_data)` | Get PDF bytes (returns length) |
| `pdf_save_to_file(handle, path)` | Save PDF to file (returns 0 on success) |
| `pdf_free(handle)` | Free PDF handle |
| `pdf_version()` | Get library version string |

### Supported Languages

| Language | FFI Method | Example File |
|----------|------------|--------------|
| C/C++ | Native | `examples/c_example.c` |
| Rust | FFI bindings | `examples/rust_ffi_example/` |
| Python | ctypes | `examples/python_example.py` |
| Node.js | ffi-napi | `examples/node_example.js` |
| Go | cgo | `examples/go_example.go` |
| Ruby | ffi gem | `examples/ruby_example.rb` |

Only the small "create a PDF with text and save it" surface is exposed
over the C ABI (`pdf_create_simple`/`pdf_get_data`/`pdf_save_to_file`/
`pdf_free`/`pdf_version`, see [`src/ffi.rs`](src/ffi.rs)) — rendering,
editing, forms, redaction and signing are Rust-only APIs today, not part
of the FFI surface.

## License

MIT

## Contributing

Contributions are welcome! Please ensure:

1. All tests pass: `cargo test --all-features`
2. Code is formatted: `cargo fmt`
3. No clippy warnings: `cargo clippy --all-features --all-targets -- -D warnings`
4. New capabilities are documented in [ARCHITECTURE.md](ARCHITECTURE.md)
   (module inventory, gaps) and, if security-relevant, in
   [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md)
