//! Demonstrates opening a password-protected PDF: encrypts a document with
//! `EncryptionConfig` (README.md's "Password protection" quick start), then
//! reopens the encrypted bytes with `EditableDocument::from_bytes_with_password`.
//!
//! Scoped to exactly the two algorithms `EncryptionConfig` can itself
//! produce: AES-128 (`/V 4 /R 4`) and AES-256 (`/V 5 /R 6`). Any other
//! encryption scheme (legacy RC4, `/V 1`/`/V 2`) fails closed with a
//! distinct `ParserError::UnsupportedEncryption` rather than being
//! silently mis-decrypted; a wrong password fails closed with
//! `ParserError::IncorrectPassword`.
//!
//! Run with:
//! ```text
//! cargo run --features "parser encryption" --example open_encrypted_pdf_demo
//! ```

use rust_pdf::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(ContentBuilder::new().text("F1", 14.0, 72.0, 760.0, "Confidential"))
        .build();

    let encrypted_bytes = DocumentBuilder::new()
        .encrypt(
            EncryptionConfig::aes256()
                .user_password("user123")
                .owner_password("owner456"),
        )
        .page(page)
        .build()?
        .save_to_bytes()?;
    std::fs::write("tests/output/open_encrypted_demo.pdf", &encrypted_bytes)?;
    println!("wrote tests/output/open_encrypted_demo.pdf (user password: user123)");

    // The plain, no-password entry point still refuses an encrypted file
    // outright -- this is unchanged, deliberate behavior (see
    // RenderError::PasswordRequired's docs).
    let rejected_err = EditableDocument::from_bytes(encrypted_bytes.clone())
        .err()
        .expect("from_bytes (no password) must still refuse an encrypted document");
    println!("from_bytes (no password): correctly refused -- {rejected_err}");

    // The wrong password fails closed with a distinct error, not a silent
    // garbage decrypt.
    let wrong_password_err =
        EditableDocument::from_bytes_with_password(encrypted_bytes.clone(), "not-it")
            .err()
            .expect("wrong password must be rejected");
    println!("from_bytes_with_password (wrong password): correctly refused -- {wrong_password_err}");

    // The right password opens it for editing, exactly like any other PDF.
    let doc = EditableDocument::from_bytes_with_password(encrypted_bytes, "user123")?;
    let page_id = doc.page_id_at(0)?;
    let text = doc.extract_page_text(page_id)?;
    assert!(text.contains("Confidential"));
    println!("from_bytes_with_password (correct password): opened -- page 0 text: {text:?}");

    Ok(())
}
