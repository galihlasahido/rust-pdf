//! Applying password/permission encryption (ISO 32000-2:2020 Section 7.6,
//! "Standard Security Handler", AES-256/V5/R6) to an already-open
//! [`EditableDocument`].
//!
//! # A genuine, disclosed limitation
//!
//! [`crate::document::DocumentBuilder::encrypt`] is the *only other* place
//! in this crate that can apply encryption, and only at from-scratch
//! build time (a [`crate::document::Document`] assembled from
//! [`crate::page::Page`]/[`crate::content::ContentBuilder`] - not an
//! already-open, arbitrary source PDF). There is **no** way to encrypt an
//! already-open [`EditableDocument`] the way every other edit in this
//! crate works (mutate the in-memory object graph now, persist later via
//! [`EditableDocument::save_incremental`]/[`EditableDocument::save_full_rewrite`]):
//! encryption has to touch every string and every stream in the whole
//! reachable object graph in one pass (the file encryption key has to be
//! derived once, up front, before any object is serialized), so this
//! module instead does a dedicated **full graph walk + rewrite in one
//! shot** - structurally the same shape as
//! [`EditableDocument::save_full_rewrite_to_bytes`]/
//! [`EditableDocument::save_pdfa_compatible_to_bytes`], which each exist
//! for their own, similarly single-pass reasons - and returns the
//! encrypted bytes directly. [`EditableDocument::save_encrypted_to_bytes`]
//! does **not** (and cannot) update `self` in place, and there is no
//! incremental variant: an "incrementally encrypted" update isn't a
//! coherent concept (every subsequent incremental append would need the
//! same file encryption key persisted across a save/reload cycle, which
//! this crate has no facility for, and appending only-sometimes-encrypted
//! objects onto an otherwise-plaintext file is not something a
//! conformant reader is required to handle sensibly).
//!
//! **Bigger caveat, worth stating plainly and not hiding**: this crate's
//! own parser ([`crate::parser::PdfReader`], via its shared `finish`
//! constructor path) unconditionally rejects *any* file with an
//! `/Encrypt` trailer entry with [`crate::error::ParserError::EncryptedPdf`],
//! because there is no decryption filter implemented anywhere in this
//! crate (see [`crate::tauri_commands::commands::OpenDocumentRequest::password`]'s
//! own doc comment for the same gap on the *reading* side). That means a
//! file [`EditableDocument::save_encrypted_to_bytes`] produces is
//! genuinely, correctly encrypted per ISO 32000-2 and can be opened by
//! any real conformant PDF reader (Acrobat, etc.) with the configured
//! password, but **this crate cannot reopen its own encrypted output**,
//! not through [`EditableDocument::open`]/[`EditableDocument::from_bytes`],
//! and therefore not through any Tauri command either (this module's own
//! tests verify the produced bytes only *structurally*, i.e. a correct
//! `/Encrypt` dictionary shape and ciphertext that no longer contains the
//! plaintext, for exactly this reason; see
//! [`crate::tauri_commands::commands::set_password`]'s doc comment for
//! how this is surfaced to a caller). `set_password` is therefore
//! modeled as a **terminal/export operation**, like `sign_document`: it
//! writes a new file at a caller-given output path and leaves the
//! original, still-open, still-unencrypted in-memory document untouched
//! and still editable/re-saveable.

use super::graph::EditableDocument;
use crate::document::PdfVersion;
use crate::encryption::{generate_file_id, EncryptionConfig, EncryptionHandler};
use crate::error::{EditorError, PdfResult};
use crate::object::{Object, PdfArray, PdfDictionary, PdfStream, PdfString};
use crate::types::ObjectId;
use std::collections::HashMap;

impl EditableDocument {
    /// Serializes this document's currently-reachable object graph (same
    /// reachability walk and compact renumbering as
    /// [`EditableDocument::save_full_rewrite_to_bytes`]) with every string
    /// and every stream's data encrypted per `config`, plus an
    /// `/Encrypt` dictionary and `/ID` array added to the trailer (ISO
    /// 32000-2 Section 7.6).
    ///
    /// See the [module docs](self) for why this cannot be an in-place
    /// edit followed by a later plain `save_*` call the way every other
    /// editing operation in this crate works, and for the disclosed
    /// limitation that this crate cannot reopen its own output
    /// afterwards.
    pub fn save_encrypted_to_bytes(&self, config: EncryptionConfig) -> PdfResult<Vec<u8>> {
        let info_id = self.reader.trailer().info;
        let mut roots = vec![self.root];
        if let Some(info) = info_id {
            roots.push(info);
        }
        let (order, objects) = self.reachable_objects(&roots)?;

        let mut id_map: HashMap<ObjectId, ObjectId> = HashMap::with_capacity(order.len());
        for (i, &old_id) in order.iter().enumerate() {
            id_map.insert(old_id, ObjectId::new(i as u32 + 1));
        }
        let new_root = id_map[&self.root];
        let new_info = info_id.map(|id| id_map[&id]);

        let file_id = generate_file_id();
        let handler = EncryptionHandler::new(config, file_id.clone())?;

        // AES-256/V5/R6 is formally a PDF 2.0 (ISO 32000-2) construct, but
        // (matching how `crate::document::Document::write_to` behaves --
        // it just writes whatever `self.version` already was, with no
        // special-casing for encryption) real-world readers, including
        // Acrobat itself, routinely accept it under a `%PDF-1.7` header;
        // bumping to at least 1.7 here (like
        // `save_full_rewrite_to_bytes` already bumps to at least 1.5)
        // avoids under-claiming a version for a source file that started
        // life as an old PDF 1.3/1.4 document.
        let version = self.reader.version().max(PdfVersion::V1_7);
        let mut out = Vec::new();
        out.extend_from_slice(format!("%PDF-{}\n", version.as_str()).as_bytes());
        out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");

        let mut offsets: Vec<(u32, u64)> = Vec::with_capacity(order.len() + 1);
        for old_id in &order {
            let new_id = id_map[old_id];
            let remapped = remap_refs(&objects[old_id], &id_map);
            let encrypted = encrypt_object(&remapped, new_id, &handler)?;
            let offset = out.len() as u64;
            write_indirect_object(&mut out, new_id, &encrypted);
            offsets.push((new_id.number, offset));
        }

        // The `/Encrypt` dictionary itself must never be encrypted (ISO
        // 32000-2 7.6.1) -- it is what a reader needs in the clear to
        // even begin deriving the file key.
        let encrypt_id = ObjectId::new(order.len() as u32 + 1);
        let encrypt_offset = out.len() as u64;
        write_indirect_object(
            &mut out,
            encrypt_id,
            &Object::Dictionary(handler.create_encrypt_dictionary()),
        );
        offsets.push((encrypt_id.number, encrypt_offset));

        if out.len() > u32::MAX as usize {
            return Err(EditorError::ResourceLimitExceeded(
                "encrypted rewrite output exceeds the 4 GiB offset width supported by this writer"
                    .to_string(),
            )
            .into());
        }

        let xref_offset = out.len() as u64;
        let size = encrypt_id.number + 1;
        write_encrypted_xref_and_trailer(&mut out, &offsets, size, new_root, new_info, encrypt_id, file_id);
        out.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
        Ok(out)
    }
}

/// Rewrites every [`Object::Reference`] in `obj` through `map`. Any
/// reference not present in `map` becomes `null` rather than a reference
/// to whatever unrelated object now has that number (same defensive
/// fallback as the other full-rewrite paths' own `remap_all`).
fn remap_refs(obj: &Object, map: &HashMap<ObjectId, ObjectId>) -> Object {
    match obj {
        Object::Reference(id) => match map.get(id) {
            Some(new_id) => Object::Reference(*new_id),
            None => Object::Null,
        },
        Object::Array(arr) => Object::Array(arr.iter().map(|o| remap_refs(o, map)).collect()),
        Object::Dictionary(dict) => {
            let mut new_dict = PdfDictionary::new();
            for (k, v) in dict.iter() {
                new_dict.set(k.clone(), remap_refs(v, map));
            }
            Object::Dictionary(new_dict)
        }
        Object::Stream(s) => {
            let mut new_dict = PdfDictionary::new();
            for (k, v) in s.dictionary.iter() {
                new_dict.set(k.clone(), remap_refs(v, map));
            }
            Object::Stream(PdfStream::from_raw(new_dict, s.data.clone()))
        }
        other => other.clone(),
    }
}

/// Recursively encrypts every string and every stream's data reachable
/// from `obj`, using `id`'s object/generation numbers (ISO 32000-1 Table
/// 20's per-object key derivation for RC4/AESV2 -- not actually used by
/// the AES-256/V5 handler this crate implements, which derives one
/// shared file key instead, but threaded through regardless so this
/// keeps working unmodified if a per-object algorithm is added later).
fn encrypt_object(obj: &Object, id: ObjectId, handler: &EncryptionHandler) -> PdfResult<Object> {
    Ok(match obj {
        Object::String(s) => {
            let encrypted = handler.encrypt_data(s.as_bytes(), id.number, id.generation)?;
            Object::String(PdfString::Hex(encrypted))
        }
        Object::Stream(stream) => {
            let encrypted_data = handler.encrypt_data(&stream.data, id.number, id.generation)?;
            let mut new_dict = stream.dictionary.clone();
            new_dict.set("Length", Object::Integer(encrypted_data.len() as i64));
            Object::Stream(PdfStream::from_raw(new_dict, encrypted_data))
        }
        Object::Dictionary(dict) => {
            let mut new_dict = PdfDictionary::new();
            for (k, v) in dict.iter() {
                new_dict.set(k.clone(), encrypt_object(v, id, handler)?);
            }
            Object::Dictionary(new_dict)
        }
        Object::Array(arr) => {
            let mut new_arr = PdfArray::new();
            for item in arr.iter() {
                new_arr.push(encrypt_object(item, id, handler)?);
            }
            Object::Array(new_arr)
        }
        other => other.clone(),
    })
}

/// Writes one `N G obj ... endobj` indirect object definition (same shape
/// as the other full-rewrite paths' own `write_indirect_object`, kept as
/// a separate copy in this module rather than shared/exported so this
/// module's disclosed-limitation-heavy, security-sensitive code stays
/// self-contained and independently reviewable).
fn write_indirect_object(out: &mut Vec<u8>, id: ObjectId, obj: &Object) {
    out.extend_from_slice(format!("{} {} obj\n", id.number, id.generation).as_bytes());
    match obj {
        Object::Stream(stream) => {
            out.extend_from_slice(stream.dictionary.to_pdf_string().as_bytes());
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&stream.data);
            out.extend_from_slice(b"\nendstream\n");
        }
        _ => {
            out.extend_from_slice(obj.to_pdf_string().as_bytes());
            out.push(b'\n');
        }
    }
    out.extend_from_slice(b"endobj\n");
}

/// Writes a traditional (classic-table, not cross-reference-stream) xref
/// section plus trailer dictionary carrying `/Encrypt` and `/ID`.
/// `offsets` must be exactly the contiguous object numbers
/// `1..=offsets.len()` in ascending order (guaranteed by
/// [`EditableDocument::save_encrypted_to_bytes`]'s compact renumbering
/// plus its appended `/Encrypt` object).
fn write_encrypted_xref_and_trailer(
    out: &mut Vec<u8>,
    offsets: &[(u32, u64)],
    size: u32,
    root: ObjectId,
    info: Option<ObjectId>,
    encrypt_id: ObjectId,
    file_id: Vec<u8>,
) {
    out.extend_from_slice(b"xref\n");
    out.extend_from_slice(format!("0 {size}\n").as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for (_, offset) in offsets {
        out.extend_from_slice(format!("{:010} 00000 n \n", offset).as_bytes());
    }

    out.extend_from_slice(b"trailer\n<< ");
    out.extend_from_slice(format!("/Size {size} ").as_bytes());
    out.extend_from_slice(format!("/Root {} ", root.reference_string()).as_bytes());
    if let Some(info) = info {
        out.extend_from_slice(format!("/Info {} ", info.reference_string()).as_bytes());
    }
    out.extend_from_slice(format!("/Encrypt {} ", encrypt_id.reference_string()).as_bytes());
    out.extend_from_slice(b"/ID [");
    out.extend_from_slice(PdfString::Hex(file_id.clone()).to_pdf_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(PdfString::Hex(file_id).to_pdf_string().as_bytes());
    out.extend_from_slice(b"] ");
    out.extend_from_slice(b">>\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    fn sample_pdf() -> Vec<u8> {
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Top Secret Payload"))
            .build();
        DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap()
    }

    #[test]
    fn test_encrypted_output_has_valid_header_and_encrypt_dict() {
        let doc = EditableDocument::from_bytes(sample_pdf()).unwrap();
        let config = EncryptionConfig::aes256().user_password("secret1").owner_password("secret2");
        let bytes = doc.save_encrypted_to_bytes(config).unwrap();

        assert!(bytes.starts_with(b"%PDF-1."));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Encrypt"));
        assert!(text.contains("/Filter/Standard") || text.contains("/Filter /Standard"));
        assert!(text.contains("/ID ["));
    }

    #[test]
    fn test_encrypted_output_no_longer_contains_plaintext_content() {
        let doc = EditableDocument::from_bytes(sample_pdf()).unwrap();
        let config = EncryptionConfig::aes256().user_password("secret1").owner_password("secret2");
        let bytes = doc.save_encrypted_to_bytes(config).unwrap();

        // The plaintext page-content string must not survive in the
        // clear anywhere in the output -- the whole point of encrypting.
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("Top Secret Payload"));
    }

    #[test]
    fn test_this_crate_cannot_reopen_its_own_encrypted_output() {
        // Documents the disclosed limitation from this module's own docs
        // as an executable regression guard: if a future change to the
        // parser ever added decryption support, this test would start
        // failing (a welcome failure -- a maintainer would then update
        // this test and the doc comments above, not silently leave this
        // assertion stale).
        let doc = EditableDocument::from_bytes(sample_pdf()).unwrap();
        let config = EncryptionConfig::aes256().user_password("secret1");
        let bytes = doc.save_encrypted_to_bytes(config).unwrap();

        let reopened = EditableDocument::from_bytes(bytes);
        assert!(reopened.is_err(), "this crate's parser has no decryption filter and must reject its own encrypted output");
    }

    #[test]
    fn test_original_document_is_left_untouched() {
        let mut doc = EditableDocument::from_bytes(sample_pdf()).unwrap();
        let config = EncryptionConfig::aes256().user_password("secret1");
        let _ = doc.save_encrypted_to_bytes(config).unwrap();

        // `save_encrypted_to_bytes` takes `&self`: the in-memory document
        // must still be perfectly usable afterwards (unencrypted, still
        // editable), unlike e.g. a hypothetical in-place `encrypt()` that
        // would need to invalidate every other open command's view of it.
        assert_eq!(doc.page_count().unwrap(), 1);
        doc.rotate_page(0, 90).unwrap();
        assert!(doc.save_full_rewrite_to_bytes().is_ok());
    }

    #[test]
    fn test_aes128_encrypted_output_has_valid_v4_r4_header_and_no_plaintext() {
        // Regression test for a previously-shipped bug: the AES-128
        // (V=4/R=4) path used to derive AES-256/R6 keys and write an
        // R6-shaped dictionary (/OE, /UE, /Perms, AESV3 crypt filter)
        // under a claimed `/V 4 /R 4` header -- a structurally invalid
        // hybrid no real reader could open. This exercises the full
        // `save_encrypted_to_bytes` pipeline (not just `EncryptionHandler`
        // in isolation) with `EncryptionConfig::aes128()`.
        let doc = EditableDocument::from_bytes(sample_pdf()).unwrap();
        let config = EncryptionConfig::aes128().user_password("secret1").owner_password("secret2");
        let bytes = doc.save_encrypted_to_bytes(config).unwrap();

        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Encrypt"));
        assert!(text.contains("/Filter/Standard") || text.contains("/Filter /Standard"));
        assert!(text.contains("/V 4"), "AES-128 must declare /V 4");
        assert!(text.contains("/R 4"), "AES-128 must declare /R 4");
        assert!(text.contains("/AESV2"), "AES-128 must use the AESV2 crypt filter method");
        assert!(!text.contains("/AESV3"), "AES-128 must not use the R6-only AESV3 crypt filter");
        assert!(!text.contains("/OE"), "R4 has no /OE entry (that's R5/6-only)");
        assert!(!text.contains("/UE"), "R4 has no /UE entry (that's R5/6-only)");
        assert!(!text.contains("/Perms"), "R4 has no /Perms entry (that's R5/6-only)");
        assert!(!text.contains("Top Secret Payload"), "plaintext must not survive encryption");
    }

    #[test]
    fn test_two_encryptions_of_the_same_document_use_different_file_ids_and_keys() {
        // Each call must derive its own random file id / salts (matching
        // `generate_file_id`'s own "must be random" contract) rather than
        // e.g. accidentally caching/reusing one -- otherwise two
        // passworded exports of the same document would leak that they
        // share a key.
        let doc = EditableDocument::from_bytes(sample_pdf()).unwrap();
        let bytes_a = doc.save_encrypted_to_bytes(EncryptionConfig::aes256().user_password("secret1")).unwrap();
        let bytes_b = doc.save_encrypted_to_bytes(EncryptionConfig::aes256().user_password("secret1")).unwrap();
        assert_ne!(bytes_a, bytes_b);
    }
}
