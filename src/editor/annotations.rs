//! Markup annotations (ISO 32000-1:2008 Section 12.5 "Annotations"):
//! highlight, underline, strikeout, free text, stamp, ink and
//! text-note/popup ("comment") annotations - create, edit, delete, list,
//! each with a generated `/AP /N` appearance stream so the annotation
//! renders correctly in any conformant reader rather than relying on the
//! reader to synthesize one.

use super::graph::EditableDocument;
use super::util::{appearance_xobject, appearance_xobject_with_extra_resources, rect_from_object, rect_to_array, to_pdf_text_string};
use crate::color::Color;
use crate::error::{EditorError, PdfResult};
use crate::object::{Object, PdfArray, PdfDictionary, PdfName};
use crate::types::{ObjectId, Rectangle};

/// Maximum number of annotations processed in one call (list/delete),
/// bounding work against a corrupt/adversarial `/Annots` array.
const MAX_ANNOTS_PER_PAGE: usize = 100_000;

/// The kind of a markup/comment annotation, as read back from `/Subtype`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationKind {
    /// ISO 32000-1 12.5.6.10.
    Highlight,
    /// ISO 32000-1 12.5.6.10.
    Underline,
    /// ISO 32000-1 12.5.6.10.
    StrikeOut,
    /// ISO 32000-1 12.5.6.6.
    FreeText,
    /// ISO 32000-1 12.5.6.12.
    Stamp,
    /// ISO 32000-1 12.5.6.13.
    Ink,
    /// ISO 32000-1 12.5.6.4 ("sticky note").
    Text,
    /// ISO 32000-1 12.5.6.2.
    Popup,
    /// Any other `/Subtype` this module doesn't specifically model.
    Other,
}

impl AnnotationKind {
    fn subtype_name(self) -> &'static str {
        match self {
            AnnotationKind::Highlight => "Highlight",
            AnnotationKind::Underline => "Underline",
            AnnotationKind::StrikeOut => "StrikeOut",
            AnnotationKind::FreeText => "FreeText",
            AnnotationKind::Stamp => "Stamp",
            AnnotationKind::Ink => "Ink",
            AnnotationKind::Text => "Text",
            AnnotationKind::Popup => "Popup",
            AnnotationKind::Other => "",
        }
    }

    fn from_subtype(name: &str) -> Self {
        match name {
            "Highlight" => AnnotationKind::Highlight,
            "Underline" => AnnotationKind::Underline,
            "StrikeOut" => AnnotationKind::StrikeOut,
            "FreeText" => AnnotationKind::FreeText,
            "Stamp" => AnnotationKind::Stamp,
            "Ink" => AnnotationKind::Ink,
            "Text" => AnnotationKind::Text,
            "Popup" => AnnotationKind::Popup,
            _ => AnnotationKind::Other,
        }
    }
}

/// A read-back summary of one annotation, returned by
/// [`EditableDocument::list_annotations`].
#[derive(Debug, Clone)]
pub struct AnnotationInfo {
    /// The annotation's own object id.
    pub id: ObjectId,
    /// Its `/Subtype`.
    pub kind: AnnotationKind,
    /// Its `/Rect`.
    pub rect: Rectangle,
    /// Its `/Contents` (comment text / free text body), if any.
    pub contents: Option<String>,
    /// Its `/T` (author/title), if any.
    pub author: Option<String>,
}

impl EditableDocument {
    // -- Highlight / underline / strikeout (text markup) -------------------

    /// Adds a highlight annotation (ISO 32000-1 12.5.6.10) covering the
    /// given quadrilaterals (`(llx, lly, urx, ury)` rectangles in page
    /// user space - one per highlighted line/word run), rendered as a
    /// semi-transparent color wash using a `Multiply` blend mode
    /// (matching how Acrobat itself renders highlights so text underneath
    /// stays legible).
    pub fn add_highlight_annotation(&mut self, page_index: usize, quads: &[(f64, f64, f64, f64)], color: Color) -> PdfResult<ObjectId> {
        self.add_text_markup(page_index, quads, color, AnnotationKind::Highlight)
    }

    /// Adds an underline annotation (ISO 32000-1 12.5.6.10): a line drawn
    /// under each quadrilateral.
    pub fn add_underline_annotation(&mut self, page_index: usize, quads: &[(f64, f64, f64, f64)], color: Color) -> PdfResult<ObjectId> {
        self.add_text_markup(page_index, quads, color, AnnotationKind::Underline)
    }

    /// Adds a strikeout annotation (ISO 32000-1 12.5.6.10): a line drawn
    /// through the middle of each quadrilateral.
    pub fn add_strikeout_annotation(&mut self, page_index: usize, quads: &[(f64, f64, f64, f64)], color: Color) -> PdfResult<ObjectId> {
        self.add_text_markup(page_index, quads, color, AnnotationKind::StrikeOut)
    }

    fn add_text_markup(&mut self, page_index: usize, quads: &[(f64, f64, f64, f64)], color: Color, kind: AnnotationKind) -> PdfResult<ObjectId> {
        if quads.is_empty() {
            return Err(EditorError::InvalidArgument("text markup annotation needs at least one quadrilateral".to_string()).into());
        }
        let page_id = self.page_id_at(page_index)?;
        let rect = bounding_rect(quads);
        let annot_id = self.allocate_id();

        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked(kind.subtype_name())));
        dict.set("Rect", Object::Array(rect_to_array(rect)));
        dict.set("QuadPoints", Object::Array(quad_points_array(quads)));
        dict.set("C", color_to_array(color));
        dict.set("F", Object::Integer(4)); // Print flag (ISO 32000-1 Table 165).

        self.set_object(annot_id, Object::Dictionary(dict));
        if kind == AnnotationKind::Highlight {
            let content = highlight_appearance(quads, rect, color);
            self.set_annotation_appearance_with_extgstate(annot_id, content, rect)?;
        } else {
            let height_fraction = if kind == AnnotationKind::Underline { 0.08 } else { 0.5 };
            let content = line_markup_appearance(quads, rect, color, height_fraction);
            self.set_annotation_appearance(annot_id, content, rect)?;
        }
        self.add_annot_to_page(page_id, annot_id)?;
        Ok(annot_id)
    }

    // -- Free text -----------------------------------------------------------

    /// Adds a free-text annotation (ISO 32000-1 12.5.6.6): a text box
    /// drawn directly on the page (not attached to any underlying markup
    /// like a comment popup).
    pub fn add_freetext_annotation(&mut self, page_index: usize, rect: Rectangle, text: &str, font_size: f64, color: Color) -> PdfResult<ObjectId> {
        let page_id = self.page_id_at(page_index)?;
        let annot_id = self.allocate_id();

        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("FreeText")));
        dict.set("Rect", Object::Array(rect_to_array(rect)));
        dict.set("Contents", Object::String(to_pdf_text_string(text)));
        dict.set("DA", Object::String(crate::object::PdfString::literal(format!("{} {font_size} Tf", color_operator(color, true)))));
        dict.set("F", Object::Integer(4));

        let content = freetext_appearance(text, font_size, color, rect);
        self.set_object(annot_id, Object::Dictionary(dict));
        self.set_annotation_appearance(annot_id, content, rect)?;
        self.add_annot_to_page(page_id, annot_id)?;
        Ok(annot_id)
    }

    // -- Stamp -----------------------------------------------------------------

    /// Adds a (textual, non-icon) stamp annotation (ISO 32000-1 12.5.6.12)
    /// with `label` (e.g. `"APPROVED"`, `"DRAFT"`) drawn diagonally across
    /// `rect` in `color`. Real-world stamps are frequently a bitmap; this
    /// crate synthesizes a vector appearance instead so no image assets
    /// are required.
    pub fn add_stamp_annotation(&mut self, page_index: usize, rect: Rectangle, label: &str, color: Color) -> PdfResult<ObjectId> {
        let page_id = self.page_id_at(page_index)?;
        let annot_id = self.allocate_id();

        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Stamp")));
        dict.set("Rect", Object::Array(rect_to_array(rect)));
        dict.set("Name", Object::Name(PdfName::new_unchecked("Draft")));
        dict.set("Contents", Object::String(to_pdf_text_string(label)));
        dict.set("F", Object::Integer(4));

        let content = stamp_appearance(label, color, rect);
        self.set_object(annot_id, Object::Dictionary(dict));
        self.set_annotation_appearance(annot_id, content, rect)?;
        self.add_annot_to_page(page_id, annot_id)?;
        Ok(annot_id)
    }

    // -- Ink ---------------------------------------------------------------

    /// Adds an ink (freehand drawing) annotation (ISO 32000-1 12.5.6.13):
    /// `strokes` is one polyline per pen stroke, in page user-space
    /// points. `/Rect` is computed automatically as the bounding box of
    /// every point, padded by half the line width.
    pub fn add_ink_annotation(&mut self, page_index: usize, strokes: &[Vec<(f64, f64)>], color: Color, line_width: f64) -> PdfResult<ObjectId> {
        let non_empty: Vec<&Vec<(f64, f64)>> = strokes.iter().filter(|s| !s.is_empty()).collect();
        if non_empty.is_empty() {
            return Err(EditorError::InvalidArgument("ink annotation needs at least one non-empty stroke".to_string()).into());
        }
        let page_id = self.page_id_at(page_index)?;
        let pad = line_width.max(0.5);
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for stroke in &non_empty {
            for &(x, y) in stroke.iter() {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
        let rect = Rectangle::new(min_x - pad, min_y - pad, max_x + pad, max_y + pad);
        let annot_id = self.allocate_id();

        let mut ink_list = PdfArray::new();
        for stroke in &non_empty {
            let mut points = PdfArray::new();
            for &(x, y) in stroke.iter() {
                points.push(Object::Real(x));
                points.push(Object::Real(y));
            }
            ink_list.push(Object::Array(points));
        }

        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Ink")));
        dict.set("Rect", Object::Array(rect_to_array(rect)));
        dict.set("InkList", Object::Array(ink_list));
        dict.set("C", color_to_array(color));
        let mut bs = PdfDictionary::new();
        bs.set("W", Object::Real(line_width));
        dict.set("BS", Object::Dictionary(bs));
        dict.set("F", Object::Integer(4));

        let content = ink_appearance(&non_empty, color, line_width, rect);
        self.set_object(annot_id, Object::Dictionary(dict));
        self.set_annotation_appearance(annot_id, content, rect)?;
        self.add_annot_to_page(page_id, annot_id)?;
        Ok(annot_id)
    }

    // -- Text note ("sticky note") + popup / comment ------------------------

    /// Adds a "sticky note" text annotation (ISO 32000-1 12.5.6.4) at
    /// `at` (its icon is drawn in a small fixed-size box anchored there)
    /// carrying `contents` as its comment text, together with a linked,
    /// closed `/Popup` window (12.5.6.2) a viewer can open to read/edit
    /// the comment. Returns `(text_note_id, popup_id)`.
    pub fn add_comment(&mut self, page_index: usize, at: (f64, f64), contents: &str, author: Option<&str>) -> PdfResult<(ObjectId, ObjectId)> {
        let page_id = self.page_id_at(page_index)?;
        let icon_rect = Rectangle::new(at.0, at.1, at.0 + 20.0, at.1 + 20.0);
        let note_id = self.allocate_id();
        let popup_id = self.allocate_id();

        let popup_rect = Rectangle::new(at.0 + 24.0, at.1, at.0 + 224.0, at.1 + 100.0);
        let mut popup = PdfDictionary::new();
        popup.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        popup.set("Subtype", Object::Name(PdfName::new_unchecked("Popup")));
        popup.set("Rect", Object::Array(rect_to_array(popup_rect)));
        popup.set("Parent", Object::Reference(note_id));
        popup.set("Open", Object::Boolean(false));
        self.set_object(popup_id, Object::Dictionary(popup));

        let mut note = PdfDictionary::new();
        note.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        note.set("Subtype", Object::Name(PdfName::new_unchecked("Text")));
        note.set("Rect", Object::Array(rect_to_array(icon_rect)));
        note.set("Contents", Object::String(to_pdf_text_string(contents)));
        note.set("Name", Object::Name(PdfName::new_unchecked("Comment")));
        note.set("Popup", Object::Reference(popup_id));
        note.set("F", Object::Integer(4));
        if let Some(a) = author {
            note.set("T", Object::String(to_pdf_text_string(a)));
        }
        self.set_object(note_id, Object::Dictionary(note));
        self.set_annotation_appearance(note_id, note_icon_appearance(), icon_rect)?;

        self.add_annot_to_page(page_id, note_id)?;
        self.add_annot_to_page(page_id, popup_id)?;
        Ok((note_id, popup_id))
    }

    /// Attaches a `/Popup` window to an *existing* markup annotation (any
    /// subtype with a `/Contents`, e.g. a highlight) that doesn't already
    /// have one, so a reader can show/edit a comment on it.
    pub fn add_popup(&mut self, page_index: usize, parent_annot_id: ObjectId, rect: Rectangle, open: bool) -> PdfResult<ObjectId> {
        let page_id = self.page_id_at(page_index)?;
        let mut parent = self.get_dictionary(parent_annot_id)?;
        let popup_id = self.allocate_id();

        let mut popup = PdfDictionary::new();
        popup.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        popup.set("Subtype", Object::Name(PdfName::new_unchecked("Popup")));
        popup.set("Rect", Object::Array(rect_to_array(rect)));
        popup.set("Parent", Object::Reference(parent_annot_id));
        popup.set("Open", Object::Boolean(open));
        self.set_object(popup_id, Object::Dictionary(popup));

        parent.set("Popup", Object::Reference(popup_id));
        self.set_object(parent_annot_id, Object::Dictionary(parent));
        self.add_annot_to_page(page_id, popup_id)?;
        Ok(popup_id)
    }

    // -- Edit / delete / list -----------------------------------------------

    /// Updates an annotation's `/Contents` (comment text). For a
    /// `FreeText` annotation this also regenerates its visible appearance,
    /// since the contents *are* what's drawn; for every other kind
    /// `/Contents` is metadata (the comment text shown in a popup) and
    /// does not affect the on-page appearance.
    pub fn edit_annotation_contents(&mut self, annot_id: ObjectId, new_contents: &str) -> PdfResult<()> {
        let mut dict = self.get_dictionary(annot_id)?;
        dict.set("Contents", Object::String(to_pdf_text_string(new_contents)));
        let is_freetext = matches!(dict.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "FreeText");
        let rect = dict.get("Rect").and_then(rect_from_object);
        self.set_object(annot_id, Object::Dictionary(dict.clone()));

        if is_freetext {
            if let Some(rect) = rect {
                let (size, color) = dict
                    .get("DA")
                    .and_then(|o| match o {
                        Object::String(s) => Some(super::util::from_pdf_text_string(s)),
                        _ => None,
                    })
                    .map(|da| super::util::parse_da(&da, 12.0))
                    .unwrap_or((12.0, Color::BLACK));
                let content = freetext_appearance(new_contents, size, color, rect);
                self.set_annotation_appearance(annot_id, content, rect)?;
            }
        }
        Ok(())
    }

    /// Removes an annotation (and, if it is a markup annotation with a
    /// linked `/Popup`, that popup too) from its page's `/Annots` array.
    /// The underlying object(s) become unreachable garbage, collected by
    /// [`EditableDocument::save_full_rewrite`].
    pub fn delete_annotation(&mut self, page_index: usize, annot_id: ObjectId) -> PdfResult<()> {
        let page_id = self.page_id_at(page_index)?;
        let dict = self
            .get_dictionary(annot_id)
            .map_err(|_| EditorError::AnnotationNotFound(annot_id.number, annot_id.generation))?;
        let popup_id = match dict.get("Popup") {
            Some(Object::Reference(id)) => Some(*id),
            _ => None,
        };

        let mut page = self.get_dictionary(page_id)?;
        let Some(Object::Array(annots)) = page.get("Annots") else {
            return Err(EditorError::AnnotationNotFound(annot_id.number, annot_id.generation).into());
        };
        let kept: PdfArray = annots
            .iter()
            .filter(|a| !matches!(a, Object::Reference(id) if *id == annot_id || Some(*id) == popup_id))
            .cloned()
            .collect();
        if kept.len() == annots.len() {
            return Err(EditorError::AnnotationNotFound(annot_id.number, annot_id.generation).into());
        }
        if kept.is_empty() {
            page.remove("Annots");
        } else {
            page.set("Annots", Object::Array(kept));
        }
        self.set_object(page_id, Object::Dictionary(page));
        Ok(())
    }

    /// Lists every annotation on `page_index`.
    pub fn list_annotations(&self, page_index: usize) -> PdfResult<Vec<AnnotationInfo>> {
        let page_id = self.page_id_at(page_index)?;
        let page = self.get_dictionary(page_id)?;
        let Some(Object::Array(annots)) = page.get("Annots") else { return Ok(Vec::new()) };

        let mut out = Vec::new();
        for a in annots.iter().take(MAX_ANNOTS_PER_PAGE) {
            let Object::Reference(id) = a else { continue };
            let Ok(dict) = self.get_dictionary(*id) else { continue };
            let Some(rect) = dict.get("Rect").and_then(rect_from_object) else { continue };
            let subtype = match dict.get("Subtype") {
                Some(Object::Name(n)) => n.as_str().to_string(),
                _ => String::new(),
            };
            out.push(AnnotationInfo {
                id: *id,
                kind: AnnotationKind::from_subtype(&subtype),
                rect,
                contents: dict.get("Contents").and_then(|o| match o {
                    Object::String(s) => Some(super::util::from_pdf_text_string(s)),
                    _ => None,
                }),
                author: dict.get("T").and_then(|o| match o {
                    Object::String(s) => Some(super::util::from_pdf_text_string(s)),
                    _ => None,
                }),
            });
        }
        Ok(out)
    }

    // -- Shared plumbing -----------------------------------------------------

    fn set_annotation_appearance(&mut self, annot_id: ObjectId, content: String, rect: Rectangle) -> PdfResult<()> {
        // BBox is expressed in the appearance stream's own coordinate
        // space; using the annotation's own width/height with content
        // drawn relative to (0,0) (every appearance builder below already
        // translates by -rect.llx/-rect.lly) keeps this consistent with
        // how the widget appearances in `editor::forms` are built.
        let stream = appearance_xobject(&content, rect.width(), rect.height());
        self.write_annotation_appearance(annot_id, stream)
    }

    /// Like [`EditableDocument::set_annotation_appearance`], but declares
    /// an `/ExtGState /GS1` resource (`ca 0.4`, `/BM /Multiply`) that
    /// [`highlight_appearance`] relies on for its translucent wash.
    fn set_annotation_appearance_with_extgstate(&mut self, annot_id: ObjectId, content: String, rect: Rectangle) -> PdfResult<()> {
        let mut extgstate_dict = PdfDictionary::new();
        let mut gs1 = PdfDictionary::new();
        gs1.set("Type", Object::Name(PdfName::new_unchecked("ExtGState")));
        gs1.set("ca", Object::Real(0.4));
        gs1.set("BM", Object::Name(PdfName::new_unchecked("Multiply")));
        extgstate_dict.set("GS1", Object::Dictionary(gs1));
        let mut extra = PdfDictionary::new();
        extra.set("ExtGState", Object::Dictionary(extgstate_dict));

        let stream = appearance_xobject_with_extra_resources(&content, rect.width(), rect.height(), extra);
        self.write_annotation_appearance(annot_id, stream)
    }

    fn write_annotation_appearance(&mut self, annot_id: ObjectId, stream: crate::object::PdfStream) -> PdfResult<()> {
        let ap_id = self.allocate_id();
        self.set_object(ap_id, Object::Stream(stream));
        let mut dict = self.get_dictionary(annot_id)?;
        let mut ap = PdfDictionary::new();
        ap.set("N", Object::Reference(ap_id));
        dict.set("AP", Object::Dictionary(ap));
        self.set_object(annot_id, Object::Dictionary(dict));
        Ok(())
    }

}

// -- Geometry / QuadPoints -------------------------------------------------

fn bounding_rect(quads: &[(f64, f64, f64, f64)]) -> Rectangle {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for &(llx, lly, urx, ury) in quads {
        min_x = min_x.min(llx);
        min_y = min_y.min(lly);
        max_x = max_x.max(urx);
        max_y = max_y.max(ury);
    }
    Rectangle::new(min_x, min_y, max_x, max_y)
}

/// Builds `/QuadPoints` (ISO 32000-1 Table 179) for a set of rectangles.
///
/// The spec's own prose describing point order is famously inconsistent
/// with how every major PDF producer/consumer (including Adobe's own)
/// actually writes/reads it; this follows the de facto convention: per
/// quad, `(x1,y1)`=top-left, `(x2,y2)`=top-right, `(x3,y3)`=bottom-left,
/// `(x4,y4)`=bottom-right.
fn quad_points_array(quads: &[(f64, f64, f64, f64)]) -> PdfArray {
    let mut arr = PdfArray::new();
    for &(llx, lly, urx, ury) in quads {
        arr.push(Object::Real(llx));
        arr.push(Object::Real(ury));
        arr.push(Object::Real(urx));
        arr.push(Object::Real(ury));
        arr.push(Object::Real(llx));
        arr.push(Object::Real(lly));
        arr.push(Object::Real(urx));
        arr.push(Object::Real(lly));
    }
    arr
}

fn color_to_array(color: Color) -> Object {
    let mut arr = PdfArray::new();
    match color {
        Color::Gray(g) => arr.push(Object::Real(g.level)),
        Color::Rgb(c) => {
            arr.push(Object::Real(c.r));
            arr.push(Object::Real(c.g));
            arr.push(Object::Real(c.b));
        }
        Color::Cmyk(c) => {
            arr.push(Object::Real(c.c));
            arr.push(Object::Real(c.m));
            arr.push(Object::Real(c.y));
            arr.push(Object::Real(c.k));
        }
    }
    Object::Array(arr)
}

fn color_operator(color: Color, fill: bool) -> String {
    match color {
        Color::Gray(g) => format!("{} {}", g.level, if fill { "g" } else { "G" }),
        Color::Rgb(c) => format!("{} {} {} {}", c.r, c.g, c.b, if fill { "rg" } else { "RG" }),
        Color::Cmyk(c) => format!("{} {} {} {} {}", c.c, c.m, c.y, c.k, if fill { "k" } else { "K" }),
    }
}

// -- Appearance stream generators ------------------------------------------
//
// Every appearance stream below is drawn in its own local coordinate
// space with (0, 0) at the annotation Rect's lower-left corner (matching
// the `/BBox [0 0 width height]` that `util::appearance_xobject` always
// emits), so quad/point coordinates (which are in page space) are
// translated by `-rect.llx, -rect.lly` first.

/// Builds a highlight's translucent-wash appearance. Relies on the caller
/// ([`EditableDocument::set_annotation_appearance_with_extgstate`])
/// declaring the `/GS1` `/ExtGState` resource this content references.
fn highlight_appearance(quads: &[(f64, f64, f64, f64)], rect: Rectangle, color: Color) -> String {
    let mut s = String::from("q\n/GS1 gs\n");
    s.push_str(&color_operator(color, true));
    s.push('\n');
    for &(llx, lly, urx, ury) in quads {
        s.push_str(&format!("{} {} {} {} re f\n", llx - rect.llx, lly - rect.lly, urx - llx, ury - lly));
    }
    s.push_str("Q\n");
    s
}

fn line_markup_appearance(quads: &[(f64, f64, f64, f64)], rect: Rectangle, color: Color, height_fraction: f64) -> String {
    let mut s = String::from("q\n");
    s.push_str(&color_operator(color, false));
    s.push('\n');
    for &(llx, lly, urx, ury) in quads {
        let h = ury - lly;
        let width = (h * 0.08).max(0.5);
        let y = lly + h * height_fraction;
        s.push_str(&format!("{width} w\n"));
        s.push_str(&format!("{} {} m\n", llx - rect.llx, y - rect.lly));
        s.push_str(&format!("{} {} l\n", urx - rect.llx, y - rect.lly));
        s.push_str("S\n");
    }
    s.push_str("Q\n");
    s
}

fn freetext_appearance(text: &str, font_size: f64, color: Color, rect: Rectangle) -> String {
    let mut s = String::from("q\n");
    s.push_str(&color_operator(Color::BLACK, false));
    s.push_str("\n1 w\n");
    s.push_str(&format!("0.5 0.5 {} {} re S\n", (rect.width() - 1.0).max(0.0), (rect.height() - 1.0).max(0.0)));
    s.push_str("BT\n");
    s.push_str(&color_operator(color, true));
    s.push('\n');
    s.push_str(&format!("/Helv {font_size} Tf\n"));
    s.push_str(&format!("{} TL\n", font_size * 1.2));
    let padding = 3.0;
    s.push_str(&format!("{padding} {} Td\n", rect.height() - font_size - padding));
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            s.push_str("T*\n");
        }
        s.push_str(&format!("({}) Tj\n", escape_pdf_literal(line)));
    }
    s.push_str("ET\nQ\n");
    s
}

fn stamp_appearance(label: &str, color: Color, rect: Rectangle) -> String {
    let mut s = String::from("q\n");
    s.push_str(&color_operator(color, false));
    s.push_str("\n2 w\n");
    let inset = 3.0;
    s.push_str(&format!("{inset} {inset} {} {} re S\n", (rect.width() - 2.0 * inset).max(0.0), (rect.height() - 2.0 * inset).max(0.0)));
    s.push_str("BT\n");
    s.push_str(&color_operator(color, true));
    s.push('\n');
    let font_size = (rect.height() * 0.5).clamp(6.0, 24.0);
    s.push_str(&format!("/Helv {font_size} Tf\n"));
    let text_width_estimate = label.len() as f64 * font_size * 0.55;
    let tx = ((rect.width() - text_width_estimate) / 2.0).max(inset);
    let ty = (rect.height() - font_size) / 2.0;
    s.push_str(&format!("{tx} {ty} Td\n"));
    s.push_str(&format!("({}) Tj\n", escape_pdf_literal(label)));
    s.push_str("ET\nQ\n");
    s
}

fn ink_appearance(strokes: &[&Vec<(f64, f64)>], color: Color, line_width: f64, rect: Rectangle) -> String {
    let mut s = String::from("q\n");
    s.push_str(&color_operator(color, false));
    s.push('\n');
    s.push_str(&format!("{line_width} w\n1 J\n1 j\n"));
    for stroke in strokes {
        let mut points = stroke.iter();
        if let Some(&(x0, y0)) = points.next() {
            s.push_str(&format!("{} {} m\n", x0 - rect.llx, y0 - rect.lly));
            for &(x, y) in points {
                s.push_str(&format!("{} {} l\n", x - rect.llx, y - rect.lly));
            }
            s.push_str("S\n");
        }
    }
    s.push_str("Q\n");
    s
}

fn note_icon_appearance() -> String {
    // A small, simple "sticky note" glyph: a filled square with folded
    // top-right corner and three horizontal "text" lines - legible at
    // the default 20x20 icon size without needing any external image.
    let mut s = String::from("q\n1 0.92 0.4 rg\n0 0 0 RG\n0.75 w\n");
    s.push_str("2 2 16 14 re B\n");
    s.push_str("0 0 0 RG\n0.5 w\n");
    s.push_str("4 12 m 14 12 l S\n4 9 m 14 9 l S\n4 6 m 11 6 l S\n");
    s.push_str("Q\n");
    s
}

fn escape_pdf_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    fn doc_with_one_page() -> EditableDocument {
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Hello annotated World"))
            .build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        EditableDocument::from_bytes(bytes).unwrap()
    }

    #[test]
    fn test_add_highlight_annotation() {
        let mut doc = doc_with_one_page();
        let id = doc.add_highlight_annotation(0, &[(72.0, 745.0, 200.0, 760.0)], Color::rgb(1.0, 1.0, 0.0)).unwrap();
        let annots = doc.list_annotations(0).unwrap();
        assert_eq!(annots.len(), 1);
        assert_eq!(annots[0].id, id);
        assert_eq!(annots[0].kind, AnnotationKind::Highlight);

        let dict = doc.get_dictionary(id).unwrap();
        assert!(matches!(dict.get("QuadPoints"), Some(Object::Array(a)) if a.len() == 8));
        assert!(dict.get("AP").is_some());
    }

    #[test]
    fn test_add_underline_and_strikeout() {
        let mut doc = doc_with_one_page();
        doc.add_underline_annotation(0, &[(72.0, 745.0, 200.0, 760.0)], Color::BLACK).unwrap();
        doc.add_strikeout_annotation(0, &[(72.0, 745.0, 200.0, 760.0)], Color::RED).unwrap();
        let annots = doc.list_annotations(0).unwrap();
        assert_eq!(annots.len(), 2);
        assert_eq!(annots[0].kind, AnnotationKind::Underline);
        assert_eq!(annots[1].kind, AnnotationKind::StrikeOut);
    }

    #[test]
    fn test_text_markup_rejects_empty_quads() {
        let mut doc = doc_with_one_page();
        assert!(doc.add_highlight_annotation(0, &[], Color::BLACK).is_err());
    }

    #[test]
    fn test_add_freetext_annotation_and_edit_contents() {
        let mut doc = doc_with_one_page();
        let id = doc.add_freetext_annotation(0, Rectangle::new(100.0, 100.0, 300.0, 150.0), "Original note", 12.0, Color::BLACK).unwrap();
        let annots = doc.list_annotations(0).unwrap();
        assert_eq!(annots[0].contents.as_deref(), Some("Original note"));

        doc.edit_annotation_contents(id, "Edited note").unwrap();
        let annots = doc.list_annotations(0).unwrap();
        assert_eq!(annots[0].contents.as_deref(), Some("Edited note"));

        let dict = doc.get_dictionary(id).unwrap();
        let Some(Object::Dictionary(ap)) = dict.get("AP") else { panic!("expected AP") };
        let Some(Object::Reference(ap_id)) = ap.get("N") else { panic!("expected AP/N ref") };
        let Some(Object::Stream(stream)) = doc.get_object(*ap_id) else { panic!("expected stream") };
        let content = String::from_utf8_lossy(&stream.data);
        assert!(content.contains("Edited note"), "appearance was: {content}");
    }

    #[test]
    fn test_add_stamp_annotation() {
        let mut doc = doc_with_one_page();
        let id = doc.add_stamp_annotation(0, Rectangle::new(400.0, 700.0, 550.0, 750.0), "APPROVED", Color::RED).unwrap();
        let dict = doc.get_dictionary(id).unwrap();
        assert!(matches!(dict.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Stamp"));
        assert!(dict.get("AP").is_some());
    }

    #[test]
    fn test_add_ink_annotation_computes_bbox() {
        let mut doc = doc_with_one_page();
        let strokes = vec![vec![(100.0, 100.0), (120.0, 140.0), (150.0, 110.0)]];
        let id = doc.add_ink_annotation(0, &strokes, Color::BLUE, 2.0).unwrap();
        let dict = doc.get_dictionary(id).unwrap();
        let rect = dict.get("Rect").and_then(rect_from_object).unwrap();
        assert!(rect.llx <= 100.0 && rect.urx >= 150.0);
        assert!(rect.lly <= 100.0 && rect.ury >= 140.0);
        assert!(matches!(dict.get("InkList"), Some(Object::Array(a)) if a.len() == 1));
    }

    #[test]
    fn test_ink_annotation_rejects_all_empty_strokes() {
        let mut doc = doc_with_one_page();
        assert!(doc.add_ink_annotation(0, &[vec![], vec![]], Color::BLACK, 1.0).is_err());
    }

    #[test]
    fn test_add_comment_creates_linked_popup() {
        let mut doc = doc_with_one_page();
        let (note_id, popup_id) = doc.add_comment(0, (300.0, 400.0), "Please review this paragraph.", Some("Reviewer")).unwrap();

        let annots = doc.list_annotations(0).unwrap();
        assert_eq!(annots.len(), 2);

        let note = doc.get_dictionary(note_id).unwrap();
        assert_eq!(note.get("Popup"), Some(&Object::Reference(popup_id)));
        let popup = doc.get_dictionary(popup_id).unwrap();
        assert_eq!(popup.get("Parent"), Some(&Object::Reference(note_id)));
    }

    #[test]
    fn test_add_popup_for_existing_markup_annotation() {
        let mut doc = doc_with_one_page();
        let highlight_id = doc.add_highlight_annotation(0, &[(72.0, 745.0, 200.0, 760.0)], Color::rgb(1.0, 1.0, 0.0)).unwrap();
        let popup_id = doc.add_popup(0, highlight_id, Rectangle::new(210.0, 745.0, 400.0, 810.0), true).unwrap();

        let parent = doc.get_dictionary(highlight_id).unwrap();
        assert_eq!(parent.get("Popup"), Some(&Object::Reference(popup_id)));
    }

    #[test]
    fn test_delete_annotation_removes_it_and_its_popup() {
        let mut doc = doc_with_one_page();
        let (note_id, _popup_id) = doc.add_comment(0, (300.0, 400.0), "Delete me", None).unwrap();
        assert_eq!(doc.list_annotations(0).unwrap().len(), 2);

        doc.delete_annotation(0, note_id).unwrap();
        assert!(doc.list_annotations(0).unwrap().is_empty());
    }

    #[test]
    fn test_delete_nonexistent_annotation_errors() {
        let mut doc = doc_with_one_page();
        let bogus = ObjectId::new(9999);
        assert!(doc.delete_annotation(0, bogus).is_err());
    }

    #[test]
    fn test_annotation_roundtrip_through_incremental_save() {
        let mut doc = doc_with_one_page();
        doc.add_highlight_annotation(0, &[(72.0, 745.0, 200.0, 760.0)], Color::rgb(1.0, 1.0, 0.0)).unwrap();
        doc.add_ink_annotation(0, &[vec![(50.0, 50.0), (80.0, 90.0)]], Color::BLUE, 1.5).unwrap();
        doc.add_comment(0, (300.0, 400.0), "A comment", Some("Alice")).unwrap();

        let saved = doc.save_incremental_to_bytes().unwrap();
        let lopdf_doc = lopdf::Document::load_mem(&saved).expect("lopdf must open the annotated document");
        assert!(!lopdf_doc.get_pages().is_empty());

        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let annots = reopened.list_annotations(0).unwrap();
        assert_eq!(annots.len(), 4); // highlight + ink + text-note + popup
    }
}
