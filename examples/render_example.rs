//! Demonstrates this crate's pure-Rust page rendering pipeline (the
//! `render`/`native-render` Cargo features, see `src/render/mod.rs`):
//!
//! 1. Build a tiny in-memory PDF, open it with
//!    [`rust_pdf::render::PdfRenderer`], and rasterize page 0 to an
//!    [`rust_pdf::render::RgbaImage`] (a whole-page render, plus a
//!    tile/viewport render for the zoom/pan use case), saving each as a
//!    PNG under `tests/output/`.
//! 2. Show how to inspect `RenderWarning`s (`rust_pdf::render::native`),
//!    the structured, non-panicking way this pure-Rust engine reports a
//!    construct it doesn't fully support -- e.g. a `JBIG2Decode`-filtered
//!    image or a Type1/bare-CFF font program (known, documented gaps; see
//!    ARCHITECTURE.md §8d and `src/render/native/mod.rs`'s module docs).
//!    Note that [`rust_pdf::render::PdfRenderer::render_page`] itself does
//!    *not* surface these warnings (its `Result` is just the image or a
//!    hard [`rust_pdf::error::RenderError`]) -- to see *which* constructs
//!    fell back to a placeholder, call the lower-level
//!    `render::native::render_content_stream` directly against a single
//!    content stream + `/Resources`, as step 2 below does.
//!
//! Run with:
//! ```text
//! cargo run --example render_example --features full,render
//! ```

use rust_pdf::prelude::*;
use rust_pdf::render::native::{render_content_stream, RenderWarning};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    // -----------------------------------------------------------------
    // 1. Whole-document rendering via `PdfRenderer::render_page`.
    // -----------------------------------------------------------------

    // Build a small source document with the plain document-builder API
    // (no pre-existing sample file needed for this example).
    let content = ContentBuilder::new()
        .text("F1", 24.0, 72.0, 720.0, "rust-pdf native renderer demo")
        .text("F1", 12.0, 72.0, 690.0, "Rasterized entirely in pure Rust - no native/FFI dependency.");
    let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(content).build();
    let bytes = DocumentBuilder::new().title("Render Example").page(page).build()?.save_to_bytes()?;

    // `PdfRenderer` opens the document with this crate's own pure-Rust
    // parser/editor, resolves the page's effective /MediaBox, /Rotate and
    // /Resources (ISO 32000-1 Table 30), and rasterizes its content
    // stream(s) via the `native-render` interpreter.
    let renderer = PdfRenderer::open_bytes(bytes)?;
    println!("document has {} page(s)", renderer.page_count());

    // Full page at 150 DPI -> `image::RgbaImage`, saved directly as PNG.
    let page_image = renderer.render_page(0, 150.0, None)?;
    let page_path = "tests/output/render_example_page0.png";
    page_image.save(page_path)?;
    println!("wrote {page_path} ({}x{} px @ 150 DPI)", page_image.width(), page_image.height());

    // A viewport/tile render (the zoom/pan use case a desktop viewer
    // should use instead of holding a full-resolution raster of every
    // page in memory): only the top-left 300x200 device-pixel rectangle
    // of the page rendered at 300 DPI.
    let tile = renderer.render_page(0, 300.0, Some(Viewport::new(0, 0, 300, 200)))?;
    let tile_path = "tests/output/render_example_tile.png";
    tile.save(tile_path)?;
    println!("wrote {tile_path} ({}x{} px tile @ 300 DPI)", tile.width(), tile.height());

    // -----------------------------------------------------------------
    // 2. Inspecting `RenderWarning`s via the lower-level
    //    `render::native::render_content_stream` entry point.
    // -----------------------------------------------------------------
    demo_render_warnings()?;

    Ok(())
}

/// Renders a synthetic content stream containing two constructs this
/// pure-Rust engine deliberately does not fully support -- a `JBIG2Decode`
/// image XObject and a bare-CFF ("Type1C") Type1 font program -- and
/// prints the `RenderWarning`s it records for each. Both fail *closed*:
/// no panic, and a documented placeholder/skip (a flat grey box for the
/// image, no glyph at all for the unrenderable font) rather than a silent
/// or fabricated mis-render.
fn demo_render_warnings() -> Result<(), Box<dyn std::error::Error>> {
    // --- An image XObject declaring JBIG2Decode (ISO 32000-1 7.4.7) ---
    // There is no mature pure-Rust JBIG2 decoder in the ecosystem today,
    // so this filter is a hard, structural gap (see
    // `src/render/native/image.rs`'s module docs), not a "didn't get to
    // it yet" one. The same applies to `JPXDecode` (JPEG2000).
    let mut image_dict = PdfDictionary::new();
    image_dict.set("Type", Object::Name(PdfName::new_unchecked("XObject")));
    image_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Image")));
    image_dict.set("Width", Object::Integer(50));
    image_dict.set("Height", Object::Integer(50));
    image_dict.set("BitsPerComponent", Object::Integer(1));
    image_dict.set("ColorSpace", Object::Name(PdfName::new_unchecked("DeviceGray")));
    image_dict.set("Filter", Object::Name(PdfName::new_unchecked("JBIG2Decode")));
    // The actual bytes are irrelevant/garbage here -- there is no decoder
    // to even attempt running them against.
    let image_stream = PdfStream::with_dictionary(image_dict, vec![0u8; 16]);

    let mut xobjects = PdfDictionary::new();
    xobjects.set("Im1", Object::Stream(image_stream));

    // --- A Type1 font whose only embedded program is a bare (non-`sfnt`
    // -wrapped) CFF stream, which `ttf-parser` (this crate's only
    // font-parsing dependency) cannot parse at all. ---
    let mut font_file = PdfStream::new(b"\x01\x00garbage-not-an-sfnt-container-cff-program".to_vec());
    font_file.dictionary.set("Subtype", Object::Name(PdfName::new_unchecked("Type1C")));
    let mut descriptor = PdfDictionary::new();
    descriptor.set("FontFile3", Object::Stream(font_file));

    let mut font_dict = PdfDictionary::new();
    font_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Type1")));
    font_dict.set("FirstChar", Object::Integer(65));
    let mut widths = PdfArray::new();
    widths.push(Object::Integer(700));
    font_dict.set("Widths", Object::Array(widths));
    font_dict.set("FontDescriptor", Object::Dictionary(descriptor));

    let mut fonts = PdfDictionary::new();
    fonts.set("F1", Object::Dictionary(font_dict));

    let mut resources = PdfDictionary::new();
    resources.set("XObject", Object::Dictionary(xobjects));
    resources.set("Font", Object::Dictionary(fonts));

    // Paint the JBIG2 image, show a (unrenderable) glyph with the Type1
    // font, then paint an unrelated green rectangle -- demonstrating that
    // both gaps fail closed without aborting the rest of the render.
    let content = b"q 100 0 0 100 50 50 cm /Im1 Do Q BT /F1 40 Tf 10 10 Td (A) Tj ET 0 1 0 rg 150 10 30 30 re f";
    let media_box = Rectangle::new(0.0, 0.0, 200.0, 200.0);

    let output = render_content_stream(content, 200, 200, media_box, Some(&resources))?;

    println!("\nlow-level render_content_stream produced {} warning(s):", output.warnings.len());
    for warning in &output.warnings {
        // `RenderWarning` implements `Display` with a human-readable
        // message naming the affected resource and the reason.
        println!("  - {warning}");
        match warning {
            RenderWarning::UnsupportedImageFilter { name, filter } => {
                println!("    (image '{name}': filter '{filter}' has no pure-Rust decoder -> painted as a flat placeholder)");
            }
            RenderWarning::UnsupportedFontProgram { resource_name, .. } => {
                println!("    (font '{resource_name}': program could not be parsed -> glyph skipped, not fabricated)");
            }
            _ => {}
        }
    }

    Ok(())
}
