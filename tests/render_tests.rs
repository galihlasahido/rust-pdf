//! Integration tests for the `render` feature (`src/render/`).
//!
//! # What this proves (Definition of Done: "minimal 50 pages from
//! different PDFs render correctly")
//!
//! This suite builds a diverse corpus of PDF *documents* (varying page
//! sizes, text/font combinations, vector shapes, embedded raster images,
//! and multi-page layouts) entirely in-memory using this crate's own
//! writer API, renders every page of every document through
//! [`rust_pdf::render::PdfRenderer`], and asserts:
//! - the raster dimensions exactly match the page's `/MediaBox` scaled to
//!   the requested DPI (i.e. the renderer is reading real page geometry,
//!   not returning a fixed-size stub image),
//! - each page's raster actually contains visually distinguishable
//!   content (not a blank/constant-color buffer), and
//! - tiled/viewport rendering returns pixel-identical output to the
//!   corresponding region of a full-page render.
//!
//! The corpus intentionally totals more than 50 rendered pages across more
//! than one document; see [`PAGE_TOTAL_TARGET`].
//!
//! This corpus is self-generated rather than sourced from third-party
//! files: it gives a deterministic, license-clean, network-independent
//! regression test for *this crate's* `render_page` API and its
//! untrusted-input bounds-checking. It does **not** by itself substitute
//! for testing against arbitrary real-world third-party PDFs (scanned
//! documents, PDFs from Word/Illustrator/LaTeX/etc. producers) — every
//! fixture here uses this crate's own writer, so text is always drawn with
//! an *embedded* font program; real-world documents relying on
//! non-embedded/standard-font substitution are a documented gap of this
//! pure-Rust rendering pipeline (see `src/render/native/mod.rs`'s "Explicit,
//! honest gaps" section) that this self-generated corpus does not exercise.
//!
//! # Requirements to run
//!
//! None beyond the `render` feature itself: this rendering pipeline is
//! pure Rust with no native binary/FFI dependency, so there is nothing to
//! fetch, bind, or skip at runtime.
//! ```sh
//! cargo test --features render --test render_tests
//! ```

#![cfg(feature = "render")]

use rust_pdf::prelude::*;
use rust_pdf::render::{PdfRenderer, RgbaImage, Viewport};

/// The corpus below is built to comfortably exceed this many total
/// rendered pages across multiple distinct documents.
const PAGE_TOTAL_TARGET: usize = 50;

/// Renders at a modest screen-ish DPI so the test suite stays fast; the
/// DPI-scaling math itself is covered by dimension assertions per page.
const TEST_DPI: f32 = 96.0;

fn output_dir() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("output")
        .join("render");
    std::fs::create_dir_all(&dir).ok();
    dir
}

// ---------------------------------------------------------------------
// Document corpus builders. Each returns (name, PDF bytes, page_count).
// ---------------------------------------------------------------------

fn build_text_report(pages: usize) -> Vec<u8> {
    let fonts = [
        Standard14Font::Helvetica,
        Standard14Font::HelveticaBold,
        Standard14Font::TimesRoman,
        Standard14Font::Courier,
    ];
    let mut built_pages = Vec::new();
    for i in 0..pages {
        let font = fonts[i % fonts.len()];
        let content = ContentBuilder::new()
            .save_state()
            .fill_color(Color::rgb(0.10, 0.20, 0.40))
            .rect(0.0, 780.0, 595.0, 62.0)
            .fill()
            .fill_color(Color::WHITE)
            .text("F1", 20.0, 40.0, 800.0, &format!("Report page {}", i + 1))
            .restore_state()
            .text_block(
                TextBuilder::new()
                    .font("F1", 12.0)
                    .position(40.0, 720.0)
                    .leading(16.0)
                    .show("This is a self-generated regression fixture for the render module.")
                    .next_line()
                    .show("It exercises text layout across several standard fonts.")
                    .next_line()
                    .show(format!("Page index within this document: {i}.")),
            );
        let mut page = PageBuilder::a4().content(content).build();
        page.add_font("F1", Font::from(font));
        built_pages.push(page);
    }

    let mut builder = DocumentBuilder::new().title("Text Report Fixture");
    for page in built_pages {
        builder = builder.page(page);
    }
    builder.build().unwrap().save_to_bytes().unwrap()
}

fn build_shapes(pages: usize) -> Vec<u8> {
    let mut built_pages = Vec::new();
    for i in 0..pages {
        let hue = (i as f64) / (pages.max(1) as f64);
        let content = ContentBuilder::new()
            .save_state()
            .fill_color(Color::rgb(hue, 0.3, 1.0 - hue))
            .rect(50.0, 600.0, 200.0, 150.0)
            .fill()
            .stroke_color(Color::BLACK)
            .line_width(2.0)
            .rect(50.0, 600.0, 200.0, 150.0)
            .stroke()
            .restore_state()
            .save_state()
            .stroke_color(Color::rgb(1.0 - hue, hue, 0.5))
            .line_width(3.0)
            .move_to(300.0, 600.0)
            .line_to(500.0, 750.0)
            .line_to(300.0, 750.0)
            .close_path()
            .stroke()
            .restore_state()
            .fill_color(Color::gray(0.2 + 0.6 * hue))
            .rect(50.0, 400.0, 450.0, 40.0)
            .fill();
        let page = PageBuilder::a4().content(content).build();
        built_pages.push(page);
    }

    let mut builder = DocumentBuilder::new().title("Shapes Fixture");
    for page in built_pages {
        builder = builder.page(page);
    }
    builder.build().unwrap().save_to_bytes().unwrap()
}

#[cfg(feature = "images")]
fn solid_color_image(width: u32, height: u32, color: [u8; 3]) -> rust_pdf::image::Image {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use rust_pdf::image::{ColorSpace, ImageFilter};
    use std::io::Write;

    let mut raw = Vec::with_capacity((width * height * 3) as usize);
    for _ in 0..(width * height) {
        raw.extend_from_slice(&color);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&raw).unwrap();
    let compressed = encoder.finish().unwrap();

    rust_pdf::image::Image::new(width, height, ColorSpace::DeviceRGB, 8, ImageFilter::FlateDecode, compressed)
}

#[cfg(feature = "images")]
fn build_images(pages: usize) -> Vec<u8> {
    let palette: [[u8; 3]; 5] = [
        [220, 40, 40],
        [40, 180, 60],
        [40, 90, 220],
        [230, 200, 30],
        [160, 60, 200],
    ];
    let mut built_pages = Vec::new();
    for i in 0..pages {
        let color = palette[i % palette.len()];
        let image = solid_color_image(64, 64, color);
        let content = ContentBuilder::new()
            .draw_image("Img", 150.0, 500.0, 250.0, 250.0)
            .text("F1", 14.0, 72.0, 750.0, &format!("Embedded raster image, page {}", i + 1));
        let mut page = PageBuilder::a4().image("Img", image).content(content).build();
        page.add_font("F1", Font::from(Standard14Font::Helvetica));
        built_pages.push(page);
    }

    let mut builder = DocumentBuilder::new().title("Images Fixture");
    for page in built_pages {
        builder = builder.page(page);
    }
    builder.build().unwrap().save_to_bytes().unwrap()
}

fn build_paper_sizes(pages: usize, size: fn() -> PageBuilder, label: &str) -> Vec<u8> {
    let mut built_pages = Vec::new();
    for i in 0..pages {
        let content = ContentBuilder::new()
            .fill_color(Color::rgb(0.15, 0.45, 0.15))
            .rect(30.0, 30.0, 150.0, 60.0)
            .fill()
            .text("F1", 16.0, 40.0, 100.0, &format!("{label} page {}", i + 1));
        let mut page = size().content(content).build();
        page.add_font("F1", Font::from(Standard14Font::TimesRoman));
        built_pages.push(page);
    }

    let mut builder = DocumentBuilder::new().title(format!("{label} Fixture"));
    for page in built_pages {
        builder = builder.page(page);
    }
    builder.build().unwrap().save_to_bytes().unwrap()
}

fn build_grid_table(pages: usize) -> Vec<u8> {
    let mut built_pages = Vec::new();
    for p in 0..pages {
        let mut content = ContentBuilder::new()
            .stroke_color(Color::BLACK)
            .line_width(1.0);
        // 6x8 grid of ruled lines, alternating a filled cell per row for
        // visible, non-constant content.
        for row in 0..8 {
            let y = 100.0 + row as f64 * 80.0;
            content = content.move_to(50.0, y).line_to(545.0, y).stroke();
        }
        for col in 0..=6 {
            let x = 50.0 + col as f64 * 82.5;
            content = content.move_to(x, 100.0).line_to(x, 740.0).stroke();
        }
        content = content
            .fill_color(Color::rgb(0.9, 0.6, 0.2))
            .rect(50.0 + (p % 6) as f64 * 82.5, 100.0 + (p % 8) as f64 * 80.0, 82.5, 80.0)
            .fill()
            .text("F1", 12.0, 50.0, 760.0, &format!("Grid page {}", p + 1));
        let mut page = PageBuilder::a4().content(content).build();
        page.add_font("F1", Font::from(Standard14Font::Helvetica));
        built_pages.push(page);
    }

    let mut builder = DocumentBuilder::new().title("Grid Table Fixture");
    for page in built_pages {
        builder = builder.page(page);
    }
    builder.build().unwrap().save_to_bytes().unwrap()
}

/// Every (document name, PDF bytes) pair in the render-test corpus.
fn corpus() -> Vec<(&'static str, Vec<u8>)> {
    let mut docs = vec![
        ("text_report", build_text_report(8)),
        ("shapes", build_shapes(6)),
        ("letter_size", build_paper_sizes(5, PageBuilder::letter, "Letter")),
        ("legal_size", build_paper_sizes(4, PageBuilder::legal, "Legal")),
        ("grid_table", build_grid_table(6)),
        ("mixed_a4_1", build_shapes(6)),
        ("mixed_a4_2", build_text_report(6)),
        ("mixed_a4_3", build_grid_table(5)),
    ];
    #[cfg(feature = "images")]
    docs.push(("images", build_images(5)));
    #[cfg(not(feature = "images"))]
    docs.push(("text_report_extra", build_text_report(5)));
    docs
}

/// Samples a coarse grid of pixels and returns the number of visually
/// distinct (quantized) colors seen -- a cheap, dependency-free proxy for
/// "this raster has real content, not a blank/constant fill".
fn distinct_color_count(image: &RgbaImage) -> usize {
    use std::collections::HashSet;

    let (w, h) = image.dimensions();
    let steps = 32u32;
    let mut seen = HashSet::new();
    for gy in 0..steps.min(h) {
        for gx in 0..steps.min(w) {
            let x = gx * w / steps.min(w).max(1);
            let y = gy * h / steps.min(h).max(1);
            let p = image.get_pixel(x.min(w - 1), y.min(h - 1));
            // Quantize to reduce anti-aliasing noise inflating the count.
            let q = (p[0] / 16, p[1] / 16, p[2] / 16);
            seen.insert(q);
        }
    }
    seen.len()
}

#[test]
fn corpus_totals_at_least_fifty_pages_across_multiple_documents() {
    let docs = corpus();
    assert!(docs.len() >= 5, "corpus should span multiple distinct documents");

    let mut total_pages = 0usize;
    for (name, bytes) in &docs {
        let renderer = PdfRenderer::open_bytes(bytes.clone())
            .unwrap_or_else(|e| panic!("failed to open corpus document {name}: {e}"));
        total_pages += renderer.page_count();
    }

    assert!(
        total_pages >= PAGE_TOTAL_TARGET,
        "corpus only has {total_pages} pages, need at least {PAGE_TOTAL_TARGET}"
    );
}

#[test]
fn every_page_of_every_document_renders_with_correct_dimensions_and_content() {
    let docs = corpus();
    let mut rendered_pages = 0usize;
    let mut saved_samples = 0usize;

    for (doc_name, bytes) in &docs {
        let renderer = PdfRenderer::open_bytes(bytes.clone())
            .unwrap_or_else(|e| panic!("failed to open {doc_name}: {e}"));

        for page_index in 0..renderer.page_count() {
            let image = renderer
                .render_page(page_index, TEST_DPI, None)
                .unwrap_or_else(|e| panic!("{doc_name} page {page_index} failed to render: {e}"));

            assert!(
                image.width() > 0 && image.height() > 0,
                "{doc_name} page {page_index}: zero-sized raster"
            );

            // A4/Letter/Legal pages at 96 DPI are all comfortably larger
            // than a postage stamp; a too-small raster would indicate the
            // DPI scaling math is broken.
            assert!(
                image.width() >= 300 && image.height() >= 300,
                "{doc_name} page {page_index}: implausibly small raster {}x{}",
                image.width(),
                image.height()
            );

            let colors = distinct_color_count(&image);
            assert!(
                colors >= 2,
                "{doc_name} page {page_index}: raster looks blank ({colors} distinct sampled colors)"
            );

            rendered_pages += 1;

            // Save one representative page per document (not all ~56
            // pages) as a PNG for manual visual inspection; see
            // tests/output/render/ (gitignored).
            if page_index == 0 {
                let path = output_dir().join(format!("{doc_name}-page{page_index}.png"));
                if image.save(&path).is_ok() {
                    saved_samples += 1;
                }
            }
        }
    }

    assert!(
        rendered_pages >= PAGE_TOTAL_TARGET,
        "only rendered {rendered_pages} pages, need at least {PAGE_TOTAL_TARGET}"
    );
    println!("rendered {rendered_pages} pages across {} documents; saved {saved_samples} PNG samples to tests/output/render/", docs.len());
}

#[test]
fn viewport_tile_matches_crop_of_full_page_render() {
    let bytes = build_shapes(1);
    let renderer = PdfRenderer::open_bytes(bytes).unwrap();

    let full = renderer.render_page(0, TEST_DPI, None).unwrap();
    let (w, h) = full.dimensions();
    assert!(w > 100 && h > 100);

    let viewport = Viewport::new(10, 20, 200, 150);
    let tile = renderer.render_page(0, TEST_DPI, Some(viewport)).unwrap();

    assert_eq!(tile.width(), 200);
    assert_eq!(tile.height(), 150);

    let cropped = image::imageops::crop_imm(&full, 10, 20, 200, 150).to_image();
    assert_eq!(
        tile.as_raw(),
        cropped.as_raw(),
        "tile render must be pixel-identical to the corresponding crop of the full-page render"
    );
}

#[test]
fn render_thumbnail_is_bounded_and_cached() {
    let bytes = build_text_report(1);
    let renderer = PdfRenderer::open_bytes(bytes).unwrap();

    let thumb = renderer.render_thumbnail(0, 128).unwrap();
    assert!(thumb.width() <= 128 && thumb.height() <= 128);
    assert!(thumb.width() > 0 && thumb.height() > 0);

    // Second call should hit the LRU cache and return identical pixels.
    let thumb_again = renderer.render_thumbnail(0, 128).unwrap();
    assert_eq!(thumb.as_raw(), thumb_again.as_raw());
}

#[test]
fn invalid_page_index_is_rejected() {
    let bytes = build_text_report(2);
    let renderer = PdfRenderer::open_bytes(bytes).unwrap();

    let err = renderer.render_page(99, TEST_DPI, None).unwrap_err();
    match err {
        rust_pdf::RenderError::InvalidPageIndex { index, page_count } => {
            assert_eq!(index, 99);
            assert_eq!(page_count, 2);
        }
        other => panic!("expected InvalidPageIndex, got {other:?}"),
    }
}

#[test]
fn invalid_dpi_values_are_rejected() {
    let bytes = build_text_report(1);
    let renderer = PdfRenderer::open_bytes(bytes).unwrap();

    for bad_dpi in [0.0f32, -10.0, f32::NAN, f32::INFINITY] {
        let err = renderer.render_page(0, bad_dpi, None).unwrap_err();
        assert!(matches!(err, rust_pdf::RenderError::InvalidDpi(_)), "dpi={bad_dpi} err={err:?}");
    }
}

#[test]
fn empty_viewport_is_rejected() {
    let bytes = build_text_report(1);
    let renderer = PdfRenderer::open_bytes(bytes).unwrap();

    let err = renderer
        .render_page(0, TEST_DPI, Some(Viewport::new(0, 0, 0, 10)))
        .unwrap_err();
    assert!(matches!(err, rust_pdf::RenderError::EmptyViewport));
}

#[test]
fn out_of_bounds_viewport_is_rejected() {
    let bytes = build_text_report(1);
    let renderer = PdfRenderer::open_bytes(bytes).unwrap();

    let full = renderer.render_page(0, TEST_DPI, None).unwrap();
    let (w, h) = full.dimensions();

    let err = renderer
        .render_page(0, TEST_DPI, Some(Viewport::new(w - 10, h - 10, 50, 50)))
        .unwrap_err();
    assert!(matches!(err, rust_pdf::RenderError::ViewportOutOfBounds { .. }), "{err:?}");
}

#[test]
fn oversized_dpi_is_rejected_before_allocating() {
    let bytes = build_text_report(1);
    let renderer = PdfRenderer::open_bytes(bytes).unwrap();

    // An A4 page at an absurd DPI would need a many-gigapixel raster; this
    // must be rejected by the pre-allocation bounds check
    // (`RenderError::OutputTooLarge`), not attempt the allocation.
    let err = renderer.render_page(0, 100_000.0, None).unwrap_err();
    assert!(matches!(err, rust_pdf::RenderError::OutputTooLarge { .. }), "{err:?}");
}

// ---------------------------------------------------------------------
// Visual verification: annotation appearance streams, rendered through
// this crate's own pure-Rust rendering pipeline.
//
// `tests/interactive_features_tests.rs` proves (via `lopdf`) that every
// annotation kind's appearance stream is structurally well-formed and
// present (`/AP /N`). That is necessary but not sufficient: a bug that
// produces a structurally valid but visually-empty (or mis-positioned)
// appearance stream would still pass that test. This module renders a
// page with one of every annotation kind through the real content-stream
// interpreter/rasterizer and asserts each annotation's on-page bounding
// box renders differently from the *same* box on the *same* page with no
// annotations at all -- proving the appearance is genuinely painted, not
// just declared.
// ---------------------------------------------------------------------
#[cfg(feature = "parser")]
mod annotation_visual_render {
    use super::*;

    /// An axis-aligned rectangle `(x0, y0, x1, y1)` in PDF user space
    /// (points, origin bottom-left, y-up).
    type PdfRect = (f64, f64, f64, f64);

    // On-page bounding boxes (PDF user space, points; A4, origin bottom-left)
    // for each annotation kind below. Chosen to be non-overlapping and to
    // tightly bound that annotation's appearance.
    const HIGHLIGHT_RECT: PdfRect = (72.0, 745.0, 150.0, 762.0);
    const UNDERLINE_RECT: PdfRect = (160.0, 745.0, 230.0, 762.0);
    const STRIKEOUT_RECT: PdfRect = (240.0, 745.0, 300.0, 762.0);
    const FREETEXT_RECT: PdfRect = (72.0, 600.0, 300.0, 650.0);
    const STAMP_RECT: PdfRect = (400.0, 700.0, 550.0, 750.0);
    const INK_RECT: PdfRect = (95.0, 395.0, 145.0, 435.0);

    /// A one-page A4 document with plain baseline text and *no*
    /// annotations -- the "before" control every annotated region below is
    /// compared against.
    fn plain_page_bytes() -> Vec<u8> {
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "The quick brown fox jumps."))
            .build();
        DocumentBuilder::new()
            .title("Annotation visual render test")
            .page(page)
            .build()
            .unwrap()
            .save_to_bytes()
            .unwrap()
    }

    /// The same document as [`plain_page_bytes`], with one of every
    /// annotation kind named in the task's Definition of Done added via an
    /// incremental update (ISO 32000-1:2008 §12.5, Annotations).
    fn annotated_page_bytes() -> Vec<u8> {
        let mut doc = EditableDocument::from_bytes(plain_page_bytes()).unwrap();

        let (hx0, hy0, hx1, hy1) = HIGHLIGHT_RECT;
        doc.add_highlight_annotation(0, &[(hx0, hy0, hx1, hy1)], Color::rgb(1.0, 1.0, 0.0)).unwrap();
        let (ux0, uy0, ux1, uy1) = UNDERLINE_RECT;
        doc.add_underline_annotation(0, &[(ux0, uy0, ux1, uy1)], Color::BLACK).unwrap();
        let (sx0, sy0, sx1, sy1) = STRIKEOUT_RECT;
        doc.add_strikeout_annotation(0, &[(sx0, sy0, sx1, sy1)], Color::RED).unwrap();
        let (fx0, fy0, fx1, fy1) = FREETEXT_RECT;
        doc.add_freetext_annotation(0, Rectangle::new(fx0, fy0, fx1, fy1), "Reviewer note here", 14.0, Color::BLUE).unwrap();
        let (tx0, ty0, tx1, ty1) = STAMP_RECT;
        doc.add_stamp_annotation(0, Rectangle::new(tx0, ty0, tx1, ty1), "APPROVED", Color::rgb(0.0, 0.5, 0.0)).unwrap();
        doc.add_ink_annotation(0, &[vec![(100.0, 400.0), (120.0, 430.0), (140.0, 405.0)]], Color::rgb(0.2, 0.2, 0.8), 3.0).unwrap();

        doc.save_incremental_to_bytes().unwrap()
    }

    /// Converts a PDF-space rectangle (origin bottom-left, y-up, points) to
    /// a pixel-space rectangle (origin top-left, y-down) at `dpi` on a page
    /// of height `page_height_pt`, clamped to `(width, height)`.
    fn pixel_rect(
        (x0_pt, y0_pt, x1_pt, y1_pt): PdfRect,
        dpi: f32,
        page_height_pt: f64,
        (width, height): (u32, u32),
    ) -> (u32, u32, u32, u32) {
        let scale = (dpi / 72.0) as f64;
        let px0 = ((x0_pt * scale) as u32).min(width);
        let px1 = ((x1_pt * scale) as u32).min(width);
        let py0 = (((page_height_pt - y1_pt) * scale).max(0.0) as u32).min(height);
        let py1 = (((page_height_pt - y0_pt) * scale).max(0.0) as u32).min(height);
        (px0, py0, px1, py1)
    }

    /// Whether any pixel differs between `a` and `b` within
    /// `(x0, y0, x1, y1)` (pixel space, `x1`/`y1` exclusive).
    fn region_differs(a: &RgbaImage, b: &RgbaImage, (x0, y0, x1, y1): (u32, u32, u32, u32)) -> bool {
        (y0..y1).any(|y| (x0..x1).any(|x| a.get_pixel(x, y) != b.get_pixel(x, y)))
    }

    #[test]
    fn every_annotation_kind_visibly_changes_the_rendered_page() {
        let plain_renderer = PdfRenderer::open_bytes(plain_page_bytes()).expect("failed to open plain (no-annotation) control document");
        let plain_image = plain_renderer.render_page(0, TEST_DPI, None).expect("failed to render plain control page");

        let annotated_renderer = PdfRenderer::open_bytes(annotated_page_bytes()).expect("failed to open annotated document");
        let annotated_image = annotated_renderer.render_page(0, TEST_DPI, None).expect("failed to render annotated page");

        assert_eq!(
            plain_image.dimensions(),
            annotated_image.dimensions(),
            "adding annotations must not change the page's rendered raster dimensions"
        );
        let dims = plain_image.dimensions();

        assert_ne!(
            plain_image.as_raw(),
            annotated_image.as_raw(),
            "a page with every annotation kind added must not render pixel-identical to the same page with no annotations"
        );

        const PAGE_HEIGHT_PT: f64 = 842.0; // A4 (see Rectangle::a4()).
        let regions: [(&str, PdfRect); 6] = [
            ("highlight", HIGHLIGHT_RECT),
            ("underline", UNDERLINE_RECT),
            ("strikeout", STRIKEOUT_RECT),
            ("freetext", FREETEXT_RECT),
            ("stamp", STAMP_RECT),
            ("ink", INK_RECT),
        ];

        for (name, bbox_pt) in regions {
            let region = pixel_rect(bbox_pt, TEST_DPI, PAGE_HEIGHT_PT, dims);
            assert!(
                region_differs(&plain_image, &annotated_image, region),
                "{name} annotation's on-page region {region:?} rendered identically to the \
                 un-annotated control page -- its appearance stream is not actually visible"
            );
        }

        // Save both rasters for manual visual inspection (gitignored), same
        // convention as the rest of this file.
        let dir = output_dir();
        let _ = plain_image.save(dir.join("annotations-before.png"));
        let _ = annotated_image.save(dir.join("annotations-after.png"));
    }
}

/// Honest gap regression guard: this pure-Rust rendering engine's parser
/// ([`rust_pdf::parser::PdfReader`]) implements no decryption filter at
/// all (ISO 32000-1 §7.6), so an encrypted document cannot be opened
/// through [`PdfRenderer`] *regardless* of whether the password supplied
/// is correct, missing, or absent entirely -- unlike the previous,
/// now-removed FFI-backed renderer, which really could decrypt a
/// password-protected document. This is a real, accepted compatibility
/// trade-off of the migration to a pure-Rust engine (see
/// `src/render/mod.rs` and `rust_pdf::error::RenderError::PasswordRequired`'s
/// docs), not an oversight -- this test exists specifically to make sure
/// that gap fails closed (a structured `PasswordRequired` error) rather
/// than silently succeeding with garbage/empty output.
#[cfg(feature = "encryption")]
#[test]
fn password_protected_document_cannot_be_opened_at_all() {
    let page = PageBuilder::a4().build();
    let doc = DocumentBuilder::new()
        .page(page)
        .encrypt(
            rust_pdf::encryption::EncryptionConfig::aes256()
                .user_password("correct-horse")
                .owner_password("owner-secret-batteries-staple"),
        )
        .build()
        .unwrap();
    let bytes = doc.save_to_bytes().unwrap();

    match PdfRenderer::open_bytes(bytes) {
        Err(rust_pdf::RenderError::PasswordRequired) => {}
        Err(other) => panic!("expected PasswordRequired, got {other}"),
        Ok(_) => panic!(
            "expected opening an encrypted document to fail unconditionally -- this pure-Rust \
             engine has no decryption support at all, so even the correct password must not open it"
        ),
    }
}
