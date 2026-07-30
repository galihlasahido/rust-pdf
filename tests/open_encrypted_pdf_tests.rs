//! Integration tests for opening an encrypted PDF with a password
//! (`PdfReader::from_bytes_with_password`/`from_file_with_password`,
//! `EditableDocument::from_bytes_with_password`/`open_with_password`, and
//! `PdfRenderer::open_bytes_with_password`/`open_file_with_password`).
//!
//! The primary scenario ([`opening_own_encrypted_output_with_the_right_password_round_trips_content`])
//! is the one this whole feature exists for: encrypt a document with the
//! already-existing, already-tested write path
//! (`EditableDocument::save_encrypted_to_bytes`), then confirm the new
//! read path can open the result back up and recover the original
//! content -- something this crate's parser could not do at all before
//! (see `src/editor/encrypt.rs`'s module docs for the previously-disclosed
//! gap this closes for the two algorithms this crate can itself produce).

#![cfg(all(feature = "parser", feature = "encryption"))]

use rust_pdf::prelude::*;

const PAGE_TEXT: &str = "Confidential quarterly figures: revenue up 12%";

fn build_document() -> Vec<u8> {
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(
            ContentBuilder::new()
                .text("F1", 12.0, 72.0, 750.0, PAGE_TEXT)
                .graphics(
                    // A large, unmissable block of color so the
                    // renderer-integration test below can cheaply confirm
                    // "this raster has real content" via coarse pixel
                    // sampling, independent of exactly where/how small the
                    // text glyphs themselves land.
                    GraphicsBuilder::new()
                        .fill_color(Color::rgb(0.1, 0.4, 0.9))
                        .rect(50.0, 400.0, 300.0, 200.0)
                        .fill(),
                ),
        )
        .build();
    DocumentBuilder::new()
        .title("Encrypted round-trip test document")
        .page(page)
        .build()
        .unwrap()
        .save_to_bytes()
        .unwrap()
}

fn encrypt(config: EncryptionConfig) -> Vec<u8> {
    let doc = EditableDocument::from_bytes(build_document()).unwrap();
    doc.save_encrypted_to_bytes(config).unwrap()
}

// -------------------------------------------------------------------
// (a) The single most important scenario: encrypt via the existing
// write path, then open the result back up with the right password and
// confirm content round-trips.
// -------------------------------------------------------------------

#[test]
fn opening_own_encrypted_output_with_the_right_password_round_trips_content() {
    for config in [
        EncryptionConfig::aes256()
            .user_password("correct horse battery staple")
            .owner_password("owner-secret"),
        EncryptionConfig::aes128()
            .user_password("correct horse battery staple")
            .owner_password("owner-secret"),
    ] {
        let algorithm = config.algorithm;
        let encrypted = encrypt(config);

        // Through `PdfReader` directly.
        let reader = rust_pdf::parser::PdfReader::from_bytes_with_password(
            encrypted.clone(),
            "correct horse battery staple",
        )
        .unwrap_or_else(|e| panic!("{algorithm:?}: expected Ok, got {e}"));
        assert_eq!(reader.page_count(), 1, "{algorithm:?}: wrong page count");
        let catalog = reader
            .catalog()
            .unwrap_or_else(|| panic!("{algorithm:?}: catalog should resolve"));
        assert!(
            catalog.get("Pages").is_some(),
            "{algorithm:?}: catalog should have a /Pages entry"
        );

        // Through `EditableDocument` (the higher-level API a real caller
        // uses), asserting the *actual page content* -- not just
        // structure -- survives the encrypt/decrypt round trip.
        let doc =
            EditableDocument::from_bytes_with_password(encrypted, "correct horse battery staple")
                .unwrap_or_else(|e| panic!("{algorithm:?}: expected Ok, got {e}"));
        assert_eq!(doc.page_count().unwrap(), 1);
        let page_id = doc.page_id_at(0).unwrap();
        let text = doc.extract_page_text(page_id).unwrap();
        assert!(
            text.contains(PAGE_TEXT),
            "{algorithm:?}: expected extracted text to contain {PAGE_TEXT:?}, got {text:?}"
        );
    }
}

/// Same round trip via a file on disk ([`PdfReader::from_file_with_password`]/
/// [`EditableDocument::open_with_password`]), not just in-memory bytes --
/// confirms the memory-mapped-file backing path also goes through
/// decryption correctly, not just the owned-buffer path.
#[test]
fn opening_own_encrypted_output_from_a_file_round_trips_content() {
    let encrypted = encrypt(EncryptionConfig::aes256().user_password("hunter2"));

    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &encrypted).unwrap();

    let doc = EditableDocument::open_with_password(file.path(), "hunter2").unwrap();
    let page_id = doc.page_id_at(0).unwrap();
    let text = doc.extract_page_text(page_id).unwrap();
    assert!(text.contains(PAGE_TEXT));
}

// -------------------------------------------------------------------
// (b) Wrong password fails with a clear, distinct error.
// -------------------------------------------------------------------

#[test]
fn opening_with_the_wrong_password_fails_with_a_distinct_error() {
    for config in [
        EncryptionConfig::aes256().user_password("right-password"),
        EncryptionConfig::aes128().user_password("right-password"),
    ] {
        let algorithm = config.algorithm;
        let encrypted = encrypt(config);

        let result = rust_pdf::parser::PdfReader::from_bytes_with_password(
            encrypted.clone(),
            "totally-wrong-password",
        );
        match result {
            Err(rust_pdf::PdfError::Parser(rust_pdf::ParserError::IncorrectPassword)) => {}
            Err(other) => panic!("{algorithm:?}: expected IncorrectPassword, got {other}"),
            Ok(_) => panic!("{algorithm:?}: wrong password must not open the document"),
        }

        // Same distinct error through the higher-level `EditableDocument`
        // entry point.
        let result =
            EditableDocument::from_bytes_with_password(encrypted, "totally-wrong-password");
        match result {
            Err(rust_pdf::PdfError::Parser(rust_pdf::ParserError::IncorrectPassword)) => {}
            Err(other) => panic!("{algorithm:?}: expected IncorrectPassword, got {other}"),
            Ok(_) => panic!("{algorithm:?}: wrong password must not open the document"),
        }
    }
}

/// An empty-string password is still just "a password" as far as
/// [`ParserError::IncorrectPassword`] validation goes: it must not be
/// treated as equivalent to "no password supplied" (that's
/// [`ParserError::EncryptedPdf`], see the no-password test below), nor
/// must it accidentally succeed.
#[test]
fn opening_with_an_empty_password_when_one_is_required_fails_not_panics() {
    let encrypted = encrypt(EncryptionConfig::aes256().user_password("not-empty"));

    let result = rust_pdf::parser::PdfReader::from_bytes_with_password(encrypted, "");
    assert!(matches!(
        result,
        Err(rust_pdf::PdfError::Parser(
            rust_pdf::ParserError::IncorrectPassword
        ))
    ));
}

// -------------------------------------------------------------------
// (c) No password supplied when one is required still surfaces the
// existing `PasswordRequired`-equivalent behavior (`ParserError::EncryptedPdf`,
// and `RenderError::PasswordRequired` through `PdfRenderer`).
// -------------------------------------------------------------------

#[test]
fn opening_an_encrypted_document_without_a_password_still_fails_the_same_way_as_before() {
    let encrypted = encrypt(EncryptionConfig::aes256().user_password("secret"));

    // The pre-existing no-password constructors keep their pre-existing
    // behavior exactly: outright rejection, unconditionally.
    let result = rust_pdf::parser::PdfReader::from_bytes(encrypted.clone());
    assert!(matches!(
        result,
        Err(rust_pdf::PdfError::Parser(
            rust_pdf::ParserError::EncryptedPdf
        ))
    ));

    let result = EditableDocument::from_bytes(encrypted);
    assert!(matches!(
        result,
        Err(rust_pdf::PdfError::Parser(
            rust_pdf::ParserError::EncryptedPdf
        ))
    ));
}

/// An unencrypted document opened through the *password-aware*
/// constructors must still open fine, simply ignoring the (unnecessary)
/// password -- the new entry points are a strict superset of the old
/// ones' capability, not a behavior change for plain documents.
#[test]
fn password_aware_open_on_an_unencrypted_document_ignores_the_password() {
    let plain = build_document();
    let doc = EditableDocument::from_bytes_with_password(plain, "irrelevant").unwrap();
    assert_eq!(doc.page_count().unwrap(), 1);
}

#[cfg(feature = "render")]
mod render_integration {
    use super::*;
    use rust_pdf::render::PdfRenderer;

    /// [`PdfRenderer::open_bytes_with_password`] must round-trip through
    /// to an actual, non-blank rendered page -- not just "the document
    /// structure opened" -- confirming decrypted content streams really
    /// do reach the content-stream interpreter.
    #[test]
    fn renderer_opens_encrypted_document_with_correct_password_and_renders() {
        let encrypted = encrypt(EncryptionConfig::aes256().user_password("render-me"));
        let renderer = PdfRenderer::open_bytes_with_password(encrypted, "render-me").unwrap();
        assert_eq!(renderer.page_count(), 1);

        let image = renderer.render_page(0, 72.0, None).unwrap();
        // A rendered page of real text content must not be a single flat
        // color (i.e. actual glyph ink is present). Sample a coarse grid
        // rather than every pixel (cheap, dependency-free proxy for "this
        // raster has real content", same approach `tests/render_tests.rs`
        // uses).
        let (w, h) = image.dimensions();
        let mut seen = std::collections::HashSet::new();
        for gy in 0..32u32.min(h) {
            for gx in 0..32u32.min(w) {
                let x = (gx * w / 32).min(w - 1);
                let y = (gy * h / 32).min(h - 1);
                let p = image.get_pixel(x, y);
                seen.insert((p[0] / 16, p[1] / 16, p[2] / 16));
            }
        }
        assert!(
            seen.len() > 1,
            "expected the rendered page to contain visible text, got a flat image"
        );
    }

    /// (c) via the renderer's own no-password entry point: unchanged,
    /// still `RenderError::PasswordRequired`.
    #[test]
    fn renderer_without_a_password_still_reports_password_required() {
        let encrypted = encrypt(EncryptionConfig::aes256().user_password("render-me"));
        match PdfRenderer::open_bytes(encrypted).err() {
            Some(rust_pdf::RenderError::PasswordRequired) => {}
            other => panic!("expected PasswordRequired, got {other:?}"),
        }
    }

    /// (b) via the renderer's password-aware entry point: a wrong
    /// password is a document-load failure, not `PasswordRequired`
    /// (which specifically means "no password was offered at all").
    #[test]
    fn renderer_with_wrong_password_is_a_document_load_error_not_password_required() {
        let encrypted = encrypt(EncryptionConfig::aes256().user_password("render-me"));
        match PdfRenderer::open_bytes_with_password(encrypted, "nope").err() {
            Some(rust_pdf::RenderError::DocumentLoad(_)) => {}
            other => panic!("expected DocumentLoad, got {other:?}"),
        }
    }
}
