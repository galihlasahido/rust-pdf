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
mod config;
mod pkcs7;
mod signer;

pub use certificate::{Certificate, PrivateKey};
pub use config::SignatureConfig;
pub use pkcs7::Pkcs7Builder;
pub use signer::{ByteRange, DocumentSigner, SignatureInfo};

use crate::error::SignatureError;

/// Result type for signature operations.
pub type SignatureResult<T> = Result<T, SignatureError>;

/// Supported signature algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    /// RSA with SHA-256.
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
}

impl Default for SignatureAlgorithm {
    fn default() -> Self {
        SignatureAlgorithm::RsaSha256
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
}
