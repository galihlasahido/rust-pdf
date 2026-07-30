//! PDF stream filter implementations (ISO 32000-1:2008 Section 7.4).
//!
//! A PDF stream's `/Filter` entry (a name or array of names) lists the
//! filters that must be applied, in order, to recover the stream's original
//! data; `/DecodeParms` supplies matching per-filter parameters. This
//! module implements each filter as a standalone, panic-free function over
//! untrusted byte slices, plus a small dispatcher ([`decode_filter`]) used
//! by [`crate::object::PdfStream`].
//!
//! All decoders in this module treat their input as untrusted: they never
//! `unwrap`/`expect`/index-panic on attacker-controlled data, and they
//! bound the size of the data they will produce (see [`MAX_DECODED_SIZE`])
//! to defend against decompression-bomb style denial of service.

mod ascii85;
mod ascii_hex;
mod ccitt;
// `dct` depends on the optional `jpeg-decoder` crate (pulled in by the
// `parser` feature); gate the module itself, not just its call site below,
// so `compression`/`images` can build standalone without `parser` (see
// `decode_filter`'s already-gated `DCTDecode` arm for the matching runtime
// behavior when this feature is off).
#[cfg(feature = "jpeg-decoder")]
mod dct;
mod lzw;
mod predictor;
mod run_length;

pub use ascii85::decode_ascii85;
pub use ascii_hex::decode_ascii_hex;
pub use ccitt::{decode_ccitt, CcittParams};
#[cfg(feature = "jpeg-decoder")]
pub use dct::{decode_dct, DctImage};
pub use lzw::decode_lzw;
pub use predictor::{apply_predictor, PredictorParams};
pub use run_length::decode_run_length;

use crate::error::CompressionError;
use crate::object::{Object, PdfDictionary};

/// Maximum permitted size (in bytes) of the data produced by decoding a
/// single filter application. This bounds memory usage against maliciously
/// crafted streams that claim a tiny compressed size but decompress to
/// gigabytes (a "decompression bomb"). 512 MiB is generous for legitimate
/// documents while still bounding worst-case memory use for untrusted
/// input.
pub const MAX_DECODED_SIZE: usize = 512 * 1024 * 1024;

/// Decodes `data` using the named filter and optional `/DecodeParms`
/// dictionary. `DCTDecode` and `CCITTFaxDecode` results are the raw
/// (interleaved / packed) image samples, not further filterable byte
/// streams; per ISO 32000-1 7.4 these are always the last filter in a
/// chain for image data.
pub fn decode_filter(
    name: &str,
    data: &[u8],
    params: Option<&PdfDictionary>,
) -> Result<Vec<u8>, CompressionError> {
    match name {
        "ASCIIHexDecode" | "AHx" => decode_ascii_hex(data),
        "ASCII85Decode" | "A85" => decode_ascii85(data),
        "RunLengthDecode" | "RL" => decode_run_length(data),
        "LZWDecode" | "LZW" => {
            let early_change = params
                .and_then(|d| d.get("EarlyChange"))
                .and_then(as_integer)
                .map(|v| v != 0)
                .unwrap_or(true);
            let decoded = decode_lzw(data, early_change)?;
            apply_stream_predictor(decoded, params)
        }
        #[cfg(feature = "compression")]
        "FlateDecode" | "Fl" => {
            let decoded = decode_flate(data)?;
            apply_stream_predictor(decoded, params)
        }
        #[cfg(not(feature = "compression"))]
        "FlateDecode" | "Fl" => Err(CompressionError::DecompressionFailed(
            "FlateDecode: the `compression` feature is not enabled".to_string(),
        )),
        "DCTDecode" | "DCT" => {
            #[cfg(feature = "jpeg-decoder")]
            {
                Ok(decode_dct(data)?.data)
            }
            #[cfg(not(feature = "jpeg-decoder"))]
            {
                Err(CompressionError::DecompressionFailed(
                    "DCTDecode: the `jpeg-decoder` feature is not enabled".to_string(),
                ))
            }
        }
        "CCITTFaxDecode" | "CCF" => {
            let ccitt_params = ccitt_params_from_dict(params);
            decode_ccitt(data, ccitt_params)
        }
        "Crypt" => {
            // Identity for now: the decrypt step (if any) happens before
            // filters run; a /Crypt filter with a non-Identity /Name is not
            // yet supported.
            Ok(data.to_vec())
        }
        other => Err(CompressionError::DecompressionFailed(format!(
            "unsupported filter: {}",
            other
        ))),
    }
}

#[cfg(feature = "compression")]
fn decode_flate(data: &[u8]) -> Result<Vec<u8>, CompressionError> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    let mut decoder = ZlibDecoder::new(data);
    let mut out = Vec::new();
    // `take` bounds worst-case allocation from a decompression bomb; we
    // read one extra byte to detect truncation vs. exact-size streams.
    let mut limited = (&mut decoder).take(MAX_DECODED_SIZE as u64 + 1);
    limited
        .read_to_end(&mut out)
        .map_err(|e| CompressionError::DecompressionFailed(e.to_string()))?;

    if out.len() > MAX_DECODED_SIZE {
        return Err(CompressionError::DecompressionFailed(
            "FlateDecode: decoded output exceeds maximum allowed size".to_string(),
        ));
    }

    Ok(out)
}

fn apply_stream_predictor(
    data: Vec<u8>,
    params: Option<&PdfDictionary>,
) -> Result<Vec<u8>, CompressionError> {
    let Some(params) = params else {
        return Ok(data);
    };
    let predictor = params.get("Predictor").and_then(as_integer).unwrap_or(1);
    if predictor <= 1 {
        return Ok(data);
    }
    let pred_params = PredictorParams {
        predictor,
        colors: params.get("Colors").and_then(as_integer).unwrap_or(1),
        bits_per_component: params
            .get("BitsPerComponent")
            .and_then(as_integer)
            .unwrap_or(8),
        columns: params.get("Columns").and_then(as_integer).unwrap_or(1),
    };
    apply_predictor(&data, pred_params)
}

fn ccitt_params_from_dict(params: Option<&PdfDictionary>) -> CcittParams {
    let mut p = CcittParams::default();
    let Some(params) = params else {
        return p;
    };
    if let Some(k) = params.get("K").and_then(as_integer) {
        p.k = k;
    }
    if let Some(columns) = params.get("Columns").and_then(as_integer) {
        p.columns = columns;
    }
    if let Some(rows) = params.get("Rows").and_then(as_integer) {
        p.rows = rows;
    }
    if let Some(Object::Boolean(b)) = params.get("BlackIs1") {
        p.black_is_1 = *b;
    }
    if let Some(Object::Boolean(b)) = params.get("EncodedByteAlign") {
        p.encoded_byte_align = *b;
    }
    p
}

fn as_integer(obj: &Object) -> Option<i64> {
    match obj {
        Object::Integer(n) => Some(*n),
        Object::Real(r) => Some(*r as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_ascii_hex() {
        let out = decode_filter("ASCIIHexDecode", b"48656C6C6F>", None).unwrap();
        assert_eq!(out, b"Hello");
    }

    #[test]
    fn dispatches_abbreviated_names() {
        let out = decode_filter("AHx", b"48656C6C6F>", None).unwrap();
        assert_eq!(out, b"Hello");
        let out = decode_filter("RL", &[4u8, b'H', b'e', b'l', b'l', b'o', 128], None).unwrap();
        assert_eq!(out, b"Hello");
    }

    #[test]
    fn unsupported_filter_is_rejected_not_panicking() {
        let result = decode_filter("JBIG2Decode", b"whatever", None);
        assert!(result.is_err());
    }

    #[test]
    fn lzw_applies_predictor_from_params() {
        // No predictor requested -> passthrough of raw LZW output.
        let mut params = PdfDictionary::new();
        params.set("Predictor", Object::Integer(1));
        // Empty LZW stream (immediate EOD, code 257 in 9 bits = 100000001)
        let data = [0b1000_0000, 0b1000_0000];
        let out = decode_filter("LZWDecode", &data, Some(&params));
        assert!(out.is_ok());
    }
}
