//! Integration tests for the "Interactive Features" task
//! (`rust_pdf::editor`'s AcroForm, annotation, outline and tagged-structure
//! submodules), covering its Definition of Done end-to-end:
//!
//! - Form fill roundtrip (fill -> save -> reopen) preserves every field's
//!   value.
//! - Annotations (highlight, underline, strikeout, freetext, stamp, ink,
//!   comment/popup) are present with a valid appearance stream an
//!   independent reader (`lopdf`) can open.
//! - A tagged logical structure tree (heading/paragraph/table/figure with
//!   alt text) exists for a sample document.

#![cfg(feature = "parser")]

use rust_pdf::forms::{CheckBox, ComboBox, RadioButton, RadioGroup, TextField};
use rust_pdf::prelude::*;

fn sample_form_document() -> Vec<u8> {
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .form_field(TextField::new("full_name").rect(100.0, 700.0, 250.0, 20.0))
        .form_field(CheckBox::new("newsletter").rect(100.0, 660.0, 18.0, 18.0))
        .form_field(
            RadioGroup::new("shirt_size")
                .add_button(RadioButton::new("S").rect(100.0, 620.0, 18.0, 18.0))
                .add_button(RadioButton::new("M").rect(130.0, 620.0, 18.0, 18.0))
                .add_button(RadioButton::new("L").rect(160.0, 620.0, 18.0, 18.0)),
        )
        .form_field(ComboBox::new("country").rect(100.0, 580.0, 150.0, 20.0).options(vec!["Indonesia", "Malaysia", "Singapore"]))
        .content(ContentBuilder::new().text("F1", 14.0, 72.0, 760.0, "Registration Form"))
        .build();
    DocumentBuilder::new().title("Registration Form").page(page).build().unwrap().save_to_bytes().unwrap()
}

// ---------------------------------------------------------------------
// DoD: form fill roundtrip (fill -> save -> reopen) preserves values.
// ---------------------------------------------------------------------

#[test]
fn dod_form_fill_roundtrip_preserves_every_field_type() {
    let original = sample_form_document();
    let mut doc = EditableDocument::from_bytes(original).unwrap();

    doc.set_text_value("full_name", "Siti Rahayu").unwrap();
    doc.set_checkbox_checked("newsletter", true).unwrap();
    doc.set_radio_value("shirt_size", "M").unwrap();
    doc.set_choice_value("country", "Malaysia").unwrap();
    // Also exercise "create" - a signature field added to an existing PDF.
    doc.add_signature_field(0, "approval_signature", Rectangle::new(350.0, 700.0, 550.0, 740.0)).unwrap();

    let saved = doc.save_incremental_to_bytes().unwrap();

    // An independent reader must still be able to open the filled file.
    let lopdf_doc = lopdf::Document::load_mem(&saved).expect("lopdf must open the filled-in form");
    assert_eq!(lopdf_doc.get_pages().len(), 1);

    // Re-open with our own reader and confirm every value survived.
    let reopened = EditableDocument::from_bytes(saved).unwrap();
    assert_eq!(reopened.get_text_value("full_name").unwrap().as_deref(), Some("Siti Rahayu"));
    assert!(reopened.get_checkbox_checked("newsletter").unwrap());
    assert_eq!(reopened.get_radio_value("shirt_size").unwrap().as_deref(), Some("M"));
    assert_eq!(reopened.get_choice_value("country").unwrap().as_deref(), Some("Malaysia"));
    assert_eq!(reopened.field_type("approval_signature").unwrap().as_deref(), Some("Sig"));

    let mut names = reopened.field_names().unwrap();
    names.sort();
    assert_eq!(names, vec!["approval_signature", "country", "full_name", "newsletter", "shirt_size"]);
}

#[test]
fn dod_form_fill_roundtrip_survives_full_rewrite_too() {
    let original = sample_form_document();
    let mut doc = EditableDocument::from_bytes(original).unwrap();
    doc.set_text_value("full_name", "Budi Santoso").unwrap();
    doc.set_checkbox_checked("newsletter", false).unwrap();

    let rewritten = doc.save_full_rewrite_to_bytes().unwrap();
    lopdf::Document::load_mem(&rewritten).expect("lopdf must open the full-rewritten form");

    let reopened = EditableDocument::from_bytes(rewritten).unwrap();
    assert_eq!(reopened.get_text_value("full_name").unwrap().as_deref(), Some("Budi Santoso"));
    assert!(!reopened.get_checkbox_checked("newsletter").unwrap());
}

#[test]
fn dod_flatten_form_then_reopen_has_no_interactive_fields_left() {
    let original = sample_form_document();
    let mut doc = EditableDocument::from_bytes(original).unwrap();
    doc.set_text_value("full_name", "Final Answer").unwrap();
    doc.flatten_form().unwrap();

    let saved = doc.save_incremental_to_bytes().unwrap();
    lopdf::Document::load_mem(&saved).expect("lopdf must open the flattened form");

    let reopened = EditableDocument::from_bytes(saved).unwrap();
    assert!(reopened.field_names().unwrap().is_empty());
}

// ---------------------------------------------------------------------
// DoD: annotations render correctly (valid appearance stream, correct
// geometry/metadata) and are visible to another reader.
// ---------------------------------------------------------------------

#[test]
fn dod_every_annotation_kind_has_a_valid_appearance_and_round_trips() {
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "The quick brown fox jumps."))
        .build();
    let original = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
    let mut doc = EditableDocument::from_bytes(original).unwrap();

    doc.add_highlight_annotation(0, &[(72.0, 745.0, 150.0, 762.0)], Color::rgb(1.0, 1.0, 0.0)).unwrap();
    doc.add_underline_annotation(0, &[(160.0, 745.0, 230.0, 762.0)], Color::BLACK).unwrap();
    doc.add_strikeout_annotation(0, &[(240.0, 745.0, 300.0, 762.0)], Color::RED).unwrap();
    doc.add_freetext_annotation(0, Rectangle::new(72.0, 600.0, 300.0, 650.0), "Reviewer note here", 11.0, Color::BLUE).unwrap();
    doc.add_stamp_annotation(0, Rectangle::new(400.0, 700.0, 550.0, 750.0), "APPROVED", Color::rgb(0.0, 0.5, 0.0)).unwrap();
    doc.add_ink_annotation(0, &[vec![(100.0, 400.0), (120.0, 430.0), (140.0, 405.0)]], Color::rgb(0.2, 0.2, 0.8), 2.0).unwrap();
    doc.add_comment(0, (72.0, 500.0), "Please double check the figures.", Some("Editor")).unwrap();

    let saved = doc.save_incremental_to_bytes().unwrap();
    let lopdf_doc = lopdf::Document::load_mem(&saved).expect("lopdf must open the annotated document");
    assert_eq!(lopdf_doc.get_pages().len(), 1);

    let reopened = EditableDocument::from_bytes(saved).unwrap();
    let annots = reopened.list_annotations(0).unwrap();
    // highlight, underline, strikeout, freetext, stamp, ink, text-note, popup
    assert_eq!(annots.len(), 8);

    for a in &annots {
        // Every markup annotation must carry a usable appearance stream
        // (`/AP /N`) so a reader that doesn't regenerate appearances
        // itself still displays it correctly.
        if a.kind != rust_pdf::AnnotationKind::Popup {
            let Some(Object::Dictionary(dict)) = reopened.get_object(a.id) else {
                panic!("annotation {:?} did not resolve to a dictionary", a.id)
            };
            assert!(dict.get("AP").is_some(), "{:?} annotation is missing /AP", a.kind);
        }
    }

    let kinds: Vec<_> = annots.iter().map(|a| a.kind).collect();
    assert!(kinds.contains(&rust_pdf::AnnotationKind::Highlight));
    assert!(kinds.contains(&rust_pdf::AnnotationKind::Underline));
    assert!(kinds.contains(&rust_pdf::AnnotationKind::StrikeOut));
    assert!(kinds.contains(&rust_pdf::AnnotationKind::FreeText));
    assert!(kinds.contains(&rust_pdf::AnnotationKind::Stamp));
    assert!(kinds.contains(&rust_pdf::AnnotationKind::Ink));
    assert!(kinds.contains(&rust_pdf::AnnotationKind::Text));
    assert!(kinds.contains(&rust_pdf::AnnotationKind::Popup));
}

#[test]
fn dod_annotation_edit_and_delete_round_trip() {
    let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(ContentBuilder::new()).build();
    let original = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
    let mut doc = EditableDocument::from_bytes(original).unwrap();

    let freetext_id = doc.add_freetext_annotation(0, Rectangle::new(72.0, 600.0, 300.0, 650.0), "First draft", 11.0, Color::BLACK).unwrap();
    let (comment_id, _popup_id) = doc.add_comment(0, (72.0, 500.0), "Original comment", None).unwrap();

    doc.edit_annotation_contents(freetext_id, "Revised text").unwrap();
    doc.delete_annotation(0, comment_id).unwrap();

    let saved = doc.save_incremental_to_bytes().unwrap();
    let reopened = EditableDocument::from_bytes(saved).unwrap();
    let annots = reopened.list_annotations(0).unwrap();
    assert_eq!(annots.len(), 1, "the comment + its popup must both be gone");
    assert_eq!(annots[0].contents.as_deref(), Some("Revised text"));
}

// ---------------------------------------------------------------------
// DoD: an outline (bookmark) tree and named destinations exist and
// round-trip.
// ---------------------------------------------------------------------

#[test]
fn dod_outline_and_named_destinations_round_trip() {
    let mut builder = DocumentBuilder::new();
    for i in 0..4 {
        let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, &format!("Page {i}"))).build();
        builder = builder.page(page);
    }
    let original = builder.build().unwrap().save_to_bytes().unwrap();
    let mut doc = EditableDocument::from_bytes(original).unwrap();

    let part1 = doc.add_bookmark(None, "Part I", rust_pdf::Destination::fit(0)).unwrap();
    doc.add_bookmark(Some(part1), "Chapter 1", rust_pdf::Destination::fit(1)).unwrap();
    doc.add_bookmark(Some(part1), "Chapter 2", rust_pdf::Destination::fit(2)).unwrap();
    doc.add_bookmark(None, "Appendix", rust_pdf::Destination::fit(3)).unwrap();
    doc.add_named_destination("appendix", rust_pdf::Destination::fit(3)).unwrap();

    let saved = doc.save_incremental_to_bytes().unwrap();
    lopdf::Document::load_mem(&saved).expect("lopdf must open the outlined document");

    let reopened = EditableDocument::from_bytes(saved).unwrap();
    let bookmarks = reopened.list_bookmarks().unwrap();
    assert_eq!(bookmarks.len(), 2);
    assert_eq!(bookmarks[0].title, "Part I");
    assert_eq!(bookmarks[0].children.len(), 2);
    assert_eq!(bookmarks[1].title, "Appendix");
    assert_eq!(
        reopened.get_named_destination("appendix").unwrap(),
        Some(rust_pdf::Destination::FitPage { page_index: 3 })
    );
}

// ---------------------------------------------------------------------
// DoD: a tagged logical structure tree exists for at least one sample
// document (heading, paragraph, table, figure with alt text).
// ---------------------------------------------------------------------

#[test]
fn dod_tagged_structure_tree_sample_document() {
    let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(ContentBuilder::new()).build();
    let original = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
    let mut doc = EditableDocument::from_bytes(original).unwrap();

    let root = doc.add_document_structure_root().unwrap();
    doc.add_tagged_content(0, Some(root), rust_pdf::StructType::Heading(1), &ContentBuilder::new().text("F1", 18.0, 72.0, 780.0, "Quarterly Report"), None)
        .unwrap();
    doc.add_tagged_content(0, Some(root), rust_pdf::StructType::Paragraph, &ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Revenue grew 12% year over year."), None)
        .unwrap();

    let table = doc.add_tagged_content(0, Some(root), rust_pdf::StructType::Table, &ContentBuilder::new(), None).unwrap();
    let row = doc.add_tagged_content(0, Some(table), rust_pdf::StructType::TableRow, &ContentBuilder::new(), None).unwrap();
    doc.add_tagged_content(0, Some(row), rust_pdf::StructType::TableCell, &ContentBuilder::new().text("F1", 10.0, 72.0, 700.0, "Q1"), None).unwrap();
    doc.add_tagged_content(0, Some(row), rust_pdf::StructType::TableCell, &ContentBuilder::new().text("F1", 10.0, 140.0, 700.0, "$1.2M"), None).unwrap();

    doc.add_tagged_content(0, Some(root), rust_pdf::StructType::Figure, &ContentBuilder::new(), Some("Bar chart of quarterly revenue growth")).unwrap();

    let saved = doc.save_incremental_to_bytes().unwrap();
    lopdf::Document::load_mem(&saved).expect("lopdf must open the tagged document");

    let reopened = EditableDocument::from_bytes(saved).unwrap();
    let tree = reopened.struct_tree().unwrap().expect("a struct tree must exist");
    let document = &tree.children[0];
    assert_eq!(document.struct_type, "Document");

    let types: Vec<_> = document.children.iter().map(|c| c.struct_type.as_str()).collect();
    assert_eq!(types, vec!["H1", "P", "Table", "Figure"]);

    let table_node = document.children.iter().find(|c| c.struct_type == "Table").unwrap();
    assert_eq!(table_node.children[0].struct_type, "TR");
    assert_eq!(table_node.children[0].children.len(), 2);
    assert!(table_node.children[0].children.iter().all(|c| c.struct_type == "TD"));

    let figure_node = document.children.iter().find(|c| c.struct_type == "Figure").unwrap();
    assert_eq!(figure_node.alt_text.as_deref(), Some("Bar chart of quarterly revenue growth"));

    // Catalog-level Tagged PDF markers (ISO 32000-1 14.7.1, 14.8.1).
    let Some(Object::Dictionary(catalog)) = reopened.get_object(reopened.catalog_id()) else {
        panic!("catalog did not resolve to a dictionary")
    };
    assert!(catalog.get("StructTreeRoot").is_some());
    assert!(catalog.get("MarkInfo").is_some());
}
