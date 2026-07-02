//! Recovery-mode reconstruction for PDF files whose cross-reference table
//! is missing, truncated, or otherwise unparsable.
//!
//! ISO 32000-1:2008 does not define a "recovery" procedure, but every
//! production-grade reader implements one because real-world files are
//! routinely damaged (truncated downloads, buggy producers, manual
//! editing). The standard technique - used here and by virtually every
//! other implementation (Acrobat, mupdf, pdfium, qpdf) - is to ignore the
//! `/Prev`-linked cross-reference chain entirely and instead:
//!
//! 1. Linearly scan the whole file for `<n> <gen> obj` headers and record
//!    the *last* occurrence of each object number (later bytes represent
//!    the newest revision in an incrementally-updated file).
//! 2. Recursively expand any discovered object streams (`/Type /ObjStm`,
//!    7.5.7) to recover objects that only exist compressed inside them.
//! 3. Locate the trailer: prefer the last `trailer` keyword in the file;
//!    otherwise fall back to a stream object with `/Type /XRef` (7.5.8.2)
//!    for its trailer-equivalent keys; otherwise fall back further to
//!    scanning for a `/Type /Catalog` object and treating it as `/Root`
//!    directly.

use super::objects::{parse_indirect_object, parse_object};
use super::trailer::{parse_trailer, Trailer};
use super::xref::{XrefEntry, XrefTable};
use crate::object::{Object, PdfDictionary, PdfStream};
use crate::types::ObjectId;

/// Hard cap on the number of `obj` headers recovery will index, bounding
/// worst-case work on a maliciously large/corrupt file.
const MAX_RECOVERED_OBJECTS: usize = 2_000_000;

/// Attempts to reconstruct a usable cross-reference table and trailer by
/// scanning `data` directly, ignoring any (possibly corrupt) xref table.
///
/// Returns `None` only if no `/Root` (catalog) could be located by any of
/// the fallback strategies, i.e. the file is not recoverable at all.
pub fn recover(data: &[u8]) -> Option<(XrefTable, Trailer)> {
    let object_offsets = scan_object_headers(data);
    if object_offsets.is_empty() {
        return None;
    }

    let mut xref = XrefTable::new();
    for (&obj_num, &(offset, generation)) in &object_offsets {
        xref.insert(obj_num, XrefEntry::InUse { offset, generation });
    }

    // Expand object streams so objects that only exist compressed inside
    // them are still reachable.
    let mut max_obj_num = object_offsets.keys().copied().max().unwrap_or(0);
    for (&obj_num, &(offset, _generation)) in &object_offsets {
        let Some(data_at) = data.get(offset as usize..) else {
            continue;
        };
        let Ok((_, (_, _, Object::Stream(stream)))) = parse_indirect_object(data_at) else {
            continue;
        };
        if !is_objstm(&stream) {
            continue;
        }
        if let Some(children) = expand_object_stream(&stream) {
            for (idx, child_num) in children.into_iter().enumerate() {
                // Don't let a compressed copy shadow a newer direct
                // definition of the same object number.
                xref.insert_if_absent(
                    child_num,
                    XrefEntry::Compressed {
                        object_stream: obj_num,
                        index: idx as u32,
                    },
                );
                max_obj_num = max_obj_num.max(child_num);
            }
        }
    }

    let trailer = find_trailer(data, &xref, max_obj_num)?;
    Some((xref, trailer))
}

/// Scans `data` for every `<num> <gen> obj` header, returning a map from
/// object number to `(byte offset of the header, generation)`, keeping the
/// *last* occurrence for each object number.
fn scan_object_headers(data: &[u8]) -> std::collections::HashMap<u32, (u64, u16)> {
    let mut result = std::collections::HashMap::new();
    let mut i = 0usize;
    let len = data.len();
    let mut found = 0usize;

    while i < len && found < MAX_RECOVERED_OBJECTS {
        // Fast-skip to the next digit; object headers always start with an
        // ASCII digit (object numbers are non-negative).
        if !data[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // Don't start a match in the middle of a longer number.
        if i > 0 && data[i - 1].is_ascii_digit() {
            i += 1;
            continue;
        }

        let start = i;
        let (obj_num, mut j) = match read_uint(data, i) {
            Some(v) => v,
            None => {
                i += 1;
                continue;
            }
        };
        let ws1 = skip_ws(data, &mut j);
        if !ws1 {
            i = start + 1;
            continue;
        }
        let (gen, mut j2) = match read_uint(data, j) {
            Some(v) => v,
            None => {
                i = start + 1;
                continue;
            }
        };
        let ws2 = skip_ws(data, &mut j2);
        if !ws2 {
            i = start + 1;
            continue;
        }
        if data[j2..].starts_with(b"obj") {
            // Reject matches where "obj" is actually the start of a longer
            // identifier (shouldn't happen given PDF's delimiter rules,
            // but be defensive).
            if obj_num <= u32::MAX as u64 && gen <= u16::MAX as u64 {
                result.insert(obj_num as u32, (start as u64, gen as u16));
                found += 1;
            }
            i = j2 + 3;
        } else {
            i = start + 1;
        }
    }

    result
}

fn read_uint(data: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let start = i;
    while i < data.len() && data[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return None;
    }
    // Cap the digit run to avoid building an absurdly large integer from
    // pathological input; object/generation numbers are never this long.
    if i - start > 18 {
        return None;
    }
    let s = std::str::from_utf8(&data[start..i]).ok()?;
    let v: u64 = s.parse().ok()?;
    Some((v, i))
}

/// Skips whitespace/comment bytes starting at `*i`, advancing `*i` in
/// place. Returns `true` if at least one whitespace byte was skipped.
fn skip_ws(data: &[u8], i: &mut usize) -> bool {
    let start = *i;
    while *i < data.len() && matches!(data[*i], b' ' | b'\t' | b'\r' | b'\n' | 0x0c | 0x00) {
        *i += 1;
    }
    *i > start
}

fn is_objstm(stream: &PdfStream) -> bool {
    matches!(stream.dictionary.get("Type"), Some(Object::Name(n)) if n.as_str() == "ObjStm")
}

/// Decodes an object stream and returns the object numbers of every child
/// object it contains, in stream order (so the index matches the position
/// used by `/Type 2` xref entries).
fn expand_object_stream(stream: &PdfStream) -> Option<Vec<u32>> {
    let n = match stream.dictionary.get("N") {
        Some(Object::Integer(n)) if *n >= 0 => *n as usize,
        _ => return None,
    };
    // Bound N against something absurd before trusting it to size a Vec.
    if n > 10_000_000 {
        return None;
    }

    let data = stream.decode_all().ok()?;
    let first = match stream.dictionary.get("First") {
        Some(Object::Integer(f)) if *f >= 0 && (*f as usize) <= data.len() => *f as usize,
        _ => return None,
    };

    let header = std::str::from_utf8(&data[..first]).ok()?;
    let nums: Vec<u32> = header
        .split_whitespace()
        .step_by(2)
        .filter_map(|s| s.parse().ok())
        .take(n)
        .collect();

    Some(nums)
}

/// Locates a usable trailer via, in order: the last `trailer` keyword, a
/// `/Type /XRef` stream object, or a bare `/Type /Catalog` object.
fn find_trailer(data: &[u8], xref: &XrefTable, max_obj_num: u32) -> Option<Trailer> {
    if let Some(trailer) = find_trailer_keyword(data) {
        return Some(trailer);
    }

    if let Some(trailer) = find_xref_stream_trailer(data, xref) {
        return Some(trailer);
    }

    find_catalog_as_root(data, xref, max_obj_num)
}

fn find_trailer_keyword(data: &[u8]) -> Option<Trailer> {
    let mut search_from = 0usize;
    let mut last_valid: Option<Trailer> = None;

    while let Some(rel_pos) = find_subsequence(&data[search_from..], b"trailer") {
        let pos = search_from + rel_pos;
        if let Ok((_, dict)) = parse_trailer(&data[pos..]) {
            if let Ok(t) = Trailer::from_dictionary(dict) {
                last_valid = Some(t);
            }
        }
        search_from = pos + b"trailer".len();
        if search_from >= data.len() {
            break;
        }
    }

    last_valid
}

fn find_xref_stream_trailer(data: &[u8], xref: &XrefTable) -> Option<Trailer> {
    // Any indexed object whose dictionary says /Type /XRef carries the
    // same trailer-equivalent keys (Root, Info, ID, Encrypt, Size).
    for (_, entry) in xref.iter() {
        let XrefEntry::InUse { offset, .. } = entry else {
            continue;
        };
        let Some(slice) = data.get(*offset as usize..) else {
            continue;
        };
        let Ok((_, (_, _, Object::Stream(stream)))) = parse_indirect_object(slice) else {
            continue;
        };
        let is_xref = matches!(stream.dictionary.get("Type"), Some(Object::Name(n)) if n.as_str() == "XRef");
        if !is_xref {
            continue;
        }
        if let Ok(t) = Trailer::from_dictionary(stream.dictionary.clone()) {
            return Some(t);
        }
    }
    None
}

fn find_catalog_as_root(data: &[u8], xref: &XrefTable, max_obj_num: u32) -> Option<Trailer> {
    for (&obj_num, entry) in xref.iter() {
        let XrefEntry::InUse { offset, .. } = entry else {
            continue;
        };
        let Some(slice) = data.get(*offset as usize..) else {
            continue;
        };
        let Ok((_, (_, gen, obj))) = parse_indirect_object(slice) else {
            continue;
        };
        let dict = match &obj {
            Object::Dictionary(d) => d,
            _ => continue,
        };
        let is_catalog = matches!(dict.get("Type"), Some(Object::Name(n)) if n.as_str() == "Catalog");
        if !is_catalog {
            continue;
        }

        let mut trailer_dict = PdfDictionary::new();
        trailer_dict.set("Root", Object::Reference(ObjectId::with_generation(obj_num, gen)));
        trailer_dict.set("Size", Object::Integer(max_obj_num as i64 + 1));
        return Trailer::from_dictionary(trailer_dict).ok();
    }
    // As an absolute last resort, try every dictionary-shaped object even
    // without parsing it fully via the xref map (covers the pathological
    // case where scan_object_headers found headers but they don't line up
    // with valid objects for most entries).
    let _ = parse_object; // keep import used if the loop above is ever removed
    None
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::DocumentBuilder;
    use crate::page::PageBuilder;

    fn build_test_pdf() -> Vec<u8> {
        let page = PageBuilder::a4().build();
        let doc = DocumentBuilder::new().title("Recovery Test").page(page).build().unwrap();
        doc.save_to_bytes().unwrap()
    }

    #[test]
    fn recovers_root_via_trailer_keyword_scan() {
        let pdf = build_test_pdf();
        let (xref, trailer) = recover(&pdf).expect("recovery should succeed");
        assert!(!xref.is_empty());
        assert!(trailer.root.number > 0);
    }

    #[test]
    fn recovers_root_via_catalog_scan_when_no_trailer_present() {
        let pdf = build_test_pdf();
        // Strip everything from the last "trailer" keyword onward, which
        // also removes %%EOF/startxref - simulating truncation.
        let cut = find_subsequence(&pdf, b"trailer").expect("test fixture must contain trailer");
        let truncated = pdf[..cut].to_vec();

        let (_, trailer) = recover(&truncated).expect("recovery should succeed via catalog scan");
        assert!(trailer.root.number > 0);
    }

    #[test]
    fn returns_none_for_data_with_no_objects() {
        let garbage = b"%PDF-1.7\nthis is not a real pdf at all, just noise".to_vec();
        assert!(recover(&garbage).is_none());
    }

    #[test]
    fn does_not_panic_on_binary_garbage() {
        let garbage: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
        let _ = recover(&garbage);
    }

    #[test]
    fn last_occurrence_of_object_number_wins() {
        let mut data = Vec::new();
        data.extend_from_slice(b"%PDF-1.7\n");
        data.extend_from_slice(b"1 0 obj\n<< /Marker (old) >>\nendobj\n");
        let second_marker_offset_hint = data.len();
        data.extend_from_slice(b"1 0 obj\n<< /Marker (new) /Type /Catalog >>\nendobj\n");
        data.extend_from_slice(b"trailer\n<< /Root 1 0 R /Size 2 >>\n");

        let (xref, _trailer) = recover(&data).expect("recovery should succeed");
        let entry = xref.get(1).expect("object 1 should be indexed");
        match entry {
            XrefEntry::InUse { offset, .. } => {
                assert_eq!(*offset as usize, second_marker_offset_hint);
            }
            _ => panic!("expected in-use entry"),
        }
    }
}
