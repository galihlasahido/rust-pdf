//! Text extraction from an existing page's content stream (ISO 32000-1:2008
//! 9.10 "Extraction of Text Content").
//!
//! Handles both:
//! - **simple fonts** (`/Subtype /Type1`, `/TrueType`, ...): one byte per
//!   character code (7.8.2); and
//! - **composite (Type 0) fonts** (9.7): codes of whatever width the
//!   active CMap uses — this implementation assumes the common
//!   `Identity-H`/`Identity-V` case of 2-byte codes, which is what this
//!   crate's own writer ([`crate::font::cid`]) always produces, and is
//!   also what the overwhelming majority of third-party CJK PDF producers
//!   use.
//!
//! For each, Unicode text is recovered by, in order of preference:
//! 1. the font's own `/ToUnicode` CMap (9.10.3) — authoritative when
//!    present, for both embedded and non-embedded fonts;
//! 2. for simple fonts with no `/ToUnicode`: the `/Encoding` name/base
//!    encoding via [`crate::font::encoding`]'s WinAnsi-based fallback
//!    table (glyph-name overrides from an `/Encoding` dictionary's
//!    `/Differences` are not resolved — that needs the Adobe Glyph List,
//!    which is not implemented — so those individual codes fall back to
//!    the base encoding instead of their overridden glyph);
//! 3. otherwise (composite font, no `/ToUnicode`): there is no reliable
//!    fallback (a raw CID has no inherent Unicode meaning), so the
//!    REPLACEMENT CHARACTER is emitted for each code. Real-world CID PDFs
//!    intended to be searchable/extractable essentially always carry a
//!    `/ToUnicode` CMap for exactly this reason.
//!
//! This intentionally does not attempt layout reconstruction (word/line
//! detection beyond simple heuristics) — see the [`crate::editor`] module
//! docs for why a full text-layout engine is out of scope.

use super::content_stream::{parse_content_stream, ContentItem};
use super::graph::EditableDocument;
use crate::error::PdfResult;
use crate::font::encoding::win_ansi_bytes_to_string;
use crate::font::tounicode::parse_tounicode_cmap;
use crate::object::{Object, PdfDictionary, PdfString};
use crate::types::ObjectId;
use std::collections::BTreeMap;

/// How a font resource's character codes should be decoded to Unicode.
enum FontDecoder {
    /// Simple font: 1 byte per code.
    Simple {
        tounicode: Option<BTreeMap<u32, String>>,
    },
    /// Composite (Type 0) font: 2 bytes per code (`Identity-H`/`-V`
    /// assumption, see module docs).
    Composite {
        tounicode: Option<BTreeMap<u32, String>>,
    },
}

impl FontDecoder {
    fn code_width(&self) -> usize {
        match self {
            FontDecoder::Simple { .. } => 1,
            FontDecoder::Composite { .. } => 2,
        }
    }

    fn decode(&self, bytes: &[u8]) -> String {
        let width = self.code_width();
        let mut out = String::new();
        for chunk in bytes.chunks(width) {
            let code = chunk
                .iter()
                .fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
            match self {
                FontDecoder::Simple { tounicode } => {
                    if let Some(text) = tounicode.as_ref().and_then(|m| m.get(&code)) {
                        out.push_str(text);
                    } else if chunk.len() == 1 {
                        out.push_str(&win_ansi_bytes_to_string(chunk));
                    }
                }
                FontDecoder::Composite { tounicode } => {
                    if let Some(text) = tounicode.as_ref().and_then(|m| m.get(&code)) {
                        out.push_str(text);
                    } else {
                        out.push('\u{FFFD}');
                    }
                }
            }
        }
        out
    }
}

impl EditableDocument {
    /// Extracts a best-effort Unicode text representation of `page_id`'s
    /// content stream (ISO 32000-1 9.10).
    pub fn extract_page_text(&self, page_id: ObjectId) -> PdfResult<String> {
        let resources = self.effective_resources(page_id)?;
        let font_dict = match resources.get("Font") {
            Some(Object::Dictionary(d)) => d.clone(),
            Some(Object::Reference(id)) => self.get_dictionary(*id).unwrap_or_default(),
            _ => PdfDictionary::new(),
        };

        let mut decoders: BTreeMap<String, FontDecoder> = BTreeMap::new();
        for (name, value) in font_dict.iter() {
            let Ok(dict) = self.resolve_dict_or_ref(value) else {
                continue;
            };
            decoders.insert(name.clone(), self.build_font_decoder(&dict));
        }

        let content = self.page_content_bytes(page_id)?;
        let items = parse_content_stream(&content);

        let mut out = String::new();
        let mut current: Option<&FontDecoder> = None;
        for item in &items {
            let ContentItem::Op { operator, operands } = item else {
                continue;
            };
            match operator.as_str() {
                "Tf" => {
                    if let Some(Object::Name(name)) = operands.first() {
                        current = decoders.get(name.as_str());
                    }
                }
                "Tj" => {
                    if let Some(Object::String(s)) = operands.last() {
                        append_decoded(&mut out, current, s);
                    }
                }
                "'" => {
                    out.push('\n');
                    if let Some(Object::String(s)) = operands.last() {
                        append_decoded(&mut out, current, s);
                    }
                }
                "\"" => {
                    out.push('\n');
                    if let Some(Object::String(s)) = operands.last() {
                        append_decoded(&mut out, current, s);
                    }
                }
                "TJ" => {
                    if let Some(Object::Array(arr)) = operands.first() {
                        for elem in arr.iter() {
                            match elem {
                                Object::String(s) => append_decoded(&mut out, current, s),
                                Object::Integer(n) if *n < -100 => out.push(' '),
                                Object::Real(n) if *n < -100.0 => out.push(' '),
                                _ => {}
                            }
                        }
                    }
                }
                "T*" | "Td" | "TD" => out.push('\n'),
                "ET" => out.push('\n'),
                _ => {}
            }
        }
        Ok(out)
    }

    /// Resolves the effective `/Resources` dictionary for a page, following
    /// `/Parent` (inheritable attribute, ISO 32000-1 Table 30) if the leaf
    /// doesn't set it directly.
    ///
    /// `pub(super)` rather than private so [`crate::editor::redact`] (area
    /// and font/`ToUnicode` bookkeeping needs the same font-resource
    /// resolution this module already implements) can reuse it instead of
    /// duplicating the `/Parent`-inheritance walk.
    pub(super) fn effective_resources(&self, mut id: ObjectId) -> PdfResult<PdfDictionary> {
        for _ in 0..64 {
            let dict = self.get_dictionary(id)?;
            if let Some(res) = dict.get("Resources") {
                if let Ok(d) = self.resolve_dict_or_ref(res) {
                    return Ok(d);
                }
            }
            match dict.get("Parent") {
                Some(Object::Reference(parent)) => id = *parent,
                _ => break,
            }
        }
        Ok(PdfDictionary::new())
    }

    pub(super) fn resolve_dict_or_ref(&self, obj: &Object) -> PdfResult<PdfDictionary> {
        match obj {
            Object::Dictionary(d) => Ok(d.clone()),
            Object::Reference(id) => self.get_dictionary(*id),
            _ => Ok(PdfDictionary::new()),
        }
    }

    fn build_font_decoder(&self, dict: &PdfDictionary) -> FontDecoder {
        let tounicode = match dict.get("ToUnicode") {
            Some(Object::Reference(id)) => match self.get_object(*id) {
                Some(Object::Stream(s)) => s
                    .decode_all()
                    .ok()
                    .map(|bytes| parse_tounicode_cmap(&bytes)),
                _ => None,
            },
            _ => None,
        };

        let is_composite = matches!(dict.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Type0");
        if is_composite {
            FontDecoder::Composite { tounicode }
        } else {
            FontDecoder::Simple { tounicode }
        }
    }
}

fn append_decoded(out: &mut String, decoder: Option<&FontDecoder>, s: &PdfString) {
    let bytes = s.as_bytes();
    match decoder {
        Some(d) => out.push_str(&d.decode(bytes)),
        // No active font resolved (malformed content stream): fall back to
        // the WinAnsi table rather than dropping the text silently.
        None => out.push_str(&win_ansi_bytes_to_string(bytes)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[test]
    fn extracts_simple_font_text_via_winansi_fallback() {
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Hello"))
            .build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();

        let doc = EditableDocument::from_bytes(bytes).unwrap();
        let page_id = doc.page_id_at(0).unwrap();
        let text = doc.extract_page_text(page_id).unwrap();
        assert!(text.contains("Hello"), "extracted text was: {text:?}");
    }

    #[test]
    fn extracts_latin1_accented_text_via_winansi_fallback() {
        // A content stream with the raw WinAnsi-encoded byte 0xE9 ('é') in
        // a literal string, and no /ToUnicode CMap — exactly the case the
        // WinAnsi fallback table exists for. Built by hand (rather than via
        // `ContentBuilder::text`, whose `Operator::ShowText(String)` writes
        // each Rust `char` as UTF-8, not single-byte WinAnsi — a
        // pre-existing limitation of this crate's text-authoring helpers,
        // out of scope for the font-embedding task) so this test exercises
        // exactly the extraction-side decoding it's meant to cover.
        let mut data = Vec::new();
        data.extend_from_slice(b"%PDF-1.7\n");
        let obj1 = data.len();
        data.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let obj2 = data.len();
        data.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        let obj3 = data.len();
        data.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n",
        );
        let obj4 = data.len();
        let content = b"BT /F1 12 Tf 72 700 Td (Caf\xE9) Tj ET";
        data.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        data.extend_from_slice(content);
        data.extend_from_slice(b"\nendstream\nendobj\n");
        let obj5 = data.len();
        data.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
        );
        let xref_off = data.len();
        data.extend_from_slice(b"xref\n0 6\n");
        data.extend_from_slice(b"0000000000 65535 f \n");
        for off in [obj1, obj2, obj3, obj4, obj5] {
            data.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
        }
        data.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\n");
        data.extend_from_slice(format!("startxref\n{xref_off}\n%%EOF\n").as_bytes());

        let doc = EditableDocument::from_bytes(data).expect("hand-built PDF must parse");
        let page_id = doc.page_id_at(0).unwrap();
        let text = doc.extract_page_text(page_id).unwrap();
        assert!(text.contains('\u{E9}'), "extracted text was: {text:?}");
    }

    #[test]
    fn extracts_missing_font_gracefully() {
        // Content stream references a font ("/F9") absent from Resources:
        // must not panic, and should still fall back to decoding the
        // literal string bytes via the no-active-font WinAnsi fallback.
        let page = PageBuilder::a4()
            .content(ContentBuilder::new().raw("BT /F9 12 Tf (Hi) Tj ET"))
            .build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        let doc = EditableDocument::from_bytes(bytes).unwrap();
        let page_id = doc.page_id_at(0).unwrap();
        let text = doc.extract_page_text(page_id).unwrap();
        assert!(text.contains("Hi"), "extracted text was: {text:?}");
    }
}
