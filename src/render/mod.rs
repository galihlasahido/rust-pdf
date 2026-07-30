//! Page rasterization: renders PDF pages to raster buffers suitable for
//! display in a desktop viewer (e.g. a Tauri `<canvas>`/GPU texture).
//!
//! This module is a **from-scratch, pure-Rust rendering pipeline with no
//! native binary or FFI dependency at all**, built from two layers behind
//! two Cargo features:
//!
//! - `native-render`: [`native`], the content-stream interpreter and 2D
//!   rasterizer (ISO 32000-1:2008 Chapter 8/9/11 operators, backed by
//!   `tiny-skia` for rasterization and `ttf-parser` for font outlines).
//!   Operates on a single already-decoded content stream plus its
//!   `/Resources` -- it has no PDF *document* (page tree, xref) access of
//!   its own. See [`native`]'s module docs for exactly what operator/
//!   color-space/font/filter coverage this phase implements, and its
//!   explicitly-documented, honestly-labeled gaps (JBIG2/JPX images,
//!   Type1/bare-CFF font programs, true ICC color management, Patterns/
//!   shadings, and more -- every one of these fails closed with a
//!   structured, non-panicking warning/placeholder, never a silent
//!   mis-render).
//! - `render`: [`PdfRenderer`], the whole-document API built on top of
//!   `native-render` and this crate's own structural parser/editor
//!   ([`crate::editor::EditableDocument`]): given a page index, it
//!   resolves the *effective* `/MediaBox`/`/Rotate`/`/Resources`/content
//!   streams (following `/Parent` inheritance per ISO 32000-1 Table 30),
//!   hands the content stream to [`native::render_content_stream`], and
//!   applies the page's rotation and any requested tile/viewport crop.
//!
//! # Migration history: this module previously used a native/FFI engine
//!
//! An earlier phase of this crate rendered pages by binding, via FFI, to a
//! mature, widely-deployed third-party C/C++ PDF-rendering engine -- a
//! deliberate trade-off at the time, favoring that engine's real-world PDF
//! compatibility (built from over a decade of fuzzing/regression testing)
//! over the multi-year effort of writing a from-scratch content-stream
//! interpreter and rasterizer. That trade-off required bundling a native,
//! per-platform shared library at application-packaging time and
//! serializing every render call behind a process-wide lock, since that
//! native library's C API was not safe to call concurrently. (See this
//! project's `ARCHITECTURE.md` and version-control history for the full,
//! named account of that earlier design and the migration away from it.)
//!
//! This module has since been **fully migrated off that FFI dependency**:
//! `rust-pdf` (including this `render`/`native-render` pair of features)
//! is now 100% pure Rust, with no native binary to bundle, load, or
//! version-match at deployment time. This was an explicit, accepted
//! trade-off in the *other* direction -- real-world compatibility gaps
//! (see [`native`]'s module docs: JBIG2/JPX images, Type1/bare-CFF font
//! programs, non-embedded/system font substitution, true ICC color
//! management, Patterns/shadings) and likely slower rasterization than a
//! decade-matured C++ engine, in exchange for a dependency-light, fully
//! auditable, memory-safe rendering pipeline with no FFI/crash-level risk
//! and no per-platform native binary to bundle. Every one of those gaps
//! fails closed (a structured, documented warning or placeholder) rather
//! than silently mis-rendering or panicking -- see [`native`]'s "Explicit,
//! honest gaps" section for the exhaustive list.
//!
//! One concrete, welcome consequence of this migration: because
//! [`PdfRenderer`] now wraps [`crate::editor::EditableDocument`] (a plain,
//! `Send + Sync` Rust value, not a handle into a process-wide native
//! library singleton), it may be constructed, held, and called
//! concurrently from multiple threads without any dedicated
//! single-threaded "actor" or global FFI lock -- see
//! [`crate::tauri_commands`] for how the Tauri command layer takes
//! advantage of this.
//!
//! ## Known limitation: tile rendering re-renders the full page
//!
//! [`PdfRenderer::render_page`]'s tiled/`Viewport` path currently renders
//! the *whole* page at the requested DPI internally and then crops the
//! requested rectangle out of that raster, rather than the content-stream
//! interpreter honoring a device-space sub-rectangle directly. Practically
//! this means memory use for tiled rendering is still bounded by the *full
//! page* at the requested DPI (checked against [`MAX_RENDER_PIXELS`]), not
//! by the tile size. Implementing genuine partial-raster tiling (bounded
//! by tile size, not full-page size, so very high zoom levels on very
//! large pages stay cheap) is a reasonable follow-up.
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
//! [`crate::error::RenderError::OutputTooLarge`] *before* allocating a
//! raster, for both full-page and tiled/viewport renders. A malformed or
//! missing `/MediaBox` falls back to a US Letter default rather than
//! propagating non-finite/degenerate geometry into the rasterizer (see
//! [`crate::editor::EditableDocument::effective_media_box`]'s docs).
//!
//! # Example
//!
//! ```no_run
//! # #[cfg(feature = "render")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use rust_pdf::render::{PdfRenderer, Viewport};
//!
//! let renderer = PdfRenderer::open_file("example.pdf")?;
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

#[cfg(feature = "render")]
mod cache;
#[cfg(feature = "render")]
mod renderer;

#[cfg(feature = "render")]
pub use renderer::PdfRenderer;
// Only [`crate::tauri_commands::commands::render_page_impl`] calls this
// directly (`PdfRenderer::render_page` calls it too, but from *within*
// `renderer`, so that alone doesn't need this re-export) -- cfg-gated so
// a `render`-without-`tauri` build doesn't warn about an unused `pub(crate)`
// import.
#[cfg(all(feature = "render", feature = "tauri"))]
pub(crate) use renderer::render_page_document;

/// A pure-Rust content-stream interpreter and 2D rasterizer (no native
/// binary/FFI dependency), gated behind the `native-render` Cargo feature.
/// See its module docs for the current phase's scope and its explicitly
/// documented gaps.
#[cfg(feature = "native-render")]
pub mod native;

/// Rendered page/tile output.
///
/// Re-exports the `image` crate's `RgbaImage` (an 8-bit-per-channel,
/// non-premultiplied `ImageBuffer<Rgba<u8>, Vec<u8>>`) so callers can use it
/// directly with `image`'s encoders (`RgbaImage::save(...)`, PNG/etc.), or
/// read `.as_raw()` to hand the pixel buffer straight to a GPU texture
/// upload. `tiny-skia`'s own `Pixmap` (premultiplied-alpha) is converted to
/// this straight-alpha representation once, at the end of rendering -- see
/// `renderer::pixmap_to_rgba_image`.
#[cfg(feature = "render")]
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
#[cfg(feature = "render")]
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
/// top-left corner (the conventional origin for raster image/bitmap
/// coordinate systems) — *not* PDF user-space points, whose origin is
/// bottom-left (ISO 32000-1 §8.3.2.3).
#[cfg(feature = "render")]
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

#[cfg(feature = "render")]
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

#[cfg(all(test, feature = "render"))]
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
