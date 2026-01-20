//! PDF Array object.

use super::Object;

/// A PDF array object.
///
/// Arrays in PDF are written as: [element1 element2 element3]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PdfArray {
    elements: Vec<Object>,
}

impl PdfArray {
    /// Creates a new empty array.
    pub fn new() -> Self {
        Self {
            elements: Vec::new(),
        }
    }

    /// Creates an array with the given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            elements: Vec::with_capacity(capacity),
        }
    }

    /// Creates an array from a vector of objects.
    pub fn from_objects(objects: Vec<Object>) -> Self {
        Self { elements: objects }
    }

    /// Adds an element to the array.
    pub fn push(&mut self, element: impl Into<Object>) {
        self.elements.push(element.into());
    }

    /// Returns the number of elements in the array.
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// Returns true if the array is empty.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// Returns an iterator over the elements.
    pub fn iter(&self) -> impl Iterator<Item = &Object> {
        self.elements.iter()
    }

    /// Returns the elements as a slice.
    pub fn as_slice(&self) -> &[Object] {
        &self.elements
    }

    /// Gets an element by index.
    pub fn get(&self, index: usize) -> Option<&Object> {
        self.elements.get(index)
    }

    /// Serializes the array to PDF format.
    pub fn to_pdf_string(&self) -> String {
        let mut result = String::from("[");

        for (i, element) in self.elements.iter().enumerate() {
            if i > 0 {
                result.push(' ');
            }
            result.push_str(&element.to_pdf_string());
        }

        result.push(']');
        result
    }
}

impl FromIterator<Object> for PdfArray {
    fn from_iter<I: IntoIterator<Item = Object>>(iter: I) -> Self {
        Self {
            elements: iter.into_iter().collect(),
        }
    }
}

impl IntoIterator for PdfArray {
    type Item = Object;
    type IntoIter = std::vec::IntoIter<Object>;

    fn into_iter(self) -> Self::IntoIter {
        self.elements.into_iter()
    }
}

impl<'a> IntoIterator for &'a PdfArray {
    type Item = &'a Object;
    type IntoIter = std::slice::Iter<'a, Object>;

    fn into_iter(self) -> Self::IntoIter {
        self.elements.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::PdfName;

    #[test]
    fn test_empty_array() {
        let arr = PdfArray::new();
        assert_eq!(arr.to_pdf_string(), "[]");
    }

    #[test]
    fn test_array_with_numbers() {
        let mut arr = PdfArray::new();
        arr.push(Object::Integer(1));
        arr.push(Object::Integer(2));
        arr.push(Object::Integer(3));
        assert_eq!(arr.to_pdf_string(), "[1 2 3]");
    }

    #[test]
    fn test_array_with_mixed() {
        let mut arr = PdfArray::new();
        arr.push(Object::Integer(100));
        arr.push(Object::Real(0.5));
        arr.push(Object::Name(PdfName::new_unchecked("Test")));
        assert_eq!(arr.to_pdf_string(), "[100 0.5 /Test]");
    }

    #[test]
    fn test_array_len() {
        let mut arr = PdfArray::new();
        assert!(arr.is_empty());
        arr.push(Object::Integer(1));
        assert_eq!(arr.len(), 1);
    }
}
