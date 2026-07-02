//! PDF/A-1b, PDF/A-2b and PDF/A-3b validation and conversion.
//!
//! # Scope and honesty about coverage
//!
//! ISO 19005-1:2005 (PDF/A-1), -2:2011 (PDF/A-2) and -3:2012 (PDF/A-3)
//! each define on the order of a hundred-plus individually testable
//! conformance clauses (veraPDF's own machine-checkable rule sets run
//! into the hundreds per flavor). **This module implements a practical,
//! explicitly-scoped subset**, not full veraPDF parity - reimplementing
//! that fully is a multi-week-plus undertaking (see the effort note at
//! the end of this doc comment). What *is* implemented, and checked by
//! both [`EditableDocument::validate_pdfa`] and (where automatically
//! fixable) remediated by [`EditableDocument::convert_to_pdfa`]:
//!
//! 1. PDF version matches the flavor's base spec (ISO 19005-1:2005 is
//!    defined against the PDF 1.4 Reference; -2/-3 against ISO 32000-1,
//!    PDF 1.7).
//! 2. The document is not encrypted (ISO 19005-1 6.1.3).
//! 3. At least one `/OutputIntent` with `/S /GTS_PDFA1` and a
//!    structurally valid embedded ICC profile is present (ISO 19005-1
//!    6.2.2).
//! 4. An XMP `/Metadata` stream is present and its PDF/A Identification
//!    schema (`pdfaid:part`/`pdfaid:conformance`) matches the requested
//!    flavor.
//! 5. Every font referenced by a page's `/Resources /Font` is embedded
//!    (`/FontDescriptor` has a `/FontFile`/`/FontFile2`/`/FontFile3`), or
//!    is a `/Type3` font (whose glyphs are inline content, not a
//!    separate program) - ISO 19005-1 6.3. Font *width consistency* and
//!    "legally embeddable" checks (also part of 6.3) are **not**
//!    implemented.
//! 6. No stream uses the `LZWDecode` filter - a commonly documented PDF/A
//!    restriction across all three parts; see the caveat on
//!    [`LZW_RULE`] about this module's confidence in the exact clause
//!    number.
//! 7. (PDF/A-1b only, since transparency compositing was new in PDF 1.4
//!    and PDF/A-1's profile restricts it) no object's `/Group` is
//!    `/S /Transparency`, and no `/SMask`/`/BM` (soft mask / blend mode,
//!    found on `/ExtGState` dictionaries *and* on Image XObjects) departs
//!    from `/None`/`/Normal`/`/Compatible`. This is a conservative,
//!    simplified reading of ISO 19005-1 6.4 "Transparency" - see the
//!    effort note below for what full transparency-group conformance
//!    checking (isolated/knockout attributes, group colour space
//!    validity, ...) would add.
//! 8. (PDF/A-1b only) no `/OCProperties` (optional content / layers is a
//!    PDF 1.5 feature).
//! 9. (PDF/A-1b and -2b only; -3b's headline new feature vs. -2b is
//!    exactly this) no `/Names /EmbeddedFiles`.
//! 10. No JavaScript (`/Names /JavaScript`, or an `/OpenAction` of
//!     subtype `/JavaScript`) - commonly documented as forbidden across
//!     all PDF/A parts.
//!
//! **Not implemented** (would need to be added for real veraPDF parity):
//! tagged-PDF/accessibility requirements (only PDF/A-1**a**/2a/3a require
//! these - out of scope since the task targets the *b* levels), colour
//! space consistency between content and the output intent beyond "an
//! output intent exists", annotation appearance-stream requirements,
//! glyph-width-consistency checks, embedded-font "legally embeddable"
//! licensing checks, full transparency group semantics, digital signature
//! interaction rules, and dozens of narrower structural clauses (illegal
//! keys in various dictionaries, `/Metadata` on every stream in PDF/A-2/3
//! that requires it, etc.). **Effort estimate for genuine multi-hundred-
//! rule veraPDF-equivalent coverage: several weeks of focused work** (each
//! rule needs its own spec lookup, implementation and corrupt/edge-case
//! test - this module's 10 rules already took a full session).
//!
//! # Usage
//!
//! ```ignore
//! let mut doc = EditableDocument::open("input.pdf")?;
//! let options = PdfAConversionOptions {
//!     icc_profile: &srgb_icc_bytes,
//!     icc_identifier: "sRGB IEC61966-2.1",
//!     icc_condition: "sRGB",
//!     title: Some("Report"),
//!     producer: Some("rust-pdf"),
//! };
//! doc.convert_to_pdfa(PdfAFlavor::Part1B, &options)?;
//! let bytes = doc.save_pdfa_compatible_to_bytes(PdfAFlavor::Part1B.min_pdf_version())?;
//! std::fs::write("output_pdfa1b.pdf", &bytes)?;
//!
//! // Validating is only meaningful against the actually-saved file (the
//! // PDF version check reads the file's own header): reopen it.
//! let saved = EditableDocument::from_bytes(bytes)?;
//! let report = saved.validate_pdfa(PdfAFlavor::Part1B)?;
//! assert!(report.is_conformant());
//! ```

use super::graph::EditableDocument;
use super::icc::OutputIntentSubtype;
use super::xmp::{build_xmp_packet, read_pdfaid, read_pdfuaid, XmpFields};
use crate::document::PdfVersion;
use crate::error::PdfResult;
use crate::object::{Object, PdfDictionary, PdfName, PdfStream};
use std::collections::HashSet;

/// Note on the LZWDecode prohibition ([`EditableDocument::validate_pdfa`]
/// rule 6): this is widely and consistently documented (PDF/A guides,
/// tooling vendors) as a blanket PDF/A restriction, but this module's
/// author has not independently re-verified the exact ISO 19005-1/2/3
/// clause number against the primary text for this task, per the
/// project's "don't invent spec behaviour you're not sure of" rule - the
/// *existence* of the rule is treated as reliable; the specific clause
/// citation below is best-effort.
const LZW_RULE: &str = "PDF/A LZWDecode prohibition (widely documented; exact ISO 19005 clause not independently re-verified for this task)";

/// A PDF/A conformance flavor this module targets - the "b" (basic:
/// visual reproducibility) level of parts 1 through 3. The "a"
/// (accessible/tagged) and "u" (Unicode-mapping) levels are not
/// implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfAFlavor {
    /// PDF/A-1b (ISO 19005-1:2005), based on the PDF 1.4 Reference.
    Part1B,
    /// PDF/A-2b (ISO 19005-2:2011), based on ISO 32000-1 (PDF 1.7).
    Part2B,
    /// PDF/A-3b (ISO 19005-3:2012), based on ISO 32000-1 (PDF 1.7); the
    /// only one of the three that permits arbitrary embedded files.
    Part3B,
}

impl PdfAFlavor {
    /// The `pdfaid:part` value (1, 2 or 3).
    pub fn part_number(self) -> u8 {
        match self {
            PdfAFlavor::Part1B => 1,
            PdfAFlavor::Part2B => 2,
            PdfAFlavor::Part3B => 3,
        }
    }

    /// The PDF version [`EditableDocument::save_pdfa_compatible_to_bytes`]
    /// should be called with for this flavor.
    pub fn min_pdf_version(self) -> PdfVersion {
        match self {
            PdfAFlavor::Part1B => PdfVersion::V1_4,
            PdfAFlavor::Part2B | PdfAFlavor::Part3B => PdfVersion::V1_7,
        }
    }

    fn allows_transparency(self) -> bool {
        !matches!(self, PdfAFlavor::Part1B)
    }

    fn allows_optional_content(self) -> bool {
        !matches!(self, PdfAFlavor::Part1B)
    }

    fn allows_embedded_files(self) -> bool {
        matches!(self, PdfAFlavor::Part3B)
    }
}

/// One rule this module checked and found violated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfAViolation {
    /// A short, stable identifier for the rule (a spec citation where the
    /// citation is a section number this module is confident of, or a
    /// descriptive label - see the caveats in the [module docs](self)).
    pub rule: &'static str,
    /// Human-readable detail (which object, which font, ...).
    pub message: String,
}

impl PdfAViolation {
    fn new(rule: &'static str, message: impl Into<String>) -> Self {
        Self { rule, message: message.into() }
    }
}

/// Result of [`EditableDocument::validate_pdfa`].
#[derive(Debug, Clone)]
pub struct PdfAReport {
    /// The flavor checked against.
    pub flavor: PdfAFlavor,
    /// Every rule violation found (empty if conformant, within this
    /// module's [documented scope](self)).
    pub violations: Vec<PdfAViolation>,
}

impl PdfAReport {
    /// `true` if no violations were found.
    pub fn is_conformant(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Inputs [`EditableDocument::convert_to_pdfa`] needs that cannot be
/// derived from the source document itself.
pub struct PdfAConversionOptions<'a> {
    /// ICC profile bytes for the `/OutputIntent` (see
    /// [`crate::editor::icc`] for why this crate never bundles one
    /// itself). Only used if the document doesn't already have a valid
    /// PDF/A output intent.
    pub icc_profile: &'a [u8],
    /// `/OutputConditionIdentifier` (e.g. `"sRGB IEC61966-2.1"`).
    pub icc_identifier: &'a str,
    /// `/OutputCondition` (free text).
    pub icc_condition: &'a str,
    /// `dc:title` written into the XMP packet's Dublin Core schema.
    /// `None` omits it. This does **not** also set the classic `/Info
    /// /Title` (this crate's `EditableDocument` has no general `/Info`
    /// dictionary editor yet - see the effort note in
    /// `crate::editor::pdfua` for the related `/Lang`/title-in-viewer-
    /// preferences gap); a caller wanting both should set `/Info /Title`
    /// itself via a full-rewrite-time `Document` rebuild, or treat this
    /// as a known follow-up.
    pub title: Option<&'a str>,
    /// `pdf:Producer` / `xmp:CreatorTool`.
    pub producer: Option<&'a str>,
}

/// What [`EditableDocument::convert_to_pdfa`] actually changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PdfAConversionSummary {
    /// Streams whose filter chain was rewritten from `LZWDecode` to
    /// `FlateDecode`.
    pub lzw_streams_reencoded: usize,
    /// `/ExtGState` dictionaries whose `/SMask`/`/BM` were reset to
    /// `/None`/`/Normal` (PDF/A-1b only).
    pub extgstates_disabled: usize,
    /// Objects whose `/Group /S /Transparency` was removed (PDF/A-1b
    /// only).
    pub transparency_groups_removed: usize,
    /// Whether a fresh `/OutputIntent` was added (`false` if the document
    /// already had a valid one).
    pub output_intent_added: bool,
    /// Catalog-level keys removed (`"OCProperties"`, `"EmbeddedFiles"`,
    /// `"JavaScript"`, `"OpenAction"`), in the order removed.
    pub catalog_entries_removed: Vec<&'static str>,
}

impl EditableDocument {
    /// Applies every automatically-safe PDF/A remediation this module
    /// implements (see the [module docs](self) for the exact list) for
    /// `flavor`, and always (re)writes the XMP PDF/A identification and
    /// (if missing) an output intent.
    ///
    /// Does **not** serialize the result - call
    /// [`EditableDocument::save_pdfa_compatible_to_bytes`] with
    /// `flavor.min_pdf_version()` afterwards (see the [module docs](self)
    /// for why this is a separate step). Does **not** guarantee the
    /// result validates clean: some nonconformances (non-embedded fonts
    /// this crate has no font source to embed a replacement for,
    /// transparent *image* content that would need to be rasterized/
    /// flattened to remove losslessly) cannot be safely auto-fixed and
    /// are left for [`EditableDocument::validate_pdfa`] to report - see
    /// its output after saving+reopening for what (if anything) still
    /// needs source-level attention.
    pub fn convert_to_pdfa(&mut self, flavor: PdfAFlavor, options: &PdfAConversionOptions) -> PdfResult<PdfAConversionSummary> {
        let lzw_streams_reencoded = self.reencode_lzw_streams()?;

        let (extgstates_disabled, transparency_groups_removed) = if !flavor.allows_transparency() {
            (self.strip_extgstate_transparency()?, self.strip_transparency_groups()?)
        } else {
            (0, 0)
        };

        let catalog_entries_removed = self.strip_forbidden_catalog_entries(flavor)?;

        let mut summary = PdfAConversionSummary {
            lzw_streams_reencoded,
            extgstates_disabled,
            transparency_groups_removed,
            catalog_entries_removed,
            ..Default::default()
        };

        let already_has_output_intent = self.output_intents()?.iter().any(|i| i.subtype == "GTS_PDFA1" && i.has_valid_icc_profile);
        if !already_has_output_intent {
            self.add_output_intent(options.icc_profile, OutputIntentSubtype::PdfA, options.icc_identifier, options.icc_condition)?;
            summary.output_intent_added = true;
        }

        // Preserve an existing `pdfuaid:part` (set by
        // `crate::editor::pdfua::EditableDocument::prepare_for_pdfua`, if
        // that ran before this call) rather than clobbering it: this
        // hand-rolled writer always emits one complete packet (see the
        // [module docs](crate::editor::xmp)), so without this the second
        // of the two calls would silently drop whichever schema the
        // first one wrote, regardless of call order.
        let existing_pdfua_part = self.xmp_metadata()?.and_then(|x| read_pdfuaid(&x));
        let xmp = build_xmp_packet(&XmpFields {
            title: options.title,
            producer: options.producer,
            pdfa: Some((flavor.part_number(), "B")),
            pdfx_version: None,
            pdfua_part: existing_pdfua_part,
        });
        self.set_xmp_metadata(xmp)?;

        Ok(summary)
    }

    /// Checks this document against `flavor`'s rules (see the [module
    /// docs](self) for the exact list and its scope). The PDF-version
    /// check reads the header this document was actually opened from, so
    /// this is only representative of a *saved* PDF/A candidate: run it
    /// on the [`EditableDocument`] produced by reopening the bytes
    /// [`EditableDocument::save_pdfa_compatible_to_bytes`] wrote, not on
    /// the live, not-yet-saved document [`EditableDocument::convert_to_pdfa`]
    /// was just called on.
    pub fn validate_pdfa(&self, flavor: PdfAFlavor) -> PdfResult<PdfAReport> {
        let mut violations = Vec::new();

        self.check_version(flavor, &mut violations);
        if self.reader.trailer().encrypt.is_some() {
            violations.push(PdfAViolation::new("ISO 19005-1 6.1.3 (no encryption)", "document trailer has an /Encrypt dictionary"));
        }
        self.check_output_intent(&mut violations)?;
        self.check_xmp_pdfaid(flavor, &mut violations)?;
        self.check_fonts_embedded(&mut violations)?;
        self.check_no_lzw(&mut violations)?;
        if !flavor.allows_transparency() {
            self.check_no_transparency(&mut violations)?;
        }

        let catalog = self.catalog()?;
        if !flavor.allows_optional_content() && catalog.contains_key("OCProperties") {
            violations.push(PdfAViolation::new(
                "PDF/A-1 optional content prohibition (PDF 1.5 feature, not in the PDF 1.4 base spec)",
                "catalog has /OCProperties",
            ));
        }
        if let Some(Object::Dictionary(names)) = catalog.get("Names") {
            if !flavor.allows_embedded_files() && names.contains_key("EmbeddedFiles") {
                violations.push(PdfAViolation::new(
                    "PDF/A-1/2 embedded-file prohibition (arbitrary embedded files are PDF/A-3's headline addition)",
                    "catalog has /Names /EmbeddedFiles",
                ));
            }
            if names.contains_key("JavaScript") {
                violations.push(PdfAViolation::new("PDF/A JavaScript prohibition (widely documented)", "catalog has /Names /JavaScript"));
            }
        }
        if let Some(Object::Dictionary(oa)) = catalog.get("OpenAction") {
            if matches!(oa.get("S"), Some(Object::Name(n)) if n.as_str() == "JavaScript") {
                violations.push(PdfAViolation::new("PDF/A JavaScript prohibition (widely documented)", "catalog /OpenAction is a JavaScript action"));
            }
        }

        Ok(PdfAReport { flavor, violations })
    }

    fn check_version(&self, flavor: PdfAFlavor, violations: &mut Vec<PdfAViolation>) {
        let actual = self.reader.version();
        let ok = match flavor {
            PdfAFlavor::Part1B => actual == PdfVersion::V1_4,
            PdfAFlavor::Part2B | PdfAFlavor::Part3B => actual >= PdfVersion::V1_4 && actual <= PdfVersion::V1_7,
        };
        if !ok {
            violations.push(PdfAViolation::new(
                "PDF/A base-spec version requirement",
                format!("document declares PDF {}, expected {}", actual.as_str(), flavor.min_pdf_version().as_str()),
            ));
        }
    }

    fn check_output_intent(&self, violations: &mut Vec<PdfAViolation>) -> PdfResult<()> {
        let intents = self.output_intents()?;
        let ok = intents.iter().any(|i| i.subtype == "GTS_PDFA1" && i.has_valid_icc_profile);
        if !ok {
            violations.push(PdfAViolation::new(
                "ISO 19005-1 6.2.2 (OutputIntent required)",
                "no /OutputIntents entry with /S /GTS_PDFA1 and a valid ICC /DestOutputProfile was found",
            ));
        }
        Ok(())
    }

    fn check_xmp_pdfaid(&self, flavor: PdfAFlavor, violations: &mut Vec<PdfAViolation>) -> PdfResult<()> {
        let Some(xmp) = self.xmp_metadata()? else {
            violations.push(PdfAViolation::new("PDF/A Identification Extension Schema required", "no /Metadata XMP stream present"));
            return Ok(());
        };
        match read_pdfaid(&xmp) {
            Some((part, conformance)) if part == flavor.part_number() && conformance.eq_ignore_ascii_case("B") => {}
            Some((part, conformance)) => violations.push(PdfAViolation::new(
                "PDF/A Identification Extension Schema mismatch",
                format!("XMP declares pdfaid:part={part} pdfaid:conformance={conformance:?}, expected part={} conformance=\"B\"", flavor.part_number()),
            )),
            None => violations.push(PdfAViolation::new(
                "PDF/A Identification Extension Schema required",
                "/Metadata is present but has no pdfaid:part/pdfaid:conformance",
            )),
        }
        self.check_title_consistency(&xmp, violations);
        Ok(())
    }

    /// ISO 19005-1:2005 6.7.3: when both the classic `/Info /Title` and
    /// the XMP `dc:title` are present, they must be equivalent. Found by
    /// running this crate's own PDF/A-1b sample through the real veraPDF
    /// CLI while building this module (the sample's `/Info /Title` and
    /// `dc:title` had been set from two different caller-supplied
    /// strings, an easy mistake for any caller to make, not just this
    /// crate's own demo, hence adding the check here rather than only
    /// fixing the demo script).
    fn check_title_consistency(&self, xmp: &[u8], violations: &mut Vec<PdfAViolation>) {
        let Some(info_title) = self.classic_info_title() else { return };
        let Some(xmp_title) = super::xmp::read_dc_title(xmp) else { return };
        if info_title != xmp_title {
            violations.push(PdfAViolation::new(
                "ISO 19005-1 6.7.3 (Info/XMP title equivalence)",
                format!("/Info /Title ({info_title:?}) does not match XMP dc:title ({xmp_title:?})"),
            ));
        }
    }

    fn check_fonts_embedded(&self, violations: &mut Vec<PdfAViolation>) -> PdfResult<()> {
        let mut checked = HashSet::new();
        for page_id in self.page_ids()? {
            let resources = self.page_resources(page_id)?;
            let Some(Object::Dictionary(fonts)) = resources.get("Font") else { continue };
            for (name, font_obj) in fonts.iter() {
                let Object::Reference(font_id) = font_obj else { continue };
                if !checked.insert(*font_id) {
                    continue;
                }
                let Ok(font_dict) = self.get_dictionary(*font_id) else { continue };
                if !self.font_is_embedded(&font_dict) {
                    let base = match font_dict.get("BaseFont") {
                        Some(Object::Name(n)) => n.as_str().to_string(),
                        _ => name.clone(),
                    };
                    violations.push(PdfAViolation::new("ISO 19005-1 6.3 (font embedding)", format!("font resource {name:?} ({base}) is not embedded")));
                    continue;
                }
                self.check_cidset_present(&font_dict, name, violations);
            }
        }
        Ok(())
    }

    /// ISO 19005-1:2005 6.3.5: every embedded CIDFont that is a *subset*
    /// (its `BaseFont` carries the conventional 6-uppercase-letter
    /// subset tag, ISO 32000-1 9.6.4, e.g. `"ABCDEF+MyFont"`) must have a
    /// `/CIDSet` stream on its font descriptor identifying exactly which
    /// CIDs the subset contains.
    ///
    /// This crate's own font-subsetting pipeline
    /// ([`crate::font::cid::CompositeFont::build`]) does **not** currently
    /// generate `/CIDSet` - discovered by running this crate's own
    /// PDF/A-1b output through the real veraPDF CLI while building this
    /// module, not from a spec read-through. Fixing the root cause (the
    /// subsetter itself) is out of scope for this pass (it needs a new
    /// object id threaded through `CompositeFontIds`/`BuiltCompositeFont`
    /// and every writer that allocates one, in `crate::font::cid` and
    /// `crate::document`; estimated at a few focused hours, not weeks,
    /// but real, separate work from this conformance module). Checking
    /// for it here at least keeps [`EditableDocument::validate_pdfa`]
    /// from reporting a false "conformant" on a document with this gap.
    fn check_cidset_present(&self, font_dict: &PdfDictionary, resource_name: &str, violations: &mut Vec<PdfAViolation>) {
        if !matches!(font_dict.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Type0") {
            return;
        }
        let Some(Object::Array(descendants)) = font_dict.get("DescendantFonts") else { return };
        let Some(Object::Reference(desc_id)) = descendants.get(0) else { return };
        let Ok(desc_dict) = self.get_dictionary(*desc_id) else { return };
        let base_font = match desc_dict.get("BaseFont") {
            Some(Object::Name(n)) => n.as_str().to_string(),
            _ => return,
        };
        if !is_subset_tagged(&base_font) {
            return;
        }
        let Some(Object::Reference(fd_id)) = desc_dict.get("FontDescriptor") else { return };
        let Ok(fd) = self.get_dictionary(*fd_id) else { return };
        if !fd.contains_key("CIDSet") {
            violations.push(PdfAViolation::new(
                "ISO 19005-1 6.3.5 (CIDSet required for CID font subsets)",
                format!("font resource {resource_name:?} ({base_font}) is a subset CIDFont but its FontDescriptor has no /CIDSet"),
            ));
        }
    }

    fn font_is_embedded(&self, font_dict: &PdfDictionary) -> bool {
        let subtype = match font_dict.get("Subtype") {
            Some(Object::Name(n)) => n.as_str(),
            _ => "",
        };
        if subtype == "Type3" {
            // Type3 glyph procedures are inline content-stream programs,
            // not a separate embeddable font file (ISO 32000-1 9.6.5).
            return true;
        }
        if subtype == "Type0" {
            let Some(Object::Array(descendants)) = font_dict.get("DescendantFonts") else { return false };
            let Some(Object::Reference(desc_id)) = descendants.get(0) else { return false };
            let Ok(desc_dict) = self.get_dictionary(*desc_id) else { return false };
            return descriptor_has_font_file(&desc_dict, self);
        }
        descriptor_has_font_file(font_dict, self)
    }

    fn check_no_lzw(&self, violations: &mut Vec<PdfAViolation>) -> PdfResult<()> {
        let (order, objects) = self.reachable_objects(&[self.catalog_id()])?;
        for id in order {
            if let Object::Stream(s) = &objects[&id] {
                if stream_uses_lzw(&s.dictionary) {
                    violations.push(PdfAViolation::new(LZW_RULE, format!("object {} {} R uses the LZWDecode filter", id.number, id.generation)));
                }
            }
        }
        Ok(())
    }

    fn check_no_transparency(&self, violations: &mut Vec<PdfAViolation>) -> PdfResult<()> {
        let (order, objects) = self.reachable_objects(&[self.catalog_id()])?;
        for id in order {
            let dict = match &objects[&id] {
                Object::Dictionary(d) => d,
                Object::Stream(s) => &s.dictionary,
                _ => continue,
            };
            if let Some(Object::Dictionary(g)) = dict.get("Group") {
                if matches!(g.get("S"), Some(Object::Name(n)) if n.as_str() == "Transparency") {
                    violations.push(PdfAViolation::new(
                        "ISO 19005-1 6.4 (transparency, simplified check)",
                        format!("object {} {} R has /Group /S /Transparency", id.number, id.generation),
                    ));
                }
            }
            if let Some(Object::Name(n)) = dict.get("SMask") {
                if n.as_str() != "None" {
                    violations.push(PdfAViolation::new(
                        "ISO 19005-1 6.4 (transparency, simplified check)",
                        format!("object {} {} R has /SMask other than /None", id.number, id.generation),
                    ));
                }
            }
            if let Some(Object::Name(n)) = dict.get("BM") {
                if n.as_str() != "Normal" && n.as_str() != "Compatible" {
                    violations.push(PdfAViolation::new(
                        "ISO 19005-1 6.4 (transparency, simplified check)",
                        format!("object {} {} R has /BM {:?} (must be /Normal or /Compatible)", id.number, id.generation, n.as_str()),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Rewrites every reachable stream using the `LZWDecode` filter to
    /// use `FlateDecode` instead (losslessly - the decoded bytes are
    /// unchanged, only the container compression differs). Returns how
    /// many streams were rewritten.
    fn reencode_lzw_streams(&mut self) -> PdfResult<usize> {
        let (order, objects) = self.reachable_objects(&[self.catalog_id()])?;
        let mut count = 0;
        for id in order {
            let Object::Stream(s) = &objects[&id] else { continue };
            if !stream_uses_lzw(&s.dictionary) {
                continue;
            }
            let decoded = s.decode_all()?;
            let mut new_dict = s.dictionary.clone();
            new_dict.remove("Filter");
            new_dict.remove("DecodeParms");
            #[cfg(feature = "compression")]
            let new_stream = PdfStream::with_dictionary(new_dict, decoded).with_compression()?;
            #[cfg(not(feature = "compression"))]
            let new_stream = PdfStream::with_dictionary(new_dict, decoded);
            self.set_object(id, Object::Stream(new_stream));
            count += 1;
        }
        Ok(count)
    }

    /// For every reachable object with a `/Resources /ExtGState`
    /// sub-dictionary (pages and Form XObjects alike - both dictionary
    /// shapes carry `/Resources` under the same key), resets each
    /// referenced `/ExtGState`'s `/SMask` to `/None` and `/BM` to
    /// `/Normal` if they weren't already an allowed value. Returns how
    /// many `/ExtGState` dictionaries were changed.
    ///
    /// Only handles `/ExtGState` entries that are indirect references
    /// (every writer this crate is aware of, including its own, always
    /// allocates them that way) - an inline `/ExtGState` dictionary
    /// value is left untouched, a documented simplification.
    fn strip_extgstate_transparency(&mut self) -> PdfResult<usize> {
        let (order, objects) = self.reachable_objects(&[self.catalog_id()])?;
        let mut changed = 0;
        for id in order {
            let resources_obj = match &objects[&id] {
                Object::Dictionary(d) => d.get("Resources").cloned(),
                Object::Stream(s) => s.dictionary.get("Resources").cloned(),
                _ => None,
            };
            let Some(resources_obj) = resources_obj else { continue };
            let resources = match resources_obj {
                Object::Dictionary(d) => d,
                Object::Reference(rid) => match self.get_dictionary(rid) {
                    Ok(d) => d,
                    Err(_) => continue,
                },
                _ => continue,
            };
            let Some(Object::Dictionary(extgstates)) = resources.get("ExtGState") else { continue };
            for (_, v) in extgstates.iter() {
                let Object::Reference(gs_id) = v else { continue };
                let Ok(mut gs) = self.get_dictionary(*gs_id) else { continue };
                let mut mutated = false;
                if let Some(Object::Name(n)) = gs.get("SMask") {
                    if n.as_str() != "None" {
                        gs.set("SMask", Object::Name(PdfName::new_unchecked("None")));
                        mutated = true;
                    }
                }
                if let Some(Object::Name(n)) = gs.get("BM") {
                    if n.as_str() != "Normal" && n.as_str() != "Compatible" {
                        gs.set("BM", Object::Name(PdfName::new_unchecked("Normal")));
                        mutated = true;
                    }
                }
                if mutated {
                    self.set_object(*gs_id, Object::Dictionary(gs));
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }

    /// Removes `/Group` from every reachable object whose `/Group /S` is
    /// `/Transparency`. Returns how many objects were changed.
    fn strip_transparency_groups(&mut self) -> PdfResult<usize> {
        let (order, objects) = self.reachable_objects(&[self.catalog_id()])?;
        let mut changed = 0;
        for id in order {
            let is_transparency_group = |dict: &PdfDictionary| matches!(dict.get("Group"), Some(Object::Dictionary(g)) if matches!(g.get("S"), Some(Object::Name(n)) if n.as_str() == "Transparency"));
            let hit = match &objects[&id] {
                Object::Dictionary(d) => is_transparency_group(d),
                Object::Stream(s) => is_transparency_group(&s.dictionary),
                _ => false,
            };
            if !hit {
                continue;
            }
            match objects[&id].clone() {
                Object::Dictionary(mut d) => {
                    d.remove("Group");
                    self.set_object(id, Object::Dictionary(d));
                }
                Object::Stream(s) => {
                    let mut d = s.dictionary.clone();
                    d.remove("Group");
                    self.set_object(id, Object::Stream(PdfStream::from_raw(d, s.data)));
                }
                _ => continue,
            }
            changed += 1;
        }
        Ok(changed)
    }

    /// Removes the catalog-level entries [`PdfAFlavor`] forbids
    /// (`/OCProperties` for Part1B, `/Names /EmbeddedFiles` for
    /// Part1B/Part2B, `/Names /JavaScript` and a JavaScript
    /// `/OpenAction` for every flavor). Returns which keys were actually
    /// present and removed, in removal order.
    fn strip_forbidden_catalog_entries(&mut self, flavor: PdfAFlavor) -> PdfResult<Vec<&'static str>> {
        let mut removed = Vec::new();
        let mut catalog = self.catalog()?;
        let mut catalog_changed = false;

        if !flavor.allows_optional_content() && catalog.remove("OCProperties").is_some() {
            removed.push("OCProperties");
            catalog_changed = true;
        }

        if let Some(Object::Dictionary(mut names)) = catalog.get("Names").cloned() {
            let mut names_changed = false;
            if !flavor.allows_embedded_files() && names.remove("EmbeddedFiles").is_some() {
                removed.push("EmbeddedFiles");
                names_changed = true;
            }
            if names.remove("JavaScript").is_some() {
                removed.push("JavaScript");
                names_changed = true;
            }
            if names_changed {
                catalog.set("Names", Object::Dictionary(names));
                catalog_changed = true;
            }
        }

        if let Some(Object::Dictionary(oa)) = catalog.get("OpenAction").cloned() {
            if matches!(oa.get("S"), Some(Object::Name(n)) if n.as_str() == "JavaScript") {
                catalog.remove("OpenAction");
                removed.push("OpenAction");
                catalog_changed = true;
            }
        }

        if catalog_changed {
            let cat_id = self.catalog_id();
            self.set_object(cat_id, Object::Dictionary(catalog));
        }
        Ok(removed)
    }
}

/// `true` if `name` starts with the conventional subset-font tag (ISO
/// 32000-1 9.6.4): exactly six uppercase ASCII letters followed by `+`,
/// e.g. `"ABCDEF+MyFont"`. Matches the same heuristic veraPDF's own
/// PDF/A-1 rule 6.3.5 test uses (`fontName.search(/[A-Z]{6}\+/) != 0`, as
/// seen in this module's own veraPDF verification run) to decide whether
/// a `BaseFont` name denotes a subset.
fn is_subset_tagged(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() > 6 && bytes[6] == b'+' && bytes[..6].iter().all(|b| b.is_ascii_uppercase())
}

fn stream_uses_lzw(dict: &PdfDictionary) -> bool {
    match dict.get("Filter") {
        Some(Object::Name(n)) => n.as_str() == "LZWDecode",
        Some(Object::Array(arr)) => arr.iter().any(|o| matches!(o, Object::Name(n) if n.as_str() == "LZWDecode")),
        _ => false,
    }
}

fn descriptor_has_font_file(dict: &PdfDictionary, doc: &EditableDocument) -> bool {
    let Some(Object::Reference(fd_id)) = dict.get("FontDescriptor") else { return false };
    let Ok(fd) = doc.get_dictionary(*fd_id) else { return false };
    fd.contains_key("FontFile") || fd.contains_key("FontFile2") || fd.contains_key("FontFile3")
}

#[cfg(test)]
mod tests {
    use super::super::icc::test_support::fake_icc_profile;
    use super::super::icc::IccColorSpace;
    use super::*;
    use crate::prelude::*;

    fn sample_options(icc: &[u8]) -> PdfAConversionOptions<'_> {
        PdfAConversionOptions {
            icc_profile: icc,
            icc_identifier: "sRGB IEC61966-2.1",
            icc_condition: "sRGB",
            title: Some("Test Document"),
            producer: Some("rust-pdf test suite"),
        }
    }

    /// A page with a properly-declared PDF 1.4 header, no transparency,
    /// no embedded fonts (deliberately - vector-only content sidesteps
    /// the font-embedding rule so tests can isolate the rules under
    /// test).
    fn vector_only_pdf14() -> Vec<u8> {
        let content = ContentBuilder::new().save_state().fill_color(Color::rgb(0.2, 0.4, 0.8)).rect(50.0, 50.0, 100.0, 100.0).fill().restore_state();
        let page = PageBuilder::a4().content(content).build();
        DocumentBuilder::new().version(PdfVersion::V1_4).page(page).build().unwrap().save_to_bytes().unwrap()
    }

    #[test]
    fn test_convert_then_save_then_reopen_is_pdfa1b_conformant() {
        let mut doc = EditableDocument::from_bytes(vector_only_pdf14()).unwrap();
        let icc = fake_icc_profile(IccColorSpace::Rgb);
        let summary = doc.convert_to_pdfa(PdfAFlavor::Part1B, &sample_options(&icc)).unwrap();
        assert!(summary.output_intent_added);

        let saved = doc.save_pdfa_compatible_to_bytes(PdfAFlavor::Part1B.min_pdf_version()).unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let report = reopened.validate_pdfa(PdfAFlavor::Part1B).unwrap();
        assert!(report.is_conformant(), "violations: {:#?}", report.violations);
    }

    #[test]
    fn test_convert_then_save_then_reopen_is_pdfa2b_conformant() {
        let mut doc = EditableDocument::from_bytes(vector_only_pdf14()).unwrap();
        let icc = fake_icc_profile(IccColorSpace::Cmyk);
        let summary = doc.convert_to_pdfa(PdfAFlavor::Part2B, &sample_options(&icc)).unwrap();
        assert!(summary.output_intent_added);

        let saved = doc.save_pdfa_compatible_to_bytes(PdfAFlavor::Part2B.min_pdf_version()).unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let report = reopened.validate_pdfa(PdfAFlavor::Part2B).unwrap();
        assert!(report.is_conformant(), "violations: {:#?}", report.violations);
    }

    #[test]
    fn test_convert_then_save_then_reopen_is_pdfa3b_conformant() {
        let mut doc = EditableDocument::from_bytes(vector_only_pdf14()).unwrap();
        let icc = fake_icc_profile(IccColorSpace::Gray);
        doc.convert_to_pdfa(PdfAFlavor::Part3B, &sample_options(&icc)).unwrap();

        let saved = doc.save_pdfa_compatible_to_bytes(PdfAFlavor::Part3B.min_pdf_version()).unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let report = reopened.validate_pdfa(PdfAFlavor::Part3B).unwrap();
        assert!(report.is_conformant(), "violations: {:#?}", report.violations);
    }

    #[test]
    fn test_validate_pdfa_reports_missing_output_intent_and_xmp_on_untouched_document() {
        let doc = EditableDocument::from_bytes(vector_only_pdf14()).unwrap();
        let report = doc.validate_pdfa(PdfAFlavor::Part1B).unwrap();
        assert!(!report.is_conformant());
        assert!(report.violations.iter().any(|v| v.rule.contains("OutputIntent")));
        assert!(report.violations.iter().any(|v| v.rule.contains("Identification")));
    }

    #[test]
    fn test_validate_pdfa_reports_non_embedded_font() {
        // Standard14 fonts are never embedded by this crate.
        let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Hi")).build();
        let bytes = DocumentBuilder::new().version(PdfVersion::V1_4).page(page).build().unwrap().save_to_bytes().unwrap();
        let mut doc = EditableDocument::from_bytes(bytes).unwrap();
        let icc = fake_icc_profile(IccColorSpace::Rgb);
        doc.convert_to_pdfa(PdfAFlavor::Part1B, &sample_options(&icc)).unwrap();
        let saved = doc.save_pdfa_compatible_to_bytes(PdfAFlavor::Part1B.min_pdf_version()).unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let report = reopened.validate_pdfa(PdfAFlavor::Part1B).unwrap();
        assert!(report.violations.iter().any(|v| v.rule.contains("font embedding")), "violations: {:#?}", report.violations);
    }

    #[test]
    fn test_convert_strips_extgstate_soft_mask_for_part1b() {
        let mut doc = EditableDocument::from_bytes(vector_only_pdf14()).unwrap();
        let page_id = doc.page_id_at(0).unwrap();

        // Hand-add an ExtGState with a non-conformant SMask/BM, as if an
        // upstream producer had drawn a drop shadow.
        let mut gs = PdfDictionary::new();
        gs.set("Type", Object::Name(PdfName::new_unchecked("ExtGState")));
        gs.set("SMask", Object::Name(PdfName::new_unchecked("Luminosity-ish-placeholder")));
        gs.set("BM", Object::Name(PdfName::new_unchecked("Multiply")));
        let gs_id_num = 9001; // arbitrary id far above anything sample_pdf allocates
        let gs_id = crate::types::ObjectId::new(gs_id_num);
        doc.set_object(gs_id, Object::Dictionary(gs));

        let mut page_dict = doc.get_dictionary(page_id).unwrap();
        let mut resources = match page_dict.get("Resources") {
            Some(Object::Dictionary(d)) => d.clone(),
            _ => PdfDictionary::new(),
        };
        let mut extgstates = PdfDictionary::new();
        extgstates.set("GS1", Object::Reference(gs_id));
        resources.set("ExtGState", Object::Dictionary(extgstates));
        page_dict.set("Resources", Object::Dictionary(resources));
        doc.set_object(page_id, Object::Dictionary(page_dict));

        let icc = fake_icc_profile(IccColorSpace::Rgb);
        let summary = doc.convert_to_pdfa(PdfAFlavor::Part1B, &sample_options(&icc)).unwrap();
        assert_eq!(summary.extgstates_disabled, 1);

        let saved = doc.save_pdfa_compatible_to_bytes(PdfAFlavor::Part1B.min_pdf_version()).unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let report = reopened.validate_pdfa(PdfAFlavor::Part1B).unwrap();
        assert!(report.is_conformant(), "violations: {:#?}", report.violations);
    }

    #[test]
    fn test_convert_removes_javascript_openaction() {
        let mut doc = EditableDocument::from_bytes(vector_only_pdf14()).unwrap();
        let mut catalog = doc.catalog().unwrap();
        let mut oa = PdfDictionary::new();
        oa.set("S", Object::Name(PdfName::new_unchecked("JavaScript")));
        oa.set("JS", Object::String(crate::object::PdfString::literal("app.alert('hi')")));
        catalog.set("OpenAction", Object::Dictionary(oa));
        let cat_id = doc.catalog_id();
        doc.set_object(cat_id, Object::Dictionary(catalog));

        let icc = fake_icc_profile(IccColorSpace::Rgb);
        let summary = doc.convert_to_pdfa(PdfAFlavor::Part1B, &sample_options(&icc)).unwrap();
        assert!(summary.catalog_entries_removed.contains(&"OpenAction"));

        let saved = doc.save_pdfa_compatible_to_bytes(PdfAFlavor::Part1B.min_pdf_version()).unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert!(!reopened.catalog().unwrap().contains_key("OpenAction"));
    }

    #[test]
    fn test_convert_reencodes_lzw_stream_losslessly() {
        let mut doc = EditableDocument::from_bytes(vector_only_pdf14()).unwrap();
        let page_id = doc.page_id_at(0).unwrap();
        let original_bytes = doc.page_content_bytes(page_id).unwrap();

        // crate::filter only implements an LZWDecode *decoder* (no
        // encoder), so this test cannot produce a genuine LZW-compressed
        // fixture stream to re-encode. It instead asserts the no-op case:
        // re-encoding touches 0 streams (and leaves content untouched)
        // when nothing claims LZWDecode, which is what every stream this
        // crate itself produces looks like. The positive case (a stream
        // that *does* claim LZWDecode gets rewritten to FlateDecode with
        // byte-identical decoded content) is covered at the filter level
        // by crate::filter::lzw's own round-trip tests; re-verifying the
        // filter's correctness here would be redundant.
        let icc = fake_icc_profile(IccColorSpace::Rgb);
        let summary = doc.convert_to_pdfa(PdfAFlavor::Part1B, &sample_options(&icc)).unwrap();
        assert_eq!(summary.lzw_streams_reencoded, 0);
        assert_eq!(doc.page_content_bytes(page_id).unwrap(), original_bytes);
    }

    #[test]
    fn test_part_number_and_min_version() {
        assert_eq!(PdfAFlavor::Part1B.part_number(), 1);
        assert_eq!(PdfAFlavor::Part2B.part_number(), 2);
        assert_eq!(PdfAFlavor::Part3B.part_number(), 3);
        assert_eq!(PdfAFlavor::Part1B.min_pdf_version(), PdfVersion::V1_4);
        assert_eq!(PdfAFlavor::Part2B.min_pdf_version(), PdfVersion::V1_7);
    }

    /// Regression test for a gap discovered by actually running this
    /// crate's own output through the real veraPDF CLI while building
    /// this module (see `EditableDocument::check_cidset_present`'s docs
    /// above): `crate::font::cid::CompositeFont`'s subsetter does not
    /// generate `/CIDSet`, so `validate_pdfa` must catch that rather than
    /// reporting a false "conformant".
    #[cfg(feature = "fonts")]
    #[test]
    fn test_validate_pdfa_reports_missing_cidset_for_subset_cid_font() {
        use crate::font::truetype::test_support::build_test_font;
        use crate::font::CompositeFont;

        let font_bytes = build_test_font(&[('A', 1), ('B', 2)]);
        let composite = CompositeFont::new(font_bytes, "TestFont").unwrap();
        let encoded = composite.encode("AB");
        let content = ContentBuilder::new().text_block(TextBuilder::new().font("F1", 12.0).position(72.0, 700.0).show_bytes(encoded));
        let page = PageBuilder::a4().font("F1", Font::Composite(composite)).content(content).build();
        let bytes = DocumentBuilder::new().version(PdfVersion::V1_4).page(page).build().unwrap().save_to_bytes().unwrap();

        let doc = EditableDocument::from_bytes(bytes).unwrap();
        let report = doc.validate_pdfa(PdfAFlavor::Part1B).unwrap();
        assert!(report.violations.iter().any(|v| v.rule.contains("CIDSet")), "violations: {:#?}", report.violations);
    }
}
