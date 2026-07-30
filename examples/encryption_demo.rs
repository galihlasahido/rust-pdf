//! Demonstrates AES-256 password protection on a document this crate
//! creates (README.md's "Password protection" quick start). Note this is
//! write-only: this crate has no code path to *open* an existing
//! encrypted PDF (see README.md's "Known limitations").
//!
//! Run with:
//! ```text
//! cargo run --features encryption --example encryption_demo
//! ```

use rust_pdf::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(ContentBuilder::new().text("F1", 14.0, 72.0, 760.0, "Confidential"))
        .build();

    let doc = DocumentBuilder::new()
        .encrypt(
            EncryptionConfig::aes256()
                .user_password("user123")
                .owner_password("owner456")
                .permissions(Permissions::default().allow_printing(true).allow_copying(false)),
        )
        .page(page)
        .build()?;

    doc.save_to_file("tests/output/encrypted_demo.pdf")?;
    println!("wrote tests/output/encrypted_demo.pdf (user password: user123, owner: owner456)");
    Ok(())
}
