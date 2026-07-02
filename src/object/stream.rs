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

    /// Fully decodes the stream data by applying every filter named in
    /// `/Filter` (a single name or an array of names), in order, using the
    /// matching entries of `/DecodeParms` (ISO 32000-1:2008 Section 7.4,
    /// Table 6). Unlike [`PdfStream::decompress`] (which only understands
    /// `FlateDecode`), this supports the full filter set implemented in
    /// [`crate::filter`]: `ASCIIHexDecode`, `ASCII85Decode`,
    /// `RunLengthDecode`, `LZWDecode`, `FlateDecode` (both with PNG/TIFF
    /// predictor support), `DCTDecode` and `CCITTFaxDecode`.
    ///
    /// For image streams whose last filter is `DCTDecode` or
    /// `CCITTFaxDecode`, the returned bytes are the final raw/packed image
    /// samples, not something meant to be filtered further.
    #[cfg(feature = "compression")]
    pub fn decode_all(&self) -> Result<Vec<u8>, CompressionError> {
        use crate::filter::decode_filter;

        let filters: Vec<String> = match self.dictionary.get("Filter") {
            Some(Object::Name(n)) => vec![n.as_str().to_string()],
            Some(Object::Array(arr)) => arr
                .iter()
                .filter_map(|o| match o {
                    Object::Name(n) => Some(n.as_str().to_string()),
                    _ => None,
                })
                .collect(),
            _ => return Ok(self.data.clone()),
        };

        let parms: Vec<Option<PdfDictionary>> = match self.dictionary.get("DecodeParms") {
            Some(Object::Dictionary(d)) => vec![Some(d.clone())],
            Some(Object::Array(arr)) => arr
                .iter()
                .map(|o| match o {
                    Object::Dictionary(d) => Some(d.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };

        let mut data = self.data.clone();
        for (i, filter) in filters.iter().enumerate() {
            let params = parms.get(i).and_then(|p| p.as_ref());
            data = decode_filter(filter, &data, params)?;
        }

        Ok(data)
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
            assert!(!compressed.is_empty());
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

    #[cfg(feature = "compression")]
    mod decode_all_tests {
        use super::*;
        use crate::object::PdfArray;

        #[test]
        fn decode_all_passthrough_when_no_filter() {
            let stream = PdfStream::from_text("raw data");
            assert_eq!(stream.decode_all().unwrap(), b"raw data");
        }

        #[test]
        fn decode_all_ascii_hex() {
            let mut dict = PdfDictionary::new();
            dict.set("Filter", Object::Name(PdfName::new_unchecked("ASCIIHexDecode")));
            let stream = PdfStream::from_raw(dict, b"48656C6C6F>".to_vec());
            assert_eq!(stream.decode_all().unwrap(), b"Hello");
        }

        #[test]
        fn decode_all_run_length() {
            let mut dict = PdfDictionary::new();
            dict.set("Filter", Object::Name(PdfName::new_unchecked("RunLengthDecode")));
            let stream = PdfStream::from_raw(dict, vec![4, b'H', b'e', b'l', b'l', b'o', 128]);
            assert_eq!(stream.decode_all().unwrap(), b"Hello");
        }

        #[test]
        fn decode_all_filter_chain_ascii85_then_flate() {
            let original = b"Hello, chained filters!".to_vec();
            let flate_stream = PdfStream::from_text(String::from_utf8(original.clone()).unwrap())
                .with_compression()
                .unwrap();
            let flate_bytes = flate_stream.data().to_vec();
            let ascii85 = crate::filter::decode_ascii85; // sanity: module is reachable
            let _ = ascii85; // silence unused import when feature combos vary

            // Encode: raw -> Flate -> ASCII85, so Filter = [ASCII85Decode FlateDecode]
            // must be applied in that order to recover the original bytes.
            let ascii85_encoded = ascii85_encode_for_test(&flate_bytes);

            let mut dict = PdfDictionary::new();
            let mut filters = PdfArray::new();
            filters.push(Object::Name(PdfName::new_unchecked("ASCII85Decode")));
            filters.push(Object::Name(PdfName::new_unchecked("FlateDecode")));
            dict.set("Filter", Object::Array(filters));

            let stream = PdfStream::from_raw(dict, ascii85_encoded);
            assert_eq!(stream.decode_all().unwrap(), original);
        }

        #[test]
        fn decode_all_flate_with_png_predictor() {
            // 2x2 grayscale image, 1 byte/pixel, PNG "Up" filter on row 2.
            let raw_rows: Vec<u8> = vec![0, 10, 20, 2, 1, 1];
            let stream_data = PdfStream::new(raw_rows.clone()).with_compression().unwrap();

            let mut dict = stream_data.dictionary.clone();
            let mut parms = PdfDictionary::new();
            parms.set("Predictor", Object::Integer(15));
            parms.set("Colors", Object::Integer(1));
            parms.set("BitsPerComponent", Object::Integer(8));
            parms.set("Columns", Object::Integer(2));
            dict.set("DecodeParms", Object::Dictionary(parms));

            let stream = PdfStream::from_raw(dict, stream_data.data().to_vec());
            let decoded = stream.decode_all().unwrap();
            // Row1 (filter=None): [10,20]; Row2 (filter=Up): [1,1] + [10,20] = [11,21]
            assert_eq!(decoded, vec![10, 20, 11, 21]);
        }

        #[test]
        fn decode_all_unsupported_filter_errors_cleanly() {
            let mut dict = PdfDictionary::new();
            dict.set("Filter", Object::Name(PdfName::new_unchecked("JBIG2Decode")));
            let stream = PdfStream::from_raw(dict, b"whatever".to_vec());
            assert!(stream.decode_all().is_err());
        }

        /// Minimal ASCII85 encoder used only to build test fixtures (the
        /// production code only needs to *decode* ASCII85).
        fn ascii85_encode_for_test(data: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            for chunk in data.chunks(4) {
                let mut buf = [0u8; 4];
                buf[..chunk.len()].copy_from_slice(chunk);
                let value = u32::from_be_bytes(buf);
                if chunk.len() == 4 && value == 0 {
                    out.push(b'z');
                    continue;
                }
                let mut digits = [0u8; 5];
                let mut v = value;
                for i in (0..5).rev() {
                    digits[i] = (v % 85) as u8 + 0x21;
                    v /= 85;
                }
                out.extend_from_slice(&digits[..chunk.len() + 1]);
            }
            out.extend_from_slice(b"~>");
            out
        }
    }
}
