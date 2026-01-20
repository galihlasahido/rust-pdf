//! Integration tests for rust-pdf library.
//!
//! These tests generate actual PDF files to the tests/output directory
//! for manual verification.

use rust_pdf::prelude::*;
use std::fs;
use std::path::PathBuf;

/// Returns the output directory path, creating it if necessary.
fn output_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("output");
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Helper to save a document and verify it was created.
fn save_and_verify(doc: Document, name: &str) -> Vec<u8> {
    let path = output_dir().join(name);
    doc.save_to_file(&path).unwrap();

    let bytes = fs::read(&path).unwrap();
    let content = String::from_utf8_lossy(&bytes);

    // Basic PDF structure verification
    assert!(content.starts_with("%PDF-"), "Missing PDF header");
    assert!(content.contains("%%EOF"), "Missing EOF marker");
    assert!(content.contains("/Type /Catalog"), "Missing catalog");
    assert!(content.contains("xref"), "Missing xref table");
    assert!(content.contains("trailer"), "Missing trailer");

    println!("Created: {} ({} bytes)", path.display(), bytes.len());
    bytes
}

#[test]
fn test_minimal_pdf() {
    let page = PageBuilder::a4().build();

    let doc = DocumentBuilder::new()
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "minimal.pdf");
}

#[test]
fn test_hello_world_pdf() {
    let content = ContentBuilder::new()
        .text("F1", 24.0, 72.0, 750.0, "Hello, World!");

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Hello World")
        .author("rust-pdf")
        .page(page)
        .build()
        .unwrap();

    let bytes = save_and_verify(doc, "hello_world.pdf");
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("Hello, World!"));
}

#[test]
fn test_multiline_text() {
    let text_block = TextBuilder::new()
        .font("F1", 14.0)
        .position(72.0, 750.0)
        .leading(18.0)
        .show("Line 1: This is the first line of text.")
        .next_line()
        .show("Line 2: This is the second line.")
        .next_line()
        .show("Line 3: And here's a third line.")
        .next_line()
        .show("Line 4: Finally, the last line.");

    let content = ContentBuilder::new().text_block(text_block);

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::TimesRoman)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Multiline Text Test")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "multiline_text.pdf");
}

#[test]
fn test_graphics_shapes() {
    let graphics = GraphicsBuilder::new()
        // Red filled rectangle
        .save_state()
        .fill_color(Color::rgb(1.0, 0.0, 0.0))
        .rect(72.0, 650.0, 150.0, 100.0)
        .fill()
        .restore_state()
        // Blue stroked rectangle
        .save_state()
        .stroke_color(Color::rgb(0.0, 0.0, 1.0))
        .line_width(3.0)
        .rect(250.0, 650.0, 150.0, 100.0)
        .stroke()
        .restore_state()
        // Green filled circle
        .save_state()
        .fill_color(Color::rgb(0.0, 0.5, 0.0))
        .filled_circle(147.0, 500.0, 50.0)
        .restore_state()
        // Orange stroked circle
        .save_state()
        .stroke_color(Color::rgb(1.0, 0.5, 0.0))
        .line_width(2.0)
        .stroked_circle(325.0, 500.0, 50.0)
        .restore_state()
        // Diagonal line
        .save_state()
        .stroke_color(Color::gray(0.3))
        .line_width(1.0)
        .line(72.0, 350.0, 500.0, 350.0)
        .restore_state()
        // Dashed line
        .save_state()
        .stroke_color(Color::BLACK)
        .dashed_line(5.0, 3.0)
        .line(72.0, 300.0, 500.0, 300.0)
        .restore_state();

    let content = ContentBuilder::new()
        .graphics(graphics)
        .text("F1", 16.0, 72.0, 780.0, "Graphics Test: Shapes and Lines");

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Graphics Test")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "graphics_shapes.pdf");
}

#[test]
fn test_all_standard_fonts() {
    let mut content = ContentBuilder::new()
        .text("F0", 18.0, 72.0, 800.0, "The 14 Standard PDF Fonts");

    let fonts = Standard14Font::all();
    let mut page_fonts = vec![("F0".to_string(), Font::from(Standard14Font::HelveticaBold))];

    for (i, font) in fonts.iter().enumerate() {
        let font_name = format!("F{}", i + 1);
        let y = 760.0 - (i as f64 * 50.0);

        // Font name label
        content = content.text("F0", 10.0, 72.0, y, &format!("{}: ", font.postscript_name()));

        // Sample text in that font
        content = content.text(&font_name, 14.0, 180.0, y, "The quick brown fox jumps over the lazy dog");

        page_fonts.push((font_name, Font::from(*font)));
    }

    let mut page = PageBuilder::a4().content(content).build();
    for (name, font) in page_fonts {
        page.add_font(name, font);
    }

    let doc = DocumentBuilder::new()
        .title("Standard 14 Fonts")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "all_standard_fonts.pdf");
}

#[test]
fn test_multi_page_document() {
    let mut pages = Vec::new();

    for i in 1..=5 {
        let content = ContentBuilder::new()
            .text("F1", 24.0, 72.0, 750.0, &format!("Page {} of 5", i))
            .text("F1", 12.0, 72.0, 700.0, &format!("This is content on page {}.", i));

        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(content)
            .build();

        pages.push(page);
    }

    let doc = DocumentBuilder::new()
        .title("Multi-page Document")
        .pages(pages)
        .build()
        .unwrap();

    assert_eq!(doc.page_count(), 5);

    let bytes = save_and_verify(doc, "multi_page.pdf");
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("/Count 5"));
}

#[test]
fn test_color_modes() {
    let content = ContentBuilder::new()
        .text("F1", 16.0, 72.0, 800.0, "Color Modes Test")
        // Grayscale
        .text("F1", 12.0, 72.0, 750.0, "Grayscale:")
        .fill_color(Color::gray(0.0))
        .rect(150.0, 745.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::gray(0.25))
        .rect(200.0, 745.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::gray(0.5))
        .rect(250.0, 745.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::gray(0.75))
        .rect(300.0, 745.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::gray(1.0))
        .stroke_color(Color::BLACK)
        .line_width(0.5)
        .rect(350.0, 745.0, 40.0, 20.0)
        .fill_and_stroke()
        // RGB
        .text("F1", 12.0, 72.0, 700.0, "RGB:")
        .fill_color(Color::RED)
        .rect(150.0, 695.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::GREEN)
        .rect(200.0, 695.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::BLUE)
        .rect(250.0, 695.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::rgb(1.0, 1.0, 0.0))
        .rect(300.0, 695.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::rgb(1.0, 0.0, 1.0))
        .rect(350.0, 695.0, 40.0, 20.0)
        .fill()
        // CMYK
        .text("F1", 12.0, 72.0, 650.0, "CMYK:")
        .fill_color(Color::cmyk(1.0, 0.0, 0.0, 0.0))
        .rect(150.0, 645.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::cmyk(0.0, 1.0, 0.0, 0.0))
        .rect(200.0, 645.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::cmyk(0.0, 0.0, 1.0, 0.0))
        .rect(250.0, 645.0, 40.0, 20.0)
        .fill()
        .fill_color(Color::cmyk(0.0, 0.0, 0.0, 1.0))
        .rect(300.0, 645.0, 40.0, 20.0)
        .fill();

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Color Modes")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "color_modes.pdf");
}

#[test]
fn test_transformations() {
    let content = ContentBuilder::new()
        .text("F1", 16.0, 72.0, 800.0, "Transformation Test")
        // Normal rectangle
        .save_state()
        .fill_color(Color::rgb(0.8, 0.8, 0.8))
        .rect(72.0, 600.0, 100.0, 50.0)
        .fill()
        .restore_state()
        // Translated rectangle
        .save_state()
        .translate(200.0, 0.0)
        .fill_color(Color::rgb(0.6, 0.6, 1.0))
        .rect(72.0, 600.0, 100.0, 50.0)
        .fill()
        .restore_state()
        // Scaled rectangle
        .save_state()
        .translate(72.0, 450.0)
        .scale(1.5, 1.5)
        .fill_color(Color::rgb(0.6, 1.0, 0.6))
        .rect(0.0, 0.0, 100.0, 50.0)
        .fill()
        .restore_state()
        // Rotated rectangle
        .save_state()
        .translate(350.0, 500.0)
        .rotate(30.0)
        .fill_color(Color::rgb(1.0, 0.6, 0.6))
        .rect(-50.0, -25.0, 100.0, 50.0)
        .fill()
        .restore_state();

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Transformations")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "transformations.pdf");
}

#[test]
fn test_pdf_version_2() {
    let content = ContentBuilder::new()
        .text("F1", 24.0, 72.0, 750.0, "PDF 2.0 Document");

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .version(PdfVersion::V2_0)
        .title("PDF 2.0 Test")
        .page(page)
        .build()
        .unwrap();

    let bytes = save_and_verify(doc, "pdf_version_2.pdf");
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.starts_with("%PDF-2.0"));
}

#[test]
fn test_document_metadata() {
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Document with metadata"))
        .build();

    let doc = DocumentBuilder::new()
        .version(PdfVersion::V1_7)
        .title("Metadata Test Document")
        .author("Test Author Name")
        .subject("Testing PDF metadata")
        .keywords("rust, pdf, test, metadata")
        .creator("rust-pdf integration tests")
        .producer("rust-pdf library v0.1.0")
        .page(page)
        .build()
        .unwrap();

    let bytes = save_and_verify(doc, "document_metadata.pdf");
    let content = String::from_utf8_lossy(&bytes);

    assert!(content.contains("/Title (Metadata Test Document)"));
    assert!(content.contains("/Author (Test Author Name)"));
    assert!(content.contains("/Subject (Testing PDF metadata)"));
    assert!(content.contains("/Keywords (rust, pdf, test, metadata)"));
    assert!(content.contains("/Creator (rust-pdf integration tests)"));
    assert!(content.contains("/Producer (rust-pdf library v0.1.0)"));
}

#[test]
fn test_different_page_sizes() {
    let pages = vec![
        ("A4", PageBuilder::a4()),
        ("Letter", PageBuilder::letter()),
        ("A3", PageBuilder::a3()),
        ("A5", PageBuilder::a5()),
        ("Legal", PageBuilder::legal()),
        ("Custom 400x600", PageBuilder::custom(400.0, 600.0)),
    ];

    let pages: Vec<Page> = pages
        .into_iter()
        .map(|(name, builder)| {
            builder
                .font("F1", Standard14Font::Helvetica)
                .content(ContentBuilder::new().text("F1", 18.0, 72.0, 100.0, &format!("Page size: {}", name)))
                .build()
        })
        .collect();

    let doc = DocumentBuilder::new()
        .title("Page Sizes")
        .pages(pages)
        .build()
        .unwrap();

    save_and_verify(doc, "page_sizes.pdf");
}

#[test]
fn test_text_with_special_characters() {
    let content = ContentBuilder::new()
        .text("F1", 14.0, 72.0, 750.0, "Special characters test:")
        .text("F1", 12.0, 72.0, 720.0, "Parentheses: (hello) (world)")
        .text("F1", 12.0, 72.0, 700.0, "Backslash: C:\\Users\\test")
        .text("F1", 12.0, 72.0, 680.0, "Quotes: \"Hello\" and 'World'")
        .text("F1", 12.0, 72.0, 660.0, "Ampersand: Tom & Jerry")
        .text("F1", 12.0, 72.0, 640.0, "Less/Greater: 5 < 10 > 3")
        .text("F1", 12.0, 72.0, 620.0, "Percent: 100% complete");

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Special Characters")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "special_characters.pdf");
}

#[test]
fn test_complex_graphics_path() {
    let content = ContentBuilder::new()
        .text("F1", 16.0, 72.0, 800.0, "Complex Path Test")
        // Draw a star shape
        .save_state()
        .fill_color(Color::rgb(1.0, 0.8, 0.0))
        .stroke_color(Color::rgb(0.8, 0.4, 0.0))
        .line_width(2.0)
        .move_to(300.0, 700.0)
        .line_to(315.0, 650.0)
        .line_to(370.0, 650.0)
        .line_to(325.0, 615.0)
        .line_to(345.0, 560.0)
        .line_to(300.0, 595.0)
        .line_to(255.0, 560.0)
        .line_to(275.0, 615.0)
        .line_to(230.0, 650.0)
        .line_to(285.0, 650.0)
        .close_path()
        .fill_and_stroke()
        .restore_state()
        // Draw a bezier curve
        .save_state()
        .stroke_color(Color::BLUE)
        .line_width(3.0)
        .move_to(72.0, 400.0)
        .curve_to(150.0, 500.0, 250.0, 300.0, 350.0, 400.0)
        .stroke()
        .restore_state();

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Complex Paths")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "complex_paths.pdf");
}

// ============================================================================
// GRAPHICS EXAMPLES
// ============================================================================

#[test]
fn test_line_styles() {
    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 800.0, "Line Styles Demo")
        // Line width variations
        .text("F1", 12.0, 72.0, 760.0, "Line widths:")
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(0.5)
        .move_to(150.0, 755.0)
        .line_to(500.0, 755.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(1.0)
        .move_to(150.0, 740.0)
        .line_to(500.0, 740.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(2.0)
        .move_to(150.0, 720.0)
        .line_to(500.0, 720.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(4.0)
        .move_to(150.0, 695.0)
        .line_to(500.0, 695.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(8.0)
        .move_to(150.0, 665.0)
        .line_to(500.0, 665.0)
        .stroke()
        .restore_state()
        // Line cap styles
        .text("F1", 12.0, 72.0, 620.0, "Line caps (butt, round, square):")
        .save_state()
        .line_width(15.0)
        .stroke_color(Color::BLUE)
        .line_cap(0) // Butt
        .move_to(150.0, 600.0)
        .line_to(250.0, 600.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(15.0)
        .stroke_color(Color::GREEN)
        .line_cap(1) // Round
        .move_to(280.0, 600.0)
        .line_to(380.0, 600.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(15.0)
        .stroke_color(Color::RED)
        .line_cap(2) // Square
        .move_to(410.0, 600.0)
        .line_to(510.0, 600.0)
        .stroke()
        .restore_state()
        // Line join styles
        .text("F1", 12.0, 72.0, 540.0, "Line joins (miter, round, bevel):")
        .save_state()
        .line_width(10.0)
        .stroke_color(Color::BLUE)
        .line_join(0) // Miter
        .move_to(150.0, 520.0)
        .line_to(180.0, 480.0)
        .line_to(210.0, 520.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(10.0)
        .stroke_color(Color::GREEN)
        .line_join(1) // Round
        .move_to(280.0, 520.0)
        .line_to(310.0, 480.0)
        .line_to(340.0, 520.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(10.0)
        .stroke_color(Color::RED)
        .line_join(2) // Bevel
        .move_to(410.0, 520.0)
        .line_to(440.0, 480.0)
        .line_to(470.0, 520.0)
        .stroke()
        .restore_state()
        // Dash patterns
        .text("F1", 12.0, 72.0, 420.0, "Dash patterns:")
        .save_state()
        .line_width(2.0)
        .dash(vec![5.0, 5.0], 0.0)
        .move_to(150.0, 400.0)
        .line_to(500.0, 400.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(2.0)
        .dash(vec![10.0, 5.0], 0.0)
        .move_to(150.0, 380.0)
        .line_to(500.0, 380.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(2.0)
        .dash(vec![15.0, 5.0, 5.0, 5.0], 0.0)
        .move_to(150.0, 360.0)
        .line_to(500.0, 360.0)
        .stroke()
        .restore_state()
        .save_state()
        .line_width(2.0)
        .dash(vec![1.0, 3.0], 0.0)
        .move_to(150.0, 340.0)
        .line_to(500.0, 340.0)
        .stroke()
        .restore_state();

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Line Styles")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "line_styles.pdf");
}

#[test]
fn test_polygons() {
    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 800.0, "Polygons Demo")
        // Triangle
        .text("F1", 10.0, 85.0, 720.0, "Triangle")
        .save_state()
        .fill_color(Color::rgb(0.9, 0.3, 0.3))
        .move_to(120.0, 650.0)
        .line_to(70.0, 580.0)
        .line_to(170.0, 580.0)
        .close_path()
        .fill()
        .restore_state()
        // Square
        .text("F1", 10.0, 225.0, 720.0, "Square")
        .save_state()
        .fill_color(Color::rgb(0.3, 0.9, 0.3))
        .rect(200.0, 580.0, 80.0, 80.0)
        .fill()
        .restore_state()
        // Pentagon
        .text("F1", 10.0, 350.0, 720.0, "Pentagon")
        .save_state()
        .fill_color(Color::rgb(0.3, 0.3, 0.9))
        .move_to(380.0, 660.0)
        .line_to(418.0, 628.0)
        .line_to(404.0, 582.0)
        .line_to(356.0, 582.0)
        .line_to(342.0, 628.0)
        .close_path()
        .fill()
        .restore_state()
        // Hexagon
        .text("F1", 10.0, 480.0, 720.0, "Hexagon")
        .save_state()
        .fill_color(Color::rgb(0.9, 0.6, 0.0))
        .move_to(540.0, 660.0)
        .line_to(575.0, 640.0)
        .line_to(575.0, 600.0)
        .line_to(540.0, 580.0)
        .line_to(505.0, 600.0)
        .line_to(505.0, 640.0)
        .close_path()
        .fill()
        .restore_state()
        // Star (5-point)
        .text("F1", 10.0, 85.0, 520.0, "5-Point Star")
        .save_state()
        .fill_color(Color::rgb(1.0, 0.8, 0.0))
        .stroke_color(Color::rgb(0.8, 0.5, 0.0))
        .line_width(2.0)
        .move_to(120.0, 480.0)
        .line_to(135.0, 430.0)
        .line_to(185.0, 430.0)
        .line_to(145.0, 395.0)
        .line_to(160.0, 345.0)
        .line_to(120.0, 375.0)
        .line_to(80.0, 345.0)
        .line_to(95.0, 395.0)
        .line_to(55.0, 430.0)
        .line_to(105.0, 430.0)
        .close_path()
        .fill_and_stroke()
        .restore_state()
        // Arrow
        .text("F1", 10.0, 225.0, 520.0, "Arrow")
        .save_state()
        .fill_color(Color::rgb(0.5, 0.0, 0.5))
        .move_to(200.0, 410.0)
        .line_to(280.0, 410.0)
        .line_to(280.0, 440.0)
        .line_to(320.0, 395.0)
        .line_to(280.0, 350.0)
        .line_to(280.0, 380.0)
        .line_to(200.0, 380.0)
        .close_path()
        .fill()
        .restore_state()
        // House shape
        .text("F1", 10.0, 365.0, 520.0, "House")
        .save_state()
        .fill_color(Color::rgb(0.6, 0.4, 0.2))
        .move_to(380.0, 480.0)
        .line_to(430.0, 430.0)
        .line_to(430.0, 350.0)
        .line_to(330.0, 350.0)
        .line_to(330.0, 430.0)
        .close_path()
        .fill()
        .restore_state()
        .save_state()
        .fill_color(Color::rgb(0.8, 0.2, 0.2))
        .move_to(380.0, 500.0)
        .line_to(320.0, 430.0)
        .line_to(440.0, 430.0)
        .close_path()
        .fill()
        .restore_state()
        // Cross
        .text("F1", 10.0, 490.0, 520.0, "Cross")
        .save_state()
        .fill_color(Color::RED)
        .move_to(520.0, 480.0)
        .line_to(540.0, 480.0)
        .line_to(540.0, 440.0)
        .line_to(580.0, 440.0)
        .line_to(580.0, 420.0)
        .line_to(540.0, 420.0)
        .line_to(540.0, 380.0)
        .line_to(520.0, 380.0)
        .line_to(520.0, 420.0)
        .line_to(480.0, 420.0)
        .line_to(480.0, 440.0)
        .line_to(520.0, 440.0)
        .close_path()
        .fill()
        .restore_state();

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Polygons")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "polygons.pdf");
}

#[test]
fn test_circles_and_curves() {
    let graphics = GraphicsBuilder::new()
        // Filled circles in a row
        .save_state()
        .fill_color(Color::RED)
        .filled_circle(100.0, 700.0, 30.0)
        .restore_state()
        .save_state()
        .fill_color(Color::GREEN)
        .filled_circle(180.0, 700.0, 30.0)
        .restore_state()
        .save_state()
        .fill_color(Color::BLUE)
        .filled_circle(260.0, 700.0, 30.0)
        .restore_state()
        .save_state()
        .fill_color(Color::rgb(1.0, 0.5, 0.0))
        .filled_circle(340.0, 700.0, 30.0)
        .restore_state()
        .save_state()
        .fill_color(Color::rgb(0.5, 0.0, 0.5))
        .filled_circle(420.0, 700.0, 30.0)
        .restore_state()
        // Concentric circles
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.0)
        .stroked_circle(200.0, 550.0, 20.0)
        .stroked_circle(200.0, 550.0, 40.0)
        .stroked_circle(200.0, 550.0, 60.0)
        .stroked_circle(200.0, 550.0, 80.0)
        .stroked_circle(200.0, 550.0, 100.0)
        .restore_state()
        // Olympic rings style
        .save_state()
        .stroke_color(Color::rgb(0.0, 0.0, 0.7))
        .line_width(4.0)
        .stroked_circle(380.0, 580.0, 30.0)
        .restore_state()
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(4.0)
        .stroked_circle(450.0, 580.0, 30.0)
        .restore_state()
        .save_state()
        .stroke_color(Color::rgb(0.8, 0.0, 0.0))
        .line_width(4.0)
        .stroked_circle(520.0, 580.0, 30.0)
        .restore_state()
        .save_state()
        .stroke_color(Color::rgb(1.0, 0.8, 0.0))
        .line_width(4.0)
        .stroked_circle(415.0, 545.0, 30.0)
        .restore_state()
        .save_state()
        .stroke_color(Color::rgb(0.0, 0.6, 0.0))
        .line_width(4.0)
        .stroked_circle(485.0, 545.0, 30.0)
        .restore_state()
        // Bezier curves
        .save_state()
        .stroke_color(Color::rgb(0.8, 0.0, 0.4))
        .line_width(3.0)
        .move_to(72.0, 350.0)
        .curve_to(150.0, 450.0, 250.0, 250.0, 350.0, 350.0)
        .stroke()
        .restore_state()
        .save_state()
        .stroke_color(Color::rgb(0.0, 0.4, 0.8))
        .line_width(3.0)
        .move_to(350.0, 350.0)
        .curve_to(450.0, 450.0, 500.0, 250.0, 530.0, 350.0)
        .stroke()
        .restore_state()
        // Wave pattern
        .save_state()
        .stroke_color(Color::rgb(0.2, 0.6, 0.2))
        .line_width(2.0)
        .move_to(72.0, 200.0)
        .curve_to(122.0, 250.0, 172.0, 150.0, 222.0, 200.0)
        .curve_to(272.0, 250.0, 322.0, 150.0, 372.0, 200.0)
        .curve_to(422.0, 250.0, 472.0, 150.0, 522.0, 200.0)
        .stroke()
        .restore_state();

    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 800.0, "Circles and Curves Demo")
        .text("F1", 10.0, 72.0, 750.0, "Filled circles:")
        .text("F1", 10.0, 72.0, 620.0, "Concentric circles:")
        .text("F1", 10.0, 350.0, 620.0, "Olympic rings:")
        .text("F1", 10.0, 72.0, 400.0, "Bezier curves:")
        .text("F1", 10.0, 72.0, 260.0, "Wave pattern:")
        .graphics(graphics);

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Circles and Curves")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "circles_curves.pdf");
}

#[test]
fn test_overlapping_shapes() {
    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 800.0, "Overlapping Shapes Demo")
        // Overlapping rectangles
        .save_state()
        .fill_color(Color::rgb(1.0, 0.0, 0.0))
        .rect(100.0, 600.0, 150.0, 100.0)
        .fill()
        .restore_state()
        .save_state()
        .fill_color(Color::rgb(0.0, 1.0, 0.0))
        .rect(150.0, 650.0, 150.0, 100.0)
        .fill()
        .restore_state()
        .save_state()
        .fill_color(Color::rgb(0.0, 0.0, 1.0))
        .rect(200.0, 700.0, 150.0, 100.0)
        .fill()
        .restore_state()
        // Overlapping circles (Venn diagram style)
        .save_state()
        .fill_color(Color::rgb(1.0, 0.5, 0.5))
        .move_to(470.0, 700.0)
        .curve_to(470.0, 744.0, 434.0, 780.0, 390.0, 780.0)
        .curve_to(346.0, 780.0, 310.0, 744.0, 310.0, 700.0)
        .curve_to(310.0, 656.0, 346.0, 620.0, 390.0, 620.0)
        .curve_to(434.0, 620.0, 470.0, 656.0, 470.0, 700.0)
        .fill()
        .restore_state()
        .save_state()
        .fill_color(Color::rgb(0.5, 1.0, 0.5))
        .move_to(530.0, 700.0)
        .curve_to(530.0, 744.0, 494.0, 780.0, 450.0, 780.0)
        .curve_to(406.0, 780.0, 370.0, 744.0, 370.0, 700.0)
        .curve_to(370.0, 656.0, 406.0, 620.0, 450.0, 620.0)
        .curve_to(494.0, 620.0, 530.0, 656.0, 530.0, 700.0)
        .fill()
        .restore_state()
        // Stacked squares with different colors
        .text("F1", 10.0, 72.0, 560.0, "Stacked squares:")
        .save_state()
        .fill_color(Color::rgb(0.2, 0.2, 0.8))
        .rect(100.0, 350.0, 200.0, 200.0)
        .fill()
        .restore_state()
        .save_state()
        .fill_color(Color::rgb(0.8, 0.2, 0.2))
        .rect(130.0, 380.0, 140.0, 140.0)
        .fill()
        .restore_state()
        .save_state()
        .fill_color(Color::rgb(0.2, 0.8, 0.2))
        .rect(160.0, 410.0, 80.0, 80.0)
        .fill()
        .restore_state()
        .save_state()
        .fill_color(Color::WHITE)
        .rect(180.0, 430.0, 40.0, 40.0)
        .fill()
        .restore_state()
        // Concentric rectangles
        .text("F1", 10.0, 350.0, 560.0, "Concentric rectangles:")
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(2.0)
        .rect(380.0, 350.0, 180.0, 180.0)
        .stroke()
        .rect(395.0, 365.0, 150.0, 150.0)
        .stroke()
        .rect(410.0, 380.0, 120.0, 120.0)
        .stroke()
        .rect(425.0, 395.0, 90.0, 90.0)
        .stroke()
        .rect(440.0, 410.0, 60.0, 60.0)
        .stroke()
        .rect(455.0, 425.0, 30.0, 30.0)
        .stroke()
        .restore_state();

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Overlapping Shapes")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "overlapping_shapes.pdf");
}

#[test]
fn test_grid_pattern() {
    let mut content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 800.0, "Grid Pattern Demo")
        .save_state()
        .stroke_color(Color::gray(0.7))
        .line_width(0.5);

    // Draw vertical lines
    for i in 0..11 {
        let x = 72.0 + (i as f64 * 45.0);
        content = content
            .move_to(x, 300.0)
            .line_to(x, 750.0)
            .stroke();
    }

    // Draw horizontal lines
    for i in 0..11 {
        let y = 300.0 + (i as f64 * 45.0);
        content = content
            .move_to(72.0, y)
            .line_to(522.0, y)
            .stroke();
    }

    content = content.restore_state();

    // Draw some colored cells
    let cells = [
        (0, 0, Color::RED),
        (1, 1, Color::GREEN),
        (2, 2, Color::BLUE),
        (3, 3, Color::rgb(1.0, 1.0, 0.0)),
        (4, 4, Color::rgb(1.0, 0.0, 1.0)),
        (5, 5, Color::rgb(0.0, 1.0, 1.0)),
        (0, 9, Color::rgb(0.5, 0.5, 0.5)),
        (9, 0, Color::rgb(1.0, 0.5, 0.0)),
    ];

    for (col, row, color) in cells.iter() {
        let x = 72.0 + (*col as f64 * 45.0);
        let y = 300.0 + (*row as f64 * 45.0);
        content = content
            .save_state()
            .fill_color(color.clone())
            .rect(x, y, 45.0, 45.0)
            .fill()
            .restore_state();
    }

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Grid Pattern")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "grid_pattern.pdf");
}

#[test]
fn test_color_palette() {
    let mut content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 800.0, "Color Palette Demo")
        .text("F1", 12.0, 72.0, 760.0, "Named Colors:");

    let colors = [
        ("RED", Color::RED),
        ("GREEN", Color::GREEN),
        ("BLUE", Color::BLUE),
        ("BLACK", Color::BLACK),
        ("WHITE", Color::WHITE),
        ("YELLOW", Color::rgb(1.0, 1.0, 0.0)),
        ("CYAN", Color::rgb(0.0, 1.0, 1.0)),
        ("MAGENTA", Color::rgb(1.0, 0.0, 1.0)),
    ];

    for (i, (name, color)) in colors.iter().enumerate() {
        let col = (i % 4) as f64;
        let row = (i / 4) as f64;
        let x = 72.0 + col * 130.0;
        let y = 680.0 - row * 60.0;

        content = content
            .save_state()
            .fill_color(color.clone())
            .rect(x, y, 40.0, 40.0)
            .fill()
            .restore_state();

        if matches!(color, Color::Rgb(c) if c.r == 1.0 && c.g == 1.0 && c.b == 1.0) {
            content = content
                .save_state()
                .stroke_color(Color::BLACK)
                .line_width(0.5)
                .rect(x, y, 40.0, 40.0)
                .stroke()
                .restore_state();
        }

        content = content.text("F1", 9.0, x + 45.0, y + 15.0, name);
    }

    // RGB Gradient
    content = content.text("F1", 12.0, 72.0, 550.0, "RGB Gradient:");
    for i in 0..20 {
        let x = 72.0 + (i as f64 * 24.0);
        let r = i as f64 / 19.0;
        content = content
            .save_state()
            .fill_color(Color::rgb(r, 0.0, 0.0))
            .rect(x, 490.0, 24.0, 40.0)
            .fill()
            .restore_state();
    }

    for i in 0..20 {
        let x = 72.0 + (i as f64 * 24.0);
        let g = i as f64 / 19.0;
        content = content
            .save_state()
            .fill_color(Color::rgb(0.0, g, 0.0))
            .rect(x, 440.0, 24.0, 40.0)
            .fill()
            .restore_state();
    }

    for i in 0..20 {
        let x = 72.0 + (i as f64 * 24.0);
        let b = i as f64 / 19.0;
        content = content
            .save_state()
            .fill_color(Color::rgb(0.0, 0.0, b))
            .rect(x, 390.0, 24.0, 40.0)
            .fill()
            .restore_state();
    }

    // Grayscale gradient
    content = content.text("F1", 12.0, 72.0, 350.0, "Grayscale Gradient:");
    for i in 0..20 {
        let x = 72.0 + (i as f64 * 24.0);
        let gray = i as f64 / 19.0;
        content = content
            .save_state()
            .fill_color(Color::gray(gray))
            .rect(x, 290.0, 24.0, 40.0)
            .fill()
            .restore_state();
    }

    // CMYK colors
    content = content.text("F1", 12.0, 72.0, 250.0, "CMYK Colors:");
    let cmyk_colors = [
        ("Cyan", 1.0, 0.0, 0.0, 0.0),
        ("Magenta", 0.0, 1.0, 0.0, 0.0),
        ("Yellow", 0.0, 0.0, 1.0, 0.0),
        ("Black", 0.0, 0.0, 0.0, 1.0),
        ("Orange", 0.0, 0.5, 1.0, 0.0),
        ("Purple", 0.5, 1.0, 0.0, 0.0),
    ];

    for (i, (name, c, m, y, k)) in cmyk_colors.iter().enumerate() {
        let x = 72.0 + (i as f64 * 85.0);
        content = content
            .save_state()
            .fill_color(Color::cmyk(*c, *m, *y, *k))
            .rect(x, 190.0, 40.0, 40.0)
            .fill()
            .restore_state()
            .text("F1", 8.0, x, 180.0, name);
    }

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Color Palette")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "color_palette.pdf");
}

// ============================================================================
// TEXT EXAMPLES
// ============================================================================

#[test]
fn test_text_styles() {
    let mut page = PageBuilder::a4().build();
    page.add_font("Helvetica", Font::from(Standard14Font::Helvetica));
    page.add_font("HelveticaBold", Font::from(Standard14Font::HelveticaBold));
    page.add_font("HelveticaOblique", Font::from(Standard14Font::HelveticaOblique));
    page.add_font("HelveticaBoldOblique", Font::from(Standard14Font::HelveticaBoldOblique));
    page.add_font("Times", Font::from(Standard14Font::TimesRoman));
    page.add_font("TimesBold", Font::from(Standard14Font::TimesBold));
    page.add_font("TimesItalic", Font::from(Standard14Font::TimesItalic));
    page.add_font("Courier", Font::from(Standard14Font::Courier));

    let content = ContentBuilder::new()
        .text("HelveticaBold", 20.0, 72.0, 780.0, "Text Styles Demo")
        // Font variations
        .text("Helvetica", 12.0, 72.0, 740.0, "Regular Helvetica text")
        .text("HelveticaBold", 12.0, 72.0, 720.0, "Bold Helvetica text")
        .text("HelveticaOblique", 12.0, 72.0, 700.0, "Oblique Helvetica text")
        .text("HelveticaBoldOblique", 12.0, 72.0, 680.0, "Bold Oblique Helvetica text")
        // Times variations
        .text("Times", 12.0, 72.0, 640.0, "Regular Times Roman text")
        .text("TimesBold", 12.0, 72.0, 620.0, "Bold Times text")
        .text("TimesItalic", 12.0, 72.0, 600.0, "Italic Times text")
        // Courier
        .text("Courier", 12.0, 72.0, 560.0, "Monospace Courier text")
        // Font sizes
        .text("Helvetica", 14.0, 72.0, 520.0, "Font Sizes:")
        .text("Helvetica", 8.0, 72.0, 500.0, "8pt text")
        .text("Helvetica", 10.0, 72.0, 485.0, "10pt text")
        .text("Helvetica", 12.0, 72.0, 468.0, "12pt text")
        .text("Helvetica", 14.0, 72.0, 448.0, "14pt text")
        .text("Helvetica", 18.0, 72.0, 425.0, "18pt text")
        .text("Helvetica", 24.0, 72.0, 395.0, "24pt text")
        .text("Helvetica", 36.0, 72.0, 355.0, "36pt text");

    page.set_content(content);

    let doc = DocumentBuilder::new()
        .title("Text Styles")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "text_styles.pdf");
}

#[test]
fn test_text_colors() {
    let mut content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 780.0, "Text Colors Demo");

    // Text in different colors
    let colors = [
        ("Red text", Color::RED),
        ("Green text", Color::GREEN),
        ("Blue text", Color::BLUE),
        ("Orange text", Color::rgb(1.0, 0.5, 0.0)),
        ("Purple text", Color::rgb(0.5, 0.0, 0.5)),
        ("Teal text", Color::rgb(0.0, 0.5, 0.5)),
        ("Gray text", Color::gray(0.5)),
        ("Dark gray text", Color::gray(0.3)),
    ];

    for (i, (text, color)) in colors.iter().enumerate() {
        let y = 740.0 - (i as f64 * 30.0);
        content = content
            .save_state()
            .fill_color(color.clone())
            .text("F1", 14.0, 72.0, y, text)
            .restore_state();
    }

    // Rainbow text
    content = content.text("F1", 14.0, 72.0, 450.0, "Rainbow colors:");
    let rainbow = [
        Color::RED,
        Color::rgb(1.0, 0.5, 0.0),
        Color::rgb(1.0, 1.0, 0.0),
        Color::GREEN,
        Color::BLUE,
        Color::rgb(0.3, 0.0, 0.5),
        Color::rgb(0.5, 0.0, 0.5),
    ];

    for (i, color) in rainbow.iter().enumerate() {
        let x = 72.0 + (i as f64 * 70.0);
        content = content
            .save_state()
            .fill_color(color.clone())
            .text("F1", 24.0, x, 400.0, "TEXT")
            .restore_state();
    }

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::HelveticaBold)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Text Colors")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "text_colors.pdf");
}

#[test]
fn test_text_positioning() {
    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 800.0, "Text Positioning Demo")
        // Horizontal positioning
        .text("F1", 12.0, 72.0, 750.0, "Left aligned (x=72)")
        .text("F1", 12.0, 250.0, 730.0, "Centered at x=250")
        .text("F1", 12.0, 400.0, 710.0, "Right area (x=400)")
        // Vertical text ladder
        .text("F1", 10.0, 72.0, 650.0, "Step 1")
        .text("F1", 10.0, 112.0, 630.0, "Step 2")
        .text("F1", 10.0, 152.0, 610.0, "Step 3")
        .text("F1", 10.0, 192.0, 590.0, "Step 4")
        .text("F1", 10.0, 232.0, 570.0, "Step 5")
        // Diagonal text placement
        .text("F1", 10.0, 350.0, 650.0, "Diagonal 1")
        .text("F1", 10.0, 380.0, 620.0, "Diagonal 2")
        .text("F1", 10.0, 410.0, 590.0, "Diagonal 3")
        .text("F1", 10.0, 440.0, 560.0, "Diagonal 4")
        .text("F1", 10.0, 470.0, 530.0, "Diagonal 5")
        // Grid of text
        .text("F1", 12.0, 72.0, 480.0, "Text Grid:")
        .text("F1", 10.0, 72.0, 450.0, "[0,0]")
        .text("F1", 10.0, 172.0, 450.0, "[1,0]")
        .text("F1", 10.0, 272.0, 450.0, "[2,0]")
        .text("F1", 10.0, 372.0, 450.0, "[3,0]")
        .text("F1", 10.0, 72.0, 420.0, "[0,1]")
        .text("F1", 10.0, 172.0, 420.0, "[1,1]")
        .text("F1", 10.0, 272.0, 420.0, "[2,1]")
        .text("F1", 10.0, 372.0, 420.0, "[3,1]")
        .text("F1", 10.0, 72.0, 390.0, "[0,2]")
        .text("F1", 10.0, 172.0, 390.0, "[1,2]")
        .text("F1", 10.0, 272.0, 390.0, "[2,2]")
        .text("F1", 10.0, 372.0, 390.0, "[3,2]");

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Text Positioning")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "text_positioning.pdf");
}

#[test]
fn test_text_spacing() {
    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 800.0, "Text Spacing Demo")
        // Character spacing
        .text("F1", 12.0, 72.0, 750.0, "Character Spacing:")
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 720.0)
                .character_spacing(0.0)
                .show("Normal spacing (0)")
        )
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 700.0)
                .character_spacing(2.0)
                .show("Wide spacing (2)")
        )
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 680.0)
                .character_spacing(5.0)
                .show("Very wide (5)")
        )
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 660.0)
                .character_spacing(-0.5)
                .show("Tight spacing (-0.5)")
        )
        // Word spacing
        .text("F1", 12.0, 72.0, 610.0, "Word Spacing:")
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 580.0)
                .word_spacing(0.0)
                .show("Normal word spacing between words")
        )
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 560.0)
                .word_spacing(10.0)
                .show("Wide word spacing between words")
        )
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 540.0)
                .word_spacing(20.0)
                .show("Very wide word spacing")
        )
        // Leading (line spacing)
        .text("F1", 12.0, 72.0, 490.0, "Leading (line spacing):")
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 460.0)
                .leading(14.0)
                .show("Line 1 with 14pt leading")
                .next_line()
                .show("Line 2 with 14pt leading")
                .next_line()
                .show("Line 3 with 14pt leading")
        )
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(300.0, 460.0)
                .leading(24.0)
                .show("Line 1 with 24pt leading")
                .next_line()
                .show("Line 2 with 24pt leading")
                .next_line()
                .show("Line 3 with 24pt leading")
        )
        // Horizontal scaling
        .text("F1", 12.0, 72.0, 350.0, "Horizontal Scaling:")
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 320.0)
                .horizontal_scaling(100.0)
                .show("Normal (100%)")
        )
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 300.0)
                .horizontal_scaling(150.0)
                .show("Stretched (150%)")
        )
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(72.0, 280.0)
                .horizontal_scaling(75.0)
                .show("Condensed (75%)")
        );

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Text Spacing")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "text_spacing.pdf");
}

#[test]
fn test_paragraph() {
    let _lorem = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris \
nisi ut aliquip ex ea commodo consequat.";

    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 800.0, "Paragraph Demo")
        // Simulate paragraph with multiple lines
        .text_block(
            TextBuilder::new()
                .font("F1", 11.0)
                .position(72.0, 760.0)
                .leading(16.0)
                .show("Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do")
                .next_line()
                .show("eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim")
                .next_line()
                .show("ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut")
                .next_line()
                .show("aliquip ex ea commodo consequat. Duis aute irure dolor in")
                .next_line()
                .show("reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla")
                .next_line()
                .show("pariatur. Excepteur sint occaecat cupidatat non proident, sunt in")
                .next_line()
                .show("culpa qui officia deserunt mollit anim id est laborum.")
        )
        // Another paragraph
        .text("F1", 12.0, 72.0, 620.0, "Second Paragraph:")
        .text_block(
            TextBuilder::new()
                .font("F2", 11.0)
                .position(72.0, 600.0)
                .leading(16.0)
                .show("Curabitur pretium tincidunt lacus. Nulla gravida orci a odio. Nullam")
                .next_line()
                .show("varius, turpis et commodo pharetra, est eros bibendum elit, nec luctus")
                .next_line()
                .show("magna felis sollicitudin mauris. Integer in mauris eu nibh euismod")
                .next_line()
                .show("gravida. Duis ac tellus et risus vulputate vehicula.")
        )
        // Indented paragraph
        .text("F1", 12.0, 72.0, 500.0, "Indented Paragraph:")
        .text_block(
            TextBuilder::new()
                .font("F1", 11.0)
                .position(90.0, 480.0)  // Indented
                .leading(16.0)
                .show("This paragraph is indented from the left margin. Indentation is")
                .next_line()
                .show("commonly used for block quotes, code examples, or to visually")
                .next_line()
                .show("separate sections of text from the main body content.")
        );

    let mut page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();
    page.add_font("F2", Font::from(Standard14Font::TimesRoman));

    let doc = DocumentBuilder::new()
        .title("Paragraphs")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "paragraph.pdf");
}

// ============================================================================
// DOCUMENT TEMPLATES
// ============================================================================

#[test]
fn test_simple_invoice() {
    let content = ContentBuilder::new()
        // Header
        .text("F1", 24.0, 72.0, 780.0, "INVOICE")
        .text("F2", 10.0, 72.0, 755.0, "Invoice #: INV-2024-001")
        .text("F2", 10.0, 72.0, 742.0, "Date: January 20, 2024")
        // Company info (right side)
        .text("F1", 12.0, 400.0, 780.0, "ACME Corporation")
        .text("F2", 10.0, 400.0, 765.0, "123 Business Street")
        .text("F2", 10.0, 400.0, 752.0, "City, State 12345")
        .text("F2", 10.0, 400.0, 739.0, "contact@acme.com")
        // Bill To
        .text("F1", 12.0, 72.0, 690.0, "Bill To:")
        .text("F2", 10.0, 72.0, 675.0, "John Doe")
        .text("F2", 10.0, 72.0, 662.0, "456 Customer Lane")
        .text("F2", 10.0, 72.0, 649.0, "Town, State 67890")
        // Table header
        .save_state()
        .fill_color(Color::gray(0.9))
        .rect(72.0, 580.0, 468.0, 25.0)
        .fill()
        .restore_state()
        .text("F1", 10.0, 80.0, 588.0, "Description")
        .text("F1", 10.0, 320.0, 588.0, "Qty")
        .text("F1", 10.0, 380.0, 588.0, "Price")
        .text("F1", 10.0, 470.0, 588.0, "Total")
        // Table rows
        .save_state()
        .stroke_color(Color::gray(0.7))
        .line_width(0.5)
        .move_to(72.0, 580.0)
        .line_to(540.0, 580.0)
        .stroke()
        .restore_state()
        .text("F2", 10.0, 80.0, 558.0, "Web Development Services")
        .text("F2", 10.0, 330.0, 558.0, "40")
        .text("F2", 10.0, 375.0, 558.0, "$75.00")
        .text("F2", 10.0, 460.0, 558.0, "$3,000.00")
        .text("F2", 10.0, 80.0, 538.0, "Hosting (1 year)")
        .text("F2", 10.0, 330.0, 538.0, "1")
        .text("F2", 10.0, 375.0, 538.0, "$200.00")
        .text("F2", 10.0, 460.0, 538.0, "$200.00")
        .text("F2", 10.0, 80.0, 518.0, "Domain Registration")
        .text("F2", 10.0, 330.0, 518.0, "1")
        .text("F2", 10.0, 375.0, 518.0, "$15.00")
        .text("F2", 10.0, 460.0, 518.0, "$15.00")
        // Totals
        .save_state()
        .stroke_color(Color::gray(0.7))
        .line_width(0.5)
        .move_to(350.0, 480.0)
        .line_to(540.0, 480.0)
        .stroke()
        .restore_state()
        .text("F1", 10.0, 370.0, 458.0, "Subtotal:")
        .text("F2", 10.0, 460.0, 458.0, "$3,215.00")
        .text("F1", 10.0, 370.0, 438.0, "Tax (8%):")
        .text("F2", 10.0, 460.0, 438.0, "$257.20")
        .save_state()
        .fill_color(Color::gray(0.9))
        .rect(350.0, 405.0, 190.0, 25.0)
        .fill()
        .restore_state()
        .text("F1", 12.0, 370.0, 413.0, "TOTAL:")
        .text("F1", 12.0, 455.0, 413.0, "$3,472.20")
        // Footer
        .text("F2", 10.0, 72.0, 350.0, "Payment Terms: Net 30")
        .text("F2", 10.0, 72.0, 335.0, "Please make checks payable to ACME Corporation")
        .text("F2", 9.0, 72.0, 100.0, "Thank you for your business!");

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));

    let doc = DocumentBuilder::new()
        .title("Invoice INV-2024-001")
        .author("ACME Corporation")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "invoice.pdf");
}

#[test]
fn test_business_card() {
    // Business card size: 3.5" x 2" = 252pt x 144pt
    let content = ContentBuilder::new()
        // Background rectangle
        .save_state()
        .fill_color(Color::rgb(0.1, 0.2, 0.4))
        .rect(0.0, 0.0, 252.0, 144.0)
        .fill()
        .restore_state()
        // Accent line
        .save_state()
        .fill_color(Color::rgb(0.9, 0.6, 0.2))
        .rect(0.0, 40.0, 252.0, 3.0)
        .fill()
        .restore_state()
        // Name (white text)
        .save_state()
        .fill_color(Color::WHITE)
        .text("F1", 14.0, 15.0, 110.0, "JOHN DOE")
        .restore_state()
        // Title (light blue text)
        .save_state()
        .fill_color(Color::rgb(0.7, 0.7, 0.8))
        .text("F2", 9.0, 15.0, 95.0, "Senior Software Engineer")
        .restore_state()
        // Contact info (white text)
        .save_state()
        .fill_color(Color::WHITE)
        .text("F2", 7.0, 15.0, 28.0, "john.doe@email.com")
        .text("F2", 7.0, 15.0, 18.0, "+1 (555) 123-4567")
        .text("F2", 7.0, 15.0, 8.0, "www.johndoe.dev")
        .restore_state()
        // Company logo area (placeholder)
        .save_state()
        .fill_color(Color::WHITE)
        .rect(180.0, 90.0, 55.0, 35.0)
        .fill()
        .restore_state()
        // Logo text
        .save_state()
        .fill_color(Color::rgb(0.1, 0.2, 0.4))
        .text("F1", 8.0, 190.0, 105.0, "TECH")
        .text("F1", 8.0, 190.0, 95.0, "CORP")
        .restore_state();

    let mut page = PageBuilder::custom(252.0, 144.0).content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));

    let doc = DocumentBuilder::new()
        .title("Business Card")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "business_card.pdf");
}

#[test]
fn test_certificate() {
    let content = ContentBuilder::new()
        // Border
        .save_state()
        .stroke_color(Color::rgb(0.6, 0.5, 0.0))
        .line_width(3.0)
        .rect(30.0, 30.0, 535.0, 782.0)
        .stroke()
        .restore_state()
        .save_state()
        .stroke_color(Color::rgb(0.8, 0.7, 0.2))
        .line_width(1.0)
        .rect(40.0, 40.0, 515.0, 762.0)
        .stroke()
        .restore_state()
        // Header decoration
        .save_state()
        .fill_color(Color::rgb(0.8, 0.7, 0.2))
        .rect(200.0, 750.0, 195.0, 3.0)
        .fill()
        .restore_state()
        // Title
        .text("F1", 36.0, 140.0, 700.0, "Certificate")
        .text("F2", 18.0, 195.0, 660.0, "of Achievement")
        // Decorative line
        .save_state()
        .stroke_color(Color::rgb(0.6, 0.5, 0.0))
        .line_width(0.5)
        .move_to(150.0, 640.0)
        .line_to(445.0, 640.0)
        .stroke()
        .restore_state()
        // Presented to
        .text("F3", 14.0, 220.0, 600.0, "This certificate is presented to")
        // Recipient name
        .text("F1", 28.0, 180.0, 540.0, "Jane Smith")
        // Underline for name
        .save_state()
        .stroke_color(Color::rgb(0.6, 0.5, 0.0))
        .line_width(1.0)
        .move_to(150.0, 530.0)
        .line_to(445.0, 530.0)
        .stroke()
        .restore_state()
        // Achievement description
        .text("F3", 12.0, 125.0, 480.0, "in recognition of outstanding performance and dedication")
        .text("F3", 12.0, 165.0, 460.0, "in the field of Software Development")
        // Date
        .text("F3", 11.0, 235.0, 380.0, "Awarded on January 20, 2024")
        // Signatures
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(0.5)
        .move_to(100.0, 280.0)
        .line_to(250.0, 280.0)
        .stroke()
        .move_to(350.0, 280.0)
        .line_to(500.0, 280.0)
        .stroke()
        .restore_state()
        .text("F3", 10.0, 135.0, 260.0, "Program Director")
        .text("F3", 10.0, 395.0, 260.0, "CEO")
        // Footer
        .text("F3", 9.0, 210.0, 100.0, "Certificate ID: CERT-2024-0042");

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::TimesBold));
    page.add_font("F2", Font::from(Standard14Font::TimesItalic));
    page.add_font("F3", Font::from(Standard14Font::TimesRoman));

    let doc = DocumentBuilder::new()
        .title("Certificate of Achievement")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "certificate.pdf");
}

#[test]
fn test_letter_template() {
    let content = ContentBuilder::new()
        // Letterhead
        .text("F1", 18.0, 72.0, 780.0, "COMPANY NAME")
        .text("F2", 9.0, 72.0, 762.0, "123 Corporate Drive, Suite 100 | City, State 12345")
        .text("F2", 9.0, 72.0, 750.0, "Phone: (555) 123-4567 | Email: info@company.com | www.company.com")
        // Horizontal line
        .save_state()
        .stroke_color(Color::rgb(0.0, 0.3, 0.6))
        .line_width(2.0)
        .move_to(72.0, 740.0)
        .line_to(540.0, 740.0)
        .stroke()
        .restore_state()
        // Date
        .text("F2", 11.0, 72.0, 700.0, "January 20, 2024")
        // Recipient
        .text("F2", 11.0, 72.0, 660.0, "Mr. John Smith")
        .text("F2", 11.0, 72.0, 646.0, "ABC Organization")
        .text("F2", 11.0, 72.0, 632.0, "456 Client Avenue")
        .text("F2", 11.0, 72.0, 618.0, "Town, State 67890")
        // Salutation
        .text("F2", 11.0, 72.0, 580.0, "Dear Mr. Smith,")
        // Body paragraphs
        .text_block(
            TextBuilder::new()
                .font("F2", 11.0)
                .position(72.0, 545.0)
                .leading(16.0)
                .show("Thank you for your recent inquiry regarding our services. We are pleased to")
                .next_line()
                .show("provide you with the information you requested and look forward to the")
                .next_line()
                .show("opportunity to work with your organization.")
        )
        .text_block(
            TextBuilder::new()
                .font("F2", 11.0)
                .position(72.0, 475.0)
                .leading(16.0)
                .show("Our team has reviewed your requirements and we believe we can offer a")
                .next_line()
                .show("comprehensive solution that meets your needs. Please find enclosed our")
                .next_line()
                .show("detailed proposal outlining our recommended approach and pricing.")
        )
        .text_block(
            TextBuilder::new()
                .font("F2", 11.0)
                .position(72.0, 405.0)
                .leading(16.0)
                .show("Should you have any questions or require additional information, please do")
                .next_line()
                .show("not hesitate to contact me directly. We value your business and look forward")
                .next_line()
                .show("to your response.")
        )
        // Closing
        .text("F2", 11.0, 72.0, 320.0, "Sincerely,")
        // Signature area
        .text("F3", 14.0, 72.0, 270.0, "Sarah Johnson")
        .text("F2", 11.0, 72.0, 252.0, "Sarah Johnson")
        .text("F2", 10.0, 72.0, 238.0, "Director of Business Development")
        .text("F2", 10.0, 72.0, 224.0, "sarah.johnson@company.com")
        // Footer
        .save_state()
        .stroke_color(Color::gray(0.7))
        .line_width(0.5)
        .move_to(72.0, 100.0)
        .line_to(540.0, 100.0)
        .stroke()
        .restore_state()
        .text("F2", 8.0, 220.0, 85.0, "Confidential - For Intended Recipient Only");

    let mut page = PageBuilder::letter().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));
    page.add_font("F3", Font::from(Standard14Font::HelveticaOblique));

    let doc = DocumentBuilder::new()
        .title("Business Letter")
        .author("Company Name")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "letter_template.pdf");
}

#[test]
fn test_report_cover() {
    let content = ContentBuilder::new()
        // Top accent bar
        .save_state()
        .fill_color(Color::rgb(0.0, 0.3, 0.6))
        .rect(0.0, 780.0, 595.0, 62.0)
        .fill()
        .restore_state()
        // Company name in header (white text)
        .save_state()
        .fill_color(Color::WHITE)
        .text("F1", 12.0, 72.0, 800.0, "COMPANY NAME")
        .restore_state()
        // Main title
        .text("F1", 32.0, 72.0, 550.0, "Annual Report")
        .text("F1", 32.0, 72.0, 510.0, "2024")
        // Subtitle
        .text("F2", 16.0, 72.0, 460.0, "Financial Performance & Strategic Overview")
        // Decorative line
        .save_state()
        .fill_color(Color::rgb(0.0, 0.3, 0.6))
        .rect(72.0, 440.0, 200.0, 4.0)
        .fill()
        .restore_state()
        // Description
        .text_block(
            TextBuilder::new()
                .font("F3", 11.0)
                .position(72.0, 400.0)
                .leading(16.0)
                .show("This report presents a comprehensive overview of our")
                .next_line()
                .show("company's performance, achievements, and strategic")
                .next_line()
                .show("initiatives for the fiscal year 2024.")
        )
        // Key metrics boxes
        .save_state()
        .fill_color(Color::gray(0.95))
        .rect(72.0, 250.0, 140.0, 80.0)
        .fill()
        .rect(227.0, 250.0, 140.0, 80.0)
        .fill()
        .rect(382.0, 250.0, 140.0, 80.0)
        .fill()
        .restore_state()
        .text("F1", 24.0, 100.0, 305.0, "$12.5M")
        .text("F3", 9.0, 105.0, 265.0, "Revenue")
        .text("F1", 24.0, 260.0, 305.0, "25%")
        .text("F3", 9.0, 267.0, 265.0, "Growth")
        .text("F1", 24.0, 410.0, 305.0, "500+")
        .text("F3", 9.0, 415.0, 265.0, "Employees")
        // Bottom info
        .text("F3", 10.0, 72.0, 120.0, "Prepared by: Finance Department")
        .text("F3", 10.0, 72.0, 105.0, "Date: January 2024")
        .text("F3", 10.0, 72.0, 90.0, "Classification: Internal Use Only")
        // Footer bar
        .save_state()
        .fill_color(Color::rgb(0.0, 0.3, 0.6))
        .rect(0.0, 0.0, 595.0, 40.0)
        .fill()
        .fill_color(Color::WHITE)
        .text("F3", 9.0, 220.0, 18.0, "www.company.com")
        .restore_state();

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));
    page.add_font("F3", Font::from(Standard14Font::Helvetica));

    let doc = DocumentBuilder::new()
        .title("Annual Report 2024")
        .author("Company Name")
        .subject("Annual Financial Report")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "report_cover.pdf");
}

// ============================================================================
// ADVANCED GRAPHICS
// ============================================================================

#[test]
fn test_chart_bar() {
    let data = [
        ("Q1", 65.0, Color::rgb(0.2, 0.4, 0.8)),
        ("Q2", 85.0, Color::rgb(0.3, 0.5, 0.9)),
        ("Q3", 75.0, Color::rgb(0.4, 0.6, 0.9)),
        ("Q4", 95.0, Color::rgb(0.5, 0.7, 1.0)),
    ];

    let mut content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 780.0, "Bar Chart Demo")
        .text("F2", 12.0, 72.0, 750.0, "Quarterly Sales Performance");

    // Y-axis
    content = content
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.0)
        .move_to(100.0, 300.0)
        .line_to(100.0, 700.0)
        .stroke()
        .restore_state();

    // X-axis
    content = content
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.0)
        .move_to(100.0, 300.0)
        .line_to(500.0, 300.0)
        .stroke()
        .restore_state();

    // Grid lines
    for i in 0..5 {
        let y = 300.0 + (i as f64 * 100.0);
        content = content
            .save_state()
            .stroke_color(Color::gray(0.8))
            .line_width(0.5)
            .move_to(100.0, y)
            .line_to(500.0, y)
            .stroke()
            .restore_state()
            .text("F3", 9.0, 70.0, y - 4.0, &format!("{}", i * 25));
    }

    // Bars
    for (i, (label, value, color)) in data.iter().enumerate() {
        let x = 130.0 + (i as f64 * 90.0);
        let bar_height = *value * 4.0;
        content = content
            .save_state()
            .fill_color(color.clone())
            .rect(x, 300.0, 60.0, bar_height)
            .fill()
            .restore_state()
            .text("F3", 10.0, x + 18.0, 285.0, label)
            .text("F3", 9.0, x + 15.0, 305.0 + bar_height, &format!("{:.0}%", value));
    }

    // Title for Y-axis
    content = content.text("F3", 10.0, 40.0, 500.0, "Sales %");

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));
    page.add_font("F3", Font::from(Standard14Font::Helvetica));

    let doc = DocumentBuilder::new()
        .title("Bar Chart")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "chart_bar.pdf");
}

#[test]
fn test_chart_line() {
    let data_points = [
        (0, 40.0),
        (1, 55.0),
        (2, 45.0),
        (3, 70.0),
        (4, 65.0),
        (5, 80.0),
        (6, 75.0),
        (7, 90.0),
        (8, 85.0),
        (9, 95.0),
    ];

    let mut content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 780.0, "Line Chart Demo")
        .text("F2", 12.0, 72.0, 750.0, "Monthly Growth Trend");

    // Axes
    content = content
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.0)
        .move_to(100.0, 300.0)
        .line_to(100.0, 700.0)
        .stroke()
        .move_to(100.0, 300.0)
        .line_to(520.0, 300.0)
        .stroke()
        .restore_state();

    // Grid lines
    for i in 0..5 {
        let y = 300.0 + (i as f64 * 100.0);
        content = content
            .save_state()
            .stroke_color(Color::gray(0.85))
            .line_width(0.5)
            .move_to(100.0, y)
            .line_to(520.0, y)
            .stroke()
            .restore_state()
            .text("F3", 9.0, 70.0, y - 4.0, &format!("{}", i * 25));
    }

    // X-axis labels
    for i in 0..10 {
        let x = 120.0 + (i as f64 * 40.0);
        content = content.text("F3", 8.0, x - 8.0, 285.0, &format!("M{}", i + 1));
    }

    // Draw line
    content = content
        .save_state()
        .stroke_color(Color::rgb(0.2, 0.4, 0.8))
        .line_width(2.0);

    let (first_i, first_val) = data_points[0];
    let first_x = 120.0 + (first_i as f64 * 40.0);
    let first_y = 300.0 + (first_val * 4.0);
    content = content.move_to(first_x, first_y);

    for (i, val) in data_points.iter().skip(1) {
        let x = 120.0 + (*i as f64 * 40.0);
        let y = 300.0 + (*val * 4.0);
        content = content.line_to(x, y);
    }
    content = content.stroke().restore_state();

    // Draw points
    for (i, val) in data_points.iter() {
        let x = 120.0 + (*i as f64 * 40.0);
        let y = 300.0 + (*val * 4.0);
        content = content
            .save_state()
            .fill_color(Color::rgb(0.2, 0.4, 0.8));

        // Draw a small filled circle (approximated with a square for simplicity)
        let graphics = GraphicsBuilder::new()
            .fill_color(Color::rgb(0.2, 0.4, 0.8))
            .filled_circle(x, y, 4.0);
        content = content.graphics(graphics).restore_state();
    }

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));
    page.add_font("F3", Font::from(Standard14Font::Helvetica));

    let doc = DocumentBuilder::new()
        .title("Line Chart")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "chart_line.pdf");
}

#[test]
fn test_pie_chart_segments() {
    // Simplified pie chart using segments
    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 780.0, "Pie Chart Demo")
        .text("F2", 12.0, 72.0, 750.0, "Market Share Distribution")
        // Pie segments (approximated with filled shapes)
        // Segment 1: 40% (red)
        .save_state()
        .fill_color(Color::rgb(0.9, 0.2, 0.2))
        .move_to(300.0, 500.0)
        .line_to(300.0, 620.0)
        .curve_to(360.0, 620.0, 410.0, 580.0, 420.0, 530.0)
        .line_to(300.0, 500.0)
        .fill()
        .restore_state()
        // Segment 2: 30% (blue)
        .save_state()
        .fill_color(Color::rgb(0.2, 0.4, 0.8))
        .move_to(300.0, 500.0)
        .line_to(420.0, 530.0)
        .curve_to(420.0, 470.0, 390.0, 410.0, 340.0, 390.0)
        .line_to(300.0, 500.0)
        .fill()
        .restore_state()
        // Segment 3: 20% (green)
        .save_state()
        .fill_color(Color::rgb(0.2, 0.7, 0.3))
        .move_to(300.0, 500.0)
        .line_to(340.0, 390.0)
        .curve_to(280.0, 370.0, 220.0, 400.0, 190.0, 450.0)
        .line_to(300.0, 500.0)
        .fill()
        .restore_state()
        // Segment 4: 10% (yellow)
        .save_state()
        .fill_color(Color::rgb(0.95, 0.8, 0.2))
        .move_to(300.0, 500.0)
        .line_to(190.0, 450.0)
        .curve_to(180.0, 490.0, 180.0, 540.0, 200.0, 580.0)
        .line_to(300.0, 500.0)
        .fill()
        .restore_state()
        // Remaining segment (purple)
        .save_state()
        .fill_color(Color::rgb(0.6, 0.3, 0.7))
        .move_to(300.0, 500.0)
        .line_to(200.0, 580.0)
        .curve_to(230.0, 610.0, 270.0, 620.0, 300.0, 620.0)
        .line_to(300.0, 500.0)
        .fill()
        .restore_state()
        // Legend
        .text("F2", 12.0, 72.0, 320.0, "Legend:")
        .save_state()
        .fill_color(Color::rgb(0.9, 0.2, 0.2))
        .rect(72.0, 290.0, 15.0, 15.0)
        .fill()
        .restore_state()
        .text("F3", 10.0, 92.0, 293.0, "Product A (40%)")
        .save_state()
        .fill_color(Color::rgb(0.2, 0.4, 0.8))
        .rect(72.0, 270.0, 15.0, 15.0)
        .fill()
        .restore_state()
        .text("F3", 10.0, 92.0, 273.0, "Product B (30%)")
        .save_state()
        .fill_color(Color::rgb(0.2, 0.7, 0.3))
        .rect(72.0, 250.0, 15.0, 15.0)
        .fill()
        .restore_state()
        .text("F3", 10.0, 92.0, 253.0, "Product C (20%)")
        .save_state()
        .fill_color(Color::rgb(0.95, 0.8, 0.2))
        .rect(72.0, 230.0, 15.0, 15.0)
        .fill()
        .restore_state()
        .text("F3", 10.0, 92.0, 233.0, "Product D (7%)")
        .save_state()
        .fill_color(Color::rgb(0.6, 0.3, 0.7))
        .rect(72.0, 210.0, 15.0, 15.0)
        .fill()
        .restore_state()
        .text("F3", 10.0, 92.0, 213.0, "Other (3%)");

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));
    page.add_font("F3", Font::from(Standard14Font::Helvetica));

    let doc = DocumentBuilder::new()
        .title("Pie Chart")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "chart_pie.pdf");
}

#[test]
fn test_flowchart() {
    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 780.0, "Flowchart Demo")
        // Start (oval)
        .save_state()
        .fill_color(Color::rgb(0.8, 0.9, 0.8))
        .stroke_color(Color::rgb(0.2, 0.5, 0.2))
        .line_width(2.0)
        .move_to(340.0, 720.0)
        .curve_to(340.0, 740.0, 260.0, 740.0, 260.0, 720.0)
        .curve_to(260.0, 700.0, 340.0, 700.0, 340.0, 720.0)
        .fill_and_stroke()
        .restore_state()
        .text("F2", 10.0, 285.0, 716.0, "Start")
        // Arrow down
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.5)
        .move_to(300.0, 700.0)
        .line_to(300.0, 665.0)
        .stroke()
        .fill_color(Color::BLACK)
        .move_to(300.0, 660.0)
        .line_to(295.0, 670.0)
        .line_to(305.0, 670.0)
        .close_path()
        .fill()
        .restore_state()
        // Process box 1
        .save_state()
        .fill_color(Color::rgb(0.85, 0.85, 1.0))
        .stroke_color(Color::rgb(0.3, 0.3, 0.7))
        .line_width(2.0)
        .rect(230.0, 600.0, 140.0, 55.0)
        .fill_and_stroke()
        .restore_state()
        .text("F2", 10.0, 258.0, 622.0, "Process Data")
        // Arrow down
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.5)
        .move_to(300.0, 600.0)
        .line_to(300.0, 565.0)
        .stroke()
        .fill_color(Color::BLACK)
        .move_to(300.0, 560.0)
        .line_to(295.0, 570.0)
        .line_to(305.0, 570.0)
        .close_path()
        .fill()
        .restore_state()
        // Decision diamond
        .save_state()
        .fill_color(Color::rgb(1.0, 0.95, 0.8))
        .stroke_color(Color::rgb(0.7, 0.5, 0.0))
        .line_width(2.0)
        .move_to(300.0, 555.0)
        .line_to(370.0, 505.0)
        .line_to(300.0, 455.0)
        .line_to(230.0, 505.0)
        .close_path()
        .fill_and_stroke()
        .restore_state()
        .text("F2", 10.0, 270.0, 501.0, "Valid?")
        // Yes arrow (down)
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.5)
        .move_to(300.0, 455.0)
        .line_to(300.0, 420.0)
        .stroke()
        .fill_color(Color::BLACK)
        .move_to(300.0, 415.0)
        .line_to(295.0, 425.0)
        .line_to(305.0, 425.0)
        .close_path()
        .fill()
        .restore_state()
        .text("F3", 9.0, 305.0, 435.0, "Yes")
        // No arrow (right)
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.5)
        .move_to(370.0, 505.0)
        .line_to(450.0, 505.0)
        .line_to(450.0, 627.0)
        .line_to(375.0, 627.0)
        .stroke()
        .fill_color(Color::BLACK)
        .move_to(370.0, 627.0)
        .line_to(380.0, 622.0)
        .line_to(380.0, 632.0)
        .close_path()
        .fill()
        .restore_state()
        .text("F3", 9.0, 380.0, 510.0, "No")
        // Process box 2
        .save_state()
        .fill_color(Color::rgb(0.85, 0.85, 1.0))
        .stroke_color(Color::rgb(0.3, 0.3, 0.7))
        .line_width(2.0)
        .rect(230.0, 355.0, 140.0, 55.0)
        .fill_and_stroke()
        .restore_state()
        .text("F2", 10.0, 258.0, 377.0, "Save Result")
        // Arrow down
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.5)
        .move_to(300.0, 355.0)
        .line_to(300.0, 320.0)
        .stroke()
        .fill_color(Color::BLACK)
        .move_to(300.0, 315.0)
        .line_to(295.0, 325.0)
        .line_to(305.0, 325.0)
        .close_path()
        .fill()
        .restore_state()
        // End (oval)
        .save_state()
        .fill_color(Color::rgb(1.0, 0.85, 0.85))
        .stroke_color(Color::rgb(0.7, 0.2, 0.2))
        .line_width(2.0)
        .move_to(340.0, 290.0)
        .curve_to(340.0, 310.0, 260.0, 310.0, 260.0, 290.0)
        .curve_to(260.0, 270.0, 340.0, 270.0, 340.0, 290.0)
        .fill_and_stroke()
        .restore_state()
        .text("F2", 10.0, 290.0, 286.0, "End");

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));
    page.add_font("F3", Font::from(Standard14Font::Helvetica));

    let doc = DocumentBuilder::new()
        .title("Flowchart")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "flowchart.pdf");
}

#[test]
fn test_table_layout() {
    let mut content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 780.0, "Table Layout Demo");

    // Table header
    let header_y = 720.0;
    content = content
        .save_state()
        .fill_color(Color::rgb(0.2, 0.3, 0.5))
        .rect(72.0, header_y, 468.0, 25.0)
        .fill()
        .restore_state();

    // Header text (white)
    content = content
        .save_state()
        .fill_color(Color::WHITE)
        .text("F1", 10.0, 80.0, header_y + 8.0, "ID")
        .text("F1", 10.0, 130.0, header_y + 8.0, "Name")
        .text("F1", 10.0, 280.0, header_y + 8.0, "Department")
        .text("F1", 10.0, 420.0, header_y + 8.0, "Salary")
        .restore_state();

    // Table rows
    let rows = [
        ("001", "John Smith", "Engineering", "$85,000"),
        ("002", "Jane Doe", "Marketing", "$72,000"),
        ("003", "Bob Johnson", "Sales", "$68,000"),
        ("004", "Alice Brown", "Engineering", "$92,000"),
        ("005", "Charlie Wilson", "HR", "$65,000"),
        ("006", "Diana Lee", "Finance", "$78,000"),
        ("007", "Edward Chen", "Engineering", "$88,000"),
        ("008", "Fiona Garcia", "Marketing", "$71,000"),
    ];

    for (i, (id, name, dept, salary)) in rows.iter().enumerate() {
        let y = header_y - 25.0 - (i as f64 * 25.0);

        // Alternating row colors
        if i % 2 == 0 {
            content = content
                .save_state()
                .fill_color(Color::gray(0.95))
                .rect(72.0, y, 468.0, 25.0)
                .fill()
                .restore_state();
        }

        // Row borders
        content = content
            .save_state()
            .stroke_color(Color::gray(0.8))
            .line_width(0.5)
            .move_to(72.0, y)
            .line_to(540.0, y)
            .stroke()
            .restore_state();

        // Cell content
        content = content
            .text("F2", 9.0, 80.0, y + 8.0, id)
            .text("F2", 9.0, 130.0, y + 8.0, name)
            .text("F2", 9.0, 280.0, y + 8.0, dept)
            .text("F2", 9.0, 420.0, y + 8.0, salary);
    }

    // Table border
    let table_height = 25.0 + (rows.len() as f64 * 25.0);
    content = content
        .save_state()
        .stroke_color(Color::rgb(0.2, 0.3, 0.5))
        .line_width(1.5)
        .rect(72.0, header_y - table_height + 25.0, 468.0, table_height)
        .stroke()
        .restore_state();

    // Column separators
    let col_positions = [120.0, 270.0, 410.0];
    for x in col_positions.iter() {
        content = content
            .save_state()
            .stroke_color(Color::gray(0.7))
            .line_width(0.5)
            .move_to(*x, header_y + 25.0)
            .line_to(*x, header_y - table_height + 25.0)
            .stroke()
            .restore_state();
    }

    // Footer
    content = content.text("F3", 9.0, 72.0, header_y - table_height, &format!("Total records: {}", rows.len()));

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));
    page.add_font("F3", Font::from(Standard14Font::HelveticaOblique));

    let doc = DocumentBuilder::new()
        .title("Table Layout")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "table_layout.pdf");
}

// ============================================================================
// MULTI-PAGE EXAMPLES
// ============================================================================

#[test]
fn test_multipage_report() {
    let mut pages = Vec::new();

    // Page 1: Cover
    let cover_content = ContentBuilder::new()
        .save_state()
        .fill_color(Color::rgb(0.1, 0.2, 0.4))
        .rect(0.0, 600.0, 595.0, 242.0)
        .fill()
        // Title (white)
        .fill_color(Color::WHITE)
        .text("F1", 36.0, 72.0, 720.0, "Technical Report")
        // Subtitle (light blue)
        .fill_color(Color::rgb(0.8, 0.8, 0.9))
        .text("F2", 18.0, 72.0, 680.0, "System Architecture Overview")
        .restore_state()
        .text("F2", 14.0, 72.0, 400.0, "Version 1.0")
        .text("F2", 14.0, 72.0, 380.0, "January 2024")
        .text("F2", 12.0, 72.0, 320.0, "Prepared by: Engineering Team")
        .text("F2", 12.0, 72.0, 300.0, "Classification: Internal");

    let mut page1 = PageBuilder::a4().content(cover_content).build();
    page1.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page1.add_font("F2", Font::from(Standard14Font::Helvetica));
    pages.push(page1);

    // Page 2: Table of Contents
    let toc_content = ContentBuilder::new()
        .text("F1", 24.0, 72.0, 780.0, "Table of Contents")
        .save_state()
        .stroke_color(Color::gray(0.5))
        .line_width(1.0)
        .move_to(72.0, 765.0)
        .line_to(250.0, 765.0)
        .stroke()
        .restore_state()
        .text("F2", 12.0, 72.0, 720.0, "1. Introduction")
        .text("F2", 12.0, 500.0, 720.0, "3")
        .text("F2", 12.0, 72.0, 700.0, "2. System Overview")
        .text("F2", 12.0, 500.0, 700.0, "4")
        .text("F2", 12.0, 72.0, 680.0, "3. Architecture Design")
        .text("F2", 12.0, 500.0, 680.0, "5")
        .text("F2", 12.0, 72.0, 660.0, "4. Implementation Details")
        .text("F2", 12.0, 500.0, 660.0, "6")
        .text("F2", 12.0, 72.0, 640.0, "5. Conclusion")
        .text("F2", 12.0, 500.0, 640.0, "7")
        // Dotted lines
        .save_state()
        .stroke_color(Color::gray(0.6))
        .dash(vec![1.0, 3.0], 0.0)
        .line_width(0.5)
        .move_to(160.0, 723.0)
        .line_to(495.0, 723.0)
        .stroke()
        .move_to(195.0, 703.0)
        .line_to(495.0, 703.0)
        .stroke()
        .move_to(210.0, 683.0)
        .line_to(495.0, 683.0)
        .stroke()
        .move_to(235.0, 663.0)
        .line_to(495.0, 663.0)
        .stroke()
        .move_to(165.0, 643.0)
        .line_to(495.0, 643.0)
        .stroke()
        .restore_state()
        // Page number
        .text("F2", 10.0, 290.0, 50.0, "Page 2");

    let mut page2 = PageBuilder::a4().content(toc_content).build();
    page2.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page2.add_font("F2", Font::from(Standard14Font::Helvetica));
    pages.push(page2);

    // Pages 3-7: Content pages
    let sections = [
        ("1. Introduction", "This document provides an overview of the system architecture and design decisions made during the development phase. The following sections detail the key components and their interactions."),
        ("2. System Overview", "The system is designed as a microservices architecture with several key components including the API Gateway, Authentication Service, and Data Processing Pipeline. Each component is independently deployable and scalable."),
        ("3. Architecture Design", "The architecture follows a layered approach with clear separation of concerns. The presentation layer handles user interactions, the business logic layer processes requests, and the data access layer manages persistence."),
        ("4. Implementation Details", "The implementation uses modern technologies including containerization with Docker, orchestration with Kubernetes, and continuous integration/deployment pipelines. Code quality is maintained through automated testing."),
        ("5. Conclusion", "The proposed architecture provides a solid foundation for future growth and scalability. Regular reviews and updates will ensure the system continues to meet evolving requirements."),
    ];

    for (i, (title, body)) in sections.iter().enumerate() {
        let page_num = i + 3;
        let content = ContentBuilder::new()
            // Header with white text
            .save_state()
            .fill_color(Color::rgb(0.1, 0.2, 0.4))
            .rect(0.0, 800.0, 595.0, 42.0)
            .fill()
            .fill_color(Color::WHITE)
            .text("F3", 10.0, 72.0, 815.0, "Technical Report - System Architecture Overview")
            .restore_state()
            // Title
            .text("F1", 20.0, 72.0, 750.0, *title)
            // Body
            .text_block(
                TextBuilder::new()
                    .font("F2", 11.0)
                    .position(72.0, 710.0)
                    .leading(18.0)
                    .show(*body)
            )
            // Lorem ipsum filler
            .text_block(
                TextBuilder::new()
                    .font("F2", 11.0)
                    .position(72.0, 650.0)
                    .leading(16.0)
                    .show("Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod")
                    .next_line()
                    .show("tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim")
                    .next_line()
                    .show("veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea")
                    .next_line()
                    .show("commodo consequat. Duis aute irure dolor in reprehenderit in voluptate")
                    .next_line()
                    .show("velit esse cillum dolore eu fugiat nulla pariatur.")
            )
            // Footer
            .save_state()
            .stroke_color(Color::gray(0.5))
            .line_width(0.5)
            .move_to(72.0, 70.0)
            .line_to(523.0, 70.0)
            .stroke()
            .restore_state()
            .text("F2", 10.0, 280.0, 50.0, &format!("Page {}", page_num));

        let mut page = PageBuilder::a4().content(content).build();
        page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
        page.add_font("F2", Font::from(Standard14Font::Helvetica));
        page.add_font("F3", Font::from(Standard14Font::Helvetica));
        pages.push(page);
    }

    let doc = DocumentBuilder::new()
        .title("Technical Report")
        .author("Engineering Team")
        .subject("System Architecture Overview")
        .pages(pages)
        .build()
        .unwrap();

    assert_eq!(doc.page_count(), 7);
    save_and_verify(doc, "multipage_report.pdf");
}

#[test]
fn test_catalog_pages() {
    let products = [
        ("Product A", "High-quality widget with premium features", "$99.99"),
        ("Product B", "Economy option for budget-conscious buyers", "$49.99"),
        ("Product C", "Professional grade tool for experts", "$199.99"),
        ("Product D", "Compact and portable design", "$79.99"),
        ("Product E", "Deluxe edition with extras", "$149.99"),
        ("Product F", "Entry-level option for beginners", "$29.99"),
    ];

    let mut pages = Vec::new();

    // 2 products per page
    for (page_idx, chunk) in products.chunks(2).enumerate() {
        let mut content = ContentBuilder::new()
            // Header with white text
            .save_state()
            .fill_color(Color::rgb(0.9, 0.1, 0.1))
            .rect(0.0, 800.0, 595.0, 42.0)
            .fill()
            .fill_color(Color::WHITE)
            .text("F1", 14.0, 72.0, 815.0, "Product Catalog 2024")
            .restore_state();

        for (i, (name, desc, price)) in chunk.iter().enumerate() {
            let y_offset = if i == 0 { 700.0 } else { 400.0 };

            // Product box
            content = content
                .save_state()
                .stroke_color(Color::gray(0.7))
                .line_width(1.0)
                .rect(72.0, y_offset - 150.0, 451.0, 200.0)
                .stroke()
                .restore_state();

            // Image placeholder
            content = content
                .save_state()
                .fill_color(Color::gray(0.9))
                .rect(82.0, y_offset - 140.0, 150.0, 150.0)
                .fill()
                .restore_state()
                .text("F3", 10.0, 130.0, y_offset - 70.0, "[Image]");

            // Product info
            content = content
                .text("F1", 16.0, 250.0, y_offset + 30.0, name)
                .text("F2", 11.0, 250.0, y_offset, desc)
                .text("F1", 18.0, 250.0, y_offset - 80.0, price)
                .text("F2", 10.0, 250.0, y_offset - 110.0, "Free shipping on orders over $100");
        }

        // Page number
        content = content.text("F2", 10.0, 280.0, 50.0, &format!("Page {}", page_idx + 1));

        let mut page = PageBuilder::a4().content(content).build();
        page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
        page.add_font("F2", Font::from(Standard14Font::Helvetica));
        page.add_font("F3", Font::from(Standard14Font::HelveticaOblique));
        pages.push(page);
    }

    let doc = DocumentBuilder::new()
        .title("Product Catalog 2024")
        .pages(pages)
        .build()
        .unwrap();

    save_and_verify(doc, "catalog_pages.pdf");
}

// ============================================================================
// MISCELLANEOUS EXAMPLES
// ============================================================================

#[test]
fn test_watermark_effect() {
    let content = ContentBuilder::new()
        // Background watermark (light gray)
        .save_state()
        .fill_color(Color::gray(0.85))
        .text("F1", 60.0, 80.0, 400.0, "CONFIDENTIAL")
        .restore_state()
        // Main content on top
        .text("F1", 18.0, 72.0, 780.0, "Document with Watermark")
        .text_block(
            TextBuilder::new()
                .font("F2", 11.0)
                .position(72.0, 740.0)
                .leading(16.0)
                .show("This document contains confidential information. The watermark effect")
                .next_line()
                .show("is achieved by layering a light gray text behind the main content.")
                .next_line()
                .show("In a production system, you would typically rotate the watermark text")
                .next_line()
                .show("diagonally across the page for better visibility.")
        )
        .text_block(
            TextBuilder::new()
                .font("F2", 11.0)
                .position(72.0, 650.0)
                .leading(16.0)
                .show("Lorem ipsum dolor sit amet, consectetur adipiscing elit. Nullam")
                .next_line()
                .show("auctor, nisl nec tincidunt lacinia, nunc nisl aliquam massa, nec")
                .next_line()
                .show("lacinia nunc nisl nec nunc. Sed euismod, nisl nec tincidunt lacinia.")
        );

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));

    let doc = DocumentBuilder::new()
        .title("Confidential Document")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "watermark.pdf");
}

#[test]
fn test_decorative_borders() {
    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 780.0, "Decorative Borders Demo")
        // Simple border
        .text("F2", 12.0, 72.0, 720.0, "Simple Border:")
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(2.0)
        .rect(72.0, 620.0, 200.0, 80.0)
        .stroke()
        .restore_state()
        // Double border
        .text("F2", 12.0, 300.0, 720.0, "Double Border:")
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(2.0)
        .rect(300.0, 620.0, 200.0, 80.0)
        .stroke()
        .line_width(1.0)
        .rect(305.0, 625.0, 190.0, 70.0)
        .stroke()
        .restore_state()
        // Rounded corners (approximated)
        .text("F2", 12.0, 72.0, 580.0, "Rounded Corners:")
        .save_state()
        .stroke_color(Color::rgb(0.2, 0.4, 0.8))
        .line_width(2.0)
        // Top
        .move_to(92.0, 560.0)
        .line_to(252.0, 560.0)
        // Top-right corner
        .curve_to(262.0, 560.0, 272.0, 550.0, 272.0, 540.0)
        // Right
        .line_to(272.0, 500.0)
        // Bottom-right corner
        .curve_to(272.0, 490.0, 262.0, 480.0, 252.0, 480.0)
        // Bottom
        .line_to(92.0, 480.0)
        // Bottom-left corner
        .curve_to(82.0, 480.0, 72.0, 490.0, 72.0, 500.0)
        // Left
        .line_to(72.0, 540.0)
        // Top-left corner
        .curve_to(72.0, 550.0, 82.0, 560.0, 92.0, 560.0)
        .stroke()
        .restore_state()
        // Dashed border
        .text("F2", 12.0, 300.0, 580.0, "Dashed Border:")
        .save_state()
        .stroke_color(Color::rgb(0.8, 0.2, 0.2))
        .line_width(2.0)
        .dash(vec![8.0, 4.0], 0.0)
        .rect(300.0, 480.0, 200.0, 80.0)
        .stroke()
        .restore_state()
        // Decorative corner border
        .text("F2", 12.0, 72.0, 440.0, "Corner Accents:")
        .save_state()
        .stroke_color(Color::rgb(0.0, 0.5, 0.0))
        .line_width(3.0)
        // Top-left corner
        .move_to(72.0, 420.0)
        .line_to(72.0, 400.0)
        .move_to(72.0, 420.0)
        .line_to(92.0, 420.0)
        // Top-right corner
        .move_to(272.0, 420.0)
        .line_to(272.0, 400.0)
        .move_to(272.0, 420.0)
        .line_to(252.0, 420.0)
        // Bottom-left corner
        .move_to(72.0, 340.0)
        .line_to(72.0, 360.0)
        .move_to(72.0, 340.0)
        .line_to(92.0, 340.0)
        // Bottom-right corner
        .move_to(272.0, 340.0)
        .line_to(272.0, 360.0)
        .move_to(272.0, 340.0)
        .line_to(252.0, 340.0)
        .stroke()
        .restore_state()
        // Shadow effect
        .text("F2", 12.0, 300.0, 440.0, "Shadow Effect:")
        .save_state()
        .fill_color(Color::gray(0.7))
        .rect(308.0, 332.0, 200.0, 80.0)
        .fill()
        .fill_color(Color::WHITE)
        .rect(300.0, 340.0, 200.0, 80.0)
        .fill()
        .stroke_color(Color::BLACK)
        .line_width(1.0)
        .rect(300.0, 340.0, 200.0, 80.0)
        .stroke()
        .restore_state();

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));

    let doc = DocumentBuilder::new()
        .title("Decorative Borders")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "borders.pdf");
}

#[test]
fn test_icons_and_symbols() {
    let mut page = PageBuilder::a4().build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));
    page.add_font("Symbol", Font::from(Standard14Font::Symbol));
    page.add_font("Dingbats", Font::from(Standard14Font::ZapfDingbats));

    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 780.0, "Symbols and Dingbats Demo")
        // Symbol font samples
        .text("F2", 14.0, 72.0, 740.0, "Symbol Font Characters:")
        .text("Symbol", 16.0, 72.0, 710.0, "abcdefghijklmnopqrstuvwxyz")
        .text("Symbol", 16.0, 72.0, 685.0, "ABCDEFGHIJKLMNOPQRSTUVWXYZ")
        .text("Symbol", 24.0, 72.0, 650.0, "±×÷≤≥≠∞√∂∫∑∏")
        // ZapfDingbats samples
        .text("F2", 14.0, 72.0, 600.0, "ZapfDingbats Characters:")
        .text("Dingbats", 18.0, 72.0, 570.0, "✓✗★☆♠♣♥♦")
        .text("Dingbats", 24.0, 72.0, 530.0, "➀➁➂➃➄➅➆➇➈➉")
        .text("Dingbats", 24.0, 72.0, 490.0, "➊➋➌➍➎➏➐➑➒➓")
        // Arrows from Dingbats
        .text("F2", 14.0, 72.0, 440.0, "Arrow Symbols:")
        .text("Dingbats", 24.0, 72.0, 400.0, "➔➜➝➞➟➠➡➢➣➤")
        // Star ratings
        .text("F2", 14.0, 72.0, 340.0, "Star Ratings Example:")
        .text("F2", 11.0, 72.0, 310.0, "5 stars:")
        .text("Dingbats", 16.0, 130.0, 310.0, "★★★★★")
        .text("F2", 11.0, 72.0, 285.0, "4 stars:")
        .text("Dingbats", 16.0, 130.0, 285.0, "★★★★☆")
        .text("F2", 11.0, 72.0, 260.0, "3 stars:")
        .text("Dingbats", 16.0, 130.0, 260.0, "★★★☆☆");

    page.set_content(content);

    let doc = DocumentBuilder::new()
        .title("Symbols and Dingbats")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "symbols.pdf");
}

#[test]
fn test_measurement_ruler() {
    let mut content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 800.0, "Measurement Ruler Demo")
        .text("F2", 10.0, 72.0, 780.0, "1 inch = 72 points");

    // Horizontal ruler (in points)
    content = content
        .text("F2", 10.0, 72.0, 720.0, "Horizontal Ruler (points):")
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.0)
        .move_to(72.0, 700.0)
        .line_to(522.0, 700.0)
        .stroke()
        .restore_state();

    // Tick marks every 72 points (1 inch)
    for i in 0..7 {
        let x = 72.0 + (i as f64 * 72.0);
        content = content
            .save_state()
            .stroke_color(Color::BLACK)
            .line_width(1.0)
            .move_to(x, 700.0)
            .line_to(x, 685.0)
            .stroke()
            .restore_state()
            .text("F3", 8.0, x - 5.0, 675.0, &format!("{}", i));
    }

    // Half-inch marks
    for i in 0..13 {
        let x = 72.0 + (i as f64 * 36.0);
        if i % 2 != 0 {
            content = content
                .save_state()
                .stroke_color(Color::BLACK)
                .line_width(0.5)
                .move_to(x, 700.0)
                .line_to(x, 690.0)
                .stroke()
                .restore_state();
        }
    }

    // Vertical ruler
    content = content
        .text("F2", 10.0, 72.0, 620.0, "Vertical Ruler:")
        .save_state()
        .stroke_color(Color::BLACK)
        .line_width(1.0)
        .move_to(100.0, 600.0)
        .line_to(100.0, 200.0)
        .stroke()
        .restore_state();

    // Vertical tick marks
    for i in 0..6 {
        let y = 600.0 - (i as f64 * 72.0);
        content = content
            .save_state()
            .stroke_color(Color::BLACK)
            .line_width(1.0)
            .move_to(100.0, y)
            .line_to(115.0, y)
            .stroke()
            .restore_state()
            .text("F3", 8.0, 120.0, y - 4.0, &format!("{} in", i));
    }

    // Grid with measurements
    content = content.text("F2", 10.0, 250.0, 620.0, "1-inch Grid:");

    for row in 0..4 {
        for col in 0..4 {
            let x = 250.0 + (col as f64 * 72.0);
            let y = 550.0 - (row as f64 * 72.0);
            content = content
                .save_state()
                .stroke_color(Color::gray(0.5))
                .line_width(0.5)
                .rect(x, y, 72.0, 72.0)
                .stroke()
                .restore_state();
        }
    }

    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::HelveticaBold));
    page.add_font("F2", Font::from(Standard14Font::Helvetica));
    page.add_font("F3", Font::from(Standard14Font::Helvetica));

    let doc = DocumentBuilder::new()
        .title("Measurement Ruler")
        .page(page)
        .build()
        .unwrap();

    save_and_verify(doc, "ruler.pdf");
}

// ==================== Compression Tests ====================

#[cfg(feature = "compression")]
mod compression_tests {
    use super::*;

    /// Helper to save a compressed document and verify it.
    fn save_compressed_and_verify(doc: Document, name: &str) -> Vec<u8> {
        let path = output_dir().join(name);
        doc.save_to_file(&path).unwrap();

        let bytes = fs::read(&path).unwrap();
        let content = String::from_utf8_lossy(&bytes);

        // Basic PDF structure verification
        assert!(content.starts_with("%PDF-"), "Missing PDF header");
        assert!(content.contains("%%EOF"), "Missing EOF marker");
        assert!(content.contains("/Type /Catalog"), "Missing catalog");
        
        // Verify compression is applied
        assert!(content.contains("/Filter /FlateDecode"), "Missing FlateDecode filter");

        println!("Created compressed: {} ({} bytes)", path.display(), bytes.len());
        bytes
    }

    #[test]
    fn test_compressed_minimal_pdf() {
        let page = PageBuilder::a4().build();

        let doc = DocumentBuilder::new()
            .compress_streams(true)
            .page(page)
            .build()
            .unwrap();

        save_compressed_and_verify(doc, "compressed_minimal.pdf");
    }

    #[test]
    fn test_compressed_text_pdf() {
        let content = ContentBuilder::new()
            .text("F1", 24.0, 72.0, 750.0, "This is a compressed PDF!")
            .text("F1", 12.0, 72.0, 700.0, "The content stream is compressed using FlateDecode.");

        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(content)
            .build();

        let doc = DocumentBuilder::new()
            .title("Compressed PDF Test")
            .compress_streams(true)
            .page(page)
            .build()
            .unwrap();

        save_compressed_and_verify(doc, "compressed_text.pdf");
    }

    #[test]
    fn test_compressed_vs_uncompressed_size() {
        // Create a page with lots of content
        let mut content = ContentBuilder::new();
        for i in 0..50 {
            let y = 750.0 - (i as f64 * 14.0);
            content = content.text("F1", 10.0, 72.0, y, &format!("Line {} - This is test content for compression comparison.", i + 1));
        }

        let page_uncompressed = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(content.clone())
            .build();

        let page_compressed = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(content)
            .build();

        // Uncompressed document
        let doc_uncompressed = DocumentBuilder::new()
            .title("Uncompressed")
            .page(page_uncompressed)
            .build()
            .unwrap();

        // Compressed document
        let doc_compressed = DocumentBuilder::new()
            .title("Compressed")
            .compress_streams(true)
            .page(page_compressed)
            .build()
            .unwrap();

        let uncompressed_path = output_dir().join("size_comparison_uncompressed.pdf");
        let compressed_path = output_dir().join("size_comparison_compressed.pdf");

        doc_uncompressed.save_to_file(&uncompressed_path).unwrap();
        doc_compressed.save_to_file(&compressed_path).unwrap();

        let uncompressed_size = fs::metadata(&uncompressed_path).unwrap().len();
        let compressed_size = fs::metadata(&compressed_path).unwrap().len();

        println!("Uncompressed: {} bytes", uncompressed_size);
        println!("Compressed: {} bytes", compressed_size);
        println!("Compression ratio: {:.2}%", (1.0 - compressed_size as f64 / uncompressed_size as f64) * 100.0);

        // Compressed should be smaller (for content with lots of repetition)
        assert!(compressed_size < uncompressed_size, "Compressed PDF should be smaller");
    }

    #[test]
    fn test_compressed_multi_page_pdf() {
        let mut pages = Vec::new();
        for i in 1..=5 {
            let content = ContentBuilder::new()
                .text("F1", 24.0, 72.0, 750.0, &format!("Compressed Page {}", i))
                .text("F1", 12.0, 72.0, 700.0, "This page uses FlateDecode compression.");

            let page = PageBuilder::a4()
                .font("F1", Standard14Font::Helvetica)
                .content(content)
                .build();
            pages.push(page);
        }

        let doc = DocumentBuilder::new()
            .title("Multi-Page Compressed PDF")
            .compress_streams(true)
            .pages(pages)
            .build()
            .unwrap();

        let bytes = save_compressed_and_verify(doc, "compressed_multi_page.pdf");
        let content = String::from_utf8_lossy(&bytes);
        
        // Count number of FlateDecode filters (should be 5 for 5 pages)
        let filter_count = content.matches("/Filter /FlateDecode").count();
        assert_eq!(filter_count, 5, "Each page should have compressed content");
    }

    #[test]
    fn test_compressed_graphics_pdf() {
        let content = ContentBuilder::new()
            .save_state()
            .fill_color(Color::rgb(0.2, 0.4, 0.8))
            .rect(100.0, 600.0, 200.0, 100.0)
            .fill()
            .restore_state()
            .save_state()
            .stroke_color(Color::RED)
            .line_width(3.0)
            .move_to(50.0, 400.0)
            .line_to(300.0, 400.0)
            .line_to(175.0, 550.0)
            .close_path()
            .stroke()
            .restore_state();

        let page = PageBuilder::a4()
            .content(content)
            .build();

        let doc = DocumentBuilder::new()
            .title("Compressed Graphics")
            .compress_streams(true)
            .page(page)
            .build()
            .unwrap();

        save_compressed_and_verify(doc, "compressed_graphics.pdf");
    }

    #[test]
    fn test_content_builder_build_compressed() {
        let content = ContentBuilder::new()
            .text("F1", 24.0, 72.0, 750.0, "Built with build_compressed()");

        // Test the build_compressed method on ContentBuilder
        let stream = content.build_compressed().unwrap();
        assert!(stream.is_compressed());

        // Verify the stream has the FlateDecode filter
        let dict_str = stream.dictionary_to_pdf_string();
        assert!(dict_str.contains("/Filter /FlateDecode"));

        // Verify the data can be decompressed back
        let original_bytes = ContentBuilder::new()
            .text("F1", 24.0, 72.0, 750.0, "Built with build_compressed()")
            .build_bytes();
        let decompressed = stream.decompress().unwrap();
        assert_eq!(decompressed, original_bytes);
    }

    #[test]
    fn test_stream_compression_roundtrip() {
        use rust_pdf::PdfStream;

        let original_data = "BT /F1 24 Tf 72 750 Td (Test content for compression) Tj ET";
        let stream = PdfStream::from_text(original_data);

        // Compress
        let compressed = stream.with_compression().unwrap();
        assert!(compressed.is_compressed());

        // Decompress and verify
        let decompressed = compressed.decompress().unwrap();
        assert_eq!(String::from_utf8_lossy(&decompressed), original_data);
    }
}

/// Image tests (requires "images" feature)
#[cfg(feature = "images")]
mod image_tests {
    use super::*;
    use rust_pdf::image::{ColorSpace, Image, ImageFilter, ImageXObject};

    /// Creates a simple test image (red square) with raw FlateDecode data.
    fn create_test_image(width: u32, height: u32, color: [u8; 3]) -> Image {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        // Create RGB pixel data
        let mut raw_data = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..(width * height) {
            raw_data.push(color[0]);
            raw_data.push(color[1]);
            raw_data.push(color[2]);
        }

        // Compress the data
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw_data).unwrap();
        let compressed = encoder.finish().unwrap();

        Image::new(
            width,
            height,
            ColorSpace::DeviceRGB,
            8,
            ImageFilter::FlateDecode,
            compressed,
        )
    }

    /// Creates a test image with alpha channel.
    fn create_test_image_with_alpha(width: u32, height: u32, color: [u8; 3], alpha: u8) -> Image {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        // Create RGB pixel data
        let mut raw_data = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..(width * height) {
            raw_data.push(color[0]);
            raw_data.push(color[1]);
            raw_data.push(color[2]);
        }

        // Compress the RGB data
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw_data).unwrap();
        let compressed_rgb = encoder.finish().unwrap();

        // Create alpha channel data
        let alpha_data = vec![alpha; (width * height) as usize];
        let mut alpha_encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        alpha_encoder.write_all(&alpha_data).unwrap();
        let compressed_alpha = alpha_encoder.finish().unwrap();

        let mut image = Image::new(
            width,
            height,
            ColorSpace::DeviceRGB,
            8,
            ImageFilter::FlateDecode,
            compressed_rgb,
        );

        // Add soft mask (alpha channel)
        image.soft_mask = Some(Box::new(Image::new(
            width,
            height,
            ColorSpace::DeviceGray,
            8,
            ImageFilter::FlateDecode,
            compressed_alpha,
        )));

        image
    }

    #[test]
    fn test_simple_image_pdf() {
        // Create a 100x100 red image
        let image = create_test_image(100, 100, [255, 0, 0]);

        let content = ContentBuilder::new()
            .draw_image("Img1", 100.0, 600.0, 200.0, 200.0);

        let page = PageBuilder::a4()
            .image("Img1", image)
            .content(content)
            .build();

        let doc = DocumentBuilder::new()
            .title("Simple Image Test")
            .page(page)
            .build()
            .unwrap();

        let bytes = save_and_verify(doc, "image_simple.pdf");
        let content = String::from_utf8_lossy(&bytes);

        // Verify XObject reference
        assert!(content.contains("/XObject"), "Missing XObject in resources");
        assert!(content.contains("/Img1"), "Missing image reference");
        assert!(content.contains("/Subtype /Image"), "Missing image subtype");
        assert!(content.contains("/Width 100"), "Missing image width");
        assert!(content.contains("/Height 100"), "Missing image height");
        assert!(content.contains("/ColorSpace /DeviceRGB"), "Missing color space");
    }

    #[test]
    fn test_multiple_images_pdf() {
        // Create different colored images
        let red_image = create_test_image(50, 50, [255, 0, 0]);
        let green_image = create_test_image(50, 50, [0, 255, 0]);
        let blue_image = create_test_image(50, 50, [0, 0, 255]);

        let content = ContentBuilder::new()
            .draw_image("ImgRed", 100.0, 600.0, 100.0, 100.0)
            .draw_image("ImgGreen", 250.0, 600.0, 100.0, 100.0)
            .draw_image("ImgBlue", 400.0, 600.0, 100.0, 100.0);

        let page = PageBuilder::a4()
            .image("ImgRed", red_image)
            .image("ImgGreen", green_image)
            .image("ImgBlue", blue_image)
            .content(content)
            .build();

        let doc = DocumentBuilder::new()
            .title("Multiple Images Test")
            .page(page)
            .build()
            .unwrap();

        let bytes = save_and_verify(doc, "image_multiple.pdf");
        let content = String::from_utf8_lossy(&bytes);

        // Verify all images are referenced
        assert!(content.contains("/ImgRed"), "Missing red image reference");
        assert!(content.contains("/ImgGreen"), "Missing green image reference");
        assert!(content.contains("/ImgBlue"), "Missing blue image reference");
    }

    #[test]
    fn test_image_with_alpha_pdf() {
        // Create a semi-transparent image
        let image = create_test_image_with_alpha(100, 100, [0, 128, 255], 128);
        assert!(image.has_alpha(), "Image should have alpha channel");

        let content = ContentBuilder::new()
            // Draw background rectangle first
            .save_state()
            .fill_color(Color::rgb(1.0, 0.9, 0.8))
            .rect(50.0, 550.0, 300.0, 200.0)
            .fill()
            .restore_state()
            // Draw the semi-transparent image over it
            .draw_image("Img1", 100.0, 600.0, 200.0, 100.0);

        let page = PageBuilder::a4()
            .image("Img1", image)
            .content(content)
            .build();

        let doc = DocumentBuilder::new()
            .title("Image with Alpha Test")
            .page(page)
            .build()
            .unwrap();

        let bytes = save_and_verify(doc, "image_with_alpha.pdf");
        let content = String::from_utf8_lossy(&bytes);

        // Verify soft mask is present
        assert!(content.contains("/SMask"), "Missing soft mask reference");
    }

    #[test]
    fn test_image_xobject_structure() {
        let image = create_test_image(200, 150, [64, 128, 192]);
        let xobject = ImageXObject::from_image(&image);

        assert_eq!(xobject.width, 200);
        assert_eq!(xobject.height, 150);
        assert!(!xobject.has_soft_mask());

        let dict_str = xobject.stream.dictionary_to_pdf_string();
        assert!(dict_str.contains("/Type /XObject"));
        assert!(dict_str.contains("/Subtype /Image"));
        assert!(dict_str.contains("/Width 200"));
        assert!(dict_str.contains("/Height 150"));
        assert!(dict_str.contains("/ColorSpace /DeviceRGB"));
        assert!(dict_str.contains("/BitsPerComponent 8"));
        assert!(dict_str.contains("/Filter /FlateDecode"));
    }

    #[test]
    fn test_image_xobject_with_alpha() {
        let image = create_test_image_with_alpha(100, 100, [255, 255, 0], 200);
        let xobject = ImageXObject::from_image(&image);

        assert!(xobject.has_soft_mask());
        assert!(xobject.soft_mask.is_some());

        let mask_stream = xobject.soft_mask.unwrap();
        let mask_dict = mask_stream.dictionary_to_pdf_string();
        assert!(mask_dict.contains("/ColorSpace /DeviceGray"));
    }

    #[test]
    fn test_grayscale_image_pdf() {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        // Create a grayscale gradient
        let width = 100u32;
        let height = 100u32;
        let mut raw_data = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for _ in 0..width {
                raw_data.push((y * 255 / height) as u8);
            }
        }

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw_data).unwrap();
        let compressed = encoder.finish().unwrap();

        let image = Image::new(
            width,
            height,
            ColorSpace::DeviceGray,
            8,
            ImageFilter::FlateDecode,
            compressed,
        );

        let content = ContentBuilder::new()
            .draw_image("Gradient", 100.0, 600.0, 200.0, 200.0);

        let page = PageBuilder::a4()
            .image("Gradient", image)
            .content(content)
            .build();

        let doc = DocumentBuilder::new()
            .title("Grayscale Image Test")
            .page(page)
            .build()
            .unwrap();

        let bytes = save_and_verify(doc, "image_grayscale.pdf");
        let content = String::from_utf8_lossy(&bytes);

        assert!(content.contains("/ColorSpace /DeviceGray"), "Missing grayscale color space");
    }

    #[test]
    fn test_image_with_text_pdf() {
        let image = create_test_image(80, 80, [200, 100, 50]);

        let content = ContentBuilder::new()
            .text("F1", 24.0, 72.0, 750.0, "Image with Text Demo")
            .text("F1", 12.0, 72.0, 700.0, "Below is an embedded image:")
            .draw_image("Logo", 72.0, 550.0, 150.0, 150.0)
            .text("F1", 12.0, 72.0, 520.0, "The image above was embedded using rust-pdf.");

        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .image("Logo", image)
            .content(content)
            .build();

        let doc = DocumentBuilder::new()
            .title("Image with Text")
            .author("rust-pdf")
            .page(page)
            .build()
            .unwrap();

        save_and_verify(doc, "image_with_text.pdf");
    }

    #[test]
    fn test_multi_page_images_pdf() {
        let mut pages = Vec::new();

        for i in 0..3 {
            // Create a different colored image for each page
            let colors = [[255, 0, 0], [0, 255, 0], [0, 0, 255]];
            let image = create_test_image(100, 100, colors[i]);
            let img_name = format!("Img{}", i + 1);

            let content = ContentBuilder::new()
                .text("F1", 18.0, 72.0, 750.0, &format!("Page {} Image Demo", i + 1))
                .draw_image(&img_name, 200.0, 400.0, 200.0, 200.0);

            let page = PageBuilder::a4()
                .font("F1", Standard14Font::Helvetica)
                .image(&img_name, image)
                .content(content)
                .build();

            pages.push(page);
        }

        let doc = DocumentBuilder::new()
            .title("Multi-Page Images")
            .pages(pages)
            .build()
            .unwrap();

        let bytes = save_and_verify(doc, "image_multi_page.pdf");
        let content = String::from_utf8_lossy(&bytes);

        // Verify all pages have images
        assert_eq!(content.matches("/Subtype /Image").count(), 3, "Should have 3 images");
    }

    #[test]
    fn test_image_aspect_ratio() {
        let image = create_test_image(200, 100, [128, 128, 128]);
        assert_eq!(image.aspect_ratio(), 2.0);

        let image2 = create_test_image(100, 200, [128, 128, 128]);
        assert_eq!(image2.aspect_ratio(), 0.5);

        let image3 = create_test_image(100, 100, [128, 128, 128]);
        assert_eq!(image3.aspect_ratio(), 1.0);
    }

    #[test]
    fn test_paint_xobject_operator() {
        let content = ContentBuilder::new()
            .paint_xobject("TestImage");

        let output = content.build_string();
        assert_eq!(output, "/TestImage Do");
    }

    #[test]
    fn test_draw_image_operator() {
        let content = ContentBuilder::new()
            .draw_image("Img1", 100.0, 200.0, 300.0, 400.0);

        let output = content.build_string();
        assert!(output.contains("q"), "Should save state");
        assert!(output.contains("300 0 0 400 100 200 cm"), "Should have transform matrix");
        assert!(output.contains("/Img1 Do"), "Should paint XObject");
        assert!(output.contains("Q"), "Should restore state");
    }
}

// ============================================================================
// Encryption Tests
// ============================================================================

#[cfg(feature = "encryption")]
mod encryption_tests {
    use super::*;
    use rust_pdf::encryption::{EncryptionConfig, Permissions};

    /// Helper to save an encrypted document and verify structure.
    fn save_encrypted_and_verify(doc: Document, name: &str) -> Vec<u8> {
        let path = output_dir().join(name);
        doc.save_to_file(&path).unwrap();

        let bytes = fs::read(&path).unwrap();
        let content = String::from_utf8_lossy(&bytes);

        // Basic PDF structure verification
        assert!(content.starts_with("%PDF-"), "Missing PDF header");
        assert!(content.contains("%%EOF"), "Missing EOF marker");
        assert!(content.contains("/Type /Catalog"), "Missing catalog");
        assert!(content.contains("xref"), "Missing xref table");
        assert!(content.contains("trailer"), "Missing trailer");

        // Encryption-specific verification
        assert!(content.contains("/Encrypt"), "Missing Encrypt reference in trailer");
        assert!(content.contains("/ID [<"), "Missing ID array in trailer");
        assert!(content.contains("/Filter /Standard"), "Missing Standard filter");
        assert!(content.contains("/V 5"), "Missing V value for AES-256");
        assert!(content.contains("/R 6"), "Missing R value for AES-256");
        assert!(content.contains("/CF"), "Missing crypt filter");
        assert!(content.contains("/StmF /StdCF"), "Missing stream filter");
        assert!(content.contains("/StrF /StdCF"), "Missing string filter");

        println!("Created encrypted: {} ({} bytes)", path.display(), bytes.len());
        bytes
    }

    #[test]
    fn test_encrypted_minimal_pdf() {
        let content = ContentBuilder::new()
            .text("F1", 24.0, 72.0, 750.0, "Encrypted PDF Document")
            .text("F1", 12.0, 72.0, 720.0, "This file is protected with AES-256 encryption.")
            .text("F1", 12.0, 72.0, 700.0, "User password: user123")
            .text("F1", 12.0, 72.0, 680.0, "Owner password: owner456");

        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(content)
            .build();

        let doc = DocumentBuilder::new()
            .title("Encrypted Minimal PDF")
            .encrypt(EncryptionConfig::aes256()
                .user_password("user123")
                .owner_password("owner456"))
            .page(page)
            .build()
            .unwrap();

        save_encrypted_and_verify(doc, "encrypted_minimal.pdf");
    }

    #[test]
    fn test_encrypted_with_content() {
        let content = ContentBuilder::new()
            .text("F1", 24.0, 72.0, 750.0, "This document is encrypted!")
            .text("F1", 12.0, 72.0, 720.0, "You need the password to open it.");

        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(content)
            .build();

        let doc = DocumentBuilder::new()
            .title("Encrypted Document")
            .author("rust-pdf")
            .encrypt(EncryptionConfig::aes256()
                .user_password("secret")
                .owner_password("admin"))
            .page(page)
            .build()
            .unwrap();

        let bytes = save_encrypted_and_verify(doc, "encrypted_with_content.pdf");
        let content_str = String::from_utf8_lossy(&bytes);

        // Verify metadata is present
        assert!(content_str.contains("/Title"), "Should have title");
        assert!(content_str.contains("/Author"), "Should have author");
    }

    #[test]
    fn test_encrypted_with_permissions() {
        let content = ContentBuilder::new()
            .text("F1", 24.0, 72.0, 750.0, "PDF with Custom Permissions")
            .text("F1", 12.0, 72.0, 720.0, "This document opens without a password.")
            .text("F1", 12.0, 72.0, 700.0, "")
            .text("F1", 14.0, 72.0, 670.0, "Permissions:")
            .text("F1", 12.0, 90.0, 650.0, "- Printing: ALLOWED")
            .text("F1", 12.0, 90.0, 635.0, "- Copying text: DENIED")
            .text("F1", 12.0, 90.0, 620.0, "- Modifying: DENIED")
            .text("F1", 12.0, 72.0, 590.0, "")
            .text("F1", 12.0, 72.0, 570.0, "Owner password: owner");

        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(content)
            .build();

        let permissions = Permissions::new()
            .allow_printing(true)
            .allow_copying(false)
            .allow_modifying(false);

        let doc = DocumentBuilder::new()
            .title("PDF with Permissions")
            .encrypt(EncryptionConfig::aes256()
                .user_password("")  // Empty password allows open without password
                .owner_password("owner")
                .permissions(permissions))
            .page(page)
            .build()
            .unwrap();

        let bytes = save_encrypted_and_verify(doc, "encrypted_permissions.pdf");
        let content_str = String::from_utf8_lossy(&bytes);

        // Verify permissions entry
        assert!(content_str.contains("/P "), "Should have permissions value");
    }

    #[test]
    fn test_encrypted_multipage() {
        let mut pages = Vec::new();
        for i in 1..=3 {
            let content = ContentBuilder::new()
                .text("F1", 18.0, 72.0, 750.0, &format!("Encrypted Page {}", i));

            let page = PageBuilder::a4()
                .font("F1", Standard14Font::Helvetica)
                .content(content)
                .build();

            pages.push(page);
        }

        let doc = DocumentBuilder::new()
            .title("Multi-Page Encrypted")
            .encrypt(EncryptionConfig::aes256()
                .user_password("multi")
                .owner_password("pages"))
            .pages(pages)
            .build()
            .unwrap();

        let bytes = save_encrypted_and_verify(doc, "encrypted_multipage.pdf");
        let content_str = String::from_utf8_lossy(&bytes);

        assert!(content_str.contains("/Count 3"), "Should have 3 pages");
    }
}

// ============================================================================
// Digital Signatures Tests
// ============================================================================

#[cfg(feature = "signatures")]
mod signature_tests {
    use super::*;
    use rust_pdf::signatures::{
        ByteRange, SignatureAlgorithm, SignatureConfig, SignatureInfo,
    };

    #[test]
    fn test_signature_config_builder() {
        let config = SignatureConfig::new()
            .name("John Doe")
            .reason("Document Approval")
            .location("San Francisco, CA")
            .contact_info("john@example.com")
            .algorithm(SignatureAlgorithm::RsaSha256)
            .signature_size(16384);

        assert_eq!(config.name, Some("John Doe".to_string()));
        assert_eq!(config.reason, Some("Document Approval".to_string()));
        assert_eq!(config.location, Some("San Francisco, CA".to_string()));
        assert_eq!(config.contact_info, Some("john@example.com".to_string()));
        assert_eq!(config.algorithm, SignatureAlgorithm::RsaSha256);
        assert_eq!(config.signature_size, 16384);
    }

    #[test]
    fn test_signature_algorithm_oids() {
        assert_eq!(
            SignatureAlgorithm::RsaSha256.oid(),
            "1.2.840.113549.1.1.11"
        );
        assert_eq!(
            SignatureAlgorithm::RsaSha384.oid(),
            "1.2.840.113549.1.1.12"
        );
        assert_eq!(
            SignatureAlgorithm::RsaSha512.oid(),
            "1.2.840.113549.1.1.13"
        );
        assert_eq!(
            SignatureAlgorithm::EcdsaP256Sha256.oid(),
            "1.2.840.10045.4.3.2"
        );
    }

    #[test]
    fn test_signature_info_structure() {
        let info = SignatureInfo {
            name: Some("Test Signer".to_string()),
            reason: Some("Testing".to_string()),
            location: Some("Test Location".to_string()),
            contact_info: Some("test@example.com".to_string()),
            signing_time: Some("D:20250120120000+00'00'".to_string()),
            byte_range: ByteRange::new(0, 100, 200, 300),
            is_valid: Some(true),
        };

        assert_eq!(info.name.as_deref(), Some("Test Signer"));
        assert_eq!(info.reason.as_deref(), Some("Testing"));
        assert_eq!(info.byte_range.offset1, 0);
        assert_eq!(info.byte_range.length1, 100);
        assert_eq!(info.byte_range.offset2, 200);
        assert_eq!(info.byte_range.length2, 300);
        assert_eq!(info.is_valid, Some(true));
    }

    #[test]
    fn test_byte_range_creation() {
        let br = ByteRange::new(0, 1000, 2000, 500);
        assert_eq!(br.offset1, 0);
        assert_eq!(br.length1, 1000);
        assert_eq!(br.offset2, 2000);
        assert_eq!(br.length2, 500);
    }

    #[test]
    fn test_signature_config_defaults() {
        let config = SignatureConfig::default();
        assert!(config.name.is_none());
        assert!(config.reason.is_none());
        assert!(config.location.is_none());
        assert!(config.contact_info.is_none());
        assert_eq!(config.algorithm, SignatureAlgorithm::RsaSha256);
        assert_eq!(config.signature_size, 8192);
        assert!(config.embed_certificate_chain);
    }

    #[test]
    fn test_all_signature_algorithms() {
        let algorithms = [
            SignatureAlgorithm::RsaSha256,
            SignatureAlgorithm::RsaSha384,
            SignatureAlgorithm::RsaSha512,
            SignatureAlgorithm::EcdsaP256Sha256,
        ];

        for algo in algorithms {
            // Test that OID is not empty
            assert!(!algo.oid().is_empty());
            // Test that digest OID is not empty
            assert!(!algo.digest_oid().is_empty());
        }
    }
}
