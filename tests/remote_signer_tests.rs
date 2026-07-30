//! Integration tests for the [`RemoteSigner`] trait (HSM/KMS-backed signing).
//!
//! These mirror the pattern used in `tests/signature_verification_tests.rs`:
//! shell out to the system `openssl` binary to generate a throwaway RSA
//! key/self-signed certificate, then drive `IncrementalSigner` through the
//! public API.
//!
//! The centerpiece ([`test_remote_signer_byte_identical_to_local_key`]) is
//! the strongest correctness proof available without a real HSM: a fake
//! `RemoteSigner` that just forwards to a local `PrivateKey` must produce
//! byte-for-byte identical signed-PDF output to signing with that same
//! `PrivateKey` directly, for the same base document/algorithm/key.

#![cfg(feature = "signatures")]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use rust_pdf::prelude::*;
use rust_pdf::signatures::{
    Certificate, IncrementalSigner, PrivateKey, RemoteSigner, SignatureAlgorithm, SignatureResult,
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

fn sample_pdf_bytes() -> Vec<u8> {
    let content = ContentBuilder::new().text("F1", 24.0, 72.0, 750.0, "Signed document");
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();
    let doc = DocumentBuilder::new()
        .title("Remote signer test")
        .page(page)
        .build()
        .expect("document should build");
    doc.save_to_bytes().expect("document should serialize")
}

/// A fake "remote" signing backend that simply wraps a local [`PrivateKey`]
/// -- i.e. it never actually leaves the process. This is not a stand-in for
/// a real HSM/KMS integration test (no such thing is available here); it
/// exists purely to prove the `RemoteSigner` plumbing in `signer.rs` and
/// `pkcs7.rs` routes bytes through identically to the local-key path, by
/// giving both paths the same underlying key and comparing output.
#[derive(Debug)]
struct FakeRemoteSigner {
    key: PrivateKey,
    cert: Certificate,
}

impl RemoteSigner for FakeRemoteSigner {
    fn sign_digest(
        &self,
        digest: &[u8],
        _algorithm: SignatureAlgorithm,
    ) -> SignatureResult<Vec<u8>> {
        // `PrivateKey::sign` also ignores its own configured algorithm
        // hint and just signs whatever bytes it's given (see
        // `PrivateKey::sign_rsa` et al.) -- mirroring that here is exactly
        // what makes this fake produce byte-identical output to the local
        // path for the same key and input.
        self.key.sign(digest)
    }

    fn certificate(&self) -> &Certificate {
        &self.cert
    }
}

#[test]
fn test_remote_signer_byte_identical_to_local_key() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "remote-signer");

    // Both signers start from the exact same base PDF bytes (built once)
    // so the only difference between the two runs is which signing path
    // produces the `/Contents` PKCS#7 signature.
    let base_pdf = sample_pdf_bytes();

    let signed_local = IncrementalSigner::new(base_pdf.clone())
        .certificate(cert.clone())
        .private_key(key.clone())
        .name("Remote Signer Test")
        .reason("Testing RemoteSigner")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("local-key sign should succeed");

    // Deliberately omit `.certificate(..)` here to also exercise the
    // "auto-populated from RemoteSigner" precedence rule documented on
    // `IncrementalSigner::remote_signer`.
    let remote = Arc::new(FakeRemoteSigner {
        key: key.clone(),
        cert: cert.clone(),
    });
    let signed_remote = IncrementalSigner::new(base_pdf)
        .remote_signer(remote)
        .name("Remote Signer Test")
        .reason("Testing RemoteSigner")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("remote-signer sign should succeed");

    assert_eq!(
        signed_local, signed_remote,
        "RemoteSigner path must produce byte-identical output to the local-key path \
         for the same document/algorithm/key"
    );

    // Belt-and-braces: also confirm the remote-signed PDF is independently
    // verifiable (not just bit-identical to something that happens to also
    // be broken).
    let results = SignatureVerifier::new(signed_remote)
        .verify()
        .expect("verify should succeed");
    assert_eq!(results.len(), 1);
    assert!(
        results[0].is_valid,
        "remote-signed signature should verify: {:?}",
        results[0].error
    );
    assert_eq!(
        results[0].signer_name.as_deref(),
        Some("Remote Signer Test")
    );
}

#[test]
fn test_remote_signer_explicit_certificate_overrides_remote_signer_certificate() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    // Two distinct certs for the same underlying key: one "reported" by the
    // RemoteSigner, one set explicitly via `.certificate(..)`. Per the
    // documented precedence rule, the explicit one must win.
    let (key, cert_from_remote) = generate_rsa_cert(dir.path(), "remote-cert");
    let (_unused_key, cert_explicit) = generate_rsa_cert(dir.path(), "explicit-cert");

    let base_pdf = sample_pdf_bytes();

    let remote = Arc::new(FakeRemoteSigner {
        key,
        cert: cert_from_remote,
    });

    let signed = IncrementalSigner::new(base_pdf)
        .certificate(cert_explicit.clone())
        .remote_signer(remote)
        .name("Precedence Test")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("sign should succeed");

    let results = SignatureVerifier::new(signed)
        .verify()
        .expect("verify should succeed");
    assert_eq!(results.len(), 1);
    let embedded_cert = results[0]
        .certificate
        .as_ref()
        .expect("signature should carry a certificate");
    assert_eq!(
        embedded_cert.serial_number(),
        cert_explicit.serial_number(),
        "explicit .certificate(..) call should take precedence over RemoteSigner::certificate()"
    );
}

#[test]
fn test_remote_signer_takes_precedence_over_private_key() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    // `wrong_key` stands in for a stale `.private_key(..)` call that should
    // be ignored once `.remote_signer(..)` is also set. No `.certificate(..)`
    // call at all here, so the certificate must also come from the remote
    // signer (not from `wrong_key`'s would-be certificate, which is never
    // even generated).
    let (wrong_key, _wrong_key_unused_cert) = generate_rsa_cert(dir.path(), "wrong");
    let (remote_key, remote_cert) = generate_rsa_cert(dir.path(), "remote");

    let base_pdf = sample_pdf_bytes();
    let remote = Arc::new(FakeRemoteSigner {
        key: remote_key,
        cert: remote_cert.clone(),
    });

    let signed = IncrementalSigner::new(base_pdf)
        .private_key(wrong_key)
        .remote_signer(remote)
        .name("Precedence Over Local Key")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("sign should succeed");

    let results = SignatureVerifier::new(signed)
        .verify()
        .expect("verify should succeed");
    assert_eq!(results.len(), 1);
    assert!(
        results[0].is_valid,
        "signature must validate against the remote signer's certificate/key, \
         not the stale local .certificate()/.private_key() values: {:?}",
        results[0].error
    );
    let embedded_cert = results[0]
        .certificate
        .as_ref()
        .expect("signature should carry a certificate");
    assert_eq!(embedded_cert.serial_number(), remote_cert.serial_number());
}
