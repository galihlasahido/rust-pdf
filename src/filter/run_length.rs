//! RunLengthDecode filter (ISO 32000-1:2008 Section 7.4.5).
//!
//! Data is a sequence of "runs", each introduced by a length byte:
//! - `0..=127`: the next `length + 1` bytes are copied literally.
//! - `129..=255`: the next single byte is repeated `257 - length` times.
//! - `128`: EOD (end of data) marker.

use crate::error::CompressionError;
use crate::filter::MAX_DECODED_SIZE;

/// Decodes data encoded with the `RunLengthDecode` filter.
///
/// Per 7.4.5, a length byte of 128 signals end-of-data. A run whose data
/// bytes would run past the end of the input is treated as corrupt input
/// and rejected (rather than panicking on an out-of-bounds slice).
pub fn decode_run_length(data: &[u8]) -> Result<Vec<u8>, CompressionError> {
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < data.len() {
        let length = data[i];
        i += 1;

        if length == 128 {
            break;
        } else if length < 128 {
            let count = length as usize + 1;
            let end = i.checked_add(count).ok_or_else(|| {
                CompressionError::DecompressionFailed("RunLengthDecode: overflow".to_string())
            })?;
            if end > data.len() {
                return Err(CompressionError::DecompressionFailed(
                    "RunLengthDecode: literal run truncated".to_string(),
                ));
            }
            out.extend_from_slice(&data[i..end]);
            i = end;
        } else {
            // length in 129..=255
            if i >= data.len() {
                return Err(CompressionError::DecompressionFailed(
                    "RunLengthDecode: replicate run missing data byte".to_string(),
                ));
            }
            let count = 257usize - length as usize;
            let byte = data[i];
            i += 1;
            out.resize(out.len() + count, byte);
        }

        if out.len() > MAX_DECODED_SIZE {
            return Err(CompressionError::DecompressionFailed(
                "RunLengthDecode: decoded output exceeds maximum allowed size".to_string(),
            ));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_literal_run() {
        // length=4 means copy next 5 bytes literally.
        let input = [4u8, b'H', b'e', b'l', b'l', b'o', 128];
        let out = decode_run_length(&input).unwrap();
        assert_eq!(out, b"Hello");
    }

    #[test]
    fn decodes_replicate_run() {
        // length=257-5=252 -> unsigned byte 252 means repeat next byte 5 times.
        let input = [252u8, b'A', 128];
        let out = decode_run_length(&input).unwrap();
        assert_eq!(out, b"AAAAA");
    }

    #[test]
    fn stops_at_eod_marker() {
        let input = [0u8, b'X', 128, 0, b'Y'];
        let out = decode_run_length(&input).unwrap();
        assert_eq!(out, b"X");
    }

    #[test]
    fn missing_eod_is_tolerated() {
        let input = [0u8, b'X'];
        let out = decode_run_length(&input).unwrap();
        assert_eq!(out, b"X");
    }

    #[test]
    fn truncated_literal_run_is_rejected() {
        // Claims 10 literal bytes follow but only 2 are present.
        let input = [9u8, b'A', b'B'];
        let result = decode_run_length(&input);
        assert!(result.is_err());
    }

    #[test]
    fn truncated_replicate_run_is_rejected() {
        let input = [252u8];
        let result = decode_run_length(&input);
        assert!(result.is_err());
    }

    #[test]
    fn empty_input() {
        let out = decode_run_length(&[]).unwrap();
        assert!(out.is_empty());
    }
}
