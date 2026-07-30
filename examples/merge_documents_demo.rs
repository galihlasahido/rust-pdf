//! Demonstrates merging two documents with `EditableDocument::append_document`,
//! including that the source document's bookmark/outline tree is imported
//! too (with page-index destinations remapped to where those pages land in
//! the merged document), not just its pages.
//!
//! Run with:
//! ```text
//! cargo run --features parser --example merge_documents_demo
//! ```

use rust_pdf::editor::Destination;
use rust_pdf::prelude::*;

fn build_doc(title: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(ContentBuilder::new().text("F1", 14.0, 72.0, 760.0, title))
        .build();
    Ok(DocumentBuilder::new()
        .title(title)
        .page(page)
        .build()?
        .save_to_bytes()?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    let mut report = EditableDocument::from_bytes(build_doc("Q1 Report")?)?;
    report.add_bookmark(None, "Q1 Report", Destination::fit(0))?;

    let mut appendix = EditableDocument::from_bytes(build_doc("Appendix A")?)?;
    appendix.add_bookmark(None, "Appendix A", Destination::fit(0))?;

    // Appends `appendix`'s pages after `report`'s own -- and, since
    // `append_document` also imports the source's outline tree, its
    // "Appendix A" bookmark comes along too, with its page-0 destination
    // remapped to page 1 (where it actually lands in the merged document).
    report.append_document(&appendix)?;

    assert_eq!(report.page_count()?, 2);
    let bookmarks = report.list_bookmarks()?;
    assert_eq!(bookmarks.len(), 2, "Q1 Report + Appendix A, both top-level");
    assert_eq!(bookmarks[0].title, "Q1 Report");
    assert_eq!(bookmarks[1].title, "Appendix A");
    println!(
        "merged: {} pages, bookmarks = {:?}",
        report.page_count()?,
        bookmarks.iter().map(|b| &b.title).collect::<Vec<_>>()
    );

    report.save_full_rewrite("tests/output/merged_report.pdf")?;
    println!("wrote tests/output/merged_report.pdf");
    Ok(())
}
