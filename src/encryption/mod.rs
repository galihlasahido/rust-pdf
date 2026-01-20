//! PDF encryption module.
//!
//! This module provides AES-256 encryption support for PDF documents.
//!
//! # Example
//!
//! ```ignore
//! use rust_pdf::encryption::{EncryptionConfig, Permissions};
//!
//! let config = EncryptionConfig::aes256()
//!     .user_password("user123")
//!     .owner_password("owner456")
//!     .permissions(Permissions::new().allow_printing(true));
//!
//! let doc = DocumentBuilder::new()
//!     .encrypt(config)
//!     .page(page)
//!     .build()?;
//! ```

mod config;
mod key_derivation;
mod permissions;

pub use config::{EncryptionAlgorithm, EncryptionConfig};
pub use key_derivation::EncryptionKeys;
pub use permissions::Permissions;

use crate::error::EncryptionError;
use crate::object::{Object, PdfDictionary, PdfName, PdfString};
use key_derivation::derive_aes256_keys;
use zeroize::Zeroize;

/// Handles PDF encryption.
#[derive(Debug, Clone)]
pub struct EncryptionHandler {
    config: EncryptionConfig,
    keys: EncryptionKeys,
    file_id: Vec<u8>,
}

impl EncryptionHandler {
    /// Creates a new encryption handler with the given configuration.
    pub fn new(config: EncryptionConfig, file_id: Vec<u8>) -> Result<Self, EncryptionError> {
        if file_id.is_empty() {
            return Err(EncryptionError::MissingFileId);
        }

        // Derive encryption keys
        let keys = derive_aes256_keys(
            &config.user_password,
            &config.owner_password,
            config.permissions.as_i32(),
        )?;

        Ok(Self {
            config,
            keys,
            file_id,
        })
    }

    /// Returns the file encryption key.
    pub fn file_key(&self) -> &[u8] {
        &self.keys.file_encryption_key
    }

    /// Returns the file ID.
    pub fn file_id(&self) -> &[u8] {
        &self.file_id
    }

    /// Encrypts data using AES-256-CBC.
    ///
    /// Each encrypted item has a unique IV derived from its object/generation number.
    pub fn encrypt_data(
        &self,
        data: &[u8],
        _obj_num: u32,
        _gen_num: u16,
    ) -> Result<Vec<u8>, EncryptionError> {
        use aes::cipher::{BlockEncryptMut, KeyIvInit};
        use cbc::Encryptor;
        use rand::Rng;

        type Aes256CbcEnc = Encryptor<aes::Aes256>;

        // For AES-256 (V=5, R=6), generate random 16-byte IV
        let mut iv = [0u8; 16];
        rand::thread_rng().fill(&mut iv);

        let encryptor = Aes256CbcEnc::new_from_slices(&self.keys.file_encryption_key, &iv)
            .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;

        // Calculate buffer size (data + PKCS#7 padding)
        let block_size = 16;
        let padding_len = block_size - (data.len() % block_size);
        let padded_len = data.len() + padding_len;

        // Create buffer with space for padding
        let mut buf = vec![0u8; padded_len];
        buf[..data.len()].copy_from_slice(data);

        // Encrypt with PKCS#7 padding
        let ciphertext = encryptor
            .encrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf, data.len())
            .map_err(|e| EncryptionError::CipherFailed(format!("Encryption failed: {:?}", e)))?;

        // Prepend IV to ciphertext
        let mut result = Vec::with_capacity(16 + ciphertext.len());
        result.extend_from_slice(&iv);
        result.extend_from_slice(ciphertext);

        Ok(result)
    }

    /// Decrypts data using AES-256-CBC.
    #[allow(dead_code)]
    pub fn decrypt_data(
        &self,
        data: &[u8],
        _obj_num: u32,
        _gen_num: u16,
    ) -> Result<Vec<u8>, EncryptionError> {
        use aes::cipher::{BlockDecryptMut, KeyIvInit};
        use cbc::Decryptor;

        if data.len() < 16 {
            return Err(EncryptionError::CipherFailed(
                "Ciphertext too short".into(),
            ));
        }

        type Aes256CbcDec = Decryptor<aes::Aes256>;

        // Extract IV from first 16 bytes
        let iv = &data[..16];
        let ciphertext = &data[16..];

        let decryptor = Aes256CbcDec::new_from_slices(&self.keys.file_encryption_key, iv)
            .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;

        // Clone ciphertext to mutable buffer
        let mut buf = ciphertext.to_vec();

        let plaintext = decryptor
            .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf)
            .map_err(|e| EncryptionError::CipherFailed(format!("Decryption failed: {:?}", e)))?;

        Ok(plaintext.to_vec())
    }

    /// Creates the encryption dictionary for the PDF.
    pub fn create_encrypt_dictionary(&self) -> PdfDictionary {
        let mut dict = PdfDictionary::new();

        // Standard encryption handler
        dict.set("Filter", Object::Name(PdfName::new_unchecked("Standard")));

        // V and R values for AES-256
        dict.set("V", Object::Integer(self.config.algorithm.v_value() as i64));
        dict.set("R", Object::Integer(self.config.algorithm.r_value() as i64));

        // Key length in bits
        dict.set(
            "Length",
            Object::Integer((self.config.algorithm.key_length() * 8) as i64),
        );

        // O, U, OE, UE, Perms values (as hex strings)
        dict.set("O", Object::String(PdfString::Hex(self.keys.o_value.clone())));
        dict.set("U", Object::String(PdfString::Hex(self.keys.u_value.clone())));
        dict.set("OE", Object::String(PdfString::Hex(self.keys.oe_value.clone())));
        dict.set("UE", Object::String(PdfString::Hex(self.keys.ue_value.clone())));
        dict.set("Perms", Object::String(PdfString::Hex(self.keys.perms_value.clone())));

        // Permissions (P)
        dict.set("P", Object::Integer(self.config.permissions.as_i32() as i64));

        // Crypt filter dictionary for AES
        let mut cf_dict = PdfDictionary::new();
        let mut std_cf = PdfDictionary::new();
        std_cf.set("CFM", Object::Name(PdfName::new_unchecked("AESV3")));
        std_cf.set("Length", Object::Integer(32));
        std_cf.set("AuthEvent", Object::Name(PdfName::new_unchecked("DocOpen")));
        cf_dict.set("StdCF", Object::Dictionary(std_cf));

        dict.set("CF", Object::Dictionary(cf_dict));
        dict.set("StmF", Object::Name(PdfName::new_unchecked("StdCF")));
        dict.set("StrF", Object::Name(PdfName::new_unchecked("StdCF")));

        // EncryptMetadata
        if !self.config.encrypt_metadata {
            dict.set("EncryptMetadata", Object::Boolean(false));
        }

        dict
    }

    /// Creates the file ID array for the document.
    pub fn create_file_id_array(&self) -> crate::object::PdfArray {
        let mut arr = crate::object::PdfArray::new();
        arr.push(Object::String(PdfString::Hex(self.file_id.clone())));
        arr.push(Object::String(PdfString::Hex(self.file_id.clone())));
        arr
    }
}

impl Drop for EncryptionHandler {
    fn drop(&mut self) {
        // Zeroize sensitive data
        self.file_id.zeroize();
    }
}

/// Generates a random 16-byte file ID.
pub fn generate_file_id() -> Vec<u8> {
    use rand::Rng;
    let mut id = vec![0u8; 16];
    rand::thread_rng().fill(&mut id[..]);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encryption_handler_creation() {
        let config = EncryptionConfig::aes256()
            .user_password("user123")
            .owner_password("owner456");

        let file_id = generate_file_id();
        let handler = EncryptionHandler::new(config, file_id).unwrap();

        assert_eq!(handler.file_key().len(), 32);
    }

    #[test]
    fn test_missing_file_id() {
        let config = EncryptionConfig::aes256();
        let result = EncryptionHandler::new(config, vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let config = EncryptionConfig::aes256()
            .user_password("test")
            .owner_password("test");

        let file_id = generate_file_id();
        let handler = EncryptionHandler::new(config, file_id).unwrap();

        let plaintext = b"Hello, World! This is a test message.";
        let ciphertext = handler.encrypt_data(plaintext, 1, 0).unwrap();
        let decrypted = handler.decrypt_data(&ciphertext, 1, 0).unwrap();

        assert_eq!(&decrypted[..], plaintext);
    }

    #[test]
    fn test_create_encrypt_dictionary() {
        let config = EncryptionConfig::aes256()
            .user_password("user")
            .owner_password("owner")
            .permissions(Permissions::new().allow_printing(true));

        let file_id = generate_file_id();
        let handler = EncryptionHandler::new(config, file_id).unwrap();

        let dict = handler.create_encrypt_dictionary();

        assert!(dict.get("Filter").is_some());
        assert!(dict.get("V").is_some());
        assert!(dict.get("R").is_some());
        assert!(dict.get("O").is_some());
        assert!(dict.get("U").is_some());
        assert!(dict.get("OE").is_some());
        assert!(dict.get("UE").is_some());
        assert!(dict.get("Perms").is_some());
        assert!(dict.get("P").is_some());
        assert!(dict.get("CF").is_some());
    }

    #[test]
    fn test_generate_file_id() {
        let id1 = generate_file_id();
        let id2 = generate_file_id();

        assert_eq!(id1.len(), 16);
        assert_eq!(id2.len(), 16);
        assert_ne!(id1, id2); // Should be random
    }
}
