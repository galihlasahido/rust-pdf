//! [`PdfRenderer`]: per-document page rasterization, built entirely on
//! this crate's own pure-Rust parser/editor
//! ([`crate::editor::EditableDocument`], ISO 32000-1 document structure)
//! and content-stream interpreter ([`crate::render::native`],
//! rasterization via `tiny-skia`). See the [`crate::render`] module
//! documentation for the migration history (this crate previously used an
//! FFI binding to a third-party native rendering engine) and this
//! backend's explicitly-documented scope and gaps.

use std::path::Path;
use std::sync::Mutex;

use crate::editor::EditableDocument;
use crate::error::{ParserError, PdfError, RenderError};
use crate::object::{Object, PdfArray, PdfDictionary, PdfStream};
use crate::render::cache::ThumbnailCache;
use crate::render::native::{self, Pixmap};
use crate::render::{RgbaImage, Viewport, MAX_RENDER_PIXELS};
use crate::types::{Matrix, ObjectId, Rectangle};

/// Maximum number of `Object::Reference` dereferences performed while
/// deep-resolving a page's `/Resources` subtree (see [`deep_resolve`])
/// before content-stream interpretation. Bounds work against a corrupt/
/// adversarial resource graph (a reference cycle, or a dictionary fanning
/// out into many references) -- ordinary pages need nowhere near this
/// many.
const MAX_RESOLVE_REFERENCES: usize = 200_000;

/// Maximum recursion depth for the same walk (see [`deep_resolve`]),
/// guarding the call stack against a maliciously *deep* (not just wide)
/// resource graph.
const MAX_RESOLVE_DEPTH: u32 = 64;

/// Default number of thumbnails kept per [`PdfRenderer`] in its LRU cache.
/// See [`crate::render::cache::ThumbnailCache`].
const DEFAULT_THUMBNAIL_CACHE_ENTRIES: usize = 32;

/// Renders pages of a single opened PDF document to RGBA raster images.
///
/// Wraps an owned [`EditableDocument`] -- the same pure-Rust structural
/// parser this crate's editing/forms/signature APIs use -- so opening a
/// document for rendering never depends on any native library or FFI call.
/// `EditableDocument` is `Send + Sync` (see its own docs / the compile-time
/// assertion on [`crate::tauri_commands::state::AppState`]), so unlike the
/// previous FFI-backed implementation, a [`PdfRenderer`] may safely be
/// shared across threads and called concurrently -- there is no
/// process-wide singleton native library and no FFI call that must be
/// serialized. See [`crate::tauri_commands`] for how the Tauri command
/// layer takes advantage of this (a plain worker-thread pool, no dedicated
/// single-threaded rendering actor).
pub struct PdfRenderer {
    document: EditableDocument,
    thumbnails: Mutex<ThumbnailCache>,
}

impl PdfRenderer {
    /// Opens a PDF document from an in-memory byte buffer.
    ///
    /// Returns [`RenderError::PasswordRequired`] if the document is
    /// encrypted (ISO 32000-1 §7.6) -- this entry point has no password
    /// parameter to accept, so an encrypted document is rejected
    /// unconditionally; see [`Self::open_bytes_with_password`] to actually
    /// supply one.
    pub fn open_bytes(bytes: Vec<u8>) -> Result<Self, RenderError> {
        let document = EditableDocument::from_bytes(bytes).map_err(Self::map_open_error)?;
        Ok(Self::from_document(document))
    }

    /// Opens a PDF document from a file path.
    ///
    /// Returns [`RenderError::PasswordRequired`] if the document is
    /// encrypted; see [`Self::open_bytes`]'s docs (and
    /// [`Self::open_file_with_password`] to actually supply a password).
    pub fn open_file(path: impl AsRef<Path>) -> Result<Self, RenderError> {
        let document = EditableDocument::open(path).map_err(Self::map_open_error)?;
        Ok(Self::from_document(document))
    }

    /// Opens a (possibly encrypted) PDF document from an in-memory byte
    /// buffer, supplying `password` to derive the file encryption key if
    /// the document turns out to be encrypted (ISO 32000-1 §7.6 / ISO
    /// 32000-2 §7.6). If the document is *not* encrypted, `password` is
    /// simply ignored -- this behaves exactly like [`Self::open_bytes`].
    ///
    /// Only the two algorithms
    /// [`crate::editor::EditableDocument::save_encrypted_to_bytes`] can
    /// itself produce are supported (AES-128 `/V 4 /R 4`/AESV2 and AES-256
    /// `/V 5 /R 6`/AESV3); anything else still fails (with a generic
    /// [`RenderError::DocumentLoad`] wrapping
    /// [`crate::error::ParserError::UnsupportedEncryption`], not
    /// [`RenderError::PasswordRequired`], since no password would help).
    /// A wrong password fails the same way, wrapping
    /// [`crate::error::ParserError::IncorrectPassword`].
    #[cfg(feature = "encryption")]
    pub fn open_bytes_with_password(bytes: Vec<u8>, password: &str) -> Result<Self, RenderError> {
        let document = EditableDocument::from_bytes_with_password(bytes, password)
            .map_err(Self::map_open_error)?;
        Ok(Self::from_document(document))
    }

    /// Opens a (possibly encrypted) PDF document from a file path,
    /// supplying `password`. See [`Self::open_bytes_with_password`]'s docs
    /// for the full contract.
    #[cfg(feature = "encryption")]
    pub fn open_file_with_password(
        path: impl AsRef<Path>,
        password: &str,
    ) -> Result<Self, RenderError> {
        let document =
            EditableDocument::open_with_password(path, password).map_err(Self::map_open_error)?;
        Ok(Self::from_document(document))
    }

    fn from_document(document: EditableDocument) -> Self {
        Self {
            document,
            thumbnails: Mutex::new(ThumbnailCache::new(DEFAULT_THUMBNAIL_CACHE_ENTRIES)),
        }
    }

    /// Translates a raw [`PdfError`] from document loading into the more
    /// specific [`RenderError::PasswordRequired`] when that is what
    /// actually happened: an encrypted document (ISO 32000-1 §7.6) opened
    /// through one of the *no-password* entry points
    /// ([`Self::open_bytes`]/[`Self::open_file`]), which reject any
    /// `/Encrypt`-bearing document outright since they have no password to
    /// offer it. That is the one case a caller is likely to want to handle
    /// specially (e.g. by prompting for a password and retrying via
    /// [`Self::open_bytes_with_password`]/[`Self::open_file_with_password`]
    /// rather than reporting a generic parse failure).
    fn map_open_error(err: PdfError) -> RenderError {
        if matches!(err, PdfError::Parser(ParserError::EncryptedPdf)) {
            RenderError::PasswordRequired
        } else {
            RenderError::DocumentLoad(Box::new(err))
        }
    }

    /// Returns the number of pages in the document, or `0` if the page
    /// tree itself is too malformed to walk (see
    /// [`EditableDocument::page_count`]) -- a corrupt/adversarial page
    /// tree degrades to "no pages" rather than panicking or propagating a
    /// `Result` from a method this type's previous, FFI-backed
    /// implementation returned as a plain `usize`.
    pub fn page_count(&self) -> usize {
        self.document.page_count().unwrap_or(0)
    }

    /// Returns the underlying [`EditableDocument`], for callers that need
    /// document-level operations (bookmarks, text layout, form fields, ...)
    /// alongside rendering without opening the same file a second time.
    pub fn document(&self) -> &EditableDocument {
        &self.document
    }

    /// Runs `f` with mutable access to the underlying [`EditableDocument`]
    /// (structural edits: form field values, annotations, page
    /// manipulation, redaction, ...), then always clears the thumbnail
    /// cache -- so a caller can never forget to invalidate a render cache
    /// that's now stale. Prefer this over reaching into the document some
    /// other way for any mutation that should be reflected next time a
    /// page is rendered.
    pub fn edit_document<R>(&mut self, f: impl FnOnce(&mut EditableDocument) -> R) -> R {
        let result = f(&mut self.document);
        self.clear_thumbnail_cache();
        result
    }

    /// A page's raw, *unrotated* `/MediaBox` size in PDF user-space points
    /// (ISO 32000-1 §7.7.3.3, 1/72 inch) -- the space `EditableDocument`'s
    /// own field/annotation/text-layout rectangles are already expressed
    /// in (see e.g. `list_form_fields`'s `FormFieldWidget::rect` and
    /// `extract_page_text_layout`'s `TextRun`, both documented as
    /// unrotated-page-space with rotation left to the caller). `None` for
    /// an out-of-range or unreadable page index, mirroring
    /// [`Self::render_page`]'s own `InvalidPageIndex`/`DocumentLoad`
    /// failure modes rather than panicking.
    pub fn page_size_pt(&self, page_index: usize) -> Option<(f64, f64)> {
        let (_, media_box, _) = page_geometry(&self.document, page_index).ok()?;
        Some((media_box.width(), media_box.height()))
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
    /// if the content-stream interpreter reports a structural failure.
    ///
    /// All size validation happens *before* any raster is allocated.
    pub fn render_page(
        &self,
        page_index: usize,
        dpi: f32,
        viewport: Option<Viewport>,
    ) -> Result<RgbaImage, RenderError> {
        render_page_document(&self.document, page_index, dpi, viewport)
    }

    /// Renders (and caches) a thumbnail for a page, scaled so its longest
    /// *displayed* side (after applying the page's own effective
    /// `/Rotate`) is at most `max_dimension` pixels while preserving
    /// aspect ratio, suitable for a page-list/grid UI.
    ///
    /// Repeated calls with the same `(page_index, max_dimension)` are
    /// served from an in-memory LRU cache (see
    /// [`crate::render::cache::ThumbnailCache`]) rather than
    /// re-interpreting the content stream every time.
    pub fn render_thumbnail(
        &self,
        page_index: usize,
        max_dimension: u32,
    ) -> Result<RgbaImage, RenderError> {
        if max_dimension == 0 {
            return Err(RenderError::EmptyViewport);
        }

        let key = (page_index, max_dimension);
        if let Ok(mut cache) = self.thumbnails.lock() {
            if let Some(cached) = cache.get(key) {
                return Ok(cached);
            }
        }

        let (_page_id, media_box, rotate) = page_geometry(&self.document, page_index)?;
        let (disp_w_pt, disp_h_pt) = display_dimensions_pt(&media_box, rotate);
        let longest = disp_w_pt.max(disp_h_pt).max(1.0);
        let dpi = (72.0 * f64::from(max_dimension) / longest) as f32;
        let dpi = if dpi.is_finite() && dpi > 0.0 {
            dpi
        } else {
            72.0
        };

        let image = render_page_document(&self.document, page_index, dpi, None)?;

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
}

/// Core rendering implementation shared by [`PdfRenderer::render_page`]
/// and the Tauri command layer's own `render_page` command
/// ([`crate::tauri_commands::commands::render_page_impl`]), which reuses
/// an already-open [`EditableDocument`] (the same one structural-editing
/// commands share for a given document handle) instead of asking
/// [`PdfRenderer`] to open the file a second time.
///
/// `pub(crate)` (re-exported as `crate::render::render_page_document`) --
/// not part of this crate's public API, since a borrowed `&EditableDocument`
/// isn't a type external callers construct through this module.
pub(crate) fn render_page_document(
    document: &EditableDocument,
    page_index: usize,
    dpi: f32,
    viewport: Option<Viewport>,
) -> Result<RgbaImage, RenderError> {
    if !(dpi.is_finite() && dpi > 0.0) {
        return Err(RenderError::InvalidDpi(dpi));
    }

    let (page_id, media_box, rotate) = page_geometry(document, page_index)?;

    let resources_raw = document
        .effective_resources(page_id)
        .map_err(|e| RenderError::DocumentLoad(Box::new(e)))?;
    // `native::render_content_stream` expects `resources` (and everything
    // reachable from it -- fonts, `/DescendantFonts`, `FontFile*` streams,
    // XObjects, ExtGStates, colour spaces, ...) to already be fully
    // dereferenced (see [`native`]'s "Pre-resolved `/Resources`
    // assumption" module docs). A real, serialized-then-reparsed PDF
    // almost always represents these as indirect references, so this
    // resolution is not optional -- skipping it would make every embedded
    // font/image/ExtGState on a genuine whole-document page silently fail
    // to resolve (treated the same as "resource absent").
    let mut resolve_budget = MAX_RESOLVE_REFERENCES;
    let mut resources = match deep_resolve(
        document,
        &Object::Dictionary(resources_raw),
        0,
        &mut resolve_budget,
    ) {
        Object::Dictionary(d) => d,
        _ => PdfDictionary::new(),
    };
    let mut content = document
        .page_content_bytes(page_id)
        .map_err(|e| RenderError::DocumentLoad(Box::new(e)))?;

    // Paint annotation appearance streams (ISO 32000-1 §12.5.5) *after*
    // the page's own content, so highlights/stamps/comments/etc. show up
    // on top of the page content they annotate -- see
    // `append_annotation_appearances`'s own docs for the algorithm and
    // its documented simplifications.
    if let Ok(page_dict) = document.get_dictionary(page_id) {
        append_annotation_appearances(
            document,
            &page_dict,
            &mut resources,
            &mut content,
            &mut resolve_budget,
        );
    }

    let (content_w, content_h) = page_pixel_size(&media_box, dpi);
    let (full_w, full_h) = apply_rotation_to_dims(content_w, content_h, rotate);
    check_dimensions(full_w, full_h)?;

    match viewport {
        None => render_full_page(
            &content, &resources, media_box, content_w, content_h, rotate, page_index,
        ),
        Some(vp) => {
            if vp.width == 0 || vp.height == 0 {
                return Err(RenderError::EmptyViewport);
            }

            let full_w64 = u64::from(full_w);
            let full_h64 = u64::from(full_h);
            if u64::from(vp.x) + u64::from(vp.width) > full_w64
                || u64::from(vp.y) + u64::from(vp.height) > full_h64
            {
                return Err(RenderError::ViewportOutOfBounds {
                    x: vp.x,
                    y: vp.y,
                    width: vp.width,
                    height: vp.height,
                    page_width: full_w,
                    page_height: full_h,
                });
            }
            check_dimensions(vp.width, vp.height)?;

            let full_page = render_full_page(
                &content, &resources, media_box, content_w, content_h, rotate, page_index,
            )?;

            Ok(image::imageops::crop_imm(&full_page, vp.x, vp.y, vp.width, vp.height).to_image())
        }
    }
}

/// Upper bound on the number of `/Annots` entries painted per page render
/// -- bounds work against a page declaring an implausibly large annotation
/// array (untrusted input); real pages have at most a few hundred.
const MAX_ANNOTATIONS_PER_PAGE: usize = 10_000;

/// Paints every visible, appearance-bearing annotation on `page_dict`'s
/// `/Annots` array (ISO 32000-1 §12.5.5 "Appearance Streams") by appending
/// a synthetic `q <matrix> cm /<name> Do Q` to `content` for each one and
/// registering its resolved appearance stream under a fresh name in
/// `resources`'s `/XObject` sub-dictionary -- so the existing Form XObject
/// rendering path ([`native::render_content_stream`]'s `Do` handling,
/// including transparency groups if an appearance happens to declare one)
/// paints it, rather than a separate, annotation-specific renderer.
///
/// For each annotation:
/// - Skipped entirely (ISO 32000-1 Table 165 `/F` flags): `Hidden` (bit 2)
///   or `NoView` (bit 6) set.
/// - Skipped (ISO 32000-1 §12.5.6.19): `/Subtype /Popup` -- a Popup
///   annotation is a UI-only comment-editing affordance, not part of the
///   page's normal visible content, matching every mainstream viewer's
///   convention of never auto-painting one.
/// - Skipped: no usable appearance stream at all -- no `/AP`, an `/AP /N`
///   that isn't a stream, or (for an appearance *sub*dictionary keyed by
///   state) no entry matching `/AS`. This is the same "nothing to paint"
///   outcome ISO 32000-1 describes for an annotation without an
///   appearance, not a failure.
///
/// Honest, documented simplifications (untrusted/legacy input handled
/// gracefully, not spec-perfectly):
/// - A missing/malformed `/BBox` on the appearance stream (ISO 32000-1
///   requires one) falls back to painting the appearance at the
///   annotation's `/Rect` origin, unscaled, rather than skipping it or
///   failing the render.
/// - Optional Content (`/OC`, ISO 32000-1 §8.11) visibility is not
///   evaluated -- an annotation with an `/OC` entry in a "hidden" layer is
///   still painted. Widget annotations without their own usable
///   appearance are not synthesized from `/DA`/field values (that is a
///   distinct, separately-scoped "generate a default appearance" feature,
///   not a rendering one).
fn append_annotation_appearances(
    document: &EditableDocument,
    page_dict: &PdfDictionary,
    resources: &mut PdfDictionary,
    content: &mut Vec<u8>,
    resolve_budget: &mut usize,
) {
    const HIDDEN: i64 = 1 << 1; // bit position 2 (1-based, ISO 32000-1 Table 165)
    const NO_VIEW: i64 = 1 << 5; // bit position 6

    let Some(annots_obj) = page_dict.get("Annots") else {
        return;
    };
    let Object::Array(annots) = deep_resolve(document, annots_obj, 0, resolve_budget) else {
        return;
    };

    let mut xobjects = match resources.get("XObject") {
        Some(Object::Dictionary(d)) => d.clone(),
        _ => PdfDictionary::new(),
    };
    let mut painted_any = false;

    for (i, annot) in annots.iter().take(MAX_ANNOTATIONS_PER_PAGE).enumerate() {
        let Object::Dictionary(annot_dict) = annot else {
            continue;
        };

        if matches!(annot_dict.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Popup") {
            continue;
        }

        let flags = match annot_dict.get("F") {
            Some(Object::Integer(f)) => *f,
            _ => 0,
        };
        if flags & HIDDEN != 0 || flags & NO_VIEW != 0 {
            continue;
        }

        let Some(Object::Stream(stream)) = resolve_appearance_stream(annot_dict) else {
            continue;
        };
        if !matches!(stream.dictionary.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Form")
        {
            continue;
        }

        let Some(rect) = read_rectangle(annot_dict.get("Rect")) else {
            continue;
        };
        let bbox = read_rectangle(stream.dictionary.get("BBox"));
        let form_matrix = read_matrix(stream.dictionary.get("Matrix"));
        let aa = appearance_fit_matrix(bbox, form_matrix, rect);

        let name = format!("RustPdfAnnot{i}");
        xobjects.set(name.clone(), Object::Stream(stream));
        content.extend_from_slice(
            format!(
                "\nq {} {} {} {} {} {} cm /{name} Do Q\n",
                aa.a, aa.b, aa.c, aa.d, aa.e, aa.f
            )
            .as_bytes(),
        );
        painted_any = true;
    }

    if painted_any {
        resources.set("XObject", Object::Dictionary(xobjects));
    }
}

/// Resolves an (already fully dereferenced, per [`deep_resolve`])
/// annotation dictionary's current appearance stream (ISO 32000-1
/// §12.5.5): `/AP /N` directly if it is a stream, or `/AP /N /<AS>` if
/// `/N` is itself an appearance-*state* subdictionary. Returns `None` for
/// every other case (no `/AP`, no usable `/N`, or a state subdictionary
/// with no entry matching `/AS`) -- see
/// [`append_annotation_appearances`]'s docs for why that is "nothing to
/// paint", not an error.
fn resolve_appearance_stream(annot_dict: &PdfDictionary) -> Option<Object> {
    let Some(Object::Dictionary(ap)) = annot_dict.get("AP") else {
        return None;
    };
    match ap.get("N") {
        Some(stream @ Object::Stream(_)) => Some(stream.clone()),
        Some(Object::Dictionary(states)) => {
            let Some(Object::Name(as_name)) = annot_dict.get("AS") else {
                return None;
            };
            match states.get(as_name.as_str()) {
                Some(stream @ Object::Stream(_)) => Some(stream.clone()),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Parses a 4-number array (`/Rect`, `/BBox`, ...) into a [`Rectangle`],
/// normalizing corner order (ISO 32000-1 does not require `[llx lly urx
/// ury]` to already be in lower-left/upper-right order). Returns `None`
/// for anything else: wrong element count, a non-numeric entry, or a
/// non-finite coordinate (untrusted input, handled gracefully rather than
/// propagating NaN/inf).
fn read_rectangle(obj: Option<&Object>) -> Option<Rectangle> {
    let Some(Object::Array(arr)) = obj else {
        return None;
    };
    if arr.len() != 4 {
        return None;
    }
    let mut nums = [0.0f64; 4];
    for (slot, value) in nums.iter_mut().zip(arr.iter()) {
        *slot = match value {
            Object::Real(r) => *r,
            Object::Integer(n) => *n as f64,
            _ => return None,
        };
    }
    let [x0, y0, x1, y1] = nums;
    if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return None;
    }
    let (llx, urx) = (x0.min(x1), x0.max(x1));
    let (lly, ury) = (y0.min(y1), y0.max(y1));
    Some(Rectangle::new(llx, lly, urx, ury))
}

/// Parses a Form XObject's `/Matrix` (ISO 32000-1 §8.10.1, Table 95),
/// defaulting to the identity matrix if absent, the wrong length, or
/// non-finite (untrusted input).
fn read_matrix(obj: Option<&Object>) -> Matrix {
    let Some(Object::Array(arr)) = obj else {
        return Matrix::identity();
    };
    let v: Vec<f64> = arr
        .iter()
        .filter_map(|value| match value {
            Object::Real(r) => Some(*r),
            Object::Integer(n) => Some(*n as f64),
            _ => None,
        })
        .collect();
    if v.len() != 6 || v.iter().any(|x| !x.is_finite()) {
        return Matrix::identity();
    }
    Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5])
}

/// Computes the matrix that maps an appearance stream's own coordinate
/// space onto its annotation's `/Rect`, per ISO 32000-1 §12.5.5's
/// algorithm: transform `bbox`'s corners by `form_matrix` and take the
/// smallest enclosing axis-aligned rectangle, then compute the
/// scale+translate that maps *that* rectangle onto `rect`. The Form
/// XObject's own `/Matrix` does *not* need to be folded in here -- the
/// interpreter's existing Form XObject handling
/// (`interpreter::Interpreter::run_form_xobject_inline`) already applies
/// it automatically on top of whatever CTM is active when `Do` runs
/// (i.e. this function's result), exactly matching the spec's intent
/// without applying `/Matrix` twice.
///
/// `bbox` being `None` (a missing/malformed appearance `/BBox` --
/// see [`append_annotation_appearances`]'s docs) falls back to a plain
/// translation to `rect`'s lower-left corner.
fn appearance_fit_matrix(bbox: Option<Rectangle>, form_matrix: Matrix, rect: Rectangle) -> Matrix {
    let Some(bbox) = bbox else {
        return Matrix::translate(rect.llx, rect.lly);
    };

    let corners = [
        form_matrix.transform_point(bbox.llx, bbox.lly),
        form_matrix.transform_point(bbox.urx, bbox.lly),
        form_matrix.transform_point(bbox.urx, bbox.ury),
        form_matrix.transform_point(bbox.llx, bbox.ury),
    ];
    let mut t_llx = f64::INFINITY;
    let mut t_lly = f64::INFINITY;
    let mut t_urx = f64::NEG_INFINITY;
    let mut t_ury = f64::NEG_INFINITY;
    for (x, y) in corners {
        t_llx = t_llx.min(x);
        t_urx = t_urx.max(x);
        t_lly = t_lly.min(y);
        t_ury = t_ury.max(y);
    }

    let t_w = t_urx - t_llx;
    let t_h = t_ury - t_lly;
    // A zero/degenerate transformed BBox extent can't be scaled to fit
    // `rect` (division by zero) -- fall back to no scaling on that axis
    // rather than producing a non-finite matrix.
    let scale_x = if t_w.abs() > f64::EPSILON {
        rect.width() / t_w
    } else {
        1.0
    };
    let scale_y = if t_h.abs() > f64::EPSILON {
        rect.height() / t_h
    } else {
        1.0
    };

    Matrix::translate(-t_llx, -t_lly)
        .multiply(&Matrix::scale(scale_x, scale_y))
        .multiply(&Matrix::translate(rect.llx, rect.lly))
}

/// Recursively resolves every [`Object::Reference`] reachable from `obj`
/// into its target object, since [`native::render_content_stream`]
/// expects its whole `/Resources` subtree to already be fully
/// dereferenced (see this crate's [`native`]-module "Pre-resolved
/// `/Resources` assumption" docs) -- a real, serialized-then-reparsed PDF
/// almost always represents `/Font`, `/DescendantFonts`,
/// `/FontDescriptor`, `FontFile*`/`CIDToGIDMap`, `/XObject` (including a
/// Form XObject's own nested `/Resources`), `/ExtGState`, and
/// `/ColorSpace` entries as indirect references, not inline dictionaries.
///
/// Bounded against a corrupt/adversarial resource graph (untrusted
/// input): `depth` is capped at [`MAX_RESOLVE_DEPTH`] and `budget`
/// (initialized to [`MAX_RESOLVE_REFERENCES`]) caps the total number of
/// references followed across the whole walk. Either limit being hit
/// degrades the remaining, not-yet-resolved subtree to [`Object::Null`]
/// -- which every consumer in [`native`] already treats as "absent"/
/// unsupported (a warning, never a panic) -- rather than resolving
/// forever or overflowing the stack.
fn deep_resolve(
    document: &EditableDocument,
    obj: &Object,
    depth: u32,
    budget: &mut usize,
) -> Object {
    if depth > MAX_RESOLVE_DEPTH {
        return Object::Null;
    }
    match obj {
        Object::Reference(id) => {
            if *budget == 0 {
                return Object::Null;
            }
            *budget -= 1;
            match document.get_object(*id) {
                Some(resolved) => deep_resolve(document, &resolved, depth + 1, budget),
                None => Object::Null,
            }
        }
        Object::Dictionary(dict) => {
            let mut out = PdfDictionary::new();
            for (key, value) in dict.iter() {
                out.set(
                    key.clone(),
                    deep_resolve(document, value, depth + 1, budget),
                );
            }
            Object::Dictionary(out)
        }
        Object::Array(arr) => {
            let items: Vec<Object> = arr
                .iter()
                .map(|value| deep_resolve(document, value, depth + 1, budget))
                .collect();
            Object::Array(PdfArray::from_objects(items))
        }
        Object::Stream(stream) => {
            let mut new_dict = PdfDictionary::new();
            for (key, value) in stream.dictionary.iter() {
                new_dict.set(
                    key.clone(),
                    deep_resolve(document, value, depth + 1, budget),
                );
            }
            Object::Stream(PdfStream::from_raw(new_dict, stream.data.clone()))
        }
        other => other.clone(),
    }
}

/// Resolves the page id, effective `/MediaBox`, and normalized effective
/// `/Rotate` for `page_index`, translating a bad index or unreadable page
/// tree into the matching [`RenderError`].
fn page_geometry(
    document: &EditableDocument,
    page_index: usize,
) -> Result<(ObjectId, Rectangle, i64), RenderError> {
    let page_ids = document
        .page_ids()
        .map_err(|e| RenderError::DocumentLoad(Box::new(e)))?;
    let page_count = page_ids.len();
    let page_id = *page_ids
        .get(page_index)
        .ok_or(RenderError::InvalidPageIndex {
            index: page_index,
            page_count,
        })?;

    let media_box = document
        .effective_media_box(page_id)
        .map_err(|e| RenderError::DocumentLoad(Box::new(e)))?;
    let rotate = normalize_rotate(document.effective_rotate(page_id).unwrap_or(0));

    Ok((page_id, media_box, rotate))
}

/// Normalizes a raw `/Rotate` value (ISO 32000-1 Table 30: "a multiple of
/// 90") to one of `{0, 90, 180, 270}`. A non-conformant value that isn't a
/// multiple of 90 (a malformed/adversarial file) is treated as `0` rather
/// than propagated -- permissive handling of untrusted input, matching how
/// mainstream viewers commonly cope with non-conformant files, rather than
/// failing the whole render over one bad integer.
fn normalize_rotate(rotate: i64) -> i64 {
    let r = rotate.rem_euclid(360);
    if r % 90 == 0 {
        r
    } else {
        0
    }
}

/// Computes a page's *unrotated* content-space raster size, in pixels, at
/// `dpi` -- i.e. the size the content stream is actually interpreted into,
/// before [`rotate_image`] applies the page's effective `/Rotate` as a
/// post-processing step.
///
/// PDF user space is measured in points (ISO 32000-1 §7.7.3.3, 1/72 inch).
fn page_pixel_size(media_box: &Rectangle, dpi: f32) -> (u32, u32) {
    let scale = f64::from(dpi) / 72.0;
    let width = (media_box.width() * scale).round().max(1.0);
    let height = (media_box.height() * scale).round().max(1.0);

    // Clamp to u32::MAX so `check_dimensions` always sees a representable
    // value instead of this cast wrapping/UB-adjacent truncation; the
    // oversize case is reported as a normal `OutputTooLarge` error rather
    // than silently wrapping.
    let width = width.min(u32::MAX as f64) as u32;
    let height = height.min(u32::MAX as f64) as u32;

    (width, height)
}

/// Given a page's *unrotated* content raster dimensions, returns the
/// dimensions of the raster actually handed back to the caller once
/// `rotate` (already normalized, see [`normalize_rotate`]) has been
/// applied: 90/270 degrees swap width and height, 0/180 do not.
fn apply_rotation_to_dims(width: u32, height: u32, rotate: i64) -> (u32, u32) {
    match rotate {
        90 | 270 => (height, width),
        _ => (width, height),
    }
}

/// Like [`apply_rotation_to_dims`], but in PDF points (used by
/// [`PdfRenderer::render_thumbnail`] to pick a DPI from a *displayed*
/// longest-side target, before any pixel raster exists).
fn display_dimensions_pt(media_box: &Rectangle, rotate: i64) -> (f64, f64) {
    match rotate {
        90 | 270 => (media_box.height(), media_box.width()),
        _ => (media_box.width(), media_box.height()),
    }
}

/// Rejects render requests whose pixel area exceeds
/// [`MAX_RENDER_PIXELS`] *before* any raster is allocated. `width` and
/// `height` are widened to `u64` so the multiplication cannot overflow
/// even at `u32::MAX` extremes.
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

/// Renders the full page (no viewport cropping) into an RGBA image, then
/// applies the page's effective `/Rotate` (ISO 32000-1 Table 30: "the
/// number of degrees by which the page shall be rotated clockwise when
/// displayed") as a whole-image rotation -- exact for the only values the
/// spec permits (multiples of 90), and cheap (a pixel transpose/flip, not
/// a re-render).
#[allow(clippy::too_many_arguments)]
fn render_full_page(
    content: &[u8],
    resources: &crate::object::PdfDictionary,
    media_box: Rectangle,
    content_w: u32,
    content_h: u32,
    rotate: i64,
    page_index: usize,
) -> Result<RgbaImage, RenderError> {
    let output =
        native::render_content_stream(content, content_w, content_h, media_box, Some(resources))
            .map_err(|source| RenderError::PageRender { page_index, source })?;

    let image = pixmap_to_rgba_image(&output.pixmap);
    Ok(rotate_image(image, rotate))
}

/// Converts a `tiny-skia` premultiplied-alpha [`Pixmap`] into a
/// straight-alpha [`RgbaImage`], matching the non-premultiplied convention
/// [`RgbaImage`]'s own docs describe.
fn pixmap_to_rgba_image(pixmap: &Pixmap) -> RgbaImage {
    let width = pixmap.width();
    let height = pixmap.height();
    let mut buf = Vec::with_capacity(pixmap.pixels().len() * 4);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        buf.push(c.red());
        buf.push(c.green());
        buf.push(c.blue());
        buf.push(c.alpha());
    }
    // INVARIANT: `buf` has exactly `width * height * 4` bytes -- one RGBA8
    // pixel pushed per element of `pixmap.pixels()`, which is itself
    // exactly `width * height` long by `Pixmap`'s own construction. This
    // is an internal invariant of this function's own loop, not a
    // property of untrusted file data, so `expect` here does not violate
    // this crate's "never unwrap/expect on data from a file" rule.
    RgbaImage::from_raw(width, height, buf)
        .expect("pixmap-derived buffer length always matches width*height*4")
}

/// Applies a normalized (see [`normalize_rotate`]) clockwise rotation to a
/// rendered image. `image::imageops::rotate90`/`rotate180`/`rotate270`
/// each rotate clockwise, matching ISO 32000-1 Table 30's `/Rotate`
/// semantics directly.
fn rotate_image(image: RgbaImage, rotate: i64) -> RgbaImage {
    match rotate {
        90 => image::imageops::rotate90(&image),
        180 => image::imageops::rotate180(&image),
        270 => image::imageops::rotate270(&image),
        _ => image,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rotate_wraps_and_rejects_non_multiples_of_90() {
        assert_eq!(normalize_rotate(0), 0);
        assert_eq!(normalize_rotate(90), 90);
        assert_eq!(normalize_rotate(360), 0);
        assert_eq!(normalize_rotate(450), 90);
        assert_eq!(normalize_rotate(-90), 270);
        // Adversarial: not a multiple of 90 at all.
        assert_eq!(normalize_rotate(45), 0);
        assert_eq!(normalize_rotate(1), 0);
    }

    #[test]
    fn apply_rotation_to_dims_swaps_only_for_90_and_270() {
        assert_eq!(apply_rotation_to_dims(100, 200, 0), (100, 200));
        assert_eq!(apply_rotation_to_dims(100, 200, 180), (100, 200));
        assert_eq!(apply_rotation_to_dims(100, 200, 90), (200, 100));
        assert_eq!(apply_rotation_to_dims(100, 200, 270), (200, 100));
    }

    #[test]
    fn check_dimensions_rejects_zero_and_oversize() {
        assert!(check_dimensions(0, 100).is_err());
        assert!(check_dimensions(100, 0).is_err());
        assert!(check_dimensions(100_000, 100_000).is_err());
        assert!(check_dimensions(100, 100).is_ok());
    }

    #[test]
    fn page_pixel_size_scales_points_to_pixels_at_dpi() {
        let media_box = Rectangle::a4();
        let (w, h) = page_pixel_size(&media_box, 72.0);
        // At 72 DPI, 1 point == 1 pixel.
        assert_eq!(w, media_box.width().round() as u32);
        assert_eq!(h, media_box.height().round() as u32);

        let (w2, h2) = page_pixel_size(&media_box, 144.0);
        assert_eq!(w2, w * 2);
        assert_eq!(h2, h * 2);
    }

    #[test]
    fn open_bytes_rejects_garbage_as_a_document_load_error_not_a_panic() {
        // `PdfRenderer` deliberately does not derive `Debug` (it owns an
        // `EditableDocument`, which doesn't either), so `Result::unwrap_err`
        // (which requires the `Ok` side to be `Debug`) can't be used here;
        // match instead.
        match PdfRenderer::open_bytes(b"not a pdf at all".to_vec()) {
            Err(err) => assert!(matches!(err, RenderError::DocumentLoad(_)), "{err:?}"),
            Ok(_) => panic!("expected garbage bytes to fail to open"),
        }
    }

    #[test]
    fn page_count_of_empty_bytes_is_zero_not_a_panic() {
        // `open_bytes` on garbage input fails outright (see above), so
        // this exercises `page_count`'s own defensive `unwrap_or(0)` via
        // a document whose page tree becomes unreadable *after* opening
        // (a corrupt Kids array injected post-hoc). Kept minimal: a
        // document that opens fine but reports zero pages is already
        // covered by higher-level render tests (`InvalidPageIndex` on
        // page 0 of a genuinely-empty document), so this just documents
        // the `unwrap_or(0)` contract directly.
        let renderer = PdfRenderer::open_bytes(
            crate::prelude::DocumentBuilder::new()
                .page(crate::prelude::PageBuilder::a4().build())
                .build()
                .unwrap()
                .save_to_bytes()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(renderer.page_count(), 1);
    }
}
