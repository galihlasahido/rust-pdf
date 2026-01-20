//! PDF document signing functionality.

use crate::document::Document;
use crate::error::SignatureError;
use crate::object::{Object, PdfDictionary, PdfName, PdfString};
use super::{Certificate, Pkcs7Builder, PrivateKey, SignatureAlgorithm, SignatureConfig, SignatureResult};

/// Signs PDF documents with X.509 certificates.
#[derive(Debug)]
pub struct DocumentSigner {
    /// The document to sign.
    document: Document,
    /// The signer's certificate.
    certificate: Option<Certificate>,
    /// Additional certificates in the chain.
    certificate_chain: Vec<Certificate>,
    /// The private key for signing.
    private_key: Option<PrivateKey>,
    /// Signature configuration.
    config: SignatureConfig,
}

impl DocumentSigner {
    /// Creates a new document signer for the given document.
    pub fn new(document: Document) -> Self {
        Self {
            document,
            certificate: None,
            certificate_chain: Vec::new(),
            private_key: None,
            config: SignatureConfig::default(),
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

    /// Sets the private key.
    pub fn private_key(mut self, key: PrivateKey) -> Self {
        self.private_key = Some(key);
        self
    }

    /// Sets the signer's name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.config = self.config.name(name);
        self
    }

    /// Sets the reason for signing.
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.config = self.config.reason(reason);
        self
    }

    /// Sets the location of signing.
    pub fn location(mut self, location: impl Into<String>) -> Self {
        self.config = self.config.location(location);
        self
    }

    /// Sets contact information.
    pub fn contact_info(mut self, info: impl Into<String>) -> Self {
        self.config = self.config.contact_info(info);
        self
    }

    /// Sets the signature algorithm.
    pub fn algorithm(mut self, algo: SignatureAlgorithm) -> Self {
        self.config = self.config.algorithm(algo);
        self
    }

    /// Sets the signature configuration.
    pub fn config(mut self, config: SignatureConfig) -> Self {
        self.config = config;
        self
    }

    /// Signs the document and returns the signed PDF bytes.
    pub fn sign(self) -> SignatureResult<Vec<u8>> {
        let cert = self.certificate.as_ref().ok_or_else(|| {
            SignatureError::SigningFailed("Certificate not set".to_string())
        })?;

        let key = self.private_key.as_ref().ok_or_else(|| {
            SignatureError::SigningFailed("Private key not set".to_string())
        })?;

        // First, generate the PDF with a placeholder signature
        let (pdf_with_placeholder, byte_range, sig_offset, sig_length) =
            self.create_pdf_with_placeholder()?;

        // Calculate the data to sign (everything except the signature placeholder)
        let data_to_sign = self.extract_signed_data(&pdf_with_placeholder, &byte_range)?;

        // Create the PKCS#7 signature
        let mut pkcs7_builder = Pkcs7Builder::new()
            .certificate(cert.clone())
            .algorithm(self.config.algorithm);

        for chain_cert in &self.certificate_chain {
            pkcs7_builder = pkcs7_builder.add_chain_certificate(chain_cert.clone());
        }

        let pkcs7_signature = pkcs7_builder.build(&data_to_sign, key)?;

        // Embed the signature into the PDF
        let signed_pdf = self.embed_signature(
            pdf_with_placeholder,
            sig_offset,
            sig_length,
            &pkcs7_signature,
        )?;

        Ok(signed_pdf)
    }

    /// Creates a PDF with a placeholder for the signature.
    fn create_pdf_with_placeholder(&self) -> SignatureResult<(Vec<u8>, ByteRange, usize, usize)> {
        // For now, we'll create a simplified signed PDF structure
        // In a full implementation, we'd need to:
        // 1. Serialize the document
        // 2. Add an AcroForm with a signature field
        // 3. Add the signature dictionary with placeholder

        let pdf_bytes = self.document.save_to_bytes().map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to serialize document: {}", e))
        })?;

        // Find the end of the PDF (before %%EOF)
        let eof_pos = find_eof_position(&pdf_bytes)?;

        // Create signature dictionary
        let _sig_dict = self.build_signature_dictionary()?;

        // Calculate positions
        let sig_hex_length = self.config.signature_size * 2; // Hex encoding doubles the size
        let sig_contents_start = eof_pos + self.calculate_sig_dict_prefix_length();
        let sig_contents_end = sig_contents_start + sig_hex_length;

        // Build byte range
        let byte_range = ByteRange {
            offset1: 0,
            length1: sig_contents_start as i64,
            offset2: sig_contents_end as i64,
            length2: 0, // Will be calculated after we know total length
        };

        // For simplicity, return the basic structure
        // A full implementation would properly integrate with the PDF structure
        let mut output = pdf_bytes[..eof_pos].to_vec();

        // Add signature object (simplified)
        let _sig_obj_start = output.len();
        let sig_obj_num = 1000; // Use a high object number
        output.extend_from_slice(format!("\n{} 0 obj\n", sig_obj_num).as_bytes());

        let sig_dict_str = self.build_signature_dictionary_string(
            sig_contents_start,
            sig_hex_length,
            &byte_range,
        );
        output.extend_from_slice(sig_dict_str.as_bytes());

        output.extend_from_slice(b"\nendobj\n");

        // Add xref and trailer updates
        output.extend_from_slice(b"%%EOF\n");

        // Update byte range with actual length
        let final_byte_range = ByteRange {
            offset1: 0,
            length1: sig_contents_start as i64,
            offset2: (sig_contents_start + sig_hex_length + 2) as i64, // +2 for angle brackets
            length2: (output.len() - sig_contents_start - sig_hex_length - 2) as i64,
        };

        Ok((output, final_byte_range, sig_contents_start, sig_hex_length))
    }

    /// Calculates the prefix length of the signature dictionary.
    fn calculate_sig_dict_prefix_length(&self) -> usize {
        // Approximate length of signature dictionary before /Contents value
        200
    }

    /// Builds the signature dictionary string.
    fn build_signature_dictionary_string(
        &self,
        _contents_offset: usize,
        contents_length: usize,
        byte_range: &ByteRange,
    ) -> String {
        let mut dict = String::new();
        dict.push_str("<< /Type /Sig");
        dict.push_str(" /Filter /Adobe.PPKLite");
        dict.push_str(" /SubFilter /adbe.pkcs7.detached");

        // ByteRange array
        dict.push_str(&format!(
            " /ByteRange [{} {} {} {}]",
            byte_range.offset1,
            byte_range.length1,
            byte_range.offset2,
            byte_range.length2
        ));

        // Contents placeholder (will be filled with signature)
        dict.push_str(" /Contents <");
        dict.push_str(&"0".repeat(contents_length));
        dict.push('>');

        // Optional fields
        if let Some(ref name) = self.config.name {
            dict.push_str(&format!(" /Name ({})", escape_pdf_string(name)));
        }
        if let Some(ref reason) = self.config.reason {
            dict.push_str(&format!(" /Reason ({})", escape_pdf_string(reason)));
        }
        if let Some(ref location) = self.config.location {
            dict.push_str(&format!(" /Location ({})", escape_pdf_string(location)));
        }
        if let Some(ref contact) = self.config.contact_info {
            dict.push_str(&format!(" /ContactInfo ({})", escape_pdf_string(contact)));
        }

        // Signing time
        let timestamp = format_pdf_timestamp();
        dict.push_str(&format!(" /M ({})", timestamp));

        dict.push_str(" >>");
        dict
    }

    /// Builds the signature dictionary.
    fn build_signature_dictionary(&self) -> SignatureResult<PdfDictionary> {
        let mut dict = PdfDictionary::new();

        dict.set("Type", Object::Name(PdfName::new_unchecked("Sig")));
        dict.set("Filter", Object::Name(PdfName::new_unchecked("Adobe.PPKLite")));
        dict.set("SubFilter", Object::Name(PdfName::new_unchecked("adbe.pkcs7.detached")));

        if let Some(ref name) = self.config.name {
            dict.set("Name", Object::String(PdfString::literal(name)));
        }
        if let Some(ref reason) = self.config.reason {
            dict.set("Reason", Object::String(PdfString::literal(reason)));
        }
        if let Some(ref location) = self.config.location {
            dict.set("Location", Object::String(PdfString::literal(location)));
        }
        if let Some(ref contact) = self.config.contact_info {
            dict.set("ContactInfo", Object::String(PdfString::literal(contact)));
        }

        Ok(dict)
    }

    /// Extracts the data to be signed based on the byte range.
    fn extract_signed_data(&self, pdf_bytes: &[u8], byte_range: &ByteRange) -> SignatureResult<Vec<u8>> {
        let mut data = Vec::new();

        // First range
        if byte_range.offset1 >= 0 && byte_range.length1 > 0 {
            let start = byte_range.offset1 as usize;
            let end = start + byte_range.length1 as usize;
            if end <= pdf_bytes.len() {
                data.extend_from_slice(&pdf_bytes[start..end]);
            }
        }

        // Second range
        if byte_range.offset2 >= 0 && byte_range.length2 > 0 {
            let start = byte_range.offset2 as usize;
            let end = start + byte_range.length2 as usize;
            if end <= pdf_bytes.len() {
                data.extend_from_slice(&pdf_bytes[start..end]);
            }
        }

        Ok(data)
    }

    /// Embeds the signature into the PDF.
    fn embed_signature(
        &self,
        mut pdf_bytes: Vec<u8>,
        sig_offset: usize,
        sig_length: usize,
        signature: &[u8],
    ) -> SignatureResult<Vec<u8>> {
        // Convert signature to hex
        let sig_hex: String = signature.iter().map(|b| format!("{:02X}", b)).collect();

        // Pad with zeros if needed
        let padded_hex = if sig_hex.len() < sig_length {
            let padding = "0".repeat(sig_length - sig_hex.len());
            sig_hex + &padding
        } else if sig_hex.len() > sig_length {
            return Err(SignatureError::SigningFailed(
                "Signature too large for reserved space".to_string(),
            ));
        } else {
            sig_hex
        };

        // Replace the placeholder
        let hex_bytes = padded_hex.as_bytes();
        if sig_offset + hex_bytes.len() <= pdf_bytes.len() {
            pdf_bytes[sig_offset..sig_offset + hex_bytes.len()].copy_from_slice(hex_bytes);
        }

        Ok(pdf_bytes)
    }
}

/// Represents a PDF signature byte range.
#[derive(Debug, Clone, Copy)]
pub struct ByteRange {
    /// Start of first range (always 0).
    pub offset1: i64,
    /// Length of first range.
    pub length1: i64,
    /// Start of second range.
    pub offset2: i64,
    /// Length of second range.
    pub length2: i64,
}

impl ByteRange {
    /// Creates a new byte range.
    pub fn new(offset1: i64, length1: i64, offset2: i64, length2: i64) -> Self {
        Self {
            offset1,
            length1,
            offset2,
            length2,
        }
    }
}

/// Information about a signature in a PDF.
#[derive(Debug, Clone)]
pub struct SignatureInfo {
    /// The signer's name.
    pub name: Option<String>,
    /// Reason for signing.
    pub reason: Option<String>,
    /// Location of signing.
    pub location: Option<String>,
    /// Contact information.
    pub contact_info: Option<String>,
    /// Signing time.
    pub signing_time: Option<String>,
    /// The byte range covered by the signature.
    pub byte_range: ByteRange,
    /// Whether the signature is valid.
    pub is_valid: Option<bool>,
}

impl SignatureInfo {
    /// Creates a new signature info with default values.
    pub fn new() -> Self {
        Self {
            name: None,
            reason: None,
            location: None,
            contact_info: None,
            signing_time: None,
            byte_range: ByteRange::new(0, 0, 0, 0),
            is_valid: None,
        }
    }
}

impl Default for SignatureInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Finds the position of %%EOF in the PDF.
fn find_eof_position(pdf_bytes: &[u8]) -> SignatureResult<usize> {
    let eof_marker = b"%%EOF";
    let content = pdf_bytes;

    // Search backwards from the end
    for i in (0..content.len().saturating_sub(eof_marker.len())).rev() {
        if &content[i..i + eof_marker.len()] == eof_marker {
            return Ok(i);
        }
    }

    Err(SignatureError::ByteRangeError(
        "Could not find %%EOF marker".to_string(),
    ))
}

/// Escapes a string for PDF.
fn escape_pdf_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Formats the current time as a PDF timestamp.
fn format_pdf_timestamp() -> String {
    // D:YYYYMMDDHHmmSSOHH'mm'
    // For now, use a fixed timestamp format
    // In production, you'd use actual current time
    "D:20250120120000+00'00'".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_range() {
        let br = ByteRange::new(0, 100, 200, 300);
        assert_eq!(br.offset1, 0);
        assert_eq!(br.length1, 100);
        assert_eq!(br.offset2, 200);
        assert_eq!(br.length2, 300);
    }

    #[test]
    fn test_signature_info_default() {
        let info = SignatureInfo::default();
        assert!(info.name.is_none());
        assert!(info.reason.is_none());
        assert!(info.is_valid.is_none());
    }

    #[test]
    fn test_escape_pdf_string() {
        assert_eq!(escape_pdf_string("Hello"), "Hello");
        assert_eq!(escape_pdf_string("Hello (World)"), "Hello \\(World\\)");
        assert_eq!(escape_pdf_string("Back\\slash"), "Back\\\\slash");
    }

    #[test]
    fn test_find_eof_position() {
        let pdf = b"Some content\n%%EOF\n";
        let pos = find_eof_position(pdf).unwrap();
        assert_eq!(pos, 13);
    }
}
