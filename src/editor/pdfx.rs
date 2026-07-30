//! PDF/X colour-space constraint checking (ISO 15930, the PDF/X family of
//! print-production conformance standards).
//!
//! # Scope
//!
//! The task this module implements asks specifically for "PDF/X colour
//! space constraint" checking, not full PDF/X-1a/-3/-4 conformance (which,
//! like PDF/A, is a large multi-clause standard covering trim/bleed boxes,
//! required output-referred rendering intents, annotation restrictions,
//! font embedding, etc. - see [`crate::editor::pdfa`]'s module docs for
//! the same kind of scoping note, which applies here too). What this
//! module checks is the single constraint most universally associated
//! with PDF/X and most consistently documented across every PDF/X
//! sub-standard this crate's author is aware of: **content must be
//! expressed in a print-safe (device-independent-via-output-intent or
//! device CMYK/Gray) colour space, never bare `DeviceRGB`/`CalRGB`**,
//! plus the print-workflow output intent PDF/X requires to make "CMYK"
//! meaningful in the first place (ISO 15930 requires an `/OutputIntent`
//! identifying the target print condition, the same mechanism PDF/A uses
//! for a different purpose - see [`crate::editor::icc`]).
//!
//! This is deliberately the strict, PDF/X-1a-style reading (no
//! RGB/CalRGB/Lab anywhere, full stop) rather than the more permissive
//! ICC-managed-RGB-via-output-intent model PDF/X-3/-4 allow - the task
//! wording ("PDF/X color space constraint", singular) is read as asking
//! for *the* well-known PDF/X colour rule, and the strict reading is the
//! safe default when a caller hasn't specified which PDF/X sub-standard
//! they're targeting.
//!
//! Two colour-space entry points are checked: content-stream `rg`/`RG`
//! operators (ISO 32000-1 8.6.8, Table 74 - always unambiguously
//! `DeviceRGB`) and Image XObject `/ColorSpace` entries (ISO 32000-1
//! 8.9.5.2, Table 89: `DeviceRGB`, `CalRGB`, or an `ICCBased` stream whose
//! `/N` is 3). **Not checked**: colour set via a named `/ColorSpace`
//! resource entry through the `cs`/`CS`/`scn`/`SCN` operators (ISO
//! 32000-1 8.6.8) - resolving those requires tracking graphics-state
//! colour-space selection through a content stream, which this module
//! does not implement; only the operators that are unambiguous on their
//! own (`rg`/`RG`, and image dictionaries) are checked. Shading patterns,
//! separation/DeviceN alternate spaces, and Lab are also not inspected.

use super::content_stream::{parse_content_stream, ContentItem};
use super::icc::OutputIntentSubtype;
use crate::error::PdfResult;
use crate::object::{Object, PdfDictionary};
use crate::types::ObjectId;
use std::collections::HashSet;

/// One PDF/X colour-space rule violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfXViolation {
    /// Short, stable rule label.
    pub rule: &'static str,
    /// Human-readable detail (which page/object, which colour space).
    pub message: String,
}

impl PdfXViolation {
    fn new(rule: &'static str, message: impl Into<String>) -> Self {
        Self { rule, message: message.into() }
    }
}

/// Result of [`EditableDocument::validate_pdfx_color`].
#[derive(Debug, Clone)]
pub struct PdfXColorReport {
    /// Every violation found (empty if conformant, within this module's
    /// [documented scope](self)).
    pub violations: Vec<PdfXViolation>,
}

impl PdfXColorReport {
    /// `true` if no violations were found.
    pub fn is_conformant(&self) -> bool {
        self.violations.is_empty()
    }
}

impl super::graph::EditableDocument {
    /// Checks the document against this module's [PDF/X colour-space
    /// constraint](self): a CMYK-capable `/OutputIntent` (`/S /GTS_PDFX`)
    /// must be present, and no content-stream `rg`/`RG` operator or Image
    /// XObject `/ColorSpace` may be `DeviceRGB`/`CalRGB`/3-component
    /// `ICCBased`.
    pub fn validate_pdfx_color(&self) -> PdfResult<PdfXColorReport> {
        let mut violations = Vec::new();

        let has_output_intent = self.output_intents()?.iter().any(|i| i.subtype == "GTS_PDFX" && i.has_valid_icc_profile);
        if !has_output_intent {
            violations.push(PdfXViolation::new(
                "ISO 15930 (PDF/X output intent required)",
                "no /OutputIntents entry with /S /GTS_PDFX and a valid ICC /DestOutputProfile was found",
            ));
        }

        for page_id in self.page_ids()? {
            self.check_page_content_rgb(page_id, &mut violations)?;
            self.check_page_image_colorspaces(page_id, &mut violations)?;
        }

        Ok(PdfXColorReport { violations })
    }

    /// Adds a PDF/X (`/S /GTS_PDFX`) output intent - the print-condition
    /// identification [`EditableDocument::validate_pdfx_color`] requires.
    /// A thin, differently-labeled wrapper over
    /// [`EditableDocument::add_output_intent`] (see
    /// [`crate::editor::icc`]); kept as its own method so callers working
    /// against the PDF/X checklist don't need to import the more general
    /// ICC module just to spell `OutputIntentSubtype::PdfX`.
    pub fn add_pdfx_output_intent(&mut self, icc_profile: &[u8], identifier: &str, condition: &str) -> PdfResult<ObjectId> {
        self.add_output_intent(icc_profile, OutputIntentSubtype::PdfX, identifier, condition)
    }

    fn check_page_content_rgb(&self, page_id: ObjectId, violations: &mut Vec<PdfXViolation>) -> PdfResult<()> {
        let bytes = self.page_content_bytes(page_id)?;
        let items = parse_content_stream(&bytes);
        let mut rg_count = 0usize;
        for item in &items {
            if let ContentItem::Op { operator, .. } = item {
                if operator == "rg" || operator == "RG" {
                    rg_count += 1;
                }
            }
        }
        if rg_count > 0 {
            violations.push(PdfXViolation::new(
                "PDF/X-1a-style CMYK-only colour constraint",
                format!("page object {} {} R uses DeviceRGB fill/stroke ({} `rg`/`RG` operator(s))", page_id.number, page_id.generation, rg_count),
            ));
        }
        Ok(())
    }

    fn check_page_image_colorspaces(&self, page_id: ObjectId, violations: &mut Vec<PdfXViolation>) -> PdfResult<()> {
        let resources = self.page_resources(page_id)?;
        let Some(Object::Dictionary(xobjects)) = resources.get("XObject") else { return Ok(()) };
        let mut checked = HashSet::new();
        for (name, obj) in xobjects.iter() {
            let Object::Reference(id) = obj else { continue };
            if !checked.insert(*id) {
                continue;
            }
            let Some(Object::Stream(stream)) = self.get_object(*id) else { continue };
            if !matches!(stream.dictionary.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Image") {
                continue;
            }
            if let Some(reason) = self.image_colorspace_is_rgb(&stream.dictionary) {
                violations.push(PdfXViolation::new(
                    "PDF/X-1a-style CMYK-only colour constraint",
                    format!("image XObject {name:?} ({} {} R) uses {reason}", id.number, id.generation),
                ));
            }
        }
        Ok(())
    }

    /// Returns `Some(description)` if `image_dict`'s `/ColorSpace` is
    /// (or resolves to) an RGB-family space this module rejects.
    fn image_colorspace_is_rgb(&self, image_dict: &PdfDictionary) -> Option<String> {
        match image_dict.get("ColorSpace") {
            Some(Object::Name(n)) if n.as_str() == "DeviceRGB" => Some("DeviceRGB".to_string()),
            Some(Object::Name(n)) if n.as_str() == "CalRGB" => Some("CalRGB".to_string()),
            Some(Object::Array(arr)) => {
                // `[/CalRGB <<...>>]` or `[/ICCBased n 0 R]`.
                match arr.get(0) {
                    Some(Object::Name(n)) if n.as_str() == "CalRGB" => Some("CalRGB".to_string()),
                    Some(Object::Name(n)) if n.as_str() == "ICCBased" => {
                        let Some(Object::Reference(profile_id)) = arr.get(1) else { return None };
                        let Some(Object::Stream(profile_stream)) = self.get_object(*profile_id) else { return None };
                        match profile_stream.dictionary.get("N") {
                            Some(Object::Integer(3)) => Some("a 3-component (RGB) ICCBased colour space".to_string()),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::icc::test_support::fake_icc_profile;
    use super::super::icc::IccColorSpace;
    use crate::prelude::*;

    fn doc_with_one_page() -> EditableDocument {
        let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(ContentBuilder::new()).build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        EditableDocument::from_bytes(bytes).unwrap()
    }

    #[test]
    fn test_missing_output_intent_and_no_rgb_use_is_reported() {
        let doc = doc_with_one_page();
        let report = doc.validate_pdfx_color().unwrap();
        assert!(!report.is_conformant());
        assert_eq!(report.violations.len(), 1);
        assert!(report.violations[0].rule.contains("output intent"));
    }

    #[test]
    fn test_device_rgb_fill_is_flagged() {
        let mut doc = doc_with_one_page();
        let page_id = doc.page_id_at(0).unwrap();
        let rgb_content = ContentBuilder::new().save_state().fill_color(Color::rgb(1.0, 0.0, 0.0)).rect(0.0, 0.0, 10.0, 10.0).fill().restore_state();
        doc.replace_page_content(page_id, &rgb_content).unwrap();

        let icc = fake_icc_profile(IccColorSpace::Cmyk);
        doc.add_pdfx_output_intent(&icc, "FOGRA39", "Coated").unwrap();

        let report = doc.validate_pdfx_color().unwrap();
        assert!(report.violations.iter().any(|v| v.message.contains("DeviceRGB")), "violations: {:#?}", report.violations);
    }

    #[test]
    fn test_cmyk_fill_with_output_intent_is_conformant() {
        let mut doc = doc_with_one_page();
        let page_id = doc.page_id_at(0).unwrap();
        let cmyk_content = ContentBuilder::new().save_state().fill_color(Color::cmyk(0.0, 0.0, 0.0, 1.0)).rect(0.0, 0.0, 10.0, 10.0).fill().restore_state();
        doc.replace_page_content(page_id, &cmyk_content).unwrap();

        let icc = fake_icc_profile(IccColorSpace::Cmyk);
        doc.add_pdfx_output_intent(&icc, "FOGRA39", "Coated").unwrap();

        let report = doc.validate_pdfx_color().unwrap();
        assert!(report.is_conformant(), "violations: {:#?}", report.violations);
    }

    #[cfg(feature = "images")]
    #[test]
    fn test_rgb_image_xobject_is_flagged() {
        use crate::image::{ColorSpace, Image, ImageFilter};
        use crate::types::Rectangle;

        let mut doc = doc_with_one_page();
        let page_id = doc.page_id_at(0).unwrap();
        let image = Image::new(2, 2, ColorSpace::DeviceRGB, 8, ImageFilter::FlateDecode, vec![0, 0, 0, 255, 255, 255, 128, 128, 128, 0, 0, 0]);
        doc.draw_image(page_id, "Im1", &image, Rectangle::new(0.0, 0.0, 10.0, 10.0)).unwrap();

        let icc = fake_icc_profile(IccColorSpace::Cmyk);
        doc.add_pdfx_output_intent(&icc, "FOGRA39", "Coated").unwrap();

        let report = doc.validate_pdfx_color().unwrap();
        assert!(report.violations.iter().any(|v| v.message.contains("DeviceRGB")), "violations: {:#?}", report.violations);
    }

    #[test]
    fn test_output_intent_survives_and_is_recognized_after_full_rewrite() {
        let mut doc = doc_with_one_page();
        let icc = fake_icc_profile(IccColorSpace::Cmyk);
        doc.add_pdfx_output_intent(&icc, "FOGRA39", "Coated").unwrap();
        let saved = doc.save_full_rewrite_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let report = reopened.validate_pdfx_color().unwrap();
        assert!(report.is_conformant(), "violations: {:#?}", report.violations);
    }
}
