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

mod lexer;
mod objects;
mod trailer;
mod xref;

pub use trailer::Trailer;
pub use xref::{XrefEntry, XrefTable};

use crate::document::PdfVersion;
use crate::error::{ParserError, PdfResult};
use crate::object::{Object, PdfDictionary};
use crate::types::ObjectId;
use objects::parse_indirect_object;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use trailer::parse_trailer;
use xref::{find_startxref, parse_xref_table};

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
    pub fn from_bytes(data: Vec<u8>) -> PdfResult<Self> {
        // Parse header
        let version = Self::parse_header(&data)?;

        // Find startxref
        let xref_offset = find_startxref(&data)?;

        // Parse xref table and trailer
        let (xref, trailer) = Self::parse_xref_and_trailer(&data, xref_offset)?;

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

    /// Parses the xref table and trailer.
    fn parse_xref_and_trailer(
        data: &[u8],
        mut xref_offset: u64,
    ) -> Result<(XrefTable, Trailer), ParserError> {
        let mut combined_xref = XrefTable::new();
        let mut final_trailer: Option<Trailer> = None;

        // Follow the chain of xref tables (for incremental updates)
        loop {
            let xref_data = &data[xref_offset as usize..];

            // Check if this is a traditional xref table or xref stream
            if xref_data.starts_with(b"xref") {
                // Traditional xref table
                let (remaining, xref) = parse_xref_table(xref_data)
                    .map_err(|_| ParserError::InvalidXref)?;

                combined_xref.merge(xref);

                // Parse trailer
                let (_, trailer_dict) = parse_trailer(remaining)
                    .map_err(|_| ParserError::InvalidTrailer)?;

                let trailer = Trailer::from_dictionary(trailer_dict)?;

                // Save the first trailer (most recent) as final
                if final_trailer.is_none() {
                    let prev = trailer.prev;
                    final_trailer = Some(trailer);

                    if let Some(prev_offset) = prev {
                        xref_offset = prev_offset;
                        continue;
                    }
                }
            } else {
                // Could be an xref stream (PDF 1.5+)
                // For now, try to parse as an indirect object
                let (_, (_, _, obj)) = parse_indirect_object(xref_data)
                    .map_err(|_| ParserError::InvalidXrefStream)?;

                match obj {
                    Object::Stream(stream) => {
                        // Parse xref stream
                        let xref = Self::parse_xref_stream(&stream)?;
                        combined_xref.merge(xref);

                        // Get trailer info from stream dictionary
                        let trailer = Trailer::from_dictionary(stream.dictionary.clone())?;

                        if final_trailer.is_none() {
                            let prev = trailer.prev;
                            final_trailer = Some(trailer);

                            if let Some(prev_offset) = prev {
                                xref_offset = prev_offset;
                                continue;
                            }
                        }
                    }
                    _ => return Err(ParserError::InvalidXrefStream),
                }
            }

            break;
        }

        let trailer = final_trailer.ok_or(ParserError::InvalidTrailer)?;
        Ok((combined_xref, trailer))
    }

    /// Parses an xref stream (PDF 1.5+).
    fn parse_xref_stream(stream: &crate::object::PdfStream) -> Result<XrefTable, ParserError> {
        let dict = &stream.dictionary;

        // Get W array (field widths)
        let w = match dict.get("W") {
            Some(Object::Array(arr)) => arr,
            _ => return Err(ParserError::InvalidXrefStream),
        };

        if w.len() != 3 {
            return Err(ParserError::InvalidXrefStream);
        }

        let w1 = match w.get(0) {
            Some(Object::Integer(n)) => *n as usize,
            _ => return Err(ParserError::InvalidXrefStream),
        };
        let w2 = match w.get(1) {
            Some(Object::Integer(n)) => *n as usize,
            _ => return Err(ParserError::InvalidXrefStream),
        };
        let w3 = match w.get(2) {
            Some(Object::Integer(n)) => *n as usize,
            _ => return Err(ParserError::InvalidXrefStream),
        };

        let entry_size = w1 + w2 + w3;

        // Get Index array (optional, defaults to [0 Size])
        let size = match dict.get("Size") {
            Some(Object::Integer(n)) => *n as u32,
            _ => return Err(ParserError::InvalidXrefStream),
        };

        let index: Vec<(u32, u32)> = match dict.get("Index") {
            Some(Object::Array(arr)) => {
                let mut pairs = Vec::new();
                let mut iter = arr.iter();
                while let (Some(Object::Integer(start)), Some(Object::Integer(count))) =
                    (iter.next(), iter.next())
                {
                    pairs.push((*start as u32, *count as u32));
                }
                pairs
            }
            _ => vec![(0, size)],
        };

        // Decompress stream data
        #[cfg(feature = "compression")]
        let data = if stream.is_compressed() {
            stream.decompress()?
        } else {
            stream.data().to_vec()
        };

        #[cfg(not(feature = "compression"))]
        let data = stream.data().to_vec();

        // Parse entries
        let mut table = XrefTable::new();
        let mut data_offset = 0;

        for (start, count) in index {
            for i in 0..count {
                let obj_num = start + i;

                if data_offset + entry_size > data.len() {
                    return Err(ParserError::InvalidXrefStream);
                }

                // Read type field
                let entry_type = if w1 == 0 {
                    1 // Default type is 1 (in use)
                } else {
                    Self::read_int(&data[data_offset..data_offset + w1])
                };

                // Read field 2
                let field2 = Self::read_int(&data[data_offset + w1..data_offset + w1 + w2]);

                // Read field 3
                let field3 = Self::read_int(&data[data_offset + w1 + w2..data_offset + entry_size]);

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

                table.insert(obj_num, entry);
                data_offset += entry_size;
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
    pub fn resolve_reference(&self, id: ObjectId) -> Option<Object> {
        // Check cache first
        if let Some(obj) = self.object_cache.get(&id) {
            return Some(obj.clone());
        }

        // Get xref entry
        let entry = self.xref.get(id.number)?;

        match entry {
            XrefEntry::InUse { offset, .. } => {
                // Parse object at offset
                let data = &self.data[*offset as usize..];
                let (_, (_, _, obj)) = parse_indirect_object(data).ok()?;
                Some(obj)
            }
            XrefEntry::Compressed {
                object_stream,
                index,
            } => {
                // Object is in an object stream - more complex
                self.resolve_compressed_object(*object_stream, *index)
            }
            XrefEntry::Free { .. } => None,
        }
    }

    /// Resolves an object from a compressed object stream.
    fn resolve_compressed_object(&self, stream_num: u32, index: u32) -> Option<Object> {
        // Get the object stream
        let stream_entry = self.xref.get(stream_num)?;
        let offset = stream_entry.offset()?;

        let data = &self.data[offset as usize..];
        let (_, (_, _, stream_obj)) = parse_indirect_object(data).ok()?;

        let stream = match stream_obj {
            Object::Stream(s) => s,
            _ => return None,
        };

        // Get N (number of objects) and First (offset to first object)
        let dict = &stream.dictionary;
        let num_objects = match dict.get("N") {
            Some(Object::Integer(n)) => *n as usize,
            _ => return None,
        };
        let first = match dict.get("First") {
            Some(Object::Integer(f)) => *f as usize,
            _ => return None,
        };

        // Decompress stream
        #[cfg(feature = "compression")]
        let stream_data = if stream.is_compressed() {
            stream.decompress().ok()?
        } else {
            stream.data().to_vec()
        };

        #[cfg(not(feature = "compression"))]
        let stream_data = stream.data().to_vec();

        // Parse the header (N pairs of obj_num, offset)
        let header = &stream_data[..first];
        let objects_data = &stream_data[first..];

        // Find the offset for our object
        let header_str = std::str::from_utf8(header).ok()?;
        let nums: Vec<i64> = header_str
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();

        if nums.len() < (index as usize + 1) * 2 {
            return None;
        }

        let obj_offset = nums[index as usize * 2 + 1] as usize;

        // Find end offset (next object's offset or end of data)
        let next_offset = if (index as usize + 1) < num_objects {
            nums[(index as usize + 1) * 2 + 1] as usize
        } else {
            objects_data.len()
        };

        // Parse the object
        let obj_data = &objects_data[obj_offset..next_offset];
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
