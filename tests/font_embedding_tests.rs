//! Integration tests for embedded TrueType/OpenType font loading, Type 0/CID
//! composite fonts (CJK), font subsetting, and text extraction — covering
//! the Fonts-phase Definition of Done:
//!
//! - A document with CJK text is built as a well-formed Type 0/CIDFontType2
//!   PDF (glyph *rendering* fidelity itself is delegated to Pdfium via the
//!   `render` feature; see `ARCHITECTURE.md` and `src/font/cid.rs` docs).
//! - Text extraction recovers correct Unicode for both an embedded
//!   (composite, CJK) font and a non-embedded (Standard 14) font.
//! - Subsetting produces a smaller file than a full embed of the same font.
//!
//! These tests use a small, hand-built synthetic sfnt/TrueType font (see
//! [`build_test_font`] below) rather than a vendored real-world font, to
//! avoid adding third-party font binaries (and their licensing overhead) to
//! the repository purely for test fixtures — see the task report for the
//! recommendation to add a real OFL-licensed CJK font under
//! `tests/fixtures/` for stronger regression coverage.

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
