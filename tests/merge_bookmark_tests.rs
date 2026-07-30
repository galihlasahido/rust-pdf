//! Integration tests for `EditableDocument::append_document`'s outline
//! (bookmark) import: when merging one document's pages into another, the
//! source's outline tree must come along too, appended as new top-level
//! node(s) after the destination's own existing top-level bookmarks, with
//! every destination page index remapped to the newly-imported pages'
//! positions in the destination. See `src/editor/pages.rs`
//! (`EditableDocument::append_document`, `import_outline_from`) and
//! `src/editor/outline.rs` (`add_bookmark_opt`).

#![cfg(feature = "parser")]

use rust_pdf::prelude::*;

fn doc_with_pages(n: usize) -> EditableDocument {
    let mut builder = DocumentBuilder::new();
    for i in 0..n {
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, &format!("Page {i}")))
            .build();
        builder = builder.page(page);
    }
    let bytes = builder.build().unwrap().save_to_bytes().unwrap();
    EditableDocument::from_bytes(bytes).unwrap()
}

// ---------------------------------------------------------------------
// No-op when the source document has no outline at all.
// ---------------------------------------------------------------------

#[test]
fn merge_with_no_source_outline_is_a_noop_not_an_error() {
    let mut a = doc_with_pages(2);
    a.add_bookmark(None, "Existing", Destination::fit(0))
        .unwrap();
    let b = doc_with_pages(3); // no bookmarks at all

    a.append_document(&b).unwrap();

    assert_eq!(a.page_count().unwrap(), 5);
    let bookmarks = a.list_bookmarks().unwrap();
    assert_eq!(bookmarks.len(), 1, "must not fabricate bookmarks");
    assert_eq!(bookmarks[0].title, "Existing");
}

// ---------------------------------------------------------------------
// Merging a source with bookmarks appends them as new top-level nodes
// after the destination's own, with page indices remapped.
// ---------------------------------------------------------------------

#[test]
fn merge_imports_source_outline_as_new_top_level_nodes_with_remapped_pages() {
    let mut a = doc_with_pages(2);
    a.add_bookmark(None, "A - Chapter 1", Destination::fit(0))
        .unwrap();

    let mut b = doc_with_pages(3);
    b.add_bookmark(None, "B - Chapter 1", Destination::fit(0))
        .unwrap();
    b.add_bookmark(None, "B - Chapter 2", Destination::fit(2))
        .unwrap();

    a.append_document(&b).unwrap();

    assert_eq!(a.page_count().unwrap(), 5);
    let bookmarks = a.list_bookmarks().unwrap();
    assert_eq!(bookmarks.len(), 3, "existing + 2 imported top-level nodes");

    // Destination's own bookmark must survive untouched, first.
    assert_eq!(bookmarks[0].title, "A - Chapter 1");
    assert_eq!(
        bookmarks[0].dest,
        Some(Destination::FitPage { page_index: 0 })
    );

    // Imported bookmarks follow, in source order, with page indices
    // shifted by the destination's original page count (2).
    assert_eq!(bookmarks[1].title, "B - Chapter 1");
    assert_eq!(
        bookmarks[1].dest,
        Some(Destination::FitPage { page_index: 2 })
    );
    assert_eq!(bookmarks[2].title, "B - Chapter 2");
    assert_eq!(
        bookmarks[2].dest,
        Some(Destination::FitPage { page_index: 4 })
    );
}

// ---------------------------------------------------------------------
// Nested (multi-level) source outlines are copied with structure intact,
// and XYZ destinations are remapped too (not just Fit).
// ---------------------------------------------------------------------

#[test]
fn merge_preserves_nested_bookmark_structure_and_xyz_destinations() {
    let mut a = doc_with_pages(1);

    let mut b = doc_with_pages(2);
    let part = b.add_bookmark(None, "Part I", Destination::fit(0)).unwrap();
    b.add_bookmark(
        Some(part),
        "Section 1.1",
        Destination::Xyz {
            page_index: 1,
            left: Some(10.0),
            top: Some(700.0),
            zoom: Some(1.5),
        },
    )
    .unwrap();

    a.append_document(&b).unwrap();

    let bookmarks = a.list_bookmarks().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].title, "Part I");
    assert_eq!(
        bookmarks[0].dest,
        Some(Destination::FitPage { page_index: 1 }) // a had 1 page: offset = 1
    );
    assert_eq!(bookmarks[0].children.len(), 1);
    assert_eq!(bookmarks[0].children[0].title, "Section 1.1");
    assert_eq!(
        bookmarks[0].children[0].dest,
        Some(Destination::Xyz {
            page_index: 2, // source page 1 + offset 1
            left: Some(10.0),
            top: Some(700.0),
            zoom: Some(1.5),
        })
    );
}

// ---------------------------------------------------------------------
// Merging into a document that itself has no outline yet still works
// (the `/Outlines` root gets created fresh) and doesn't duplicate.
// ---------------------------------------------------------------------

#[test]
fn merge_into_destination_with_no_existing_outline_creates_one_cleanly() {
    let mut a = doc_with_pages(1); // no bookmarks
    let mut b = doc_with_pages(2);
    b.add_bookmark(None, "Only chapter", Destination::fit(1))
        .unwrap();

    a.append_document(&b).unwrap();

    let bookmarks = a.list_bookmarks().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].title, "Only chapter");
    assert_eq!(
        bookmarks[0].dest,
        Some(Destination::FitPage { page_index: 2 }) // a had 1 page: 1 + 1
    );

    // Round-trips through an incremental save too.
    let saved = a.save_incremental_to_bytes().unwrap();
    let reopened = EditableDocument::from_bytes(saved).unwrap();
    let bookmarks_after = reopened.list_bookmarks().unwrap();
    assert_eq!(bookmarks_after.len(), 1);
    assert_eq!(bookmarks_after[0].title, "Only chapter");
}

// ---------------------------------------------------------------------
// Merging twice appends twice, without corrupting the first import.
// ---------------------------------------------------------------------

#[test]
fn merging_twice_appends_both_times_without_corrupting_prior_import() {
    let mut a = doc_with_pages(1);

    let mut b = doc_with_pages(1);
    b.add_bookmark(None, "From B", Destination::fit(0)).unwrap();

    let mut c = doc_with_pages(1);
    c.add_bookmark(None, "From C", Destination::fit(0)).unwrap();

    a.append_document(&b).unwrap();
    a.append_document(&c).unwrap();

    assert_eq!(a.page_count().unwrap(), 3);
    let bookmarks = a.list_bookmarks().unwrap();
    let titles: Vec<_> = bookmarks.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, vec!["From B", "From C"]);
    assert_eq!(
        bookmarks[0].dest,
        Some(Destination::FitPage { page_index: 1 })
    );
    assert_eq!(
        bookmarks[1].dest,
        Some(Destination::FitPage { page_index: 2 })
    );
}
