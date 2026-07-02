//! Document outline (bookmark) tree read/write (ISO 32000-1:2008 Section
//! 12.3.3 "Document Outline") and named destinations (Section 7.7.4
//! "Name Dictionary" / 7.9.6 "Name Trees").

use super::graph::EditableDocument;
use super::util::{from_pdf_text_string, to_pdf_text_string};
use crate::error::{EditorError, PdfResult};
use crate::object::{Object, PdfArray, PdfDictionary, PdfName};
use crate::types::ObjectId;

/// Maximum number of outline nodes visited while listing/removing, or
/// object-graph nodes visited while resolving a destination, bounding
/// work against a corrupt/adversarial (e.g. cyclic `/Next`) outline tree.
const MAX_OUTLINE_NODES: usize = 200_000;

/// Maximum `/Parent` chain length walked while propagating a
/// `/Count` update up to the outline root.
const MAX_PARENT_DEPTH: usize = 1_000;

/// A page destination (ISO 32000-1 12.3.2, Table 151). Only the two most
/// common fit modes are modeled; other explicit-destination forms
/// (`/FitH`, `/FitV`, `/FitR`, `/FitB`, ...) round-trip fine through
/// [`EditableDocument::list_bookmarks`] as `Destination::Xyz`-shaped data
/// is not attempted for them - such a node is reported with `dest: None`
/// rather than guessed at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Destination {
    /// `[page /Fit]`: fit the whole page in the window.
    FitPage {
        /// 0-based target page index.
        page_index: usize,
    },
    /// `[page /XYZ left top zoom]`: scroll to `(left, top)` at `zoom`
    /// magnification (`None` in any position means "leave unchanged",
    /// per ISO 32000-1 Table 151).
    Xyz {
        /// 0-based target page index.
        page_index: usize,
        /// Left coordinate, in unrotated page space, or `None`.
        left: Option<f64>,
        /// Top coordinate, in unrotated page space, or `None`.
        top: Option<f64>,
        /// Zoom factor (1.0 = 100%), or `None`.
        zoom: Option<f64>,
    },
}

impl Destination {
    /// Convenience constructor: fit the page to the window.
    pub fn fit(page_index: usize) -> Self {
        Destination::FitPage { page_index }
    }

    fn page_index(&self) -> usize {
        match self {
            Destination::FitPage { page_index } | Destination::Xyz { page_index, .. } => *page_index,
        }
    }

    fn to_array(self, doc: &EditableDocument) -> PdfResult<PdfArray> {
        let page_id = doc.page_id_at(self.page_index())?;
        let mut arr = PdfArray::new();
        arr.push(Object::Reference(page_id));
        match self {
            Destination::FitPage { .. } => {
                arr.push(Object::Name(PdfName::new_unchecked("Fit")));
            }
            Destination::Xyz { left, top, zoom, .. } => {
                arr.push(Object::Name(PdfName::new_unchecked("XYZ")));
                arr.push(num_or_null(left));
                arr.push(num_or_null(top));
                arr.push(num_or_null(zoom));
            }
        }
        Ok(arr)
    }

    fn from_array(doc: &EditableDocument, arr: &PdfArray) -> Option<Destination> {
        let Object::Reference(page_id) = arr.get(0)? else { return None };
        let page_index = doc.page_ids().ok()?.into_iter().position(|id| id == *page_id)?;
        let Object::Name(kind) = arr.get(1)? else { return None };
        match kind.as_str() {
            "Fit" | "FitB" => Some(Destination::FitPage { page_index }),
            "XYZ" => Some(Destination::Xyz {
                page_index,
                left: arr.get(2).and_then(|o| o.as_real()),
                top: arr.get(3).and_then(|o| o.as_real()),
                zoom: arr.get(4).and_then(|o| o.as_real()),
            }),
            _ => None,
        }
    }
}

fn num_or_null(v: Option<f64>) -> Object {
    match v {
        Some(n) => Object::Real(n),
        None => Object::Null,
    }
}

/// One node of the outline (bookmark) tree, as returned by
/// [`EditableDocument::list_bookmarks`].
#[derive(Debug, Clone)]
pub struct BookmarkNode {
    /// The outline item's own object id (pass to
    /// [`EditableDocument::remove_bookmark`] or as the `parent` of
    /// [`EditableDocument::add_bookmark`]).
    pub id: ObjectId,
    /// The bookmark's display title (`/Title`).
    pub title: String,
    /// Where it points, if resolvable (direct array or indirect
    /// reference to one, or a named destination this document defines).
    pub dest: Option<Destination>,
    /// Child bookmarks, in document order.
    pub children: Vec<BookmarkNode>,
}

impl EditableDocument {
    // -- Bookmark tree -------------------------------------------------

    /// Returns the full outline (bookmark) tree, in document order.
    pub fn list_bookmarks(&self) -> PdfResult<Vec<BookmarkNode>> {
        let Ok(catalog) = self.catalog() else { return Ok(Vec::new()) };
        let Some(Object::Reference(outlines_id)) = catalog.get("Outlines") else { return Ok(Vec::new()) };
        let Ok(outlines) = self.get_dictionary(*outlines_id) else { return Ok(Vec::new()) };
        let Some(Object::Reference(first)) = outlines.get("First") else { return Ok(Vec::new()) };

        let mut visited = std::collections::HashSet::new();
        self.list_siblings(*first, &mut visited)
    }

    fn list_siblings(&self, mut node_id: ObjectId, visited: &mut std::collections::HashSet<ObjectId>) -> PdfResult<Vec<BookmarkNode>> {
        let mut out = Vec::new();
        loop {
            if visited.len() >= MAX_OUTLINE_NODES || !visited.insert(node_id) {
                break;
            }
            let Ok(dict) = self.get_dictionary(node_id) else { break };
            let title = match dict.get("Title") {
                Some(Object::String(s)) => from_pdf_text_string(s),
                _ => String::new(),
            };
            let dest = self.resolve_dest(&dict);
            let children = match dict.get("First") {
                Some(Object::Reference(first)) => self.list_siblings(*first, visited)?,
                _ => Vec::new(),
            };
            out.push(BookmarkNode { id: node_id, title, dest, children });

            match dict.get("Next") {
                Some(Object::Reference(next)) => node_id = *next,
                _ => break,
            }
        }
        Ok(out)
    }

    /// Resolves an outline item's (or, generically, any dictionary's)
    /// `/Dest`, or `/A` action with `/S /GoTo`, to a [`Destination`].
    fn resolve_dest(&self, dict: &PdfDictionary) -> Option<Destination> {
        if let Some(dest) = dict.get("Dest") {
            if let Some(d) = self.resolve_dest_value(dest) {
                return Some(d);
            }
        }
        if let Some(Object::Dictionary(action)) = dict.get("A") {
            if matches!(action.get("S"), Some(Object::Name(n)) if n.as_str() == "GoTo") {
                if let Some(d) = action.get("D") {
                    return self.resolve_dest_value(d);
                }
            }
        }
        None
    }

    fn resolve_dest_value(&self, dest: &Object) -> Option<Destination> {
        match dest {
            Object::Array(arr) => Destination::from_array(self, arr),
            Object::Reference(id) => match self.get_object(*id)? {
                Object::Array(arr) => Destination::from_array(self, &arr),
                _ => None,
            },
            Object::Name(n) => self.get_named_destination(n.as_str()).ok().flatten(),
            Object::String(s) => self.get_named_destination(&from_pdf_text_string(s)).ok().flatten(),
            _ => None,
        }
    }

    /// Adds a new bookmark titled `title` pointing at `dest`, as the last
    /// child of `parent` (or of the document's top-level outline if
    /// `parent` is `None`). Creates the `/Outlines` root (and sets
    /// `Catalog /Outlines`) if this is the document's first bookmark.
    /// Returns the new item's object id.
    pub fn add_bookmark(&mut self, parent: Option<ObjectId>, title: &str, dest: Destination) -> PdfResult<ObjectId> {
        let outlines_id = self.outlines_root_id()?;
        let parent_id = parent.unwrap_or(outlines_id);

        let dest_array = dest.to_array(self)?;
        let item_id = self.allocate_id();
        let mut item = PdfDictionary::new();
        item.set("Title", Object::String(to_pdf_text_string(title)));
        item.set("Parent", Object::Reference(parent_id));
        item.set("Dest", Object::Array(dest_array));
        self.set_object(item_id, Object::Dictionary(item));

        let mut parent_dict = self.get_dictionary(parent_id)?;
        match parent_dict.get("Last") {
            Some(Object::Reference(old_last)) => {
                let old_last = *old_last;
                let mut old_last_dict = self.get_dictionary(old_last)?;
                old_last_dict.set("Next", Object::Reference(item_id));
                self.set_object(old_last, Object::Dictionary(old_last_dict));

                let mut item_dict = self.get_dictionary(item_id)?;
                item_dict.set("Prev", Object::Reference(old_last));
                self.set_object(item_id, Object::Dictionary(item_dict));
            }
            _ => {
                parent_dict.set("First", Object::Reference(item_id));
            }
        }
        parent_dict.set("Last", Object::Reference(item_id));
        self.set_object(parent_id, Object::Dictionary(parent_dict));

        self.bump_ancestor_counts(parent_id, 1)?;
        Ok(item_id)
    }

    /// Removes a bookmark (and its entire subtree) from the outline tree,
    /// unlinking it from its siblings/parent. The node and its
    /// descendants become unreachable garbage, collected by
    /// [`EditableDocument::save_full_rewrite`].
    pub fn remove_bookmark(&mut self, id: ObjectId) -> PdfResult<()> {
        let dict = self
            .get_dictionary(id)
            .map_err(|_| EditorError::OutlineItemNotFound(id.number, id.generation))?;
        let Some(Object::Reference(parent_id)) = dict.get("Parent").cloned() else {
            return Err(EditorError::OutlineItemNotFound(id.number, id.generation).into());
        };
        let prev = match dict.get("Prev") {
            Some(Object::Reference(p)) => Some(*p),
            _ => None,
        };
        let next = match dict.get("Next") {
            Some(Object::Reference(n)) => Some(*n),
            _ => None,
        };

        match prev {
            Some(p) => {
                let mut d = self.get_dictionary(p)?;
                match next {
                    Some(n) => d.set("Next", Object::Reference(n)),
                    None => {
                        d.remove("Next");
                    }
                }
                self.set_object(p, Object::Dictionary(d));
            }
            None => {
                let mut parent_dict = self.get_dictionary(parent_id)?;
                match next {
                    Some(n) => parent_dict.set("First", Object::Reference(n)),
                    None => {
                        parent_dict.remove("First");
                    }
                }
                self.set_object(parent_id, Object::Dictionary(parent_dict));
            }
        }
        match next {
            Some(n) => {
                let mut d = self.get_dictionary(n)?;
                match prev {
                    Some(p) => d.set("Prev", Object::Reference(p)),
                    None => {
                        d.remove("Prev");
                    }
                }
                self.set_object(n, Object::Dictionary(d));
            }
            None => {
                let mut parent_dict = self.get_dictionary(parent_id)?;
                match prev {
                    Some(p) => parent_dict.set("Last", Object::Reference(p)),
                    None => {
                        parent_dict.remove("Last");
                    }
                }
                self.set_object(parent_id, Object::Dictionary(parent_dict));
            }
        }

        let removed = self.count_subtree(id)?;
        self.bump_ancestor_counts(parent_id, -(removed as i64))?;
        Ok(())
    }

    fn outlines_root_id(&mut self) -> PdfResult<ObjectId> {
        let mut catalog = self.catalog()?;
        if let Some(Object::Reference(id)) = catalog.get("Outlines") {
            return Ok(*id);
        }
        let id = self.allocate_id();
        let mut outlines = PdfDictionary::new();
        outlines.set("Type", Object::Name(PdfName::new_unchecked("Outlines")));
        self.set_object(id, Object::Dictionary(outlines));
        catalog.set("Outlines", Object::Reference(id));
        let root = self.catalog_id();
        self.set_object(root, Object::Dictionary(catalog));
        Ok(id)
    }

    /// Adds `delta` to `/Count` (ISO 32000-1 Table 152/153) of `start`
    /// and every ancestor up to (and including) the outline root.
    fn bump_ancestor_counts(&mut self, start: ObjectId, delta: i64) -> PdfResult<()> {
        let mut id = start;
        for _ in 0..MAX_PARENT_DEPTH {
            let mut dict = self.get_dictionary(id)?;
            let current = match dict.get("Count") {
                Some(Object::Integer(c)) => *c,
                _ => 0,
            };
            dict.set("Count", Object::Integer(current + delta));
            let parent = match dict.get("Parent") {
                Some(Object::Reference(p)) => Some(*p),
                _ => None,
            };
            self.set_object(id, Object::Dictionary(dict));
            match parent {
                Some(p) => id = p,
                None => break,
            }
        }
        Ok(())
    }

    fn count_subtree(&self, id: ObjectId) -> PdfResult<usize> {
        let mut visited = std::collections::HashSet::new();
        self.count_subtree_inner(id, &mut visited)
    }

    fn count_subtree_inner(&self, id: ObjectId, visited: &mut std::collections::HashSet<ObjectId>) -> PdfResult<usize> {
        if visited.len() >= MAX_OUTLINE_NODES || !visited.insert(id) {
            return Ok(0);
        }
        let mut total = 1;
        if let Ok(dict) = self.get_dictionary(id) {
            if let Some(Object::Reference(first)) = dict.get("First") {
                let mut sibling = Some(*first);
                while let Some(sid) = sibling {
                    if visited.len() >= MAX_OUTLINE_NODES {
                        break;
                    }
                    total += self.count_subtree_inner(sid, visited)?;
                    let sdict = self.get_dictionary(sid)?;
                    sibling = match sdict.get("Next") {
                        Some(Object::Reference(n)) => Some(*n),
                        _ => None,
                    };
                }
            }
        }
        Ok(total)
    }

    // -- Named destinations (7.7.4 / 7.9.6) -------------------------------

    /// Adds (or replaces) a named destination in the document's
    /// `/Names /Dests` name tree (ISO 32000-1 7.7.4, 7.9.6), stored as a
    /// single, sorted leaf node - always spec-legal regardless of how
    /// many entries it holds, if simpler than a balanced multi-level
    /// tree for the sizes this crate expects to handle.
    pub fn add_named_destination(&mut self, name: &str, dest: Destination) -> PdfResult<()> {
        let dest_array = dest.to_array(self)?;
        let dests_id = self.names_dests_id()?;
        let mut dests = self.get_dictionary(dests_id)?;
        let mut names = match dests.get("Names") {
            Some(Object::Array(a)) => a.clone(),
            _ => PdfArray::new(),
        };

        // Replace an existing entry with the same name if present, else
        // insert keeping the array sorted by name (required for a
        // conformant name-tree leaf, ISO 32000-1 7.9.6).
        let mut pairs: Vec<(String, Object)> = names
            .iter()
            .collect::<Vec<_>>()
            .chunks_exact(2)
            .filter_map(|c| match c[0] {
                Object::String(ref s) => Some((from_pdf_text_string(s), c[1].clone())),
                _ => None,
            })
            .collect();
        pairs.retain(|(n, _)| n != name);
        pairs.push((name.to_string(), Object::Array(dest_array)));
        pairs.sort_by(|a, b| a.0.cmp(&b.0));

        names = PdfArray::new();
        for (n, v) in pairs {
            names.push(Object::String(to_pdf_text_string(&n)));
            names.push(v);
        }
        dests.set("Names", Object::Array(names));
        self.set_object(dests_id, Object::Dictionary(dests));
        Ok(())
    }

    /// Looks up a named destination by name.
    pub fn get_named_destination(&self, name: &str) -> PdfResult<Option<Destination>> {
        let Ok(catalog) = self.catalog() else { return Ok(None) };
        let Some(Object::Reference(names_id)) = catalog.get("Names") else { return Ok(None) };
        let Ok(names_dict) = self.get_dictionary(*names_id) else { return Ok(None) };
        let Some(Object::Reference(dests_id)) = names_dict.get("Dests") else { return Ok(None) };
        let Ok(dests) = self.get_dictionary(*dests_id) else { return Ok(None) };
        let Some(Object::Array(arr)) = dests.get("Names") else { return Ok(None) };

        for pair in arr.as_slice().chunks_exact(2) {
            if let Object::String(s) = &pair[0] {
                if from_pdf_text_string(s) == name {
                    return Ok(self.resolve_dest_value(&pair[1]));
                }
            }
        }
        Ok(None)
    }

    /// Lists every named destination's name, in tree (sorted) order.
    pub fn named_destinations(&self) -> PdfResult<Vec<String>> {
        let Ok(catalog) = self.catalog() else { return Ok(Vec::new()) };
        let Some(Object::Reference(names_id)) = catalog.get("Names") else { return Ok(Vec::new()) };
        let Ok(names_dict) = self.get_dictionary(*names_id) else { return Ok(Vec::new()) };
        let Some(Object::Reference(dests_id)) = names_dict.get("Dests") else { return Ok(Vec::new()) };
        let Ok(dests) = self.get_dictionary(*dests_id) else { return Ok(Vec::new()) };
        let Some(Object::Array(arr)) = dests.get("Names") else { return Ok(Vec::new()) };

        Ok(arr
            .as_slice()
            .chunks_exact(2)
            .filter_map(|pair| match &pair[0] {
                Object::String(s) => Some(from_pdf_text_string(s)),
                _ => None,
            })
            .collect())
    }

    fn names_dests_id(&mut self) -> PdfResult<ObjectId> {
        let mut catalog = self.catalog()?;
        let names_id = match catalog.get("Names") {
            Some(Object::Reference(id)) => *id,
            _ => {
                let id = self.allocate_id();
                self.set_object(id, Object::Dictionary(PdfDictionary::new()));
                catalog.set("Names", Object::Reference(id));
                let root = self.catalog_id();
                self.set_object(root, Object::Dictionary(catalog));
                id
            }
        };
        let mut names_dict = self.get_dictionary(names_id)?;
        if let Some(Object::Reference(id)) = names_dict.get("Dests") {
            return Ok(*id);
        }
        let dests_id = self.allocate_id();
        let mut dests = PdfDictionary::new();
        dests.set("Names", Object::Array(PdfArray::new()));
        self.set_object(dests_id, Object::Dictionary(dests));
        names_dict.set("Dests", Object::Reference(dests_id));
        self.set_object(names_id, Object::Dictionary(names_dict));
        Ok(dests_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    fn doc_with_pages(n: usize) -> EditableDocument {
        let mut builder = DocumentBuilder::new();
        for i in 0..n {
            let page = PageBuilder::a4()
                .font("F1", Standard14Font::Helvetica)
                .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, &format!("Page {i}")))
                .build();
            builder = builder.page(page);
        }
        let bytes = builder.build().unwrap().save_to_bytes().unwrap();
        EditableDocument::from_bytes(bytes).unwrap()
    }

    #[test]
    fn test_add_and_list_top_level_bookmarks() {
        let mut doc = doc_with_pages(3);
        doc.add_bookmark(None, "Chapter 1", Destination::fit(0)).unwrap();
        doc.add_bookmark(None, "Chapter 2", Destination::fit(1)).unwrap();

        let bookmarks = doc.list_bookmarks().unwrap();
        assert_eq!(bookmarks.len(), 2);
        assert_eq!(bookmarks[0].title, "Chapter 1");
        assert_eq!(bookmarks[0].dest, Some(Destination::FitPage { page_index: 0 }));
        assert_eq!(bookmarks[1].title, "Chapter 2");
        assert_eq!(bookmarks[1].dest, Some(Destination::FitPage { page_index: 1 }));
    }

    #[test]
    fn test_add_nested_bookmark() {
        let mut doc = doc_with_pages(3);
        let parent = doc.add_bookmark(None, "Part I", Destination::fit(0)).unwrap();
        doc.add_bookmark(Some(parent), "Section 1.1", Destination::fit(1)).unwrap();
        doc.add_bookmark(Some(parent), "Section 1.2", Destination::fit(2)).unwrap();

        let bookmarks = doc.list_bookmarks().unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].children.len(), 2);
        assert_eq!(bookmarks[0].children[0].title, "Section 1.1");
        assert_eq!(bookmarks[0].children[1].title, "Section 1.2");
    }

    #[test]
    fn test_xyz_destination_round_trips() {
        let mut doc = doc_with_pages(1);
        doc.add_bookmark(None, "Top", Destination::Xyz { page_index: 0, left: Some(10.0), top: Some(700.0), zoom: Some(1.5) })
            .unwrap();
        let bookmarks = doc.list_bookmarks().unwrap();
        assert_eq!(
            bookmarks[0].dest,
            Some(Destination::Xyz { page_index: 0, left: Some(10.0), top: Some(700.0), zoom: Some(1.5) })
        );
    }

    #[test]
    fn test_bookmark_survives_incremental_save() {
        let mut doc = doc_with_pages(2);
        doc.add_bookmark(None, "Intro", Destination::fit(0)).unwrap();
        let saved = doc.save_incremental_to_bytes().unwrap();

        let lopdf_doc = lopdf::Document::load_mem(&saved).expect("lopdf must open the file");
        assert!(!lopdf_doc.get_pages().is_empty());

        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let bookmarks = reopened.list_bookmarks().unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].title, "Intro");
    }

    #[test]
    fn test_remove_bookmark_relinks_siblings() {
        let mut doc = doc_with_pages(3);
        doc.add_bookmark(None, "One", Destination::fit(0)).unwrap();
        let two = doc.add_bookmark(None, "Two", Destination::fit(1)).unwrap();
        doc.add_bookmark(None, "Three", Destination::fit(2)).unwrap();

        doc.remove_bookmark(two).unwrap();
        let bookmarks = doc.list_bookmarks().unwrap();
        let titles: Vec<_> = bookmarks.iter().map(|b| b.title.as_str()).collect();
        assert_eq!(titles, vec!["One", "Three"]);
    }

    #[test]
    fn test_remove_first_and_last_bookmark() {
        let mut doc = doc_with_pages(2);
        let one = doc.add_bookmark(None, "One", Destination::fit(0)).unwrap();
        let two = doc.add_bookmark(None, "Two", Destination::fit(1)).unwrap();

        doc.remove_bookmark(one).unwrap();
        assert_eq!(doc.list_bookmarks().unwrap()[0].title, "Two");

        doc.remove_bookmark(two).unwrap();
        assert!(doc.list_bookmarks().unwrap().is_empty());
    }

    #[test]
    fn test_remove_nonexistent_bookmark_errors() {
        let mut doc = doc_with_pages(1);
        assert!(doc.remove_bookmark(ObjectId::new(9999)).is_err());
    }

    #[test]
    fn test_named_destination_round_trip() {
        let mut doc = doc_with_pages(3);
        doc.add_named_destination("chapter2", Destination::fit(1)).unwrap();
        doc.add_named_destination("appendix", Destination::fit(2)).unwrap();

        assert_eq!(doc.get_named_destination("chapter2").unwrap(), Some(Destination::FitPage { page_index: 1 }));
        let mut names = doc.named_destinations().unwrap();
        names.sort();
        assert_eq!(names, vec!["appendix", "chapter2"]);
        assert_eq!(doc.get_named_destination("missing").unwrap(), None);
    }

    #[test]
    fn test_named_destination_survives_incremental_save() {
        let mut doc = doc_with_pages(2);
        doc.add_named_destination("start", Destination::fit(0)).unwrap();
        let saved = doc.save_incremental_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert_eq!(reopened.get_named_destination("start").unwrap(), Some(Destination::FitPage { page_index: 0 }));
    }

    #[test]
    fn test_add_named_destination_replaces_existing() {
        let mut doc = doc_with_pages(3);
        doc.add_named_destination("target", Destination::fit(0)).unwrap();
        doc.add_named_destination("target", Destination::fit(2)).unwrap();
        assert_eq!(doc.get_named_destination("target").unwrap(), Some(Destination::FitPage { page_index: 2 }));
        assert_eq!(doc.named_destinations().unwrap().len(), 1);
    }

    #[test]
    fn test_bookmark_pointing_at_out_of_range_page_errors() {
        let mut doc = doc_with_pages(1);
        assert!(doc.add_bookmark(None, "Bad", Destination::fit(5)).is_err());
    }

    #[test]
    fn test_cyclic_next_chain_does_not_hang() {
        // Two outline items whose /Next pointers form a 2-cycle.
        let mut doc = doc_with_pages(1);
        let a = doc.allocate_id();
        let b = doc.allocate_id();
        let mut a_dict = PdfDictionary::new();
        a_dict.set("Title", Object::String(crate::object::PdfString::literal("A")));
        a_dict.set("Next", Object::Reference(b));
        doc.set_object(a, Object::Dictionary(a_dict));
        let mut b_dict = PdfDictionary::new();
        b_dict.set("Title", Object::String(crate::object::PdfString::literal("B")));
        b_dict.set("Next", Object::Reference(a));
        doc.set_object(b, Object::Dictionary(b_dict));

        let outlines_id = doc.allocate_id();
        let mut outlines = PdfDictionary::new();
        outlines.set("Type", Object::Name(PdfName::new_unchecked("Outlines")));
        outlines.set("First", Object::Reference(a));
        doc.set_object(outlines_id, Object::Dictionary(outlines));
        let mut catalog = doc.catalog().unwrap();
        catalog.set("Outlines", Object::Reference(outlines_id));
        let root = doc.catalog_id();
        doc.set_object(root, Object::Dictionary(catalog));

        // Must terminate.
        let bookmarks = doc.list_bookmarks().unwrap();
        assert_eq!(bookmarks.len(), 2);
    }
}
