//! Approximate per-run text-layer geometry for a rendered page - the
//! bounding boxes a UI needs to overlay a selectable/copyable
//! transparent text layer on top of a page raster (mirroring what e.g.
//! pdf.js's own "text layer" does; ISO 32000-1 has no equivalent
//! built-in structure of its own - a rendered PDF page is fundamentally
//! just painted shapes, not a document of positioned, selectable text
//! runs).
//!
//! [`EditableDocument::extract_page_text_layout`] walks a page's content
//! stream tracking the text/CTM matrices (`Tm`/`Td`/`TD`/`T*`/`cm`/`q`/`Q`,
//! the same subset [`super::redact`] tracks) and, for every
//! text-showing operator (`Tj`/`'`/`"`/`TJ`), decodes its Unicode text
//! (same decoding as [`super::text_extract::EditableDocument::extract_page_text`])
//! and computes its rendered bounding box via
//! [`super::text_geometry::compute_show_text_advance_and_bbox`] - the
//! same approximate-but-documented geometry
//! [`super::redact::EditableDocument::apply_redaction`] uses to decide
//! what to remove.
//!
//! # Why one [`TextRun`] per text-showing operator, not per word/line
//!
//! A "run" here is exactly the span of characters one `Tj`/`'`/`"`/`TJ`
//! call shows. In practice this is usually close to one visual line (a
//! PDF producer commonly emits a single `TJ` per line, using its numeric
//! adjustments for inter-glyph kerning rather than splitting into
//! multiple operators) but it can just as easily be a single word, a
//! partial word, or several lines' worth of text, depending on how the
//! producing application chose to chunk its content stream. There is no
//! reliable way to recover "line" or "word" as a first-class concept
//! from content-stream operators alone without full layout
//! re-analysis/glyph shaping (see [`super::redact`]'s and
//! [`crate::editor`]'s module docs for why that is out of scope
//! crate-wide). A caller wanting visual lines can approximate them by
//! merging adjacent runs whose `y`/`height` overlap; this module leaves
//! that to the caller rather than guessing.
//!
//! # Known limitations (disclosed, not silently wrong)
//!
//! - **Approximate glyph metrics**, identical to
//!   [`super::redact`]'s bounding-box computation: real `/Widths`/`/W`
//!   advance widths when present, a fixed generous fallback otherwise,
//!   and a fixed ascent/descent rather than the font's actual
//!   `/FontDescriptor` values. Good enough to place a
//!   same-order-of-magnitude selectable region under the rendered
//!   glyphs; not pixel-accurate to each glyph, especially for
//!   proportional fonts, non-Latin scripts, or justified text with
//!   custom word spacing.
//! - **Coordinates are in the page's own unrotated default user space**
//!   (matching `/MediaBox`, before `/Rotate` is applied) - the same
//!   space [`EditableDocument::effective_media_box`] (via
//!   `pages::effective_rotate`/`effective_media_box`) returns and
//!   `crate::render::render_page_document`'s *unrotated* content raster
//!   is measured in, before that function's own post-processing rotation
//!   step. A caller overlaying these boxes on `render_page`'s actual
//!   (rotated) output must itself account for the page's `/Rotate` for
//!   both the raster and these boxes consistently; this module does not
//!   rotate for you - see
//!   `crate::tauri_commands::commands::get_text_layout_impl`'s docs for
//!   how (and whether) the Tauri command layer currently handles this.
//! - **Nested Form XObjects are not descended into** - only the page's
//!   own content stream is walked, matching
//!   `extract_page_text`/`redact`'s same limitation.
//! - **No word/character-level boxes** - see "Why one `TextRun` per
//!   operator" above.

use super::content_stream::{parse_content_stream, ContentItem};
use super::graph::EditableDocument;
use super::text_geometry::{compute_show_text_advance_and_bbox, matrix_from_operands, owned_pieces_from_tj_array, FontWidths, OwnedPiece};
use crate::error::PdfResult;
use crate::font::encoding::win_ansi_bytes_to_string;
use crate::font::tounicode::parse_tounicode_cmap;
use crate::object::{Object, PdfDictionary, PdfString};
use crate::types::{Matrix, ObjectId};
use std::collections::BTreeMap;

/// One text-showing operator's decoded text and rendered bounding box,
/// in the page's default user space (ISO 32000-1 8.3.2.2: origin at the
/// page's lower-left, `y` increasing upward) - see the [module
/// docs](self) for granularity and accuracy caveats.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    /// Decoded Unicode text this run shows (see
    /// [`super::text_extract::EditableDocument::extract_page_text`]'s
    /// docs for exactly how codes are resolved to Unicode).
    pub text: String,
    /// Lower-left X of the run's bounding box (page default user space).
    pub x: f64,
    /// Lower-left Y of the run's bounding box (page default user space).
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

// Mirrors `super::text_extract`'s private `FontDecoder`/
// `build_font_decoder`. Deliberately duplicated (a small, ~30-line,
// self-contained enum with no dependency on this module's matrix
// tracking) rather than widened to `pub(crate)` there: that module is
// covered by its own tests and this keeps its existing private surface
// untouched for the sake of one extra caller, unlike the font-width
// measurement/matrix math this module *does* share via
// `text_geometry` (a much larger, easier-to-drift-apart piece of logic
// where duplication would be the real risk).
enum FontDecoder {
    Simple { tounicode: Option<BTreeMap<u32, String>> },
    Composite { tounicode: Option<BTreeMap<u32, String>> },
}

impl FontDecoder {
    fn code_width(&self) -> usize {
        match self {
            FontDecoder::Simple { .. } => 1,
            FontDecoder::Composite { .. } => 2,
        }
    }

    fn decode(&self, bytes: &[u8]) -> String {
        let width = self.code_width();
        let mut out = String::new();
        for chunk in bytes.chunks(width) {
            let code = chunk.iter().fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
            match self {
                FontDecoder::Simple { tounicode } => {
                    if let Some(text) = tounicode.as_ref().and_then(|m| m.get(&code)) {
                        out.push_str(text);
                    } else if chunk.len() == 1 {
                        out.push_str(&win_ansi_bytes_to_string(chunk));
                    }
                }
                FontDecoder::Composite { tounicode } => {
                    if let Some(text) = tounicode.as_ref().and_then(|m| m.get(&code)) {
                        out.push_str(text);
                    } else {
                        out.push('\u{FFFD}');
                    }
                }
            }
        }
        out
    }
}

fn build_font_decoder(doc: &EditableDocument, dict: &PdfDictionary) -> FontDecoder {
    let tounicode = match dict.get("ToUnicode") {
        Some(Object::Reference(id)) => match doc.get_object(*id) {
            Some(Object::Stream(s)) => s.decode_all().ok().map(|bytes| parse_tounicode_cmap(&bytes)),
            _ => None,
        },
        _ => None,
    };
    let is_composite = matches!(dict.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Type0");
    if is_composite {
        FontDecoder::Composite { tounicode }
    } else {
        FontDecoder::Simple { tounicode }
    }
}

fn decode_string(decoder: Option<&FontDecoder>, s: &PdfString) -> String {
    let bytes = s.as_bytes();
    match decoder {
        Some(d) => d.decode(bytes),
        // No active font resolved (malformed content stream): fall back
        // to the WinAnsi table, matching `text_extract`'s convention.
        None => win_ansi_bytes_to_string(bytes),
    }
}

/// Appends `text`/`bbox` as a new [`TextRun`], unless `text` is entirely
/// whitespace (nothing there for a caller to usefully select/overlay).
fn push_run(runs: &mut Vec<TextRun>, text: String, bbox: (f64, f64, f64, f64)) {
    if text.trim().is_empty() {
        return;
    }
    let (minx, miny, maxx, maxy) = bbox;
    runs.push(TextRun {
        text,
        x: minx,
        y: miny,
        width: (maxx - minx).max(0.0),
        height: (maxy - miny).max(0.0),
    });
}

impl EditableDocument {
    /// Approximate per-run text-layer geometry for `page_id` - see the
    /// [module docs](self) for what a "run" is and this feature's
    /// disclosed limitations.
    pub fn extract_page_text_layout(&self, page_id: ObjectId) -> PdfResult<Vec<TextRun>> {
        let resources = self.effective_resources(page_id)?;
        let font_dict = match resources.get("Font") {
            Some(Object::Dictionary(d)) => d.clone(),
            Some(Object::Reference(id)) => self.get_dictionary(*id).unwrap_or_default(),
            _ => PdfDictionary::new(),
        };

        let mut decoders: BTreeMap<String, FontDecoder> = BTreeMap::new();
        let mut widths: BTreeMap<String, FontWidths> = BTreeMap::new();
        for (name, value) in font_dict.iter() {
            let dict = match value {
                Object::Dictionary(d) => d.clone(),
                Object::Reference(id) => self.get_dictionary(*id).unwrap_or_default(),
                _ => continue,
            };
            decoders.insert(name.clone(), build_font_decoder(self, &dict));
            widths.insert(name.clone(), FontWidths::from_font_dict(self, &dict));
        }

        let content = self.page_content_bytes(page_id)?;
        let items = parse_content_stream(&content);

        let mut runs = Vec::new();
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

        for item in &items {
            let ContentItem::Op { operator, operands } = item else { continue };
            match operator.as_str() {
                "q" => ctm_stack.push(ctm),
                "Q" => {
                    if let Some(m) = ctm_stack.pop() {
                        ctm = m;
                    }
                }
                "cm" => {
                    if let Some(m) = matrix_from_operands(operands) {
                        ctm = m.multiply(&ctm);
                    }
                }
                "BT" => {
                    tm = Matrix::identity();
                    tlm = Matrix::identity();
                }
                "Tf" => {
                    if let (Some(Object::Name(n)), Some(sz)) =
                        (operands.first(), operands.get(1).and_then(|o| o.as_real()))
                    {
                        cur_font = Some(n.as_str().to_string());
                        font_size = sz;
                    }
                }
                "Tc" => {
                    if let Some(v) = operands.first().and_then(|o| o.as_real()) {
                        char_spacing = v;
                    }
                }
                "Tw" => {
                    if let Some(v) = operands.first().and_then(|o| o.as_real()) {
                        word_spacing = v;
                    }
                }
                "Tz" => {
                    if let Some(v) = operands.first().and_then(|o| o.as_real()) {
                        h_scale = v / 100.0;
                    }
                }
                "TL" => {
                    if let Some(v) = operands.first().and_then(|o| o.as_real()) {
                        leading = v;
                    }
                }
                "Ts" => {
                    if let Some(v) = operands.first().and_then(|o| o.as_real()) {
                        rise = v;
                    }
                }
                "Td" => {
                    if let (Some(tx), Some(ty)) =
                        (operands.first().and_then(|o| o.as_real()), operands.get(1).and_then(|o| o.as_real()))
                    {
                        tlm = Matrix::translate(tx, ty).multiply(&tlm);
                        tm = tlm;
                    }
                }
                "TD" => {
                    if let (Some(tx), Some(ty)) =
                        (operands.first().and_then(|o| o.as_real()), operands.get(1).and_then(|o| o.as_real()))
                    {
                        leading = -ty;
                        tlm = Matrix::translate(tx, ty).multiply(&tlm);
                        tm = tlm;
                    }
                }
                "T*" => {
                    tlm = Matrix::translate(0.0, -leading).multiply(&tlm);
                    tm = tlm;
                }
                "Tm" => {
                    if let Some(m) = matrix_from_operands(operands) {
                        tm = m;
                        tlm = m;
                    }
                }
                "Tj" => {
                    if let Some(Object::String(s)) = operands.last() {
                        let fw = cur_font.as_deref().and_then(|n| widths.get(n));
                        let decoder = cur_font.as_deref().and_then(|n| decoders.get(n));
                        let text = decode_string(decoder, s);
                        let pieces = [OwnedPiece::Str(s.as_bytes().to_vec())];
                        let (new_tm, bbox) = compute_show_text_advance_and_bbox(
                            &pieces, fw, font_size, char_spacing, word_spacing, h_scale, rise, tm, ctm,
                        );
                        tm = new_tm;
                        push_run(&mut runs, text, bbox);
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
                    if let Some(Object::String(s)) = operands.last() {
                        let fw = cur_font.as_deref().and_then(|n| widths.get(n));
                        let decoder = cur_font.as_deref().and_then(|n| decoders.get(n));
                        let text = decode_string(decoder, s);
                        let pieces = [OwnedPiece::Str(s.as_bytes().to_vec())];
                        let (new_tm, bbox) = compute_show_text_advance_and_bbox(
                            &pieces, fw, font_size, char_spacing, word_spacing, h_scale, rise, tm, ctm,
                        );
                        tm = new_tm;
                        push_run(&mut runs, text, bbox);
                    }
                }
                "TJ" => {
                    if let Some(Object::Array(arr)) = operands.first() {
                        let decoder = cur_font.as_deref().and_then(|n| decoders.get(n));
                        let mut text = String::new();
                        for elem in arr.iter() {
                            match elem {
                                Object::String(s) => text.push_str(&decode_string(decoder, s)),
                                Object::Integer(n) if *n < -100 => text.push(' '),
                                Object::Real(n) if *n < -100.0 => text.push(' '),
                                _ => {}
                            }
                        }
                        let fw = cur_font.as_deref().and_then(|n| widths.get(n));
                        let pieces = owned_pieces_from_tj_array(arr);
                        let (new_tm, bbox) = compute_show_text_advance_and_bbox(
                            &pieces, fw, font_size, char_spacing, word_spacing, h_scale, rise, tm, ctm,
                        );
                        tm = new_tm;
                        push_run(&mut runs, text, bbox);
                    }
                }
                _ => {}
            }
        }

        Ok(runs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

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
    fn single_run_reports_decoded_text_and_a_plausible_box() {
        let (doc, id) = page_with_text_at("Hello, world!", 72.0, 700.0, 24.0);
        let runs = doc.extract_page_text_layout(id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "Hello, world!");
        // Baseline sits at y=700; the box must straddle it (descender
        // below, ascender above) rather than e.g. collapsing to a point.
        assert!(runs[0].y < 700.0 && runs[0].y + runs[0].height > 700.0);
        // Sanity: a 13-character run at 24pt is nowhere near either 0
        // width nor absurdly wide.
        assert!(runs[0].width > 50.0 && runs[0].width < 400.0);
        // x roughly starts where the text was placed (72pt).
        assert!((runs[0].x - 72.0).abs() < 5.0);
    }

    #[test]
    fn multiple_lines_produce_runs_with_increasing_y_going_down_the_page() {
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(
                ContentBuilder::new()
                    .text("F1", 12.0, 72.0, 750.0, "First line")
                    .text("F1", 12.0, 72.0, 700.0, "Second line"),
            )
            .build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        let doc = EditableDocument::from_bytes(bytes).unwrap();
        let id = doc.page_id_at(0).unwrap();

        let runs = doc.extract_page_text_layout(id).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "First line");
        assert_eq!(runs[1].text, "Second line");
        // Page default user space has y increasing upward, so the run
        // drawn lower on the (visual) page has a smaller y.
        assert!(runs[1].y < runs[0].y);
    }

    #[test]
    fn empty_page_yields_no_runs() {
        let page = PageBuilder::a4().build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        let doc = EditableDocument::from_bytes(bytes).unwrap();
        let id = doc.page_id_at(0).unwrap();
        assert!(doc.extract_page_text_layout(id).unwrap().is_empty());
    }
}
