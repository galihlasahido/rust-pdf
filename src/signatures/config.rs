//! Signature configuration.

use super::SignatureAlgorithm;

/// Configuration for PDF digital signatures.
#[derive(Debug, Clone)]
pub struct SignatureConfig {
    /// The signer's name.
    pub name: Option<String>,
    /// Reason for signing.
    pub reason: Option<String>,
    /// Location of signing.
    pub location: Option<String>,
    /// Contact information.
    pub contact_info: Option<String>,
    /// The signature algorithm to use.
    pub algorithm: SignatureAlgorithm,
    /// Whether to embed the full certificate chain.
    pub embed_certificate_chain: bool,
    /// Reserved space for the signature (in bytes).
    /// Should be large enough to hold the PKCS#7 signature.
    pub signature_size: usize,
}

impl SignatureConfig {
    /// Creates a new signature configuration with default settings.
    pub fn new() -> Self {
        Self {
            name: None,
            reason: None,
            location: None,
            contact_info: None,
            algorithm: SignatureAlgorithm::default(),
            embed_certificate_chain: true,
            signature_size: 8192, // Default 8KB for signature
        }
    }

    /// Sets the signer's name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets the reason for signing.
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Sets the location of signing.
    pub fn location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Sets contact information.
    pub fn contact_info(mut self, info: impl Into<String>) -> Self {
        self.contact_info = Some(info.into());
        self
    }

    /// Sets the signature algorithm.
    pub fn algorithm(mut self, algo: SignatureAlgorithm) -> Self {
        self.algorithm = algo;
        self
    }

    /// Sets whether to embed the full certificate chain.
    pub fn embed_certificate_chain(mut self, embed: bool) -> Self {
        self.embed_certificate_chain = embed;
        self
    }

    /// Sets the reserved signature size in bytes.
    pub fn signature_size(mut self, size: usize) -> Self {
        self.signature_size = size;
        self
    }
}

impl Default for SignatureConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_config_builder() {
        let config = SignatureConfig::new()
            .name("John Doe")
            .reason("Document approval")
            .location("San Francisco, CA")
            .contact_info("john@example.com")
            .algorithm(SignatureAlgorithm::RsaSha256)
            .signature_size(16384);

        assert_eq!(config.name, Some("John Doe".to_string()));
        assert_eq!(config.reason, Some("Document approval".to_string()));
        assert_eq!(config.location, Some("San Francisco, CA".to_string()));
        assert_eq!(config.contact_info, Some("john@example.com".to_string()));
        assert_eq!(config.algorithm, SignatureAlgorithm::RsaSha256);
        assert_eq!(config.signature_size, 16384);
    }

    #[test]
    fn test_signature_config_default() {
        let config = SignatureConfig::default();
        assert!(config.name.is_none());
        assert!(config.reason.is_none());
        assert_eq!(config.signature_size, 8192);
        assert!(config.embed_certificate_chain);
    }
}
