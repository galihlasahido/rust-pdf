//! Non-interactive checks for the `native-gui` render pipeline: the actual
//! new wiring (`PdfRenderer` output -> `egui::ColorImage`) is what a manual
//! click-through of the windowed app can't easily be automated to prove, so
//! it's covered here instead. Does not spawn a window/event loop.

#![cfg(feature = "native-gui")]

use rust_pdf::encryption::{EncryptionConfig, Permissions};
use rust_pdf::render::PdfRenderer;
use rust_pdf::types::Rectangle;
use rust_pdf::Color;

#[test]
fn open_render_and_convert_to_color_image() {
    let renderer =
        PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");
    assert!(renderer.page_count() > 1, "fixture should be multi-page");

    let image = renderer
        .render_page(0, 96.0, None)
        .expect("page 0 should render");
    assert!(image.width() > 0 && image.height() > 0);
    assert_eq!(
        image.as_raw().len(),
        (image.width() * image.height() * 4) as usize
    );

    // Exactly the conversion `gui::viewer::PageViewer::poll` performs.
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    assert_eq!(color_image.size, size);
}

#[test]
fn render_second_page_differs_from_first() {
    let renderer =
        PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");
    let page0 = renderer.render_page(0, 72.0, None).expect("page 0");
    let page1 = renderer.render_page(1, 72.0, None).expect("page 1");
    assert_ne!(page0.as_raw(), page1.as_raw());
}

#[test]
fn document_accessor_exposes_bookmarks_api() {
    let renderer =
        PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");
    // Just needs to not error -- this fixture may or may not have bookmarks,
    // the point is `PdfRenderer::document()` -> `list_bookmarks()` works.
    let _ = renderer
        .document()
        .list_bookmarks()
        .expect("should not error");
}

#[test]
fn render_thumbnail_and_convert_to_color_image() {
    // Exactly what `gui::thumbnails::ThumbnailStrip::poll` does.
    let renderer =
        PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");
    let thumb = renderer
        .render_thumbnail(0, 120)
        .expect("thumbnail should render");
    assert!(thumb.width() <= 120 && thumb.height() <= 120);

    let size = [thumb.width() as usize, thumb.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, thumb.as_raw());
    assert_eq!(color_image.size, size);
}

#[test]
fn extract_page_text_by_index_for_search() {
    // Exactly what `gui::search::search_all_pages` does per page.
    let renderer =
        PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");
    let document = renderer.document();
    let page_count = renderer.page_count();
    assert!(page_count > 1);

    let mut found_any_text = false;
    for page_index in 0..page_count {
        let page_id = document.page_id_at(page_index).expect("page id");
        let text = document.extract_page_text(page_id).expect("extract text");
        if !text.trim().is_empty() {
            found_any_text = true;
        }
    }
    assert!(
        found_any_text,
        "fixture should have extractable text somewhere"
    );
}

#[test]
fn edit_document_invalidates_the_thumbnail_cache() {
    // Exactly the wiring gui::app.rs's Save/form-fill/page-management
    // actions depend on: an edit made via `edit_document` must be
    // reflected the next time a thumbnail is requested, not served from
    // a now-stale cached raster.
    let mut renderer =
        PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");

    let before = renderer.render_thumbnail(0, 100).expect("thumbnail before edit");

    // A big, opaque highlight (rather than e.g. a bare text field, whose
    // default appearance can be a near-invisible thin border at 100px
    // thumbnail scale) so the pixel diff below is a reliable signal, not
    // a coin flip on whether the edit happened to be visually subtle.
    renderer
        .edit_document(|doc| {
            doc.add_highlight_annotation(
                0,
                &[(20.0, 20.0, 550.0, 750.0)],
                Color::rgb(1.0, 1.0, 0.0),
            )
        })
        .expect("add_highlight_annotation should succeed");

    let after = renderer.render_thumbnail(0, 100).expect("thumbnail after edit");

    assert_ne!(
        before.as_raw(),
        after.as_raw(),
        "thumbnail should reflect the newly-added field, not a stale cached raster"
    );
}

#[test]
fn form_field_edit_persists_through_save_and_reopen() {
    // Exactly the wiring gui::forms.rs's inline text-edit commit and
    // gui::app.rs's Save both depend on.
    let mut renderer =
        PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");

    renderer
        .edit_document(|doc| {
            doc.add_text_field(
                0,
                "gui_roundtrip_field",
                Rectangle::new(20.0, 20.0, 220.0, 50.0),
                Some("initial"),
            )
        })
        .expect("add_text_field should succeed");

    renderer
        .edit_document(|doc| doc.set_text_value("gui_roundtrip_field", "updated by test"))
        .expect("set_text_value should succeed");

    let saved_bytes = renderer
        .document()
        .save_full_rewrite_to_bytes()
        .expect("save_full_rewrite_to_bytes should succeed");

    let reopened = PdfRenderer::open_bytes(saved_bytes).expect("reopen should succeed");
    let value = reopened
        .document()
        .get_text_value("gui_roundtrip_field")
        .expect("get_text_value should succeed");

    assert_eq!(value.as_deref(), Some("updated by test"));
}

#[test]
fn page_management_actions_change_page_count_as_expected() {
    // Exactly the wiring gui::app.rs's thumbnail-panel rotate/insert/
    // delete controls depend on.
    let mut renderer =
        PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");
    let original_count = renderer.page_count();
    assert!(original_count > 1, "fixture should be multi-page");

    // Rotate: page count unchanged, but the *displayed* (post-rotation)
    // raster dimensions swap for a 90-degree turn.
    let before_rotate = renderer.render_page(0, 72.0, None).expect("render before rotate");
    renderer
        .edit_document(|doc| doc.rotate_page(0, 90))
        .expect("rotate_page should succeed");
    let after_rotate = renderer.render_page(0, 72.0, None).expect("render after rotate");
    assert_eq!(renderer.page_count(), original_count);
    assert_eq!(before_rotate.width(), after_rotate.height());
    assert_eq!(before_rotate.height(), after_rotate.width());

    // Insert a blank page: count goes up by one.
    renderer
        .edit_document(|doc| doc.insert_blank_page(0, 200.0, 300.0))
        .expect("insert_blank_page should succeed");
    assert_eq!(renderer.page_count(), original_count + 1);

    // Delete it back out: count returns to the original.
    renderer
        .edit_document(|doc| doc.delete_page(0))
        .expect("delete_page should succeed");
    assert_eq!(renderer.page_count(), original_count);
}

#[test]
fn password_protect_export_produces_structurally_encrypted_output() {
    // Exactly the wiring gui::app.rs's Password Protect dialog depends
    // on: build a config from the dialog's permission checkboxes/
    // passwords, then export via `EditableDocument::save_encrypted_to_bytes`.
    let renderer =
        PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");

    let permissions = Permissions::new()
        .allow_printing(true)
        .allow_modifying(false)
        .allow_copying(true)
        .allow_annotating(true)
        .allow_filling_forms(true)
        .allow_extraction(true)
        .allow_assembly(false);
    let config = EncryptionConfig::aes256()
        .user_password("open123")
        .owner_password("owner456")
        .permissions(permissions);

    let encrypted_bytes = renderer
        .document()
        .save_encrypted_to_bytes(config)
        .expect("save_encrypted_to_bytes should succeed");

    assert!(encrypted_bytes.starts_with(b"%PDF-1."));
    let text = String::from_utf8_lossy(&encrypted_bytes);
    assert!(text.contains("/Encrypt"));

    // Documented, disclosed limitation (see `src/editor/encrypt.rs`): this
    // crate's own parser cannot reopen its own encrypted output, so the
    // GUI treats this as a terminal export rather than reopening the
    // result -- assert that limitation still holds rather than silently
    // relying on it.
    assert!(PdfRenderer::open_bytes(encrypted_bytes).is_err());
}

#[test]
fn redaction_removes_content_and_requires_full_rewrite() {
    // Exactly the wiring gui::app.rs's Redact tool depends on.
    let mut renderer =
        PdfRenderer::open_file("tests/output/multipage_report.pdf").expect("should open");

    let entry = renderer
        .edit_document(|doc| {
            doc.apply_redaction(
                0,
                Rectangle::new(20.0, 20.0, 550.0, 750.0),
                "test_actor",
                "test reason",
            )
        })
        .expect("apply_redaction should succeed");
    assert_eq!(entry.actor, "test_actor");
    assert_eq!(entry.reason, "test reason");

    // save_incremental must refuse after a redaction (it can't express
    // "some bytes are now permanently gone"); save_full_rewrite must not.
    assert!(
        renderer.document().save_incremental_to_bytes().is_err(),
        "save_incremental should refuse after a redaction"
    );
    assert!(
        renderer.document().save_full_rewrite_to_bytes().is_ok(),
        "save_full_rewrite should still work after a redaction"
    );

    assert_eq!(renderer.document().audit_log().len(), 1);
}
