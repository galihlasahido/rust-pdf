//! ASCIIHexDecode filter (ISO 32000-1:2008 Section 7.4.2).
//!
//! ASCIIHexDecode data consists of hexadecimal digit pairs, optionally
//! separated by whitespace, terminated by the `>` character (EOD marker).
//! An odd number of digits is padded with an implicit trailing `0` as
//! required by the spec.

use crate::error::CompressionError;

/// Decodes data encoded with the `ASCIIHexDecode` filter.
///
/// Whitespace between digits is ignored. Decoding stops at the first `>`
/// character (or at the end of input if no terminator is present, which is
/// tolerated for robustness against truncated/corrupt streams). Any byte
/// that is neither a hex digit, whitespace, nor the `>` terminator is
/// rejected as invalid input, since ISO 32000-1 7.4.2 only permits those.
pub fn decode_ascii_hex(data: &[u8]) -> Result<Vec<u8>, CompressionError> {
    let mut nibbles: Vec<u8> = Vec::with_capacity(data.len());

    for &b in data {
        match b {
            b'>' => break,
            b'0'..=b'9' => nibbles.push(b - b'0'),
            b'a'..=b'f' => nibbles.push(b - b'a' + 10),
            b'A'..=b'F' => nibbles.push(b - b'A' + 10),
            b' ' | b'\t' | b'\r' | b'\n' | 0x0c | 0x00 => continue,
            _ => {
                return Err(CompressionError::DecompressionFailed(format!(
                    "ASCIIHexDecode: invalid byte 0x{:02x}",
                    b
                )))
            }
        }
    }

    if nibbles.len() % 2 == 1 {
        nibbles.push(0);
    }

    let mut out = Vec::with_capacity(nibbles.len() / 2);
    for pair in nibbles.chunks_exact(2) {
        out.push((pair[0] << 4) | pair[1]);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_simple_hex() {
        let out = decode_ascii_hex(b"48656C6C6F>").unwrap();
        assert_eq!(out, b"Hello");
    }

    #[test]
    fn decodes_with_whitespace() {
        let out = decode_ascii_hex(b"48 65 6C 6C 6F >").unwrap();
        assert_eq!(out, b"Hello");
    }

    #[test]
    fn pads_odd_digit_count() {
        // "48656C6C6F" without a final digit -> "4" should be padded to "40"
        let out = decode_ascii_hex(b"4>").unwrap();
        assert_eq!(out, vec![0x40]);
    }

    #[test]
    fn missing_terminator_is_tolerated() {
        let out = decode_ascii_hex(b"48656C6C6F").unwrap();
        assert_eq!(out, b"Hello");
    }

    #[test]
    fn rejects_invalid_byte() {
        let err = decode_ascii_hex(b"48ZZ>");
        assert!(err.is_err());
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let out = decode_ascii_hex(b">").unwrap();
        assert!(out.is_empty());
    }
}
