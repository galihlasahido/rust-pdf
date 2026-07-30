//! Demonstrates building an AcroForm, filling it programmatically via
//! [`EditableDocument`], and flattening it to a non-interactive,
//! visually-identical document (README.md's Forms quick start).
//!
//! Run with:
//! ```text
//! cargo run --features parser --example form_fill_flatten_demo
//! ```

use rust_pdf::forms::{CheckBox, TextField};
use rust_pdf::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    // 1. Build a document with a text field and a checkbox (AcroForm,
    //    ISO 32000-1 12.7).
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .form_field(TextField::new("full_name").rect(100.0, 700.0, 250.0, 20.0))
        .form_field(CheckBox::new("subscribe").rect(100.0, 660.0, 18.0, 18.0))
        .content(ContentBuilder::new().text("F1", 14.0, 72.0, 760.0, "Sign-up Form"))
        .build();
    let bytes = DocumentBuilder::new().title("Sign-up Form").page(page).build()?.save_to_bytes()?;

    // 2. Fill it in.
    let mut doc = EditableDocument::from_bytes(bytes)?;
    doc.set_text_value("full_name", "Andi Wijaya")?;
    doc.set_checkbox_checked("subscribe", true)?;
    doc.save_incremental("tests/output/signup_filled.pdf")?;
    println!("wrote tests/output/signup_filled.pdf (fields still interactive)");

    // 3. Flatten: bake widget appearances into page content and drop
    //    /AcroForm - the result has no interactive fields left.
    doc.flatten_form()?;
    doc.save_full_rewrite("tests/output/signup_flattened.pdf")?;
    println!("wrote tests/output/signup_flattened.pdf (flattened, no interactive fields)");

    assert!(doc.field_names()?.is_empty());
    Ok(())
}
