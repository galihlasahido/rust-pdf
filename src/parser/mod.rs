//! PDF parsing module.
//!
//! This module provides functionality for reading and parsing existing PDF documents.
//!
//! # Example
//!
//! ```ignore
//! use rust_pdf::parser::PdfReader;
//!
//! let reader = PdfReader::from_file("document.pdf")?;
//! println!("Page count: {}", reader.page_count());
//! ```

mod inline_image;
mod lexer;
mod objects;
mod recovery;
mod trailer;
mod xref;

pub use inline_image::{parse_inline_image, InlineImage};
pub use trailer::Trailer;
pub use xref::{XrefEntry, XrefTable};

// Re-exported for `crate::editor`: incremental save (ISO 32000-1 7.5.6)
// needs to chain a new update's `/Prev` to the byte offset of the base
// file's own final `startxref`, and this is the already-tested routine
// that locates it. Not part of the public API.
pub(crate) use xref::find_startxref;

use crate::document::PdfVersion;
use crate::error::{ParserError, PdfResult};
use crate::object::{Object, PdfDictionary};
use crate::types::ObjectId;
use objects::parse_indirect_object;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use trailer::parse_trailer;
use xref::parse_xref_table;

/// Hard cap on the number of `/Prev`/`/XRefStm`-linked cross-reference
/// sections that will be followed. Real files rarely have more than a
/// handful of incremental updates; this bounds work (and, combined with
/// the visited-offset check, guarantees termination) even on a file whose
/// `/Prev` chain is corrupt or maliciously long.
const MAX_XREF_SECTIONS: usize = 4096;

/// Extracts a hybrid-reference file's `/XRefStm` offset (ISO 32000-1
/// 7.5.8.4) from a trailer dictionary, if present.
fn xref_stm_offset(dict: &PdfDictionary) -> Option<u64> {
    match dict.get("XRefStm") {
        Some(Object::Integer(n)) if *n >= 0 => Some(*n as u64),
        _ => None,
    }
}

/// A PDF document reader.
///
/// Provides read-only access to PDF document structure and content.
#[derive(Debug)]
pub struct PdfReader {
    /// Raw PDF data.
    data: Vec<u8>,
    /// PDF version.
    version: PdfVersion,
    /// Cross-reference table.
    xref: XrefTable,
    /// Trailer information.
    trailer: Trailer,
    /// Object cache.
    object_cache: HashMap<ObjectId, Object>,
}

impl PdfReader {
    /// Opens a PDF file for reading.
    pub fn from_file(path: impl AsRef<Path>) -> PdfResult<Self> {
        let data = fs::read(path)?;
        Self::from_bytes(data)
    }

    /// Opens a PDF from bytes.
    ///
    /// If the cross-reference table cannot be located or parsed (a
    /// corrupt/truncated file, or one with a broken `/Prev` chain), this
    /// falls back to recovery mode: a linear scan of the file for `obj`
    /// headers (see the crate-internal `recovery` module for details).
    /// This mirrors the behaviour of production PDF readers, which never
    /// simply give up on a broken xref table when the objects themselves
    /// are still present in the file.
    pub fn from_bytes(data: Vec<u8>) -> PdfResult<Self> {
        // Parse header
        let version = Self::parse_header(&data)?;

        let parsed = find_startxref(&data)
            .ok()
            .and_then(|offset| Self::parse_xref_and_trailer(&data, offset).ok());

        let (xref, trailer) = match parsed {
            Some(result) => result,
            None => recovery::recover(&data).ok_or(ParserError::InvalidTrailer)?,
        };

        // Check for encryption
        if trailer.encrypt.is_some() {
            return Err(ParserError::EncryptedPdf.into());
        }

        Ok(Self {
            data,
            version,
            xref,
            trailer,
            object_cache: HashMap::new(),
        })
    }

    /// Like [`PdfReader::from_bytes`], but always uses recovery-mode
    /// reconstruction (ignoring any cross-reference table present),
    /// regardless of whether the file's own xref table would parse. This
    /// is useful for diagnosing/repairing files whose xref table parses
    /// "successfully" but points at the wrong offsets.
    pub fn from_bytes_recovery_only(data: Vec<u8>) -> PdfResult<Self> {
        let version = Self::parse_header(&data)?;
        let (xref, trailer) = recovery::recover(&data).ok_or(ParserError::InvalidTrailer)?;

        if trailer.encrypt.is_some() {
            return Err(ParserError::EncryptedPdf.into());
        }

        Ok(Self {
            data,
            version,
            xref,
            trailer,
            object_cache: HashMap::new(),
        })
    }

    /// Parses the PDF header to get the version.
    fn parse_header(data: &[u8]) -> Result<PdfVersion, ParserError> {
        if data.len() < 8 {
            return Err(ParserError::InvalidHeader);
        }

        // Header format: %PDF-X.Y
        if !data.starts_with(b"%PDF-") {
            return Err(ParserError::InvalidHeader);
        }

        let version_str = std::str::from_utf8(&data[5..8])
            .map_err(|_| ParserError::InvalidHeader)?;

        PdfVersion::try_from(version_str)
            .map_err(|_| ParserError::InvalidHeader)
    }

    /// Parses the xref table and trailer, following the chain of
    /// incremental updates via `/Prev` (ISO 32000-1 7.5.4) and, for
    /// hybrid-reference files, the supplementary cross-reference stream
    /// named by `/XRefStm` (7.5.8.4).
    ///
    /// Entries from more recent revisions always take precedence over
    /// older ones (each section is merged with "first write wins", and
    /// sections are visited newest-first).
    fn parse_xref_and_trailer(
        data: &[u8],
        start_offset: u64,
    ) -> Result<(XrefTable, Trailer), ParserError> {
        let mut combined_xref = XrefTable::new();
        let mut final_trailer: Option<Trailer> = None;
        let mut visited: HashSet<u64> = HashSet::new();
        let mut xref_offset = start_offset;
        let mut sections = 0usize;

        loop {
            sections += 1;
            if sections > MAX_XREF_SECTIONS {
                break;
            }
            // Guard against a `/Prev` cycle (self-referential or looping
            // chain), which would otherwise never terminate.
            if !visited.insert(xref_offset) {
                break;
            }

            let Some(xref_data) = data.get(xref_offset as usize..) else {
                // Offset points outside the file. If we already have a
                // trailer from a newer revision, stop here and use what we
                // have rather than failing the whole document.
                break;
            };

            let section = Self::parse_one_xref_section(xref_data);
            let Ok((section_xref, section_trailer)) = section else {
                break;
            };

            combined_xref.merge(section_xref);

            let is_first = final_trailer.is_none();
            let prev = section_trailer.prev;
            let xref_stm = xref_stm_offset(&section_trailer.dict);

            if is_first {
                final_trailer = Some(section_trailer);
            }

            // Hybrid-reference file: the traditional table's trailer may
            // point at a supplementary xref *stream* covering the same
            // revision (needed so pre-1.5 readers can still find the
            // basic entries while 1.5+ readers also get entries for
            // objects stored in object streams). Its entries are lower
            // priority than the traditional section we just merged, but
            // higher priority than anything from `/Prev`.
            if let Some(stm_offset) = xref_stm {
                if let Some(stm_data) = data.get(stm_offset as usize..) {
                    if let Ok((_, (_, _, Object::Stream(stream)))) = parse_indirect_object(stm_data)
                    {
                        if let Ok(stm_xref) = Self::parse_xref_stream(&stream) {
                            combined_xref.merge(stm_xref);
                        }
                    }
                }
            }

            match prev {
                Some(prev_offset) => xref_offset = prev_offset,
                None => break,
            }
        }

        let trailer = final_trailer.ok_or(ParserError::InvalidTrailer)?;
        Ok((combined_xref, trailer))
    }

    /// Parses a single xref section (traditional table+trailer, or xref
    /// stream) starting at `xref_data` (which is `data[offset..]`).
    fn parse_one_xref_section(xref_data: &[u8]) -> Result<(XrefTable, Trailer), ParserError> {
        if xref_data.starts_with(b"xref") {
            let (remaining, xref) =
                parse_xref_table(xref_data).map_err(|_| ParserError::InvalidXref)?;
            let (_, trailer_dict) =
                parse_trailer(remaining).map_err(|_| ParserError::InvalidTrailer)?;
            let trailer = Trailer::from_dictionary(trailer_dict)?;
            Ok((xref, trailer))
        } else {
            let (_, (_, _, obj)) =
                parse_indirect_object(xref_data).map_err(|_| ParserError::InvalidXrefStream)?;
            match obj {
                Object::Stream(stream) => {
                    let xref = Self::parse_xref_stream(&stream)?;
                    let trailer = Trailer::from_dictionary(stream.dictionary.clone())?;
                    Ok((xref, trailer))
                }
                _ => Err(ParserError::InvalidXrefStream),
            }
        }
    }

    /// Parses a cross-reference stream (ISO 32000-1:2008 Section 7.5.8).
    fn parse_xref_stream(stream: &crate::object::PdfStream) -> Result<XrefTable, ParserError> {
        let dict = &stream.dictionary;

        // Get W array (field widths).
        let w = match dict.get("W") {
            Some(Object::Array(arr)) => arr,
            _ => return Err(ParserError::InvalidXrefStream),
        };
        if w.len() != 3 {
            return Err(ParserError::InvalidXrefStream);
        }

        // Field widths are byte counts; the spec doesn't bound them, but
        // `read_int` folds bytes into a u64 (8 bytes), and no legitimate
        // xref stream needs wider fields than that. Reject anything
        // larger rather than silently truncating high-order bytes.
        let widths: Vec<usize> = w
            .iter()
            .map(|o| match o {
                Object::Integer(n) if *n >= 0 && *n <= 8 => Ok(*n as usize),
                _ => Err(ParserError::InvalidXrefStream),
            })
            .collect::<Result<_, _>>()?;
        let (w1, w2, w3) = (widths[0], widths[1], widths[2]);
        let entry_size = w1 + w2 + w3;
        if entry_size == 0 {
            // W = [0 0 0] would otherwise make every entry zero-width,
            // turning the loop below into an unbounded insert loop.
            return Err(ParserError::InvalidXrefStream);
        }

        // Get Index array (optional, defaults to [0 Size]).
        let size = match dict.get("Size") {
            Some(Object::Integer(n)) if *n >= 0 => *n as u64,
            _ => return Err(ParserError::InvalidXrefStream),
        };

        let index: Vec<(u64, u64)> = match dict.get("Index") {
            Some(Object::Array(arr)) => {
                let mut pairs = Vec::new();
                let mut iter = arr.iter();
                while let (Some(start), Some(count)) = (iter.next(), iter.next()) {
                    match (start, count) {
                        (Object::Integer(s), Object::Integer(c)) if *s >= 0 && *c >= 0 => {
                            pairs.push((*s as u64, *c as u64));
                        }
                        _ => return Err(ParserError::InvalidXrefStream),
                    }
                }
                pairs
            }
            _ => vec![(0, size)],
        };

        // Decode stream data, honouring the full filter chain and any
        // PNG/TIFF predictor (most real-world xref streams use
        // FlateDecode with Predictor 12).
        #[cfg(feature = "compression")]
        let data = stream.decode_all()?;
        #[cfg(not(feature = "compression"))]
        let data = stream.data().to_vec();

        // Parse entries. The `data_offset + entry_size > data.len()` bound
        // check below already guarantees termination in a number of steps
        // proportional to `data.len() / entry_size` (a real byte count),
        // regardless of how large a malicious `count` claims to be.
        let mut table = XrefTable::new();
        let mut data_offset = 0usize;

        for (start, count) in index {
            let mut obj_num = start;
            for _ in 0..count {
                if data_offset + entry_size > data.len() {
                    return Err(ParserError::InvalidXrefStream);
                }
                if obj_num > u32::MAX as u64 {
                    return Err(ParserError::InvalidXrefStream);
                }

                let entry_type = if w1 == 0 {
                    1 // Default type is 1 (in use), per 7.5.8.2 Table 17.
                } else {
                    Self::read_int(&data[data_offset..data_offset + w1])
                };
                let field2 = Self::read_int(&data[data_offset + w1..data_offset + w1 + w2]);
                let field3 =
                    Self::read_int(&data[data_offset + w1 + w2..data_offset + entry_size]);

                let entry = match entry_type {
                    0 => XrefEntry::Free {
                        next_free: field2 as u32,
                        generation: field3 as u16,
                    },
                    1 => XrefEntry::InUse {
                        offset: field2,
                        generation: field3 as u16,
                    },
                    2 => XrefEntry::Compressed {
                        object_stream: field2 as u32,
                        index: field3 as u32,
                    },
                    _ => return Err(ParserError::InvalidXrefStream),
                };

                table.insert(obj_num as u32, entry);
                data_offset += entry_size;
                obj_num = obj_num.saturating_add(1);
            }
        }

        Ok(table)
    }

    /// Reads an integer from bytes (big-endian).
    fn read_int(bytes: &[u8]) -> u64 {
        let mut result = 0u64;
        for &b in bytes {
            result = (result << 8) | (b as u64);
        }
        result
    }

    /// Returns the PDF version.
    pub fn version(&self) -> PdfVersion {
        self.version
    }

    /// Returns the number of pages in the document.
    pub fn page_count(&self) -> usize {
        self.get_page_count_from_tree().unwrap_or(0)
    }

    /// Gets the page count from the page tree.
    fn get_page_count_from_tree(&self) -> Option<usize> {
        let root = self.resolve_reference(self.trailer.root)?;

        let pages_ref = match root {
            Object::Dictionary(dict) => match dict.get("Pages") {
                Some(Object::Reference(id)) => *id,
                _ => return None,
            },
            _ => return None,
        };

        let pages = self.resolve_reference(pages_ref)?;

        match pages {
            Object::Dictionary(dict) => match dict.get("Count") {
                Some(Object::Integer(count)) => Some(*count as usize),
                _ => None,
            },
            _ => None,
        }
    }

    /// Returns the catalog (root) dictionary.
    pub fn catalog(&self) -> Option<PdfDictionary> {
        let obj = self.resolve_reference(self.trailer.root)?;
        match obj {
            Object::Dictionary(dict) => Some(dict),
            _ => None,
        }
    }

    /// Returns the document info dictionary if present.
    pub fn info(&self) -> Option<PdfDictionary> {
        let info_id = self.trailer.info?;
        let obj = self.resolve_reference(info_id)?;
        match obj {
            Object::Dictionary(dict) => Some(dict),
            _ => None,
        }
    }

    /// Gets an object by its ID.
    pub fn get_object(&self, id: ObjectId) -> Option<&Object> {
        // First check cache
        if let Some(obj) = self.object_cache.get(&id) {
            return Some(obj);
        }

        // Object not in cache - we'd need to parse it
        // For now, return None (cache is only populated during resolve_reference)
        None
    }

    /// Resolves an object reference, returning the referenced object.
    ///
    /// Never panics on a malformed/adversarial xref table: every byte
    /// offset taken from the (untrusted) xref data is validated against
    /// the actual file length before use.
    pub fn resolve_reference(&self, id: ObjectId) -> Option<Object> {
        // Check cache first
        if let Some(obj) = self.object_cache.get(&id) {
            return Some(obj.clone());
        }

        // Get xref entry
        let entry = self.xref.get(id.number)?;

        match entry {
            XrefEntry::InUse { offset, .. } => {
                // Parse object at offset. `offset` comes straight from the
                // (untrusted) xref table/stream, so it must be checked
                // against the file length rather than indexed directly.
                let data = self.data.get(*offset as usize..)?;
                let (_, (_, _, obj)) = parse_indirect_object(data).ok()?;
                Some(obj)
            }
            XrefEntry::Compressed {
                object_stream,
                index,
            } => self.resolve_compressed_object(*object_stream, *index),
            XrefEntry::Free { .. } => None,
        }
    }

    /// Resolves an object from a compressed object stream (ISO 32000-1
    /// Section 7.5.7). All indices/offsets taken from the object stream's
    /// header (itself untrusted data from the file) are bounds-checked
    /// before use; this function returns `None` rather than panicking on
    /// any malformed value.
    fn resolve_compressed_object(&self, stream_num: u32, index: u32) -> Option<Object> {
        // Get the object stream
        let stream_entry = self.xref.get(stream_num)?;
        let offset = stream_entry.offset()?;

        let data = self.data.get(offset as usize..)?;
        let (_, (_, _, stream_obj)) = parse_indirect_object(data).ok()?;

        let stream = match stream_obj {
            Object::Stream(s) => s,
            _ => return None,
        };

        // Get N (number of objects) and First (offset to first object)
        let dict = &stream.dictionary;
        let num_objects = match dict.get("N") {
            Some(Object::Integer(n)) if *n >= 0 => *n as usize,
            _ => return None,
        };

        // Decode stream (full filter chain, with predictor support).
        #[cfg(feature = "compression")]
        let stream_data = stream.decode_all().ok()?;
        #[cfg(not(feature = "compression"))]
        let stream_data = stream.data().to_vec();

        let first = match dict.get("First") {
            Some(Object::Integer(f)) if *f >= 0 && (*f as usize) <= stream_data.len() => {
                *f as usize
            }
            _ => return None,
        };

        // Parse the header (N pairs of obj_num, offset).
        let header = stream_data.get(..first)?;
        let objects_data = stream_data.get(first..)?;

        let header_str = std::str::from_utf8(header).ok()?;
        let nums: Vec<i64> = header_str
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();

        let index = index as usize;
        if nums.len() < (index + 1) * 2 {
            return None;
        }

        let obj_offset = *nums.get(index * 2 + 1)?;
        if obj_offset < 0 {
            return None;
        }
        let obj_offset = obj_offset as usize;

        // Find end offset (next object's offset or end of data). Guard
        // against a corrupt/adversarial header claiming an out-of-range
        // `next_offset` too.
        let next_offset = if index + 1 < num_objects && nums.len() >= (index + 2) * 2 {
            let n = *nums.get((index + 1) * 2 + 1)?;
            if n < 0 {
                return None;
            }
            (n as usize).min(objects_data.len())
        } else {
            objects_data.len()
        };

        if obj_offset > next_offset {
            return None;
        }

        // Parse the object
        let obj_data = objects_data.get(obj_offset..next_offset)?;
        let (_, obj) = objects::parse_object(obj_data).ok()?;

        Some(obj)
    }

    /// Returns the raw PDF data.
    pub fn raw_data(&self) -> &[u8] {
        &self.data
    }

    /// Returns the xref table.
    pub fn xref(&self) -> &XrefTable {
        &self.xref
    }

    /// Returns the trailer.
    pub fn trailer(&self) -> &Trailer {
        &self.trailer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::PdfName;

    fn create_simple_pdf() -> Vec<u8> {
        use crate::prelude::*;

        let page = PageBuilder::a4().build();
        let doc = DocumentBuilder::new()
            .title("Test Document")
            .page(page)
            .build()
            .unwrap();

        doc.save_to_bytes().unwrap()
    }

    #[test]
    fn test_parse_simple_pdf() {
        let pdf_bytes = create_simple_pdf();
        let reader = PdfReader::from_bytes(pdf_bytes).unwrap();

        assert_eq!(reader.version(), PdfVersion::V1_7);
        assert_eq!(reader.page_count(), 1);
    }

    #[test]
    fn test_parse_header() {
        let data = b"%PDF-1.7\nrest of document";
        let version = PdfReader::parse_header(data).unwrap();
        assert_eq!(version, PdfVersion::V1_7);
    }

    #[test]
    fn test_parse_header_v2() {
        let data = b"%PDF-2.0\nrest of document";
        let version = PdfReader::parse_header(data).unwrap();
        assert_eq!(version, PdfVersion::V2_0);
    }

    #[test]
    fn test_invalid_header() {
        let data = b"not a pdf";
        let result = PdfReader::parse_header(data);
        assert!(result.is_err());
    }

    /// Regression test for a `cargo fuzz` finding (`parse_pdf` target):
    /// a corrupt file with no valid xref falls into recovery mode, which
    /// scans for `obj`/`endobj` and re-parses each candidate object -
    /// including literal strings containing an octal escape greater than
    /// `\377` (e.g. `\553`), which must wrap per ISO 32000-1 7.3.4.2
    /// rather than panic on integer overflow.
    #[test]
    fn test_from_bytes_does_not_panic_on_octal_escape_overflow_fuzz_finding() {
        let data: Vec<u8> = vec![
            37, 80, 68, 70, 45, 49, 46, 55, 32, 48, 32, 111, 98, 106, 10, 60, 60, 32, 47, 84, 80,
            54, 40, 92, 53, 53, 51, 32, 10,
        ];
        // Must not panic; whether it successfully opens or returns an
        // error is not the point of this regression test.
        let _ = PdfReader::from_bytes(data);
    }

    /// Minimal hand-rolled builder for constructing byte-exact PDF fixtures
    /// (multi-revision xref chains, hybrid-reference files, object
    /// streams, ...) that exercise structures the high-level
    /// `DocumentBuilder` doesn't produce.
    struct RawPdfBuilder {
        buf: Vec<u8>,
    }

    impl RawPdfBuilder {
        fn new() -> Self {
            let mut buf = Vec::new();
            buf.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
            Self { buf }
        }

        fn offset(&self) -> u64 {
            self.buf.len() as u64
        }

        /// Appends a simple (non-stream) indirect object and returns its
        /// byte offset.
        fn object(&mut self, num: u32, body: &str) -> u64 {
            let off = self.offset();
            self.buf
                .extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", num, body).as_bytes());
            off
        }

        /// Appends a stream object (with the given extra dictionary
        /// entries, e.g. `"/Type /ObjStm /N 2 /First 8"`) and returns its
        /// byte offset.
        fn stream_object(&mut self, num: u32, extra_dict: &str, data: &[u8]) -> u64 {
            let off = self.offset();
            self.buf.extend_from_slice(
                format!("{} 0 obj\n<< /Length {} {} >>\nstream\n", num, data.len(), extra_dict)
                    .as_bytes(),
            );
            self.buf.extend_from_slice(data);
            self.buf.extend_from_slice(b"\nendstream\nendobj\n");
            off
        }

        /// Appends a traditional `xref` table (a single `0 <size>`
        /// subsection covering object numbers `0..size`, using `Free` for
        /// any gaps) plus a `trailer` dictionary, `startxref`, and
        /// `%%EOF`. Returns the byte offset of the `xref` keyword (what
        /// `startxref` in a subsequent revision, or the file's own
        /// trailing `startxref`, should point at).
        fn xref_and_trailer(
            &mut self,
            entries: &[(u32, u64)],
            size: u32,
            trailer_extra: &str,
        ) -> u64 {
            let xref_off = self.offset();
            let mut table = std::collections::HashMap::new();
            for &(num, off) in entries {
                table.insert(num, off);
            }

            self.buf.extend_from_slice(format!("xref\n0 {}\n", size).as_bytes());
            for n in 0..size {
                if n == 0 {
                    self.buf.extend_from_slice(b"0000000000 65535 f \n");
                } else if let Some(&off) = table.get(&n) {
                    self.buf
                        .extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
                } else {
                    self.buf.extend_from_slice(b"0000000000 00000 f \n");
                }
            }

            self.buf.extend_from_slice(
                format!("trailer\n<< /Size {} {} >>\n", size, trailer_extra).as_bytes(),
            );
            self.buf
                .extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_off).as_bytes());

            xref_off
        }

        fn finish(self) -> Vec<u8> {
            self.buf
        }
    }

    #[test]
    fn test_incremental_update_prev_chain() {
        let mut b = RawPdfBuilder::new();

        // Revision 1: object 1 is a Catalog whose /Foo is "old".
        let obj1_off = b.object(1, "<< /Type /Catalog /Foo (old) >>");
        let rev1_xref = b.xref_and_trailer(&[(1, obj1_off)], 2, "/Root 1 0 R");

        // Revision 2 (incremental update): object 1 is redefined with
        // /Foo = "new"; the new xref section only lists the changed
        // object and chains back via /Prev to revision 1's xref.
        let obj1_new_off = b.object(1, "<< /Type /Catalog /Foo (new) >>");
        b.xref_and_trailer(
            &[(1, obj1_new_off)],
            2,
            &format!("/Root 1 0 R /Prev {}", rev1_xref),
        );

        let reader = PdfReader::from_bytes(b.finish()).unwrap();
        let catalog = reader.catalog().unwrap();
        match catalog.get("Foo") {
            Some(Object::String(s)) => assert_eq!(s.as_bytes(), b"new"),
            other => panic!("expected the most recent revision's value, got {:?}", other),
        }
    }

    #[test]
    fn test_xref_prev_cycle_does_not_hang() {
        let mut b = RawPdfBuilder::new();
        let obj1_off = b.object(1, "<< /Type /Catalog >>");

        // First write a placeholder xref/trailer so we know its offset,
        // then rewrite trailer_extra to point /Prev at itself once we
        // know that offset (a malicious/corrupt self-referential chain).
        let xref_off = b.offset();
        b.buf.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n");
        b.buf
            .extend_from_slice(format!("{:010} 00000 n \n", obj1_off).as_bytes());
        b.buf.extend_from_slice(
            format!("trailer\n<< /Size 2 /Root 1 0 R /Prev {} >>\n", xref_off).as_bytes(),
        );
        b.buf
            .extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_off).as_bytes());

        // Must terminate (not hang) and still recover a usable trailer.
        let result = PdfReader::from_bytes(b.finish());
        assert!(result.is_ok());
    }

    #[test]
    fn test_hybrid_reference_file_xrefstm() {
        let mut b = RawPdfBuilder::new();

        let obj1_off = b.object(1, "<< /Type /Catalog >>");
        let obj2_off = b.object(2, "<< /Type /Pages /Kids [] /Count 0 >>");

        // A minimal uncompressed xref stream (W = [1 4 1], one entry for
        // object 2) supplementing the traditional table below via
        // /XRefStm, per ISO 32000-1 7.5.8.4.
        let mut xref_stream_data = Vec::new();
        xref_stream_data.push(1u8); // type = in use
        xref_stream_data.extend_from_slice(&(obj2_off as u32).to_be_bytes());
        xref_stream_data.push(0); // generation
        let xrefstm_off = b.stream_object(
            3,
            "/Type /XRef /W [1 4 1] /Index [2 1] /Size 4",
            &xref_stream_data,
        );

        // Traditional table only lists object 1; object 2 is only
        // reachable via the hybrid /XRefStm.
        b.xref_and_trailer(
            &[(1, obj1_off)],
            4,
            &format!("/Root 1 0 R /XRefStm {}", xrefstm_off),
        );

        let reader = PdfReader::from_bytes(b.finish()).unwrap();
        assert!(reader.xref().get(2).is_some());
        assert_eq!(reader.page_count(), 0);
    }

    #[test]
    fn test_object_stream_resolution_via_xref_stream() {
        let mut b = RawPdfBuilder::new();

        let obj1_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");

        // Object stream (object 3) containing object 2 (an empty Pages
        // dict) at relative offset 0. Object 2 has *no* standalone
        // indirect definition anywhere else in the file - it is only
        // reachable via the object stream, exactly like real-world
        // producers that compress most objects into ObjStms.
        let objstm_body = b"<< /Type /Pages /Kids [] /Count 0 >>";
        let header = b"2 0"; // "obj_num offset" pairs, offset relative to /First
        let mut full = header.to_vec();
        full.push(b' ');
        full.extend_from_slice(objstm_body);
        let first = header.len() + 1;
        let objstm_off = b.stream_object(3, &format!("/Type /ObjStm /N 1 /First {}", first), &full);

        // Cross-reference stream (object 4, but it does not need to
        // describe itself: parse_xref_and_trailer locates it directly via
        // startxref, not through the table). W = [1 4 2].
        let (w1, w2, w3) = (1usize, 4usize, 2usize);
        let mut entries = Vec::new();
        let mut push_entry = |t: u8, f2: u32, f3: u16| {
            entries.push(t);
            entries.extend_from_slice(&f2.to_be_bytes());
            entries.extend_from_slice(&f3.to_be_bytes());
        };
        push_entry(0, 0, 65535); // object 0: free (required head entry)
        push_entry(1, obj1_off as u32, 0); // object 1: in use (Catalog)
        push_entry(2, 3, 0); // object 2: compressed in stream 3, index 0
        push_entry(1, objstm_off as u32, 0); // object 3: in use (the ObjStm)
        debug_assert_eq!(entries.len(), 4 * (w1 + w2 + w3));

        b.stream_object(
            4,
            &format!(
                "/Type /XRef /W [{} {} {}] /Index [0 4] /Size 4 /Root 1 0 R",
                w1, w2, w3
            ),
            &entries,
        );
        let xref_stream_off = {
            // The stream we just wrote starts right before its own
            // "4 0 obj" header; recompute that start offset.
            let marker = b"4 0 obj";
            let hay = &b.buf[..];
            hay.windows(marker.len())
                .rposition(|w| w == marker)
                .expect("xref stream object header must be present") as u64
        };
        b.buf
            .extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_stream_off).as_bytes());

        let reader = PdfReader::from_bytes(b.finish()).unwrap();
        assert_eq!(reader.page_count(), 0);
        let pages = reader.catalog().unwrap();
        match pages.get("Pages") {
            Some(Object::Reference(id)) => {
                let resolved = reader.resolve_reference(*id).unwrap();
                match resolved {
                    Object::Dictionary(d) => {
                        assert_eq!(d.get("Type"), Some(&Object::Name(PdfName::new_unchecked("Pages"))));
                    }
                    _ => panic!("expected Pages dictionary resolved from object stream"),
                }
            }
            other => panic!("expected /Pages reference, got {:?}", other),
        }
    }

    #[test]
    fn test_recovery_mode_when_xref_is_corrupted() {
        let mut b = RawPdfBuilder::new();
        let obj1_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
        let obj2_off = b.object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
        let obj3_off = b.object(
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>",
        );
        let real_xref_off = b.xref_and_trailer(
            &[(1, obj1_off), (2, obj2_off), (3, obj3_off)],
            4,
            "/Root 1 0 R",
        );

        let mut data = b.finish();
        // Corrupt the xref table in place: overwrite the "xref" keyword
        // with garbage so it can neither be parsed as a traditional table
        // nor as an xref stream/indirect object.
        let start = real_xref_off as usize;
        for (i, byte) in data[start..start + 4].iter_mut().enumerate() {
            *byte = b'X' + i as u8;
        }

        // A naive parser would now fail outright; recovery mode must
        // reconstruct the structure by scanning for `obj`/`endobj`.
        let reader = PdfReader::from_bytes(data).expect("recovery mode should succeed");
        assert_eq!(reader.page_count(), 1);
        assert!(reader.catalog().is_some());
    }

    #[test]
    fn test_recovery_mode_explicit_entry_point() {
        let mut b = RawPdfBuilder::new();
        let obj1_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
        let obj2_off = b.object(2, "<< /Type /Pages /Kids [] /Count 0 >>");
        b.xref_and_trailer(&[(1, obj1_off), (2, obj2_off)], 3, "/Root 1 0 R");

        let reader = PdfReader::from_bytes_recovery_only(b.finish()).unwrap();
        assert!(reader.catalog().is_some());
        assert_eq!(reader.page_count(), 0);
    }

    #[test]
    fn test_catalog_access() {
        let pdf_bytes = create_simple_pdf();
        let reader = PdfReader::from_bytes(pdf_bytes).unwrap();

        let catalog = reader.catalog().unwrap();
        assert!(catalog.get("Type").is_some());
        assert!(catalog.get("Pages").is_some());
    }

    #[test]
    fn test_info_access() {
        let pdf_bytes = create_simple_pdf();
        let reader = PdfReader::from_bytes(pdf_bytes).unwrap();

        let info = reader.info().unwrap();
        assert!(info.get("Title").is_some());
    }

    #[test]
    fn test_roundtrip_multi_page() {
        use crate::prelude::*;

        let page1 = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Page 1"))
            .build();

        let page2 = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Page 2"))
            .build();

        let doc = DocumentBuilder::new()
            .page(page1)
            .page(page2)
            .build()
            .unwrap();

        let pdf_bytes = doc.save_to_bytes().unwrap();
        let reader = PdfReader::from_bytes(pdf_bytes).unwrap();

        assert_eq!(reader.page_count(), 2);
    }
}
