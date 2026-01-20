//! # rust-pdf
//!
//! A Rust library for creating PDF documents.
//!
//! ## Features
//!
//! - **PDF Creation**: Generate valid PDF 1.7 and 2.0 documents
//! - **Text Support**: Use all 14 standard PDF fonts
//! - **Graphics**: Draw shapes, lines, and apply colors
//! - **Builder Pattern**: Fluent API for document construction
//!
//! ## Quick Start
//!
//! ```rust
//! use rust_pdf::prelude::*;
//!
//! // Create a simple PDF with text
//! let content = ContentBuilder::new()
//!     .text("F1", 24.0, 72.0, 750.0, "Hello, World!");
//!
//! let page = PageBuilder::a4()
//!     .font("F1", Standard14Font::Helvetica)
//!     .content(content)
//!     .build();
//!
//! let doc = DocumentBuilder::new()
//!     .title("My Document")
//!     .author("rust-pdf")
//!     .page(page)
//!     .build()
//!     .unwrap();
//!
//! // Save to file
//! // doc.save_to_file("output.pdf").unwrap();
//!
//! // Or get bytes
//! let _bytes = doc.save_to_bytes().unwrap();
//! ```
//!
//! ## Drawing Graphics
//!
//! ```rust
//! use rust_pdf::prelude::*;
//!
//! let content = ContentBuilder::new()
//!     .save_state()
//!     .fill_color(Color::rgb(1.0, 0.0, 0.0))
//!     .rect(100.0, 100.0, 200.0, 150.0)
//!     .fill()
//!     .restore_state()
//!     .save_state()
//!     .stroke_color(Color::BLUE)
//!     .line_width(2.0)
//!     .move_to(100.0, 300.0)
//!     .line_to(300.0, 500.0)
//!     .stroke()
//!     .restore_state();
//! ```

// Module declarations
pub mod color;
pub mod content;
pub mod document;
#[cfg(feature = "encryption")]
pub mod encryption;
pub mod error;
pub mod font;
pub mod forms;
#[cfg(feature = "images")]
pub mod image;
pub mod object;
pub mod page;
#[cfg(feature = "parser")]
pub mod parser;
#[cfg(feature = "signatures")]
pub mod signatures;
pub mod types;
pub mod writer;

// FFI interface for C/dynamic library usage
pub mod ffi;

// Re-export commonly used types
pub use color::{CmykColor, Color, GrayColor, RgbColor};
pub use content::{ContentBuilder, GraphicsBuilder, Operator, TextBuilder, TextElement};
pub use document::{Document, DocumentBuilder, DocumentInfo, PdfVersion};
pub use error::{ContentError, DocumentError, FormError, ObjectError, PdfError, PdfResult, WriterError};
#[cfg(feature = "compression")]
pub use error::CompressionError;
#[cfg(feature = "images")]
pub use error::ImageError;
#[cfg(feature = "parser")]
pub use error::ParserError;
#[cfg(feature = "encryption")]
pub use error::EncryptionError;
#[cfg(feature = "signatures")]
pub use error::SignatureError;
#[cfg(feature = "encryption")]
pub use encryption::{EncryptionConfig, EncryptionHandler, Permissions};
#[cfg(feature = "signatures")]
pub use signatures::{ByteRange, Certificate, DocumentSigner, PrivateKey, SignatureAlgorithm, SignatureConfig, SignatureInfo};
pub use font::{Font, FontMetrics, Standard14Font};
pub use forms::{
    AppearanceBuilder, BorderStyle, CheckBox, ComboBox, FieldFlags, FormField, FormFieldTrait,
    FormFieldType, ListBox, PushButton, RadioButton, RadioGroup, TextField,
};
#[cfg(feature = "images")]
pub use image::{ColorSpace, Image, ImageFilter, ImageXObject};
#[cfg(feature = "parser")]
pub use parser::{PdfReader, Trailer, XrefEntry, XrefTable};
pub use object::{
    DictionaryBuilder, Object, PdfArray, PdfDictionary, PdfName, PdfStream, PdfString,
    StreamBuilder,
};
pub use page::{Page, PageBuilder};
pub use types::{Matrix, ObjectId, Rectangle};
pub use writer::PdfWriter;

/// Prelude module for convenient imports.
///
/// Use `use rust_pdf::prelude::*;` to import all commonly used types.
pub mod prelude {
    pub use crate::color::{CmykColor, Color, GrayColor, RgbColor};
    pub use crate::content::{
        kern, text, ContentBuilder, GraphicsBuilder, Operator, TextBuilder, TextElement,
    };
    pub use crate::document::{Document, DocumentBuilder, DocumentInfo, PdfVersion};
    pub use crate::error::{PdfError, PdfResult};
    #[cfg(feature = "compression")]
    pub use crate::error::CompressionError;
    #[cfg(feature = "images")]
    pub use crate::error::ImageError;
    #[cfg(feature = "parser")]
    pub use crate::error::ParserError;
    #[cfg(feature = "encryption")]
    pub use crate::error::EncryptionError;
    #[cfg(feature = "signatures")]
    pub use crate::error::SignatureError;
    #[cfg(feature = "encryption")]
    pub use crate::encryption::{EncryptionConfig, EncryptionHandler, Permissions};
    #[cfg(feature = "signatures")]
    pub use crate::signatures::{ByteRange, Certificate, DocumentSigner, PrivateKey, SignatureAlgorithm, SignatureConfig, SignatureInfo};
    pub use crate::font::{Font, FontMetrics, Standard14Font};
    pub use crate::forms::{
        AppearanceBuilder, BorderStyle, CheckBox, ComboBox, FieldFlags, FormField,
        FormFieldTrait, FormFieldType, ListBox, PushButton, RadioButton, RadioGroup, TextField,
    };
    #[cfg(feature = "images")]
    pub use crate::image::{ColorSpace, Image, ImageFilter, ImageXObject};
    pub use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream, PdfString};
    #[cfg(feature = "parser")]
    pub use crate::parser::PdfReader;
    pub use crate::page::{Page, PageBuilder};
    pub use crate::types::{Matrix, ObjectId, Rectangle};
}

#[cfg(test)]
mod tests {
    use super::prelude::*;

    #[test]
    fn test_simple_pdf_creation() {
        let content = ContentBuilder::new()
            .text("F1", 24.0, 72.0, 750.0, "Hello, World!");

        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(content)
            .build();

        let doc = DocumentBuilder::new()
            .title("Test Document")
            .page(page)
            .build()
            .unwrap();

        let bytes = doc.save_to_bytes().unwrap();
        let content = String::from_utf8_lossy(&bytes);

        assert!(content.starts_with("%PDF-1.7"));
        assert!(content.contains("/Type /Catalog"));
        assert!(content.contains("/Type /Page"));
        assert!(content.contains("Hello, World!"));
        assert!(content.contains("%%EOF"));
    }

    #[test]
    fn test_multi_page_pdf() {
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

        assert_eq!(doc.page_count(), 2);

        let bytes = doc.save_to_bytes().unwrap();
        let content = String::from_utf8_lossy(&bytes);
        assert!(content.contains("/Count 2"));
    }

    #[test]
    fn test_graphics_content() {
        let graphics = GraphicsBuilder::new()
            .fill_color(Color::RED)
            .rect(100.0, 100.0, 200.0, 150.0)
            .fill()
            .stroke_color(Color::BLUE)
            .line_width(2.0)
            .move_to(50.0, 50.0)
            .line_to(250.0, 250.0)
            .stroke();

        let content = ContentBuilder::new().graphics(graphics);

        let page = PageBuilder::a4().content(content).build();

        let doc = DocumentBuilder::new().page(page).build().unwrap();

        let bytes = doc.save_to_bytes().unwrap();
        let content = String::from_utf8_lossy(&bytes);

        assert!(content.contains("1 0 0 rg")); // Red fill
        assert!(content.contains("100 100 200 150 re")); // Rectangle
        assert!(content.contains("0 0 1 RG")); // Blue stroke
    }

    #[test]
    fn test_document_info() {
        let page = PageBuilder::a4().build();

        let doc = DocumentBuilder::new()
            .version(PdfVersion::V1_7)
            .title("My PDF")
            .author("Test Author")
            .subject("Test Subject")
            .keywords("rust, pdf, test")
            .creator("rust-pdf")
            .producer("rust-pdf library")
            .page(page)
            .build()
            .unwrap();

        let bytes = doc.save_to_bytes().unwrap();
        let content = String::from_utf8_lossy(&bytes);

        assert!(content.contains("/Title (My PDF)"));
        assert!(content.contains("/Author (Test Author)"));
    }

    #[test]
    fn test_all_standard_fonts() {
        for (i, font) in Standard14Font::all().iter().enumerate() {
            let font_name = format!("F{}", i + 1);
            let content = ContentBuilder::new()
                .text(&font_name, 12.0, 72.0, 700.0 - (i as f64 * 20.0), font.postscript_name());

            let page = PageBuilder::a4()
                .font(&font_name, Font::from(*font))
                .content(content)
                .build();

            let doc = DocumentBuilder::new().page(page).build().unwrap();
            let bytes = doc.save_to_bytes().unwrap();

            let pdf_content = String::from_utf8_lossy(&bytes);
            assert!(pdf_content.contains(font.postscript_name()));
        }
    }

    #[test]
    fn test_color_types() {
        let content = ContentBuilder::new()
            .fill_color(Color::gray(0.5))
            .rect(50.0, 700.0, 100.0, 50.0)
            .fill()
            .fill_color(Color::rgb(1.0, 0.5, 0.0))
            .rect(50.0, 600.0, 100.0, 50.0)
            .fill()
            .fill_color(Color::cmyk(1.0, 0.0, 1.0, 0.0))
            .rect(50.0, 500.0, 100.0, 50.0)
            .fill();

        let page = PageBuilder::a4().content(content).build();
        let doc = DocumentBuilder::new().page(page).build().unwrap();

        let bytes = doc.save_to_bytes().unwrap();
        let pdf_content = String::from_utf8_lossy(&bytes);

        assert!(pdf_content.contains("0.5 g")); // Gray
        assert!(pdf_content.contains("1 0.5 0 rg")); // RGB
        assert!(pdf_content.contains("1 0 1 0 k")); // CMYK
    }
}
