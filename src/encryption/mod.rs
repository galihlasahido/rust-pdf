//! PDF encryption module.
//!
//! This module provides AES-256 (V=5/R=6, ISO 32000-2) and AES-128
//! (V=4/R=4, ISO 32000-1) encryption support for PDF documents.
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
use key_derivation::{derive_aes128_keys, derive_aes256_keys, derive_object_key_r4};
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

        // Derive encryption keys. The two algorithms use genuinely
        // different key-derivation schemes (ISO 32000-2 Algorithm 2.A-2.D
        // for AES-256/R6 vs ISO 32000-1 Algorithms 2/3.3/3.5 for
        // AES-128/R4) -- see `create_encrypt_dictionary` and
        // `encrypt_data` below for the corresponding differences in the
        // emitted dictionary shape and per-object encryption.
        let keys = match config.algorithm {
            EncryptionAlgorithm::Aes256 => derive_aes256_keys(
                &config.user_password,
                &config.owner_password,
                config.permissions.as_i32(),
            )?,
            EncryptionAlgorithm::Aes128 => derive_aes128_keys(
                &config.user_password,
                &config.owner_password,
                config.permissions.as_i32(),
                &file_id,
                config.encrypt_metadata,
            )?,
        };

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

    /// Encrypts data using AES-256-CBC or AES-128-CBC, depending on
    /// `config.algorithm`.
    ///
    /// Each encrypted item has a unique random IV. For AES-128 (V=4,
    /// R=4), the *file* key is never used directly: ISO 32000-1's
    /// Algorithm 1 requires deriving a distinct per-object key from
    /// `obj_num`/`gen_num` first (unlike AES-256/V=5/R=6, where the file
    /// key doubles as the object key and `obj_num`/`gen_num` are unused).
    pub fn encrypt_data(
        &self,
        data: &[u8],
        obj_num: u32,
        gen_num: u16,
    ) -> Result<Vec<u8>, EncryptionError> {
        match self.config.algorithm {
            EncryptionAlgorithm::Aes256 => {
                Self::aes256_cbc_encrypt(&self.keys.file_encryption_key, data)
            }
            EncryptionAlgorithm::Aes128 => {
                let object_key =
                    derive_object_key_r4(&self.keys.file_encryption_key, obj_num, gen_num);
                Self::aes128_cbc_encrypt(&object_key, data)
            }
        }
    }

    fn aes256_cbc_encrypt(key: &[u8], data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        use aes::cipher::{BlockEncryptMut, KeyIvInit};
        use cbc::Encryptor;
        use rand::Rng;

        type Aes256CbcEnc = Encryptor<aes::Aes256>;

        let mut iv = [0u8; 16];
        rand::thread_rng().fill(&mut iv);

        let encryptor = Aes256CbcEnc::new_from_slices(key, &iv)
            .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;

        let block_size = 16;
        let padding_len = block_size - (data.len() % block_size);
        let padded_len = data.len() + padding_len;

        let mut buf = vec![0u8; padded_len];
        buf[..data.len()].copy_from_slice(data);

        let ciphertext = encryptor
            .encrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf, data.len())
            .map_err(|e| EncryptionError::CipherFailed(format!("Encryption failed: {:?}", e)))?;

        let mut result = Vec::with_capacity(16 + ciphertext.len());
        result.extend_from_slice(&iv);
        result.extend_from_slice(ciphertext);

        Ok(result)
    }

    fn aes128_cbc_encrypt(key: &[u8], data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        use aes::cipher::{BlockEncryptMut, KeyIvInit};
        use cbc::Encryptor;
        use rand::Rng;

        type Aes128CbcEnc = Encryptor<aes::Aes128>;

        let mut iv = [0u8; 16];
        rand::thread_rng().fill(&mut iv);

        let encryptor = Aes128CbcEnc::new_from_slices(key, &iv)
            .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;

        let block_size = 16;
        let padding_len = block_size - (data.len() % block_size);
        let padded_len = data.len() + padding_len;

        let mut buf = vec![0u8; padded_len];
        buf[..data.len()].copy_from_slice(data);

        let ciphertext = encryptor
            .encrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf, data.len())
            .map_err(|e| EncryptionError::CipherFailed(format!("Encryption failed: {:?}", e)))?;

        let mut result = Vec::with_capacity(16 + ciphertext.len());
        result.extend_from_slice(&iv);
        result.extend_from_slice(ciphertext);

        Ok(result)
    }

    /// Decrypts data using AES-256-CBC or AES-128-CBC, mirroring
    /// [`Self::encrypt_data`]'s algorithm/per-object-key handling.
    #[allow(dead_code)]
    pub fn decrypt_data(
        &self,
        data: &[u8],
        obj_num: u32,
        gen_num: u16,
    ) -> Result<Vec<u8>, EncryptionError> {
        use aes::cipher::{BlockDecryptMut, KeyIvInit};
        use cbc::Decryptor;

        if data.len() < 16 {
            return Err(EncryptionError::CipherFailed(
                "Ciphertext too short".into(),
            ));
        }

        let iv = &data[..16];
        let ciphertext = &data[16..];

        match self.config.algorithm {
            EncryptionAlgorithm::Aes256 => {
                type Aes256CbcDec = Decryptor<aes::Aes256>;
                let decryptor =
                    Aes256CbcDec::new_from_slices(&self.keys.file_encryption_key, iv)
                        .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;
                let mut buf = ciphertext.to_vec();
                let plaintext = decryptor
                    .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf)
                    .map_err(|e| {
                        EncryptionError::CipherFailed(format!("Decryption failed: {:?}", e))
                    })?;
                Ok(plaintext.to_vec())
            }
            EncryptionAlgorithm::Aes128 => {
                type Aes128CbcDec = Decryptor<aes::Aes128>;
                let object_key =
                    derive_object_key_r4(&self.keys.file_encryption_key, obj_num, gen_num);
                let decryptor = Aes128CbcDec::new_from_slices(&object_key, iv)
                    .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;
                let mut buf = ciphertext.to_vec();
                let plaintext = decryptor
                    .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf)
                    .map_err(|e| {
                        EncryptionError::CipherFailed(format!("Decryption failed: {:?}", e))
                    })?;
                Ok(plaintext.to_vec())
            }
        }
    }

    /// Creates the encryption dictionary for the PDF.
    ///
    /// The shape genuinely differs by algorithm, not just the `/V`/`/R`
    /// values: AES-256 (V=5/R=6) is ISO 32000-2's revision-6 scheme and
    /// requires `/OE`/`/UE`/`/Perms` plus an `AESV3` crypt filter, while
    /// AES-128 (V=4/R=4) is ISO 32000-1's revision-4 scheme, which has no
    /// `/OE`/`/UE`/`/Perms` entries at all (Algorithm 3.3/3.4 produce
    /// only 32-byte `/O`/`/U` values) and uses an `AESV2` crypt filter
    /// with a 16-byte key. Emitting the R=6-shaped fields under a `/V 4
    /// /R 4` header (or vice versa) would be a structurally invalid
    /// hybrid that no conformant reader can parse.
    pub fn create_encrypt_dictionary(&self) -> PdfDictionary {
        let mut dict = PdfDictionary::new();

        // Standard encryption handler
        dict.set("Filter", Object::Name(PdfName::new_unchecked("Standard")));

        dict.set("V", Object::Integer(self.config.algorithm.v_value() as i64));
        dict.set("R", Object::Integer(self.config.algorithm.r_value() as i64));

        // Key length in bits
        dict.set(
            "Length",
            Object::Integer((self.config.algorithm.key_length() * 8) as i64),
        );

        // O and U values (as hex strings) are common to both algorithms.
        dict.set("O", Object::String(PdfString::Hex(self.keys.o_value.clone())));
        dict.set("U", Object::String(PdfString::Hex(self.keys.u_value.clone())));

        // OE, UE, Perms only exist for the R=5/6 (AES-256) scheme (ISO
        // 32000-2 Table 20); R=4 (AES-128) has no equivalent entries.
        if self.config.algorithm == EncryptionAlgorithm::Aes256 {
            dict.set("OE", Object::String(PdfString::Hex(self.keys.oe_value.clone())));
            dict.set("UE", Object::String(PdfString::Hex(self.keys.ue_value.clone())));
            dict.set("Perms", Object::String(PdfString::Hex(self.keys.perms_value.clone())));
        }

        // Permissions (P)
        dict.set("P", Object::Integer(self.config.permissions.as_i32() as i64));

        // Crypt filter dictionary: AESV3/32-byte key for AES-256,
        // AESV2/16-byte key for AES-128.
        let cfm_name = match self.config.algorithm {
            EncryptionAlgorithm::Aes256 => "AESV3",
            EncryptionAlgorithm::Aes128 => "AESV2",
        };
        let mut cf_dict = PdfDictionary::new();
        let mut std_cf = PdfDictionary::new();
        std_cf.set("CFM", Object::Name(PdfName::new_unchecked(cfm_name)));
        std_cf.set(
            "Length",
            Object::Integer(self.config.algorithm.key_length() as i64),
        );
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

    // ===================================================================
    // AES-128 (V=4/R=4) end-to-end handler behavior. These exist because
    // a previous review found that `EncryptionHandler` silently derived
    // AES-256/R6 keys and wrote an AES-256-shaped `/CF`/`/OE`/`/UE`/
    // `/Perms` regardless of the *selected* algorithm, while still
    // claiming `/V 4 /R 4` -- a structurally invalid hybrid no real
    // reader can open. These tests pin the AES-128 path's actual output
    // shape and correctness so that regression can't reoccur silently.
    // ===================================================================

    #[test]
    fn test_aes128_handler_creation_derives_16_byte_key() {
        let config = EncryptionConfig::aes128().user_password("user123").owner_password("owner456");
        let file_id = generate_file_id();
        let handler = EncryptionHandler::new(config, file_id).unwrap();

        assert_eq!(handler.file_key().len(), 16, "AES-128 file key must be 16 bytes, not 32");
    }

    #[test]
    fn test_aes128_encrypt_decrypt_roundtrip() {
        let config = EncryptionConfig::aes128().user_password("test").owner_password("test");
        let file_id = generate_file_id();
        let handler = EncryptionHandler::new(config, file_id).unwrap();

        let plaintext = b"Hello, World! This is an AES-128 test message.";
        let ciphertext = handler.encrypt_data(plaintext, 3, 0).unwrap();
        let decrypted = handler.decrypt_data(&ciphertext, 3, 0).unwrap();

        assert_eq!(&decrypted[..], plaintext);
    }

    #[test]
    fn test_aes128_encrypt_requires_matching_object_id_to_decrypt() {
        // Confirms per-object key derivation (Algorithm 1) is actually
        // wired into encrypt/decrypt: ciphertext produced for object (5,
        // 0) must NOT decrypt correctly under object (7, 0)'s key --
        // otherwise every object would silently share one key, which is
        // only valid for the AES-256/R6 scheme, not R4.
        let config = EncryptionConfig::aes128().user_password("test");
        let file_id = generate_file_id();
        let handler = EncryptionHandler::new(config, file_id).unwrap();

        let plaintext = b"0123456789ABCDEF"; // exactly one block
        let ciphertext = handler.encrypt_data(plaintext, 5, 0).unwrap();

        // Decrypting under the wrong object id must not silently recover
        // the exact original plaintext (either an outright cipher error,
        // due to bad PKCS#7 padding, or, if padding happens to validate,
        // different bytes).
        match handler.decrypt_data(&ciphertext, 7, 0) {
            Ok(wrong_plaintext) => assert_ne!(
                wrong_plaintext, plaintext,
                "decrypting with the wrong object's key must not reproduce the original plaintext"
            ),
            Err(_) => {} // also an acceptable outcome (bad padding)
        }
    }

    #[test]
    fn test_aes128_encrypt_dictionary_is_structurally_valid_r4() {
        let config = EncryptionConfig::aes128()
            .user_password("user")
            .owner_password("owner")
            .permissions(Permissions::new().allow_printing(true));
        let file_id = generate_file_id();
        let handler = EncryptionHandler::new(config, file_id).unwrap();

        let dict = handler.create_encrypt_dictionary();

        assert_eq!(dict.get("V"), Some(&Object::Integer(4)));
        assert_eq!(dict.get("R"), Some(&Object::Integer(4)));
        assert_eq!(dict.get("Length"), Some(&Object::Integer(128)));

        // R4 has no OE/UE/Perms entries -- those are R5/6-only (ISO
        // 32000-2 Table 20). Emitting them under /V 4 /R 4 would be the
        // exact structurally-invalid hybrid this fix corrects.
        assert!(dict.get("OE").is_none(), "R4 dictionary must not carry an /OE entry");
        assert!(dict.get("UE").is_none(), "R4 dictionary must not carry a /UE entry");
        assert!(dict.get("Perms").is_none(), "R4 dictionary must not carry a /Perms entry");

        // O/U must be 32 bytes (not 48, unlike R6).
        match dict.get("O") {
            Some(Object::String(PdfString::Hex(o))) => assert_eq!(o.len(), 32),
            other => panic!("expected /O to be a hex string, got {other:?}"),
        }
        match dict.get("U") {
            Some(Object::String(PdfString::Hex(u))) => assert_eq!(u.len(), 32),
            other => panic!("expected /U to be a hex string, got {other:?}"),
        }

        // Crypt filter must be AESV2 with a 16-byte (128-bit) key, not
        // AESV3/32-byte.
        match dict.get("CF") {
            Some(Object::Dictionary(cf)) => match cf.get("StdCF") {
                Some(Object::Dictionary(std_cf)) => {
                    assert_eq!(
                        std_cf.get("CFM"),
                        Some(&Object::Name(PdfName::new_unchecked("AESV2")))
                    );
                    assert_eq!(std_cf.get("Length"), Some(&Object::Integer(16)));
                }
                other => panic!("expected /CF/StdCF to be a dictionary, got {other:?}"),
            },
            other => panic!("expected /CF to be a dictionary, got {other:?}"),
        }
    }

    #[test]
    fn test_aes128_and_aes256_dictionaries_differ_in_shape() {
        // Direct regression guard for the originally-reported bug: an
        // AES-128-configured handler's dictionary must genuinely differ
        // from an AES-256-configured one (not just in /V and /R, which
        // was already true even in the buggy version), specifically in
        // whether OE/UE/Perms are present and which CFM is used.
        let file_id_128 = generate_file_id();
        let handler_128 =
            EncryptionHandler::new(EncryptionConfig::aes128().user_password("pw"), file_id_128).unwrap();
        let file_id_256 = generate_file_id();
        let handler_256 =
            EncryptionHandler::new(EncryptionConfig::aes256().user_password("pw"), file_id_256).unwrap();

        let dict_128 = handler_128.create_encrypt_dictionary();
        let dict_256 = handler_256.create_encrypt_dictionary();

        assert!(dict_128.get("OE").is_none());
        assert!(dict_256.get("OE").is_some());

        let cfm_128 = match dict_128.get("CF") {
            Some(Object::Dictionary(cf)) => match cf.get("StdCF") {
                Some(Object::Dictionary(std_cf)) => std_cf.get("CFM").cloned(),
                _ => None,
            },
            _ => None,
        };
        let cfm_256 = match dict_256.get("CF") {
            Some(Object::Dictionary(cf)) => match cf.get("StdCF") {
                Some(Object::Dictionary(std_cf)) => std_cf.get("CFM").cloned(),
                _ => None,
            },
            _ => None,
        };
        assert_eq!(cfm_128, Some(Object::Name(PdfName::new_unchecked("AESV2"))));
        assert_eq!(cfm_256, Some(Object::Name(PdfName::new_unchecked("AESV3"))));
    }
}
