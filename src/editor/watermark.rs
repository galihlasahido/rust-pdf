//! Text watermarking via content-stream injection (ISO 32000-1:2008
//! Section 7.8.2 "Content Streams"): stamps a rotated, semi-transparent
//! text watermark across every page of an already-open document, drawn
//! *over* whatever content the page already has (painter's model) rather
//! than replacing it.
//!
//! Like [`EditableDocument::add_highlight_annotation`]'s translucent
//! wash, opacity is achieved via an `/ExtGState` `/ca` entry (ISO
//! 32000-1 Table 58); like every annotation appearance in
//! [`crate::editor::annotations`], text is drawn with a Standard-14
//! Helvetica font (no embedding needed) added directly to the page's own
//! `/Resources` rather than relying on the AcroForm `/DR` or any font the
//! page's existing content happens to declare. Because this crate has no
//! font-metrics table for Standard-14 fonts, horizontal centering uses
//! the same fixed-fraction-of-`font_size`-per-character estimate already
//! used by [`crate::editor::annotations`]'s stamp appearance, not exact
//! glyph widths - good enough to roughly center a watermark, not
//! typographically exact. For the same single-byte-encoding reason as
//! [`EditableDocument::replace_page_text`], watermark text is only
//! guaranteed to render correctly for Latin-1/WinAnsi-representable
//! characters.

use super::graph::EditableDocument;
use super::util::unique_resource_name;
use crate::color::Color;
use crate::content::ContentBuilder;
use crate::error::{EditorError, PdfResult};
use crate::object::{Object, PdfDictionary, PdfName};
use crate::types::{ObjectId, Rectangle};

/// Options for [`EditableDocument::add_text_watermark`].
#[derive(Debug, Clone)]
pub struct WatermarkOptions<'a> {
    /// The watermark text (must be non-empty).
    pub text: &'a str,
    /// Font size in PDF points (must be positive and finite).
    pub font_size: f64,
    /// Fill opacity: `0.0` (invisible) ..= `1.0` (fully opaque). Values
    /// outside that range are silently clamped rather than rejected.
    pub opacity: f64,
    /// Counter-clockwise rotation in degrees around the page's center
    /// (a classic watermark look is `45.0`, drawn diagonally).
    pub rotation_degrees: f64,
    /// Fill color.
    pub color: Color,
}

impl Default for WatermarkOptions<'_> {
    fn default() -> Self {
        Self {
            text: "",
            font_size: 48.0,
            opacity: 0.3,
            rotation_degrees: 45.0,
            color: Color::gray(0.5),
        }
    }
}

impl EditableDocument {
    /// Stamps `options.text` across every page of this document (see the
    /// [module docs](self) for exactly how). Returns the number of pages
    /// watermarked (i.e. this document's page count).
    pub fn add_text_watermark(&mut self, options: &WatermarkOptions) -> PdfResult<usize> {
        if options.text.is_empty() {
            return Err(EditorError::InvalidArgument("watermark text must not be empty".to_string()).into());
        }
        if !(options.font_size.is_finite() && options.font_size > 0.0) {
            return Err(EditorError::InvalidArgument(
                "watermark font_size must be a positive, finite number".to_string(),
            )
            .into());
        }

        let page_count = self.page_count()?;
        for index in 0..page_count {
            let page_id = self.page_id_at(index)?;
            self.add_text_watermark_to_page(page_id, options)?;
        }
        Ok(page_count)
    }

    fn add_text_watermark_to_page(&mut self, page_id: ObjectId, options: &WatermarkOptions) -> PdfResult<()> {
        let rect = self.page_media_box_for_watermark(page_id);

        let mut page_dict = self.get_dictionary(page_id)?;
        let mut resources = match page_dict.get("Resources") {
            Some(Object::Dictionary(d)) => d.clone(),
            Some(Object::Reference(id)) => self.get_dictionary(*id)?,
            _ => PdfDictionary::new(),
        };

        let mut fonts = match resources.get("Font") {
            Some(Object::Dictionary(d)) => d.clone(),
            _ => PdfDictionary::new(),
        };
        let font_name = unique_resource_name(&fonts, "WMHelv");
        let mut helv = PdfDictionary::new();
        helv.set("Type", Object::Name(PdfName::new_unchecked("Font")));
        helv.set("Subtype", Object::Name(PdfName::new_unchecked("Type1")));
        helv.set("BaseFont", Object::Name(PdfName::new_unchecked("Helvetica")));
        fonts.set(font_name.clone(), Object::Dictionary(helv));
        resources.set("Font", Object::Dictionary(fonts));

        let mut extgstates = match resources.get("ExtGState") {
            Some(Object::Dictionary(d)) => d.clone(),
            _ => PdfDictionary::new(),
        };
        let gs_name = unique_resource_name(&extgstates, "WMGS");
        let mut gs = PdfDictionary::new();
        gs.set("Type", Object::Name(PdfName::new_unchecked("ExtGState")));
        gs.set("ca", Object::Real(options.opacity.clamp(0.0, 1.0)));
        extgstates.set(gs_name.clone(), Object::Dictionary(gs));
        resources.set("ExtGState", Object::Dictionary(extgstates));

        page_dict.set("Resources", Object::Dictionary(resources));
        self.set_object(page_id, Object::Dictionary(page_dict));

        let center_x = rect.llx + rect.width() / 2.0;
        let center_y = rect.lly + rect.height() / 2.0;
        // See the [module docs](self) for why this is an estimate, not
        // exact font metrics.
        let estimated_width = options.text.chars().count() as f64 * options.font_size * 0.5;

        let content = ContentBuilder::new()
            .raw(format!("/{gs_name} gs"))
            .fill_color(options.color)
            .translate(center_x, center_y)
            .rotate(options.rotation_degrees)
            .text(
                &font_name,
                options.font_size,
                -estimated_width / 2.0,
                -options.font_size / 3.0,
                options.text,
            );

        self.append_page_content(page_id, &content)
    }

    /// Reads a page's own `/MediaBox`, walking `/Parent` if the leaf
    /// doesn't set one directly (ISO 32000-1 Table 30, inheritable),
    /// falling back to US Letter for a missing/malformed one.
    ///
    /// A local, `render`-feature-independent copy of the same walk
    /// `EditableDocument::effective_media_box` (in
    /// [`super::pages`]) already does for [`crate::render::PdfRenderer`] -
    /// that one is `#[cfg(feature = "render")]`-gated (its only caller),
    /// and widening its availability is out of scope for this
    /// watermarking feature, so this is a small, deliberately separate
    /// copy rather than a shared helper.
    fn page_media_box_for_watermark(&self, mut id: ObjectId) -> Rectangle {
        for _ in 0..64 {
            let Ok(dict) = self.get_dictionary(id) else { break };
            if let Some(Object::Array(arr)) = dict.get("MediaBox") {
                if arr.len() == 4 {
                    let vals: Option<Vec<f64>> = arr.iter().map(|o| o.as_real()).collect();
                    if let Some(vals) = vals {
                        let (llx, urx) = (vals[0].min(vals[2]), vals[0].max(vals[2]));
                        let (lly, ury) = (vals[1].min(vals[3]), vals[1].max(vals[3]));
                        if urx > llx && ury > lly {
                            return Rectangle::new(llx, lly, urx, ury);
                        }
                    }
                }
            }
            match dict.get("Parent") {
                Some(Object::Reference(parent)) => id = *parent,
                _ => break,
            }
        }
        Rectangle::letter()
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
                .content(ContentBuilder::new().text("F1", 12.0, 72.0, 700.0, &format!("Page {i} body text")))
                .build();
            builder = builder.page(page);
        }
        let bytes = builder.build().unwrap().save_to_bytes().unwrap();
        EditableDocument::from_bytes(bytes).unwrap()
    }

    #[test]
    fn test_watermark_applied_to_every_page() {
        let mut doc = doc_with_pages(3);
        let options = WatermarkOptions {
            text: "CONFIDENTIAL",
            ..WatermarkOptions::default()
        };
        let count = doc.add_text_watermark(&options).unwrap();
        assert_eq!(count, 3);

        for i in 0..3 {
            let page_id = doc.page_id_at(i).unwrap();
            let bytes = doc.page_content_bytes(page_id).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(text.contains("(CONFIDENTIAL) Tj"), "page {i} content was: {text}");
            // Original content must survive underneath.
            assert!(text.contains(&format!("Page {i} body text")));
        }
    }

    #[test]
    fn test_watermark_adds_font_and_extgstate_resources() {
        let mut doc = doc_with_pages(1);
        let options = WatermarkOptions {
            text: "DRAFT",
            opacity: 0.25,
            ..WatermarkOptions::default()
        };
        doc.add_text_watermark(&options).unwrap();

        let page_id = doc.page_id_at(0).unwrap();
        let page_dict = doc.get_dictionary(page_id).unwrap();
        let Some(Object::Dictionary(resources)) = page_dict.get("Resources") else {
            panic!("expected a Resources dictionary")
        };
        let Some(Object::Dictionary(fonts)) = resources.get("Font") else {
            panic!("expected a Font resources dictionary")
        };
        // The page's own original font ("F1") plus the newly-added
        // watermark font must both be present.
        assert!(fonts.contains_key("F1"));
        assert!(fonts.iter().any(|(name, _)| name.starts_with("WMHelv")));

        let Some(Object::Dictionary(extgstates)) = resources.get("ExtGState") else {
            panic!("expected an ExtGState resources dictionary")
        };
        assert!(extgstates.iter().any(|(name, _)| name.starts_with("WMGS")));
    }

    #[test]
    fn test_watermark_rejects_empty_text() {
        let mut doc = doc_with_pages(1);
        let options = WatermarkOptions {
            text: "",
            ..WatermarkOptions::default()
        };
        assert!(doc.add_text_watermark(&options).is_err());
    }

    #[test]
    fn test_watermark_rejects_non_positive_font_size() {
        let mut doc = doc_with_pages(1);
        let options = WatermarkOptions {
            text: "X",
            font_size: 0.0,
            ..WatermarkOptions::default()
        };
        assert!(doc.add_text_watermark(&options).is_err());
    }

    #[test]
    fn test_watermarked_document_still_saves_and_reopens() {
        let mut doc = doc_with_pages(2);
        let options = WatermarkOptions {
            text: "SAMPLE",
            ..WatermarkOptions::default()
        };
        doc.add_text_watermark(&options).unwrap();
        let bytes = doc.save_full_rewrite_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(bytes).unwrap();
        assert_eq!(reopened.page_count().unwrap(), 2);
        let content_bytes = reopened.page_content_bytes(reopened.page_id_at(0).unwrap()).unwrap();
        let text = String::from_utf8_lossy(&content_bytes);
        assert!(text.contains("(SAMPLE) Tj"));
    }
}
