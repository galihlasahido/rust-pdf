//! PDF Digital Signatures support.
//!
//! This module provides functionality for signing PDF documents using X.509 certificates
//! and PKCS#7 detached signatures, compliant with PDF 1.7/2.0 specifications.
//!
//! # Features
//!
//! - Sign PDF documents with RSA or ECDSA keys
//! - Support for X.509 certificates in PEM format
//! - PKCS#7 (CMS) signature containers
//! - Signature validation and verification
//!
//! # Example
//!
//! ```ignore
//! use rust_pdf::prelude::*;
//! use rust_pdf::signatures::{SignatureConfig, Certificate, PrivateKey, DocumentSigner};
//!
//! // Load certificate and private key
//! let cert = Certificate::from_pem_file("cert.pem")?;
//! let key = PrivateKey::from_pem_file("key.pem")?;
//!
//! // Create and sign a document
//! let doc = DocumentBuilder::new()
//!     .page(page)
//!     .build()?;
//!
//! let signed = DocumentSigner::new(doc)
//!     .certificate(cert)
//!     .private_key(key)
//!     .reason("Document approval")
//!     .location("San Francisco")
//!     .sign()?;
//! ```

mod certificate;
mod chain;
mod config;
mod dss;
mod pkcs7;
mod signer;
mod timestamp;
mod verifier;

pub use certificate::{Certificate, PrivateKey};
pub use chain::{validate_chain, ChainValidationResult};
pub use config::{PadesLevel, SignatureConfig, VisibleSignature};
pub use dss::{embed_document_security_store, DssEntry};
pub use pkcs7::Pkcs7Builder;
pub use signer::{ByteRange, DocumentSigner, IncrementalSigner, SignatureInfo};
pub use timestamp::{
    build_timestamp_request, parse_timestamp_response, TimestampAuthorityClient,
    TimestampRequest, TimestampToken,
};
pub use verifier::{SignatureVerifier, VerifiedSignature};

use crate::error::SignatureError;

/// Result type for signature operations.
pub type SignatureResult<T> = Result<T, SignatureError>;

/// Supported signature algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignatureAlgorithm {
    /// RSA with SHA-256.
    #[default]
    RsaSha256,
    /// RSA with SHA-384.
    RsaSha384,
    /// RSA with SHA-512.
    RsaSha512,
    /// ECDSA with P-256 curve and SHA-256.
    EcdsaP256Sha256,
}

impl SignatureAlgorithm {
    /// Returns the OID for this algorithm.
    pub fn oid(&self) -> &'static str {
        match self {
            SignatureAlgorithm::RsaSha256 => "1.2.840.113549.1.1.11",
            SignatureAlgorithm::RsaSha384 => "1.2.840.113549.1.1.12",
            SignatureAlgorithm::RsaSha512 => "1.2.840.113549.1.1.13",
            SignatureAlgorithm::EcdsaP256Sha256 => "1.2.840.10045.4.3.2",
        }
    }

    /// Returns the digest algorithm OID.
    pub fn digest_oid(&self) -> &'static str {
        match self {
            SignatureAlgorithm::RsaSha256 | SignatureAlgorithm::EcdsaP256Sha256 => {
                "2.16.840.1.101.3.4.2.1" // SHA-256
            }
            SignatureAlgorithm::RsaSha384 => "2.16.840.1.101.3.4.2.2", // SHA-384
            SignatureAlgorithm::RsaSha512 => "2.16.840.1.101.3.4.2.3", // SHA-512
        }
    }

    /// Looks up a signature algorithm from its signature algorithm OID.
    ///
    /// This is the reverse of [`SignatureAlgorithm::oid`], used when parsing
    /// a CMS `SignerInfo` to determine which algorithm was used to sign.
    pub fn from_oid(oid: &str) -> Option<Self> {
        match oid {
            "1.2.840.113549.1.1.11" => Some(SignatureAlgorithm::RsaSha256),
            "1.2.840.113549.1.1.12" => Some(SignatureAlgorithm::RsaSha384),
            "1.2.840.113549.1.1.13" => Some(SignatureAlgorithm::RsaSha512),
            "1.2.840.10045.4.3.2" => Some(SignatureAlgorithm::EcdsaP256Sha256),
            _ => None,
        }
    }

    /// Resolves a `SignerInfo`'s algorithm from its `signatureAlgorithm` and
    /// `digestAlgorithm` OIDs together.
    ///
    /// [`SignatureAlgorithm::oid`]/[`Pkcs7Builder`] always emit the combined
    /// "hash-with-signature" OID (e.g. `sha256WithRSAEncryption`,
    /// RFC 8017 §8.2) as `signatureAlgorithm`, which [`SignatureAlgorithm::from_oid`]
    /// alone is enough to resolve. But RFC 5652 §5.3 also permits CMS
    /// producers to put the *bare* key-type OID (e.g. `rsaEncryption`,
    /// `1.2.840.113549.1.1.1`) in `signatureAlgorithm` and rely on the
    /// separate `digestAlgorithm` field to convey the hash -- this is what
    /// e.g. OpenSSL's `ts` (RFC 3161 timestamp authority) implementation
    /// does. Verifying only real-world-produced CMS (not just our own
    /// output) needs to accept both conventions.
    pub(crate) fn from_oids(signature_algorithm_oid: &str, digest_algorithm_oid: &str) -> Option<Self> {
        if let Some(algo) = Self::from_oid(signature_algorithm_oid) {
            return Some(algo);
        }

        // Bare `rsaEncryption`: resolve via the separate digest OID.
        if signature_algorithm_oid == "1.2.840.113549.1.1.1" {
            return match digest_algorithm_oid {
                "2.16.840.1.101.3.4.2.1" => Some(SignatureAlgorithm::RsaSha256), // SHA-256
                "2.16.840.1.101.3.4.2.2" => Some(SignatureAlgorithm::RsaSha384), // SHA-384
                "2.16.840.1.101.3.4.2.3" => Some(SignatureAlgorithm::RsaSha512), // SHA-512
                _ => None,
            };
        }

        None
    }
}

/// Computes the message digest for `data` using the hash algorithm
/// associated with `algo`.
///
/// Shared by [`Pkcs7Builder`] (when signing) and [`SignatureVerifier`]
/// (when verifying) so both sides hash the same way.
pub(crate) fn digest_for_algorithm(algo: SignatureAlgorithm, data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256, Sha384, Sha512};

    match algo {
        SignatureAlgorithm::RsaSha256 | SignatureAlgorithm::EcdsaP256Sha256 => {
            let mut hasher = Sha256::new();
            hasher.update(data);
            hasher.finalize().to_vec()
        }
        SignatureAlgorithm::RsaSha384 => {
            let mut hasher = Sha384::new();
            hasher.update(data);
            hasher.finalize().to_vec()
        }
        SignatureAlgorithm::RsaSha512 => {
            let mut hasher = Sha512::new();
            hasher.update(data);
            hasher.finalize().to_vec()
        }
    }
}


/// PDF signature dictionary field names.
pub mod fields {
    /// Signature type.
    pub const TYPE: &str = "Sig";
    /// Filter name (Adobe.PPKLite).
    pub const FILTER: &str = "Adobe.PPKLite";
    /// Sub-filter for PKCS#7 detached.
    pub const SUB_FILTER_PKCS7_DETACHED: &str = "adbe.pkcs7.detached";
    /// Sub-filter for ETSI CAdES.
    pub const SUB_FILTER_ETSI_CADES: &str = "ETSI.CAdES.detached";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_algorithm_oid() {
        assert_eq!(
            SignatureAlgorithm::RsaSha256.oid(),
            "1.2.840.113549.1.1.11"
        );
        assert_eq!(
            SignatureAlgorithm::EcdsaP256Sha256.oid(),
            "1.2.840.10045.4.3.2"
        );
    }

    #[test]
    fn test_signature_algorithm_default() {
        assert_eq!(SignatureAlgorithm::default(), SignatureAlgorithm::RsaSha256);
    }

    #[test]
    fn test_signature_algorithm_from_oid_roundtrip() {
        for algo in [
            SignatureAlgorithm::RsaSha256,
            SignatureAlgorithm::RsaSha384,
            SignatureAlgorithm::RsaSha512,
            SignatureAlgorithm::EcdsaP256Sha256,
        ] {
            assert_eq!(SignatureAlgorithm::from_oid(algo.oid()), Some(algo));
        }
        assert_eq!(SignatureAlgorithm::from_oid("0.0.0"), None);
    }

    #[test]
    fn test_digest_for_algorithm_matches_sha256() {
        use sha2::{Digest, Sha256};

        let data = b"hello world";
        let expected = {
            let mut hasher = Sha256::new();
            hasher.update(data);
            hasher.finalize().to_vec()
        };
        assert_eq!(digest_for_algorithm(SignatureAlgorithm::RsaSha256, data), expected);
    }
}
