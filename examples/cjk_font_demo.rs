//! Demonstrates embedding a real CJK TrueType font as a Type 0/CIDFontType2
//! composite font (README.md's CJK/font-subsetting quick start), with
//! automatic subsetting to only the glyphs actually used.
//!
//! Uses the small, OFL-licensed Noto Sans SC derivative already vendored
//! at `tests/fixtures/fonts/NotoSansSC-Subset.ttf` for
//! `tests/font_embedding_tests.rs` (see that directory's `NOTICE.md` for
//! provenance) so this example needs no external font file.
//!
//! Run with:
//! ```text
//! cargo run --features "fonts parser" --example cjk_font_demo
//! ```

use rust_pdf::font::CompositeFont;
use rust_pdf::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    let font_bytes = std::fs::read("tests/fixtures/fonts/NotoSansSC-Subset.ttf")?;

    // `subset(true)` (the default) keeps only the glyphs this document
    // actually references in the embedded font program.
    let font = CompositeFont::new(font_bytes, "NotoSansSC")?.subset(true);
    let cid_bytes = font.encode("中文测试");

    let page = PageBuilder::a4()
        .font("F1", font)
        .content(ContentBuilder::new().text_block(
            TextBuilder::new().font("F1", 24.0).position(72.0, 700.0).show_bytes(cid_bytes),
        ))
        .build();

    let bytes = DocumentBuilder::new().title("CJK demo").page(page).build()?.save_to_bytes()?;
    std::fs::write("tests/output/cjk_demo.pdf", &bytes)?;
    println!("wrote tests/output/cjk_demo.pdf ({} bytes)", bytes.len());

    // Round-trip through the parser/editor to confirm the CID text
    // extracts back to the correct Unicode.
    let doc = EditableDocument::from_bytes(bytes)?;
    let page_id = doc.page_id_at(0)?;
    let extracted = doc.extract_page_text(page_id)?;
    println!("extracted text: {}", extracted.trim());
    assert_eq!(extracted.trim(), "中文测试");

    Ok(())
}
