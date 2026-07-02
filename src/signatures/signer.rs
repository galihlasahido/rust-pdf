//! PDF document signing functionality.
//!
//! This module implements PDF digital signatures compliant with PDF 1.7/2.0.
//! It creates proper PDF structures including AcroForm, signature fields,
//! and uses incremental updates for signature embedding.

use crate::document::Document;
use crate::error::SignatureError;
use super::{Certificate, Pkcs7Builder, PrivateKey, SignatureAlgorithm, SignatureConfig, SignatureResult};

/// Signature placeholder size in bytes (32KB should be enough for most signatures).
const SIGNATURE_SIZE: usize = 32768;

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
        let pdf_with_placeholder = self.create_pdf_with_placeholder()?;

        // Calculate the byte ranges for the signature
        let byte_range = self.calculate_byte_range(&pdf_with_placeholder)?;

        // Update the ByteRange placeholder with actual values
        let pdf_with_byte_range = self.update_byte_range_placeholder(pdf_with_placeholder, &byte_range)?;

        // Extract the data to sign (everything except the signature placeholder)
        let data_to_sign = self.extract_signed_data(&pdf_with_byte_range, &byte_range);

        // Create the PKCS#7 signature
        let mut pkcs7_builder = Pkcs7Builder::new()
            .certificate(cert.clone())
            .algorithm(self.config.algorithm);

        for chain_cert in &self.certificate_chain {
            pkcs7_builder = pkcs7_builder.add_chain_certificate(chain_cert.clone());
        }

        let pkcs7_signature = pkcs7_builder.build(&data_to_sign, key)?;

        // Embed the signature into the PDF
        let signed_pdf = self.embed_signature(pdf_with_byte_range, &byte_range, &pkcs7_signature)?;

        Ok(signed_pdf)
    }

    /// Creates a PDF with a placeholder for the signature.
    /// This uses incremental update to add signature structures.
    fn create_pdf_with_placeholder(&self) -> SignatureResult<Vec<u8>> {
        // Serialize the base document
        let pdf_bytes = self.document.save_to_bytes().map_err(|e| {
            SignatureError::SigningFailed(format!("Failed to serialize document: {}", e))
        })?;

        // Clone pdf_bytes since we need to reference it later for parsing objects
        let original_pdf = pdf_bytes.clone();

        // Parse the PDF to find key information
        let (prev_xref, root_obj_num, next_obj_id, page_obj_num) = self.parse_pdf_info(&original_pdf)?;

        // Ensure the PDF ends with newline
        let mut output = pdf_bytes;
        if !output.ends_with(b"\n") {
            output.push(b'\n');
        }

        // Assign object IDs for new objects
        let sig_dict_id = next_obj_id;
        let sig_field_id = next_obj_id + 1;
        let appearance_id = next_obj_id + 2;
        let acro_form_id = next_obj_id + 3;
        let updated_page_id = page_obj_num; // We'll update the existing page
        let updated_catalog_id = root_obj_num; // We'll update the existing catalog
        let final_next_id = next_obj_id + 4;

        // Track offsets for xref
        let mut object_offsets: Vec<(u32, usize)> = Vec::new();

        // 1. Build signature dictionary with placeholder
        let sig_dict_offset = output.len();
        let sig_dict = self.build_signature_dictionary(sig_dict_id);
        output.extend_from_slice(sig_dict.as_bytes());
        object_offsets.push((sig_dict_id, sig_dict_offset));

        // 2. Build signature field widget annotation
        let sig_field_offset = output.len();
        let sig_field = self.build_signature_field(sig_field_id, sig_dict_id, appearance_id, page_obj_num);
        output.extend_from_slice(sig_field.as_bytes());
        object_offsets.push((sig_field_id, sig_field_offset));

        // 3. Build appearance XObject (empty form)
        let appearance_offset = output.len();
        let appearance = self.build_appearance_object(appearance_id);
        output.extend_from_slice(appearance.as_bytes());
        object_offsets.push((appearance_id, appearance_offset));

        // 4. Build AcroForm object
        let acro_form_offset = output.len();
        let acro_form = self.build_acro_form(acro_form_id, sig_field_id);
        output.extend_from_slice(acro_form.as_bytes());
        object_offsets.push((acro_form_id, acro_form_offset));

        // 5. Build updated page with Annots
        let updated_page_offset = output.len();
        let updated_page = self.build_updated_page(&original_pdf, updated_page_id, sig_field_id)?;
        output.extend_from_slice(updated_page.as_bytes());
        object_offsets.push((updated_page_id, updated_page_offset));

        // 6. Build updated catalog with AcroForm reference
        let updated_catalog_offset = output.len();
        let updated_catalog = self.build_updated_catalog(&original_pdf, updated_catalog_id, acro_form_id)?;
        output.extend_from_slice(updated_catalog.as_bytes());
        object_offsets.push((updated_catalog_id, updated_catalog_offset));

        // 7. Build incremental xref table
        let xref_offset = output.len();
        let xref = self.build_incremental_xref(&object_offsets, prev_xref, root_obj_num, final_next_id);
        output.extend_from_slice(xref.as_bytes());

        // 8. Add startxref and %%EOF
        output.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_offset).as_bytes());

        Ok(output)
    }

    /// Parses the PDF to find key information.
    fn parse_pdf_info(&self, pdf_bytes: &[u8]) -> SignatureResult<(usize, u32, u32, u32)> {
        // Use lossy conversion since PDFs may contain binary streams
        let content = String::from_utf8_lossy(pdf_bytes);

        // Find startxref position
        let startxref_pos = content.rfind("startxref").ok_or_else(|| {
            SignatureError::SigningFailed("Could not find startxref".to_string())
        })?;

        // Extract the xref offset
        let after_startxref = &content[startxref_pos + 9..];
        let prev_xref: usize = after_startxref
            .trim()
            .lines()
            .next()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| {
                SignatureError::SigningFailed("Could not parse xref offset".to_string())
            })?;

        // Find /Root reference in trailer
        let root_obj_num = self.find_root_object(&content)?;

        // Find the highest object number
        let next_obj_id = self.find_next_object_id(&content);

        // Find the first page object number
        let page_obj_num = self.find_first_page_object(&content)?;

        Ok((prev_xref, root_obj_num, next_obj_id, page_obj_num))
    }

    /// Finds the Root object number from the trailer.
    fn find_root_object(&self, content: &str) -> SignatureResult<u32> {
        // Look for /Root N 0 R in trailer
        for line in content.lines().rev() {
            if let Some(pos) = line.find("/Root") {
                let after_root = &line[pos..];
                let parts: Vec<&str> = after_root.split_whitespace().collect();
                if parts.len() >= 3 {
                    if let Ok(num) = parts[1].parse::<u32>() {
                        return Ok(num);
                    }
                }
            }
        }

        // Try regex-like search
        let bytes = content.as_bytes();
        let mut i = bytes.len();
        while i > 10 {
            i -= 1;
            if bytes[i..].starts_with(b"/Root") {
                let rest = &content[i + 5..];
                let trimmed = rest.trim_start();
                if let Some(space_pos) = trimmed.find(|c: char| c.is_whitespace()) {
                    if let Ok(num) = trimmed[..space_pos].parse::<u32>() {
                        return Ok(num);
                    }
                }
            }
        }

        Err(SignatureError::SigningFailed("Could not find /Root reference".to_string()))
    }

    /// Finds the next available object ID.
    fn find_next_object_id(&self, content: &str) -> u32 {
        let mut max_id: u32 = 0;

        // Find all "N 0 obj" patterns
        let mut chars = content.chars().peekable();
        let mut num_str = String::new();

        while let Some(c) = chars.next() {
            if c.is_ascii_digit() {
                num_str.push(c);
            } else if c.is_whitespace() && !num_str.is_empty() {
                // Check if followed by "0 obj"
                let rest: String = chars.clone().take(5).collect();
                if rest.starts_with("0 obj") || rest.starts_with("0  obj") {
                    if let Ok(num) = num_str.parse::<u32>() {
                        if num > max_id {
                            max_id = num;
                        }
                    }
                }
                num_str.clear();
            } else {
                num_str.clear();
            }
        }

        // Also check /Size in trailer
        if let Some(size_pos) = content.rfind("/Size") {
            let after_size = &content[size_pos + 5..];
            let trimmed = after_size.trim_start();
            if let Some(end) = trimmed.find(|c: char| !c.is_ascii_digit()) {
                if let Ok(size) = trimmed[..end].parse::<u32>() {
                    if size > max_id {
                        max_id = size;
                    }
                }
            }
        }

        max_id + 1
    }

    /// Finds the first page object number.
    fn find_first_page_object(&self, content: &str) -> SignatureResult<u32> {
        // Look for /Type /Page (not /Pages)
        // First find /Pages reference
        if let Some(pages_pos) = content.find("/Pages") {
            let after_pages = &content[pages_pos + 6..];
            let trimmed = after_pages.trim_start();

            // Check if it's a reference (N 0 R)
            let parts: Vec<&str> = trimmed.split_whitespace().take(3).collect();
            if parts.len() >= 3 && parts[2] == "R" {
                if let Ok(pages_obj_num) = parts[0].parse::<u32>() {
                    // Find the Pages object and get Kids array
                    let pages_pattern = format!("{} 0 obj", pages_obj_num);
                    if let Some(pages_obj_pos) = content.find(&pages_pattern) {
                        let pages_obj = &content[pages_obj_pos..];
                        // Find /Kids array
                        if let Some(kids_pos) = pages_obj.find("/Kids") {
                            let after_kids = &pages_obj[kids_pos + 5..];
                            // Find [ and extract first reference
                            if let Some(bracket_pos) = after_kids.find('[') {
                                let after_bracket = &after_kids[bracket_pos + 1..];
                                let parts: Vec<&str> = after_bracket.split_whitespace().take(3).collect();
                                if parts.len() >= 2 {
                                    if let Ok(page_num) = parts[0].parse::<u32>() {
                                        return Ok(page_num);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Fallback: find first /Type /Page pattern
        let mut search_pos = 0;
        while let Some(type_pos) = content[search_pos..].find("/Type") {
            let abs_pos = search_pos + type_pos;
            let after_type = &content[abs_pos + 5..];
            let trimmed = after_type.trim_start();

            if trimmed.starts_with("/Page") && !trimmed.starts_with("/Pages") {
                // Found /Type /Page, now find the object number
                // Search backwards for "N 0 obj"
                let before = &content[..abs_pos];
                if let Some(obj_pos) = before.rfind(" 0 obj") {
                    let before_obj = &before[..obj_pos];
                    // Find the number before " 0 obj"
                    let mut num_end = before_obj.len();
                    while num_end > 0 && before_obj.chars().nth(num_end - 1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
                        num_end -= 1;
                    }
                    if let Ok(page_num) = before_obj[num_end..].parse::<u32>() {
                        return Ok(page_num);
                    }
                }
            }
            search_pos = abs_pos + 5;
        }

        Err(SignatureError::SigningFailed("Could not find page object".to_string()))
    }

    /// Builds the signature dictionary with placeholder.
    fn build_signature_dictionary(&self, obj_id: u32) -> String {
        let signer_name = self.config.name.as_deref().unwrap_or("Unknown");
        let timestamp = format_pdf_timestamp();

        let mut dict = format!("{} 0 obj\n<<\n", obj_id);
        dict.push_str("/Type /Sig\n");
        dict.push_str("/Filter /Adobe.PPKLite\n");
        dict.push_str("/SubFilter /adbe.pkcs7.detached\n");

        // ByteRange placeholder with fixed-width (10 digits each)
        dict.push_str("/ByteRange [0000000000 0000000000 0000000000 0000000000]\n");

        // Contents placeholder for signature (hex encoded, so double the size)
        dict.push_str("/Contents <");
        dict.push_str(&"0".repeat(SIGNATURE_SIZE * 2));
        dict.push_str(">\n");

        // Signer name
        dict.push_str(&format!("/Name ({})\n", escape_pdf_string(signer_name)));

        // Signing time
        dict.push_str(&format!("/M ({})\n", timestamp));

        // Optional fields
        if let Some(ref reason) = self.config.reason {
            dict.push_str(&format!("/Reason ({})\n", escape_pdf_string(reason)));
        }
        if let Some(ref location) = self.config.location {
            dict.push_str(&format!("/Location ({})\n", escape_pdf_string(location)));
        }
        if let Some(ref contact) = self.config.contact_info {
            dict.push_str(&format!("/ContactInfo ({})\n", escape_pdf_string(contact)));
        }

        dict.push_str(">>\nendobj\n");
        dict
    }

    /// Builds the signature field widget annotation.
    fn build_signature_field(&self, field_id: u32, sig_dict_id: u32, appearance_id: u32, page_id: u32) -> String {
        let field_name = "Signature1";

        let mut field = format!("{} 0 obj\n<<\n", field_id);
        field.push_str("/Type /Annot\n");
        field.push_str("/Subtype /Widget\n");
        field.push_str("/FT /Sig\n");
        field.push_str("/F 132\n"); // Flags: Print (4) + Locked (128)
        field.push_str("/Rect [0 0 0 0]\n"); // Invisible signature
        field.push_str(&format!("/T ({})\n", field_name));
        field.push_str(&format!("/V {} 0 R\n", sig_dict_id));
        field.push_str(&format!("/P {} 0 R\n", page_id));
        field.push_str(&format!("/AP << /N {} 0 R >>\n", appearance_id));
        field.push_str(">>\nendobj\n");
        field
    }

    /// Builds the appearance XObject (empty form for invisible signature).
    fn build_appearance_object(&self, obj_id: u32) -> String {
        let mut obj = format!("{} 0 obj\n<<\n", obj_id);
        obj.push_str("/Type /XObject\n");
        obj.push_str("/Subtype /Form\n");
        obj.push_str("/FormType 1\n");
        obj.push_str("/BBox [0 0 0 0]\n");
        obj.push_str("/Resources << >>\n");
        obj.push_str("/Length 0\n");
        obj.push_str(">>\nstream\nendstream\nendobj\n");
        obj
    }

    /// Builds the AcroForm dictionary.
    fn build_acro_form(&self, obj_id: u32, sig_field_id: u32) -> String {
        let mut form = format!("{} 0 obj\n<<\n", obj_id);
        form.push_str(&format!("/Fields [{} 0 R]\n", sig_field_id));
        form.push_str("/SigFlags 3\n"); // SignaturesExist (1) + AppendOnly (2)
        form.push_str(">>\nendobj\n");
        form
    }

    /// Builds updated page object with Annots array.
    fn build_updated_page(&self, pdf_bytes: &[u8], page_id: u32, sig_field_id: u32) -> SignatureResult<String> {
        // Use lossy conversion since PDFs may contain binary streams
        let content = String::from_utf8_lossy(pdf_bytes);

        // Find the page object
        let page_pattern = format!("{} 0 obj", page_id);
        let page_start = content.find(&page_pattern).ok_or_else(|| {
            SignatureError::SigningFailed(format!("Could not find page object {}", page_id))
        })?;

        // Find the end of the page object
        let page_content = &content[page_start..];
        let endobj_pos = page_content.find("endobj").ok_or_else(|| {
            SignatureError::SigningFailed("Could not find endobj for page".to_string())
        })?;

        let page_obj = &page_content[..endobj_pos + 6];

        // Check if page already has Annots
        if page_obj.contains("/Annots") {
            // Add to existing Annots
            let annots_start = page_obj.find("/Annots").unwrap();
            let after_annots = &page_obj[annots_start + 7..];

            if let Some(bracket_pos) = after_annots.find('[') {
                let before_bracket = &page_obj[..annots_start + 7 + bracket_pos + 1];
                let after_bracket = &after_annots[bracket_pos + 1..];

                // Find the closing bracket
                if let Some(close_pos) = after_bracket.find(']') {
                    let annots_content = &after_bracket[..close_pos];
                    let rest = &after_bracket[close_pos..];

                    let new_annots = format!("{} {} 0 R", annots_content.trim(), sig_field_id);
                    return Ok(format!("{}[{}]{}", before_bracket, new_annots, rest));
                }
            }
        }

        // Add new Annots array before >>
        let dict_end = page_obj.rfind(">>").ok_or_else(|| {
            SignatureError::SigningFailed("Invalid page object structure".to_string())
        })?;

        let before_end = &page_obj[..dict_end];

        // Build the updated object with proper formatting
        Ok(format!("{} 0 obj\n{}/Annots [{} 0 R]\n>>\nendobj\n", page_id, before_end.trim_start_matches(&format!("{} 0 obj", page_id)).trim(), sig_field_id))
    }

    /// Builds updated catalog object with AcroForm reference.
    fn build_updated_catalog(&self, pdf_bytes: &[u8], catalog_id: u32, acro_form_id: u32) -> SignatureResult<String> {
        // Use lossy conversion since PDFs may contain binary streams
        let content = String::from_utf8_lossy(pdf_bytes);

        // Find the catalog object
        let catalog_pattern = format!("{} 0 obj", catalog_id);
        let catalog_start = content.find(&catalog_pattern).ok_or_else(|| {
            SignatureError::SigningFailed(format!("Could not find catalog object {}", catalog_id))
        })?;

        // Find the end of the catalog object
        let catalog_content = &content[catalog_start..];
        let endobj_pos = catalog_content.find("endobj").ok_or_else(|| {
            SignatureError::SigningFailed("Could not find endobj for catalog".to_string())
        })?;

        let catalog_obj = &catalog_content[..endobj_pos + 6];

        // Check if catalog already has AcroForm
        if catalog_obj.contains("/AcroForm") {
            // Replace existing AcroForm reference
            let acro_start = catalog_obj.find("/AcroForm").unwrap();
            let after_acro = &catalog_obj[acro_start + 9..];

            // Find the end of the reference (N 0 R)
            let trimmed = after_acro.trim_start();
            let parts: Vec<&str> = trimmed.split_whitespace().take(3).collect();
            if parts.len() >= 3 && parts[2] == "R" {
                let old_ref_len = trimmed.find('R').unwrap() + 1;
                let before_acro = &catalog_obj[..acro_start];
                let after_ref_start = acro_start + 9 + (after_acro.len() - trimmed.len()) + old_ref_len;
                let after_ref = &catalog_obj[after_ref_start..];

                return Ok(format!("{}/AcroForm {} 0 R{}", before_acro, acro_form_id, after_ref));
            }
        }

        // Add new AcroForm reference before >>
        let dict_end = catalog_obj.rfind(">>").ok_or_else(|| {
            SignatureError::SigningFailed("Invalid catalog object structure".to_string())
        })?;

        let before_end = &catalog_obj[..dict_end];

        // Build the updated object with proper formatting
        Ok(format!("{} 0 obj\n{}/AcroForm {} 0 R\n>>\nendobj\n", catalog_id, before_end.trim_start_matches(&format!("{} 0 obj", catalog_id)).trim(), acro_form_id))
    }

    /// Builds the incremental xref table.
    fn build_incremental_xref(
        &self,
        object_offsets: &[(u32, usize)],
        prev_xref: usize,
        root_obj_num: u32,
        next_obj_id: u32,
    ) -> String {
        let mut xref = String::from("xref\n");

        // Add free object entry
        xref.push_str("0 1\n");
        xref.push_str("0000000000 65535 f \n");

        // Add entries for each object, grouped by consecutive object numbers
        let mut sorted_offsets = object_offsets.to_vec();
        sorted_offsets.sort_by_key(|(id, _)| *id);

        for (obj_id, offset) in &sorted_offsets {
            xref.push_str(&format!("{} 1\n", obj_id));
            xref.push_str(&format!("{:010} 00000 n \n", offset));
        }

        // Trailer
        xref.push_str("trailer\n");
        xref.push_str("<<\n");
        xref.push_str(&format!("/Size {}\n", next_obj_id));
        xref.push_str(&format!("/Root {} 0 R\n", root_obj_num));
        xref.push_str(&format!("/Prev {}\n", prev_xref));
        xref.push_str(">>\n");

        xref
    }

    /// Calculates the byte range for the signature.
    fn calculate_byte_range(&self, pdf_bytes: &[u8]) -> SignatureResult<ByteRange> {
        // Search for /Contents < as bytes to avoid UTF-8 position issues
        let pattern = b"/Contents <";
        let contents_start = pdf_bytes
            .windows(pattern.len())
            .position(|window| window == pattern)
            .ok_or_else(|| {
                SignatureError::ByteRangeError("Could not find /Contents in signature".to_string())
            })?;

        // Position of < is at contents_start + 10 (length of "/Contents ")
        let hex_open = contents_start + 10;

        // Find the closing > by searching for it after the hex content
        // We know the hex content is SIGNATURE_SIZE * 2 bytes of zeros
        let expected_close = hex_open + 1 + (SIGNATURE_SIZE * 2);

        // Verify there's a > at the expected position
        if expected_close >= pdf_bytes.len() || pdf_bytes[expected_close] != b'>' {
            // Fallback: search for > after hex_open
            let hex_end = pdf_bytes[hex_open..]
                .iter()
                .position(|&b| b == b'>')
                .ok_or_else(|| {
                    SignatureError::ByteRangeError("Could not find closing > for Contents".to_string())
                })? + hex_open;

            return Ok(ByteRange {
                offset1: 0,
                length1: (hex_open + 1) as i64,
                offset2: hex_end as i64,
                length2: (pdf_bytes.len() - hex_end) as i64,
            });
        }

        // ByteRange: [start1, length1, start2, length2]
        // start1 = 0 (beginning of file)
        // length1 = position right after '<'
        // start2 = position of '>'
        // length2 = length from '>' to end of file
        Ok(ByteRange {
            offset1: 0,
            length1: (hex_open + 1) as i64,
            offset2: expected_close as i64,
            length2: (pdf_bytes.len() - expected_close) as i64,
        })
    }

    /// Updates the ByteRange placeholder with actual values.
    fn update_byte_range_placeholder(&self, pdf_bytes: Vec<u8>, byte_range: &ByteRange) -> SignatureResult<Vec<u8>> {
        // Find the ByteRange placeholder by searching for the byte pattern
        let placeholder = b"/ByteRange [0000000000 0000000000 0000000000 0000000000]";
        let replacement = format!(
            "/ByteRange [{:010} {:010} {:010} {:010}]",
            byte_range.offset1,
            byte_range.length1,
            byte_range.offset2,
            byte_range.length2
        );

        // Find the placeholder position
        let mut pos = None;
        for i in 0..pdf_bytes.len().saturating_sub(placeholder.len()) {
            if &pdf_bytes[i..i + placeholder.len()] == placeholder {
                pos = Some(i);
                break;
            }
        }

        let placeholder_pos = pos.ok_or_else(|| {
            SignatureError::SigningFailed("Could not find ByteRange placeholder".to_string())
        })?;

        // Replace the placeholder
        let mut result = Vec::with_capacity(pdf_bytes.len());
        result.extend_from_slice(&pdf_bytes[..placeholder_pos]);
        result.extend_from_slice(replacement.as_bytes());
        result.extend_from_slice(&pdf_bytes[placeholder_pos + placeholder.len()..]);

        Ok(result)
    }

    /// Extracts the data to be signed based on the byte range.
    fn extract_signed_data(&self, pdf_bytes: &[u8], byte_range: &ByteRange) -> Vec<u8> {
        let mut data = Vec::new();

        // First range: from offset1 for length1 bytes
        let start1 = byte_range.offset1 as usize;
        let end1 = start1 + byte_range.length1 as usize;
        if end1 <= pdf_bytes.len() {
            data.extend_from_slice(&pdf_bytes[start1..end1]);
        }

        // Second range: from offset2 for length2 bytes
        let start2 = byte_range.offset2 as usize;
        let end2 = start2 + byte_range.length2 as usize;
        if end2 <= pdf_bytes.len() {
            data.extend_from_slice(&pdf_bytes[start2..end2]);
        }

        data
    }

    /// Embeds the signature into the PDF.
    fn embed_signature(
        &self,
        pdf_bytes: Vec<u8>,
        byte_range: &ByteRange,
        signature: &[u8],
    ) -> SignatureResult<Vec<u8>> {
        // Convert signature to hex (uppercase)
        let sig_hex: String = signature.iter().map(|b| format!("{:02X}", b)).collect();

        // Pad with zeros to fill the placeholder
        let placeholder_size = SIGNATURE_SIZE * 2;
        let padded_hex = if sig_hex.len() < placeholder_size {
            let padding = "0".repeat(placeholder_size - sig_hex.len());
            sig_hex + &padding
        } else if sig_hex.len() > placeholder_size {
            return Err(SignatureError::SigningFailed(
                "Signature too large for reserved space".to_string(),
            ));
        } else {
            sig_hex
        };

        // Replace the placeholder in the PDF
        // The hex content starts at byte_range.length1 (after the '<')
        // and ends at byte_range.offset2 (before the '>')
        let start = byte_range.length1 as usize;
        let end = byte_range.offset2 as usize;

        let mut result = Vec::with_capacity(pdf_bytes.len());
        result.extend_from_slice(&pdf_bytes[..start]);
        result.extend_from_slice(padded_hex.as_bytes());
        result.extend_from_slice(&pdf_bytes[end..]);

        Ok(result)
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

/// Escapes a string for PDF.
fn escape_pdf_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Formats the current time as a PDF timestamp.
fn format_pdf_timestamp() -> String {
    // D:YYYYMMDDHHmmSSOHH'mm'
    // Get current time
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let secs = now.as_secs();

    // Simple UTC time calculation (approximate)
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;

    // Calculate year, month, day (simplified algorithm)
    let mut year = 1970;
    let mut remaining_days = days_since_epoch as i64;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let mut month = 1;
    let days_in_months = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    for days in days_in_months {
        if remaining_days < days {
            break;
        }
        remaining_days -= days;
        month += 1;
    }

    let day = remaining_days + 1;
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;
    let second = time_of_day % 60;

    format!("D:{:04}{:02}{:02}{:02}{:02}{:02}+00'00'", year, month, day, hour, minute, second)
}

/// Checks if a year is a leap year.
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Signs existing PDF documents with additional signatures.
///
/// This signer adds a new signature to an already-existing PDF,
/// supporting multiple signatures on the same document.
#[derive(Debug)]
pub struct IncrementalSigner {
    /// The existing PDF bytes.
    pdf_bytes: Vec<u8>,
    /// The signer's certificate.
    certificate: Option<Certificate>,
    /// Additional certificates in the chain.
    certificate_chain: Vec<Certificate>,
    /// The private key for signing.
    private_key: Option<PrivateKey>,
    /// Signature configuration.
    config: SignatureConfig,
}

impl IncrementalSigner {
    /// Creates a new incremental signer for the given PDF bytes.
    pub fn new(pdf_bytes: Vec<u8>) -> Self {
        Self {
            pdf_bytes,
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

    /// Signs the PDF and returns the signed PDF bytes with the new signature.
    pub fn sign(self) -> SignatureResult<Vec<u8>> {
        let cert = self.certificate.as_ref().ok_or_else(|| {
            SignatureError::SigningFailed("Certificate not set".to_string())
        })?;

        let key = self.private_key.as_ref().ok_or_else(|| {
            SignatureError::SigningFailed("Private key not set".to_string())
        })?;

        // Create PDF with new signature placeholder
        let pdf_with_placeholder = self.create_pdf_with_new_signature()?;

        // Calculate the byte ranges for the new signature
        let byte_range = self.calculate_byte_range(&pdf_with_placeholder)?;

        // Update the ByteRange placeholder with actual values
        let pdf_with_byte_range = self.update_byte_range_placeholder(pdf_with_placeholder, &byte_range)?;

        // Extract the data to sign
        let data_to_sign = self.extract_signed_data(&pdf_with_byte_range, &byte_range);

        // Create the PKCS#7 signature
        let mut pkcs7_builder = Pkcs7Builder::new()
            .certificate(cert.clone())
            .algorithm(self.config.algorithm);

        for chain_cert in &self.certificate_chain {
            pkcs7_builder = pkcs7_builder.add_chain_certificate(chain_cert.clone());
        }

        let pkcs7_signature = pkcs7_builder.build(&data_to_sign, key)?;

        // Embed the signature
        let signed_pdf = self.embed_signature(pdf_with_byte_range, &byte_range, &pkcs7_signature)?;

        Ok(signed_pdf)
    }

    /// Parses the PDF to find key information.
    fn parse_pdf_info(&self) -> SignatureResult<(usize, u32, u32, u32, Option<u32>, u32)> {
        let pdf_bytes = &self.pdf_bytes;

        // Find startxref position
        let pattern = b"startxref";
        let startxref_pos = pdf_bytes
            .windows(pattern.len())
            .rposition(|window| window == pattern)
            .ok_or_else(|| {
                SignatureError::SigningFailed("Could not find startxref".to_string())
            })?;

        // Extract the xref offset
        let after_startxref = &pdf_bytes[startxref_pos + 9..];
        let offset_str: String = after_startxref
            .iter()
            .take_while(|&&b| b.is_ascii_digit() || b.is_ascii_whitespace())
            .filter(|&&b| b.is_ascii_digit())
            .map(|&b| b as char)
            .collect();

        let prev_xref: usize = offset_str.parse().map_err(|_| {
            SignatureError::SigningFailed("Could not parse xref offset".to_string())
        })?;

        // Use lossy conversion for text parsing
        let content = String::from_utf8_lossy(pdf_bytes);

        // Find /Root reference
        let root_obj_num = self.find_root_object(&content)?;

        // Find next object ID
        let next_obj_id = self.find_next_object_id(&content);

        // Find first page object
        let page_obj_num = self.find_first_page_object(&content)?;

        // Find existing AcroForm object number (if any)
        let acro_form_obj = self.find_acro_form_object(&content);

        // Count existing signatures
        let sig_count = self.count_existing_signatures(&content);

        Ok((prev_xref, root_obj_num, next_obj_id, page_obj_num, acro_form_obj, sig_count))
    }

    /// Finds the Root object number.
    fn find_root_object(&self, content: &str) -> SignatureResult<u32> {
        // Look for /Root N 0 R in trailer
        for line in content.lines().rev() {
            if let Some(pos) = line.find("/Root") {
                let after_root = &line[pos..];
                let parts: Vec<&str> = after_root.split_whitespace().collect();
                if parts.len() >= 3 {
                    if let Ok(num) = parts[1].parse::<u32>() {
                        return Ok(num);
                    }
                }
            }
        }
        Err(SignatureError::SigningFailed("Could not find /Root reference".to_string()))
    }

    /// Finds the next available object ID.
    fn find_next_object_id(&self, content: &str) -> u32 {
        let mut max_id: u32 = 0;

        // Find all "N 0 obj" patterns
        let mut chars = content.chars().peekable();
        let mut num_str = String::new();

        while let Some(c) = chars.next() {
            if c.is_ascii_digit() {
                num_str.push(c);
            } else if c.is_whitespace() && !num_str.is_empty() {
                let rest: String = chars.clone().take(5).collect();
                if rest.starts_with("0 obj") || rest.starts_with("0  obj") {
                    if let Ok(num) = num_str.parse::<u32>() {
                        if num > max_id {
                            max_id = num;
                        }
                    }
                }
                num_str.clear();
            } else {
                num_str.clear();
            }
        }

        // Check /Size in trailer
        if let Some(size_pos) = content.rfind("/Size") {
            let after_size = &content[size_pos + 5..];
            let trimmed = after_size.trim_start();
            if let Some(end) = trimmed.find(|c: char| !c.is_ascii_digit()) {
                if let Ok(size) = trimmed[..end].parse::<u32>() {
                    if size > max_id {
                        max_id = size;
                    }
                }
            }
        }

        max_id + 1
    }

    /// Finds the first page object number.
    fn find_first_page_object(&self, content: &str) -> SignatureResult<u32> {
        // Look for /Type /Page (not /Pages)
        if let Some(pages_pos) = content.find("/Pages") {
            let after_pages = &content[pages_pos + 6..];
            let trimmed = after_pages.trim_start();
            let parts: Vec<&str> = trimmed.split_whitespace().take(3).collect();
            if parts.len() >= 3 && parts[2] == "R" {
                if let Ok(pages_obj_num) = parts[0].parse::<u32>() {
                    let pages_pattern = format!("{} 0 obj", pages_obj_num);
                    if let Some(pages_obj_pos) = content.find(&pages_pattern) {
                        let pages_obj = &content[pages_obj_pos..];
                        if let Some(kids_pos) = pages_obj.find("/Kids") {
                            let after_kids = &pages_obj[kids_pos + 5..];
                            if let Some(bracket_pos) = after_kids.find('[') {
                                let after_bracket = &after_kids[bracket_pos + 1..];
                                let parts: Vec<&str> = after_bracket.split_whitespace().take(3).collect();
                                if parts.len() >= 2 {
                                    if let Ok(page_num) = parts[0].parse::<u32>() {
                                        return Ok(page_num);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Err(SignatureError::SigningFailed("Could not find page object".to_string()))
    }

    /// Finds existing AcroForm object number if present.
    fn find_acro_form_object(&self, content: &str) -> Option<u32> {
        // Look for /AcroForm N 0 R in catalog
        if let Some(pos) = content.find("/AcroForm") {
            let after_acro = &content[pos + 9..];
            let trimmed = after_acro.trim_start();
            let parts: Vec<&str> = trimmed.split_whitespace().take(3).collect();
            if parts.len() >= 3 && parts[2] == "R" {
                if let Ok(num) = parts[0].parse::<u32>() {
                    return Some(num);
                }
            }
        }
        None
    }

    /// Counts existing signatures in the PDF.
    fn count_existing_signatures(&self, content: &str) -> u32 {
        let mut count = 0;
        let mut search_pos = 0;
        while let Some(pos) = content[search_pos..].find("/FT /Sig") {
            count += 1;
            search_pos += pos + 8;
        }
        count
    }

    /// Creates a PDF with a new signature placeholder using incremental update.
    fn create_pdf_with_new_signature(&self) -> SignatureResult<Vec<u8>> {
        let (prev_xref, root_obj_num, next_obj_id, page_obj_num, existing_acro_form, sig_count) =
            self.parse_pdf_info()?;

        let original_pdf = &self.pdf_bytes;
        let mut output = original_pdf.clone();

        // Ensure PDF ends with newline
        if !output.ends_with(b"\n") {
            output.push(b'\n');
        }

        // Assign new object IDs
        let sig_dict_id = next_obj_id;
        let sig_field_id = next_obj_id + 1;
        let appearance_id = next_obj_id + 2;
        let acro_form_id = if let Some(id) = existing_acro_form {
            id
        } else {
            next_obj_id + 3
        };
        let final_next_id = if existing_acro_form.is_some() {
            next_obj_id + 3
        } else {
            next_obj_id + 4
        };

        let sig_name = format!("Signature{}", sig_count + 1);

        // Track offsets
        let mut object_offsets: Vec<(u32, usize)> = Vec::new();

        // 1. Build signature dictionary
        let sig_dict_offset = output.len();
        let sig_dict = self.build_signature_dictionary(sig_dict_id);
        output.extend_from_slice(sig_dict.as_bytes());
        object_offsets.push((sig_dict_id, sig_dict_offset));

        // 2. Build signature field widget
        let sig_field_offset = output.len();
        let sig_field = self.build_signature_field(sig_field_id, sig_dict_id, appearance_id, page_obj_num, &sig_name);
        output.extend_from_slice(sig_field.as_bytes());
        object_offsets.push((sig_field_id, sig_field_offset));

        // 3. Build appearance XObject
        let appearance_offset = output.len();
        let appearance = self.build_appearance_object(appearance_id);
        output.extend_from_slice(appearance.as_bytes());
        object_offsets.push((appearance_id, appearance_offset));

        // 4. Build or update AcroForm
        let acro_form_offset = output.len();
        let acro_form = if let Some(existing_id) = existing_acro_form {
            self.build_updated_acro_form(original_pdf, existing_id, sig_field_id)?
        } else {
            self.build_new_acro_form(acro_form_id, sig_field_id)
        };
        output.extend_from_slice(acro_form.as_bytes());
        object_offsets.push((acro_form_id, acro_form_offset));

        // 5. Build updated page with new annotation
        let updated_page_offset = output.len();
        let updated_page = self.build_updated_page(original_pdf, page_obj_num, sig_field_id)?;
        output.extend_from_slice(updated_page.as_bytes());
        object_offsets.push((page_obj_num, updated_page_offset));

        // 6. Build updated catalog if needed (only if we created new AcroForm)
        if existing_acro_form.is_none() {
            let updated_catalog_offset = output.len();
            let updated_catalog = self.build_updated_catalog(original_pdf, root_obj_num, acro_form_id)?;
            output.extend_from_slice(updated_catalog.as_bytes());
            object_offsets.push((root_obj_num, updated_catalog_offset));
        }

        // 7. Build incremental xref
        let xref_offset = output.len();
        let xref = self.build_incremental_xref(&object_offsets, prev_xref, root_obj_num, final_next_id);
        output.extend_from_slice(xref.as_bytes());

        // 8. Add startxref and %%EOF
        output.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_offset).as_bytes());

        Ok(output)
    }

    /// Builds the signature dictionary.
    fn build_signature_dictionary(&self, obj_id: u32) -> String {
        let signer_name = self.config.name.as_deref().unwrap_or("Unknown");
        let timestamp = format_pdf_timestamp();

        let mut dict = format!("{} 0 obj\n<<\n", obj_id);
        dict.push_str("/Type /Sig\n");
        dict.push_str("/Filter /Adobe.PPKLite\n");
        dict.push_str("/SubFilter /adbe.pkcs7.detached\n");
        dict.push_str("/ByteRange [0000000000 0000000000 0000000000 0000000000]\n");
        dict.push_str("/Contents <");
        dict.push_str(&"0".repeat(SIGNATURE_SIZE * 2));
        dict.push_str(">\n");
        dict.push_str(&format!("/Name ({})\n", escape_pdf_string(signer_name)));
        dict.push_str(&format!("/M ({})\n", timestamp));

        if let Some(ref reason) = self.config.reason {
            dict.push_str(&format!("/Reason ({})\n", escape_pdf_string(reason)));
        }
        if let Some(ref location) = self.config.location {
            dict.push_str(&format!("/Location ({})\n", escape_pdf_string(location)));
        }
        if let Some(ref contact) = self.config.contact_info {
            dict.push_str(&format!("/ContactInfo ({})\n", escape_pdf_string(contact)));
        }

        dict.push_str(">>\nendobj\n");
        dict
    }

    /// Builds the signature field widget.
    fn build_signature_field(&self, field_id: u32, sig_dict_id: u32, appearance_id: u32, page_id: u32, field_name: &str) -> String {
        let mut field = format!("{} 0 obj\n<<\n", field_id);
        field.push_str("/Type /Annot\n");
        field.push_str("/Subtype /Widget\n");
        field.push_str("/FT /Sig\n");
        field.push_str("/F 132\n");
        field.push_str("/Rect [0 0 0 0]\n");
        field.push_str(&format!("/T ({})\n", field_name));
        field.push_str(&format!("/V {} 0 R\n", sig_dict_id));
        field.push_str(&format!("/P {} 0 R\n", page_id));
        field.push_str(&format!("/AP << /N {} 0 R >>\n", appearance_id));
        field.push_str(">>\nendobj\n");
        field
    }

    /// Builds the appearance XObject.
    fn build_appearance_object(&self, obj_id: u32) -> String {
        let mut obj = format!("{} 0 obj\n<<\n", obj_id);
        obj.push_str("/Type /XObject\n");
        obj.push_str("/Subtype /Form\n");
        obj.push_str("/FormType 1\n");
        obj.push_str("/BBox [0 0 0 0]\n");
        obj.push_str("/Resources << >>\n");
        obj.push_str("/Length 0\n");
        obj.push_str(">>\nstream\nendstream\nendobj\n");
        obj
    }

    /// Builds a new AcroForm dictionary.
    fn build_new_acro_form(&self, obj_id: u32, sig_field_id: u32) -> String {
        let mut form = format!("{} 0 obj\n<<\n", obj_id);
        form.push_str(&format!("/Fields [{} 0 R]\n", sig_field_id));
        form.push_str("/SigFlags 3\n");
        form.push_str(">>\nendobj\n");
        form
    }

    /// Builds updated AcroForm with new field added.
    fn build_updated_acro_form(&self, pdf_bytes: &[u8], acro_form_id: u32, new_field_id: u32) -> SignatureResult<String> {
        let content = String::from_utf8_lossy(pdf_bytes);

        let acro_pattern = format!("{} 0 obj", acro_form_id);
        let acro_start = content.find(&acro_pattern).ok_or_else(|| {
            SignatureError::SigningFailed(format!("Could not find AcroForm object {}", acro_form_id))
        })?;

        let acro_content = &content[acro_start..];
        let endobj_pos = acro_content.find("endobj").ok_or_else(|| {
            SignatureError::SigningFailed("Could not find endobj for AcroForm".to_string())
        })?;

        let acro_obj = &acro_content[..endobj_pos + 6];

        // Find /Fields array and add new field
        if let Some(fields_pos) = acro_obj.find("/Fields") {
            let after_fields = &acro_obj[fields_pos + 7..];
            if let Some(bracket_pos) = after_fields.find('[') {
                let after_bracket = &after_fields[bracket_pos + 1..];
                if let Some(close_pos) = after_bracket.find(']') {
                    let existing_fields = after_bracket[..close_pos].trim();

                    // Build updated AcroForm
                    let mut form = format!("{} 0 obj\n<<\n", acro_form_id);
                    if existing_fields.is_empty() {
                        form.push_str(&format!("/Fields [{} 0 R]\n", new_field_id));
                    } else {
                        form.push_str(&format!("/Fields [{} {} 0 R]\n", existing_fields, new_field_id));
                    }
                    form.push_str("/SigFlags 3\n");
                    form.push_str(">>\nendobj\n");
                    return Ok(form);
                }
            }
        }

        Err(SignatureError::SigningFailed("Could not parse AcroForm /Fields".to_string()))
    }

    /// Builds updated page with new annotation.
    fn build_updated_page(&self, pdf_bytes: &[u8], page_id: u32, sig_field_id: u32) -> SignatureResult<String> {
        let content = String::from_utf8_lossy(pdf_bytes);

        let page_pattern = format!("{} 0 obj", page_id);
        let page_start = content.find(&page_pattern).ok_or_else(|| {
            SignatureError::SigningFailed(format!("Could not find page object {}", page_id))
        })?;

        let page_content = &content[page_start..];
        let endobj_pos = page_content.find("endobj").ok_or_else(|| {
            SignatureError::SigningFailed("Could not find endobj for page".to_string())
        })?;

        let page_obj = &page_content[..endobj_pos + 6];

        // Check if page already has Annots
        if page_obj.contains("/Annots") {
            let annots_start = page_obj.find("/Annots").unwrap();
            let after_annots = &page_obj[annots_start + 7..];

            if let Some(bracket_pos) = after_annots.find('[') {
                let after_bracket = &after_annots[bracket_pos + 1..];
                if let Some(close_pos) = after_bracket.find(']') {
                    let existing_annots = after_bracket[..close_pos].trim();

                    // Rebuild the page object with updated Annots
                    let dict_end = page_obj.rfind(">>").ok_or_else(|| {
                        SignatureError::SigningFailed("Invalid page structure".to_string())
                    })?;

                    let before_annots = &page_obj[..annots_start];
                    let after_close = &page_obj[annots_start + 7 + bracket_pos + 1 + close_pos + 1..dict_end];

                    let result = format!(
                        "{} 0 obj\n{}/Annots [{} {} 0 R]{}\n>>\nendobj\n",
                        page_id,
                        before_annots.trim_start_matches(&format!("{} 0 obj", page_id)).trim(),
                        existing_annots,
                        sig_field_id,
                        after_close.trim_end_matches(">>").trim_end_matches("\n").trim()
                    );
                    return Ok(result);
                }
            }
        }

        // Add new Annots array
        let dict_end = page_obj.rfind(">>").ok_or_else(|| {
            SignatureError::SigningFailed("Invalid page structure".to_string())
        })?;

        let before_end = &page_obj[..dict_end];
        Ok(format!(
            "{} 0 obj\n{}/Annots [{} 0 R]\n>>\nendobj\n",
            page_id,
            before_end.trim_start_matches(&format!("{} 0 obj", page_id)).trim(),
            sig_field_id
        ))
    }

    /// Builds updated catalog with AcroForm reference.
    fn build_updated_catalog(&self, pdf_bytes: &[u8], catalog_id: u32, acro_form_id: u32) -> SignatureResult<String> {
        let content = String::from_utf8_lossy(pdf_bytes);

        let catalog_pattern = format!("{} 0 obj", catalog_id);
        let catalog_start = content.find(&catalog_pattern).ok_or_else(|| {
            SignatureError::SigningFailed(format!("Could not find catalog object {}", catalog_id))
        })?;

        let catalog_content = &content[catalog_start..];
        let endobj_pos = catalog_content.find("endobj").ok_or_else(|| {
            SignatureError::SigningFailed("Could not find endobj for catalog".to_string())
        })?;

        let catalog_obj = &catalog_content[..endobj_pos + 6];
        let dict_end = catalog_obj.rfind(">>").ok_or_else(|| {
            SignatureError::SigningFailed("Invalid catalog structure".to_string())
        })?;

        let before_end = &catalog_obj[..dict_end];
        Ok(format!(
            "{} 0 obj\n{}/AcroForm {} 0 R\n>>\nendobj\n",
            catalog_id,
            before_end.trim_start_matches(&format!("{} 0 obj", catalog_id)).trim(),
            acro_form_id
        ))
    }

    /// Builds the incremental xref table.
    fn build_incremental_xref(&self, object_offsets: &[(u32, usize)], prev_xref: usize, root_obj_num: u32, next_obj_id: u32) -> String {
        let mut xref = String::from("xref\n");
        xref.push_str("0 1\n");
        xref.push_str("0000000000 65535 f \n");

        let mut sorted_offsets = object_offsets.to_vec();
        sorted_offsets.sort_by_key(|(id, _)| *id);

        for (obj_id, offset) in &sorted_offsets {
            xref.push_str(&format!("{} 1\n", obj_id));
            xref.push_str(&format!("{:010} 00000 n \n", offset));
        }

        xref.push_str("trailer\n<<\n");
        xref.push_str(&format!("/Size {}\n", next_obj_id));
        xref.push_str(&format!("/Root {} 0 R\n", root_obj_num));
        xref.push_str(&format!("/Prev {}\n", prev_xref));
        xref.push_str(">>\n");

        xref
    }

    /// Calculates the byte range for the signature.
    fn calculate_byte_range(&self, pdf_bytes: &[u8]) -> SignatureResult<ByteRange> {
        // Search for the LAST /Contents < pattern (the new signature)
        let pattern = b"/Contents <";
        let mut last_pos = None;

        for i in (0..pdf_bytes.len().saturating_sub(pattern.len())).rev() {
            if &pdf_bytes[i..i + pattern.len()] == pattern {
                last_pos = Some(i);
                break;
            }
        }

        let contents_start = last_pos.ok_or_else(|| {
            SignatureError::ByteRangeError("Could not find /Contents in signature".to_string())
        })?;

        let hex_open = contents_start + 10;
        let expected_close = hex_open + 1 + (SIGNATURE_SIZE * 2);

        if expected_close >= pdf_bytes.len() || pdf_bytes[expected_close] != b'>' {
            return Err(SignatureError::ByteRangeError("Could not find closing > for Contents".to_string()));
        }

        Ok(ByteRange {
            offset1: 0,
            length1: (hex_open + 1) as i64,
            offset2: expected_close as i64,
            length2: (pdf_bytes.len() - expected_close) as i64,
        })
    }

    /// Updates the ByteRange placeholder with actual values.
    fn update_byte_range_placeholder(&self, pdf_bytes: Vec<u8>, byte_range: &ByteRange) -> SignatureResult<Vec<u8>> {
        // Find the LAST ByteRange placeholder (the new signature)
        let placeholder = b"/ByteRange [0000000000 0000000000 0000000000 0000000000]";
        let replacement = format!(
            "/ByteRange [{:010} {:010} {:010} {:010}]",
            byte_range.offset1,
            byte_range.length1,
            byte_range.offset2,
            byte_range.length2
        );

        let mut last_pos = None;
        for i in (0..pdf_bytes.len().saturating_sub(placeholder.len())).rev() {
            if &pdf_bytes[i..i + placeholder.len()] == placeholder {
                last_pos = Some(i);
                break;
            }
        }

        let placeholder_pos = last_pos.ok_or_else(|| {
            SignatureError::SigningFailed("Could not find ByteRange placeholder".to_string())
        })?;

        let mut result = Vec::with_capacity(pdf_bytes.len());
        result.extend_from_slice(&pdf_bytes[..placeholder_pos]);
        result.extend_from_slice(replacement.as_bytes());
        result.extend_from_slice(&pdf_bytes[placeholder_pos + placeholder.len()..]);

        Ok(result)
    }

    /// Extracts the data to be signed.
    fn extract_signed_data(&self, pdf_bytes: &[u8], byte_range: &ByteRange) -> Vec<u8> {
        let mut data = Vec::new();

        let start1 = byte_range.offset1 as usize;
        let end1 = start1 + byte_range.length1 as usize;
        if end1 <= pdf_bytes.len() {
            data.extend_from_slice(&pdf_bytes[start1..end1]);
        }

        let start2 = byte_range.offset2 as usize;
        let end2 = start2 + byte_range.length2 as usize;
        if end2 <= pdf_bytes.len() {
            data.extend_from_slice(&pdf_bytes[start2..end2]);
        }

        data
    }

    /// Embeds the signature into the PDF.
    fn embed_signature(&self, pdf_bytes: Vec<u8>, byte_range: &ByteRange, signature: &[u8]) -> SignatureResult<Vec<u8>> {
        let sig_hex: String = signature.iter().map(|b| format!("{:02X}", b)).collect();

        let placeholder_size = SIGNATURE_SIZE * 2;
        let padded_hex = if sig_hex.len() < placeholder_size {
            let padding = "0".repeat(placeholder_size - sig_hex.len());
            sig_hex + &padding
        } else if sig_hex.len() > placeholder_size {
            return Err(SignatureError::SigningFailed("Signature too large".to_string()));
        } else {
            sig_hex
        };

        let start = byte_range.length1 as usize;
        let end = byte_range.offset2 as usize;

        let mut result = Vec::with_capacity(pdf_bytes.len());
        result.extend_from_slice(&pdf_bytes[..start]);
        result.extend_from_slice(padded_hex.as_bytes());
        result.extend_from_slice(&pdf_bytes[end..]);

        Ok(result)
    }
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
    fn test_format_pdf_timestamp() {
        let ts = format_pdf_timestamp();
        assert!(ts.starts_with("D:"));
        assert!(ts.contains("+00'00'"));
    }

    #[test]
    fn test_is_leap_year() {
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2023));
    }
}
