//! PDF Dictionary object.

use super::{Object, PdfName};
use indexmap::IndexMap;

/// A PDF dictionary object.
///
/// Dictionaries in PDF are written as: << /Key1 value1 /Key2 value2 >>
/// The IndexMap preserves insertion order, which is important for
/// reproducible PDF output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PdfDictionary {
    entries: IndexMap<String, Object>,
}

impl PdfDictionary {
    /// Creates a new empty dictionary.
    pub fn new() -> Self {
        Self {
            entries: IndexMap::new(),
        }
    }

    /// Creates a dictionary with the given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: IndexMap::with_capacity(capacity),
        }
    }

    /// Sets a key-value pair in the dictionary.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<Object>) {
        self.entries.insert(key.into(), value.into());
    }

    /// Sets a key-value pair using a PdfName as key.
    pub fn set_name(&mut self, key: &PdfName, value: impl Into<Object>) {
        self.entries.insert(key.as_str().to_string(), value.into());
    }

    /// Gets a value by key.
    pub fn get(&self, key: &str) -> Option<&Object> {
        self.entries.get(key)
    }

    /// Checks if the dictionary contains a key.
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Removes a key from the dictionary and returns its value.
    pub fn remove(&mut self, key: &str) -> Option<Object> {
        self.entries.shift_remove(key)
    }

    /// Returns the number of entries in the dictionary.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the dictionary is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns an iterator over the key-value pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Object)> {
        self.entries.iter()
    }

    /// Serializes the dictionary to PDF format.
    pub fn to_pdf_string(&self) -> String {
        let mut result = String::from("<<");

        for (key, value) in &self.entries {
            result.push(' ');
            // Keys are always names, so prepend /
            result.push('/');
            // Escape the key if necessary
            for byte in key.bytes() {
                if Self::needs_escape(byte) {
                    result.push('#');
                    result.push_str(&format!("{:02X}", byte));
                } else {
                    result.push(byte as char);
                }
            }
            result.push(' ');
            result.push_str(&value.to_pdf_string());
        }

        result.push_str(" >>");
        result
    }

    /// Checks if a byte needs to be escaped in a PDF name.
    fn needs_escape(byte: u8) -> bool {
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
}

/// Builder for creating PDF dictionaries fluently.
#[derive(Debug, Default)]
pub struct DictionaryBuilder {
    dict: PdfDictionary,
}

impl DictionaryBuilder {
    /// Creates a new dictionary builder.
    pub fn new() -> Self {
        Self {
            dict: PdfDictionary::new(),
        }
    }

    /// Sets a key-value pair.
    pub fn set(mut self, key: impl Into<String>, value: impl Into<Object>) -> Self {
        self.dict.set(key, value);
        self
    }

    /// Sets the /Type key.
    pub fn type_name(self, name: impl Into<String>) -> Self {
        self.set("Type", Object::Name(PdfName::new_unchecked(name)))
    }

    /// Builds the dictionary.
    pub fn build(self) -> PdfDictionary {
        self.dict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_dictionary() {
        let dict = PdfDictionary::new();
        assert_eq!(dict.to_pdf_string(), "<< >>");
    }

    #[test]
    fn test_dictionary_with_entries() {
        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Page")));
        dict.set("Count", Object::Integer(1));
        assert_eq!(dict.to_pdf_string(), "<< /Type /Page /Count 1 >>");
    }

    #[test]
    fn test_dictionary_builder() {
        let dict = DictionaryBuilder::new()
            .type_name("Catalog")
            .set("Version", Object::Name(PdfName::new_unchecked("1.7")))
            .build();

        assert!(dict.to_pdf_string().contains("/Type /Catalog"));
    }

    #[test]
    fn test_dictionary_get() {
        let mut dict = PdfDictionary::new();
        dict.set("Key", Object::Integer(42));
        assert_eq!(dict.get("Key"), Some(&Object::Integer(42)));
        assert_eq!(dict.get("NotFound"), None);
    }

    #[test]
    fn test_dictionary_preserves_order() {
        let mut dict = PdfDictionary::new();
        dict.set("A", Object::Integer(1));
        dict.set("B", Object::Integer(2));
        dict.set("C", Object::Integer(3));

        let keys: Vec<_> = dict.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["A", "B", "C"]);
    }
}
