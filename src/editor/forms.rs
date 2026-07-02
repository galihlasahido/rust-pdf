//! Reading, filling, creating and flattening AcroForm interactive form
//! fields (ISO 32000-1:2008 Section 12.7 "Interactive Forms") on an
//! already-loaded [`EditableDocument`].
//!
//! # Field model
//!
//! An AcroForm field is a (possibly multi-level) tree: the document
//! catalog's `/AcroForm /Fields` array holds the root fields; a
//! non-terminal field's `/Kids` are further field dictionaries, while a
//! *terminal* field's `/Kids` (if any) are `Widget` annotations - the
//! on-page appearances of that one field (ISO 32000-1 12.7.3.1, 12.7.3.2).
//! A field with no `/Kids` at all is a "merged" field/widget: the same
//! dictionary is simultaneously the field and its one on-page annotation.
//! Fields are addressed by their *fully qualified name*: the `/T` (partial
//! name) of every ancestor, joined with `.` (12.7.3.2).
//!
//! # Appearance regeneration
//!
//! Every setter here regenerates the affected widget's `/AP /N` appearance
//! stream (12.5.5, 12.7.3.3) using [`crate::forms::AppearanceBuilder`] with
//! a synthetic, self-contained `/Helv` (Helvetica) resource - see
//! [`super::util::appearance_xobject`]. This means a filled-in field
//! always renders correctly (including in readers that, unlike Acrobat,
//! do not honor `/NeedAppearances` and regenerate appearances themselves),
//! but a field whose original producer used a custom embedded font via
//! `/DA` will visually switch to Helvetica after being filled through this
//! API. Reproducing arbitrary embedded-font text layout for form
//! appearance generation is a much larger feature (effectively the same
//! text-shaping problem noted in the [`crate::editor`] module docs for
//! `replace_page_text`) and is out of scope here.

use super::graph::EditableDocument;
use super::util::{appearance_xobject, parse_da, rect_from_object, rect_to_array, to_pdf_text_string, unique_resource_name};
use crate::color::Color;
use crate::error::{EditorError, PdfResult};
use crate::forms::{AppearanceBuilder, BorderStyle};
use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfString};
use crate::types::{ObjectId, Rectangle};

/// Maximum number of field-tree nodes visited while resolving a field name
/// or listing all fields. Bounds work against a corrupt/adversarial
/// `/Kids` structure; real forms (thousands of fields) are nowhere near
/// this.
const MAX_FIELD_NODES: usize = 200_000;

/// Maximum `/Parent` chain length followed while computing a field's fully
/// qualified name or an inherited attribute (ISO 32000-1 12.7.3.2's
/// inheritance chain has no defined maximum, but real forms never nest
/// more than a handful of levels deep).
const MAX_PARENT_DEPTH: u32 = 64;

impl EditableDocument {
    // -- Discovery ---------------------------------------------------

    /// Returns the fully qualified names of every terminal (fillable)
    /// AcroForm field in the document, in document (Fields-array/Kids)
    /// order.
    pub fn field_names(&self) -> PdfResult<Vec<String>> {
        Ok(self.walk_fields()?.into_iter().map(|(name, _)| name).collect())
    }

    /// Returns the PDF field-type name (`Tx`, `Btn`, `Ch`, or `Sig`; ISO
    /// 32000-1 Table 220) of the named field, inheriting `/FT` from an
    /// ancestor if the field itself doesn't set it (12.7.3.2).
    pub fn field_type(&self, name: &str) -> PdfResult<Option<String>> {
        let Some(id) = self.find_field(name)? else { return Ok(None) };
        Ok(self.effective(id, "FT").and_then(|o| match o {
            Object::Name(n) => Some(n.as_str().to_string()),
            _ => None,
        }))
    }

    // -- Text fields ---------------------------------------------------

    /// Returns a text field's current value (`/V`, ISO 32000-1 12.7.4.3),
    /// decoded as a PDF text string.
    pub fn get_text_value(&self, name: &str) -> PdfResult<Option<String>> {
        let id = self.require_field(name)?;
        self.require_ft(id, name, "Tx", "text")?;
        Ok(self.get_dictionary(id)?.get("V").and_then(text_of))
    }

    /// Sets a text field's value and regenerates its appearance stream(s).
    pub fn set_text_value(&mut self, name: &str, value: &str) -> PdfResult<()> {
        let id = self.require_field(name)?;
        self.require_ft(id, name, "Tx", "text")?;

        let mut dict = self.get_dictionary(id)?;
        dict.set("V", Object::String(to_pdf_text_string(value)));
        let da = effective_da_string(self, id);
        self.set_object(id, Object::Dictionary(dict));

        for widget_id in self.widget_ids_of(id)? {
            self.regenerate_text_appearance(widget_id, value, &da)?;
        }
        Ok(())
    }

    fn regenerate_text_appearance(&mut self, widget_id: ObjectId, value: &str, da: &str) -> PdfResult<()> {
        let widget = self.get_dictionary(widget_id)?;
        let Some(rect) = widget.get("Rect").and_then(rect_from_object) else {
            return Ok(()); // No Rect: nothing to draw an appearance for.
        };
        let (size, color) = parse_da(da, 12.0);
        let (bg, border, style, width) = mk_colors(&widget);

        let builder = AppearanceBuilder::new(rect.with_origin())
            .background_color(bg)
            .border_color(border)
            .border_style(style)
            .border_width(width);
        let content = builder.build_text_appearance(Some(value), "Helv", size, color);
        self.set_widget_normal_appearance(widget_id, content, rect.width(), rect.height())
    }

    // -- Checkboxes ------------------------------------------------------

    /// Returns whether a checkbox field is checked (its `/V` is anything
    /// other than the `/Off` name, ISO 32000-1 12.7.4.2.3).
    pub fn get_checkbox_checked(&self, name: &str) -> PdfResult<bool> {
        let id = self.require_field(name)?;
        self.require_ft(id, name, "Btn", "checkbox")?;
        Ok(is_on(self.get_dictionary(id)?.get("V")))
    }

    /// Sets a checkbox field's checked state, updating `/V` and every
    /// widget's `/AS` (appearance state) to match. The "on" state name is
    /// taken from whichever key (other than `/Off`) the widget's existing
    /// `/AP /N` sub-dictionary already uses, defaulting to `/Yes` if the
    /// widget has no appearance dictionary yet.
    pub fn set_checkbox_checked(&mut self, name: &str, checked: bool) -> PdfResult<()> {
        let id = self.require_field(name)?;
        self.require_ft(id, name, "Btn", "checkbox")?;
        let widget_ids = self.widget_ids_of(id)?;

        let on_state = widget_ids
            .first()
            .and_then(|&w| self.on_state_name(w))
            .unwrap_or_else(|| "Yes".to_string());
        let state_name = if checked { on_state.as_str() } else { "Off" };

        let mut dict = self.get_dictionary(id)?;
        dict.set("V", Object::Name(PdfName::new_unchecked(state_name)));
        self.set_object(id, Object::Dictionary(dict));

        for widget_id in widget_ids {
            let mut w = self.get_dictionary(widget_id)?;
            w.set("AS", Object::Name(PdfName::new_unchecked(state_name)));
            self.set_object(widget_id, Object::Dictionary(w));
        }
        Ok(())
    }

    /// Returns the appearance-state name (other than `/Off`) declared in a
    /// widget's `/AP /N` sub-dictionary, if any.
    fn on_state_name(&self, widget_id: ObjectId) -> Option<String> {
        let widget = self.get_dictionary(widget_id).ok()?;
        let Object::Dictionary(ap) = widget.get("AP")? else { return None };
        let Object::Dictionary(n) = ap.get("N")? else { return None };
        let result = n.iter().map(|(k, _)| k.clone()).find(|k| k != "Off");
        result
    }

    // -- Radio button groups ----------------------------------------------

    /// Returns a radio group's currently selected export value, or `None`
    /// if nothing is selected (`/V` is `/Off` or absent).
    pub fn get_radio_value(&self, name: &str) -> PdfResult<Option<String>> {
        let id = self.require_field(name)?;
        self.require_ft(id, name, "Btn", "radio")?;
        match self.get_dictionary(id)?.get("V") {
            Some(Object::Name(n)) if n.as_str() != "Off" => Ok(Some(n.as_str().to_string())),
            _ => Ok(None),
        }
    }

    /// Selects the radio button in `name`'s group whose widget declares
    /// `export_value` as an `/AP /N` state, setting the parent's `/V` and
    /// every kid widget's `/AS` accordingly (ISO 32000-1 12.7.4.2.3).
    /// Every other button in the group is set to `/Off`.
    pub fn set_radio_value(&mut self, name: &str, export_value: &str) -> PdfResult<()> {
        let id = self.require_field(name)?;
        self.require_ft(id, name, "Btn", "radio")?;

        let mut dict = self.get_dictionary(id)?;
        dict.set("V", Object::Name(PdfName::new_unchecked(export_value)));
        self.set_object(id, Object::Dictionary(dict));

        for widget_id in self.widget_ids_of(id)? {
            let has_state = self.on_state_name(widget_id).as_deref() == Some(export_value);
            let mut w = self.get_dictionary(widget_id)?;
            let as_name = if has_state { export_value } else { "Off" };
            w.set("AS", Object::Name(PdfName::new_unchecked(as_name)));
            self.set_object(widget_id, Object::Dictionary(w));
        }
        Ok(())
    }

    // -- Choice fields (combo / list boxes) -------------------------------

    /// Returns a choice field's current value (`/V`) as a single string.
    /// For a multi-select list box with several selected entries, only
    /// the first is returned; use [`EditableDocument::get_choice_values`]
    /// for the full selection.
    pub fn get_choice_value(&self, name: &str) -> PdfResult<Option<String>> {
        Ok(self.get_choice_values(name)?.into_iter().next())
    }

    /// Returns every currently selected value of a choice field (`/V`,
    /// which ISO 32000-1 12.7.4.4 allows to be either a single text
    /// string or an array of them for a multi-select list box).
    pub fn get_choice_values(&self, name: &str) -> PdfResult<Vec<String>> {
        let id = self.require_field(name)?;
        self.require_ft(id, name, "Ch", "choice")?;
        Ok(match self.get_dictionary(id)?.get("V") {
            Some(Object::String(s)) => vec![super::util::from_pdf_text_string(s)],
            Some(Object::Array(a)) => a.iter().filter_map(text_of).collect(),
            _ => Vec::new(),
        })
    }

    /// Returns a choice field's `/Opt` option list (12.7.4.4), as
    /// display strings.
    pub fn choice_options(&self, name: &str) -> PdfResult<Vec<String>> {
        let id = self.require_field(name)?;
        self.require_ft(id, name, "Ch", "choice")?;
        Ok(match self.get_dictionary(id)?.get("Opt") {
            Some(Object::Array(a)) => a
                .iter()
                .filter_map(|o| match o {
                    Object::String(s) => Some(super::util::from_pdf_text_string(s)),
                    // Each entry may itself be a 2-element [export, display] array.
                    Object::Array(pair) => pair.get(1).or_else(|| pair.get(0)).and_then(text_of),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        })
    }

    /// Sets a (single-select) choice field's value, which must match one
    /// of its `/Opt` entries, and regenerates its appearance.
    pub fn set_choice_value(&mut self, name: &str, value: &str) -> PdfResult<()> {
        let id = self.require_field(name)?;
        self.require_ft(id, name, "Ch", "choice")?;
        let options = self.choice_options(name)?;
        let index = options.iter().position(|o| o == value);

        let mut dict = self.get_dictionary(id)?;
        dict.set("V", Object::String(to_pdf_text_string(value)));
        if let Some(idx) = index {
            let mut arr = PdfArray::new();
            arr.push(Object::Integer(idx as i64));
            dict.set("I", Object::Array(arr));
        }
        let is_combo = matches!(dict.get("Ff"), Some(Object::Integer(f)) if f & (1 << 17) != 0);
        let da = effective_da_string(self, id);
        self.set_object(id, Object::Dictionary(dict));

        let selected = vec![index.unwrap_or(0)];
        for widget_id in self.widget_ids_of(id)? {
            self.regenerate_choice_appearance(widget_id, &options, &selected, value, is_combo, &da)?;
        }
        Ok(())
    }

    fn regenerate_choice_appearance(
        &mut self,
        widget_id: ObjectId,
        options: &[String],
        selected: &[usize],
        selected_text: &str,
        is_combo: bool,
        da: &str,
    ) -> PdfResult<()> {
        let widget = self.get_dictionary(widget_id)?;
        let Some(rect) = widget.get("Rect").and_then(rect_from_object) else { return Ok(()) };
        let (size, color) = parse_da(da, 12.0);
        let (bg, border, style, width) = mk_colors(&widget);
        let builder = AppearanceBuilder::new(rect.with_origin())
            .background_color(bg)
            .border_color(border)
            .border_style(style)
            .border_width(width);
        let content = if is_combo {
            builder.build_combobox_appearance(Some(selected_text), "Helv", size, color)
        } else {
            builder.build_listbox_appearance(options, selected, "Helv", size, color)
        };
        self.set_widget_normal_appearance(widget_id, content, rect.width(), rect.height())
    }

    // -- Form flattening (12.7.2) -----------------------------------------

    /// "Flattens" every AcroForm field into ordinary, non-interactive page
    /// content: for every widget, its current `/AP /N` appearance (the
    /// state selected by `/AS` for a checkbox/radio button, or the sole
    /// stream for any other field type) is painted onto the widget's page
    /// at its `/Rect`, mapped through the appearance-stream algorithm of
    /// ISO 32000-1 12.5.5 ("Appearance Streams"): the appearance's `BBox`
    /// is transformed by its own `/Matrix`, the smallest upright rectangle
    /// enclosing the four transformed corners is computed, and a matrix
    /// `A` mapping that rectangle onto `/Rect` is derived and prepended.
    /// After every widget has been baked in, all widget annotations are
    /// removed from their pages and the `/AcroForm` entry is removed from
    /// the catalog, leaving a document with no interactive fields left but
    /// visually identical to the filled-in form.
    pub fn flatten_form(&mut self) -> PdfResult<()> {
        let fields = self.walk_fields()?;
        for (_, field_id) in &fields {
            for widget_id in self.widget_ids_of(*field_id)? {
                self.flatten_widget(widget_id)?;
            }
        }

        // Detach every flattened widget from its page's /Annots and drop
        // the AcroForm entirely; there is nothing interactive left.
        let widget_ids: std::collections::HashSet<ObjectId> = fields
            .iter()
            .flat_map(|(_, id)| self.widget_ids_of(*id).unwrap_or_default())
            .collect();
        for page_id in self.page_ids()? {
            let mut page = self.get_dictionary(page_id)?;
            if let Some(Object::Array(annots)) = page.get("Annots") {
                let kept: PdfArray = annots
                    .iter()
                    .filter(|a| !matches!(a, Object::Reference(id) if widget_ids.contains(id)))
                    .cloned()
                    .collect();
                if kept.is_empty() {
                    page.remove("Annots");
                } else {
                    page.set("Annots", Object::Array(kept));
                }
                self.set_object(page_id, Object::Dictionary(page));
            }
        }

        let mut catalog = self.catalog()?;
        if catalog.remove("AcroForm").is_some() {
            let root = self.catalog_id();
            self.set_object(root, Object::Dictionary(catalog));
        }
        Ok(())
    }

    fn flatten_widget(&mut self, widget_id: ObjectId) -> PdfResult<()> {
        let widget = self.get_dictionary(widget_id)?;
        let Some(rect) = widget.get("Rect").and_then(rect_from_object) else { return Ok(()) };
        let Some(Object::Reference(page_id)) = widget.get("P").cloned() else {
            // No /P back-reference: fall back to searching every page's
            // /Annots for this widget rather than silently skipping it.
            let Some(page_id) = self.find_annotation_page(widget_id)? else { return Ok(()) };
            return self.flatten_widget_onto(widget_id, page_id, rect);
        };
        self.flatten_widget_onto(widget_id, page_id, rect)
    }

    fn flatten_widget_onto(&mut self, widget_id: ObjectId, page_id: ObjectId, rect: Rectangle) -> PdfResult<()> {
        let widget = self.get_dictionary(widget_id)?;
        let Some(ap_id) = self.resolve_normal_appearance_stream(&widget) else { return Ok(()) };
        let Some(Object::Stream(stream)) = self.get_object(ap_id) else { return Ok(()) };

        let bbox = stream
            .dictionary
            .get("BBox")
            .and_then(rect_from_object)
            .unwrap_or_else(|| Rectangle::new(0.0, 0.0, rect.width(), rect.height()));
        let form_matrix = matrix_from_object(stream.dictionary.get("Matrix"));

        // ISO 32000-1 12.5.5 step (a): transform BBox's 4 corners by
        // /Matrix and take the bounding box of the result.
        let corners = [
            form_matrix.transform_point(bbox.llx, bbox.lly),
            form_matrix.transform_point(bbox.urx, bbox.lly),
            form_matrix.transform_point(bbox.urx, bbox.ury),
            form_matrix.transform_point(bbox.llx, bbox.ury),
        ];
        let tx_min = corners.iter().map(|(x, _)| *x).fold(f64::INFINITY, f64::min);
        let tx_max = corners.iter().map(|(x, _)| *x).fold(f64::NEG_INFINITY, f64::max);
        let ty_min = corners.iter().map(|(_, y)| *y).fold(f64::INFINITY, f64::min);
        let ty_max = corners.iter().map(|(_, y)| *y).fold(f64::NEG_INFINITY, f64::max);
        let (tw, th) = (tx_max - tx_min, ty_max - ty_min);

        // Step (b): matrix A mapping the transformed box onto /Rect.
        let sx = if tw.abs() > 1e-9 { rect.width() / tw } else { 1.0 };
        let sy = if th.abs() > 1e-9 { rect.height() / th } else { 1.0 };
        let a_matrix = crate::types::Matrix::new(sx, 0.0, 0.0, sy, rect.llx - tx_min * sx, rect.lly - ty_min * sy);
        let paint_matrix = form_matrix.multiply(&a_matrix);

        // Add the appearance stream as an XObject resource on the page
        // and paint it with the derived matrix.
        let mut page = self.get_dictionary(page_id)?;
        let mut resources = match page.get("Resources") {
            Some(Object::Dictionary(d)) => d.clone(),
            Some(Object::Reference(id)) => self.get_dictionary(*id)?,
            _ => PdfDictionary::new(),
        };
        let mut xobjects = match resources.get("XObject") {
            Some(Object::Dictionary(d)) => d.clone(),
            _ => PdfDictionary::new(),
        };
        let name = unique_resource_name(&xobjects, "FlatForm");
        xobjects.set(name.clone(), Object::Reference(ap_id));
        resources.set("XObject", Object::Dictionary(xobjects));
        page.set("Resources", Object::Dictionary(resources));
        self.set_object(page_id, Object::Dictionary(page));

        let [a, b, c, d, e, f] = paint_matrix.to_array();
        let ops = format!("q\n{a} {b} {c} {d} {e} {f} cm\n/{name} Do\nQ\n");
        let mut bytes = self.page_content_bytes(page_id)?;
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(ops.as_bytes());
        self.set_page_content_bytes(page_id, bytes)
    }

    fn resolve_normal_appearance_stream(&self, widget: &PdfDictionary) -> Option<ObjectId> {
        let Some(Object::Dictionary(ap)) = widget.get("AP") else { return None };
        match ap.get("N") {
            Some(Object::Reference(id)) => Some(*id),
            Some(Object::Dictionary(states)) => {
                let as_name = match widget.get("AS") {
                    Some(Object::Name(n)) => Some(n.as_str()),
                    _ => None,
                };
                let entry = as_name
                    .and_then(|n| states.get(n))
                    .or_else(|| states.iter().next().map(|(_, v)| v));
                match entry {
                    Some(Object::Reference(id)) => Some(*id),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn find_annotation_page(&self, annot_id: ObjectId) -> PdfResult<Option<ObjectId>> {
        for page_id in self.page_ids()? {
            let dict = self.get_dictionary(page_id)?;
            if let Some(Object::Array(annots)) = dict.get("Annots") {
                if annots.iter().any(|a| matches!(a, Object::Reference(id) if *id == annot_id)) {
                    return Ok(Some(page_id));
                }
            }
        }
        Ok(None)
    }

    // -- Field creation ----------------------------------------------------

    /// Adds a new single-line text field to `page_index` and registers it
    /// in the document's `/AcroForm` (creating one if this is the first
    /// field added). Returns the new field's object id.
    pub fn add_text_field(
        &mut self,
        page_index: usize,
        name: &str,
        rect: Rectangle,
        initial_value: Option<&str>,
    ) -> PdfResult<ObjectId> {
        self.reject_duplicate_field_name(name)?;
        let page_id = self.page_id_at(page_index)?;
        let field_id = self.allocate_id();

        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
        dict.set("FT", Object::Name(PdfName::new_unchecked("Tx")));
        dict.set("T", Object::String(to_pdf_text_string(name)));
        dict.set("P", Object::Reference(page_id));
        dict.set("Rect", Object::Array(rect_to_array(rect)));
        dict.set("F", Object::Integer(4));
        dict.set("DA", Object::String(PdfString::literal("/Helv 12 Tf 0 g")));
        if let Some(v) = initial_value {
            dict.set("V", Object::String(to_pdf_text_string(v)));
        }
        self.set_object(field_id, Object::Dictionary(dict));
        self.register_field(field_id, page_id)?;

        let builder = AppearanceBuilder::new(rect.with_origin())
            .background_color(Color::WHITE)
            .border_color(Color::BLACK);
        let content = builder.build_text_appearance(initial_value, "Helv", 12.0, Color::BLACK);
        self.set_widget_normal_appearance(field_id, content, rect.width(), rect.height())?;
        Ok(field_id)
    }

    /// Adds a new checkbox field to `page_index`.
    pub fn add_checkbox_field(&mut self, page_index: usize, name: &str, rect: Rectangle, checked: bool) -> PdfResult<ObjectId> {
        self.reject_duplicate_field_name(name)?;
        let page_id = self.page_id_at(page_index)?;
        let field_id = self.allocate_id();
        let state = if checked { "Yes" } else { "Off" };

        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
        dict.set("FT", Object::Name(PdfName::new_unchecked("Btn")));
        dict.set("T", Object::String(to_pdf_text_string(name)));
        dict.set("P", Object::Reference(page_id));
        dict.set("Rect", Object::Array(rect_to_array(rect)));
        dict.set("F", Object::Integer(4));
        dict.set("V", Object::Name(PdfName::new_unchecked(state)));
        dict.set("AS", Object::Name(PdfName::new_unchecked(state)));
        self.set_object(field_id, Object::Dictionary(dict));
        self.register_field(field_id, page_id)?;

        let builder = AppearanceBuilder::new(rect.with_origin())
            .background_color(Color::WHITE)
            .border_color(Color::BLACK);
        let on_id = self.allocate_id();
        let off_id = self.allocate_id();
        self.set_object(
            on_id,
            Object::Stream(appearance_xobject(&builder.build_checkbox_checked(Color::BLACK), rect.width(), rect.height())),
        );
        self.set_object(
            off_id,
            Object::Stream(appearance_xobject(&builder.build_checkbox_unchecked(), rect.width(), rect.height())),
        );
        let mut n_dict = PdfDictionary::new();
        n_dict.set("Yes", Object::Reference(on_id));
        n_dict.set("Off", Object::Reference(off_id));
        let mut ap = PdfDictionary::new();
        ap.set("N", Object::Dictionary(n_dict));
        let mut dict = self.get_dictionary(field_id)?;
        dict.set("AP", Object::Dictionary(ap));
        self.set_object(field_id, Object::Dictionary(dict));
        Ok(field_id)
    }

    /// Adds a new radio button group to `page_index`, with one widget per
    /// `(export_value, rect)` pair in `options`. `selected` is the index
    /// into `options` that starts selected, if any.
    pub fn add_radio_group_field(
        &mut self,
        page_index: usize,
        name: &str,
        options: &[(String, Rectangle)],
        selected: Option<usize>,
    ) -> PdfResult<ObjectId> {
        self.reject_duplicate_field_name(name)?;
        let page_id = self.page_id_at(page_index)?;
        let group_id = self.allocate_id();

        let mut kids = PdfArray::new();
        let mut kid_ids = Vec::with_capacity(options.len());
        for (i, (export_value, rect)) in options.iter().enumerate() {
            let widget_id = self.allocate_id();
            let is_selected = selected == Some(i);
            let state = if is_selected { export_value.as_str() } else { "Off" };

            let mut widget = PdfDictionary::new();
            widget.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
            widget.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
            widget.set("Parent", Object::Reference(group_id));
            widget.set("P", Object::Reference(page_id));
            widget.set("Rect", Object::Array(rect_to_array(*rect)));
            widget.set("F", Object::Integer(4));
            widget.set("AS", Object::Name(PdfName::new_unchecked(state)));

            let builder = AppearanceBuilder::new(rect.with_origin())
                .background_color(Color::WHITE)
                .border_color(Color::BLACK);
            let on_id = self.allocate_id();
            let off_id = self.allocate_id();
            self.set_object(
                on_id,
                Object::Stream(appearance_xobject(&builder.build_radio_selected(Color::BLACK), rect.width(), rect.height())),
            );
            self.set_object(
                off_id,
                Object::Stream(appearance_xobject(&builder.build_radio_unselected(), rect.width(), rect.height())),
            );
            let mut n_dict = PdfDictionary::new();
            n_dict.set(export_value.as_str(), Object::Reference(on_id));
            n_dict.set("Off", Object::Reference(off_id));
            let mut ap = PdfDictionary::new();
            ap.set("N", Object::Dictionary(n_dict));
            widget.set("AP", Object::Dictionary(ap));

            self.set_object(widget_id, Object::Dictionary(widget));
            kids.push(Object::Reference(widget_id));
            kid_ids.push(widget_id);
        }
        let mut group = PdfDictionary::new();
        group.set("FT", Object::Name(PdfName::new_unchecked("Btn")));
        group.set("T", Object::String(to_pdf_text_string(name)));
        group.set("Ff", Object::Integer((1 << 15) | (1 << 14))); // Radio | NoToggleToOff
        group.set("Kids", Object::Array(kids));
        let v = selected
            .and_then(|i| options.get(i))
            .map(|(v, _)| v.as_str())
            .unwrap_or("Off");
        group.set("V", Object::Name(PdfName::new_unchecked(v)));
        self.set_object(group_id, Object::Dictionary(group));

        self.register_field(group_id, page_id)?;
        for widget_id in kid_ids {
            self.add_annot_to_page(page_id, widget_id)?;
        }
        Ok(group_id)
    }

    /// Adds a new dropdown (combo box) field to `page_index`.
    pub fn add_combobox_field(
        &mut self,
        page_index: usize,
        name: &str,
        rect: Rectangle,
        options: &[String],
        selected_index: Option<usize>,
    ) -> PdfResult<ObjectId> {
        self.reject_duplicate_field_name(name)?;
        let page_id = self.page_id_at(page_index)?;
        let field_id = self.allocate_id();

        let mut opt = PdfArray::new();
        for o in options {
            opt.push(Object::String(to_pdf_text_string(o)));
        }

        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
        dict.set("FT", Object::Name(PdfName::new_unchecked("Ch")));
        dict.set("T", Object::String(to_pdf_text_string(name)));
        dict.set("P", Object::Reference(page_id));
        dict.set("Rect", Object::Array(rect_to_array(rect)));
        dict.set("F", Object::Integer(4));
        dict.set("Ff", Object::Integer(1 << 17)); // Combo
        dict.set("Opt", Object::Array(opt));
        dict.set("DA", Object::String(PdfString::literal("/Helv 12 Tf 0 g")));
        let selected = selected_index.and_then(|i| options.get(i).map(|v| (i, v.clone())));
        let selected_text = selected.as_ref().map(|(_, v)| v.clone());
        if let Some((idx, ref v)) = selected {
            dict.set("V", Object::String(to_pdf_text_string(v)));
            let mut i_arr = PdfArray::new();
            i_arr.push(Object::Integer(idx as i64));
            dict.set("I", Object::Array(i_arr));
        }
        self.set_object(field_id, Object::Dictionary(dict));
        self.register_field(field_id, page_id)?;

        let builder = AppearanceBuilder::new(rect.with_origin())
            .background_color(Color::WHITE)
            .border_color(Color::BLACK);
        let content = builder.build_combobox_appearance(selected_text.as_deref(), "Helv", 12.0, Color::BLACK);
        self.set_widget_normal_appearance(field_id, content, rect.width(), rect.height())?;
        Ok(field_id)
    }

    /// Adds a new, unsigned digital-signature field (`/FT /Sig`, ISO
    /// 32000-1 12.7.4.5) to `page_index`, with a placeholder "not signed"
    /// appearance. This only creates the interactive field; producing an
    /// actual `/V` signature dictionary (byte-range hashing, PKCS#7/CMS
    /// encoding) is handled by [`crate::signatures`], not this module.
    pub fn add_signature_field(&mut self, page_index: usize, name: &str, rect: Rectangle) -> PdfResult<ObjectId> {
        self.reject_duplicate_field_name(name)?;
        let page_id = self.page_id_at(page_index)?;
        let field_id = self.allocate_id();

        let mut dict = PdfDictionary::new();
        dict.set("Type", Object::Name(PdfName::new_unchecked("Annot")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Widget")));
        dict.set("FT", Object::Name(PdfName::new_unchecked("Sig")));
        dict.set("T", Object::String(to_pdf_text_string(name)));
        dict.set("P", Object::Reference(page_id));
        dict.set("Rect", Object::Array(rect_to_array(rect)));
        dict.set("F", Object::Integer(4));
        self.set_object(field_id, Object::Dictionary(dict));
        self.register_field(field_id, page_id)?;

        let builder = AppearanceBuilder::new(rect.with_origin())
            .background_color(Color::gray(0.95))
            .border_color(Color::BLACK)
            .border_style(BorderStyle::Dashed);
        let content = builder.build_text_appearance(Some("Not signed"), "Helv", 10.0, Color::gray(0.4));
        self.set_widget_normal_appearance(field_id, content, rect.width(), rect.height())?;
        Ok(field_id)
    }

    // -- Internal plumbing -------------------------------------------------

    fn reject_duplicate_field_name(&self, name: &str) -> PdfResult<()> {
        if self.find_field(name)?.is_some() {
            return Err(EditorError::DuplicateFieldName(name.to_string()).into());
        }
        Ok(())
    }

    /// Appends `field_id` to `/AcroForm /Fields` (creating the AcroForm
    /// dictionary if needed) and to `page_id`'s `/Annots`.
    fn register_field(&mut self, field_id: ObjectId, page_id: ObjectId) -> PdfResult<()> {
        let mut catalog = self.catalog()?;
        let acroform_id = match catalog.get("AcroForm") {
            Some(Object::Reference(id)) => *id,
            _ => {
                let id = self.allocate_id();
                let mut af = PdfDictionary::new();
                af.set("Fields", Object::Array(PdfArray::new()));
                af.set("DA", Object::String(PdfString::literal("/Helv 12 Tf 0 g")));
                let mut dr = PdfDictionary::new();
                let mut font_dict = PdfDictionary::new();
                let mut helv = PdfDictionary::new();
                helv.set("Type", Object::Name(PdfName::new_unchecked("Font")));
                helv.set("Subtype", Object::Name(PdfName::new_unchecked("Type1")));
                helv.set("BaseFont", Object::Name(PdfName::new_unchecked("Helvetica")));
                font_dict.set("Helv", Object::Dictionary(helv));
                dr.set("Font", Object::Dictionary(font_dict));
                af.set("DR", Object::Dictionary(dr));
                self.set_object(id, Object::Dictionary(af));
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
        fields.push(Object::Reference(field_id));
        acroform.set("Fields", Object::Array(fields));
        self.set_object(acroform_id, Object::Dictionary(acroform));

        self.add_annot_to_page(page_id, field_id)
    }

    /// Replaces a single-appearance widget's `/AP /N` with a freshly built
    /// stream, allocating an object id for it if the widget didn't already
    /// have one to reuse.
    fn set_widget_normal_appearance(&mut self, widget_id: ObjectId, content: String, width: f64, height: f64) -> PdfResult<()> {
        let mut dict = self.get_dictionary(widget_id)?;
        let ap_id = match dict.get("AP") {
            Some(Object::Dictionary(ap)) => match ap.get("N") {
                Some(Object::Reference(id)) => *id,
                _ => self.allocate_id(),
            },
            _ => self.allocate_id(),
        };
        self.set_object(ap_id, Object::Stream(appearance_xobject(&content, width, height)));
        let mut ap = PdfDictionary::new();
        ap.set("N", Object::Reference(ap_id));
        dict.set("AP", Object::Dictionary(ap));
        self.set_object(widget_id, Object::Dictionary(dict));
        Ok(())
    }

    fn require_field(&self, name: &str) -> PdfResult<ObjectId> {
        self.find_field(name)?.ok_or_else(|| EditorError::FieldNotFound(name.to_string()).into())
    }

    fn require_ft(&self, id: ObjectId, name: &str, expected_ft: &str, expected_label: &'static str) -> PdfResult<()> {
        let actual = self.effective(id, "FT");
        let ok = matches!(&actual, Some(Object::Name(n)) if n.as_str() == expected_ft)
            && (expected_ft != "Btn" || self.matches_button_kind(id, expected_label));
        if ok {
            Ok(())
        } else {
            Err(EditorError::WrongFieldType { name: name.to_string(), expected: expected_label }.into())
        }
    }

    /// Distinguishes checkbox vs. radio-group `/Btn` fields by the
    /// `Radio` flag (ISO 32000-1 Table 227, bit 16).
    fn matches_button_kind(&self, id: ObjectId, expected_label: &'static str) -> bool {
        let is_radio = matches!(self.effective(id, "Ff"), Some(Object::Integer(f)) if f & (1 << 15) != 0);
        match expected_label {
            "radio" => is_radio,
            "checkbox" => !is_radio,
            _ => true,
        }
    }

    /// Finds the field with fully qualified name `name`, if any.
    fn find_field(&self, name: &str) -> PdfResult<Option<ObjectId>> {
        Ok(self.walk_fields()?.into_iter().find(|(n, _)| n == name).map(|(_, id)| id))
    }

    /// Walks every root `/AcroForm /Fields` entry, returning
    /// `(fully_qualified_name, terminal_field_id)` for every terminal
    /// (fillable) field.
    fn walk_fields(&self) -> PdfResult<Vec<(String, ObjectId)>> {
        let Ok(catalog) = self.catalog() else { return Ok(Vec::new()) };
        let Some(Object::Reference(acroform_id)) = catalog.get("AcroForm") else { return Ok(Vec::new()) };
        let Ok(acroform) = self.get_dictionary(*acroform_id) else { return Ok(Vec::new()) };
        let Some(Object::Array(roots)) = acroform.get("Fields") else { return Ok(Vec::new()) };

        let mut out = Vec::new();
        let mut visited = std::collections::HashSet::new();
        for f in roots.iter() {
            if let Object::Reference(id) = f {
                self.walk_field_node(*id, "", 0, &mut visited, &mut out)?;
            }
        }
        Ok(out)
    }

    fn walk_field_node(
        &self,
        id: ObjectId,
        prefix: &str,
        depth: u32,
        visited: &mut std::collections::HashSet<ObjectId>,
        out: &mut Vec<(String, ObjectId)>,
    ) -> PdfResult<()> {
        if depth > MAX_PARENT_DEPTH || out.len() + visited.len() > MAX_FIELD_NODES {
            return Ok(());
        }
        if !visited.insert(id) {
            return Ok(()); // Cycle guard.
        }
        let Ok(dict) = self.get_dictionary(id) else { return Ok(()) };
        let own_t = dict.get("T").and_then(text_of).unwrap_or_default();
        let fq = if own_t.is_empty() {
            prefix.to_string()
        } else if prefix.is_empty() {
            own_t
        } else {
            format!("{prefix}.{own_t}")
        };

        match dict.get("Kids") {
            Some(Object::Array(kids)) if !kids.is_empty() && !self.kids_are_widgets(kids) => {
                for kid in kids.iter() {
                    if let Object::Reference(kid_id) = kid {
                        self.walk_field_node(*kid_id, &fq, depth + 1, visited, out)?;
                    }
                }
            }
            _ => out.push((fq, id)),
        }
        Ok(())
    }

    fn kids_are_widgets(&self, kids: &PdfArray) -> bool {
        kids.iter().any(|k| {
            matches!(k, Object::Reference(id) if matches!(
                self.get_dictionary(*id).ok().and_then(|d| d.get("Subtype").cloned()),
                Some(Object::Name(n)) if n.as_str() == "Widget"
            ))
        })
    }

    /// Returns the widget annotation id(s) for a terminal field: its own
    /// id if merged (no `/Kids`), or its `/Kids` if they are widgets.
    fn widget_ids_of(&self, field_id: ObjectId) -> PdfResult<Vec<ObjectId>> {
        let dict = self.get_dictionary(field_id)?;
        match dict.get("Kids") {
            Some(Object::Array(kids)) if !kids.is_empty() && self.kids_are_widgets(kids) => Ok(kids
                .iter()
                .filter_map(|k| match k {
                    Object::Reference(id) => Some(*id),
                    _ => None,
                })
                .collect()),
            _ => Ok(vec![field_id]),
        }
    }

    /// Walks `/Parent` to find the first (nearest-ancestor-first) value of
    /// `key`, per the inheritable-attribute rule of ISO 32000-1 12.7.3.2.
    fn effective(&self, mut id: ObjectId, key: &str) -> Option<Object> {
        for _ in 0..MAX_PARENT_DEPTH {
            let dict = self.get_dictionary(id).ok()?;
            if let Some(v) = dict.get(key) {
                return Some(v.clone());
            }
            match dict.get("Parent") {
                Some(Object::Reference(parent)) => id = *parent,
                _ => return None,
            }
        }
        None
    }
}

/// Reads the `/BG`, `/BC`, border-style and border-width from a widget's
/// `/MK` and `/BS` entries (ISO 32000-1 12.5.6.19, Table 189), falling
/// back to plain white/black/solid/1pt if absent - this crate does not
/// try to preserve a source field's exact original styling when
/// regenerating an appearance, only its colors/border where declared.
fn mk_colors(widget: &PdfDictionary) -> (Color, Color, BorderStyle, f64) {
    let mk = match widget.get("MK") {
        Some(Object::Dictionary(d)) => Some(d),
        _ => None,
    };
    let bg = mk.and_then(|d| d.get("BG")).and_then(color_from_array).unwrap_or(Color::WHITE);
    let bc = mk.and_then(|d| d.get("BC")).and_then(color_from_array).unwrap_or(Color::BLACK);
    let (style, width) = match widget.get("BS") {
        Some(Object::Dictionary(bs)) => {
            let style = match bs.get("S") {
                Some(Object::Name(n)) => match n.as_str() {
                    "D" => BorderStyle::Dashed,
                    "B" => BorderStyle::Beveled,
                    "I" => BorderStyle::Inset,
                    "U" => BorderStyle::Underline,
                    _ => BorderStyle::Solid,
                },
                _ => BorderStyle::Solid,
            };
            let width = bs.get("W").and_then(|o| o.as_real()).unwrap_or(1.0);
            (style, width)
        }
        _ => (BorderStyle::Solid, 1.0),
    };
    (bg, bc, style, width)
}

fn color_from_array(obj: &Object) -> Option<Color> {
    let Object::Array(arr) = obj else { return None };
    match arr.len() {
        1 => Some(Color::gray(arr.get(0)?.as_real()?)),
        3 => Some(Color::rgb(arr.get(0)?.as_real()?, arr.get(1)?.as_real()?, arr.get(2)?.as_real()?)),
        4 => Some(Color::cmyk(arr.get(0)?.as_real()?, arr.get(1)?.as_real()?, arr.get(2)?.as_real()?, arr.get(3)?.as_real()?)),
        _ => None,
    }
}

fn text_of(obj: &Object) -> Option<String> {
    match obj {
        Object::String(s) => Some(super::util::from_pdf_text_string(s)),
        _ => None,
    }
}

fn is_on(v: Option<&Object>) -> bool {
    matches!(v, Some(Object::Name(n)) if n.as_str() != "Off")
}

fn matrix_from_object(obj: Option<&Object>) -> crate::types::Matrix {
    let Some(Object::Array(arr)) = obj else { return crate::types::Matrix::identity() };
    if arr.len() != 6 {
        return crate::types::Matrix::identity();
    }
    let mut v = [0.0f64; 6];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = arr.get(i).and_then(|o| o.as_real()).unwrap_or(if i == 0 || i == 3 { 1.0 } else { 0.0 });
    }
    crate::types::Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5])
}

/// Resolves the effective `/DA` default-appearance string for a field
/// (own, else inherited from `/AcroForm /DA`), defaulting to plain black
/// 12pt Helvetica if neither is present.
fn effective_da_string(doc: &EditableDocument, field_id: ObjectId) -> String {
    if let Some(Object::String(s)) = doc.effective(field_id, "DA") {
        return super::util::from_pdf_text_string(&s);
    }
    if let Ok(catalog) = doc.catalog() {
        if let Some(Object::Reference(acroform_id)) = catalog.get("AcroForm") {
            if let Ok(acroform) = doc.get_dictionary(*acroform_id) {
                if let Some(Object::String(s)) = acroform.get("DA") {
                    return super::util::from_pdf_text_string(s);
                }
            }
        }
    }
    "/Helv 12 Tf 0 g".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use crate::forms::{CheckBox, ComboBox, RadioButton, RadioGroup, TextField};

    fn doc_with_form() -> EditableDocument {
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .form_field(TextField::new("name").rect(100.0, 700.0, 200.0, 20.0).default_value("placeholder"))
            .form_field(CheckBox::new("subscribe").rect(100.0, 650.0, 20.0, 20.0))
            .form_field(
                RadioGroup::new("plan")
                    .add_button(RadioButton::new("basic").rect(100.0, 600.0, 20.0, 20.0))
                    .add_button(RadioButton::new("pro").rect(140.0, 600.0, 20.0, 20.0)),
            )
            .form_field(ComboBox::new("country").rect(100.0, 550.0, 150.0, 20.0).options(vec!["USA", "Canada", "UK"]))
            .content(ContentBuilder::new())
            .build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        EditableDocument::from_bytes(bytes).unwrap()
    }

    #[test]
    fn test_field_names_lists_all_top_level_fields() {
        let doc = doc_with_form();
        let mut names = doc.field_names().unwrap();
        names.sort();
        assert_eq!(names, vec!["country", "name", "plan", "subscribe"]);
    }

    #[test]
    fn test_field_type_reports_pdf_ft() {
        let doc = doc_with_form();
        assert_eq!(doc.field_type("name").unwrap().as_deref(), Some("Tx"));
        assert_eq!(doc.field_type("subscribe").unwrap().as_deref(), Some("Btn"));
        assert_eq!(doc.field_type("country").unwrap().as_deref(), Some("Ch"));
        assert_eq!(doc.field_type("nonexistent").unwrap(), None);
    }

    #[test]
    fn test_text_field_fill_roundtrip() {
        let mut doc = doc_with_form();
        doc.set_text_value("name", "Ada Lovelace").unwrap();
        assert_eq!(doc.get_text_value("name").unwrap().as_deref(), Some("Ada Lovelace"));

        let saved = doc.save_incremental_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert_eq!(reopened.get_text_value("name").unwrap().as_deref(), Some("Ada Lovelace"));
    }

    #[test]
    fn test_text_field_fill_non_ascii_roundtrip() {
        let mut doc = doc_with_form();
        doc.set_text_value("name", "Zoé Müller").unwrap();
        let saved = doc.save_incremental_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert_eq!(reopened.get_text_value("name").unwrap().as_deref(), Some("Zoé Müller"));
    }

    #[test]
    fn test_checkbox_fill_roundtrip() {
        let mut doc = doc_with_form();
        assert!(!doc.get_checkbox_checked("subscribe").unwrap());
        doc.set_checkbox_checked("subscribe", true).unwrap();
        assert!(doc.get_checkbox_checked("subscribe").unwrap());

        let saved = doc.save_incremental_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert!(reopened.get_checkbox_checked("subscribe").unwrap());
    }

    #[test]
    fn test_radio_fill_roundtrip() {
        let mut doc = doc_with_form();
        doc.set_radio_value("plan", "pro").unwrap();
        assert_eq!(doc.get_radio_value("plan").unwrap().as_deref(), Some("pro"));

        let saved = doc.save_incremental_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert_eq!(reopened.get_radio_value("plan").unwrap().as_deref(), Some("pro"));
    }

    #[test]
    fn test_combobox_fill_roundtrip() {
        let mut doc = doc_with_form();
        doc.set_choice_value("country", "Canada").unwrap();
        assert_eq!(doc.get_choice_value("country").unwrap().as_deref(), Some("Canada"));

        let saved = doc.save_incremental_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert_eq!(reopened.get_choice_value("country").unwrap().as_deref(), Some("Canada"));
    }

    #[test]
    fn test_set_value_on_missing_field_errors() {
        let mut doc = doc_with_form();
        assert!(doc.set_text_value("does-not-exist", "x").is_err());
    }

    #[test]
    fn test_set_value_with_wrong_type_errors() {
        let mut doc = doc_with_form();
        assert!(doc.set_checkbox_checked("name", true).is_err());
        assert!(doc.set_text_value("subscribe", "x").is_err());
    }

    #[test]
    fn test_add_text_field_to_existing_pdf() {
        let bytes = DocumentBuilder::new()
            .page(PageBuilder::a4().build())
            .build()
            .unwrap()
            .save_to_bytes()
            .unwrap();
        let mut doc = EditableDocument::from_bytes(bytes).unwrap();
        doc.add_text_field(0, "new_field", Rectangle::new(50.0, 700.0, 250.0, 720.0), Some("hi"))
            .unwrap();
        assert_eq!(doc.get_text_value("new_field").unwrap().as_deref(), Some("hi"));

        let saved = doc.save_incremental_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        assert_eq!(reopened.field_names().unwrap(), vec!["new_field"]);
        assert_eq!(reopened.get_text_value("new_field").unwrap().as_deref(), Some("hi"));
    }

    #[test]
    fn test_add_duplicate_field_name_errors() {
        let mut doc = doc_with_form();
        assert!(doc.add_text_field(0, "name", Rectangle::new(0.0, 0.0, 10.0, 10.0), None).is_err());
    }

    #[test]
    fn test_add_checkbox_and_radio_and_combobox_and_signature() {
        let bytes = DocumentBuilder::new()
            .page(PageBuilder::a4().build())
            .build()
            .unwrap()
            .save_to_bytes()
            .unwrap();
        let mut doc = EditableDocument::from_bytes(bytes).unwrap();

        doc.add_checkbox_field(0, "agree", Rectangle::new(10.0, 10.0, 30.0, 30.0), true).unwrap();
        assert!(doc.get_checkbox_checked("agree").unwrap());

        doc.add_radio_group_field(
            0,
            "tier",
            &[("gold".to_string(), Rectangle::new(0.0, 0.0, 10.0, 10.0)), ("silver".to_string(), Rectangle::new(20.0, 0.0, 30.0, 10.0))],
            Some(1),
        )
        .unwrap();
        assert_eq!(doc.get_radio_value("tier").unwrap().as_deref(), Some("silver"));

        doc.add_combobox_field(0, "language", Rectangle::new(0.0, 40.0, 100.0, 60.0), &["EN".to_string(), "FR".to_string()], Some(0))
            .unwrap();
        assert_eq!(doc.get_choice_value("language").unwrap().as_deref(), Some("EN"));

        doc.add_signature_field(0, "sig1", Rectangle::new(0.0, 80.0, 100.0, 120.0)).unwrap();
        assert_eq!(doc.field_type("sig1").unwrap().as_deref(), Some("Sig"));

        let saved = doc.save_incremental_to_bytes().unwrap();
        let lopdf_doc = lopdf::Document::load_mem(&saved).expect("lopdf must open the saved document");
        assert!(!lopdf_doc.get_pages().is_empty());
    }

    #[test]
    fn test_flatten_form_removes_acroform_and_widgets_but_keeps_visual_text() {
        let mut doc = doc_with_form();
        doc.set_text_value("name", "Flattened Value").unwrap();
        doc.flatten_form().unwrap();

        assert!(doc.field_names().unwrap().is_empty());
        let page_id = doc.page_id_at(0).unwrap();
        let page = doc.get_dictionary(page_id).unwrap();
        assert!(page.get("Annots").is_none());

        // The visible text now lives inside a painted Form XObject
        // (`/FlatForm Do`) rather than inline in the page's own content
        // stream - that's what "flattening" means (ISO 32000-1 12.5.5):
        // resolve the XObject the page now references and check *its*
        // stream data for the filled-in value.
        let content = String::from_utf8_lossy(&doc.page_content_bytes(page_id).unwrap()).into_owned();
        assert!(content.contains("/FlatForm Do"), "flattened content was: {content}");
        let resources = match page.get("Resources") {
            Some(Object::Dictionary(d)) => d.clone(),
            other => panic!("expected page /Resources, got {other:?}"),
        };
        let xobjects = match resources.get("XObject") {
            Some(Object::Dictionary(d)) => d.clone(),
            other => panic!("expected /Resources /XObject, got {other:?}"),
        };
        let Some(Object::Reference(xobj_id)) = xobjects.get("FlatForm") else {
            panic!("expected /FlatForm XObject resource")
        };
        let Some(Object::Stream(xobj_stream)) = doc.get_object(*xobj_id) else {
            panic!("expected /FlatForm to resolve to a stream")
        };
        let xobj_content = String::from_utf8_lossy(&xobj_stream.data).into_owned();
        assert!(xobj_content.contains("Flattened Value"), "flattened XObject content was: {xobj_content}");

        let saved = doc.save_incremental_to_bytes().unwrap();
        lopdf::Document::load_mem(&saved).expect("lopdf must open the flattened document");
    }

    #[test]
    fn test_malformed_field_tree_does_not_hang_or_panic() {
        // A field whose own Kids array contains a self-reference cycle.
        let bytes = DocumentBuilder::new().page(PageBuilder::a4().build()).build().unwrap().save_to_bytes().unwrap();
        let mut doc = EditableDocument::from_bytes(bytes).unwrap();

        let field_id = doc.allocate_id();
        let mut field = PdfDictionary::new();
        field.set("FT", Object::Name(PdfName::new_unchecked("Tx")));
        field.set("T", Object::String(PdfString::literal("cyclic")));
        let mut kids = PdfArray::new();
        kids.push(Object::Reference(field_id)); // points at itself
        field.set("Kids", Object::Array(kids));
        doc.set_object(field_id, Object::Dictionary(field));

        let mut acroform = PdfDictionary::new();
        let mut fields = PdfArray::new();
        fields.push(Object::Reference(field_id));
        acroform.set("Fields", Object::Array(fields));
        let acroform_id = doc.allocate_id();
        doc.set_object(acroform_id, Object::Dictionary(acroform));
        let mut catalog = doc.catalog().unwrap();
        catalog.set("AcroForm", Object::Reference(acroform_id));
        let root = doc.catalog_id();
        doc.set_object(root, Object::Dictionary(catalog));

        // Must terminate rather than infinitely recurse.
        let names = doc.field_names().unwrap();
        assert!(names.is_empty() || names == vec!["cyclic".to_string()]);
    }
}
