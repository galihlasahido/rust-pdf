//! LZWDecode filter (ISO 32000-1:2008 Section 7.4.4).
//!
//! PDF uses a variant of the LZW compression algorithm (the same variable-
//! width code stream as TIFF's LZW). Codes are packed MSB-first starting at
//! 9 bits wide, growing up to 12 bits as the code table fills. Code `256` is
//! the clear-table marker and `257` is the end-of-data marker. The
//! `/EarlyChange` decode parameter (default `1`, see Table 8) controls
//! whether the code width grows one code early.

use crate::error::CompressionError;
use crate::filter::MAX_DECODED_SIZE;

const CLEAR_TABLE: u16 = 256;
const EOD: u16 = 257;
const MAX_CODE_WIDTH: u8 = 12;
const MAX_TABLE_SIZE: usize = 1 << MAX_CODE_WIDTH; // 4096

/// A simple MSB-first variable-width bit reader over a byte slice.
struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    /// Reads `n` bits (n <= 16), MSB-first. Returns `None` at end of input.
    fn read_bits(&mut self, n: u8) -> Option<u16> {
        let mut result: u16 = 0;
        for _ in 0..n {
            if self.byte_pos >= self.data.len() {
                return None;
            }
            let byte = self.data[self.byte_pos];
            let bit = (byte >> (7 - self.bit_pos)) & 1;
            result = (result << 1) | bit as u16;
            self.bit_pos += 1;
            if self.bit_pos == 8 {
                self.bit_pos = 0;
                self.byte_pos += 1;
            }
        }
        Some(result)
    }
}

/// Decodes data encoded with the `LZWDecode` filter.
///
/// `early_change` corresponds to the `/EarlyChange` DecodeParms entry
/// (default `true`, i.e. value `1`).
pub fn decode_lzw(data: &[u8], early_change: bool) -> Result<Vec<u8>, CompressionError> {
    let mut reader = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();

    // table[code] = the byte string that code represents. Codes 0..=255 are
    // literal single bytes; 256/257 are control codes; 258.. are built
    // dynamically.
    let mut table: Vec<Vec<u8>> = Vec::with_capacity(MAX_TABLE_SIZE);
    let init_table = |t: &mut Vec<Vec<u8>>| {
        t.clear();
        for b in 0u16..256 {
            t.push(vec![b as u8]);
        }
        t.push(Vec::new()); // 256: clear table (unused as data)
        t.push(Vec::new()); // 257: EOD (unused as data)
    };
    init_table(&mut table);

    let mut code_width: u8 = 9;
    let mut prev: Option<Vec<u8>> = None;

    let early = if early_change { 1usize } else { 0 };

    // Tolerate missing EOD on truncated streams: `read_bits` returning
    // `None` simply ends decoding with whatever was recovered so far.
    while let Some(code) = reader.read_bits(code_width) {
        if code == CLEAR_TABLE {
            init_table(&mut table);
            code_width = 9;
            prev = None;
            continue;
        }

        if code == EOD {
            break;
        }

        let entry: Vec<u8> = if (code as usize) < table.len() {
            table[code as usize].clone()
        } else if code as usize == table.len() {
            // Special case: code refers to the entry about to be added.
            match &prev {
                Some(p) if !p.is_empty() => {
                    let mut e = p.clone();
                    e.push(p[0]);
                    e
                }
                _ => {
                    return Err(CompressionError::DecompressionFailed(
                        "LZWDecode: invalid code sequence at stream start".to_string(),
                    ))
                }
            }
        } else {
            return Err(CompressionError::DecompressionFailed(format!(
                "LZWDecode: code {} out of range (table size {})",
                code,
                table.len()
            )));
        };

        out.extend_from_slice(&entry);
        if out.len() > MAX_DECODED_SIZE {
            return Err(CompressionError::DecompressionFailed(
                "LZWDecode: decoded output exceeds maximum allowed size".to_string(),
            ));
        }

        if let Some(p) = &prev {
            if table.len() < MAX_TABLE_SIZE {
                let mut new_entry = p.clone();
                new_entry.push(entry[0]);
                table.push(new_entry);

                // Early-change: bump the code width one code sooner than
                // the table size would otherwise require.
                let table_len = table.len() + early;
                if table_len == 512 && code_width == 9 {
                    code_width = 10;
                } else if table_len == 1024 && code_width == 10 {
                    code_width = 11;
                } else if table_len == 2048 && code_width == 11 {
                    code_width = 12;
                }
            }
        }

        prev = Some(entry);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips a small buffer through a minimal PDF-compatible LZW
    /// encoder (independent, simplified implementation used only for
    /// testing) and verifies `decode_lzw` recovers the original bytes.
    fn encode_lzw(data: &[u8], early_change: bool) -> Vec<u8> {
        struct BitWriter {
            out: Vec<u8>,
            cur: u32,
            nbits: u8,
        }
        impl BitWriter {
            fn push(&mut self, code: u16, width: u8) {
                self.cur = (self.cur << width) | code as u32;
                self.nbits += width;
                while self.nbits >= 8 {
                    let shift = self.nbits - 8;
                    self.out.push((self.cur >> shift) as u8);
                    self.nbits -= 8;
                    self.cur &= (1 << self.nbits) - 1;
                }
            }
            fn finish(mut self) -> Vec<u8> {
                if self.nbits > 0 {
                    let shift = 8 - self.nbits;
                    self.out.push((self.cur << shift) as u8);
                }
                self.out
            }
        }

        let mut bw = BitWriter {
            out: Vec::new(),
            cur: 0,
            nbits: 0,
        };
        let mut table: std::collections::HashMap<Vec<u8>, u16> = (0u16..256)
            .map(|b| (vec![b as u8], b))
            .collect();
        let mut next_code: u16 = 258;
        let mut width: u8 = 9;
        let early = if early_change { 1u16 } else { 0 };

        bw.push(CLEAR_TABLE, width);
        let mut w: Vec<u8> = Vec::new();
        for &byte in data {
            let mut wc = w.clone();
            wc.push(byte);
            if table.contains_key(&wc) {
                w = wc;
            } else {
                bw.push(*table.get(&w).unwrap(), width);
                if next_code < 4096 {
                    table.insert(wc, next_code);
                    next_code += 1;
                }
                if next_code + early == 512 {
                    width = 10;
                } else if next_code + early == 1024 {
                    width = 11;
                } else if next_code + early == 2048 {
                    width = 12;
                } else if next_code == 4096 {
                    bw.push(CLEAR_TABLE, width);
                    table = (0u16..256).map(|b| (vec![b as u8], b)).collect();
                    next_code = 258;
                    width = 9;
                }
                w = vec![byte];
            }
        }
        if !w.is_empty() {
            bw.push(*table.get(&w).unwrap(), width);
        }
        bw.push(EOD, width);
        bw.finish()
    }

    #[test]
    fn roundtrip_simple_text_early_change() {
        let data = b"TOBEORNOTTOBEORTOBEORNOT".to_vec();
        let encoded = encode_lzw(&data, true);
        let decoded = decode_lzw(&encoded, true).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn roundtrip_no_early_change() {
        let data = b"AAAAAAAAAAAAAAAAAAAABBBBBBBBBBBBBBBBBBBB".to_vec();
        let encoded = encode_lzw(&data, false);
        let decoded = decode_lzw(&encoded, false).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn roundtrip_repetitive_data_grows_table() {
        let data: Vec<u8> = (0..2000).map(|i| (i % 17) as u8).collect();
        let encoded = encode_lzw(&data, true);
        let decoded = decode_lzw(&encoded, true).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let out = decode_lzw(&[], true).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn invalid_code_at_start_is_rejected() {
        // First code after an implicit clear is 258 (0b100000010 in 9
        // bits), which cannot be valid since nothing has been added to the
        // table yet.
        let data = [0b1000_0001, 0b0000_0000];
        let result = decode_lzw(&data, true);
        assert!(result.is_err());
    }

    #[test]
    fn truncated_stream_is_tolerated_not_panicking() {
        // A single incomplete 9-bit code: should not panic, just stop.
        let data = [0xFFu8];
        let result = decode_lzw(&data, true);
        assert!(result.is_ok());
    }
}
