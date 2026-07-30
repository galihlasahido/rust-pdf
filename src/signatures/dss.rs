//! Document Security Store (DSS) embedding -- PAdES "B-LT" (long-term
//! validation) structural support.
//!
//! A PAdES B-LT signature adds revocation material (CRLs and/or OCSP
//! responses) and any missing certificate-chain certificates *into the PDF
//! itself*, via a `/DSS` (Document Security Store) dictionary hung off the
//! document catalog, so the signature can still be validated after the
//! original certificates/CRLs/OCSP responses may no longer be reachable
//! online. This mechanism originates in Adobe's PDF extensions and was
//! folded into ETSI EN 319 142-1 ("PAdES digital signatures") as the `/DSS`
//! object; we are not confident enough in the exact ISO 32000-2 clause
//! number to cite one here, so we cite ETSI EN 319 142-1 (the authoritative
//! source for `/DSS`) instead of guessing an ISO clause.
//!
//! # What this module does and does not do
//!
//! This module only handles the **structural** side: given DER-encoded
//! certificate/CRL/OCSP-response bytes (which the caller must obtain --
//! typically by fetching them from a CA's CRL distribution point / OCSP
//! responder over the network before calling this), it embeds them into a
//! `/DSS` dictionary via an incremental update, appended strictly after the
//! existing file content so it cannot alter the byte range covered by any
//! existing signature (multiple signatures remain valid, same guarantee as
//! [`super::IncrementalSigner`]).
//!
//! It does **not**:
//! - fetch CRLs or OCSP responses itself (no network I/O in this crate --
//!   consistent with [`super::timestamp`]'s reasoning for why RFC 3161 is
//!   transport-agnostic here too),
//! - build a `/VRI` (Validation-Related Information) sub-dictionary that
//!   disambiguates *which* revocation material belongs to *which*
//!   signature -- only the flat, document-wide `/Certs`, `/CRLs`, `/OCSPs`
//!   arrays are written. A validator that only reads `/DSS` (not `/VRI`)
//!   -- which is a legal fallback per the spec -- still benefits.
//!
//! Building full OCSP-request/response and CRL-fetch-and-parse support is a
//! substantial undertaking on par with re-implementing chunks of a mature
//! PKI library (OpenSSL's OCSP/X509 stack, or an equivalent Rust crate);
//! that is out of scope here and is called out explicitly rather than
//! silently half-done.

use crate::error::SignatureError;
use super::SignatureResult;

/// DER-encoded revocation/certificate material to embed in a `/DSS`.
#[derive(Debug, Clone, Default)]
pub struct DssEntry {
    /// DER-encoded X.509 certificates (e.g. intermediate CAs not already
    /// embedded in the CMS `SignedData.certificates`).
    pub certs: Vec<Vec<u8>>,
    /// DER-encoded CRLs (RFC 5280 `CertificateList`).
    pub crls: Vec<Vec<u8>>,
    /// DER-encoded OCSP responses (RFC 6960 `OCSPResponse`).
    pub ocsps: Vec<Vec<u8>>,
}

impl DssEntry {
    /// Returns `true` if there is nothing to embed.
    pub fn is_empty(&self) -> bool {
        self.certs.is_empty() && self.crls.is_empty() && self.ocsps.is_empty()
    }
}

/// Upper bound on how many objects a single [`embed_document_security_store`]
/// call will add, guarding against a caller-supplied [`DssEntry`] (which may
/// ultimately be populated from untrusted/attacker-influenced revocation
/// responses) from making this allocate an unreasonable number of PDF
/// objects.
const MAX_DSS_OBJECTS: usize = 4096;

/// Appends a `/DSS` (Document Security Store) to `pdf_bytes` via an
/// incremental update, embedding `entry`'s certificates/CRLs/OCSP responses
/// and wiring `/Root /DSS` to point at it.
///
/// Must be called on an already-fully-signed PDF (i.e. after
/// [`super::DocumentSigner::sign`] / [`super::IncrementalSigner::sign`]).
/// Because this only *appends* bytes, it never touches the byte range any
/// existing signature covers -- existing signatures remain valid exactly as
/// with a second [`super::IncrementalSigner`] pass.
pub fn embed_document_security_store(
    pdf_bytes: Vec<u8>,
    entry: &DssEntry,
) -> SignatureResult<Vec<u8>> {
    if entry.is_empty() {
        return Err(SignatureError::SigningFailed(
            "DssEntry has no certificates, CRLs, or OCSP responses to embed".to_string(),
        ));
    }
    let total_objects = entry.certs.len() + entry.crls.len() + entry.ocsps.len();
    if total_objects > MAX_DSS_OBJECTS {
        return Err(SignatureError::SigningFailed(format!(
            "DssEntry has too many objects ({total_objects}, limit {MAX_DSS_OBJECTS})"
        )));
    }

    let (prev_xref, root_obj_num, next_obj_id) = parse_pdf_info(&pdf_bytes)?;

    let mut output = pdf_bytes;
    if !output.ends_with(b"\n") {
        output.push(b'\n');
    }

    let mut object_offsets: Vec<(u32, usize)> = Vec::new();
    let mut next_id = next_obj_id;

    let write_blob = |output: &mut Vec<u8>, offsets: &mut Vec<(u32, usize)>, id: u32, bytes: &[u8]| {
        offsets.push((id, output.len()));
        output.extend_from_slice(format!("{id} 0 obj\n<< /Length {} >>\nstream\n", bytes.len()).as_bytes());
        output.extend_from_slice(bytes);
        output.extend_from_slice(b"\nendstream\nendobj\n");
    };

    let mut cert_ids = Vec::with_capacity(entry.certs.len());
    for cert_der in &entry.certs {
        let id = next_id;
        next_id += 1;
        write_blob(&mut output, &mut object_offsets, id, cert_der);
        cert_ids.push(id);
    }

    let mut crl_ids = Vec::with_capacity(entry.crls.len());
    for crl_der in &entry.crls {
        let id = next_id;
        next_id += 1;
        write_blob(&mut output, &mut object_offsets, id, crl_der);
        crl_ids.push(id);
    }

    let mut ocsp_ids = Vec::with_capacity(entry.ocsps.len());
    for ocsp_der in &entry.ocsps {
        let id = next_id;
        next_id += 1;
        write_blob(&mut output, &mut object_offsets, id, ocsp_der);
        ocsp_ids.push(id);
    }

    let dss_id = next_id;
    next_id += 1;
    let dss_offset = output.len();
    let mut dss = format!("{dss_id} 0 obj\n<<\n");
    dss.push_str(&format!("/Certs [{}]\n", ids_to_refs(&cert_ids)));
    dss.push_str(&format!("/CRLs [{}]\n", ids_to_refs(&crl_ids)));
    dss.push_str(&format!("/OCSPs [{}]\n", ids_to_refs(&ocsp_ids)));
    dss.push_str(">>\nendobj\n");
    output.extend_from_slice(dss.as_bytes());
    object_offsets.push((dss_id, dss_offset));

    let updated_catalog_offset = output.len();
    let updated_catalog = build_updated_catalog(&output, root_obj_num, dss_id)?;
    output.extend_from_slice(updated_catalog.as_bytes());
    object_offsets.push((root_obj_num, updated_catalog_offset));

    let xref_offset = output.len();
    let xref = build_incremental_xref(&object_offsets, prev_xref, root_obj_num, next_id);
    output.extend_from_slice(xref.as_bytes());
    output.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());

    Ok(output)
}

fn ids_to_refs(ids: &[u32]) -> String {
    ids.iter().map(|id| format!("{id} 0 R")).collect::<Vec<_>>().join(" ")
}

/// Finds `(prev_xref_offset, root_object_number, next_free_object_id)`.
/// Deliberately independent from `signer.rs`'s equivalent (private, tied to
/// `DocumentSigner`/`IncrementalSigner`) rather than sharing it, matching
/// this codebase's existing per-signer-type duplication of the same
/// incremental-update bookkeeping.
fn parse_pdf_info(pdf_bytes: &[u8]) -> SignatureResult<(usize, u32, u32)> {
    let content = String::from_utf8_lossy(pdf_bytes);

    let startxref_pos = content.rfind("startxref").ok_or_else(|| {
        SignatureError::SigningFailed("Could not find startxref".to_string())
    })?;
    let after_startxref = &content[startxref_pos + 9..];
    let prev_xref: usize = after_startxref
        .trim()
        .lines()
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| SignatureError::SigningFailed("Could not parse xref offset".to_string()))?;

    let root_obj_num = find_root_object(&content)?;
    let next_obj_id = find_next_object_id(&content);

    Ok((prev_xref, root_obj_num, next_obj_id))
}

fn find_root_object(content: &str) -> SignatureResult<u32> {
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

fn find_next_object_id(content: &str) -> u32 {
    let mut max_id: u32 = 0;
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

fn build_updated_catalog(pdf_bytes: &[u8], catalog_id: u32, dss_id: u32) -> SignatureResult<String> {
    let content = String::from_utf8_lossy(pdf_bytes);

    let catalog_pattern = format!("{catalog_id} 0 obj");
    let catalog_start = content.find(&catalog_pattern).ok_or_else(|| {
        SignatureError::SigningFailed(format!("Could not find catalog object {catalog_id}"))
    })?;
    let catalog_content = &content[catalog_start..];
    let endobj_pos = catalog_content.find("endobj").ok_or_else(|| {
        SignatureError::SigningFailed("Could not find endobj for catalog".to_string())
    })?;
    let catalog_obj = &catalog_content[..endobj_pos + 6];

    if let Some(dss_pos) = catalog_obj.find("/DSS") {
        // Replace an existing `/DSS N 0 R` reference (a second DSS embed,
        // e.g. after a later signature).
        let after_dss = &catalog_obj[dss_pos + 4..];
        let trimmed = after_dss.trim_start();
        let parts: Vec<&str> = trimmed.split_whitespace().take(3).collect();
        if parts.len() >= 3 && parts[2] == "R" {
            let old_ref_len = trimmed.find('R').unwrap() + 1;
            let before_dss = &catalog_obj[..dss_pos];
            let after_ref_start = dss_pos + 4 + (after_dss.len() - trimmed.len()) + old_ref_len;
            let after_ref = &catalog_obj[after_ref_start..];
            return Ok(format!("{catalog_id} 0 obj\n{before_dss}/DSS {dss_id} 0 R{after_ref}"));
        }
    }

    let dict_end = catalog_obj.rfind(">>").ok_or_else(|| {
        SignatureError::SigningFailed("Invalid catalog object structure".to_string())
    })?;
    let before_end = &catalog_obj[..dict_end];
    Ok(format!(
        "{catalog_id} 0 obj\n{}/DSS {dss_id} 0 R\n>>\nendobj\n",
        before_end.trim_start_matches(&catalog_pattern).trim()
    ))
}

fn build_incremental_xref(
    object_offsets: &[(u32, usize)],
    prev_xref: usize,
    root_obj_num: u32,
    next_obj_id: u32,
) -> String {
    let mut xref = String::from("xref\n0 1\n0000000000 65535 f \n");

    let mut sorted_offsets = object_offsets.to_vec();
    sorted_offsets.sort_by_key(|(id, _)| *id);
    for (obj_id, offset) in &sorted_offsets {
        xref.push_str(&format!("{obj_id} 1\n{offset:010} 00000 n \n"));
    }

    xref.push_str("trailer\n<<\n");
    xref.push_str(&format!("/Size {next_obj_id}\n"));
    xref.push_str(&format!("/Root {root_obj_num} 0 R\n"));
    xref.push_str(&format!("/Prev {prev_xref}\n"));
    xref.push_str(">>\n");
    xref
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dss_entry_is_empty() {
        assert!(DssEntry::default().is_empty());
        let entry = DssEntry { certs: vec![vec![1, 2, 3]], ..Default::default() };
        assert!(!entry.is_empty());
    }

    #[test]
    fn test_embed_document_security_store_rejects_empty_entry() {
        let err = embed_document_security_store(b"%PDF-1.7\n%%EOF".to_vec(), &DssEntry::default())
            .unwrap_err();
        assert!(matches!(err, SignatureError::SigningFailed(_)));
    }

    #[test]
    fn test_embed_document_security_store_rejects_oversized_entry() {
        let entry = DssEntry {
            certs: vec![vec![0u8]; MAX_DSS_OBJECTS + 1],
            ..Default::default()
        };
        let err = embed_document_security_store(b"%PDF-1.7\n%%EOF".to_vec(), &entry).unwrap_err();
        assert!(matches!(err, SignatureError::SigningFailed(_)));
    }

    #[test]
    fn test_ids_to_refs() {
        assert_eq!(ids_to_refs(&[]), "");
        assert_eq!(ids_to_refs(&[1, 2]), "1 0 R 2 0 R");
    }

    #[test]
    fn test_find_next_object_id() {
        let pdf = "1 0 obj\n<<>>\nendobj\n5 0 obj\n<<>>\nendobj\n";
        assert_eq!(find_next_object_id(pdf), 6);
    }
}
