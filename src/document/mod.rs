//! PDF Document structure and building.

mod info;
mod version;

pub use info::{DocumentInfo, DocumentInfoBuilder};
pub use version::PdfVersion;

use crate::error::{DocumentError, PdfResult};
use crate::forms::{AppearanceBuilder, FormField, FormFieldType};
use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream, PdfString};
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

        // Check if any page has form fields
        let has_forms = self.pages.iter().any(|p| p.has_form_fields());

        // Allocate AcroForm ID if needed
        let acroform_id = if has_forms {
            Some(pdf_writer.allocate_id())
        } else {
            None
        };

        // Allocate form field and widget IDs for each page
        // Structure: Vec<Vec<(field_id, appearance_normal_id, appearance_down_id_opt)>>
        let mut form_field_ids: Vec<Vec<FormFieldIds>> = Vec::new();
        for page in &self.pages {
            let mut page_field_ids = Vec::new();
            for field in page.form_fields() {
                let field_id = pdf_writer.allocate_id();
                let ap_normal_id = pdf_writer.allocate_id();

                // For checkboxes, radio buttons, we need both on and off appearances
                let ap_down_id = match field.field_type {
                    FormFieldType::CheckBox | FormFieldType::RadioButton => {
                        Some(pdf_writer.allocate_id())
                    }
                    _ => None,
                };

                // For radio groups, allocate IDs for each button's widget
                let radio_widget_ids: Vec<(ObjectId, ObjectId, ObjectId)> = if field.field_type == FormFieldType::RadioButton {
                    field.radio_buttons.iter().map(|_| {
                        let widget_id = pdf_writer.allocate_id();
                        let ap_on = pdf_writer.allocate_id();
                        let ap_off = pdf_writer.allocate_id();
                        (widget_id, ap_on, ap_off)
                    }).collect()
                } else {
                    Vec::new()
                };

                page_field_ids.push(FormFieldIds {
                    field_id,
                    ap_normal_id,
                    ap_down_id,
                    radio_widget_ids,
                });
            }
            form_field_ids.push(page_field_ids);
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

        // Add AcroForm reference if forms exist
        if let Some(acroform_id) = acroform_id {
            catalog.set("AcroForm", Object::Reference(acroform_id));
        }

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

            // Annotations (form fields) - Note: For radio groups, we add each button widget
            let page_form_ids = &form_field_ids[i];
            if !page_form_ids.is_empty() {
                let mut annots = PdfArray::new();
                for field_ids in page_form_ids {
                    if !field_ids.radio_widget_ids.is_empty() {
                        // Radio group: add each button widget as annotation
                        for (widget_id, _, _) in &field_ids.radio_widget_ids {
                            annots.push(Object::Reference(*widget_id));
                        }
                    } else {
                        // Regular field: field itself is the widget
                        annots.push(Object::Reference(field_ids.field_id));
                    }
                }
                page_dict.set("Annots", Object::Array(annots));
            }

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

            // Write form field dictionaries and appearances for this page
            let page_form_ids = &form_field_ids[i];
            for (j, field_ids) in page_form_ids.iter().enumerate() {
                let field = &page.form_fields[j];
                write_form_field(&mut pdf_writer, field, field_ids, page_id)?;
            }
        }

        // Write AcroForm dictionary if forms exist
        if let Some(acroform_id) = acroform_id {
            let mut acroform = PdfDictionary::new();

            // Collect all field references
            let mut fields = PdfArray::new();
            for page_field_ids in &form_field_ids {
                for field_ids in page_field_ids {
                    fields.push(Object::Reference(field_ids.field_id));
                }
            }
            acroform.set("Fields", Object::Array(fields));

            // Default appearance string (DA)
            acroform.set("DA", Object::String(PdfString::literal("/Helv 12 Tf 0 g")));

            // Default resources for form fields
            let mut dr = PdfDictionary::new();
            let mut font_dict = PdfDictionary::new();

            // Add Helvetica as default form font
            let mut helv = PdfDictionary::new();
            helv.set("Type", Object::Name(PdfName::new_unchecked("Font")));
            helv.set("Subtype", Object::Name(PdfName::new_unchecked("Type1")));
            helv.set("BaseFont", Object::Name(PdfName::new_unchecked("Helvetica")));
            helv.set("Encoding", Object::Name(PdfName::new_unchecked("WinAnsiEncoding")));
            font_dict.set("Helv", Object::Dictionary(helv));

            // Add ZapfDingbats for checkmarks
            let mut zadb = PdfDictionary::new();
            zadb.set("Type", Object::Name(PdfName::new_unchecked("Font")));
            zadb.set("Subtype", Object::Name(PdfName::new_unchecked("Type1")));
            zadb.set("BaseFont", Object::Name(PdfName::new_unchecked("ZapfDingbats")));
            font_dict.set("ZaDb", Object::Dictionary(zadb));

            dr.set("Font", Object::Dictionary(font_dict));
            acroform.set("DR", Object::Dictionary(dr));

            // NeedAppearances flag - let viewer generate appearances if needed
            acroform.set("NeedAppearances", Object::Boolean(false));

            pdf_writer.write_object_with_id(acroform_id, &Object::Dictionary(acroform))?;
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

/// Helper struct for tracking form field object IDs.
struct FormFieldIds {
    field_id: ObjectId,
    ap_normal_id: ObjectId,
    ap_down_id: Option<ObjectId>,
    radio_widget_ids: Vec<(ObjectId, ObjectId, ObjectId)>, // (widget_id, ap_on, ap_off)
}

/// Writes a form field and its appearances to the PDF.
fn write_form_field<W: Write>(
    pdf_writer: &mut PdfWriter<W>,
    field: &FormField,
    ids: &FormFieldIds,
    page_id: ObjectId,
) -> PdfResult<()> {
    match field.field_type {
        FormFieldType::Text => {
            write_text_field(pdf_writer, field, ids, page_id)?;
        }
        FormFieldType::CheckBox => {
            write_checkbox(pdf_writer, field, ids, page_id)?;
        }
        FormFieldType::RadioButton => {
            write_radio_group(pdf_writer, field, ids, page_id)?;
        }
        FormFieldType::ComboBox => {
            write_combobox(pdf_writer, field, ids, page_id)?;
        }
        FormFieldType::ListBox => {
            write_listbox(pdf_writer, field, ids, page_id)?;
        }
        FormFieldType::PushButton => {
            write_pushbutton(pdf_writer, field, ids, page_id)?;
        }
    }

    Ok(())
}

/// Writes a text field.
fn write_text_field<W: Write>(
    pdf_writer: &mut PdfWriter<W>,
    field: &FormField,
    ids: &FormFieldIds,
    page_id: ObjectId,
) -> PdfResult<()> {
    let mut dict = PdfDictionary::new();

    // Required entries
    dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
    dict.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
    dict.set("FT", Object::Name(PdfName::new_unchecked("Tx")));
    dict.set("T", Object::String(PdfString::literal(&field.name)));
    dict.set("P", Object::Reference(page_id));

    // Rectangle
    let rect_array = rect_to_array(&field.rect);
    dict.set("Rect", Object::Array(rect_array));

    // Flags
    dict.set("Ff", Object::Integer(field.flags.bits() as i64));

    // Field flags (annotation)
    dict.set("F", Object::Integer(4)); // Print flag

    // Default value
    if let Some(ref dv) = field.default_value {
        dict.set("DV", Object::String(PdfString::literal(dv)));
    }

    // Current value
    if let Some(ref v) = field.value {
        dict.set("V", Object::String(PdfString::literal(v)));
    }

    // Max length
    if let Some(max_len) = field.max_length {
        dict.set("MaxLen", Object::Integer(max_len as i64));
    }

    // Default appearance
    let da = format!("/Helv {} Tf {} g",
        field.font_size,
        if let crate::color::Color::Gray(g) = &field.text_color { g.level } else { 0.0 }
    );
    dict.set("DA", Object::String(PdfString::literal(da)));

    // Border style
    let mut bs = PdfDictionary::new();
    bs.set("Type", Object::Name(PdfName::new_unchecked("Border")));
    bs.set("W", Object::Real(field.border_width));
    bs.set("S", Object::Name(PdfName::new_unchecked(field.border_style.pdf_code())));
    dict.set("BS", Object::Dictionary(bs));

    // Appearance characteristics
    let mut mk = PdfDictionary::new();
    if let Some(ref bg) = field.background_color {
        mk.set("BG", color_to_array(bg));
    }
    if let Some(ref bc) = field.border_color {
        mk.set("BC", color_to_array(bc));
    }
    if !mk.is_empty() {
        dict.set("MK", Object::Dictionary(mk));
    }

    // Build and set appearance
    let builder = AppearanceBuilder::new(field.rect.with_origin())
        .background_color(field.background_color.unwrap_or(crate::color::Color::WHITE))
        .border_color(field.border_color.unwrap_or(crate::color::Color::BLACK))
        .border_style(field.border_style)
        .border_width(field.border_width);

    let ap_stream = builder.build_text_appearance(
        field.value.as_deref(),
        "Helv",
        field.font_size,
        field.text_color,
    );

    // Create appearance stream
    let mut ap_dict = PdfDictionary::new();
    ap_dict.set("N", Object::Reference(ids.ap_normal_id));
    dict.set("AP", Object::Dictionary(ap_dict));

    // Write field dictionary
    pdf_writer.write_object_with_id(ids.field_id, &Object::Dictionary(dict))?;

    // Write appearance stream
    let stream = create_appearance_stream(&ap_stream, &field.rect.with_origin());
    pdf_writer.write_object_with_id(ids.ap_normal_id, &Object::Stream(stream))?;

    Ok(())
}

/// Writes a checkbox field.
fn write_checkbox<W: Write>(
    pdf_writer: &mut PdfWriter<W>,
    field: &FormField,
    ids: &FormFieldIds,
    page_id: ObjectId,
) -> PdfResult<()> {
    let mut dict = PdfDictionary::new();

    // Required entries
    dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
    dict.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
    dict.set("FT", Object::Name(PdfName::new_unchecked("Btn")));
    dict.set("T", Object::String(PdfString::literal(&field.name)));
    dict.set("P", Object::Reference(page_id));

    // Rectangle
    let rect_array = rect_to_array(&field.rect);
    dict.set("Rect", Object::Array(rect_array));

    // Field flags (annotation)
    dict.set("F", Object::Integer(4)); // Print flag

    // Value - checked state
    let export_value = field.export_value.as_deref().unwrap_or("Yes");
    if field.checked {
        dict.set("V", Object::Name(PdfName::new_unchecked(export_value)));
        dict.set("AS", Object::Name(PdfName::new_unchecked(export_value)));
    } else {
        dict.set("V", Object::Name(PdfName::new_unchecked("Off")));
        dict.set("AS", Object::Name(PdfName::new_unchecked("Off")));
    }

    // Border style
    let mut bs = PdfDictionary::new();
    bs.set("Type", Object::Name(PdfName::new_unchecked("Border")));
    bs.set("W", Object::Real(field.border_width));
    bs.set("S", Object::Name(PdfName::new_unchecked(field.border_style.pdf_code())));
    dict.set("BS", Object::Dictionary(bs));

    // Appearance characteristics
    let mut mk = PdfDictionary::new();
    if let Some(ref bg) = field.background_color {
        mk.set("BG", color_to_array(bg));
    }
    if let Some(ref bc) = field.border_color {
        mk.set("BC", color_to_array(bc));
    }
    mk.set("CA", Object::String(PdfString::literal("4"))); // Checkmark character
    dict.set("MK", Object::Dictionary(mk));

    // Build appearances
    let builder = AppearanceBuilder::new(field.rect.with_origin())
        .background_color(field.background_color.unwrap_or(crate::color::Color::WHITE))
        .border_color(field.border_color.unwrap_or(crate::color::Color::BLACK))
        .border_style(field.border_style)
        .border_width(field.border_width);

    let checked_stream = builder.build_checkbox_checked(field.text_color);
    let unchecked_stream = builder.build_checkbox_unchecked();

    // Create appearance dictionary with both states
    let mut ap_dict = PdfDictionary::new();
    let mut n_dict = PdfDictionary::new();
    n_dict.set(export_value, Object::Reference(ids.ap_normal_id));
    if let Some(ap_down_id) = ids.ap_down_id {
        n_dict.set("Off", Object::Reference(ap_down_id));
    }
    ap_dict.set("N", Object::Dictionary(n_dict));
    dict.set("AP", Object::Dictionary(ap_dict));

    // Write field dictionary
    pdf_writer.write_object_with_id(ids.field_id, &Object::Dictionary(dict))?;

    // Write appearance streams
    let stream_checked = create_appearance_stream(&checked_stream, &field.rect.with_origin());
    pdf_writer.write_object_with_id(ids.ap_normal_id, &Object::Stream(stream_checked))?;

    if let Some(ap_down_id) = ids.ap_down_id {
        let stream_unchecked = create_appearance_stream(&unchecked_stream, &field.rect.with_origin());
        pdf_writer.write_object_with_id(ap_down_id, &Object::Stream(stream_unchecked))?;
    }

    Ok(())
}

/// Writes a radio button group.
fn write_radio_group<W: Write>(
    pdf_writer: &mut PdfWriter<W>,
    field: &FormField,
    ids: &FormFieldIds,
    page_id: ObjectId,
) -> PdfResult<()> {
    // Parent field dictionary (the group)
    let mut parent_dict = PdfDictionary::new();
    parent_dict.set("FT", Object::Name(PdfName::new_unchecked("Btn")));
    parent_dict.set("T", Object::String(PdfString::literal(&field.name)));

    // Radio button flags
    let flags = field.flags.bits() as i64;
    parent_dict.set("Ff", Object::Integer(flags));

    // Kids array (references to widget annotations)
    let mut kids = PdfArray::new();
    for (widget_id, _, _) in &ids.radio_widget_ids {
        kids.push(Object::Reference(*widget_id));
    }
    parent_dict.set("Kids", Object::Array(kids));

    // Selected value
    let selected_idx = field.selected_indices.first().copied();
    if let Some(idx) = selected_idx {
        if idx < field.radio_buttons.len() {
            let value = field.radio_buttons[idx].get_export_value();
            parent_dict.set("V", Object::Name(PdfName::new_unchecked(value)));
        }
    } else {
        parent_dict.set("V", Object::Name(PdfName::new_unchecked("Off")));
    }

    // Write parent dictionary
    pdf_writer.write_object_with_id(ids.field_id, &Object::Dictionary(parent_dict))?;

    // Write each button widget
    for (i, ((widget_id, ap_on_id, ap_off_id), button)) in ids.radio_widget_ids.iter().zip(field.radio_buttons.iter()).enumerate() {
        let mut widget_dict = PdfDictionary::new();

        widget_dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        widget_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
        widget_dict.set("Parent", Object::Reference(ids.field_id));
        widget_dict.set("P", Object::Reference(page_id));

        // Rectangle
        let button_rect = button.get_rect();
        let rect_array = rect_to_array(&button_rect);
        widget_dict.set("Rect", Object::Array(rect_array));

        // Annotation flags
        widget_dict.set("F", Object::Integer(4)); // Print flag

        // Appearance state
        let export_value = button.get_export_value();
        if selected_idx == Some(i) {
            widget_dict.set("AS", Object::Name(PdfName::new_unchecked(export_value)));
        } else {
            widget_dict.set("AS", Object::Name(PdfName::new_unchecked("Off")));
        }

        // Border style
        let mut bs = PdfDictionary::new();
        bs.set("Type", Object::Name(PdfName::new_unchecked("Border")));
        bs.set("W", Object::Real(field.border_width));
        bs.set("S", Object::Name(PdfName::new_unchecked(field.border_style.pdf_code())));
        widget_dict.set("BS", Object::Dictionary(bs));

        // Appearance characteristics
        let mut mk = PdfDictionary::new();
        if let Some(ref bg) = field.background_color {
            mk.set("BG", color_to_array(bg));
        }
        if let Some(ref bc) = field.border_color {
            mk.set("BC", color_to_array(bc));
        }
        widget_dict.set("MK", Object::Dictionary(mk));

        // Build appearances
        let builder = AppearanceBuilder::new(button_rect.with_origin())
            .background_color(field.background_color.unwrap_or(crate::color::Color::WHITE))
            .border_color(field.border_color.unwrap_or(crate::color::Color::BLACK))
            .border_style(field.border_style)
            .border_width(field.border_width);

        let selected_stream = builder.build_radio_selected(field.text_color);
        let unselected_stream = builder.build_radio_unselected();

        // Create appearance dictionary
        let mut ap_dict = PdfDictionary::new();
        let mut n_dict = PdfDictionary::new();
        n_dict.set(export_value, Object::Reference(*ap_on_id));
        n_dict.set("Off", Object::Reference(*ap_off_id));
        ap_dict.set("N", Object::Dictionary(n_dict));
        widget_dict.set("AP", Object::Dictionary(ap_dict));

        // Write widget dictionary
        pdf_writer.write_object_with_id(*widget_id, &Object::Dictionary(widget_dict))?;

        // Write appearance streams
        let stream_on = create_appearance_stream(&selected_stream, &button_rect.with_origin());
        pdf_writer.write_object_with_id(*ap_on_id, &Object::Stream(stream_on))?;

        let stream_off = create_appearance_stream(&unselected_stream, &button_rect.with_origin());
        pdf_writer.write_object_with_id(*ap_off_id, &Object::Stream(stream_off))?;
    }

    Ok(())
}

/// Writes a combo box field.
fn write_combobox<W: Write>(
    pdf_writer: &mut PdfWriter<W>,
    field: &FormField,
    ids: &FormFieldIds,
    page_id: ObjectId,
) -> PdfResult<()> {
    let mut dict = PdfDictionary::new();

    // Required entries
    dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
    dict.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
    dict.set("FT", Object::Name(PdfName::new_unchecked("Ch")));
    dict.set("T", Object::String(PdfString::literal(&field.name)));
    dict.set("P", Object::Reference(page_id));

    // Rectangle
    let rect_array = rect_to_array(&field.rect);
    dict.set("Rect", Object::Array(rect_array));

    // Field flags (Combo flag is set)
    dict.set("Ff", Object::Integer(field.flags.bits() as i64));

    // Annotation flags
    dict.set("F", Object::Integer(4)); // Print flag

    // Options
    let mut opt = PdfArray::new();
    for option in &field.options {
        opt.push(Object::String(PdfString::literal(option)));
    }
    dict.set("Opt", Object::Array(opt));

    // Selected value
    if let Some(&idx) = field.selected_indices.first() {
        if idx < field.options.len() {
            dict.set("V", Object::String(PdfString::literal(&field.options[idx])));
            dict.set("I", Object::Array({
                let mut arr = PdfArray::new();
                arr.push(Object::Integer(idx as i64));
                arr
            }));
        }
    }

    // Default appearance
    let da = format!("/Helv {} Tf {} g",
        field.font_size,
        if let crate::color::Color::Gray(g) = &field.text_color { g.level } else { 0.0 }
    );
    dict.set("DA", Object::String(PdfString::literal(da)));

    // Border style
    let mut bs = PdfDictionary::new();
    bs.set("Type", Object::Name(PdfName::new_unchecked("Border")));
    bs.set("W", Object::Real(field.border_width));
    bs.set("S", Object::Name(PdfName::new_unchecked(field.border_style.pdf_code())));
    dict.set("BS", Object::Dictionary(bs));

    // Appearance characteristics
    let mut mk = PdfDictionary::new();
    if let Some(ref bg) = field.background_color {
        mk.set("BG", color_to_array(bg));
    }
    if let Some(ref bc) = field.border_color {
        mk.set("BC", color_to_array(bc));
    }
    dict.set("MK", Object::Dictionary(mk));

    // Build appearance
    let builder = AppearanceBuilder::new(field.rect.with_origin())
        .background_color(field.background_color.unwrap_or(crate::color::Color::WHITE))
        .border_color(field.border_color.unwrap_or(crate::color::Color::BLACK))
        .border_style(field.border_style)
        .border_width(field.border_width);

    let selected_text = field.selected_indices.first()
        .and_then(|&idx| field.options.get(idx))
        .map(|s| s.as_str());

    let ap_stream = builder.build_combobox_appearance(
        selected_text,
        "Helv",
        field.font_size,
        field.text_color,
    );

    // Create appearance dictionary
    let mut ap_dict = PdfDictionary::new();
    ap_dict.set("N", Object::Reference(ids.ap_normal_id));
    dict.set("AP", Object::Dictionary(ap_dict));

    // Write field dictionary
    pdf_writer.write_object_with_id(ids.field_id, &Object::Dictionary(dict))?;

    // Write appearance stream
    let stream = create_appearance_stream(&ap_stream, &field.rect.with_origin());
    pdf_writer.write_object_with_id(ids.ap_normal_id, &Object::Stream(stream))?;

    Ok(())
}

/// Writes a list box field.
fn write_listbox<W: Write>(
    pdf_writer: &mut PdfWriter<W>,
    field: &FormField,
    ids: &FormFieldIds,
    page_id: ObjectId,
) -> PdfResult<()> {
    let mut dict = PdfDictionary::new();

    // Required entries
    dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
    dict.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
    dict.set("FT", Object::Name(PdfName::new_unchecked("Ch")));
    dict.set("T", Object::String(PdfString::literal(&field.name)));
    dict.set("P", Object::Reference(page_id));

    // Rectangle
    let rect_array = rect_to_array(&field.rect);
    dict.set("Rect", Object::Array(rect_array));

    // Field flags (no Combo flag for list box)
    dict.set("Ff", Object::Integer(field.flags.bits() as i64));

    // Annotation flags
    dict.set("F", Object::Integer(4)); // Print flag

    // Options
    let mut opt = PdfArray::new();
    for option in &field.options {
        opt.push(Object::String(PdfString::literal(option)));
    }
    dict.set("Opt", Object::Array(opt));

    // Selected values
    if !field.selected_indices.is_empty() {
        if field.selected_indices.len() == 1 {
            let idx = field.selected_indices[0];
            if idx < field.options.len() {
                dict.set("V", Object::String(PdfString::literal(&field.options[idx])));
            }
        } else {
            let mut v = PdfArray::new();
            for &idx in &field.selected_indices {
                if idx < field.options.len() {
                    v.push(Object::String(PdfString::literal(&field.options[idx])));
                }
            }
            dict.set("V", Object::Array(v));
        }

        let mut i = PdfArray::new();
        for &idx in &field.selected_indices {
            i.push(Object::Integer(idx as i64));
        }
        dict.set("I", Object::Array(i));
    }

    // Default appearance
    let da = format!("/Helv {} Tf {} g",
        field.font_size,
        if let crate::color::Color::Gray(g) = &field.text_color { g.level } else { 0.0 }
    );
    dict.set("DA", Object::String(PdfString::literal(da)));

    // Border style
    let mut bs = PdfDictionary::new();
    bs.set("Type", Object::Name(PdfName::new_unchecked("Border")));
    bs.set("W", Object::Real(field.border_width));
    bs.set("S", Object::Name(PdfName::new_unchecked(field.border_style.pdf_code())));
    dict.set("BS", Object::Dictionary(bs));

    // Appearance characteristics
    let mut mk = PdfDictionary::new();
    if let Some(ref bg) = field.background_color {
        mk.set("BG", color_to_array(bg));
    }
    if let Some(ref bc) = field.border_color {
        mk.set("BC", color_to_array(bc));
    }
    dict.set("MK", Object::Dictionary(mk));

    // Build appearance
    let builder = AppearanceBuilder::new(field.rect.with_origin())
        .background_color(field.background_color.unwrap_or(crate::color::Color::WHITE))
        .border_color(field.border_color.unwrap_or(crate::color::Color::BLACK))
        .border_style(field.border_style)
        .border_width(field.border_width);

    let ap_stream = builder.build_listbox_appearance(
        &field.options,
        &field.selected_indices,
        "Helv",
        field.font_size,
        field.text_color,
    );

    // Create appearance dictionary
    let mut ap_dict = PdfDictionary::new();
    ap_dict.set("N", Object::Reference(ids.ap_normal_id));
    dict.set("AP", Object::Dictionary(ap_dict));

    // Write field dictionary
    pdf_writer.write_object_with_id(ids.field_id, &Object::Dictionary(dict))?;

    // Write appearance stream
    let stream = create_appearance_stream(&ap_stream, &field.rect.with_origin());
    pdf_writer.write_object_with_id(ids.ap_normal_id, &Object::Stream(stream))?;

    Ok(())
}

/// Writes a push button field.
fn write_pushbutton<W: Write>(
    pdf_writer: &mut PdfWriter<W>,
    field: &FormField,
    ids: &FormFieldIds,
    page_id: ObjectId,
) -> PdfResult<()> {
    let mut dict = PdfDictionary::new();

    // Required entries
    dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
    dict.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
    dict.set("FT", Object::Name(PdfName::new_unchecked("Btn")));
    dict.set("T", Object::String(PdfString::literal(&field.name)));
    dict.set("P", Object::Reference(page_id));

    // Rectangle
    let rect_array = rect_to_array(&field.rect);
    dict.set("Rect", Object::Array(rect_array));

    // Field flags (Push button flag)
    dict.set("Ff", Object::Integer(field.flags.bits() as i64));

    // Annotation flags
    dict.set("F", Object::Integer(4)); // Print flag

    // Border style
    let mut bs = PdfDictionary::new();
    bs.set("Type", Object::Name(PdfName::new_unchecked("Border")));
    bs.set("W", Object::Real(field.border_width));
    bs.set("S", Object::Name(PdfName::new_unchecked(field.border_style.pdf_code())));
    dict.set("BS", Object::Dictionary(bs));

    // Appearance characteristics
    let mut mk = PdfDictionary::new();
    if let Some(ref bg) = field.background_color {
        mk.set("BG", color_to_array(bg));
    }
    if let Some(ref bc) = field.border_color {
        mk.set("BC", color_to_array(bc));
    }
    if let Some(ref caption) = field.caption {
        mk.set("CA", Object::String(PdfString::literal(caption)));
    }
    dict.set("MK", Object::Dictionary(mk));

    // Build appearance
    let builder = AppearanceBuilder::new(field.rect.with_origin())
        .background_color(field.background_color.unwrap_or(crate::color::Color::gray(0.9)))
        .border_color(field.border_color.unwrap_or(crate::color::Color::BLACK))
        .border_style(field.border_style)
        .border_width(field.border_width);

    let caption = field.caption.as_deref().unwrap_or("Button");
    let ap_stream = builder.build_button_appearance(
        caption,
        "Helv",
        field.font_size,
        field.text_color,
    );

    // Create appearance dictionary
    let mut ap_dict = PdfDictionary::new();
    ap_dict.set("N", Object::Reference(ids.ap_normal_id));
    dict.set("AP", Object::Dictionary(ap_dict));

    // Write field dictionary
    pdf_writer.write_object_with_id(ids.field_id, &Object::Dictionary(dict))?;

    // Write appearance stream
    let stream = create_appearance_stream(&ap_stream, &field.rect.with_origin());
    pdf_writer.write_object_with_id(ids.ap_normal_id, &Object::Stream(stream))?;

    Ok(())
}

/// Converts a rectangle to a PDF array.
fn rect_to_array(rect: &crate::types::Rectangle) -> PdfArray {
    let mut arr = PdfArray::new();
    arr.push(Object::Real(rect.llx));
    arr.push(Object::Real(rect.lly));
    arr.push(Object::Real(rect.urx));
    arr.push(Object::Real(rect.ury));
    arr
}

/// Converts a color to a PDF array.
fn color_to_array(color: &crate::color::Color) -> Object {
    let mut arr = PdfArray::new();
    match color {
        crate::color::Color::Gray(g) => {
            arr.push(Object::Real(g.level));
        }
        crate::color::Color::Rgb(rgb) => {
            arr.push(Object::Real(rgb.r));
            arr.push(Object::Real(rgb.g));
            arr.push(Object::Real(rgb.b));
        }
        crate::color::Color::Cmyk(cmyk) => {
            arr.push(Object::Real(cmyk.c));
            arr.push(Object::Real(cmyk.m));
            arr.push(Object::Real(cmyk.y));
            arr.push(Object::Real(cmyk.k));
        }
    }
    Object::Array(arr)
}

/// Creates an appearance stream from content and bounding box.
fn create_appearance_stream(content: &str, bbox: &crate::types::Rectangle) -> PdfStream {
    let mut dict = PdfDictionary::new();
    dict.set("Type", Object::Name(PdfName::new_unchecked("XObject")));
    dict.set("Subtype", Object::Name(PdfName::new_unchecked("Form")));

    let mut bbox_arr = PdfArray::new();
    bbox_arr.push(Object::Real(0.0));
    bbox_arr.push(Object::Real(0.0));
    bbox_arr.push(Object::Real(bbox.width()));
    bbox_arr.push(Object::Real(bbox.height()));
    dict.set("BBox", Object::Array(bbox_arr));

    // Add Resources with font
    let mut resources = PdfDictionary::new();
    let mut font_dict = PdfDictionary::new();

    let mut helv = PdfDictionary::new();
    helv.set("Type", Object::Name(PdfName::new_unchecked("Font")));
    helv.set("Subtype", Object::Name(PdfName::new_unchecked("Type1")));
    helv.set("BaseFont", Object::Name(PdfName::new_unchecked("Helvetica")));
    font_dict.set("Helv", Object::Dictionary(helv));

    resources.set("Font", Object::Dictionary(font_dict));
    dict.set("Resources", Object::Dictionary(resources));

    let data = content.as_bytes().to_vec();
    dict.set("Length", Object::Integer(data.len() as i64));

    PdfStream::from_raw(dict, data)
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
