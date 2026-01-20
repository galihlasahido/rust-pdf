//! PDF Stream object.

use super::{Object, PdfDictionary, PdfName};

#[cfg(feature = "compression")]
use crate::error::CompressionError;

/// A PDF stream object.
///
/// Streams consist of a dictionary followed by the stream data:
/// ```text
/// << /Length 123 >>
/// stream
/// ...binary or text data...
/// endstream
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct PdfStream {
    /// The stream dictionary (must contain at least /Length).
    pub dictionary: PdfDictionary,
    /// The raw stream data.
    pub data: Vec<u8>,
}

impl PdfStream {
    /// Creates a new stream with the given data.
    ///
    /// The /Length key is automatically set based on the data.
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        let data = data.into();
        let mut dictionary = PdfDictionary::new();
        dictionary.set("Length", Object::Integer(data.len() as i64));

        Self { dictionary, data }
    }

    /// Creates a stream with a custom dictionary.
    ///
    /// The /Length key will be set/overwritten based on the actual data length.
    pub fn with_dictionary(mut dictionary: PdfDictionary, data: impl Into<Vec<u8>>) -> Self {
        let data = data.into();
        dictionary.set("Length", Object::Integer(data.len() as i64));
        Self { dictionary, data }
    }

    /// Creates a stream from raw dictionary and data without modifying the dictionary.
    ///
    /// This is useful when the dictionary already has the correct /Length set,
    /// such as when creating encrypted streams.
    pub fn from_raw(dictionary: PdfDictionary, data: Vec<u8>) -> Self {
        Self { dictionary, data }
    }

    /// Creates a stream from text content.
    pub fn from_text(text: impl Into<String>) -> Self {
        Self::new(text.into().into_bytes())
    }

    /// Adds a filter to the stream dictionary.
    pub fn add_filter(&mut self, filter: &str) {
        self.dictionary
            .set("Filter", Object::Name(PdfName::new_unchecked(filter)));
    }

    /// Returns the stream data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Returns the length of the stream data.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns true if the stream data is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Serializes the stream to PDF format (dictionary only, for object definition).
    ///
    /// Note: The actual stream content is written separately by the writer.
    pub fn dictionary_to_pdf_string(&self) -> String {
        self.dictionary.to_pdf_string()
    }

    /// Serializes the complete stream to PDF format.
    pub fn to_pdf_bytes(&self) -> Vec<u8> {
        let mut result = Vec::new();

        // Dictionary
        result.extend_from_slice(self.dictionary.to_pdf_string().as_bytes());
        result.extend_from_slice(b"\nstream\n");

        // Stream data
        result.extend_from_slice(&self.data);

        // End stream
        result.extend_from_slice(b"\nendstream");

        result
    }

    /// Returns true if the stream has a compression filter applied.
    pub fn is_compressed(&self) -> bool {
        self.dictionary.get("Filter").is_some()
    }

    /// Compresses the stream data using Flate compression.
    ///
    /// This consumes the stream and returns a new compressed stream
    /// with the `/Filter /FlateDecode` entry in the dictionary.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let stream = PdfStream::from_text("Hello, World!");
    /// let compressed = stream.with_compression()?;
    /// assert!(compressed.is_compressed());
    /// ```
    #[cfg(feature = "compression")]
    pub fn with_compression(mut self) -> Result<Self, CompressionError> {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        // Don't compress already compressed streams
        if self.is_compressed() {
            return Ok(self);
        }

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&self.data)
            .map_err(|e| CompressionError::CompressionFailed(e.to_string()))?;

        self.data = encoder
            .finish()
            .map_err(|e| CompressionError::CompressionFailed(e.to_string()))?;

        // Update Length and set Filter
        self.dictionary
            .set("Length", Object::Integer(self.data.len() as i64));
        self.dictionary
            .set("Filter", Object::Name(PdfName::new_unchecked("FlateDecode")));

        Ok(self)
    }

    /// Decompresses the stream data if it's compressed with FlateDecode.
    ///
    /// Returns the decompressed data, or the original data if not compressed.
    #[cfg(feature = "compression")]
    pub fn decompress(&self) -> Result<Vec<u8>, CompressionError> {
        use flate2::read::ZlibDecoder;
        use std::io::Read;

        // Check if stream is compressed with FlateDecode
        let is_flate = match self.dictionary.get("Filter") {
            Some(Object::Name(name)) => name.as_str() == "FlateDecode",
            _ => false,
        };

        if !is_flate {
            return Ok(self.data.clone());
        }

        let mut decoder = ZlibDecoder::new(&self.data[..]);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|e| CompressionError::DecompressionFailed(e.to_string()))?;

        Ok(decompressed)
    }
}

/// Builder for creating PDF streams fluently.
#[derive(Debug, Default)]
pub struct StreamBuilder {
    dictionary: PdfDictionary,
    data: Vec<u8>,
    #[cfg(feature = "compression")]
    compress: bool,
}

impl StreamBuilder {
    /// Creates a new stream builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the stream data from bytes.
    pub fn data(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.data = data.into();
        self
    }

    /// Sets the stream data from text.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.data = text.into().into_bytes();
        self
    }

    /// Sets a dictionary entry.
    pub fn set(mut self, key: impl Into<String>, value: impl Into<Object>) -> Self {
        self.dictionary.set(key, value);
        self
    }

    /// Adds a filter.
    pub fn filter(mut self, filter: &str) -> Self {
        self.dictionary
            .set("Filter", Object::Name(PdfName::new_unchecked(filter)));
        self
    }

    /// Enables compression for the stream.
    ///
    /// When enabled, the stream data will be compressed using Flate compression
    /// when `build_compressed()` is called.
    #[cfg(feature = "compression")]
    pub fn compress(mut self) -> Self {
        self.compress = true;
        self
    }

    /// Builds the stream.
    pub fn build(self) -> PdfStream {
        PdfStream::with_dictionary(self.dictionary, self.data)
    }

    /// Builds the stream with optional compression.
    ///
    /// If `compress()` was called, the stream data will be compressed.
    #[cfg(feature = "compression")]
    pub fn build_compressed(self) -> Result<PdfStream, CompressionError> {
        let stream = PdfStream::with_dictionary(self.dictionary, self.data);
        if self.compress {
            stream.with_compression()
        } else {
            Ok(stream)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_from_text() {
        let stream = PdfStream::from_text("Hello, World!");
        assert_eq!(stream.len(), 13);
        assert_eq!(stream.data(), b"Hello, World!");
    }

    #[test]
    fn test_stream_length_in_dictionary() {
        let stream = PdfStream::new(vec![1, 2, 3, 4, 5]);
        let dict_str = stream.dictionary_to_pdf_string();
        assert!(dict_str.contains("/Length 5"));
    }

    #[test]
    fn test_stream_to_pdf_bytes() {
        let stream = PdfStream::from_text("Test");
        let bytes = stream.to_pdf_bytes();
        let text = String::from_utf8_lossy(&bytes);

        assert!(text.contains("<< /Length 4 >>"));
        assert!(text.contains("stream\n"));
        assert!(text.contains("Test"));
        assert!(text.contains("\nendstream"));
    }

    #[test]
    fn test_stream_builder() {
        let stream = StreamBuilder::new()
            .text("Content stream data")
            .filter("FlateDecode")
            .build();

        assert!(!stream.is_empty());
        assert!(stream.dictionary.contains_key("Filter"));
    }

    #[test]
    fn test_is_compressed() {
        let uncompressed = PdfStream::from_text("Hello");
        assert!(!uncompressed.is_compressed());

        let mut compressed = PdfStream::from_text("Hello");
        compressed.add_filter("FlateDecode");
        assert!(compressed.is_compressed());
    }

    #[cfg(feature = "compression")]
    mod compression_tests {
        use super::*;

        #[test]
        fn test_stream_compression() {
            let stream = PdfStream::from_text("Hello, World! This is a test of compression.");
            let original_len = stream.len();

            let compressed = stream.with_compression().unwrap();

            assert!(compressed.is_compressed());
            assert!(compressed.dictionary.get("Filter").is_some());

            // Check that the dictionary contains FlateDecode
            let dict_str = compressed.dictionary_to_pdf_string();
            assert!(dict_str.contains("/Filter /FlateDecode"));

            // For very small data, compression might not reduce size,
            // but the filter should still be applied
            assert!(compressed.len() > 0);
            assert!(compressed.len() != original_len || original_len < 50);
        }

        #[test]
        fn test_stream_compression_and_decompression_roundtrip() {
            let original_data = "Hello, World! This is a test of compression that should be long enough to actually compress well. ".repeat(10);
            let stream = PdfStream::from_text(&original_data);

            let compressed = stream.with_compression().unwrap();

            // Should be compressed
            assert!(compressed.is_compressed());
            assert!(compressed.len() < original_data.len());

            // Decompress and verify
            let decompressed = compressed.decompress().unwrap();
            assert_eq!(String::from_utf8_lossy(&decompressed), original_data);
        }

        #[test]
        fn test_double_compression_is_idempotent() {
            let stream = PdfStream::from_text("Some test data for compression");
            let compressed = stream.with_compression().unwrap();
            let compressed_len = compressed.len();

            // Compressing again should not change anything
            let double_compressed = compressed.with_compression().unwrap();
            assert_eq!(double_compressed.len(), compressed_len);
        }

        #[test]
        fn test_decompress_uncompressed_stream() {
            let stream = PdfStream::from_text("Hello, World!");
            let data = stream.decompress().unwrap();
            assert_eq!(data, b"Hello, World!");
        }

        #[test]
        fn test_stream_builder_with_compression() {
            let stream = StreamBuilder::new()
                .text("Content stream data that should be compressed")
                .compress()
                .build_compressed()
                .unwrap();

            assert!(stream.is_compressed());
            assert!(stream.dictionary.get("Filter").is_some());
        }

        #[test]
        fn test_stream_builder_without_compression() {
            let stream = StreamBuilder::new()
                .text("Content stream data")
                .build_compressed()
                .unwrap();

            // Should not be compressed because compress() was not called
            assert!(!stream.is_compressed());
        }
    }
}
