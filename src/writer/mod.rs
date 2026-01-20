//! PDF file writing functionality.

mod serializer;
mod xref;

pub use serializer::Serializer;
pub use xref::{XrefEntry, XrefTable};

use crate::error::{PdfResult, WriterError};
use crate::object::Object;
use crate::types::ObjectId;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

#[cfg(feature = "encryption")]
use crate::encryption::EncryptionHandler;

/// A PDF writer that manages object allocation and file output.
pub struct PdfWriter<W: Write> {
    serializer: Serializer<W>,
    xref: XrefTable,
    next_object_number: u32,
    version: String,
    #[cfg(feature = "encryption")]
    encryption_handler: Option<EncryptionHandler>,
}

impl<W: Write> PdfWriter<W> {
    /// Creates a new PDF writer with the given output.
    pub fn new(writer: W, version: &str) -> Self {
        Self {
            serializer: Serializer::new(writer),
            xref: XrefTable::new(),
            next_object_number: 1,
            version: version.to_string(),
            #[cfg(feature = "encryption")]
            encryption_handler: None,
        }
    }

    /// Sets the encryption handler for encrypting streams and strings.
    #[cfg(feature = "encryption")]
    pub fn set_encryption_handler(&mut self, handler: EncryptionHandler) {
        self.encryption_handler = Some(handler);
    }

    /// Allocates the next object ID.
    pub fn allocate_id(&mut self) -> ObjectId {
        let id = ObjectId::new(self.next_object_number);
        self.next_object_number += 1;
        id
    }

    /// Returns the next object number that will be allocated.
    pub fn peek_next_id(&self) -> u32 {
        self.next_object_number
    }

    /// Writes the PDF header.
    pub fn write_header(&mut self) -> PdfResult<()> {
        self.serializer
            .write_header(&self.version)
            .map_err(|e| WriterError::Structure(e.to_string()))?;
        Ok(())
    }

    /// Writes an object and records its position in the xref table.
    ///
    /// Returns the object ID.
    pub fn write_object(&mut self, object: &Object) -> PdfResult<ObjectId> {
        let id = self.allocate_id();
        self.write_object_with_id(id, object)?;
        Ok(id)
    }

    /// Writes an object with a specific ID.
    pub fn write_object_with_id(&mut self, id: ObjectId, object: &Object) -> PdfResult<()> {
        self.write_object_internal(id, object, true)
    }

    /// Writes an object without encryption (for encryption dictionary, etc.).
    #[cfg(feature = "encryption")]
    pub fn write_object_unencrypted(&mut self, id: ObjectId, object: &Object) -> PdfResult<()> {
        self.write_object_internal(id, object, false)
    }

    /// Internal method for writing objects with optional encryption.
    #[allow(unused_variables)]
    fn write_object_internal(&mut self, id: ObjectId, object: &Object, encrypt: bool) -> PdfResult<()> {
        // Encrypt the object if encryption is enabled and requested
        #[cfg(feature = "encryption")]
        let object = if encrypt {
            if let Some(ref handler) = self.encryption_handler {
                self.encrypt_object(object, id, handler)?
            } else {
                object.clone()
            }
        } else {
            object.clone()
        };

        #[cfg(not(feature = "encryption"))]
        let object = object.clone();

        let offset = self
            .serializer
            .write_object(id, &object)
            .map_err(|e| WriterError::Structure(e.to_string()))?;

        self.xref.add_object(id, offset);
        Ok(())
    }

    /// Encrypts an object (streams and strings) using the encryption handler.
    #[cfg(feature = "encryption")]
    fn encrypt_object(
        &self,
        object: &Object,
        id: ObjectId,
        handler: &EncryptionHandler,
    ) -> PdfResult<Object> {
        use crate::object::{PdfArray, PdfDictionary, PdfStream, PdfString};

        match object {
            Object::String(s) => {
                // Encrypt string content
                let plain_bytes = s.as_bytes();
                let encrypted = handler
                    .encrypt_data(&plain_bytes, id.number, id.generation)
                    .map_err(|e| WriterError::Structure(e.to_string()))?;
                Ok(Object::String(PdfString::Hex(encrypted)))
            }
            Object::Stream(stream) => {
                // Encrypt stream data
                let encrypted_data = handler
                    .encrypt_data(stream.data(), id.number, id.generation)
                    .map_err(|e| WriterError::Structure(e.to_string()))?;

                // Create new stream with encrypted data and updated length
                let mut new_dict = stream.dictionary.clone();
                new_dict.set("Length", Object::Integer(encrypted_data.len() as i64));

                Ok(Object::Stream(PdfStream::from_raw(new_dict, encrypted_data)))
            }
            Object::Dictionary(dict) => {
                // Recursively encrypt strings in dictionary
                let mut new_dict = PdfDictionary::new();
                for (key, value) in dict.iter() {
                    let encrypted_value = self.encrypt_object(value, id, handler)?;
                    new_dict.set(key, encrypted_value);
                }
                Ok(Object::Dictionary(new_dict))
            }
            Object::Array(arr) => {
                // Recursively encrypt strings in array
                let mut new_arr = PdfArray::new();
                for item in arr.iter() {
                    let encrypted_item = self.encrypt_object(item, id, handler)?;
                    new_arr.push(encrypted_item);
                }
                Ok(Object::Array(new_arr))
            }
            // Other types don't need encryption
            _ => Ok(object.clone()),
        }
    }

    /// Writes the trailer, xref table, and EOF.
    pub fn write_trailer(
        &mut self,
        root_id: ObjectId,
        info_id: Option<ObjectId>,
    ) -> PdfResult<()> {
        self.write_trailer_with_encryption(root_id, info_id, None, None)
    }

    /// Writes the trailer with optional encryption, xref table, and EOF.
    pub fn write_trailer_with_encryption(
        &mut self,
        root_id: ObjectId,
        info_id: Option<ObjectId>,
        encrypt_id: Option<ObjectId>,
        file_id: Option<&[u8]>,
    ) -> PdfResult<()> {
        // Write the xref table
        let xref_content = self.xref.to_xref_string();
        let xref_offset = self.serializer.position();
        self.serializer
            .write_str(&xref_content)
            .map_err(|e| WriterError::Structure(e.to_string()))?;

        // Write trailer
        self.serializer
            .write_trailer_with_encryption(self.xref.size(), root_id, info_id, encrypt_id, file_id)
            .map_err(|e| WriterError::Structure(e.to_string()))?;

        // Write startxref and EOF
        self.serializer
            .write_startxref(xref_offset)
            .map_err(|e| WriterError::Structure(e.to_string()))?;

        // Flush
        self.serializer
            .flush()
            .map_err(|e| WriterError::Structure(e.to_string()))?;

        Ok(())
    }

    /// Returns the underlying writer.
    pub fn into_inner(self) -> W {
        self.serializer.into_inner()
    }
}

impl PdfWriter<BufWriter<File>> {
    /// Creates a new PDF writer that writes to a file.
    pub fn create_file(path: impl AsRef<Path>, version: &str) -> PdfResult<Self> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        Ok(Self::new(writer, version))
    }
}

impl PdfWriter<Vec<u8>> {
    /// Creates a new PDF writer that writes to memory.
    pub fn create_memory(version: &str) -> Self {
        Self::new(Vec::new(), version)
    }

    /// Returns the written bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{PdfDictionary, PdfName};

    #[test]
    fn test_allocate_id() {
        let mut writer = PdfWriter::create_memory("1.7");
        let id1 = writer.allocate_id();
        let id2 = writer.allocate_id();

        assert_eq!(id1.number, 1);
        assert_eq!(id2.number, 2);
    }

    #[test]
    fn test_write_minimal_pdf() {
        let mut writer = PdfWriter::create_memory("1.7");
        writer.write_header().unwrap();

        // Write catalog
        let mut catalog = PdfDictionary::new();
        catalog.set("Type", Object::Name(PdfName::catalog()));

        let catalog_id = writer.write_object(&Object::Dictionary(catalog)).unwrap();

        // Write trailer
        writer.write_trailer(catalog_id, None).unwrap();

        let bytes = writer.into_bytes();
        let content = String::from_utf8_lossy(&bytes);

        assert!(content.starts_with("%PDF-1.7"));
        assert!(content.contains("/Type /Catalog"));
        assert!(content.contains("xref"));
        assert!(content.contains("trailer"));
        assert!(content.contains("%%EOF"));
    }
}
