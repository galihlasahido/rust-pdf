//! Fuzz target for content-stream inline image parsing
//! (`BI ... ID ... EI`, ISO 32000-1 8.9.7), including its `EI` scanning
//! heuristic which walks raw, attacker-controlled binary data looking for
//! a whitespace-delimited terminator.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rust_pdf::parse_inline_image;

fuzz_target!(|data: &[u8]| {
    let _ = parse_inline_image(data);
});
