//! PDF String object.

/// A PDF string object, which can be either literal or hexadecimal.
///
/// Literal strings are enclosed in parentheses: (Hello)
/// Hexadecimal strings are enclosed in angle brackets: <48656C6C6F>
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfString {
    /// A literal string enclosed in parentheses.
    Literal(Vec<u8>),
    /// A hexadecimal string enclosed in angle brackets.
    Hex(Vec<u8>),
}

impl PdfString {
    /// Creates a new literal string from text.
    pub fn literal(text: impl Into<String>) -> Self {
        Self::Literal(text.into().into_bytes())
    }

    /// Creates a new literal string from bytes.
    pub fn literal_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Literal(bytes.into())
    }

    /// Creates a new hexadecimal string from bytes.
    pub fn hex(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Hex(bytes.into())
    }

    /// Creates a hexadecimal string from text.
    pub fn hex_from_text(text: impl Into<String>) -> Self {
        Self::Hex(text.into().into_bytes())
    }

    /// Returns the raw bytes of the string.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Literal(bytes) | Self::Hex(bytes) => bytes,
        }
    }

    /// Serializes the string to PDF format.
    pub fn to_pdf_string(&self) -> String {
        match self {
            Self::Literal(bytes) => Self::escape_literal(bytes),
            Self::Hex(bytes) => Self::encode_hex(bytes),
        }
    }

    /// Escapes a literal string for PDF output.
    fn escape_literal(bytes: &[u8]) -> String {
        let mut result = String::with_capacity(bytes.len() + 10);
        result.push('(');

        for &byte in bytes {
            match byte {
                b'\\' => result.push_str("\\\\"),
                b'(' => result.push_str("\\("),
                b')' => result.push_str("\\)"),
                b'\n' => result.push_str("\\n"),
                b'\r' => result.push_str("\\r"),
                b'\t' => result.push_str("\\t"),
                b'\x08' => result.push_str("\\b"),
                b'\x0C' => result.push_str("\\f"),
                0..=31 | 127..=255 => {
                    // Use octal escape for non-printable characters
                    result.push_str(&format!("\\{:03o}", byte));
                }
                _ => result.push(byte as char),
            }
        }

        result.push(')');
        result
    }

    /// Encodes bytes as a hexadecimal string.
    fn encode_hex(bytes: &[u8]) -> String {
        let mut result = String::with_capacity(bytes.len() * 2 + 2);
        result.push('<');

        for byte in bytes {
            result.push_str(&format!("{:02X}", byte));
        }

        result.push('>');
        result
    }

    /// Attempts to convert the string to a UTF-8 string.
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(self.as_bytes()).into_owned()
    }
}

impl From<&str> for PdfString {
    fn from(s: &str) -> Self {
        Self::literal(s)
    }
}

impl From<String> for PdfString {
    fn from(s: String) -> Self {
        Self::literal(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_simple() {
        let s = PdfString::literal("Hello");
        assert_eq!(s.to_pdf_string(), "(Hello)");
    }

    #[test]
    fn test_literal_with_parentheses() {
        let s = PdfString::literal("Hello (World)");
        assert_eq!(s.to_pdf_string(), "(Hello \\(World\\))");
    }

    #[test]
    fn test_literal_with_backslash() {
        let s = PdfString::literal("C:\\path");
        assert_eq!(s.to_pdf_string(), "(C:\\\\path)");
    }

    #[test]
    fn test_literal_with_newline() {
        let s = PdfString::literal("Line1\nLine2");
        assert_eq!(s.to_pdf_string(), "(Line1\\nLine2)");
    }

    #[test]
    fn test_hex_string() {
        let s = PdfString::hex(vec![0x48, 0x65, 0x6C, 0x6C, 0x6F]);
        assert_eq!(s.to_pdf_string(), "<48656C6C6F>");
    }

    #[test]
    fn test_hex_from_text() {
        let s = PdfString::hex_from_text("Hi");
        assert_eq!(s.to_pdf_string(), "<4869>");
    }

    #[test]
    fn test_from_str() {
        let s: PdfString = "Test".into();
        assert_eq!(s.to_pdf_string(), "(Test)");
    }

    #[test]
    fn test_to_string_lossy() {
        let s = PdfString::literal("Hello");
        assert_eq!(s.to_string_lossy(), "Hello");
    }
}
