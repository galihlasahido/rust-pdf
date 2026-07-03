//! Definition-of-done tests for the "Color Spaces & Images" phase, driven
//! through the full [`render_content_stream`] pipeline (not just the
//! `colorspace`/`image` modules' own unit tests): `cs`/`scn` with
//! DeviceGray/RGB/CMYK/Indexed/Separation colour spaces, image XObjects
//! (`Do`) using JPEG (`DCTDecode`) and CCITT (`CCITTFaxDecode`) filters
//! reused from [`crate::filter`], and the explicit JBIG2Decode placeholder
//! gap.

use super::*;
use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream};
use crate::types::Rectangle;

fn page() -> Rectangle {
    Rectangle::new(0.0, 0.0, 200.0, 200.0)
}

fn pixel(out: &NativeRenderOutput, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let p = out
        .pixmap
        .pixel(x, y)
        .unwrap_or_else(|| panic!("pixel ({x},{y}) out of bounds"))
        .demultiply();
    (p.red(), p.green(), p.blue(), p.alpha())
}

fn name(s: &str) -> Object {
    Object::Name(PdfName::new_unchecked(s))
}

fn resources_with_xobject(im_name: &str, stream: PdfStream) -> PdfDictionary {
    let mut xobjects = PdfDictionary::new();
    xobjects.set(im_name, Object::Stream(stream));
    let mut resources = PdfDictionary::new();
    resources.set("XObject", Object::Dictionary(xobjects));
    resources
}

// ---------------------------------------------------------------------
// Colour spaces via `cs`/`scn` (ISO 32000-1 8.6.6-8.6.8), not just the
// legacy `g`/`rg`/`k` device operators.
// ---------------------------------------------------------------------

#[test]
fn cs_scn_device_gray_rgb_cmyk_paint_expected_pixels() {
    // Three side-by-side rectangles, each in a different Device colour
    // space selected via `cs`/`scn` (not `g`/`rg`/`k`).
    let content = b"\
        /DeviceGray cs 0.0 scn 0 0 60 200 re f \
        /DeviceRGB cs 0 1 0 scn 60 0 60 200 re f \
        /DeviceCMYK cs 0 0 1 0 scn 120 0 80 200 re f";
    let out = render_content_stream(content, 200, 200, page(), None).unwrap();
    assert!(out.warnings.is_empty(), "unexpected warnings: {:?}", out.warnings);
    assert_eq!(pixel(&out, 30, 100), (0, 0, 0, 255), "DeviceGray 0.0 via cs/scn -> black");
    assert_eq!(pixel(&out, 90, 100), (0, 255, 0, 255), "DeviceRGB green via cs/scn");
    assert_eq!(pixel(&out, 160, 100), (255, 255, 0, 255), "DeviceCMYK yellow via cs/scn");
}

#[test]
fn cs_scn_indexed_color_space_paints_palette_entries() {
    let lookup = crate::object::PdfString::literal_bytes(vec![
        255, 0, 0, // index 0: red
        0, 0, 255, // index 1: blue
    ]);
    let mut cs_dict = PdfDictionary::new();
    cs_dict.set(
        "CS0",
        Object::Array(PdfArray::from_objects(vec![
            name("Indexed"),
            name("DeviceRGB"),
            Object::Integer(1),
            Object::String(lookup),
        ])),
    );
    let mut resources = PdfDictionary::new();
    resources.set("ColorSpace", Object::Dictionary(cs_dict));

    let content = b"/CS0 cs 0 scn 0 0 100 200 re f /CS0 cs 1 scn 100 0 100 200 re f";
    let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
    assert!(out.warnings.is_empty(), "unexpected warnings: {:?}", out.warnings);
    assert_eq!(pixel(&out, 50, 100), (255, 0, 0, 255), "Indexed palette entry 0 -> red");
    assert_eq!(pixel(&out, 150, 100), (0, 0, 255, 255), "Indexed palette entry 1 -> blue");
}

#[test]
fn cs_scn_separation_tint_transform_is_evaluated() {
    // Separation "Spot" -> DeviceGray via a Type 2 (Exponential) function:
    // C0=0 (no ink -> black... wait, use C0=1,C1=0 so tint 1.0 -> gray 0
    // (dark, "full ink"), tint 0.0 -> gray 1 (white, "no ink") -- matches
    // typical real-world Separation semantics.
    let mut func_dict = PdfDictionary::new();
    func_dict.set("FunctionType", Object::Integer(2));
    func_dict.set(
        "Domain",
        Object::Array(PdfArray::from_objects(vec![Object::Real(0.0), Object::Real(1.0)])),
    );
    func_dict.set("C0", Object::Array(PdfArray::from_objects(vec![Object::Real(1.0)])));
    func_dict.set("C1", Object::Array(PdfArray::from_objects(vec![Object::Real(0.0)])));
    func_dict.set("N", Object::Real(1.0));

    let mut cs_dict = PdfDictionary::new();
    cs_dict.set(
        "CS0",
        Object::Array(PdfArray::from_objects(vec![
            name("Separation"),
            name("Spot"),
            name("DeviceGray"),
            Object::Dictionary(func_dict),
        ])),
    );
    let mut resources = PdfDictionary::new();
    resources.set("ColorSpace", Object::Dictionary(cs_dict));

    let content = b"/CS0 cs 1.0 scn 0 0 100 200 re f /CS0 cs 0.0 scn 100 0 100 200 re f";
    let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
    assert!(out.warnings.is_empty(), "unexpected warnings: {:?}", out.warnings);
    assert_eq!(pixel(&out, 50, 100), (0, 0, 0, 255), "full tint (1.0) -> black via alternate DeviceGray");
    assert_eq!(pixel(&out, 150, 100), (255, 255, 255, 255), "no tint (0.0) -> white via alternate DeviceGray");
}

// ---------------------------------------------------------------------
// Image XObjects (`Do`) -- JPEG (DCTDecode) and CCITT (CCITTFaxDecode),
// reusing the decoders already implemented in `crate::filter`.
// ---------------------------------------------------------------------

/// A real, valid 4x4 baseline JPEG encoding a solid reddish colour
/// (`RGB(200, 40, 40)`), produced offline with Pillow -- exercises the
/// actual `jpeg-decoder`-backed `DCTDecode` path end to end, not a fake.
#[rustfmt::skip]
const TINY_RED_JPEG: &[u8] = &[
    0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01,
    0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43,
    0x00, 0x03, 0x02, 0x02, 0x03, 0x02, 0x02, 0x03, 0x03, 0x03, 0x03, 0x04,
    0x03, 0x03, 0x04, 0x05, 0x08, 0x05, 0x05, 0x04, 0x04, 0x05, 0x0A, 0x07,
    0x07, 0x06, 0x08, 0x0C, 0x0A, 0x0C, 0x0C, 0x0B, 0x0A, 0x0B, 0x0B, 0x0D,
    0x0E, 0x12, 0x10, 0x0D, 0x0E, 0x11, 0x0E, 0x0B, 0x0B, 0x10, 0x16, 0x10,
    0x11, 0x13, 0x14, 0x15, 0x15, 0x15, 0x0C, 0x0F, 0x17, 0x18, 0x16, 0x14,
    0x18, 0x12, 0x14, 0x15, 0x14, 0xFF, 0xDB, 0x00, 0x43, 0x01, 0x03, 0x04,
    0x04, 0x05, 0x04, 0x05, 0x09, 0x05, 0x05, 0x09, 0x14, 0x0D, 0x0B, 0x0D,
    0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14,
    0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14,
    0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14,
    0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14,
    0x14, 0x14, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x04, 0x00, 0x04, 0x03,
    0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xFF, 0xC4, 0x00,
    0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05,
    0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xC4, 0x00, 0xB5, 0x10, 0x00,
    0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00,
    0x00, 0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21,
    0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81,
    0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24,
    0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25,
    0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A,
    0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56,
    0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A,
    0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86,
    0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99,
    0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3,
    0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6,
    0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9,
    0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1,
    0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFF, 0xC4, 0x00,
    0x1F, 0x01, 0x00, 0x03, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05,
    0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xC4, 0x00, 0xB5, 0x11, 0x00,
    0x02, 0x01, 0x02, 0x04, 0x04, 0x03, 0x04, 0x07, 0x05, 0x04, 0x04, 0x00,
    0x01, 0x02, 0x77, 0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31,
    0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08,
    0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33, 0x52, 0xF0, 0x15,
    0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18,
    0x19, 0x1A, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39,
    0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55,
    0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x82, 0x83, 0x84,
    0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
    0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA,
    0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4,
    0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7,
    0xD8, 0xD9, 0xDA, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA,
    0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00,
    0x0C, 0x03, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3F, 0x00, 0xF1,
    0x4A, 0x28, 0xA2, 0xBF, 0x37, 0x3F, 0xB5, 0x4F, 0xFF, 0xD9,
];

#[test]
fn jpeg_image_xobject_paints_non_blank_content() {
    let mut dict = PdfDictionary::new();
    dict.set("Type", name("XObject"));
    dict.set("Subtype", name("Image"));
    dict.set("Width", Object::Integer(4));
    dict.set("Height", Object::Integer(4));
    dict.set("BitsPerComponent", Object::Integer(8));
    dict.set("ColorSpace", name("DeviceRGB"));
    dict.set("Filter", name("DCTDecode"));
    let stream = PdfStream::with_dictionary(dict, TINY_RED_JPEG.to_vec());
    let resources = resources_with_xobject("Im1", stream);

    // Places the image filling the [50,50]-[150,150] user-space square.
    let content = b"q 100 0 0 100 50 50 cm /Im1 Do Q";
    let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
    assert!(out.warnings.is_empty(), "unexpected warnings: {:?}", out.warnings);

    let (r, g, b, a) = pixel(&out, 100, 100);
    assert_eq!(a, 255);
    assert_ne!((r, g, b), (255, 255, 255), "JPEG image must paint non-blank content, not leave the white background");
    assert_ne!((r, g, b), (0, 0, 0), "decoded solid-red JPEG should not come out as black");
    assert!(r > g && r > b, "expected a reddish decoded pixel, got rgb({r},{g},{b})");

    // Outside the placed image, background is untouched.
    assert_eq!(pixel(&out, 10, 10), (255, 255, 255, 255));
}

/// Builds a Group-4 (CCITT K<0) bitstream for a 1-row, 16-column image:
/// left half white, right half black -- using the same hand-assembled
/// bit-string technique `crate::filter::ccitt`'s own tests use (Horizontal
/// mode: white run of 8, then black run of 8).
fn ccitt_half_white_half_black_16x1() -> Vec<u8> {
    // Horizontal mode "001", then the White/Black terminating codewords
    // for run-length 8 from ITU-T T.4 Table 2/3: white(8) = "10011",
    // black(8) = "000101" (same tables `crate::filter::ccitt`'s own tests
    // exercise, just a different run length than its `decodes_horizontal_
    // mode_black_run` test uses).
    let mut bits = String::new();
    bits.push_str("001"); // Horizontal mode
    bits.push_str("10011"); // white run length 8
    bits.push_str("000101"); // black run length 8
    let mut out = Vec::new();
    let mut cur = 0u8;
    let mut n = 0u8;
    for c in bits.chars() {
        cur = (cur << 1) | u8::from(c == '1');
        n += 1;
        if n == 8 {
            out.push(cur);
            cur = 0;
            n = 0;
        }
    }
    if n > 0 {
        cur <<= 8 - n;
        out.push(cur);
    }
    out
}

#[test]
fn ccitt_image_xobject_paints_non_blank_black_and_white_halves() {
    let data = ccitt_half_white_half_black_16x1();

    let mut parms = PdfDictionary::new();
    parms.set("K", Object::Integer(-1));
    parms.set("Columns", Object::Integer(16));
    parms.set("Rows", Object::Integer(1));

    let mut dict = PdfDictionary::new();
    dict.set("Type", name("XObject"));
    dict.set("Subtype", name("Image"));
    dict.set("Width", Object::Integer(16));
    dict.set("Height", Object::Integer(1));
    dict.set("BitsPerComponent", Object::Integer(1));
    dict.set("ColorSpace", name("DeviceGray"));
    dict.set("Filter", name("CCITTFaxDecode"));
    dict.set("DecodeParms", Object::Dictionary(parms));
    let stream = PdfStream::with_dictionary(dict, data);
    let resources = resources_with_xobject("Im1", stream);

    let content = b"q 160 0 0 160 20 20 cm /Im1 Do Q";
    let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
    assert!(out.warnings.is_empty(), "unexpected warnings: {:?}", out.warnings);

    // Sample a pixel comfortably inside each half of the placed image
    // (device y around 100, inside the [20,180] vertical placement).
    let left = pixel(&out, 40, 100);
    let right = pixel(&out, 160, 100);
    assert_ne!(left, right, "CCITT image must show two visibly different halves, not a blank/uniform fill");
    // Whichever polarity this decoder uses, both a black-ish and a
    // white-ish sample must be present -- i.e. genuinely non-blank
    // content, not just "some anti-aliasing noise".
    let is_extreme = |p: (u8, u8, u8, u8)| p.0 <= 10 || p.0 >= 245;
    assert!(is_extreme(left) && is_extreme(right), "expected near-pure black/white halves, got {left:?} / {right:?}");
}

// ---------------------------------------------------------------------
// The explicit, documented JBIG2Decode/JPXDecode gap: a structured
// placeholder, never a panic, never a silently blank page.
// ---------------------------------------------------------------------

#[test]
fn jbig2_image_xobject_yields_placeholder_not_panic_not_silent_blank() {
    let mut dict = PdfDictionary::new();
    dict.set("Type", name("XObject"));
    dict.set("Subtype", name("Image"));
    dict.set("Width", Object::Integer(50));
    dict.set("Height", Object::Integer(50));
    dict.set("BitsPerComponent", Object::Integer(1));
    dict.set("ColorSpace", name("DeviceGray"));
    dict.set("Filter", name("JBIG2Decode"));
    // Content is irrelevant/garbage -- there is no pure-Rust JBIG2 decoder
    // to even attempt running it against.
    let stream = PdfStream::with_dictionary(dict, vec![0u8; 16]);
    let resources = resources_with_xobject("Im1", stream);

    let content = b"q 100 0 0 100 50 50 cm /Im1 Do Q";
    let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();

    // (1) Structured indication: a specific, typed warning naming both the
    // resource and the unsupported filter -- not a generic/silent failure.
    assert!(out
        .warnings
        .iter()
        .any(|w| matches!(
            w,
            RenderWarning::UnsupportedImageFilter { name, filter }
                if name == "Im1" && filter == "JBIG2Decode"
        )));

    // (2) Not silently blank: the placeholder is visibly painted (a
    // distinctive mid-grey, neither the white page background nor
    // plausible real black content) over the image's placement rect.
    let (r, g, b, a) = pixel(&out, 100, 100);
    assert_eq!(a, 255);
    assert_ne!((r, g, b), (255, 255, 255), "must not be silently blank (white background) for JBIG2");
    assert_eq!((r, g, b), (160, 160, 160), "expected the documented flat mid-grey placeholder");
}

#[test]
fn jpx_image_xobject_also_yields_placeholder_not_panic() {
    let mut dict = PdfDictionary::new();
    dict.set("Type", name("XObject"));
    dict.set("Subtype", name("Image"));
    dict.set("Width", Object::Integer(20));
    dict.set("Height", Object::Integer(20));
    dict.set("ColorSpace", name("DeviceRGB"));
    dict.set("Filter", name("JPXDecode"));
    let stream = PdfStream::with_dictionary(dict, vec![0u8; 16]);
    let resources = resources_with_xobject("Im1", stream);

    let content = b"q 50 0 0 50 10 10 cm /Im1 Do Q";
    let out = render_content_stream(content, 200, 200, page(), Some(&resources)).unwrap();
    assert!(out
        .warnings
        .iter()
        .any(|w| matches!(w, RenderWarning::UnsupportedImageFilter { filter, .. } if filter == "JPXDecode")));
    let (r, g, b, _) = pixel(&out, 20, 180);
    assert_ne!((r, g, b), (255, 255, 255));
}

// ---------------------------------------------------------------------
// Inline images (`BI`/`ID`/`EI`) go through the same pipeline.
// ---------------------------------------------------------------------

#[test]
fn inline_image_paints_non_blank_content() {
    // 2x1 DeviceGray, 8 bpc, ASCIIHex-encoded: black then white.
    let content = b"q 100 0 0 100 50 50 cm BI /W 2 /H 1 /BPC 8 /CS /G /F /AHx ID 00FF> EI Q";
    let out = render_content_stream(content, 200, 200, page(), None).unwrap();
    assert!(out.warnings.is_empty(), "unexpected warnings: {:?}", out.warnings);
    let left = pixel(&out, 60, 100);
    let right = pixel(&out, 140, 100);
    assert_ne!(left, right);
}

#[test]
fn inline_image_with_jbig2_is_rejected_gracefully() {
    // Not valid per spec (JBIG2Decode isn't allowed on inline images) but
    // must still fail closed rather than panicking if seen anyway.
    let content = b"BI /W 8 /H 8 /BPC 1 /CS /G /F /JBIG2Decode ID \x00\x00\x00\x00\x00\x00\x00\x00 EI";
    let out = render_content_stream(content, 200, 200, page(), None).unwrap();
    assert!(out
        .warnings
        .iter()
        .any(|w| matches!(w, RenderWarning::UnsupportedImageFilter { filter, .. } if filter == "JBIG2Decode")));
}
