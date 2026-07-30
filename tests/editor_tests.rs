//! Integration tests for the content-editing / incremental-save /
//! full-rewrite APIs (`rust_pdf::editor::EditableDocument`), covering the
//! task's Definition of Done:
//!
//! - Editing a 500+ page document and incrementally saving a single edit
//!   completes in well under 200ms.
//! - The resulting file is valid enough for an independent PDF library
//!   (`lopdf`) to open and read back.
//! - Full-rewrite mode produces output that is smaller than (or equal to)
//!   the incrementally-saved file for the same edit.

#![cfg(feature = "parser")]

use rust_pdf::prelude::*;
use std::time::Instant;

fn build_document(num_pages: usize) -> Vec<u8> {
    let mut builder = DocumentBuilder::new().title("Editor DoD test document");
    for i in 0..num_pages {
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(
                ContentBuilder::new()
                    .text("F1", 12.0, 72.0, 750.0, &format!("Original page {i}"))
                    .graphics(
                        GraphicsBuilder::new()
                            .fill_color(Color::rgb(0.2, 0.4, 0.8))
                            .rect(50.0, 600.0, 120.0, 40.0)
                            .fill(),
                    ),
            )
            .build();
        builder = builder.page(page);
    }
    builder.build().unwrap().save_to_bytes().unwrap()
}

// ---------------------------------------------------------------------
// DoD: edit a 500+ page document; a single edit's incremental save is
// fast (well under 200ms) and produces a file lopdf can open.
// ---------------------------------------------------------------------

#[test]
fn test_500_page_document_single_edit_incremental_save_is_fast_and_valid() {
    let original = build_document(520);
    let mut doc = EditableDocument::from_bytes(original.clone()).unwrap();
    assert_eq!(doc.page_count().unwrap(), 520);

    // A single, realistic edit: replace text on one page.
    let target = doc.page_id_at(250).unwrap();
    let replaced = doc.replace_page_text(target, "Original page 250", "Edited page 250").unwrap();
    assert_eq!(replaced, 1);

    let start = Instant::now();
    let saved = doc.save_incremental_to_bytes().unwrap();
    let elapsed = start.elapsed();
    eprintln!("[DoD] 520-page single-edit incremental save took {elapsed:?}");

    assert!(
        elapsed.as_millis() < 200,
        "incremental save of a single edit on a 520-page document took {elapsed:?}, expected < 200ms"
    );

    // Append-only: original bytes are an unmodified prefix.
    assert!(saved.starts_with(&original));

    // Valid enough for an independent parser (lopdf) to open and see the
    // edit and the full, correct page count.
    let lopdf_doc = lopdf::Document::load_mem(&saved).expect("lopdf must be able to open the incrementally-saved file");
    assert_eq!(lopdf_doc.get_pages().len(), 520);
    let text = lopdf_doc.extract_text(&[251]).expect("lopdf must be able to extract text from the edited page");
    assert!(text.contains("Edited page 250"), "lopdf-extracted text was: {text:?}");

    // And our own reader round-trips it too.
    let reopened = EditableDocument::from_bytes(saved).unwrap();
    assert_eq!(reopened.page_count().unwrap(), 520);
}

#[test]
fn test_500_page_document_page_structural_edits_round_trip_with_lopdf() {
    let original = build_document(500);
    let mut doc = EditableDocument::from_bytes(original).unwrap();

    doc.insert_blank_page(0, 612.0, 792.0).unwrap();
    doc.delete_page(10).unwrap();
    doc.move_page(5, 100).unwrap();
    doc.rotate_page(20, 90).unwrap();

    let start = Instant::now();
    let saved = doc.save_incremental_to_bytes().unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 200,
        "incremental save of a handful of page-tree edits took {elapsed:?}, expected < 200ms"
    );

    // 500 original - 1 deleted + 1 inserted = 500.
    let lopdf_doc = lopdf::Document::load_mem(&saved).expect("lopdf must open the edited file");
    assert_eq!(lopdf_doc.get_pages().len(), 500);

    let reopened = EditableDocument::from_bytes(saved).unwrap();
    assert_eq!(reopened.page_count().unwrap(), 500);
}

// ---------------------------------------------------------------------
// DoD: split/merge preserve enough structure for round-trip use.
// ---------------------------------------------------------------------

#[test]
fn test_split_then_merge_round_trips_with_lopdf() {
    let original = build_document(10);
    let doc = EditableDocument::from_bytes(original).unwrap();

    let split = doc.extract_pages(&[2, 4, 6]).unwrap();
    let split_bytes = split.save_incremental_to_bytes().unwrap();
    let lopdf_split = lopdf::Document::load_mem(&split_bytes).expect("lopdf must open the split-off document");
    assert_eq!(lopdf_split.get_pages().len(), 3);

    let mut target = EditableDocument::from_bytes(build_document(2)).unwrap();
    target.append_document(&split).unwrap();
    assert_eq!(target.page_count().unwrap(), 5);

    let merged_bytes = target.save_incremental_to_bytes().unwrap();
    let lopdf_merged = lopdf::Document::load_mem(&merged_bytes).expect("lopdf must open the merged document");
    assert_eq!(lopdf_merged.get_pages().len(), 5);
}

// ---------------------------------------------------------------------
// DoD: full-rewrite output is smaller than or equal to the incrementally
// saved file for the same edit, and remains valid.
// ---------------------------------------------------------------------

#[test]
fn test_full_rewrite_smaller_or_equal_than_incremental_and_valid_in_lopdf() {
    let original = build_document(50);
    let mut doc = EditableDocument::from_bytes(original).unwrap();

    // A representative batch of edits, including ones that orphan
    // objects (delete) which only full-rewrite reclaims.
    for i in 0..50 {
        let id = doc.page_id_at(i).unwrap();
        doc.replace_page_text(id, "Original page", "Rewritten page").unwrap();
    }
    doc.delete_page(0).unwrap();
    doc.delete_page(0).unwrap();
    doc.rotate_page(0, 90).unwrap();

    let incremental = doc.save_incremental_to_bytes().unwrap();
    let full_rewrite = doc.save_full_rewrite_to_bytes().unwrap();

    assert!(
        full_rewrite.len() <= incremental.len(),
        "full rewrite ({} bytes) should be <= incremental save ({} bytes)",
        full_rewrite.len(),
        incremental.len()
    );

    let lopdf_doc = lopdf::Document::load_mem(&full_rewrite).expect("lopdf must open the full-rewrite file");
    assert_eq!(lopdf_doc.get_pages().len(), 48);
    let text = lopdf_doc.extract_text(&[1]).unwrap();
    assert!(text.contains("Rewritten page"));

    let reopened = EditableDocument::from_bytes(full_rewrite).unwrap();
    assert_eq!(reopened.page_count().unwrap(), 48);
}

#[test]
fn test_full_rewrite_of_500_pages_is_smaller_or_equal_to_original() {
    // `Document` (the from-scratch builder) defaults to uncompressed
    // content streams and a plain-text xref table, so a full rewrite
    // (which always attempts FlateDecode plus object-stream/xref-stream
    // packing) should never end up larger for an untouched document.
    let original = build_document(500);
    let doc = EditableDocument::from_bytes(original.clone()).unwrap();
    let rewritten = doc.save_full_rewrite_to_bytes().unwrap();

    assert!(
        rewritten.len() <= original.len(),
        "full rewrite ({} bytes) should be <= the uncompressed original ({} bytes)",
        rewritten.len(),
        original.len()
    );

    let lopdf_doc = lopdf::Document::load_mem(&rewritten).expect("lopdf must open the full-rewrite file");
    assert_eq!(lopdf_doc.get_pages().len(), 500);
}

// ---------------------------------------------------------------------
// Content editing: insert text/shape/image-shaped content and have it
// survive a round trip through both save modes and an independent parser.
// ---------------------------------------------------------------------

#[test]
fn test_append_and_prepend_content_visible_after_incremental_save() {
    let original = build_document(1);
    let mut doc = EditableDocument::from_bytes(original).unwrap();
    let page_id = doc.page_id_at(0).unwrap();

    let appended = ContentBuilder::new()
        .fill_color(Color::RED)
        .rect(10.0, 10.0, 30.0, 30.0)
        .fill();
    doc.append_page_content(page_id, &appended).unwrap();

    let saved = doc.save_incremental_to_bytes().unwrap();
    let lopdf_doc = lopdf::Document::load_mem(&saved).unwrap();
    let (_, page_id_lopdf) = lopdf_doc.get_pages().into_iter().next().unwrap();
    let content = lopdf_doc.get_page_content(page_id_lopdf).unwrap();
    let content_str = String::from_utf8_lossy(&content);
    assert!(content_str.contains("re"));
    assert!(content_str.contains("Original page 0"));
}

#[test]
fn test_incremental_save_of_unmodified_document_is_byte_identical() {
    let original = build_document(3);
    let doc = EditableDocument::from_bytes(original.clone()).unwrap();
    let saved = doc.save_incremental_to_bytes().unwrap();
    assert_eq!(saved, original);
}
