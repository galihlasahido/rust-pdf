//! PDF Name object.

use crate::error::ObjectError;

/// A PDF name object (e.g., /Type, /Page, /Font).
///
/// Names in PDF start with a forward slash and can contain any characters
/// except whitespace and delimiters. Special characters are encoded using
/// the #xx hexadecimal notation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PdfName(String);

impl PdfName {
    /// Creates a new PDF name from a string.
    ///
    /// The input should not include the leading slash.
    /// Invalid characters will cause an error.
    pub fn new(name: impl Into<String>) -> Result<Self, ObjectError> {
        let name = name.into();
        if name.is_empty() {
            return Err(ObjectError::InvalidName("Name cannot be empty".to_string()));
        }
        // Validate: no null bytes
        if name.contains('\0') {
            return Err(ObjectError::InvalidName(
                "Name cannot contain null bytes".to_string(),
            ));
        }
        Ok(Self(name))
    }

    /// Creates a PDF name without validation (use for known-good names).
    ///
    /// # Safety
    /// The caller must ensure the name is valid.
    pub fn new_unchecked(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Returns the raw name without the leading slash.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Serializes the name to PDF format with proper escaping.
    ///
    /// Characters that need escaping (codes < 33, > 126, #, and delimiters)
    /// are encoded as #xx where xx is the hex code.
    pub fn to_pdf_string(&self) -> String {
        let mut result = String::with_capacity(self.0.len() + 10);
        result.push('/');

        for byte in self.0.bytes() {
            if Self::needs_escape(byte) {
                result.push('#');
                result.push_str(&format!("{:02X}", byte));
            } else {
                result.push(byte as char);
            }
        }

        result
    }

    /// Checks if a byte needs to be escaped in a PDF name.
    fn needs_escape(byte: u8) -> bool {
        // Escape: control chars, whitespace, delimiters, and #
        !(33..=126).contains(&byte)
            || byte == b'#'
            || byte == b'/'
            || byte == b'%'
            || byte == b'('
            || byte == b')'
            || byte == b'<'
            || byte == b'>'
            || byte == b'['
            || byte == b']'
            || byte == b'{'
            || byte == b'}'
    }

    // Common PDF names as constants
    pub const TYPE: PdfName = PdfName(String::new());
    pub const PAGE: PdfName = PdfName(String::new());
    pub const PAGES: PdfName = PdfName(String::new());
    pub const CATALOG: PdfName = PdfName(String::new());
}

// Pre-defined common names
impl PdfName {
    pub fn type_name() -> Self {
        Self::new_unchecked("Type")
    }

    pub fn page() -> Self {
        Self::new_unchecked("Page")
    }

    pub fn pages() -> Self {
        Self::new_unchecked("Pages")
    }

    pub fn catalog() -> Self {
        Self::new_unchecked("Catalog")
    }

    pub fn font() -> Self {
        Self::new_unchecked("Font")
    }

    pub fn resources() -> Self {
        Self::new_unchecked("Resources")
    }

    pub fn media_box() -> Self {
        Self::new_unchecked("MediaBox")
    }

    pub fn contents() -> Self {
        Self::new_unchecked("Contents")
    }

    pub fn parent() -> Self {
        Self::new_unchecked("Parent")
    }

    pub fn kids() -> Self {
        Self::new_unchecked("Kids")
    }

    pub fn count() -> Self {
        Self::new_unchecked("Count")
    }

    pub fn length() -> Self {
        Self::new_unchecked("Length")
    }

    pub fn subtype() -> Self {
        Self::new_unchecked("Subtype")
    }

    pub fn base_font() -> Self {
        Self::new_unchecked("BaseFont")
    }

    pub fn type1() -> Self {
        Self::new_unchecked("Type1")
    }

    pub fn root() -> Self {
        Self::new_unchecked("Root")
    }

    pub fn size() -> Self {
        Self::new_unchecked("Size")
    }

    pub fn info() -> Self {
        Self::new_unchecked("Info")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_name() {
        let name = PdfName::new("Type").unwrap();
        assert_eq!(name.to_pdf_string(), "/Type");
    }

    #[test]
    fn test_name_with_space() {
        let name = PdfName::new("Hello World").unwrap();
        // Space (0x20) should be escaped as #20
        assert_eq!(name.to_pdf_string(), "/Hello#20World");
    }

    #[test]
    fn test_name_with_hash() {
        let name = PdfName::new("Name#1").unwrap();
        assert_eq!(name.to_pdf_string(), "/Name#231");
    }

    #[test]
    fn test_empty_name_error() {
        let result = PdfName::new("");
        assert!(result.is_err());
    }

    #[test]
    fn test_predefined_names() {
        assert_eq!(PdfName::type_name().to_pdf_string(), "/Type");
        assert_eq!(PdfName::page().to_pdf_string(), "/Page");
        assert_eq!(PdfName::catalog().to_pdf_string(), "/Catalog");
    }
}
