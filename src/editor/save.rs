//! Persisting an [`EditableDocument`]'s edits: either as an incremental
//! update (ISO 32000-1:2008 Section 7.5.6) or a full, compacted rewrite
//! using object streams and a cross-reference stream (Sections 7.5.7 /
//! 7.5.8).

use super::graph::EditableDocument;
use crate::document::PdfVersion;
use crate::error::{EditorError, PdfResult};
use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream};
use crate::types::ObjectId;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Objects packed per `/ObjStm` container (ISO 32000-1 7.5.7) in a full
/// rewrite. Bounded (rather than one giant object stream) so a single
/// container's decompressed size, and the cost of re-decoding it to
/// fetch one object, stays reasonable.
const OBJECTS_PER_STREAM: usize = 200;

impl EditableDocument {
    /// Saves this document's edits as an incremental update (ISO 32000-1
    /// 7.5.6): the original file's bytes are untouched, and only new or
    /// modified objects plus a new cross-reference section and trailer
    /// (chained to the original via `/Prev`) are appended. Cost is
    /// proportional to the size of the *edit*, not the size of the
    /// document, which is what makes this fast enough for interactive
    /// "save after one change" use even on large documents.
    ///
    /// A no-op edit (nothing recorded in the overlay) writes the original
    /// bytes back out unchanged.
    ///
    /// Returns [`EditorError::RedactionRequiresFullRewrite`] if any
    /// `redact_*`/`strip_document_metadata` call
    /// ([`crate::editor::redact`]) has run this session - see that
    /// module's docs for why an incremental update cannot make a
    /// redaction permanent.
    pub fn save_incremental(&self, path: impl AsRef<Path>) -> PdfResult<()> {
        let bytes = self.save_incremental_to_bytes()?;
        let mut file = File::create(path)?;
        file.write_all(&bytes)?;
        Ok(())
    }

    /// Like [`EditableDocument::save_incremental`], returning the bytes
    /// instead of writing them to a file.
    pub fn save_incremental_to_bytes(&self) -> PdfResult<Vec<u8>> {
        if self.redaction_applied {
            return Err(EditorError::RedactionRequiresFullRewrite.into());
        }
        let mut out = self.reader.raw_data().to_vec();
        if self.overlay.is_empty() {
            return Ok(out);
        }
        if !out.ends_with(b"\n") {
            out.push(b'\n');
        }

        // ISO 32000-1 7.5.6: the new update's trailer must chain back to
        // the base file's own most recent cross-reference section via
        // /Prev, so that a reader walking the whole history still finds
        // it (even though our new section only lists what changed).
        let prev_xref = crate::parser::find_startxref(self.reader.raw_data())
            .map_err(|_| EditorError::MissingBaseXref)?;

        let mut offsets: Vec<(u32, u64)> = Vec::with_capacity(self.overlay.len());
        for (&id, obj) in self.overlay.iter() {
            let offset = out.len() as u64;
            write_indirect_object(&mut out, id, obj);
            offsets.push((id.number, offset));
        }
        offsets.sort_unstable_by_key(|(num, _)| *num);

        let xref_offset = out.len() as u64;
        write_incremental_xref_and_trailer(
            &mut out,
            &offsets,
            self.next_object_number,
            self.root,
            self.reader.trailer().info,
            prev_xref,
        );
        out.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
        Ok(out)
    }

    /// Saves this document as a full, compacted rewrite: only objects
    /// still reachable from the trailer's `/Root` (and `/Info`, if
    /// present) are written - so anything an edit orphaned (e.g. a
    /// deleted page's content stream, or an intermediate `/Pages` node
    /// left behind by [`EditableDocument::insert_blank_page`]'s page-tree
    /// flattening) is dropped instead of accumulating as dead weight -
    /// object numbers are compacted to a dense `1..=N` range, and objects
    /// are packed into FlateDecode-compressed object streams (ISO
    /// 32000-1 7.5.7) with a single compressed cross-reference stream
    /// (7.5.8) rather than a verbose plain-text xref table. This is
    /// slower than [`EditableDocument::save_incremental`] (it walks and
    /// re-serializes the whole reachable graph) but produces the
    /// smallest, cleanest output, so use it before long-term storage or
    /// distribution rather than after every single edit.
    pub fn save_full_rewrite(&self, path: impl AsRef<Path>) -> PdfResult<()> {
        let bytes = self.save_full_rewrite_to_bytes()?;
        let mut file = File::create(path)?;
        file.write_all(&bytes)?;
        Ok(())
    }

    /// Like [`EditableDocument::save_full_rewrite`], returning the bytes
    /// instead of writing them to a file.
    pub fn save_full_rewrite_to_bytes(&self) -> PdfResult<Vec<u8>> {
        let info_id = self.reader.trailer().info;

        // 1. Reachability: walk from /Root (+/Info) so anything orphaned
        //    by an edit is dropped.
        let mut roots = vec![self.root];
        if let Some(info) = info_id {
            roots.push(info);
        }
        let (order, objects) = self.reachable_objects(&roots)?;

        // 2. Compact renumbering: id_map[old] = new, assigned in visita-
        //    tion (BFS) order starting at 1.
        let mut id_map: HashMap<ObjectId, ObjectId> = HashMap::with_capacity(order.len());
        for (i, &old_id) in order.iter().enumerate() {
            id_map.insert(old_id, ObjectId::new(i as u32 + 1));
        }
        let new_root = id_map[&self.root];
        let new_info = info_id.map(|id| id_map[&id]);

        // 3. Remap every object's internal references, and split into
        //    streams (which must stay ordinary indirect objects, ISO
        //    32000-1 7.5.7) vs. everything else (packed into /ObjStm
        //    containers).
        let mut stream_objs: Vec<(u32, PdfStream)> = Vec::new();
        let mut plain_objs: Vec<(u32, Object)> = Vec::new();
        for old_id in &order {
            let new_num = id_map[old_id].number;
            let remapped = remap_all(&objects[old_id], &id_map);
            match remapped {
                Object::Stream(s) => {
                    let s = if s.is_compressed() {
                        s
                    } else {
                        // Best-effort: shrink previously-uncompressed
                        // streams (typically page content streams).
                        s.with_compression().unwrap_or_else(|_| {
                            // Compression cannot fail for in-memory Vec<u8>
                            // output in practice; fall back to
                            // uncompressed rather than lose the object if
                            // it somehow did.
                            PdfStream::new(Vec::new())
                        })
                    };
                    stream_objs.push((new_num, s));
                }
                other => plain_objs.push((new_num, other)),
            }
        }

        let highest_content_num = order.len() as u32;
        let mut next_num = highest_content_num + 1;

        // 4. Header.
        let version = self.reader.version().max(PdfVersion::V1_5);
        let mut out = Vec::new();
        out.extend_from_slice(format!("%PDF-{}\n", version.as_str()).as_bytes());
        out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");

        // 5. Write every stream object as an ordinary indirect object,
        //    recording its offset for the xref stream.
        let mut xref: HashMap<u32, XrefTarget> = HashMap::with_capacity(order.len() + 16);
        for (num, stream) in &stream_objs {
            let offset = out.len() as u64;
            write_stream_object(&mut out, ObjectId::new(*num), stream);
            xref.insert(*num, XrefTarget::Offset(offset));
        }

        // 6. Pack every non-stream object into compressed /ObjStm
        //    containers, OBJECTS_PER_STREAM at a time.
        for chunk in plain_objs.chunks(OBJECTS_PER_STREAM) {
            let objstm_num = next_num;
            next_num += 1;

            let mut header = String::new();
            let mut body = Vec::new();
            for (i, (num, obj)) in chunk.iter().enumerate() {
                let rel_offset = body.len();
                header.push_str(&format!("{num} {rel_offset} "));
                body.extend_from_slice(obj.to_pdf_string().as_bytes());
                body.push(b' ');
                xref.insert(*num, XrefTarget::Compressed { stream: objstm_num, index: i as u32 });
            }
            let first = header.len() as i64;
            let mut full_data = header.into_bytes();
            full_data.extend_from_slice(&body);

            let mut dict = PdfDictionary::new();
            dict.set("Type", Object::Name(PdfName::new_unchecked("ObjStm")));
            dict.set("N", Object::Integer(chunk.len() as i64));
            dict.set("First", Object::Integer(first));
            let stream = PdfStream::with_dictionary(dict, full_data)
                .with_compression()
                .unwrap_or_else(|_| PdfStream::new(Vec::new()));

            let offset = out.len() as u64;
            write_stream_object(&mut out, ObjectId::new(objstm_num), &stream);
            xref.insert(objstm_num, XrefTarget::Offset(offset));
        }

        // 7. Cross-reference stream (ISO 32000-1 7.5.8), describing every
        //    object including itself.
        let xref_stream_num = next_num;
        let xref_offset = out.len() as u64;
        xref.insert(xref_stream_num, XrefTarget::Offset(xref_offset));

        // The xref stream's W=[1 4 2] encodes byte offsets in 4 bytes;
        // refuse to silently truncate (and thus corrupt) an offset that
        // has grown past u32::MAX rather than writing a wrapped value.
        if out.len() > u32::MAX as usize {
            return Err(EditorError::ResourceLimitExceeded(
                "full-rewrite output exceeds the 4 GiB offset width supported by this writer"
                    .to_string(),
            )
            .into());
        }

        let size = xref_stream_num + 1;
        let xref_stream = build_xref_stream(size, &xref, new_root, new_info, self.reader.trailer().id.clone());
        write_stream_object(&mut out, ObjectId::new(xref_stream_num), &xref_stream);

        out.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
        Ok(out)
    }

    /// Like [`EditableDocument::save_full_rewrite_to_bytes`] (same
    /// reachability walk, same compact renumbering, same "drop anything
    /// orphaned by an edit" behaviour), but deliberately avoids every
    /// PDF-1.5+-only construct: no compressed object streams (`/ObjStm`,
    /// ISO 32000-1 7.5.7) and no cross-reference *stream* (`/Type /XRef`,
    /// 7.5.8) - every object is written as an ordinary indirect object and
    /// the file ends in a traditional `xref`/`trailer` section (7.5.4),
    /// exactly like a hand-authored PDF 1.4 file would.
    ///
    /// This is what [`crate::editor::pdfa::PdfAFlavor::Part1B`] conversion
    /// uses to write its output: ISO 19005-1:2005 (PDF/A-1) is defined in
    /// terms of the PDF 1.4 Reference, which predates both object streams
    /// and cross-reference streams, so a PDF/A-1 file must not contain
    /// them even though [`crate::editor::EditableDocument::save_full_rewrite_to_bytes`]
    /// would otherwise always prefer them for size. PDF/A-2 and PDF/A-3
    /// are defined against ISO 32000-1 (PDF 1.7) and do permit both, but
    /// this writer is also used for those flavors for simplicity (a
    /// classic xref table is always valid PDF 1.7 too, just less compact).
    /// See the effort-estimate note in `crate::editor::pdfa` for why a
    /// second, ObjStm-using PDF/A-2/3-specific writer was not built in
    /// this pass.
    ///
    /// `version` is written verbatim into the `%PDF-x.y` header (the
    /// caller - [`crate::editor::pdfa::PdfAFlavor::min_pdf_version`] -
    /// decides which version each flavor requires); unlike
    /// `save_full_rewrite_to_bytes`, this method does **not** silently
    /// bump it to 1.5, since doing so would itself be a PDF/A-1
    /// nonconformance.
    pub fn save_pdfa_compatible_to_bytes(&self, version: PdfVersion) -> PdfResult<Vec<u8>> {
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

        let mut out = Vec::new();
        out.extend_from_slice(format!("%PDF-{}\n", version.as_str()).as_bytes());
        out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");

        let mut offsets: Vec<(u32, u64)> = Vec::with_capacity(order.len());
        for old_id in &order {
            let new_id = id_map[old_id];
            let remapped = remap_all(&objects[old_id], &id_map);
            // Content streams still benefit from FlateDecode (a plain
            // filter, not an ObjStm) - only the *container* format
            // (classic table vs. compressed stream) is what PDF/A-1
            // restricts. The XML metadata stream (ISO 32000-1 14.3.2) is
            // the one stream PDF/A explicitly forbids compressing (ISO
            // 19005-1:2005 6.7.2 - a validator/indexer must be able to
            // find the PDF/A identification schema without running a
            // Flate decoder first), so it's left exactly as
            // [`crate::editor::xmp::EditableDocument::set_xmp_metadata`]
            // built it (uncompressed) regardless of this method's usual
            // "compress every plain stream" behaviour.
            let remapped = match remapped {
                Object::Stream(s) if !s.is_compressed() && !is_metadata_stream(&s.dictionary) => {
                    Object::Stream(s.with_compression().unwrap_or_else(|_| PdfStream::new(Vec::new())))
                }
                other => other,
            };
            let offset = out.len() as u64;
            write_indirect_object(&mut out, new_id, &remapped);
            offsets.push((new_id.number, offset));
        }

        if out.len() > u32::MAX as usize {
            return Err(EditorError::ResourceLimitExceeded(
                "PDF/A-compatible rewrite output exceeds the 4 GiB offset width supported by this writer"
                    .to_string(),
            )
            .into());
        }

        let xref_offset = out.len() as u64;
        let size = order.len() as u32 + 1;
        // ISO 19005-1:2005 6.1.3 (via the base PDF Reference's 3.4.4/
        // 3.4.5) requires the trailer to carry an `/ID`; generate one if
        // the source document never had one (e.g. it was itself built by
        // `DocumentBuilder`, which doesn't set one) rather than silently
        // omitting a required key.
        let file_id = self.reader.trailer().id.clone().unwrap_or_else(|| {
            let id = generate_file_id_bytes(out.len());
            (id.clone(), id)
        });
        write_classic_xref_and_trailer(&mut out, &offsets, size, new_root, new_info, Some(file_id));
        out.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
        Ok(out)
    }
}

/// `true` if `dict` is a `/Type /Metadata` stream dictionary (ISO
/// 32000-1 14.3.2, Table 315).
fn is_metadata_stream(dict: &PdfDictionary) -> bool {
    matches!(dict.get("Type"), Some(Object::Name(n)) if n.as_str() == "Metadata")
}

/// A dependency-free (no `encryption` feature, no extra crate), non-
/// cryptographic 16-byte file identifier for
/// [`EditableDocument::save_pdfa_compatible_to_bytes`]'s `/ID` fallback.
/// Unlike the `encryption` feature's own file-id generator (which needs
/// real randomness because it feeds key derivation), a plain `/ID` only
/// needs to be "practically unique per save", not unpredictable, so
/// hashing wall-clock time, the process id and the output size so far is
/// sufficient here without pulling in the `rand`/`encryption` feature.
fn generate_file_id_bytes(output_len_so_far: usize) -> Vec<u8> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut h1 = DefaultHasher::new();
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos().hash(&mut h1);
    output_len_so_far.hash(&mut h1);
    let a = h1.finish();

    let mut h2 = DefaultHasher::new();
    a.hash(&mut h2);
    std::process::id().hash(&mut h2);
    output_len_so_far.wrapping_mul(2654435761).hash(&mut h2);
    let b = h2.finish();

    let mut id = Vec::with_capacity(16);
    id.extend_from_slice(&a.to_be_bytes());
    id.extend_from_slice(&b.to_be_bytes());
    id
}

/// Where an object ended up after a full rewrite, for xref-stream
/// purposes (ISO 32000-1 Table 18).
enum XrefTarget {
    /// Type 1: an ordinary indirect object at this byte offset.
    Offset(u64),
    /// Type 2: packed inside object-stream number `stream` at `index`.
    Compressed { stream: u32, index: u32 },
}

/// Rewrites every [`Object::Reference`] in `obj` through `map`. Any
/// reference not present in `map` (should not happen for a graph that
/// was itself collected by following every reference, but kept as a
/// defensive fallback against surprising input) becomes `null` rather
/// than a reference to whatever unrelated object now has that number.
fn remap_all(obj: &Object, map: &HashMap<ObjectId, ObjectId>) -> Object {
    match obj {
        Object::Reference(id) => match map.get(id) {
            Some(new_id) => Object::Reference(*new_id),
            None => Object::Null,
        },
        Object::Array(arr) => Object::Array(arr.iter().map(|o| remap_all(o, map)).collect()),
        Object::Dictionary(dict) => {
            let mut new_dict = PdfDictionary::new();
            for (k, v) in dict.iter() {
                new_dict.set(k.clone(), remap_all(v, map));
            }
            Object::Dictionary(new_dict)
        }
        Object::Stream(s) => {
            let mut new_dict = PdfDictionary::new();
            for (k, v) in s.dictionary.iter() {
                new_dict.set(k.clone(), remap_all(v, map));
            }
            Object::Stream(PdfStream::from_raw(new_dict, s.data.clone()))
        }
        other => other.clone(),
    }
}

/// Writes one `N G obj ... endobj` indirect object definition.
fn write_indirect_object(out: &mut Vec<u8>, id: ObjectId, obj: &Object) {
    out.extend_from_slice(format!("{} {} obj\n", id.number, id.generation).as_bytes());
    match obj {
        Object::Stream(stream) => write_stream_body(out, stream),
        _ => {
            out.extend_from_slice(obj.to_pdf_string().as_bytes());
            out.push(b'\n');
        }
    }
    out.extend_from_slice(b"endobj\n");
}

/// Writes one `N G obj ... endobj` indirect *stream* object definition
/// without requiring the caller to wrap `stream` in an owned [`Object`]
/// (avoiding a clone of potentially large stream data).
fn write_stream_object(out: &mut Vec<u8>, id: ObjectId, stream: &PdfStream) {
    out.extend_from_slice(format!("{} {} obj\n", id.number, id.generation).as_bytes());
    write_stream_body(out, stream);
    out.extend_from_slice(b"endobj\n");
}

fn write_stream_body(out: &mut Vec<u8>, stream: &PdfStream) {
    out.extend_from_slice(stream.dictionary.to_pdf_string().as_bytes());
    out.extend_from_slice(b"\nstream\n");
    out.extend_from_slice(&stream.data);
    out.extend_from_slice(b"\nendstream\n");
}

/// Writes a traditional-format xref section (ISO 32000-1 7.5.4) covering
/// exactly the touched object numbers in `offsets` (already sorted,
/// ascending, unique), grouped into contiguous subsections, plus the
/// trailer dictionary chaining back to `prev` via `/Prev`.
fn write_incremental_xref_and_trailer(
    out: &mut Vec<u8>,
    offsets: &[(u32, u64)],
    size: u32,
    root: ObjectId,
    info: Option<ObjectId>,
    prev: u64,
) {
    out.extend_from_slice(b"xref\n");
    let mut i = 0;
    while i < offsets.len() {
        let start = offsets[i].0;
        let mut j = i;
        while j + 1 < offsets.len() && offsets[j + 1].0 == offsets[j].0 + 1 {
            j += 1;
        }
        out.extend_from_slice(format!("{} {}\n", start, j - i + 1).as_bytes());
        for entry in &offsets[i..=j] {
            out.extend_from_slice(format!("{:010} 00000 n \n", entry.1).as_bytes());
        }
        i = j + 1;
    }

    out.extend_from_slice(b"trailer\n<< ");
    out.extend_from_slice(format!("/Size {size} ").as_bytes());
    out.extend_from_slice(format!("/Root {} ", root.reference_string()).as_bytes());
    if let Some(info) = info {
        out.extend_from_slice(format!("/Info {} ", info.reference_string()).as_bytes());
    }
    out.extend_from_slice(format!("/Prev {prev} ").as_bytes());
    out.extend_from_slice(b">>\n");
}

/// Writes a traditional, full (not incremental) xref section (ISO
/// 32000-1 7.5.4) plus trailer dictionary for
/// [`EditableDocument::save_pdfa_compatible_to_bytes`]. `offsets` must be
/// exactly the contiguous object numbers `1..=offsets.len()` in ascending
/// order (guaranteed by that method's compact renumbering) so this can
/// emit them as one `0 size` subsection headed by the conventional free
/// object 0.
fn write_classic_xref_and_trailer(
    out: &mut Vec<u8>,
    offsets: &[(u32, u64)],
    size: u32,
    root: ObjectId,
    info: Option<ObjectId>,
    file_id: Option<(Vec<u8>, Vec<u8>)>,
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
    if let Some((id1, id2)) = file_id {
        out.extend_from_slice(b"/ID [");
        out.extend_from_slice(crate::object::PdfString::Hex(id1).to_pdf_string().as_bytes());
        out.push(b' ');
        out.extend_from_slice(crate::object::PdfString::Hex(id2).to_pdf_string().as_bytes());
        out.extend_from_slice(b"] ");
    }
    out.extend_from_slice(b">>\n");
}

/// Builds the cross-reference stream object (ISO 32000-1 7.5.8) for a
/// full rewrite: `W = [1 4 2]` (type; offset-or-objstm-number;
/// generation-or-index), a single `Index` range `[0, size]` since object
/// numbers were assigned densely with no gaps, and object 0 as the
/// (unused) head of the free list.
fn build_xref_stream(
    size: u32,
    xref: &HashMap<u32, XrefTarget>,
    root: ObjectId,
    info: Option<ObjectId>,
    file_id: Option<(Vec<u8>, Vec<u8>)>,
) -> PdfStream {
    let mut data = Vec::with_capacity(size as usize * 7);

    // Object 0: free, head of the (here, always-empty) free list.
    data.push(0u8);
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&0xFFFFu16.to_be_bytes());

    for num in 1..size {
        match xref.get(&num) {
            Some(XrefTarget::Offset(offset)) => {
                data.push(1u8);
                data.extend_from_slice(&(*offset as u32).to_be_bytes());
                data.extend_from_slice(&0u16.to_be_bytes());
            }
            Some(XrefTarget::Compressed { stream, index }) => {
                data.push(2u8);
                data.extend_from_slice(&stream.to_be_bytes());
                data.extend_from_slice(&(*index as u16).to_be_bytes());
            }
            None => {
                // Should not happen (every 1..size was assigned during
                // renumbering), but degrade to "free" rather than emit a
                // bogus entry if it somehow does.
                data.push(0u8);
                data.extend_from_slice(&0u32.to_be_bytes());
                data.extend_from_slice(&0xFFFFu16.to_be_bytes());
            }
        }
    }

    let mut dict = PdfDictionary::new();
    dict.set("Type", Object::Name(PdfName::new_unchecked("XRef")));
    dict.set("Size", Object::Integer(size as i64));
    let mut w = PdfArray::new();
    w.push(Object::Integer(1));
    w.push(Object::Integer(4));
    w.push(Object::Integer(2));
    dict.set("W", Object::Array(w));
    dict.set("Root", Object::Reference(root));
    if let Some(info) = info {
        dict.set("Info", Object::Reference(info));
    }
    if let Some((id1, id2)) = file_id {
        let mut arr = PdfArray::new();
        arr.push(Object::String(crate::object::PdfString::Hex(id1)));
        arr.push(Object::String(crate::object::PdfString::Hex(id2)));
        dict.set("ID", Object::Array(arr));
    }

    PdfStream::with_dictionary(dict, data)
        .with_compression()
        .unwrap_or_else(|_| PdfStream::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;

    fn sample_pdf(num_pages: usize) -> Vec<u8> {
        let mut builder = DocumentBuilder::new();
        for i in 0..num_pages {
            let page = PageBuilder::a4()
                .font("F1", Standard14Font::Helvetica)
                .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, &format!("Page {i}")))
                .build();
            builder = builder.page(page);
        }
        builder.build().unwrap().save_to_bytes().unwrap()
    }

    #[test]
    fn test_incremental_noop_returns_original_bytes() {
        let original = sample_pdf(2);
        let doc = EditableDocument::from_bytes(original.clone()).unwrap();
        let saved = doc.save_incremental_to_bytes().unwrap();
        assert_eq!(saved, original);
    }

    #[test]
    fn test_incremental_save_appends_and_reopens() {
        let original = sample_pdf(3);
        let mut doc = EditableDocument::from_bytes(original.clone()).unwrap();
        let page_id = doc.page_id_at(0).unwrap();
        doc.replace_page_text(page_id, "Page 0", "Edited!").unwrap();

        let saved = doc.save_incremental_to_bytes().unwrap();
        // Incremental save is append-only: original bytes are an exact
        // prefix of the saved file.
        assert!(saved.len() > original.len());
        assert!(saved.starts_with(&original));

        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert_eq!(reopened.page_count().unwrap(), 3);
        let bytes = reopened.page_content_bytes(reopened.page_id_at(0).unwrap()).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("Edited!"));
    }

    #[test]
    fn test_incremental_save_chains_prev_and_reader_sees_latest() {
        let original = sample_pdf(1);
        let mut doc = EditableDocument::from_bytes(original).unwrap();
        let page_id = doc.page_id_at(0).unwrap();
        doc.replace_page_text(page_id, "Page 0", "First edit").unwrap();
        let after_first = doc.save_incremental_to_bytes().unwrap();

        // Load the once-edited file fresh and edit it again: this
        // exercises a two-revision /Prev chain.
        let mut doc2 = EditableDocument::from_bytes(after_first).unwrap();
        let page_id2 = doc2.page_id_at(0).unwrap();
        doc2.replace_page_text(page_id2, "First edit", "Second edit").unwrap();
        let after_second = doc2.save_incremental_to_bytes().unwrap();

        let final_doc = EditableDocument::from_bytes(after_second).unwrap();
        let bytes = final_doc
            .page_content_bytes(final_doc.page_id_at(0).unwrap())
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("Second edit"));
    }

    #[test]
    fn test_full_rewrite_round_trips_and_drops_orphans() {
        let original = sample_pdf(4);
        let mut doc = EditableDocument::from_bytes(original).unwrap();
        doc.delete_page(1).unwrap();

        let rewritten = doc.save_full_rewrite_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(rewritten).unwrap();
        assert_eq!(reopened.page_count().unwrap(), 3);
    }

    #[test]
    fn test_full_rewrite_smaller_than_uncompressed_original() {
        // `Document` defaults to uncompressed content streams, so a
        // multi-page document has obvious room for a full rewrite (which
        // always attempts FlateDecode) to shrink it.
        let original = sample_pdf(20);
        let doc = EditableDocument::from_bytes(original.clone()).unwrap();
        let rewritten = doc.save_full_rewrite_to_bytes().unwrap();
        assert!(
            rewritten.len() <= original.len(),
            "full rewrite ({} bytes) should not be larger than the uncompressed original ({} bytes)",
            rewritten.len(),
            original.len()
        );
    }

    #[test]
    fn test_full_rewrite_of_untouched_document_preserves_page_count_and_text() {
        let original = sample_pdf(5);
        let doc = EditableDocument::from_bytes(original).unwrap();
        let rewritten = doc.save_full_rewrite_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(rewritten).unwrap();
        assert_eq!(reopened.page_count().unwrap(), 5);
        for i in 0..5 {
            let id = reopened.page_id_at(i).unwrap();
            let bytes = reopened.page_content_bytes(id).unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains(&format!("Page {i}")));
        }
    }

    #[test]
    fn test_pdfa_compatible_save_round_trips() {
        let original = sample_pdf(3);
        let doc = EditableDocument::from_bytes(original).unwrap();
        let saved = doc.save_pdfa_compatible_to_bytes(PdfVersion::V1_4).unwrap();
        assert!(saved.starts_with(b"%PDF-1.4\n"));

        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert_eq!(reopened.page_count().unwrap(), 3);
        for i in 0..3 {
            let id = reopened.page_id_at(i).unwrap();
            let bytes = reopened.page_content_bytes(id).unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains(&format!("Page {i}")));
        }
    }

    #[test]
    fn test_pdfa_compatible_save_never_emits_object_streams_or_xref_streams() {
        // The whole point of this writer (vs. save_full_rewrite_to_bytes)
        // is avoiding PDF-1.5+-only constructs, which PDF/A-1 forbids.
        let original = sample_pdf(10);
        let doc = EditableDocument::from_bytes(original).unwrap();
        let saved = doc.save_pdfa_compatible_to_bytes(PdfVersion::V1_4).unwrap();
        let text = String::from_utf8_lossy(&saved);
        assert!(!text.contains("/ObjStm"), "must not use compressed object streams");
        assert!(!text.contains("/Type/XRef") && !text.contains("/Type /XRef"), "must not use a cross-reference stream");
        assert!(text.contains("\nxref\n"), "must use a classic xref table");
        assert!(text.contains("trailer"), "must use a classic trailer dictionary");
    }

    #[test]
    fn test_pdfa_compatible_save_drops_orphans_like_full_rewrite() {
        let original = sample_pdf(4);
        let mut doc = EditableDocument::from_bytes(original).unwrap();
        doc.delete_page(1).unwrap();
        let saved = doc.save_pdfa_compatible_to_bytes(PdfVersion::V1_7).unwrap();
        assert!(saved.starts_with(b"%PDF-1.7\n"));
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert_eq!(reopened.page_count().unwrap(), 3);
    }
}
