//! [`PdfiumLibrary`] (native library binding) and [`PdfRenderer`]
//! (per-document page rasterization) — see the [`crate::render`] module
//! documentation for the overall design rationale.

use std::mem::ManuallyDrop;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

use pdfium_render::prelude::{
    PdfDocument, PdfPage, Pdfium, PdfiumError, PdfiumInternalError, PdfRenderConfig,
};

use crate::error::RenderError;
use crate::render::cache::ThumbnailCache;
use crate::render::{RgbaImage, Viewport, MAX_RENDER_PIXELS};

/// Default number of thumbnails kept per [`PdfRenderer`] in its LRU cache.
/// See [`crate::render::cache::ThumbnailCache`].
const DEFAULT_THUMBNAIL_CACHE_ENTRIES: usize = 32;

/// Serializes every call that crosses the FFI boundary into `libpdfium`.
///
/// **Why this exists:** `pdfium-render`'s `thread_safe` Cargo feature (on
/// by default, and on for this crate) only implements Rust's `Send`/`Sync`
/// marker traits for [`Pdfium`] so the wrapper *type* can live in
/// shared/static application state (e.g. a Tauri app's managed state).
/// It does **not** add any internal locking around actual calls into the
/// native library, and Pdfium's own C API is documented upstream as *not*
/// safe to call concurrently from multiple threads. This was confirmed
/// empirically during development of this module: running this crate's
/// own `render` test suite with a parallel test harness (multiple threads
/// calling into the same process-wide Pdfium instance at once) reliably
/// aborted the process (SIGABRT) inside `libpdfium`, not a catchable Rust
/// panic. A second, related crash (SIGSEGV) was found when a
/// [`PdfRenderer`] being dropped (which closes its document via
/// `FPDF_CloseDocument`) raced with another thread still rendering --
/// see the `document` field's doc comment on [`PdfRenderer`] and its
/// `Drop` impl for how that is closed under this same lock. Per this
/// crate's rule against unsafe/undefined FFI behavior, every public
/// [`PdfiumLibrary`]/[`PdfRenderer`] method that touches Pdfium therefore
/// takes this lock for its whole FFI-touching body, including document
/// teardown.
///
/// If the lock is poisoned by a prior panic while held, it is recovered
/// (`PoisonError::into_inner`) rather than propagating the poison forever:
/// the guarded value is a unit `()` with no invariant that a panic could
/// have left inconsistent, so poisoning would otherwise permanently and
/// unnecessarily disable all rendering for the rest of the process.
fn ffi_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A loaded, process-wide binding to the native Pdfium shared library.
///
/// Pdfium's C API is a process-wide singleton (`pdfium-render` enforces
/// this: the underlying `FPDF_InitLibrary` call may only happen once per
/// process). Construct exactly one [`PdfiumLibrary`] at application
/// startup and share a reference to it across every [`PdfRenderer`] you
/// open; a second call to [`PdfiumLibrary::bind`] or
/// [`PdfiumLibrary::bind_from_path`] in the same process returns
/// [`RenderError::LibraryLoad`] rather than re-initializing.
pub struct PdfiumLibrary {
    pdfium: Pdfium,
}

impl PdfiumLibrary {
    /// Loads the native Pdfium shared library.
    ///
    /// Search order:
    /// 1. The directory named by the `RUST_PDF_PDFIUM_LIB_DIR` environment
    ///    variable, if set (this is what `scripts/fetch_pdfium.sh`-fetched
    ///    binaries, or a Tauri app's resolved resource directory, should
    ///    point at).
    /// 2. The operating system's standard shared-library search path, via
    ///    [`Pdfium::bind_to_system_library`], for systems where Pdfium is
    ///    already installed.
    ///
    /// Returns [`RenderError::LibraryLoad`] if neither succeeds.
    pub fn bind() -> Result<Self, RenderError> {
        let _guard = ffi_lock();

        if let Ok(dir) = std::env::var("RUST_PDF_PDFIUM_LIB_DIR") {
            let library_path = Pdfium::pdfium_platform_library_name_at_path(&dir);
            if let Ok(library) = Self::bind_to_library_path_locked(&library_path) {
                return Ok(library);
            }
        }

        Pdfium::bind_to_system_library()
            .map(|bindings| Self {
                pdfium: Pdfium::new(bindings),
            })
            .map_err(|source| RenderError::LibraryLoad {
                tried: "RUST_PDF_PDFIUM_LIB_DIR env var, then system library search path"
                    .to_string(),
                source,
            })
    }

    /// Loads the native Pdfium shared library from an explicit directory
    /// containing the platform's `libpdfium.{dylib,so,dll}`.
    pub fn bind_from_path(dir: impl AsRef<Path>) -> Result<Self, RenderError> {
        let _guard = ffi_lock();

        let library_path = Pdfium::pdfium_platform_library_name_at_path(dir.as_ref());
        Self::bind_to_library_path_locked(&library_path)
    }

    /// Shared implementation for [`PdfiumLibrary::bind`] and
    /// [`PdfiumLibrary::bind_from_path`]. Callers must already hold
    /// [`ffi_lock`] -- this does *not* take it itself, to avoid the
    /// non-reentrant `std::sync::Mutex` deadlocking when `bind()` tries
    /// its `RUST_PDF_PDFIUM_LIB_DIR` fallback.
    fn bind_to_library_path_locked(library_path: &Path) -> Result<Self, RenderError> {
        Pdfium::bind_to_library(library_path)
            .map(|bindings| Self {
                pdfium: Pdfium::new(bindings),
            })
            .map_err(|source| RenderError::LibraryLoad {
                tried: library_path.display().to_string(),
                source,
            })
    }
}

/// Renders pages of a single opened PDF document to RGBA raster images.
///
/// Borrows the [`PdfiumLibrary`] it was opened against for its whole
/// lifetime (`'lib`), so a document cannot outlive the native library
/// binding it depends on.
pub struct PdfRenderer<'lib> {
    // Wrapped in `ManuallyDrop` so our custom `Drop` impl (below) can run
    // `PdfDocument`'s own drop glue (which calls `FPDF_CloseDocument`)
    // *while still holding* `ffi_lock`. Without this, the field would drop
    // automatically after a derived/empty `Drop::drop` body returns,
    // outside the lock -- which was observed to SIGSEGV when a document
    // close raced with another thread's render call. See `ffi_lock`'s doc
    // comment and this type's own `Drop` impl.
    document: ManuallyDrop<PdfDocument<'lib>>,
    thumbnails: Mutex<ThumbnailCache>,
}

impl<'lib> PdfRenderer<'lib> {
    /// Opens a PDF document from an in-memory byte buffer.
    ///
    /// `password`, if the document is encrypted (ISO 32000-1 §7.6), is
    /// tried as both the user and owner password. Ownership of `bytes` is
    /// taken by the returned [`PdfRenderer`] (via the underlying
    /// [`PdfDocument`]); no separate lifetime management of the byte
    /// buffer is required from the caller.
    pub fn open_bytes(
        library: &'lib PdfiumLibrary,
        bytes: Vec<u8>,
        password: Option<&str>,
    ) -> Result<Self, RenderError> {
        let _guard = ffi_lock();

        let document = library
            .pdfium
            .load_pdf_from_byte_vec(bytes, password)
            .map_err(Self::map_open_error)?;

        Ok(Self::from_document(document))
    }

    /// Opens a PDF document from a file path.
    ///
    /// `password`, if the document is encrypted (ISO 32000-1 §7.6), is
    /// tried as both the user and owner password.
    pub fn open_file(
        library: &'lib PdfiumLibrary,
        path: impl AsRef<Path>,
        password: Option<&str>,
    ) -> Result<Self, RenderError> {
        let _guard = ffi_lock();

        let document = library
            .pdfium
            .load_pdf_from_file(path.as_ref(), password)
            .map_err(Self::map_open_error)?;

        Ok(Self::from_document(document))
    }

    fn from_document(document: PdfDocument<'lib>) -> Self {
        Self {
            document: ManuallyDrop::new(document),
            thumbnails: Mutex::new(ThumbnailCache::new(DEFAULT_THUMBNAIL_CACHE_ENTRIES)),
        }
    }

    /// Translates a raw [`PdfiumError`] from document loading into the
    /// more specific [`RenderError::PasswordRequired`] when that is what
    /// actually happened, since that is the one case a caller is likely to
    /// want to handle specially (e.g. by prompting the user).
    fn map_open_error(err: PdfiumError) -> RenderError {
        match err {
            PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
                RenderError::PasswordRequired
            }
            other => RenderError::DocumentLoad(other),
        }
    }

    /// Returns the number of pages in the document.
    pub fn page_count(&self) -> usize {
        let _guard = ffi_lock();
        self.page_count_locked()
    }

    /// Implementation of [`Self::page_count`] for callers that already
    /// hold [`ffi_lock`] (avoids deadlocking the non-reentrant
    /// `std::sync::Mutex` when called from [`Self::render_page`]/
    /// [`Self::render_thumbnail`]).
    fn page_count_locked(&self) -> usize {
        self.document.pages().len() as usize
    }

    /// Renders a page to an RGBA raster image at the given resolution.
    ///
    /// # Parameters
    /// - `page_index`: zero-based page index; must be `< self.page_count()`.
    /// - `dpi`: dots per inch to rasterize at. PDF user space is measured
    ///   in points, 1/72 inch (ISO 32000-1 §7.7.3.3), so the full page's
    ///   pixel size is `page_points * dpi / 72`.
    /// - `viewport`: `None` renders the whole page. `Some(rect)` renders
    ///   only that device-pixel sub-rectangle of the full page-at-`dpi`
    ///   raster — this is the tile-based rendering path a desktop viewer
    ///   should use for zoom/pan, so it need not hold a full-resolution
    ///   raster of every page in memory, only the tiles currently visible.
    ///
    /// # Errors
    /// Returns [`RenderError::InvalidDpi`], [`RenderError::InvalidPageIndex`],
    /// [`RenderError::OutputTooLarge`] (the page/viewport pixel size exceeds
    /// [`MAX_RENDER_PIXELS`] — see the module-level "Untrusted input
    /// handling" section), [`RenderError::EmptyViewport`],
    /// [`RenderError::ViewportOutOfBounds`], or [`RenderError::PageRender`]
    /// if Pdfium itself reports a rendering failure.
    ///
    /// All size validation happens *before* any bitmap is allocated.
    pub fn render_page(
        &self,
        page_index: usize,
        dpi: f32,
        viewport: Option<Viewport>,
    ) -> Result<RgbaImage, RenderError> {
        if !(dpi.is_finite() && dpi > 0.0) {
            return Err(RenderError::InvalidDpi(dpi));
        }

        let _guard = ffi_lock();

        let page_count = self.page_count_locked();
        if page_index >= page_count {
            return Err(RenderError::InvalidPageIndex {
                index: page_index,
                page_count,
            });
        }

        let page = self
            .document
            .pages()
            .get(page_index as i32)
            .map_err(|source| RenderError::PageRender { page_index, source })?;

        let (full_width_px, full_height_px) = Self::page_pixel_size(&page, dpi);
        Self::check_dimensions(full_width_px, full_height_px)?;

        match viewport {
            None => Self::render_full_page(&page, page_index, full_width_px, full_height_px),
            Some(vp) => {
                if vp.width == 0 || vp.height == 0 {
                    return Err(RenderError::EmptyViewport);
                }

                let full_w = full_width_px as u64;
                let full_h = full_height_px as u64;
                if u64::from(vp.x) + u64::from(vp.width) > full_w
                    || u64::from(vp.y) + u64::from(vp.height) > full_h
                {
                    return Err(RenderError::ViewportOutOfBounds {
                        x: vp.x,
                        y: vp.y,
                        width: vp.width,
                        height: vp.height,
                        page_width: full_width_px,
                        page_height: full_height_px,
                    });
                }
                Self::check_dimensions(vp.width, vp.height)?;

                let full_page =
                    Self::render_full_page(&page, page_index, full_width_px, full_height_px)?;

                Ok(image::imageops::crop_imm(&full_page, vp.x, vp.y, vp.width, vp.height)
                    .to_image())
            }
        }
    }

    /// Renders (and caches) a thumbnail for a page, scaled so its longest
    /// side is at most `max_dimension` pixels while preserving aspect
    /// ratio, suitable for a page-list/grid UI.
    ///
    /// Repeated calls with the same `(page_index, max_dimension)` are
    /// served from an in-memory LRU cache (see
    /// [`crate::render::cache::ThumbnailCache`]) rather than re-invoking
    /// Pdfium every time.
    pub fn render_thumbnail(
        &self,
        page_index: usize,
        max_dimension: u32,
    ) -> Result<RgbaImage, RenderError> {
        if max_dimension == 0 {
            return Err(RenderError::EmptyViewport);
        }

        let key = (page_index, max_dimension);

        // Cache lookups do not touch the Pdfium FFI boundary (see
        // `ThumbnailCache`, which just holds already-rendered `RgbaImage`
        // buffers), so this is intentionally outside `ffi_lock`.
        if let Ok(mut cache) = self.thumbnails.lock() {
            if let Some(cached) = cache.get(key) {
                return Ok(cached);
            }
        }

        let _guard = ffi_lock();

        let page_count = self.page_count_locked();
        if page_index >= page_count {
            return Err(RenderError::InvalidPageIndex {
                index: page_index,
                page_count,
            });
        }

        let page = self
            .document
            .pages()
            .get(page_index as i32)
            .map_err(|source| RenderError::PageRender { page_index, source })?;

        Self::check_dimensions(max_dimension, max_dimension)?;

        let config = PdfRenderConfig::new().thumbnail(max_dimension as i32);
        let bitmap = page
            .render_with_config(&config)
            .map_err(|source| RenderError::PageRender { page_index, source })?;
        let image = bitmap
            .as_image()
            .map_err(|source| RenderError::PageRender { page_index, source })?
            .into_rgba8();

        if let Ok(mut cache) = self.thumbnails.lock() {
            cache.insert(key, image.clone());
        }

        Ok(image)
    }

    /// Discards all cached thumbnails. Useful if the underlying document
    /// content has changed (not applicable to a read-only [`PdfRenderer`]
    /// today, but kept as a small, explicit escape hatch for callers that
    /// re-open a renderer in place).
    pub fn clear_thumbnail_cache(&self) {
        if let Ok(mut cache) = self.thumbnails.lock() {
            *cache = ThumbnailCache::new(DEFAULT_THUMBNAIL_CACHE_ENTRIES);
        }
    }

    /// Computes a page's full raster size, in pixels, at `dpi`.
    ///
    /// PDF user space is measured in points (ISO 32000-1 §7.7.3.3, 1/72
    /// inch); [`PdfPage::width`]/[`PdfPage::height`] already account for
    /// the page's own `/Rotate` entry (ISO 32000-1 Table 30), so no
    /// separate rotation handling is needed here.
    fn page_pixel_size(page: &PdfPage, dpi: f32) -> (u32, u32) {
        let scale = dpi / 72.0;
        let width = (page.width().value * scale).round().max(1.0);
        let height = (page.height().value * scale).round().max(1.0);

        // Clamp to u32::MAX so `check_dimensions` always sees a
        // representable value instead of this cast wrapping/UB-adjacent
        // truncation; the oversize case is reported as a normal
        // `OutputTooLarge` error rather than silently wrapping.
        let width = width.min(u32::MAX as f32) as u32;
        let height = height.min(u32::MAX as f32) as u32;

        (width, height)
    }

    /// Rejects render requests whose pixel area exceeds
    /// [`MAX_RENDER_PIXELS`] *before* any bitmap is allocated. `width` and
    /// `height` are widened to `u64` so the multiplication cannot
    /// overflow even at `u32::MAX` extremes.
    fn check_dimensions(width: u32, height: u32) -> Result<(), RenderError> {
        let pixels = u64::from(width) * u64::from(height);
        if pixels == 0 || pixels > MAX_RENDER_PIXELS {
            return Err(RenderError::OutputTooLarge {
                width,
                height,
                pixels,
                max_pixels: MAX_RENDER_PIXELS,
            });
        }
        Ok(())
    }

    /// Renders the full page (no viewport cropping) into an RGBA image of
    /// exactly `width` x `height` pixels.
    ///
    /// Deliberately uses Pdfium's default, most-exercised rendering
    /// configuration (target width/height "auto-scaling" mode, form-field
    /// and annotation rendering left on) rather than a hand-rolled
    /// transformation matrix: that default path is what `pdfium-render`'s
    /// own examples and test suite exercise, and is the path known to
    /// correctly honor the page's rotation and orientation. See the
    /// module-level "Native vs. FFI rendering decision" section for why
    /// this crate defers to Pdfium's tested behavior rather than
    /// constructing custom device-space transforms whose interaction with
    /// Pdfium's internal rotation handling is not something this crate can
    /// verify against the spec alone.
    fn render_full_page(
        page: &PdfPage,
        page_index: usize,
        width: u32,
        height: u32,
    ) -> Result<RgbaImage, RenderError> {
        let config = PdfRenderConfig::new()
            .set_target_width(width as i32)
            .set_target_height(height as i32);

        let bitmap = page
            .render_with_config(&config)
            .map_err(|source| RenderError::PageRender { page_index, source })?;

        let image = bitmap
            .as_image()
            .map_err(|source| RenderError::PageRender { page_index, source })?
            .into_rgba8();

        Ok(image)
    }
}

impl<'lib> Drop for PdfRenderer<'lib> {
    /// Closes the underlying [`PdfDocument`] (which calls Pdfium's
    /// `FPDF_CloseDocument`) while holding [`ffi_lock`], so a document
    /// being closed on one thread can never race with a render call --
    /// or another document being opened/closed -- on another thread. See
    /// the `document` field's doc comment on [`PdfRenderer`].
    fn drop(&mut self) {
        let _guard = ffi_lock();
        // SAFETY: `ManuallyDrop::drop` is only called here, and `Drop::drop`
        // itself is guaranteed by the language to run at most once per
        // value, so `self.document` is never accessed or dropped again
        // after this point.
        unsafe {
            ManuallyDrop::drop(&mut self.document);
        }
    }
}
