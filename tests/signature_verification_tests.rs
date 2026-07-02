//! Integration tests for PDF digital signature verification.
//!
//! These tests shell out to the system `openssl` binary to generate
//! throwaway RSA/EC keys and self-signed certificates, mirroring the
//! pattern already used in `examples/digital_signature_example.rs`.

#![cfg(feature = "signatures")]

use std::path::{Path, PathBuf};
use std::process::Command;

use rust_pdf::error::SignatureError;
use rust_pdf::prelude::*;
use rust_pdf::signatures::{
    embed_document_security_store, Certificate, DocumentSigner, DssEntry, IncrementalSigner,
    PadesLevel, PrivateKey, SignatureAlgorithm, SignatureConfig, SignatureResult,
    SignatureVerifier, TimestampAuthorityClient, VisibleSignature,
};
use std::sync::Arc;
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

// --- PAdES / RFC 3161 / chain validation / visible appearance / DSS ---

/// Extracts the (trimmed to actual DER length) PKCS#7/CMS signature bytes
/// and the exact document bytes covered by `/ByteRange`, by scanning the
/// raw PDF the same way `signer.rs`/`verifier.rs` do internally. Used to
/// cross-check our own output against `openssl cms -verify`.
fn extract_signature_der_and_content(pdf_bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let pat = b"/Contents <";
    let start = pdf_bytes
        .windows(pat.len())
        .position(|w| w == pat)
        .expect("no /Contents in signed PDF")
        + pat.len();
    let end = start
        + pdf_bytes[start..]
            .iter()
            .position(|&b| b == b'>')
            .expect("no closing > for /Contents");
    let hex_bytes: Vec<u8> = pdf_bytes[start..end]
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let hex_str = std::str::from_utf8(&hex_bytes).unwrap();
    let mut sig_der = Vec::with_capacity(hex_str.len() / 2);
    let raw: Vec<u8> = hex_str.bytes().collect();
    for chunk in raw.chunks(2) {
        let s = std::str::from_utf8(chunk).unwrap();
        sig_der.push(u8::from_str_radix(s, 16).unwrap());
    }
    let trimmed = trim_der_length(&sig_der);

    let br_pat = b"/ByteRange [";
    let br_start = pdf_bytes
        .windows(br_pat.len())
        .position(|w| w == br_pat)
        .expect("no /ByteRange")
        + br_pat.len();
    let br_end = br_start
        + pdf_bytes[br_start..]
            .iter()
            .position(|&b| b == b']')
            .expect("no closing ] for /ByteRange");
    let br_str = std::str::from_utf8(&pdf_bytes[br_start..br_end]).unwrap();
    let nums: Vec<i64> = br_str.split_whitespace().map(|s| s.parse().unwrap()).collect();
    assert_eq!(nums.len(), 4);

    let mut content = Vec::new();
    content.extend_from_slice(&pdf_bytes[nums[0] as usize..(nums[0] + nums[1]) as usize]);
    content.extend_from_slice(&pdf_bytes[nums[2] as usize..(nums[2] + nums[3]) as usize]);

    (trimmed, content)
}

/// Reads a DER `SEQUENCE` tag+length header to find where the real content
/// ends, trimming the trailing zero-byte padding in the `/Contents`
/// placeholder (mirrors `verifier.rs::trim_to_der_length`).
fn trim_der_length(bytes: &[u8]) -> Vec<u8> {
    assert_eq!(bytes.first(), Some(&0x30), "expected a DER SEQUENCE");
    let first_len_byte = bytes[1];
    let (header_len, content_len) = if first_len_byte & 0x80 == 0 {
        (2usize, first_len_byte as usize)
    } else {
        let num_octets = (first_len_byte & 0x7F) as usize;
        let mut len = 0usize;
        for &b in &bytes[2..2 + num_octets] {
            len = (len << 8) | b as usize;
        }
        (2 + num_octets, len)
    };
    bytes[..header_len + content_len].to_vec()
}

/// Cross-verifies a signed PDF's embedded CMS signature against the system
/// `openssl cms -verify` (DoD requirement: signatures produced by this
/// crate must pass an independent, standards-based verifier). `-noverify`
/// skips CA trust-chain checking (the throwaway self-signed test certs
/// aren't in any trust store) but still fully verifies the CMS signature
/// value against the embedded certificate and the digest against the
/// supplied content.
fn assert_openssl_cms_verify(dir: &Path, pdf_bytes: &[u8], label: &str) {
    let (sig_der, content) = extract_signature_der_and_content(pdf_bytes);
    let sig_path = dir.join(format!("{label}_sig.der"));
    let content_path = dir.join(format!("{label}_content.bin"));
    std::fs::write(&sig_path, &sig_der).unwrap();
    std::fs::write(&content_path, &content).unwrap();

    // `-binary` is required: without it, `openssl cms -verify` applies
    // S/MIME text canonicalization (e.g. LF -> CRLF) to the detached
    // content before hashing it, which would corrupt the digest comparison
    // for arbitrary binary content like a PDF.
    let output = Command::new("openssl")
        .args(["cms", "-verify", "-binary", "-in"])
        .arg(&sig_path)
        .args(["-inform", "DER", "-content"])
        .arg(&content_path)
        .args(["-noverify", "-out"])
        .arg(dir.join(format!("{label}_out.bin")))
        .output()
        .expect("failed to run openssl cms -verify");

    assert!(
        output.status.success(),
        "openssl cms -verify failed for {label}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cross_verify_with_openssl_cms() {
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
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("sign should succeed");

    assert_openssl_cms_verify(dir.path(), &signed, "basic");
}

#[test]
fn test_pades_b_b_subfilter_and_cross_verify() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "signer1");

    let config = SignatureConfig::new()
        .name("PAdES Signer")
        .pades_level(PadesLevel::B)
        .algorithm(SignatureAlgorithm::RsaSha256);

    let signed = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .config(config)
        .sign()
        .expect("PAdES B-B sign should succeed");

    let text = String::from_utf8_lossy(&signed);
    assert!(
        text.contains("/SubFilter /ETSI.CAdES.detached"),
        "PadesLevel::B must use the ETSI.CAdES.detached SubFilter"
    );

    let results = SignatureVerifier::new(signed.clone()).verify().unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].is_valid, "{:?}", results[0].error);

    assert_openssl_cms_verify(dir.path(), &signed, "pades_b_b");
}

/// A [`TimestampAuthorityClient`] backed by a fully offline `openssl ts`
/// responder (self-signed TSA cert with `extendedKeyUsage=timeStamping`),
/// so RFC 3161 timestamping can be tested without any network access.
#[derive(Debug)]
struct OpensslTsaClient {
    dir: PathBuf,
    config_path: PathBuf,
    key_path: PathBuf,
    cert_path: PathBuf,
}

impl TimestampAuthorityClient for OpensslTsaClient {
    fn timestamp(&self, tsq_der: &[u8]) -> SignatureResult<Vec<u8>> {
        let req_path = self.dir.join("req.tsq");
        let resp_path = self.dir.join("resp.tsr");
        std::fs::write(&req_path, tsq_der)
            .map_err(|e| SignatureError::TimestampError(format!("write tsq: {e}")))?;

        let output = Command::new("openssl")
            .args(["ts", "-reply", "-config"])
            .arg(&self.config_path)
            .args(["-queryfile"])
            .arg(&req_path)
            .args(["-inkey"])
            .arg(&self.key_path)
            .args(["-signer"])
            .arg(&self.cert_path)
            .args(["-out"])
            .arg(&resp_path)
            .output()
            .map_err(|e| SignatureError::TimestampError(format!("spawn openssl ts: {e}")))?;

        if !output.status.success() {
            return Err(SignatureError::TimestampError(format!(
                "openssl ts -reply failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        std::fs::read(&resp_path).map_err(|e| SignatureError::TimestampError(format!("read tsr: {e}")))
    }
}

/// Sets up a throwaway, fully offline RFC 3161 TSA (self-signed cert, local
/// `openssl ts` config) inside `dir`.
fn setup_openssl_tsa(dir: &Path) -> OpensslTsaClient {
    let key_path = dir.join("tsa_key.pem");
    let cert_path = dir.join("tsa_cert.pem");

    let status = Command::new("openssl")
        .args(["genrsa", "-out", key_path.to_str().unwrap(), "2048"])
        .status()
        .expect("failed to run openssl genrsa for TSA key");
    assert!(status.success());

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
            "/CN=Test TSA/O=Test/C=US",
            "-addext",
            "extendedKeyUsage=critical,timeStamping",
        ])
        .status()
        .expect("failed to run openssl req for TSA cert");
    assert!(status.success());

    let serial_path = dir.join("tsaserial");
    std::fs::write(&serial_path, "01").unwrap();

    let config_path = dir.join("openssl_ts.cnf");
    let config = format!(
        "[ tsa ]\n\
         default_tsa = tsa_config1\n\
         [ tsa_config1 ]\n\
         dir = {dir}\n\
         serial = {serial}\n\
         crypto_device = builtin\n\
         signer_cert = {cert}\n\
         signer_key = {key}\n\
         signer_digest = sha256\n\
         default_policy = 1.2.3.4.5.6.7.8.1\n\
         digests = sha256,sha384,sha512\n\
         accuracy = secs:1\n\
         clock_precision_digits = 0\n\
         ordering = yes\n\
         tsa_name = yes\n\
         ess_cert_id_chain = no\n\
         ess_cert_id_alg = sha256\n",
        dir = dir.display(),
        serial = serial_path.display(),
        cert = cert_path.display(),
        key = key_path.display(),
    );
    std::fs::write(&config_path, config).unwrap();

    OpensslTsaClient { dir: dir.to_path_buf(), config_path, key_path, cert_path }
}

#[test]
fn test_pades_b_t_rfc3161_timestamp_end_to_end() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "signer1");
    let tsa = Arc::new(setup_openssl_tsa(dir.path()));

    let config = SignatureConfig::new()
        .name("B-T Signer")
        .pades_level(PadesLevel::T)
        .timestamp_authority(tsa)
        .algorithm(SignatureAlgorithm::RsaSha256);

    let signed = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .config(config)
        .sign()
        .expect("PAdES B-T sign (with a real, offline RFC 3161 TSA) should succeed");

    let results = SignatureVerifier::new(signed.clone()).verify().unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].is_valid, "{:?}", results[0].error);

    let ts = results[0]
        .timestamp
        .as_ref()
        .expect("PadesLevel::T signature should carry an embedded RFC 3161 timestamp token");
    assert!(ts.valid, "timestamp token should validate: {:?}", ts.error);
    assert!(ts.gen_time.is_some());
    assert!(ts.tsa_certificate.is_some());

    assert_openssl_cms_verify(dir.path(), &signed, "pades_b_t");
}

#[test]
fn test_pades_level_t_without_timestamp_authority_fails_fast() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "signer1");

    let config = SignatureConfig::new().pades_level(PadesLevel::T);
    let err = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .config(config)
        .sign()
        .expect_err("PadesLevel::T without a timestamp_authority must error, not silently downgrade");

    assert!(matches!(err, SignatureError::SigningFailed(_)));
}

/// Generates a root CA certificate and a leaf certificate signed by it
/// (rather than self-signed), for certificate-chain-validation tests.
/// Returns `(leaf_key, leaf_cert, root_cert)`.
fn generate_ca_signed_leaf(dir: &Path) -> (PrivateKey, Certificate, Certificate) {
    let root_key = dir.join("root_key.pem");
    let root_cert = dir.join("root_cert.pem");
    let status = Command::new("openssl")
        .args(["genrsa", "-out", root_key.to_str().unwrap(), "2048"])
        .status()
        .expect("openssl genrsa (root)");
    assert!(status.success());

    let status = Command::new("openssl")
        .args([
            "req",
            "-new",
            "-x509",
            "-key",
            root_key.to_str().unwrap(),
            "-out",
            root_cert.to_str().unwrap(),
            "-days",
            "3650",
            "-subj",
            "/CN=Test Root CA/O=Test/C=US",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        ])
        .status()
        .expect("openssl req -x509 (root)");
    assert!(status.success());

    let leaf_key = dir.join("leaf_key.pem");
    let leaf_csr = dir.join("leaf.csr");
    let leaf_cert = dir.join("leaf_cert.pem");

    let status = Command::new("openssl")
        .args(["genrsa", "-out", leaf_key.to_str().unwrap(), "2048"])
        .status()
        .expect("openssl genrsa (leaf)");
    assert!(status.success());

    let status = Command::new("openssl")
        .args([
            "req",
            "-new",
            "-key",
            leaf_key.to_str().unwrap(),
            "-out",
            leaf_csr.to_str().unwrap(),
            "-subj",
            "/CN=Leaf Signer/O=Test/C=US",
        ])
        .status()
        .expect("openssl req -new (leaf csr)");
    assert!(status.success());

    let status = Command::new("openssl")
        .args([
            "x509",
            "-req",
            "-in",
            leaf_csr.to_str().unwrap(),
            "-CA",
            root_cert.to_str().unwrap(),
            "-CAkey",
            root_key.to_str().unwrap(),
            "-CAcreateserial",
            "-out",
            leaf_cert.to_str().unwrap(),
            "-days",
            "365",
            "-sha256",
        ])
        .status()
        .expect("openssl x509 -req (sign leaf with root)");
    assert!(status.success());

    let key = PrivateKey::from_pem_file(&leaf_key).expect("load leaf key");
    let leaf = Certificate::from_pem_file(&leaf_cert).expect("load leaf cert");
    let root = Certificate::from_pem_file(&root_cert).expect("load root cert");
    (key, leaf, root)
}

#[test]
fn test_certificate_chain_validation_trusted_and_untrusted() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, leaf, root) = generate_ca_signed_leaf(dir.path());

    // The leaf's own chain certificate (the root) is intentionally *not*
    // embedded via `add_chain_certificate` -- this exercises resolving the
    // issuer purely from the verifier's own trust anchors, as a real
    // verifier's trust store would.
    let signed = DocumentSigner::new(sample_document())
        .certificate(leaf)
        .private_key(key)
        .name("Chain Signer")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("sign should succeed");

    let trusted_results = SignatureVerifier::new(signed.clone())
        .with_trust_anchors(vec![root.clone()])
        .verify()
        .expect("verify should succeed");
    assert_eq!(trusted_results.len(), 1);
    assert!(trusted_results[0].is_valid);
    let chain = trusted_results[0]
        .chain
        .as_ref()
        .expect("chain validation result should be present");
    assert!(chain.trusted, "chain should be trusted via the supplied root: {:?}", chain.error);
    assert_eq!(chain.chain.len(), 2, "chain should be [leaf, root]");

    // Without a trust anchor, the chain can't be validated as trusted, but
    // the signature is still cryptographically valid (these are
    // independent checks -- see `VerifiedSignature` docs).
    let untrusted_results = SignatureVerifier::new(signed.clone()).verify().expect("verify should succeed");
    assert!(untrusted_results[0].is_valid);
    assert!(!untrusted_results[0].chain.as_ref().unwrap().trusted);

    // With an unrelated trust anchor (a different, self-signed cert), the
    // chain must still not be reported as trusted.
    let (_unrelated_key, unrelated_cert) = generate_rsa_cert(dir.path(), "unrelated");
    let wrong_anchor_results = SignatureVerifier::new(signed)
        .with_trust_anchors(vec![unrelated_cert])
        .verify()
        .expect("verify should succeed");
    assert!(!wrong_anchor_results[0].chain.as_ref().unwrap().trusted);
}

#[test]
fn test_visible_signature_appearance() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "signer1");

    let config = SignatureConfig::new()
        .name("Visible Signer")
        .reason("Approval")
        .visible(VisibleSignature::new(50.0, 50.0, 200.0, 60.0))
        .algorithm(SignatureAlgorithm::RsaSha256);

    let signed = DocumentSigner::new(sample_document())
        .certificate(cert)
        .private_key(key)
        .config(config)
        .sign()
        .expect("visible signature sign should succeed");

    let text = String::from_utf8_lossy(&signed);
    assert!(
        text.contains("/Rect [50 50 250 110]"),
        "signature widget should use the configured visible rect"
    );
    assert!(text.contains("Digitally signed by"));
    assert!(text.contains("Visible Signer"));
    assert!(text.contains("/BaseFont /Helvetica"));

    let results = SignatureVerifier::new(signed.clone()).verify().unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].is_valid, "{:?}", results[0].error);

    assert_openssl_cms_verify(dir.path(), &signed, "visible");
}

#[test]
fn test_two_sequential_signatures_with_pades_timestamp_and_visible_appearance() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key1, cert1) = generate_rsa_cert(dir.path(), "signer1");
    let (key2, cert2) = generate_rsa_cert(dir.path(), "signer2");
    let tsa = Arc::new(setup_openssl_tsa(dir.path()));

    let config1 = SignatureConfig::new()
        .name("First Signer")
        .pades_level(PadesLevel::B)
        .visible(VisibleSignature::new(50.0, 50.0, 180.0, 50.0))
        .algorithm(SignatureAlgorithm::RsaSha256);

    let signed_once = DocumentSigner::new(sample_document())
        .certificate(cert1)
        .private_key(key1)
        .config(config1)
        .sign()
        .expect("first (PAdES B-B, visible) sign should succeed");

    let config2 = SignatureConfig::new()
        .name("Second Signer")
        .pades_level(PadesLevel::T)
        .timestamp_authority(tsa)
        .visible(VisibleSignature::new(250.0, 50.0, 180.0, 50.0))
        .algorithm(SignatureAlgorithm::RsaSha256);

    let signed_twice = IncrementalSigner::new(signed_once)
        .certificate(cert2)
        .private_key(key2)
        .config(config2)
        .sign()
        .expect("second (PAdES B-T, visible) incremental sign should succeed");

    let results = SignatureVerifier::new(signed_twice).verify().expect("verify should succeed");
    assert_eq!(results.len(), 2, "both signatures must survive the incremental update");
    for sig in &results {
        assert!(sig.is_valid, "signature should be valid: {:?} (signer {:?})", sig.error, sig.signer_name);
    }

    let names: Vec<Option<String>> = results.iter().map(|s| s.signer_name.clone()).collect();
    assert!(names.contains(&Some("First Signer".to_string())));
    assert!(names.contains(&Some("Second Signer".to_string())));

    // Only the second signature requested a timestamp.
    let with_timestamp = results.iter().filter(|s| s.timestamp.is_some()).count();
    assert_eq!(with_timestamp, 1);
    let ts = results.iter().find_map(|s| s.timestamp.as_ref()).unwrap();
    assert!(ts.valid, "{:?}", ts.error);
}

#[test]
fn test_dss_embedding_preserves_signature_validity() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "signer1");

    let signed = DocumentSigner::new(sample_document())
        .certificate(cert.clone())
        .private_key(key)
        .name("DSS Signer")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()
        .expect("sign should succeed");

    let baseline = SignatureVerifier::new(signed.clone()).verify().unwrap();
    assert_eq!(baseline.len(), 1);
    assert!(baseline[0].is_valid);

    let entry = DssEntry {
        certs: vec![cert.der_bytes().to_vec()],
        ..Default::default()
    };
    let with_dss = embed_document_security_store(signed, &entry).expect("DSS embedding should succeed");

    let text = String::from_utf8_lossy(&with_dss);
    assert!(text.contains("/DSS "), "catalog should reference the new /DSS");

    // Appending a DSS via incremental update must not touch the byte range
    // the existing signature covers -- it must remain valid, same
    // "append-only, never invalidates earlier signatures" guarantee as a
    // second `IncrementalSigner` pass.
    let after = SignatureVerifier::new(with_dss).verify().expect("verify should succeed");
    assert_eq!(after.len(), 1);
    assert!(after[0].is_valid, "{:?}", after[0].error);
}
