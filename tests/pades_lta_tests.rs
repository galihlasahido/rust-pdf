//! Integration test for PAdES "B-LTA" (long-term archival) document
//! timestamps: [`rust_pdf::signatures::embed_document_timestamp`].
//!
//! Mirrors `tests/signature_verification_tests.rs`'s approach (shells out to
//! the system `openssl` binary for throwaway keys/certs and a fully offline
//! RFC 3161 TSA) but lives in its own file per this repo's convention of one
//! feature's tests per file, rather than appending to that shared file.

#![cfg(feature = "signatures")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use rust_pdf::error::SignatureError;
use rust_pdf::prelude::*;
use rust_pdf::signatures::{
    embed_document_security_store, embed_document_timestamp, Certificate, DocumentSigner, DssEntry,
    PadesLevel, PrivateKey, SignatureAlgorithm, SignatureConfig, SignatureResult,
    SignatureVerifier, TimestampAuthorityClient,
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

fn sample_document() -> Document {
    let content = ContentBuilder::new().text("F1", 24.0, 72.0, 750.0, "Signed document");
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();
    DocumentBuilder::new()
        .title("PAdES B-LTA test")
        .page(page)
        .build()
        .expect("build document")
}

/// A [`TimestampAuthorityClient`] backed by a fully offline `openssl ts`
/// responder (self-signed TSA cert with `extendedKeyUsage=timeStamping`),
/// so RFC 3161 timestamping can be tested without any network access.
/// (Duplicated from `signature_verification_tests.rs` rather than shared --
/// each integration test file is its own crate.)
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

        std::fs::read(&resp_path)
            .map_err(|e| SignatureError::TimestampError(format!("read tsr: {e}")))
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

    OpensslTsaClient {
        dir: dir.to_path_buf(),
        config_path,
        key_path,
        cert_path,
    }
}

/// Reads a DER `SEQUENCE` tag+length header to find where the real content
/// ends, trimming the trailing zero-byte `/Contents` placeholder padding
/// (mirrors `verifier.rs::trim_to_der_length`).
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

/// Extracts the *last* `/ByteRange [...]` and `/Contents <...>` pair found
/// in `pdf_bytes` (i.e. the most-recently-appended incremental update's --
/// here, the archive timestamp's `/DocTimeStamp` dictionary) as
/// `((offset1, length1, offset2, length2), timestamp_token_der)`.
fn extract_last_byte_range_and_contents(pdf_bytes: &[u8]) -> ((i64, i64, i64, i64), Vec<u8>) {
    let pat = b"/Contents <";
    let start = pdf_bytes
        .windows(pat.len())
        .rposition(|w| w == pat)
        .expect("no /Contents in PDF")
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
    let mut der = Vec::with_capacity(hex_str.len() / 2);
    let raw: Vec<u8> = hex_str.bytes().collect();
    for chunk in raw.chunks(2) {
        let s = std::str::from_utf8(chunk).unwrap();
        der.push(u8::from_str_radix(s, 16).unwrap());
    }
    let token_der = trim_der_length(&der);

    let br_pat = b"/ByteRange [";
    let br_start = pdf_bytes
        .windows(br_pat.len())
        .rposition(|w| w == br_pat)
        .expect("no /ByteRange in PDF")
        + br_pat.len();
    let br_end = br_start
        + pdf_bytes[br_start..]
            .iter()
            .position(|&b| b == b']')
            .expect("no closing ] for /ByteRange");
    let br_str = std::str::from_utf8(&pdf_bytes[br_start..br_end]).unwrap();
    let nums: Vec<i64> = br_str
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    assert_eq!(nums.len(), 4);

    ((nums[0], nums[1], nums[2], nums[3]), token_der)
}

/// Cross-verifies an RFC 3161 archive timestamp token against the system
/// `openssl ts -verify` (an independent, standards-based verifier), using
/// the throwaway TSA's own self-signed certificate as the trust anchor
/// (mirroring `signature_verification_tests.rs::assert_openssl_cms_verify`'s
/// "cross-verify with an independent tool" approach for ordinary
/// signatures). Confirms both that the token's `messageImprint` matches the
/// digest of `covered_bytes` and that the TSA's CMS signature over
/// `TSTInfo` validates.
fn assert_openssl_ts_verify(
    dir: &Path,
    tsa_cert_path: &Path,
    covered_bytes: &[u8],
    token_der: &[u8],
) {
    let content_path = dir.join("lta_content.bin");
    std::fs::write(&content_path, covered_bytes).unwrap();
    let token_path = dir.join("lta_token.der");
    std::fs::write(&token_path, token_der).unwrap();

    let digest_output = Command::new("openssl")
        .args(["dgst", "-sha256"])
        .arg(&content_path)
        .output()
        .expect("failed to run openssl dgst");
    assert!(digest_output.status.success());
    // Output looks like "SHA256(<path>)= <hex>\n".
    let stdout = String::from_utf8_lossy(&digest_output.stdout);
    let digest_hex = stdout
        .trim()
        .rsplit(' ')
        .next()
        .expect("openssl dgst output missing digest")
        .to_string();

    let output = Command::new("openssl")
        .args(["ts", "-verify", "-digest"])
        .arg(&digest_hex)
        .args(["-in"])
        .arg(&token_path)
        .args(["-token_in", "-CAfile"])
        .arg(tsa_cert_path)
        .output()
        .expect("failed to run openssl ts -verify");

    assert!(
        output.status.success(),
        "openssl ts -verify failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Full PAdES "B-LTA" chain: sign (B-T), embed a `/DSS`, then embed an
/// archive timestamp over the result -- and assert every step produces a
/// structurally valid incremental update that doesn't disturb what came
/// before it.
#[test]
fn test_pades_b_lta_full_chain_is_structurally_valid_incremental_update() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let (key, cert) = generate_rsa_cert(dir.path(), "signer1");
    let signing_tsa: Arc<dyn TimestampAuthorityClient> = Arc::new(setup_openssl_tsa(dir.path()));
    let archive_tsa_impl = setup_openssl_tsa(dir.path());
    let archive_tsa_cert_path = dir.path().join("archive_tsa_cert.pem");
    std::fs::copy(&archive_tsa_impl.cert_path, &archive_tsa_cert_path).unwrap();
    let archive_tsa: Arc<dyn TimestampAuthorityClient> = Arc::new(archive_tsa_impl);

    // 1. Sign at PAdES "B-T" (signature + RFC 3161 token over the signature
    //    value).
    let config = SignatureConfig::new()
        .name("B-LTA Signer")
        .pades_level(PadesLevel::T)
        .timestamp_authority(signing_tsa.clone())
        .algorithm(SignatureAlgorithm::RsaSha256);

    let signed = DocumentSigner::new(sample_document())
        .certificate(cert.clone())
        .private_key(key)
        .config(config)
        .sign()
        .expect("PAdES B-T sign should succeed");

    let baseline = SignatureVerifier::new(signed.clone()).verify().unwrap();
    assert_eq!(baseline.len(), 1);
    assert!(baseline[0].is_valid, "{:?}", baseline[0].error);

    // 2. Embed a `/DSS` (B-LT): here, just the signer's own certificate --
    //    this crate doesn't fetch real CRL/OCSP material (see `dss.rs`).
    let dss_entry = DssEntry {
        certs: vec![cert.der_bytes().to_vec()],
        ..Default::default()
    };
    let with_dss =
        embed_document_security_store(signed, &dss_entry).expect("DSS embedding should succeed");

    let text = String::from_utf8_lossy(&with_dss);
    assert!(
        text.contains("/DSS "),
        "catalog should reference the new /DSS"
    );

    // 3. Embed the archive timestamp (B-LTA) over the whole thing, including
    //    the DSS just added.
    let with_lta = embed_document_timestamp(with_dss, &archive_tsa, SignatureAlgorithm::RsaSha256)
        .expect("archive timestamp embedding should succeed");

    let text = String::from_utf8_lossy(&with_lta);
    assert!(
        text.contains("/Type /DocTimeStamp"),
        "must add a /Type /DocTimeStamp dictionary"
    );
    assert!(
        text.contains("/SubFilter /ETSI.RFC3161"),
        "the DocTimeStamp dictionary must use the ETSI.RFC3161 subfilter"
    );
    assert!(
        text.contains("/DSS "),
        "the earlier /DSS reference must survive the further incremental update"
    );

    // Appending the DSS and archive timestamp via incremental updates must
    // never touch the byte range the original signature covers -- it (and
    // its embedded RFC 3161 "B-T" token) must remain fully valid, same
    // append-only guarantee as a second `IncrementalSigner` pass.
    let after = SignatureVerifier::new(with_lta.clone())
        .verify()
        .expect("verify should succeed");
    assert_eq!(after.len(), 1);
    assert!(after[0].is_valid, "{:?}", after[0].error);
    let ts = after[0]
        .timestamp
        .as_ref()
        .expect("the original PAdES B-T timestamp token should still be present");
    assert!(
        ts.valid,
        "B-T timestamp should still validate: {:?}",
        ts.error
    );

    // Structural check on the newly-appended `/DocTimeStamp`'s `/ByteRange`:
    // it must cover the whole file except its own `/Contents` placeholder
    // (offset1 == 0, and the second range runs to the end of the file).
    let (byte_range, token_der) = extract_last_byte_range_and_contents(&with_lta);
    let (offset1, length1, offset2, length2) = byte_range;
    assert_eq!(
        offset1, 0,
        "ByteRange must start at the beginning of the file"
    );
    assert_eq!(
        offset2 + length2,
        with_lta.len() as i64,
        "ByteRange's second span must reach the end of the file"
    );
    assert!(
        length1 > 0 && length2 > 0,
        "both ByteRange spans must be non-empty"
    );

    // Cryptographically cross-verify the archive timestamp token against an
    // independent tool: its messageImprint must match the digest of the
    // exact bytes its own /ByteRange covers, and its CMS signature must
    // validate against the (self-signed, so also the trust anchor) TSA
    // certificate.
    let mut covered = Vec::new();
    covered.extend_from_slice(&with_lta[offset1 as usize..(offset1 + length1) as usize]);
    covered.extend_from_slice(&with_lta[offset2 as usize..(offset2 + length2) as usize]);

    assert_openssl_ts_verify(dir.path(), &archive_tsa_cert_path, &covered, &token_der);
}

/// [`embed_document_timestamp`] must reject a PDF it can't find a
/// `startxref`/page/AcroForm structure in, rather than panicking or
/// producing a malformed file -- exercised without needing a real TSA since
/// this fails before any request would be built.
#[test]
fn test_embed_document_timestamp_rejects_malformed_pdf() {
    #[derive(Debug)]
    struct UnusedTsaClient;
    impl TimestampAuthorityClient for UnusedTsaClient {
        fn timestamp(&self, _tsq_der: &[u8]) -> SignatureResult<Vec<u8>> {
            unreachable!("must not be called for a PDF that fails structural parsing")
        }
    }

    let client: Arc<dyn TimestampAuthorityClient> = Arc::new(UnusedTsaClient);
    let err = embed_document_timestamp(
        b"%PDF-1.7\n%%EOF".to_vec(),
        &client,
        SignatureAlgorithm::RsaSha256,
    )
    .expect_err("a PDF with no startxref/pages/AcroForm must be rejected");
    assert!(matches!(err, SignatureError::SigningFailed(_)));
}
