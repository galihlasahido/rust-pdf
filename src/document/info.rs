//! PDF Document Information Dictionary.

use crate::object::{Object, PdfDictionary, PdfString};

/// Document metadata (Info dictionary).
#[derive(Debug, Clone, Default)]
pub struct DocumentInfo {
    /// The document's title.
    pub title: Option<String>,
    /// The name of the person who created the document.
    pub author: Option<String>,
    /// The subject of the document.
    pub subject: Option<String>,
    /// Keywords associated with the document.
    pub keywords: Option<String>,
    /// The name of the application that created the original document.
    pub creator: Option<String>,
    /// The name of the application that produced the PDF.
    pub producer: Option<String>,
    /// The date and time the document was created.
    pub creation_date: Option<String>,
    /// The date and time the document was most recently modified.
    pub mod_date: Option<String>,
}

impl DocumentInfo {
    /// Creates a new empty document info.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the document title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the document author.
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Sets the document subject.
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Sets the document keywords.
    pub fn keywords(mut self, keywords: impl Into<String>) -> Self {
        self.keywords = Some(keywords.into());
        self
    }

    /// Sets the creator application name.
    pub fn creator(mut self, creator: impl Into<String>) -> Self {
        self.creator = Some(creator.into());
        self
    }

    /// Sets the producer application name.
    pub fn producer(mut self, producer: impl Into<String>) -> Self {
        self.producer = Some(producer.into());
        self
    }

    /// Sets the creation date (should be in PDF date format).
    pub fn creation_date(mut self, date: impl Into<String>) -> Self {
        self.creation_date = Some(date.into());
        self
    }

    /// Sets the modification date (should be in PDF date format).
    pub fn mod_date(mut self, date: impl Into<String>) -> Self {
        self.mod_date = Some(date.into());
        self
    }

    /// Returns true if all fields are empty.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.author.is_none()
            && self.subject.is_none()
            && self.keywords.is_none()
            && self.creator.is_none()
            && self.producer.is_none()
            && self.creation_date.is_none()
            && self.mod_date.is_none()
    }

    /// Converts the document info to a PDF dictionary.
    pub fn to_dictionary(&self) -> PdfDictionary {
        let mut dict = PdfDictionary::new();

        if let Some(ref title) = self.title {
            dict.set("Title", Object::String(PdfString::literal(title)));
        }
        if let Some(ref author) = self.author {
            dict.set("Author", Object::String(PdfString::literal(author)));
        }
        if let Some(ref subject) = self.subject {
            dict.set("Subject", Object::String(PdfString::literal(subject)));
        }
        if let Some(ref keywords) = self.keywords {
            dict.set("Keywords", Object::String(PdfString::literal(keywords)));
        }
        if let Some(ref creator) = self.creator {
            dict.set("Creator", Object::String(PdfString::literal(creator)));
        }
        if let Some(ref producer) = self.producer {
            dict.set("Producer", Object::String(PdfString::literal(producer)));
        }
        if let Some(ref creation_date) = self.creation_date {
            dict.set(
                "CreationDate",
                Object::String(PdfString::literal(creation_date)),
            );
        }
        if let Some(ref mod_date) = self.mod_date {
            dict.set("ModDate", Object::String(PdfString::literal(mod_date)));
        }

        dict
    }
}

/// Builder for document info with a fluent interface.
pub struct DocumentInfoBuilder {
    info: DocumentInfo,
}

impl DocumentInfoBuilder {
    /// Creates a new document info builder.
    pub fn new() -> Self {
        Self {
            info: DocumentInfo::new(),
        }
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

    /// Builds the document info.
    pub fn build(self) -> DocumentInfo {
        self.info
    }
}

impl Default for DocumentInfoBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_info_builder() {
        let info = DocumentInfoBuilder::new()
            .title("Test Document")
            .author("Test Author")
            .build();

        assert_eq!(info.title, Some("Test Document".to_string()));
        assert_eq!(info.author, Some("Test Author".to_string()));
    }

    #[test]
    fn test_to_dictionary() {
        let info = DocumentInfo::new()
            .title("My PDF")
            .author("John Doe");

        let dict = info.to_dictionary();
        assert!(dict.contains_key("Title"));
        assert!(dict.contains_key("Author"));
        assert!(!dict.contains_key("Subject"));
    }

    #[test]
    fn test_is_empty() {
        let info = DocumentInfo::new();
        assert!(info.is_empty());

        let info_with_title = DocumentInfo::new().title("Test");
        assert!(!info_with_title.is_empty());
    }

    #[test]
    fn test_fluent_interface() {
        let info = DocumentInfo::new()
            .title("Title")
            .author("Author")
            .subject("Subject")
            .keywords("key1, key2")
            .creator("Creator App")
            .producer("rust-pdf");

        assert!(info.title.is_some());
        assert!(info.producer.is_some());
    }
}
