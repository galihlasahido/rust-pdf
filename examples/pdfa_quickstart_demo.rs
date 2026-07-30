//! Small, self-contained PDF/A-2b conversion quick start (no external
//! files required), for README.md. See `examples/pdfa_convert_demo.rs`
//! for a fuller demo that uses a real vendored ICC profile/font and
//! cross-checks the output against the veraPDF CLI.
//!
//! Run with:
//! ```text
//! cargo run --features parser --example pdfa_quickstart_demo
//! ```

use rust_pdf::editor::{PdfAConversionOptions, PdfAFlavor};
use rust_pdf::prelude::*;

/// A minimal, syntactically valid ICC.1 header (the 128-byte fixed
/// header plus a little slack), enough to exercise the `/OutputIntent`
/// embedding mechanics end-to-end. Real applications should embed a real
/// vendored profile (e.g. `sRGB2014.icc`); this crate deliberately never
/// bundles or synthesizes one for production use (see `src/editor/icc.rs`).
fn synthetic_srgb_icc_profile() -> Vec<u8> {
    let mut data = vec![0u8; 200];
    let size = (data.len() as u32).to_be_bytes();
    data[0..4].copy_from_slice(&size);
    data[12..16].copy_from_slice(b"mntr");
    data[16..20].copy_from_slice(b"RGB ");
    data[20..24].copy_from_slice(b"XYZ ");
    data[36..40].copy_from_slice(b"acsp");
    data
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(ContentBuilder::new().text("F1", 14.0, 72.0, 760.0, "Archival copy"))
        .build();
    let bytes = DocumentBuilder::new().title("Archival copy").page(page).build()?.save_to_bytes()?;

    let mut doc = EditableDocument::from_bytes(bytes)?;

    let icc_profile = synthetic_srgb_icc_profile();
    let options = PdfAConversionOptions {
        icc_profile: &icc_profile,
        icc_identifier: "sRGB IEC61966-2.1",
        icc_condition: "sRGB",
        title: Some("Archival copy"),
        producer: Some("rust-pdf"),
    };
    let summary = doc.convert_to_pdfa(PdfAFlavor::Part2B, &options)?;
    println!("PDF/A-2b conversion summary: {summary:?}");

    let pdfa_bytes = doc.save_pdfa_compatible_to_bytes(PdfAFlavor::Part2B.min_pdf_version())?;
    std::fs::write("tests/output/quickstart_pdfa2b.pdf", &pdfa_bytes)?;

    // Re-open and validate against this crate's own (explicitly
    // partial - see src/editor/pdfa.rs module docs) PDF/A-2b rule set.
    let reopened = EditableDocument::from_bytes(pdfa_bytes)?;
    let report = reopened.validate_pdfa(PdfAFlavor::Part2B)?;
    println!("is_conformant (this crate's subset of ISO 19005-2 rules): {}", report.is_conformant());
    for violation in &report.violations {
        println!("  violation: {violation:?}");
    }

    Ok(())
}
