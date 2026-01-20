//! X.509 certificate and private key handling.

use crate::error::SignatureError;
use super::SignatureResult;
use std::fs;
use std::path::Path;

/// An X.509 certificate for PDF signing.
#[derive(Debug, Clone)]
pub struct Certificate {
    /// The raw DER-encoded certificate bytes.
    der_bytes: Vec<u8>,
    /// The certificate subject name (common name).
    subject_name: String,
    /// The certificate issuer name.
    issuer_name: String,
    /// Serial number as hex string.
    serial_number: String,
}

impl Certificate {
    /// Loads a certificate from a PEM file.
    pub fn from_pem_file(path: impl AsRef<Path>) -> SignatureResult<Self> {
        let pem_data = fs::read_to_string(path.as_ref()).map_err(|e| {
            SignatureError::CertificateLoadFailed(format!("Failed to read file: {}", e))
        })?;

        Self::from_pem(&pem_data)
    }

    /// Loads a certificate from PEM data.
    pub fn from_pem(pem_data: &str) -> SignatureResult<Self> {
        use x509_cert::Certificate as X509Cert;
        use der::Decode;

        // Parse PEM to get DER bytes
        let der_bytes = pem_to_der(pem_data, "CERTIFICATE")?;

        // Parse the certificate
        let cert = X509Cert::from_der(&der_bytes).map_err(|e| {
            SignatureError::CertificateLoadFailed(format!("Failed to parse certificate: {}", e))
        })?;

        // Extract subject name
        let subject_name = extract_common_name(&cert.tbs_certificate.subject)
            .unwrap_or_else(|| "Unknown".to_string());

        // Extract issuer name
        let issuer_name = extract_common_name(&cert.tbs_certificate.issuer)
            .unwrap_or_else(|| "Unknown".to_string());

        // Extract serial number
        let serial_bytes = cert.tbs_certificate.serial_number.as_bytes();
        let serial_number = serial_bytes
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<String>();

        Ok(Self {
            der_bytes,
            subject_name,
            issuer_name,
            serial_number,
        })
    }

    /// Loads a certificate from DER bytes.
    pub fn from_der(der_bytes: &[u8]) -> SignatureResult<Self> {
        use x509_cert::Certificate as X509Cert;
        use der::Decode;

        let cert = X509Cert::from_der(der_bytes).map_err(|e| {
            SignatureError::CertificateLoadFailed(format!("Failed to parse certificate: {}", e))
        })?;

        let subject_name = extract_common_name(&cert.tbs_certificate.subject)
            .unwrap_or_else(|| "Unknown".to_string());
        let issuer_name = extract_common_name(&cert.tbs_certificate.issuer)
            .unwrap_or_else(|| "Unknown".to_string());
        let serial_bytes = cert.tbs_certificate.serial_number.as_bytes();
        let serial_number = serial_bytes
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<String>();

        Ok(Self {
            der_bytes: der_bytes.to_vec(),
            subject_name,
            issuer_name,
            serial_number,
        })
    }

    /// Returns the subject name (common name).
    pub fn subject_name(&self) -> &str {
        &self.subject_name
    }

    /// Returns the issuer name.
    pub fn issuer_name(&self) -> &str {
        &self.issuer_name
    }

    /// Returns the serial number as hex string.
    pub fn serial_number(&self) -> &str {
        &self.serial_number
    }

    /// Returns the raw DER-encoded bytes.
    pub fn der_bytes(&self) -> &[u8] {
        &self.der_bytes
    }
}

/// A private key for PDF signing.
#[derive(Clone)]
pub struct PrivateKey {
    /// The key type.
    key_type: KeyType,
    /// Raw key bytes (DER encoded).
    der_bytes: Vec<u8>,
}

/// The type of private key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    /// RSA private key.
    Rsa,
    /// ECDSA P-256 private key.
    EcdsaP256,
}

impl PrivateKey {
    /// Loads a private key from a PEM file.
    pub fn from_pem_file(path: impl AsRef<Path>) -> SignatureResult<Self> {
        let pem_data = fs::read_to_string(path.as_ref()).map_err(|e| {
            SignatureError::PrivateKeyLoadFailed(format!("Failed to read file: {}", e))
        })?;

        Self::from_pem(&pem_data)
    }

    /// Loads a private key from PEM data.
    pub fn from_pem(pem_data: &str) -> SignatureResult<Self> {
        // Try PKCS#8 format first
        if pem_data.contains("BEGIN PRIVATE KEY") {
            let der_bytes = pem_to_der(pem_data, "PRIVATE KEY")?;
            return Self::from_pkcs8_der(&der_bytes);
        }

        // Try RSA private key format
        if pem_data.contains("BEGIN RSA PRIVATE KEY") {
            let der_bytes = pem_to_der(pem_data, "RSA PRIVATE KEY")?;
            return Ok(Self {
                key_type: KeyType::Rsa,
                der_bytes,
            });
        }

        // Try EC private key format
        if pem_data.contains("BEGIN EC PRIVATE KEY") {
            let der_bytes = pem_to_der(pem_data, "EC PRIVATE KEY")?;
            return Ok(Self {
                key_type: KeyType::EcdsaP256,
                der_bytes,
            });
        }

        Err(SignatureError::PrivateKeyLoadFailed(
            "Unsupported private key format".to_string(),
        ))
    }

    /// Loads a private key from PKCS#8 DER bytes.
    fn from_pkcs8_der(der_bytes: &[u8]) -> SignatureResult<Self> {
        use pkcs8::PrivateKeyInfo;
        use der::Decode;

        let key_info = PrivateKeyInfo::from_der(der_bytes).map_err(|e| {
            SignatureError::PrivateKeyLoadFailed(format!("Failed to parse PKCS#8 key: {}", e))
        })?;

        // Check the algorithm OID to determine key type
        let oid = key_info.algorithm.oid;

        // RSA OID: 1.2.840.113549.1.1.1
        let rsa_oid = const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
        // EC OID: 1.2.840.10045.2.1
        let ec_oid = const_oid::ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");

        let key_type = if oid == rsa_oid {
            KeyType::Rsa
        } else if oid == ec_oid {
            KeyType::EcdsaP256
        } else {
            return Err(SignatureError::PrivateKeyLoadFailed(format!(
                "Unsupported key algorithm OID: {}",
                oid
            )));
        };

        Ok(Self {
            key_type,
            der_bytes: der_bytes.to_vec(),
        })
    }

    /// Returns the key type.
    pub fn key_type(&self) -> KeyType {
        self.key_type
    }

    /// Returns the raw DER-encoded bytes.
    pub fn der_bytes(&self) -> &[u8] {
        &self.der_bytes
    }

    /// Signs data using this private key.
    pub fn sign(&self, data: &[u8]) -> SignatureResult<Vec<u8>> {
        match self.key_type {
            KeyType::Rsa => self.sign_rsa(data),
            KeyType::EcdsaP256 => self.sign_ecdsa(data),
        }
    }

    /// Signs data with RSA-SHA256.
    fn sign_rsa(&self, data: &[u8]) -> SignatureResult<Vec<u8>> {
        use rsa::{RsaPrivateKey, pkcs1v15::SigningKey};
        use sha2::Sha256;
        use signature::{Signer, SignatureEncoding};
        use pkcs8::DecodePrivateKey;

        let private_key = RsaPrivateKey::from_pkcs8_der(&self.der_bytes).map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to parse RSA key: {}", e))
        })?;

        let signing_key = SigningKey::<Sha256>::new(private_key);
        let signature = signing_key.sign(data);

        Ok(signature.to_bytes().to_vec())
    }

    /// Signs data with ECDSA P-256.
    fn sign_ecdsa(&self, data: &[u8]) -> SignatureResult<Vec<u8>> {
        use p256::ecdsa::{SigningKey, Signature};
        use signature::Signer;
        use pkcs8::DecodePrivateKey;

        let signing_key = SigningKey::from_pkcs8_der(&self.der_bytes).map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to parse ECDSA key: {}", e))
        })?;

        let signature: Signature = signing_key.sign(data);

        Ok(signature.to_der().as_bytes().to_vec())
    }
}

impl std::fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateKey")
            .field("key_type", &self.key_type)
            .field("der_bytes_len", &self.der_bytes.len())
            .finish()
    }
}

/// Extracts the common name from an X.509 name.
fn extract_common_name(name: &x509_cert::name::Name) -> Option<String> {
    use const_oid::db::rfc4519::CN;

    for rdn in name.0.iter() {
        for attr in rdn.0.iter() {
            if attr.oid == CN {
                if let Ok(s) = std::str::from_utf8(attr.value.value()) {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

/// Converts PEM data to DER bytes.
fn pem_to_der(pem_data: &str, expected_label: &str) -> SignatureResult<Vec<u8>> {
    // Find the PEM block
    let begin_marker = format!("-----BEGIN {}-----", expected_label);
    let end_marker = format!("-----END {}-----", expected_label);

    let start = pem_data.find(&begin_marker).ok_or_else(|| {
        SignatureError::CertificateLoadFailed(format!(
            "Missing {} PEM header",
            expected_label
        ))
    })?;

    let end = pem_data.find(&end_marker).ok_or_else(|| {
        SignatureError::CertificateLoadFailed(format!(
            "Missing {} PEM footer",
            expected_label
        ))
    })?;

    let base64_data: String = pem_data[start + begin_marker.len()..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    // Decode base64
    let der_bytes = base64_decode(&base64_data)?;

    Ok(der_bytes)
}

/// Simple base64 decoder.
fn base64_decode(input: &str) -> SignatureResult<Vec<u8>> {
    fn decode_char(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => None, // Padding
            _ => None,
        }
    }

    let input = input.as_bytes();
    let mut output = Vec::with_capacity(input.len() * 3 / 4);

    for chunk in input.chunks(4) {
        if chunk.len() < 4 {
            break;
        }

        let a = decode_char(chunk[0]).unwrap_or(0);
        let b = decode_char(chunk[1]).unwrap_or(0);
        let c = decode_char(chunk[2]);
        let d = decode_char(chunk[3]);

        output.push((a << 2) | (b >> 4));

        if let Some(c) = c {
            output.push((b << 4) | (c >> 2));
            if let Some(d) = d {
                output.push((c << 6) | d);
            }
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_decode() {
        let decoded = base64_decode("SGVsbG8=").unwrap();
        assert_eq!(decoded, b"Hello");

        let decoded = base64_decode("SGVsbG8gV29ybGQh").unwrap();
        assert_eq!(decoded, b"Hello World!");
    }

    #[test]
    fn test_key_type_debug() {
        assert_eq!(format!("{:?}", KeyType::Rsa), "Rsa");
        assert_eq!(format!("{:?}", KeyType::EcdsaP256), "EcdsaP256");
    }
}
