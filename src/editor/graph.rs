//! Core mutable object graph for [`EditableDocument`].

use super::audit::{self, RedactionAuditEntry};
use crate::error::{EditorError, PdfResult};
use crate::object::{Object, PdfDictionary};
use crate::parser::PdfReader;
use crate::types::ObjectId;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

/// Maximum number of nodes visited while walking a page tree (ISO 32000-1
/// 7.7.3). Bounds work on a corrupt/adversarial `/Kids` structure (e.g. a
/// node listing itself, or a `/Count` that lies) - real documents this
/// crate expects to edit (up to tens of thousands of pages) are nowhere
/// near this.
const MAX_PAGE_TREE_NODES: usize = 500_000;

/// Maximum `/Kids`-nesting depth allowed while walking a page tree.
const MAX_PAGE_TREE_DEPTH: u32 = 64;

/// Maximum number of objects visited by [`EditableDocument::reachable_objects`].
/// Shared bound for every full-graph walk (full rewrite, PDF/A-safe save,
/// and the conformance validators/converters in
/// [`crate::editor::pdfa`]/[`crate::editor::pdfx`]/[`crate::editor::pdfua`]),
/// so a corrupt/adversarial resource graph (e.g. a cyclic `/Resources` or
/// `/Group` reference) cannot force unbounded work in any of them.
const MAX_REACHABLE_OBJECTS: usize = 5_000_000;

/// An existing PDF document loaded for in-place editing.
///
/// See the [module docs](crate::editor) for the incremental-vs-full-rewrite
/// save tradeoff and the scope of the page-tree/content-stream editing
/// operations.
///
/// Internally, objects are represented as a base [`PdfReader`] (the
/// original file, read-only) plus an `overlay` of objects that have been
/// added or replaced since loading. Every read goes through the overlay
/// first so a caller always sees its own edits; every write only ever
/// touches the overlay, never the original bytes, which is what makes
/// incremental save (append-only) possible.
pub struct EditableDocument {
    pub(crate) reader: PdfReader,
    /// Objects created or modified since loading, keyed by their (final)
    /// object id. Iteration order is insertion order, which keeps
    /// incremental-save diffs small and deterministic.
    pub(crate) overlay: IndexMap<ObjectId, Object>,
    /// Next never-yet-used object number. Seeded from the highest object
    /// number referenced by the base file's cross-reference table so
    /// newly allocated ids can never collide with an existing object.
    pub(crate) next_object_number: u32,
    /// The catalog's object id (ISO 32000-1 7.7.2). Fixed at load time;
    /// edits that need to change the catalog (e.g. splicing in an
    /// `/Outlines` tree) mutate this same id in the overlay rather than
    /// allocating a new catalog object.
    pub(crate) root: ObjectId,
    /// The redaction audit trail (see [`crate::editor::audit`]), seeded
    /// from whatever was already persisted in the base file's catalog (if
    /// any) and appended to by every `redact_*`/`strip_document_metadata`
    /// call this session.
    pub(crate) audit_log: Vec<RedactionAuditEntry>,
    /// Object id the audit log stream is written to. `None` until the
    /// first redaction this session needs to persist one (an id is
    /// allocated lazily rather than always reserving one, so opening a
    /// document read-only never allocates an unused object).
    pub(crate) audit_log_object_id: Option<ObjectId>,
    /// Set once any `redact_*`/`strip_document_metadata` call has run.
    /// [`EditableDocument::save_incremental`]/`save_incremental_to_bytes`
    /// refuse to run while this is set: an incremental update only
    /// *appends* bytes (ISO 32000-1 7.5.6), so the pre-redaction content
    /// this session just removed from the object graph would still be
    /// sitting in the file's earlier bytes, completely recoverable -
    /// exactly the "hidden revision" problem redaction must not
    /// reintroduce. Use [`EditableDocument::save_full_rewrite`] (or
    /// `save_full_rewrite_to_bytes`) after redacting.
    pub(crate) redaction_applied: bool,
}

impl EditableDocument {
    /// Loads a PDF file for editing.
    pub fn open(path: impl AsRef<Path>) -> PdfResult<Self> {
        let reader = PdfReader::from_file(path)?;
        Self::from_reader(reader)
    }

    /// Loads a PDF already held in memory for editing.
    pub fn from_bytes(data: Vec<u8>) -> PdfResult<Self> {
        let reader = PdfReader::from_bytes(data)?;
        Self::from_reader(reader)
    }

    /// Loads an encrypted PDF file for editing, deriving the file
    /// encryption key from the trailer's `/Encrypt` dictionary and
    /// `password` (ISO 32000-1 §7.6 / ISO 32000-2 §7.6). See
    /// [`crate::parser::PdfReader::from_file_with_password`] for the exact
    /// contract (supported algorithms, error cases, and the no-op behavior
    /// when the file turns out not to be encrypted at all).
    #[cfg(feature = "encryption")]
    pub fn open_with_password(path: impl AsRef<Path>, password: &str) -> PdfResult<Self> {
        let reader = PdfReader::from_file_with_password(path, password)?;
        Self::from_reader(reader)
    }

    /// Loads an encrypted PDF already held in memory for editing. See
    /// [`Self::open_with_password`]/
    /// [`crate::parser::PdfReader::from_bytes_with_password`] for the full
    /// contract.
    #[cfg(feature = "encryption")]
    pub fn from_bytes_with_password(data: Vec<u8>, password: &str) -> PdfResult<Self> {
        let reader = PdfReader::from_bytes_with_password(data, password)?;
        Self::from_reader(reader)
    }

    fn from_reader(reader: PdfReader) -> PdfResult<Self> {
        let root = reader.trailer().root;

        // Validate eagerly: every operation below assumes the catalog
        // resolves to a dictionary with a usable /Pages entry, per
        // ISO 32000-1 Table 28.
        let catalog = reader
            .resolve_reference(root)
            .and_then(|o| match o {
                Object::Dictionary(d) => Some(d),
                _ => None,
            })
            .ok_or(EditorError::MissingCatalog)?;
        match catalog.get("Pages") {
            Some(Object::Reference(_)) => {}
            _ => return Err(EditorError::MissingCatalog.into()),
        }

        // Object numbers must never collide with anything already in the
        // base file (across its full /Prev incremental-update chain), so
        // seed the allocator from the highest one seen anywhere in the
        // merged xref table.
        let next_object_number = reader.xref().iter().map(|(num, _)| *num).max().unwrap_or(0) + 1;

        let audit_log_object_id = match catalog.get(audit::AUDIT_LOG_CATALOG_KEY) {
            Some(Object::Reference(id)) => Some(*id),
            _ => None,
        };
        let audit_log =
            audit::load_from_catalog(&catalog, |id| match reader.resolve_reference(id) {
                Some(Object::Stream(s)) => s.decode_all().ok(),
                _ => None,
            });

        Ok(Self {
            reader,
            overlay: IndexMap::new(),
            next_object_number,
            root,
            audit_log,
            audit_log_object_id,
            redaction_applied: false,
        })
    }

    /// Returns the object id of the document catalog.
    pub fn catalog_id(&self) -> ObjectId {
        self.root
    }

    /// Resolves an (indirect or overlay) object by id.
    ///
    /// Checks the overlay (edits made this session) first, then falls
    /// back to the original file. Returns `None` for a dangling/free
    /// reference rather than erroring, matching ISO 32000-1 7.3.10 ("an
    /// indirect reference to an undefined object... shall be treated as
    /// a reference to the null object").
    pub fn get_object(&self, id: ObjectId) -> Option<Object> {
        if let Some(obj) = self.overlay.get(&id) {
            return Some(obj.clone());
        }
        self.reader.resolve_reference(id)
    }

    /// Resolves an object expected to be a dictionary (or a stream, whose
    /// dictionary is returned) by id.
    pub(crate) fn get_dictionary(&self, id: ObjectId) -> PdfResult<PdfDictionary> {
        match self.get_object(id) {
            Some(Object::Dictionary(d)) => Ok(d),
            Some(Object::Stream(s)) => Ok(s.dictionary),
            _ => Err(EditorError::UnresolvedObject(id.number, id.generation).into()),
        }
    }

    /// Records an object as created/modified in the overlay.
    pub(crate) fn set_object(&mut self, id: ObjectId, object: Object) {
        self.overlay.insert(id, object);
    }

    /// Allocates a fresh object id (generation 0) guaranteed not to
    /// collide with any object number used anywhere in the base file or
    /// already allocated this session.
    pub(crate) fn allocate_id(&mut self) -> ObjectId {
        let id = ObjectId::new(self.next_object_number);
        self.next_object_number += 1;
        id
    }

    /// Returns the catalog dictionary.
    pub(crate) fn catalog(&self) -> PdfResult<PdfDictionary> {
        self.get_dictionary(self.root)
    }

    /// Returns the object id of the page tree root (`Catalog/Pages`).
    pub(crate) fn pages_root_id(&self) -> PdfResult<ObjectId> {
        match self.catalog()?.get("Pages") {
            Some(Object::Reference(id)) => Ok(*id),
            _ => Err(EditorError::MissingCatalog.into()),
        }
    }

    /// Walks the page tree (ISO 32000-1 7.7.3) and returns the object ids
    /// of every leaf `/Page` node, in document order.
    ///
    /// A node is treated as an intermediate `/Pages` node if it has a
    /// `/Kids` array (regardless of `/Type`, for leniency with malformed
    /// producers) and as a leaf page otherwise. Guards against cycles and
    /// excessive fan-out/depth in the (untrusted, possibly hand-crafted)
    /// source file.
    pub fn page_ids(&self) -> PdfResult<Vec<ObjectId>> {
        let root = self.pages_root_id()?;
        let mut out = Vec::new();
        let mut visited = std::collections::HashSet::new();
        self.walk_page_tree(root, 0, &mut visited, &mut out)?;
        Ok(out)
    }

    fn walk_page_tree(
        &self,
        node_id: ObjectId,
        depth: u32,
        visited: &mut std::collections::HashSet<ObjectId>,
        out: &mut Vec<ObjectId>,
    ) -> PdfResult<()> {
        if depth > MAX_PAGE_TREE_DEPTH {
            return Err(EditorError::PageTreeTooLarge("max depth exceeded").into());
        }
        if out.len() + visited.len() > MAX_PAGE_TREE_NODES {
            return Err(EditorError::PageTreeTooLarge("max node count exceeded").into());
        }
        if !visited.insert(node_id) {
            // Cycle: a node reachable via itself. Silently stop
            // descending this branch rather than looping forever.
            return Ok(());
        }

        let dict = self.get_dictionary(node_id).map_err(|_| {
            EditorError::MalformedPageTree(format!(
                "node {} {} R does not resolve to a dictionary",
                node_id.number, node_id.generation
            ))
        })?;

        match dict.get("Kids") {
            Some(Object::Array(kids)) => {
                for kid in kids.iter() {
                    if let Object::Reference(kid_id) = kid {
                        self.walk_page_tree(*kid_id, depth + 1, visited, out)?;
                    }
                }
                Ok(())
            }
            _ => {
                out.push(node_id);
                Ok(())
            }
        }
    }

    /// Returns the number of leaf pages in the document.
    pub fn page_count(&self) -> PdfResult<usize> {
        Ok(self.page_ids()?.len())
    }

    /// Returns the object id of the page at `index` (0-based, document
    /// order).
    pub fn page_id_at(&self, index: usize) -> PdfResult<ObjectId> {
        let ids = self.page_ids()?;
        ids.get(index).copied().ok_or_else(|| {
            EditorError::InvalidPageIndex {
                index,
                count: ids.len(),
            }
            .into()
        })
    }

    /// Appends `annot_id` to `page_id`'s `/Annots` array (ISO 32000-1
    /// 7.7.3.3, Table 30), creating the array if the page doesn't have
    /// one yet. Shared by the AcroForm widget, annotation and outline
    /// submodules, which all add entries to a page's annotation list.
    pub(crate) fn add_annot_to_page(
        &mut self,
        page_id: ObjectId,
        annot_id: ObjectId,
    ) -> PdfResult<()> {
        let mut page = self.get_dictionary(page_id)?;
        let mut annots = match page.get("Annots") {
            Some(Object::Array(a)) => a.clone(),
            _ => crate::object::PdfArray::new(),
        };
        annots.push(Object::Reference(annot_id));
        page.set("Annots", Object::Array(annots));
        self.set_object(page_id, Object::Dictionary(page));
        Ok(())
    }

    /// Returns the classic `/Info /Title` (ISO 32000-1 14.3.3, Table 317)
    /// if the document has an `/Info` dictionary with one, decoded via
    /// [`super::util::from_pdf_text_string`]. Shared by
    /// [`crate::editor::pdfa`]'s `/Info`-vs-XMP title equivalence check
    /// (ISO 19005-1 6.7.3) and [`crate::editor::pdfua`]'s XMP `dc:title`
    /// fallback (ISO 14289-1 7.1's "the metadata stream must contain
    /// dc:title" requirement) - both need "does this document already
    /// have a title", just for different reasons.
    pub(crate) fn classic_info_title(&self) -> Option<String> {
        let info_dict = self
            .reader
            .trailer()
            .info
            .and_then(|id| self.get_dictionary(id).ok())?;
        match info_dict.get("Title") {
            Some(Object::String(s)) => Some(super::util::from_pdf_text_string(s)),
            _ => None,
        }
    }

    /// Returns `page_id`'s own `/Resources` dictionary (ISO 32000-1
    /// 7.7.3.4, Table 30), resolving one level of indirection if it's a
    /// reference. Does **not** walk up the page tree for an inherited
    /// `/Resources` on an ancestor `/Pages` node (a rare, legal-but-
    /// unusual authoring style, ISO 32000-1 7.7.3.4's inheritable
    /// attributes) - every producer this crate is aware of, including
    /// this crate's own [`crate::page::PageBuilder`], always sets
    /// `/Resources` directly on the page. Callers that need the fully
    /// spec-correct inherited lookup should treat this as a documented
    /// simplification, not a guarantee.
    pub(crate) fn page_resources(&self, page_id: ObjectId) -> PdfResult<PdfDictionary> {
        let page = self.get_dictionary(page_id)?;
        match page.get("Resources") {
            Some(Object::Dictionary(d)) => Ok(d.clone()),
            Some(Object::Reference(id)) => self.get_dictionary(*id),
            _ => Ok(PdfDictionary::new()),
        }
    }

    /// BFS over every [`Object::Reference`] reachable from `roots` (direct
    /// or indirect, through dictionaries/arrays/stream dictionaries),
    /// returning visitation order (breadth-first, so e.g. a full rewrite
    /// can assign compact ids in a stable order) and the resolved objects.
    ///
    /// This is the one bounded graph walk shared by
    /// [`EditableDocument::save_full_rewrite_to_bytes`],
    /// [`EditableDocument::save_pdfa_compatible_to_bytes`]
    /// (`crate::editor::save`) and the conformance checkers in
    /// `crate::editor::pdfa`/`pdfx`/`pdfua` - every one of them needs "every
    /// object the document actually uses" over an untrusted object graph,
    /// so the [`MAX_REACHABLE_OBJECTS`] bound protects all of them
    /// identically rather than each reimplementing (and each having to be
    /// independently proven to bound) its own walk.
    pub(crate) fn reachable_objects(
        &self,
        roots: &[ObjectId],
    ) -> PdfResult<(Vec<ObjectId>, HashMap<ObjectId, Object>)> {
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
                    "reachable-object graph exceeded the maximum supported size".to_string(),
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

/// Collects every [`Object::Reference`] directly contained in `obj` (one
/// level of dictionaries/arrays/stream dictionaries - the caller's BFS
/// handles following them transitively).
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn test_open_valid_document() {
        let doc = EditableDocument::from_bytes(sample_pdf(3)).unwrap();
        assert_eq!(doc.page_count().unwrap(), 3);
        let ids = doc.page_ids().unwrap();
        assert_eq!(ids.len(), 3);
        // Distinct page objects.
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
    }

    #[test]
    fn test_open_rejects_garbage() {
        let result = EditableDocument::from_bytes(b"not a pdf".to_vec());
        assert!(result.is_err());
    }

    #[test]
    fn test_open_rejects_pdf_without_pages() {
        // A syntactically valid PDF whose catalog has no /Pages entry.
        let data = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog >>\nendobj\n\
xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n52\n%%EOF\n"
            .to_vec();
        let result = EditableDocument::from_bytes(data);
        assert!(result.is_err());
    }

    #[test]
    fn test_next_object_number_above_existing_max() {
        let doc = EditableDocument::from_bytes(sample_pdf(1)).unwrap();
        let max_existing = doc.reader.xref().iter().map(|(n, _)| *n).max().unwrap();
        assert!(doc.next_object_number > max_existing);
    }

    #[test]
    fn test_cyclic_page_tree_does_not_hang() {
        // Pages node whose own Kids array references itself. Byte offsets
        // are computed rather than hand-guessed so the xref table is
        // actually accurate (an inaccurate-but-syntactically-valid table
        // would parse "successfully" and then fail to resolve objects for
        // an unrelated reason, defeating the point of this test).
        let mut data = Vec::new();
        data.extend_from_slice(b"%PDF-1.7\n");
        let obj1_off = data.len();
        data.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let obj2_off = data.len();
        data.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n");
        let xref_off = data.len();
        data.extend_from_slice(b"xref\n0 3\n");
        data.extend_from_slice(b"0000000000 65535 f \n");
        data.extend_from_slice(format!("{:010} 00000 n \n", obj1_off).as_bytes());
        data.extend_from_slice(format!("{:010} 00000 n \n", obj2_off).as_bytes());
        data.extend_from_slice(b"trailer\n<< /Size 3 /Root 1 0 R >>\n");
        data.extend_from_slice(format!("startxref\n{xref_off}\n%%EOF\n").as_bytes());

        let doc = EditableDocument::from_bytes(data).unwrap();
        // Must terminate; a self-referential Pages node has no leaves.
        let result = doc.page_ids();
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }
}
