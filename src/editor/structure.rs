//! Minimal Tagged PDF logical structure tree support (ISO 32000-1:2008
//! Section 14.7 "Logical Structure" / 14.8 "Tagged PDF"): headings,
//! paragraphs, tables (with rows/cells) and figures with alternate text,
//! tied back to the actual page content via marked-content sequences
//! (14.6) and the structure/content correspondence machinery of 14.7.4
//! (`/StructParents` on the page, a `/ParentTree` number tree on
//! `/StructTreeRoot`).
//!
//! This is *not* a general accessibility/tagging engine: there is no
//! automatic tag inference from existing untagged content, no table
//! header scope (`/Scope`) handling, and no `/RoleMap` for custom types.
//! It gives a caller who already knows the semantic structure of what
//! they are drawing a way to record that structure correctly - which is
//! what "produce one tagged sample document" (this task's Definition of
//! Done) requires - not a way to retrofit tags onto arbitrary existing
//! PDF content.

use super::graph::EditableDocument;
use super::util::to_pdf_text_string;
use crate::content::ContentBuilder;
use crate::error::PdfResult;
use crate::object::{Object, PdfArray, PdfDictionary, PdfName};
use crate::types::ObjectId;

/// Maximum number of structure-tree nodes visited while reading the tree
/// back, bounding work against a corrupt/adversarial `/K` graph.
const MAX_STRUCT_NODES: usize = 200_000;

/// A standard structure type (ISO 32000-1 14.8.4, Table 337) this module
/// directly supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructType {
    /// `/Document`: the root of a document's content.
    Document,
    /// `/H1`..`/H6` for `1..=6`; plain `/H` for `0`.
    Heading(u8),
    /// `/P`: a paragraph.
    Paragraph,
    /// `/Table`.
    Table,
    /// `/TR`: a table row.
    TableRow,
    /// `/TD`: a (non-header) table data cell.
    TableCell,
    /// `/Figure`.
    Figure,
}

impl StructType {
    fn tag(self) -> String {
        match self {
            StructType::Document => "Document".to_string(),
            StructType::Heading(0) => "H".to_string(),
            StructType::Heading(n) => format!("H{}", n.min(6)),
            StructType::Paragraph => "P".to_string(),
            StructType::Table => "Table".to_string(),
            StructType::TableRow => "TR".to_string(),
            StructType::TableCell => "TD".to_string(),
            StructType::Figure => "Figure".to_string(),
        }
    }
}

/// A node of the logical structure tree, as returned by
/// [`EditableDocument::struct_tree`].
#[derive(Debug, Clone)]
pub struct StructNode {
    /// The structure element's own object id.
    pub id: ObjectId,
    /// Its `/S` (structure type name), e.g. `"P"`, `"H1"`, `"Figure"`.
    pub struct_type: String,
    /// Its `/Alt` (alternate description), if any (ISO 32000-1 14.7.2,
    /// required by PDF/UA for `/Figure` elements).
    pub alt_text: Option<String>,
    /// The MCID this element's own marked-content sequence uses, if it
    /// directly owns one (a `/K` integer rather than child elements).
    pub mcid: Option<i64>,
    /// Child structure elements, in document order.
    pub children: Vec<StructNode>,
}

impl EditableDocument {
    /// Ensures the document has a `/StructTreeRoot` (creating an empty one,
    /// and setting `/Catalog /MarkInfo /Marked true`, if absent) and
    /// returns its object id.
    pub fn ensure_struct_tree_root(&mut self) -> PdfResult<ObjectId> {
        let mut catalog = self.catalog()?;
        if let Some(Object::Reference(id)) = catalog.get("StructTreeRoot") {
            return Ok(*id);
        }
        let root_id = self.allocate_id();
        let parent_tree_id = self.allocate_id();

        let mut parent_tree = PdfDictionary::new();
        parent_tree.set("Nums", Object::Array(PdfArray::new()));
        self.set_object(parent_tree_id, Object::Dictionary(parent_tree));

        let mut root = PdfDictionary::new();
        root.set("Type", Object::Name(PdfName::new_unchecked("StructTreeRoot")));
        root.set("K", Object::Array(PdfArray::new()));
        root.set("ParentTree", Object::Reference(parent_tree_id));
        root.set("ParentTreeNextKey", Object::Integer(0));
        self.set_object(root_id, Object::Dictionary(root));

        catalog.set("StructTreeRoot", Object::Reference(root_id));
        let mut mark_info = PdfDictionary::new();
        mark_info.set("Marked", Object::Boolean(true));
        catalog.set("MarkInfo", Object::Dictionary(mark_info));
        let cat_id = self.catalog_id();
        self.set_object(cat_id, Object::Dictionary(catalog));
        Ok(root_id)
    }

    /// Adds (or returns the existing) `/Document` structure element that
    /// is the tree's sole top-level child, a common convention (ISO
    /// 32000-1 14.7.2's worked examples) for anchoring every other
    /// element under one root node.
    pub fn add_document_structure_root(&mut self) -> PdfResult<ObjectId> {
        let struct_root_id = self.ensure_struct_tree_root()?;
        let mut root = self.get_dictionary(struct_root_id)?;
        if let Some(Object::Array(k)) = root.get("K") {
            if let Some(Object::Reference(id)) = k.get(0) {
                if matches!(
                    self.get_dictionary(*id).ok().and_then(|d| d.get("S").cloned()),
                    Some(Object::Name(n)) if n.as_str() == "Document"
                ) {
                    return Ok(*id);
                }
            }
        }
        let doc_elem_id = self.allocate_id();
        let mut elem = PdfDictionary::new();
        elem.set("Type", Object::Name(PdfName::new_unchecked("StructElem")));
        elem.set("S", Object::Name(PdfName::new_unchecked("Document")));
        elem.set("P", Object::Reference(struct_root_id));
        self.set_object(doc_elem_id, Object::Dictionary(elem));

        let mut k = match root.get("K") {
            Some(Object::Array(a)) => a.clone(),
            _ => PdfArray::new(),
        };
        k.push(Object::Reference(doc_elem_id));
        root.set("K", Object::Array(k));
        self.set_object(struct_root_id, Object::Dictionary(root));
        Ok(doc_elem_id)
    }

    /// Draws `content` onto `page_index`, wrapped in a marked-content
    /// sequence (`/<Tag> <</MCID n>> BDC ... EMC`, ISO 32000-1 14.6) and
    /// linked to a new `/StructElem` of type `struct_type`, itself a
    /// child of `parent` (or the `/StructTreeRoot` directly if `None`).
    /// `alt_text` sets `/Alt` (used for `/Figure`, ISO 32000-1 14.7.2 -
    /// PDF/UA requires every figure have one). Returns the new structure
    /// element's object id, suitable as the `parent` of a nested call
    /// (e.g. a `Table` -> `TableRow` -> `TableCell` hierarchy).
    pub fn add_tagged_content(
        &mut self,
        page_index: usize,
        parent: Option<ObjectId>,
        struct_type: StructType,
        content: &ContentBuilder,
        alt_text: Option<&str>,
    ) -> PdfResult<ObjectId> {
        let struct_root_id = self.ensure_struct_tree_root()?;
        let page_id = self.page_id_at(page_index)?;
        let parent_ref = parent.unwrap_or(struct_root_id);

        let key = self.page_struct_parents_key(page_id, struct_root_id)?;
        let mut per_page = self.parent_tree_array(struct_root_id, key)?;
        let mcid = per_page.len() as i64;

        let elem_id = self.allocate_id();
        let mut elem = PdfDictionary::new();
        elem.set("Type", Object::Name(PdfName::new_unchecked("StructElem")));
        elem.set("S", Object::Name(PdfName::new_unchecked(struct_type.tag())));
        elem.set("P", Object::Reference(parent_ref));
        elem.set("Pg", Object::Reference(page_id));
        elem.set("K", Object::Integer(mcid));
        if let Some(alt) = alt_text {
            elem.set("Alt", Object::String(to_pdf_text_string(alt)));
        }
        self.set_object(elem_id, Object::Dictionary(elem));

        let mut parent_dict = self.get_dictionary(parent_ref)?;
        let mut k = match parent_dict.get("K") {
            Some(Object::Array(a)) => a.clone(),
            _ => PdfArray::new(),
        };
        k.push(Object::Reference(elem_id));
        parent_dict.set("K", Object::Array(k));
        self.set_object(parent_ref, Object::Dictionary(parent_dict));

        per_page.push(Object::Reference(elem_id));
        self.set_parent_tree_array(struct_root_id, key, per_page)?;

        let tag = struct_type.tag();
        let mut bytes = self.page_content_bytes(page_id)?;
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(format!("/{tag} <</MCID {mcid}>> BDC\nq\n").as_bytes());
        bytes.extend_from_slice(&content.build_bytes());
        bytes.extend_from_slice(b"\nQ\nEMC\n");
        self.set_page_content_bytes(page_id, bytes)?;

        Ok(elem_id)
    }

    /// Returns the full logical structure tree, or `None` if the
    /// document has no `/StructTreeRoot`.
    pub fn struct_tree(&self) -> PdfResult<Option<StructNode>> {
        let Ok(catalog) = self.catalog() else { return Ok(None) };
        let Some(Object::Reference(root_id)) = catalog.get("StructTreeRoot") else { return Ok(None) };
        let Ok(root) = self.get_dictionary(*root_id) else { return Ok(None) };
        let mut visited = std::collections::HashSet::new();
        let children = match root.get("K") {
            Some(Object::Array(k)) => self.read_struct_children(k, &mut visited)?,
            _ => Vec::new(),
        };
        Ok(Some(StructNode { id: *root_id, struct_type: "StructTreeRoot".to_string(), alt_text: None, mcid: None, children }))
    }

    fn read_struct_children(&self, k: &PdfArray, visited: &mut std::collections::HashSet<ObjectId>) -> PdfResult<Vec<StructNode>> {
        let mut out = Vec::new();
        for entry in k.iter() {
            let Object::Reference(id) = entry else { continue };
            if visited.len() >= MAX_STRUCT_NODES || !visited.insert(*id) {
                continue;
            }
            let Ok(dict) = self.get_dictionary(*id) else { continue };
            let struct_type = match dict.get("S") {
                Some(Object::Name(n)) => n.as_str().to_string(),
                _ => String::new(),
            };
            let alt_text = dict.get("Alt").and_then(|o| match o {
                Object::String(s) => Some(super::util::from_pdf_text_string(s)),
                _ => None,
            });
            let (mcid, children) = match dict.get("K") {
                Some(Object::Integer(m)) => (Some(*m), Vec::new()),
                Some(Object::Array(kids)) => (None, self.read_struct_children(kids, visited)?),
                Some(Object::Reference(kid)) => {
                    let mut arr = PdfArray::new();
                    arr.push(Object::Reference(*kid));
                    (None, self.read_struct_children(&arr, visited)?)
                }
                _ => (None, Vec::new()),
            };
            out.push(StructNode { id: *id, struct_type, alt_text, mcid, children });
        }
        Ok(out)
    }

    /// Returns `page_id`'s `/StructParents` key, assigning (and
    /// persisting, via `/StructTreeRoot /ParentTreeNextKey`) a fresh one
    /// if the page doesn't have one yet.
    fn page_struct_parents_key(&mut self, page_id: ObjectId, struct_root_id: ObjectId) -> PdfResult<i64> {
        let mut page = self.get_dictionary(page_id)?;
        if let Some(Object::Integer(k)) = page.get("StructParents") {
            return Ok(*k);
        }
        let mut root = self.get_dictionary(struct_root_id)?;
        let next = match root.get("ParentTreeNextKey") {
            Some(Object::Integer(n)) => *n,
            _ => 0,
        };
        root.set("ParentTreeNextKey", Object::Integer(next + 1));
        self.set_object(struct_root_id, Object::Dictionary(root));
        page.set("StructParents", Object::Integer(next));
        self.set_object(page_id, Object::Dictionary(page));
        Ok(next)
    }

    fn parent_tree_array(&self, struct_root_id: ObjectId, key: i64) -> PdfResult<PdfArray> {
        let root = self.get_dictionary(struct_root_id)?;
        let Some(Object::Reference(pt_id)) = root.get("ParentTree") else { return Ok(PdfArray::new()) };
        let pt = self.get_dictionary(*pt_id)?;
        let Some(Object::Array(nums)) = pt.get("Nums") else { return Ok(PdfArray::new()) };
        for pair in nums.as_slice().chunks_exact(2) {
            if let Object::Integer(k) = &pair[0] {
                if *k == key {
                    if let Object::Array(a) = &pair[1] {
                        return Ok(a.clone());
                    }
                }
            }
        }
        Ok(PdfArray::new())
    }

    /// Replaces (or inserts) the `key -> arr` entry in
    /// `/StructTreeRoot /ParentTree /Nums`, keeping it sorted by key as
    /// required for a conformant number-tree leaf (ISO 32000-1 7.9.7).
    fn set_parent_tree_array(&mut self, struct_root_id: ObjectId, key: i64, arr: PdfArray) -> PdfResult<()> {
        let root = self.get_dictionary(struct_root_id)?;
        let Some(Object::Reference(pt_id)) = root.get("ParentTree").cloned() else { return Ok(()) };
        let mut pt = self.get_dictionary(pt_id)?;
        let mut pairs: Vec<(i64, Object)> = match pt.get("Nums") {
            Some(Object::Array(nums)) => nums
                .as_slice()
                .chunks_exact(2)
                .filter_map(|p| match &p[0] {
                    Object::Integer(k) => Some((*k, p[1].clone())),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        pairs.retain(|(k, _)| *k != key);
        pairs.push((key, Object::Array(arr)));
        pairs.sort_by_key(|(k, _)| *k);

        let mut nums = PdfArray::new();
        for (k, v) in pairs {
            nums.push(Object::Integer(k));
            nums.push(v);
        }
        pt.set("Nums", Object::Array(nums));
        self.set_object(pt_id, Object::Dictionary(pt));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    fn doc_with_one_page() -> EditableDocument {
        let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(ContentBuilder::new()).build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        EditableDocument::from_bytes(bytes).unwrap()
    }

    #[test]
    fn test_ensure_struct_tree_root_sets_mark_info() {
        let mut doc = doc_with_one_page();
        doc.ensure_struct_tree_root().unwrap();
        let catalog = doc.catalog().unwrap();
        assert!(catalog.get("StructTreeRoot").is_some());
        assert!(matches!(catalog.get("MarkInfo"), Some(Object::Dictionary(_))));
    }

    #[test]
    fn test_tagged_heading_and_paragraph_sample_document() {
        let mut doc = doc_with_one_page();
        let root = doc.add_document_structure_root().unwrap();

        let heading_content = ContentBuilder::new().text("F1", 18.0, 72.0, 780.0, "Report Title");
        let h1 = doc.add_tagged_content(0, Some(root), StructType::Heading(1), &heading_content, None).unwrap();

        let para_content = ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "This is body text.");
        doc.add_tagged_content(0, Some(root), StructType::Paragraph, &para_content, None).unwrap();

        let tree = doc.struct_tree().unwrap().expect("struct tree must exist");
        assert_eq!(tree.children.len(), 1); // the /Document root
        let document = &tree.children[0];
        assert_eq!(document.struct_type, "Document");
        assert_eq!(document.children.len(), 2);
        assert_eq!(document.children[0].struct_type, "H1");
        assert_eq!(document.children[0].id, h1);
        assert_eq!(document.children[1].struct_type, "P");

        let page_id = doc.page_id_at(0).unwrap();
        let content = String::from_utf8_lossy(&doc.page_content_bytes(page_id).unwrap()).into_owned();
        assert!(content.contains("/H1 <</MCID 0>> BDC"), "content was: {content}");
        assert!(content.contains("/P <</MCID 1>> BDC"), "content was: {content}");
        assert!(content.contains("Report Title"));
        assert!(content.contains("This is body text."));
    }

    #[test]
    fn test_tagged_table_and_figure_with_alt_text() {
        let mut doc = doc_with_one_page();
        let table = doc.add_tagged_content(0, None, StructType::Table, &ContentBuilder::new(), None).unwrap();
        let row = doc.add_tagged_content(0, Some(table), StructType::TableRow, &ContentBuilder::new(), None).unwrap();
        let cell_content = ContentBuilder::new().text("F1", 10.0, 100.0, 700.0, "Cell A1");
        doc.add_tagged_content(0, Some(row), StructType::TableCell, &cell_content, None).unwrap();

        let figure_content = ContentBuilder::new(); // A real figure would `Do` an image XObject here.
        doc.add_tagged_content(0, None, StructType::Figure, &figure_content, Some("A bar chart showing quarterly revenue")).unwrap();

        let tree = doc.struct_tree().unwrap().unwrap();
        // Two top-level children: Table and Figure.
        assert_eq!(tree.children.len(), 2);
        let table_node = &tree.children[0];
        assert_eq!(table_node.struct_type, "Table");
        assert_eq!(table_node.children[0].struct_type, "TR");
        assert_eq!(table_node.children[0].children[0].struct_type, "TD");

        let figure_node = &tree.children[1];
        assert_eq!(figure_node.struct_type, "Figure");
        assert_eq!(figure_node.alt_text.as_deref(), Some("A bar chart showing quarterly revenue"));
    }

    #[test]
    fn test_struct_parents_key_reused_across_calls_on_same_page() {
        let mut doc = doc_with_one_page();
        doc.add_tagged_content(0, None, StructType::Paragraph, &ContentBuilder::new(), None).unwrap();
        doc.add_tagged_content(0, None, StructType::Paragraph, &ContentBuilder::new(), None).unwrap();

        let page_id = doc.page_id_at(0).unwrap();
        let page = doc.get_dictionary(page_id).unwrap();
        assert_eq!(page.get("StructParents"), Some(&Object::Integer(0)));

        let content = String::from_utf8_lossy(&doc.page_content_bytes(page_id).unwrap()).into_owned();
        assert!(content.contains("MCID 0"));
        assert!(content.contains("MCID 1"));
    }

    #[test]
    fn test_tagged_document_survives_incremental_save_and_lopdf_reads_it() {
        let mut doc = doc_with_one_page();
        let root = doc.add_document_structure_root().unwrap();
        doc.add_tagged_content(0, Some(root), StructType::Heading(1), &ContentBuilder::new().text("F1", 18.0, 72.0, 780.0, "Title"), None)
            .unwrap();

        let saved = doc.save_incremental_to_bytes().unwrap();
        lopdf::Document::load_mem(&saved).expect("lopdf must open the tagged document");

        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let tree = reopened.struct_tree().unwrap().unwrap();
        assert_eq!(tree.children[0].struct_type, "Document");
        assert_eq!(tree.children[0].children[0].struct_type, "H1");
    }

    #[test]
    fn test_no_struct_tree_returns_none() {
        let doc = doc_with_one_page();
        assert!(doc.struct_tree().unwrap().is_none());
    }
}
