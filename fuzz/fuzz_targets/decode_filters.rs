//! Fuzz target for the stream filter decoders (ISO 32000-1 7.4):
//! ASCIIHexDecode, ASCII85Decode, RunLengthDecode, LZWDecode, FlateDecode,
//! DCTDecode and CCITTFaxDecode. These all run directly on
//! attacker-controlled bytes taken straight from a PDF file, so the only
//! acceptable outcomes are `Ok` or a clean `Err` - never a panic, hang, or
//! unbounded allocation.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rust_pdf::filter::{decode_filter, CcittParams};
use rust_pdf::object::{Object, PdfDictionary};

const FILTERS: &[&str] = &[
    "ASCIIHexDecode",
    "ASCII85Decode",
    "RunLengthDecode",
    "LZWDecode",
    "FlateDecode",
    "DCTDecode",
    "CCITTFaxDecode",
];

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let selector = data[0] as usize % FILTERS.len();
    let filter = FILTERS[selector];
    let payload = &data[1..];

    // Plain decode, no params.
    let _ = decode_filter(filter, payload, None);

    // Also exercise the predictor/DecodeParms-aware path for filters that
    // support it, and CCITTFaxDecode's own parameter dictionary, using
    // small fixed values so the fuzzer's time budget goes toward the
    // decoder logic rather than parameter-space exploration.
    let mut params = PdfDictionary::new();
    params.set("Predictor", Object::Integer(12));
    params.set("Colors", Object::Integer(1));
    params.set("BitsPerComponent", Object::Integer(8));
    params.set("Columns", Object::Integer(4));
    params.set("K", Object::Integer(-1));
    params.set("Rows", Object::Integer(4));
    let _ = decode_filter(filter, payload, Some(&params));

    if filter == "CCITTFaxDecode" {
        let ccitt_params = CcittParams {
            k: -1,
            columns: 8,
            rows: 8,
            black_is_1: payload.first().is_some_and(|b| b % 2 == 0),
            encoded_byte_align: payload.first().is_some_and(|b| b % 3 == 0),
        };
        let _ = rust_pdf::filter::decode_ccitt(payload, ccitt_params);
    }
});
