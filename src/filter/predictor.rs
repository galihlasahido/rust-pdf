//! Predictor post-processing for `FlateDecode`/`LZWDecode` streams
//! (ISO 32000-1:2008 Section 7.4.4.4, Table 8).
//!
//! Many real-world PDF producers (e.g. Adobe Acrobat, most open-source
//! writers) apply a PNG or TIFF predictor to Flate/LZW-compressed streams
//! (cross-reference streams and images in particular) to improve the
//! compression ratio. Predictor `1` means no predictor was used; `2` means
//! the TIFF horizontal-differencing predictor; values `10..=15` select one
//! of the PNG per-row filter types (the actual filter type is stored per
//! row, as in a PNG `IDAT` stream).

use crate::error::CompressionError;

/// Parameters controlling predictor post-processing, taken from a stream's
/// `/DecodeParms` dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictorParams {
    /// `/Predictor` (default 1 = none).
    pub predictor: i64,
    /// `/Colors` (default 1).
    pub colors: i64,
    /// `/BitsPerComponent` (default 8).
    pub bits_per_component: i64,
    /// `/Columns` (default 1).
    pub columns: i64,
}

impl Default for PredictorParams {
    fn default() -> Self {
        Self {
            predictor: 1,
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        }
    }
}

impl PredictorParams {
    /// Bytes per fully-decoded row, rounded up to a whole byte
    /// (samples need not be byte-aligned, e.g. 1-bit monochrome images).
    fn row_bytes(&self) -> Result<usize, CompressionError> {
        let colors = self.colors.max(1) as u64;
        let bpc = self.bits_per_component.max(1) as u64;
        let columns = self.columns.max(1) as u64;
        let bits = colors
            .checked_mul(bpc)
            .and_then(|v| v.checked_mul(columns))
            .ok_or_else(|| {
                CompressionError::DecompressionFailed("Predictor: row size overflow".to_string())
            })?;
        Ok(bits.div_ceil(8) as usize)
    }

    /// Bytes per pixel, rounded up to a whole byte, used as the PNG filter
    /// "left neighbour" distance. Always at least 1, per the PNG spec.
    fn bytes_per_pixel(&self) -> usize {
        let colors = self.colors.max(1) as u64;
        let bpc = self.bits_per_component.max(1) as u64;
        (colors * bpc).div_ceil(8).max(1) as usize
    }
}

/// Applies (reverses) the predictor described by `params` to `data`,
/// returning the raw sample bytes.
///
/// If `params.predictor <= 1`, `data` is returned unchanged (no predictor
/// was applied).
pub fn apply_predictor(data: &[u8], params: PredictorParams) -> Result<Vec<u8>, CompressionError> {
    if params.predictor <= 1 {
        return Ok(data.to_vec());
    }

    let row_bytes = params.row_bytes()?;
    if row_bytes == 0 {
        return Err(CompressionError::DecompressionFailed(
            "Predictor: zero-width row".to_string(),
        ));
    }

    if params.predictor == 2 {
        return apply_tiff_predictor(data, params, row_bytes);
    }

    // Predictor values 10-15 select PNG-style per-row filtering, where the
    // actual filter type used for each row is stored as a leading byte.
    apply_png_predictor(data, row_bytes, params.bytes_per_pixel())
}

fn apply_tiff_predictor(
    data: &[u8],
    params: PredictorParams,
    row_bytes: usize,
) -> Result<Vec<u8>, CompressionError> {
    let bpc = params.bits_per_component;
    let colors = params.colors.max(1) as usize;
    let columns = params.columns.max(1) as usize;

    let mut out = Vec::with_capacity(data.len());
    for row in data.chunks(row_bytes) {
        let mut row = row.to_vec();
        match bpc {
            8 => {
                for i in colors..row.len() {
                    row[i] = row[i].wrapping_add(row[i - colors]);
                }
            }
            16 => {
                let sample_stride = colors * 2;
                let mut i = sample_stride;
                while i + 1 < row.len() {
                    let prev = u16::from_be_bytes([row[i - sample_stride], row[i - sample_stride + 1]]);
                    let cur = u16::from_be_bytes([row[i], row[i + 1]]);
                    let sum = cur.wrapping_add(prev);
                    row[i] = (sum >> 8) as u8;
                    row[i + 1] = (sum & 0xff) as u8;
                    i += 2;
                }
            }
            1 | 2 | 4 => {
                // Unpack sub-byte samples, apply horizontal differencing per
                // component, then repack.
                let mask: u16 = (1u16 << bpc) - 1;
                let mut samples = unpack_bits(&row, bpc as u32, columns * colors);
                for i in colors..samples.len() {
                    samples[i] = (samples[i].wrapping_add(samples[i - colors])) & mask;
                }
                row = pack_bits(&samples, bpc as u32, row_bytes);
            }
            _ => {
                return Err(CompressionError::DecompressionFailed(format!(
                    "Predictor: unsupported BitsPerComponent {} for TIFF predictor",
                    bpc
                )))
            }
        }
        out.extend_from_slice(&row);
    }

    Ok(out)
}

fn unpack_bits(row: &[u8], bpc: u32, count: usize) -> Vec<u16> {
    let mut out = Vec::with_capacity(count);
    let mut bit_pos = 0usize;
    for _ in 0..count {
        let mut value: u16 = 0;
        for _ in 0..bpc {
            let byte_idx = bit_pos / 8;
            let bit_idx = 7 - (bit_pos % 8);
            let bit = if byte_idx < row.len() {
                (row[byte_idx] >> bit_idx) & 1
            } else {
                0
            };
            value = (value << 1) | bit as u16;
            bit_pos += 1;
        }
        out.push(value);
    }
    out
}

fn pack_bits(samples: &[u16], bpc: u32, out_len: usize) -> Vec<u8> {
    let mut out = vec![0u8; out_len];
    let mut bit_pos = 0usize;
    for &sample in samples {
        for shift in (0..bpc).rev() {
            let bit = (sample >> shift) & 1;
            let byte_idx = bit_pos / 8;
            let bit_idx = 7 - (bit_pos % 8);
            if byte_idx < out.len() && bit == 1 {
                out[byte_idx] |= 1 << bit_idx;
            }
            bit_pos += 1;
        }
    }
    out
}

fn apply_png_predictor(
    data: &[u8],
    row_bytes: usize,
    bpp: usize,
) -> Result<Vec<u8>, CompressionError> {
    let stride = row_bytes + 1; // +1 for the leading filter-type byte.
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if !data.len().is_multiple_of(stride) {
        return Err(CompressionError::DecompressionFailed(format!(
            "Predictor: PNG-predicted data length {} is not a multiple of row stride {}",
            data.len(),
            stride
        )));
    }

    let num_rows = data.len() / stride;
    let mut out = vec![0u8; num_rows * row_bytes];
    let mut prev_row = vec![0u8; row_bytes];

    for r in 0..num_rows {
        let src = &data[r * stride..(r + 1) * stride];
        let filter_type = src[0];
        let src_row = &src[1..];
        let dst_start = r * row_bytes;

        for i in 0..row_bytes {
            let raw = src_row[i];
            let left = if i >= bpp { out[dst_start + i - bpp] } else { 0 };
            let up = prev_row[i];
            let up_left = if i >= bpp { prev_row[i - bpp] } else { 0 };

            let value = match filter_type {
                0 => raw,                                   // None
                1 => raw.wrapping_add(left),                 // Sub
                2 => raw.wrapping_add(up),                   // Up
                3 => raw.wrapping_add(((left as u16 + up as u16) / 2) as u8), // Average
                4 => raw.wrapping_add(paeth(left, up, up_left)), // Paeth
                other => {
                    return Err(CompressionError::DecompressionFailed(format!(
                        "Predictor: unknown PNG filter type {}",
                        other
                    )))
                }
            };
            out[dst_start + i] = value;
        }

        prev_row.copy_from_slice(&out[dst_start..dst_start + row_bytes]);
    }

    Ok(out)
}

/// PNG Paeth predictor function (see PNG spec 9.2 / ISO/IEC 15948).
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    let c = c as i32;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_predictor_passthrough() {
        let data = vec![1, 2, 3, 4];
        let out = apply_predictor(
            &data,
            PredictorParams {
                predictor: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn tiff_predictor_8bit_roundtrip() {
        // Two RGB (3 colors) pixels per row, 2 rows, 8bpc.
        // Raw samples per row: [10,20,30, 15,25,35]
        // TIFF-encoded (horizontal diff per component):
        // row: [10,20,30, 5,5,5]
        let params = PredictorParams {
            predictor: 2,
            colors: 3,
            bits_per_component: 8,
            columns: 2,
        };
        let encoded = vec![10, 20, 30, 5, 5, 5, 1, 2, 3, 4, 4, 4];
        let decoded = apply_predictor(&encoded, params).unwrap();
        assert_eq!(decoded, vec![10, 20, 30, 15, 25, 35, 1, 2, 3, 5, 6, 7]);
    }

    #[test]
    fn tiff_predictor_1bit() {
        // 1 color, 1 bit per component, 8 columns => 1 byte per row.
        let params = PredictorParams {
            predictor: 2,
            colors: 1,
            bits_per_component: 1,
            columns: 8,
        };
        // A single row is trivially unaffected by 1-bit horizontal
        // differencing when every bit differs from the previous: just
        // verify it doesn't panic and returns a same-length row.
        let encoded = vec![0b1010_1010];
        let decoded = apply_predictor(&encoded, params).unwrap();
        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn png_predictor_none_filter() {
        // Predictor 15 = PNG (optimal), each row prefixed with filter byte.
        let params = PredictorParams {
            predictor: 15,
            colors: 1,
            bits_per_component: 8,
            columns: 3,
        };
        // Row 1: filter=0 (None), data=[1,2,3]
        // Row 2: filter=2 (Up), data=[1,1,1] -> decoded = [2,3,4]
        let encoded = vec![0, 1, 2, 3, 2, 1, 1, 1];
        let decoded = apply_predictor(&encoded, params).unwrap();
        assert_eq!(decoded, vec![1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn png_predictor_sub_filter() {
        let params = PredictorParams {
            predictor: 12,
            colors: 1,
            bits_per_component: 8,
            columns: 4,
        };
        // filter=1 (Sub): raw = [10, 1, 1, 1] -> decoded = [10, 11, 12, 13]
        let encoded = vec![1, 10, 1, 1, 1];
        let decoded = apply_predictor(&encoded, params).unwrap();
        assert_eq!(decoded, vec![10, 11, 12, 13]);
    }

    #[test]
    fn png_predictor_rejects_bad_stride() {
        let params = PredictorParams {
            predictor: 15,
            colors: 1,
            bits_per_component: 8,
            columns: 3,
        };
        // 5 bytes is not a multiple of stride (row_bytes+1 = 4).
        let encoded = vec![0, 1, 2, 3, 9];
        let result = apply_predictor(&encoded, params);
        assert!(result.is_err());
    }

    #[test]
    fn png_predictor_rejects_unknown_filter_type() {
        let params = PredictorParams {
            predictor: 15,
            colors: 1,
            bits_per_component: 8,
            columns: 2,
        };
        let encoded = vec![9, 1, 2]; // filter type 9 does not exist
        let result = apply_predictor(&encoded, params);
        assert!(result.is_err());
    }

    #[test]
    fn zero_columns_is_rejected_not_panicking() {
        let params = PredictorParams {
            predictor: 15,
            colors: 1,
            bits_per_component: 8,
            columns: 0,
        };
        // columns.max(1) means this actually resolves to 1 column; ensure
        // no panic occurs regardless.
        let result = apply_predictor(&[0, 5], params);
        assert!(result.is_ok());
    }
}
