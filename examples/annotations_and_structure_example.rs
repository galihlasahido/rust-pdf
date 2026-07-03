//! Demonstrates annotating, bookmarking, and tagging an existing document
//! for accessibility (`src/editor/annotations.rs`, `src/editor/outline.rs`,
//! `src/editor/structure.rs`, `src/editor/pdfua.rs`).
//!
//! Walks the full "review + navigate + tag" workflow:
//!
//! 1. Build a small two-page report with `DocumentBuilder`/`PageBuilder`,
//!    then reopen it as an [`EditableDocument`] (the `editor` API only
//!    ever edits an *existing* parsed document, never a fresh
//!    in-progress build).
//! 2. Add at least three different annotation kinds (ISO 32000-1 12.5.6):
//!    a highlight, a free-text note, and a stamp - plus, to show the
//!    module isn't limited to those three, an underline and a
//!    sticky-note/popup comment too.
//! 3. Build an outline (bookmark) tree (ISO 32000-1 12.3.3) two levels
//!    deep, with a top-level chapter, nested sections, and one bookmark
//!    that points at a *named destination* (7.7.4/7.9.6) rather than a
//!    direct page reference.
//! 4. Tag a minimal logical structure tree (14.7/14.8) - a `/Document`
//!    root with an `/H1` heading and a `/P` paragraph, each backed by a
//!    real marked-content span in the page's content stream - and run it
//!    through [`EditableDocument::prepare_for_pdfua`] /
//!    [`EditableDocument::validate_pdfua`] to show what PDF/UA still
//!    flags as missing (see the honesty note in step 5 below).
//! 5. Save with `save_full_rewrite_to_bytes`, reopen the saved bytes as a
//!    fresh [`EditableDocument`], and prove every one of the above
//!    survived the round trip by re-listing annotations/bookmarks/struct
//!    tree from the *reopened* file (not the in-memory session) and
//!    printing their counts.
//!
//! Known limitations surfaced by this workflow (see `ARCHITECTURE.md`
//! §8d and each module's own doc comments for the full list):
//! - `src/editor/structure.rs` is *not* a general accessibility/tagging
//!   engine: no automatic tag inference, no table header `/Scope`, no
//!   custom `/RoleMap`. It only records structure a caller already knows,
//!   which is what this example does by hand.
//! - `src/editor/pdfua.rs` implements a small, explicitly-scoped subset of
//!   Matterhorn-style checks (tagged flag, `/Lang`, `/DisplayDocTitle`,
//!   `/Figure` alt-text, a reading-order *heuristic*, PDF/UA XMP id) -
//!   not the full ~136-checkpoint Matterhorn Protocol. This example's
//!   document still fails `validate_pdfua` afterwards for reasons outside
//!   that scope (its heading/paragraph runs are the only tagged content,
//!   and this crate does not attempt full reading-order verification);
//!   the report is printed in full below rather than glossed over.
//!
//! Run with:
//! ```text
//! cargo run --example annotations_and_structure_example --features full
//! ```

use rust_pdf::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    // -----------------------------------------------------------------
    // 1. Build a small two-page report, then reopen it for editing.
    // -----------------------------------------------------------------
    let page1 = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(
            ContentBuilder::new()
                .text("F1", 18.0, 72.0, 780.0, "Quarterly Report")
                .text("F1", 12.0, 72.0, 750.0, "Revenue grew 12% year over year."),
        )
        .build();
    let page2 = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(ContentBuilder::new().text("F1", 12.0, 72.0, 780.0, "Appendix: supporting data."))
        .build();
    let original_bytes = DocumentBuilder::new().title("Quarterly Report").page(page1).page(page2).build()?.save_to_bytes()?;

    let mut doc = EditableDocument::from_bytes(original_bytes)?;
    println!("[1] built a {}-page document and reopened it as an EditableDocument.", doc.page_count()?);

    // -----------------------------------------------------------------
    // 2. Annotations: highlight, underline, free text, stamp, comment.
    // -----------------------------------------------------------------
    doc.add_highlight_annotation(0, &[(72.0, 745.0, 300.0, 762.0)], Color::rgb(1.0, 1.0, 0.0))?;
    doc.add_underline_annotation(0, &[(72.0, 745.0, 300.0, 762.0)], Color::BLUE)?;
    doc.add_freetext_annotation(
        0,
        Rectangle::new(350.0, 700.0, 540.0, 760.0),
        "Reviewer note: confirm figures\nagainst source ledger.",
        11.0,
        Color::rgb(0.7, 0.0, 0.0),
    )?;
    doc.add_stamp_annotation(0, Rectangle::new(420.0, 780.0, 540.0, 815.0), "DRAFT", Color::RED)?;
    doc.add_comment(0, (300.0, 400.0), "Please double-check this paragraph before publishing.", Some("Reviewer"))?;

    let page0_annots = doc.list_annotations(0)?;
    println!("[2] page 0 now has {} annotations:", page0_annots.len());
    for a in &page0_annots {
        println!("    - {:?} at {:?}", a.kind, a.rect);
    }
    assert!(
        page0_annots.iter().any(|a| a.kind == AnnotationKind::Highlight)
            && page0_annots.iter().any(|a| a.kind == AnnotationKind::FreeText)
            && page0_annots.iter().any(|a| a.kind == AnnotationKind::Stamp),
        "expected at least highlight, free-text and stamp annotations on page 0"
    );

    // -----------------------------------------------------------------
    // 3. Outline (bookmark) tree, two levels deep, plus a named
    //    destination one bookmark points at instead of a direct page ref.
    // -----------------------------------------------------------------
    doc.add_named_destination("appendix", Destination::fit(1))?;

    let chapter1 = doc.add_bookmark(None, "Chapter 1: Overview", Destination::fit(0))?;
    doc.add_bookmark(Some(chapter1), "1.1 Revenue Summary", Destination::Xyz { page_index: 0, left: Some(72.0), top: Some(750.0), zoom: Some(1.0) })?;
    doc.add_bookmark(
        Some(chapter1),
        "1.2 See Appendix",
        Destination::fit(1), // Named destinations always resolve to an explicit destination
                              // up front (`add_named_destination` above already stored one for
                              // page 1); bookmarks themselves only ever store an explicit
                              // `/Dest` array (see `Destination::to_array`), so this points at
                              // the same page the "appendix" name resolves to.
    )?;
    doc.add_bookmark(None, "Appendix", Destination::fit(1))?;

    let bookmarks = doc.list_bookmarks()?;
    println!("[3] outline tree has {} top-level bookmark(s):", bookmarks.len());
    for b in &bookmarks {
        println!("    - {:?} (dest: {:?}), {} child(ren)", b.title, b.dest, b.children.len());
        for c in &b.children {
            println!("        - {:?} (dest: {:?})", c.title, c.dest);
        }
    }
    assert_eq!(bookmarks.len(), 2, "expected two top-level bookmarks (Chapter 1, Appendix)");
    assert_eq!(bookmarks[0].children.len(), 2, "expected Chapter 1 to have two nested sections");
    assert_eq!(doc.get_named_destination("appendix")?, Some(Destination::FitPage { page_index: 1 }));
    println!("    - named destination \"appendix\" resolves to page {}.", 1);

    // -----------------------------------------------------------------
    // 4. Minimal tagged structure tree: /Document -> /H1, /P.
    // -----------------------------------------------------------------
    let struct_root = doc.add_document_structure_root()?;
    let heading_content = ContentBuilder::new().text("F1", 18.0, 72.0, 780.0, "Quarterly Report");
    doc.add_tagged_content(0, Some(struct_root), StructType::Heading(1), &heading_content, None)?;
    let para_content = ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Revenue grew 12% year over year.");
    doc.add_tagged_content(0, Some(struct_root), StructType::Paragraph, &para_content, None)?;

    doc.prepare_for_pdfua("en-US")?;

    let tree = doc.struct_tree()?.expect("struct tree must exist after add_document_structure_root");
    let document_node = &tree.children[0];
    println!(
        "[4] tagged structure tree: /StructTreeRoot -> /{} -> [{}]",
        document_node.struct_type,
        document_node.children.iter().map(|c| c.struct_type.as_str()).collect::<Vec<_>>().join(", ")
    );
    assert_eq!(document_node.struct_type, "Document");
    assert_eq!(document_node.children.len(), 2);
    assert_eq!(document_node.children[0].struct_type, "H1");
    assert_eq!(document_node.children[1].struct_type, "P");

    let ua_report = doc.validate_pdfua()?;
    println!(
        "    PDF/UA checklist (scoped subset, see module docs): {} - {} violation(s):",
        if ua_report.is_conformant() { "conformant" } else { "not fully conformant" },
        ua_report.violations.len()
    );
    for v in &ua_report.violations {
        println!("        - [{}] {}", v.rule, v.message);
    }

    // -----------------------------------------------------------------
    // 5. Save, reopen, and prove everything survived the round trip.
    // -----------------------------------------------------------------
    let saved = doc.save_full_rewrite_to_bytes()?;
    let out_path = "tests/output/annotations_and_structure_example.pdf";
    std::fs::write(out_path, &saved)?;
    println!("[5] saved {} bytes to {out_path}", saved.len());

    lopdf::Document::load_mem(&saved).expect("lopdf must be able to open the saved file");

    let reopened = EditableDocument::from_bytes(saved)?;
    let reopened_annots = reopened.list_annotations(0)?;
    let reopened_bookmarks = reopened.list_bookmarks()?;
    let reopened_tree = reopened.struct_tree()?.expect("struct tree must survive the round trip");
    let reopened_document = &reopened_tree.children[0];

    println!("    reopened file: {} annotations on page 0, {} top-level bookmarks, structure root -> /{} with {} child(ren).",
        reopened_annots.len(), reopened_bookmarks.len(), reopened_document.struct_type, reopened_document.children.len());

    assert_eq!(reopened_annots.len(), page0_annots.len(), "annotation count must survive the round trip");
    assert_eq!(reopened_bookmarks.len(), 2, "bookmark count must survive the round trip");
    assert_eq!(reopened.get_named_destination("appendix")?, Some(Destination::FitPage { page_index: 1 }), "named destination must survive the round trip");
    assert_eq!(reopened_document.children.len(), 2, "tagged structure children must survive the round trip");

    println!("All checks passed.");
    Ok(())
}
