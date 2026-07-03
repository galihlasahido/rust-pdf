//! Demonstrates the read-only [`PdfReader`] API (README.md's "Reading
//! existing PDFs" quick start) - memory-mapped via `PdfReader::from_file`
//! so opening a large PDF does not require reading the whole file into
//! process memory (see `src/parser/mod.rs`'s module docs).
//!
//! Run with:
//! ```text
//! cargo run --features parser --example read_existing_pdf_demo
//! ```

use rust_pdf::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    // Build a small sample document to read back.
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(ContentBuilder::new().text("F1", 14.0, 72.0, 760.0, "Sample document"))
        .build();
    DocumentBuilder::new()
        .title("Sample document")
        .page(page)
        .build()?
        .save_to_file("tests/output/read_demo_source.pdf")?;

    let reader = PdfReader::from_file("tests/output/read_demo_source.pdf")?;

    println!("Pages: {}", reader.page_count());
    println!("Version: {:?}", reader.version());

    let trailer = reader.trailer();
    println!("Root: {:?}", trailer.root);

    let catalog = reader.catalog().ok_or("document has no /Root catalog")?;
    println!("Catalog keys: {:?}", catalog.iter().map(|(k, _)| k).collect::<Vec<_>>());

    Ok(())
}
