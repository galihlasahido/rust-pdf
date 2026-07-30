//! RFC 6960 Online Certificate Status Protocol (OCSP) support.
//!
//! Implements building a DER-encoded `OCSPRequest` and parsing a
//! DER-encoded `OCSPResponse`, per [RFC 6960] ("X.509 Internet Public Key
//! Infrastructure Online Certificate Status Protocol - OCSP"). A validated
//! OCSP response is the other half (alongside CRLs) of the revocation
//! material a PAdES "B-LT" signature embeds via `/DSS`
//! ([`super::dss::DssEntry::ocsps`]) so the signature can still be validated
//! after the original OCSP responder may no longer be reachable online.
//!
//! # Why this module doesn't do its own HTTP
//!
//! Exactly [`super::timestamp`]'s reasoning applies here (see that module's
//! doc comment for the full argument): OCSP requests/responses are
//! transport-agnostic DER messages, usually POSTed over HTTP with
//! `Content-Type: application/ocsp-request` (RFC 6960 §4.1, appendix A.1),
//! but bundling an HTTP client into this crate would force a specific TLS
//! stack/proxy/async model onto every consumer. Instead, callers implement
//! [`OcspTransport`] using whatever HTTP client their application already
//! uses -- its `post` method has the identical shape as
//! [`super::TimestampAuthorityClient::timestamp`], just renamed to reflect
//! that OCSP is naturally a URL-addressed `POST` (the responder URL usually
//! comes from the certificate itself, see [`ocsp_responder_url`]) rather
//! than a single well-known TSA endpoint.
//!
//! # Disclosed limitations
//!
//! - **Hash algorithm is fixed to SHA-256.** RFC 6960's `CertID.hashAlgorithm`
//!   is an arbitrary `AlgorithmIdentifier`, and in practice a large number of
//!   real-world OCSP responders still only recognize a SHA-1 `CertID` (SHA-1
//!   was the original RFC 2560 default). This crate has no SHA-1 dependency
//!   (only `sha2`, pulled in transitively via the `encryption` feature that
//!   `signatures` already depends on) and this module's file-ownership scope
//!   for this change does not extend to `Cargo.toml`, so adding one is out of
//!   scope here. [`build_ocsp_request`] and [`parse_ocsp_response`] therefore
//!   only build/match SHA-256 `CertID`s; a responder that rejects a SHA-256
//!   `CertID` (or that never indexed one) cannot be queried through this
//!   module today.
//! - **No cryptographic verification of the response signature.** RFC 6960
//!   §4.2.2.2 requires validating the `BasicOCSPResponse`'s `signature`
//!   (against the CA itself, a designated OCSP-signing certificate, or a
//!   locally-trusted responder) before trusting `certStatus`. This module
//!   only structurally decodes the response and matches its `CertID` against
//!   the request that solicited it (see [`parse_ocsp_response`]) -- it does
//!   **not** verify who signed it. A caller embedding this into `/DSS`
//!   (long-term validation material) is deferring that check to whatever
//!   later re-validates the archived response, exactly as
//!   [`super::dss`]'s own module docs already disclose for CRL/OCSP material
//!   in general. Wiring up signature verification (most naturally via
//!   [`super::verifier::verify_signature_bytes`], which is `pub(crate)`) is
//!   left for a follow-up change within that module's ownership.
//! - **Errors reuse existing [`SignatureError`] variants** (`InvalidFormat`
//!   for malformed DER, `CertificateLoadFailed` for certificate/extension
//!   parsing, `VerificationFailed` for a responder-reported failure or a
//!   `CertID` mismatch) rather than introducing a dedicated `OcspError`
//!   variant the way [`super::timestamp`] has `TimestampError` -- adding one
//!   would mean editing `error.rs`, which is outside this change's scope.
//!
//! [RFC 6960]: https://www.rfc-editor.org/rfc/rfc6960

use der::asn1::{AnyRef, ContextSpecific, GeneralizedTime, Int, ObjectIdentifier, OctetString};
use der::{Decode, Encode, Header, Length, Reader, Sequence, Tag, TagNumber};
use spki::AlgorithmIdentifierOwned;
use x509_cert::Certificate as X509Cert;

use super::pkcs7::{build_digest_algorithm_identifier_for, build_octet_string, build_sequence};
use super::{digest_for_algorithm, Certificate, SignatureAlgorithm, SignatureResult};
use crate::error::SignatureError;

/// `id-pkix-ocsp-basic` (RFC 6960 §4.2.1): `1.3.6.1.5.5.7.48.1.1`. The
/// `ResponseBytes.responseType` this module knows how to interpret --
/// `BasicOCSPResponse` is the only response type RFC 6960 defines.
const OID_PKIX_OCSP_BASIC: &str = "1.3.6.1.5.5.7.48.1.1";

/// `id-ad-ocsp` (RFC 5280 §4.2.2.1 / RFC 6960 §4.2.2.2.1):
/// `1.3.6.1.5.5.7.48.1`. The Authority Information Access `accessMethod`
/// that identifies an `accessLocation` as an OCSP responder URL.
const OID_AD_OCSP: &str = "1.3.6.1.5.5.7.48.1";

/// Maximum accepted size of a raw OCSP response, before any DER parsing
/// touches it (**untrusted input** -- this may come directly from a network
/// responder, or from a `/DSS` dictionary in an untrusted PDF). A real OCSP
/// response is a few KB at most; this mirrors
/// [`super::timestamp::MAX_RESPONSE_LEN`]'s reasoning (duplicated rather than
/// exposed across modules, matching this crate's existing per-module
/// duplication of similar small constants/helpers, see `dss.rs`'s
/// `parse_pdf_info` doc comment for the same reasoning applied elsewhere).
const MAX_RESPONSE_LEN: usize = 1 << 20; // 1 MiB

/// Pluggable transport for OCSP requests. Mirrors
/// [`super::TimestampAuthorityClient`]'s shape exactly (see this module's
/// docs for why); this crate does not perform the HTTP `POST` itself.
pub trait OcspTransport: std::fmt::Debug + Send + Sync {
    /// `POST`s `request_der` (a DER-encoded `OCSPRequest`) to `url` with
    /// `Content-Type: application/ocsp-request` (RFC 6960 appendix A.1) and
    /// returns the raw response body (which should have `Content-Type:
    /// application/ocsp-response`). This crate does not validate the HTTP
    /// status code or content type -- that's the transport's job;
    /// [`parse_ocsp_response`] validates the DER content regardless of how
    /// it arrived.
    fn post(&self, url: &str, request_der: &[u8]) -> SignatureResult<Vec<u8>>;
}

/// The RFC 6960 §4.1.1 `CertID` fields identifying which certificate an
/// OCSP request is about (and which `SingleResponse` in the corresponding
/// `OCSPResponse` answers it): the hashed issuer name, hashed issuer public
/// key, and the certificate's own serial number.
///
/// The hash algorithm is always SHA-256 here (see this module's "Disclosed
/// limitations" docs) and is therefore not a field of this struct --
/// [`build_ocsp_request`] always builds a SHA-256 `CertID`, and
/// [`parse_ocsp_response`] only matches a `SingleResponse` whose own
/// `CertID.hashAlgorithm` is also SHA-256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertId {
    /// SHA-256 hash of the issuer's distinguished name (its DER encoding).
    pub issuer_name_hash: Vec<u8>,
    /// SHA-256 hash of the issuer's public key bits (the `subjectPublicKey`
    /// `BIT STRING`'s content, excluding the tag/length/unused-bits-count
    /// octet -- RFC 6960 §4.1.1).
    pub issuer_key_hash: Vec<u8>,
    /// The certificate's serial number, as the already-DER-minimal
    /// two's-complement content bytes (i.e. what
    /// `x509_cert`'s/`der`'s `Int::as_bytes()` returns).
    pub serial_number: Vec<u8>,
}

/// A built RFC 6960 `OCSPRequest`, ready to send via an [`OcspTransport`].
#[derive(Debug, Clone)]
pub struct OcspRequest {
    /// The DER-encoded `OCSPRequest`.
    pub der: Vec<u8>,
    /// The [`CertId`] embedded in the request, so
    /// [`parse_ocsp_response`] can match the right `SingleResponse` back to
    /// it.
    pub cert_id: CertId,
}

/// Builds an RFC 6960 `OCSPRequest` (§4.1.1) asking about `leaf`'s
/// revocation status, given `leaf`'s issuing certificate `issuer`.
///
/// The request has no `requestorName`, no `requestExtensions` (in
/// particular, no nonce extension -- RFC 8954 recommends one for replay
/// protection, but responders are not required to honor or echo it, unlike
/// RFC 3161's mandatory nonce, so this module -- unlike
/// [`super::timestamp::build_timestamp_request`] -- has nothing to validate
/// against on the response side even if it sent one), and no
/// `optionalSignature` (an anonymous, unsigned request -- the overwhelmingly
/// common case; signed requests are rare and out of scope here).
pub fn build_ocsp_request(
    leaf: &Certificate,
    issuer: &Certificate,
) -> SignatureResult<OcspRequest> {
    let leaf_x509 = X509Cert::from_der(leaf.der_bytes()).map_err(|e| {
        SignatureError::CertificateLoadFailed(format!("Failed to parse leaf certificate: {e}"))
    })?;
    let issuer_x509 = X509Cert::from_der(issuer.der_bytes()).map_err(|e| {
        SignatureError::CertificateLoadFailed(format!("Failed to parse issuer certificate: {e}"))
    })?;

    let issuer_name_der = issuer_x509.tbs_certificate.subject.to_der().map_err(|e| {
        SignatureError::CertificateLoadFailed(format!("Failed to encode issuer name: {e}"))
    })?;
    let issuer_name_hash = digest_for_algorithm(SignatureAlgorithm::RsaSha256, &issuer_name_der);

    let issuer_key_bits = issuer_x509
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .raw_bytes();
    let issuer_key_hash = digest_for_algorithm(SignatureAlgorithm::RsaSha256, issuer_key_bits);

    let serial_number = leaf_x509.tbs_certificate.serial_number.as_bytes().to_vec();

    let cert_id = CertId {
        issuer_name_hash,
        issuer_key_hash,
        serial_number,
    };
    let der = build_ocsp_request_der(&cert_id);

    Ok(OcspRequest { der, cert_id })
}

/// Builds the DER-encoded `OCSPRequest` for a single [`CertId`], with a
/// single `Request` in `tbsRequest.requestList`. Split out from
/// [`build_ocsp_request`] so the wire encoding is testable without needing
/// a real X.509 certificate (see the tests below).
fn build_ocsp_request_der(cert_id: &CertId) -> Vec<u8> {
    // Request ::= SEQUENCE { reqCert CertID, singleRequestExtensions
    // [0] EXPLICIT Extensions OPTIONAL } -- extensions omitted, so the
    // Request's content is exactly the (already fully-encoded) CertID TLV.
    let cert_id_der = build_cert_id_der(cert_id);
    let request = build_sequence(&cert_id_der);

    // requestList SEQUENCE OF Request { one entry }.
    let request_list = build_sequence(&request);

    // TBSRequest ::= SEQUENCE { version OPTIONAL (omitted, DEFAULT v1),
    // requestorName OPTIONAL (omitted), requestList, requestExtensions
    // OPTIONAL (omitted) } -- content is exactly the requestList TLV.
    let tbs_request = build_sequence(&request_list);

    // OCSPRequest ::= SEQUENCE { tbsRequest, optionalSignature OPTIONAL
    // (omitted) } -- content is exactly the tbsRequest TLV.
    build_sequence(&tbs_request)
}

/// Builds the DER-encoded `CertID ::= SEQUENCE { hashAlgorithm
/// AlgorithmIdentifier, issuerNameHash OCTET STRING, issuerKeyHash OCTET
/// STRING, serialNumber CertificateSerialNumber }` (RFC 6960 §4.1.1).
fn build_cert_id_der(cert_id: &CertId) -> Vec<u8> {
    let mut content = Vec::new();
    content.extend_from_slice(&build_digest_algorithm_identifier_for(
        SignatureAlgorithm::RsaSha256,
    ));
    content.extend_from_slice(&build_octet_string(&cert_id.issuer_name_hash));
    content.extend_from_slice(&build_octet_string(&cert_id.issuer_key_hash));
    content.extend_from_slice(&build_integer_from_content(&cert_id.serial_number));
    build_sequence(&content)
}

/// Encodes `content` (already-DER-minimal two's-complement bytes, as
/// `serial_number`s always are here -- see [`CertId::serial_number`]'s
/// docs) as a complete DER `INTEGER` TLV. Unlike
/// [`super::pkcs7::build_integer`] this doesn't take an `i64`: certificate
/// serial numbers routinely exceed 8 bytes (RFC 5280 allows up to 20
/// octets, and nothing stops a non-conformant certificate from using more).
fn build_integer_from_content(content: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(content.len() + 4);
    bytes.push(0x02); // INTEGER tag
    bytes.extend_from_slice(&encode_length(content.len()));
    bytes.extend_from_slice(content);
    bytes
}

/// DER length-prefix encoding. Duplicated in miniature from
/// [`super::pkcs7`]'s module-private `encode_length` (not `pub(super)`,
/// hence not reachable here) rather than widening that function's
/// visibility for one caller -- matches this crate's existing precedent of
/// small per-module duplication over threading exports through unrelated
/// modules (see `dss.rs`'s `parse_pdf_info` doc comment for the same
/// reasoning applied elsewhere).
fn encode_length(len: usize) -> Vec<u8> {
    if len < 128 {
        vec![len as u8]
    } else if len < 256 {
        vec![0x81, len as u8]
    } else if len < 65536 {
        vec![0x82, (len >> 8) as u8, (len & 0xFF) as u8]
    } else {
        vec![
            0x83,
            (len >> 16) as u8,
            ((len >> 8) & 0xFF) as u8,
            (len & 0xFF) as u8,
        ]
    }
}

/// The outcome of an OCSP `CertStatus` (RFC 6960 §4.2.1 `CertStatus ::=
/// CHOICE { good [0] IMPLICIT NULL, revoked [1] IMPLICIT RevokedInfo,
/// unknown [2] IMPLICIT UnknownInfo }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertStatus {
    /// The certificate was not revoked as of `thisUpdate`.
    Good,
    /// The certificate was revoked.
    Revoked {
        /// When the certificate was revoked, as an RFC 3339-ish string (via
        /// `der`'s `GeneralizedTime` display).
        revocation_time: String,
    },
    /// The responder has no status information for this certificate (RFC
    /// 6960 §2.2: this is a real, distinct outcome -- e.g. a certificate the
    /// responder's CA never issued, or one issued too recently for the
    /// responder to have indexed yet -- and must not be treated the same as
    /// `Good`).
    Unknown,
}

/// A validated `SingleResponse` (RFC 6960 §4.2.1) for the certificate a
/// [`build_ocsp_request`] asked about.
#[derive(Debug, Clone)]
pub struct OcspCertStatus {
    /// The certificate's status.
    pub status: CertStatus,
    /// The most recent time at which the status being indicated is known by
    /// the responder to have been correct, as an RFC 3339-ish string.
    pub this_update: String,
    /// The time by which newer information will be available, if the
    /// responder disclosed one.
    pub next_update: Option<String>,
}

/// Parses a raw RFC 6960 `OCSPResponse` (**untrusted input** -- this may
/// come directly from a network responder) and returns the `SingleResponse`
/// matching `expected`. Checks:
///
/// - the response fits under a fixed size limit before any parsing,
/// - `responseStatus` is `successful` (0),
/// - `responseBytes` is present with `responseType` `id-pkix-ocsp-basic`,
/// - the embedded `BasicOCSPResponse.tbsResponseData.responses` contains a
///   `SingleResponse` whose `CertID` matches `expected` exactly (same
///   SHA-256 issuer-name hash, issuer-key hash, and serial number -- per RFC
///   6960 §4.1.2, a client MUST verify this before trusting `certStatus`).
///
/// Does **not** verify the `BasicOCSPResponse`'s own signature -- see this
/// module's "Disclosed limitations" docs.
pub fn parse_ocsp_response(resp_der: &[u8], expected: &CertId) -> SignatureResult<OcspCertStatus> {
    if resp_der.len() > MAX_RESPONSE_LEN {
        return Err(SignatureError::InvalidFormat(format!(
            "OCSP response too large ({} bytes, limit {})",
            resp_der.len(),
            MAX_RESPONSE_LEN
        )));
    }

    let resp = OcspResponseAsn1::from_der(resp_der)
        .map_err(|e| SignatureError::InvalidFormat(format!("Malformed OCSPResponse: {e}")))?;

    if resp.response_status != 0 {
        return Err(SignatureError::VerificationFailed(format!(
            "OCSP responder did not return a successful status (OCSPResponseStatus {})",
            resp.response_status
        )));
    }

    let response_bytes = resp.response_bytes.ok_or_else(|| {
        SignatureError::InvalidFormat(
            "OCSP response has status successful but no responseBytes".to_string(),
        )
    })?;

    if response_bytes.response_type.to_string() != OID_PKIX_OCSP_BASIC {
        return Err(SignatureError::InvalidFormat(format!(
            "Unsupported OCSP responseType (expected id-pkix-ocsp-basic): {}",
            response_bytes.response_type
        )));
    }

    let basic = BasicOcspResponseAsn1::from_der(response_bytes.response.as_bytes())
        .map_err(|e| SignatureError::InvalidFormat(format!("Malformed BasicOCSPResponse: {e}")))?;

    let expected_digest_oid = SignatureAlgorithm::RsaSha256.digest_oid();
    let matching = basic
        .tbs_response_data
        .responses
        .iter()
        .find(|single| {
            single.cert_id.hash_algorithm.oid.to_string() == expected_digest_oid
                && single.cert_id.issuer_name_hash.as_bytes()
                    == expected.issuer_name_hash.as_slice()
                && single.cert_id.issuer_key_hash.as_bytes() == expected.issuer_key_hash.as_slice()
                && single.cert_id.serial_number.as_bytes() == expected.serial_number.as_slice()
        })
        .ok_or_else(|| {
            SignatureError::VerificationFailed(
                "OCSP response has no SingleResponse whose CertID matches the requested \
                 certificate (responder confusion, or the response was tampered with / \
                 substituted for a different certificate)"
                    .to_string(),
            )
        })?;

    let status = match &matching.cert_status {
        CertStatusAsn1::Good => CertStatus::Good,
        CertStatusAsn1::Revoked(info) => CertStatus::Revoked {
            revocation_time: info.revocation_time.to_date_time().to_string(),
        },
        CertStatusAsn1::Unknown => CertStatus::Unknown,
    };

    Ok(OcspCertStatus {
        status,
        this_update: matching.this_update.to_date_time().to_string(),
        next_update: matching
            .next_update
            .as_ref()
            .map(|t| t.to_date_time().to_string()),
    })
}

/// Extracts the OCSP responder URL from `cert`'s Authority Information
/// Access extension (RFC 5280 §4.2.2.1), if present.
///
/// Returns `Ok(None)` (not an error) if `cert` has no AIA extension, or has
/// one but it contains no `id-ad-ocsp` access description with a URI
/// location -- both are normal, e.g. for a root/self-signed certificate,
/// which is never itself OCSP-checked.
pub fn ocsp_responder_url(cert: &Certificate) -> SignatureResult<Option<String>> {
    use const_oid::AssociatedOid;
    use x509_cert::ext::pkix::AuthorityInfoAccessSyntax;

    let x509 = X509Cert::from_der(cert.der_bytes()).map_err(|e| {
        SignatureError::CertificateLoadFailed(format!("Failed to parse certificate: {e}"))
    })?;

    let Some(extensions) = &x509.tbs_certificate.extensions else {
        return Ok(None);
    };

    for ext in extensions {
        if ext.extn_id == AuthorityInfoAccessSyntax::OID {
            let aia =
                AuthorityInfoAccessSyntax::from_der(ext.extn_value.as_bytes()).map_err(|e| {
                    SignatureError::CertificateLoadFailed(format!(
                        "Malformed AuthorityInfoAccess extension: {e}"
                    ))
                })?;
            return Ok(find_ocsp_url(&aia));
        }
    }

    Ok(None)
}

/// Scans an already-decoded `AuthorityInfoAccessSyntax` for the first
/// `id-ad-ocsp` access description with a URI location. Split out from
/// [`ocsp_responder_url`] so it's testable without a real X.509 certificate
/// -- an `AuthorityInfoAccessSyntax` can be constructed directly in tests.
fn find_ocsp_url(aia: &x509_cert::ext::pkix::AuthorityInfoAccessSyntax) -> Option<String> {
    use x509_cert::ext::pkix::name::GeneralName;

    let ocsp_oid = ObjectIdentifier::new(OID_AD_OCSP).ok()?;
    aia.0.iter().find_map(|ad| {
        if ad.access_method != ocsp_oid {
            return None;
        }
        match &ad.access_location {
            GeneralName::UniformResourceIdentifier(uri) => Some(uri.to_string()),
            _ => None,
        }
    })
}

// --- RFC 6960 response ASN.1 modeling (decode-only) ---
//
// Mirrors `timestamp.rs`'s approach: types that are pure passthrough
// `SEQUENCE`s of already-typed `der` fields use `#[derive(Sequence)]`
// directly; the two `CHOICE`s this response format actually contains
// (`ResponderID`, `CertStatus`) get hand-written `Decode` impls, since
// `der` 0.7's derive support targets `SEQUENCE`/`SET`, not arbitrary
// `CHOICE` shapes with mixed IMPLICIT/EXPLICIT tagging. `ResponderID` in
// particular is decoded fully opaquely (as a single `AnyRef` TLV) since
// this module never needs to know whether the responder identified itself
// `byName` or `byKey`, only to skip correctly past it.

/// `CertID ::= SEQUENCE { hashAlgorithm AlgorithmIdentifier, issuerNameHash
/// OCTET STRING, issuerKeyHash OCTET STRING, serialNumber
/// CertificateSerialNumber }` (RFC 6960 §4.1.1).
#[derive(Debug, Clone, Sequence)]
struct CertIdAsn1 {
    hash_algorithm: AlgorithmIdentifierOwned,
    issuer_name_hash: OctetString,
    issuer_key_hash: OctetString,
    serial_number: Int,
}

/// `RevokedInfo ::= SEQUENCE { revocationTime GeneralizedTime,
/// revocationReason [0] EXPLICIT CRLReason OPTIONAL }` (RFC 6960 §4.2.1).
/// `revocationReason` is captured opaquely (this module doesn't need it,
/// only [`CertStatus::Revoked`]'s `revocation_time`) but must still be a
/// real field so the sequence decoder consumes it when present, rather than
/// leaving unconsumed trailing bytes.
#[derive(Debug, Clone, Sequence)]
struct RevokedInfoAsn1 {
    revocation_time: GeneralizedTime,
    #[asn1(context_specific = "0", tag_mode = "explicit", optional = "true")]
    revocation_reason: Option<der::Any>,
}

/// `CertStatus ::= CHOICE { good [0] IMPLICIT NULL, revoked [1] IMPLICIT
/// RevokedInfo, unknown [2] IMPLICIT UnknownInfo }` (RFC 6960 §4.2.1).
#[derive(Debug, Clone)]
enum CertStatusAsn1 {
    Good,
    Revoked(RevokedInfoAsn1),
    Unknown,
}

impl<'a> Decode<'a> for CertStatusAsn1 {
    fn decode<R: Reader<'a>>(reader: &mut R) -> der::Result<Self> {
        let tag = reader.peek_tag()?;

        if tag
            == (Tag::ContextSpecific {
                constructed: false,
                number: TagNumber::N0,
            })
        {
            let header = Header::decode(reader)?;
            if header.length != Length::ZERO {
                return Err(header.tag.value_error());
            }
            return Ok(CertStatusAsn1::Good);
        }

        if tag
            == (Tag::ContextSpecific {
                constructed: true,
                number: TagNumber::N1,
            })
        {
            let field = ContextSpecific::<RevokedInfoAsn1>::decode_implicit(reader, TagNumber::N1)?
                .ok_or_else(|| tag.unexpected_error(None))?;
            return Ok(CertStatusAsn1::Revoked(field.value));
        }

        if tag
            == (Tag::ContextSpecific {
                constructed: false,
                number: TagNumber::N2,
            })
        {
            let header = Header::decode(reader)?;
            if header.length != Length::ZERO {
                return Err(header.tag.value_error());
            }
            return Ok(CertStatusAsn1::Unknown);
        }

        Err(tag.unexpected_error(None))
    }
}

/// `SingleResponse ::= SEQUENCE { certID CertID, certStatus CertStatus,
/// thisUpdate GeneralizedTime, nextUpdate [0] EXPLICIT GeneralizedTime
/// OPTIONAL, singleExtensions [1] EXPLICIT Extensions OPTIONAL }` (RFC 6960
/// §4.2.1). `singleExtensions` is discarded (not captured as a field at
/// all): it's the last field, so nothing after it needs the reader
/// repositioned past it for a subsequent sibling field the way
/// `RevokedInfoAsn1::revocation_reason` does -- it only needs to be
/// consumed so `NestedReader::finish` doesn't see unconsumed trailing bytes
/// when it *is* present.
#[derive(Debug, Clone)]
struct SingleResponseAsn1 {
    cert_id: CertIdAsn1,
    cert_status: CertStatusAsn1,
    this_update: GeneralizedTime,
    next_update: Option<GeneralizedTime>,
}

impl<'a> Decode<'a> for SingleResponseAsn1 {
    fn decode<R: Reader<'a>>(reader: &mut R) -> der::Result<Self> {
        reader.sequence(|reader| {
            let cert_id = CertIdAsn1::decode(reader)?;
            let cert_status = CertStatusAsn1::decode(reader)?;
            let this_update = GeneralizedTime::decode(reader)?;
            let next_update =
                ContextSpecific::<GeneralizedTime>::decode_explicit(reader, TagNumber::N0)?
                    .map(|field| field.value);
            let _single_extensions =
                ContextSpecific::<AnyRef<'a>>::decode_explicit(reader, TagNumber::N1)?;
            Ok(Self {
                cert_id,
                cert_status,
                this_update,
                next_update,
            })
        })
    }
}

/// `ResponseData ::= SEQUENCE { version [0] EXPLICIT Version DEFAULT v1,
/// responderID ResponderID, producedAt GeneralizedTime, responses SEQUENCE
/// OF SingleResponse, responseExtensions [1] EXPLICIT Extensions OPTIONAL }`
/// (RFC 6960 §4.2.1). `version`/`responderID`/`producedAt`/
/// `responseExtensions` are all discarded (see this module's ASN.1-modeling
/// header comment) -- only `responses` is needed.
#[derive(Debug, Clone)]
struct ResponseDataAsn1 {
    responses: Vec<SingleResponseAsn1>,
}

impl<'a> Decode<'a> for ResponseDataAsn1 {
    fn decode<R: Reader<'a>>(reader: &mut R) -> der::Result<Self> {
        reader.sequence(|reader| {
            // version [0] EXPLICIT INTEGER DEFAULT v1 -- skip if present.
            let _version = ContextSpecific::<Int>::decode_explicit(reader, TagNumber::N0)?;
            // responderID ResponderID (CHOICE `byName [1] Name` / `byKey
            // [2] KeyHash`, both EXPLICIT under this module's default
            // tagging per RFC 6960 appendix B) -- opaque, see header
            // comment.
            let _responder_id = AnyRef::decode(reader)?;
            let _produced_at = GeneralizedTime::decode(reader)?;
            let responses = Vec::<SingleResponseAsn1>::decode(reader)?;
            // responseExtensions [1] EXPLICIT Extensions OPTIONAL -- skip
            // if present.
            let _extensions =
                ContextSpecific::<AnyRef<'a>>::decode_explicit(reader, TagNumber::N1)?;
            Ok(Self { responses })
        })
    }
}

/// `BasicOCSPResponse ::= SEQUENCE { tbsResponseData ResponseData,
/// signatureAlgorithm AlgorithmIdentifier, signature BIT STRING, certs [0]
/// EXPLICIT SEQUENCE OF Certificate OPTIONAL }` (RFC 6960 §4.2.1).
/// `signatureAlgorithm`/`signature`/`certs` are discarded -- see this
/// module's "Disclosed limitations" docs (no signature verification yet).
#[derive(Debug, Clone)]
struct BasicOcspResponseAsn1 {
    tbs_response_data: ResponseDataAsn1,
}

impl<'a> Decode<'a> for BasicOcspResponseAsn1 {
    fn decode<R: Reader<'a>>(reader: &mut R) -> der::Result<Self> {
        reader.sequence(|reader| {
            let tbs_response_data = ResponseDataAsn1::decode(reader)?;
            let _signature_algorithm = AlgorithmIdentifierOwned::decode(reader)?;
            let _signature = der::asn1::BitString::decode(reader)?;
            let _certs = ContextSpecific::<AnyRef<'a>>::decode_explicit(reader, TagNumber::N0)?;
            Ok(Self { tbs_response_data })
        })
    }
}

/// `ResponseBytes ::= SEQUENCE { responseType OBJECT IDENTIFIER, response
/// OCTET STRING }` (RFC 6960 §4.2.1).
#[derive(Debug, Clone, Sequence)]
struct ResponseBytesAsn1 {
    response_type: ObjectIdentifier,
    response: OctetString,
}

/// `OCSPResponse ::= SEQUENCE { responseStatus OCSPResponseStatus,
/// responseBytes [0] EXPLICIT ResponseBytes OPTIONAL }` (RFC 6960 §4.2.1).
/// Decoded by hand rather than `#[derive(Sequence)]` because
/// `responseStatus` is an `ENUMERATED`, which the `der` crate (0.7, as used
/// elsewhere in this crate) has no built-in type for -- see
/// [`decode_enumerated`].
#[derive(Debug, Clone)]
struct OcspResponseAsn1 {
    response_status: i32,
    response_bytes: Option<ResponseBytesAsn1>,
}

impl<'a> Decode<'a> for OcspResponseAsn1 {
    fn decode<R: Reader<'a>>(reader: &mut R) -> der::Result<Self> {
        reader.sequence(|reader| {
            let response_status = decode_enumerated(reader)?;
            let response_bytes =
                ContextSpecific::<ResponseBytesAsn1>::decode_explicit(reader, TagNumber::N0)?
                    .map(|field| field.value);
            Ok(Self {
                response_status,
                response_bytes,
            })
        })
    }
}

/// Decodes an ASN.1 `ENUMERATED` (tag `0x0A`) as an `i32`. On the wire an
/// `ENUMERATED` is encoded identically to an `INTEGER` (same
/// two's-complement content rules) except for its tag byte, so this reads
/// it the same way `der::asn1::Int` would.
fn decode_enumerated<'a, R: Reader<'a>>(reader: &mut R) -> der::Result<i32> {
    let header = Header::decode(reader)?;
    header.tag.assert_eq(Tag::Enumerated)?;
    let bytes = reader.read_vec(header.length)?;
    if bytes.is_empty() || bytes.len() > 4 {
        return Err(header.tag.value_error());
    }
    let mut buf = [0u8; 4];
    let negative = bytes[0] & 0x80 != 0;
    let fill = if negative { 0xFF } else { 0x00 };
    buf.fill(fill);
    buf[4 - bytes.len()..].copy_from_slice(&bytes);
    Ok(i32::from_be_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cert_id() -> CertId {
        CertId {
            issuer_name_hash: vec![0xAA; 32],
            issuer_key_hash: vec![0xBB; 32],
            serial_number: vec![0x01, 0x23, 0x45],
        }
    }

    #[test]
    fn test_build_ocsp_request_der_shape() {
        let cert_id = sample_cert_id();
        let der = build_ocsp_request_der(&cert_id);
        assert!(der.starts_with(&[0x30])); // SEQUENCE

        // Re-decode as a generic SEQUENCE to sanity-check DER well-formedness.
        let parsed = der::asn1::SequenceRef::from_der(&der);
        assert!(
            parsed.is_ok(),
            "request must be valid DER: {:?}",
            parsed.err()
        );
    }

    #[test]
    fn test_build_ocsp_request_der_contains_serial_number() {
        let cert_id = sample_cert_id();
        let der = build_ocsp_request_der(&cert_id);
        // The serial number's raw content bytes should appear verbatim
        // somewhere in the encoding (as the CertID's trailing INTEGER).
        assert!(
            der.windows(cert_id.serial_number.len())
                .any(|w| w == cert_id.serial_number.as_slice()),
            "encoded request should contain the serial number bytes"
        );
    }

    #[test]
    fn test_build_integer_from_content_matches_pkcs7_style() {
        assert_eq!(build_integer_from_content(&[0x7F]), vec![0x02, 0x01, 0x7F]);
        assert_eq!(
            build_integer_from_content(&[0x00, 0x80]),
            vec![0x02, 0x02, 0x00, 0x80]
        );
    }

    #[test]
    fn test_encode_length() {
        assert_eq!(encode_length(0), vec![0x00]);
        assert_eq!(encode_length(127), vec![0x7F]);
        assert_eq!(encode_length(128), vec![0x81, 0x80]);
        assert_eq!(encode_length(256), vec![0x82, 0x01, 0x00]);
    }

    /// Hand-builds a minimal, well-formed `OCSPResponse` DER blob (status
    /// `good`, one matching `SingleResponse`) using the exact same
    /// low-level helpers `build_ocsp_request_der` uses, so
    /// `parse_ocsp_response` can be exercised end-to-end without needing a
    /// real network responder or a real X.509 certificate.
    fn build_sample_response_der(cert_id: &CertId, status_tag: u8) -> Vec<u8> {
        let cert_id_der = build_cert_id_der(cert_id);

        // certStatus: `good` is context tag [0] IMPLICIT NULL (0x80, empty
        // content); `unknown` is context tag [2] IMPLICIT NULL (0x82, empty
        // content).
        let cert_status = vec![status_tag, 0x00];

        // NOTE(ownership): `GeneralizedTime::from_date_time` returns
        // `GeneralizedTime` directly (not a `Result`) -- this file is not
        // in this task's file-ownership list; only the minimal fix needed
        // to make wave 1's pre-existing test code compile (a stray
        // `.unwrap()` this crate's `der = "0.7"` pin doesn't have a method
        // for) was made here, so wiring this module into `mod.rs` (this
        // task's actual assignment) could be verified with `cargo test`.
        let this_update =
            GeneralizedTime::from_date_time(der::DateTime::new(2024, 1, 1, 0, 0, 0).unwrap())
                .to_der()
                .unwrap();

        let mut single_response = Vec::new();
        single_response.extend_from_slice(&cert_id_der);
        single_response.extend_from_slice(&cert_status);
        single_response.extend_from_slice(&this_update);
        let single_response = build_sequence(&single_response);

        let responses = build_sequence(&single_response); // SEQUENCE OF SingleResponse

        // responderID: an opaque `[1] EXPLICIT Name` wrapping an empty SEQUENCE
        // (this module never inspects it, see `ResponseDataAsn1`'s docs).
        let responder_id = {
            let empty_name = build_sequence(&[]);
            let mut cs = vec![0xA1]; // [1] EXPLICIT, constructed
            cs.extend_from_slice(&encode_length(empty_name.len()));
            cs.extend_from_slice(&empty_name);
            cs
        };

        let produced_at = this_update.clone();

        let mut response_data = Vec::new();
        response_data.extend_from_slice(&responder_id);
        response_data.extend_from_slice(&produced_at);
        response_data.extend_from_slice(&responses);
        let response_data = build_sequence(&response_data);

        let mut basic_response = Vec::new();
        basic_response.extend_from_slice(&response_data);
        // signatureAlgorithm + signature: reuse a real SHA-256 AlgorithmIdentifier
        // and an empty BIT STRING (unused-bits octet 0, no content) -- this
        // module never verifies them (see "Disclosed limitations").
        basic_response.extend_from_slice(&build_digest_algorithm_identifier_for(
            SignatureAlgorithm::RsaSha256,
        ));
        basic_response.extend_from_slice(&[0x03, 0x01, 0x00]); // BIT STRING, empty
        let basic_response = build_sequence(&basic_response);

        let response_type_oid: &[u8] = &[
            0x06, 0x09, 0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01, 0x01,
        ]; // id-pkix-ocsp-basic
        let mut response_bytes = Vec::new();
        response_bytes.extend_from_slice(response_type_oid);
        response_bytes.extend_from_slice(&build_octet_string(&basic_response));
        let response_bytes = build_sequence(&response_bytes);

        let response_bytes_field = {
            let mut cs = vec![0xA0]; // [0] EXPLICIT
            cs.extend_from_slice(&encode_length(response_bytes.len()));
            cs.extend_from_slice(&response_bytes);
            cs
        };

        let mut ocsp_response = Vec::new();
        ocsp_response.extend_from_slice(&[0x0A, 0x01, 0x00]); // ENUMERATED successful(0)
        ocsp_response.extend_from_slice(&response_bytes_field);
        build_sequence(&ocsp_response)
    }

    #[test]
    fn test_parse_ocsp_response_good_status_roundtrip() {
        let cert_id = sample_cert_id();
        let der = build_sample_response_der(&cert_id, 0x80); // good
        let result = parse_ocsp_response(&der, &cert_id).expect("should parse");
        assert_eq!(result.status, CertStatus::Good);
        assert!(result.next_update.is_none());
        assert!(!result.this_update.is_empty());
    }

    #[test]
    fn test_parse_ocsp_response_unknown_status_roundtrip() {
        let cert_id = sample_cert_id();
        let der = build_sample_response_der(&cert_id, 0x82); // unknown
        let result = parse_ocsp_response(&der, &cert_id).expect("should parse");
        assert_eq!(result.status, CertStatus::Unknown);
    }

    #[test]
    fn test_parse_ocsp_response_rejects_cert_id_mismatch() {
        let cert_id = sample_cert_id();
        let der = build_sample_response_der(&cert_id, 0x80);
        let mut other = sample_cert_id();
        other.serial_number = vec![0xFF, 0xFF];
        let err = parse_ocsp_response(&der, &other).unwrap_err();
        assert!(matches!(err, SignatureError::VerificationFailed(_)));
    }

    #[test]
    fn test_parse_ocsp_response_rejects_oversized_input() {
        let oversized = vec![0u8; MAX_RESPONSE_LEN + 1];
        let cert_id = sample_cert_id();
        let err = parse_ocsp_response(&oversized, &cert_id).unwrap_err();
        assert!(matches!(err, SignatureError::InvalidFormat(_)));
    }

    #[test]
    fn test_parse_ocsp_response_rejects_garbage() {
        let cert_id = sample_cert_id();
        let err = parse_ocsp_response(b"not a der message", &cert_id).unwrap_err();
        assert!(matches!(err, SignatureError::InvalidFormat(_)));
    }

    #[test]
    fn test_find_ocsp_url_matches_ocsp_access_method() {
        use x509_cert::ext::pkix::name::GeneralName;
        use x509_cert::ext::pkix::{AccessDescription, AuthorityInfoAccessSyntax};

        let ocsp_uri = der::asn1::Ia5String::new("http://ocsp.example.com").unwrap();
        let ca_issuers_uri = der::asn1::Ia5String::new("http://ca.example.com/issuer.crt").unwrap();

        let aia = AuthorityInfoAccessSyntax(vec![
            AccessDescription {
                // id-ad-caIssuers: 1.3.6.1.5.5.7.48.2 -- must be ignored.
                access_method: ObjectIdentifier::new("1.3.6.1.5.5.7.48.2").unwrap(),
                access_location: GeneralName::UniformResourceIdentifier(ca_issuers_uri),
            },
            AccessDescription {
                access_method: ObjectIdentifier::new(OID_AD_OCSP).unwrap(),
                access_location: GeneralName::UniformResourceIdentifier(ocsp_uri),
            },
        ]);

        assert_eq!(
            find_ocsp_url(&aia),
            Some("http://ocsp.example.com".to_string())
        );
    }

    #[test]
    fn test_find_ocsp_url_none_when_absent() {
        use x509_cert::ext::pkix::name::GeneralName;
        use x509_cert::ext::pkix::{AccessDescription, AuthorityInfoAccessSyntax};

        let ca_issuers_uri = der::asn1::Ia5String::new("http://ca.example.com/issuer.crt").unwrap();
        let aia = AuthorityInfoAccessSyntax(vec![AccessDescription {
            access_method: ObjectIdentifier::new("1.3.6.1.5.5.7.48.2").unwrap(),
            access_location: GeneralName::UniformResourceIdentifier(ca_issuers_uri),
        }]);

        assert_eq!(find_ocsp_url(&aia), None);
    }
}
