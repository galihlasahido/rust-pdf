//! Signature configuration.

use std::sync::Arc;

use super::{SignatureAlgorithm, TimestampAuthorityClient};

/// How closely a signature conforms to the PAdES (PDF Advanced Electronic
/// Signatures, ETSI EN 319 142-1) baseline profiles.
///
/// Each level is a strict superset of the previous one's on-disk
/// requirements:
/// - [`PadesLevel::None`]: a plain PKCS#7 detached signature
///   (`SubFilter /adbe.pkcs7.detached`), the historical default.
/// - [`PadesLevel::B`]: CAdES-BES / PAdES "B-B" baseline
///   (`SubFilter /ETSI.CAdES.detached` plus the `signing-certificate-v2`
///   signed attribute, RFC 5035 §3).
/// - [`PadesLevel::T`]: "B-B" plus an RFC 3161 timestamp token over the
///   signature value ("B-T" -- trusted time). Requires
///   [`SignatureConfig::timestamp_authority`] to be set; the signer returns
///   an error at sign time otherwise.
///
/// PAdES "B-LT" (long-term validation) is not part of this enum: it is
/// applied *after* signing via [`super::embed_document_security_store`],
/// since it operates on an already-signed PDF rather than changing how the
/// signature itself is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadesLevel {
    /// Plain PKCS#7 detached signature; not a PAdES signature at all.
    #[default]
    None,
    /// PAdES "B-B" baseline.
    B,
    /// PAdES "B-T" (baseline + RFC 3161 timestamp).
    T,
}

/// A visible signature appearance: where on the (first) page to draw the
/// signature widget, in default user space units (points, ISO 32000-1
/// §7.9.5 / §8.3), with the origin at the page's lower-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleSignature {
    /// Left edge x-coordinate.
    pub x: f32,
    /// Bottom edge y-coordinate.
    pub y: f32,
    /// Width of the signature widget.
    pub width: f32,
    /// Height of the signature widget.
    pub height: f32,
}

impl VisibleSignature {
    /// Creates a new visible-signature placement.
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }
}

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
    /// PAdES conformance level to target (see [`PadesLevel`]).
    pub pades_level: PadesLevel,
    /// RFC 3161 Time-Stamp Authority client. Required when `pades_level` is
    /// [`PadesLevel::T`]; optional (but still honored) otherwise -- setting
    /// it without `PadesLevel::T` still embeds a timestamp token, just
    /// without the CAdES-BES signing-certificate binding.
    pub timestamp_authority: Option<Arc<dyn TimestampAuthorityClient>>,
    /// Where to draw a visible signature widget on the first page. `None`
    /// (the default) keeps the historical invisible (zero-size) widget.
    pub visible: Option<VisibleSignature>,
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
            // 16KB comfortably fits a bare single-certificate signature
            // (a few KB) as well as PAdES "B-T" (adds an embedded RFC 3161
            // token, which itself embeds the TSA's certificate) or a short
            // certificate chain; callers with longer chains or several
            // intermediate CAs should raise this via `signature_size(..)`.
            signature_size: 16384,
            pades_level: PadesLevel::default(),
            timestamp_authority: None,
            visible: None,
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

    /// Sets the target PAdES conformance level.
    pub fn pades_level(mut self, level: PadesLevel) -> Self {
        self.pades_level = level;
        self
    }

    /// Sets the RFC 3161 Time-Stamp Authority client used to obtain a
    /// timestamp token during signing.
    pub fn timestamp_authority(mut self, client: Arc<dyn TimestampAuthorityClient>) -> Self {
        self.timestamp_authority = Some(client);
        self
    }

    /// Sets where to draw a visible signature widget on the first page.
    pub fn visible(mut self, rect: VisibleSignature) -> Self {
        self.visible = Some(rect);
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
        assert_eq!(config.signature_size, 16384);
        assert!(config.embed_certificate_chain);
        assert_eq!(config.pades_level, PadesLevel::None);
        assert!(config.timestamp_authority.is_none());
        assert!(config.visible.is_none());
    }

    #[test]
    fn test_pades_level_default_is_none() {
        assert_eq!(PadesLevel::default(), PadesLevel::None);
    }

    #[test]
    fn test_signature_config_pades_and_visible_builders() {
        let rect = VisibleSignature::new(10.0, 20.0, 200.0, 60.0);
        let config = SignatureConfig::new().pades_level(PadesLevel::B).visible(rect);

        assert_eq!(config.pades_level, PadesLevel::B);
        assert_eq!(config.visible, Some(rect));
    }

    #[derive(Debug)]
    struct StubTimestampClient;
    impl super::super::TimestampAuthorityClient for StubTimestampClient {
        fn timestamp(&self, _tsq_der: &[u8]) -> super::super::SignatureResult<Vec<u8>> {
            Err(crate::error::SignatureError::TimestampError("stub".to_string()))
        }
    }

    #[test]
    fn test_signature_config_timestamp_authority_builder() {
        let config = SignatureConfig::new().timestamp_authority(Arc::new(StubTimestampClient));
        assert!(config.timestamp_authority.is_some());
    }
}
