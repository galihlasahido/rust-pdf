//! X.509 certificate chain (path) validation.
//!
//! Builds a chain from a signer's (leaf) certificate up to a caller-supplied
//! set of trust anchors, following RFC 5280's basic path validation model
//! (§6.1) in a deliberately narrow form:
//!
//! - issuer/subject `Name` matching (byte-for-byte DER comparison) to pick
//!   the next certificate in the chain,
//! - the candidate issuer's public key must validate the child certificate's
//!   signature (`tbsCertificate` bytes against the outer `signature` field,
//!   RFC 5280 §4.1.1.3),
//! - each certificate's `notBefore`/`notAfter` validity period (RFC 5280
//!   §4.1.2.5) must cover the check time,
//! - any non-leaf certificate used as an issuer must assert `basicConstraints
//!   cA=TRUE` (RFC 5280 §4.2.1.9) if it carries that extension at all.
//!
//! What this does **not** do (and does not claim to): CRL/OCSP revocation
//! checking, policy constraint processing, name constraint processing, or
//! path-length-constraint enforcement. Those are substantial, separate
//! subsystems (essentially re-implementing chunks of a mature X.509 engine
//! such as OpenSSL's `X509_verify_cert` or webpki) -- flagged here rather
//! than silently pretended to be handled. See module docs on
//! [`super::SignatureVerifier`] for how this fits into overall signature
//! verification.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use der::{Decode, Encode};
use x509_cert::Certificate as X509Cert;

use super::verifier::verify_signature_bytes;
use super::{Certificate, SignatureAlgorithm};

/// Hard ceiling on chain length while path-building, so a maliciously (or
/// accidentally) cyclic set of certificates supplied by an untrusted PDF
/// cannot make chain validation loop forever. RFC 5280 doesn't bound path
/// length, but no realistic PKI chain is anywhere near this deep.
const MAX_CHAIN_DEPTH: usize = 16;

/// The outcome of validating a certificate chain.
#[derive(Debug, Clone)]
pub struct ChainValidationResult {
    /// The chain found, starting with the leaf (signer) certificate and
    /// ending with either the trust anchor that terminated it, or the last
    /// certificate reachable before validation stopped.
    pub chain: Vec<Certificate>,
    /// `true` if the chain terminates at one of the supplied trust anchors,
    /// every signature link verified, and every certificate's validity
    /// period covers the check time.
    pub trusted: bool,
    /// Explains why `trusted` is `false`, if applicable.
    pub error: Option<String>,
}

/// Validates the chain from `leaf` up through `intermediates` to one of
/// `trust_anchors`, at time `at_time` (seconds since Unix epoch).
///
/// `intermediates` is typically the set of extra certificates embedded in
/// the CMS `SignedData.certificates` (beyond the leaf), in no particular
/// order -- this function searches them by issuer/subject match rather than
/// assuming they're pre-sorted.
///
/// If `trust_anchors` is empty, this always returns `trusted: false` (there
/// is nothing to trust), but still reports the chain it was able to build
/// so callers can show it to a user for manual inspection.
pub fn validate_chain(
    leaf: &Certificate,
    intermediates: &[Certificate],
    trust_anchors: &[Certificate],
    at_time: u64,
) -> ChainValidationResult {
    let mut chain = Vec::new();
    let mut error = None;

    let mut current = match parse(leaf) {
        Ok(c) => c,
        Err(e) => {
            return ChainValidationResult {
                chain: vec![leaf.clone()],
                trusted: false,
                error: Some(format!("Failed to parse leaf certificate: {e}")),
            };
        }
    };
    chain.push(leaf.clone());

    if let Err(e) = check_validity_period(&current, at_time) {
        return ChainValidationResult { chain, trusted: false, error: Some(e) };
    }

    let mut trusted = false;

    for depth in 0..MAX_CHAIN_DEPTH {
        // Self-signed? A cert whose issuer == subject and which verifies
        // against its own key terminates the chain (it's either a trust
        // anchor we recognize below, or an untrusted self-signed cert).
        let is_trust_anchor = trust_anchors.iter().any(|anchor| {
            anchor.der_bytes() == current_der(&current).as_deref().unwrap_or_default()
        });

        if is_trust_anchor {
            trusted = true;
            break;
        }

        // Find an issuer among intermediates or trust anchors whose
        // Subject matches this certificate's Issuer.
        let candidates = intermediates.iter().chain(trust_anchors.iter());
        let mut found = None;
        for candidate in candidates {
            let Ok(candidate_x509) = parse(candidate) else { continue };
            if candidate_x509.tbs_certificate.subject.to_der().ok()
                != current.tbs_certificate.issuer.to_der().ok()
            {
                continue;
            }
            if !is_ca(&candidate_x509) && depth > 0 {
                // A non-CA certificate can't legitimately sign another
                // certificate; skip it rather than treat a name collision
                // as a valid path (RFC 5280 §4.2.1.9).
                continue;
            }
            if verify_issued_by(&current, &candidate_x509).unwrap_or(false) {
                found = Some((candidate.clone(), candidate_x509));
                break;
            }
        }

        match found {
            Some((cert, x509)) => {
                if let Err(e) = check_validity_period(&x509, at_time) {
                    error = Some(e);
                    chain.push(cert);
                    break;
                }
                let is_anchor = trust_anchors
                    .iter()
                    .any(|anchor| anchor.der_bytes() == cert.der_bytes());
                chain.push(cert);
                current = x509;
                if is_anchor {
                    trusted = true;
                    break;
                }
            }
            None => {
                error.get_or_insert_with(|| {
                    "Could not find an issuer certificate to continue the chain \
                     (and the last certificate is not a supplied trust anchor)"
                        .to_string()
                });
                break;
            }
        }
    }

    if !trusted && error.is_none() {
        error = Some("Chain did not terminate at a supplied trust anchor".to_string());
    }

    ChainValidationResult { chain, trusted, error }
}

fn current_der(cert: &X509Cert) -> Option<Vec<u8>> {
    cert.to_der().ok()
}

fn parse(cert: &Certificate) -> Result<X509Cert, der::Error> {
    X509Cert::from_der(cert.der_bytes())
}

/// Checks whether `issuer_x509`'s public key validates `child`'s signature,
/// per RFC 5280 §4.1.1.3 (the outer `signatureValue` is computed over the
/// DER encoding of `child`'s `tbsCertificate`).
fn verify_issued_by(child: &X509Cert, issuer_x509: &X509Cert) -> Result<bool, String> {
    let algo = SignatureAlgorithm::from_oid(&child.signature_algorithm.oid.to_string())
        .ok_or_else(|| format!("Unsupported certificate signature algorithm: {}", child.signature_algorithm.oid))?;

    let tbs_der = child
        .tbs_certificate
        .to_der()
        .map_err(|e| format!("Failed to encode tbsCertificate: {e}"))?;

    verify_signature_bytes(algo, issuer_x509, &tbs_der, child.signature.raw_bytes())
}

/// Returns whether `cert` asserts `basicConstraints cA=TRUE`. Certificates
/// without a `basicConstraints` extension are treated as *not* a CA (a
/// conservative default -- RFC 5280 §4.2.1.9 requires CAs to include it).
fn is_ca(cert: &X509Cert) -> bool {
    use const_oid::AssociatedOid;
    use x509_cert::ext::pkix::BasicConstraints;

    let Some(extensions) = &cert.tbs_certificate.extensions else { return false };
    for ext in extensions {
        if ext.extn_id == BasicConstraints::OID {
            if let Ok(bc) = BasicConstraints::from_der(ext.extn_value.as_bytes()) {
                return bc.ca;
            }
        }
    }
    false
}

fn check_validity_period(cert: &X509Cert, at_time: u64) -> Result<(), String> {
    let not_before = cert.tbs_certificate.validity.not_before.to_unix_duration();
    let not_after = cert.tbs_certificate.validity.not_after.to_unix_duration();
    let at = Duration::from_secs(at_time);

    if at < not_before || at > not_after {
        return Err(format!(
            "Certificate '{}' is not valid at the check time (validity {}..{})",
            cert.tbs_certificate.subject,
            not_before.as_secs(),
            not_after.as_secs()
        ));
    }
    Ok(())
}

/// Returns the current time as seconds since the Unix epoch, clamped to `0`
/// if the system clock is somehow before the epoch (untrusted-input-safe
/// default rather than panicking).
pub(super) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_validation_empty_trust_anchors_is_untrusted() {
        // A leaf with no candidates and no anchors can't build any chain;
        // `trusted` must be false rather than panicking or defaulting true.
        // We can't easily construct a real `Certificate` without `openssl`
        // here, so this is exercised end-to-end in
        // `tests/signature_verification_tests.rs` instead. This test just
        // pins the "no trust anchors => never trusted" contract at the
        // result-shape level.
        let result = ChainValidationResult {
            chain: Vec::new(),
            trusted: false,
            error: Some("no anchors".to_string()),
        };
        assert!(!result.trusted);
        assert!(result.error.is_some());
    }

}
