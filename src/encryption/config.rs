//! Encryption configuration.

use super::permissions::Permissions;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Encryption algorithm to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionAlgorithm {
    /// AES-128 encryption (PDF 1.5+, V=4, R=4).
    Aes128,
    /// AES-256 encryption (PDF 2.0, V=5, R=6).
    Aes256,
}

impl EncryptionAlgorithm {
    /// Returns the key length in bytes.
    pub fn key_length(&self) -> usize {
        match self {
            EncryptionAlgorithm::Aes128 => 16,
            EncryptionAlgorithm::Aes256 => 32,
        }
    }

    /// Returns the encryption dictionary V value.
    pub fn v_value(&self) -> i32 {
        match self {
            EncryptionAlgorithm::Aes128 => 4,
            EncryptionAlgorithm::Aes256 => 5,
        }
    }

    /// Returns the encryption dictionary R value.
    pub fn r_value(&self) -> i32 {
        match self {
            EncryptionAlgorithm::Aes128 => 4,
            EncryptionAlgorithm::Aes256 => 6,
        }
    }
}

impl Default for EncryptionAlgorithm {
    fn default() -> Self {
        EncryptionAlgorithm::Aes256
    }
}

/// Configuration for PDF encryption.
#[derive(Debug, Clone, Zeroize, ZeroizeOnDrop)]
pub struct EncryptionConfig {
    /// The encryption algorithm.
    #[zeroize(skip)]
    pub algorithm: EncryptionAlgorithm,
    /// User password (for opening the document).
    pub user_password: String,
    /// Owner password (for full access).
    pub owner_password: String,
    /// Document permissions.
    pub permissions: Permissions,
    /// Whether to encrypt metadata.
    #[zeroize(skip)]
    pub encrypt_metadata: bool,
}

impl EncryptionConfig {
    /// Creates a new encryption config with AES-256 encryption.
    pub fn aes256() -> Self {
        Self {
            algorithm: EncryptionAlgorithm::Aes256,
            user_password: String::new(),
            owner_password: String::new(),
            permissions: Permissions::new(),
            encrypt_metadata: true,
        }
    }

    /// Creates a new encryption config with AES-128 encryption.
    pub fn aes128() -> Self {
        Self {
            algorithm: EncryptionAlgorithm::Aes128,
            user_password: String::new(),
            owner_password: String::new(),
            permissions: Permissions::new(),
            encrypt_metadata: true,
        }
    }

    /// Sets the user password (for opening the document).
    ///
    /// An empty password means anyone can open the document,
    /// but permissions still apply.
    pub fn user_password(mut self, password: impl Into<String>) -> Self {
        self.user_password = password.into();
        self
    }

    /// Sets the owner password (for full access).
    ///
    /// The owner password allows bypassing document permissions.
    /// If not set, a random password will be generated.
    pub fn owner_password(mut self, password: impl Into<String>) -> Self {
        self.owner_password = password.into();
        self
    }

    /// Sets the document permissions.
    pub fn permissions(mut self, permissions: Permissions) -> Self {
        self.permissions = permissions;
        self
    }

    /// Sets whether to encrypt document metadata.
    ///
    /// If false, metadata like title and author remain readable
    /// without the password.
    pub fn encrypt_metadata(mut self, encrypt: bool) -> Self {
        self.encrypt_metadata = encrypt;
        self
    }

    /// Returns true if the config has a user password set.
    pub fn has_user_password(&self) -> bool {
        !self.user_password.is_empty()
    }

    /// Returns true if the config has an owner password set.
    pub fn has_owner_password(&self) -> bool {
        !self.owner_password.is_empty()
    }
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self::aes256()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aes256_config() {
        let config = EncryptionConfig::aes256();
        assert_eq!(config.algorithm, EncryptionAlgorithm::Aes256);
        assert_eq!(config.algorithm.key_length(), 32);
        assert_eq!(config.algorithm.v_value(), 5);
        assert_eq!(config.algorithm.r_value(), 6);
    }

    #[test]
    fn test_aes128_config() {
        let config = EncryptionConfig::aes128();
        assert_eq!(config.algorithm, EncryptionAlgorithm::Aes128);
        assert_eq!(config.algorithm.key_length(), 16);
        assert_eq!(config.algorithm.v_value(), 4);
        assert_eq!(config.algorithm.r_value(), 4);
    }

    #[test]
    fn test_config_builder() {
        let config = EncryptionConfig::aes256()
            .user_password("user123")
            .owner_password("owner456")
            .permissions(Permissions::new().allow_printing(true))
            .encrypt_metadata(false);

        assert!(config.has_user_password());
        assert!(config.has_owner_password());
        assert!(config.permissions.can_print());
        assert!(!config.encrypt_metadata);
    }

    #[test]
    fn test_empty_passwords() {
        let config = EncryptionConfig::aes256();
        assert!(!config.has_user_password());
        assert!(!config.has_owner_password());
    }
}
