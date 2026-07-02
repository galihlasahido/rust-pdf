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

/// Maximum number of objects visited while walking the reachable-object
/// graph for a full rewrite. Bounds work against a corrupt/adversarial
/// document (e.g. a cyclic `/Kids` or resource graph); real documents
/// this crate targets (tens of thousands of pages) are nowhere near this.
const MAX_REACHABLE_OBJECTS: usize = 5_000_000;

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
        let (order, objects) = self.collect_reachable(&roots)?;

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

    /// BFS over every [`Object::Reference`] reachable from `roots`,
    /// returning visitation order (used to assign compact new numbers)
    /// and the resolved objects.
    fn collect_reachable(
        &self,
        roots: &[ObjectId],
    ) -> PdfResult<(Vec<ObjectId>, HashMap<ObjectId, Object>)> {
        use std::collections::{HashSet, VecDeque};

        let mut order = Vec::new();
        let mut objects = HashMap::new();
        let mut queued: HashSet<ObjectId> = HashSet::new();
        let mut queue: VecDeque<ObjectId> = VecDeque::new();

        for &r in roots {
            if queued.insert(r) {
                queue.push_back(r);
            }
        }

        while let Some(id) = queue.pop_front() {
            if order.len() >= MAX_REACHABLE_OBJECTS {
                return Err(EditorError::ResourceLimitExceeded(
                    "full-rewrite reachable-object graph exceeded the maximum supported size"
                        .to_string(),
                )
                .into());
            }
            let Some(obj) = self.get_object(id) else {
                continue;
            };
            let mut refs = Vec::new();
            collect_all_refs(&obj, &mut refs);
            for r in refs {
                if queued.insert(r) {
                    queue.push_back(r);
                }
            }
            order.push(id);
            objects.insert(id, obj);
        }

        Ok((order, objects))
    }
}

/// Where an object ended up after a full rewrite, for xref-stream
/// purposes (ISO 32000-1 Table 18).
enum XrefTarget {
    /// Type 1: an ordinary indirect object at this byte offset.
    Offset(u64),
    /// Type 2: packed inside object-stream number `stream` at `index`.
    Compressed { stream: u32, index: u32 },
}

fn collect_all_refs(obj: &Object, out: &mut Vec<ObjectId>) {
    match obj {
        Object::Reference(id) => out.push(*id),
        Object::Array(arr) => {
            for item in arr.iter() {
                collect_all_refs(item, out);
            }
        }
        Object::Dictionary(dict) => {
            for (_, v) in dict.iter() {
                collect_all_refs(v, out);
            }
        }
        Object::Stream(s) => {
            for (_, v) in s.dictionary.iter() {
                collect_all_refs(v, out);
            }
        }
        _ => {}
    }
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
}
