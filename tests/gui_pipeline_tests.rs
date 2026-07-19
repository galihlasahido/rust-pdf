//! Non-interactive checks for the `native-gui` render pipeline: the actual
//! new wiring (`PdfRenderer` output -> `egui::ColorImage`) is what a manual
//! click-through of the windowed app can't easily be automated to prove, so
//! it's covered here instead. Does not spawn a window/event loop.

#![cfg(feature = "native-gui")]

use rust_pdf::render::PdfRenderer;

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
