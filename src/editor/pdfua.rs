//! PDF/UA (ISO 14289-1:2014) checklist validation, in the spirit of the
//! PDF Association's *Matterhorn Protocol* (a checklist of roughly 136
//! machine- and human-checkable failure conditions organized under
//! PDF/UA's clauses, maintained by the PDF Association).
//!
//! # Scope and honesty about coverage
//!
//! This module implements a **small, explicitly-scoped subset** of
//! Matterhorn-style checks, not the full protocol. Two things are worth
//! being explicit about:
//!
//! - The real Matterhorn Protocol assigns each checkpoint a stable id
//!   like `13-004` (organized by clause). This module's author does not
//!   have the primary Matterhorn Protocol document memorized precisely
//!   enough to cite specific checkpoint ids with confidence (per this
//!   project's "don't invent details you're not sure of" rule), so
//!   violations below are labeled with descriptive, stable-within-this-
//!   crate rule names instead of official checkpoint ids. Treat this as
//!   "Matterhorn-*inspired*", not "Matterhorn-*numbered*".
//! - Real reading-order verification (Matterhorn clause 09, "Logical
//!   Reading Order") requires confirming a page's marked-content
//!   sequences appear in the structure tree in the same order they're
//!   painted, which needs deep per-operator MCID extraction and ordering
//!   analysis. What's implemented here ([`check_reading_order_heuristic`])
//!   is a much weaker proxy - "does every page with non-blank content
//!   have *some* `/StructParents` entry (i.e. at least one tagged content
//!   span)" - documented as a heuristic, not a real order check, at its
//!   definition.
//!
//! What *is* implemented:
//! 1. The document is tagged (`/MarkInfo /Marked true`, ISO 32000-1
//!    14.7.2) and has a non-empty `/StructTreeRoot`.
//! 2. The catalog declares a natural language (`/Lang`, ISO 32000-1
//!    14.9.2) - PDF/UA requires a document-level language be set so
//!    assistive technology knows how to pronounce/hyphenate text whose
//!    own structure elements don't override it.
//! 3. `/ViewerPreferences /DisplayDocTitle` is `true` (ISO 32000-1 Table
//!    150) - PDF/UA requires the viewer chrome show the document's
//!    actual title, not its filename.
//! 4. Every `/Figure` structure element (walked via
//!    [`crate::editor::structure`]'s existing `struct_tree()`) has a
//!    non-empty `/Alt` (ISO 32000-1 14.7.2 - required content for
//!    PDF/UA's "meaningful alternate description" rule).
//! 5. The reading-order heuristic described above.
//! 6. The XMP `/Metadata` declares `pdfuaid:part` `1` (ISO 14289-1:2014
//!    clause 5) - added after discovering, via a real veraPDF `ua1` run
//!    against this crate's own tagged sample while building this module,
//!    that this is checked independently of PDF/A's own `pdfaid` schema.
//!
//! **Not implemented** (needed for real Matterhorn/PDF/UA-1 parity):
//! true reading-order verification, table header scope (`/Scope`,
//! `/Headers`/`/ID`) checks, link/annotation accessible-name checks,
//! colour-contrast/"don't convey information by colour alone" checks
//! (not even machine-checkable from structure alone), form field
//! `/TU` (tooltip) presence, and dozens more. **Effort estimate for
//! genuine Matterhorn-equivalent (136-checkpoint) coverage: several
//! weeks** - each checkpoint needs its own content-stream/structure
//! analysis and the harder ones (reading order, colour-alone) need
//! algorithms well beyond a dictionary/tree walk.

use super::graph::EditableDocument;
use super::structure::StructNode;
use crate::error::PdfResult;
use crate::object::{Object, PdfDictionary};

/// One checklist item that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfUaViolation {
    /// Short, stable rule label (see the [module docs](self) for why
    /// these are not official Matterhorn checkpoint ids).
    pub rule: &'static str,
    /// Human-readable detail.
    pub message: String,
}

impl PdfUaViolation {
    fn new(rule: &'static str, message: impl Into<String>) -> Self {
        Self { rule, message: message.into() }
    }
}

/// Result of [`EditableDocument::validate_pdfua`].
#[derive(Debug, Clone)]
pub struct PdfUaReport {
    /// Every violation found (empty if conformant, within this module's
    /// [documented scope](self)).
    pub violations: Vec<PdfUaViolation>,
}

impl PdfUaReport {
    /// `true` if no violations were found.
    pub fn is_conformant(&self) -> bool {
        self.violations.is_empty()
    }
}

impl EditableDocument {
    /// Sets the catalog's `/Lang` (ISO 32000-1 14.9.2), the document's
    /// default natural language (e.g. `"en-US"`, `"id-ID"`).
    pub fn set_document_language(&mut self, lang: &str) -> PdfResult<()> {
        let mut catalog = self.catalog()?;
        catalog.set("Lang", Object::String(super::util::to_pdf_text_string(lang)));
        let cat_id = self.catalog_id();
        self.set_object(cat_id, Object::Dictionary(catalog));
        Ok(())
    }

    /// Sets `/ViewerPreferences /DisplayDocTitle` (ISO 32000-1 Table 150).
    pub fn set_display_doc_title(&mut self, enabled: bool) -> PdfResult<()> {
        let mut catalog = self.catalog()?;
        let mut vp = match catalog.get("ViewerPreferences") {
            Some(Object::Dictionary(d)) => d.clone(),
            _ => PdfDictionary::new(),
        };
        vp.set("DisplayDocTitle", Object::Boolean(enabled));
        catalog.set("ViewerPreferences", Object::Dictionary(vp));
        let cat_id = self.catalog_id();
        self.set_object(cat_id, Object::Dictionary(catalog));
        Ok(())
    }

    /// Convenience: ensures a (possibly empty) `/StructTreeRoot` exists
    /// (via [`EditableDocument::ensure_struct_tree_root`], which also
    /// sets `/MarkInfo /Marked true`), then applies
    /// [`EditableDocument::set_document_language`] and
    /// [`EditableDocument::set_display_doc_title`]. Does **not** tag any
    /// content itself - callers must still build the structure tree with
    /// [`EditableDocument::add_tagged_content`] (see
    /// [`crate::editor::structure`]) so [`EditableDocument::validate_pdfua`]'s
    /// figure-alt-text and reading-order checks have something to find.
    pub fn prepare_for_pdfua(&mut self, lang: &str) -> PdfResult<()> {
        self.ensure_struct_tree_root()?;
        self.set_document_language(lang)?;
        self.set_display_doc_title(true)?;
        self.set_pdfua_xmp_identification()?;
        Ok(())
    }

    /// Sets `pdfuaid:part` (`1`, for PDF/UA-1/ISO 14289-1:2014) in the
    /// document's XMP `/Metadata` (ISO 14289-1:2014 clause 5 requires
    /// this - discovered by running this crate's own tagged sample
    /// through the real veraPDF `ua1` flavour while building this
    /// module, not from a spec read-through first).
    ///
    /// [`crate::editor::xmp`]'s packet writer always emits one complete
    /// packet rather than editing an existing one in place, so this
    /// reads back whatever `dc:title`/`pdfaid` are already set (if any -
    /// e.g. by [`crate::editor::pdfa::EditableDocument::convert_to_pdfa`],
    /// whichever order the two run in) and re-includes them, rather than
    /// silently dropping them.
    fn set_pdfua_xmp_identification(&mut self) -> PdfResult<()> {
        let existing = self.xmp_metadata()?;
        // ISO 14289-1:2014 7.1 requires the metadata stream itself
        // contain `dc:title` - falling back to the classic `/Info
        // /Title` (rather than leaving `dc:title` unset when no XMP
        // packet exists yet) is what makes `prepare_for_pdfua` alone
        // enough to satisfy that, for the common case of a document
        // that already has an `/Info /Title` but no XMP at all.
        let title = existing.as_deref().and_then(super::xmp::read_dc_title).or_else(|| self.classic_info_title());
        // `read_pdfaid` returns an owned `String` for the conformance
        // letter, but `XmpFields::pdfa` wants `&'static str`. Every value
        // this crate itself ever *writes* there is `"B"`
        // (`crate::editor::pdfa::PdfAFlavor` only implements the "b"
        // levels), so re-widening to the `'static` `"B"` is lossless for
        // anything this module could have produced; a packet from
        // elsewhere claiming `"a"`/`"u"` would have its conformance
        // letter silently normalized to `"B"` here, which is an accepted
        // simplification of this hand-rolled (non-merging) XMP writer,
        // not a claim that the document is actually "b"-conformant.
        let pdfa = existing.as_deref().and_then(super::xmp::read_pdfaid).map(|(part, _conformance)| (part, "B"));
        let xmp = super::xmp::build_xmp_packet(&super::xmp::XmpFields { title: title.as_deref(), pdfa, pdfua_part: Some(1), ..Default::default() });
        self.set_xmp_metadata(xmp)?;
        Ok(())
    }

    /// Checks this document against the [checklist](self) above.
    pub fn validate_pdfua(&self) -> PdfResult<PdfUaReport> {
        let mut violations = Vec::new();

        let catalog = self.catalog()?;
        let marked = matches!(catalog.get("MarkInfo"), Some(Object::Dictionary(mi)) if matches!(mi.get("Marked"), Some(Object::Boolean(true))));
        if !marked {
            violations.push(PdfUaViolation::new("tagged-document flag", "/MarkInfo /Marked is not true"));
        }

        let tree = self.struct_tree()?;
        match &tree {
            None => violations.push(PdfUaViolation::new("structure tree required", "document has no /StructTreeRoot")),
            Some(root) if root.children.is_empty() => {
                violations.push(PdfUaViolation::new("structure tree required", "/StructTreeRoot has no children (nothing tagged)"))
            }
            Some(_) => {}
        }

        if !matches!(catalog.get("Lang"), Some(Object::String(s)) if !s.as_bytes().is_empty()) {
            violations.push(PdfUaViolation::new("document language required", "catalog /Lang is missing or empty"));
        }

        let display_title = matches!(catalog.get("ViewerPreferences"), Some(Object::Dictionary(vp)) if matches!(vp.get("DisplayDocTitle"), Some(Object::Boolean(true))));
        if !display_title {
            violations.push(PdfUaViolation::new("DisplayDocTitle required", "/ViewerPreferences /DisplayDocTitle is not true"));
        }

        if let Some(root) = &tree {
            check_figure_alt_text(root, &mut violations);
        }

        self.check_reading_order_heuristic(&mut violations)?;

        let pdfua_xmp_ok = self.xmp_metadata()?.as_deref().and_then(super::xmp::read_pdfuaid) == Some(1);
        if !pdfua_xmp_ok {
            violations.push(PdfUaViolation::new(
                "ISO 14289-1:2014 clause 5 (PDF/UA Identification Schema)",
                "XMP /Metadata is missing or has no pdfuaid:part=1",
            ));
        }

        Ok(PdfUaReport { violations })
    }

    /// **Heuristic, not a real reading-order check** - see the [module
    /// docs](self). Flags any page whose decoded content stream contains
    /// non-whitespace bytes but whose page dictionary has no
    /// `/StructParents` entry (i.e. no marked-content sequence on that
    /// page was ever associated with a structure element).
    fn check_reading_order_heuristic(&self, violations: &mut Vec<PdfUaViolation>) -> PdfResult<()> {
        for (index, page_id) in self.page_ids()?.into_iter().enumerate() {
            let content = self.page_content_bytes(page_id)?;
            let has_visible_content = content.iter().any(|b| !b.is_ascii_whitespace());
            if !has_visible_content {
                continue;
            }
            let page = self.get_dictionary(page_id)?;
            if !page.contains_key("StructParents") {
                violations.push(PdfUaViolation::new(
                    "reading-order heuristic (approximate - see module docs)",
                    format!("page {index} has non-blank content but no /StructParents entry (nothing on it appears to be tagged)"),
                ));
            }
        }
        Ok(())
    }
}

fn check_figure_alt_text(node: &StructNode, violations: &mut Vec<PdfUaViolation>) {
    if node.struct_type == "Figure" {
        let missing = match &node.alt_text {
            None => true,
            Some(s) => s.trim().is_empty(),
        };
        if missing {
            violations.push(PdfUaViolation::new(
                "figure alternate text required",
                format!("/Figure structure element {} {} R has no (or empty) /Alt", node.id.number, node.id.generation),
            ));
        }
    }
    for child in &node.children {
        check_figure_alt_text(child, violations);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::ContentBuilder;
    use crate::editor::StructType;
    use crate::prelude::*;

    fn doc_with_one_page() -> EditableDocument {
        let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(ContentBuilder::new()).build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        EditableDocument::from_bytes(bytes).unwrap()
    }

    #[test]
    fn test_untouched_document_fails_every_check() {
        let doc = doc_with_one_page();
        let report = doc.validate_pdfua().unwrap();
        assert!(!report.is_conformant());
        assert!(report.violations.iter().any(|v| v.rule.contains("tagged-document")));
        assert!(report.violations.iter().any(|v| v.rule.contains("language")));
        assert!(report.violations.iter().any(|v| v.rule.contains("DisplayDocTitle")));
    }

    #[test]
    fn test_prepare_for_pdfua_fixes_the_document_level_checks() {
        let mut doc = doc_with_one_page();
        doc.prepare_for_pdfua("en-US").unwrap();
        let report = doc.validate_pdfua().unwrap();
        assert!(!report.violations.iter().any(|v| v.rule.contains("tagged-document")));
        assert!(!report.violations.iter().any(|v| v.rule.contains("language")));
        assert!(!report.violations.iter().any(|v| v.rule.contains("DisplayDocTitle")));
        // Still fails: no content was ever tagged.
        assert!(report.violations.iter().any(|v| v.rule.contains("structure tree")));
    }

    #[test]
    fn test_figure_without_alt_text_is_flagged() {
        let mut doc = doc_with_one_page();
        doc.prepare_for_pdfua("en-US").unwrap();
        let root = doc.add_document_structure_root().unwrap();
        doc.add_tagged_content(0, Some(root), StructType::Figure, &ContentBuilder::new(), None).unwrap();

        let report = doc.validate_pdfua().unwrap();
        assert!(report.violations.iter().any(|v| v.rule.contains("figure alternate text")));
    }

    #[test]
    fn test_figure_with_alt_text_is_not_flagged() {
        let mut doc = doc_with_one_page();
        doc.prepare_for_pdfua("en-US").unwrap();
        let root = doc.add_document_structure_root().unwrap();
        doc.add_tagged_content(0, Some(root), StructType::Figure, &ContentBuilder::new(), Some("A descriptive caption")).unwrap();

        let report = doc.validate_pdfua().unwrap();
        assert!(!report.violations.iter().any(|v| v.rule.contains("figure alternate text")), "violations: {:#?}", report.violations);
    }

    #[test]
    fn test_reading_order_heuristic_flags_untagged_visible_content() {
        let mut doc = doc_with_one_page();
        doc.prepare_for_pdfua("en-US").unwrap();
        let page_id = doc.page_id_at(0).unwrap();
        // Draw content directly (not via add_tagged_content), so the page
        // has visible content but no /StructParents.
        let content = ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "untagged text");
        doc.replace_page_content(page_id, &content).unwrap();

        let report = doc.validate_pdfua().unwrap();
        assert!(report.violations.iter().any(|v| v.rule.contains("reading-order")));
    }

    #[test]
    fn test_fully_tagged_document_passes_all_implemented_checks() {
        let mut doc = doc_with_one_page();
        doc.prepare_for_pdfua("en-US").unwrap();
        let root = doc.add_document_structure_root().unwrap();
        let heading = ContentBuilder::new().text("F1", 18.0, 72.0, 780.0, "Title");
        doc.add_tagged_content(0, Some(root), StructType::Heading(1), &heading, None).unwrap();

        let report = doc.validate_pdfua().unwrap();
        assert!(report.is_conformant(), "violations: {:#?}", report.violations);
    }

    #[test]
    fn test_prepare_for_pdfua_sets_pdfuaid_part_one() {
        let mut doc = doc_with_one_page();
        doc.prepare_for_pdfua("en-US").unwrap();
        let xmp = doc.xmp_metadata().unwrap().unwrap();
        assert_eq!(crate::editor::xmp::read_pdfuaid(&xmp), Some(1));
    }

    /// Regression test for a gap discovered by running this crate's own
    /// tagged sample through the real veraPDF `ua1` flavour while
    /// building this module (ISO 14289-1:2014 7.1 requires the metadata
    /// stream itself contain `dc:title`, distinct from the classic
    /// `/Info /Title`): `prepare_for_pdfua` must seed `dc:title` from an
    /// existing `/Info /Title` when it builds the document's first XMP
    /// packet, not leave it unset.
    #[test]
    fn test_prepare_for_pdfua_seeds_dc_title_from_classic_info_title() {
        let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(ContentBuilder::new()).build();
        let bytes = DocumentBuilder::new().title("Quarterly Report").page(page).build().unwrap().save_to_bytes().unwrap();
        let mut doc = EditableDocument::from_bytes(bytes).unwrap();

        doc.prepare_for_pdfua("en-US").unwrap();

        let xmp = doc.xmp_metadata().unwrap().unwrap();
        assert_eq!(crate::editor::xmp::read_dc_title(&xmp).as_deref(), Some("Quarterly Report"));
    }
}
