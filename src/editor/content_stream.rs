//! A generic, round-trippable parser for PDF content-stream operators
//! (ISO 32000-1:2008 Section 7.8.2 "Content Streams", Table 51).
//!
//! A content stream is a sequence of *operands* (objects, per 7.3, except
//! that indirect references and streams are not allowed as operands)
//! followed by an *operator* keyword, plus the special `BI`/`ID`/`EI`
//! inline-image construct (8.9.7). This is intentionally a much smaller
//! grammar than [`crate::content::Operator`] (which is a *builder* enum
//! covering only the operators this crate knows how to construct): here
//! we need to preserve every operator an arbitrary, already-existing PDF
//! might contain, including ones this crate has no dedicated variant for
//! (e.g. `sh`, `BDC`/`EMC`, `gs`), so operators are kept as a generic
//! `(name, operands)` pair rather than mapped onto that enum.
//!
//! Re-serialization normalizes whitespace and number formatting (see
//! [`crate::object::Object::to_pdf_string`]); it is semantically
//! equivalent but not byte-identical to the input.

use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfString};
use crate::parser::InlineImage;

/// One item of a parsed content stream.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ContentItem {
    /// `operand1 operand2 ... operandN operator`, e.g. `100 200 m` or
    /// `(Hello) Tj`.
    Op {
        operator: String,
        operands: Vec<Object>,
    },
    /// A `BI ... ID ... EI` inline image, kept opaque.
    InlineImage(InlineImage),
    /// A trailing byte range that failed to parse as either of the
    /// above. Preserved verbatim (rather than dropped) so a corrupt tail
    /// doesn't silently lose content; see [`parse_content_stream`].
    Raw(Vec<u8>),
}

/// Parses `data` into a sequence of [`ContentItem`]s.
///
/// This is a best-effort parser over untrusted input: if a syntax error
/// is hit partway through, parsing stops and everything from that point
/// to the end of `data` is preserved as a single [`ContentItem::Raw`]
/// rather than being discarded, so editing operations that don't touch
/// the malformed region are still lossless for the rest of the stream.
pub(crate) fn parse_content_stream(data: &[u8]) -> Vec<ContentItem> {
    let mut items = Vec::new();
    let mut input = data;

    loop {
        let after_ws = skip_ws_and_comments(input);
        if after_ws.is_empty() {
            break;
        }

        if strip_prefix_word(after_ws, b"BI").is_some() {
            // `BI` is itself a valid "keyword" token, but ISO 32000-1
            // reserves it exclusively for inline images, so try that
            // first.
            match crate::parser::parse_inline_image(after_ws) {
                Ok((rest, img)) => {
                    items.push(ContentItem::InlineImage(img));
                    input = rest;
                    continue;
                }
                Err(_) => {
                    items.push(ContentItem::Raw(after_ws.to_vec()));
                    break;
                }
            }
        }

        match parse_statement(after_ws) {
            Some((rest, operator, operands)) => {
                items.push(ContentItem::Op { operator, operands });
                input = rest;
            }
            None => {
                items.push(ContentItem::Raw(after_ws.to_vec()));
                break;
            }
        }
    }

    items
}

/// Parses one `operand* operator` statement, returning the remaining
/// input, the operator keyword, and the operands collected before it.
fn parse_statement(input: &[u8]) -> Option<(&[u8], String, Vec<Object>)> {
    let mut operands = Vec::new();
    let mut cur = input;

    loop {
        cur = skip_ws_and_comments(cur);
        if cur.is_empty() {
            return None;
        }

        if let Some((rest, operand)) = parse_operand(cur) {
            operands.push(operand);
            cur = rest;
            continue;
        }

        // Not an operand: must be the operator keyword terminating this
        // statement.
        let (rest, keyword) = parse_keyword(cur)?;
        return Some((rest, keyword, operands));
    }
}

/// Attempts to parse one operand object (number, string, name, array,
/// dictionary, boolean, or null). Content-stream operands never include
/// indirect references or streams (ISO 32000-1 7.8.2).
fn parse_operand(input: &[u8]) -> Option<(&[u8], Object)> {
    let bytes = input;
    match bytes.first()? {
        b'/' => {
            let (rest, name) = take_name(&bytes[1..])?;
            Some((rest, Object::Name(PdfName::new_unchecked(name))))
        }
        b'(' => {
            let (rest, s) = take_literal_string(bytes)?;
            Some((rest, Object::String(PdfString::Literal(s))))
        }
        b'<' if bytes.get(1) == Some(&b'<') => {
            let (rest, dict) = take_dict(bytes)?;
            Some((rest, Object::Dictionary(dict)))
        }
        b'<' => {
            let (rest, s) = take_hex_string(bytes)?;
            Some((rest, Object::String(PdfString::Hex(s))))
        }
        b'[' => {
            let (rest, arr) = take_array(bytes)?;
            Some((rest, Object::Array(arr)))
        }
        b'+' | b'-' | b'.' | b'0'..=b'9' => take_number(bytes),
        _ => {
            // `true`/`false`/`null` are keyword-shaped but are operands,
            // not operators; special-case them ahead of the generic
            // keyword scan.
            if let Some(rest) = strip_prefix_word(bytes, b"true") {
                Some((rest, Object::Boolean(true)))
            } else if let Some(rest) = strip_prefix_word(bytes, b"false") {
                Some((rest, Object::Boolean(false)))
            } else if let Some(rest) = strip_prefix_word(bytes, b"null") {
                Some((rest, Object::Null))
            } else {
                None
            }
        }
    }
}

/// Matches `word` at the start of `input` provided it is followed by a
/// delimiter/whitespace/EOF (so e.g. `nullify` doesn't match `null`).
fn strip_prefix_word<'a>(input: &'a [u8], word: &[u8]) -> Option<&'a [u8]> {
    let rest = input.strip_prefix(word)?;
    match rest.first() {
        None => Some(rest),
        Some(&c) if is_whitespace(c) || is_delimiter(c) => Some(rest),
        _ => None,
    }
}

fn is_whitespace(c: u8) -> bool {
    matches!(c, b'\0' | b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

fn is_delimiter(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn skip_ws_and_comments(mut input: &[u8]) -> &[u8] {
    loop {
        while let Some(&c) = input.first() {
            if is_whitespace(c) {
                input = &input[1..];
            } else {
                break;
            }
        }
        if input.first() == Some(&b'%') {
            // Comment: runs to end of line (ISO 32000-1 7.2.4).
            let end = input.iter().position(|&c| c == b'\n' || c == b'\r').unwrap_or(input.len());
            input = &input[end..];
            continue;
        }
        break;
    }
    input
}

/// A bare operator keyword: a maximal run of regular (non-whitespace,
/// non-delimiter) characters, e.g. `Tj`, `re`, `f*`, `'`, `"`, `BDC`.
fn parse_keyword(input: &[u8]) -> Option<(&[u8], String)> {
    let end = input
        .iter()
        .position(|&c| is_whitespace(c) || is_delimiter(c))
        .unwrap_or(input.len());
    if end == 0 {
        return None;
    }
    let s = std::str::from_utf8(&input[..end]).ok()?.to_string();
    Some((&input[end..], s))
}

fn take_name(input: &[u8]) -> Option<(&[u8], String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < input.len() {
        let c = input[i];
        if is_whitespace(c) || is_delimiter(c) {
            break;
        }
        if c == b'#' && i + 2 < input.len() {
            let hex = std::str::from_utf8(&input[i + 1..i + 3]).ok()?;
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    Some((&input[i..], String::from_utf8_lossy(&out).into_owned()))
}

fn take_number(input: &[u8]) -> Option<(&[u8], Object)> {
    let end = input
        .iter()
        .position(|&c| !(c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.'))
        .unwrap_or(input.len());
    if end == 0 {
        return None;
    }
    let s = std::str::from_utf8(&input[..end]).ok()?;
    let rest = &input[end..];
    if s.contains('.') {
        s.parse::<f64>().ok().map(|v| (rest, Object::Real(v)))
    } else {
        s.parse::<i64>()
            .map(|v| (rest, Object::Integer(v)))
            .or_else(|_| s.parse::<f64>().map(|v| (rest, Object::Real(v))))
            .ok()
    }
}

fn take_literal_string(input: &[u8]) -> Option<(&[u8], Vec<u8>)> {
    debug_assert_eq!(input.first(), Some(&b'('));
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut i = 1usize;
    loop {
        let &c = input.get(i)?;
        match c {
            b'(' => {
                depth += 1;
                out.push(c);
                i += 1;
            }
            b')' => {
                if depth == 0 {
                    i += 1;
                    return Some((&input[i..], out));
                }
                depth -= 1;
                out.push(c);
                i += 1;
            }
            b'\\' => {
                let &next = input.get(i + 1)?;
                match next {
                    b'n' => {
                        out.push(b'\n');
                        i += 2;
                    }
                    b'r' => {
                        out.push(b'\r');
                        i += 2;
                    }
                    b't' => {
                        out.push(b'\t');
                        i += 2;
                    }
                    b'b' => {
                        out.push(0x08);
                        i += 2;
                    }
                    b'f' => {
                        out.push(0x0C);
                        i += 2;
                    }
                    b'(' | b')' | b'\\' => {
                        out.push(next);
                        i += 2;
                    }
                    b'\n' => {
                        i += 2; // Line continuation: escaped newline is dropped.
                    }
                    b'\r' => {
                        i += 2;
                        if input.get(i) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    b'0'..=b'7' => {
                        let mut val: u32 = 0;
                        let mut n = 0;
                        i += 1;
                        while n < 3 {
                            match input.get(i) {
                                Some(&d @ b'0'..=b'7') => {
                                    val = val * 8 + (d - b'0') as u32;
                                    i += 1;
                                    n += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push((val & 0xFF) as u8);
                    }
                    other => {
                        out.push(other);
                        i += 2;
                    }
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
}

fn take_hex_string(input: &[u8]) -> Option<(&[u8], Vec<u8>)> {
    debug_assert_eq!(input.first(), Some(&b'<'));
    let mut i = 1usize;
    let mut digits = Vec::new();
    loop {
        let &c = input.get(i)?;
        if c == b'>' {
            i += 1;
            break;
        }
        if c.is_ascii_hexdigit() {
            digits.push(c);
        } else if !is_whitespace(c) {
            return None;
        }
        i += 1;
    }
    if digits.len() % 2 == 1 {
        digits.push(b'0');
    }
    let mut out = Vec::with_capacity(digits.len() / 2);
    for chunk in digits.chunks(2) {
        let s = std::str::from_utf8(chunk).ok()?;
        out.push(u8::from_str_radix(s, 16).ok()?);
    }
    Some((&input[i..], out))
}

fn take_array(input: &[u8]) -> Option<(&[u8], PdfArray)> {
    debug_assert_eq!(input.first(), Some(&b'['));
    let mut cur = &input[1..];
    let mut arr = PdfArray::new();
    loop {
        cur = skip_ws_and_comments(cur);
        if cur.first() == Some(&b']') {
            return Some((&cur[1..], arr));
        }
        let (rest, obj) = parse_operand(cur)?;
        arr.push(obj);
        cur = rest;
    }
}

fn take_dict(input: &[u8]) -> Option<(&[u8], PdfDictionary)> {
    debug_assert!(input.starts_with(b"<<"));
    let mut cur = &input[2..];
    let mut dict = PdfDictionary::new();
    loop {
        cur = skip_ws_and_comments(cur);
        if cur.starts_with(b">>") {
            return Some((&cur[2..], dict));
        }
        if cur.first() != Some(&b'/') {
            return None;
        }
        let (rest, key) = take_name(&cur[1..])?;
        cur = skip_ws_and_comments(rest);
        let (rest, value) = parse_operand(cur)?;
        dict.set(key, value);
        cur = rest;
    }
}

/// Re-serializes parsed content items back into content-stream bytes.
pub(crate) fn serialize_content_stream(items: &[ContentItem]) -> Vec<u8> {
    let mut out = Vec::new();
    for item in items {
        match item {
            ContentItem::Op { operator, operands } => {
                for operand in operands {
                    out.extend_from_slice(operand.to_pdf_string().as_bytes());
                    out.push(b' ');
                }
                out.extend_from_slice(operator.as_bytes());
                out.push(b'\n');
            }
            ContentItem::InlineImage(img) => {
                out.extend_from_slice(b"BI\n");
                for (key, value) in img.dictionary.iter() {
                    out.push(b'/');
                    out.extend_from_slice(key.as_bytes());
                    out.push(b' ');
                    out.extend_from_slice(value.to_pdf_string().as_bytes());
                    out.push(b'\n');
                }
                out.extend_from_slice(b"ID ");
                out.extend_from_slice(&img.data);
                out.extend_from_slice(b"\nEI\n");
            }
            ContentItem::Raw(bytes) => {
                out.extend_from_slice(bytes);
            }
        }
    }
    out
}

/// Replaces every occurrence of `find` with `replace` inside the raw
/// bytes of text-showing operators (`Tj`, `'`, `"`, and the string
/// elements of `TJ`'s array). Returns the number of operands modified.
///
/// This is a byte-level substring replace, appropriate for simple
/// single-byte text encodings (WinAnsiEncoding/PDFDocEncoding/ASCII); see
/// the [module docs](crate::editor) for why full text-layout-aware
/// replacement is out of scope.
pub(crate) fn replace_text_in_items(items: &mut [ContentItem], find: &[u8], replace: &[u8]) -> usize {
    if find.is_empty() {
        return 0;
    }
    let mut count = 0;
    for item in items.iter_mut() {
        let ContentItem::Op { operator, operands } = item else {
            continue;
        };
        match operator.as_str() {
            "Tj" | "'" => {
                if let Some(Object::String(s)) = operands.last_mut() {
                    count += replace_in_string(s, find, replace);
                }
            }
            "\"" => {
                if let Some(Object::String(s)) = operands.last_mut() {
                    count += replace_in_string(s, find, replace);
                }
            }
            "TJ" => {
                if let Some(Object::Array(arr)) = operands.first_mut() {
                    let mut new_elems = Vec::with_capacity(arr.len());
                    for elem in arr.iter() {
                        let mut elem = elem.clone();
                        if let Object::String(s) = &mut elem {
                            count += replace_in_string(s, find, replace);
                        }
                        new_elems.push(elem);
                    }
                    *arr = new_elems.into_iter().collect();
                }
            }
            _ => {}
        }
    }
    count
}

fn replace_in_string(s: &mut PdfString, find: &[u8], replace: &[u8]) -> usize {
    let bytes = s.as_bytes();
    if !bytes.windows(find.len().max(1)).any(|w| w == find) {
        return 0;
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut count = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(find) {
            out.extend_from_slice(replace);
            i += find.len();
            count += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    *s = match s {
        PdfString::Hex(_) => PdfString::Hex(out),
        PdfString::Literal(_) => PdfString::Literal(out),
    };
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_simple_operators() {
        let src = b"q 1 0 0 rg 100 100 200 150 re f Q";
        let items = parse_content_stream(src);
        // q | (1 0 0) rg | (100 100 200 150) re | f | Q
        assert_eq!(items.len(), 5);
        match &items[0] {
            ContentItem::Op { operator, operands } => {
                assert_eq!(operator, "q");
                assert!(operands.is_empty());
            }
            other => panic!("expected Op, got {other:?}"),
        }
        let out = serialize_content_stream(&items);
        let out_str = String::from_utf8(out).unwrap();
        assert!(out_str.contains("1 0 0 rg"));
        assert!(out_str.contains("100 100 200 150 re"));
    }

    #[test]
    fn test_parse_text_showing_operators() {
        let src = b"BT /F1 12 Tf (Hello) Tj ET";
        let items = parse_content_stream(src);
        let op_names: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                ContentItem::Op { operator, .. } => Some(operator.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(op_names, vec!["BT", "Tf", "Tj", "ET"]);
    }

    #[test]
    fn test_parse_tj_array() {
        let src = b"[(Hello) -250 (World)] TJ";
        let items = parse_content_stream(src);
        match &items[0] {
            ContentItem::Op { operator, operands } => {
                assert_eq!(operator, "TJ");
                assert_eq!(operands.len(), 1);
                match &operands[0] {
                    Object::Array(arr) => assert_eq!(arr.len(), 3),
                    other => panic!("expected array, got {other:?}"),
                }
            }
            other => panic!("expected Op, got {other:?}"),
        }
    }

    #[test]
    fn test_replace_text_in_tj() {
        let src = b"BT (Hello World) Tj ET";
        let mut items = parse_content_stream(src);
        let count = replace_text_in_items(&mut items, b"World", b"Rust");
        assert_eq!(count, 1);
        let out = serialize_content_stream(&items);
        assert!(String::from_utf8(out).unwrap().contains("Hello Rust"));
    }

    #[test]
    fn test_replace_text_in_tj_array() {
        let src = b"[(Hello) -250 (World)] TJ";
        let mut items = parse_content_stream(src);
        let count = replace_text_in_items(&mut items, b"World", b"Rust!!");
        assert_eq!(count, 1);
        let out = String::from_utf8(serialize_content_stream(&items)).unwrap();
        assert!(out.contains("Rust!!"));
        assert!(!out.contains("World"));
    }

    #[test]
    fn test_malformed_trailing_bytes_preserved_not_dropped() {
        // Unterminated literal string at the end - must not panic, and
        // the well-formed prefix must still come through.
        let src = b"q Q (unterminated";
        let items = parse_content_stream(src);
        assert!(matches!(items.last(), Some(ContentItem::Raw(_))));
        let out = serialize_content_stream(&items);
        // Nothing is silently discarded: the raw tail bytes are retained.
        assert!(out.ends_with(b"(unterminated"));
    }

    #[test]
    fn test_comment_is_skipped() {
        let src = b"q % this is a comment\nQ";
        let items = parse_content_stream(src);
        let op_names: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                ContentItem::Op { operator, .. } => Some(operator.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(op_names, vec!["q", "Q"]);
    }

    #[test]
    fn test_empty_stream() {
        assert!(parse_content_stream(b"").is_empty());
        assert!(parse_content_stream(b"   \n  ").is_empty());
    }
}
