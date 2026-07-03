//! Demonstrates the "edit an existing PDF" workflow documented in
//! README.md's Quick Start: open a generated document with
//! [`EditableDocument`], add a bookmark/outline entry, tag a paragraph
//! for the logical structure tree (Tagged PDF, ISO 32000-1 14.7/14.8),
//! redact an area, add a highlight annotation, save incrementally, then
//! rasterize page 0 with the pure-Rust [`rust_pdf::render::PdfRenderer`].
//!
//! Run with:
//! ```text
//! cargo run --features "parser fonts render" --example render_and_edit_demo
//! ```

use rust_pdf::editor::{Destination, StructType};
use rust_pdf::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    // 1. Build a small source document with plain rust-pdf/document APIs.
    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 760.0, "Quarterly Report")
        .text("F1", 12.0, 72.0, 700.0, "Confidential customer id: CUST-004821");
    let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(content).build();
    let bytes = DocumentBuilder::new().title("Quarterly Report").page(page).build()?.save_to_bytes()?;

    // 2. Re-open it for structural editing.
    let mut doc = EditableDocument::from_bytes(bytes)?;

    // Outline/bookmark (ISO 32000-1 12.3.3).
    doc.add_bookmark(None, "Quarterly Report", Destination::fit(0))?;

    // Tagged structure: wrap the heading in a /H1 structure element
    // (ISO 32000-1 14.7/14.8, the basis for PDF/UA).
    doc.add_tagged_content(
        0,
        None,
        StructType::Heading(1),
        &ContentBuilder::new().text("F1", 18.0, 72.0, 760.0, "Quarterly Report"),
        None,
    )?;

    // Permanently redact the customer id line (text + any image in the
    // rectangle are actually removed from the object graph, not just
    // covered - see src/editor/redact.rs).
    doc.apply_redaction(
        0,
        Rectangle::new(72.0, 690.0, 400.0, 712.0),
        "compliance-bot",
        "PII removed before external distribution",
    )?;

    // Highlight annotation (ISO 32000-1 12.5.6.10).
    doc.add_highlight_annotation(0, &[(72.0, 755.0, 300.0, 772.0)], Color::rgb(1.0, 1.0, 0.0))?;

    // Redaction forces a full rewrite (an incremental update would leave
    // the pre-redaction bytes recoverable in the file's earlier
    // revision) - see EditableDocument::save_incremental's docs.
    doc.save_full_rewrite("tests/output/edited_report.pdf")?;
    println!("wrote tests/output/edited_report.pdf");

    for entry in doc.audit_log() {
        println!("audit: {} redacted {} text run(s) - {}", entry.actor, entry.text_runs_removed, entry.reason);
    }

    // 3. Render page 0 of the edited file with the pure-Rust rasterizer
    //    (no native/FFI dependency - see src/render/mod.rs).
    #[cfg(feature = "render")]
    {
        let renderer = rust_pdf::render::PdfRenderer::open_file("tests/output/edited_report.pdf")?;
        let image = renderer.render_page(0, 150.0, None)?;
        image.save("tests/output/edited_report_page0.png")?;
        println!("wrote tests/output/edited_report_page0.png ({}x{})", image.width(), image.height());
    }

    Ok(())
}
