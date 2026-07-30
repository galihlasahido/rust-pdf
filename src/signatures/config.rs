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

/// A PDF *certification signature*'s declared modification-permission level
/// (ISO 32000-1 12.8.2.2 / 12.7.4.5 "DocMDP"). Only the **first** signature
/// applied to a document may be a certification signature (see
/// [`SignatureConfig::certify`]); any signature after it is necessarily an
/// *approval* signature, which carries no DocMDP semantics of its own.
///
/// Each variant maps to a `/DocMDP` transform-params `/P` integer (ISO
/// 32000-1 Table 254):
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificationLevel {
    /// `P = 1`: no further changes to the document are permitted at all.
    /// Any subsequent modification -- including additional signatures --
    /// invalidates the certification.
    NoChanges,
    /// `P = 2`: filling in existing form fields (and signing existing
    /// signature fields, which is itself a form fill) is permitted; nothing
    /// else.
    FormFillingOnly,
    /// `P = 3`: filling in form fields, adding or modifying annotations, and
    /// applying further (approval) signatures are all permitted.
    FormFillingAnnotationsAndSigning,
}

impl CertificationLevel {
    /// The `/P` integer this level writes into the DocMDP `/TransformParams`
    /// dictionary (ISO 32000-1 12.8.2.2 Table 254).
    pub fn p_value(self) -> i64 {
        match self {
            CertificationLevel::NoChanges => 1,
            CertificationLevel::FormFillingOnly => 2,
            CertificationLevel::FormFillingAnnotationsAndSigning => 3,
        }
    }

    /// The reverse of [`CertificationLevel::p_value`]: resolves a `/P`
    /// integer read back out of a signed PDF's `/TransformParams`
    /// dictionary into a level, for [`super::verifier::SignatureVerifier`].
    /// `None` for any value outside `1..=3` (ISO 32000-1 doesn't define
    /// other values; a reader encountering one should treat the DocMDP
    /// permission as unrecognized rather than guess).
    pub fn from_p_value(p: i64) -> Option<Self> {
        match p {
            1 => Some(CertificationLevel::NoChanges),
            2 => Some(CertificationLevel::FormFillingOnly),
            3 => Some(CertificationLevel::FormFillingAnnotationsAndSigning),
            _ => None,
        }
    }
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
        Self {
            x,
            y,
            width,
            height,
        }
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
    /// Makes this a *certification signature* (ISO 32000-1 12.8.2.2 /
    /// 12.7.4.5 "DocMDP") at the given permission level, instead of a plain
    /// approval signature. `None` (the default) produces the historical
    /// approval-only signature with no DocMDP semantics.
    ///
    /// Only valid for the **first** signature applied to a document --
    /// [`super::DocumentSigner::sign`] / [`super::IncrementalSigner::sign`]
    /// reject this with a [`crate::error::SignatureError`] if the document
    /// already carries one or more signatures, rather than silently
    /// producing an on-disk certification that violates the one-per-document
    /// rule ISO 32000-1 12.8.2.2 states for `/Perms /DocMDP`.
    pub certification: Option<CertificationLevel>,
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
            certification: None,
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

    /// Makes the signature produced from this config a *certification
    /// signature* at `level` (see [`SignatureConfig::certification`]).
    /// Only meaningful for the first signature on a document -- the signer
    /// rejects signing with this set if the document already has one.
    pub fn certify(mut self, level: CertificationLevel) -> Self {
        self.certification = Some(level);
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
        assert!(config.certification.is_none());
    }

    #[test]
    fn test_pades_level_default_is_none() {
        assert_eq!(PadesLevel::default(), PadesLevel::None);
    }

    #[test]
    fn test_certification_level_p_values() {
        assert_eq!(CertificationLevel::NoChanges.p_value(), 1);
        assert_eq!(CertificationLevel::FormFillingOnly.p_value(), 2);
        assert_eq!(
            CertificationLevel::FormFillingAnnotationsAndSigning.p_value(),
            3
        );
    }

    #[test]
    fn test_certification_level_from_p_value_roundtrip() {
        for level in [
            CertificationLevel::NoChanges,
            CertificationLevel::FormFillingOnly,
            CertificationLevel::FormFillingAnnotationsAndSigning,
        ] {
            assert_eq!(
                CertificationLevel::from_p_value(level.p_value()),
                Some(level)
            );
        }
        assert_eq!(CertificationLevel::from_p_value(0), None);
        assert_eq!(CertificationLevel::from_p_value(4), None);
    }

    #[test]
    fn test_signature_config_certify_builder() {
        let config = SignatureConfig::new().certify(CertificationLevel::NoChanges);
        assert_eq!(config.certification, Some(CertificationLevel::NoChanges));
    }

    #[test]
    fn test_signature_config_pades_and_visible_builders() {
        let rect = VisibleSignature::new(10.0, 20.0, 200.0, 60.0);
        let config = SignatureConfig::new()
            .pades_level(PadesLevel::B)
            .visible(rect);

        assert_eq!(config.pades_level, PadesLevel::B);
        assert_eq!(config.visible, Some(rect));
    }

    #[derive(Debug)]
    struct StubTimestampClient;
    impl super::super::TimestampAuthorityClient for StubTimestampClient {
        fn timestamp(&self, _tsq_der: &[u8]) -> super::super::SignatureResult<Vec<u8>> {
            Err(crate::error::SignatureError::TimestampError(
                "stub".to_string(),
            ))
        }
    }

    #[test]
    fn test_signature_config_timestamp_authority_builder() {
        let config = SignatureConfig::new().timestamp_authority(Arc::new(StubTimestampClient));
        assert!(config.timestamp_authority.is_some());
    }
}
