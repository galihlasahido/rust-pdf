//! PDF object serialization.

use crate::object::{Object, PdfStream};
use crate::types::ObjectId;
use std::io::{self, Write};

/// Serializes PDF objects to bytes.
pub struct Serializer<W: Write> {
    writer: W,
    position: u64,
}

impl<W: Write> Serializer<W> {
    /// Creates a new serializer wrapping the given writer.
    pub fn new(writer: W) -> Self {
        Self { writer, position: 0 }
    }

    /// Returns the current byte position.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Writes bytes and updates the position.
    pub fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.position += bytes.len() as u64;
        Ok(())
    }

    /// Writes a string and updates the position.
    pub fn write_str(&mut self, s: &str) -> io::Result<()> {
        self.write_bytes(s.as_bytes())
    }

    /// Writes a newline.
    pub fn write_newline(&mut self) -> io::Result<()> {
        self.write_bytes(b"\n")
    }

    /// Writes the PDF header.
    pub fn write_header(&mut self, version: &str) -> io::Result<()> {
        self.write_str(&format!("%PDF-{}\n", version))?;
        // Write binary marker (high-bit bytes to indicate binary content)
        self.write_bytes(b"%\xE2\xE3\xCF\xD3\n")?;
        Ok(())
    }

    /// Writes an indirect object definition.
    ///
    /// Returns the byte offset where the object starts.
    pub fn write_object(&mut self, id: ObjectId, object: &Object) -> io::Result<u64> {
        let offset = self.position;

        // Write object header
        self.write_str(&format!("{} {} obj\n", id.number, id.generation))?;

        // Write the object content
        match object {
            Object::Stream(stream) => {
                self.write_stream(stream)?;
            }
            _ => {
                self.write_str(&object.to_pdf_string())?;
                self.write_newline()?;
            }
        }

        // Write object footer
        self.write_str("endobj\n")?;

        Ok(offset)
    }

    /// Writes a stream object.
    fn write_stream(&mut self, stream: &PdfStream) -> io::Result<()> {
        // Write dictionary
        self.write_str(&stream.dictionary.to_pdf_string())?;
        self.write_str("\nstream\n")?;

        // Write stream data
        self.write_bytes(stream.data())?;

        // Write stream end
        self.write_str("\nendstream\n")?;

        Ok(())
    }

    /// Writes the cross-reference table.
    ///
    /// Returns the byte offset where the xref starts.
    pub fn write_xref(&mut self, offsets: &[(ObjectId, u64)]) -> io::Result<u64> {
        let xref_offset = self.position;

        self.write_str("xref\n")?;

        // Write the xref section header
        // We assume objects are numbered 0 to N consecutively
        let max_obj_num = offsets.iter().map(|(id, _)| id.number).max().unwrap_or(0);
        self.write_str(&format!("0 {}\n", max_obj_num + 1))?;

        // Write entry for object 0 (free entry, head of free list)
        self.write_str("0000000000 65535 f \n")?;

        // Create a map for quick lookup
        let offset_map: std::collections::HashMap<u32, u64> =
            offsets.iter().map(|(id, off)| (id.number, *off)).collect();

        // Write entries for objects 1 to max
        for obj_num in 1..=max_obj_num {
            if let Some(&offset) = offset_map.get(&obj_num) {
                self.write_str(&format!("{:010} {:05} n \n", offset, 0))?;
            } else {
                // Free entry (shouldn't happen in well-formed documents)
                self.write_str("0000000000 65535 f \n")?;
            }
        }

        Ok(xref_offset)
    }

    /// Writes the trailer dictionary.
    pub fn write_trailer(
        &mut self,
        size: u32,
        root_id: ObjectId,
        info_id: Option<ObjectId>,
    ) -> io::Result<()> {
        self.write_trailer_with_encryption(size, root_id, info_id, None, None)
    }

    /// Writes the trailer dictionary with optional encryption.
    pub fn write_trailer_with_encryption(
        &mut self,
        size: u32,
        root_id: ObjectId,
        info_id: Option<ObjectId>,
        encrypt_id: Option<ObjectId>,
        file_id: Option<&[u8]>,
    ) -> io::Result<()> {
        self.write_str("trailer\n")?;
        self.write_str("<< ")?;
        self.write_str(&format!("/Size {} ", size))?;
        self.write_str(&format!("/Root {} ", root_id.reference_string()))?;

        if let Some(info) = info_id {
            self.write_str(&format!("/Info {} ", info.reference_string()))?;
        }

        if let Some(encrypt) = encrypt_id {
            self.write_str(&format!("/Encrypt {} ", encrypt.reference_string()))?;
        }

        if let Some(id) = file_id {
            // Write ID array with the same ID twice (required for encrypted PDFs)
            let hex_id: String = id.iter().map(|b| format!("{:02X}", b)).collect();
            self.write_str(&format!("/ID [<{}> <{}>] ", hex_id, hex_id))?;
        }

        self.write_str(">>\n")?;

        Ok(())
    }

    /// Writes the startxref marker and EOF.
    pub fn write_startxref(&mut self, xref_offset: u64) -> io::Result<()> {
        self.write_str("startxref\n")?;
        self.write_str(&format!("{}\n", xref_offset))?;
        self.write_str("%%EOF\n")?;
        Ok(())
    }

    /// Flushes the underlying writer.
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    /// Returns the underlying writer.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{PdfDictionary, PdfName};

    #[test]
    fn test_write_header() {
        let mut buffer = Vec::new();
        let mut serializer = Serializer::new(&mut buffer);

        serializer.write_header("1.7").unwrap();

        let output = String::from_utf8_lossy(&buffer);
        assert!(output.starts_with("%PDF-1.7"));
    }

    #[test]
    fn test_write_object() {
        let mut buffer = Vec::new();
        let mut serializer = Serializer::new(&mut buffer);

        let id = ObjectId::new(1);
        let obj = Object::Integer(42);
        serializer.write_object(id, &obj).unwrap();

        let output = String::from_utf8_lossy(&buffer);
        assert!(output.contains("1 0 obj"));
        assert!(output.contains("42"));
        assert!(output.contains("endobj"));
    }

    #[test]
    fn test_write_dictionary_object() {
        let mut buffer = Vec::new();
        let mut serializer = Serializer::new(&mut buffer);

        let id = ObjectId::new(1);
        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Catalog")));

        serializer.write_object(id, &Object::Dictionary(dict)).unwrap();

        let output = String::from_utf8_lossy(&buffer);
        assert!(output.contains("/Type /Catalog"));
    }

    #[test]
    fn test_position_tracking() {
        let mut buffer = Vec::new();
        let mut serializer = Serializer::new(&mut buffer);

        assert_eq!(serializer.position(), 0);
        serializer.write_str("Hello").unwrap();
        assert_eq!(serializer.position(), 5);
    }
}
