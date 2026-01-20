//! PKCS#7 (CMS) signature container building.

use crate::error::SignatureError;
use super::{Certificate, PrivateKey, SignatureAlgorithm, SignatureResult};
use sha2::{Sha256, Sha384, Sha512, Digest};

/// Builder for creating PKCS#7 (CMS) SignedData structures.
#[derive(Debug)]
pub struct Pkcs7Builder {
    /// The signer's certificate.
    certificate: Option<Certificate>,
    /// Additional certificates in the chain.
    certificate_chain: Vec<Certificate>,
    /// The signature algorithm.
    algorithm: SignatureAlgorithm,
}

impl Pkcs7Builder {
    /// Creates a new PKCS#7 builder.
    pub fn new() -> Self {
        Self {
            certificate: None,
            certificate_chain: Vec::new(),
            algorithm: SignatureAlgorithm::default(),
        }
    }

    /// Sets the signer's certificate.
    pub fn certificate(mut self, cert: Certificate) -> Self {
        self.certificate = Some(cert);
        self
    }

    /// Adds a certificate to the chain.
    pub fn add_chain_certificate(mut self, cert: Certificate) -> Self {
        self.certificate_chain.push(cert);
        self
    }

    /// Sets the signature algorithm.
    pub fn algorithm(mut self, algo: SignatureAlgorithm) -> Self {
        self.algorithm = algo;
        self
    }

    /// Builds a PKCS#7 SignedData structure.
    ///
    /// This creates a CMS SignedData structure with:
    /// - Version 1
    /// - Digest algorithm (SHA-256/384/512)
    /// - Encapsulated content (none for detached signature)
    /// - Certificates
    /// - Signer infos
    pub fn build(
        &self,
        data_to_sign: &[u8],
        private_key: &PrivateKey,
    ) -> SignatureResult<Vec<u8>> {
        let cert = self.certificate.as_ref().ok_or_else(|| {
            SignatureError::SigningFailed("Certificate not set".to_string())
        })?;

        // Compute message digest
        let digest = self.compute_digest(data_to_sign);

        // Create the signed attributes and sign them
        let signed_attrs = self.build_signed_attributes(&digest)?;
        let signature = private_key.sign(&signed_attrs)?;

        // Build the CMS SignedData structure
        let cms_data = self.build_cms_signed_data(cert, &digest, &signature)?;

        Ok(cms_data)
    }

    /// Computes the message digest.
    fn compute_digest(&self, data: &[u8]) -> Vec<u8> {
        match self.algorithm {
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

    /// Builds the signed attributes to be signed.
    fn build_signed_attributes(&self, digest: &[u8]) -> SignatureResult<Vec<u8>> {
        // Build a DER-encoded SET of attributes:
        // - Content type (OID for data)
        // - Signing time
        // - Message digest
        let mut attrs = Vec::new();

        // Content type attribute (OID 1.2.840.113549.1.9.3)
        // Value: OID for data (1.2.840.113549.1.7.1)
        let content_type_attr = build_attribute(
            &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x03], // 1.2.840.113549.1.9.3
            &build_oid(&[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x01]), // data OID
        );
        attrs.extend_from_slice(&content_type_attr);

        // Message digest attribute (OID 1.2.840.113549.1.9.4)
        let digest_attr = build_attribute(
            &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x04], // 1.2.840.113549.1.9.4
            &build_octet_string(digest),
        );
        attrs.extend_from_slice(&digest_attr);

        // Wrap as SET
        let set = build_set(&attrs);

        Ok(set)
    }

    /// Builds the CMS SignedData structure.
    fn build_cms_signed_data(
        &self,
        cert: &Certificate,
        digest: &[u8],
        signature: &[u8],
    ) -> SignatureResult<Vec<u8>> {
        // SignedData structure:
        // SEQUENCE {
        //   version INTEGER
        //   digestAlgorithms SET OF AlgorithmIdentifier
        //   encapContentInfo EncapsulatedContentInfo
        //   certificates [0] IMPLICIT CertificateSet OPTIONAL
        //   signerInfos SET OF SignerInfo
        // }

        let mut signed_data = Vec::new();

        // Version (1 for basic)
        signed_data.extend_from_slice(&build_integer(1));

        // DigestAlgorithms SET
        let digest_alg = self.build_digest_algorithm_identifier();
        let digest_algs_set = build_set(&digest_alg);
        signed_data.extend_from_slice(&digest_algs_set);

        // EncapContentInfo (detached, so no content)
        let content_info = self.build_encap_content_info();
        signed_data.extend_from_slice(&content_info);

        // Certificates [0] IMPLICIT
        let certs = self.build_certificates(cert);
        let certs_implicit = build_context_specific(0, &certs, true);
        signed_data.extend_from_slice(&certs_implicit);

        // SignerInfos SET
        let signer_info = self.build_signer_info(cert, digest, signature)?;
        let signer_infos_set = build_set(&signer_info);
        signed_data.extend_from_slice(&signer_infos_set);

        // Wrap SignedData in SEQUENCE
        let signed_data_seq = build_sequence(&signed_data);

        // ContentInfo wrapper
        // SEQUENCE {
        //   contentType OBJECT IDENTIFIER (signedData)
        //   content [0] EXPLICIT SignedData
        // }
        let mut content_info_wrapper = Vec::new();
        // OID for signedData: 1.2.840.113549.1.7.2
        content_info_wrapper.extend_from_slice(&build_oid(&[
            0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02,
        ]));
        content_info_wrapper.extend_from_slice(&build_context_specific(0, &signed_data_seq, false));

        Ok(build_sequence(&content_info_wrapper))
    }

    /// Builds the digest algorithm identifier.
    fn build_digest_algorithm_identifier(&self) -> Vec<u8> {
        let oid_bytes = match self.algorithm {
            SignatureAlgorithm::RsaSha256 | SignatureAlgorithm::EcdsaP256Sha256 => {
                // SHA-256: 2.16.840.1.101.3.4.2.1
                vec![0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01]
            }
            SignatureAlgorithm::RsaSha384 => {
                // SHA-384: 2.16.840.1.101.3.4.2.2
                vec![0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02]
            }
            SignatureAlgorithm::RsaSha512 => {
                // SHA-512: 2.16.840.1.101.3.4.2.3
                vec![0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03]
            }
        };

        let mut alg_id = Vec::new();
        alg_id.extend_from_slice(&build_oid(&oid_bytes));
        alg_id.extend_from_slice(&[0x05, 0x00]); // NULL parameters

        build_sequence(&alg_id)
    }

    /// Builds the encapsulated content info (for detached signature).
    fn build_encap_content_info(&self) -> Vec<u8> {
        // For detached signature, just the content type OID
        // data: 1.2.840.113549.1.7.1
        build_sequence(&build_oid(&[
            0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x01,
        ]))
    }

    /// Builds the certificates set.
    fn build_certificates(&self, cert: &Certificate) -> Vec<u8> {
        let mut certs = Vec::new();
        certs.extend_from_slice(cert.der_bytes());

        for chain_cert in &self.certificate_chain {
            certs.extend_from_slice(chain_cert.der_bytes());
        }

        certs
    }

    /// Builds the signer info structure.
    fn build_signer_info(
        &self,
        cert: &Certificate,
        digest: &[u8],
        signature: &[u8],
    ) -> SignatureResult<Vec<u8>> {
        let mut signer_info = Vec::new();

        // Version (1)
        signer_info.extend_from_slice(&build_integer(1));

        // IssuerAndSerialNumber
        let issuer_serial = self.build_issuer_and_serial(cert)?;
        signer_info.extend_from_slice(&issuer_serial);

        // DigestAlgorithm
        signer_info.extend_from_slice(&self.build_digest_algorithm_identifier());

        // SignedAttrs [0] IMPLICIT
        let signed_attrs = self.build_signed_attributes(digest)?;
        let signed_attrs_implicit = build_context_specific(0, &signed_attrs[1..], true); // Skip SET tag
        signer_info.extend_from_slice(&signed_attrs_implicit);

        // SignatureAlgorithm
        signer_info.extend_from_slice(&self.build_signature_algorithm_identifier());

        // Signature value
        signer_info.extend_from_slice(&build_octet_string(signature));

        Ok(build_sequence(&signer_info))
    }

    /// Builds the signature algorithm identifier.
    fn build_signature_algorithm_identifier(&self) -> Vec<u8> {
        let oid_bytes = match self.algorithm {
            SignatureAlgorithm::RsaSha256 => {
                // sha256WithRSAEncryption: 1.2.840.113549.1.1.11
                vec![0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0B]
            }
            SignatureAlgorithm::RsaSha384 => {
                // sha384WithRSAEncryption: 1.2.840.113549.1.1.12
                vec![0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0C]
            }
            SignatureAlgorithm::RsaSha512 => {
                // sha512WithRSAEncryption: 1.2.840.113549.1.1.13
                vec![0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0D]
            }
            SignatureAlgorithm::EcdsaP256Sha256 => {
                // ecdsa-with-SHA256: 1.2.840.10045.4.3.2
                vec![0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02]
            }
        };

        let mut alg_id = Vec::new();
        alg_id.extend_from_slice(&build_oid(&oid_bytes));
        alg_id.extend_from_slice(&[0x05, 0x00]); // NULL parameters

        build_sequence(&alg_id)
    }

    /// Builds the issuer and serial number from the certificate.
    fn build_issuer_and_serial(&self, cert: &Certificate) -> SignatureResult<Vec<u8>> {
        use x509_cert::Certificate as X509Cert;
        use der::{Decode, Encode};

        let x509 = X509Cert::from_der(cert.der_bytes()).map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to parse certificate: {}", e))
        })?;

        // Get the issuer name DER encoding
        let issuer_der = x509.tbs_certificate.issuer.to_der().map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to encode issuer: {}", e))
        })?;

        // Get the serial number DER encoding
        let serial_der = x509.tbs_certificate.serial_number.to_der().map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to encode serial: {}", e))
        })?;

        let mut issuer_serial = Vec::new();
        issuer_serial.extend_from_slice(&issuer_der);
        issuer_serial.extend_from_slice(&serial_der);

        Ok(build_sequence(&issuer_serial))
    }
}

impl Default for Pkcs7Builder {
    fn default() -> Self {
        Self::new()
    }
}

// Helper functions for DER encoding

/// Builds a DER-encoded INTEGER.
fn build_integer(value: i64) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.push(0x02); // INTEGER tag

    if value == 0 {
        bytes.push(0x01);
        bytes.push(0x00);
    } else {
        let mut val = value;
        let mut int_bytes = Vec::new();

        while val != 0 && val != -1 {
            int_bytes.push((val & 0xFF) as u8);
            val >>= 8;
        }

        // Add sign byte if needed
        if value > 0 && (int_bytes.last().map(|&b| b & 0x80 != 0).unwrap_or(false)) {
            int_bytes.push(0x00);
        }

        int_bytes.reverse();
        bytes.push(int_bytes.len() as u8);
        bytes.extend_from_slice(&int_bytes);
    }

    bytes
}

/// Builds a DER-encoded SEQUENCE.
fn build_sequence(content: &[u8]) -> Vec<u8> {
    let mut seq = Vec::new();
    seq.push(0x30); // SEQUENCE tag
    seq.extend_from_slice(&encode_length(content.len()));
    seq.extend_from_slice(content);
    seq
}

/// Builds a DER-encoded SET.
fn build_set(content: &[u8]) -> Vec<u8> {
    let mut set = Vec::new();
    set.push(0x31); // SET tag
    set.extend_from_slice(&encode_length(content.len()));
    set.extend_from_slice(content);
    set
}

/// Builds a DER-encoded OCTET STRING.
fn build_octet_string(content: &[u8]) -> Vec<u8> {
    let mut os = Vec::new();
    os.push(0x04); // OCTET STRING tag
    os.extend_from_slice(&encode_length(content.len()));
    os.extend_from_slice(content);
    os
}

/// Builds a DER-encoded OID.
fn build_oid(oid_bytes: &[u8]) -> Vec<u8> {
    let mut oid = Vec::new();
    oid.push(0x06); // OID tag
    oid.push(oid_bytes.len() as u8);
    oid.extend_from_slice(oid_bytes);
    oid
}

/// Builds a DER-encoded context-specific tag.
fn build_context_specific(tag_num: u8, content: &[u8], implicit: bool) -> Vec<u8> {
    let mut cs = Vec::new();
    let tag = if implicit {
        0xA0 | tag_num // IMPLICIT context-specific
    } else {
        0xA0 | tag_num // EXPLICIT context-specific
    };
    cs.push(tag);
    cs.extend_from_slice(&encode_length(content.len()));
    cs.extend_from_slice(content);
    cs
}

/// Builds a DER-encoded attribute.
fn build_attribute(oid_bytes: &[u8], value: &[u8]) -> Vec<u8> {
    let mut attr = Vec::new();
    attr.extend_from_slice(&build_oid(oid_bytes));
    attr.extend_from_slice(&build_set(value));
    build_sequence(&attr)
}

/// Encodes a length in DER format.
fn encode_length(len: usize) -> Vec<u8> {
    if len < 128 {
        vec![len as u8]
    } else if len < 256 {
        vec![0x81, len as u8]
    } else if len < 65536 {
        vec![0x82, (len >> 8) as u8, (len & 0xFF) as u8]
    } else if len < 16777216 {
        vec![
            0x83,
            (len >> 16) as u8,
            ((len >> 8) & 0xFF) as u8,
            (len & 0xFF) as u8,
        ]
    } else {
        vec![
            0x84,
            (len >> 24) as u8,
            ((len >> 16) & 0xFF) as u8,
            ((len >> 8) & 0xFF) as u8,
            (len & 0xFF) as u8,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_integer() {
        let int = build_integer(1);
        assert_eq!(int, vec![0x02, 0x01, 0x01]);

        let int = build_integer(0);
        assert_eq!(int, vec![0x02, 0x01, 0x00]);

        let int = build_integer(127);
        assert_eq!(int, vec![0x02, 0x01, 0x7F]);

        let int = build_integer(128);
        assert_eq!(int, vec![0x02, 0x02, 0x00, 0x80]); // Needs leading 0
    }

    #[test]
    fn test_build_sequence() {
        let content = vec![0x02, 0x01, 0x01]; // INTEGER 1
        let seq = build_sequence(&content);
        assert_eq!(seq, vec![0x30, 0x03, 0x02, 0x01, 0x01]);
    }

    #[test]
    fn test_encode_length() {
        assert_eq!(encode_length(0), vec![0x00]);
        assert_eq!(encode_length(127), vec![0x7F]);
        assert_eq!(encode_length(128), vec![0x81, 0x80]);
        assert_eq!(encode_length(256), vec![0x82, 0x01, 0x00]);
    }

    #[test]
    fn test_build_octet_string() {
        let os = build_octet_string(&[0x01, 0x02, 0x03]);
        assert_eq!(os, vec![0x04, 0x03, 0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_pkcs7_builder_new() {
        let builder = Pkcs7Builder::new();
        assert!(builder.certificate.is_none());
        assert!(builder.certificate_chain.is_empty());
    }
}
