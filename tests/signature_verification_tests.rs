//! Integration tests for PDF digital signature verification.
//!
//! These tests shell out to the system `openssl` binary to generate
//! throwaway RSA/EC keys and self-signed certificates, mirroring the
//! pattern already used in `examples/digital_signature_example.rs`.

#![cfg(feature = "signatures")]

use std::path::Path;
use std::process::Command;

use rust_pdf::prelude::*;
use rust_pdf::signatures::{
    Certificate, DocumentSigner, IncrementalSigner, PrivateKey, SignatureAlgorithm,
    SignatureVerifier,
};
use tempfile::TempDir;

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

/// Generates an EC key in PKCS#8 form (`openssl genpkey`).
fn generate_ec_cert(dir: &Path, name: &str) -> (PrivateKey, Certificate) {
    generate_ec_cert_with(
        dir,
        name,
        &["genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256"],
    )
}

/// Generates an EC key in legacy SEC1 form (`openssl ecparam -genkey`,
/// `-----BEGIN EC PRIVATE KEY-----`) -- the traditional format most
/// commonly produced by OpenSSL tooling for EC keys.
fn generate_ec_cert_sec1(dir: &Path, name: &str) -> (PrivateKey, Certificate) {
    generate_ec_cert_with(dir, name, &["ecparam", "-name", "prime256v1", "-genkey", "-noout"])
}

fn generate_ec_cert_with(dir: &Path, name: &str, keygen_args: &[&str]) -> (PrivateKey, Certificate) {
    let key_path = dir.join(format!("{name}_key.pem"));
    let cert_path = dir.join(format!("{name}_cert.pem"));

    let status = Command::new("openssl")
        .args(keygen_args)
        .args(["-out", key_path.to_str().unwrap()])
        .status()
        .expect("failed to run openssl key generation");
    assert!(status.success(), "openssl key generation failed");

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

/// Generates an RSA key in legacy PKCS#1 form (`openssl genrsa -traditional`,
/// `-----BEGIN RSA PRIVATE KEY-----`).
fn generate_rsa_cert_pkcs1(dir: &Path, name: &str) -> (PrivateKey, Certificate) {
    let key_path = dir.join(format!("{name}_key.pem"));
    let cert_path = dir.join(format!("{name}_cert.pem"));

    let status = Command::new("openssl")
        .args(["genrsa", "-traditional", "-out", key_path.to_str().unwrap(), "2048"])
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
    let content = ContentBuilder::new().text("F1", 24.0, 72.0, 750.0, "Signed document");
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();
    DocumentBuilder::new()
        .title("Signature verification test")
        .page(page)
        .build()
        .expect("document should build")
}

#[test]
fn test_verify_single_rsa_signature() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "signer1");

    let signed = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .name("John Doe")
        .reason("Testing")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("sign should succeed");

    let results = SignatureVerifier::new(signed)
        .verify()
        .expect("verify should succeed");

    assert_eq!(results.len(), 1);
    let sig = &results[0];
    assert!(sig.is_valid, "signature should be valid: {:?}", sig.error);
    assert_eq!(sig.signer_name.as_deref(), Some("John Doe"));
    assert_eq!(sig.reason.as_deref(), Some("Testing"));
    assert!(sig.certificate.is_some());
}

#[test]
fn test_verify_multi_signature() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key1, cert1) = generate_rsa_cert(dir.path(), "signer1");
    let (key2, cert2) = generate_rsa_cert(dir.path(), "signer2");

    let signed_once = DocumentSigner::new(sample_document())
        .certificate(cert1)
        .private_key(key1)
        .name("John Doe")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("first sign should succeed");

    let signed_twice = IncrementalSigner::new(signed_once)
        .certificate(cert2)
        .private_key(key2)
        .name("Jane Smith")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("second sign should succeed");

    let results = SignatureVerifier::new(signed_twice)
        .verify()
        .expect("verify should succeed");

    assert_eq!(results.len(), 2);
    for sig in &results {
        assert!(
            sig.is_valid,
            "signature should be valid: {:?} (signer: {:?})",
            sig.error, sig.signer_name
        );
    }

    let names: Vec<Option<String>> = results.iter().map(|s| s.signer_name.clone()).collect();
    assert!(names.contains(&Some("John Doe".to_string())));
    assert!(names.contains(&Some("Jane Smith".to_string())));
}

#[test]
fn test_tamper_detection() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "signer1");

    let mut signed = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .name("John Doe")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("sign should succeed");

    let baseline = SignatureVerifier::new(signed.clone()).verify().unwrap();
    assert_eq!(baseline.len(), 1);
    assert!(baseline[0].is_valid);

    // Flip a byte early in the file (inside the PDF header, well before the
    // signature dictionary) -- stays inside the signed ByteRange without
    // corrupting the /Type /Sig /ByteRange /Contents scan itself.
    signed[10] ^= 0xFF;

    let tampered = SignatureVerifier::new(signed).verify().unwrap();
    assert_eq!(tampered.len(), 1);
    assert!(!tampered[0].is_valid);
    assert!(tampered[0].error.is_some());
}

#[test]
fn test_verify_ecdsa_signature() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_ec_cert(dir.path(), "signer_ec");

    let signed = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .name("EC Signer")
        .algorithm(SignatureAlgorithm::EcdsaP256Sha256)
        .sign()
        .expect("sign should succeed");

    let results = SignatureVerifier::new(signed)
        .verify()
        .expect("verify should succeed");

    assert_eq!(results.len(), 1);
    assert!(results[0].is_valid, "signature should be valid: {:?}", results[0].error);
}

#[test]
fn test_verify_ecdsa_signature_sec1_key() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_ec_cert_sec1(dir.path(), "signer_ec_sec1");

    let signed = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .name("EC Signer (SEC1 key)")
        .algorithm(SignatureAlgorithm::EcdsaP256Sha256)
        .sign()
        .expect("sign with a SEC1-format EC key should succeed");

    let results = SignatureVerifier::new(signed)
        .verify()
        .expect("verify should succeed");

    assert_eq!(results.len(), 1);
    assert!(results[0].is_valid, "signature should be valid: {:?}", results[0].error);
}

#[test]
fn test_sign_with_pkcs1_rsa_key() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert_pkcs1(dir.path(), "signer_pkcs1");

    let signed = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .name("PKCS1 RSA Signer")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("sign with a PKCS#1-format RSA key should succeed");

    let results = SignatureVerifier::new(signed)
        .verify()
        .expect("verify should succeed");

    assert_eq!(results.len(), 1);
    assert!(results[0].is_valid, "signature should be valid: {:?}", results[0].error);
}

#[test]
fn test_verify_unsigned_pdf_has_no_signatures() {
    let doc = sample_document();
    let bytes = doc.save_to_bytes().unwrap();

    let results = SignatureVerifier::new(bytes).verify().unwrap();
    assert!(results.is_empty());
}
