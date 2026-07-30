//! Colour space resolution and sample-to-RGB conversion beyond the three
//! Device colour spaces (ISO 32000-1:2008 §8.6 "Colour Spaces"), used by
//! the `cs`/`CS`/`sc`/`SC`/`scn`/`SCN` operators
//! ([`super::interpreter`]) and by image sample decoding
//! ([`super::image`]).
//!
//! # Scope of this phase
//!
//! - **Indexed** (§8.6.6.3): a base colour space plus a lookup table,
//!   resolved recursively (an `Indexed` image is a very common encoding
//!   for palette/GIF-like scanned content).
//! - **Separation and DeviceN** (§8.6.6.4/§8.6.6.5): an alternate colour
//!   space plus a tint-transform [`super::function::PdfFunction`]
//!   (Types 0/2/3/4 -- see that module's docs). Evaluated for real, not
//!   approximated.
//! - **ICCBased** (§8.6.5.5): **there is no mature pure-Rust ICC colour
//!   management engine this crate could adopt** (this is an explicit,
//!   documented gap, not a silently-claimed feature). This module
//!   approximates an ICCBased space by resolving its `/Alternate` entry
//!   if present, or -- if not -- a heuristic guess from its `/N`
//!   (component count: 1 -> DeviceGray, 3 -> DeviceRGB, 4 -> DeviceCMYK).
//!   Both paths are wrapped in [`ColorSpace::IccApproximated`] so callers
//!   can distinguish "genuinely accurate" from "approximated" and record
//!   [`super::error::RenderWarning::IccColorApproximated`] rather than
//!   silently claiming ICC accuracy this phase does not have.
//! - **CalGray/CalRGB** are treated as their Device equivalent (no gamma/
//!   white-point calibration applied) -- a common, minor approximation,
//!   not specially warned about.
//! - **Lab and Pattern** colour spaces, and any unrecognised colour space
//!   family, resolve to [`ColorSpace::Unsupported`] (Lab) or
//!   [`ColorSpace::Pattern`] (Pattern) -- callers skip painting and record
//!   [`super::error::RenderWarning::UnsupportedColorSpace`] /
//!   [`super::error::RenderWarning::PatternColorUnsupported`] rather than
//!   guessing a colour.
//!
//! # Untrusted input handling
//!
//! Colour space resolution recurses (an `Indexed` base can itself be an
//! `ICCBased`/`Separation`/...) and is bounded by
//! [`MAX_COLORSPACE_DEPTH`]. Indexed lookup-table sizes and `/hival` are
//! bounds-checked before indexing; anything malformed enough to not fit
//! this module's expectations resolves to [`ColorSpace::Unsupported`]
//! rather than panicking or indexing out of bounds.

use crate::object::{Object, PdfArray, PdfDictionary};

use super::function::{parse_function, PdfFunction};

/// Bound on colour-space nesting (`Indexed` base, `ICCBased` alternate,
/// ...) to defend against a pathologically/adversarially deep chain.
const MAX_COLORSPACE_DEPTH: usize = 12;

/// Bound on a Separation/DeviceN colourant count (`/DeviceN`'s names
/// array length), so a crafted huge names array can't force an
/// oversized per-pixel component buffer.
const MAX_TINT_COMPONENTS: usize = 32;

/// Bound on `Indexed`'s `/HiVal` (ISO 32000-1 §8.6.6.3 caps this at 255
/// for any *sensible* bits-per-component, but this module is defensive
/// against a crafted higher value regardless).
const MAX_INDEXED_HIVAL: i64 = 65_535;

/// A resolved colour space, ready to convert raw (already `/Decode`-array-
/// mapped, for images) or operand (for `sc`/`scn`) component values into an
/// RGB [`tiny_skia::Color`]. See the [module docs](self) for exactly what
/// each variant does and does not implement.
#[derive(Debug)]
pub(super) enum ColorSpace {
    DeviceGray,
    DeviceRGB,
    DeviceCMYK,
    /// ISO 32000-1 §8.6.6.3: `base` is resolved recursively; `lookup` is
    /// `(hival+1) * base.components()` bytes, one 0..=255 byte per base
    /// component per palette entry.
    Indexed {
        base: Box<ColorSpace>,
        hival: u32,
        lookup: Vec<u8>,
    },
    /// ISO 32000-1 §8.6.6.4 (Separation, `n == 1`) / §8.6.6.5 (DeviceN,
    /// `n` == the colourant count).
    TintTransform {
        n: usize,
        alternate: Box<ColorSpace>,
        function: Box<PdfFunction>,
    },
    /// An ICCBased space this phase approximates rather than colour-manages
    /// -- see the [module docs](self). `inner` is either the resolved
    /// `/Alternate` or the `/N`-based heuristic guess.
    IccApproximated { inner: Box<ColorSpace> },
    /// `/Pattern` (with or without an underlying colour space for
    /// uncoloured patterns) -- out of scope this phase (§8.7). `scn`/`SCN`
    /// naming a pattern leaves the current colour unchanged.
    Pattern,
    /// Anything this module could not resolve: an unrecognised colour
    /// space family (e.g. `/Lab`), a malformed colour space array, an
    /// unresolvable `/Resources /ColorSpace` name, or a Separation/DeviceN
    /// whose tint-transform function this crate's function evaluator (see
    /// [`super::function`]) could not parse. The `String` is a
    /// human-readable reason, used only for diagnostics/warnings.
    Unsupported(String),
}

impl ColorSpace {
    /// Number of input colour components this space's `sc`/`scn`/image
    /// samples carry (e.g. 3 for DeviceRGB, 1 for Indexed's palette index,
    /// `n` for a DeviceN with `n` colourants).
    pub(super) fn components(&self) -> usize {
        match self {
            ColorSpace::DeviceGray => 1,
            ColorSpace::DeviceRGB => 3,
            ColorSpace::DeviceCMYK => 4,
            ColorSpace::Indexed { .. } => 1,
            ColorSpace::TintTransform { n, .. } => *n,
            ColorSpace::IccApproximated { inner } => inner.components(),
            ColorSpace::Pattern | ColorSpace::Unsupported(_) => 0,
        }
    }

    /// The initial colour value `cs`/`CS` resets the current colour to
    /// (ISO 32000-1 §8.6.3's black default for Device/CalGray/CalRGB/
    /// Indexed-index-0, and §8.6.6.4's "initial colour value of 1.0 for
    /// each colourant" for Separation/DeviceN).
    pub(super) fn initial_components(&self) -> Vec<f64> {
        match self {
            ColorSpace::DeviceGray => vec![0.0],
            ColorSpace::DeviceRGB => vec![0.0, 0.0, 0.0],
            ColorSpace::DeviceCMYK => vec![0.0, 0.0, 0.0, 1.0],
            ColorSpace::Indexed { .. } => vec![0.0],
            ColorSpace::TintTransform { n, .. } => vec![1.0; *n],
            ColorSpace::IccApproximated { inner } => inner.initial_components(),
            ColorSpace::Pattern | ColorSpace::Unsupported(_) => Vec::new(),
        }
    }

    /// Converts already-decoded component values (in each space's own
    /// native range -- `0.0..=1.0` per Device/tint component, a raw
    /// palette index for `Indexed`) plus a constant alpha into an RGB
    /// [`tiny_skia::Color`]. Returns `None` if this space cannot produce a
    /// colour at all ([`ColorSpace::Pattern`]/[`ColorSpace::Unsupported`])
    /// or if a Separation/DeviceN's tint-transform function failed to
    /// evaluate for this specific input (rare: the function parsed
    /// successfully at colour-space-resolution time, but a PostScript
    /// calculator function can still fail per-input, e.g. a stack
    /// underflow inside an `ifelse` branch that isn't taken for most
    /// inputs).
    pub(super) fn to_rgba(&self, comps: &[f64], alpha: f32) -> Option<tiny_skia::Color> {
        match self {
            ColorSpace::DeviceGray => comps.first().map(|&g| super::color::device_gray(g, alpha)),
            ColorSpace::DeviceRGB => {
                if comps.len() >= 3 {
                    Some(super::color::device_rgb(comps[0], comps[1], comps[2], alpha))
                } else {
                    None
                }
            }
            ColorSpace::DeviceCMYK => {
                if comps.len() >= 4 {
                    Some(super::color::device_cmyk(comps[0], comps[1], comps[2], comps[3], alpha))
                } else {
                    None
                }
            }
            ColorSpace::Indexed { base, hival, lookup } => {
                let idx = comps.first().copied().unwrap_or(0.0).round();
                if !idx.is_finite() || idx < 0.0 {
                    return None;
                }
                let idx = (idx as u64).min(*hival as u64) as usize;
                let n = base.components();
                let start = idx.checked_mul(n)?;
                let end = start.checked_add(n)?;
                if end > lookup.len() {
                    return None;
                }
                let base_comps: Vec<f64> = lookup[start..end].iter().map(|&b| f64::from(b) / 255.0).collect();
                base.to_rgba(&base_comps, alpha)
            }
            ColorSpace::TintTransform { alternate, function, .. } => {
                let out = function.eval(comps)?;
                alternate.to_rgba(&out, alpha)
            }
            ColorSpace::IccApproximated { inner } => inner.to_rgba(comps, alpha),
            ColorSpace::Pattern | ColorSpace::Unsupported(_) => None,
        }
    }

    /// Whether this space is one this module could not resolve into
    /// something paintable (used by callers to decide whether to record
    /// [`super::error::RenderWarning::UnsupportedColorSpace`]).
    pub(super) fn is_unsupported(&self) -> bool {
        matches!(self, ColorSpace::Unsupported(_))
    }

    /// A human-readable name for diagnostics/warnings.
    pub(super) fn description(&self) -> String {
        match self {
            ColorSpace::DeviceGray => "DeviceGray".to_string(),
            ColorSpace::DeviceRGB => "DeviceRGB".to_string(),
            ColorSpace::DeviceCMYK => "DeviceCMYK".to_string(),
            ColorSpace::Indexed { .. } => "Indexed".to_string(),
            ColorSpace::TintTransform { n, .. } => format!("Separation/DeviceN({n})"),
            ColorSpace::IccApproximated { inner } => format!("ICCBased(~{})", inner.description()),
            ColorSpace::Pattern => "Pattern".to_string(),
            ColorSpace::Unsupported(reason) => reason.clone(),
        }
    }
}

fn object_dict(obj: &Object) -> Option<&PdfDictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dictionary),
        _ => None,
    }
}

fn lookup_bytes(obj: &Object) -> Vec<u8> {
    match obj {
        Object::String(s) => s.as_bytes().to_vec(),
        Object::Stream(s) => {
            #[cfg(feature = "compression")]
            {
                s.decode_all().unwrap_or_default()
            }
            #[cfg(not(feature = "compression"))]
            {
                let _ = s;
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// Resolves a `cs`/`CS`/image `/ColorSpace` value (ISO 32000-1 §8.6) into a
/// [`ColorSpace`], consulting `resources`' `/ColorSpace` subdictionary
/// (ISO 32000-1 §7.8.3) for a named resource. Never panics on malformed
/// input -- falls back to [`ColorSpace::Unsupported`].
pub(super) fn resolve_color_space(obj: &Object, resources: Option<&PdfDictionary>) -> ColorSpace {
    resolve_inner(obj, resources, 0)
}

fn resolve_inner(obj: &Object, resources: Option<&PdfDictionary>, depth: usize) -> ColorSpace {
    if depth > MAX_COLORSPACE_DEPTH {
        return ColorSpace::Unsupported("colour space nesting too deep".to_string());
    }
    match obj {
        Object::Name(n) => match n.as_str() {
            "DeviceGray" | "CalGray" | "G" => ColorSpace::DeviceGray,
            "DeviceRGB" | "CalRGB" | "RGB" => ColorSpace::DeviceRGB,
            "DeviceCMYK" | "CMYK" => ColorSpace::DeviceCMYK,
            "Pattern" => ColorSpace::Pattern,
            other => {
                let looked_up = resources
                    .and_then(|r| r.get("ColorSpace"))
                    .and_then(Object::as_dictionary)
                    .and_then(|csd| csd.get(other));
                match looked_up {
                    Some(o) => resolve_inner(o, resources, depth + 1),
                    None => ColorSpace::Unsupported(format!("unresolvable colour space resource /{other}")),
                }
            }
        },
        Object::Array(arr) => {
            let Some(Object::Name(family)) = arr.get(0) else {
                return ColorSpace::Unsupported("malformed colour space array".to_string());
            };
            match family.as_str() {
                "ICCBased" => {
                    let dict = arr.get(1).and_then(object_dict);
                    if let Some(alt) = dict.and_then(|d| d.get("Alternate")) {
                        ColorSpace::IccApproximated {
                            inner: Box::new(resolve_inner(alt, resources, depth + 1)),
                        }
                    } else {
                        let n = dict.and_then(|d| d.get("N")).and_then(Object::as_integer).unwrap_or(0);
                        let heuristic = match n {
                            1 => ColorSpace::DeviceGray,
                            3 => ColorSpace::DeviceRGB,
                            4 => ColorSpace::DeviceCMYK,
                            _ => {
                                return ColorSpace::Unsupported(format!(
                                    "ICCBased with no /Alternate and unrecognised /N {n}"
                                ))
                            }
                        };
                        ColorSpace::IccApproximated {
                            inner: Box::new(heuristic),
                        }
                    }
                }
                "Indexed" | "I" => {
                    let Some(base_obj) = arr.get(1) else {
                        return ColorSpace::Unsupported("Indexed missing base colour space".to_string());
                    };
                    let base = resolve_inner(base_obj, resources, depth + 1);
                    let hival = arr.get(2).and_then(Object::as_integer).unwrap_or(-1);
                    if !(0..=MAX_INDEXED_HIVAL).contains(&hival) {
                        return ColorSpace::Unsupported(format!("Indexed has invalid /hival {hival}"));
                    }
                    let lookup = arr.get(3).map(lookup_bytes).unwrap_or_default();
                    ColorSpace::Indexed {
                        base: Box::new(base),
                        hival: hival as u32,
                        lookup,
                    }
                }
                "Separation" | "DeviceN" => {
                    let is_separation = family.as_str() == "Separation";
                    let n = if is_separation {
                        1
                    } else {
                        arr.get(1).and_then(Object::as_array).map_or(0, PdfArray::len)
                    };
                    if n == 0 || n > MAX_TINT_COMPONENTS {
                        return ColorSpace::Unsupported(format!("{}: invalid colourant count", family.as_str()));
                    }
                    let Some(alt_obj) = arr.get(2) else {
                        return ColorSpace::Unsupported(format!("{}: missing alternate colour space", family.as_str()));
                    };
                    let alternate = resolve_inner(alt_obj, resources, depth + 1);
                    let Some(func_obj) = arr.get(3) else {
                        return ColorSpace::Unsupported(format!("{}: missing tint transform function", family.as_str()));
                    };
                    match parse_function(func_obj, 0) {
                        Some(f) => ColorSpace::TintTransform {
                            n,
                            alternate: Box::new(alternate),
                            function: Box::new(f),
                        },
                        None => ColorSpace::Unsupported(format!(
                            "{}: unparseable/unsupported tint transform function",
                            family.as_str()
                        )),
                    }
                }
                "CalGray" => ColorSpace::DeviceGray,
                "CalRGB" => ColorSpace::DeviceRGB,
                "Pattern" => ColorSpace::Pattern,
                other => ColorSpace::Unsupported(format!("unsupported colour space family /{other}")),
            }
        }
        _ => ColorSpace::Unsupported("malformed colour space object".to_string()),
    }
}

/// The default `/Decode` array (ISO 32000-1 Table 90) for an image sample
/// in `cs` at `bpc` bits per component, used when the image dictionary has
/// no explicit `/Decode` entry. `Indexed`'s default is `[0, 2^bpc - 1]`
/// (i.e. the raw sample *is* the palette index, unscaled); every other
/// space defaults to `[0, 1]` per component.
pub(super) fn default_decode(cs: &ColorSpace, bpc: u32) -> Vec<(f64, f64)> {
    match cs {
        ColorSpace::Indexed { .. } => vec![(0.0, ((1u64 << bpc) - 1) as f64)],
        ColorSpace::IccApproximated { inner } => default_decode(inner, bpc),
        other => vec![(0.0, 1.0); other.components().max(1)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::PdfName;

    fn name(s: &str) -> Object {
        Object::Name(PdfName::new_unchecked(s))
    }

    #[test]
    fn resolves_device_names() {
        assert!(matches!(resolve_color_space(&name("DeviceGray"), None), ColorSpace::DeviceGray));
        assert!(matches!(resolve_color_space(&name("DeviceRGB"), None), ColorSpace::DeviceRGB));
        assert!(matches!(resolve_color_space(&name("DeviceCMYK"), None), ColorSpace::DeviceCMYK));
    }

    #[test]
    fn resolves_named_resource_via_resources_dict() {
        let mut cs_dict = PdfDictionary::new();
        cs_dict.set("CS0", name("DeviceRGB"));
        let mut resources = PdfDictionary::new();
        resources.set("ColorSpace", Object::Dictionary(cs_dict));
        let resolved = resolve_color_space(&name("CS0"), Some(&resources));
        assert!(matches!(resolved, ColorSpace::DeviceRGB));
    }

    #[test]
    fn unresolvable_resource_name_is_unsupported_not_panic() {
        let resolved = resolve_color_space(&name("NoSuchSpace"), None);
        assert!(resolved.is_unsupported());
    }

    #[test]
    fn indexed_resolves_palette_entry_to_base_rgb() {
        let base = name("DeviceRGB");
        let lookup = vec![255u8, 0, 0, /* index 0 = red */ 0, 255, 0 /* index 1 = green */];
        let arr = PdfArray::from_objects(vec![
            name("Indexed"),
            base,
            Object::Integer(1),
            Object::String(crate::object::PdfString::literal_bytes(lookup)),
        ]);
        let cs = resolve_color_space(&Object::Array(arr), None);
        assert_eq!(cs.components(), 1);
        let red = cs.to_rgba(&[0.0], 1.0).unwrap();
        assert_eq!((red.red(), red.green(), red.blue()), (1.0, 0.0, 0.0));
        let green = cs.to_rgba(&[1.0], 1.0).unwrap();
        assert_eq!((green.red(), green.green(), green.blue()), (0.0, 1.0, 0.0));
    }

    #[test]
    fn indexed_out_of_range_index_is_clamped_to_hival() {
        let lookup = vec![10u8, 20, 30];
        let arr = PdfArray::from_objects(vec![
            name("Indexed"),
            name("DeviceRGB"),
            Object::Integer(0),
            Object::String(crate::object::PdfString::literal_bytes(lookup)),
        ]);
        let cs = resolve_color_space(&Object::Array(arr), None);
        // hival=0 means only index 0 is valid; a huge requested index must
        // not panic (out-of-bounds slice), it should clamp.
        let c = cs.to_rgba(&[999.0], 1.0).unwrap();
        assert_eq!((c.red() * 255.0).round() as u8, 10);
    }

    #[test]
    fn separation_tint_transform_maps_through_alternate() {
        // Separation "Spot" -> DeviceGray via a Type 2 function that maps
        // tint directly to gray (identity: N=1, C0=0, C1=1).
        let mut func_dict = PdfDictionary::new();
        func_dict.set("FunctionType", Object::Integer(2));
        func_dict.set("Domain", Object::Array(PdfArray::from_objects(vec![Object::Real(0.0), Object::Real(1.0)])));
        func_dict.set("C0", Object::Array(PdfArray::from_objects(vec![Object::Real(0.0)])));
        func_dict.set("C1", Object::Array(PdfArray::from_objects(vec![Object::Real(1.0)])));
        func_dict.set("N", Object::Real(1.0));

        let arr = PdfArray::from_objects(vec![
            name("Separation"),
            name("Spot"),
            name("DeviceGray"),
            Object::Dictionary(func_dict),
        ]);
        let cs = resolve_color_space(&Object::Array(arr), None);
        assert_eq!(cs.components(), 1);
        let full_tint = cs.to_rgba(&[1.0], 1.0).unwrap();
        assert_eq!((full_tint.red(), full_tint.green(), full_tint.blue()), (1.0, 1.0, 1.0));
        let no_tint = cs.to_rgba(&[0.0], 1.0).unwrap();
        assert_eq!((no_tint.red(), no_tint.green(), no_tint.blue()), (0.0, 0.0, 0.0));
    }

    #[test]
    fn separation_missing_function_is_unsupported_not_panic() {
        let arr = PdfArray::from_objects(vec![name("Separation"), name("Spot"), name("DeviceGray")]);
        let cs = resolve_color_space(&Object::Array(arr), None);
        assert!(cs.is_unsupported());
    }

    #[test]
    fn icc_based_falls_back_to_alternate() {
        let mut stream_dict = PdfDictionary::new();
        stream_dict.set("N", Object::Integer(3));
        stream_dict.set("Alternate", name("DeviceRGB"));
        let stream = Object::Stream(crate::object::PdfStream::with_dictionary(stream_dict, vec![]));
        let arr = PdfArray::from_objects(vec![name("ICCBased"), stream]);
        let cs = resolve_color_space(&Object::Array(arr), None);
        assert!(matches!(cs, ColorSpace::IccApproximated { .. }));
        assert_eq!(cs.components(), 3);
    }

    #[test]
    fn icc_based_heuristic_from_n_when_no_alternate() {
        let mut stream_dict = PdfDictionary::new();
        stream_dict.set("N", Object::Integer(4));
        let stream = Object::Stream(crate::object::PdfStream::with_dictionary(stream_dict, vec![]));
        let arr = PdfArray::from_objects(vec![name("ICCBased"), stream]);
        let cs = resolve_color_space(&Object::Array(arr), None);
        assert_eq!(cs.components(), 4);
        // 4-component heuristic is DeviceCMYK: all-zero should be white.
        let white = cs.to_rgba(&[0.0, 0.0, 0.0, 0.0], 1.0).unwrap();
        assert_eq!((white.red(), white.green(), white.blue()), (1.0, 1.0, 1.0));
    }

    #[test]
    fn lab_and_pattern_are_not_silently_approximated() {
        let lab = resolve_color_space(&Object::Array(PdfArray::from_objects(vec![name("Lab")])), None);
        assert!(lab.is_unsupported());
        let pattern = resolve_color_space(&name("Pattern"), None);
        assert!(matches!(pattern, ColorSpace::Pattern));
        assert!(pattern.to_rgba(&[], 1.0).is_none());
    }

    #[test]
    fn default_decode_for_indexed_is_raw_range() {
        let cs = ColorSpace::Indexed {
            base: Box::new(ColorSpace::DeviceRGB),
            hival: 255,
            lookup: Vec::new(),
        };
        assert_eq!(default_decode(&cs, 8), vec![(0.0, 255.0)]);
    }

    #[test]
    fn default_decode_for_device_rgb_is_unit_range() {
        assert_eq!(default_decode(&ColorSpace::DeviceRGB, 8), vec![(0.0, 1.0); 3]);
    }
}
