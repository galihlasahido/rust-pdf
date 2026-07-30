//! X.509 certificate and private key handling.

use super::SignatureResult;
use crate::error::SignatureError;
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
        use der::Decode;
        use x509_cert::Certificate as X509Cert;

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
        use der::Decode;
        use x509_cert::Certificate as X509Cert;

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

    /// Checks whether the certificate's validity period covers the current
    /// system time.
    ///
    /// This only checks the `notBefore`/`notAfter` fields — it does not
    /// verify the certificate against a trust store or check revocation.
    pub fn is_currently_valid(&self) -> SignatureResult<bool> {
        use der::Decode;
        use std::time::SystemTime;
        use x509_cert::Certificate as X509Cert;

        let cert = X509Cert::from_der(&self.der_bytes).map_err(|e| {
            SignatureError::CertificateLoadFailed(format!("Failed to parse certificate: {}", e))
        })?;

        let not_before = cert.tbs_certificate.validity.not_before.to_unix_duration();
        let not_after = cert.tbs_certificate.validity.not_after.to_unix_duration();

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| {
                SignatureError::CertificateLoadFailed(format!("System clock error: {}", e))
            })?;

        Ok(now >= not_before && now <= not_after)
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
    /// ECDSA P-384 private key.
    EcdsaP384,
    /// ECDSA P-521 private key.
    EcdsaP521,
}

/// SEC 2 / RFC 5480 `namedCurve` OIDs for the three NIST curves this crate
/// signs with. Shared by both the PKCS#8 (`algorithm.parameters`) and SEC1
/// (`ECPrivateKey.parameters`) code paths below, since both encode the
/// curve the same way -- as this bare OID.
const OID_P256: &str = "1.2.840.10045.3.1.7";
const OID_P384: &str = "1.3.132.0.34";
const OID_P521: &str = "1.3.132.0.35";

impl PrivateKey {
    /// Loads a private key from a PEM file.
    pub fn from_pem_file(path: impl AsRef<Path>) -> SignatureResult<Self> {
        let pem_data = fs::read_to_string(path.as_ref()).map_err(|e| {
            SignatureError::PrivateKeyLoadFailed(format!("Failed to read file: {}", e))
        })?;

        Self::from_pem(&pem_data)
    }

    /// Loads a private key from PEM data.
    ///
    /// Keys in the legacy PKCS#1 (`BEGIN RSA PRIVATE KEY`) and SEC1
    /// (`BEGIN EC PRIVATE KEY`) formats are normalized to PKCS#8 DER on
    /// load, since [`PrivateKey::sign`] only knows how to parse PKCS#8.
    pub fn from_pem(pem_data: &str) -> SignatureResult<Self> {
        // Try PKCS#8 format first
        if pem_data.contains("BEGIN PRIVATE KEY") {
            let der_bytes = pem_to_der(pem_data, "PRIVATE KEY")?;
            return Self::from_pkcs8_der(&der_bytes);
        }

        // Try RSA private key format (PKCS#1)
        if pem_data.contains("BEGIN RSA PRIVATE KEY") {
            let der_bytes = pem_to_der(pem_data, "RSA PRIVATE KEY")?;
            return Ok(Self {
                key_type: KeyType::Rsa,
                der_bytes: rsa_pkcs1_der_to_pkcs8(&der_bytes)?,
            });
        }

        // Try EC private key format (SEC1)
        if pem_data.contains("BEGIN EC PRIVATE KEY") {
            let der_bytes = pem_to_der(pem_data, "EC PRIVATE KEY")?;
            let (key_type, pkcs8_der) = ec_sec1_der_to_pkcs8(&der_bytes)?;
            return Ok(Self {
                key_type,
                der_bytes: pkcs8_der,
            });
        }

        Err(SignatureError::PrivateKeyLoadFailed(
            "Unsupported private key format".to_string(),
        ))
    }

    /// Loads a private key from PKCS#8 DER bytes.
    fn from_pkcs8_der(der_bytes: &[u8]) -> SignatureResult<Self> {
        use der::Decode;
        use pkcs8::PrivateKeyInfo;

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
            // For an EC key, `algorithm.oid` is only the generic
            // `id-ecPublicKey` -- the actual curve lives in the sibling
            // `algorithm.parameters` field as a bare `namedCurve` OID
            // (RFC 5480 §2.1.1), so it must be inspected separately to tell
            // P-256/P-384/P-521 apart.
            ec_curve_key_type(&key_info)?
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
            KeyType::EcdsaP256 => self.sign_ecdsa_p256(data),
            KeyType::EcdsaP384 => self.sign_ecdsa_p384(data),
            KeyType::EcdsaP521 => self.sign_ecdsa_p521(data),
        }
    }

    /// Signs data with RSA-SHA256.
    fn sign_rsa(&self, data: &[u8]) -> SignatureResult<Vec<u8>> {
        use pkcs8::DecodePrivateKey;
        use rsa::{pkcs1v15::SigningKey, RsaPrivateKey};
        use sha2::Sha256;
        use signature::{SignatureEncoding, Signer};

        let private_key = RsaPrivateKey::from_pkcs8_der(&self.der_bytes).map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to parse RSA key: {}", e))
        })?;

        let signing_key = SigningKey::<Sha256>::new(private_key);
        let signature = signing_key.sign(data);

        Ok(signature.to_bytes().to_vec())
    }

    /// Signs data with ECDSA P-256.
    fn sign_ecdsa_p256(&self, data: &[u8]) -> SignatureResult<Vec<u8>> {
        use p256::ecdsa::{Signature, SigningKey};
        use pkcs8::DecodePrivateKey;
        use signature::Signer;

        let signing_key = SigningKey::from_pkcs8_der(&self.der_bytes).map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to parse ECDSA key: {}", e))
        })?;

        let signature: Signature = signing_key.sign(data);

        Ok(signature.to_der().as_bytes().to_vec())
    }

    /// Signs data with ECDSA P-384.
    fn sign_ecdsa_p384(&self, data: &[u8]) -> SignatureResult<Vec<u8>> {
        use p384::ecdsa::{Signature, SigningKey};
        use pkcs8::DecodePrivateKey;
        use signature::Signer;

        let signing_key = SigningKey::from_pkcs8_der(&self.der_bytes).map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to parse ECDSA key: {}", e))
        })?;

        let signature: Signature = signing_key.sign(data);

        Ok(signature.to_der().as_bytes().to_vec())
    }

    /// Signs data with ECDSA P-521.
    ///
    /// Unlike `p256`/`p384`, `p521::ecdsa::SigningKey` is a hand-rolled
    /// newtype (see that crate's own "TODO: use RFC6979 + upstream types
    /// from the `ecdsa` crate" comment) which does not implement
    /// `DecodePrivateKey`/PKCS#8 itself -- only `p521::SecretKey` (a plain
    /// `elliptic_curve::SecretKey<NistP521>` alias) does. So the PKCS#8 key
    /// is parsed as a `SecretKey` first and its raw scalar handed to
    /// `SigningKey::from_bytes`.
    fn sign_ecdsa_p521(&self, data: &[u8]) -> SignatureResult<Vec<u8>> {
        use p521::ecdsa::{Signature, SigningKey};
        use pkcs8::DecodePrivateKey;
        use signature::Signer;

        let secret_key = p521::SecretKey::from_pkcs8_der(&self.der_bytes).map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to parse ECDSA key: {}", e))
        })?;
        let signing_key = SigningKey::from_bytes(&secret_key.to_bytes()).map_err(|e| {
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

/// Determines which NIST curve a PKCS#8-encoded EC private key uses.
///
/// `key_info.algorithm.oid` for an EC key is only ever the generic
/// `id-ecPublicKey` (`1.2.840.10045.2.1`) -- the curve itself is carried in
/// the sibling `algorithm.parameters` field as a bare `namedCurve` OID
/// (RFC 5480 §2.1.1), which is what this decodes.
fn ec_curve_key_type(key_info: &pkcs8::PrivateKeyInfo<'_>) -> SignatureResult<KeyType> {
    let params = key_info.algorithm.parameters.ok_or_else(|| {
        SignatureError::PrivateKeyLoadFailed(
            "EC private key is missing its namedCurve parameters".to_string(),
        )
    })?;

    let curve_oid = const_oid::ObjectIdentifier::try_from(params).map_err(|e| {
        SignatureError::PrivateKeyLoadFailed(format!("Failed to parse EC curve OID: {}", e))
    })?;

    key_type_for_curve_oid(&curve_oid.to_string())
}

/// Maps a SEC 2 / RFC 5480 `namedCurve` OID (as a string, e.g. from
/// [`const_oid::ObjectIdentifier::to_string`]) to the [`KeyType`] this crate
/// supports signing/verifying with.
fn key_type_for_curve_oid(oid: &str) -> SignatureResult<KeyType> {
    match oid {
        OID_P256 => Ok(KeyType::EcdsaP256),
        OID_P384 => Ok(KeyType::EcdsaP384),
        OID_P521 => Ok(KeyType::EcdsaP521),
        other => Err(SignatureError::PrivateKeyLoadFailed(format!(
            "Unsupported EC curve OID: {}",
            other
        ))),
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

/// Converts a PKCS#1-encoded RSA private key (`RSAPrivateKey` DER, as
/// produced by e.g. `openssl genrsa`) to PKCS#8 DER.
fn rsa_pkcs1_der_to_pkcs8(der_bytes: &[u8]) -> SignatureResult<Vec<u8>> {
    use rsa::pkcs1::DecodeRsaPrivateKey;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::RsaPrivateKey;

    let key = RsaPrivateKey::from_pkcs1_der(der_bytes).map_err(|e| {
        SignatureError::PrivateKeyLoadFailed(format!("Failed to parse PKCS#1 RSA key: {}", e))
    })?;
    let pkcs8_doc = key.to_pkcs8_der().map_err(|e| {
        SignatureError::PrivateKeyLoadFailed(format!("Failed to convert RSA key to PKCS#8: {}", e))
    })?;

    Ok(pkcs8_doc.as_bytes().to_vec())
}

/// Converts a SEC1-encoded EC private key (`ECPrivateKey` DER, as produced
/// by e.g. `openssl ecparam -genkey`) to PKCS#8 DER, also returning which
/// curve it turned out to be.
///
/// Unlike the PKCS#8 case, SEC1's `ECPrivateKey.parameters` `namedCurve` OID
/// is OPTIONAL (SEC1 §C.4/RFC 5915) -- `openssl ecparam -genkey` always
/// emits it, but a strictly-conformant producer need not. When present we
/// use it directly; when absent we fall back to the private-key scalar's
/// byte length, which is unambiguous for these three curves (32/48/66
/// bytes for P-256/P-384/P-521 respectively -- each curve's exact-length
/// case in `elliptic_curve::SecretKey::from_slice` only matches its own
/// size, so trying curves smallest-first and stopping at the first that
/// parses can't misidentify a larger key as a smaller curve).
fn ec_sec1_der_to_pkcs8(der_bytes: &[u8]) -> SignatureResult<(KeyType, Vec<u8>)> {
    use sec1::EcPrivateKey;

    let ec_key = EcPrivateKey::try_from(der_bytes).map_err(|e| {
        SignatureError::PrivateKeyLoadFailed(format!("Failed to parse SEC1 EC key: {}", e))
    })?;

    let key_type = match ec_key.parameters.and_then(|p| p.named_curve()) {
        Some(oid) => Some(key_type_for_curve_oid(&oid.to_string())?),
        None => None,
    };

    match key_type {
        Some(KeyType::EcdsaP256) => Ok((KeyType::EcdsaP256, sec1_to_pkcs8_p256(der_bytes)?)),
        Some(KeyType::EcdsaP384) => Ok((KeyType::EcdsaP384, sec1_to_pkcs8_p384(der_bytes)?)),
        Some(KeyType::EcdsaP521) => Ok((KeyType::EcdsaP521, sec1_to_pkcs8_p521(der_bytes)?)),
        Some(KeyType::Rsa) => unreachable!("key_type_for_curve_oid never returns KeyType::Rsa"),
        // No embedded curve OID: try each curve smallest-first (see doc
        // comment above) and take the first that parses.
        None => sec1_to_pkcs8_p256(der_bytes)
            .map(|pkcs8| (KeyType::EcdsaP256, pkcs8))
            .or_else(|_| sec1_to_pkcs8_p384(der_bytes).map(|pkcs8| (KeyType::EcdsaP384, pkcs8)))
            .or_else(|_| sec1_to_pkcs8_p521(der_bytes).map(|pkcs8| (KeyType::EcdsaP521, pkcs8))),
    }
}

/// Parses a SEC1 `ECPrivateKey` DER blob as a P-256 key and re-encodes it as
/// PKCS#8 DER.
fn sec1_to_pkcs8_p256(der_bytes: &[u8]) -> SignatureResult<Vec<u8>> {
    use p256::pkcs8::EncodePrivateKey;
    use p256::SecretKey;

    let key = SecretKey::from_sec1_der(der_bytes).map_err(|e| {
        SignatureError::PrivateKeyLoadFailed(format!("Failed to parse SEC1 EC key: {}", e))
    })?;
    let pkcs8_doc = key.to_pkcs8_der().map_err(|e| {
        SignatureError::PrivateKeyLoadFailed(format!("Failed to convert EC key to PKCS#8: {}", e))
    })?;

    Ok(pkcs8_doc.as_bytes().to_vec())
}

/// Parses a SEC1 `ECPrivateKey` DER blob as a P-384 key and re-encodes it as
/// PKCS#8 DER.
fn sec1_to_pkcs8_p384(der_bytes: &[u8]) -> SignatureResult<Vec<u8>> {
    use p384::pkcs8::EncodePrivateKey;
    use p384::SecretKey;

    let key = SecretKey::from_sec1_der(der_bytes).map_err(|e| {
        SignatureError::PrivateKeyLoadFailed(format!("Failed to parse SEC1 EC key: {}", e))
    })?;
    let pkcs8_doc = key.to_pkcs8_der().map_err(|e| {
        SignatureError::PrivateKeyLoadFailed(format!("Failed to convert EC key to PKCS#8: {}", e))
    })?;

    Ok(pkcs8_doc.as_bytes().to_vec())
}

/// Parses a SEC1 `ECPrivateKey` DER blob as a P-521 key and re-encodes it as
/// PKCS#8 DER.
fn sec1_to_pkcs8_p521(der_bytes: &[u8]) -> SignatureResult<Vec<u8>> {
    use p521::pkcs8::EncodePrivateKey;
    use p521::SecretKey;

    let key = SecretKey::from_sec1_der(der_bytes).map_err(|e| {
        SignatureError::PrivateKeyLoadFailed(format!("Failed to parse SEC1 EC key: {}", e))
    })?;
    let pkcs8_doc = key.to_pkcs8_der().map_err(|e| {
        SignatureError::PrivateKeyLoadFailed(format!("Failed to convert EC key to PKCS#8: {}", e))
    })?;

    Ok(pkcs8_doc.as_bytes().to_vec())
}

/// Converts PEM data to DER bytes.
fn pem_to_der(pem_data: &str, expected_label: &str) -> SignatureResult<Vec<u8>> {
    // Find the PEM block
    let begin_marker = format!("-----BEGIN {}-----", expected_label);
    let end_marker = format!("-----END {}-----", expected_label);

    let start = pem_data.find(&begin_marker).ok_or_else(|| {
        SignatureError::CertificateLoadFailed(format!("Missing {} PEM header", expected_label))
    })?;

    let end = pem_data.find(&end_marker).ok_or_else(|| {
        SignatureError::CertificateLoadFailed(format!("Missing {} PEM footer", expected_label))
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
        assert_eq!(format!("{:?}", KeyType::EcdsaP384), "EcdsaP384");
        assert_eq!(format!("{:?}", KeyType::EcdsaP521), "EcdsaP521");
    }

    #[test]
    fn test_key_type_for_curve_oid() {
        assert_eq!(
            key_type_for_curve_oid(OID_P256).unwrap(),
            KeyType::EcdsaP256
        );
        assert_eq!(
            key_type_for_curve_oid(OID_P384).unwrap(),
            KeyType::EcdsaP384
        );
        assert_eq!(
            key_type_for_curve_oid(OID_P521).unwrap(),
            KeyType::EcdsaP521
        );
        assert!(key_type_for_curve_oid("1.2.3.4").is_err());
    }
}
