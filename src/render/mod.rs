//! Page rasterization: renders PDF pages to RGBA raster buffers suitable
//! for display in a desktop viewer (e.g. a Tauri `<canvas>`/GPU texture).
//!
//! This module is gated behind the `render` Cargo feature and is a thin,
//! safety-checked wrapper around Google's Pdfium engine via the
//! [`pdfium-render`](https://docs.rs/pdfium-render) FFI binding. It does
//! **not** contain a content-stream interpreter of its own: rasterization
//! (path fill/stroke, glyph rendering, clipping, transparency groups/blend
//! modes per ISO 32000-1 §11, and color space conversion per ISO 32000-1
//! §8.6) is performed entirely inside the native `libpdfium` binary.
//!
//! # Native vs. FFI rendering decision
//!
//! **Decision: FFI to Pdfium, not a from-scratch Rust rasterizer.**
//!
//! `rust-pdf` already contains a full PDF *object model, parser, and
//! writer*. Rendering, however, is a fundamentally different engineering
//! problem: it requires a content-stream interpreter plus a 2D
//! rasterizer/font-shaping pipeline that correctly reproduces two decades
//! of "PDF producers violate the spec in every possible way" behavior that
//! cannot be derived from ISO 32000 alone. Crates such as `tiny-skia` and
//! `ttf-parser` are **not** PDF parsers — they are a 2D rasterizer and a
//! font-file parser, respectively. Using them would still require this
//! crate to build, from zero, the content-stream interpreter, all
//! ISO 32000 color spaces (§8.6: DeviceGray/RGB/CMYK, CalGray/CalRGB, Lab,
//! ICCBased, Indexed, Separation/DeviceN), every filter needed to decode
//! real-world images (DCT/JPX/JBIG2/CCITT — and the Rust ecosystem has no
//! mature JBIG2 or JPX decoder today, both of which are common in scanned
//! enterprise documents), CID/Type0 font handling with embedded CFF/
//! TrueType/OpenType programs, transparency groups and blend modes
//! (§11), annotation appearance streams (§12.5.5), and AcroForm field
//! appearance generation — i.e. re-implementing the majority of a
//! mature PDF engine. That is a multi-year, multi-person effort (see
//! `ARCHITECTURE.md` §10 for the fuller gap analysis written during the
//! Phase 0 audit of this codebase), and the resulting renderer would still
//! be measured, on day one, against real-world PDF compatibility that
//! Pdfium has spent over a decade and tens of thousands of regression
//! files getting right.
//!
//! Pdfium is the PDF engine used by Google Chrome, so it is exercised at a
//! scale (billions of page views) and against a corpus of real-world PDF
//! producer quirks that no from-scratch Rust implementation could
//! realistically match in this project's timeframe. It has a mature,
//! actively maintained Rust binding (`pdfium-render`) and prebuilt,
//! per-platform binaries are published by `bblanchon/pdfium-binaries` that
//! are straightforward to bundle as a native resource in a Tauri
//! application (see "Bundling in a Tauri app" below).
//!
//! The explicit trade-off accepted by this decision:
//! - **Native binary dependency.** The application must ship a
//!   `libpdfium.{dylib,so,dll}` per target platform/architecture (a few MB
//!   each) instead of being 100% pure Rust. `rust-pdf` itself stays a pure
//!   Rust crate for structure/generation/signing; only the optional
//!   `render` feature pulls in an FFI dependency, and it is not part of
//!   the `full` feature for that reason.
//! - **FFI/crash-level risk** is real (a `panic`-free Rust program can
//!   still segfault if the native library is corrupted, mismatched for
//!   the platform, or hits a Pdfium-side bug) but is minimized by the fact
//!   that Pdfium is continuously fuzzed by Google/Chromium's fuzzing
//!   infrastructure — far beyond what this project could do for a
//!   from-scratch renderer in this phase.
//! - **We do not control Pdfium's internal correctness.** Bugs in Pdfium
//!   itself cannot be fixed in this repository; only worked around or
//!   reported upstream. This module's job is to (a) load the library
//!   safely, (b) validate all caller/file-derived inputs before handing
//!   them to the FFI boundary (see "Untrusted input handling" below), and
//!   (c) convert results into `image::RgbaImage` with clear error types —
//!   not to second-guess Pdfium's rasterization itself.
//!
//! ## Bundling in a Tauri app
//!
//! `pdfium-render` loads Pdfium *dynamically at run time* (via
//! `libloading`), not via static linking, so there is no special build-time
//! toolchain requirement for `rust-pdf` itself. A Tauri application:
//! 1. Downloads the platform-appropriate archive from
//!    <https://github.com/bblanchon/pdfium-binaries/releases> (this repo's
//!    `scripts/fetch_pdfium.sh` does this for local development/testing).
//! 2. Ships the shared library as a Tauri "resource" or "external binary"
//!    for each target triple, per Tauri's bundling docs.
//! 3. At startup, calls [`PdfiumLibrary::bind_from_path`] (or sets
//!    `RUST_PDF_PDFIUM_LIB_DIR` and calls [`PdfiumLibrary::bind`]) pointing
//!    at the resolved resource directory before constructing any
//!    [`PdfRenderer`].
//!
//! ## Known limitation: tile rendering re-renders the full page
//!
//! [`PdfRenderer::render_page`]'s tiled/`Viewport` path currently renders
//! the *whole* page at the requested DPI internally and then crops the
//! requested rectangle out of that raster, rather than asking Pdfium to
//! rasterize only the requested tile via a custom device-space transform
//! (`FPDF_RenderPageBitmapWithMatrix`). This was a deliberate choice for
//! this phase: it reuses `pdfium-render`'s default, most-exercised
//! rendering path (which is known to correctly honor page rotation), and
//! this crate could not, from the ISO 32000 spec alone, confirm exactly
//! how a hand-rolled transform matrix interacts with Pdfium's internal
//! page-rotation handling — getting that wrong would silently produce
//! *visually incorrect* tiles, which is worse than the current, slightly
//! less memory-efficient behavior. Practically this means memory use for
//! tiled rendering is still bounded by the *full page* at the requested
//! DPI (checked against [`MAX_RENDER_PIXELS`]), not by the tile size.
//! Implementing genuine partial-bitmap tiling (bounded by tile size, not
//! full-page size, so very high zoom levels on very large pages stay
//! cheap) is a reasonable follow-up, estimated at 1-3 engineer-days
//! including empirical verification against rotated real-world PDFs.
//!
//! ## Untrusted input handling
//!
//! Per this crate's mandatory rules, page geometry read from a PDF file
//! (`/MediaBox`, ISO 32000-1 §7.7.3.3) is untrusted: a crafted file could
//! declare an enormous page combined with a caller-requested DPI to try to
//! force an unbounded allocation (a decompression-bomb-style attack, just
//! against the *rendering* memory budget instead of a stream filter). This
//! module always computes the target raster's pixel count up front and
//! rejects renders exceeding [`MAX_RENDER_PIXELS`] with
//! [`crate::error::RenderError::OutputTooLarge`] *before* asking Pdfium to
//! allocate a bitmap, for both full-page and tiled/viewport renders.
//!
//! # Example
//!
//! ```no_run
//! # #[cfg(feature = "render")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use rust_pdf::render::{PdfRenderer, PdfiumLibrary, Viewport};
//!
//! // Bind the native Pdfium library once per process (share this across
//! // every document/renderer you open).
//! let library = PdfiumLibrary::bind()?;
//!
//! let renderer = PdfRenderer::open_file(&library, "example.pdf", None)?;
//!
//! // Render page 0 at 150 DPI, full page.
//! let page_image = renderer.render_page(0, 150.0, None)?;
//! page_image.save("page-0.png")?;
//!
//! // Render only the top-left 512x512 device-pixel tile (zoom/pan use case).
//! let tile = renderer.render_page(0, 300.0, Some(Viewport::new(0, 0, 512, 512)))?;
//! tile.save("page-0-tile.png")?;
//!
//! // Cached thumbnail generation for a page list/grid UI.
//! let thumb = renderer.render_thumbnail(0, 128)?;
//! thumb.save("page-0-thumb.png")?;
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "render"))]
//! # fn main() {}
//! ```

mod cache;
mod renderer;

pub use renderer::{PdfRenderer, PdfiumLibrary};

/// Rendered page/tile output.
///
/// Re-exports the `image` crate's `RgbaImage` (an 8-bit-per-channel,
/// non-premultiplied `ImageBuffer<Rgba<u8>, Vec<u8>>`) so callers can use it
/// directly with `image`'s encoders (`RgbaImage::save(...)`, PNG/etc.), or
/// read `.as_raw()` to hand the pixel buffer straight to a GPU texture
/// upload. This is the *same* `image` crate version `pdfium-render` itself
/// uses internally (both depend on `image = "0.25"`), so no extra
/// conversion or copy is needed at the FFI boundary.
pub type RgbaImage = image::RgbaImage;

/// The maximum number of pixels (`width * height`) a single
/// [`PdfRenderer::render_page`] or [`PdfRenderer::render_thumbnail`] call
/// will allocate a raster for.
///
/// `64_000_000` px is about 256 MiB as RGBA8 (e.g. an 8000x8000 px page) —
/// generous for desktop print-quality zoom levels, while still bounding
/// worst-case memory use against a malicious `/MediaBox` combined with a
/// large caller-requested DPI. See the "Untrusted input handling" section
/// of the module documentation.
pub const MAX_RENDER_PIXELS: u64 = 64_000_000;

/// A device-pixel sub-rectangle of a page rendered at a given DPI.
///
/// Passing `Some(Viewport { .. })` to [`PdfRenderer::render_page`] requests
/// only that rectangle of the page-at-that-DPI raster (a "tile"), which is
/// how zoom/pan should be implemented in a desktop viewer: render the
/// tiles that are actually visible in the current scroll/zoom window
/// rather than the whole page at maximum zoom. Passing `None` renders the
/// whole page.
///
/// Coordinates are in device pixels with the origin at the page's
/// top-left corner (matching how raster image coordinates, and Pdfium's
/// own bitmap coordinate system, are conventionally addressed) — *not*
/// PDF user-space points, whose origin is bottom-left (ISO 32000-1
/// §8.3.2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    /// Left offset of the viewport, in device pixels from the page's
    /// left edge.
    pub x: u32,
    /// Top offset of the viewport, in device pixels from the page's top
    /// edge.
    pub y: u32,
    /// Viewport width in device pixels.
    pub width: u32,
    /// Viewport height in device pixels.
    pub height: u32,
}

impl Viewport {
    /// Creates a new [`Viewport`] rectangle.
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_new_sets_all_fields() {
        let vp = Viewport::new(10, 20, 300, 400);
        assert_eq!(vp.x, 10);
        assert_eq!(vp.y, 20);
        assert_eq!(vp.width, 300);
        assert_eq!(vp.height, 400);
    }

    // Regression guard, checked at compile time: this constant must be
    // nonzero and stay well below `u32::MAX * u32::MAX` so that
    // width*height computed in u64 (see
    // `renderer::PdfRenderer::check_dimensions`) never silently wraps.
    const _: () = assert!(MAX_RENDER_PIXELS > 0);
    const _: () = assert!(MAX_RENDER_PIXELS < (u32::MAX as u64) * (u32::MAX as u64));
}
