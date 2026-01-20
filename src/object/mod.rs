//! PDF Object types and serialization.

mod array;
mod dictionary;
mod name;
mod stream;
mod string;

pub use array::PdfArray;
pub use dictionary::{DictionaryBuilder, PdfDictionary};
pub use name::PdfName;
pub use stream::{PdfStream, StreamBuilder};
pub use string::PdfString;

use crate::types::ObjectId;

/// A PDF object that can be serialized to PDF format.
///
/// This enum represents all possible PDF object types as defined in the
/// PDF specification.
#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    /// A null object.
    Null,
    /// A boolean value.
    Boolean(bool),
    /// An integer number.
    Integer(i64),
    /// A real (floating-point) number.
    Real(f64),
    /// A name object (e.g., /Type).
    Name(PdfName),
    /// A string object (literal or hexadecimal).
    String(PdfString),
    /// An array of objects.
    Array(PdfArray),
    /// A dictionary of key-value pairs.
    Dictionary(PdfDictionary),
    /// A stream with dictionary and binary data.
    Stream(PdfStream),
    /// An indirect reference to another object.
    Reference(ObjectId),
}

impl Object {
    /// Serializes the object to PDF format.
    pub fn to_pdf_string(&self) -> String {
        match self {
            Object::Null => "null".to_string(),
            Object::Boolean(b) => if *b { "true" } else { "false" }.to_string(),
            Object::Integer(i) => i.to_string(),
            Object::Real(r) => format_real(*r),
            Object::Name(n) => n.to_pdf_string(),
            Object::String(s) => s.to_pdf_string(),
            Object::Array(a) => a.to_pdf_string(),
            Object::Dictionary(d) => d.to_pdf_string(),
            Object::Stream(s) => {
                // For streams, we just output the dictionary part
                // The actual stream content is handled by the writer
                s.dictionary_to_pdf_string()
            }
            Object::Reference(id) => id.reference_string(),
        }
    }

    /// Returns true if this is a null object.
    pub fn is_null(&self) -> bool {
        matches!(self, Object::Null)
    }

    /// Returns true if this is a stream object.
    pub fn is_stream(&self) -> bool {
        matches!(self, Object::Stream(_))
    }

    /// Attempts to get the object as an integer.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Object::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Attempts to get the object as a real number.
    pub fn as_real(&self) -> Option<f64> {
        match self {
            Object::Real(r) => Some(*r),
            Object::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Attempts to get the object as a dictionary.
    pub fn as_dictionary(&self) -> Option<&PdfDictionary> {
        match self {
            Object::Dictionary(d) => Some(d),
            _ => None,
        }
    }

    /// Attempts to get the object as an array.
    pub fn as_array(&self) -> Option<&PdfArray> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }
}

/// Formats a real number for PDF output.
///
/// Removes trailing zeros and unnecessary decimal point.
fn format_real(r: f64) -> String {
    // Handle special cases
    if r == 0.0 {
        return "0".to_string();
    }

    // Check if it's actually an integer
    if r.fract() == 0.0 && r.abs() < i64::MAX as f64 {
        return (r as i64).to_string();
    }

    // Format with reasonable precision
    let s = format!("{:.6}", r);

    // Trim trailing zeros after decimal point
    let s = s.trim_end_matches('0');

    // Trim trailing decimal point if no fractional part
    let s = s.trim_end_matches('.');

    s.to_string()
}

// Conversion implementations

impl From<bool> for Object {
    fn from(b: bool) -> Self {
        Object::Boolean(b)
    }
}

impl From<i32> for Object {
    fn from(i: i32) -> Self {
        Object::Integer(i as i64)
    }
}

impl From<i64> for Object {
    fn from(i: i64) -> Self {
        Object::Integer(i)
    }
}

impl From<f32> for Object {
    fn from(f: f32) -> Self {
        Object::Real(f as f64)
    }
}

impl From<f64> for Object {
    fn from(f: f64) -> Self {
        Object::Real(f)
    }
}

impl From<PdfName> for Object {
    fn from(n: PdfName) -> Self {
        Object::Name(n)
    }
}

impl From<PdfString> for Object {
    fn from(s: PdfString) -> Self {
        Object::String(s)
    }
}

impl From<&str> for Object {
    fn from(s: &str) -> Self {
        Object::String(PdfString::literal(s))
    }
}

impl From<String> for Object {
    fn from(s: String) -> Self {
        Object::String(PdfString::literal(s))
    }
}

impl From<PdfArray> for Object {
    fn from(a: PdfArray) -> Self {
        Object::Array(a)
    }
}

impl From<PdfDictionary> for Object {
    fn from(d: PdfDictionary) -> Self {
        Object::Dictionary(d)
    }
}

impl From<PdfStream> for Object {
    fn from(s: PdfStream) -> Self {
        Object::Stream(s)
    }
}

impl From<ObjectId> for Object {
    fn from(id: ObjectId) -> Self {
        Object::Reference(id)
    }
}

impl<T: Into<Object>> From<Vec<T>> for Object {
    fn from(vec: Vec<T>) -> Self {
        let array = PdfArray::from_objects(vec.into_iter().map(Into::into).collect());
        Object::Array(array)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_null() {
        assert_eq!(Object::Null.to_pdf_string(), "null");
    }

    #[test]
    fn test_boolean() {
        assert_eq!(Object::Boolean(true).to_pdf_string(), "true");
        assert_eq!(Object::Boolean(false).to_pdf_string(), "false");
    }

    #[test]
    fn test_integer() {
        assert_eq!(Object::Integer(42).to_pdf_string(), "42");
        assert_eq!(Object::Integer(-100).to_pdf_string(), "-100");
    }

    #[test]
    fn test_real() {
        assert_eq!(Object::Real(3.14).to_pdf_string(), "3.14");
        assert_eq!(Object::Real(1.0).to_pdf_string(), "1");
        assert_eq!(Object::Real(0.5).to_pdf_string(), "0.5");
    }

    #[test]
    fn test_format_real_removes_trailing_zeros() {
        assert_eq!(format_real(1.5000), "1.5");
        assert_eq!(format_real(2.0), "2");
        assert_eq!(format_real(0.0), "0");
    }

    #[test]
    fn test_reference() {
        let id = ObjectId::new(5);
        assert_eq!(Object::Reference(id).to_pdf_string(), "5 0 R");
    }

    #[test]
    fn test_from_conversions() {
        let _: Object = true.into();
        let _: Object = 42i32.into();
        let _: Object = 3.14f64.into();
        let _: Object = "test".into();
    }

    #[test]
    fn test_as_methods() {
        let int = Object::Integer(42);
        assert_eq!(int.as_integer(), Some(42));
        assert_eq!(int.as_real(), Some(42.0));

        let real = Object::Real(3.14);
        assert_eq!(real.as_integer(), None);
        assert_eq!(real.as_real(), Some(3.14));
    }
}
