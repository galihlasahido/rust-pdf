//! Integration tests for DocMDP certification signatures
//! ([`SignatureConfig::certify`] / [`CertificationLevel`]).
//!
//! These mirror the pattern already used in
//! `tests/signature_verification_tests.rs` and `tests/remote_signer_tests.rs`:
//! shell out to the system `openssl` binary to generate a throwaway RSA
//! key/self-signed certificate, then drive [`DocumentSigner`] /
//! [`IncrementalSigner`] through the public API.

#![cfg(feature = "signatures")]

use std::path::Path;
use std::process::Command;

use rust_pdf::prelude::*;
use rust_pdf::signatures::{
    Certificate, CertificationLevel, DocumentSigner, IncrementalSigner, PrivateKey,
    SignatureAlgorithm, SignatureConfig, SignatureVerifier,
};

fn openssl_available() -> bool {
    Command::new("openssl")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn generate_rsa_cert(dir: &Path, name: &str) -> (PrivateKey, Certificate) {
    let key_path = dir.join(format!("{name}_key.pem"));
    let cert_path = dir.join(format!("{name}_cert.pem"));

    let status = Command::new("openssl")
        .args(["genrsa", "-out", key_path.to_str().unwrap(), "2048"])
        .status()
        .expect("failed to run openssl genrsa");
    assert!(status.success(), "openssl genrsa failed");

    let subject = format!("/CN={name}/O=Test/C=US");
    let status = Command::new("openssl")
        .args([
            "req",
            "-new",
            "-x509",
            "-key",
            key_path.to_str().unwrap(),
            "-out",
            cert_path.to_str().unwrap(),
            "-days",
            "365",
            "-subj",
            &subject,
        ])
        .status()
        .expect("failed to run openssl req");
    assert!(status.success(), "openssl req failed");

    let key = PrivateKey::from_pem_file(&key_path).expect("load private key");
    let cert = Certificate::from_pem_file(&cert_path).expect("load certificate");
    (key, cert)
}

fn sample_document() -> Document {
    let content = ContentBuilder::new().text("F1", 24.0, 72.0, 750.0, "DocMDP test document");
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();
    DocumentBuilder::new()
        .title("DocMDP certification test")
        .page(page)
        .build()
        .expect("document should build")
}

fn sample_pdf_bytes() -> Vec<u8> {
    sample_document()
        .save_to_bytes()
        .expect("document should serialize")
}

/// Certifying a fresh, never-before-signed document must succeed, embed a
/// `/DocMDP` reference + `/Perms` catalog entry, and round-trip through
/// [`SignatureVerifier`] with the correct declared [`CertificationLevel`].
#[test]
fn test_certify_fresh_document_round_trips_through_verifier() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = tempfile::TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "docmdp-certifier");

    let config = SignatureConfig::new()
        .name("Certifying Author")
        .reason("Certified document")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .certify(CertificationLevel::FormFillingOnly);

    let signed_pdf = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .config(config)
        .sign()
        .expect("certifying a fresh document should succeed");

    // The signed bytes should carry the DocMDP structures.
    let text = String::from_utf8_lossy(&signed_pdf);
    assert!(
        text.contains("/TransformMethod /DocMDP"),
        "signed PDF should contain a DocMDP /Reference entry"
    );
    assert!(
        text.contains("/Perms"),
        "signed PDF should contain a /Perms catalog entry"
    );

    let results = SignatureVerifier::new(signed_pdf)
        .verify()
        .expect("verify should succeed");
    assert_eq!(results.len(), 1);
    assert!(
        results[0].is_valid,
        "certifying signature should verify cryptographically: {:?}",
        results[0].error
    );
    assert_eq!(
        results[0].certification_level,
        Some(CertificationLevel::FormFillingOnly),
        "verifier should surface the declared certification level"
    );
}

/// Certifying via [`IncrementalSigner`] on a document that has zero
/// existing signatures must also succeed and round-trip, exercising the
/// other of the two signer paths named in the task.
#[test]
fn test_certify_fresh_document_via_incremental_signer() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = tempfile::TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "docmdp-incremental-certifier");

    let config = SignatureConfig::new()
        .name("Certifying Author")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .certify(CertificationLevel::NoChanges);

    let signed_pdf = IncrementalSigner::new(sample_pdf_bytes())
        .certificate(cert)
        .private_key(key)
        .config(config)
        .sign()
        .expect("certifying via IncrementalSigner on an unsigned document should succeed");

    let results = SignatureVerifier::new(signed_pdf)
        .verify()
        .expect("verify should succeed");
    assert_eq!(results.len(), 1);
    assert!(results[0].is_valid, "error: {:?}", results[0].error);
    assert_eq!(
        results[0].certification_level,
        Some(CertificationLevel::NoChanges)
    );
}

/// A plain (non-certifying) approval signature must not report a
/// certification level at all.
#[test]
fn test_approval_signature_has_no_certification_level() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = tempfile::TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "docmdp-approval-only");

    let signed_pdf = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .name("Approver")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("plain approval signing should succeed");

    let results = SignatureVerifier::new(signed_pdf)
        .verify()
        .expect("verify should succeed");
    assert_eq!(results.len(), 1);
    assert!(results[0].is_valid);
    assert_eq!(results[0].certification_level, None);
}

/// Attempting to certify a document via [`IncrementalSigner`] that already
/// carries a signature must be rejected -- a certification signature is
/// only valid as the *first* signature on a document (ISO 32000-1
/// 12.8.2.2).
#[test]
fn test_certify_already_signed_document_is_rejected() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = tempfile::TempDir::new().unwrap();
    let (key1, cert1) = generate_rsa_cert(dir.path(), "docmdp-first-signer");
    let (key2, cert2) = generate_rsa_cert(dir.path(), "docmdp-second-signer");

    // First, apply an ordinary approval signature.
    let once_signed = IncrementalSigner::new(sample_pdf_bytes())
        .certificate(cert1)
        .private_key(key1)
        .name("First Signer")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("first approval signature should succeed");

    // A second signature that also asks to be a certification signature
    // must be rejected, since the document is no longer unsigned.
    let config = SignatureConfig::new()
        .name("Second Signer")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .certify(CertificationLevel::NoChanges);

    let result = IncrementalSigner::new(once_signed)
        .certificate(cert2)
        .private_key(key2)
        .config(config)
        .sign();

    assert!(
        result.is_err(),
        "certifying an already-signed document should be rejected"
    );
}

/// Certifying twice in a row via [`IncrementalSigner`] (certify, then try
/// to certify again on the result) must also be rejected -- covers the
/// case where the *first* signature on the once-signed document was itself
/// a certification signature, not just an ordinary approval one.
#[test]
fn test_certify_already_certified_document_is_rejected() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = tempfile::TempDir::new().unwrap();
    let (key1, cert1) = generate_rsa_cert(dir.path(), "docmdp-double-certify-first");
    let (key2, cert2) = generate_rsa_cert(dir.path(), "docmdp-double-certify-second");

    let certified_once = IncrementalSigner::new(sample_pdf_bytes())
        .certificate(cert1)
        .private_key(key1)
        .name("First Signer")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .config(
            SignatureConfig::new()
                .name("First Signer")
                .algorithm(SignatureAlgorithm::RsaSha256)
                .certify(CertificationLevel::FormFillingAnnotationsAndSigning),
        )
        .sign()
        .expect("first certification should succeed");

    let config = SignatureConfig::new()
        .name("Second Signer")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .certify(CertificationLevel::NoChanges);

    let result = IncrementalSigner::new(certified_once)
        .certificate(cert2)
        .private_key(key2)
        .config(config)
        .sign();

    assert!(
        result.is_err(),
        "a second certification signature on an already-certified document should be rejected"
    );
}
