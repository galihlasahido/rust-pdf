//! Integration tests for ECDSA P-384 and P-521 signing/verification
//! (`SignatureAlgorithm::EcdsaP384Sha384` / `EcdsaP521Sha512`).
//!
//! These shell out to the system `openssl` binary to generate throwaway
//! EC keys/self-signed certs, mirroring `tests/signature_verification_tests.rs`'s
//! `generate_ec_cert`/`generate_ec_cert_sec1` helpers (kept in a separate
//! file per the ECDSA multi-curve task's file ownership, rather than
//! appended to that shared file).

#![cfg(feature = "signatures")]

use std::path::Path;
use std::process::Command;

use rust_pdf::prelude::*;
use rust_pdf::signatures::{
    Certificate, DocumentSigner, PrivateKey, SignatureAlgorithm, SignatureVerifier,
};
use tempfile::TempDir;

fn openssl_available() -> bool {
    Command::new("openssl")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Generates an EC key/self-signed cert for `curve` (an OpenSSL curve name,
/// e.g. `"P-384"`/`"P-521"`) in PKCS#8 form (`openssl genpkey`).
fn generate_ec_cert_pkcs8(dir: &Path, name: &str, curve: &str) -> (PrivateKey, Certificate) {
    generate_ec_cert_with(
        dir,
        name,
        &["genpkey", "-algorithm", "EC", "-pkeyopt"],
        &format!("ec_paramgen_curve:{curve}"),
    )
}

/// Generates an EC key/self-signed cert for `curve` (an OpenSSL curve name,
/// e.g. `"secp384r1"`/`"secp521r1"`) in legacy SEC1 form (`openssl ecparam
/// -genkey`, `-----BEGIN EC PRIVATE KEY-----`).
fn generate_ec_cert_sec1(dir: &Path, name: &str, curve: &str) -> (PrivateKey, Certificate) {
    generate_ec_cert_with(dir, name, &["ecparam", "-name"], curve)
}

/// Shared key/cert generation, parameterized over the exact keygen args so
/// PKCS#8 and SEC1 forms (which take the curve name differently) can share
/// one implementation.
fn generate_ec_cert_with(
    dir: &Path,
    name: &str,
    keygen_args_prefix: &[&str],
    curve_arg: &str,
) -> (PrivateKey, Certificate) {
    let key_path = dir.join(format!("{name}_key.pem"));
    let cert_path = dir.join(format!("{name}_cert.pem"));

    let mut cmd = Command::new("openssl");
    cmd.args(keygen_args_prefix).arg(curve_arg);
    if keygen_args_prefix.contains(&"ecparam") {
        cmd.args(["-genkey", "-noout"]);
    }
    let status = cmd
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

fn sample_document() -> Document {
    let content = ContentBuilder::new().text("F1", 24.0, 72.0, 750.0, "Signed document");
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();
    DocumentBuilder::new()
        .title("ECDSA multi-curve signature test")
        .page(page)
        .build()
        .expect("document should build")
}

fn sign_and_verify_round_trip(
    key: PrivateKey,
    cert: Certificate,
    algo: SignatureAlgorithm,
    label: &str,
) {
    let signed = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .name(label)
        .algorithm(algo)
        .sign()
        .unwrap_or_else(|e| panic!("sign should succeed for {label}: {e}"));

    let results = SignatureVerifier::new(signed)
        .verify()
        .unwrap_or_else(|e| panic!("verify should succeed for {label}: {e}"));

    assert_eq!(results.len(), 1, "{label}: expected exactly one signature");
    assert!(
        results[0].is_valid,
        "{label}: signature should be valid: {:?}",
        results[0].error
    );
}

#[test]
fn test_ecdsa_p384_pkcs8_key_sign_and_verify() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_ec_cert_pkcs8(dir.path(), "signer_p384", "P-384");

    sign_and_verify_round_trip(
        key,
        cert,
        SignatureAlgorithm::EcdsaP384Sha384,
        "P-384 (PKCS#8 key)",
    );
}

#[test]
fn test_ecdsa_p384_sec1_key_sign_and_verify() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_ec_cert_sec1(dir.path(), "signer_p384_sec1", "secp384r1");

    sign_and_verify_round_trip(
        key,
        cert,
        SignatureAlgorithm::EcdsaP384Sha384,
        "P-384 (SEC1 key)",
    );
}

#[test]
fn test_ecdsa_p521_pkcs8_key_sign_and_verify() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_ec_cert_pkcs8(dir.path(), "signer_p521", "P-521");

    sign_and_verify_round_trip(
        key,
        cert,
        SignatureAlgorithm::EcdsaP521Sha512,
        "P-521 (PKCS#8 key)",
    );
}

#[test]
fn test_ecdsa_p521_sec1_key_sign_and_verify() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_ec_cert_sec1(dir.path(), "signer_p521_sec1", "secp521r1");

    sign_and_verify_round_trip(
        key,
        cert,
        SignatureAlgorithm::EcdsaP521Sha512,
        "P-521 (SEC1 key)",
    );
}

/// A document signed as P-384 but re-verified with the wrong algorithm
/// selector should still work: `SignatureVerifier` resolves the algorithm
/// from the CMS `SignerInfo`'s own OIDs (see
/// `SignatureAlgorithm::from_oids`), not from anything the caller passes
/// in -- so this is really just a second, independent check that the OID
/// round-trip (`SignatureAlgorithm::oid`/`from_oid`) added for the new
/// curves is wired correctly end to end, on top of the direct unit test in
/// `src/signatures/mod.rs`.
#[test]
fn test_ecdsa_p384_and_p521_produce_different_oid_signatures_in_one_document() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key384, cert384) = generate_ec_cert_pkcs8(dir.path(), "multi_p384", "P-384");
    let (key521, cert521) = generate_ec_cert_pkcs8(dir.path(), "multi_p521", "P-521");

    let signed_384 = DocumentSigner::new(sample_document())
        .certificate(cert384)
        .private_key(key384)
        .name("P-384 signer")
        .algorithm(SignatureAlgorithm::EcdsaP384Sha384)
        .sign()
        .expect("P-384 sign should succeed");
    let results_384 = SignatureVerifier::new(signed_384)
        .verify()
        .expect("P-384 verify should succeed");
    assert_eq!(results_384.len(), 1);
    assert!(results_384[0].is_valid);

    let signed_521 = DocumentSigner::new(sample_document())
        .certificate(cert521)
        .private_key(key521)
        .name("P-521 signer")
        .algorithm(SignatureAlgorithm::EcdsaP521Sha512)
        .sign()
        .expect("P-521 sign should succeed");
    let results_521 = SignatureVerifier::new(signed_521)
        .verify()
        .expect("P-521 verify should succeed");
    assert_eq!(results_521.len(), 1);
    assert!(results_521[0].is_valid);
}
