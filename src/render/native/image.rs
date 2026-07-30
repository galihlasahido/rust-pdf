//! Image XObject (`Do`, ISO 32000-1:2008 §8.9.5) and inline image (`BI`/
//! `ID`/`EI`, §8.9.7) decoding: unpacks already-filter-decoded sample bytes
//! (reusing the filter decoders already implemented in [`crate::filter`] --
//! `DCTDecode` via `jpeg-decoder`, `CCITTFaxDecode`, `LZWDecode`,
//! `RunLengthDecode`, `ASCII85Decode`/`ASCIIHexDecode`, `FlateDecode`/
//! predictor -- via [`crate::object::PdfStream::decode_all`]) into a
//! straight RGBA8 raster [`super::interpreter::Interpreter`] can hand to
//! `tiny-skia` as a `Pixmap`.
//!
//! # Explicit, honest gap: JBIG2Decode and JPXDecode
//!
//! **There is no mature pure-Rust decoder for either JBIG2 or JPEG2000
//! (`JPXDecode`) in the ecosystem today.** This is a hard, structural gap,
//! not a "didn't get to it yet" one -- see `ARCHITECTURE.md`'s rendering
//! section and the crate-level task notes this phase was built against.
//! When an image's filter chain contains either, this module returns
//! [`ImageResult::UnsupportedFilter`] *before* attempting any generic
//! filter decode, so the caller ([`super::interpreter`]) can paint a
//! clearly-artificial, documented placeholder (a flat mid-grey box, the
//! same "broken image" convention most browsers use) and record
//! [`super::error::RenderWarning::UnsupportedImageFilter`] -- this is
//! never silently blank and never a panic, but it is also honestly *not*
//! "the image, rendered": it is a structured stand-in that says so.
//!
//! # Scope of what *is* implemented this phase
//!
//! - Device (Gray/RGB/CMYK), Indexed, Separation/DeviceN and
//!   (approximated) ICCBased colour images at 1/2/4/8/16 bits per
//!   component (see [`super::colorspace`]).
//! - `/ImageMask true` stencil masks, painted using the current
//!   non-stroking colour (ISO 32000-1 §8.9.6.2), honoring `/Decode [1 0]`
//!   inversion.
//! - Any filter [`crate::filter`] already implements: `DCTDecode` (JPEG),
//!   `CCITTFaxDecode` (Group 4 only -- see `crate::filter::ccitt`'s own
//!   documented limitation), `LZWDecode`, `FlateDecode`, `RunLengthDecode`,
//!   `ASCII85Decode`/`ASCIIHexDecode`, in any chained combination.
//!
//! # Image-level soft masks (`/SMask`, ISO 32000-1 §11.6.5.3)
//!
//! An image XObject's own `/SMask` entry (a separate DeviceGray image
//! stream providing a per-pixel alpha channel for the base image) **is**
//! decoded and applied -- this is a real implementation, not a
//! placeholder: the mask image is decoded through the same filter/bit-
//! unpacking pipeline as any other image, resampled (nearest-neighbor,
//! not bilinear -- a documented simplification) to the base image's
//! dimensions if they differ, and its gray sample (post-`/Decode`, so
//! `/Decode [1 0]` correctly inverts it) becomes that pixel's alpha
//! (ISO 32000-1's default SMask `/Decode [0 1]`: `0` = transparent, `1` =
//! opaque). If the `/SMask` stream itself can't be decoded (missing
//! geometry, `JBIG2Decode`/`JPXDecode`, a non-DeviceGray colour space,
//! ...), the *base* image still paints, but fully opaque, and
//! [`super::error::RenderWarning::ImageSoftMaskDecodeFailed`] is recorded
//! once -- never a hard failure of the whole image over a broken mask.
//! `/Matte` (pre-blended/"matted" colour un-premultiplication, ISO
//! 32000-1 §11.6.5.3) is **not** implemented -- a documented, minor gap
//! (only affects the exact edge colour of a matted image, not whether it
//! renders).
//!
//! # Explicit, honest gaps beyond JBIG2/JPX
//!
//! - **No explicit-mask (`/Mask`, colour-key array or stencil-image)
//!   compositing.** This is the older (pre-`/SMask`) PDF 1.3 masking
//!   mechanism (ISO 32000-1 §8.9.6.4), distinct from the `/SMask`
//!   soft-mask entry above (which *is* implemented). Every decoded image
//!   this phase paints ignores a `/Mask` entry -- not specially warned
//!   about per-image (would be noisy); documented here as a known
//!   simplification.
//! - **No colour-managed CMYK JPEG Adobe-inversion detection** beyond
//!   whatever the image's own `/Decode` array explicitly requests --
//!   relies on the PDF producer having set `/Decode [1 0 1 0 1 0 1 0]`
//!   when needed, same as most simple (non-Adobe-heuristic) renderers.

use tiny_skia::Color;

use crate::object::{Object, PdfDictionary, PdfStream};
use crate::parser::InlineImage;

use super::bits::BitReader;
use super::colorspace::{self, ColorSpace};

/// Hard cap on `width * height` for an image XObject/inline image, applied
/// *before* any pixel buffer is allocated -- an untrusted `/Width`/
/// `/Height` pair must not be able to force an unbounded allocation
/// (decompression-bomb-style attack against the rasterizer instead of a
/// stream filter). Matches the order of magnitude of
/// [`crate::render::MAX_RENDER_PIXELS`] (that constant isn't reused
/// directly since it's `render`-feature-gated, not `native-render`).
pub(super) const MAX_IMAGE_PIXELS: u64 = 64_000_000;

/// A decoded image ready to paint: straight (non-premultiplied) RGBA8,
/// row-major, top row first. Alpha is `0`/`255` unless an `/SMask` was
/// present and successfully decoded (see the [module docs](self)), in
/// which case alpha is the mask's per-pixel gray value. Since this is
/// still *straight* (non-premultiplied) alpha, not premultiplied, the
/// caller (`interpreter::draw_image_pixels`) must premultiply before
/// handing it to `tiny-skia::Pixmap::from_vec`, which requires
/// premultiplied data.
pub(super) struct DecodedImage {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
    /// Set if this image declared an `/SMask` that could not be decoded
    /// (see [module docs](self)) -- the base image above is still fully
    /// opaque in that case, and the caller should surface this as
    /// [`super::error::RenderWarning::ImageSoftMaskDecodeFailed`].
    pub(super) smask_warning: Option<String>,
}

/// The outcome of attempting to decode an image XObject/inline image.
pub(super) enum ImageResult {
    Ok(DecodedImage),
    /// The filter chain includes `JBIG2Decode` or `JPXDecode` -- a hard,
    /// documented gap (see [module docs](self)). Carries the filter name.
    UnsupportedFilter(String),
    /// Any other failure: missing/invalid `/Width`/`/Height`, oversized
    /// dimensions, a filter decode error (corrupt JPEG, unsupported CCITT
    /// `K >= 0`, ...), a color space this phase can't resolve, or a
    /// decoded byte count that doesn't match the declared geometry.
    Failed(String),
}

fn dict_get<'a>(dict: &'a PdfDictionary, full: &str, abbrev: &str) -> Option<&'a Object> {
    dict.get(full).or_else(|| dict.get(abbrev))
}

fn dict_get_int(dict: &PdfDictionary, full: &str, abbrev: &str) -> Option<i64> {
    dict_get(dict, full, abbrev).and_then(Object::as_integer)
}

fn as_f64(o: &Object) -> Option<f64> {
    o.as_real().filter(|v| v.is_finite())
}

fn pairs(dict: &PdfDictionary, full: &str, abbrev: &str) -> Option<Vec<(f64, f64)>> {
    let arr = dict_get(dict, full, abbrev)?.as_array()?;
    let flat: Vec<f64> = arr.iter().filter_map(as_f64).collect();
    Some(flat.chunks_exact(2).map(|c| (c[0], c[1])).collect())
}

/// Filter names declared by `/Filter` (or inline images' abbreviated
/// `/F`), as a `Vec` regardless of whether the dictionary entry was a bare
/// `Name` or an `Array` of them (ISO 32000-1 §7.4).
fn filter_name_list(dict: &PdfDictionary) -> Vec<String> {
    match dict_get(dict, "Filter", "F") {
        Some(Object::Name(n)) => vec![n.as_str().to_string()],
        Some(Object::Array(arr)) => arr
            .iter()
            .filter_map(|o| match o {
                Object::Name(n) => Some(n.as_str().to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Returns the first filter name that this crate has no decoder for at
/// all (`JBIG2Decode`/`JPXDecode` -- see the [module docs](self)), if any.
fn unsupported_filter(dict: &PdfDictionary) -> Option<String> {
    filter_name_list(dict)
        .into_iter()
        .find(|f| f == "JBIG2Decode" || f == "JPXDecode")
}

/// Decodes an image XObject stream (ISO 32000-1 §8.9.5) into paintable
/// pixels. `fill_color` is the current non-stroking colour, used only if
/// the image is `/ImageMask true`.
pub(super) fn decode_image_xobject(
    stream: &PdfStream,
    resources: Option<&PdfDictionary>,
    fill_color: Color,
) -> ImageResult {
    if let Some(filter) = unsupported_filter(&stream.dictionary) {
        return ImageResult::UnsupportedFilter(filter);
    }
    let data = match decode_all(stream) {
        Ok(d) => d,
        Err(e) => return ImageResult::Failed(e),
    };
    decode_pixels(&stream.dictionary, &data, resources, fill_color)
}

/// Decodes an inline image (ISO 32000-1 §8.9.7) into paintable pixels.
pub(super) fn decode_inline_image(
    img: &InlineImage,
    resources: Option<&PdfDictionary>,
    fill_color: Color,
) -> ImageResult {
    if let Some(filter) = unsupported_filter(&img.dictionary) {
        return ImageResult::UnsupportedFilter(filter);
    }
    // `PdfStream::decode_all` (reused below, per this crate's reuse-don't-
    // rewrite rule) only recognises the full `/Filter`/`/DecodeParms` key
    // names -- it has no idea inline images may abbreviate them as `/F`/
    // `/DP` (ISO 32000-1 Table 92). Normalize just those two keys onto a
    // copy of the dictionary before decoding so the *same* filter dispatch
    // logic `decode_image_xobject` uses applies here too, rather than
    // duplicating the filter-chain loop.
    let stream = PdfStream::from_raw(normalize_inline_dict(&img.dictionary), img.data.clone());
    let data = match decode_all(&stream) {
        Ok(d) => d,
        Err(e) => return ImageResult::Failed(e),
    };
    decode_pixels(&stream.dictionary, &data, resources, fill_color)
}

/// Copies `dict`, filling in the full `/Filter`/`/DecodeParms` keys from
/// their inline-image-only abbreviated forms (`/F`/`/DP`) if the full key
/// isn't already present, so [`PdfStream::decode_all`] (which only looks
/// for the full names) works unmodified for inline images too.
fn normalize_inline_dict(dict: &PdfDictionary) -> PdfDictionary {
    let mut out = dict.clone();
    if !out.contains_key("Filter") {
        if let Some(f) = dict.get("F").cloned() {
            out.set("Filter", f);
        }
    }
    if !out.contains_key("DecodeParms") {
        if let Some(dp) = dict.get("DP").cloned() {
            out.set("DecodeParms", dp);
        }
    }
    out
}

/// Applies `stream`'s `/Filter` chain, if any (reused by
/// [`super::interpreter`] for decoding a Form XObject's content stream --
/// the same "already-filter-decoded bytes" contract images need applies
/// there too).
#[cfg(feature = "compression")]
pub(super) fn decode_all(stream: &PdfStream) -> Result<Vec<u8>, String> {
    stream.decode_all().map_err(|e| e.to_string())
}

#[cfg(not(feature = "compression"))]
pub(super) fn decode_all(stream: &PdfStream) -> Result<Vec<u8>, String> {
    if stream.dictionary.get("Filter").is_none() && stream.dictionary.get("F").is_none() {
        Ok(stream.data.clone())
    } else {
        Err("image filter decoding requires the `compression` feature".to_string())
    }
}

/// Shared pixel-unpacking logic for both XObject and inline images, once
/// `data` has already had its `/Filter` chain applied.
fn decode_pixels(
    dict: &PdfDictionary,
    data: &[u8],
    resources: Option<&PdfDictionary>,
    fill_color: Color,
) -> ImageResult {
    let width = match dict_get_int(dict, "Width", "W") {
        Some(w) if w > 0 && w <= u32::MAX as i64 => w as u32,
        other => return ImageResult::Failed(format!("missing or invalid /Width: {other:?}")),
    };
    let height = match dict_get_int(dict, "Height", "H") {
        Some(h) if h > 0 && h <= u32::MAX as i64 => h as u32,
        other => return ImageResult::Failed(format!("missing or invalid /Height: {other:?}")),
    };
    let total_pixels = u64::from(width) * u64::from(height);
    if total_pixels == 0 || total_pixels > MAX_IMAGE_PIXELS {
        return ImageResult::Failed(format!(
            "image dimensions {width}x{height} rejected (exceeds {MAX_IMAGE_PIXELS} px bound)"
        ));
    }

    let is_mask = matches!(dict_get(dict, "ImageMask", "IM"), Some(Object::Boolean(true)));
    let decode_array = pairs(dict, "Decode", "D");

    if is_mask {
        return decode_image_mask(width, height, data, decode_array, fill_color);
    }

    let bpc = match dict_get_int(dict, "BitsPerComponent", "BPC") {
        Some(v) if [1, 2, 4, 8, 16].contains(&v) => v as u32,
        Some(v) => return ImageResult::Failed(format!("unsupported /BitsPerComponent {v}")),
        None => 8,
    };

    let Some(cs_obj) = dict_get(dict, "ColorSpace", "CS") else {
        return ImageResult::Failed("missing /ColorSpace".to_string());
    };
    let color_space = colorspace::resolve_color_space(cs_obj, resources);
    if color_space.is_unsupported() {
        return ImageResult::Failed(format!("unsupported image colour space: {}", color_space.description()));
    }

    let n_comp = color_space.components().max(1);
    let row_bits = (width as u64) * n_comp as u64 * u64::from(bpc);
    let row_bytes = row_bits.div_ceil(8) as usize;
    let expected_len = row_bytes.saturating_mul(height as usize);
    if data.len() < expected_len {
        return ImageResult::Failed(format!(
            "decoded image data too short: {} bytes, need {expected_len} for {width}x{height}x{n_comp}@{bpc}bpc",
            data.len()
        ));
    }

    let decode = decode_array.unwrap_or_else(|| colorspace::default_decode(&color_space, bpc));
    if decode.len() < n_comp {
        return ImageResult::Failed("/Decode array shorter than colour space component count".to_string());
    }

    let Some(mut rgba) = unpack_color_rows(width, height, row_bytes, bpc, n_comp, data, &decode, &color_space) else {
        return ImageResult::Failed("colour space could not produce a colour for at least one pixel".to_string());
    };

    // ISO 32000-1 §11.6.5.3: an image XObject's own `/SMask` entry (a
    // separate DeviceGray stream) supplies this image's per-pixel alpha.
    // A decode failure here doesn't fail the whole image -- it paints
    // fully opaque instead, with the reason surfaced to the caller (see
    // [module docs](self)).
    let smask_warning = match dict.get("SMask") {
        None => None,
        Some(Object::Stream(smask_stream)) => apply_image_smask(&mut rgba, width, height, smask_stream).err(),
        Some(_) => Some("/SMask present but not a stream".to_string()),
    };

    // Straight-alpha samples above must become premultiplied before
    // `tiny-skia` (which requires premultiplied RGBA8) sees them. A
    // no-op whenever alpha is uniformly 255 (no `/SMask`, the common
    // case), so this is safe to always apply rather than branching.
    premultiply_rgba(&mut rgba);

    ImageResult::Ok(DecodedImage {
        width,
        height,
        rgba,
        smask_warning,
    })
}

/// Decodes an image XObject's `/SMask` entry (ISO 32000-1 §11.6.5.3) and
/// applies it as `base_rgba`'s alpha channel, resampling with
/// nearest-neighbor (a documented simplification, not bilinear) if the
/// mask's dimensions differ from the base image's. Returns `Err` (leaving
/// `base_rgba` untouched -- still fully opaque) if the mask can't be
/// decoded at all.
fn apply_image_smask(base_rgba: &mut [u8], base_w: u32, base_h: u32, smask_stream: &PdfStream) -> Result<(), String> {
    if let Some(filter) = unsupported_filter(&smask_stream.dictionary) {
        return Err(format!("uses unsupported filter {filter}"));
    }
    let data = decode_all(smask_stream)?;
    let (mask_w, mask_h, gray) = decode_gray_mask_samples(&smask_stream.dictionary, &data)?;

    for row in 0..base_h as usize {
        let my = ((row * mask_h as usize) / base_h as usize).min(mask_h as usize - 1);
        for col in 0..base_w as usize {
            let mx = ((col * mask_w as usize) / base_w as usize).min(mask_w as usize - 1);
            let g = gray[my * mask_w as usize + mx];
            base_rgba[(row * base_w as usize + col) * 4 + 3] = g;
        }
    }
    Ok(())
}

/// Decodes a DeviceGray mask image's samples (used by `/SMask`, ISO
/// 32000-1 §11.6.5.3) into a flat `width*height` array of 0..255 gray
/// values, applying `/Decode` (default `[0 1]`, so a raw sample of `0`
/// means fully transparent and the max value means fully opaque -- per
/// spec's default for this specific entry, the *opposite* sense of
/// `/ImageMask`'s default). Ignores any `/ColorSpace` entry the mask
/// stream might carry (ISO 32000-1 requires `/SMask` images be
/// DeviceGray unconditionally).
fn decode_gray_mask_samples(dict: &PdfDictionary, data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let width = match dict_get_int(dict, "Width", "W") {
        Some(w) if w > 0 && w <= u32::MAX as i64 => w as u32,
        other => return Err(format!("missing or invalid /Width: {other:?}")),
    };
    let height = match dict_get_int(dict, "Height", "H") {
        Some(h) if h > 0 && h <= u32::MAX as i64 => h as u32,
        other => return Err(format!("missing or invalid /Height: {other:?}")),
    };
    let total_pixels = u64::from(width) * u64::from(height);
    if total_pixels == 0 || total_pixels > MAX_IMAGE_PIXELS {
        return Err(format!("dimensions {width}x{height} rejected (exceeds {MAX_IMAGE_PIXELS} px bound)"));
    }

    let bpc = match dict_get_int(dict, "BitsPerComponent", "BPC") {
        Some(v) if [1, 2, 4, 8, 16].contains(&v) => v as u32,
        Some(v) => return Err(format!("unsupported /BitsPerComponent {v}")),
        None => 8,
    };
    let row_bytes = ((u64::from(width)) * u64::from(bpc)).div_ceil(8) as usize;
    let expected_len = row_bytes.saturating_mul(height as usize);
    if data.len() < expected_len {
        return Err(format!("decoded data too short: {} bytes, need {expected_len}", data.len()));
    }

    let decode = pairs(dict, "Decode", "D").unwrap_or_default();
    let (d0, d1) = *decode.first().unwrap_or(&(0.0, 1.0));
    let max_raw = ((1u64 << bpc) - 1) as f64;

    let mut gray = vec![0u8; width as usize * height as usize];
    for row in 0..height as usize {
        let row_data = &data[row * row_bytes..row * row_bytes + row_bytes];
        let mut reader = BitReader::new(row_data);
        for col in 0..width as usize {
            let raw = reader.read_bits(bpc);
            let v = if max_raw > 0.0 { d0 + f64::from(raw) * (d1 - d0) / max_raw } else { d0 };
            gray[row * width as usize + col] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    Ok((width, height, gray))
}

/// Premultiplies straight-alpha RGBA8 samples in place (`c' = c*a/255`),
/// required before handing pixel data to `tiny-skia::Pixmap::from_vec`
/// (which requires premultiplied input). A no-op wherever alpha is
/// already 255 (the `c*255/255 == c` identity), so it's safe to call
/// unconditionally rather than only when an `/SMask` was actually applied.
fn premultiply_rgba(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = u16::from(px[3]);
        px[0] = ((u16::from(px[0]) * a) / 255) as u8;
        px[1] = ((u16::from(px[1]) * a) / 255) as u8;
        px[2] = ((u16::from(px[2]) * a) / 255) as u8;
    }
}

/// Unpacks and colour-converts every pixel; returns `None` (aborting the
/// whole image, rather than fabricating a partially-wrong raster) if the
/// colour space fails to produce a colour for any single pixel.
#[allow(clippy::too_many_arguments)]
fn unpack_color_rows(
    width: u32,
    height: u32,
    row_bytes: usize,
    bpc: u32,
    n_comp: usize,
    data: &[u8],
    decode: &[(f64, f64)],
    color_space: &ColorSpace,
) -> Option<Vec<u8>> {
    let max_raw = ((1u64 << bpc) - 1) as f64;
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    let mut comps = vec![0f64; n_comp];

    for row in 0..height as usize {
        let row_data = &data[row * row_bytes..row * row_bytes + row_bytes];
        let mut reader = BitReader::new(row_data);
        for col in 0..width as usize {
            for (i, c) in comps.iter_mut().enumerate() {
                let raw = reader.read_bits(bpc);
                let (d0, d1) = decode[i];
                *c = if max_raw > 0.0 { d0 + (f64::from(raw)) * (d1 - d0) / max_raw } else { d0 };
            }
            let color = color_space.to_rgba(&comps, 1.0)?;
            let idx = (row * width as usize + col) * 4;
            rgba[idx] = (color.red() * 255.0).round() as u8;
            rgba[idx + 1] = (color.green() * 255.0).round() as u8;
            rgba[idx + 2] = (color.blue() * 255.0).round() as u8;
            rgba[idx + 3] = 255;
        }
    }
    Some(rgba)
}

/// Decodes an `/ImageMask true` stencil (ISO 32000-1 §8.9.6.2): 1 bit per
/// pixel, painted with `fill_color` where the sample (after `/Decode`)
/// is `0`, transparent where it is `1`. `/Decode [1 0]` inverts this.
fn decode_image_mask(
    width: u32,
    height: u32,
    data: &[u8],
    decode: Option<Vec<(f64, f64)>>,
    fill_color: Color,
) -> ImageResult {
    let row_bytes = (u64::from(width)).div_ceil(8) as usize;
    let expected_len = row_bytes.saturating_mul(height as usize);
    if data.len() < expected_len {
        return ImageResult::Failed(format!(
            "decoded image-mask data too short: {} bytes, need {expected_len}",
            data.len()
        ));
    }

    let invert = matches!(decode.as_deref(), Some([(d0, _)]) if *d0 > 0.5);
    let r = (fill_color.red() * 255.0).round() as u8;
    let g = (fill_color.green() * 255.0).round() as u8;
    let b = (fill_color.blue() * 255.0).round() as u8;

    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for row in 0..height as usize {
        let row_data = &data[row * row_bytes..row * row_bytes + row_bytes];
        let mut reader = BitReader::new(row_data);
        for col in 0..width as usize {
            let bit = reader.read_bits(1);
            let paint = if invert { bit == 1 } else { bit == 0 };
            let idx = (row * width as usize + col) * 4;
            if paint {
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 255;
            }
            // else: leave as the zero-initialized fully transparent pixel.
        }
    }
    ImageResult::Ok(DecodedImage {
        width,
        height,
        rgba,
        smask_warning: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{PdfArray, PdfName, PdfString};

    fn name(s: &str) -> Object {
        Object::Name(PdfName::new_unchecked(s))
    }

    fn gray_image_dict(width: i64, height: i64, bpc: i64) -> PdfDictionary {
        let mut dict = PdfDictionary::new();
        dict.set("Width", Object::Integer(width));
        dict.set("Height", Object::Integer(height));
        dict.set("BitsPerComponent", Object::Integer(bpc));
        dict.set("ColorSpace", name("DeviceGray"));
        dict
    }

    #[test]
    fn decodes_uncompressed_2x1_gray_image() {
        let dict = gray_image_dict(2, 1, 8);
        let stream = PdfStream::with_dictionary(dict, vec![0x00, 0xFF]);
        let result = decode_image_xobject(&stream, None, Color::BLACK);
        match result {
            ImageResult::Ok(img) => {
                assert_eq!((img.width, img.height), (2, 1));
                assert_eq!(&img.rgba[0..4], &[0, 0, 0, 255]);
                assert_eq!(&img.rgba[4..8], &[255, 255, 255, 255]);
            }
            ImageResult::Failed(e) => panic!("expected Ok, got Failed: {e}"),
            ImageResult::UnsupportedFilter(f) => panic!("expected Ok, got UnsupportedFilter: {f}"),
        }
    }

    #[test]
    fn decodes_1bpc_image_mask_with_fill_color() {
        let mut dict = PdfDictionary::new();
        dict.set("Width", Object::Integer(8));
        dict.set("Height", Object::Integer(1));
        dict.set("ImageMask", Object::Boolean(true));
        // 0b1010_0000: bits 0 and 2 (from MSB) are "paint" (0), rest masked.
        // Actually with default Decode [0 1]: bit==0 -> paint.
        let stream = PdfStream::with_dictionary(dict, vec![0b1010_0000]);
        let result = decode_image_xobject(&stream, None, Color::from_rgba8(10, 20, 30, 255));
        match result {
            ImageResult::Ok(img) => {
                assert_eq!(&img.rgba[0..4], &[0, 0, 0, 0], "bit=1 -> masked out (transparent)");
                assert_eq!(&img.rgba[4..8], &[10, 20, 30, 255], "bit=0 -> painted with fill color");
            }
            other => panic!("expected Ok, got {}", describe(&other)),
        }
    }

    #[test]
    fn jbig2_filter_yields_structured_unsupported_result_not_panic() {
        let mut dict = PdfDictionary::new();
        dict.set("Width", Object::Integer(10));
        dict.set("Height", Object::Integer(10));
        dict.set("BitsPerComponent", Object::Integer(1));
        dict.set("ColorSpace", name("DeviceGray"));
        dict.set("Filter", name("JBIG2Decode"));
        let stream = PdfStream::with_dictionary(dict, vec![0u8; 4]);
        let result = decode_image_xobject(&stream, None, Color::BLACK);
        match result {
            ImageResult::UnsupportedFilter(f) => assert_eq!(f, "JBIG2Decode"),
            other => panic!("expected UnsupportedFilter, got {}", describe(&other)),
        }
    }

    #[test]
    fn jpx_filter_yields_structured_unsupported_result_not_panic() {
        let mut dict = PdfDictionary::new();
        dict.set("Width", Object::Integer(10));
        dict.set("Height", Object::Integer(10));
        dict.set("ColorSpace", name("DeviceRGB"));
        dict.set("Filter", name("JPXDecode"));
        let stream = PdfStream::with_dictionary(dict, vec![0u8; 4]);
        let result = decode_image_xobject(&stream, None, Color::BLACK);
        assert!(matches!(result, ImageResult::UnsupportedFilter(f) if f == "JPXDecode"));
    }

    #[test]
    fn oversized_dimensions_are_rejected_before_allocating() {
        let mut dict = PdfDictionary::new();
        dict.set("Width", Object::Integer(100_000));
        dict.set("Height", Object::Integer(100_000));
        dict.set("BitsPerComponent", Object::Integer(8));
        dict.set("ColorSpace", name("DeviceGray"));
        let stream = PdfStream::with_dictionary(dict, vec![0u8; 16]);
        let result = decode_image_xobject(&stream, None, Color::BLACK);
        assert!(matches!(result, ImageResult::Failed(_)));
    }

    #[test]
    fn truncated_data_is_a_failure_not_a_panic() {
        let dict = gray_image_dict(4, 4, 8);
        // Way too little data for a 4x4x1x8bpc image (needs 16 bytes).
        let stream = PdfStream::with_dictionary(dict, vec![0u8; 2]);
        let result = decode_image_xobject(&stream, None, Color::BLACK);
        assert!(matches!(result, ImageResult::Failed(_)));
    }

    #[test]
    fn indexed_image_resolves_palette() {
        let lookup = PdfString::literal_bytes(vec![255u8, 0, 0, 0, 0, 255]);
        let cs = Object::Array(PdfArray::from_objects(vec![
            name("Indexed"),
            name("DeviceRGB"),
            Object::Integer(1),
            Object::String(lookup),
        ]));
        let mut dict = PdfDictionary::new();
        dict.set("Width", Object::Integer(2));
        dict.set("Height", Object::Integer(1));
        dict.set("BitsPerComponent", Object::Integer(8));
        dict.set("ColorSpace", cs);
        let stream = PdfStream::with_dictionary(dict, vec![0u8, 1u8]);
        let result = decode_image_xobject(&stream, None, Color::BLACK);
        match result {
            ImageResult::Ok(img) => {
                assert_eq!(&img.rgba[0..4], &[255, 0, 0, 255]);
                assert_eq!(&img.rgba[4..8], &[0, 0, 255, 255]);
            }
            other => panic!("expected Ok, got {}", describe(&other)),
        }
    }

    fn describe(r: &ImageResult) -> String {
        match r {
            ImageResult::Ok(_) => "Ok".to_string(),
            ImageResult::UnsupportedFilter(f) => format!("UnsupportedFilter({f})"),
            ImageResult::Failed(e) => format!("Failed({e})"),
        }
    }

    fn gray_smask_stream(width: i64, height: i64, samples: Vec<u8>) -> Object {
        let mut dict = PdfDictionary::new();
        dict.set("Width", Object::Integer(width));
        dict.set("Height", Object::Integer(height));
        dict.set("BitsPerComponent", Object::Integer(8));
        dict.set("ColorSpace", name("DeviceGray"));
        Object::Stream(PdfStream::with_dictionary(dict, samples))
    }

    /// ISO 32000-1 §11.6.5.3: a valid `/SMask` becomes this image's
    /// per-pixel alpha (straight, i.e. matching the raw gray sample once
    /// scaled to 0..255 -- premultiplication happens afterward).
    #[test]
    fn smask_becomes_base_image_alpha() {
        let mut dict = gray_image_dict(2, 1, 8);
        // Force a non-gray base colour so premultiplication is visible:
        // pure red at both pixels.
        dict.set("ColorSpace", name("DeviceRGB"));
        dict.set("SMask", gray_smask_stream(2, 1, vec![0, 255]));
        let stream = PdfStream::with_dictionary(dict, vec![255, 0, 0, 255, 0, 0]);
        let result = decode_image_xobject(&stream, None, Color::BLACK);
        match result {
            ImageResult::Ok(img) => {
                assert!(img.smask_warning.is_none(), "unexpected smask warning: {:?}", img.smask_warning);
                // Pixel 0: alpha 0 -> premultiplied to fully transparent black.
                assert_eq!(&img.rgba[0..4], &[0, 0, 0, 0]);
                // Pixel 1: alpha 255 -> unchanged opaque red.
                assert_eq!(&img.rgba[4..8], &[255, 0, 0, 255]);
            }
            other => panic!("expected Ok, got {}", describe(&other)),
        }
    }

    /// `/SMask` with different dimensions than the base image is
    /// nearest-neighbor resampled (a documented simplification), not
    /// rejected.
    #[test]
    fn smask_with_mismatched_dimensions_is_resampled() {
        let mut dict = gray_image_dict(4, 1, 8);
        dict.set("ColorSpace", name("DeviceRGB"));
        // A 2x1 mask covering a 4x1 base image: samples [0,255] resample
        // to columns [0,0,1,1] under nearest-neighbor.
        dict.set("SMask", gray_smask_stream(2, 1, vec![0, 255]));
        let stream = PdfStream::with_dictionary(
            dict,
            vec![255, 0, 0, /**/ 255, 0, 0, /**/ 255, 0, 0, /**/ 255, 0, 0],
        );
        let result = decode_image_xobject(&stream, None, Color::BLACK);
        match result {
            ImageResult::Ok(img) => {
                assert!(img.smask_warning.is_none());
                assert_eq!(img.rgba[3], 0, "col 0 should map to mask sample 0");
                assert_eq!(img.rgba[7], 0, "col 1 should map to mask sample 0");
                assert_eq!(img.rgba[11], 255, "col 2 should map to mask sample 255");
                assert_eq!(img.rgba[15], 255, "col 3 should map to mask sample 255");
            }
            other => panic!("expected Ok, got {}", describe(&other)),
        }
    }

    /// Adversarial/corrupt input: an `/SMask` entry that isn't a stream at
    /// all doesn't fail the whole image -- it paints fully opaque and
    /// records a reason via `smask_warning`.
    #[test]
    fn malformed_smask_entry_falls_back_to_opaque_with_warning() {
        let mut dict = gray_image_dict(2, 1, 8);
        dict.set("SMask", Object::Integer(42)); // not a stream
        let stream = PdfStream::with_dictionary(dict, vec![0x00, 0xFF]);
        let result = decode_image_xobject(&stream, None, Color::BLACK);
        match result {
            ImageResult::Ok(img) => {
                assert!(img.smask_warning.is_some());
                assert_eq!(img.rgba[3], 255, "base image should still be fully opaque");
                assert_eq!(img.rgba[7], 255);
            }
            other => panic!("expected Ok, got {}", describe(&other)),
        }
    }

    /// Adversarial/corrupt input: an `/SMask` stream with a declared size
    /// that doesn't match its actual (short) data doesn't panic -- it's
    /// treated the same as "no usable soft mask" (opaque base image, plus
    /// a recorded reason).
    #[test]
    fn truncated_smask_data_falls_back_to_opaque_with_warning() {
        let mut dict = gray_image_dict(2, 1, 8);
        dict.set("SMask", gray_smask_stream(4, 4, vec![0u8; 2])); // needs 16 bytes
        let stream = PdfStream::with_dictionary(dict, vec![0x00, 0xFF]);
        let result = decode_image_xobject(&stream, None, Color::BLACK);
        match result {
            ImageResult::Ok(img) => {
                assert!(img.smask_warning.is_some());
                assert_eq!(img.rgba[3], 255);
                assert_eq!(img.rgba[7], 255);
            }
            other => panic!("expected Ok, got {}", describe(&other)),
        }
    }
}
