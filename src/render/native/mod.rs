//! A pure-Rust content-stream interpreter and 2D rasterizer -- no native
//! binary, no FFI, at all. Gated behind the `native-render` Cargo feature.
//!
//! [`crate::render::PdfRenderer`] (feature `render`) is the whole-document
//! API built on top of this module: it resolves a page's effective
//! `/MediaBox`/`/Rotate`/`/Resources`/content streams and hands the
//! content stream to [`render_content_stream`] below, which has no PDF
//! *document* (page tree, xref) access of its own -- see
//! [`crate::render`]'s module docs for how the two layers fit together and
//! for the migration history that led here. The 2D rasterizer backend is
//! [`tiny-skia`](https://docs.rs/tiny-skia) (pure Rust, BSD-3-Clause);
//! font outline extraction uses `ttf-parser` (this crate's `fonts`
//! feature, pulled in automatically by `native-render`).
//!
//! # Current phase: "Text Rendering"
//!
//! Building on the prior "Content-Stream Interpreter Core" phase (vector
//! graphics: graphics-state stack, path construction/painting, clipping,
//! basic ExtGState, Device color spaces -- see `interpreter`'s docs for
//! that operator table), this phase adds ISO 32000-1:2008 Chapter 9 text
//! showing:
//!
//! - Text-positioning operators: `Tf Td TD Tm T*` (9.4.2), plus the text
//!   state operators `Tc Tw Tz TL Tr Ts` (9.3) and `BT`/`ET` (9.4.1).
//! - Text-showing operators: `Tj TJ '` `"` (9.4.3), including per-glyph
//!   advance computation (9.4.4: `Tc`/`Tw`/`Tz` and `TJ` number
//!   adjustments).
//! - **Simple TrueType/OpenType fonts** (9.6.3): character code ->
//!   Unicode (via [`crate::font::encoding`]'s WinAnsi-ish fallback, same
//!   table [`crate::editor::text_extract`] already uses) -> glyph ID via
//!   the embedded font's own `cmap`, outline extracted with `ttf-parser`
//!   and converted to a `tiny-skia` path (see `glyph`).
//! - **Composite (Type 0/CIDFontType2) fonts** (9.7), including CJK:
//!   glyph selection reuses the code/CID/GID conventions established by
//!   [`crate::font::cid::CompositeFont`] (this crate's own writer) --
//!   2-byte `Identity-H` codes, honoring an explicit `/CIDToGIDMap` if
//!   present (see `font::resolve_font`'s docs for exactly what "reuse"
//!   means here, since the writer and this reader are necessarily
//!   different code paths).
//! - **Type 3 fonts** (9.6.5): each glyph's `CharProc` is itself a
//!   content stream, run *recursively* through this same interpreter
//!   (`interpreter::Interpreter::run_type3_glyph`), bounded by
//!   [`font::MAX_TYPE3_DEPTH`] against a self-referential/infinite Type 3
//!   font (untrusted input).
//!
//! See the `interpreter`/`font`/`glyph` submodules' docs for the exact
//! operator dispatch table and font-resolution rules.
//!
//! # Current phase: "Color Spaces & Images"
//!
//! Building on the above, this phase adds:
//!
//! - **Indexed** and **Separation**/**DeviceN** colour spaces (ISO
//!   32000-1:2008 §8.6.6.3-§8.6.6.5) via the `cs`/`CS`/`sc`/`SC`/`scn`/
//!   `SCN` operators, including real evaluation (not a stub) of the
//!   Separation/DeviceN tint-transform function -- Types 0 (Sampled), 2
//!   (Exponential), 3 (Stitching) and 4 (PostScript calculator); see
//!   [`function`]'s docs for the operator subset Type 4 implements.
//! - **ICCBased** colour spaces -- **approximated**, not colour-managed:
//!   resolved to `/Alternate` if present, else a heuristic guess from
//!   `/N`. There is no mature pure-Rust ICC engine this crate could
//!   adopt; see [`colorspace`]'s docs for exactly what "approximated"
//!   means here and [`error::RenderWarning::IccColorApproximated`] for how
//!   this is surfaced to callers (never silently claimed as accurate).
//! - Image XObjects (`Do`) and inline images (`BI`/`ID`/`EI`), wired to
//!   the filter decoders already implemented in [`crate::filter`] (reused,
//!   not reimplemented) -- see [`image`]'s docs for the exact scope,
//!   including the **explicit, hard gap**: `JBIG2Decode` and `JPXDecode`
//!   have no mature pure-Rust decoder in this ecosystem, so those images
//!   render as a documented, structured placeholder (never silently
//!   blank, never a panic) -- see [`image`] and
//!   [`error::RenderWarning::UnsupportedImageFilter`].
//!
//! See the `colorspace`/`function`/`image` submodules' docs for full
//! detail.
//!
//! # Current phase: "Transparency & Blend Modes"
//!
//! Building on the above, this phase adds ISO 32000-1:2008 Chapter 11
//! ("Transparency"):
//!
//! - **Blend modes** (§11.3.5, `/BM` in an ExtGState): all 16 standard
//!   blend modes (`Normal`/`Compatible`, `Multiply`, `Screen`, `Overlay`,
//!   `Darken`, `Lighten`, `ColorDodge`, `ColorBurn`, `HardLight`,
//!   `SoftLight`, `Difference`, `Exclusion`, and the non-separable `Hue`,
//!   `Saturation`, `Color`, `Luminosity`) map 1:1 onto
//!   `tiny_skia::BlendMode`, which implements the same compositing
//!   formulas the spec defines -- a real implementation, not an
//!   approximation. Applies to fills, strokes, glyphs and images alike.
//!   An unrecognised name falls back to `Normal` per the spec's own
//!   fallback rule, recording [`error::RenderWarning::UnsupportedBlendMode`].
//! - **Transparency groups** (§11.4, Form XObjects with `/Group
//!   /S /Transparency`): Form XObjects are now painted at all (previously
//!   an unconditional gap). A transparency-group Form painted via `Do` is
//!   rendered into an isolated offscreen buffer (contents composite among
//!   themselves at full opacity/`Normal` blend), and *that result* is
//!   composited into the page using the outer `ca`/blend mode -- the
//!   detail that makes "several overlapping semi-transparent shapes
//!   forming one semi-transparent group" render correctly instead of each
//!   overlap darkening further. **Approximation, documented**: every
//!   group is treated as isolated regardless of its actual `/I` entry,
//!   and knockout (`/K`) is not implemented.
//! - **Soft masks** (§11.6): both kinds --
//!   - An ExtGState `/SMask` (§11.6.4.3/§11.6.5.2) naming a Luminosity- or
//!     Alpha-type mask group is rendered (once, at `gs` time, against the
//!     CTM then in effect) into a canvas-sized 8-bit mask and applied to
//!     every subsequent paint until cleared (`/SMask /None`) or replaced.
//!     **Approximation, documented**: `/BC` (custom backdrop colour) and
//!     `/TR` (transfer function) are recognised but ignored (identity
//!     transfer function; the spec's own Luminosity-is-black/
//!     Alpha-is-transparent default backdrop is always used), recording
//!     [`error::RenderWarning::SoftMaskParameterIgnored`].
//!   - An image XObject's own `/SMask` (§11.6.5.3, a companion DeviceGray
//!     alpha-channel image) is decoded and applied as that image's
//!     per-pixel alpha -- previously an unconditional gap ("every image
//!     is fully opaque"). **Approximation, documented**: resampled with
//!     nearest-neighbor if dimensions differ from the base image; `/Matte`
//!     (un-premultiplication of matted colour) is not implemented. A
//!     decode failure here doesn't fail the whole image -- it paints
//!     fully opaque instead, recording
//!     [`error::RenderWarning::ImageSoftMaskDecodeFailed`].
//! - `ca`/`CA` (constant alpha, ISO 32000-1 §11.3.7.2) were already
//!   implemented in the prior "Content-Stream Interpreter Core" phase;
//!   unchanged here.
//!
//! Both Form XObject recursion contexts introduced this phase (a
//! transparency group's own content invoking `Do` again, and an ExtGState
//! `/SMask`'s group doing the same) are bounded by
//! [`interpreter::MAX_FORM_XOBJECT_DEPTH`], shared with (not reset across)
//! the existing Type 3 glyph recursion depth counter, against a
//! self-referential/mutually-recursive set of Form XObjects (untrusted
//! input) -- see [`error::RenderWarning::FormXObjectRecursionLimitExceeded`].
//!
//! See the `interpreter`/`image` submodules' docs for full detail.
//!
//! # Explicit, honest gaps (not implemented, not silently faked)
//!
//! The following are **out of scope for this phase** and are recorded as
//! a structured [`RenderWarning::UnsupportedOperator`] (or a dedicated
//! variant) rather than being silently mis-rendered, guessed at, or
//! causing a panic:
//!
//! - **Type1 and bare/un-wrapped CFF embedded font programs are a
//!   documented, structural gap: no mature pure-Rust Type1
//!   (`eexec`-encrypted charstring) or bare-CFF interpreter exists in
//!   this ecosystem today.** `ttf-parser` (this crate's only font-parsing
//!   dependency) requires an `sfnt`/OpenType table directory; it cannot
//!   parse a raw Type1 `PFA`/`PFB` program or a `FontFile3` `/Type1C`/
//!   `/CIDFontType0C` stream that isn't wrapped in an OpenType container.
//!   Text using such a font renders **nothing** for that font (but still
//!   advances the pen using its declared `/Widths`/`/W`, so surrounding
//!   text doesn't visually collapse), with
//!   [`RenderWarning::UnsupportedFontProgram`] recorded once -- never a
//!   panic, never a silently fabricated placeholder box mislabeled as
//!   "rendered". OpenType fonts whose outlines merely *happen* to be
//!   CFF-flavored (a proper `sfnt` container) are **not** part of this
//!   gap -- `ttf-parser` genuinely parses those, so this really is
//!   supported, not a fallback wearing a disguise. See `font`'s module
//!   docs for the exact classification logic.
//! - **Non-embedded fonts are also not rendered** -- this phase has no
//!   standard/system-font substitution database at all (unlike a mature
//!   desktop PDF viewer). Any font (of *any* `/Subtype`, including TrueType) with no
//!   `FontFile`/`FontFile2`/`FontFile3` fails the same way as the
//!   Type1/CFF gap above. This is a distinct, separately-documented gap
//!   from Type1/CFF -- do not conflate "we have no charstring
//!   interpreter" with "we have no font substitution logic".
//! - **Only horizontal writing mode** -- vertical CID fonts
//!   (`Identity-V` and friends) are not detected; text is always
//!   positioned as if horizontal.
//! - **Only 2-byte codes are assumed for every composite (Type 0) font**
//!   -- matches the same documented simplification already shipped in
//!   [`crate::editor::text_extract`] (this crate's own writer only ever
//!   emits `Identity-H`). A composite font genuinely using a different
//!   CMap will be mis-chunked.
//! - **`/Encoding` `/Differences` and symbolic (non-Unicode-`cmap`)
//!   simple fonts** are not specially resolved -- same documented gap as
//!   `text_extract` (needs the Adobe Glyph List, not implemented).
//! - **Text clipping render modes** (`Tr` 4-7) paint the same as their
//!   non-clipping counterpart (0-3) but do not add glyph outlines to the
//!   clip path -- an intentional simplification (see
//!   `state::TextState::render_mode`'s docs), not silent data loss.
//! - **Form XObjects are now painted** (as of the "Transparency & Blend
//!   Modes" phase above) -- both directly and as transparency groups; only
//!   an XObject `/Subtype` that's neither `/Image` nor `/Form` (e.g.
//!   `/PS`) is still unpainted, recorded as
//!   [`RenderWarning::UnsupportedXObjectSubtype`].
//! - **Shadings** (`sh`) and **Patterns** -- not painted. `scn`/`SCN`
//!   naming a Pattern colour records
//!   [`RenderWarning::PatternColorUnsupported`] and leaves the current
//!   colour unchanged.
//! - **Lab colour space** -- not implemented (would need CIE L\*a\*b\* ->
//!   device-RGB conversion this phase doesn't have); resolves to
//!   [`colorspace::ColorSpace::Unsupported`], recorded as
//!   [`RenderWarning::UnsupportedColorSpace`]. CalGray/CalRGB, by
//!   contrast, *are* handled -- approximated as their Device equivalent
//!   with no gamma/white-point calibration applied (a minor, common
//!   simplification, not separately warned about).
//! - **Image `/SMask` soft masks are now applied** (as of the
//!   "Transparency & Blend Modes" phase above) -- see [`image`]'s docs.
//!   The older, pre-`/SMask` explicit-mask mechanism (`/Mask`, ISO
//!   32000-1 §8.9.6.4: a colour-key array or stencil-image mask) is
//!   *not* implemented -- every image this phase paints ignores a
//!   `/Mask` entry. Not specially warned about per-image (would be
//!   noisy); documented here as a known simplification.
//! - **ICC color management** -- ICCBased colour spaces are
//!   *approximated* (resolved to `/Alternate` or an `/N`-based heuristic
//!   guess -- see [`colorspace`]'s docs), not colour-managed; there is no
//!   mature pure-Rust ICC engine this crate has adopted. Likewise, the
//!   CMYK->RGB conversion this phase uses for DeviceCMYK
//!   (`color::device_cmyk`) is the naive, non-color-managed formula ISO
//!   32000-1 8.6.5.3 itself documents as the fallback conversion. Neither
//!   is true ICC-profile-based color management; accurate/perceptual
//!   color reproduction against a specific ICC profile remains
//!   unimplemented, and is recorded once via
//!   [`RenderWarning::IccColorApproximated`] whenever an ICCBased space is
//!   actually used.
//! - **Transparency groups and soft masks are approximated, not
//!   spec-exact** (see the "Transparency & Blend Modes" phase section
//!   above for the precise list): every group is treated as isolated
//!   regardless of `/I`, knockout (`/K`) is not implemented, and an
//!   ExtGState `/SMask`'s `/BC`/`/TR` entries are ignored. Blend modes
//!   themselves (`/BM`) *are* fully, accurately implemented (all 16
//!   standard modes) -- it is specifically group/mask compositing
//!   *semantics* beyond isolated-groups-with-default-backdrops that are
//!   approximated here, not the per-pixel blend formulas.
//! - **Non-uniform-scale/skewed stroke width and dash length** -- this
//!   phase approximates the CTM's effect on user-space line width/dash
//!   lengths by its uniform scale factor (`sqrt(|det(CTM)|)`); see
//!   `interpreter::StrokeParams::build` for the precise caveat. A
//!   heavily skewed CTM will not produce the spec-exact elliptical pen
//!   shape.
//!
//! None of the above raise a hard [`NativeRenderError`] -- the render
//! still completes and still produces every pixel this phase *does* know
//! how to paint. Only structurally-impossible requests (zero-size output,
//! a degenerate `/MediaBox`, a `q`-flood past
//! [`interpreter::MAX_GRAPHICS_STATE_DEPTH`]) are hard errors.
//!
//! # Pre-resolved `/Resources` assumption
//!
//! Like the ExtGState lookups in the prior phase, font resolution here
//! expects `resources` (and everything reachable from it: the `/Font`
//! subdictionary, each font's `/FontDescriptor`, `/DescendantFonts`,
//! `/CharProcs`, and any embedded `FontFile*`/`CIDToGIDMap` streams) to
//! already be fully dereferenced -- `Object::Dictionary`/`Object::Stream`
//! values, not dangling `Object::Reference`s. This module has no
//! `PdfReader`/document access of its own to resolve indirect references;
//! that is the caller's responsibility (a future phase wiring this up to
//! whole-document rendering will need to walk the xref table once before
//! calling [`render_content_stream`]). An indirect reference found where
//! a dictionary/stream was expected is treated the same as "absent" --
//! e.g. a font with no readable `/FontDescriptor` is
//! [`font::UnsupportedFontReason::NotEmbedded`] -- rather than panicking
//! or trying to guess at a document it cannot see.
//!
//! # Untrusted input handling
//!
//! Content-stream bytes are untrusted (an arbitrary/possibly-adversarial
//! PDF). This module:
//! - never `unwrap()`/`expect()`s on data decoded from the stream (see
//!   `interpreter.rs`'s `nums`/`as_f64` helpers, which return `None` and
//!   skip the operator invocation rather than panicking on a malformed or
//!   non-finite operand);
//! - bounds the `q`/`Q` stack depth (
//!   [`interpreter::MAX_GRAPHICS_STATE_DEPTH`]) against a "q flood" trying
//!   to force unbounded `Vec` growth;
//! - caps the number of [`RenderWarning`]s collected so a stream consisting
//!   of millions of unsupported operators can't itself force unbounded
//!   allocation;
//! - sanitizes non-finite (`NaN`/`Infinity`) coordinates to a finite
//!   fallback, **and** clamps any finite-but-absurdly-large coordinate
//!   magnitude, before either reaches the rasterizer (see
//!   `path::sanitize_point`/`path::MAX_COORDINATE_MAGNITUDE`). The
//!   magnitude clamp exists because -- contrary to what this section
//!   previously (incorrectly) claimed -- `tiny-skia` does **not** always
//!   gracefully refuse geometry whose magnitude overflows its internal
//!   math: the "Security Hardening" phase's `render_interpreter`
//!   cargo-fuzz target found a small content stream with one huge-but-
//!   finite path coordinate that trips an internal `assert!` in
//!   `tiny_skia::scan::path::fill_path_impl` and aborts the process. See
//!   `path::MAX_COORDINATE_MAGNITUDE`'s docs and `docs/THREAT_MODEL.md`
//!   for the full account of that finding and this crate's defensive
//!   clamp (in this crate's own code, not a patch to `tiny-skia` itself)
//!   that closes it;
//! - bounds the total operator/inline-image count and wall-clock time
//!   spent interpreting one render
//!   ([`interpreter::MAX_OPERATOR_COUNT`]/[`interpreter::MAX_RENDER_DURATION`])
//!   against a content stream that is very long but never deeply nested
//!   (so none of the recursion-depth caps above would otherwise bound it),
//!   and bounds the number of points a single path object may accumulate
//!   (`path::MAX_PATH_POINTS_PER_PATH`) independent of that operator count.
//!
//! # Example
//!
//! ```no_run
//! # #[cfg(feature = "native-render")]
//! # fn main() {
//! use rust_pdf::render::native::render_content_stream;
//! use rust_pdf::types::Rectangle;
//!
//! let content = b"1 0 0 rg 100 100 200 150 re f";
//! let media_box = Rectangle::new(0.0, 0.0, 612.0, 792.0);
//! let output = render_content_stream(content, 612, 792, media_box, None).unwrap();
//! assert!(output.warnings.is_empty());
//! // Raw premultiplied RGBA8 bytes, ready for a GPU texture upload or a
//! // PNG encoder of the caller's choosing (this crate's `native-render`
//! // feature does not itself pull in `tiny-skia`'s optional PNG codec).
//! let _rgba_bytes: &[u8] = output.pixmap.data();
//! # }
//! # #[cfg(not(feature = "native-render"))]
//! # fn main() {}
//! ```

mod bits;
mod color;
mod colorspace;
mod error;
mod font;
mod function;
mod glyph;
mod image;
mod interpreter;
mod path;
mod state;

pub use error::{NativeRenderError, RenderWarning};
pub use interpreter::{render_content_stream, NativeRenderOutput, MAX_GRAPHICS_STATE_DEPTH};

/// Re-export of the `tiny-skia` pixel buffer type produced by
/// [`render_content_stream`], so callers don't need a direct `tiny-skia`
/// dependency just to hold the result. Use `.data()` for raw premultiplied
/// RGBA8 bytes or `.pixel(x, y)` for one pixel. Note that `.save_png(path)`/
/// `.encode_png()` are **not** available through this crate's `tiny-skia`
/// dependency: `native-render` deliberately builds `tiny-skia` with its
/// optional `png-format` feature (and thus the `png` crate) disabled,
/// since this crate has no need to encode PNGs itself -- callers wanting
/// PNG output should encode `.data()` with their own `image`/`png`
/// dependency (e.g. this crate's own `images` feature, if enabled).
pub use tiny_skia::Pixmap;

#[cfg(test)]
mod image_integration_tests;
#[cfg(test)]
mod text_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Rectangle;

    fn page() -> Rectangle {
        Rectangle::new(0.0, 0.0, 200.0, 200.0)
    }

    fn pixel(output: &NativeRenderOutput, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let p = output
            .pixmap
            .pixel(x, y)
            .unwrap_or_else(|| panic!("pixel ({x},{y}) out of bounds"))
            .demultiply();
        (p.red(), p.green(), p.blue(), p.alpha())
    }

    /// Test 1: A filled rectangle in pure red lands at the expected device
    /// pixels and nowhere else.
    #[test]
    fn fills_red_rectangle_at_expected_position() {
        let content = b"1 0 0 rg 50 50 100 100 re f";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        assert!(out.warnings.is_empty());

        // MediaBox 0..200 maps 1:1 to device pixels; PDF y-up flips to
        // device y-down, so user-space rect [50,50]-[150,150] should
        // land at device y in [50,150] too (200-150=50, 200-50=150).
        assert_eq!(pixel(&out, 100, 100), (255, 0, 0, 255));
        // Outside the rectangle: still the white background.
        assert_eq!(pixel(&out, 5, 5), (255, 255, 255, 255));
        assert_eq!(pixel(&out, 195, 195), (255, 255, 255, 255));
    }

    /// Test 2: DeviceGray fill.
    #[test]
    fn fills_gray_rectangle() {
        let content = b"0.5 g 0 0 200 200 re f";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        let (r, g, b, a) = pixel(&out, 100, 100);
        assert_eq!((r, g, b), (128, 128, 128));
        assert_eq!(a, 255);
    }

    /// Test 3: DeviceCMYK fill (pure yellow: C=0 M=0 Y=1 K=0 -> RGB (255,255,0)).
    #[test]
    fn fills_cmyk_yellow_rectangle() {
        let content = b"0 0 1 0 k 0 0 200 200 re f";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        assert_eq!(pixel(&out, 100, 100), (255, 255, 0, 255));
    }

    /// Test 4: `q`/`Q` isolate color changes: after `Q`, the outer color is
    /// restored for a second fill.
    #[test]
    fn q_q_restores_prior_fill_color() {
        let content = b"\
            1 0 0 rg \
            q 0 1 0 rg 0 0 50 50 re f Q \
            0 0 200 200 re f";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        // Inside the inner q..Q, green was filled first...
        // ...but the outer red fill afterward paints over the whole
        // canvas, so the final pixel is red everywhere.
        assert_eq!(pixel(&out, 10, 190), (255, 0, 0, 255));
    }

    /// Test 5: `q`/`Q` correctly isolates the CTM too: a `cm` inside `q ... Q`
    /// does not leak out.
    #[test]
    fn q_q_restores_ctm() {
        let content = b"\
            q 2 0 0 2 0 0 cm Q \
            1 0 0 rg 10 10 20 20 re f";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        // If the 2x scale had leaked, the rect would cover (20,20)-(80,80)
        // device-ish; instead it must be the unscaled 10..30 square.
        assert_eq!(pixel(&out, 20, 170), (255, 0, 0, 255)); // inside unscaled rect
        assert_eq!(pixel(&out, 60, 130), (255, 255, 255, 255)); // would be red if leaked
    }

    /// Test 6: A stroked line paints along the expected path.
    #[test]
    fn strokes_a_horizontal_line() {
        let content = b"0 0 1 RG 10 w 0 100 m 200 100 l S";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        // Center of the 10-wide horizontal stroke at y=100 (device y=100).
        assert_eq!(pixel(&out, 100, 100), (0, 0, 255, 255));
        // Far above/below the stroke should remain background white.
        assert_eq!(pixel(&out, 100, 10), (255, 255, 255, 255));
        assert_eq!(pixel(&out, 100, 190), (255, 255, 255, 255));
    }

    /// Test 7: Even-odd fill rule: two overlapping rects with `f*` should
    /// leave the intersection unpainted (donut shape), while nonzero `f`
    /// would fill it solid.
    #[test]
    fn even_odd_fill_rule_creates_hole() {
        let content = b"1 0 0 rg 0 0 150 150 re 25 25 100 100 re f*";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        // Outer ring painted...
        assert_eq!(pixel(&out, 10, 190), (255, 0, 0, 255));
        // ...but the doubly-covered inner region is NOT painted (hole),
        // since even-odd XORs overlapping subpaths.
        assert_eq!(pixel(&out, 75, 125), (255, 255, 255, 255));
    }

    /// Test 8: Clipping: content painted outside the clip rectangle must not
    /// appear, even though the fill operator covers the whole canvas.
    #[test]
    fn clip_rect_restricts_subsequent_fill() {
        let content = b"50 50 100 100 re W n 1 0 0 rg 0 0 200 200 re f";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        // Inside the clip: painted red.
        assert_eq!(pixel(&out, 100, 100), (255, 0, 0, 255));
        // Outside the clip: still background white, despite the fill
        // operator's rectangle covering the entire canvas.
        assert_eq!(pixel(&out, 5, 5), (255, 255, 255, 255));
        assert_eq!(pixel(&out, 195, 195), (255, 255, 255, 255));
    }

    /// Test 9: `gs` ExtGState alpha (`ca`) actually blends instead of fully
    /// covering the white background.
    #[test]
    fn ext_gstate_alpha_blends_with_background() {
        use crate::object::{Object, PdfDictionary};

        let mut gs1 = PdfDictionary::new();
        gs1.set("ca", Object::Real(0.5));
        let mut extgstate = PdfDictionary::new();
        extgstate.set("GS1", Object::Dictionary(gs1));
        let mut resources = PdfDictionary::new();
        resources.set("ExtGState", Object::Dictionary(extgstate));

        let content = b"/GS1 gs 1 0 0 rg 0 0 200 200 re f";
        let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
        assert!(out.warnings.is_empty());
        // 50% red over white background -> (255,127or128,127or128).
        let (r, g, b, a) = pixel(&out, 100, 100);
        assert_eq!(r, 255);
        assert!((126..=129).contains(&g), "unexpected green: {g}");
        assert_eq!(g, b);
        assert_eq!(a, 255);
    }

    /// Test 10: `Do` naming an XObject resource that doesn't exist at all
    /// (no `/Resources` supplied, here) is a warning, not a hard failure --
    /// the rest of the (graphics) content stream still renders. (Image
    /// XObjects that *do* resolve are now painted -- see the
    /// `render::native::image` module's own tests -- this test now
    /// exercises the "missing resource" gracefully-skipped path instead of
    /// the old blanket "Do is unimplemented" gap it used to.)
    #[test]
    fn unsupported_operator_is_a_warning_not_a_failure() {
        let content = b"/Im1 Do 0 1 0 rg 0 0 200 200 re f";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        assert_eq!(pixel(&out, 100, 100), (0, 255, 0, 255));
        assert!(!out.warnings.is_empty());
        assert!(out
            .warnings
            .iter()
            .any(|w| matches!(w, RenderWarning::MissingXObjectResource { name } if name == "Im1")));
    }

    /// Text-showing operators (`BT`/`Tf`/`Tj`/`ET`) are now implemented
    /// (this phase adds text rendering); a `Tf` naming a font resource
    /// that doesn't exist (no `/Resources` supplied at all, here) is a
    /// warning, not a hard failure or a panic -- the rest of the content
    /// stream (the green fill) still renders.
    #[test]
    fn tf_with_missing_font_resource_is_a_warning_not_a_failure() {
        let content = b"BT /F1 12 Tf (Hello) Tj ET 0 1 0 rg 0 0 200 200 re f";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        assert_eq!(pixel(&out, 100, 100), (0, 255, 0, 255));
        assert!(out
            .warnings
            .iter()
            .any(|w| matches!(w, RenderWarning::MissingFontResource { name } if name == "F1")));
    }

    /// Test 11: Adversarial input: zero/degenerate output dimensions and
    /// media box are rejected as structured errors, not a panic.
    #[test]
    fn degenerate_inputs_are_structured_errors_not_panics() {
        let err = render_content_stream(b"", 0, 100, page(), None).unwrap_err();
        assert!(matches!(err, NativeRenderError::InvalidDimensions { .. }));

        let degenerate_box = Rectangle::new(10.0, 10.0, 10.0, 10.0);
        let err = render_content_stream(b"", 100, 100, degenerate_box, None).unwrap_err();
        assert!(matches!(err, NativeRenderError::DegenerateMediaBox { .. }));
    }

    /// Test 12: Adversarial input: a pathological `q` flood is rejected with a
    /// bounded, structured error instead of exhausting memory.
    #[test]
    fn graphics_state_stack_overflow_is_a_structured_error() {
        let flood = "q ".repeat(MAX_GRAPHICS_STATE_DEPTH + 10);
        let err = render_content_stream(flood.as_bytes(), 10, 10, page(), None).unwrap_err();
        assert!(matches!(
            err,
            NativeRenderError::GraphicsStateStackOverflow { .. }
        ));
    }

    /// Test 13: Adversarial/malformed input: a truncated/unterminated
    /// statement at the end of the stream doesn't panic and doesn't
    /// discard the well-formed content before it.
    #[test]
    fn truncated_trailing_bytes_do_not_panic_or_lose_prior_content() {
        let content = b"1 0 0 rg 0 0 200 200 re f (unterminated";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        assert_eq!(pixel(&out, 100, 100), (255, 0, 0, 255));
        assert!(out
            .warnings
            .iter()
            .any(|w| matches!(w, RenderWarning::TruncatedContentStream)));
    }

    // ---- Transparency & blend modes (ISO 32000-1:2008 Chapter 11) ----

    /// Builds an ExtGState dictionary (`/ca`/`/CA`/`/BM`/`/SMask`, whatever
    /// `configure` sets) named `name` inside `resources`'s `/ExtGState`.
    fn add_ext_gstate(resources: &mut crate::object::PdfDictionary, name: &str, configure: impl FnOnce(&mut crate::object::PdfDictionary)) {
        use crate::object::{Object, PdfDictionary};
        let mut gs = PdfDictionary::new();
        configure(&mut gs);
        let mut extgstate = match resources.get("ExtGState").cloned() {
            Some(Object::Dictionary(d)) => d,
            _ => PdfDictionary::new(),
        };
        extgstate.set(name, Object::Dictionary(gs));
        resources.set("ExtGState", Object::Dictionary(extgstate));
    }

    /// Test 14: `/BM /Multiply` (ISO 32000-1 §11.3.5.2): a cyan rectangle
    /// painted on top of an opaque yellow one with `Multiply` blend mode
    /// produces green (`Multiply(Cs,Cb) = Cs*Cb`), not cyan (which is what
    /// plain `Normal` compositing of an opaque source would give).
    #[test]
    fn blend_mode_multiply_darkens_overlap() {
        use crate::object::PdfDictionary;

        let mut resources = PdfDictionary::new();
        add_ext_gstate(&mut resources, "GSm", |gs| {
            gs.set("BM", crate::object::Object::Name(crate::object::PdfName::new_unchecked("Multiply")));
        });

        let content = b"\
            1 1 0 rg 0 0 200 200 re f \
            /GSm gs \
            0 1 1 rg 0 0 200 200 re f";
        let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
        assert!(out.warnings.is_empty());
        // Multiply(cyan=(0,255,255), yellow=(255,255,0)) = (0,255,0).
        assert_eq!(pixel(&out, 100, 100), (0, 255, 0, 255));
    }

    /// Test 15: `/BM /Screen` (ISO 32000-1 §11.3.5.2):
    /// `Screen(Cs,Cb) = 255 - (255-Cs)*(255-Cb)/255`. Using
    /// `Cs=200, Cb=100` gives a lighter result (~222) than either input --
    /// distinguishing `Screen` from `Multiply`/`Normal`.
    #[test]
    fn blend_mode_screen_lightens_overlap() {
        use crate::object::PdfDictionary;

        let mut resources = PdfDictionary::new();
        add_ext_gstate(&mut resources, "GSs", |gs| {
            gs.set("BM", crate::object::Object::Name(crate::object::PdfName::new_unchecked("Screen")));
        });

        let base = 100.0 / 255.0;
        let src = 200.0 / 255.0;
        let content = format!(
            "{base} {base} {base} rg 0 0 200 200 re f /GSs gs {src} {src} {src} rg 0 0 200 200 re f"
        );
        let out = render_content_stream(content.as_bytes(), 200, 200, page(), Some(&resources)).unwrap();
        assert!(out.warnings.is_empty());
        let (r, g, b, a) = pixel(&out, 100, 100);
        let expected: i32 = 255 - ((255 - 200) * (255 - 100)) / 255;
        assert!((i32::from(r) - expected).abs() <= 2, "r={r} expected~{expected}");
        assert_eq!(r, g);
        assert_eq!(g, b);
        assert_eq!(a, 255);
        // Screen must lighten past both inputs.
        assert!(r > 200, "screen result {r} should exceed both inputs (100, 200)");
    }

    /// Test 16: `/BM /Darken` and `/BM /Lighten` (ISO 32000-1 §11.3.5.2)
    /// pick the per-channel min/max of source and backdrop respectively.
    #[test]
    fn blend_mode_darken_and_lighten_pick_min_max() {
        use crate::object::PdfDictionary;

        let mut resources = PdfDictionary::new();
        add_ext_gstate(&mut resources, "GSd", |gs| {
            gs.set("BM", crate::object::Object::Name(crate::object::PdfName::new_unchecked("Darken")));
        });
        add_ext_gstate(&mut resources, "GSl", |gs| {
            gs.set("BM", crate::object::Object::Name(crate::object::PdfName::new_unchecked("Lighten")));
        });

        // Left half: backdrop 50 gray, source 180 gray, Darken -> 50.
        // Right half: backdrop 50 gray, source 180 gray, Lighten -> 180.
        let content = b"\
            0.196 0.196 0.196 rg 0 0 100 200 re f \
            /GSd gs 0.706 0.706 0.706 rg 0 0 100 200 re f \
            0.196 0.196 0.196 rg 100 0 100 200 re f \
            /GSl gs 0.706 0.706 0.706 rg 100 0 100 200 re f";
        let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
        assert!(out.warnings.is_empty());
        let (r, _, _, _) = pixel(&out, 50, 100);
        assert!(r <= 55, "Darken should keep the darker backdrop, got {r}");
        let (r2, _, _, _) = pixel(&out, 150, 100);
        assert!(r2 >= 175, "Lighten should keep the lighter source, got {r2}");
    }

    /// Builds a `/Subtype /Form` XObject stream with the given `/BBox` and
    /// content bytes, optionally marked as a `/Group /S /Transparency`.
    fn form_xobject(bbox: [f64; 4], content: &[u8], transparency_group: bool) -> crate::object::Object {
        use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream};

        let mut dict = PdfDictionary::new();
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Form")));
        dict.set(
            "BBox",
            Object::Array(PdfArray::from_objects(bbox.iter().map(|v| Object::Real(*v)).collect())),
        );
        if transparency_group {
            let mut group = PdfDictionary::new();
            group.set("S", Object::Name(PdfName::new_unchecked("Transparency")));
            dict.set("Group", Object::Dictionary(group));
        }
        Object::Stream(PdfStream::with_dictionary(dict, content.to_vec()))
    }

    fn set_xobject(resources: &mut crate::object::PdfDictionary, name: &str, xobject: crate::object::Object) {
        use crate::object::{Object, PdfDictionary};
        let mut xo = match resources.get("XObject").cloned() {
            Some(Object::Dictionary(d)) => d,
            _ => PdfDictionary::new(),
        };
        xo.set(name, xobject);
        resources.set("XObject", Object::Dictionary(xo));
    }

    /// Test 17: A transparency-group Form XObject (ISO 32000-1 §11.4.5)
    /// containing a single opaque shape, painted via `Do` under an outer
    /// `ca` of 0.5, matches flat 50%-alpha compositing exactly (a sanity
    /// baseline before the "isolation matters" test below).
    #[test]
    fn transparency_group_single_shape_matches_flat_alpha() {
        use crate::object::PdfDictionary;

        let mut resources = PdfDictionary::new();
        add_ext_gstate(&mut resources, "GSouter", |gs| {
            gs.set("ca", crate::object::Object::Real(0.5));
        });
        set_xobject(
            &mut resources,
            "Grp",
            form_xobject([0.0, 0.0, 200.0, 200.0], b"1 0 0 rg 0 0 200 200 re f", true),
        );

        let content = b"/GSouter gs /Grp Do";
        let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
        assert!(out.warnings.is_empty());
        let (r, g, b, a) = pixel(&out, 100, 100);
        assert_eq!(r, 255);
        assert!((126..=129).contains(&g), "unexpected green: {g}");
        assert_eq!(g, b);
        assert_eq!(a, 255);
    }

    /// Test 18: Two overlapping semi-transparent shapes *inside* an
    /// isolated transparency group composite among themselves first
    /// (compounding to ~84% coverage), and only *then* does the group as
    /// a whole get the outer `ca` (0.5) applied -- net ~42% coverage. A
    /// (non-isolated/naive) implementation that instead applied the outer
    /// alpha to each shape individually before compositing would produce
    /// a visibly different (~51%) result. This is the crux of "shapes
    /// overlap inside a semi-transparent group" from the phase's
    /// Definition of Done.
    #[test]
    fn transparency_group_isolates_internal_alpha_from_outer_alpha() {
        use crate::object::PdfDictionary;

        let mut resources = PdfDictionary::new();
        add_ext_gstate(&mut resources, "GSouter", |gs| {
            gs.set("ca", crate::object::Object::Real(0.5));
        });
        add_ext_gstate(&mut resources, "GSinner", |gs| {
            gs.set("ca", crate::object::Object::Real(0.6));
        });
        let group_content = b"\
            /GSinner gs 1 0 0 rg 0 0 200 200 re f \
            /GSinner gs 1 0 0 rg 0 0 200 200 re f";
        set_xobject(&mut resources, "Grp", form_xobject([0.0, 0.0, 200.0, 200.0], group_content, true));

        let content = b"/GSouter gs /Grp Do";
        let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
        assert!(out.warnings.is_empty());
        let (r, g, _b, a) = pixel(&out, 100, 100);
        assert_eq!(r, 255);
        assert_eq!(a, 255);
        // Isolated-group-correct expectation: ~42% coverage -> green/blue
        // channel near 255*(1-0.42) = 147.9. The naive (non-isolated)
        // alternative would land near 255*(1-0.51) = 124.95 -- comfortably
        // outside this tolerance, so this test would fail loudly if group
        // isolation regressed to "apply outer alpha to each inner shape".
        assert!((140..=156).contains(&g), "expected ~148 for isolated-group compositing, got {g}");
    }

    /// Test 19: An ExtGState `/SMask` (ISO 32000-1 §11.6.5.2,
    /// `/S /Luminosity`) restricts subsequent painting to the mask
    /// group's bright (white) area; the mask group's implicit black
    /// backdrop hides painting everywhere else.
    #[test]
    fn ext_gstate_luminosity_soft_mask_restricts_painting() {
        use crate::object::{Object, PdfDictionary, PdfName};

        let mut resources = PdfDictionary::new();

        // ISO 32000-1 §11.6.5.2 requires the ExtGState `/SMask` dict's
        // `/G` to be the group Form XObject stream directly (not a
        // `/Resources /XObject` name lookup), so this doesn't need to be
        // registered under `/XObject` at all.
        let mask_stream = form_xobject([0.0, 0.0, 200.0, 200.0], b"1 1 1 rg 0 0 100 200 re f", false);
        let mut smask_dict = PdfDictionary::new();
        smask_dict.set("G", mask_stream);
        smask_dict.set("S", Object::Name(PdfName::new_unchecked("Luminosity")));
        let mut gs_mask = PdfDictionary::new();
        gs_mask.set("SMask", Object::Dictionary(smask_dict));
        let mut extgstate = match resources.get("ExtGState").cloned() {
            Some(Object::Dictionary(d)) => d,
            _ => PdfDictionary::new(),
        };
        extgstate.set("GSmask", Object::Dictionary(gs_mask));
        resources.set("ExtGState", Object::Dictionary(extgstate));

        let content = b"/GSmask gs 1 0 0 rg 0 0 200 200 re f";
        let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
        assert!(out.warnings.is_empty(), "warnings: {:?}", out.warnings);
        // Left half (x in [0,100)): mask is white -> fully visible red.
        assert_eq!(pixel(&out, 50, 100), (255, 0, 0, 255));
        // Right half: mask backdrop is black -> fully hidden, background
        // white shows through untouched.
        assert_eq!(pixel(&out, 150, 100), (255, 255, 255, 255));
    }

    /// Test 20: An image XObject's own `/SMask` (ISO 32000-1 §11.6.5.3)
    /// makes half the image transparent (showing the white page
    /// background) and half opaque, using a 2x1 base image plus a 2x1
    /// DeviceGray mask (`[0, 255]`).
    #[test]
    fn image_smask_makes_left_pixel_transparent() {
        use crate::object::{Object, PdfDictionary, PdfName, PdfStream};

        let mut mask_dict = PdfDictionary::new();
        mask_dict.set("Width", Object::Integer(2));
        mask_dict.set("Height", Object::Integer(1));
        mask_dict.set("BitsPerComponent", Object::Integer(8));
        mask_dict.set("ColorSpace", Object::Name(PdfName::new_unchecked("DeviceGray")));
        let mask_stream = PdfStream::with_dictionary(mask_dict, vec![0u8, 255u8]);

        let mut img_dict = PdfDictionary::new();
        img_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Image")));
        img_dict.set("Width", Object::Integer(2));
        img_dict.set("Height", Object::Integer(1));
        img_dict.set("BitsPerComponent", Object::Integer(8));
        img_dict.set("ColorSpace", Object::Name(PdfName::new_unchecked("DeviceRGB")));
        img_dict.set("SMask", Object::Stream(mask_stream));
        let img_stream = PdfStream::with_dictionary(img_dict, vec![255, 0, 0, 255, 0, 0]);

        let mut resources = PdfDictionary::new();
        set_xobject(&mut resources, "Im1", Object::Stream(img_stream));

        let content = b"q 200 0 0 200 0 0 cm /Im1 Do Q";
        let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
        assert!(out.warnings.is_empty(), "warnings: {:?}", out.warnings);
        // Left column (mask sample 0 -> transparent): background shows.
        // Bicubic filtering (chosen for real-world image quality -- see
        // `draw_image_pixels`'s doc comment) has a wider sampling kernel
        // than the `Nearest` this test's exact-equality assertion used to
        // assume, so a source image this extreme (2 texels stretched
        // 100x, i.e. a worst case no real-world image resembles) picks up
        // a little colour bleed from the adjacent opaque-red texel even
        // at this sample point's block centre -- a real, expected
        // consequence of higher-quality interpolation, not a transparency
        // regression. Assert "close to background", not exact.
        let (r, g, b, a) = pixel(&out, 50, 100);
        assert_eq!((r, a), (255, 255));
        assert!(g >= 235 && b >= 235, "expected near-white (bicubic bleed tolerated), got ({r}, {g}, {b}, {a})");
        // Right column (mask sample 255 -> opaque): red paints through.
        let (r, g, b, a) = pixel(&out, 150, 100);
        assert_eq!((r, a), (255, 255));
        assert!(g <= 20 && b <= 20, "expected near-pure red (bicubic bleed tolerated), got ({r}, {g}, {b}, {a})");
    }

    /// Test 21: Adversarial input: a Form XObject whose own content
    /// stream invokes itself via `Do` is bounded by
    /// [`interpreter::MAX_FORM_XOBJECT_DEPTH`], not infinite recursion /
    /// a stack overflow.
    #[test]
    fn self_referential_form_xobject_is_bounded_not_infinite() {
        use crate::object::PdfDictionary;

        let mut resources = PdfDictionary::new();
        set_xobject(&mut resources, "Rec", form_xobject([0.0, 0.0, 200.0, 200.0], b"/Rec Do", false));

        let content = b"/Rec Do 0 1 0 rg 0 0 10 10 re f";
        let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
        assert!(out
            .warnings
            .iter()
            .any(|w| matches!(w, RenderWarning::FormXObjectRecursionLimitExceeded)));
        // The rest of the (top-level) content stream after the
        // self-referential `Do` still rendered.
        assert_eq!(pixel(&out, 5, 195), (0, 255, 0, 255));
    }
}
