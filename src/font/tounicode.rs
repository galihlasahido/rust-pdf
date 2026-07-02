//! `ToUnicode` CMap generation and parsing (ISO 32000-1:2008 9.10.3
//! "ToUnicode CMaps", and the Adobe *CMap and CIDFont Files Specification*
//! it normatively references for the CMap program syntax itself).
//!
//! A `ToUnicode` CMap is a small PostScript-like resource, embedded as a
//! PDF stream, that maps the character codes a font's `Tj`/`TJ` strings are
//! expressed in back to Unicode text — the mechanism ISO 32000-1 9.10.3
//! specifies for text extraction/search/accessibility when a font's own
//! encoding does not already imply Unicode.
//!
//! This is independent of the embedded *font program* parsing in
//! [`crate::font::truetype`]; a CMap resource is PDF/PostScript syntax, not
//! sfnt/OpenType binary, so it is implemented directly here rather than
//! through `ttf-parser`.

use std::collections::BTreeMap;

/// Hard cap on the size of a `ToUnicode` CMap stream this parser will scan.
///
/// Guards against pathological/adversarial input (e.g. a multi-gigabyte
/// decompressed stream masquerading as a CMap) driving unbounded parsing
/// work; no legitimate `ToUnicode` CMap comes close to this size.
pub const MAX_CMAP_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

/// Hard cap on the number of `bfchar`/`bfrange` entries accepted from a
/// single CMap, independent of byte size (protects the output map from an
/// adversarial CMap that declares an enormous range in `bfrange`).
const MAX_CMAP_ENTRIES: u64 = 2_000_000;

/// Builds a `ToUnicode` CMap stream body (ISO 32000-1 9.10.3) mapping each
/// `(code, unicode_text)` pair in `entries` to its Unicode string.
///
/// `code_bytes` is the number of bytes each character code occupies in the
/// font's `Tj`/`TJ` strings (`1` for simple fonts, `2` for the `Identity-H`
/// CID encoding this crate's composite fonts use); it determines the
/// `codespacerange` bounds. `entries` need not be sorted.
///
/// Each mapping is emitted as its own `bfchar` entry (simpler and always
/// spec-valid, at the cost of being less compact than `bfrange` grouping);
/// entries are chunked into groups of at most 100 per `beginbfchar`/
/// `endbfchar` block, per the Adobe CMap specification's recommendation
/// that processors may not support single blocks larger than that.
pub fn build_tounicode_cmap(entries: &[(u32, String)], code_bytes: u8) -> Vec<u8> {
    let code_bytes = code_bytes.clamp(1, 2);
    let mut sorted: Vec<(u32, String)> = entries.to_vec();
    sorted.sort_by_key(|(code, _)| *code);

    let hex_digits = code_bytes as usize * 2;
    let max_code: u32 = if code_bytes == 1 { 0xFF } else { 0xFFFF };

    let mut out = String::new();
    out.push_str("/CIDInit /ProcSet findresource begin\n");
    out.push_str("12 dict begin\n");
    out.push_str("begincmap\n");
    out.push_str("/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n");
    out.push_str("/CMapName /Adobe-Identity-UCS def\n");
    out.push_str("/CMapType 2 def\n");
    out.push_str("1 begincodespacerange\n");
    out.push_str(&format!(
        "<{:0w$X}> <{:0w$X}>\n",
        0,
        max_code,
        w = hex_digits
    ));
    out.push_str("endcodespacerange\n");

    for chunk in sorted.chunks(100) {
        out.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (code, text) in chunk {
            out.push_str(&format!(
                "<{:0w$X}> <{}>\n",
                code,
                utf16be_hex(text),
                w = hex_digits
            ));
        }
        out.push_str("endbfchar\n");
    }

    out.push_str("endcmap\n");
    out.push_str("CMapName currentdict /CMap defineresource pop\n");
    out.push_str("end\n");
    out.push_str("end\n");
    out.into_bytes()
}

/// Parses a decoded `ToUnicode` CMap stream, returning a map from character
/// code to the Unicode text it represents.
///
/// Supports both `bfchar` (single code -> text) and `bfrange` (contiguous
/// code range -> either a single incrementing destination, or an explicit
/// array of destinations) entries, since real-world PDFs commonly use
/// `bfrange` for compactness even though this crate's own writer only
/// emits `bfchar` (see [`build_tounicode_cmap`]).
///
/// This is a best-effort parser over untrusted input: malformed entries
/// are skipped rather than aborting the whole parse, and both the input
/// size and the number of produced entries are bounded (see
/// [`MAX_CMAP_BYTES`]/`MAX_CMAP_ENTRIES`) to avoid a hostile CMap (e.g. one
/// `bfrange` declaring a huge span) causing unbounded memory use.
pub fn parse_tounicode_cmap(data: &[u8]) -> BTreeMap<u32, String> {
    let mut map = BTreeMap::new();
    if data.len() > MAX_CMAP_BYTES {
        return map;
    }

    let mut entry_budget = MAX_CMAP_ENTRIES;
    let mut pos = 0usize;
    while pos < data.len() {
        pos = skip_ws(data, pos);
        if data[pos..].starts_with(b"beginbfchar") {
            pos = parse_bfchar_block(data, pos + "beginbfchar".len(), &mut map, &mut entry_budget);
        } else if data[pos..].starts_with(b"beginbfrange") {
            pos = parse_bfrange_block(data, pos + "beginbfrange".len(), &mut map, &mut entry_budget);
        } else {
            pos += 1;
        }
        if entry_budget == 0 {
            break;
        }
    }
    map
}

fn parse_bfchar_block(
    data: &[u8],
    mut pos: usize,
    map: &mut BTreeMap<u32, String>,
    budget: &mut u64,
) -> usize {
    loop {
        pos = skip_ws(data, pos);
        if pos >= data.len() || data[pos..].starts_with(b"endbfchar") {
            return pos + b"endbfchar".len().min(data.len().saturating_sub(pos));
        }
        let Some((src, next)) = read_hex(data, pos) else {
            return pos + 1;
        };
        pos = skip_ws(data, next);
        let Some((dst, next2)) = read_hex(data, pos) else {
            return pos;
        };
        pos = next2;

        if let Some(code) = bytes_to_code(&src) {
            map.insert(code, decode_utf16be(&dst));
            *budget = budget.saturating_sub(1);
            if *budget == 0 {
                return pos;
            }
        }
    }
}

fn parse_bfrange_block(
    data: &[u8],
    mut pos: usize,
    map: &mut BTreeMap<u32, String>,
    budget: &mut u64,
) -> usize {
    loop {
        pos = skip_ws(data, pos);
        if pos >= data.len() || data[pos..].starts_with(b"endbfrange") {
            return pos + b"endbfrange".len().min(data.len().saturating_sub(pos));
        }
        let Some((lo, next)) = read_hex(data, pos) else {
            return pos + 1;
        };
        pos = skip_ws(data, next);
        let Some((hi, next2)) = read_hex(data, pos) else {
            return pos;
        };
        pos = skip_ws(data, next2);

        let (Some(lo_code), Some(hi_code)) = (bytes_to_code(&lo), bytes_to_code(&hi)) else {
            return pos;
        };
        if hi_code < lo_code {
            return pos;
        }
        let span = u64::from(hi_code - lo_code) + 1;
        if span > *budget {
            return pos;
        }

        if data.get(pos) == Some(&b'[') {
            // Array form: [ <dst0> <dst1> ... ]
            pos += 1;
            let mut code = lo_code;
            loop {
                pos = skip_ws(data, pos);
                if data.get(pos) == Some(&b']') {
                    pos += 1;
                    break;
                }
                let Some((dst, next3)) = read_hex(data, pos) else {
                    break;
                };
                pos = next3;
                map.insert(code, decode_utf16be(&dst));
                *budget = budget.saturating_sub(1);
                code = code.saturating_add(1);
                if *budget == 0 {
                    return pos;
                }
            }
        } else if let Some((dst, next3)) = read_hex(data, pos) {
            pos = next3;
            // Single-destination form: successive codes get successive
            // destination values by incrementing only the low-order 16
            // bits (last UTF-16 code unit) of the destination string. This
            // is the behaviour documented for `bfrange` in Adobe's *CMap
            // and CIDFont Files Specification* (not part of ISO 32000-1
            // itself, which normatively references that document for the
            // CMap program syntax — see the module docs).
            if let Some(base) = utf16be_last_unit(&dst) {
                for (i, code) in (lo_code..=hi_code).enumerate() {
                    let unit = base.wrapping_add(i as u16);
                    let mut bytes = dst.clone();
                    let len = bytes.len();
                    if len >= 2 {
                        bytes[len - 2..].copy_from_slice(&unit.to_be_bytes());
                    }
                    map.insert(code, decode_utf16be(&bytes));
                }
            }
            *budget = budget.saturating_sub(span);
            if *budget == 0 {
                return pos;
            }
        } else {
            return pos;
        }
    }
}

fn utf16be_last_unit(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 2 {
        return None;
    }
    let n = bytes.len();
    Some(u16::from_be_bytes([bytes[n - 2], bytes[n - 1]]))
}

fn bytes_to_code(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || bytes.len() > 4 {
        return None;
    }
    let mut v = 0u32;
    for &b in bytes {
        v = (v << 8) | u32::from(b);
    }
    Some(v)
}

fn decode_utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|c| if c.len() == 2 { u16::from_be_bytes([c[0], c[1]]) } else { 0 })
        .collect();
    String::from_utf16_lossy(&units)
}

fn utf16be_hex(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 4);
    for unit in s.encode_utf16() {
        out.push_str(&format!("{:04X}", unit));
    }
    out
}

fn skip_ws(data: &[u8], mut pos: usize) -> usize {
    while pos < data.len() {
        match data[pos] {
            b' ' | b'\t' | b'\r' | b'\n' | 0x0C | b'\0' => pos += 1,
            b'%' => {
                while pos < data.len() && data[pos] != b'\n' {
                    pos += 1;
                }
            }
            _ => break,
        }
    }
    pos
}

/// Reads a `<hex...>` token starting at `pos`. Returns the decoded bytes and
/// the position just past the closing `>`.
fn read_hex(data: &[u8], pos: usize) -> Option<(Vec<u8>, usize)> {
    if data.get(pos) != Some(&b'<') {
        return None;
    }
    let mut i = pos + 1;
    let mut digits = Vec::new();
    while let Some(&c) = data.get(i) {
        if c == b'>' {
            i += 1;
            break;
        }
        if c.is_ascii_hexdigit() {
            digits.push(c);
        } else if !c.is_ascii_whitespace() {
            return None;
        }
        i += 1;
        if digits.len() > 4096 {
            return None; // sanity bound on a single token
        }
    }
    if digits.len() % 2 == 1 {
        digits.push(b'0');
    }
    let mut out = Vec::with_capacity(digits.len() / 2);
    for chunk in digits.chunks(2) {
        let s = std::str::from_utf8(chunk).ok()?;
        out.push(u8::from_str_radix(s, 16).ok()?);
    }
    Some((out, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_bfchar_simple_font() {
        let entries = vec![(0x41u32, "A".to_string()), (0x42u32, "B".to_string())];
        let stream = build_tounicode_cmap(&entries, 1);
        let text = String::from_utf8(stream.clone()).unwrap();
        assert!(text.contains("begincodespacerange"));
        assert!(text.contains("<00> <FF>"));

        let parsed = parse_tounicode_cmap(&stream);
        assert_eq!(parsed.get(&0x41), Some(&"A".to_string()));
        assert_eq!(parsed.get(&0x42), Some(&"B".to_string()));
    }

    #[test]
    fn roundtrip_bfchar_cjk_two_byte_codes() {
        let entries = vec![(1u32, "中".to_string()), (2u32, "日".to_string())];
        let stream = build_tounicode_cmap(&entries, 2);
        let text = String::from_utf8(stream.clone()).unwrap();
        assert!(text.contains("<0000> <FFFF>"));

        let parsed = parse_tounicode_cmap(&stream);
        assert_eq!(parsed.get(&1), Some(&"中".to_string()));
        assert_eq!(parsed.get(&2), Some(&"日".to_string()));
    }

    #[test]
    fn parses_bfrange_single_destination_form() {
        let cmap = b"1 beginbfrange\n<0003> <0005> <0041>\nendbfrange\n";
        let parsed = parse_tounicode_cmap(cmap);
        assert_eq!(parsed.get(&3), Some(&"A".to_string()));
        assert_eq!(parsed.get(&4), Some(&"B".to_string()));
        assert_eq!(parsed.get(&5), Some(&"C".to_string()));
    }

    #[test]
    fn parses_bfrange_array_form() {
        let cmap = b"1 beginbfrange\n<0010> <0012> [<0041> <4E2D> <65E5>]\nendbfrange\n";
        let parsed = parse_tounicode_cmap(cmap);
        assert_eq!(parsed.get(&0x10), Some(&"A".to_string()));
        assert_eq!(parsed.get(&0x11), Some(&"中".to_string()));
        assert_eq!(parsed.get(&0x12), Some(&"日".to_string()));
    }

    #[test]
    fn malformed_cmap_does_not_panic() {
        let garbage = b"beginbfchar <ZZ not hex> endbfchar beginbfrange <> <> endbfrange";
        let parsed = parse_tounicode_cmap(garbage);
        // Best-effort: no panic, and no bogus entries from unparsable hex.
        assert!(parsed.is_empty());
    }

    #[test]
    fn oversized_cmap_is_rejected() {
        let huge = vec![b' '; MAX_CMAP_BYTES + 1];
        assert!(parse_tounicode_cmap(&huge).is_empty());
    }

    #[test]
    fn adversarial_huge_bfrange_span_is_bounded() {
        // <0000> <FFFF> would naively expand to 65536 entries; still well
        // under budget, but confirms it terminates promptly and correctly.
        let cmap = b"1 beginbfrange\n<0000> <FFFF> <0041>\nendbfrange\n";
        let parsed = parse_tounicode_cmap(cmap);
        assert_eq!(parsed.len(), 65536);
        assert_eq!(parsed.get(&0), Some(&"A".to_string()));
    }
}
