//! Fuzz target for the top-level PDF reader (`PdfReader::from_bytes`),
//! which is the entry point exercising header parsing, xref
//! table/stream/hybrid parsing, the `/Prev` chain, object-stream
//! resolution, filter decoding, and recovery-mode reconstruction.
//!
//! The goal is simply: no panics, no OOM, no hangs, on arbitrary bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rust_pdf::PdfReader;

fuzz_target!(|data: &[u8]| {
    if let Ok(reader) = PdfReader::from_bytes(data.to_vec()) {
        // Touch the surface API a real caller would use, so that bugs
        // reachable only after a successful open are found too.
        let _ = reader.page_count();
        let _ = reader.catalog();
        let _ = reader.info();
        let _ = reader.version();

        // Try resolving a handful of low object numbers directly; this
        // exercises `resolve_reference`/`resolve_compressed_object`
        // (including their bounds checks) even for documents whose page
        // tree doesn't happen to reference every object.
        for n in 0..32u32 {
            let _ = reader.resolve_reference(rust_pdf::ObjectId::new(n));
        }
    }
});
