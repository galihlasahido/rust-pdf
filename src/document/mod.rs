//! PDF Document structure and building.

mod info;
mod version;

pub use info::{DocumentInfo, DocumentInfoBuilder};
pub use version::PdfVersion;

use crate::error::{DocumentError, PdfResult};
use crate::object::{Object, PdfArray, PdfDictionary, PdfName};
use crate::page::Page;
use crate::types::ObjectId;
use crate::writer::PdfWriter;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

#[cfg(feature = "encryption")]
use crate::encryption::{generate_file_id, EncryptionConfig, EncryptionHandler};

/// A complete PDF document.
#[derive(Debug)]
pub struct Document {
    /// PDF version.
    pub version: PdfVersion,
    /// Document metadata.
    pub info: DocumentInfo,
    /// Pages in the document.
    pub pages: Vec<Page>,
    /// Whether to compress content streams.
    #[cfg(feature = "compression")]
    pub compress_streams: bool,
    /// Encryption configuration.
    #[cfg(feature = "encryption")]
    pub encryption: Option<EncryptionConfig>,
}

impl Document {
    /// Creates a new document with default settings.
    pub fn new() -> Self {
        Self {
            version: PdfVersion::default(),
            info: DocumentInfo::new(),
            pages: Vec::new(),
            #[cfg(feature = "compression")]
            compress_streams: false,
            #[cfg(feature = "encryption")]
            encryption: None,
        }
    }

    /// Adds a page to the document.
    pub fn add_page(&mut self, page: Page) {
        self.pages.push(page);
    }

    /// Returns the number of pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Saves the document to a file.
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> PdfResult<()> {
        if self.pages.is_empty() {
            return Err(DocumentError::NoPages.into());
        }

        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        self.write_to(writer)
    }

    /// Saves the document to a byte vector.
    pub fn save_to_bytes(&self) -> PdfResult<Vec<u8>> {
        if self.pages.is_empty() {
            return Err(DocumentError::NoPages.into());
        }

        let buffer = Vec::new();
        let mut cursor = std::io::Cursor::new(buffer);
        self.write_to(&mut cursor)?;
        Ok(cursor.into_inner())
    }

    /// Writes the document to any writer.
    pub fn write_to<W: Write>(&self, writer: W) -> PdfResult<()> {
        let mut pdf_writer = PdfWriter::new(writer, self.version.as_str());

        // Create encryption handler if configured
        #[cfg(feature = "encryption")]
        let encryption_handler = if let Some(ref config) = self.encryption {
            let file_id = generate_file_id();
            let handler = EncryptionHandler::new(config.clone(), file_id)?;
            // Set the encryption handler on the writer so it encrypts streams/strings
            pdf_writer.set_encryption_handler(handler.clone());
            Some(handler)
        } else {
            None
        };

        // Write header
        pdf_writer.write_header()?;

        // Allocate object IDs for structure
        let catalog_id = pdf_writer.allocate_id();
        let pages_id = pdf_writer.allocate_id();

        // Allocate IDs for each page and its content
        let mut page_ids: Vec<ObjectId> = Vec::new();
        let mut content_ids: Vec<ObjectId> = Vec::new();
        let mut font_ids: Vec<Vec<(String, ObjectId)>> = Vec::new();

        // Image IDs: Vec of (name, image_id, optional soft_mask_id)
        #[cfg(feature = "images")]
        let mut image_ids: Vec<Vec<(String, ObjectId, Option<ObjectId>)>> = Vec::new();

        for page in &self.pages {
            page_ids.push(pdf_writer.allocate_id());
            content_ids.push(pdf_writer.allocate_id());

            // Allocate font IDs for this page
            let mut page_font_ids = Vec::new();
            for (font_name, _) in &page.fonts {
                page_font_ids.push((font_name.clone(), pdf_writer.allocate_id()));
            }
            font_ids.push(page_font_ids);

            // Allocate image IDs for this page
            #[cfg(feature = "images")]
            {
                let mut page_image_ids = Vec::new();
                for (image_name, image) in &page.images {
                    let image_id = pdf_writer.allocate_id();
                    let mask_id = if image.has_alpha() {
                        Some(pdf_writer.allocate_id())
                    } else {
                        None
                    };
                    page_image_ids.push((image_name.clone(), image_id, mask_id));
                }
                image_ids.push(page_image_ids);
            }
        }

        // Allocate info object ID if we have metadata
        let info_id = if !self.info.is_empty() {
            Some(pdf_writer.allocate_id())
        } else {
            None
        };

        // Allocate encrypt object ID if encryption is configured
        #[cfg(feature = "encryption")]
        let encrypt_id = if encryption_handler.is_some() {
            Some(pdf_writer.allocate_id())
        } else {
            None
        };

        // Write catalog
        let mut catalog = PdfDictionary::new();
        catalog.set("Type", Object::Name(PdfName::catalog()));
        catalog.set("Pages", Object::Reference(pages_id));
        pdf_writer.write_object_with_id(catalog_id, &Object::Dictionary(catalog))?;

        // Write pages tree
        let mut pages_dict = PdfDictionary::new();
        pages_dict.set("Type", Object::Name(PdfName::pages()));

        let mut kids = PdfArray::new();
        for &page_id in &page_ids {
            kids.push(Object::Reference(page_id));
        }
        pages_dict.set("Kids", Object::Array(kids));
        pages_dict.set("Count", Object::Integer(self.pages.len() as i64));
        pdf_writer.write_object_with_id(pages_id, &Object::Dictionary(pages_dict))?;

        // Write each page
        for (i, page) in self.pages.iter().enumerate() {
            let page_id = page_ids[i];
            let content_id = content_ids[i];
            let page_fonts = &font_ids[i];

            // Build font resources dictionary
            let mut font_dict = PdfDictionary::new();
            for (font_name, font_id) in page_fonts {
                font_dict.set(font_name, Object::Reference(*font_id));
            }

            // Build XObject resources dictionary for images
            #[cfg(feature = "images")]
            let page_images = &image_ids[i];

            #[cfg(feature = "images")]
            let mut xobject_dict = PdfDictionary::new();
            #[cfg(feature = "images")]
            for (image_name, image_id, _) in page_images {
                xobject_dict.set(image_name, Object::Reference(*image_id));
            }

            // Build resources dictionary
            let mut resources = PdfDictionary::new();
            if !font_dict.is_empty() {
                resources.set("Font", Object::Dictionary(font_dict));
            }
            #[cfg(feature = "images")]
            if !xobject_dict.is_empty() {
                resources.set("XObject", Object::Dictionary(xobject_dict));
            }

            // Build page dictionary
            let mut page_dict = PdfDictionary::new();
            page_dict.set("Type", Object::Name(PdfName::page()));
            page_dict.set("Parent", Object::Reference(pages_id));

            // MediaBox
            let media_box = page.media_box.to_array();
            let mut media_box_array = PdfArray::new();
            for val in media_box {
                media_box_array.push(Object::Real(val));
            }
            page_dict.set("MediaBox", Object::Array(media_box_array));

            // Resources
            if !resources.is_empty() {
                page_dict.set("Resources", Object::Dictionary(resources));
            }

            // Contents
            page_dict.set("Contents", Object::Reference(content_id));

            pdf_writer.write_object_with_id(page_id, &Object::Dictionary(page_dict))?;

            // Write content stream (with optional compression)
            let content_stream = page.build_content_stream();
            #[cfg(feature = "compression")]
            let content_stream = if self.compress_streams {
                content_stream.with_compression()?
            } else {
                content_stream
            };
            pdf_writer.write_object_with_id(content_id, &Object::Stream(content_stream))?;

            // Write font objects
            for (j, (_, font_id)) in page_fonts.iter().enumerate() {
                let (_, font) = &page.fonts[j];
                let font_dict = font.to_dictionary();
                pdf_writer.write_object_with_id(*font_id, &Object::Dictionary(font_dict))?;
            }

            // Write image XObject streams
            #[cfg(feature = "images")]
            {
                use crate::image::ImageXObject;

                for (j, (_, image_id, mask_id)) in page_images.iter().enumerate() {
                    let (_, image) = &page.images[j];
                    let xobject = ImageXObject::from_image_with_mask_ref(image, *mask_id);

                    // Write soft mask first if present
                    if let (Some(mask_id), Some(mask_stream)) = (mask_id, xobject.soft_mask) {
                        pdf_writer.write_object_with_id(*mask_id, &Object::Stream(mask_stream))?;
                    }

                    // Write main image stream
                    pdf_writer.write_object_with_id(*image_id, &Object::Stream(xobject.stream))?;
                }
            }
        }

        // Write info dictionary if present
        if let Some(info_id) = info_id {
            let info_dict = self.info.to_dictionary();
            pdf_writer.write_object_with_id(info_id, &Object::Dictionary(info_dict))?;
        }

        // Write encryption dictionary if encryption is enabled
        // NOTE: The encryption dictionary must NOT be encrypted itself!
        #[cfg(feature = "encryption")]
        if let (Some(encrypt_id), Some(ref handler)) = (encrypt_id, &encryption_handler) {
            let encrypt_dict = handler.create_encrypt_dictionary();
            pdf_writer.write_object_unencrypted(encrypt_id, &Object::Dictionary(encrypt_dict))?;
        }

        // Write trailer
        #[cfg(feature = "encryption")]
        {
            if let (Some(encrypt_id), Some(ref handler)) = (encrypt_id, &encryption_handler) {
                pdf_writer.write_trailer_with_encryption(
                    catalog_id,
                    info_id,
                    Some(encrypt_id),
                    Some(handler.file_id()),
                )?;
            } else {
                pdf_writer.write_trailer(catalog_id, info_id)?;
            }
        }

        #[cfg(not(feature = "encryption"))]
        pdf_writer.write_trailer(catalog_id, info_id)?;

        Ok(())
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for creating PDF documents.
#[derive(Debug, Default)]
pub struct DocumentBuilder {
    version: PdfVersion,
    info: DocumentInfo,
    pages: Vec<Page>,
    #[cfg(feature = "compression")]
    compress_streams: bool,
    #[cfg(feature = "encryption")]
    encryption: Option<EncryptionConfig>,
}

impl DocumentBuilder {
    /// Creates a new document builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the PDF version.
    pub fn version(mut self, version: PdfVersion) -> Self {
        self.version = version;
        self
    }

    /// Sets the document title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.info.title = Some(title.into());
        self
    }

    /// Sets the document author.
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.info.author = Some(author.into());
        self
    }

    /// Sets the document subject.
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.info.subject = Some(subject.into());
        self
    }

    /// Sets the document keywords.
    pub fn keywords(mut self, keywords: impl Into<String>) -> Self {
        self.info.keywords = Some(keywords.into());
        self
    }

    /// Sets the creator application.
    pub fn creator(mut self, creator: impl Into<String>) -> Self {
        self.info.creator = Some(creator.into());
        self
    }

    /// Sets the producer application.
    pub fn producer(mut self, producer: impl Into<String>) -> Self {
        self.info.producer = Some(producer.into());
        self
    }

    /// Sets the document info.
    pub fn info(mut self, info: DocumentInfo) -> Self {
        self.info = info;
        self
    }

    /// Adds a page to the document.
    pub fn page(mut self, page: Page) -> Self {
        self.pages.push(page);
        self
    }

    /// Adds multiple pages to the document.
    pub fn pages(mut self, pages: impl IntoIterator<Item = Page>) -> Self {
        self.pages.extend(pages);
        self
    }

    /// Enables compression for all content streams.
    ///
    /// When enabled, all page content streams will be compressed using
    /// Flate compression (FlateDecode filter).
    #[cfg(feature = "compression")]
    pub fn compress_streams(mut self, compress: bool) -> Self {
        self.compress_streams = compress;
        self
    }

    /// Enables encryption with the given configuration.
    ///
    /// When enabled, the document will be encrypted using AES-256.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use rust_pdf::prelude::*;
    /// use rust_pdf::encryption::{EncryptionConfig, Permissions};
    ///
    /// let doc = DocumentBuilder::new()
    ///     .encrypt(EncryptionConfig::aes256()
    ///         .user_password("user123")
    ///         .owner_password("owner456")
    ///         .permissions(Permissions::new().allow_printing(true)))
    ///     .page(page)
    ///     .build()?;
    /// ```
    #[cfg(feature = "encryption")]
    pub fn encrypt(mut self, config: EncryptionConfig) -> Self {
        self.encryption = Some(config);
        self
    }

    /// Builds the document.
    ///
    /// Returns an error if no pages have been added.
    pub fn build(self) -> PdfResult<Document> {
        if self.pages.is_empty() {
            return Err(DocumentError::NoPages.into());
        }

        Ok(Document {
            version: self.version,
            info: self.info,
            pages: self.pages,
            #[cfg(feature = "compression")]
            compress_streams: self.compress_streams,
            #[cfg(feature = "encryption")]
            encryption: self.encryption,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::PageBuilder;

    #[test]
    fn test_document_builder() {
        let page = PageBuilder::a4().build();

        let doc = DocumentBuilder::new()
            .version(PdfVersion::V1_7)
            .title("Test Document")
            .author("Test Author")
            .page(page)
            .build()
            .unwrap();

        assert_eq!(doc.version, PdfVersion::V1_7);
        assert_eq!(doc.page_count(), 1);
    }

    #[test]
    fn test_document_no_pages_error() {
        let result = DocumentBuilder::new().build();
        assert!(result.is_err());
    }

    #[test]
    fn test_save_to_bytes() {
        let page = PageBuilder::a4().build();
        let doc = DocumentBuilder::new().page(page).build().unwrap();

        let bytes = doc.save_to_bytes().unwrap();
        let content = String::from_utf8_lossy(&bytes);

        assert!(content.starts_with("%PDF-1.7"));
        assert!(content.contains("%%EOF"));
    }
}
