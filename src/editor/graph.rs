//! Core mutable object graph for [`EditableDocument`].

use crate::error::{EditorError, PdfResult};
use crate::object::{Object, PdfDictionary};
use crate::parser::PdfReader;
use crate::types::ObjectId;
use indexmap::IndexMap;
use std::path::Path;

/// Maximum number of nodes visited while walking a page tree (ISO 32000-1
/// 7.7.3). Bounds work on a corrupt/adversarial `/Kids` structure (e.g. a
/// node listing itself, or a `/Count` that lies) - real documents this
/// crate expects to edit (up to tens of thousands of pages) are nowhere
/// near this.
const MAX_PAGE_TREE_NODES: usize = 500_000;

/// Maximum `/Kids`-nesting depth allowed while walking a page tree.
const MAX_PAGE_TREE_DEPTH: u32 = 64;

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
        let next_object_number = reader
            .xref()
            .iter()
            .map(|(num, _)| *num)
            .max()
            .unwrap_or(0)
            + 1;

        Ok(Self {
            reader,
            overlay: IndexMap::new(),
            next_object_number,
            root,
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
