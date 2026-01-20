//! Key derivation functions for PDF encryption.
//!
//! Implements the key derivation algorithms from PDF 2.0 specification (ISO 32000-2).
//! Specifically Algorithm 2.A, 2.B, 2.C, and 2.D for R=6 (AES-256) encryption.

use crate::error::EncryptionError;
use sha2::{Digest, Sha256, Sha384, Sha512};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Encryption key material derived from passwords.
#[derive(Debug, Clone, Zeroize, ZeroizeOnDrop)]
pub struct EncryptionKeys {
    /// The file encryption key (32 bytes for AES-256).
    pub file_encryption_key: Vec<u8>,
    /// The O (owner) value for the encryption dictionary.
    pub o_value: Vec<u8>,
    /// The U (user) value for the encryption dictionary.
    pub u_value: Vec<u8>,
    /// The OE (owner encrypted key) value.
    pub oe_value: Vec<u8>,
    /// The UE (user encrypted key) value.
    pub ue_value: Vec<u8>,
    /// The Perms (permissions validation) value.
    pub perms_value: Vec<u8>,
}

/// Derives encryption keys for AES-256 (V=5, R=6) encryption.
///
/// This implements:
/// - Algorithm 2.B (computing U and UE values)
/// - Algorithm 2.C (computing O and OE values)
/// - Algorithm 2.D (computing Perms value)
/// from ISO 32000-2.
pub fn derive_aes256_keys(
    user_password: &str,
    owner_password: &str,
    permissions: i32,
) -> Result<EncryptionKeys, EncryptionError> {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    // Generate random salts (8 bytes each)
    let mut user_validation_salt = [0u8; 8];
    let mut user_key_salt = [0u8; 8];
    let mut owner_validation_salt = [0u8; 8];
    let mut owner_key_salt = [0u8; 8];

    rng.fill(&mut user_validation_salt);
    rng.fill(&mut user_key_salt);
    rng.fill(&mut owner_validation_salt);
    rng.fill(&mut owner_key_salt);

    // Generate random file encryption key (32 bytes)
    let mut file_encryption_key = [0u8; 32];
    rng.fill(&mut file_encryption_key);

    // Truncate/pad passwords to UTF-8 (max 127 bytes)
    let user_pwd = truncate_password(user_password);
    let owner_pwd = truncate_password(owner_password);

    // ===== Algorithm 2.B: Computing U and UE =====

    // Step 2: Compute hash using Algorithm 2.A with password and user validation salt
    let user_hash = compute_hash_2a(&user_pwd, &user_validation_salt, None)?;

    // Step 3: U = Hash (32 bytes) || user_validation_salt (8 bytes) || user_key_salt (8 bytes)
    let mut u_value = Vec::with_capacity(48);
    u_value.extend_from_slice(&user_hash);
    u_value.extend_from_slice(&user_validation_salt);
    u_value.extend_from_slice(&user_key_salt);

    // Step 4: Compute hash using Algorithm 2.A with password and user key salt
    let user_key = compute_hash_2a(&user_pwd, &user_key_salt, None)?;

    // Step 5: UE = AES-256-CBC(user_key, IV=0, file_encryption_key)
    let ue_value = aes_cbc_encrypt_no_padding(&user_key, &[0u8; 16], &file_encryption_key)?;

    // ===== Algorithm 2.C: Computing O and OE =====

    // Step 2: Compute hash using Algorithm 2.A with password, owner validation salt, and U
    let owner_hash = compute_hash_2a(&owner_pwd, &owner_validation_salt, Some(&u_value))?;

    // Step 3: O = Hash (32 bytes) || owner_validation_salt (8 bytes) || owner_key_salt (8 bytes)
    let mut o_value = Vec::with_capacity(48);
    o_value.extend_from_slice(&owner_hash);
    o_value.extend_from_slice(&owner_validation_salt);
    o_value.extend_from_slice(&owner_key_salt);

    // Step 4: Compute hash using Algorithm 2.A with password, owner key salt, and U
    let owner_key = compute_hash_2a(&owner_pwd, &owner_key_salt, Some(&u_value))?;

    // Step 5: OE = AES-256-CBC(owner_key, IV=0, file_encryption_key)
    let oe_value = aes_cbc_encrypt_no_padding(&owner_key, &[0u8; 16], &file_encryption_key)?;

    // ===== Algorithm 2.D: Computing Perms =====
    let perms_value = compute_perms(&file_encryption_key, permissions, true)?;

    Ok(EncryptionKeys {
        file_encryption_key: file_encryption_key.to_vec(),
        o_value,
        u_value,
        oe_value,
        ue_value,
        perms_value,
    })
}

/// Algorithm 2.A: Computing a hash (Security handlers of revision 6)
///
/// This is the iterative hash function used for password validation and key derivation
/// in PDF 2.0 encryption (ISO 32000-2).
///
/// # Arguments
/// * `password` - The password bytes (UTF-8, max 127 bytes)
/// * `salt` - 8-byte salt value
/// * `user_bytes` - Optional 48-byte U value (used for owner password operations)
///
/// # Returns
/// 32-byte hash value
fn compute_hash_2a(
    password: &[u8],
    salt: &[u8],
    user_bytes: Option<&[u8]>,
) -> Result<[u8; 32], EncryptionError> {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};

    type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

    // Get u slice (empty for user password, full U value for owner password)
    let u = user_bytes.unwrap_or(&[]);

    // Step 1: Initial SHA-256 hash of (password || salt || u)
    let mut hasher = Sha256::new();
    hasher.update(password);
    hasher.update(salt);
    hasher.update(u);
    let initial_hash: [u8; 32] = hasher.finalize().into();

    // Block stores the current hash (can be 32, 48, or 64 bytes depending on hash used)
    let mut block = [0u8; 64];
    block[..32].copy_from_slice(&initial_hash);
    let mut block_size = 32usize; // Starts at 32 (SHA-256), can become 48 (SHA-384) or 64 (SHA-512)

    // Pre-allocate data buffer for K1 (will be resized as needed)
    let mut data = Vec::with_capacity((password.len() + 64 + u.len()) * 64);

    let mut round_number = 0usize;

    loop {
        // Build K1: 64 repetitions of (password || block[..block_size] || u)
        let repeat_len = password.len() + block_size + u.len();
        let total_len = repeat_len * 64;

        // Resize data buffer if needed
        data.clear();
        data.resize(total_len, 0);

        // Build first repetition
        data[..password.len()].copy_from_slice(password);
        data[password.len()..password.len() + block_size].copy_from_slice(&block[..block_size]);
        data[password.len() + block_size..repeat_len].copy_from_slice(u);

        // Copy to create 64 repetitions
        for j in 1..64 {
            data.copy_within(..repeat_len, j * repeat_len);
        }

        // Encrypt K1 with AES-128-CBC
        // Key = block[0:16], IV = block[16:32]
        let aes_key: &[u8; 16] = block[..16].try_into().unwrap();
        let aes_iv: &[u8; 16] = block[16..32].try_into().unwrap();

        let encryptor = Aes128CbcEnc::new(aes_key.into(), aes_iv.into());
        let encrypted = encryptor
            .encrypt_padded_mut::<aes::cipher::block_padding::NoPadding>(&mut data[..total_len], total_len)
            .map_err(|e| EncryptionError::CipherFailed(format!("AES encryption failed: {:?}", e)))?;

        // Compute mod 3 from sum of FIRST 16 bytes of encrypted data
        let sum: usize = encrypted[..16].iter().map(|&b| b as usize).sum();
        let remainder = sum % 3;

        // Select hash algorithm and compute new block_size: 0=SHA-256(32), 1=SHA-384(48), 2=SHA-512(64)
        let new_block_size = remainder * 16 + 32;

        // Hash encrypted data with selected algorithm
        match remainder {
            0 => {
                let hash: [u8; 32] = Sha256::digest(encrypted).into();
                block[..32].copy_from_slice(&hash);
            }
            1 => {
                let hash: [u8; 48] = Sha384::digest(encrypted).into();
                block[..48].copy_from_slice(&hash);
            }
            _ => {
                let hash: [u8; 64] = Sha512::digest(encrypted).into();
                block.copy_from_slice(&hash);
            }
        }
        block_size = new_block_size;

        round_number += 1;

        // Exit condition: at least 64 rounds AND last_byte + 32 <= round_number
        // This is equivalent to: last_byte <= round_number - 32
        let last_byte = encrypted[total_len - 1] as usize;
        if round_number >= 64 && last_byte + 32 <= round_number {
            break;
        }

        // Safety limit to prevent infinite loops
        if round_number > 2048 {
            break;
        }
    }

    // Return first 32 bytes of final block
    let mut result = [0u8; 32];
    result.copy_from_slice(&block[..32]);
    Ok(result)
}

/// Truncates a password to at most 127 bytes (UTF-8).
fn truncate_password(password: &str) -> Vec<u8> {
    let bytes = password.as_bytes();
    if bytes.len() <= 127 {
        bytes.to_vec()
    } else {
        // Truncate at UTF-8 boundary
        let mut len = 127;
        while len > 0 && !password.is_char_boundary(len) {
            len -= 1;
        }
        bytes[..len].to_vec()
    }
}

/// Computes the Perms value (16 bytes).
fn compute_perms(
    file_key: &[u8],
    permissions: i32,
    encrypt_metadata: bool,
) -> Result<Vec<u8>, EncryptionError> {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    // Build the 16-byte plaintext
    let mut perms_plain = [0u8; 16];

    // Bytes 0-3: permissions (little-endian)
    perms_plain[0..4].copy_from_slice(&permissions.to_le_bytes());

    // Bytes 4-7: 0xFFFFFFFF
    perms_plain[4..8].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());

    // Byte 8: 'T' or 'F' for EncryptMetadata
    perms_plain[8] = if encrypt_metadata { b'T' } else { b'F' };

    // Byte 9: 'a'
    perms_plain[9] = b'a';

    // Byte 10: 'd'
    perms_plain[10] = b'd';

    // Byte 11: 'b'
    perms_plain[11] = b'b';

    // Bytes 12-15: random
    rng.fill(&mut perms_plain[12..16]);

    // Encrypt with AES-256-ECB (no IV, single block)
    aes_ecb_encrypt(file_key, &perms_plain)
}

/// AES-256-CBC encryption (PKCS#7 padding).
#[allow(dead_code)]
fn aes_cbc_encrypt(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    use cbc::Encryptor;

    type Aes256CbcEnc = Encryptor<aes::Aes256>;

    let encryptor = Aes256CbcEnc::new_from_slices(key, iv)
        .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;

    // Calculate buffer size (plaintext + PKCS#7 padding)
    let block_size = 16;
    let padding_len = block_size - (plaintext.len() % block_size);
    let padded_len = plaintext.len() + padding_len;

    // Create buffer with space for padding
    let mut buf = vec![0u8; padded_len];
    buf[..plaintext.len()].copy_from_slice(plaintext);

    // Encrypt with PKCS#7 padding
    let ciphertext = encryptor
        .encrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf, plaintext.len())
        .map_err(|e| EncryptionError::CipherFailed(format!("Encryption failed: {:?}", e)))?;

    Ok(ciphertext.to_vec())
}

/// AES-256-CBC encryption without padding (for block-aligned data like UE/OE).
fn aes_cbc_encrypt_no_padding(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    use cbc::Encryptor;

    type Aes256CbcEnc = Encryptor<aes::Aes256>;

    if plaintext.len() % 16 != 0 {
        return Err(EncryptionError::CipherFailed(
            "Plaintext must be block-aligned for no-padding encryption".into(),
        ));
    }

    let encryptor = Aes256CbcEnc::new_from_slices(key, iv)
        .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;

    // Create buffer - same size as plaintext since no padding
    let mut buf = plaintext.to_vec();

    // Encrypt using NoPadding since data is already block-aligned
    let ciphertext = encryptor
        .encrypt_padded_mut::<aes::cipher::block_padding::NoPadding>(&mut buf, plaintext.len())
        .map_err(|e| EncryptionError::CipherFailed(format!("Encryption failed: {:?}", e)))?;

    Ok(ciphertext.to_vec())
}

/// AES-256-ECB encryption (single block, no padding).
fn aes_ecb_encrypt(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    use aes::cipher::{BlockEncrypt, KeyInit};

    if plaintext.len() != 16 {
        return Err(EncryptionError::CipherFailed(
            "ECB plaintext must be 16 bytes".into(),
        ));
    }

    let cipher = aes::Aes256::new_from_slice(key)
        .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;

    let mut block: aes::cipher::generic_array::GenericArray<u8, _> =
        aes::cipher::generic_array::GenericArray::clone_from_slice(plaintext);
    cipher.encrypt_block(&mut block);

    Ok(block.to_vec())
}

/// AES-256-CBC decryption.
#[allow(dead_code)]
fn aes_cbc_decrypt(key: &[u8], iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    use cbc::Decryptor;

    type Aes256CbcDec = Decryptor<aes::Aes256>;

    let decryptor = Aes256CbcDec::new_from_slices(key, iv)
        .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;

    // Clone ciphertext to mutable buffer
    let mut buf = ciphertext.to_vec();

    let plaintext = decryptor
        .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf)
        .map_err(|e| EncryptionError::CipherFailed(format!("Decryption failed: {:?}", e)))?;

    Ok(plaintext.to_vec())
}

/// Verifies a user password against the stored U value.
/// Returns the file encryption key if successful.
///
/// This implements Algorithm 11 from ISO 32000-2.
pub fn verify_user_password(
    password: &str,
    u_value: &[u8],
    ue_value: &[u8],
) -> Result<Vec<u8>, EncryptionError> {
    if u_value.len() != 48 {
        return Err(EncryptionError::CipherFailed("Invalid U value length".into()));
    }
    if ue_value.len() != 32 {
        return Err(EncryptionError::CipherFailed("Invalid UE value length".into()));
    }

    let password_bytes = truncate_password(password);

    // Extract validation salt from U[32:40]
    let validation_salt = &u_value[32..40];

    // Compute hash using Algorithm 2.A with password and validation salt
    let hash = compute_hash_2a(&password_bytes, validation_salt, None)?;

    // Compare with stored hash U[0:32]
    if hash.as_slice() != &u_value[0..32] {
        return Err(EncryptionError::CipherFailed("Password verification failed".into()));
    }

    // Password is correct, now decrypt the file key from UE
    // Key salt is at U[40:48]
    let key_salt = &u_value[40..48];

    // Compute decryption key using Algorithm 2.A with password and key salt
    let decryption_key = compute_hash_2a(&password_bytes, key_salt, None)?;

    // Decrypt UE to get file key using AES-256-CBC with zero IV
    let file_key = aes_cbc_decrypt_no_padding(&decryption_key, &[0u8; 16], ue_value)?;

    Ok(file_key)
}

/// AES-256-CBC decryption without padding.
fn aes_cbc_decrypt_no_padding(key: &[u8], iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    use cbc::Decryptor;

    type Aes256CbcDec = Decryptor<aes::Aes256>;

    if ciphertext.len() % 16 != 0 {
        return Err(EncryptionError::CipherFailed(
            "Ciphertext must be block-aligned".into(),
        ));
    }

    let decryptor = Aes256CbcDec::new_from_slices(key, iv)
        .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;

    let mut buf = ciphertext.to_vec();

    let plaintext = decryptor
        .decrypt_padded_mut::<aes::cipher::block_padding::NoPadding>(&mut buf)
        .map_err(|e| EncryptionError::CipherFailed(format!("Decryption failed: {:?}", e)))?;

    Ok(plaintext.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_password() {
        assert_eq!(truncate_password("hello").len(), 5);
        assert_eq!(truncate_password("").len(), 0);

        // Test with long password
        let long_pwd = "a".repeat(200);
        assert!(truncate_password(&long_pwd).len() <= 127);
    }

    #[test]
    fn test_derive_keys() {
        let keys = derive_aes256_keys("user123", "owner456", -4).unwrap();

        // Check key lengths
        assert_eq!(keys.file_encryption_key.len(), 32);
        assert_eq!(keys.o_value.len(), 48);
        assert_eq!(keys.u_value.len(), 48);
        assert_eq!(keys.oe_value.len(), 32);
        assert_eq!(keys.ue_value.len(), 32);
        assert_eq!(keys.perms_value.len(), 16);
    }

    #[test]
    fn test_password_verification_roundtrip() {
        let password = "user123";
        let keys = derive_aes256_keys(password, "owner456", -4).unwrap();

        // Verify the password can be validated and file key recovered
        let recovered_key = verify_user_password(password, &keys.u_value, &keys.ue_value).unwrap();

        assert_eq!(recovered_key, keys.file_encryption_key);
    }

    #[test]
    fn test_wrong_password_fails() {
        let keys = derive_aes256_keys("correct", "owner456", -4).unwrap();

        let result = verify_user_password("wrong", &keys.u_value, &keys.ue_value);
        assert!(result.is_err());
    }

    #[test]
    fn test_perms_verification() {
        let keys = derive_aes256_keys("user123", "owner456", -4).unwrap();

        // Decrypt Perms with file key using AES-256-ECB
        let perms_plain = aes_ecb_decrypt(&keys.file_encryption_key, &keys.perms_value).unwrap();

        // Check "adb" marker at bytes 9-11
        assert_eq!(&perms_plain[9..12], b"adb", "Perms validation marker not found");
    }

    fn aes_ecb_decrypt(key: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        use aes::cipher::{BlockDecrypt, KeyInit};

        if ciphertext.len() != 16 {
            return Err(EncryptionError::CipherFailed("ECB ciphertext must be 16 bytes".into()));
        }

        let cipher = aes::Aes256::new_from_slice(key)
            .map_err(|e| EncryptionError::CipherFailed(e.to_string()))?;

        let mut block: aes::cipher::generic_array::GenericArray<u8, _> =
            aes::cipher::generic_array::GenericArray::clone_from_slice(ciphertext);
        cipher.decrypt_block(&mut block);

        Ok(block.to_vec())
    }

    #[test]
    fn test_aes_ecb_encrypt() {
        let key = [0u8; 32];
        let plaintext = [0u8; 16];
        let ciphertext = aes_ecb_encrypt(&key, &plaintext).unwrap();
        assert_eq!(ciphertext.len(), 16);
    }

    #[test]
    fn test_aes_cbc_roundtrip() {
        let key = [0x42u8; 32];
        let iv = [0u8; 16];
        let plaintext = b"Hello, World!!!!"; // 16 bytes

        let ciphertext = aes_cbc_encrypt(&key, &iv, plaintext).unwrap();
        let decrypted = aes_cbc_decrypt(&key, &iv, &ciphertext).unwrap();

        // Compare original with decrypted (accounting for padding)
        assert_eq!(&decrypted[..16], plaintext);
    }

    #[test]
    fn test_verify_against_qpdf_encrypted_pdf() {
        // Fresh values extracted from a qpdf-encrypted PDF with user password "user123"
        // Generated with: qpdf --encrypt user123 owner456 256 -- input.pdf output.pdf
        // U[0:32] = hash, U[32:40] = validation salt, U[40:48] = key salt
        let u_hex = "66c168209263b84540053badf8b672df19983066283b649d1d27e3dcedad9b6f8704cf156bbd53a7950139fa92839ed4";
        let ue_hex = "397ae8c0166e42dec894b9d2b5c5f5823dfc3668e5bd88f97cc6d3df88ad38d5";

        let u_value: Vec<u8> = (0..u_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&u_hex[i..i + 2], 16).unwrap())
            .collect();

        let ue_value: Vec<u8> = (0..ue_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&ue_hex[i..i + 2], 16).unwrap())
            .collect();

        assert_eq!(u_value.len(), 48, "U value should be 48 bytes");
        assert_eq!(ue_value.len(), 32, "UE value should be 32 bytes");

        // Debug: print the expected hash and salt
        let expected_hash = &u_value[0..32];
        let validation_salt = &u_value[32..40];
        println!("Expected hash (U[0:32]): {:02x?}", expected_hash);
        println!("Validation salt (U[32:40]): {:02x?}", validation_salt);

        // Compute what our Algorithm 2.A produces
        let password_bytes = b"user123";
        let computed = compute_hash_2a(password_bytes, validation_salt, None).unwrap();
        println!("Computed hash: {:02x?}", computed);

        // Compare
        println!(
            "Hashes match: {}",
            computed.as_slice() == expected_hash
        );

        // This test verifies that our Algorithm 2.A implementation can validate
        // a password against qpdf-generated encryption values
        let result = verify_user_password("user123", &u_value, &ue_value);

        // If this passes, our Algorithm 2.A matches qpdf's implementation
        assert!(
            result.is_ok(),
            "Password verification should succeed against qpdf-generated values. Error: {:?}",
            result.err()
        );
    }
}
