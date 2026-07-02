//! Produces sample PDF/A-1b, PDF/A-2b, PDF/A-3b, a PDF/UA-1 sample, and a
//! PDF/X colour-space check document for manual, real-tool verification
//! (e.g. against the veraPDF CLI) of
//! `src/editor/pdfa.rs`/`pdfx.rs`/`pdfua.rs`.
//!
//! This is a **verification aid, not a unit test**: it embeds a real
//! TrueType font read from the local filesystem (so the "fonts embedded"
//! PDF/A rule is exercised against genuine glyph outlines, not the
//! crate's empty-outline test fixture font) and an ICC profile likewise
//! read from disk, neither of which this crate bundles (see
//! `src/editor/icc.rs`'s module docs for why). Run with:
//!
//! ```text
//! cargo run --features full --example pdfa_convert_demo -- \
//!     <path-to-a-ttf-font> <path-to-an-rgb-icc-profile> <path-to-a-cmyk-icc-profile> [output-dir]
//! ```
//!
//! then check the output files with an external validator, e.g.:
//!
//! ```text
//! verapdf --flavour 1b tests/output/pdfa_vector_demo_1b.pdf
//! verapdf --flavour 2b tests/output/pdfa_vector_demo_2b.pdf
//! verapdf --flavour 3b tests/output/pdfa_vector_demo_3b.pdf
//! verapdf --flavour ua1 tests/output/pdfua_demo.pdf
//! ```
//!
//! # Why PDF/A and PDF/UA output are separate files here
//!
//! [`EditableDocument::convert_to_pdfa`] and
//! [`EditableDocument::prepare_for_pdfua`] are independent, composable
//! features, but combining *both* schemas (`pdfaid` and `pdfuaid`) into
//! one XMP packet and having it validate clean against **both** external
//! profiles turned out to need more than this pass implements: ISO
//! 19005-1:2005 6.7.9 requires every XMP property either belong to a
//! well-known schema or be declared through the PDF/A "Extension Schema"
//! mechanism (a `pdfaExtension:schemas` structure describing the
//! property), and this crate's XMP writer doesn't implement that
//! mechanism (discovered - not anticipated - by running an
//! both-schemas-at-once file through the real veraPDF CLI while building
//! this demo: PDF/A-1b/2b/3b all failed 6.7.9 on the `pdfuaid:part`
//! property the moment PDF/UA tagging was added to the same document).
//! Rather than ship a file that quietly fails one of the two profiles,
//! this demo produces separate `pdfa_*` (PDF/A only) and `pdfua_demo.pdf`
//! (PDF/UA only) samples. Implementing the Extension Schema mechanism so
//! a single file can genuinely claim both is a real, scoped follow-up
//! (estimated at a few hours: it's a fixed, well-documented XMP structure,
//! not a new validation concept).
//!
//! Two independent PDF/A base documents are produced:
//! - `pdfa_demo_*b.pdf`: has genuinely embedded text. Deliberately kept
//!   even though it currently trips [`EditableDocument::validate_pdfa`]'s
//!   `/CIDSet` check (see [`build_text_document`]'s doc comment) - a real,
//!   honestly-reported gap in this crate's font subsetter, not something
//!   to quietly avoid.
//! - `pdfa_vector_demo_*b.pdf`: vector-only (no fonts at all), which
//!   sidesteps that gap entirely and is the sample this repo's task
//!   report cites as verified clean against the real veraPDF CLI.

// Everything below needs `EditableDocument`/`PdfAFlavor`/... (the
// `parser` feature) and `CompositeFont` (the `fonts` feature), both of
// which `--features full` enables; `cargo build`/`cargo test` without
// `--features full` would otherwise fail to compile this example simply
// for lacking those types, which is a build-graph problem unrelated to
// this example's own logic. Gating the real implementation behind the
// same condition (with a plain, always-compiling stub `main` for
// everything else) keeps `cargo build`/`cargo test` green regardless of
// which features are enabled, matching this crate's other feature-gated
// examples (e.g. `digital_signature_example.rs`).
#[cfg(all(feature = "parser", feature = "fonts"))]
use rust_pdf::font::CompositeFont;
#[cfg(all(feature = "parser", feature = "fonts"))]
use rust_pdf::prelude::*;
#[cfg(all(feature = "parser", feature = "fonts"))]
use std::path::{Path, PathBuf};

#[cfg(not(all(feature = "parser", feature = "fonts")))]
fn main() {
    eprintln!("pdfa_convert_demo requires --features full (needs at least `parser` and `fonts`)");
    std::process::exit(1);
}

#[cfg(all(feature = "parser", feature = "fonts"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: {} <ttf-font-path> <rgb-icc-path> <cmyk-icc-path> [output-dir]", args[0]);
        std::process::exit(2);
    }
    let font_path = &args[1];
    let rgb_icc_path = &args[2];
    let cmyk_icc_path = &args[3];
    let output_dir = PathBuf::from(args.get(4).cloned().unwrap_or_else(|| "tests/output".to_string()));
    std::fs::create_dir_all(&output_dir)?;

    let font_bytes = std::fs::read(font_path)?;
    let rgb_icc = std::fs::read(rgb_icc_path)?;
    let cmyk_icc = std::fs::read(cmyk_icc_path)?;

    let text_base_bytes = build_text_document(&font_bytes)?;
    run_pdfa_flavors(&text_base_bytes, "pdfa_demo", &rgb_icc, &output_dir)?;

    let vector_base_bytes = build_vector_only_document()?;
    run_pdfa_flavors(&vector_base_bytes, "pdfa_vector_demo", &rgb_icc, &output_dir)?;

    // --- PDF/UA-1-only sample (see module docs for why it's separate) --
    {
        let mut doc = EditableDocument::from_bytes(vector_base_bytes.clone())?;
        tag_for_accessibility(&mut doc)?;
        let saved = doc.save_full_rewrite_to_bytes()?;
        let out_path = output_dir.join("pdfua_demo.pdf");
        std::fs::write(&out_path, &saved)?;
        println!("[PDF/UA] wrote {}", out_path.display());

        let reopened = EditableDocument::from_bytes(saved)?;
        let ua_report = reopened.validate_pdfua()?;
        println!("[PDF/UA] internal validate_pdfua: conformant={}", ua_report.is_conformant());
        for v in &ua_report.violations {
            println!("    - [{}] {}", v.rule, v.message);
        }
    }

    // --- PDF/X colour-space check sample --------------------------------
    {
        let mut doc = EditableDocument::from_bytes(vector_base_bytes)?;
        doc.add_pdfx_output_intent(&cmyk_icc, "Coated FOGRA39", "Commercial offset, coated paper")?;

        // Deliberately non-conformant: a DeviceRGB-filled rectangle.
        let page_id = doc.page_id_at(0)?;
        let rgb_content = ContentBuilder::new().save_state().fill_color(Color::rgb(0.8, 0.1, 0.1)).rect(72.0, 650.0, 150.0, 60.0).fill().restore_state();
        doc.append_page_content(page_id, &rgb_content)?;

        let report = doc.validate_pdfx_color()?;
        println!("[PDF/X] initial (has DeviceRGB content): conformant={}", report.is_conformant());
        for v in &report.violations {
            println!("    - [{}] {}", v.rule, v.message);
        }

        // Replace the RGB-filled rectangle with a CMYK-filled one to
        // demonstrate the conformant case too (the RGB one drawn above is
        // still on the page - only appended alongside, not removed - so
        // this is still expected to fail; see the printed message).
        let cmyk_content = ContentBuilder::new().save_state().fill_color(Color::cmyk(0.6, 0.2, 0.0, 0.1)).rect(360.0, 700.0, 150.0, 60.0).fill().restore_state();
        doc.append_page_content(page_id, &cmyk_content)?;
        let saved = doc.save_full_rewrite_to_bytes()?;
        let report_after = EditableDocument::from_bytes(saved.clone())?.validate_pdfx_color()?;
        println!("[PDF/X] after adding a CMYK-only swatch (RGB swatch still present too): conformant={}", report_after.is_conformant());
        for v in &report_after.violations {
            println!("    - [{}] {}", v.rule, v.message);
        }
        let out_path = output_dir.join("pdfx_color_demo.pdf");
        std::fs::write(&out_path, &saved)?;
        println!("[PDF/X] wrote {}", out_path.display());
    }

    Ok(())
}

/// Converts `base_bytes` to PDF/A-1b/-2b/-3b, saves each, writes
/// `<output_dir>/<label>_<part>b.pdf`, and prints this crate's own
/// `validate_pdfa` report for each - a quick sanity signal to read before
/// running an external validator against the same files. Deliberately
/// does **not** also apply PDF/UA tagging - see the module docs for why.
#[cfg(all(feature = "parser", feature = "fonts"))]
fn run_pdfa_flavors(base_bytes: &[u8], label: &str, rgb_icc: &[u8], output_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for flavor in [PdfAFlavor::Part1B, PdfAFlavor::Part2B, PdfAFlavor::Part3B] {
        let mut doc = EditableDocument::from_bytes(base_bytes.to_vec())?;

        let options = PdfAConversionOptions {
            icc_profile: rgb_icc,
            icc_identifier: "sRGB IEC61966-2.1",
            icc_condition: "sRGB",
            title: Some("rust-pdf PDF/A conformance sample"),
            producer: Some("rust-pdf"),
        };
        let summary = doc.convert_to_pdfa(flavor, &options)?;
        println!("[{label} {flavor:?}] conversion summary: {summary:?}");

        let saved = doc.save_pdfa_compatible_to_bytes(flavor.min_pdf_version())?;
        let out_path = output_dir.join(format!("{label}_{}b.pdf", flavor.part_number()));
        std::fs::write(&out_path, &saved)?;
        println!("[{label} {flavor:?}] wrote {}", out_path.display());

        let reopened = EditableDocument::from_bytes(saved)?;
        let pdfa_report = reopened.validate_pdfa(flavor)?;
        println!("[{label} {flavor:?}] internal validate_pdfa: conformant={}", pdfa_report.is_conformant());
        for v in &pdfa_report.violations {
            println!("    - [{}] {}", v.rule, v.message);
        }
    }
    Ok(())
}

/// Builds an untagged PDF 1.4 document with one page: a heading and a
/// paragraph in a genuinely embedded TrueType font, and a DeviceRGB
/// swatch (deliberately non-PDF/X-conformant colour - unrelated to this
/// function's own PDF/A round trip, but reused as the PDF/X demo's base).
#[cfg(all(feature = "parser", feature = "fonts"))]
fn build_text_document(font_bytes: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Subsetting deliberately disabled: this crate's subsetter
    // (`crate::font::cid::CompositeFont::build`) does not yet emit the
    // `/CIDSet` stream ISO 19005-1:2005 6.3.5 requires on *subset* CID
    // fonts - a real gap discovered while building `src/editor/pdfa.rs`'s
    // validator against the real veraPDF CLI (see that module's
    // `check_cidset_present` doc comment). A full (non-subset) embed
    // would normally sidestep 6.3.5 (it only applies to subsets), except
    // this crate's tagged-subset-style `BaseFont` naming
    // (`format!("{subset_tag}+{name}")` in `CompositeFont::build`) is
    // applied unconditionally, regardless of whether the font program was
    // actually subset - so veraPDF's own subset-detection heuristic
    // (looking for that same 6-uppercase-letter-plus-name pattern) still
    // fires here even with subsetting off. `subset(false)` is kept anyway
    // (smaller effective diff from a "real" embed, and it's still what a
    // caller should reach for once the naming is fixed); the vector-only
    // document below is what actually verifies clean.
    let composite = CompositeFont::new(font_bytes.to_vec(), "DemoFont")?.subset(false);

    let heading = composite.encode("PDF/A Conformance Sample");
    let body = composite.encode("This document is generated by rust-pdf's PDF/A converter for external validation.");

    let content = ContentBuilder::new()
        .text_block(TextBuilder::new().font("F1", 20.0).position(72.0, 760.0).show_bytes(heading))
        .text_block(TextBuilder::new().font("F1", 11.0).position(72.0, 720.0).show_bytes(body))
        .save_state()
        .fill_color(Color::rgb(0.8, 0.1, 0.1))
        .rect(72.0, 650.0, 150.0, 60.0)
        .fill()
        .restore_state();

    let page = PageBuilder::a4().font("F1", Font::Composite(composite)).content(content).build();
    let bytes = DocumentBuilder::new().version(PdfVersion::V1_4).title("rust-pdf PDF/A conformance sample").page(page).build()?.save_to_bytes()?;
    Ok(bytes)
}

/// Builds an untagged, otherwise **blank** PDF 1.4 document with one page
/// and no font resource at all (so no font-embedding rule, including the
/// `/CIDSet` gap noted on [`build_text_document`], is exercised).
#[cfg(all(feature = "parser", feature = "fonts"))]
fn build_vector_only_document() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let page = PageBuilder::a4().content(ContentBuilder::new()).build();
    let bytes = DocumentBuilder::new().version(PdfVersion::V1_4).title("rust-pdf PDF/A conformance sample").page(page).build()?.save_to_bytes()?;
    Ok(bytes)
}

/// Applies this crate's PDF/UA-lite tagging (see
/// `src/editor/pdfua.rs`/`structure.rs`): document language,
/// `DisplayDocTitle`, and a tagged `Figure` (with alt text) structure
/// element whose marked-content span *is* the page's only visible
/// content (a filled DeviceRGB rectangle) - drawn here, via
/// `add_tagged_content`, rather than by the base-document builder, so
/// there is no untagged visible content left on the page (ISO
/// 14289-1:2014 7.1's "every bit of content is either tagged or an
/// artifact" requirement - found to matter in practice by running this
/// demo's first version through the real veraPDF `ua1` flavour, where a
/// separately-drawn, untagged rectangle failed exactly this check).
#[cfg(all(feature = "parser", feature = "fonts"))]
fn tag_for_accessibility(doc: &mut EditableDocument) -> Result<(), Box<dyn std::error::Error>> {
    doc.prepare_for_pdfua("en-US")?;
    let root = doc.add_document_structure_root()?;
    let figure_content = ContentBuilder::new().save_state().fill_color(Color::rgb(0.8, 0.1, 0.1)).rect(72.0, 650.0, 150.0, 60.0).fill().restore_state();
    doc.add_tagged_content(0, Some(root), StructType::Figure, &figure_content, Some("A red rectangle used as this sample's sole graphic"))?;
    Ok(())
}
