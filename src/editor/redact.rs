//! Permanent (as opposed to merely visual) content redaction.
//!
//! ISO 32000-2:2020 12.5.6.19 defines a `/Redact` *annotation* - a marker
//! recording an intent to redact, applied later by a "redaction
//! application process" that actually removes the marked content. This
//! module implements that second half directly (there is no intermediate
//! `/Redact` annotation type here): given a page and a rectangular area
//! (in the page's default user space, ISO 32000-1:2008 8.3.2.2 - the same
//! convention every other `EditableDocument` geometry API in this crate
//! uses, e.g. [`super::annotations`]), it:
//!
//! 1. Removes every content-stream text-showing operator (`Tj`, `'`,
//!    `"`, `TJ`, ISO 32000-1 9.4.3) whose rendered bounding box
//!    intersects the area, replacing it with a position-only `Tm`
//!    operator so later, unrelated text on the same page keeps
//!    rendering at (approximately - see "Known limitations" below) the
//!    same place;
//! 2. Removes every image (an `Do`-invoked Image XObject, ISO 32000-1
//!    8.9.5, or an inline image, 8.9.7) whose painted unit square
//!    intersects the area, and additionally overwrites the underlying
//!    XObject's pixel data (and any `/SMask`/`/Mask`) with a 1x1 opaque
//!    placeholder *if* no other, unredacted `Do` anywhere in the
//!    document still uses it - so a shared logo/header image used on
//!    other, non-redacted pages is not destroyed as a side effect;
//! 3. Prunes now-orphaned `/ToUnicode` CMap entries (ISO 32000-1 9.10.3)
//!    for character codes no longer emitted anywhere in the document by
//!    the font(s) touched by (1) - codes still used by *other*, kept
//!    text are left alone.
//!
//! [`EditableDocument::redact_text`] does the byte-level equivalent for a
//! literal string match instead of a geometric area (deleting the
//! matched bytes from `Tj`/`TJ`/`'`/`"` string operands, reusing
//! [`super::content_stream::replace_text_in_items`] with an empty
//! replacement).
//!
//! [`EditableDocument::strip_document_metadata`] is a separate, explicit,
//! whole-document action (not run implicitly by the two above) that
//! clears the classic `/Info` dictionary and drops the catalog's
//! `/Metadata` (XMP) stream reference - "sanitize document" in Acrobat's
//! terms, invoked once before final distribution rather than once per
//! redacted area.
//!
//! Every one of the above records a [`super::RedactionAuditEntry`] (who,
//! when, which page/area, and *counts* of what was removed - never the
//! removed content itself) via [`EditableDocument::audit_log`]; see
//! [`super::audit`] for the persistence format.
//!
//! # Why this requires a full rewrite to actually be permanent
//!
//! [`EditableDocument::save_incremental`] (ISO 32000-1 7.5.6) only
//! *appends* bytes; the pre-redaction object bodies this module just
//! replaced in the in-memory overlay would still be sitting, completely
//! intact, earlier in the file - recoverable by anyone who looks at the
//! file's revision history instead of just the latest one, and exactly
//! the "hidden revision" problem this feature exists to close. To
//! prevent that mistake, every `redact_*`/`strip_document_metadata` call
//! sets [`EditableDocument`]'s internal `redaction_applied` flag, and
//! `save_incremental`/`save_incremental_to_bytes` refuse to run while it
//! is set (see `src/editor/save.rs`). Use
//! [`EditableDocument::save_full_rewrite`] /
//! `save_full_rewrite_to_bytes`, which only ever serializes objects still
//! reachable from the (already-edited) `/Root` - so anything an earlier
//! incremental update left orphaned (including a naive prior "redaction"
//! that only drew a black box over old text without deleting it) is
//! dropped too, not just this session's own edits.
//!
//! # Known limitations (disclosed, not silently wrong)
//!
//! - **Approximate text-run geometry.** The bounding box used to decide
//!   whether a text-showing operator falls inside the redaction area is
//!   computed from the font's `/Widths` (simple fonts) or `/W`/`/DW`
//!   (Type 0 fonts via `/DescendantFonts`, ISO 32000-1 9.7.4.3) advance
//!   widths when present, and a fixed, deliberately generous fallback
//!   width/ascent/descent otherwise (see [`DEFAULT_GLYPH_WIDTH_1000`]).
//!   This is not full glyph-accurate text shaping (no kerning pairs
//!   beyond what `/Widths`/`/W` already encode, no ligature awareness).
//!   The fallback is intentionally on the generous side: an
//!   over-estimated box can only cause *more* text to be conservatively
//!   redacted, never less, which is the safe direction for a redaction
//!   tool to be wrong in.
//! - **Whole-run granularity, not per-glyph.** If a text-showing
//!   operator's box only *partially* overlaps the redaction area, the
//!   *entire* operator (every glyph in that `Tj`/`TJ`/`'`/`"` call) is
//!   removed rather than splitting it into a kept part and a removed
//!   part. Same reasoning as above: over-redaction is the safe failure
//!   mode.
//! - **Images are fully removed, never sub-region cropped**, even when
//!   only part of the image's painted area overlaps the redaction
//!   rectangle. True pixel-level cropping would require decoding the
//!   XObject's raster samples (which, depending on `/Filter` and
//!   `/ColorSpace`, may be DCT/JPEG, arbitrary bit depth, Indexed, or -
//!   for `CCITTFaxDecode`/`JBIG2Decode`, common in scanned enterprise
//!   documents - filters this crate does not decode at all, see
//!   `ARCHITECTURE.md` §10) and re-encoding the cropped result, which is
//!   a materially larger feature (estimated 1-2 person-weeks just for
//!   the subset of encodings the `image`/`jpeg-decoder` crates already
//!   used elsewhere in this crate can round-trip, i.e.
//!   DeviceGray/RGB/CMYK at 8 bits/component; full parity with the
//!   filter set real-world scans use would be substantially more).
//!   Full removal is the conservative, always-safe substitute.
//! - **Nested Form XObjects (ISO 32000-1 8.10) are not descended into.**
//!   Only the page's own content stream and directly-`Do`-invoked Image
//!   XObjects/inline images are inspected; text or images drawn from
//!   inside a Form XObject's own content stream are not detected and
//!   therefore not redacted. Most page content this crate itself
//!   produces, and the majority of simple real-world content streams,
//!   draws directly rather than through a Form XObject wrapper, but a
//!   document that composites redaction-relevant content through one
//!   would not be fully redacted by this module alone.
//! - **Removing a run collapses `Tlm` to the post-removal `Tm`** instead
//!   of preserving both independently (which the `Tm` operator - the
//!   only way to reposition without also drawing something - cannot do
//!   in one step). This only affects further `Td`/`TD`/`T*`-relative
//!   positioning that specifically depends on a *mid-line* redacted
//!   run's original (untouched) line-start matrix, an uncommon
//!   authoring pattern; it never affects whether the redacted bytes
//!   themselves are removed, and normal same-line or next-line text
//!   positioned via later `Tm`/absolute placement is unaffected.
//! - **Reimplementing what a mature redaction pipeline (Acrobat's own,
//!   or a from-scratch content-stream-interpreter-based tool) does -
//!   full glyph-accurate layout reconstruction with a real font-shaping
//!   engine and true pixel-level image cropping across every ISO
//!   32000-1 filter/color space - is out of scope for this task and is
//!   realistically a multi-week (shaping) to multi-month (full filter
//!   parity, matching pdfium/mupdf's raster decode coverage) effort on
//!   its own; see `ARCHITECTURE.md` §10 for the crate-wide version of
//!   this same gap.**

use super::audit::{self, RedactionAuditEntry};
use super::content_stream::{parse_content_stream, replace_text_in_items, serialize_content_stream, ContentItem};
use super::graph::EditableDocument;
use crate::error::PdfResult;
use crate::font::tounicode::{build_tounicode_cmap, parse_tounicode_cmap};
use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream};
use crate::types::{Matrix, ObjectId, Rectangle};
use std::collections::{BTreeMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

/// Fallback glyph advance width (1000-unit glyph space, ISO 32000-1
/// 9.2.4) used when a simple font has no `/Widths` entry for a code (or
/// no `/Widths` array at all - common for a bare `/BaseFont` reference to
/// one of the 14 standard fonts with no embedded metrics) and for the
/// generic "no resolvable font resource at all" fallback. Deliberately
/// on the generous side of the standard-14 average-width range (~480-600
/// depending on font; see [`crate::font::Standard14Font::average_width`])
/// so an unmeasurable run is more likely to be (over-)redacted than
/// missed - see the "Known limitations" section of the [module docs](self).
const DEFAULT_GLYPH_WIDTH_1000: f64 = 600.0;

/// Fallback vertical extent (1000-unit glyph space) above/below the
/// baseline used for every text run's bounding box, regardless of the
/// font's actual `/FontDescriptor` `/Ascent`/`/Descent`. Vertical extent
/// only affects how generously a run's box is drawn, never *which*
/// glyphs get measured/redacted, so a fixed, generous constant is a safe
/// simplification (see "Known limitations").
const DEFAULT_ASCENT_1000: f64 = 750.0;
const DEFAULT_DESCENT_1000: f64 = -250.0;

/// Hard cap on a single `/W` (Type 0 CID width array, ISO 32000-1
/// 9.7.4.3) range-form entry's span (`cFirst cLast w`), so a hostile
/// `/W` declaring e.g. `[0 4000000000 500]` cannot force allocation of
/// billions of map entries.
const MAX_CID_WIDTH_RANGE: i64 = 70_000;

/// Hard cap on the total number of CID->width entries accumulated from a
/// single font's `/W` array, independent of any one range's span.
const MAX_CID_WIDTH_ENTRIES: usize = 500_000;

impl EditableDocument {
    /// Returns the redaction audit trail recorded so far this session
    /// (including entries loaded from the base file, if it was already
    /// redacted by a previous session - see [`super::audit`]).
    pub fn audit_log(&self) -> &[RedactionAuditEntry] {
        &self.audit_log
    }

    /// Permanently removes every text run and image intersecting `rect`
    /// (page default user space, ISO 32000-1 8.3.2.2) on page
    /// `page_index`, prunes now-unused `/ToUnicode` entries for the
    /// font(s) involved, and records an audit entry. See the
    /// [module docs](self) for exactly what "intersecting" and "removes"
    /// mean, and their disclosed limitations.
    pub fn apply_redaction(
        &mut self,
        page_index: usize,
        rect: Rectangle,
        actor: &str,
        reason: &str,
    ) -> PdfResult<RedactionAuditEntry> {
        let rect = normalize_rect(rect);
        let page_id = self.page_id_at(page_index)?;
        let counts = self.redact_area_on_page(page_id, rect)?;

        let mut tounicode_entries_pruned = 0usize;
        for font_id in &counts.touched_font_ids {
            tounicode_entries_pruned += self.prune_tounicode_for_font(*font_id)?;
        }

        let entry = RedactionAuditEntry {
            actor: actor.to_string(),
            reason: reason.to_string(),
            timestamp: now_pdf_date(),
            page_index: Some(page_index),
            area: Some(rect),
            text_runs_removed: counts.text_runs_removed,
            images_removed: counts.images_removed,
            tounicode_entries_pruned,
        };
        self.persist_audit_entry(entry.clone())?;
        Ok(entry)
    }

    /// Permanently removes every occurrence of the literal byte sequence
    /// `needle` from `page_index`'s text-showing operators (byte-level,
    /// like [`EditableDocument::replace_page_text`] - see that method's
    /// docs for the single-byte-encoding caveat), prunes now-unused
    /// `/ToUnicode` entries, and records an audit entry. Unlike
    /// [`EditableDocument::apply_redaction`] this has no associated area
    /// (the audit entry's `area` is `None`).
    pub fn redact_text(
        &mut self,
        page_index: usize,
        needle: &str,
        actor: &str,
        reason: &str,
    ) -> PdfResult<RedactionAuditEntry> {
        let page_id = self.page_id_at(page_index)?;
        let bytes = self.page_content_bytes(page_id)?;
        let mut items = parse_content_stream(&bytes);
        let text_runs_removed = replace_text_in_items(&mut items, needle.as_bytes(), b"");
        if text_runs_removed > 0 {
            let new_bytes = serialize_content_stream(&items);
            self.set_page_content_bytes(page_id, new_bytes)?;
        }

        let mut tounicode_entries_pruned = 0usize;
        if text_runs_removed > 0 {
            for font_id in self.font_ids_on_page(page_id)? {
                tounicode_entries_pruned += self.prune_tounicode_for_font(font_id)?;
            }
        }

        let entry = RedactionAuditEntry {
            actor: actor.to_string(),
            reason: reason.to_string(),
            timestamp: now_pdf_date(),
            page_index: Some(page_index),
            area: None,
            text_runs_removed,
            images_removed: 0,
            tounicode_entries_pruned,
        };
        self.persist_audit_entry(entry.clone())?;
        Ok(entry)
    }

    /// Clears the document's classic `/Info` dictionary (ISO 32000-1
    /// 14.3.3 - title/author/subject/keywords/creator/producer/dates) and
    /// drops the catalog's `/Metadata` (XMP, 14.3.2) stream reference, if
    /// present. A whole-document, explicit action (`page_index: None` in
    /// the resulting audit entry) meant to be invoked once as a final
    /// "sanitize before distribution" step, not automatically by every
    /// area/text redaction (which would be a surprising, possibly
    /// unwanted side effect on every single call).
    ///
    /// Like every other mutation in this module, this only actually
    /// removes the old `/Info`/`/Metadata` object *bytes* from the file
    /// when followed by [`EditableDocument::save_full_rewrite`] (dropping
    /// the old `/Metadata` stream from `/Root`'s reachable-object graph
    /// makes a full rewrite skip it entirely); the in-memory overlay
    /// change alone just makes it unreachable from the new catalog.
    pub fn strip_document_metadata(&mut self, actor: &str, reason: &str) -> PdfResult<RedactionAuditEntry> {
        if let Some(info_id) = self.reader.trailer().info {
            self.set_object(info_id, Object::Dictionary(PdfDictionary::new()));
        }
        let mut catalog = self.catalog()?;
        catalog.remove("Metadata");
        let cat_id = self.catalog_id();
        self.set_object(cat_id, Object::Dictionary(catalog));

        let entry = RedactionAuditEntry {
            actor: actor.to_string(),
            reason: reason.to_string(),
            timestamp: now_pdf_date(),
            page_index: None,
            area: None,
            text_runs_removed: 0,
            images_removed: 0,
            tounicode_entries_pruned: 0,
        };
        self.persist_audit_entry(entry.clone())?;
        Ok(entry)
    }

    /// Core geometric redaction pass over one page's content stream. See
    /// the [module docs](self) for the algorithm and its limitations.
    fn redact_area_on_page(&mut self, page_id: ObjectId, rect: Rectangle) -> PdfResult<AreaRedactionCounts> {
        let target = rect_tuple(rect);
        let resources = self.effective_resources(page_id)?;

        let font_dict = match resources.get("Font") {
            Some(Object::Dictionary(d)) => d.clone(),
            Some(Object::Reference(id)) => self.get_dictionary(*id).unwrap_or_default(),
            _ => PdfDictionary::new(),
        };
        let mut font_widths: BTreeMap<String, FontWidths> = BTreeMap::new();
        let mut font_ids: BTreeMap<String, ObjectId> = BTreeMap::new();
        for (name, value) in font_dict.iter() {
            if let Object::Reference(id) = value {
                if let Ok(dict) = self.get_dictionary(*id) {
                    font_widths.insert(name.clone(), FontWidths::from_font_dict(self, &dict));
                    font_ids.insert(name.clone(), *id);
                }
            }
        }

        let xobject_dict = match resources.get("XObject") {
            Some(Object::Dictionary(d)) => d.clone(),
            Some(Object::Reference(id)) => self.get_dictionary(*id).unwrap_or_default(),
            _ => PdfDictionary::new(),
        };
        let mut image_ids: BTreeMap<String, ObjectId> = BTreeMap::new();
        for (name, value) in xobject_dict.iter() {
            if let Object::Reference(id) = value {
                if self.is_image_xobject(*id) {
                    image_ids.insert(name.clone(), *id);
                }
            }
        }

        let bytes = self.page_content_bytes(page_id)?;
        let items = parse_content_stream(&bytes);

        let mut ctm_stack: Vec<Matrix> = Vec::new();
        let mut ctm = Matrix::identity();
        let mut tm = Matrix::identity();
        let mut tlm = Matrix::identity();
        let mut cur_font: Option<String> = None;
        let mut font_size = 0.0f64;
        let mut char_spacing = 0.0f64;
        let mut word_spacing = 0.0f64;
        let mut h_scale = 1.0f64;
        let mut leading = 0.0f64;
        let mut rise = 0.0f64;

        let mut text_runs_removed = 0usize;
        let mut images_removed = 0usize;
        let mut touched_font_ids: HashSet<ObjectId> = HashSet::new();
        let mut candidate_blank_images: HashSet<ObjectId> = HashSet::new();
        let mut new_items: Vec<ContentItem> = Vec::with_capacity(items.len());

        for item in items {
            match item {
                ContentItem::Op { operator, operands } => match operator.as_str() {
                    "q" => {
                        ctm_stack.push(ctm);
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "Q" => {
                        if let Some(m) = ctm_stack.pop() {
                            ctm = m;
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "cm" => {
                        if let Some(m) = matrix_from_operands(&operands) {
                            ctm = m.multiply(&ctm);
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "BT" => {
                        tm = Matrix::identity();
                        tlm = Matrix::identity();
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "Tf" => {
                        if let (Some(Object::Name(n)), Some(sz)) =
                            (operands.first(), operands.get(1).and_then(|o| o.as_real()))
                        {
                            cur_font = Some(n.as_str().to_string());
                            font_size = sz;
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "Tc" => {
                        if let Some(v) = operands.first().and_then(|o| o.as_real()) {
                            char_spacing = v;
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "Tw" => {
                        if let Some(v) = operands.first().and_then(|o| o.as_real()) {
                            word_spacing = v;
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "Tz" => {
                        if let Some(v) = operands.first().and_then(|o| o.as_real()) {
                            h_scale = v / 100.0;
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "TL" => {
                        if let Some(v) = operands.first().and_then(|o| o.as_real()) {
                            leading = v;
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "Ts" => {
                        if let Some(v) = operands.first().and_then(|o| o.as_real()) {
                            rise = v;
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "Td" => {
                        if let (Some(tx), Some(ty)) =
                            (operands.first().and_then(|o| o.as_real()), operands.get(1).and_then(|o| o.as_real()))
                        {
                            tlm = Matrix::translate(tx, ty).multiply(&tlm);
                            tm = tlm;
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "TD" => {
                        if let (Some(tx), Some(ty)) =
                            (operands.first().and_then(|o| o.as_real()), operands.get(1).and_then(|o| o.as_real()))
                        {
                            leading = -ty;
                            tlm = Matrix::translate(tx, ty).multiply(&tlm);
                            tm = tlm;
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "T*" => {
                        tlm = Matrix::translate(0.0, -leading).multiply(&tlm);
                        tm = tlm;
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "Tm" => {
                        if let Some(m) = matrix_from_operands(&operands) {
                            tm = m;
                            tlm = m;
                        }
                        new_items.push(ContentItem::Op { operator, operands });
                    }
                    "Tj" => {
                        let fw = cur_font.as_deref().and_then(|n| font_widths.get(n));
                        let pieces = match operands.last() {
                            Some(Object::String(s)) => vec![OwnedPiece::Str(s.as_bytes().to_vec())],
                            _ => Vec::new(),
                        };
                        let (new_tm, hit) = evaluate_show_text(
                            &pieces, fw, font_size, char_spacing, word_spacing, h_scale, rise, tm, ctm, target,
                        );
                        tm = new_tm;
                        if hit && !pieces.is_empty() {
                            text_runs_removed += 1;
                            if let Some(fid) = cur_font.as_deref().and_then(|n| font_ids.get(n)) {
                                touched_font_ids.insert(*fid);
                            }
                            new_items.push(matrix_to_tm_op(tm));
                        } else {
                            new_items.push(ContentItem::Op { operator, operands });
                        }
                    }
                    "'" | "\"" => {
                        if operator == "\"" {
                            if let (Some(aw), Some(ac)) = (
                                operands.first().and_then(|o| o.as_real()),
                                operands.get(1).and_then(|o| o.as_real()),
                            ) {
                                word_spacing = aw;
                                char_spacing = ac;
                            }
                        }
                        tlm = Matrix::translate(0.0, -leading).multiply(&tlm);
                        tm = tlm;
                        let fw = cur_font.as_deref().and_then(|n| font_widths.get(n));
                        let pieces = match operands.last() {
                            Some(Object::String(s)) => vec![OwnedPiece::Str(s.as_bytes().to_vec())],
                            _ => Vec::new(),
                        };
                        let (new_tm, hit) = evaluate_show_text(
                            &pieces, fw, font_size, char_spacing, word_spacing, h_scale, rise, tm, ctm, target,
                        );
                        tm = new_tm;
                        if hit && !pieces.is_empty() {
                            text_runs_removed += 1;
                            if let Some(fid) = cur_font.as_deref().and_then(|n| font_ids.get(n)) {
                                touched_font_ids.insert(*fid);
                            }
                            new_items.push(matrix_to_tm_op(tm));
                        } else {
                            new_items.push(ContentItem::Op { operator, operands });
                        }
                    }
                    "TJ" => {
                        let fw = cur_font.as_deref().and_then(|n| font_widths.get(n));
                        let pieces = match operands.first() {
                            Some(Object::Array(arr)) => owned_pieces_from_tj_array(arr),
                            _ => Vec::new(),
                        };
                        let (new_tm, hit) = evaluate_show_text(
                            &pieces, fw, font_size, char_spacing, word_spacing, h_scale, rise, tm, ctm, target,
                        );
                        tm = new_tm;
                        if hit && !pieces.is_empty() {
                            text_runs_removed += 1;
                            if let Some(fid) = cur_font.as_deref().and_then(|n| font_ids.get(n)) {
                                touched_font_ids.insert(*fid);
                            }
                            new_items.push(matrix_to_tm_op(tm));
                        } else {
                            new_items.push(ContentItem::Op { operator, operands });
                        }
                    }
                    "Do" => {
                        let hit = match operands.first() {
                            Some(Object::Name(n)) => match image_ids.get(n.as_str()) {
                                Some(&xid) => {
                                    let bbox = unit_square_bbox(ctm);
                                    if rects_intersect(target, bbox) {
                                        images_removed += 1;
                                        candidate_blank_images.insert(xid);
                                        true
                                    } else {
                                        false
                                    }
                                }
                                None => false,
                            },
                            _ => false,
                        };
                        if !hit {
                            new_items.push(ContentItem::Op { operator, operands });
                        }
                    }
                    _ => new_items.push(ContentItem::Op { operator, operands }),
                },
                ContentItem::InlineImage(img) => {
                    let bbox = unit_square_bbox(ctm);
                    if rects_intersect(target, bbox) {
                        images_removed += 1;
                        // No separate indirect object to blank: the pixel
                        // data lives inline in the content stream and is
                        // simply not carried over to `new_items` below.
                    } else {
                        new_items.push(ContentItem::InlineImage(img));
                    }
                }
                ContentItem::Raw(bytes) => new_items.push(ContentItem::Raw(bytes)),
            }
        }

        let new_bytes = serialize_content_stream(&new_items);
        self.set_page_content_bytes(page_id, new_bytes)?;

        if !candidate_blank_images.is_empty() {
            let still_used = self.collect_used_image_ids()?;
            for id in candidate_blank_images {
                if !still_used.contains(&id) {
                    self.blank_image_object(id, 0)?;
                }
            }
        }

        Ok(AreaRedactionCounts {
            text_runs_removed,
            images_removed,
            touched_font_ids,
        })
    }

    /// Returns whether `id` resolves to an Image XObject (ISO 32000-1
    /// 8.9.5, `/Subtype /Image`).
    fn is_image_xobject(&self, id: ObjectId) -> bool {
        matches!(
            self.get_object(id),
            Some(Object::Stream(s)) if matches!(s.dictionary.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Image")
        )
    }

    /// Overwrites the image XObject `id` (and, recursively up to a small
    /// depth bound, any `/SMask`/`/Mask` it references) with a 1x1 opaque
    /// `DeviceGray` placeholder, so the original pixel bytes - whatever
    /// filter they were encoded with - no longer exist anywhere in the
    /// object graph a subsequent [`EditableDocument::save_full_rewrite`]
    /// would serialize.
    fn blank_image_object(&mut self, id: ObjectId, depth: u32) -> PdfResult<()> {
        // Depth bound: `/SMask`/`/Mask` should never nest further per ISO
        // 32000-1 8.9.5.4/8.9.6.2, but this is untrusted file-derived
        // structure, so guard against a crafted cycle regardless.
        if depth > 4 {
            return Ok(());
        }
        let Some(Object::Stream(s)) = self.get_object(id) else {
            return Ok(());
        };
        for key in ["SMask", "Mask"] {
            if let Some(Object::Reference(mask_id)) = s.dictionary.get(key) {
                let mask_id = *mask_id;
                self.blank_image_object(mask_id, depth + 1)?;
            }
        }
        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("XObject")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Image")));
        dict.set("Width", Object::Integer(1));
        dict.set("Height", Object::Integer(1));
        dict.set("BitsPerComponent", Object::Integer(8));
        dict.set("ColorSpace", Object::Name(PdfName::new_unchecked("DeviceGray")));
        let blanked = PdfStream::with_dictionary(dict, vec![0u8]);
        self.set_object(id, Object::Stream(blanked));
        Ok(())
    }

    /// Scans every page's content stream for `Do` invocations, returning
    /// the set of Image XObject ids still actually painted somewhere in
    /// the document. Used to decide whether it's safe to destroy a
    /// candidate image's pixel data (shared resources must not be
    /// blanked just because one of possibly several pages that used them
    /// was redacted).
    fn collect_used_image_ids(&self) -> PdfResult<HashSet<ObjectId>> {
        let mut used = HashSet::new();
        for page_id in self.page_ids()? {
            let resources = self.effective_resources(page_id)?;
            let xobjects = match resources.get("XObject") {
                Some(Object::Dictionary(d)) => d.clone(),
                Some(Object::Reference(id)) => self.get_dictionary(*id).unwrap_or_default(),
                _ => continue,
            };
            let bytes = self.page_content_bytes(page_id)?;
            for item in parse_content_stream(&bytes) {
                if let ContentItem::Op { operator, operands } = item {
                    if operator == "Do" {
                        if let Some(Object::Name(n)) = operands.first() {
                            if let Some(Object::Reference(id)) = xobjects.get(n.as_str()) {
                                used.insert(*id);
                            }
                        }
                    }
                }
            }
        }
        Ok(used)
    }

    /// Returns the object ids of every font referenced from `page_id`'s
    /// effective `/Resources /Font` dictionary.
    fn font_ids_on_page(&self, page_id: ObjectId) -> PdfResult<Vec<ObjectId>> {
        let resources = self.effective_resources(page_id)?;
        let font_dict = match resources.get("Font") {
            Some(Object::Dictionary(d)) => d.clone(),
            Some(Object::Reference(id)) => self.get_dictionary(*id).unwrap_or_default(),
            _ => return Ok(Vec::new()),
        };
        Ok(font_dict
            .iter()
            .filter_map(|(_, v)| if let Object::Reference(id) = v { Some(*id) } else { None })
            .collect())
    }

    /// Rewrites `font_id`'s `/ToUnicode` CMap (ISO 32000-1 9.10.3),
    /// dropping any entry for a character code no longer emitted by that
    /// font anywhere in the document. Returns the number of entries
    /// dropped. A no-op (returns `0`, does not touch the object) if the
    /// font has no `/ToUnicode`, or every existing entry is still used.
    fn prune_tounicode_for_font(&mut self, font_id: ObjectId) -> PdfResult<usize> {
        let Ok(dict) = self.get_dictionary(font_id) else {
            return Ok(0);
        };
        let Some(Object::Reference(tounicode_id)) = dict.get("ToUnicode").cloned() else {
            return Ok(0);
        };
        let Some(Object::Stream(stream)) = self.get_object(tounicode_id) else {
            return Ok(0);
        };
        let Ok(decoded) = stream.decode_all() else {
            return Ok(0);
        };
        let map = parse_tounicode_cmap(&decoded);
        if map.is_empty() {
            return Ok(0);
        }

        let is_composite = matches!(dict.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Type0");
        let code_bytes: u8 = if is_composite { 2 } else { 1 };
        let used = self.collect_used_codes_for_font(font_id, code_bytes)?;

        let kept: Vec<(u32, String)> = map.iter().filter(|(c, _)| used.contains(c)).map(|(c, s)| (*c, s.clone())).collect();
        let removed = map.len() - kept.len();
        if removed > 0 {
            let cmap_bytes = build_tounicode_cmap(&kept, code_bytes);
            let new_stream = PdfStream::new(cmap_bytes).with_compression()?;
            self.set_object(tounicode_id, Object::Stream(new_stream));
        }
        Ok(removed)
    }

    /// Scans every page for text shown through a `/Resources /Font` entry
    /// that resolves to `target_font_id`, returning the set of character
    /// codes (`code_bytes` wide, matching [`super::text_extract`]'s
    /// `Identity-H`/`-V` assumption for composite fonts) still in use.
    fn collect_used_codes_for_font(&self, target_font_id: ObjectId, code_bytes: u8) -> PdfResult<HashSet<u32>> {
        let code_bytes = code_bytes.clamp(1, 2) as usize;
        let mut used = HashSet::new();
        for page_id in self.page_ids()? {
            let resources = self.effective_resources(page_id)?;
            let font_dict = match resources.get("Font") {
                Some(Object::Dictionary(d)) => d.clone(),
                Some(Object::Reference(id)) => self.get_dictionary(*id).unwrap_or_default(),
                _ => continue,
            };
            let names: Vec<String> = font_dict
                .iter()
                .filter(|(_, v)| matches!(v, Object::Reference(id) if *id == target_font_id))
                .map(|(k, _)| k.clone())
                .collect();
            if names.is_empty() {
                continue;
            }

            let bytes = self.page_content_bytes(page_id)?;
            let mut active = false;
            for item in parse_content_stream(&bytes) {
                let ContentItem::Op { operator, operands } = item else { continue };
                match operator.as_str() {
                    "Tf" => {
                        active = matches!(operands.first(), Some(Object::Name(n)) if names.iter().any(|nm| nm == n.as_str()));
                    }
                    "Tj" | "'" if active => {
                        if let Some(Object::String(s)) = operands.last() {
                            collect_codes(s.as_bytes(), code_bytes, &mut used);
                        }
                    }
                    "\"" if active => {
                        if let Some(Object::String(s)) = operands.last() {
                            collect_codes(s.as_bytes(), code_bytes, &mut used);
                        }
                    }
                    "TJ" if active => {
                        if let Some(Object::Array(arr)) = operands.first() {
                            for e in arr.iter() {
                                if let Object::String(s) = e {
                                    collect_codes(s.as_bytes(), code_bytes, &mut used);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(used)
    }

    /// Appends `entry` to the in-memory audit log, persists the whole log
    /// as a stream referenced from the catalog (see [`super::audit`]),
    /// and marks this session as having redacted content (see the
    /// [module docs](self) for why that then blocks incremental save).
    fn persist_audit_entry(&mut self, entry: RedactionAuditEntry) -> PdfResult<()> {
        self.audit_log.push(entry);
        let stream = audit::build_log_stream(&self.audit_log).with_compression()?;
        let id = match self.audit_log_object_id {
            Some(id) => id,
            None => {
                let id = self.allocate_id();
                self.audit_log_object_id = Some(id);
                let mut catalog = self.catalog()?;
                catalog.set(audit::AUDIT_LOG_CATALOG_KEY, Object::Reference(id));
                let cat_id = self.catalog_id();
                self.set_object(cat_id, Object::Dictionary(catalog));
                id
            }
        };
        self.set_object(id, Object::Stream(stream));
        self.redaction_applied = true;
        Ok(())
    }
}

/// Result of one page's geometric redaction pass.
struct AreaRedactionCounts {
    text_runs_removed: usize,
    images_removed: usize,
    touched_font_ids: HashSet<ObjectId>,
}

/// Per-font glyph-advance lookup used to measure a text run's width. See
/// the [module docs](self) ("Known limitations") for why this is an
/// approximation, not full text shaping.
struct FontWidths {
    /// `1` for simple fonts, `2` for the `Identity-H`/`-V` composite-font
    /// assumption this crate uses elsewhere (see
    /// [`super::text_extract`]).
    code_width_bytes: usize,
    /// Advance width (1000-unit glyph space) for a code not present in
    /// `widths_1000` (`/W`'s `/DW`, ISO 32000-1 9.7.4.3, for composite
    /// fonts; [`DEFAULT_GLYPH_WIDTH_1000`] for simple fonts, since simple
    /// fonts have no per-font default-width entry in the spec).
    default_width_1000: f64,
    widths_1000: BTreeMap<u32, f64>,
}

impl FontWidths {
    fn from_font_dict(doc: &EditableDocument, dict: &PdfDictionary) -> Self {
        let is_composite = matches!(dict.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Type0");
        if is_composite {
            let mut widths_1000 = BTreeMap::new();
            let mut default_width_1000 = 1000.0; // ISO 32000-1 9.7.4.3: DW defaults to 1000.
            if let Some(descendant) = resolve_first_descendant(doc, dict) {
                if let Some(dw) = descendant.get("DW").and_then(|o| o.as_real()) {
                    default_width_1000 = dw;
                }
                let w_obj = match descendant.get("W") {
                    Some(Object::Reference(id)) => doc.get_object(*id),
                    other => other.cloned(),
                };
                if let Some(Object::Array(w)) = w_obj {
                    widths_1000 = parse_cid_widths(&w);
                }
            }
            FontWidths { code_width_bytes: 2, default_width_1000, widths_1000 }
        } else {
            let first_char = dict.get("FirstChar").and_then(|o| o.as_integer()).unwrap_or(0).max(0) as u32;
            let mut widths_1000 = BTreeMap::new();
            let widths_obj = match dict.get("Widths") {
                Some(Object::Reference(id)) => doc.get_object(*id),
                other => other.cloned(),
            };
            if let Some(Object::Array(arr)) = widths_obj {
                for (i, w) in arr.iter().enumerate() {
                    if let Some(w) = w.as_real() {
                        widths_1000.insert(first_char.saturating_add(i as u32), w);
                    }
                }
            }
            FontWidths {
                code_width_bytes: 1,
                default_width_1000: DEFAULT_GLYPH_WIDTH_1000,
                widths_1000,
            }
        }
    }

    /// Text-space horizontal displacement (ISO 32000-1 9.4.3) for showing
    /// `bytes` under the current text state.
    fn measure_advance(&self, bytes: &[u8], font_size: f64, char_spacing: f64, word_spacing: f64, h_scale: f64) -> f64 {
        let cw = self.code_width_bytes.max(1);
        let mut total = 0.0;
        let mut i = 0;
        while i + cw <= bytes.len() {
            let mut code: u32 = 0;
            for &b in &bytes[i..i + cw] {
                code = (code << 8) | u32::from(b);
            }
            let w0 = self.widths_1000.get(&code).copied().unwrap_or(self.default_width_1000) / 1000.0;
            let is_space = cw == 1 && code == 32;
            total += (w0 * font_size + char_spacing + if is_space { word_spacing } else { 0.0 }) * h_scale;
            i += cw;
        }
        total
    }
}

fn resolve_first_descendant(doc: &EditableDocument, dict: &PdfDictionary) -> Option<PdfDictionary> {
    let arr = match dict.get("DescendantFonts") {
        Some(Object::Array(arr)) => Some(arr.clone()),
        Some(Object::Reference(id)) => match doc.get_object(*id) {
            Some(Object::Array(arr)) => Some(arr),
            _ => None,
        },
        _ => None,
    }?;
    match arr.get(0)? {
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        Object::Dictionary(d) => Some(d.clone()),
        _ => None,
    }
}

/// Parses a `/W` array (ISO 32000-1 9.7.4.3, Table 117): a sequence of
/// either `c [w1 w2 ... wn]` (individual widths starting at CID `c`) or
/// `cFirst cLast w` (one width for the whole inclusive range) entries.
/// Bounded against a hostile/corrupt array (untrusted-input rule): see
/// [`MAX_CID_WIDTH_RANGE`]/[`MAX_CID_WIDTH_ENTRIES`].
fn parse_cid_widths(arr: &PdfArray) -> BTreeMap<u32, f64> {
    let mut out = BTreeMap::new();
    let items: Vec<&Object> = arr.iter().collect();
    let mut i = 0usize;
    while i < items.len() && out.len() < MAX_CID_WIDTH_ENTRIES {
        let Some(c_first) = items[i].as_integer() else { break };
        i += 1;
        if i >= items.len() {
            break;
        }
        match items[i] {
            Object::Array(list) => {
                for (k, w) in list.iter().enumerate() {
                    if out.len() >= MAX_CID_WIDTH_ENTRIES {
                        break;
                    }
                    if let Some(w) = w.as_real() {
                        out.insert((c_first.max(0) as u32).saturating_add(k as u32), w);
                    }
                }
                i += 1;
            }
            other => {
                let Some(c_last) = other.as_integer() else { break };
                i += 1;
                let Some(w) = items.get(i).and_then(|o| o.as_real()) else { break };
                i += 1;
                let c_last_bounded = c_last.max(c_first).min(c_first.saturating_add(MAX_CID_WIDTH_RANGE));
                let mut cid = c_first;
                while cid <= c_last_bounded && out.len() < MAX_CID_WIDTH_ENTRIES {
                    out.insert(cid.max(0) as u32, w);
                    cid += 1;
                }
            }
        }
    }
    out
}

/// One element of a `TJ` array (ISO 32000-1 9.4.3): either a string to
/// show, or a numeric position adjustment.
enum OwnedPiece {
    Str(Vec<u8>),
    Adj(f64),
}

fn owned_pieces_from_tj_array(arr: &PdfArray) -> Vec<OwnedPiece> {
    arr.iter()
        .filter_map(|e| match e {
            Object::String(s) => Some(OwnedPiece::Str(s.as_bytes().to_vec())),
            Object::Integer(n) => Some(OwnedPiece::Adj(*n as f64)),
            Object::Real(n) => Some(OwnedPiece::Adj(*n)),
            _ => None,
        })
        .collect()
}

/// Text-space advance for showing `bytes` when no font resource could be
/// resolved: assumes 1-byte codes and [`DEFAULT_GLYPH_WIDTH_1000`] per
/// code (matching [`super::text_extract`]'s WinAnsi-fallback convention
/// of treating an unresolvable font as single-byte).
fn measure_bytes_no_font(bytes: &[u8], font_size: f64, char_spacing: f64, word_spacing: f64, h_scale: f64) -> f64 {
    let w0 = DEFAULT_GLYPH_WIDTH_1000 / 1000.0;
    bytes
        .iter()
        .map(|&b| {
            let is_space = b == 32;
            (w0 * font_size + char_spacing + if is_space { word_spacing } else { 0.0 }) * h_scale
        })
        .sum()
}

/// Computes the total text-space displacement of showing `pieces`
/// (ISO 32000-1 9.4.3) and the resulting device-space bounding box
/// (transforming through `tm` then `ctm`), and tests it against `target`.
/// Returns `(new_tm, hit)`; `new_tm` is always the position-after-showing
/// (ISO 32000-1 9.4.4), regardless of `hit`, so callers can unconditionally
/// use it to keep tracking position.
#[allow(clippy::too_many_arguments)]
fn evaluate_show_text(
    pieces: &[OwnedPiece],
    font: Option<&FontWidths>,
    font_size: f64,
    char_spacing: f64,
    word_spacing: f64,
    h_scale: f64,
    rise: f64,
    tm: Matrix,
    ctm: Matrix,
    target: (f64, f64, f64, f64),
) -> (Matrix, bool) {
    let mut total_advance = 0.0;
    for p in pieces {
        total_advance += match p {
            OwnedPiece::Str(bytes) => match font {
                Some(fw) => fw.measure_advance(bytes, font_size, char_spacing, word_spacing, h_scale),
                None => measure_bytes_no_font(bytes, font_size, char_spacing, word_spacing, h_scale),
            },
            OwnedPiece::Adj(n) => -(n / 1000.0) * font_size * h_scale,
        };
    }
    let y0 = rise + DEFAULT_DESCENT_1000 / 1000.0 * font_size;
    let y1 = rise + DEFAULT_ASCENT_1000 / 1000.0 * font_size;
    let (x0, x1) = (total_advance.min(0.0), total_advance.max(0.0));
    let m = tm.multiply(&ctm);
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| m.transform_point(x, y));
    let bbox = aabb(&corners);
    let hit = rects_intersect(target, bbox);
    let new_tm = Matrix::translate(total_advance, 0.0).multiply(&tm);
    (new_tm, hit)
}

/// Device-space bounding box of the unit square `[0,1]x[0,1]` transformed
/// by `ctm` - the area an Image XObject or inline image is painted into
/// (ISO 32000-1 8.9.5.2, 8.9.7).
fn unit_square_bbox(ctm: Matrix) -> (f64, f64, f64, f64) {
    let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(x, y)| ctm.transform_point(x, y));
    aabb(&corners)
}

fn aabb(pts: &[(f64, f64); 4]) -> (f64, f64, f64, f64) {
    let minx = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let maxx = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let miny = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let maxy = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    (minx, miny, maxx, maxy)
}

/// Standard axis-aligned bounding box overlap test; `a`/`b` are
/// `(minx, miny, maxx, maxy)`.
fn rects_intersect(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

fn matrix_from_operands(operands: &[Object]) -> Option<Matrix> {
    if operands.len() < 6 {
        return None;
    }
    let n = operands.len();
    let v: Vec<f64> = (n - 6..n).map(|i| operands[i].as_real()).collect::<Option<Vec<_>>>()?;
    Some(Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5]))
}

fn matrix_to_tm_op(m: Matrix) -> ContentItem {
    ContentItem::Op {
        operator: "Tm".to_string(),
        operands: vec![
            Object::Real(m.a),
            Object::Real(m.b),
            Object::Real(m.c),
            Object::Real(m.d),
            Object::Real(m.e),
            Object::Real(m.f),
        ],
    }
}

fn collect_codes(bytes: &[u8], code_width: usize, out: &mut HashSet<u32>) {
    let cw = code_width.max(1);
    let mut i = 0;
    while i + cw <= bytes.len() {
        let mut code: u32 = 0;
        for &b in &bytes[i..i + cw] {
            code = (code << 8) | u32::from(b);
        }
        out.insert(code);
        i += cw;
    }
}

/// Normalizes a caller-supplied rectangle so `llx <= urx` and
/// `lly <= ury` regardless of which corners were passed in (ISO 32000-1
/// 7.9.5 permits either ordering for a rectangle array; the same
/// convention [`super::util::rect_from_object`] applies when reading one
/// out of a PDF).
fn normalize_rect(r: Rectangle) -> Rectangle {
    Rectangle::new(r.llx.min(r.urx), r.lly.min(r.ury), r.llx.max(r.urx), r.lly.max(r.ury))
}

fn rect_tuple(r: Rectangle) -> (f64, f64, f64, f64) {
    (r.llx, r.lly, r.urx, r.ury)
}

fn now_pdf_date() -> String {
    format_pdf_date_utc(SystemTime::now())
}

/// Formats a [`SystemTime`] as a PDF date string (ISO 32000-1 7.9.4,
/// `D:YYYYMMDDHHmmSSZ`, UTC). Implemented with a small, self-contained
/// civil-calendar conversion (Howard Hinnant's `civil_from_days`
/// algorithm, public domain) rather than a `chrono` dependency, since
/// this is the only place in the crate that needs wall-clock-to-calendar
/// conversion.
fn format_pdf_date_utc(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, mo, d) = civil_from_days(days);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("D:{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use std::time::Duration;

    // ---- civil date formatting -------------------------------------

    #[test]
    fn formats_unix_epoch() {
        assert_eq!(format_pdf_date_utc(UNIX_EPOCH), "D:19700101000000Z");
    }

    #[test]
    fn formats_y2k() {
        // 946684800 is the well-known Unix timestamp for 2000-01-01T00:00:00Z.
        let t = UNIX_EPOCH + Duration::from_secs(946_684_800);
        assert_eq!(format_pdf_date_utc(t), "D:20000101000000Z");
    }

    #[test]
    fn formats_with_time_of_day() {
        // 2000-01-01T00:00:00Z + 1h2m3s.
        let t = UNIX_EPOCH + Duration::from_secs(946_684_800 + 3723);
        assert_eq!(format_pdf_date_utc(t), "D:20000101010203Z");
    }

    // ---- geometry helpers -------------------------------------------

    #[test]
    fn aabb_of_axis_aligned_square() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert_eq!(aabb(&pts), (0.0, 0.0, 10.0, 10.0));
    }

    #[test]
    fn rects_intersect_overlapping_and_disjoint() {
        assert!(rects_intersect((0.0, 0.0, 10.0, 10.0), (5.0, 5.0, 15.0, 15.0)));
        assert!(!rects_intersect((0.0, 0.0, 10.0, 10.0), (10.0, 10.0, 20.0, 20.0))); // touching edge, not overlapping
        assert!(!rects_intersect((0.0, 0.0, 10.0, 10.0), (20.0, 20.0, 30.0, 30.0)));
    }

    #[test]
    fn unit_square_bbox_under_translate_and_scale() {
        // cm = [w 0 0 h x y]: image drawn at (x,y) with size (w,h), the
        // convention `ContentBuilder::draw_image` uses.
        let ctm = Matrix::new(100.0, 0.0, 0.0, 50.0, 10.0, 20.0);
        assert_eq!(unit_square_bbox(ctm), (10.0, 20.0, 110.0, 70.0));
    }

    // ---- end-to-end: area redaction removes text -----------------

    fn page_with_text_at(text: &str, x: f64, y: f64, size: f64) -> (EditableDocument, ObjectId) {
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", size, x, y, text))
            .build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        let doc = EditableDocument::from_bytes(bytes).unwrap();
        let id = doc.page_id_at(0).unwrap();
        (doc, id)
    }

    #[test]
    fn apply_redaction_removes_intersecting_text_run() {
        let (mut doc, _id) = page_with_text_at("CONFIDENTIAL-SSN-123-45-6789", 72.0, 700.0, 24.0);
        // Generously covers the estimated run box (see FontWidths' fallback).
        let rect = Rectangle::new(50.0, 680.0, 600.0, 730.0);
        let entry = doc.apply_redaction(0, rect, "alice@example.com", "PII removal").unwrap();
        assert_eq!(entry.text_runs_removed, 1);
        assert_eq!(entry.page_index, Some(0));
        assert_eq!(entry.area, Some(rect));

        let bytes = doc.page_content_bytes(doc.page_id_at(0).unwrap()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("123-45-6789"));
    }

    #[test]
    fn apply_redaction_leaves_non_intersecting_text_alone() {
        let (mut doc, id) = page_with_text_at("Not sensitive", 72.0, 700.0, 24.0);
        let rect = Rectangle::new(400.0, 10.0, 500.0, 50.0); // far away
        let entry = doc.apply_redaction(0, rect, "alice", "test").unwrap();
        assert_eq!(entry.text_runs_removed, 0);
        let bytes = doc.page_content_bytes(id).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("Not sensitive"));
    }

    #[test]
    fn redaction_forces_full_rewrite_and_survives_reload_with_audit_log() {
        let (mut doc, _id) = page_with_text_at("SECRET-VALUE-42", 72.0, 700.0, 24.0);
        let rect = Rectangle::new(50.0, 680.0, 400.0, 730.0);
        doc.apply_redaction(0, rect, "bob@example.com", "contract redaction").unwrap();

        // Incremental save must now be refused (would leave the
        // pre-redaction content stream recoverable in the file's earlier
        // bytes).
        assert!(doc.save_incremental_to_bytes().is_err());

        let saved = doc.save_full_rewrite_to_bytes().unwrap();
        // Forensic check at the semantic (decoded content-stream) level -
        // this crate's editor always Flate-compresses page content it
        // rewrites, so a *raw*-byte grep for plaintext is not a
        // meaningful signal either way once compression is involved;
        // decoding first (exactly what any real forensic tool, e.g.
        // `qpdf --decompress`, would do) is the correct check.
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let page_bytes = reopened.page_content_bytes(reopened.page_id_at(0).unwrap()).unwrap();
        assert!(!String::from_utf8_lossy(&page_bytes).contains("SECRET-VALUE-42"));

        // The audit log survives the save/reload round trip.
        let log = reopened.audit_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].actor, "bob@example.com");
        assert_eq!(log[0].reason, "contract redaction");
        assert_eq!(log[0].page_index, Some(0));
        assert_eq!(log[0].area, Some(rect));
        assert_eq!(log[0].text_runs_removed, 1);
    }

    #[test]
    fn redact_text_literal_removes_matched_bytes_only() {
        let (mut doc, id) = page_with_text_at("Public Secret123 Public", 72.0, 700.0, 24.0);
        let entry = doc.redact_text(0, "Secret123", "carol", "keyword sweep").unwrap();
        assert_eq!(entry.text_runs_removed, 1);
        assert_eq!(entry.area, None);
        let bytes = doc.page_content_bytes(id).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("Secret123"));
        assert!(text.contains("Public"));
    }

    #[test]
    fn strip_document_metadata_clears_info_and_records_whole_document_entry() {
        let doc_bytes = DocumentBuilder::new()
            .info(DocumentInfo::new().title("Sensitive Title").author("Someone"))
            .page(PageBuilder::a4().build())
            .build()
            .unwrap()
            .save_to_bytes()
            .unwrap();
        let mut doc = EditableDocument::from_bytes(doc_bytes).unwrap();
        let entry = doc.strip_document_metadata("admin", "final sanitize pass").unwrap();
        assert_eq!(entry.page_index, None);
        assert_eq!(entry.area, None);

        let saved = doc.save_full_rewrite_to_bytes().unwrap();
        assert!(!String::from_utf8_lossy(&saved).contains("Sensitive Title"));
    }

    // ---- image redaction: raw-byte forensic proof --------------------

    #[cfg(feature = "images")]
    #[test]
    fn apply_redaction_blanks_intersecting_image_pixel_bytes() {
        use crate::image::{ColorSpace, Image, ImageFilter};

        // A distinctive, uncompressed pixel pattern that must be
        // literally, byte-for-byte present in the *original* saved file
        // and byte-for-byte absent from the *redacted* file - a genuine
        // raw-byte grep, not a decoded-stream comparison, since this
        // image is embedded without any compression filter.
        let marker: Vec<u8> = [0xABu8, 0xCDu8, 0xEFu8].repeat(16);
        let image = Image::new(4, 4, ColorSpace::DeviceRGB, 8, ImageFilter::FlateDecode, marker.clone());

        let page = PageBuilder::a4().build();
        let doc_bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        let mut doc = EditableDocument::from_bytes(doc_bytes).unwrap();
        let page_id = doc.page_id_at(0).unwrap();
        // Draw without compression so the marker bytes are literal in
        // the pre-redaction saved file (proves the "before" half of the
        // forensic claim, not just the "after" half).
        {
            use crate::image::ImageXObject;
            let image_id = doc.allocate_id();
            let xobject = ImageXObject::from_image(&image);
            doc.set_object(image_id, Object::Stream(xobject.stream));
            let mut page_dict = doc.get_dictionary(page_id).unwrap();
            let mut resources = PdfDictionary::new();
            let mut xobjects = PdfDictionary::new();
            xobjects.set("Im1", Object::Reference(image_id));
            resources.set("XObject", Object::Dictionary(xobjects));
            page_dict.set("Resources", Object::Dictionary(resources));
            doc.set_object(page_id, Object::Dictionary(page_dict));
            let draw = ContentBuilder::new().draw_image("Im1", 100.0, 100.0, 50.0, 50.0);
            doc.append_page_content(page_id, &draw).unwrap();
        }

        let before = doc.save_full_rewrite_to_bytes().unwrap();
        assert!(
            contains_bytes(&before, &marker),
            "test setup invalid: marker must be present before redaction"
        );

        let rect = Rectangle::new(90.0, 90.0, 160.0, 160.0);
        let entry = doc.apply_redaction(0, rect, "dana", "remove scanned signature").unwrap();
        assert_eq!(entry.images_removed, 1);

        let after = doc.save_full_rewrite_to_bytes().unwrap();
        assert!(
            !contains_bytes(&after, &marker),
            "redacted output still contains the original image's raw pixel bytes"
        );
    }

    // ---- hidden prior-revision content is dropped by full rewrite ----

    #[test]
    fn full_rewrite_after_redaction_drops_orphaned_prior_revision_bytes() {
        // Simulate a naive earlier "redaction" that only overwrote a
        // page's content stream via an *incremental* update (leaving the
        // original, still-sensitive object bytes physically present
        // earlier in the file - the exact vulnerability this module's
        // full-rewrite requirement exists to close).
        let (doc, page_id) = page_with_text_at("HIDDEN-REVISION-SECRET-777", 72.0, 700.0, 24.0);
        let original = doc.save_incremental_to_bytes().unwrap(); // no-op save; just get bytes
        assert!(String::from_utf8_lossy(&original).contains("HIDDEN-REVISION-SECRET-777"));

        let mut naive_edit = EditableDocument::from_bytes(original).unwrap();
        naive_edit.replace_page_content(page_id, &ContentBuilder::new().text("F1", 24.0, 72.0, 700.0, "visible now"))
            .unwrap();
        let after_naive_incremental_edit = naive_edit.save_incremental_to_bytes().unwrap();

        // Sanity check: the vulnerability is real - the "hidden" secret
        // is still literally present in the file's raw bytes because an
        // incremental update only appends.
        assert!(
            contains_bytes(&after_naive_incremental_edit, b"HIDDEN-REVISION-SECRET-777"),
            "test setup invalid: incremental update should not have removed the earlier revision's bytes"
        );

        // Now redact something on the *current* (already-not-showing-the-
        // secret) page and finalize with a full rewrite.
        let mut final_doc = EditableDocument::from_bytes(after_naive_incremental_edit).unwrap();
        let page_id2 = final_doc.page_id_at(0).unwrap();
        let rect = Rectangle::new(50.0, 680.0, 400.0, 730.0);
        final_doc.apply_redaction(0, rect, "eve", "finalize redaction").unwrap();
        let final_bytes = final_doc.save_full_rewrite_to_bytes().unwrap();

        assert!(
            !contains_bytes(&final_bytes, b"HIDDEN-REVISION-SECRET-777"),
            "full rewrite must drop the orphaned prior-revision object, not just this session's own edits"
        );
        let _ = page_id2;
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    // ---- ToUnicode pruning: shared codes across two runs -------------

    #[test]
    fn tounicode_entries_pruned_only_for_codes_no_longer_used() {
        // Hand-built (not via ContentBuilder, which writes each `char` as
        // raw UTF-8 rather than single-byte codes - see text_extract.rs
        // for the same rationale) so the codes shown exactly match a
        // hand-authored /ToUnicode CMap: two separate Tj calls under the
        // same font resource, at well-separated x positions so a
        // redaction rectangle can target only the first.
        let cmap_bytes = build_tounicode_cmap(
            &[
                (0x41, "A".to_string()),
                (0x42, "B".to_string()),
                (0x43, "C".to_string()),
                (0x44, "D".to_string()),
            ],
            1,
        );

        let mut data = Vec::new();
        data.extend_from_slice(b"%PDF-1.7\n");
        let obj1 = data.len();
        data.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let obj2 = data.len();
        data.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        let obj3 = data.len();
        data.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n",
        );
        let obj4 = data.len();
        let content = b"BT /F1 24 Tf 72 700 Td (AB) Tj ET BT /F1 24 Tf 300 700 Td (CD) Tj ET";
        data.extend_from_slice(format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes());
        data.extend_from_slice(content);
        data.extend_from_slice(b"\nendstream\nendobj\n");
        let obj5 = data.len();
        data.extend_from_slice(b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 6 0 R >>\nendobj\n");
        let obj6 = data.len();
        data.extend_from_slice(format!("6 0 obj\n<< /Length {} >>\nstream\n", cmap_bytes.len()).as_bytes());
        data.extend_from_slice(&cmap_bytes);
        data.extend_from_slice(b"\nendstream\nendobj\n");
        let xref_off = data.len();
        data.extend_from_slice(b"xref\n0 7\n");
        data.extend_from_slice(b"0000000000 65535 f \n");
        for off in [obj1, obj2, obj3, obj4, obj5, obj6] {
            data.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
        }
        data.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R >>\n");
        data.extend_from_slice(format!("startxref\n{xref_off}\n%%EOF\n").as_bytes());

        let mut doc = EditableDocument::from_bytes(data).expect("hand-built PDF must parse");
        let font_id = ObjectId::new(5);

        // Sanity: both runs decode via ToUnicode before redaction.
        let text_before = doc.extract_page_text(doc.page_id_at(0).unwrap()).unwrap();
        assert!(text_before.contains("AB"));
        assert!(text_before.contains("CD"));

        // Redact only the first run ("AB" at x=72).
        let rect = Rectangle::new(60.0, 690.0, 160.0, 725.0);
        let entry = doc.apply_redaction(0, rect, "frank", "prune test").unwrap();
        assert_eq!(entry.text_runs_removed, 1);
        assert_eq!(entry.tounicode_entries_pruned, 2);

        let dict = doc.get_dictionary(font_id).unwrap();
        let Some(Object::Reference(tid)) = dict.get("ToUnicode").cloned() else { panic!("ToUnicode missing") };
        let Some(Object::Stream(s)) = doc.get_object(tid) else { panic!("ToUnicode not a stream") };
        let decoded = s.decode_all().unwrap();
        let map = parse_tounicode_cmap(&decoded);
        assert!(!map.contains_key(&0x41));
        assert!(!map.contains_key(&0x42));
        assert!(map.contains_key(&0x43));
        assert!(map.contains_key(&0x44));

        let text_after = doc.extract_page_text(doc.page_id_at(0).unwrap()).unwrap();
        assert!(!text_after.contains("AB"));
        assert!(text_after.contains("CD"));
    }
}
