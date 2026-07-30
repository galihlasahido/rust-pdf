//! Fuzz target for the embedded TrueType/OpenType font loader
//! (`TrueTypeFont::load`, ISO 32000-1 9.9 "Embedded Font Programs").
//!
//! `FontFile2`/`FontFile3` payloads are attacker-controlled bytes taken
//! straight from a PDF file (or, for standalone font-embedding APIs, from
//! whatever untrusted source an application layer sourced a font upload
//! from). The underlying `sfnt` parsing is delegated to `ttf-parser` (see
//! the module docs on `crate::font::truetype` for why this crate does not
//! hand-roll its own font parser), but this crate's own wrapper is
//! responsible for the size/glyph-count safety limits
//! (`MAX_FONT_SIZE_BYTES`/`MAX_GLYPH_COUNT`) and for not panicking on a
//! successfully-loaded-but-degenerate face when metrics are queried
//! afterwards. The only acceptable outcomes here are `Ok`/`Err` and bounded
//! memory/time use - never a panic, hang, or unbounded allocation.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rust_pdf::font::truetype::TrueTypeFont;

fuzz_target!(|data: &[u8]| {
    // Exercise a couple of face indices: 0 (the common case) and a second
    // one to hit the `.ttc`/`.otc` collection path and its own bounds
    // checking, without spending the fuzzer's time budget on a wide sweep.
    for face_index in [0u32, 1u32] {
        if let Ok(font) = TrueTypeFont::load(data.to_vec(), face_index) {
            // Touch the metric-query surface a real caller (PDF font
            // embedding) would use, so bugs only reachable after a
            // successful load are found too.
            let _ = font.flavor();
            let _ = font.units_per_em();
            let _ = font.num_glyphs();
            let _ = font.postscript_name();

            // A handful of representative code points, including ones
            // unlikely to be present, to exercise both the found and
            // not-found paths through the font's cmap.
            for c in ['A', 'a', '0', ' ', '\u{FFFF}', '\u{10FFFF}'] {
                if let Some(gid) = font.glyph_id(c) {
                    let _ = font.glyph_advance(gid);
                }
            }
            // Also probe a few raw glyph ids directly, including ones
            // likely to be out of range for the loaded face.
            for gid in [0u16, 1u16, u16::MAX] {
                let _ = font.glyph_advance(gid);
            }
        }
    }
});
