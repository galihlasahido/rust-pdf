//! Image XObject creation for PDF.

use super::{ColorSpace, Image, ImageFilter};
use crate::object::{Object, PdfDictionary, PdfName, PdfStream};
use crate::types::ObjectId;

/// An image XObject ready to be embedded in a PDF.
#[derive(Debug)]
pub struct ImageXObject {
    /// The image stream.
    pub stream: PdfStream,
    /// Optional soft mask stream (for transparency).
    pub soft_mask: Option<PdfStream>,
    /// The width of the image in pixels.
    pub width: u32,
    /// The height of the image in pixels.
    pub height: u32,
}

impl ImageXObject {
    /// Creates an XObject from an Image.
    ///
    /// If the image has a soft mask (alpha channel), it will be included.
    pub fn from_image(image: &Image) -> Self {
        let stream = Self::build_image_stream(image, None);
        let soft_mask = image.soft_mask.as_ref().map(|mask| {
            Self::build_soft_mask_stream(mask)
        });

        Self {
            stream,
            soft_mask,
            width: image.width,
            height: image.height,
        }
    }

    /// Creates an XObject from an Image with a specific soft mask reference.
    ///
    /// This is used when the soft mask is stored as a separate object.
    pub fn from_image_with_mask_ref(image: &Image, mask_id: Option<ObjectId>) -> Self {
        let stream = Self::build_image_stream(image, mask_id);
        let soft_mask = image.soft_mask.as_ref().map(|mask| {
            Self::build_soft_mask_stream(mask)
        });

        Self {
            stream,
            soft_mask,
            width: image.width,
            height: image.height,
        }
    }

    /// Builds the main image stream.
    fn build_image_stream(image: &Image, soft_mask_ref: Option<ObjectId>) -> PdfStream {
        let mut dict = PdfDictionary::new();

        // Required entries
        dict.set("Type", Object::Name(PdfName::new_unchecked("XObject")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Image")));
        dict.set("Width", Object::Integer(image.width as i64));
        dict.set("Height", Object::Integer(image.height as i64));
        dict.set(
            "ColorSpace",
            Object::Name(PdfName::new_unchecked(image.color_space.as_pdf_name())),
        );
        dict.set(
            "BitsPerComponent",
            Object::Integer(image.bits_per_component as i64),
        );
        dict.set(
            "Filter",
            Object::Name(PdfName::new_unchecked(image.filter.as_pdf_name())),
        );
        dict.set("Length", Object::Integer(image.data.len() as i64));

        // Add soft mask reference if provided
        if let Some(mask_id) = soft_mask_ref {
            dict.set("SMask", Object::Reference(mask_id));
        }

        // For JPEG images, add decode parameters if needed
        if image.filter == ImageFilter::DCTDecode && image.color_space == ColorSpace::DeviceCMYK {
            // CMYK JPEGs often need inverted decode
            let decode = crate::object::PdfArray::from_iter([
                Object::Integer(1),
                Object::Integer(0),
                Object::Integer(1),
                Object::Integer(0),
                Object::Integer(1),
                Object::Integer(0),
                Object::Integer(1),
                Object::Integer(0),
            ]);
            dict.set("Decode", Object::Array(decode));
        }

        PdfStream::with_dictionary(dict, image.data.clone())
    }

    /// Builds a soft mask stream (alpha channel).
    fn build_soft_mask_stream(mask: &Image) -> PdfStream {
        let mut dict = PdfDictionary::new();

        dict.set("Type", Object::Name(PdfName::new_unchecked("XObject")));
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Image")));
        dict.set("Width", Object::Integer(mask.width as i64));
        dict.set("Height", Object::Integer(mask.height as i64));
        dict.set(
            "ColorSpace",
            Object::Name(PdfName::new_unchecked("DeviceGray")),
        );
        dict.set("BitsPerComponent", Object::Integer(8));
        dict.set(
            "Filter",
            Object::Name(PdfName::new_unchecked(mask.filter.as_pdf_name())),
        );
        dict.set("Length", Object::Integer(mask.data.len() as i64));

        PdfStream::with_dictionary(dict, mask.data.clone())
    }

    /// Returns true if this XObject has a soft mask.
    pub fn has_soft_mask(&self) -> bool {
        self.soft_mask.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xobject_from_image() {
        let image = Image::new(
            100,
            50,
            ColorSpace::DeviceRGB,
            8,
            ImageFilter::FlateDecode,
            vec![0; 100],
        );

        let xobject = ImageXObject::from_image(&image);

        assert_eq!(xobject.width, 100);
        assert_eq!(xobject.height, 50);
        assert!(!xobject.has_soft_mask());

        let dict_str = xobject.stream.dictionary_to_pdf_string();
        assert!(dict_str.contains("/Type /XObject"));
        assert!(dict_str.contains("/Subtype /Image"));
        assert!(dict_str.contains("/Width 100"));
        assert!(dict_str.contains("/Height 50"));
        assert!(dict_str.contains("/ColorSpace /DeviceRGB"));
        assert!(dict_str.contains("/Filter /FlateDecode"));
    }
}
