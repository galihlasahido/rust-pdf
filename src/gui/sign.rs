//! Digital-signature signing dialog: collects a certificate/private key
//! (PEM files via `rfd`) and signer info, then runs [`crate::signatures::IncrementalSigner`]
//! on a background thread and writes the signed PDF to a chosen output
//! path. Wired into `app.rs` two ways: a toolbar "Sign Document…" button
//! (invisible signature) and `Tool::SignPlace`'s drag gesture (a visible
//! signature at the drawn rectangle).

use std::path::PathBuf;
use std::sync::mpsc;

use crate::signatures::{
    Certificate, IncrementalSigner, PadesLevel, PrivateKey, SignatureAlgorithm, SignatureConfig,
    VisibleSignature,
};

pub const ALGORITHMS: [SignatureAlgorithm; 4] = [
    SignatureAlgorithm::RsaSha256,
    SignatureAlgorithm::RsaSha384,
    SignatureAlgorithm::RsaSha512,
    SignatureAlgorithm::EcdsaP256Sha256,
];

/// `SignatureAlgorithm` isn't tied to the private key's own type anywhere
/// reachable from this module (`crate::signatures::certificate::KeyType`
/// isn't part of the public API) -- rather than guess, the dialog asks the
/// user to pick the algorithm matching their key (RSA vs ECDSA).
pub fn algorithm_label(algo: SignatureAlgorithm) -> &'static str {
    match algo {
        SignatureAlgorithm::RsaSha256 => "RSA + SHA-256",
        SignatureAlgorithm::RsaSha384 => "RSA + SHA-384",
        SignatureAlgorithm::RsaSha512 => "RSA + SHA-512",
        SignatureAlgorithm::EcdsaP256Sha256 => "ECDSA P-256 + SHA-256",
    }
}

/// State for the "Sign Document" dialog window.
pub struct SignDialogState {
    pub open: bool,
    pub cert_path: Option<PathBuf>,
    pub key_path: Option<PathBuf>,
    pub name: String,
    pub reason: String,
    pub location: String,
    pub contact_info: String,
    pub algorithm: SignatureAlgorithm,
    pub pades_b: bool,
    /// A rectangle (PDF user-space points) captured via `Tool::SignPlace`
    /// for where to draw the visible signature widget. `None` produces an
    /// invisible signature. Note: `IncrementalSigner` always draws this on
    /// the document's *first* page regardless of which page the rectangle
    /// was drawn on -- a current library limitation, surfaced to the user
    /// in the dialog rather than silently misplaced.
    pub visible_rect: Option<(f64, f64, f64, f64)>,
    pub signing: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    pub error: Option<String>,
}

impl Default for SignDialogState {
    fn default() -> Self {
        Self {
            open: false,
            cert_path: None,
            key_path: None,
            name: String::new(),
            reason: String::new(),
            location: String::new(),
            contact_info: String::new(),
            algorithm: SignatureAlgorithm::RsaSha256,
            pades_b: false,
            visible_rect: None,
            signing: None,
            error: None,
        }
    }
}

impl SignDialogState {
    pub fn is_signing(&self) -> bool {
        self.signing.is_some()
    }
}

/// Runs the actual signing off the UI thread: reads the cert/key PEM
/// files, builds the `IncrementalSigner`, signs, and writes the output PDF
/// -- exactly the blocking work `actions::spawn` exists for.
#[allow(clippy::too_many_arguments)]
pub fn sign_in_background(
    pdf_bytes: Vec<u8>,
    cert_path: PathBuf,
    key_path: PathBuf,
    name: String,
    reason: String,
    location: String,
    contact_info: String,
    algorithm: SignatureAlgorithm,
    pades_b: bool,
    visible_rect: Option<(f64, f64, f64, f64)>,
    output_path: PathBuf,
) -> Result<PathBuf, String> {
    let cert = Certificate::from_pem_file(&cert_path).map_err(|e| format!("Certificate: {e}"))?;
    let key = PrivateKey::from_pem_file(&key_path).map_err(|e| format!("Private key: {e}"))?;

    let mut config = SignatureConfig::new().algorithm(algorithm);
    if !name.trim().is_empty() {
        config = config.name(name);
    }
    if !reason.trim().is_empty() {
        config = config.reason(reason);
    }
    if !location.trim().is_empty() {
        config = config.location(location);
    }
    if !contact_info.trim().is_empty() {
        config = config.contact_info(contact_info);
    }
    if pades_b {
        config = config.pades_level(PadesLevel::B);
    }
    if let Some((llx, lly, urx, ury)) = visible_rect {
        let x = llx.min(urx) as f32;
        let y = lly.min(ury) as f32;
        let width = (urx - llx).abs() as f32;
        let height = (ury - lly).abs() as f32;
        config = config.visible(VisibleSignature::new(x, y, width, height));
    }

    let signed = IncrementalSigner::new(pdf_bytes)
        .certificate(cert)
        .private_key(key)
        .config(config)
        .sign()
        .map_err(|e| format!("Signing failed: {e}"))?;

    std::fs::write(&output_path, &signed)
        .map_err(|e| format!("Failed to write output file: {e}"))?;
    Ok(output_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    use crate::signatures::SignatureVerifier;

    fn openssl_available() -> bool {
        Command::new("openssl")
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Mirrors `tests/signature_verification_tests.rs`'s `generate_rsa_cert`
    /// helper: shells out to the system `openssl` for a throwaway
    /// self-signed RSA cert/key pair.
    fn generate_rsa_cert(dir: &std::path::Path) -> (PathBuf, PathBuf) {
        let key_path = dir.join("key.pem");
        let cert_path = dir.join("cert.pem");

        let status = Command::new("openssl")
            .args(["genrsa", "-out", key_path.to_str().unwrap(), "2048"])
            .status()
            .expect("failed to run openssl genrsa");
        assert!(status.success(), "openssl genrsa failed");

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
                "/CN=Test Signer/O=Test/C=US",
            ])
            .status()
            .expect("failed to run openssl req");
        assert!(status.success(), "openssl req failed");

        (key_path, cert_path)
    }

    #[test]
    fn sign_in_background_produces_a_verifiable_signature() {
        if !openssl_available() {
            eprintln!("skipping: openssl not available");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let (key_path, cert_path) = generate_rsa_cert(dir.path());
        let output_path = dir.path().join("signed.pdf");

        let pdf_bytes =
            std::fs::read("tests/output/multipage_report.pdf").expect("read fixture");

        let result = sign_in_background(
            pdf_bytes,
            cert_path,
            key_path,
            "GUI Test Signer".to_string(),
            "Testing the sign dialog".to_string(),
            String::new(),
            String::new(),
            SignatureAlgorithm::RsaSha256,
            false,
            None,
            output_path.clone(),
        );

        let written_path = result.expect("signing should succeed");
        assert_eq!(written_path, output_path);

        let signed_bytes = std::fs::read(&output_path).expect("read signed output");
        let verified = SignatureVerifier::new(signed_bytes)
            .verify()
            .expect("verify should not error");
        assert_eq!(verified.len(), 1);
        assert!(verified[0].is_valid, "signature should verify as valid");
        assert_eq!(verified[0].signer_name.as_deref(), Some("GUI Test Signer"));
    }
}
