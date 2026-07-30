//! Fallback character-code-to-Unicode table for text extraction when a
//! simple (single-byte) font has **no** `/ToUnicode` CMap (ISO 32000-1:2008
//! 9.10.2 "Mapping Character Codes to Unicode Values", 9.10.3).
//!
//! ISO 32000-1 Annex D.2 defines `WinAnsiEncoding` as "the standard Windows
//! encoding for Latin text", explicitly noting it is based on — but not
//! byte-for-byte identical to — the Windows-1252 code page (a handful of
//! codes Annex D leaves undefined are assigned printable characters in the
//! real cp1252). This table implements the well-known Windows-1252 mapping
//! rather than transcribing Annex D's glyph-name table by hand: for
//! *best-effort text-extraction fallback* (used only when a document
//! supplies no `/ToUnicode` CMap and no better information), that handful
//! of rarely-used codes is immaterial, and cp1252 is unambiguous and
//! independently verifiable, whereas hand-transcribing Annex D from memory
//! risks subtly wrong entries. Where we are not confident of the exact
//! byte, we prefer this well-documented external reference over guessing.
//!
//! This is a genuinely different kind of "font fallback" than glyph
//! *rendering* substitution (picking a replacement typeface when a
//! referenced font isn't embedded and isn't installed) — this crate's own
//! pure-Rust rasterizer (see [`crate::render::native`], `render`/
//! `native-render` features) has no standard/system-font substitution
//! database at all, and explicitly does not paint any glyph for a
//! non-embedded font (a documented gap, not silent data loss — see that
//! module's docs). What this module provides is the fallback needed to
//! still recover readable Unicode *text* (for extraction/search, not
//! rendering) from a non-embedded (or embedded-but-`ToUnicode`-less)
//! simple font.

/// Decodes a single WinAnsiEncoding-ish byte to its best-effort Unicode
/// scalar value, per the module-level caveat above.
///
/// Returns `'\u{FFFD}'` (REPLACEMENT CHARACTER) for the small number of
/// codes cp1252 itself leaves unassigned.
pub fn win_ansi_to_unicode(code: u8) -> char {
    match code {
        0x00..=0x7F => code as char,
        0x80 => '\u{20AC}', // Euro sign
        0x81 => '\u{FFFD}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8D => '\u{FFFD}',
        0x8E => '\u{017D}',
        0x8F => '\u{FFFD}',
        0x90 => '\u{FFFD}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9D => '\u{FFFD}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        // 0xA0..=0xFF: identical to Latin-1/ISO-8859-1, i.e. code == scalar
        // value, which is also what Windows-1252 does in this range.
        other => other as char,
    }
}

/// Decodes a byte string (as found in a simple font's `Tj`/`TJ` operands)
/// to a `String` using [`win_ansi_to_unicode`] for every byte.
pub fn win_ansi_bytes_to_string(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| win_ansi_to_unicode(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_range_is_identity() {
        assert_eq!(win_ansi_to_unicode(b'A'), 'A');
        assert_eq!(win_ansi_to_unicode(b'0'), '0');
        assert_eq!(win_ansi_to_unicode(b' '), ' ');
    }

    #[test]
    fn cp1252_special_block() {
        assert_eq!(win_ansi_to_unicode(0x80), '\u{20AC}'); // Euro
        assert_eq!(win_ansi_to_unicode(0x91), '\u{2018}'); // left single quote
        assert_eq!(win_ansi_to_unicode(0x99), '\u{2122}'); // trademark
    }

    #[test]
    fn undefined_codes_map_to_replacement_char() {
        assert_eq!(win_ansi_to_unicode(0x81), '\u{FFFD}');
        assert_eq!(win_ansi_to_unicode(0x8D), '\u{FFFD}');
    }

    #[test]
    fn latin1_supplement_range_is_identity() {
        assert_eq!(win_ansi_to_unicode(0xE9), '\u{00E9}'); // é
        assert_eq!(win_ansi_to_unicode(0xFF), '\u{00FF}'); // ÿ
    }

    #[test]
    fn decodes_byte_string() {
        // "Caf\xE9" (Latin-1 'é') should decode to "Café".
        let s = win_ansi_bytes_to_string(b"Caf\xE9");
        assert_eq!(s, "Café");
    }
}
