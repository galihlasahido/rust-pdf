//! A pure-Rust content-stream interpreter and 2D rasterizer -- no native
//! binary, no FFI, at all. Gated behind the `native-render` Cargo feature.
//!
//! This coexists with [`crate::render::PdfRenderer`] (the Pdfium/FFI
//! backend, feature `render`) during the pure-Rust migration described in
//! `ARCHITECTURE.md`; neither replaces the other yet, and both can be
//! enabled at once. The 2D rasterizer backend is
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
//!   standard/system-font substitution database at all (unlike Pdfium).
//!   Any font (of *any* `/Subtype`, including TrueType) with no
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
//! - **Images and Form XObjects** (`Do`, plus inline images `BI`/`ID`/`EI`)
//!   -- not painted; recorded as warnings.
//! - **Shadings** (`sh`) and **Patterns** -- not painted.
//! - **Non-Device color spaces** (`cs`/`CS`/`sc`/`SC`/`scn`/`SCN`):
//!   CalGray, CalRGB, Lab, ICCBased, Indexed, Separation, DeviceN are not
//!   implemented this phase. Selecting one leaves the current fill/stroke
//!   color unchanged and records a warning -- it does **not** attempt an
//!   approximate conversion, to avoid silently producing plausible-looking
//!   but wrong colors.
//! - **JBIG2 and JPX (JPEG2000)** image filters -- there is no mature
//!   pure-Rust decoder for either in the ecosystem today. This phase
//!   doesn't decode any images at all (see above), so this gap doesn't yet
//!   have a code path to speak of, but it is called out here because it
//!   will remain a **hard, structural gap** even once image painting is
//!   implemented in a later phase: such images must fail closed (a
//!   placeholder/structured error), not silently blank or panic.
//! - **ICC color management** -- the CMYK->RGB conversion this phase does
//!   have (`color::device_cmyk`) is the naive, non-color-managed formula
//!   ISO 32000-1 8.6.5.3 itself documents as the fallback conversion, not
//!   true ICC-profile-based color management. There is no mature pure-Rust
//!   ICC engine this crate has adopted. Accurate/perceptual color
//!   reproduction against a specific ICC profile remains unimplemented.
//! - **Transparency groups and blend modes** (ISO 32000-1 Chapter 11)
//!   beyond flat constant alpha (`ca`/`CA`) -- not implemented.
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
//!   fallback before they reach the rasterizer (see
//!   `path::sanitize_point`); `tiny-skia` itself additionally refuses
//!   (logs and no-ops, does not panic) to rasterize path geometry whose
//!   magnitude would overflow its internal math.
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

mod color;
mod error;
mod font;
mod glyph;
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

    /// Test 10: Unsupported operators (image XObjects, this phase's `Do`)
    /// are recorded as warnings and skipped, without aborting the rest of
    /// the (graphics) content stream.
    #[test]
    fn unsupported_operator_is_a_warning_not_a_failure() {
        let content = b"/Im1 Do 0 1 0 rg 0 0 200 200 re f";
        let out = render_content_stream(content, 200, 200, page(), None).unwrap();
        assert_eq!(pixel(&out, 100, 100), (0, 255, 0, 255));
        assert!(!out.warnings.is_empty());
        assert!(out
            .warnings
            .iter()
            .any(|w| matches!(w, RenderWarning::UnsupportedOperator { operator } if operator == "Do")));
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
}
