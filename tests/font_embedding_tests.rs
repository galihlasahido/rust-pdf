//! Integration tests for embedded TrueType/OpenType font loading, Type 0/CID
//! composite fonts (CJK), font subsetting, and text extraction — covering
//! the Fonts-phase Definition of Done:
//!
//! - A document with CJK text is built as a well-formed Type 0/CIDFontType2
//!   PDF (glyph *rendering* fidelity itself is verified via the `render`
//!   feature's pure-Rust content-stream interpreter; see `ARCHITECTURE.md`
//!   and `src/font/cid.rs` docs).
//! - Text extraction recovers correct Unicode for both an embedded
//!   (composite, CJK) font and a non-embedded (Standard 14) font.
//! - Subsetting produces a smaller file than a full embed of the same font.
//!
//! Most of these tests use a small, hand-built synthetic sfnt/TrueType font
//! (see [`build_test_font`] below) rather than a vendored real-world font,
//! specifically to test the embedding/subsetting/CID-encoding/ToUnicode
//! *structure* in isolation without a large binary fixture.
//!
//! That is not sufficient on its own, though: a synthetic font's glyphs are
//! empty (zero-contour) outlines, so it can never prove that a real viewer
//! actually paints recognizable CJK glyph *shapes* on the page — only that
//! the surrounding PDF structure is well-formed. The [`cjk_visual_render`]
//! module below closes that gap with a small, real, OFL-licensed CJK font
//! (a glyph-subsetted derivative of Noto Sans SC; see
//! `tests/fixtures/fonts/NOTICE.md` for provenance and licensing) rendered
//! through this crate's own pure-Rust content-stream interpreter/rasterizer
//! (`render::native`, an *embedded* CID/Type0 TrueType font -- see that
//! module's docs: this is squarely within its supported scope, unlike
//! non-embedded/Standard-14 fonts), asserting non-background ink pixels
//! appear exactly where the CJK glyphs were placed.

#![cfg(all(feature = "fonts", feature = "parser"))]

use rust_pdf::prelude::*;

/// Builds a minimal valid TrueType font whose `cmap` maps exactly the given
/// `(char, glyph_id)` pairs (plus the mandatory `.notdef` glyph 0). All
/// glyphs are empty outlines — sufficient to exercise the embedding,
/// subsetting, CID-encoding and ToUnicode pipeline end-to-end, though not
/// to visually render a real glyph shape (see module docs).
fn build_test_font(chars: &[(char, u16)]) -> Vec<u8> {
    let num_glyphs: u16 = chars.iter().map(|&(_, g)| g).max().unwrap_or(0) + 1;

    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"cmap", build_cmap(chars)),
        (b"glyf", Vec::new()),
        (b"head", build_head()),
        (b"hhea", build_hhea(num_glyphs)),
        (b"hmtx", build_hmtx(num_glyphs)),
        (b"loca", build_loca(num_glyphs)),
        (b"maxp", build_maxp(num_glyphs)),
        (b"post", build_post()),
    ];

    tables.sort_by_key(|(tag, _)| **tag);

    let mut out = Vec::new();
    let num_tables = tables.len() as u16;
    out.extend_from_slice(&0x00010000u32.to_be_bytes());
    out.extend_from_slice(&num_tables.to_be_bytes());
    let entry_selector = (num_tables as f32).log2().floor() as u16;
    let search_range = 2u16.saturating_pow(entry_selector as u32) * 16;
    let range_shift = num_tables.saturating_mul(16).saturating_sub(search_range);
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    let dir_len = 12 + tables.len() * 16;
    let mut offset = dir_len;
    let mut records = Vec::new();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        let start = offset;
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = dir_len + body.len();
        records.push((*tag, start, data.len()));
    }

    for (tag, start, len) in &records {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(*start as u32).to_be_bytes());
        out.extend_from_slice(&(*len as u32).to_be_bytes());
    }
    out.extend_from_slice(&body);
    out
}

fn build_head() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&0x00010000u32.to_be_bytes());
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(&0x5F0F3CF5u32.to_be_bytes());
    v.extend_from_slice(&0u16.to_be_bytes());
    v.extend_from_slice(&1000u16.to_be_bytes());
    v.extend_from_slice(&0u64.to_be_bytes());
    v.extend_from_slice(&0u64.to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&1000i16.to_be_bytes());
    v.extend_from_slice(&1000i16.to_be_bytes());
    v.extend_from_slice(&0u16.to_be_bytes());
    v.extend_from_slice(&8u16.to_be_bytes());
    v.extend_from_slice(&2i16.to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v
}

fn build_hhea(num_glyphs: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&0x00010000u32.to_be_bytes());
    v.extend_from_slice(&800i16.to_be_bytes());
    v.extend_from_slice(&(-200i16).to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&1000u16.to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&1000i16.to_be_bytes());
    v.extend_from_slice(&1i16.to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&[0u8; 8]);
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&num_glyphs.to_be_bytes());
    v
}

fn build_hmtx(num_glyphs: u16) -> Vec<u8> {
    let mut v = Vec::new();
    for _ in 0..num_glyphs {
        v.extend_from_slice(&600u16.to_be_bytes());
        v.extend_from_slice(&0i16.to_be_bytes());
    }
    v
}

fn build_loca(num_glyphs: u16) -> Vec<u8> {
    let mut v = Vec::new();
    for _ in 0..=num_glyphs {
        v.extend_from_slice(&0u16.to_be_bytes());
    }
    v
}

fn build_maxp(num_glyphs: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&0x00010000u32.to_be_bytes());
    v.extend_from_slice(&num_glyphs.to_be_bytes());
    v.extend_from_slice(&[0u8; 26]);
    v
}

fn build_post() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&0x00030000u32.to_be_bytes());
    v.extend_from_slice(&0i32.to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&0i16.to_be_bytes());
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(&[0u8; 16]);
    v
}

fn build_cmap(chars: &[(char, u16)]) -> Vec<u8> {
    let mut segs: Vec<(u16, u16)> = chars.iter().map(|&(c, g)| (c as u16, g)).collect();
    segs.sort_by_key(|&(code, _)| code);

    let seg_count = segs.len() as u16 + 1;
    let mut sub = Vec::new();
    sub.extend_from_slice(&4u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    let seg_count_x2 = seg_count * 2;
    sub.extend_from_slice(&seg_count_x2.to_be_bytes());
    let entry_selector = (seg_count as f32).log2().floor() as u16;
    let search_range = 2u16.saturating_pow(entry_selector as u32) * 2;
    sub.extend_from_slice(&search_range.to_be_bytes());
    sub.extend_from_slice(&entry_selector.to_be_bytes());
    sub.extend_from_slice(&(seg_count_x2.saturating_sub(search_range)).to_be_bytes());

    for &(code, _) in &segs {
        sub.extend_from_slice(&code.to_be_bytes());
    }
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    for &(code, _) in &segs {
        sub.extend_from_slice(&code.to_be_bytes());
    }
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
    for &(code, gid) in &segs {
        sub.extend_from_slice(&gid.wrapping_sub(code).to_be_bytes());
    }
    sub.extend_from_slice(&1u16.to_be_bytes());
    for _ in 0..seg_count {
        sub.extend_from_slice(&0u16.to_be_bytes());
    }

    let len = sub.len() as u16;
    sub[2..4].copy_from_slice(&len.to_be_bytes());

    let mut v = Vec::new();
    v.extend_from_slice(&0u16.to_be_bytes());
    v.extend_from_slice(&1u16.to_be_bytes());
    v.extend_from_slice(&3u16.to_be_bytes());
    v.extend_from_slice(&1u16.to_be_bytes());
    v.extend_from_slice(&12u32.to_be_bytes());
    v.extend_from_slice(&sub);
    v
}

/// A modest CJK-plus-Latin cmap: enough distinct glyphs to make a
/// meaningful subsetting size comparison, mirroring the shape of a real
/// (much larger) CJK font's cmap.
fn cjk_font_bytes(num_extra_cjk: u16) -> Vec<u8> {
    let mut chars = vec![('A', 1u16), ('中', 2), ('文', 3), ('日', 4), ('本', 5), ('語', 6)];
    for i in 0..num_extra_cjk {
        // Fill a run of CJK Unified Ideographs beyond the ones actually used
        // by the test text, purely to bulk up the font for the size-delta
        // assertion.
        chars.push((char::from_u32(0x4E00 + 100 + i as u32).unwrap(), 7 + i));
    }
    build_test_font(&chars)
}

#[test]
fn cjk_document_is_a_well_formed_type0_cidfonttype2_pdf() {
    let font = CompositeFont::new(cjk_font_bytes(0), "TestCJKFont").unwrap();
    let cid_bytes = font.encode("中文日本語");

    let page = PageBuilder::a4()
        .font("F1", font)
        .content(
            ContentBuilder::new().text_block(
                TextBuilder::new()
                    .font("F1", 24.0)
                    .position(72.0, 700.0)
                    .show_bytes(cid_bytes),
            ),
        )
        .build();

    let bytes = DocumentBuilder::new()
        .title("CJK test document")
        .page(page)
        .build()
        .unwrap()
        .save_to_bytes()
        .unwrap();

    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/Subtype /Type0"), "missing Type0 font: {text}");
    assert!(text.contains("/Subtype /CIDFontType2"), "missing CIDFontType2 descendant");
    assert!(text.contains("/Encoding /Identity-H"), "missing Identity-H encoding");
    assert!(text.contains("/FontFile2"), "font program was not embedded");
    assert!(text.contains("/ToUnicode"), "missing ToUnicode CMap");
    assert!(text.contains("/CIDToGIDMap"), "missing CIDToGIDMap (subset is the default)");

    // Independent parser must be able to open the file and see the page.
    let lopdf_doc = lopdf::Document::load_mem(&bytes).expect("lopdf must open the CJK PDF");
    assert_eq!(lopdf_doc.get_pages().len(), 1);
}

#[test]
fn cjk_text_extraction_recovers_correct_unicode_from_embedded_font() {
    let font = CompositeFont::new(cjk_font_bytes(0), "TestCJKFont").unwrap();
    let cid_bytes = font.encode("中文日本語");

    let page = PageBuilder::a4()
        .font("F1", font)
        .content(
            ContentBuilder::new().text_block(
                TextBuilder::new()
                    .font("F1", 24.0)
                    .position(72.0, 700.0)
                    .show_bytes(cid_bytes),
            ),
        )
        .build();

    let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();

    let doc = EditableDocument::from_bytes(bytes).unwrap();
    let page_id = doc.page_id_at(0).unwrap();
    let extracted = doc.extract_page_text(page_id).unwrap();
    assert_eq!(extracted.trim(), "中文日本語", "extracted text was: {extracted:?}");
}

#[test]
fn non_embedded_standard14_text_extraction_uses_winansi_fallback() {
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::TimesRoman)
        .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Hello, World!"))
        .build();
    let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();

    let doc = EditableDocument::from_bytes(bytes).unwrap();
    let page_id = doc.page_id_at(0).unwrap();
    let extracted = doc.extract_page_text(page_id).unwrap();
    assert!(extracted.contains("Hello, World!"), "extracted text was: {extracted:?}");
}

#[test]
fn subsetting_produces_a_smaller_file_than_full_embed() {
    // A font with 300 extra unused CJK glyphs so the subsetting win is
    // large and unambiguous, mirroring embedding one weight of a real CJK
    // font (which typically covers thousands of ideographs a given
    // document only ever uses a handful of).
    let font_bytes = cjk_font_bytes(300);
    let text_used = "中文日本語";

    let build_doc = |font: CompositeFont| -> Vec<u8> {
        let cid_bytes = font.encode(text_used);
        let page = PageBuilder::a4()
            .font("F1", font)
            .content(
                ContentBuilder::new().text_block(
                    TextBuilder::new()
                        .font("F1", 24.0)
                        .position(72.0, 700.0)
                        .show_bytes(cid_bytes),
                ),
            )
            .build();
        DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap()
    };

    let subset_font = CompositeFont::new(font_bytes.clone(), "TestCJKFont").unwrap(); // subset defaults to true
    let subset_pdf = build_doc(subset_font);

    let full_font = CompositeFont::new(font_bytes, "TestCJKFont").unwrap().subset(false);
    let full_pdf = build_doc(full_font);

    assert!(
        subset_pdf.len() < full_pdf.len(),
        "subset PDF ({} bytes) should be smaller than full-embed PDF ({} bytes)",
        subset_pdf.len(),
        full_pdf.len()
    );

    // And both must still be valid, openable PDFs with the extracted text
    // intact (subsetting must not corrupt what's actually used).
    for pdf in [&subset_pdf, &full_pdf] {
        let doc = EditableDocument::from_bytes(pdf.clone()).unwrap();
        let page_id = doc.page_id_at(0).unwrap();
        let extracted = doc.extract_page_text(page_id).unwrap();
        assert_eq!(extracted.trim(), text_used);
    }
}

#[test]
fn non_embedded_composite_font_omits_font_file_but_still_declares_type0() {
    // "Font fallback when not embedded": the descendant/descriptor are
    // still written (so a viewer can attempt substitution by BaseFont
    // name), but no FontFile2/FontFile3 stream is present.
    let font = CompositeFont::new(cjk_font_bytes(0), "SystemCJKFont")
        .unwrap()
        .embed(false);
    let cid_bytes = font.encode("中文");

    let page = PageBuilder::a4()
        .font("F1", font)
        .content(
            ContentBuilder::new().text_block(
                TextBuilder::new()
                    .font("F1", 24.0)
                    .position(72.0, 700.0)
                    .show_bytes(cid_bytes),
            ),
        )
        .build();

    let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/Subtype /Type0"));
    assert!(!text.contains("/FontFile2"));
    assert!(!text.contains("/FontFile3"));

    let lopdf_doc = lopdf::Document::load_mem(&bytes).expect("lopdf must still open a non-embedded composite font PDF");
    assert_eq!(lopdf_doc.get_pages().len(), 1);
}

// ---------------------------------------------------------------------
// Visual verification: a *real* CJK font, rendered through this crate's
// own pure-Rust content-stream interpreter/rasterizer.
//
// Every test above proves the PDF *structure* around CJK text is correct
// (Type0/CIDFontType2, ToUnicode, subsetting) using a synthetic font whose
// glyphs are empty outlines. That is necessary but not sufficient: it
// cannot catch a bug in glyph-ID mapping, CIDToGIDMap, or advance-width
// computation that would still produce a structurally-valid PDF but render
// the *wrong* (or no) glyph shape. This module renders real CJK glyph
// outlines (a small OFL-licensed subset of Noto Sans SC — see
// `tests/fixtures/fonts/NOTICE.md`) through the real `render::native`
// interpreter (an *embedded* CID/Type0 TrueType font -- squarely within
// this pure-Rust renderer's documented scope) and asserts non-background
// ink pixels land exactly where the glyphs were placed.
// ---------------------------------------------------------------------
#[cfg(feature = "render")]
mod cjk_visual_render {
    use super::*;
    use rust_pdf::render::PdfRenderer;

    /// A real, OFL-licensed CJK font (glyph-subsetted Noto Sans SC; see
    /// `tests/fixtures/fonts/NOTICE.md`), with genuine multi-contour glyph
    /// outlines for every character this module renders — unlike
    /// [`super::build_test_font`]'s empty/zero-contour synthetic glyphs.
    fn real_cjk_font_bytes() -> Vec<u8> {
        include_bytes!("fixtures/fonts/NotoSansSC-Subset.ttf").to_vec()
    }

    /// The CJK text rendered by this test, all width-1000/1000-em (full
    /// square) glyphs in the fixture font, each occupying a whole
    /// `FONT_SIZE_PT`-wide advance so the on-page bounding box below is a
    /// simple, exact multiple of the font size.
    const CJK_TEXT: &str = "中文测试";
    const FONT_SIZE_PT: f64 = 60.0;
    const TEXT_ORIGIN_X_PT: f64 = 72.0;
    const TEXT_ORIGIN_Y_PT: f64 = 650.0;
    const RENDER_DPI: f32 = 150.0;

    /// Builds a one-page A4 document containing (if `with_text`) the CJK
    /// text drawn via a real, embedded [`CompositeFont`], at exactly
    /// [`TEXT_ORIGIN_X_PT`]/[`TEXT_ORIGIN_Y_PT`]/[`FONT_SIZE_PT`] — or an
    /// otherwise-identical but textless page when `with_text` is `false`,
    /// used as a same-layout "no ink here" control.
    fn build_page(with_text: bool) -> Vec<u8> {
        let mut page_builder = PageBuilder::a4();
        if with_text {
            let font = CompositeFont::new(real_cjk_font_bytes(), "NotoSansSC-Test").unwrap();
            let cid_bytes = font.encode(CJK_TEXT);
            page_builder = page_builder.font("F1", font).content(
                ContentBuilder::new().text_block(
                    TextBuilder::new()
                        .font("F1", FONT_SIZE_PT)
                        .position(TEXT_ORIGIN_X_PT, TEXT_ORIGIN_Y_PT)
                        .show_bytes(cid_bytes),
                ),
            );
        }
        let page = page_builder.build();
        DocumentBuilder::new()
            .title("CJK visual render test")
            .page(page)
            .build()
            .unwrap()
            .save_to_bytes()
            .unwrap()
    }

    /// Converts a PDF-space rectangle (origin bottom-left, y-up, in points)
    /// on an A4 page to a pixel-space rectangle (origin top-left, y-down)
    /// at `dpi`, returning `(x0, y0, x1, y1)` pixel bounds clamped to
    /// `(width, height)`.
    fn pdf_rect_to_pixel_rect(
        (x0_pt, y0_pt, x1_pt, y1_pt): (f64, f64, f64, f64),
        dpi: f32,
        (width, height): (u32, u32),
    ) -> (u32, u32, u32, u32) {
        let scale = (dpi / 72.0) as f64;
        let page_height_pt = 842.0; // A4 height in points (see Rectangle::a4()).
        let px0 = (x0_pt * scale).max(0.0) as u32;
        let px1 = ((x1_pt * scale).max(0.0) as u32).min(width);
        // Larger PDF y (higher up the page) maps to a smaller pixel row.
        let py0 = ((page_height_pt - y1_pt) * scale).max(0.0) as u32;
        let py1 = (((page_height_pt - y0_pt) * scale).max(0.0) as u32).min(height);
        (px0, py0, px1, py1)
    }

    /// Counts pixels within `(x0, y0, x1, y1)` (pixel space, `x1`/`y1`
    /// exclusive) that are meaningfully darker than a white page
    /// background -- i.e. glyph ink (including anti-aliased gray edges),
    /// not the paper.
    fn count_ink_pixels(image: &rust_pdf::render::RgbaImage, (x0, y0, x1, y1): (u32, u32, u32, u32)) -> usize {
        let mut count = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = image.get_pixel(x, y);
                let intensity = (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3;
                // White background ~= 255; require a comfortable margin
                // below that so JPEG-free PNG-exact white plus mild
                // anti-aliasing noise never false-positives as "ink".
                if intensity < 200 {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn cjk_real_glyphs_render_visible_ink() {
        // Bounding box around the expected glyph area, generously padded
        // (the fixture font's glyphs are ~1000/1000-em full-width CJK
        // ideographs with a small negative descender, per
        // `tests/fixtures/fonts/NOTICE.md`'s derivation notes), but tight
        // enough that it does not extend into any other page content.
        let bbox_pt = (
            TEXT_ORIGIN_X_PT - 8.0,
            TEXT_ORIGIN_Y_PT - 15.0,
            TEXT_ORIGIN_X_PT + FONT_SIZE_PT * CJK_TEXT.chars().count() as f64 + 8.0,
            TEXT_ORIGIN_Y_PT + FONT_SIZE_PT + 15.0,
        );

        let with_text = build_page(true);
        let renderer = PdfRenderer::open_bytes(with_text).expect("failed to open CJK render-test document");
        let image = renderer.render_page(0, RENDER_DPI, None).expect("failed to render CJK page");
        let (width, height) = image.dimensions();
        let pixel_bbox = pdf_rect_to_pixel_rect(bbox_pt, RENDER_DPI, (width, height));

        let ink_pixels = count_ink_pixels(&image, pixel_bbox);
        assert!(
            ink_pixels > 500,
            "expected substantial glyph ink in the CJK text's bounding box {pixel_bbox:?} \
             (image {width}x{height}), only found {ink_pixels} dark pixels -- real CJK glyph \
             shapes are not being rendered"
        );

        // Control: the *same* bounding box on an otherwise-identical page
        // with no text at all must be (near-)blank paper, proving the ink
        // above is specifically due to the CJK glyphs, not some unrelated
        // page decoration or rendering artifact.
        let without_text = build_page(false);
        let blank_renderer = PdfRenderer::open_bytes(without_text).expect("failed to open blank control document");
        let blank_image = blank_renderer.render_page(0, RENDER_DPI, None).expect("failed to render blank control page");
        let blank_ink_pixels = count_ink_pixels(&blank_image, pixel_bbox);
        assert!(
            blank_ink_pixels < 5,
            "expected the textless control page's identical bounding box to be blank paper, \
             found {blank_ink_pixels} dark pixels -- test bounding box must be wrong"
        );

        // Sanity: save a PNG for manual inspection (gitignored), same
        // convention as render_tests.rs.
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("output").join("render");
        std::fs::create_dir_all(&dir).ok();
        let _ = image.save(dir.join("cjk-visual-render.png"));

        println!(
            "CJK visual render: {ink_pixels} ink pixels in-bbox (text page) vs \
             {blank_ink_pixels} (blank control) at {RENDER_DPI} DPI"
        );
    }
}
