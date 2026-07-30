//! Verification of PDF digital signatures.
//!
//! This module checks signatures produced by [`super::DocumentSigner`] and
//! [`super::IncrementalSigner`] (and, best-effort, other PKCS#7/CMS detached
//! signatures using the same `Adobe.PPKLite`/`adbe.pkcs7.detached` or
//! `ETSI.CAdES.detached` shape).
//!
//! [`VerifiedSignature::is_valid`] only establishes that a signature is
//! *cryptographically self-consistent* — the signed bytes match the
//! embedded digest (so this detects any modification of the document made
//! after signing) and the embedded certificate's public key validates the
//! signature. It does **not** by itself imply the certificate is trusted.
//!
//! Two additional, separately-reported checks build on top of that:
//! - certificate chain / root-of-trust validation, via
//!   [`SignatureVerifier::with_trust_anchors`] and
//!   [`VerifiedSignature::chain`] (see [`crate::signatures::validate_chain`]
//!   for what this does and does not check -- notably, no revocation
//!   checking),
//! - RFC 3161 timestamp validation, via [`VerifiedSignature::timestamp`].

use std::fs;
use std::path::Path;

use cms::cert::CertificateChoices;
use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier};
use der::asn1::OctetString;
use der::{Decode, Encode};

use super::chain::{self, ChainValidationResult};
use super::config::CertificationLevel;
use super::timestamp;
use super::{digest_for_algorithm, ByteRange, Certificate, SignatureAlgorithm, SignatureResult};
use crate::error::SignatureError;

/// Verifies digital signatures embedded in a PDF document.
#[derive(Debug)]
pub struct SignatureVerifier {
    pdf_bytes: Vec<u8>,
    trust_anchors: Vec<Certificate>,
}

impl SignatureVerifier {
    /// Creates a verifier for the given PDF bytes.
    pub fn new(pdf_bytes: Vec<u8>) -> Self {
        Self {
            pdf_bytes,
            trust_anchors: Vec::new(),
        }
    }

    /// Creates a verifier by reading a PDF file from disk.
    pub fn from_file(path: impl AsRef<Path>) -> SignatureResult<Self> {
        let pdf_bytes = fs::read(path.as_ref()).map_err(|e| {
            SignatureError::VerificationFailed(format!("Failed to read file: {}", e))
        })?;
        Ok(Self::new(pdf_bytes))
    }

    /// Sets the trust anchors (root/intermediate CA certificates) used for
    /// certificate chain validation (see
    /// [`crate::signatures::validate_chain`]). Without this,
    /// [`VerifiedSignature::chain`] still reports the chain
    /// it was able to build from the certificates embedded in each
    /// signature, but `trusted` is always `false`.
    pub fn with_trust_anchors(mut self, trust_anchors: Vec<Certificate>) -> Self {
        self.trust_anchors = trust_anchors;
        self
    }

    /// Finds and verifies every signature present in the PDF.
    ///
    /// Returns one [`VerifiedSignature`] per signature found, in the order
    /// they appear in the file. A problem with an individual signature is
    /// reported via that signature's `error`/`is_valid` fields rather than
    /// aborting the whole scan — a PDF can contain several signatures where
    /// only one is broken. An empty PDF (no signatures) returns `Ok(vec![])`.
    pub fn verify(&self) -> SignatureResult<Vec<VerifiedSignature>> {
        Ok(find_signature_objects(&self.pdf_bytes)
            .into_iter()
            .map(|raw| verify_one(&self.pdf_bytes, raw, &self.trust_anchors))
            .collect())
    }
}

/// The result of verifying a single signature found in a PDF.
#[derive(Debug, Clone)]
pub struct VerifiedSignature {
    /// The signer's name, if present.
    pub signer_name: Option<String>,
    /// The stated reason for signing, if present.
    pub reason: Option<String>,
    /// The stated location of signing, if present.
    pub location: Option<String>,
    /// The signing time as embedded in the PDF, if present. This is
    /// asserted by the signer's own clock and is **not** trustworthy on its
    /// own -- prefer `timestamp` (an RFC 3161 TSA-asserted time) when
    /// present.
    pub signing_time: Option<String>,
    /// The byte range covered by this signature.
    pub byte_range: ByteRange,
    /// The signer's certificate, if it could be extracted from the signature.
    pub certificate: Option<Certificate>,
    /// Whether the signature is cryptographically valid: the covered bytes
    /// match the embedded digest, and the embedded certificate's public key
    /// validates the signature. Does not imply the certificate is trusted.
    ///
    /// If this is `false` because the covered bytes don't match the
    /// embedded digest, that specifically means the document was modified
    /// after this signature was applied.
    pub is_valid: bool,
    /// Whether the certificate's validity period covers the current time.
    /// `None` if this could not be determined. Informational only — not
    /// folded into `is_valid`.
    pub certificate_valid_now: Option<bool>,
    /// Certificate chain validation result (see
    /// [`SignatureVerifier::with_trust_anchors`] and
    /// [`crate::signatures::validate_chain`]).
    /// `None` if the CMS `certificates` set couldn't be read at all (in
    /// which case `error`/`is_valid` already reflect that failure).
    pub chain: Option<ChainValidationResult>,
    /// RFC 3161 timestamp validation result, if the signature carries a
    /// `id-aa-signatureTimeStampToken` unsigned attribute (PAdES "B-T").
    /// `None` if there is no timestamp token at all (not an error --most
    /// signatures won't have one).
    pub timestamp: Option<TimestampVerification>,
    /// `Some(level)` if this signature dictionary carries a DocMDP
    /// `/Reference` entry (ISO 32000-1 12.8.2.2) declaring it a
    /// *certification signature* at that [`CertificationLevel`]; `None` for
    /// an ordinary approval signature. This reflects only what is declared
    /// in the signature dictionary itself -- it is **not** cross-checked
    /// against the catalog's `/Perms /DocMDP` entry (which should point
    /// back at this same signature, see [`super::SignatureConfig::certification`])
    /// or against whether this is actually the first signature in the
    /// document (a malformed or adversarially crafted PDF could declare
    /// `/TransformMethod /DocMDP` on a later signature). Callers that need
    /// the full DocMDP trust story -- "is this genuinely the certifying
    /// signature, and were the permitted-by-`P` modifications the only ones
    /// made after it" -- must additionally check this is the *first*
    /// signature found in [`SignatureVerifier::verify`]'s returned `Vec`
    /// and reason about `byte_range` coverage themselves; this crate does
    /// not automate that cross-check.
    pub certification_level: Option<CertificationLevel>,
    /// Explains why `is_valid` is `false`, if applicable.
    pub error: Option<String>,
}

/// The result of validating an embedded RFC 3161 timestamp token (see
/// [`super::timestamp::verify_token`]).
#[derive(Debug, Clone)]
pub struct TimestampVerification {
    /// The TSA-asserted time the signature value existed
    /// (`TSTInfo.genTime`).
    pub gen_time: Option<String>,
    /// The TSA's certificate, if present in the token.
    pub tsa_certificate: Option<Certificate>,
    /// Whether the token's signature validates and its message imprint
    /// matches this signature's `SignatureValue`.
    pub valid: bool,
    /// Explains why `valid` is `false`, if applicable.
    pub error: Option<String>,
}

/// A signature dictionary located in the raw PDF bytes, with its fields
/// extracted but not yet cryptographically checked.
struct RawSignature {
    byte_range: ByteRange,
    contents_hex: Vec<u8>,
    name: Option<String>,
    reason: Option<String>,
    location: Option<String>,
    signing_time: Option<String>,
    certification_level: Option<CertificationLevel>,
}

/// Scans raw PDF bytes for `/Type /Sig` dictionaries and extracts their
/// relevant fields.
fn find_signature_objects(pdf_bytes: &[u8]) -> Vec<RawSignature> {
    const TYPE_SIG: &[u8] = b"/Type /Sig";
    const OBJ_START: &[u8] = b" 0 obj";
    const ENDOBJ: &[u8] = b"endobj";

    let mut results = Vec::new();
    let mut search_from = 0usize;

    while let Some(rel_pos) = find_bytes(&pdf_bytes[search_from..], TYPE_SIG) {
        let match_pos = search_from + rel_pos;

        let dict_start = pdf_bytes[..match_pos]
            .windows(OBJ_START.len())
            .rposition(|w| w == OBJ_START)
            .map(|p| p + OBJ_START.len())
            .unwrap_or(0);

        let dict_end = find_bytes(&pdf_bytes[match_pos..], ENDOBJ)
            .map(|p| match_pos + p)
            .unwrap_or(pdf_bytes.len());

        let slice = &pdf_bytes[dict_start..dict_end];

        if let (Some(byte_range), Some(contents_hex)) =
            (parse_byte_range(slice), parse_hex_contents(slice))
        {
            results.push(RawSignature {
                byte_range,
                contents_hex,
                name: parse_string_field(slice, b"/Name ("),
                reason: parse_string_field(slice, b"/Reason ("),
                location: parse_string_field(slice, b"/Location ("),
                signing_time: parse_string_field(slice, b"/M ("),
                certification_level: parse_certification_level(slice),
            });
        }

        search_from = match_pos + TYPE_SIG.len();
    }

    results
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_byte_range(slice: &[u8]) -> Option<ByteRange> {
    const PATTERN: &[u8] = b"/ByteRange [";
    let pos = find_bytes(slice, PATTERN)?;
    let start = pos + PATTERN.len();
    let end = start + slice[start..].iter().position(|&b| b == b']')?;
    let text = std::str::from_utf8(&slice[start..end]).ok()?;
    let nums: Vec<i64> = text
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    if nums.len() != 4 {
        return None;
    }
    Some(ByteRange::new(nums[0], nums[1], nums[2], nums[3]))
}

/// Extracts a DocMDP [`CertificationLevel`] from a signature dictionary's
/// raw bytes, if it declares one via a `/Reference [ << /TransformMethod
/// /DocMDP ... /TransformParams << ... /P <n> ... >> >> ]` entry (see
/// `signer.rs::build_docmdp_reference`, the writer side this mirrors). Not a
/// real PDF object parser -- like the rest of this module's `parse_*`
/// helpers, it locates `/TransformMethod /DocMDP` by literal byte search
/// and then reads the next `/P <digits>` token, bounded to the text before
/// the `/Reference` array's closing `]` so an unrelated later `/P` entry
/// elsewhere in the dictionary can't be misread as the DocMDP permission.
fn parse_certification_level(slice: &[u8]) -> Option<CertificationLevel> {
    const TRANSFORM_METHOD_DOCMDP: &[u8] = b"/TransformMethod /DocMDP";
    let method_pos = find_bytes(slice, TRANSFORM_METHOD_DOCMDP)?;
    let after_method = &slice[method_pos + TRANSFORM_METHOD_DOCMDP.len()..];

    let scope_end = find_bytes(after_method, b"]").unwrap_or(after_method.len());
    let scoped = &after_method[..scope_end];

    const P_KEY: &[u8] = b"/P ";
    let p_pos = find_bytes(scoped, P_KEY)?;
    let after_p = &scoped[p_pos + P_KEY.len()..];

    let digits: Vec<u8> = after_p
        .iter()
        .copied()
        .take_while(|b| b.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    let p_value: i64 = std::str::from_utf8(&digits).ok()?.parse().ok()?;
    CertificationLevel::from_p_value(p_value)
}

fn parse_hex_contents(slice: &[u8]) -> Option<Vec<u8>> {
    const PATTERN: &[u8] = b"/Contents <";
    let pos = find_bytes(slice, PATTERN)?;
    let start = pos + PATTERN.len();
    let end = start + slice[start..].iter().position(|&b| b == b'>')?;
    let hex_bytes: Vec<u8> = slice[start..end]
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    hex_decode(&hex_bytes)
}

fn hex_decode(hex: &[u8]) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.chunks(2) {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

/// Parses a `/Key (value)` field, handling the `\(`, `\)`, `\\` escapes
/// that `signer.rs::escape_pdf_string` produces.
fn parse_string_field(slice: &[u8], key_with_paren: &[u8]) -> Option<String> {
    let pos = find_bytes(slice, key_with_paren)?;
    let start = pos + key_with_paren.len();
    let mut i = start;
    while i < slice.len() {
        match slice[i] {
            b'\\' => i += 2,
            b')' => break,
            _ => i += 1,
        }
    }
    if i >= slice.len() {
        return None;
    }
    Some(unescape_pdf_string(&slice[start..i]))
}

fn unescape_pdf_string(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\\' && i + 1 < raw.len() && matches!(raw[i + 1], b'(' | b')' | b'\\') {
            out.push(raw[i + 1]);
            i += 2;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Extracts the signed bytes covered by `byte_range`, mirroring
/// `DocumentSigner::extract_signed_data`.
fn extract_signed_bytes(pdf_bytes: &[u8], byte_range: &ByteRange) -> Vec<u8> {
    let mut data = Vec::new();

    let start1 = byte_range.offset1.max(0) as usize;
    let end1 = start1.saturating_add(byte_range.length1.max(0) as usize);
    if end1 <= pdf_bytes.len() {
        data.extend_from_slice(&pdf_bytes[start1..end1]);
    }

    let start2 = byte_range.offset2.max(0) as usize;
    let end2 = start2.saturating_add(byte_range.length2.max(0) as usize);
    if end2 <= pdf_bytes.len() {
        data.extend_from_slice(&pdf_bytes[start2..end2]);
    }

    data
}

/// The `/Contents` hex field is a fixed-size placeholder padded with
/// trailing zero bytes past the actual PKCS#7 DER content (see
/// `signer.rs::embed_signature`/`SIGNATURE_SIZE`). This reads the outer
/// `SEQUENCE` tag+length header to find where the real content ends.
fn trim_to_der_length(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < 2 || bytes[0] != 0x30 {
        return None;
    }
    let first_len_byte = bytes[1];
    let (header_len, content_len) = if first_len_byte & 0x80 == 0 {
        (2usize, first_len_byte as usize)
    } else {
        let num_octets = (first_len_byte & 0x7F) as usize;
        if num_octets == 0 || num_octets > 8 {
            return None;
        }
        let len_bytes = bytes.get(2..2 + num_octets)?;
        let mut len: usize = 0;
        for &b in len_bytes {
            len = (len << 8) | b as usize;
        }
        (2 + num_octets, len)
    };

    let total = header_len.checked_add(content_len)?;
    bytes.get(..total)
}

fn verify_one(
    pdf_bytes: &[u8],
    raw: RawSignature,
    trust_anchors: &[Certificate],
) -> VerifiedSignature {
    let mut result = VerifiedSignature {
        signer_name: raw.name.clone(),
        reason: raw.reason.clone(),
        location: raw.location.clone(),
        signing_time: raw.signing_time.clone(),
        byte_range: raw.byte_range,
        certificate: None,
        is_valid: false,
        certificate_valid_now: None,
        chain: None,
        timestamp: None,
        certification_level: raw.certification_level,
        error: None,
    };

    match verify_cryptographically(pdf_bytes, &raw, trust_anchors) {
        Ok(outcome) => {
            result.certificate = outcome.certificate;
            result.certificate_valid_now = outcome.certificate_valid_now;
            result.is_valid = outcome.is_valid;
            result.chain = outcome.chain;
            result.timestamp = outcome.timestamp;
            result.error = outcome.error;
        }
        Err(e) => {
            result.error = Some(e);
        }
    }

    result
}

struct CryptoOutcome {
    certificate: Option<Certificate>,
    certificate_valid_now: Option<bool>,
    is_valid: bool,
    chain: Option<ChainValidationResult>,
    timestamp: Option<TimestampVerification>,
    error: Option<String>,
}

fn verify_cryptographically(
    pdf_bytes: &[u8],
    raw: &RawSignature,
    trust_anchors: &[Certificate],
) -> Result<CryptoOutcome, String> {
    let der_bytes = trim_to_der_length(&raw.contents_hex)
        .ok_or_else(|| "Could not determine PKCS#7 signature length".to_string())?;

    let content_info = ContentInfo::from_der(der_bytes)
        .map_err(|e| format!("Failed to parse PKCS#7 ContentInfo: {e}"))?;
    let signed_data: SignedData = content_info
        .content
        .decode_as()
        .map_err(|e| format!("Failed to parse CMS SignedData: {e}"))?;

    let signer_info = signed_data
        .signer_infos
        .0
        .iter()
        .next()
        .ok_or_else(|| "CMS SignedData has no SignerInfo".to_string())?;

    // Some CMS producers (e.g. several RFC 3161 TSAs, and some CMS
    // implementations in general) put the bare key-type OID (e.g.
    // `rsaEncryption`) in `signatureAlgorithm` rather than the combined
    // "hash-with-signature" OID, relying on the separate `digestAlgorithm`
    // field for the hash (RFC 5652 §5.3 permits both). `from_oids` resolves
    // either convention so we can verify externally-produced CMS, not just
    // our own `Pkcs7Builder` output.
    let algo = SignatureAlgorithm::from_oids(
        &signer_info.signature_algorithm.oid.to_string(),
        &signer_info.digest_alg.oid.to_string(),
    )
    .ok_or_else(|| {
        format!(
            "Unsupported signature algorithm: {} (digest {})",
            signer_info.signature_algorithm.oid, signer_info.digest_alg.oid
        )
    })?;

    let signed_attrs = signer_info
        .signed_attrs
        .as_ref()
        .ok_or_else(|| "Signature has no signed attributes (unsupported)".to_string())?;

    let message_digest_oid = const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
    let message_digest_attr = signed_attrs
        .iter()
        .find(|attr| attr.oid == message_digest_oid)
        .ok_or_else(|| "Missing messageDigest signed attribute".to_string())?;

    let digest_value: OctetString = message_digest_attr
        .values
        .get(0)
        .ok_or_else(|| "messageDigest attribute has no value".to_string())?
        .clone()
        .decode_as()
        .map_err(|e| format!("Invalid messageDigest attribute: {e}"))?;

    let signed_bytes = extract_signed_bytes(pdf_bytes, &raw.byte_range);
    let computed_digest = digest_for_algorithm(algo, &signed_bytes);
    let digest_matches = computed_digest == digest_value.as_bytes();

    let signed_attrs_der = signed_attrs
        .to_der()
        .map_err(|e| format!("Failed to re-encode signed attributes: {e}"))?;

    let mut certificate = None;
    let mut certificate_valid_now = None;
    let mut signature_valid = false;
    let mut crypto_error = None;
    let mut chain = None;

    match find_matching_certificate(&signed_data, &signer_info.sid) {
        Some(x509_cert) => match x509_cert
            .to_der()
            .map_err(|e| format!("Failed to encode embedded certificate: {e}"))
            .and_then(|bytes| {
                Certificate::from_der(&bytes)
                    .map_err(|e| format!("Failed to load embedded certificate: {e}"))
            }) {
            Ok(wrapped) => {
                certificate_valid_now = wrapped.is_currently_valid().ok();
                match verify_signature_bytes(
                    algo,
                    &x509_cert,
                    &signed_attrs_der,
                    signer_info.signature.as_bytes(),
                ) {
                    Ok(valid) => signature_valid = valid,
                    Err(e) => crypto_error = Some(e),
                }

                // Chain validation: every other certificate embedded in
                // the CMS `certificates` set (besides the leaf itself) is
                // a candidate intermediate.
                let intermediates: Vec<Certificate> = all_certificates(&signed_data)
                    .into_iter()
                    .filter(|c| c.der_bytes() != wrapped.der_bytes())
                    .collect();
                chain = Some(chain::validate_chain(
                    &wrapped,
                    &intermediates,
                    trust_anchors,
                    chain::now_unix(),
                ));

                certificate = Some(wrapped);
            }
            Err(e) => crypto_error = Some(e),
        },
        None => crypto_error = Some("No certificate found in CMS SignedData".to_string()),
    }

    let timestamp = signer_info.unsigned_attrs.as_ref().and_then(|attrs| {
        attrs.iter().find_map(|attr| {
            if attr.oid.to_string() != OID_SIGNATURE_TIMESTAMP_TOKEN {
                return None;
            }
            let value = attr.values.get(0)?;
            let token_der = value.to_der().ok()?;
            let expected_digest = digest_for_algorithm(algo, signer_info.signature.as_bytes());
            Some(
                match timestamp::verify_token(&token_der, algo, &expected_digest) {
                    Ok(token) => TimestampVerification {
                        gen_time: Some(token.gen_time),
                        tsa_certificate: token.tsa_certificate,
                        valid: true,
                        error: None,
                    },
                    Err(e) => TimestampVerification {
                        gen_time: None,
                        tsa_certificate: None,
                        valid: false,
                        error: Some(e),
                    },
                },
            )
        })
    });

    let is_valid = digest_matches && signature_valid;
    let error = if is_valid {
        None
    } else if !digest_matches {
        Some(
            "Document content does not match the signed digest (document was modified after signing)"
                .to_string(),
        )
    } else {
        crypto_error.or_else(|| Some("Cryptographic signature verification failed".to_string()))
    };

    Ok(CryptoOutcome {
        certificate,
        certificate_valid_now,
        is_valid,
        chain,
        timestamp,
        error,
    })
}

/// The `id-aa-signatureTimeStampToken` attribute OID as a string, for
/// comparing against a decoded `Attribute.oid`'s `Display` output. Kept in
/// sync with (but independent of) `timestamp.rs`'s copy since that one is
/// private to that module and used for *building* the attribute, while this
/// one is used for *recognizing* it.
const OID_SIGNATURE_TIMESTAMP_TOKEN: &str = "1.2.840.113549.1.9.16.2.14";

/// Returns every certificate embedded in the CMS `certificates` set.
fn all_certificates(signed_data: &SignedData) -> Vec<Certificate> {
    let Some(certs) = signed_data.certificates.as_ref() else {
        return Vec::new();
    };
    certs
        .0
        .iter()
        .filter_map(|c| match c {
            CertificateChoices::Certificate(cert) => cert.to_der().ok(),
            _ => None,
        })
        .filter_map(|der| Certificate::from_der(&der).ok())
        .collect()
}

fn find_matching_certificate(
    signed_data: &SignedData,
    sid: &SignerIdentifier,
) -> Option<x509_cert::Certificate> {
    let certs = signed_data.certificates.as_ref()?;
    let candidates: Vec<x509_cert::Certificate> = certs
        .0
        .iter()
        .filter_map(|c| match c {
            CertificateChoices::Certificate(cert) => Some(cert.clone()),
            _ => None,
        })
        .collect();

    if let SignerIdentifier::IssuerAndSerialNumber(ias) = sid {
        if let Some(found) = candidates.iter().find(|cert| {
            cert.tbs_certificate.issuer.to_der().ok() == ias.issuer.to_der().ok()
                && cert.tbs_certificate.serial_number.to_der().ok()
                    == ias.serial_number.to_der().ok()
        }) {
            return Some(found.clone());
        }
    }

    candidates.into_iter().next()
}

/// Verifies a raw signature (`signature_bytes`) over `message` using the
/// public key embedded in `cert`. Shared with the crate-internal `chain`
/// module to verify that a candidate issuer's key validates a child certificate's
/// `tbsCertificate` signature (RFC 5280 §4.1.1.3), and used here to verify
/// a CMS `SignerInfo.signature` over its `signedAttrs`.
pub(super) fn verify_signature_bytes(
    algo: SignatureAlgorithm,
    cert: &x509_cert::Certificate,
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<bool, String> {
    let spki_der = cert
        .tbs_certificate
        .subject_public_key_info
        .to_der()
        .map_err(|e| format!("Failed to encode public key: {e}"))?;
    let spki = spki::SubjectPublicKeyInfoRef::from_der(&spki_der)
        .map_err(|e| format!("Failed to decode public key: {e}"))?;

    match algo {
        SignatureAlgorithm::RsaSha256 => verify_rsa::<sha2::Sha256>(spki, message, signature_bytes),
        SignatureAlgorithm::RsaSha384 => verify_rsa::<sha2::Sha384>(spki, message, signature_bytes),
        SignatureAlgorithm::RsaSha512 => verify_rsa::<sha2::Sha512>(spki, message, signature_bytes),
        SignatureAlgorithm::EcdsaP256Sha256 => verify_ecdsa_p256(spki, message, signature_bytes),
        SignatureAlgorithm::EcdsaP384Sha384 => verify_ecdsa_p384(spki, message, signature_bytes),
        SignatureAlgorithm::EcdsaP521Sha512 => verify_ecdsa_p521(spki, message, signature_bytes),
    }
}

fn verify_rsa<D>(
    spki: spki::SubjectPublicKeyInfoRef<'_>,
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<bool, String>
where
    D: sha2::Digest + der::oid::AssociatedOid,
{
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use signature::Verifier;

    let verifying_key =
        VerifyingKey::<D>::try_from(spki).map_err(|e| format!("Invalid RSA public key: {e}"))?;
    let sig = Signature::try_from(signature_bytes)
        .map_err(|e| format!("Invalid RSA signature encoding: {e}"))?;

    Ok(verifying_key.verify(message, &sig).is_ok())
}

fn verify_ecdsa_p256(
    spki: spki::SubjectPublicKeyInfoRef<'_>,
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<bool, String> {
    use p256::ecdsa::{Signature, VerifyingKey};
    use signature::Verifier;

    let verifying_key =
        VerifyingKey::try_from(spki).map_err(|e| format!("Invalid ECDSA public key: {e}"))?;
    let sig = Signature::from_der(signature_bytes)
        .map_err(|e| format!("Invalid ECDSA signature encoding: {e}"))?;

    Ok(verifying_key.verify(message, &sig).is_ok())
}

// NOTE(ownership): `verify_ecdsa_p384`/`verify_ecdsa_p521` and their two
// match arms above are the minimal addition required to keep this module
// compiling once `SignatureAlgorithm` (owned by the ECDSA multi-curve task)
// gained the `EcdsaP384Sha384`/`EcdsaP521Sha512` variants -- Rust requires
// this `match` to stay exhaustive. This file is not otherwise in that
// task's file-ownership list; only this mechanical, same-shape-as-P-256
// addition was made here.

/// Verifies an ECDSA P-384 signature. See [`verify_ecdsa_p256`] -- identical
/// shape, different curve.
fn verify_ecdsa_p384(
    spki: spki::SubjectPublicKeyInfoRef<'_>,
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<bool, String> {
    use p384::ecdsa::{Signature, VerifyingKey};
    use signature::Verifier;

    let verifying_key =
        VerifyingKey::try_from(spki).map_err(|e| format!("Invalid ECDSA public key: {e}"))?;
    let sig = Signature::from_der(signature_bytes)
        .map_err(|e| format!("Invalid ECDSA signature encoding: {e}"))?;

    Ok(verifying_key.verify(message, &sig).is_ok())
}

/// Verifies an ECDSA P-521 signature.
///
/// Unlike [`verify_ecdsa_p256`]/`verify_ecdsa_p384`, `p521::ecdsa::VerifyingKey`
/// has no direct SPKI/PKCS8 support (see `sign_ecdsa_p521`'s doc comment in
/// `certificate.rs` for why) -- so this goes through `p521::PublicKey` (a
/// plain `elliptic_curve::PublicKey<NistP521>` alias, which does support
/// SPKI) and hands its affine point to `VerifyingKey::from_affine`.
fn verify_ecdsa_p521(
    spki: spki::SubjectPublicKeyInfoRef<'_>,
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<bool, String> {
    use p521::ecdsa::{Signature, VerifyingKey};
    use signature::Verifier;

    let public_key =
        p521::PublicKey::try_from(spki).map_err(|e| format!("Invalid ECDSA public key: {e}"))?;
    let verifying_key = VerifyingKey::from_affine(*public_key.as_affine())
        .map_err(|e| format!("Invalid ECDSA public key: {e}"))?;
    let sig = Signature::from_der(signature_bytes)
        .map_err(|e| format!("Invalid ECDSA signature encoding: {e}"))?;

    Ok(verifying_key.verify(message, &sig).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_decode() {
        assert_eq!(hex_decode(b"48656C6C6F"), Some(b"Hello".to_vec()));
        assert_eq!(hex_decode(b"abc"), None);
    }

    #[test]
    fn test_parse_byte_range() {
        let slice = b"/ByteRange [0 100 200 300]/Contents <ABCD>";
        let range = parse_byte_range(slice).unwrap();
        assert_eq!(range.offset1, 0);
        assert_eq!(range.length1, 100);
        assert_eq!(range.offset2, 200);
        assert_eq!(range.length2, 300);
    }

    #[test]
    fn test_parse_hex_contents() {
        let slice = b"/Contents <48656C6C6F>\n";
        assert_eq!(parse_hex_contents(slice), Some(b"Hello".to_vec()));
    }

    #[test]
    fn test_parse_string_field_with_escapes() {
        let slice = b"/Name (John \\(Doe\\))\n/Reason (Approval)\n";
        assert_eq!(
            parse_string_field(slice, b"/Name ("),
            Some("John (Doe)".to_string())
        );
        assert_eq!(
            parse_string_field(slice, b"/Reason ("),
            Some("Approval".to_string())
        );
        assert_eq!(parse_string_field(slice, b"/Location ("), None);
    }

    #[test]
    fn test_find_signature_objects_none() {
        assert!(find_signature_objects(b"%PDF-1.7\n%%EOF").is_empty());
    }

    #[test]
    fn test_trim_to_der_length_short_form() {
        let mut bytes = vec![0x30, 0x03, 0x01, 0x02, 0x03];
        bytes.extend_from_slice(&[0u8; 10]); // trailing padding
        assert_eq!(
            trim_to_der_length(&bytes),
            Some(&[0x30, 0x03, 0x01, 0x02, 0x03][..])
        );
    }

    #[test]
    fn test_trim_to_der_length_long_form() {
        let content = vec![0xAAu8; 200];
        let mut bytes = vec![0x30, 0x81, 0xC8]; // long form: 1 length octet, 0xC8 = 200
        bytes.extend_from_slice(&content);
        bytes.extend_from_slice(&[0u8; 5]); // trailing padding
        let trimmed = trim_to_der_length(&bytes).unwrap();
        assert_eq!(trimmed.len(), 3 + 200);
    }
}
