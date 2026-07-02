//! Small helpers shared by the AcroForm, annotation, outline and tagged
//! structure editing submodules.

use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream, PdfString};
use crate::types::Rectangle;

/// Encodes a Rust string as a PDF *text string* (ISO 32000-1:2008 7.9.2.2).
///
/// Text strings (as opposed to byte strings) hold human-readable text and
/// may be encoded either in PDFDocEncoding or UTF-16BE with a `U+FEFF`
/// byte-order-mark prefix. This crate does not implement the (mostly
/// Latin-1-like, but not identical) PDFDocEncoding table, so: text that is
/// pure ASCII is written as a literal string (every ASCII byte is also a
/// valid PDFDocEncoding byte, so this is always spec-correct); anything
/// else is written as UTF-16BE with the required BOM, which any conformant
/// reader must also support for text strings.
pub(crate) fn to_pdf_text_string(s: &str) -> PdfString {
    if s.is_ascii() {
        return PdfString::literal(s);
    }
    let mut bytes = vec![0xFEu8, 0xFF];
    for unit in s.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    PdfString::Hex(bytes)
}

/// Decodes a PDF text string (ISO 32000-1 7.9.2.2) back to a Rust `String`.
///
/// If the bytes start with the UTF-16BE byte-order-mark (`0xFE 0xFF`) they
/// are decoded as UTF-16BE (lossily replacing unpaired surrogates);
/// otherwise the bytes are assumed to already be ASCII/PDFDocEncoding and
/// are decoded as Latin-1 (every PDFDocEncoding code point in the common
/// printable-ASCII range maps identically to Latin-1/UTF-8), which is a
/// documented approximation rather than a full PDFDocEncoding table.
pub(crate) fn from_pdf_text_string(s: &PdfString) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    bytes.iter().map(|&b| b as char).collect()
}

/// Reads a [`Rectangle`] from a (already-resolved) PDF array object,
/// per ISO 32000-1 7.9.5 ("Rectangles"): four numbers `[llx lly urx ury]`,
/// normalized so `llx <= urx` and `lly <= ury` regardless of the order the
/// corners were written in (the spec explicitly allows either corner
/// pair/ordering).
pub(crate) fn rect_from_object(obj: &Object) -> Option<Rectangle> {
    let Object::Array(arr) = obj else { return None };
    if arr.len() != 4 {
        return None;
    }
    let mut v = [0.0f64; 4];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = arr.get(i)?.as_real()?;
    }
    let (llx, urx) = (v[0].min(v[2]), v[0].max(v[2]));
    let (lly, ury) = (v[1].min(v[3]), v[1].max(v[3]));
    Some(Rectangle::new(llx, lly, urx, ury))
}

/// Writes a [`Rectangle`] as a PDF array `[llx lly urx ury]`.
pub(crate) fn rect_to_array(rect: Rectangle) -> PdfArray {
    let mut arr = PdfArray::new();
    arr.push(Object::Real(rect.llx));
    arr.push(Object::Real(rect.lly));
    arr.push(Object::Real(rect.urx));
    arr.push(Object::Real(rect.ury));
    arr
}

/// Picks a resource name not already present in `existing`, preferring
/// `preferred` itself.
pub(crate) fn unique_resource_name(existing: &PdfDictionary, preferred: &str) -> String {
    if !existing.contains_key(preferred) {
        return preferred.to_string();
    }
    for i in 1.. {
        let candidate = format!("{preferred}{i}");
        if !existing.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Builds a minimal Form XObject appearance stream (ISO 32000-1 8.10,
/// 12.5.5) whose `/BBox` is `[0 0 width height]`, with a self-contained
/// `/Resources /Font /Helv` entry (Helvetica, standard 14 - no embedding
/// needed) so the returned stream renders standalone regardless of what
/// resources the annotation's own page or the AcroForm `/DR` happen to
/// declare.
pub(crate) fn appearance_xobject(content: &str, width: f64, height: f64) -> PdfStream {
    appearance_xobject_with_extra_resources(content, width, height, PdfDictionary::new())
}

/// Like [`appearance_xobject`], but merges `extra_resources` (e.g. an
/// `/ExtGState` entry for a highlight annotation's blend mode) into the
/// stream's `/Resources` dictionary alongside the always-present
/// `/Font /Helv`.
pub(crate) fn appearance_xobject_with_extra_resources(
    content: &str,
    width: f64,
    height: f64,
    extra_resources: PdfDictionary,
) -> PdfStream {
    let mut dict = PdfDictionary::new();
    dict.set("Type", Object::Name(PdfName::new_unchecked("XObject")));
    dict.set("Subtype", Object::Name(PdfName::new_unchecked("Form")));

    let mut bbox = PdfArray::new();
    bbox.push(Object::Real(0.0));
    bbox.push(Object::Real(0.0));
    bbox.push(Object::Real(width));
    bbox.push(Object::Real(height));
    dict.set("BBox", Object::Array(bbox));

    let mut resources = PdfDictionary::new();
    let mut font_dict = PdfDictionary::new();
    let mut helv = PdfDictionary::new();
    helv.set("Type", Object::Name(PdfName::new_unchecked("Font")));
    helv.set("Subtype", Object::Name(PdfName::new_unchecked("Type1")));
    helv.set("BaseFont", Object::Name(PdfName::new_unchecked("Helvetica")));
    font_dict.set("Helv", Object::Dictionary(helv));
    resources.set("Font", Object::Dictionary(font_dict));
    for (k, v) in extra_resources.iter() {
        resources.set(k.clone(), v.clone());
    }
    dict.set("Resources", Object::Dictionary(resources));

    PdfStream::with_dictionary(dict, content.as_bytes().to_vec())
}

/// Best-effort parse of a `/DA` default-appearance string (ISO 32000-1
/// 12.7.3.3): looks for a `/Name size Tf` font operator and a trailing
/// gray (`g`) or RGB (`rg`) color operator. This is *not* a full
/// content-stream tokenizer (DA strings are technically a restricted
/// content stream grammar); it only recognizes the flat, single-line form
/// every producer this crate is aware of (including this crate's own
/// `write_form_field`) actually emits, and falls back to sensible
/// defaults for anything else rather than failing.
pub(crate) fn parse_da(da: &str, default_size: f64) -> (f64, crate::color::Color) {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let mut size = default_size;
    let mut color = crate::color::Color::BLACK;

    for (i, tok) in tokens.iter().enumerate() {
        if *tok == "Tf" && i >= 1 {
            if let Ok(s) = tokens[i - 1].parse::<f64>() {
                if s > 0.0 {
                    size = s;
                }
            }
        } else if *tok == "g" && i >= 1 {
            if let Ok(gray) = tokens[i - 1].parse::<f64>() {
                color = crate::color::Color::gray(gray);
            }
        } else if *tok == "rg" && i >= 3 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                tokens[i - 3].parse::<f64>(),
                tokens[i - 2].parse::<f64>(),
                tokens[i - 1].parse::<f64>(),
            ) {
                color = crate::color::Color::rgb(r, g, b);
            }
        }
    }
    (size, color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ascii_round_trips_as_literal() {
        let s = to_pdf_text_string("Hello, World!");
        assert!(matches!(s, PdfString::Literal(_)));
        assert_eq!(from_pdf_text_string(&s), "Hello, World!");
    }

    #[test]
    fn test_non_ascii_round_trips_via_utf16be() {
        let s = to_pdf_text_string("caf\u{e9} \u{2603}");
        assert!(matches!(s, PdfString::Hex(_)));
        assert_eq!(s.as_bytes()[0], 0xFE);
        assert_eq!(s.as_bytes()[1], 0xFF);
        assert_eq!(from_pdf_text_string(&s), "caf\u{e9} \u{2603}");
    }

    #[test]
    fn test_rect_from_object_normalizes_corners() {
        let mut arr = PdfArray::new();
        // Written as [urx ury llx lly] - readers must still normalize.
        arr.push(Object::Real(100.0));
        arr.push(Object::Real(200.0));
        arr.push(Object::Real(10.0));
        arr.push(Object::Real(20.0));
        let rect = rect_from_object(&Object::Array(arr)).unwrap();
        assert_eq!(rect.llx, 10.0);
        assert_eq!(rect.lly, 20.0);
        assert_eq!(rect.urx, 100.0);
        assert_eq!(rect.ury, 200.0);
    }

    #[test]
    fn test_rect_from_object_rejects_malformed() {
        assert!(rect_from_object(&Object::Null).is_none());
        let mut short = PdfArray::new();
        short.push(Object::Real(1.0));
        assert!(rect_from_object(&Object::Array(short)).is_none());
    }

    #[test]
    fn test_unique_resource_name_avoids_collision() {
        let mut existing = PdfDictionary::new();
        existing.set("Helv", Object::Null);
        assert_eq!(unique_resource_name(&existing, "Arial"), "Arial");
        assert_eq!(unique_resource_name(&existing, "Helv"), "Helv1");
    }

    #[test]
    fn test_parse_da_extracts_size_and_gray() {
        let (size, color) = parse_da("/Helv 14 Tf 0.5 g", 12.0);
        assert_eq!(size, 14.0);
        assert!(matches!(color, crate::color::Color::Gray(g) if g.level == 0.5));
    }

    #[test]
    fn test_parse_da_falls_back_on_garbage() {
        let (size, _) = parse_da("not a valid DA string at all", 9.0);
        assert_eq!(size, 9.0);
    }
}
