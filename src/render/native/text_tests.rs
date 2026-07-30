//! Definition-of-done tests for the "Text Rendering" phase: actual ink
//! pixels landing inside the expected glyph bounding box for TrueType
//! simple fonts, CID/Type0 (CJK) composite fonts, and Type 3 glyph
//! procedures -- plus a test proving the Type1/bare-CFF gap fails
//! *gracefully* (no panic, no fabricated glyph) rather than being
//! silently mis-claimed as supported.

use super::*;
use crate::font::cid::CompositeFont;
use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream};
use crate::types::Rectangle;

/// 200x200 device raster over a 0..200 MediaBox (1:1 scale, so device
/// pixels and user-space points coincide except for the y-flip).
fn page() -> Rectangle {
    Rectangle::new(0.0, 0.0, 200.0, 200.0)
}

fn pixel(out: &NativeRenderOutput, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let p = out
        .pixmap
        .pixel(x, y)
        .unwrap_or_else(|| panic!("pixel ({x},{y}) out of bounds"))
        .demultiply();
    (p.red(), p.green(), p.blue(), p.alpha())
}

/// Whether any pixel in the device-space rectangle `[x0,x1) x [y0,y1)`
/// differs from the opaque-white background -- i.e. *some* ink landed
/// there. Scanning a region (rather than asserting one exact pixel) is
/// deliberate: it proves a glyph was actually painted somewhere inside
/// its expected bounding box without over-specifying exactly which
/// anti-aliased pixel should be darkest.
fn has_ink_in_rect(out: &NativeRenderOutput, x0: u32, y0: u32, x1: u32, y1: u32) -> bool {
    for y in y0..y1 {
        for x in x0..x1 {
            if pixel(out, x, y) != (255, 255, 255, 255) {
                return true;
            }
        }
    }
    false
}

/// Builds a minimal, *real* (non-empty-contour) TrueType font: one glyph
/// (gid 1, mapped from `'A'` via `cmap`) whose outline is a simple axis-
/// aligned square occupying the middle 80% of its 1000-unit em box
/// (`(100,100)`-`(900,900)`), plus the mandatory empty `.notdef` (gid 0).
///
/// This is deliberately more than
/// [`crate::font::truetype::test_support::build_test_font`] provides --
/// that helper's glyphs are all *empty* (zero-contour) outlines (fine for
/// testing PDF structure/subsetting, useless for proving actual glyph
/// *ink* lands on the page, which is exactly what this phase's
/// Definition of Done requires).
fn build_font_with_square_glyph() -> Vec<u8> {
    const UPM: u16 = 1000;

    // Simple glyph, 1 contour, 4 on-curve points, `(100,100)`-`(900,900)`,
    // each point encoded as a full (non-"short vector") signed 16-bit
    // delta from the previous point (starting at (0,0)) -- see the
    // OpenType `glyf` table spec's "Simple Glyph Description".
    let mut glyf_gid1 = Vec::new();
    glyf_gid1.extend_from_slice(&1i16.to_be_bytes()); // numberOfContours
    glyf_gid1.extend_from_slice(&100i16.to_be_bytes()); // xMin
    glyf_gid1.extend_from_slice(&100i16.to_be_bytes()); // yMin
    glyf_gid1.extend_from_slice(&900i16.to_be_bytes()); // xMax
    glyf_gid1.extend_from_slice(&900i16.to_be_bytes()); // yMax
    glyf_gid1.extend_from_slice(&3u16.to_be_bytes()); // endPtsOfContours[0] = 3 (4 points)
    glyf_gid1.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
    // Flags: ON_CURVE_POINT (bit 0) only, for all 4 points.
    glyf_gid1.extend_from_slice(&[0x01, 0x01, 0x01, 0x01]);
    // x deltas: (100,100) (900,100) (900,900) (100,900), from (0,0).
    for dx in [100i16, 800, 0, -800] {
        glyf_gid1.extend_from_slice(&dx.to_be_bytes());
    }
    for dy in [100i16, 0, 800, 0] {
        glyf_gid1.extend_from_slice(&dy.to_be_bytes());
    }
    // Pad to an even length (OpenType table/glyph alignment convention).
    if glyf_gid1.len() % 2 != 0 {
        glyf_gid1.push(0);
    }

    let glyf_gid1_len = glyf_gid1.len() as u16;
    let mut glyf = Vec::new(); // gid 0 (.notdef): empty, 0 bytes.
    glyf.extend_from_slice(&glyf_gid1);

    let mut loca = Vec::new(); // short format: offset/2 per entry, numGlyphs+1 entries.
    loca.extend_from_slice(&0u16.to_be_bytes()); // gid0 start
    loca.extend_from_slice(&0u16.to_be_bytes()); // gid0 end / gid1 start
    loca.extend_from_slice(&(glyf_gid1_len / 2).to_be_bytes()); // gid1 end

    let num_glyphs: u16 = 2;

    let mut head = Vec::new();
    head.extend_from_slice(&0x00010000u32.to_be_bytes());
    head.extend_from_slice(&0u32.to_be_bytes());
    head.extend_from_slice(&0u32.to_be_bytes());
    head.extend_from_slice(&0x5F0F3CF5u32.to_be_bytes());
    head.extend_from_slice(&0u16.to_be_bytes());
    head.extend_from_slice(&UPM.to_be_bytes());
    head.extend_from_slice(&0u64.to_be_bytes());
    head.extend_from_slice(&0u64.to_be_bytes());
    head.extend_from_slice(&0i16.to_be_bytes());
    head.extend_from_slice(&0i16.to_be_bytes());
    head.extend_from_slice(&1000i16.to_be_bytes());
    head.extend_from_slice(&1000i16.to_be_bytes());
    head.extend_from_slice(&0u16.to_be_bytes());
    head.extend_from_slice(&8u16.to_be_bytes());
    head.extend_from_slice(&2i16.to_be_bytes());
    head.extend_from_slice(&0i16.to_be_bytes()); // indexToLocFormat: short
    head.extend_from_slice(&0i16.to_be_bytes());

    let mut hhea = Vec::new();
    hhea.extend_from_slice(&0x00010000u32.to_be_bytes());
    hhea.extend_from_slice(&800i16.to_be_bytes());
    hhea.extend_from_slice(&(-200i16).to_be_bytes());
    hhea.extend_from_slice(&0i16.to_be_bytes());
    hhea.extend_from_slice(&1000u16.to_be_bytes());
    hhea.extend_from_slice(&0i16.to_be_bytes());
    hhea.extend_from_slice(&0i16.to_be_bytes());
    hhea.extend_from_slice(&1000i16.to_be_bytes());
    hhea.extend_from_slice(&1i16.to_be_bytes());
    hhea.extend_from_slice(&0i16.to_be_bytes());
    hhea.extend_from_slice(&0i16.to_be_bytes());
    hhea.extend_from_slice(&[0u8; 8]);
    hhea.extend_from_slice(&0i16.to_be_bytes());
    hhea.extend_from_slice(&num_glyphs.to_be_bytes());

    let mut hmtx = Vec::new();
    for _ in 0..num_glyphs {
        hmtx.extend_from_slice(&700u16.to_be_bytes());
        hmtx.extend_from_slice(&0i16.to_be_bytes());
    }

    let mut maxp = Vec::new();
    maxp.extend_from_slice(&0x00010000u32.to_be_bytes());
    maxp.extend_from_slice(&num_glyphs.to_be_bytes());
    maxp.extend_from_slice(&[0u8; 26]);

    // cmap: format 4, single segment mapping 'A' (0x41) -> gid 1.
    let mut cmap_sub = Vec::new();
    cmap_sub.extend_from_slice(&4u16.to_be_bytes()); // format
    cmap_sub.extend_from_slice(&0u16.to_be_bytes()); // length (patched below)
    cmap_sub.extend_from_slice(&0u16.to_be_bytes()); // language
    let seg_count_x2 = 4u16; // 1 real segment + 1 terminator segment, x2
    cmap_sub.extend_from_slice(&seg_count_x2.to_be_bytes());
    cmap_sub.extend_from_slice(&4u16.to_be_bytes()); // searchRange
    cmap_sub.extend_from_slice(&1u16.to_be_bytes()); // entrySelector
    cmap_sub.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
    cmap_sub.extend_from_slice(&0x0041u16.to_be_bytes()); // endCode[0] = 'A'
    cmap_sub.extend_from_slice(&0xFFFFu16.to_be_bytes()); // endCode[1] terminator
    cmap_sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
    cmap_sub.extend_from_slice(&0x0041u16.to_be_bytes()); // startCode[0] = 'A'
    cmap_sub.extend_from_slice(&0xFFFFu16.to_be_bytes()); // startCode[1] terminator
    // idDelta[0]: format 4's `gid = (code + idDelta) mod 65536` when
    // idRangeOffset is 0; want code 0x41 ('A') -> gid 1, so
    // idDelta = 1 - 0x41 = -64.
    cmap_sub.extend_from_slice(&(-64i16).to_be_bytes());
    cmap_sub.extend_from_slice(&1u16.to_be_bytes()); // idDelta[1] terminator (unused)
    cmap_sub.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset[0]
    cmap_sub.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset[1]
    let len = cmap_sub.len() as u16;
    cmap_sub[2..4].copy_from_slice(&len.to_be_bytes());

    let mut cmap = Vec::new();
    cmap.extend_from_slice(&0u16.to_be_bytes());
    cmap.extend_from_slice(&1u16.to_be_bytes());
    cmap.extend_from_slice(&3u16.to_be_bytes());
    cmap.extend_from_slice(&1u16.to_be_bytes());
    cmap.extend_from_slice(&12u32.to_be_bytes());
    cmap.extend_from_slice(&cmap_sub);

    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"cmap", cmap),
        (b"glyf", glyf),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"maxp", maxp),
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

/// Test: TrueType simple-font glyph rendering actually paints ink at the
/// expected device-space location.
#[test]
fn truetype_simple_font_glyph_paints_ink_in_expected_bbox() {
    let font_bytes = build_font_with_square_glyph();

    let mut descriptor = PdfDictionary::new();
    descriptor.set("FontFile2", Object::Stream(PdfStream::new(font_bytes)));
    let mut font_dict = PdfDictionary::new();
    font_dict.set("Subtype", Object::Name(PdfName::new_unchecked("TrueType")));
    font_dict.set("FirstChar", Object::Integer(65));
    let mut widths = PdfArray::new();
    widths.push(Object::Integer(700));
    font_dict.set("Widths", Object::Array(widths));
    font_dict.set("FontDescriptor", Object::Dictionary(descriptor));

    let mut fonts = PdfDictionary::new();
    fonts.set("F1", Object::Dictionary(font_dict));
    let mut resources = PdfDictionary::new();
    resources.set("Font", Object::Dictionary(fonts));

    // Tfs=100, Td(10,10): glyph square (100,100)-(900,900) in a 1000-unit
    // em maps to text-space (10,10)-(90,90), then + text origin (10,10)
    // -> user-space (20,20)-(100,100). MediaBox 0..200 == canvas 200x200
    // (1:1 scale), y-flipped: device y = 200 - user y, so the expected
    // device-space bounding box is x in [20,100), y in [100,180).
    let content = b"BT /F1 100 Tf 10 10 Td (A) Tj ET";
    let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();

    assert!(out.warnings.is_empty(), "unexpected warnings: {:?}", out.warnings);
    assert!(
        has_ink_in_rect(&out, 20, 100, 100, 180),
        "expected glyph ink inside its bounding box, found none"
    );
    // Well outside the glyph: still background white.
    assert_eq!(pixel(&out, 5, 5), (255, 255, 255, 255));
    assert_eq!(pixel(&out, 190, 190), (255, 255, 255, 255));
}

/// Test: composite (Type 0/CIDFontType2) CJK glyph rendering, reusing
/// [`crate::font::cid::CompositeFont`]'s `Identity-H` code/CID/GID
/// conventions (see `font.rs`'s module docs on what "reuse" means here)
/// against the real, OFL-licensed CJK fixture font already used by
/// `tests/font_embedding_tests.rs`'s own visual-rendering verification.
#[test]
fn composite_cid_font_cjk_glyph_paints_ink_in_expected_bbox() {
    let font_bytes = include_bytes!("../../../tests/fixtures/fonts/NotoSansSC-Subset.ttf").to_vec();

    // `CompositeFont::encode` is this crate's own writer-side CID/GID
    // selection (Identity-H: the 2-byte code == CID == original glyph
    // ID) -- reused here, not re-derived, to build the exact bytes a
    // real generated PDF would contain for this text.
    let font = CompositeFont::new(font_bytes.clone(), "NotoSansSC-Test").unwrap();
    let cid_bytes = font.encode("中");
    assert_eq!(cid_bytes.len(), 2, "Identity-H is exactly 2 bytes per code");
    let hex: String = cid_bytes.iter().map(|b| format!("{b:02X}")).collect();

    let mut descriptor = PdfDictionary::new();
    descriptor.set("FontFile2", Object::Stream(PdfStream::new(font_bytes)));
    let mut descendant = PdfDictionary::new();
    descendant.set("Subtype", Object::Name(PdfName::new_unchecked("CIDFontType2")));
    descendant.set("FontDescriptor", Object::Dictionary(descriptor));
    descendant.set("DW", Object::Integer(1000));
    // CIDToGIDMap omitted -> this phase defaults to Identity, matching
    // what `CompositeFont::build` emits for a full (unsubset) embed.

    let mut font_dict = PdfDictionary::new();
    font_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Type0")));
    font_dict.set("Encoding", Object::Name(PdfName::new_unchecked("Identity-H")));
    let mut descendants = PdfArray::new();
    descendants.push(Object::Dictionary(descendant));
    font_dict.set("DescendantFonts", Object::Array(descendants));

    let mut fonts = PdfDictionary::new();
    fonts.set("F1", Object::Dictionary(font_dict));
    let mut resources = PdfDictionary::new();
    resources.set("Font", Object::Dictionary(fonts));

    // Same positioning as the TrueType test: Tfs=100, Td(10,10). The CJK
    // glyph's exact outline shape isn't asserted, only that *some* ink
    // lands within its full-em advance box, per this phase's Definition
    // of Done ("assert ink pixel exists inside the expected glyph
    // bounding box").
    let content = format!("BT /F1 100 Tf 10 10 Td <{hex}> Tj ET").into_bytes();
    let out = render_content_stream(&content, 200, 200, page(), Some(&resources)).unwrap();

    assert!(out.warnings.is_empty(), "unexpected warnings: {:?}", out.warnings);
    // The fixture's '中' glyph outline bbox is (96,-79)-(902,840) in its
    // 1000-unit em (dumped once via `ttf_parser::Face::outline_glyph` to
    // derive this, not guessed) -- at Tfs=100 that is text-space
    // (9.6,-7.9)-(90.2,84.0), plus the (10,10) text origin -> user-space
    // (19.6,2.1)-(100.2,94.0), y-flipped to device space (MediaBox
    // 0..200 == canvas 1:1) as x in [19.6,100.2), y in [106.0,197.9).
    // Scan a generous sub-rectangle safely inside that.
    assert!(
        has_ink_in_rect(&out, 30, 120, 90, 180),
        "expected CJK glyph ink inside its bounding box, found none"
    );
    assert_eq!(pixel(&out, 5, 5), (255, 255, 255, 255));
    assert_eq!(pixel(&out, 190, 5), (255, 255, 255, 255));
}

/// Builds a Type 3 font dictionary (ISO 32000-1:2008 9.6.5) whose single
/// glyph (code `65`/`'A'`, named `/square`) is a `CharProc` that just
/// fills a rectangle -- proving Type 3 glyph rendering actually re-runs
/// the content-stream interpreter against the glyph procedure (ISO
/// 32000-1 9.6.5.2) rather than only handling TrueType/CID glyphs.
fn type3_font_dict(char_proc: &[u8]) -> PdfDictionary {
    let mut font_dict = PdfDictionary::new();
    font_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Type3")));
    let mut matrix = PdfArray::new();
    for v in [0.001, 0.0, 0.0, 0.001, 0.0, 0.0] {
        matrix.push(Object::Real(v));
    }
    font_dict.set("FontMatrix", Object::Array(matrix));

    let mut encoding = PdfDictionary::new();
    let mut diffs = PdfArray::new();
    diffs.push(Object::Integer(65));
    diffs.push(Object::Name(PdfName::new_unchecked("square")));
    encoding.set("Differences", Object::Array(diffs));
    font_dict.set("Encoding", Object::Dictionary(encoding));

    let mut char_procs = PdfDictionary::new();
    char_procs.set("square", Object::Stream(PdfStream::new(char_proc.to_vec())));
    font_dict.set("CharProcs", Object::Dictionary(char_procs));

    font_dict.set("FirstChar", Object::Integer(65));
    let mut widths = PdfArray::new();
    widths.push(Object::Integer(1000)); // glyph-space units == 1 em
    font_dict.set("Widths", Object::Array(widths));
    font_dict
}

/// Test: a Type 3 glyph's `CharProc` is run recursively through the same
/// interpreter and its painting lands at the expected device-space
/// location.
#[test]
fn type3_glyph_charproc_paints_ink_in_expected_bbox() {
    // Glyph-space square (100,100)-(900,900), same proportions as the
    // TrueType test above, filled black (the inherited default fill
    // color -- the CharProc sets no color of its own).
    let font_dict = type3_font_dict(b"100 100 800 800 re f");

    let mut fonts = PdfDictionary::new();
    fonts.set("F1", Object::Dictionary(font_dict));
    let mut resources = PdfDictionary::new();
    resources.set("Font", Object::Dictionary(fonts));

    // Identical positioning/math to the TrueType test: FontMatrix
    // 0.001 plays the exact role `1/unitsPerEm` (=1/1000) did there.
    let content = b"BT /F1 100 Tf 10 10 Td (A) Tj ET";
    let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();

    assert!(out.warnings.is_empty(), "unexpected warnings: {:?}", out.warnings);
    assert!(
        has_ink_in_rect(&out, 20, 100, 100, 180),
        "expected Type3 glyph ink inside its bounding box, found none"
    );
    assert_eq!(pixel(&out, 5, 5), (255, 255, 255, 255));
    assert_eq!(pixel(&out, 190, 190), (255, 255, 255, 255));
}

/// Test: a self-referential Type 3 font (its own `CharProc` shows text
/// using the very same font) does not hang or blow the call stack --
/// [`super::font::MAX_TYPE3_DEPTH`] bounds the recursion, and the render
/// still completes with a structured warning rather than aborting.
#[test]
fn type3_self_referential_charproc_is_bounded_not_infinite() {
    // The CharProc for 'A' itself shows "A" again with the same font --
    // a direct self-reference. Without a recursion bound this would
    // recurse until the process's call stack overflows (uncatchable).
    let font_dict = type3_font_dict(b"BT /F1 100 Tf 0 0 Td (A) Tj ET");

    let mut fonts = PdfDictionary::new();
    fonts.set("F1", Object::Dictionary(font_dict));
    let mut resources = PdfDictionary::new();
    resources.set("Font", Object::Dictionary(fonts));

    let content = b"BT /F1 100 Tf 10 10 Td (A) Tj ET";
    let out = render_content_stream(content, 200, 200, page(), Some(&resources))
        .expect("self-referential Type3 recursion must be bounded, not hang/abort");

    assert!(out
        .warnings
        .iter()
        .any(|w| matches!(w, RenderWarning::Type3RecursionLimitExceeded)));
}

/// Test: **the documented Type1/bare-CFF gap fails gracefully.** A font
/// resource whose only embedded program is a bare (non-`sfnt`-wrapped)
/// `Type1C` CFF stream -- something this crate's only font-parsing
/// dependency, `ttf-parser`, cannot parse at all -- must not panic, must
/// not silently fabricate a placeholder glyph, and must record a
/// structured warning naming the gap. Surrounding content must still
/// render normally.
#[test]
fn type1_bare_cff_font_fails_gracefully_not_panicking() {
    let mut descriptor = PdfDictionary::new();
    let mut font_file = PdfStream::new(b"\x01\x00garbage-not-an-sfnt-container-cff-program".to_vec());
    font_file.dictionary.set("Subtype", Object::Name(PdfName::new_unchecked("Type1C")));
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
    resources.set("Font", Object::Dictionary(fonts));

    // Same position/size as the TrueType test, plus an unrelated green
    // fill afterward that must still render (a broken font must not
    // abort the rest of the page).
    let content = b"BT /F1 100 Tf 10 10 Td (A) Tj ET 0 1 0 rg 150 150 40 40 re f";
    let out = render_content_stream(content, 200, 200, page(), Some(&resources))
        .expect("an unrenderable font program must fail closed, not error the whole render");

    // The documented gap: no glyph pixels at all where the (unrenderable)
    // glyph would have been -- not a fabricated placeholder box.
    assert!(
        !has_ink_in_rect(&out, 20, 100, 100, 180),
        "Type1/bare-CFF gap must render nothing, not a fake placeholder glyph"
    );
    // Explicitly recorded, not a silent skip.
    assert!(out.warnings.iter().any(|w| matches!(
        w,
        RenderWarning::UnsupportedFontProgram { resource_name, reason }
            if resource_name == "F1" && reason.contains("Type1")
    )));
    // The rest of the page (unrelated to the broken font) still rendered.
    assert_eq!(pixel(&out, 170, 30), (0, 255, 0, 255));
}
