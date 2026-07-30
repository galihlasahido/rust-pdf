//! RFC 5280 Certificate Revocation List (CRL) support.
//!
//! Complements [`super::chain`] (path building/signature validation) and
//! [`super::dss`] (embedding already-fetched revocation material into a
//! PDF's `/DSS`) with the piece neither of those provide: turning a
//! certificate's `id-ce-cRLDistributionPoints` extension into a URL, parsing
//! the DER `CertificateList` a CA publishes at that URL (RFC 5280 §5.1), and
//! checking whether a given certificate's serial number is in it.
//!
//! # Why this module doesn't do its own HTTP
//!
//! Same reasoning as [`super::timestamp`]'s TSA transport and `dss`'s stated
//! scope: fetching a CRL over HTTP (or LDAP, per RFC 5280 §4.2.1.13) means
//! picking a TLS stack, a proxy policy, timeouts, redirect handling -- all
//! things a host application (e.g. a Tauri desktop app) already has an
//! opinion on and already does. This crate stays out of that decision;
//! callers implement [`CrlTransport`] with whatever HTTP client they already
//! use and hand it to whatever drives long-term-validation assembly.
//!
//! # What this module does *not* do (disclosed limitations)
//!
//! - **Does not verify the CRL's own signature.** [`parse_crl`] only checks
//!   DER well-formedness; it is the caller's responsibility to verify
//!   `CertificateList.signature` against the issuing CA's public key before
//!   trusting `revokedCertificates` (e.g. via
//!   [`super::verifier::verify_signature_bytes`], crate-internal, over
//!   `tbs_cert_list`'s re-encoded DER -- the same signature-verification
//!   primitive [`super::chain`] uses for certificate signatures). A CRL
//!   fetched from an untrusted network location and used without this check
//!   is worthless from a security standpoint.
//! - **Does not check `thisUpdate`/`nextUpdate` freshness.** [`Crl`] exposes
//!   both as Unix timestamps so callers can decide their own staleness
//!   policy; this module does not reject an expired CRL on its own.
//! - **Does not handle indirect CRLs** (`IssuingDistributionPoint`'s
//!   `indirectCRL` flag, or a `crlIssuer` on the distribution point that
//!   differs from the certificate's own issuer) or **delta CRLs**
//!   (`deltaCRLIndicator`). Both are substantial additional RFC 5280
//!   machinery (essentially replicating a chunk of a mature CRL processor
//!   like OpenSSL's) that is out of scope here; a CRL using either is parsed
//!   structurally but its `crlExtensions` are not interpreted.
//! - **Does not act on per-entry `crlEntryExtensions`** (e.g. `CRLReason`,
//!   `certificateHold`) -- [`Crl::contains_serial`] only answers "is this
//!   serial number listed at all", matching item 3 of this module's scope.

use der::Decode;
use x509_cert::crl::CertificateList;
use x509_cert::ext::pkix::name::{DistributionPointName, GeneralName};
use x509_cert::ext::pkix::CrlDistributionPoints;
use x509_cert::Certificate as X509Cert;

use super::{Certificate, SignatureResult};
use crate::error::SignatureError;

/// `id-ce-cRLDistributionPoints` (RFC 5280 §4.2.1.13): `2.5.29.31`.
///
/// Exposed as a string (rather than only via `CrlDistributionPoints::OID`)
/// so callers matching raw extension OIDs elsewhere in this crate (as
/// [`super::timestamp`] and [`super::chain`] do for their own OIDs) have a
/// documented constant to compare against without pulling in
/// `const_oid::AssociatedOid` themselves.
pub const OID_CRL_DISTRIBUTION_POINTS: &str = "2.5.29.31";

/// Upper bound on how large a DER-encoded `CertificateList` [`parse_crl`]
/// will accept, checked before any ASN.1 parsing touches the input. A CRL
/// may legitimately be fetched from an untrusted network location (via a
/// caller's [`CrlTransport`] implementation); large CAs with big revoked
/// sets can produce multi-megabyte CRLs, so this is generous compared to
/// [`super::timestamp`]'s 1 MiB TSA-response bound, but still bounded.
const MAX_CRL_LEN: usize = 32 * 1024 * 1024; // 32 MiB

/// Pluggable transport for fetching a CRL from its distribution point URL.
///
/// Mirrors [`super::timestamp::TimestampAuthorityClient`] and the shape
/// described for an OCSP transport in `dss`'s module docs: implementations
/// issue whatever `GET` (or LDAP lookup, per RFC 5280 §4.2.1.13, though HTTP
/// distribution points are overwhelmingly the common case in practice) is
/// appropriate for `url` and return the raw response body. This crate does
/// not validate HTTP status codes or `Content-Type` -- that is the
/// transport's job; [`parse_crl`] validates the DER content regardless of
/// how it arrived.
pub trait CrlTransport: std::fmt::Debug + Send + Sync {
    /// Fetches the raw bytes at `url` (expected to be a DER-encoded
    /// `CertificateList`). Returns `Err` for any transport-level failure
    /// (network error, non-2xx status, timeout, etc.) -- implementations
    /// should use [`SignatureError::VerificationFailed`] to report those,
    /// since this crate defines no CRL-specific error variant.
    fn fetch(&self, url: &str) -> SignatureResult<Vec<u8>>;
}

/// Extracts the first usable CRL distribution point URL from `cert`'s
/// `id-ce-cRLDistributionPoints` extension (RFC 5280 §4.2.1.13), if present.
///
/// A certificate may list several `DistributionPoint`s, each of which may
/// carry several `GeneralName`s in its `fullName` choice; this returns the
/// first `uniformResourceIdentifier` name found, scanning distribution
/// points in the order they appear in the certificate. Returns `None` if:
/// - the certificate has no `id-ce-cRLDistributionPoints` extension,
/// - the extension is present but malformed,
/// - every distribution point uses `nameRelativeToCRLIssuer` (relative to
///   the issuer's own name, requiring out-of-band knowledge of the issuer's
///   CRL-hosting convention -- rare in practice and not supported here), or
/// - every distribution point's `fullName` contains no URI (e.g. only an
///   LDAP directory name).
///
/// This does not fetch anything -- pass the returned URL to a
/// [`CrlTransport`] implementation to actually retrieve the CRL.
pub fn extract_crl_distribution_point_url(cert: &Certificate) -> Option<String> {
    let x509 = X509Cert::from_der(cert.der_bytes()).ok()?;
    let extensions = x509.tbs_certificate.extensions.as_ref()?;

    let ext = extensions
        .iter()
        .find(|ext| ext.extn_id.to_string() == OID_CRL_DISTRIBUTION_POINTS)?;
    let distribution_points = CrlDistributionPoints::from_der(ext.extn_value.as_bytes()).ok()?;

    for dp in &distribution_points.0 {
        let Some(DistributionPointName::FullName(names)) = &dp.distribution_point else {
            continue;
        };
        for name in names {
            if let GeneralName::UniformResourceIdentifier(uri) = name {
                return Some(uri.as_str().to_string());
            }
        }
    }
    None
}

/// A parsed RFC 5280 `CertificateList` (CRL).
///
/// Retains the original DER bytes (so a caller can independently verify the
/// CRL's signature, which this module deliberately does not do -- see the
/// module docs) alongside the fields needed to answer a revocation-status
/// query.
#[derive(Debug, Clone)]
pub struct Crl {
    der_bytes: Vec<u8>,
    /// The issuer `Name`, rendered via `x509_cert`'s RFC 4514-ish `Display`
    /// impl, for diagnostics/logging.
    issuer: String,
    /// `thisUpdate`, as seconds since the Unix epoch.
    this_update: u64,
    /// `nextUpdate`, as seconds since the Unix epoch, if the CRL includes
    /// one (RFC 5280 §5.1.2.5 says CAs "SHOULD" include it but does not
    /// require it).
    next_update: Option<u64>,
    /// Serial numbers of every entry in `revokedCertificates`, each as the
    /// minimal big-endian byte representation `SerialNumber::as_bytes`
    /// produces (leading zero bytes stripped) -- the same representation
    /// [`Certificate`]'s own serial-number bytes are compared against in
    /// [`is_certificate_revoked`], so a direct `==` is correct without
    /// re-normalizing on every lookup.
    revoked_serials: Vec<Vec<u8>>,
}

impl Crl {
    /// The original DER bytes this [`Crl`] was parsed from.
    pub fn der_bytes(&self) -> &[u8] {
        &self.der_bytes
    }

    /// The issuer name, as rendered by `x509_cert`'s `Name` `Display` impl.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// `thisUpdate`, as seconds since the Unix epoch.
    pub fn this_update(&self) -> u64 {
        self.this_update
    }

    /// `nextUpdate`, as seconds since the Unix epoch, if present. Callers
    /// wanting to reject a stale CRL should compare this against their own
    /// notion of "now" -- this module does not enforce freshness itself
    /// (see module docs).
    pub fn next_update(&self) -> Option<u64> {
        self.next_update
    }

    /// How many entries are in `revokedCertificates`.
    pub fn revoked_count(&self) -> usize {
        self.revoked_serials.len()
    }

    /// Returns `true` if `serial` (a certificate serial number, as the
    /// minimal big-endian bytes produced by `x509_cert::serial_number::
    /// SerialNumber::as_bytes`, e.g. via [`is_certificate_revoked`]) appears
    /// in this CRL's `revokedCertificates` list.
    ///
    /// Per-entry `crlEntryExtensions` (e.g. `CRLReason`) are not consulted
    /// -- this only answers "is the serial number listed at all", matching
    /// this module's stated scope.
    pub fn contains_serial(&self, serial: &[u8]) -> bool {
        self.revoked_serials.iter().any(|s| s.as_slice() == serial)
    }
}

/// Parses a DER-encoded RFC 5280 `CertificateList` (**untrusted input** --
/// this is expected to come directly from a [`CrlTransport`] fetch, or
/// otherwise from a `/DSS /CRLs` entry embedded in a PDF by an untrusted
/// party). Checks the input fits under [`MAX_CRL_LEN`] before any DER
/// parsing touches it, then decodes structurally.
///
/// Does **not** verify `CertificateList.signature` -- see the module docs.
pub fn parse_crl(der: &[u8]) -> SignatureResult<Crl> {
    if der.len() > MAX_CRL_LEN {
        return Err(SignatureError::InvalidFormat(format!(
            "CRL too large ({} bytes, limit {})",
            der.len(),
            MAX_CRL_LEN
        )));
    }

    let cert_list = CertificateList::from_der(der)
        .map_err(|e| SignatureError::InvalidFormat(format!("Malformed CRL: {e}")))?;

    let tbs = &cert_list.tbs_cert_list;
    let revoked_serials = tbs
        .revoked_certificates
        .as_ref()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.serial_number.as_bytes().to_vec())
                .collect()
        })
        .unwrap_or_default();

    Ok(Crl {
        der_bytes: der.to_vec(),
        issuer: tbs.issuer.to_string(),
        this_update: tbs.this_update.to_unix_duration().as_secs(),
        next_update: tbs.next_update.map(|t| t.to_unix_duration().as_secs()),
        revoked_serials,
    })
}

/// Convenience wrapper around [`Crl::contains_serial`] that extracts the
/// serial number from `cert` itself, so callers holding this crate's
/// [`Certificate`] type (whose own [`Certificate::serial_number`] getter
/// returns a hex `String`, not raw bytes) don't need to re-parse or
/// re-format it by hand.
///
/// Returns `Err` only if `cert`'s DER cannot be parsed as an X.509
/// certificate at all (it is otherwise assumed valid, having presumably
/// already round-tripped through [`Certificate::from_der`] or
/// [`Certificate::from_pem`]).
pub fn is_certificate_revoked(crl: &Crl, cert: &Certificate) -> SignatureResult<bool> {
    let x509 = X509Cert::from_der(cert.der_bytes()).map_err(|e| {
        SignatureError::CertificateLoadFailed(format!(
            "Failed to parse certificate for CRL revocation check: {e}"
        ))
    })?;
    let serial = x509.tbs_certificate.serial_number.as_bytes();
    Ok(crl.contains_serial(serial))
}

#[cfg(test)]
mod tests {
    use super::*;
    use der::asn1::{BitString, Ia5String, ObjectIdentifier, OctetString};
    use der::Encode;
    use spki::AlgorithmIdentifierOwned;
    use x509_cert::crl::{RevokedCert, TbsCertList};
    use x509_cert::ext::pkix::crl::dp::DistributionPoint;
    use x509_cert::ext::pkix::name::GeneralName as Gn;
    use x509_cert::ext::{Extension, Extensions};
    use x509_cert::name::Name;
    use x509_cert::serial_number::SerialNumber;
    use x509_cert::spki::SubjectPublicKeyInfoOwned;
    use x509_cert::time::{Time, Validity};
    use x509_cert::{TbsCertificate, Version};

    /// A throwaway `sha256WithRSAEncryption`-shaped `AlgorithmIdentifier`.
    /// Its `oid` value doesn't matter for anything these tests check --
    /// only that it round-trips through DER.
    fn dummy_alg_id() -> AlgorithmIdentifierOwned {
        AlgorithmIdentifierOwned {
            oid: ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11"),
            parameters: None,
        }
    }

    fn build_test_crl(issuer_cn: &str, serials: &[&[u8]]) -> Vec<u8> {
        let issuer: Name = issuer_cn.parse().expect("valid RDN string");
        let alg = dummy_alg_id();

        let revoked_certificates = if serials.is_empty() {
            None
        } else {
            Some(
                serials
                    .iter()
                    .map(|s| RevokedCert {
                        serial_number: SerialNumber::new(s).expect("valid serial"),
                        revocation_date: Time::INFINITY,
                        crl_entry_extensions: None,
                    })
                    .collect(),
            )
        };

        let tbs_cert_list = TbsCertList {
            version: Version::V2,
            signature: alg.clone(),
            issuer,
            this_update: Time::INFINITY,
            next_update: None,
            revoked_certificates,
            crl_extensions: None,
        };

        let cert_list = CertificateList {
            tbs_cert_list,
            signature_algorithm: alg,
            signature: BitString::from_bytes(&[0u8]).expect("valid bit string"),
        };

        cert_list.to_der().expect("CertificateList must encode")
    }

    /// Builds a minimal (self-signed-shaped, not actually validly signed --
    /// these tests never check the signature) certificate DER carrying a
    /// `CRLDistributionPoints` extension with the given distribution point
    /// URLs, for exercising [`extract_crl_distribution_point_url`].
    fn build_test_certificate_with_crl_dps(urls: &[&str]) -> Certificate {
        let name: Name = "CN=Test Leaf".parse().expect("valid RDN string");
        let alg = dummy_alg_id();

        let dps = CrlDistributionPoints(
            urls.iter()
                .map(|url| DistributionPoint {
                    distribution_point: Some(DistributionPointName::FullName(vec![
                        Gn::UniformResourceIdentifier(Ia5String::new(url).expect("valid IA5 URL")),
                    ])),
                    reasons: None,
                    crl_issuer: None,
                })
                .collect(),
        );
        let extn_value = OctetString::new(dps.to_der().expect("CrlDistributionPoints must encode"))
            .expect("valid octet string");
        let extensions: Extensions = vec![Extension {
            extn_id: ObjectIdentifier::new_unwrap(OID_CRL_DISTRIBUTION_POINTS),
            critical: false,
            extn_value,
        }];

        let tbs = TbsCertificate {
            version: Version::V3,
            serial_number: SerialNumber::new(&[0x01]).expect("valid serial"),
            signature: alg.clone(),
            issuer: name.clone(),
            validity: Validity {
                not_before: Time::INFINITY,
                not_after: Time::INFINITY,
            },
            subject: name,
            subject_public_key_info: SubjectPublicKeyInfoOwned {
                algorithm: alg.clone(),
                subject_public_key: BitString::from_bytes(&[0u8]).expect("valid bit string"),
            },
            issuer_unique_id: None,
            subject_unique_id: None,
            extensions: Some(extensions),
        };

        let cert = X509Cert {
            tbs_certificate: tbs,
            signature_algorithm: alg,
            signature: BitString::from_bytes(&[0u8]).expect("valid bit string"),
        };

        let der = cert.to_der().expect("Certificate must encode");
        Certificate::from_der(&der).expect("must parse back")
    }

    #[test]
    fn test_parse_crl_round_trips_revoked_serials() {
        let der = build_test_crl("CN=Test CA", &[&[0x01, 0x02], &[0x03]]);
        let crl = parse_crl(&der).expect("must parse");
        assert_eq!(crl.revoked_count(), 2);
        assert!(crl.contains_serial(&[0x01, 0x02]));
        assert!(crl.contains_serial(&[0x03]));
        assert!(!crl.contains_serial(&[0x04]));
        assert_eq!(crl.issuer(), "CN=Test CA");
        assert_eq!(crl.der_bytes(), der.as_slice());
    }

    #[test]
    fn test_parse_crl_with_no_revoked_certificates() {
        let der = build_test_crl("CN=Test CA", &[]);
        let crl = parse_crl(&der).expect("must parse");
        assert_eq!(crl.revoked_count(), 0);
        assert!(!crl.contains_serial(&[0x01]));
    }

    #[test]
    fn test_parse_crl_rejects_oversized_input() {
        let oversized = vec![0u8; MAX_CRL_LEN + 1];
        let err = parse_crl(&oversized).unwrap_err();
        assert!(matches!(err, SignatureError::InvalidFormat(_)));
    }

    #[test]
    fn test_parse_crl_rejects_garbage() {
        let err = parse_crl(b"not a der CertificateList").unwrap_err();
        assert!(matches!(err, SignatureError::InvalidFormat(_)));
    }

    #[test]
    fn test_extract_crl_distribution_point_url_finds_first_uri() {
        let cert = build_test_certificate_with_crl_dps(&["http://ca.example.com/root.crl"]);
        let url = extract_crl_distribution_point_url(&cert);
        assert_eq!(url.as_deref(), Some("http://ca.example.com/root.crl"));
    }

    #[test]
    fn test_extract_crl_distribution_point_url_multiple_points_returns_first() {
        let cert = build_test_certificate_with_crl_dps(&[
            "http://ca.example.com/a.crl",
            "http://ca.example.com/b.crl",
        ]);
        let url = extract_crl_distribution_point_url(&cert);
        assert_eq!(url.as_deref(), Some("http://ca.example.com/a.crl"));
    }

    #[test]
    fn test_extract_crl_distribution_point_url_missing_extension_is_none() {
        // A certificate built the same way but with no extensions at all.
        let name: Name = "CN=No DPs".parse().expect("valid RDN string");
        let alg = dummy_alg_id();
        let tbs = TbsCertificate {
            version: Version::V3,
            serial_number: SerialNumber::new(&[0x01]).expect("valid serial"),
            signature: alg.clone(),
            issuer: name.clone(),
            validity: Validity {
                not_before: Time::INFINITY,
                not_after: Time::INFINITY,
            },
            subject: name,
            subject_public_key_info: SubjectPublicKeyInfoOwned {
                algorithm: alg.clone(),
                subject_public_key: BitString::from_bytes(&[0u8]).expect("valid bit string"),
            },
            issuer_unique_id: None,
            subject_unique_id: None,
            extensions: None,
        };
        let cert = X509Cert {
            tbs_certificate: tbs,
            signature_algorithm: alg,
            signature: BitString::from_bytes(&[0u8]).expect("valid bit string"),
        };
        let der = cert.to_der().expect("Certificate must encode");
        let cert = Certificate::from_der(&der).expect("must parse back");

        assert!(extract_crl_distribution_point_url(&cert).is_none());
    }

    #[test]
    fn test_is_certificate_revoked() {
        let cert = build_test_certificate_with_crl_dps(&["http://ca.example.com/root.crl"]);
        // The test certificate's serial number is a single byte 0x01 (see
        // `build_test_certificate_with_crl_dps`).
        let revoking_crl = parse_crl(&build_test_crl("CN=Test CA", &[&[0x01]])).unwrap();
        let clean_crl = parse_crl(&build_test_crl("CN=Test CA", &[&[0x02]])).unwrap();

        assert!(is_certificate_revoked(&revoking_crl, &cert).unwrap());
        assert!(!is_certificate_revoked(&clean_crl, &cert).unwrap());
    }
}
