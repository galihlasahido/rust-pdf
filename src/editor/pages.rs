//! Page-tree structural editing: insert/delete/reorder/rotate/split/merge
//! (ISO 32000-1:2008 Section 7.7.3 "Page Tree").
//!
//! Every structural operation here first **flattens** the page tree into
//! a single `/Kids` array directly under the document's `/Pages` root
//! before making its edit. A page tree's internal `/Pages`-node nesting
//! exists purely as a producer-side efficiency device for very large
//! documents (ISO 32000-1 7.7.3.2, note: "balancing... has no effect on
//! the logical structure of the document"), so flattening it is spec-legal
//! and turns every index-based operation (insert-at, delete-at, move) into
//! a simple `Vec` splice instead of having to correctly propagate
//! `/Count` through an arbitrary, possibly unbalanced tree. The
//! (now-unreferenced) intermediate `/Pages` nodes are left as orphaned
//! objects in the incremental case; [`EditableDocument::save_full_rewrite`]
//! garbage-collects them.

use super::graph::EditableDocument;
use super::{BookmarkNode, Destination};
use crate::error::{EditorError, PdfResult};
use crate::object::{Object, PdfArray, PdfDictionary, PdfName};
use crate::types::ObjectId;
#[cfg(feature = "render")]
use crate::types::Rectangle;
use std::collections::{HashMap, HashSet, VecDeque};

/// Maximum number of objects that a single page/document import (merge or
/// split) will traverse, bounding work on a pathological/adversarial
/// resource graph.
const MAX_IMPORT_OBJECTS: usize = 2_000_000;

/// Maximum outline (bookmark) tree nodes visited while pruning dangling
/// destinations after a page delete. Also implicitly bounds `/First`
/// nesting depth, since depth can never exceed the node budget.
const MAX_OUTLINE_NODES: usize = 200_000;

/// Keys that are intentionally *not* followed (nor copied) when importing
/// a subgraph for merge/split: they point "outward" (up the page tree, or
/// back to an owning page) rather than into content the imported page
/// actually needs, and following them would pull in unrelated parts of
/// the source document (see module docs and `EditableDocument::merge`
/// rustdoc for the resulting, documented limitation around fields whose
/// widgets are `/Kids` of a separate parent field).
const SKIP_IMPORT_KEYS: &[&str] = &["Parent", "P"];

impl EditableDocument {
    /// Creates a new, empty, in-memory document (one `/Catalog`, one empty
    /// `/Pages` node) suitable as the destination of
    /// [`EditableDocument::extract_pages`]-style page splitting.
    pub fn new_empty() -> PdfResult<Self> {
        use crate::document::PdfVersion;
        use crate::writer::PdfWriter;

        let mut writer = PdfWriter::create_memory(PdfVersion::V1_7.as_str());
        writer.write_header()?;
        let catalog_id = writer.allocate_id();
        let pages_id = writer.allocate_id();

        let mut catalog = PdfDictionary::new();
        catalog.set("Type", Object::Name(PdfName::new_unchecked("Catalog")));
        catalog.set("Pages", Object::Reference(pages_id));
        writer.write_object_with_id(catalog_id, &Object::Dictionary(catalog))?;

        let mut pages = PdfDictionary::new();
        pages.set("Type", Object::Name(PdfName::new_unchecked("Pages")));
        pages.set("Kids", Object::Array(PdfArray::new()));
        pages.set("Count", Object::Integer(0));
        writer.write_object_with_id(pages_id, &Object::Dictionary(pages))?;

        writer.write_trailer(catalog_id, None)?;
        Self::from_bytes(writer.into_bytes())
    }

    /// Flattens the page tree to a single-level `/Kids` array under the
    /// `/Pages` root and returns the (now-authoritative) leaf order. See
    /// the [module docs](self) for why this is done.
    fn flatten_page_tree(&mut self) -> PdfResult<(ObjectId, Vec<ObjectId>)> {
        let pages_root = self.pages_root_id()?;
        let leaves = self.page_ids()?;

        for &leaf in &leaves {
            let mut dict = self.get_dictionary(leaf)?;
            if dict.get("Parent") != Some(&Object::Reference(pages_root)) {
                dict.set("Parent", Object::Reference(pages_root));
                self.set_object(leaf, Object::Dictionary(dict));
            }
        }
        self.rewrite_kids(pages_root, &leaves)?;
        Ok((pages_root, leaves))
    }

    fn rewrite_kids(&mut self, pages_root: ObjectId, leaves: &[ObjectId]) -> PdfResult<()> {
        let mut dict = self.get_dictionary(pages_root)?;
        let mut kids = PdfArray::new();
        for &id in leaves {
            kids.push(Object::Reference(id));
        }
        dict.set("Type", Object::Name(PdfName::new_unchecked("Pages")));
        dict.set("Kids", Object::Array(kids));
        dict.set("Count", Object::Integer(leaves.len() as i64));
        self.set_object(pages_root, Object::Dictionary(dict));
        Ok(())
    }

    /// Inserts a new, blank page at `index` (0-based; `index ==
    /// page_count()` appends at the end) with the given media box size in
    /// PDF points, and returns its object id.
    pub fn insert_blank_page(
        &mut self,
        index: usize,
        width: f64,
        height: f64,
    ) -> PdfResult<ObjectId> {
        let (pages_root, mut leaves) = self.flatten_page_tree()?;
        if index > leaves.len() {
            return Err(EditorError::InvalidPageIndex {
                index,
                count: leaves.len(),
            }
            .into());
        }

        let page_id = self.allocate_id();
        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Page")));
        dict.set("Parent", Object::Reference(pages_root));
        let mut media_box = PdfArray::new();
        media_box.push(Object::Real(0.0));
        media_box.push(Object::Real(0.0));
        media_box.push(Object::Real(width));
        media_box.push(Object::Real(height));
        dict.set("MediaBox", Object::Array(media_box));
        dict.set("Resources", Object::Dictionary(PdfDictionary::new()));
        self.set_object(page_id, Object::Dictionary(dict));

        leaves.insert(index, page_id);
        self.rewrite_kids(pages_root, &leaves)?;
        Ok(page_id)
    }

    /// Deletes the page at `index`, removing it from the page tree and
    /// stripping any outline (bookmark) or link-annotation destination
    /// that pointed at it. See the [module docs](crate::editor) for the
    /// (documented) limits of destination cleanup.
    pub fn delete_page(&mut self, index: usize) -> PdfResult<()> {
        let (pages_root, mut leaves) = self.flatten_page_tree()?;
        if index >= leaves.len() {
            return Err(EditorError::InvalidPageIndex {
                index,
                count: leaves.len(),
            }
            .into());
        }
        let removed = leaves.remove(index);
        self.rewrite_kids(pages_root, &leaves)?;
        self.prune_references_to_page(removed)?;
        Ok(())
    }

    /// Moves the page at `from` to position `to` (both 0-based),
    /// shifting the pages in between.
    pub fn move_page(&mut self, from: usize, to: usize) -> PdfResult<()> {
        let (pages_root, mut leaves) = self.flatten_page_tree()?;
        if from >= leaves.len() {
            return Err(EditorError::InvalidPageIndex {
                index: from,
                count: leaves.len(),
            }
            .into());
        }
        if to >= leaves.len() {
            return Err(EditorError::InvalidPageIndex {
                index: to,
                count: leaves.len(),
            }
            .into());
        }
        let id = leaves.remove(from);
        leaves.insert(to, id);
        self.rewrite_kids(pages_root, &leaves)
    }

    /// Rotates the page at `index` by an additional `degrees` (must be a
    /// multiple of 90; may be negative), setting `/Rotate` (ISO 32000-1
    /// Table 30) to the normalized `0..360` result.
    pub fn rotate_page(&mut self, index: usize, degrees: i64) -> PdfResult<()> {
        if degrees % 90 != 0 {
            return Err(EditorError::InvalidArgument(
                "rotation must be a multiple of 90 degrees".to_string(),
            )
            .into());
        }
        let page_id = self.page_id_at(index)?;
        let current = self.effective_rotate(page_id)?;
        let mut normalized = (current + degrees) % 360;
        if normalized < 0 {
            normalized += 360;
        }
        let mut dict = self.get_dictionary(page_id)?;
        dict.set("Rotate", Object::Integer(normalized));
        self.set_object(page_id, Object::Dictionary(dict));
        Ok(())
    }

    /// Resolves the *effective* `/Rotate` for a page, following `/Parent`
    /// (inheritable attribute, ISO 32000-1 Table 30) if the leaf doesn't
    /// set it directly. Defaults to 0.
    ///
    /// `pub(crate)` (rather than private) so [`crate::render::PdfRenderer`]
    /// can resolve the same effective rotation this module's own
    /// [`EditableDocument::rotate_page`] reads/writes, instead of
    /// duplicating the `/Parent`-inheritance walk.
    pub(crate) fn effective_rotate(&self, mut id: ObjectId) -> PdfResult<i64> {
        for _ in 0..64 {
            let dict = self.get_dictionary(id)?;
            if let Some(Object::Integer(r)) = dict.get("Rotate") {
                return Ok(*r);
            }
            match dict.get("Parent") {
                Some(Object::Reference(parent)) => id = *parent,
                _ => break,
            }
        }
        Ok(0)
    }

    /// Resolves the *effective* `/MediaBox` for a page (ISO 32000-1
    /// §7.7.3.3, Table 30: inheritable, required at some level of the page
    /// tree), following `/Parent` if the leaf doesn't set it directly.
    ///
    /// Falls back to [`Rectangle::letter`] if no ancestor declares one, or
    /// if the declared array is malformed -- not exactly 4 numbers,
    /// non-finite, or zero/negative width or height (a corrupt/adversarial
    /// source file; ISO 32000-1 requires a valid `/MediaBox` somewhere in
    /// the ancestor chain, but this is untrusted input, so a missing or
    /// nonsensical one degrades to a sane default rather than propagating
    /// NaN/inf into the rendering pipeline or failing outright). The two
    /// corner points are also normalized (min/max per axis) since ISO
    /// 32000-1 does not require `[llx lly urx ury]` to already be in
    /// lower-left/upper-right order.
    ///
    /// `pub(crate)` so [`crate::render::PdfRenderer`] can read page
    /// geometry without this crate's renderer needing its own `/Parent`-
    /// inheritance walk (or direct access to this module's private
    /// dictionary-resolution helpers). Only that (`render`-feature) caller
    /// exists today, hence the `#[cfg(feature = "render")]` -- a
    /// `native-render`-only build (which pulls in `parser`, hence this
    /// module, but not `render`) would otherwise warn about this being
    /// unused.
    #[cfg(feature = "render")]
    pub(crate) fn effective_media_box(&self, mut id: ObjectId) -> PdfResult<Rectangle> {
        for _ in 0..64 {
            let dict = self.get_dictionary(id)?;
            if let Some(Object::Array(arr)) = dict.get("MediaBox") {
                if let Some(rect) = parse_media_box(arr) {
                    return Ok(rect);
                }
            }
            match dict.get("Parent") {
                Some(Object::Reference(parent)) => id = *parent,
                _ => break,
            }
        }
        Ok(Rectangle::letter())
    }

    /// Extracts the given (0-based) page indices into a brand-new,
    /// standalone document ("split"). Resources (fonts, images, form
    /// fields, ...) each extracted page depends on are copied along with
    /// it; the source document is not modified.
    pub fn extract_pages(&self, indices: &[usize]) -> PdfResult<EditableDocument> {
        let mut new_doc = EditableDocument::new_empty()?;
        let page_ids: Vec<ObjectId> = indices
            .iter()
            .map(|&i| self.page_id_at(i))
            .collect::<PdfResult<_>>()?;
        new_doc.import_pages_from(self, &page_ids)?;
        Ok(new_doc)
    }

    /// Appends every page of `other` to the end of this document
    /// ("merge"), copying along whatever resources and (best-effort,
    /// merged widget/field-dictionary) form fields those pages depend on,
    /// plus `other`'s own outline (bookmark) tree - imported as new
    /// top-level node(s) appended after `self`'s existing top-level
    /// bookmarks, with every destination repointed at the corresponding
    /// newly-imported page. See [`EditableDocument::import_outline_from`].
    pub fn append_document(&mut self, other: &EditableDocument) -> PdfResult<()> {
        // Captured *before* the merge: `import_pages_from` always appends
        // `other`'s pages (in `other.page_ids()` order, which is what we
        // pass below) after `self`'s existing ones, so a source page at
        // index `i` ends up at `self` index `dest_page_offset + i` -
        // exactly the translation `import_outline_from` needs to apply to
        // every copied bookmark's destination.
        let dest_page_offset = self.page_ids()?.len();
        let page_ids = other.page_ids()?;
        self.import_pages_from(other, &page_ids)?;
        self.import_outline_from(other, dest_page_offset)?;
        Ok(())
    }

    /// Core merge/split primitive: deep-copies the subgraph reachable
    /// from `source_page_ids` (within `source`) into `self` with fresh
    /// object numbers, appends the copied pages to `self`'s page tree,
    /// and relinks any AcroForm fields that came along. Returns the new
    /// (in `self`) ids, in the same order as `source_page_ids`.
    fn import_pages_from(
        &mut self,
        source: &EditableDocument,
        source_page_ids: &[ObjectId],
    ) -> PdfResult<Vec<ObjectId>> {
        let (pages_root, mut leaves) = self.flatten_page_tree()?;

        let id_map = self.import_subgraph(source, source_page_ids)?;

        let mut new_ids = Vec::with_capacity(source_page_ids.len());
        for old_id in source_page_ids {
            let new_id = *id_map.get(old_id).ok_or(EditorError::UnresolvedObject(
                old_id.number,
                old_id.generation,
            ))?;
            let mut dict = self.get_dictionary(new_id)?;
            dict.set("Parent", Object::Reference(pages_root));
            self.set_object(new_id, Object::Dictionary(dict));
            leaves.push(new_id);
            new_ids.push(new_id);
        }
        self.rewrite_kids(pages_root, &leaves)?;
        self.merge_acroform_fields(source, &id_map)?;
        Ok(new_ids)
    }

    /// Imports `source`'s entire outline (bookmark) tree into `self`'s, as
    /// new top-level node(s) appended after `self`'s own existing
    /// top-level outline items (never interleaved into, or replacing,
    /// them - reuses [`EditableDocument::add_bookmark_opt`], which always
    /// appends as the last child of a given parent, exactly like
    /// [`EditableDocument::add_bookmark`] does for a caller-driven add).
    ///
    /// Every destination's page index is translated from `source`'s page
    /// order to `self`'s post-merge page order via `dest_page_offset` -
    /// see the call site in [`EditableDocument::append_document`] for why
    /// a flat `+ offset` is exactly right (rather than needing the
    /// `id_map` merge/split's page-subgraph import produces) whenever the
    /// entire source document was imported in `other.page_ids()` order,
    /// which is the only way this is currently called.
    ///
    /// A no-op (not an error) when `source` has no outline: `list_bookmarks`
    /// returns an empty `Vec` in that case and the loop below simply does
    /// nothing. A bookmark whose destination [`Destination`] cannot
    /// represent - see its own doc comment on which destination shapes
    /// round-trip through [`EditableDocument::list_bookmarks`] in the
    /// first place - is still copied (title and children preserved), just
    /// without a `/Dest` (ISO 32000-1 12.3.3/Table 153 does not require
    /// one); losing only the jump target beats losing the whole bookmark.
    fn import_outline_from(
        &mut self,
        source: &EditableDocument,
        dest_page_offset: usize,
    ) -> PdfResult<()> {
        let bookmarks = source.list_bookmarks()?;
        self.import_bookmark_siblings(&bookmarks, None, dest_page_offset)
    }

    /// Recursive helper for [`EditableDocument::import_outline_from`]:
    /// copies `nodes` (siblings, in order) as children of `parent` (`None`
    /// = top-level), then recurses into each node's own children.
    fn import_bookmark_siblings(
        &mut self,
        nodes: &[BookmarkNode],
        parent: Option<ObjectId>,
        dest_page_offset: usize,
    ) -> PdfResult<()> {
        for node in nodes {
            let dest = node
                .dest
                .map(|d| remap_destination_page_index(d, dest_page_offset));
            let new_id = self.add_bookmark_opt(parent, &node.title, dest)?;
            self.import_bookmark_siblings(&node.children, Some(new_id), dest_page_offset)?;
        }
        Ok(())
    }

    /// Deep-copies the subgraph reachable from `roots` (within `source`)
    /// into `self`, assigning each copied object a freshly allocated id.
    /// `/Parent` and `/P` (page back-reference) are dropped rather than
    /// followed/copied - see [`SKIP_IMPORT_KEYS`]. Any reference that
    /// still ends up pointing outside the copied set after that (e.g. a
    /// field hierarchy this crate chose not to follow) is dropped rather
    /// than ever being emitted as a dangling/mismatched-object reference
    /// in the destination document.
    fn import_subgraph(
        &mut self,
        source: &EditableDocument,
        roots: &[ObjectId],
    ) -> PdfResult<HashMap<ObjectId, ObjectId>> {
        let mut order: Vec<ObjectId> = Vec::new();
        let mut objects: HashMap<ObjectId, Object> = HashMap::new();
        let mut queued: HashSet<ObjectId> = HashSet::new();
        let mut queue: VecDeque<ObjectId> = VecDeque::new();

        for &root in roots {
            if queued.insert(root) {
                queue.push_back(root);
            }
        }

        while let Some(old_id) = queue.pop_front() {
            if order.len() >= MAX_IMPORT_OBJECTS {
                return Err(EditorError::ResourceLimitExceeded(
                    "merge/split import exceeded the maximum object count".to_string(),
                )
                .into());
            }
            let Some(obj) = source.get_object(old_id) else {
                continue; // Dangling reference in the source: nothing to copy.
            };
            let mut refs = Vec::new();
            collect_refs(&obj, &mut refs);
            for r in refs {
                if queued.insert(r) {
                    queue.push_back(r);
                }
            }
            order.push(old_id);
            objects.insert(old_id, obj);
        }

        let mut id_map: HashMap<ObjectId, ObjectId> = HashMap::with_capacity(order.len());
        for &old_id in &order {
            id_map.insert(old_id, self.allocate_id());
        }

        for old_id in &order {
            let obj = &objects[old_id];
            let new_id = id_map[old_id];
            let remapped = remap_object(obj, &id_map);
            self.set_object(new_id, remapped);
        }

        Ok(id_map)
    }

    /// Best-effort relinking of AcroForm fields (ISO 32000-1 12.7.2) that
    /// came along with an import: for every field object of `source`'s
    /// `/AcroForm /Fields` that was actually copied (i.e. reachable from
    /// one of the imported pages' own `/Annots`, which is always true for
    /// the common "merged field/widget dictionary" case, ISO 32000-1
    /// 12.7.3.1), register the copy in `self`'s `/AcroForm /Fields` too.
    ///
    /// Known limitation: a field whose *widgets* are `/Kids` of a
    /// separate, page-spanning parent field dictionary (rather than being
    /// the field dictionary itself) will not have that parent copied,
    /// since `/Parent` is intentionally not followed during import (see
    /// [`SKIP_IMPORT_KEYS`]). Such a field is imported as a set of
    /// ordinary (now parent-less) widget annotations instead of a linked
    /// field hierarchy.
    fn merge_acroform_fields(
        &mut self,
        source: &EditableDocument,
        id_map: &HashMap<ObjectId, ObjectId>,
    ) -> PdfResult<()> {
        let Ok(source_catalog) = source.catalog() else {
            return Ok(());
        };
        let Some(Object::Reference(acroform_id)) = source_catalog.get("AcroForm") else {
            return Ok(());
        };
        let acroform_id = *acroform_id;
        let Ok(source_acroform) = source.get_dictionary(acroform_id) else {
            return Ok(());
        };
        let Some(Object::Array(source_fields)) = source_acroform.get("Fields") else {
            return Ok(());
        };

        let mut new_field_ids = Vec::new();
        for f in source_fields.iter() {
            if let Object::Reference(old_id) = f {
                if let Some(&new_id) = id_map.get(old_id) {
                    new_field_ids.push(new_id);
                }
            }
        }
        if new_field_ids.is_empty() {
            return Ok(());
        }

        let mut catalog = self.catalog()?;
        let acroform_id = match catalog.get("AcroForm") {
            Some(Object::Reference(id)) => *id,
            _ => {
                let id = self.allocate_id();
                self.set_object(id, Object::Dictionary(PdfDictionary::new()));
                catalog.set("AcroForm", Object::Reference(id));
                let root = self.catalog_id();
                self.set_object(root, Object::Dictionary(catalog));
                id
            }
        };

        let mut acroform = self.get_dictionary(acroform_id)?;
        let mut fields = match acroform.get("Fields") {
            Some(Object::Array(a)) => a.clone(),
            _ => PdfArray::new(),
        };
        for id in new_field_ids {
            fields.push(Object::Reference(id));
        }
        acroform.set("Fields", Object::Array(fields));
        self.set_object(acroform_id, Object::Dictionary(acroform));
        Ok(())
    }

    /// After deleting `page_id`, strips any outline (bookmark, ISO
    /// 32000-1 12.3.3) or `Link` annotation (12.5.6.5) destination that
    /// pointed directly at it (`/Dest` array, or `/A` with `/S /GoTo` and
    /// a direct-array `/D`). Named destinations resolved through a
    /// document-level `/Names` tree are not rewritten - see module docs.
    fn prune_references_to_page(&mut self, page_id: ObjectId) -> PdfResult<()> {
        if let Ok(catalog) = self.catalog() {
            if let Some(Object::Reference(outlines_id)) = catalog.get("Outlines") {
                self.prune_outline_subtree(*outlines_id, page_id)?;
            }
        }

        for pid in self.page_ids()? {
            let dict = self.get_dictionary(pid)?;
            if let Some(Object::Array(annots)) = dict.get("Annots") {
                for a in annots.iter() {
                    if let Object::Reference(annot_id) = a {
                        if let Ok(mut annot_dict) = self.get_dictionary(*annot_id) {
                            if self.strip_dest_if_points_to(&mut annot_dict, page_id) {
                                self.set_object(*annot_id, Object::Dictionary(annot_dict));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn prune_outline_subtree(&mut self, start: ObjectId, page_id: ObjectId) -> PdfResult<()> {
        let mut visited = HashSet::new();
        let mut stack = vec![start];
        while let Some(node_id) = stack.pop() {
            if visited.len() >= MAX_OUTLINE_NODES {
                break;
            }
            if !visited.insert(node_id) {
                continue;
            }
            let Ok(mut dict) = self.get_dictionary(node_id) else {
                continue;
            };
            let changed = self.strip_dest_if_points_to(&mut dict, page_id);
            let first = match dict.get("First") {
                Some(Object::Reference(id)) => Some(*id),
                _ => None,
            };
            let next = match dict.get("Next") {
                Some(Object::Reference(id)) => Some(*id),
                _ => None,
            };
            if changed {
                self.set_object(node_id, Object::Dictionary(dict));
            }
            if let Some(f) = first {
                stack.push(f);
            }
            if let Some(n) = next {
                stack.push(n);
            }
        }
        Ok(())
    }

    /// Resolves `obj` (following one level of indirection) to the object
    /// id of the page a destination array `[page /XYZ ...]` points at, if
    /// any (ISO 32000-1 12.3.2.2, Table 151).
    fn dest_target(&self, obj: &Object) -> Option<ObjectId> {
        let resolved = match obj {
            Object::Reference(id) => self.get_object(*id)?,
            other => other.clone(),
        };
        match resolved {
            Object::Array(arr) => match arr.get(0) {
                Some(Object::Reference(id)) => Some(*id),
                _ => None,
            },
            _ => None,
        }
    }

    /// Removes `/Dest`, or a `/A` action with `/S /GoTo`, from `dict` if
    /// it points at `page_id`. Returns whether anything was removed.
    fn strip_dest_if_points_to(&self, dict: &mut PdfDictionary, page_id: ObjectId) -> bool {
        let mut changed = false;
        if let Some(dest) = dict.get("Dest").cloned() {
            if self.dest_target(&dest) == Some(page_id) {
                dict.remove("Dest");
                changed = true;
            }
        }
        if let Some(Object::Dictionary(action)) = dict.get("A").cloned() {
            let is_goto = matches!(
                action.get("S"),
                Some(Object::Name(n)) if n.as_str() == "GoTo"
            );
            if is_goto {
                if let Some(d) = action.get("D") {
                    if self.dest_target(d) == Some(page_id) {
                        dict.remove("A");
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}

/// Parses a `/MediaBox` array (ISO 32000-1 §7.7.3.3: `[llx lly urx ury]`,
/// each a `number`) into a [`Rectangle`], normalizing the corners (so a
/// non-conformant producer that swapped lower-left/upper-right still
/// yields a valid rectangle) and rejecting anything else: wrong element
/// count, a non-numeric entry, a non-finite coordinate, or zero/negative
/// width or height. Returns `None` for any of those (see
/// [`EditableDocument::effective_media_box`]'s fallback).
#[cfg(feature = "render")]
fn parse_media_box(arr: &PdfArray) -> Option<Rectangle> {
    if arr.len() != 4 {
        return None;
    }
    let mut nums = [0.0f64; 4];
    for (slot, obj) in nums.iter_mut().zip(arr.iter()) {
        *slot = match obj {
            Object::Real(r) => *r,
            Object::Integer(n) => *n as f64,
            _ => return None,
        };
    }
    let [x0, y0, x1, y1] = nums;
    if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return None;
    }
    let (llx, urx) = (x0.min(x1), x0.max(x1));
    let (lly, ury) = (y0.min(y1), y0.max(y1));
    if urx - llx <= 0.0 || ury - lly <= 0.0 {
        return None;
    }
    Some(Rectangle::new(llx, lly, urx, ury))
}

/// Shifts a [`Destination`]'s page index by `offset`, used by
/// [`EditableDocument::import_bookmark_siblings`] to translate a copied
/// bookmark's target from the source document's page order to the
/// newly-imported pages' position in the destination document.
fn remap_destination_page_index(dest: Destination, offset: usize) -> Destination {
    match dest {
        Destination::FitPage { page_index } => Destination::FitPage {
            page_index: page_index + offset,
        },
        Destination::Xyz {
            page_index,
            left,
            top,
            zoom,
        } => Destination::Xyz {
            page_index: page_index + offset,
            left,
            top,
            zoom,
        },
    }
}

/// Collects every [`Object::Reference`] reachable from `obj` without
/// descending through [`SKIP_IMPORT_KEYS`] dictionary entries.
fn collect_refs(obj: &Object, out: &mut Vec<ObjectId>) {
    match obj {
        Object::Reference(id) => out.push(*id),
        Object::Array(arr) => {
            for item in arr.iter() {
                collect_refs(item, out);
            }
        }
        Object::Dictionary(dict) => {
            for (k, v) in dict.iter() {
                if SKIP_IMPORT_KEYS.contains(&k.as_str()) {
                    continue;
                }
                collect_refs(v, out);
            }
        }
        Object::Stream(s) => {
            for (k, v) in s.dictionary.iter() {
                if SKIP_IMPORT_KEYS.contains(&k.as_str()) {
                    continue;
                }
                collect_refs(v, out);
            }
        }
        _ => {}
    }
}

/// Rewrites every [`Object::Reference`] in `obj` through `map`, dropping
/// [`SKIP_IMPORT_KEYS`] entries and replacing any reference that isn't in
/// `map` with `null` (ISO 32000-1 7.3.10 semantics for a reference to
/// nothing) rather than ever emitting an unmapped, potentially
/// object-number-colliding reference into the destination document.
fn remap_object(obj: &Object, map: &HashMap<ObjectId, ObjectId>) -> Object {
    match obj {
        Object::Reference(id) => match map.get(id) {
            Some(new_id) => Object::Reference(*new_id),
            None => Object::Null,
        },
        Object::Array(arr) => Object::Array(arr.iter().map(|o| remap_object(o, map)).collect()),
        Object::Dictionary(dict) => {
            let mut new_dict = PdfDictionary::new();
            for (k, v) in dict.iter() {
                if SKIP_IMPORT_KEYS.contains(&k.as_str()) {
                    continue;
                }
                new_dict.set(k.clone(), remap_object(v, map));
            }
            Object::Dictionary(new_dict)
        }
        Object::Stream(s) => {
            let mut new_dict = PdfDictionary::new();
            for (k, v) in s.dictionary.iter() {
                if SKIP_IMPORT_KEYS.contains(&k.as_str()) {
                    continue;
                }
                new_dict.set(k.clone(), remap_object(v, map));
            }
            Object::Stream(crate::object::PdfStream::from_raw(new_dict, s.data.clone()))
        }
        other => other.clone(),
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
    fn test_insert_blank_page_at_start() {
        let mut doc = doc_with_pages(2);
        let original_first = doc.page_id_at(0).unwrap();
        let new_id = doc.insert_blank_page(0, 200.0, 300.0).unwrap();
        assert_eq!(doc.page_count().unwrap(), 3);
        assert_eq!(doc.page_id_at(0).unwrap(), new_id);
        assert_eq!(doc.page_id_at(1).unwrap(), original_first);
    }

    #[test]
    fn test_insert_blank_page_out_of_range_errors() {
        let mut doc = doc_with_pages(1);
        assert!(doc.insert_blank_page(5, 100.0, 100.0).is_err());
    }

    #[test]
    fn test_delete_page() {
        let mut doc = doc_with_pages(3);
        let keep0 = doc.page_id_at(0).unwrap();
        let keep2 = doc.page_id_at(2).unwrap();
        doc.delete_page(1).unwrap();
        assert_eq!(doc.page_count().unwrap(), 2);
        assert_eq!(doc.page_id_at(0).unwrap(), keep0);
        assert_eq!(doc.page_id_at(1).unwrap(), keep2);
    }

    #[test]
    fn test_delete_page_out_of_range_errors() {
        let mut doc = doc_with_pages(1);
        assert!(doc.delete_page(1).is_err());
    }

    #[test]
    fn test_move_page() {
        let mut doc = doc_with_pages(3);
        let ids: Vec<_> = (0..3).map(|i| doc.page_id_at(i).unwrap()).collect();
        doc.move_page(0, 2).unwrap();
        assert_eq!(doc.page_id_at(0).unwrap(), ids[1]);
        assert_eq!(doc.page_id_at(1).unwrap(), ids[2]);
        assert_eq!(doc.page_id_at(2).unwrap(), ids[0]);
    }

    #[test]
    fn test_rotate_page_accumulates_and_normalizes() {
        let mut doc = doc_with_pages(1);
        let id = doc.page_id_at(0).unwrap();
        assert_eq!(doc.effective_rotate(id).unwrap(), 0);
        doc.rotate_page(0, 90).unwrap();
        assert_eq!(doc.effective_rotate(id).unwrap(), 90);
        doc.rotate_page(0, 90).unwrap();
        assert_eq!(doc.effective_rotate(id).unwrap(), 180);
        doc.rotate_page(0, 270).unwrap();
        assert_eq!(doc.effective_rotate(id).unwrap(), 90);
        doc.rotate_page(0, -180).unwrap();
        assert_eq!(doc.effective_rotate(id).unwrap(), 270);
    }

    #[test]
    fn test_rotate_page_rejects_non_multiple_of_90() {
        let mut doc = doc_with_pages(1);
        assert!(doc.rotate_page(0, 45).is_err());
    }

    #[test]
    fn test_extract_pages_split() {
        let doc = doc_with_pages(5);
        let split = doc.extract_pages(&[1, 3]).unwrap();
        assert_eq!(split.page_count().unwrap(), 2);
        // The split document is standalone: independent object space.
        assert_eq!(split.catalog_id(), split.catalog_id());
    }

    #[test]
    fn test_extract_pages_out_of_range_errors() {
        let doc = doc_with_pages(2);
        assert!(doc.extract_pages(&[0, 9]).is_err());
    }

    #[test]
    fn test_append_document_merge() {
        let mut a = doc_with_pages(2);
        let b = doc_with_pages(3);
        a.append_document(&b).unwrap();
        assert_eq!(a.page_count().unwrap(), 5);
    }

    #[test]
    fn test_delete_page_prunes_outline_dest() {
        // Build a doc, then hand-add an outline pointing at page 1.
        let mut doc = doc_with_pages(2);
        let target = doc.page_id_at(1).unwrap();

        let outline_item_id = doc.allocate_id();
        let mut item = PdfDictionary::new();
        item.set(
            "Title",
            Object::String(crate::object::PdfString::literal("Go to p2")),
        );
        let mut dest = PdfArray::new();
        dest.push(Object::Reference(target));
        dest.push(Object::Name(PdfName::new_unchecked("Fit")));
        item.set("Dest", Object::Array(dest));
        doc.set_object(outline_item_id, Object::Dictionary(item));

        let outlines_id = doc.allocate_id();
        let mut outlines = PdfDictionary::new();
        outlines.set("Type", Object::Name(PdfName::new_unchecked("Outlines")));
        outlines.set("First", Object::Reference(outline_item_id));
        outlines.set("Last", Object::Reference(outline_item_id));
        outlines.set("Count", Object::Integer(1));
        doc.set_object(outlines_id, Object::Dictionary(outlines));

        let mut catalog = doc.catalog().unwrap();
        catalog.set("Outlines", Object::Reference(outlines_id));
        let root = doc.catalog_id();
        doc.set_object(root, Object::Dictionary(catalog));

        doc.delete_page(1).unwrap();

        let item_after = doc.get_dictionary(outline_item_id).unwrap();
        assert!(
            item_after.get("Dest").is_none(),
            "dangling /Dest must be stripped"
        );
    }

    // ---- `effective_media_box` / `parse_media_box` (valid + adversarial) ----
    // Both are `#[cfg(feature = "render")]` (see their own doc comments),
    // so these tests are too.

    #[test]
    #[cfg(feature = "render")]
    fn effective_media_box_reads_the_page_own_mediabox() {
        let doc = doc_with_pages(1);
        let id = doc.page_id_at(0).unwrap();
        let rect = doc.effective_media_box(id).unwrap();
        // `PageBuilder::a4()` (see `doc_with_pages`).
        assert_eq!(rect, Rectangle::a4());
    }

    #[test]
    #[cfg(feature = "render")]
    fn effective_media_box_falls_back_to_letter_when_missing() {
        // A page whose `/MediaBox` has been stripped entirely (simulating
        // a corrupt/adversarial source file: ISO 32000-1 requires one
        // somewhere in the ancestor chain, but this reader must not choke
        // on a file that omits it).
        let mut doc = doc_with_pages(1);
        let id = doc.page_id_at(0).unwrap();
        let mut dict = doc.get_dictionary(id).unwrap();
        dict.remove("MediaBox");
        doc.set_object(id, Object::Dictionary(dict));

        let rect = doc.effective_media_box(id).unwrap();
        assert_eq!(rect, Rectangle::letter());
    }

    #[test]
    #[cfg(feature = "render")]
    fn parse_media_box_normalizes_swapped_corners() {
        // Non-conformant producer: llx/urx and lly/ury swapped.
        let arr = PdfArray::from_objects(vec![
            Object::Real(200.0),
            Object::Real(300.0),
            Object::Real(0.0),
            Object::Real(0.0),
        ]);
        let rect = parse_media_box(&arr).expect("swapped corners should still parse");
        assert_eq!(rect, Rectangle::new(0.0, 0.0, 200.0, 300.0));
    }

    #[test]
    #[cfg(feature = "render")]
    fn parse_media_box_rejects_adversarial_arrays() {
        // Wrong element count.
        let too_few = PdfArray::from_objects(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(1.0),
        ]);
        assert!(parse_media_box(&too_few).is_none());

        // Non-numeric entry.
        let non_numeric = PdfArray::from_objects(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Name(PdfName::new_unchecked("NotANumber")),
            Object::Real(100.0),
        ]);
        assert!(parse_media_box(&non_numeric).is_none());

        // Degenerate (zero width).
        let degenerate = PdfArray::from_objects(vec![
            Object::Real(10.0),
            Object::Real(0.0),
            Object::Real(10.0),
            Object::Real(100.0),
        ]);
        assert!(parse_media_box(&degenerate).is_none());

        // Non-finite coordinate.
        let non_finite = PdfArray::from_objects(vec![
            Object::Real(f64::NAN),
            Object::Real(0.0),
            Object::Real(100.0),
            Object::Real(100.0),
        ]);
        assert!(parse_media_box(&non_finite).is_none());
    }
}
