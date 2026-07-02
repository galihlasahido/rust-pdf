//! RFC 3161 Time-Stamp Protocol (TSP) support.
//!
//! Implements building a `TimeStampReq` and parsing/validating a
//! `TimeStampResp`, per [RFC 3161] ("Internet X.509 Public Key
//! Infrastructure Time-Stamp Protocol (TSP)"). The resulting `TimeStampToken`
//! is embedded as the CMS unsigned attribute `id-aa-signatureTimeStampToken`
//! (RFC 5126 CAdES-T; carried in PDF via `SubFilter /ETSI.CAdES.detached`,
//! see ETSI EN 319 142-1 "PAdES digital signatures" for the "B-T" baseline
//! profile -- a "B-B" signature plus this token).
//!
//! # Why this module doesn't do its own HTTP
//!
//! RFC 3161 timestamping is transport-agnostic at the protocol level (the
//! request/response DER messages are usually POSTed over HTTP with
//! `Content-Type: application/timestamp-query`, but nothing about the ASN.1
//! requires that). Bundling an HTTP client into this crate would force a
//! specific TLS stack, proxy behavior, and async-vs-blocking model onto every
//! consumer -- including a desktop host application (e.g. Tauri) that
//! already has its own. Instead, callers implement
//! [`TimestampAuthorityClient`] using whatever HTTP client their application
//! already uses and hand it to the signer.
//!
//! [RFC 3161]: https://www.rfc-editor.org/rfc/rfc3161

use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier};
use der::asn1::{GeneralizedTime, Int, ObjectIdentifier, OctetString};
use der::{Decode, Encode, FixedTag, Reader, Sequence, Tag};
use rand::RngCore;
use spki::AlgorithmIdentifierOwned;
use x509_cert::attr::Attribute;

use crate::error::SignatureError;
use super::pkcs7::{
    build_boolean, build_digest_algorithm_identifier_for, build_integer, build_octet_string,
    build_sequence,
};
use super::verifier::verify_signature_bytes;
use super::{digest_for_algorithm, Certificate, SignatureAlgorithm, SignatureResult};

/// The `id-aa-signatureTimeStampToken` attribute OID (RFC 5126 / restated by
/// ETSI EN 319 122-1): `1.2.840.113549.1.9.16.2.14`.
const OID_SIGNATURE_TIMESTAMP_TOKEN: &str = "1.2.840.113549.1.9.16.2.14";

/// `id-ct-TSTInfo` content-type OID (RFC 3161 §2.4.2): the `eContentType`
/// that must appear inside the timestamp token's `SignedData`.
const OID_CT_TST_INFO: &str = "1.2.840.113549.1.9.16.1.4";

/// Maximum accepted size of a raw RFC 3161 response, before any DER parsing
/// touches it. A real TSA response is a few KB at most (one or two
/// certificates plus a small TSTInfo); this bounds how much untrusted
/// network input we're willing to buffer/parse, guarding against a
/// malicious or misbehaving TSA sending an oversized body.
const MAX_RESPONSE_LEN: usize = 1 << 20; // 1 MiB

/// Pluggable transport for RFC 3161 requests.
///
/// Implementations are expected to `POST` `tsq_der` to a TSA URL with
/// `Content-Type: application/timestamp-query` and return the raw response
/// body (which should have `Content-Type: application/timestamp-reply`).
/// This crate does not validate the HTTP status code or content type --
/// that's the transport's job; [`parse_timestamp_response`] validates the
/// DER content regardless of how it arrived.
pub trait TimestampAuthorityClient: std::fmt::Debug + Send + Sync {
    /// Sends `tsq_der` (a DER-encoded `TimeStampReq`) to the TSA and returns
    /// the raw response bytes (a DER-encoded `TimeStampResp`).
    fn timestamp(&self, tsq_der: &[u8]) -> SignatureResult<Vec<u8>>;
}

/// A built RFC 3161 `TimeStampReq`, ready to send via a
/// [`TimestampAuthorityClient`].
#[derive(Debug, Clone)]
pub struct TimestampRequest {
    /// The DER-encoded `TimeStampReq`.
    pub der: Vec<u8>,
    /// The nonce embedded in the request (masked to 63 bits so it always
    /// encodes as a positive `INTEGER`). [`parse_timestamp_response`] checks
    /// the response echoes this value.
    pub nonce: i64,
    /// The digest algorithm used for the message imprint.
    pub hash_algorithm: SignatureAlgorithm,
    /// The hashed message (digest of the data being timestamped).
    pub hashed_message: Vec<u8>,
}

/// Builds an RFC 3161 `TimeStampReq` (RFC 3161 §2.4.1) over
/// `hashed_message` (a pre-computed digest, *not* raw data -- callers hash
/// first with the digest algorithm matching `hash_algorithm`).
///
/// ```text
/// TimeStampReq ::= SEQUENCE  {
///    version                INTEGER  { v1(1) },
///    messageImprint         MessageImprint,
///    reqPolicy              TSAPolicyId              OPTIONAL,
///    nonce                  INTEGER                  OPTIONAL,
///    certReq                BOOLEAN                  DEFAULT FALSE,
///    extensions         [0] IMPLICIT Extensions      OPTIONAL  }
/// ```
///
/// `cert_req` should normally be `true` so the TSA includes its own
/// certificate in the response, letting [`parse_timestamp_response`]
/// validate the token without an out-of-band certificate lookup.
pub fn build_timestamp_request(
    hash_algorithm: SignatureAlgorithm,
    hashed_message: Vec<u8>,
    cert_req: bool,
) -> TimestampRequest {
    let mut nonce_bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    // Mask off the top bit so the value is unambiguously positive once
    // interpreted as a signed 64-bit integer (matches `build_integer`'s
    // two's-complement `i64` encoding).
    let nonce = (u64::from_be_bytes(nonce_bytes) & 0x7FFF_FFFF_FFFF_FFFF) as i64;

    let message_imprint = build_sequence(&{
        let mut v = build_digest_algorithm_identifier_for(hash_algorithm);
        v.extend_from_slice(&build_octet_string(&hashed_message));
        v
    });

    let mut body = Vec::new();
    body.extend_from_slice(&build_integer(1)); // version
    body.extend_from_slice(&message_imprint);
    body.extend_from_slice(&build_integer(nonce));
    body.extend_from_slice(&build_boolean(cert_req));

    TimestampRequest {
        der: build_sequence(&body),
        nonce,
        hash_algorithm,
        hashed_message,
    }
}

/// A validated RFC 3161 timestamp token, ready to embed (via
/// `embed_unsigned_timestamp_attribute`, crate-internal) or already
/// extracted during verification.
#[derive(Debug, Clone)]
pub struct TimestampToken {
    /// The raw DER-encoded `TimeStampToken` (a CMS `ContentInfo`), suitable
    /// for embedding as the `id-aa-signatureTimeStampToken` unsigned
    /// attribute value.
    pub token_der: Vec<u8>,
    /// The `genTime` field from `TSTInfo`, as an RFC 3339-ish string (via
    /// `der`'s `GeneralizedTime` display), i.e. the time the TSA asserts.
    pub gen_time: String,
    /// The TSA's certificate, if it was included in the token (it is, when
    /// the request set `certReq: true` and the TSA honors it).
    pub tsa_certificate: Option<Certificate>,
}

// --- RFC 3161 response ASN.1 modeling (decode-only) ---
//
// These types intentionally do not derive `Sequence` for every nested
// OPTIONAL field's full internal structure (e.g. `Accuracy`, `PKIFreeText`,
// `GeneralName`) -- we don't need their contents, only to skip over them
// correctly during sequential decode. `OpaqueSequence`/`OpaqueContextTag`
// give the decoder a real, statically-known tag to peek against (required
// for the `der` crate's `Option<T>` OPTIONAL-field decoding, which peeks
// `T::TAG` -- seeing `der::asn1::Any`'s tag would always match and greedily
// consume a later, unrelated field).

/// An opaque `SEQUENCE`: decodes (and skips) any well-formed `SEQUENCE`
/// without inspecting its contents. Used for RFC 3161 fields whose contents
/// this module doesn't need (`Accuracy`, `PKIFreeText`).
#[derive(Debug, Clone)]
struct OpaqueSequence(der::asn1::Any);

impl FixedTag for OpaqueSequence {
    const TAG: Tag = Tag::Sequence;
}

impl<'a> der::Decode<'a> for OpaqueSequence {
    fn decode<R: Reader<'a>>(reader: &mut R) -> der::Result<Self> {
        der::asn1::Any::decode(reader).map(OpaqueSequence)
    }
}

impl der::Encode for OpaqueSequence {
    fn encoded_len(&self) -> der::Result<der::Length> {
        self.0.encoded_len()
    }
    fn encode(&self, writer: &mut impl der::Writer) -> der::Result<()> {
        self.0.encode(writer)
    }
}

/// `MessageImprint ::= SEQUENCE { hashAlgorithm AlgorithmIdentifier, hashedMessage OCTET STRING }`
/// (RFC 3161 §2.4.1).
#[derive(Debug, Clone, Sequence)]
struct MessageImprintAsn1 {
    hash_algorithm: AlgorithmIdentifierOwned,
    hashed_message: OctetString,
}

/// `PKIStatusInfo ::= SEQUENCE { status PKIStatus, statusString PKIFreeText OPTIONAL, failInfo PKIFailureInfo OPTIONAL }`
/// (RFC 3161 §2.4.2).
#[derive(Debug, Clone, Sequence)]
struct PkiStatusInfoAsn1 {
    status: i32,
    status_string: Option<OpaqueSequence>,
    fail_info: Option<der::asn1::BitString>,
}

/// `TimeStampResp ::= SEQUENCE { status PKIStatusInfo, timeStampToken TimeStampToken OPTIONAL }`
/// (RFC 3161 §2.4.2). `TimeStampToken ::= ContentInfo` (RFC 3161 §2.4.2).
#[derive(Debug, Clone, Sequence)]
struct TimeStampRespAsn1 {
    status: PkiStatusInfoAsn1,
    time_stamp_token: Option<ContentInfo>,
}

/// `TSTInfo` (RFC 3161 §2.4.2), decode-only. `accuracy`/`tsa`/`extensions`
/// are captured opaquely (see module docs); `ordering` and `ess_cert_id_v2`
/// fields are only present at all through the `der` DEFAULT/OPTIONAL
/// machinery and not otherwise inspected.
#[derive(Debug, Clone, Sequence)]
struct TstInfoAsn1 {
    version: u8,
    policy: ObjectIdentifier,
    message_imprint: MessageImprintAsn1,
    serial_number: Int,
    gen_time: GeneralizedTime,
    accuracy: Option<OpaqueSequence>,
    #[asn1(default = "Default::default")]
    ordering: bool,
    nonce: Option<Int>,
    #[asn1(context_specific = "0", tag_mode = "explicit", optional = "true")]
    tsa: Option<der::Any>,
    #[asn1(context_specific = "1", tag_mode = "implicit", optional = "true")]
    extensions: Option<der::Any>,
}

/// Parses and validates a raw RFC 3161 `TimeStampResp` (**untrusted input**
/// -- this may come directly from a network TSA response). Checks:
///
/// - the response fits under a fixed size limit before any parsing,
/// - `status` is `granted` (0) or `grantedWithMods` (1),
/// - `timeStampToken` is present and its content type is `id-signedData`,
/// - the token's `TSTInfo.messageImprint` matches `(expected_hash_algorithm,
///   expected_hashed_message)` exactly,
/// - if `expected_nonce` is `Some`, `TSTInfo.nonce` matches it.
///
/// Does **not** verify the TSA's own CMS signature over `TSTInfo` -- that is
/// done separately by `verify_token` (crate-internal; used both right after
/// requesting a timestamp and again when re-validating an embedded token
/// during PDF signature verification).
pub fn parse_timestamp_response(
    resp_der: &[u8],
    expected_hash_algorithm: SignatureAlgorithm,
    expected_hashed_message: &[u8],
    expected_nonce: Option<i64>,
) -> SignatureResult<TimestampToken> {
    if resp_der.len() > MAX_RESPONSE_LEN {
        return Err(SignatureError::TimestampError(format!(
            "TSA response too large ({} bytes, limit {})",
            resp_der.len(),
            MAX_RESPONSE_LEN
        )));
    }

    let resp = TimeStampRespAsn1::from_der(resp_der)
        .map_err(|e| SignatureError::TimestampError(format!("Malformed TimeStampResp: {e}")))?;

    // PKIStatus: granted(0) or grantedWithMods(1) are the only "you got a
    // usable token" outcomes (RFC 3161 §2.4.2).
    if resp.status.status != 0 && resp.status.status != 1 {
        return Err(SignatureError::TimestampError(format!(
            "TSA rejected the timestamp request (PKIStatus {})",
            resp.status.status
        )));
    }

    let content_info = resp.time_stamp_token.ok_or_else(|| {
        SignatureError::TimestampError("TSA response has no timeStampToken".to_string())
    })?;

    let token_der = content_info
        .to_der()
        .map_err(|e| SignatureError::TimestampError(format!("Failed to re-encode token: {e}")))?;

    let signed_data: SignedData = content_info
        .content
        .decode_as()
        .map_err(|e| SignatureError::TimestampError(format!("Malformed token SignedData: {e}")))?;

    if signed_data.encap_content_info.econtent_type.to_string() != OID_CT_TST_INFO {
        return Err(SignatureError::TimestampError(format!(
            "Token eContentType is not id-ct-TSTInfo: {}",
            signed_data.encap_content_info.econtent_type
        )));
    }

    let econtent = signed_data
        .encap_content_info
        .econtent
        .as_ref()
        .ok_or_else(|| SignatureError::TimestampError("Token has no eContent".to_string()))?;

    let tst_info = TstInfoAsn1::from_der(econtent.value())
        .map_err(|e| SignatureError::TimestampError(format!("Malformed TSTInfo: {e}")))?;

    let expected_digest_oid = expected_hash_algorithm.digest_oid();
    if tst_info.message_imprint.hash_algorithm.oid.to_string() != expected_digest_oid {
        return Err(SignatureError::TimestampError(
            "TSTInfo messageImprint hash algorithm does not match the request".to_string(),
        ));
    }
    if tst_info.message_imprint.hashed_message.as_bytes() != expected_hashed_message {
        return Err(SignatureError::TimestampError(
            "TSTInfo messageImprint does not match the signed data (bad TSA response, or \
             tampered timestamp)"
                .to_string(),
        ));
    }

    if let Some(expected_nonce) = expected_nonce {
        let nonce_matches = tst_info
            .nonce
            .as_ref()
            .map(|n| i64_from_int(n) == Some(expected_nonce))
            .unwrap_or(false);
        if !nonce_matches {
            return Err(SignatureError::TimestampError(
                "TSTInfo nonce does not match the request (possible replay)".to_string(),
            ));
        }
    }

    let tsa_certificate = find_signer_certificate(&signed_data);

    Ok(TimestampToken {
        token_der,
        gen_time: tst_info.gen_time.to_date_time().to_string(),
        tsa_certificate,
    })
}

/// Re-decodes and cryptographically verifies an already-embedded timestamp
/// token: checks that the TSA's signature over `TSTInfo` validates against
/// the embedded (or otherwise supplied) TSA certificate, and that the
/// token's `messageImprint` matches `expected_hashed_message`.
///
/// Used by [`super::verifier::SignatureVerifier`] to validate a
/// `id-aa-signatureTimeStampToken` unsigned attribute found in a PDF's
/// `SignerInfo`, i.e. this is the "B-T" half of PAdES verification.
pub(crate) fn verify_token(
    token_der: &[u8],
    expected_hash_algorithm: SignatureAlgorithm,
    expected_hashed_message: &[u8],
) -> Result<TimestampToken, String> {
    if token_der.len() > MAX_RESPONSE_LEN {
        return Err(format!(
            "Embedded timestamp token too large ({} bytes)",
            token_der.len()
        ));
    }

    let content_info = ContentInfo::from_der(token_der)
        .map_err(|e| format!("Malformed timestamp token ContentInfo: {e}"))?;
    let signed_data: SignedData = content_info
        .content
        .decode_as()
        .map_err(|e| format!("Malformed timestamp token SignedData: {e}"))?;

    if signed_data.encap_content_info.econtent_type.to_string() != OID_CT_TST_INFO {
        return Err("Timestamp token eContentType is not id-ct-TSTInfo".to_string());
    }
    let econtent = signed_data
        .encap_content_info
        .econtent
        .as_ref()
        .ok_or_else(|| "Timestamp token has no eContent".to_string())?;
    let tst_info = TstInfoAsn1::from_der(econtent.value())
        .map_err(|e| format!("Malformed TSTInfo: {e}"))?;

    if tst_info.message_imprint.hash_algorithm.oid.to_string() != expected_hash_algorithm.digest_oid()
        || tst_info.message_imprint.hashed_message.as_bytes() != expected_hashed_message
    {
        return Err(
            "Timestamp token messageImprint does not match the signature value it is attached \
             to (the document may have been re-signed or the token copied from elsewhere)"
                .to_string(),
        );
    }

    let signer_info = signed_data
        .signer_infos
        .0
        .iter()
        .next()
        .ok_or_else(|| "Timestamp token CMS has no SignerInfo".to_string())?;

    let tsa_algo = SignatureAlgorithm::from_oids(
        &signer_info.signature_algorithm.oid.to_string(),
        &signer_info.digest_alg.oid.to_string(),
    )
    .ok_or_else(|| {
        format!(
            "Unsupported TSA signature algorithm: {} (digest {})",
            signer_info.signature_algorithm.oid, signer_info.digest_alg.oid
        )
    })?;

    let signed_attrs = signer_info
        .signed_attrs
        .as_ref()
        .ok_or_else(|| "Timestamp token SignerInfo has no signedAttrs".to_string())?;
    let signed_attrs_der = signed_attrs
        .to_der()
        .map_err(|e| format!("Failed to re-encode timestamp token signedAttrs: {e}"))?;

    let message_digest_oid = const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
    let message_digest_attr = signed_attrs
        .iter()
        .find(|attr| attr.oid == message_digest_oid)
        .ok_or_else(|| "Timestamp token signedAttrs has no messageDigest".to_string())?;
    let digest_value: OctetString = message_digest_attr
        .values
        .get(0)
        .ok_or_else(|| "Timestamp token messageDigest attribute has no value".to_string())?
        .clone()
        .decode_as()
        .map_err(|e| format!("Invalid timestamp token messageDigest attribute: {e}"))?;
    let computed_tst_info_digest = digest_for_algorithm(tsa_algo, econtent.value());
    if computed_tst_info_digest != digest_value.as_bytes() {
        return Err(
            "Timestamp token messageDigest does not match its own TSTInfo (token is internally \
             inconsistent)"
                .to_string(),
        );
    }

    let tsa_x509 = find_matching_x509_certificate(&signed_data, &signer_info.sid)
        .ok_or_else(|| "No TSA certificate found in the timestamp token".to_string())?;

    let sig_valid = verify_signature_bytes(
        tsa_algo,
        &tsa_x509,
        &signed_attrs_der,
        signer_info.signature.as_bytes(),
    )?;
    if !sig_valid {
        return Err("Timestamp token signature does not validate against the TSA certificate".to_string());
    }

    let tsa_certificate = tsa_x509
        .to_der()
        .ok()
        .and_then(|der| Certificate::from_der(&der).ok());

    Ok(TimestampToken {
        token_der: token_der.to_vec(),
        gen_time: tst_info.gen_time.to_date_time().to_string(),
        tsa_certificate,
    })
}

fn find_signer_certificate(signed_data: &SignedData) -> Option<Certificate> {
    let signer_info = signed_data.signer_infos.0.iter().next()?;
    let x509 = find_matching_x509_certificate(signed_data, &signer_info.sid)?;
    let der = x509.to_der().ok()?;
    Certificate::from_der(&der).ok()
}

fn find_matching_x509_certificate(
    signed_data: &SignedData,
    sid: &SignerIdentifier,
) -> Option<x509_cert::Certificate> {
    use cms::cert::CertificateChoices;

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

/// Extracts an `i64` from a DER `Int`, returning `None` if it doesn't fit
/// (our nonces are always masked to 63 bits, so a legitimate echo always
/// fits; a value that doesn't fit is treated as a mismatch rather than
/// silently truncated).
fn i64_from_int(int: &Int) -> Option<i64> {
    let bytes = int.as_bytes();
    if bytes.len() > 8 {
        return None;
    }
    let mut buf = [0u8; 8];
    let negative = !bytes.is_empty() && bytes[0] & 0x80 != 0;
    let fill = if negative { 0xFF } else { 0x00 };
    buf.fill(fill);
    buf[8 - bytes.len()..].copy_from_slice(bytes);
    Some(i64::from_be_bytes(buf))
}

/// Rebuilds a CMS `SignedData` with an added
/// `id-aa-signatureTimeStampToken` unsigned attribute carrying `token_der`,
/// re-encoding via the typed `cms`/`x509_cert`/`der` structures (rather than
/// hand-rolled byte concatenation) so the SET-OF/SEQUENCE length prefixes
/// are always self-consistent.
///
/// `cms_der` must be the `ContentInfo` DER produced by
/// [`super::Pkcs7Builder::build`] (exactly one `SignerInfo`, no existing
/// `unsignedAttrs`).
pub(crate) fn embed_unsigned_timestamp_attribute(
    cms_der: &[u8],
    token_der: &[u8],
) -> SignatureResult<Vec<u8>> {
    let content_info = ContentInfo::from_der(cms_der).map_err(|e| {
        SignatureError::TimestampError(format!("Failed to parse CMS for timestamp embedding: {e}"))
    })?;
    let mut signed_data: SignedData = content_info.content.decode_as().map_err(|e| {
        SignatureError::TimestampError(format!("Failed to parse SignedData for timestamp embedding: {e}"))
    })?;

    let mut signer_infos: Vec<_> = signed_data.signer_infos.0.iter().cloned().collect();
    let signer_info = signer_infos.first_mut().ok_or_else(|| {
        SignatureError::TimestampError("CMS SignedData has no SignerInfo to attach a timestamp to".to_string())
    })?;

    let token_any = der::Any::from_der(token_der).map_err(|e| {
        SignatureError::TimestampError(format!("Failed to wrap timestamp token as ANY: {e}"))
    })?;
    let oid = ObjectIdentifier::new(OID_SIGNATURE_TIMESTAMP_TOKEN).map_err(|e| {
        SignatureError::TimestampError(format!("Invalid timestamp attribute OID: {e}"))
    })?;
    let attribute = Attribute {
        oid,
        values: der::asn1::SetOfVec::try_from(vec![token_any]).map_err(|e| {
            SignatureError::TimestampError(format!("Failed to build attribute value set: {e}"))
        })?,
    };
    signer_info.unsigned_attrs = Some(
        der::asn1::SetOfVec::try_from(vec![attribute])
            .map_err(|e| SignatureError::TimestampError(format!("Failed to build unsignedAttrs: {e}")))?,
    );

    signed_data.signer_infos = cms::signed_data::SignerInfos(
        der::asn1::SetOfVec::try_from(signer_infos)
            .map_err(|e| SignatureError::TimestampError(format!("Failed to rebuild SignerInfos: {e}")))?,
    );

    let new_content_info = ContentInfo {
        content_type: content_info.content_type,
        content: der::Any::encode_from(&signed_data).map_err(|e| {
            SignatureError::TimestampError(format!("Failed to re-encode SignedData: {e}"))
        })?,
    };

    new_content_info
        .to_der()
        .map_err(|e| SignatureError::TimestampError(format!("Failed to re-encode ContentInfo: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_timestamp_request_shape() {
        let req = build_timestamp_request(SignatureAlgorithm::RsaSha256, vec![0xAB; 32], true);
        assert!(req.der.starts_with(&[0x30])); // SEQUENCE
        assert!(req.nonce >= 0);
        assert_eq!(req.hashed_message, vec![0xAB; 32]);

        // Re-decode as a generic SEQUENCE to sanity-check DER well-formedness.
        let parsed = der::asn1::SequenceRef::from_der(&req.der);
        assert!(parsed.is_ok(), "request must be valid DER: {:?}", parsed.err());
    }

    #[test]
    fn test_build_timestamp_request_nonce_is_always_positive_i64() {
        // Run several times since the nonce is random; the top bit must
        // always be masked off so `build_integer` never emits a
        // two's-complement negative encoding for it.
        for _ in 0..20 {
            let req = build_timestamp_request(SignatureAlgorithm::RsaSha256, vec![1, 2, 3], false);
            assert!(req.nonce >= 0);
        }
    }

    #[test]
    fn test_i64_from_int_roundtrip() {
        let n: i64 = 123_456_789;
        let encoded = Int::new(&n.to_be_bytes()).unwrap();
        assert_eq!(i64_from_int(&encoded), Some(n));
    }

    #[test]
    fn test_i64_from_int_rejects_oversized() {
        let big = Int::new(&[0x01; 9]).unwrap();
        assert_eq!(i64_from_int(&big), None);
    }

    #[test]
    fn test_parse_timestamp_response_rejects_oversized_input() {
        let oversized = vec![0u8; MAX_RESPONSE_LEN + 1];
        let err = parse_timestamp_response(&oversized, SignatureAlgorithm::RsaSha256, b"x", None)
            .unwrap_err();
        assert!(matches!(err, SignatureError::TimestampError(_)));
    }

    #[test]
    fn test_parse_timestamp_response_rejects_garbage() {
        let err = parse_timestamp_response(b"not a der message", SignatureAlgorithm::RsaSha256, b"x", None)
            .unwrap_err();
        assert!(matches!(err, SignatureError::TimestampError(_)));
    }

    #[test]
    fn test_verify_token_rejects_garbage() {
        let err = verify_token(b"not a der message", SignatureAlgorithm::RsaSha256, b"x").unwrap_err();
        assert!(err.contains("Malformed"));
    }
}
