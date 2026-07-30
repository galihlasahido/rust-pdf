//! Document timestamp dictionaries -- PAdES "B-LTA" (long-term archival)
//! structural support.
//!
//! ISO 32000-2:2020 §12.8.5 ("Document timestamp dictionaries") defines a
//! `/Type /DocTimeStamp` signature-shaped dictionary, recorded via the same
//! signature-field/`AcroForm` mechanism as a normal `/Type /Sig` dictionary
//! (12.7.4.3, "Signature fields"), whose `/Contents` holds a raw RFC 3161
//! `TimeStampToken` (a CMS `ContentInfo` wrapping a `SignedData` over
//! `TSTInfo`) instead of a PKCS#7 `SignedData` over the document. PAdES calls
//! a document that carries one of these over its whole byte range (including
//! any prior `/DSS`) "B-LTA" (ETSI EN 319 142-1): once the original signer's
//! certificate may no longer be trusted/verifiable, the archive timestamp's
//! own trusted time still proves the document (and its embedded revocation
//! evidence) existed, unmodified, no later than the token's `genTime`.
//!
//! # Where this fits in the PAdES chain
//!
//! Mirrors [`super::dss::embed_document_security_store`]'s shape: a separate
//! explicit function the caller chains *after* signing (and, for a real
//! B-LTA, after embedding a `/DSS` -- see [`super::embed_document_security_store`]
//! -- so the archive timestamp's byte range also covers the DSS, protecting
//! it too), rather than a [`super::PadesLevel`] variant. Typical chain:
//!
//! ```ignore
//! let signed = IncrementalSigner::new(pdf_bytes)...sign()?;
//! let with_dss = embed_document_security_store(signed, &dss_entry)?;
//! let with_lta = embed_document_timestamp(with_dss, &tsa_client, SignatureAlgorithm::RsaSha256)?;
//! ```
//!
//! Like [`super::dss`], this only *appends* bytes via an incremental update,
//! so it never touches the byte range any existing `/Sig`/`/DocTimeStamp`
//! covers -- earlier signatures/timestamps remain exactly as valid as they
//! were before this call (same guarantee as a second [`super::IncrementalSigner`]
//! pass or a [`super::embed_document_security_store`] call).
//!
//! # What this module does and does not do
//!
//! It builds the `/DocTimeStamp` dictionary, its signature field/widget, and
//! wires it into the `AcroForm` and the first page's `/Annots`, reusing the
//! request/response plumbing already implemented in [`super::timestamp`]
//! (this module does not reimplement RFC 3161 request-building or
//! response-parsing).
//!
//! It does **not**:
//! - fetch or embed the archive-timestamp TSA's own certificate/revocation
//!   material into `/DSS` -- if the archive-timestamp TSA's chain should
//!   itself be verifiable long-term, call
//!   [`super::embed_document_security_store`] again afterwards with that
//!   material,
//! - automatically re-timestamp before the embedded token (or its TSA
//!   certificate) expires -- a real long-term archival deployment needs to
//!   periodically call this again with a fresh token before that happens;
//!   this crate has no background scheduler and does not track expiry,
//! - teach [`super::SignatureVerifier`] to recognize or verify
//!   `/Type /DocTimeStamp` dictionaries -- it only scans for `/Type /Sig`
//!   (see `verifier.rs::find_signature_objects`), so a document timestamp
//!   embedded by this module is invisible to `SignatureVerifier::verify`.
//!   Verifying one requires decoding its `/Contents` as a bare
//!   `TimeStampToken` (RFC 3161 §2.4.2, not a full `TimeStampResp`) and
//!   checking the `TSTInfo.messageImprint` against the digest of its own
//!   `/ByteRange`-covered bytes -- out of scope here,
//! - write the optional `/M` (signing time) entry -- doing so would need a
//!   third copy of `signer.rs`'s PDF-date-string formatting in this crate;
//!   the token's own `TSTInfo.genTime` is the trustworthy timestamp anyway,
//!   so omitting `/M` costs nothing a verifier needs.

use std::sync::Arc;

use super::timestamp::{build_timestamp_request, parse_timestamp_response};
use super::{digest_for_algorithm, ByteRange, SignatureAlgorithm, SignatureResult};
use super::{TimestampAuthorityClient, TimestampToken};
use crate::error::SignatureError;

/// Lower/upper bounds clamped around the reserved `/Contents` placeholder
/// size, mirroring `signer.rs`'s `MIN_SIGNATURE_SIZE`/`MAX_SIGNATURE_SIZE`
/// (kept as an independent copy rather than shared constants, consistent
/// with `dss.rs`'s precedent of not sharing incremental-update bookkeeping
/// across these sibling modules).
const MIN_CONTENTS_SIZE: usize = 4096;
const MAX_CONTENTS_SIZE: usize = 1 << 20; // 1 MiB

/// Default reserved `/Contents` placeholder size (in bytes) -- an RFC 3161
/// token (TSTInfo + the TSA's CMS signature + its certificate) is typically
/// a few KB, comfortably under this; see [`embed_document_timestamp_sized`]
/// to override it for an unusually large TSA response (e.g. a long
/// TSA certificate chain).
const DEFAULT_CONTENTS_SIZE: usize = 16384;

/// Appends a PAdES "B-LTA" archive timestamp to `pdf_bytes` via an
/// incremental update: reserves a `/DocTimeStamp` dictionary, requests an
/// RFC 3161 token over the document's current bytes from `timestamp_authority`,
/// and embeds the raw `TimeStampToken` DER as `/Contents`.
///
/// Uses [`DEFAULT_CONTENTS_SIZE`] as the reserved `/Contents` size; see
/// [`embed_document_timestamp_sized`] to override it.
pub fn embed_document_timestamp(
    pdf_bytes: Vec<u8>,
    timestamp_authority: &Arc<dyn TimestampAuthorityClient>,
    algorithm: SignatureAlgorithm,
) -> SignatureResult<Vec<u8>> {
    embed_document_timestamp_sized(
        pdf_bytes,
        timestamp_authority,
        algorithm,
        DEFAULT_CONTENTS_SIZE,
    )
}

/// As [`embed_document_timestamp`], but with an explicit reserved
/// `/Contents` placeholder size (in bytes), clamped to
/// `[MIN_CONTENTS_SIZE, MAX_CONTENTS_SIZE]`.
pub fn embed_document_timestamp_sized(
    pdf_bytes: Vec<u8>,
    timestamp_authority: &Arc<dyn TimestampAuthorityClient>,
    algorithm: SignatureAlgorithm,
    contents_size: usize,
) -> SignatureResult<Vec<u8>> {
    let contents_size = contents_size.clamp(MIN_CONTENTS_SIZE, MAX_CONTENTS_SIZE);

    let info = parse_pdf_info(&pdf_bytes)?;
    let pdf_with_placeholder = create_pdf_with_placeholder(&pdf_bytes, &info, contents_size)?;

    let byte_range = calculate_byte_range(&pdf_with_placeholder, contents_size)?;
    let pdf_with_byte_range = update_byte_range_placeholder(pdf_with_placeholder, &byte_range)?;

    let data_to_timestamp = extract_ranged_data(&pdf_with_byte_range, &byte_range);
    let digest = digest_for_algorithm(algorithm, &data_to_timestamp);

    let token = request_token(timestamp_authority.as_ref(), algorithm, digest)?;

    embed_contents(
        pdf_with_byte_range,
        &byte_range,
        &token.token_der,
        contents_size,
    )
}

/// Requests and validates an RFC 3161 token over `digest`, isolated into its
/// own function purely so [`embed_document_timestamp_sized`]'s main flow
/// reads linearly (no behavior difference).
fn request_token(
    client: &dyn TimestampAuthorityClient,
    algorithm: SignatureAlgorithm,
    digest: Vec<u8>,
) -> SignatureResult<TimestampToken> {
    let request = build_timestamp_request(algorithm, digest.clone(), true);
    let response = client.timestamp(&request.der)?;
    parse_timestamp_response(&response, algorithm, &digest, Some(request.nonce))
}

/// Key facts about `pdf_bytes` needed to append the incremental update,
/// analogous to `signer.rs::IncrementalSigner::parse_pdf_info` /
/// `dss.rs::parse_pdf_info`.
struct PdfInfo {
    prev_xref: usize,
    root_obj_num: u32,
    next_obj_id: u32,
    page_obj_num: u32,
    acro_form_obj: Option<u32>,
    doc_timestamp_count: u32,
}

fn parse_pdf_info(pdf_bytes: &[u8]) -> SignatureResult<PdfInfo> {
    let content = String::from_utf8_lossy(pdf_bytes);

    let startxref_pos = content
        .rfind("startxref")
        .ok_or_else(|| SignatureError::SigningFailed("Could not find startxref".to_string()))?;
    let after_startxref = &content[startxref_pos + 9..];
    let prev_xref: usize = after_startxref
        .trim()
        .lines()
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| SignatureError::SigningFailed("Could not parse xref offset".to_string()))?;

    let root_obj_num = find_root_object(&content)?;
    let next_obj_id = find_next_object_id(&content);
    let page_obj_num = find_first_page_object(&content)?;
    let acro_form_obj = find_acro_form_object(&content);
    let doc_timestamp_count = count_matches(&content, "/Type /DocTimeStamp");

    Ok(PdfInfo {
        prev_xref,
        root_obj_num,
        next_obj_id,
        page_obj_num,
        acro_form_obj,
        doc_timestamp_count,
    })
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
    Err(SignatureError::SigningFailed(
        "Could not find /Root reference".to_string(),
    ))
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

fn find_first_page_object(content: &str) -> SignatureResult<u32> {
    if let Some(pages_pos) = content.find("/Pages") {
        let after_pages = &content[pages_pos + 6..];
        let trimmed = after_pages.trim_start();
        let parts: Vec<&str> = trimmed.split_whitespace().take(3).collect();
        if parts.len() >= 3 && parts[2] == "R" {
            if let Ok(pages_obj_num) = parts[0].parse::<u32>() {
                let pages_pattern = format!("{pages_obj_num} 0 obj");
                if let Some(pages_obj_pos) = content.find(&pages_pattern) {
                    let pages_obj = &content[pages_obj_pos..];
                    if let Some(kids_pos) = pages_obj.find("/Kids") {
                        let after_kids = &pages_obj[kids_pos + 5..];
                        if let Some(bracket_pos) = after_kids.find('[') {
                            let after_bracket = &after_kids[bracket_pos + 1..];
                            let parts: Vec<&str> =
                                after_bracket.split_whitespace().take(3).collect();
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
    Err(SignatureError::SigningFailed(
        "Could not find page object".to_string(),
    ))
}

fn find_acro_form_object(content: &str) -> Option<u32> {
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

fn count_matches(content: &str, pattern: &str) -> u32 {
    let mut count = 0;
    let mut search_pos = 0;
    while let Some(pos) = content[search_pos..].find(pattern) {
        count += 1;
        search_pos += pos + pattern.len();
    }
    count
}

/// Builds the incremental update: `/DocTimeStamp` dictionary, its signature
/// field/widget, an empty (invisible) appearance `XObject`, the `AcroForm`
/// (new or updated), the page's `/Annots`, and (only if there was no
/// existing `AcroForm`) an updated catalog -- same object shapes as
/// `signer.rs::IncrementalSigner::create_pdf_with_new_signature`, adapted
/// for a `/DocTimeStamp` rather than a `/Sig` dictionary.
fn create_pdf_with_placeholder(
    pdf_bytes: &[u8],
    info: &PdfInfo,
    contents_size: usize,
) -> SignatureResult<Vec<u8>> {
    let mut output = pdf_bytes.to_vec();
    if !output.ends_with(b"\n") {
        output.push(b'\n');
    }

    let next_obj_id = info.next_obj_id;
    let ts_dict_id = next_obj_id;
    let ts_field_id = next_obj_id + 1;
    let appearance_id = next_obj_id + 2;
    const BASE_USED: u32 = 3;
    let acro_form_id = info.acro_form_obj.unwrap_or(next_obj_id + BASE_USED);
    let final_next_id = if info.acro_form_obj.is_some() {
        next_obj_id + BASE_USED
    } else {
        next_obj_id + BASE_USED + 1
    };

    let field_name = format!("DocTimeStamp{}", info.doc_timestamp_count + 1);

    let mut object_offsets: Vec<(u32, usize)> = Vec::new();

    let ts_dict_offset = output.len();
    output.extend_from_slice(build_doc_timestamp_dictionary(ts_dict_id, contents_size).as_bytes());
    object_offsets.push((ts_dict_id, ts_dict_offset));

    let ts_field_offset = output.len();
    output.extend_from_slice(
        build_doc_timestamp_field(
            ts_field_id,
            ts_dict_id,
            appearance_id,
            info.page_obj_num,
            &field_name,
        )
        .as_bytes(),
    );
    object_offsets.push((ts_field_id, ts_field_offset));

    let appearance_offset = output.len();
    output.extend_from_slice(build_empty_appearance_object(appearance_id).as_bytes());
    object_offsets.push((appearance_id, appearance_offset));

    let acro_form_offset = output.len();
    let acro_form = if let Some(existing_id) = info.acro_form_obj {
        build_updated_acro_form(&output, existing_id, ts_field_id)?
    } else {
        build_new_acro_form(acro_form_id, ts_field_id)
    };
    output.extend_from_slice(acro_form.as_bytes());
    object_offsets.push((acro_form_id, acro_form_offset));

    let updated_page_offset = output.len();
    let updated_page = build_updated_page(&output, info.page_obj_num, ts_field_id)?;
    output.extend_from_slice(updated_page.as_bytes());
    object_offsets.push((info.page_obj_num, updated_page_offset));

    if info.acro_form_obj.is_none() {
        let updated_catalog_offset = output.len();
        let updated_catalog = build_updated_catalog(&output, info.root_obj_num, acro_form_id)?;
        output.extend_from_slice(updated_catalog.as_bytes());
        object_offsets.push((info.root_obj_num, updated_catalog_offset));
    }

    let xref_offset = output.len();
    let xref = build_incremental_xref(
        &object_offsets,
        info.prev_xref,
        info.root_obj_num,
        final_next_id,
    );
    output.extend_from_slice(xref.as_bytes());
    output.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());

    Ok(output)
}

/// Builds the `/DocTimeStamp` dictionary (ISO 32000-2:2020 §12.8.5 Table
/// 254): same `/Filter`/`/ByteRange`/`/Contents` shape as a `/Sig`
/// dictionary (`signer.rs::build_signature_dictionary`), but
/// `/Type /DocTimeStamp` and `/SubFilter /ETSI.RFC3161` (the registered
/// subfilter for a bare RFC 3161 `TimeStampToken`, ETSI EN 319 142-1).
fn build_doc_timestamp_dictionary(obj_id: u32, contents_size: usize) -> String {
    let mut dict = format!("{obj_id} 0 obj\n<<\n");
    dict.push_str("/Type /DocTimeStamp\n");
    dict.push_str("/Filter /Adobe.PPKLite\n");
    dict.push_str("/SubFilter /ETSI.RFC3161\n");
    dict.push_str("/ByteRange [0000000000 0000000000 0000000000 0000000000]\n");
    dict.push_str("/Contents <");
    dict.push_str(&"0".repeat(contents_size * 2));
    dict.push_str(">\n");
    dict.push_str(">>\nendobj\n");
    dict
}

/// Builds the timestamp's signature field/widget -- an invisible
/// (zero-size) `/FT /Sig` annotation, same shape as
/// `signer.rs::build_signature_field`.
fn build_doc_timestamp_field(
    field_id: u32,
    ts_dict_id: u32,
    appearance_id: u32,
    page_id: u32,
    field_name: &str,
) -> String {
    let mut field = format!("{field_id} 0 obj\n<<\n");
    field.push_str("/Type /Annot\n");
    field.push_str("/Subtype /Widget\n");
    field.push_str("/FT /Sig\n");
    field.push_str("/F 132\n");
    field.push_str("/Rect [0 0 0 0]\n");
    field.push_str(&format!("/T ({field_name})\n"));
    field.push_str(&format!("/V {ts_dict_id} 0 R\n"));
    field.push_str(&format!("/P {page_id} 0 R\n"));
    field.push_str(&format!("/AP << /N {appearance_id} 0 R >>\n"));
    field.push_str(">>\nendobj\n");
    field
}

/// Builds an empty (zero-size, contentless) appearance form `XObject`,
/// matching `signer.rs::build_appearance_object`'s invisible-signature
/// branch -- a document timestamp is never visibly rendered.
fn build_empty_appearance_object(obj_id: u32) -> String {
    let mut obj = format!("{obj_id} 0 obj\n<<\n");
    obj.push_str("/Type /XObject\n");
    obj.push_str("/Subtype /Form\n");
    obj.push_str("/FormType 1\n");
    obj.push_str("/BBox [0 0 0 0]\n");
    obj.push_str("/Resources << >>\n");
    obj.push_str("/Length 0\n");
    obj.push_str(">>\nstream\nendstream\nendobj\n");
    obj
}

fn build_new_acro_form(obj_id: u32, field_id: u32) -> String {
    let mut form = format!("{obj_id} 0 obj\n<<\n");
    form.push_str(&format!("/Fields [{field_id} 0 R]\n"));
    form.push_str("/SigFlags 3\n");
    form.push_str(">>\nendobj\n");
    form
}

fn build_updated_acro_form(
    pdf_bytes: &[u8],
    acro_form_id: u32,
    new_field_id: u32,
) -> SignatureResult<String> {
    let content = String::from_utf8_lossy(pdf_bytes);

    let acro_pattern = format!("{acro_form_id} 0 obj");
    let acro_start = content.find(&acro_pattern).ok_or_else(|| {
        SignatureError::SigningFailed(format!("Could not find AcroForm object {acro_form_id}"))
    })?;

    let acro_content = &content[acro_start..];
    let endobj_pos = acro_content.find("endobj").ok_or_else(|| {
        SignatureError::SigningFailed("Could not find endobj for AcroForm".to_string())
    })?;

    let acro_obj = &acro_content[..endobj_pos + 6];

    if let Some(fields_pos) = acro_obj.find("/Fields") {
        let after_fields = &acro_obj[fields_pos + 7..];
        if let Some(bracket_pos) = after_fields.find('[') {
            let after_bracket = &after_fields[bracket_pos + 1..];
            if let Some(close_pos) = after_bracket.find(']') {
                let existing_fields = after_bracket[..close_pos].trim();

                let mut form = format!("{acro_form_id} 0 obj\n<<\n");
                if existing_fields.is_empty() {
                    form.push_str(&format!("/Fields [{new_field_id} 0 R]\n"));
                } else {
                    form.push_str(&format!("/Fields [{existing_fields} {new_field_id} 0 R]\n"));
                }
                form.push_str("/SigFlags 3\n");
                form.push_str(">>\nendobj\n");
                return Ok(form);
            }
        }
    }

    Err(SignatureError::SigningFailed(
        "Could not parse AcroForm /Fields".to_string(),
    ))
}

fn build_updated_page(pdf_bytes: &[u8], page_id: u32, field_id: u32) -> SignatureResult<String> {
    let content = String::from_utf8_lossy(pdf_bytes);

    let page_pattern = format!("{page_id} 0 obj");
    let page_start = content.find(&page_pattern).ok_or_else(|| {
        SignatureError::SigningFailed(format!("Could not find page object {page_id}"))
    })?;

    let page_content = &content[page_start..];
    let endobj_pos = page_content.find("endobj").ok_or_else(|| {
        SignatureError::SigningFailed("Could not find endobj for page".to_string())
    })?;

    let page_obj = &page_content[..endobj_pos + 6];

    if page_obj.contains("/Annots") {
        let annots_start = page_obj.find("/Annots").unwrap();
        let after_annots = &page_obj[annots_start + 7..];

        if let Some(bracket_pos) = after_annots.find('[') {
            let after_bracket = &after_annots[bracket_pos + 1..];
            if let Some(close_pos) = after_bracket.find(']') {
                let existing_annots = after_bracket[..close_pos].trim();

                let dict_end = page_obj.rfind(">>").ok_or_else(|| {
                    SignatureError::SigningFailed("Invalid page structure".to_string())
                })?;

                let before_annots = &page_obj[..annots_start];
                let after_close =
                    &page_obj[annots_start + 7 + bracket_pos + 1 + close_pos + 1..dict_end];

                let result =
                    format!(
                    "{page_id} 0 obj\n{}/Annots [{existing_annots} {field_id} 0 R]{}\n>>\nendobj\n",
                    before_annots
                        .trim_start_matches(&format!("{page_id} 0 obj"))
                        .trim(),
                    after_close.trim_end_matches(">>").trim_end_matches('\n').trim()
                );
                return Ok(result);
            }
        }
    }

    let dict_end = page_obj
        .rfind(">>")
        .ok_or_else(|| SignatureError::SigningFailed("Invalid page structure".to_string()))?;

    let before_end = &page_obj[..dict_end];
    Ok(format!(
        "{page_id} 0 obj\n{}/Annots [{field_id} 0 R]\n>>\nendobj\n",
        before_end
            .trim_start_matches(&format!("{page_id} 0 obj"))
            .trim()
    ))
}

fn build_updated_catalog(
    pdf_bytes: &[u8],
    catalog_id: u32,
    acro_form_id: u32,
) -> SignatureResult<String> {
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
    let dict_end = catalog_obj
        .rfind(">>")
        .ok_or_else(|| SignatureError::SigningFailed("Invalid catalog structure".to_string()))?;

    let before_end = &catalog_obj[..dict_end];
    Ok(format!(
        "{catalog_id} 0 obj\n{}/AcroForm {acro_form_id} 0 R\n>>\nendobj\n",
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

/// Finds the byte range the *new* `/DocTimeStamp`'s `/Contents` placeholder
/// leaves outside itself -- i.e. every byte of the file except the
/// placeholder's own hex digits -- mirroring
/// `signer.rs::IncrementalSigner::calculate_byte_range`.
fn calculate_byte_range(pdf_bytes: &[u8], contents_size: usize) -> SignatureResult<ByteRange> {
    let pattern = b"/Contents <";
    let mut last_pos = None;
    for i in (0..pdf_bytes.len().saturating_sub(pattern.len())).rev() {
        if &pdf_bytes[i..i + pattern.len()] == pattern {
            last_pos = Some(i);
            break;
        }
    }

    let contents_start = last_pos.ok_or_else(|| {
        SignatureError::ByteRangeError("Could not find /Contents in DocTimeStamp".to_string())
    })?;

    let hex_open = contents_start + 10;
    let expected_close = hex_open + 1 + (contents_size * 2);

    if expected_close >= pdf_bytes.len() || pdf_bytes[expected_close] != b'>' {
        return Err(SignatureError::ByteRangeError(
            "Could not find closing > for Contents".to_string(),
        ));
    }

    Ok(ByteRange::new(
        0,
        (hex_open + 1) as i64,
        expected_close as i64,
        (pdf_bytes.len() - expected_close) as i64,
    ))
}

fn update_byte_range_placeholder(
    pdf_bytes: Vec<u8>,
    byte_range: &ByteRange,
) -> SignatureResult<Vec<u8>> {
    let placeholder = b"/ByteRange [0000000000 0000000000 0000000000 0000000000]";
    let replacement = format!(
        "/ByteRange [{:010} {:010} {:010} {:010}]",
        byte_range.offset1, byte_range.length1, byte_range.offset2, byte_range.length2
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

fn extract_ranged_data(pdf_bytes: &[u8], byte_range: &ByteRange) -> Vec<u8> {
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

fn embed_contents(
    pdf_bytes: Vec<u8>,
    byte_range: &ByteRange,
    token_der: &[u8],
    contents_size: usize,
) -> SignatureResult<Vec<u8>> {
    let hex: String = token_der.iter().map(|b| format!("{b:02X}")).collect();

    let placeholder_size = contents_size * 2;
    let padded_hex = if hex.len() < placeholder_size {
        let padding = "0".repeat(placeholder_size - hex.len());
        hex + &padding
    } else if hex.len() > placeholder_size {
        return Err(SignatureError::SigningFailed(
            "RFC 3161 timestamp token too large for the reserved /Contents size".to_string(),
        ));
    } else {
        hex
    };

    let start = byte_range.length1 as usize;
    let end = byte_range.offset2 as usize;

    let mut result = Vec::with_capacity(pdf_bytes.len());
    result.extend_from_slice(&pdf_bytes[..start]);
    result.extend_from_slice(padded_hex.as_bytes());
    result.extend_from_slice(&pdf_bytes[end..]);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_matches() {
        assert_eq!(count_matches("no matches here", "/Type /DocTimeStamp"), 0);
        let doc = "/Type /DocTimeStamp ... /Type /DocTimeStamp";
        assert_eq!(count_matches(doc, "/Type /DocTimeStamp"), 2);
    }

    #[test]
    fn test_find_next_object_id() {
        let pdf = "1 0 obj\n<<>>\nendobj\n5 0 obj\n<<>>\nendobj\n";
        assert_eq!(find_next_object_id(pdf), 6);
    }

    #[test]
    fn test_build_doc_timestamp_dictionary_shape() {
        let dict = build_doc_timestamp_dictionary(7, 128);
        assert!(dict.starts_with("7 0 obj"));
        assert!(dict.contains("/Type /DocTimeStamp"));
        assert!(dict.contains("/SubFilter /ETSI.RFC3161"));
        assert!(dict.contains("/Filter /Adobe.PPKLite"));
        // 128 reserved bytes -> 256 hex digits between the angle brackets.
        assert!(dict.contains(&format!("/Contents <{}>", "0".repeat(256))));
    }

    #[test]
    fn test_build_doc_timestamp_field_shape() {
        let field = build_doc_timestamp_field(2, 1, 3, 4, "DocTimeStamp1");
        assert!(field.contains("/FT /Sig"));
        assert!(field.contains("/V 1 0 R"));
        assert!(field.contains("/P 4 0 R"));
        assert!(field.contains("/AP << /N 3 0 R >>"));
        assert!(field.contains("/T (DocTimeStamp1)"));
    }

    #[test]
    fn test_embed_document_timestamp_sized_rejects_missing_startxref() {
        // No `/timestamp_authority` needed to hit this error path -- it
        // fails during `parse_pdf_info`, before any request is built.
        #[derive(Debug)]
        struct UnusedClient;
        impl TimestampAuthorityClient for UnusedClient {
            fn timestamp(&self, _tsq_der: &[u8]) -> SignatureResult<Vec<u8>> {
                unreachable!("must not be called")
            }
        }

        let client: Arc<dyn TimestampAuthorityClient> = Arc::new(UnusedClient);
        let err = embed_document_timestamp(
            b"%PDF-1.7\n%%EOF".to_vec(),
            &client,
            SignatureAlgorithm::RsaSha256,
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::SigningFailed(_)));
    }
}
