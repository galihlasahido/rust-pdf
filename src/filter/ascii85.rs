//! ASCII85Decode filter (ISO 32000-1:2008 Section 7.4.3).
//!
//! Encodes groups of 4 bytes as 5 ASCII characters in the range `!`..=`u`
//! (base 85). The special group `z` represents four zero bytes. The stream
//! is terminated by the two-character sequence `~>`.

use crate::error::CompressionError;

/// Decodes data encoded with the `ASCII85Decode` filter.
///
/// Whitespace anywhere in the input is ignored (7.4.3). Decoding stops at
/// the `~>` end-of-data marker; if it is missing, the remaining full/partial
/// group is still decoded for robustness against truncated streams.
pub fn decode_ascii85(data: &[u8]) -> Result<Vec<u8>, CompressionError> {
    let mut out = Vec::with_capacity(data.len() * 4 / 5 + 4);
    let mut group: [u8; 5] = [0; 5];
    let mut group_len = 0usize;

    let mut i = 0usize;
    while i < data.len() {
        let b = data[i];
        i += 1;

        match b {
            b' ' | b'\t' | b'\r' | b'\n' | 0x0c | 0x00 => continue,
            b'~' => {
                // End-of-data marker is "~>"; tolerate a bare '~' at EOF too.
                break;
            }
            b'z' if group_len == 0 => {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            0x21..=0x75 => {
                group[group_len] = b - 0x21;
                group_len += 1;
                if group_len == 5 {
                    decode_group(&group, 5, &mut out)?;
                    group_len = 0;
                }
            }
            _ => {
                return Err(CompressionError::DecompressionFailed(format!(
                    "ASCII85Decode: invalid byte 0x{:02x}",
                    b
                )))
            }
        }
    }

    if group_len > 0 {
        if group_len == 1 {
            return Err(CompressionError::DecompressionFailed(
                "ASCII85Decode: final group has only one character".to_string(),
            ));
        }
        // Pad the partial group with 'u' (0x75 -> value 84), the highest
        // valid digit, as specified in 7.4.3.
        for slot in group.iter_mut().skip(group_len) {
            *slot = 84;
        }
        decode_group(&group, group_len, &mut out)?;
    }

    Ok(out)
}

/// Decodes one base-85 group of up to 5 digits into up to 4 output bytes.
fn decode_group(group: &[u8; 5], len: usize, out: &mut Vec<u8>) -> Result<(), CompressionError> {
    let mut value: u32 = 0;
    for &digit in group.iter() {
        value = value
            .checked_mul(85)
            .and_then(|v| v.checked_add(digit as u32))
            .ok_or_else(|| {
                CompressionError::DecompressionFailed("ASCII85Decode: overflow".to_string())
            })?;
    }

    let bytes = value.to_be_bytes();
    // len-1 output bytes are produced from a group of `len` input digits.
    out.extend_from_slice(&bytes[..len - 1]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Hello World" encoded with Adobe-flavor ASCII85 (verified against
    /// Python's `base64.a85encode`), terminated with `~>`.
    const HELLO_WORLD_A85: &[u8] = b"\x38\x37\x63\x55\x52\x44\x5d\x69\x2c\x22\x45\x62\x6f\x37~>";

    #[test]
    fn decodes_simple_string() {
        let out = decode_ascii85(HELLO_WORLD_A85).unwrap();
        assert_eq!(out, b"Hello World");
    }

    #[test]
    fn decodes_z_shortcut() {
        let out = decode_ascii85(b"z~>").unwrap();
        assert_eq!(out, vec![0, 0, 0, 0]);
    }

    #[test]
    fn ignores_whitespace() {
        let mut encoded = HELLO_WORLD_A85[..HELLO_WORLD_A85.len() - 2].to_vec();
        encoded.insert(4, b'\n');
        encoded.insert(2, b' ');
        encoded.extend_from_slice(b"~>");
        let out = decode_ascii85(&encoded).unwrap();
        assert_eq!(out, b"Hello World");
    }

    #[test]
    fn missing_terminator_is_tolerated() {
        let encoded = &HELLO_WORLD_A85[..HELLO_WORLD_A85.len() - 2];
        let out = decode_ascii85(encoded).unwrap();
        assert_eq!(out, b"Hello World");
    }

    #[test]
    fn rejects_single_char_final_group() {
        // A lone trailing digit is invalid per spec. The core-encoded data
        // is 14 base85 digits (14 % 5 == 4 leftover); appending 2 more
        // digits makes 16 (16 % 5 == 1 leftover), i.e. a final group with
        // only a single digit.
        let mut encoded = HELLO_WORLD_A85[..HELLO_WORLD_A85.len() - 2].to_vec();
        encoded.extend_from_slice(b"!!");
        encoded.extend_from_slice(b"~>");
        let result = decode_ascii85(&encoded);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_invalid_byte() {
        let result = decode_ascii85(b"\x01~>");
        assert!(result.is_err());
    }

    #[test]
    fn empty_input() {
        let out = decode_ascii85(b"~>").unwrap();
        assert!(out.is_empty());
    }
}
