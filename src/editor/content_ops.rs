//! Page content-stream editing: insert/replace text, shapes and images on
//! an existing page (ISO 32000-1:2008 Section 7.8.2 "Content Streams").

use super::content_stream::{parse_content_stream, replace_text_in_items, serialize_content_stream};
use super::graph::EditableDocument;
use crate::content::ContentBuilder;
use crate::error::{EditorError, PdfResult};
use crate::object::{Object, PdfStream};
use crate::types::ObjectId;

/// Hard cap on the combined, decoded size of a single page's content
/// streams. Bounds memory use against a corrupt/adversarial source file
/// (e.g. a `/Filter` chain whose declared vs. actual expansion ratio is
/// abused as a decompression bomb); no legitimate single page comes close
/// to this.
const MAX_CONTENT_STREAM_BYTES: usize = 256 * 1024 * 1024;

impl EditableDocument {
    /// Returns a page's fully decoded content-stream bytes.
    ///
    /// A page's `/Contents` may be a single stream reference or an array
    /// of them (ISO 32000-1 7.7.3.3); per that section, multiple streams
    /// are logically concatenated with an inserted whitespace separator
    /// so that tokens spanning what was a stream boundary are never
    /// accidentally glued together (e.g. `...re` + `f...` becoming
    /// `...ref...`).
    pub fn page_content_bytes(&self, page_id: ObjectId) -> PdfResult<Vec<u8>> {
        let dict = self.get_dictionary(page_id)?;
        let ids = content_stream_ids(dict.get("Contents"));
        let mut out = Vec::new();
        for id in ids {
            let obj = self
                .get_object(id)
                .ok_or(EditorError::UnresolvedObject(id.number, id.generation))?;
            let Object::Stream(stream) = obj else {
                continue;
            };
            let decoded = stream.decode_all()?;
            if out.len().saturating_add(decoded.len()) > MAX_CONTENT_STREAM_BYTES {
                return Err(EditorError::ResourceLimitExceeded(
                    "combined page content stream exceeds the safety limit".to_string(),
                )
                .into());
            }
            if !out.is_empty() {
                out.push(b'\n');
            }
            out.extend_from_slice(&decoded);
        }
        Ok(out)
    }

    /// Replaces a page's content stream wholesale with `data`, compressing
    /// it (FlateDecode) and, where the page previously had exactly one
    /// content stream object, reusing that same object id (keeps
    /// incremental-save deltas small: a single modified object instead of
    /// one new object plus a modified page dictionary that now points at
    /// it).
    pub fn set_page_content_bytes(&mut self, page_id: ObjectId, data: Vec<u8>) -> PdfResult<()> {
        let mut dict = self.get_dictionary(page_id)?;
        let stream = PdfStream::new(data).with_compression()?;

        match dict.get("Contents") {
            // Already exactly one content stream: overwrite it in place
            // so the page dictionary itself doesn't need to change too.
            Some(Object::Reference(id)) => {
                self.set_object(*id, Object::Stream(stream));
            }
            // No contents, or an array of several streams: allocate one
            // fresh stream object and point /Contents at just that.
            _ => {
                let id = self.allocate_id();
                self.set_object(id, Object::Stream(stream));
                dict.set("Contents", Object::Reference(id));
                self.set_object(page_id, Object::Dictionary(dict));
            }
        }
        Ok(())
    }

    /// Appends operators to a page's content stream, wrapped in `q`/`Q`
    /// (ISO 32000-1 8.4.4) so the appended graphics state (color, text
    /// state, CTM, ...) never leaks into - or is affected by - whatever
    /// graphics state the existing content stream left behind, even if
    /// that stream has unbalanced `q`/`Q` pairs.
    pub fn append_page_content(&mut self, page_id: ObjectId, ops: &ContentBuilder) -> PdfResult<()> {
        let mut bytes = self.page_content_bytes(page_id)?;
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(b"q\n");
        bytes.extend_from_slice(&ops.build_bytes());
        bytes.extend_from_slice(b"\nQ\n");
        self.set_page_content_bytes(page_id, bytes)
    }

    /// Prepends operators to a page's content stream (drawn *underneath*
    /// the existing content, in painter's-model terms), wrapped in
    /// `q`/`Q` for the same isolation reason as
    /// [`EditableDocument::append_page_content`].
    pub fn prepend_page_content(&mut self, page_id: ObjectId, ops: &ContentBuilder) -> PdfResult<()> {
        let existing = self.page_content_bytes(page_id)?;
        let mut bytes = Vec::with_capacity(existing.len() + 64);
        bytes.extend_from_slice(b"q\n");
        bytes.extend_from_slice(&ops.build_bytes());
        bytes.extend_from_slice(b"\nQ\n");
        bytes.extend_from_slice(&existing);
        self.set_page_content_bytes(page_id, bytes)
    }

    /// Replaces a page's content stream wholesale with `ops`, discarding
    /// whatever was drawn on the page before.
    pub fn replace_page_content(&mut self, page_id: ObjectId, ops: &ContentBuilder) -> PdfResult<()> {
        self.set_page_content_bytes(page_id, ops.build_bytes())
    }

    /// Finds and replaces every occurrence of `find` with `replacement`
    /// inside the page's text-showing operators (`Tj`/`'`/`"`/`TJ`).
    /// Returns the number of string operands changed.
    ///
    /// This is a byte-level substring replace; see the [module
    /// docs](crate::editor) for why it's only meaningful for simple
    /// single-byte text encodings, not arbitrary CID/Type0 text.
    pub fn replace_page_text(
        &mut self,
        page_id: ObjectId,
        find: &str,
        replacement: &str,
    ) -> PdfResult<usize> {
        let bytes = self.page_content_bytes(page_id)?;
        let mut items = parse_content_stream(&bytes);
        let count = replace_text_in_items(&mut items, find.as_bytes(), replacement.as_bytes());
        if count > 0 {
            let new_bytes = serialize_content_stream(&items);
            self.set_page_content_bytes(page_id, new_bytes)?;
        }
        Ok(count)
    }

    /// Embeds `image` as a new XObject resource on `page_id` (allocating a
    /// resource name if `resource_name` collides with an existing one)
    /// and appends a `cm`/`Do` invocation drawing it within `rect` (its
    /// lower-left corner and size), in PDF user-space points (ISO 32000-1
    /// 8.9.5).
    #[cfg(feature = "images")]
    pub fn draw_image(
        &mut self,
        page_id: ObjectId,
        resource_name: &str,
        image: &crate::image::Image,
        rect: crate::types::Rectangle,
    ) -> PdfResult<()> {
        use crate::image::ImageXObject;
        use crate::object::PdfDictionary;

        let image_id = self.allocate_id();
        let mask_id = if image.has_alpha() {
            Some(self.allocate_id())
        } else {
            None
        };
        let xobject = ImageXObject::from_image_with_mask_ref(image, mask_id);
        if let (Some(mask_id), Some(mask_stream)) = (mask_id, xobject.soft_mask) {
            self.set_object(mask_id, Object::Stream(mask_stream));
        }
        self.set_object(image_id, Object::Stream(xobject.stream));

        let mut page_dict = self.get_dictionary(page_id)?;
        let mut resources = match page_dict.get("Resources") {
            Some(Object::Dictionary(d)) => d.clone(),
            Some(Object::Reference(id)) => self.get_dictionary(*id)?,
            _ => PdfDictionary::new(),
        };
        let mut xobjects = match resources.get("XObject") {
            Some(Object::Dictionary(d)) => d.clone(),
            _ => PdfDictionary::new(),
        };
        let name = unique_resource_name(&xobjects, resource_name);
        xobjects.set(name.clone(), Object::Reference(image_id));
        resources.set("XObject", Object::Dictionary(xobjects));
        page_dict.set("Resources", Object::Dictionary(resources));
        self.set_object(page_id, Object::Dictionary(page_dict));

        let draw = ContentBuilder::new().draw_image(name, rect.llx, rect.lly, rect.width(), rect.height());
        self.append_page_content(page_id, &draw)
    }
}

fn content_stream_ids(contents: Option<&Object>) -> Vec<ObjectId> {
    match contents {
        Some(Object::Reference(id)) => vec![*id],
        Some(Object::Array(arr)) => arr
            .iter()
            .filter_map(|o| match o {
                Object::Reference(id) => Some(*id),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(feature = "images")]
fn unique_resource_name(existing: &crate::object::PdfDictionary, preferred: &str) -> String {
    if !existing.contains_key(preferred) {
        return preferred.to_string();
    }
    for i in 1.. {
        let candidate = format!("{preferred}{i}");
        if !existing.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    fn doc_with_one_page() -> (EditableDocument, ObjectId) {
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Hello World"))
            .build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        let doc = EditableDocument::from_bytes(bytes).unwrap();
        let id = doc.page_id_at(0).unwrap();
        (doc, id)
    }

    #[test]
    fn test_read_page_content_bytes() {
        let (doc, id) = doc_with_one_page();
        let bytes = doc.page_content_bytes(id).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Hello World"));
    }

    #[test]
    fn test_replace_page_text() {
        let (mut doc, id) = doc_with_one_page();
        let count = doc.replace_page_text(id, "World", "Rust").unwrap();
        assert_eq!(count, 1);
        let bytes = doc.page_content_bytes(id).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Hello Rust"));
        assert!(!text.contains("Hello World"));
    }

    #[test]
    fn test_replace_page_text_no_match_is_noop() {
        let (mut doc, id) = doc_with_one_page();
        let count = doc.replace_page_text(id, "NotPresent", "X").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_append_page_content_preserves_existing() {
        let (mut doc, id) = doc_with_one_page();
        let extra = ContentBuilder::new().text("F1", 12.0, 72.0, 700.0, "Appended");
        doc.append_page_content(id, &extra).unwrap();
        let bytes = doc.page_content_bytes(id).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Hello World"));
        assert!(text.contains("Appended"));
    }

    #[test]
    fn test_replace_page_content_discards_old() {
        let (mut doc, id) = doc_with_one_page();
        let new_content = ContentBuilder::new().text("F1", 12.0, 72.0, 700.0, "Replaced");
        doc.replace_page_content(id, &new_content).unwrap();
        let bytes = doc.page_content_bytes(id).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("Hello World"));
        assert!(text.contains("Replaced"));
    }

    #[test]
    fn test_set_content_reuses_existing_stream_object_id() {
        let (mut doc, id) = doc_with_one_page();
        let contents_id_before = match doc.get_dictionary(id).unwrap().get("Contents") {
            Some(Object::Reference(r)) => *r,
            _ => panic!("expected single content stream"),
        };
        doc.replace_page_text(id, "Hello", "Hi").unwrap();
        let contents_id_after = match doc.get_dictionary(id).unwrap().get("Contents") {
            Some(Object::Reference(r)) => *r,
            _ => panic!("expected single content stream"),
        };
        assert_eq!(contents_id_before, contents_id_after);
    }

    #[cfg(feature = "images")]
    #[test]
    fn test_draw_image_adds_xobject_resource_and_do_operator() {
        use crate::image::{ColorSpace, Image, ImageFilter};
        use crate::types::Rectangle;

        let (mut doc, id) = doc_with_one_page();
        // A tiny 2x2 raw (uncompressed) RGB image is enough to exercise
        // the XObject-embedding path without needing real JPEG/PNG data.
        let image = Image::new(
            2,
            2,
            ColorSpace::DeviceRGB,
            8,
            ImageFilter::FlateDecode,
            vec![0, 0, 0, 255, 255, 255, 128, 128, 128, 0, 0, 0],
        );

        doc.draw_image(id, "Im1", &image, Rectangle::new(10.0, 10.0, 110.0, 110.0))
            .unwrap();

        let page_dict = doc.get_dictionary(id).unwrap();
        let resources = match page_dict.get("Resources") {
            Some(Object::Dictionary(d)) => d,
            other => panic!("expected Resources dictionary, got {other:?}"),
        };
        let xobjects = match resources.get("XObject") {
            Some(Object::Dictionary(d)) => d,
            other => panic!("expected XObject dictionary, got {other:?}"),
        };
        assert!(xobjects.contains_key("Im1"));

        let content = String::from_utf8_lossy(&doc.page_content_bytes(id).unwrap()).into_owned();
        assert!(content.contains("/Im1 Do"), "content stream was: {content}");
    }
}
