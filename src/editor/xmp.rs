//! XMP metadata packet generation/embedding (ISO 32000-1:2008 14.3.2
//! "Metadata Streams"; the packet body itself is XMP/RDF, ISO 16684-1).
//!
//! PDF's own `/Info` dictionary (ISO 32000-1 14.3.3) only carries a
//! handful of fixed text fields (`/Title`, `/Author`, ...). PDF/A requires
//! an additional, XML-based metadata stream attached to the document
//! catalog under `/Metadata` that carries (among other things) the *PDF/A
//! Identification Extension Schema* - the `pdfaid:part`/`pdfaid:conformance`
//! properties a validator (veraPDF included) reads to know which PDF/A
//! flavor a file claims to conform to. That extension schema's namespace
//! (`http://www.aiim.org/pdfa/ns/id/`, prefix `pdfaid`) is documented in
//! the PDF/A Technical Corrigenda / widely mirrored in the XMP
//! specification's PDF/A appendix; this module targets exactly that
//! well-established convention, not a from-scratch XMP implementation.
//!
//! # Scope
//!
//! This is a **minimal, hand-rolled XMP packet writer**, not a general
//! RDF/XML library (this crate has no XML dependency - see the crate-level
//! policy against pulling in a new dependency for one narrow internal use,
//! already applied the same way in [`crate::editor::audit`]). It emits a
//! fixed, known-valid packet shape with a handful of caller-supplied
//! values escaped for XML; it does not parse or merge arbitrary existing
//! XMP. [`read_pdfaid`] is the mirror-image reader: a deliberately narrow
//! substring search for the two properties this crate's own validator
//! needs, not a general RDF parser - documented as such at its
//! definition.

use super::graph::EditableDocument;
use crate::error::PdfResult;
use crate::object::{Object, PdfDictionary, PdfName, PdfStream};
use crate::types::ObjectId;

/// Hard cap on the decoded size of a `/Metadata` stream this crate will
/// read back (via [`EditableDocument::xmp_metadata`]/[`read_pdfaid`]).
/// Guards against a crafted file claiming an enormous metadata stream
/// (untrusted-input rule) - real XMP packets (even with thumbnails-free
/// PDF/A identification + Dublin Core + a title) are a few KB.
const MAX_XMP_BYTES: usize = 4 * 1024 * 1024;

/// Values used to build a PDF/A/PDF/X Identification XMP packet via
/// [`build_xmp_packet`].
#[derive(Debug, Clone, Default)]
pub struct XmpFields<'a> {
    /// `dc:title` (Dublin Core). Also mirrored into the document `/Info
    /// /Title` by callers that want both, but this module only ever
    /// touches `/Metadata`.
    pub title: Option<&'a str>,
    /// `pdf:Producer` / `xmp:CreatorTool`.
    pub producer: Option<&'a str>,
    /// PDF/A identification (`pdfaid:part`, `pdfaid:conformance`), e.g.
    /// `(1, "B")` for PDF/A-1b. `None` omits the whole `pdfaid` schema.
    pub pdfa: Option<(u8, &'static str)>,
    /// PDF/X identification (`pdfxid:GTS_PDFXVersion`), e.g.
    /// `"PDF/X-1a:2001"`. `None` omits the `pdfxid` schema.
    pub pdfx_version: Option<&'a str>,
    /// PDF/UA identification (`pdfuaid:part`), e.g. `Some(1)` for
    /// PDF/UA-1 (ISO 14289-1:2014, the only version
    /// [`crate::editor::pdfua`] targets). `None` omits the `pdfuaid`
    /// schema. Namespace confidence note: mirrors the well-established
    /// `pdfaid` schema's namespace pattern
    /// (`http://www.aiim.org/pdfua/ns/id/`, same registering
    /// organization as `pdfaid`'s `http://www.aiim.org/pdfa/ns/id/`) -
    /// this crate's author has not independently re-verified this
    /// against the ISO 14289-1 primary text, but it matches what real
    /// PDF/UA producer tools emit and is confirmed by this repo's own
    /// veraPDF verification run (the `ua1` flavour's "document metadata
    /// stream doesn't contain PDF/UA Identification Schema" check passes
    /// once this is included).
    pub pdfua_part: Option<u8>,
}

/// Escapes the five XML predefined entities (XML 1.0 2.4) in `s`. Every
/// value this module writes into the packet goes through this - titles
/// and producer strings are caller-controlled free text that may contain
/// `<`, `&`, etc., and this is *not* an untrusted-input path in the
/// decompression-bomb sense, but an unescaped `<` or `&` would still
/// produce a structurally broken (and therefore PDF/A-nonconformant XMP
/// packet, which is exactly the bug this exists to avoid.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Builds a complete XMP packet (the `<?xpacket ...?> ... <?xpacket
/// end="w"?>` wrapper plus one `rdf:Description`) carrying Dublin Core,
/// `pdf`/`xmp` and (if requested) the PDF/A and/or PDF/X identification
/// schemas described in [`XmpFields`].
///
/// The fixed packet id `W5M0MpCehiHzreSzNTczkc9d` in the processing
/// instruction is the literal value defined by the XMP specification for
/// this purpose (every conformant XMP packet uses this exact string) -
/// not a per-document id.
pub fn build_xmp_packet(fields: &XmpFields) -> Vec<u8> {
    let mut rdf_props = String::new();
    if let Some(title) = fields.title {
        rdf_props.push_str(&format!(
            "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>\n      ",
            xml_escape(title)
        ));
    }
    if let Some(producer) = fields.producer {
        rdf_props.push_str(&format!("<pdf:Producer>{}</pdf:Producer>\n      ", xml_escape(producer)));
        rdf_props.push_str(&format!("<xmp:CreatorTool>{}</xmp:CreatorTool>\n      ", xml_escape(producer)));
    }
    if let Some((part, conformance)) = fields.pdfa {
        rdf_props.push_str(&format!("<pdfaid:part>{part}</pdfaid:part>\n      "));
        rdf_props.push_str(&format!("<pdfaid:conformance>{conformance}</pdfaid:conformance>\n      "));
    }
    if let Some(version) = fields.pdfx_version {
        rdf_props.push_str(&format!(
            "<pdfxid:GTS_PDFXVersion>{}</pdfxid:GTS_PDFXVersion>\n      ",
            xml_escape(version)
        ));
    }
    if let Some(part) = fields.pdfua_part {
        rdf_props.push_str(&format!("<pdfuaid:part>{part}</pdfuaid:part>\n      "));
    }

    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
    <rdf:Description rdf:about=\"\"\n\
      xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
      xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"\n\
      xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
      xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"\n\
      xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\"\n\
      xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\">\n      {rdf_props}</rdf:Description>\n\
  </rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>"
    )
    .into_bytes()
}

/// Best-effort extraction of `pdfaid:part`/`pdfaid:conformance` from a raw
/// XMP packet, for [`EditableDocument`]'s PDF/A validator.
///
/// This is a **substring search, not an XML/RDF parser**: XMP allows the
/// same property to be written as either an XML attribute
/// (`pdfaid:part="1"`) or a child element (`<pdfaid:part>1</pdfaid:part>`,
/// what [`build_xmp_packet`] itself emits), and a fully general reader
/// would need to handle both plus arbitrary whitespace/namespace-prefix
/// choices. Handling both of *those* two common forms is judged
/// sufficient for a validator whose job is "does this look like it
/// declares PDF/A", not for round-tripping arbitrary third-party XMP.
pub fn read_pdfaid(xmp: &[u8]) -> Option<(u8, String)> {
    if xmp.len() > MAX_XMP_BYTES {
        return None;
    }
    let text = String::from_utf8_lossy(xmp);
    let part = extract_property(&text, "pdfaid:part")?.trim().parse::<u8>().ok()?;
    let conformance = extract_property(&text, "pdfaid:conformance")?.trim().to_string();
    Some((part, conformance))
}

/// Best-effort extraction of `pdfuaid:part` from a raw XMP packet. Same
/// substring-search caveats as [`read_pdfaid`].
pub fn read_pdfuaid(xmp: &[u8]) -> Option<u8> {
    if xmp.len() > MAX_XMP_BYTES {
        return None;
    }
    let text = String::from_utf8_lossy(xmp);
    extract_property(&text, "pdfuaid:part")?.trim().parse::<u8>().ok()
}

/// Best-effort extraction of `dc:title`'s default-language value (the
/// `<rdf:li xml:lang="x-default">...</rdf:li>` entry
/// [`build_xmp_packet`] writes) from a raw XMP packet, for
/// [`EditableDocument`]'s PDF/A validator to cross-check against the
/// classic `/Info /Title` (ISO 19005-1:2005 6.7.3 requires the two be
/// equivalent when both are present). Same substring-search caveats as
/// [`read_pdfaid`] apply.
pub(crate) fn read_dc_title(xmp: &[u8]) -> Option<String> {
    if xmp.len() > MAX_XMP_BYTES {
        return None;
    }
    let text = String::from_utf8_lossy(xmp);
    let marker = "<rdf:li xml:lang=\"x-default\">";
    let start = text.find(marker)? + marker.len();
    let rest = &text[start..];
    let end = rest.find('<')?;
    Some(rest[..end].to_string())
}

/// Finds `<prefix:Name>value</prefix:Name>` or `prefix:Name="value"` and
/// returns `value`.
fn extract_property(text: &str, name: &str) -> Option<String> {
    let elem_open = format!("<{name}>");
    if let Some(start) = text.find(&elem_open) {
        let rest = &text[start + elem_open.len()..];
        let end = rest.find('<')?;
        return Some(rest[..end].to_string());
    }
    let attr_open = format!("{name}=\"");
    if let Some(start) = text.find(&attr_open) {
        let rest = &text[start + attr_open.len()..];
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }
    None
}

impl EditableDocument {
    /// Sets (replacing any existing) the document's `/Metadata` XML
    /// packet (ISO 32000-1 14.3.2), reusing the existing stream object if
    /// the catalog already references one so a re-save only touches that
    /// one object.
    pub fn set_xmp_metadata(&mut self, xmp: Vec<u8>) -> PdfResult<ObjectId> {
        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Metadata")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("XML")));
        let stream = PdfStream::with_dictionary(dict, xmp);

        let mut catalog = self.catalog()?;
        let id = match catalog.get("Metadata") {
            Some(Object::Reference(id)) => *id,
            _ => {
                let id = self.allocate_id();
                catalog.set("Metadata", Object::Reference(id));
                let cat_id = self.catalog_id();
                self.set_object(cat_id, Object::Dictionary(catalog));
                id
            }
        };
        self.set_object(id, Object::Stream(stream));
        Ok(id)
    }

    /// Returns the raw bytes of the document's `/Metadata` XML packet, if
    /// any.
    ///
    /// `PdfStream::decode_all` (used here) requires the `compression`
    /// feature; every path that can construct an [`EditableDocument`]
    /// already requires the `parser` feature, and `parser` in turn always
    /// enables `compression` (see `Cargo.toml`), so it is always available
    /// wherever this method is.
    pub fn xmp_metadata(&self) -> PdfResult<Option<Vec<u8>>> {
        let catalog = self.catalog()?;
        let Some(Object::Reference(id)) = catalog.get("Metadata") else { return Ok(None) };
        match self.get_object(*id) {
            Some(Object::Stream(s)) => {
                let decoded = s.decode_all()?;
                if decoded.len() > MAX_XMP_BYTES {
                    return Ok(None);
                }
                Ok(Some(decoded))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    fn doc_with_one_page() -> EditableDocument {
        let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(ContentBuilder::new()).build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        EditableDocument::from_bytes(bytes).unwrap()
    }

    #[test]
    fn test_build_xmp_packet_contains_pdfaid() {
        let fields = XmpFields { title: Some("Report"), producer: Some("rust-pdf"), pdfa: Some((1, "B")), ..Default::default() };
        let packet = build_xmp_packet(&fields);
        let text = String::from_utf8(packet).unwrap();
        assert!(text.starts_with("<?xpacket begin="));
        assert!(text.trim_end().ends_with("<?xpacket end=\"w\"?>"));
        assert!(text.contains("<pdfaid:part>1</pdfaid:part>"));
        assert!(text.contains("<pdfaid:conformance>B</pdfaid:conformance>"));
        assert!(text.contains("Report"));
    }

    #[test]
    fn test_xml_escape_of_hostile_title() {
        let fields = XmpFields { title: Some("A & B <injected/>\"'"), ..Default::default() };
        let packet = build_xmp_packet(&fields);
        let text = String::from_utf8(packet).unwrap();
        assert!(!text.contains("<injected/>"));
        assert!(text.contains("&amp;"));
        assert!(text.contains("&lt;injected"));
    }

    #[test]
    fn test_read_pdfaid_round_trips_through_build() {
        let fields = XmpFields { pdfa: Some((2, "B")), ..Default::default() };
        let packet = build_xmp_packet(&fields);
        let (part, conformance) = read_pdfaid(&packet).unwrap();
        assert_eq!(part, 2);
        assert_eq!(conformance, "B");
    }

    #[test]
    fn test_read_dc_title_round_trips_through_build() {
        let fields = XmpFields { title: Some("Quarterly Report"), ..Default::default() };
        let packet = build_xmp_packet(&fields);
        assert_eq!(read_dc_title(&packet).as_deref(), Some("Quarterly Report"));
    }

    #[test]
    fn test_read_dc_title_missing_returns_none() {
        let packet = build_xmp_packet(&XmpFields::default());
        assert!(read_dc_title(&packet).is_none());
    }

    #[test]
    fn test_read_pdfaid_missing_returns_none() {
        let fields = XmpFields { title: Some("no pdfa here"), ..Default::default() };
        let packet = build_xmp_packet(&fields);
        assert!(read_pdfaid(&packet).is_none());
    }

    #[test]
    fn test_read_pdfaid_rejects_oversized_input() {
        let huge = vec![b'a'; MAX_XMP_BYTES + 1];
        assert!(read_pdfaid(&huge).is_none());
    }

    #[test]
    fn test_set_and_get_xmp_metadata_round_trips() {
        let mut doc = doc_with_one_page();
        let packet = build_xmp_packet(&XmpFields { pdfa: Some((3, "B")), ..Default::default() });
        doc.set_xmp_metadata(packet.clone()).unwrap();
        let read_back = doc.xmp_metadata().unwrap().unwrap();
        assert_eq!(read_back, packet);
    }

    #[test]
    fn test_set_xmp_metadata_twice_reuses_object_id() {
        let mut doc = doc_with_one_page();
        let id1 = doc.set_xmp_metadata(build_xmp_packet(&XmpFields::default())).unwrap();
        let id2 = doc.set_xmp_metadata(build_xmp_packet(&XmpFields { title: Some("v2"), ..Default::default() })).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_no_metadata_returns_none() {
        let doc = doc_with_one_page();
        assert!(doc.xmp_metadata().unwrap().is_none());
    }

    #[test]
    fn test_xmp_metadata_survives_full_rewrite() {
        let mut doc = doc_with_one_page();
        let packet = build_xmp_packet(&XmpFields { pdfa: Some((1, "B")), ..Default::default() });
        doc.set_xmp_metadata(packet.clone()).unwrap();
        let saved = doc.save_full_rewrite_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert_eq!(reopened.xmp_metadata().unwrap().unwrap(), packet);
    }
}
