//! Demonstrates the signature capabilities beyond the basic RSA flow in
//! `digital_signature_example.rs`: ECDSA on curves other than P-256,
//! DocMDP certification signatures, and signing via the `RemoteSigner`
//! trait (the hook an HSM/KMS integration plugs into instead of handing
//! this crate raw key material).
//!
//! OCSP request-building, CRL revocation checks, and PAdES B-LTA archive
//! timestamps aren't demonstrated end-to-end here since they need a real
//! (or fully-offline-simulated) OCSP responder/CRL distribution point/TSA
//! -- see `src/signatures/ocsp.rs`/`crl.rs`'s own module docs for the
//! request/response codec API, and `tests/pades_lta_tests.rs` for a
//! complete offline-TSA-backed B-LTA example.
//!
//! Run with:
//! ```text
//! cargo run --features signatures --example advanced_signatures_demo
//! ```

use rust_pdf::prelude::*;
use std::process::Command;

#[cfg(feature = "signatures")]
use rust_pdf::signatures::{
    Certificate, CertificationLevel, IncrementalSigner, PrivateKey, RemoteSigner,
    SignatureAlgorithm, SignatureConfig, SignatureResult, SignatureVerifier,
};

const OUTPUT_DIR: &str = "tests/output";

#[cfg(feature = "signatures")]
fn generate_rsa_cert(name: &str) -> Result<(String, String), Box<dyn std::error::Error>> {
    let key_path = format!("{OUTPUT_DIR}/{name}_key.pem");
    let cert_path = format!("{OUTPUT_DIR}/{name}_cert.pem");
    Command::new("openssl")
        .args(["genrsa", "-out", &key_path, "2048"])
        .status()?;
    Command::new("openssl")
        .args([
            "req", "-new", "-x509", "-key", &key_path, "-out", &cert_path, "-days", "365",
            "-subj", &format!("/CN={name}/O=rust-pdf demo/C=US"),
        ])
        .status()?;
    Ok((key_path, cert_path))
}

#[cfg(feature = "signatures")]
fn generate_ec_cert(name: &str, curve: &str) -> Result<(String, String), Box<dyn std::error::Error>> {
    let key_path = format!("{OUTPUT_DIR}/{name}_key.pem");
    let cert_path = format!("{OUTPUT_DIR}/{name}_cert.pem");
    Command::new("openssl")
        .args([
            "genpkey", "-algorithm", "EC", "-pkeyopt", &format!("ec_paramgen_curve:{curve}"),
            "-out", &key_path,
        ])
        .status()?;
    Command::new("openssl")
        .args([
            "req", "-new", "-x509", "-key", &key_path, "-out", &cert_path, "-days", "365",
            "-subj", &format!("/CN={name}/O=rust-pdf demo/C=US"),
        ])
        .status()?;
    Ok((key_path, cert_path))
}

#[cfg(feature = "signatures")]
fn sample_pdf_bytes(title: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(ContentBuilder::new().text("F1", 14.0, 72.0, 760.0, title))
        .build();
    Ok(DocumentBuilder::new().title(title).page(page).build()?.save_to_bytes()?)
}

/// A fake "remote" signer that just wraps a local key in-process -- not a
/// stand-in for a real HSM/KMS integration test, only proof of the trait's
/// shape: an HSM/KMS client implements `sign_digest` by calling out to the
/// remote service instead, and the private key material never has to enter
/// this process at all.
#[cfg(feature = "signatures")]
#[derive(Debug)]
struct FakeRemoteSigner {
    key: PrivateKey,
    cert: Certificate,
}

#[cfg(feature = "signatures")]
impl RemoteSigner for FakeRemoteSigner {
    fn sign_digest(&self, digest: &[u8], _algorithm: SignatureAlgorithm) -> SignatureResult<Vec<u8>> {
        self.key.sign(digest)
    }

    fn certificate(&self) -> &Certificate {
        &self.cert
    }
}

#[cfg(feature = "signatures")]
fn ecdsa_p384_demo() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n--- ECDSA P-384 ---");
    let (key_path, cert_path) = generate_ec_cert("ecdsa_p384", "P-384")?;
    let key = PrivateKey::from_pem_file(&key_path)?;
    let cert = Certificate::from_pem_file(&cert_path)?;

    let signed = IncrementalSigner::new(sample_pdf_bytes("ECDSA P-384 signed")?)
        .certificate(cert)
        .private_key(key)
        .name("ECDSA P-384 Signer")
        .algorithm(SignatureAlgorithm::EcdsaP384Sha384)
        .sign()?;

    let results = SignatureVerifier::new(signed.clone()).verify()?;
    assert_eq!(results.len(), 1);
    assert!(results[0].is_valid);
    println!("signed + verified with EcdsaP384Sha384: is_valid = {}", results[0].is_valid);

    std::fs::write(format!("{OUTPUT_DIR}/signed_ecdsa_p384.pdf"), signed)?;
    Ok(())
}

#[cfg(feature = "signatures")]
fn docmdp_certification_demo() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n--- DocMDP certification signature ---");
    let (key_path, cert_path) = generate_rsa_cert("certifier")?;
    let key = PrivateKey::from_pem_file(&key_path)?;
    let cert = Certificate::from_pem_file(&cert_path)?;

    let config = SignatureConfig::new()
        .name("Certifying Authority")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .certify(CertificationLevel::FormFillingAnnotationsAndSigning);

    let certified = IncrementalSigner::new(sample_pdf_bytes("Certified document")?)
        .certificate(cert)
        .private_key(key)
        .config(config)
        .sign()?;

    let results = SignatureVerifier::new(certified.clone()).verify()?;
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].certification_level,
        Some(CertificationLevel::FormFillingAnnotationsAndSigning)
    );
    println!(
        "certified with P=3 (form-filling + annotations + signing): certification_level = {:?}",
        results[0].certification_level
    );

    std::fs::write(format!("{OUTPUT_DIR}/signed_certified.pdf"), certified)?;
    Ok(())
}

#[cfg(feature = "signatures")]
fn remote_signer_demo() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n--- RemoteSigner (HSM/KMS hook) ---");
    let (key_path, cert_path) = generate_rsa_cert("remote_signer")?;
    let key = PrivateKey::from_pem_file(&key_path)?;
    let cert = Certificate::from_pem_file(&cert_path)?;
    let remote = std::sync::Arc::new(FakeRemoteSigner { key, cert });

    // No `.private_key(..)` call at all -- the signing key never has to be
    // loaded into this process's own memory as a `PrivateKey`, only
    // `sign_digest` calls (which `FakeRemoteSigner` here just forwards
    // locally, but a real HSM/KMS client would send over the wire) do.
    let signed = IncrementalSigner::new(sample_pdf_bytes("Remote-signed document")?)
        .remote_signer(remote)
        .name("Remote Signer")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()?;

    let results = SignatureVerifier::new(signed.clone()).verify()?;
    assert_eq!(results.len(), 1);
    assert!(results[0].is_valid);
    println!("signed via RemoteSigner (no local .private_key() call): is_valid = {}", results[0].is_valid);

    std::fs::write(format!("{OUTPUT_DIR}/signed_remote.pdf"), signed)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(not(feature = "signatures"))]
    {
        println!("This example requires the 'signatures' feature.");
        println!("Run with: cargo run --features signatures --example advanced_signatures_demo");
    }

    #[cfg(feature = "signatures")]
    {
        std::fs::create_dir_all(OUTPUT_DIR)?;
        let openssl_available = Command::new("openssl")
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !openssl_available {
            println!("This example shells out to the system `openssl` binary; none was found.");
            return Ok(());
        }

        ecdsa_p384_demo()?;
        docmdp_certification_demo()?;
        remote_signer_demo()?;
    }

    Ok(())
}
