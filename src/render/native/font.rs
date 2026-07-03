//! Resolves a page's `/Resources /Font` entries (ISO 32000-1:2008 9.6
//! "Simple Fonts", 9.7 "Composite Fonts", 9.6.5 "Type 3 Fonts") into
//! whatever this phase of the native renderer needs to paint glyphs: an
//! embedded, `ttf-parser`-loadable outline font plus glyph-selection/width
//! tables, or (honestly) a reason it cannot.
//!
//! # Scope of this phase, and what is an *explicit, documented* gap
//!
//! - **Embedded TrueType/OpenType programs only.** A font with no
//!   `FontFile`/`FontFile2`/`FontFile3` in its `/FontDescriptor` is *not*
//!   substituted with a system/standard font the way a mature desktop
//!   PDF viewer would -- this phase has no font-substitution database at all.
//!   Text using such a font renders nothing (but still advances the text
//!   position using its declared `/Widths`/`/W`, so the rest of the line
//!   doesn't visually collapse), and [`RenderWarning::UnsupportedFontProgram`]
//!   is recorded once per resource name.
//! - **Type1 and bare/un-wrapped CFF font programs are a hard, honest gap.**
//!   There is no mature pure-Rust Type1 (`eexec`-encrypted charstring) or
//!   bare-CFF interpreter in the ecosystem today (`ttf-parser`, this
//!   crate's only font-parsing dependency, requires an `sfnt`/OpenType
//!   table directory -- it cannot parse a raw Type1 `PFA`/`PFB` program or
//!   a CFF program that isn't wrapped in an OpenType container). Such
//!   fonts fail closed exactly like "not embedded" above: no glyphs
//!   painted, a warning recorded, never a panic and never a silently
//!   fabricated placeholder box mislabeled as "rendered". OpenType fonts
//!   whose outlines happen to be CFF-flavored (i.e. wrapped in a proper
//!   `sfnt` container) are **not** part of this gap -- `ttf-parser`
//!   genuinely parses those (see [`crate::font::truetype::FontFlavor::Cff`]),
//!   so this module attempts the load regardless of which `FontFile*` key
//!   the bytes came from and only classifies the result as the Type1/CFF
//!   gap if that attempt actually fails.
//! - **Only horizontal writing mode.** Vertical CID fonts (`Identity-V`
//!   and friends) are not detected or handled specially; glyphs are
//!   positioned as if horizontal, which will look wrong (but not panic)
//!   for a genuinely vertical layout.
//! - **Only a 2-byte code width is assumed for every Type0/composite
//!   font**, matching the same simplification already documented and
//!   shipped in [`crate::editor::text_extract`] (this crate's own writer,
//!   [`crate::font::cid`], only ever produces `Identity-H`, 2-byte codes).
//!   A composite font using a genuinely different (e.g. 1-byte or mixed
//!   multi-byte) predefined CMap will be mis-chunked.
//! - **Simple-font encoding**: character codes are mapped to a glyph via
//!   [`crate::font::encoding::win_ansi_to_unicode`] (this crate's existing
//!   WinAnsi-ish fallback table, already used the same way for text
//!   *extraction*) then the embedded font's own `cmap`. `/Encoding`
//!   `/Differences` overrides and symbolic (non-Unicode-cmap) fonts are
//!   not specially handled -- same documented limitation as
//!   `text_extract`, for the same reason (resolving a `/Differences`
//!   glyph name to a Unicode codepoint needs the Adobe Glyph List, which
//!   this crate does not implement).
//!
//! # Reuse of [`crate::font::cid`]'s conventions
//!
//! This module does not call into `crate::font::cid::CompositeFont`
//! directly (that type is this crate's *writer*: it turns Unicode text
//! into content-stream bytes plus the PDF object graph to embed a font).
//! But it reads composite fonts back out using exactly the conventions
//! that writer establishes and this crate's own generated PDFs rely on:
//! `Identity-H`, 2-byte codes that are the *CID*, and a CID that is either
//! the original glyph ID directly (`/CIDToGIDMap /Identity`, what
//! [`crate::font::cid::CompositeFont::build`] emits for a full/unsubset
//! embed) or looked up through an explicit `/CIDToGIDMap` stream (what it
//! emits for a subset embed) -- see [`CidToGidMap`] below.

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::font::encoding::win_ansi_to_unicode;
use crate::font::truetype::TrueTypeFont;
use crate::object::{Object, PdfDictionary};
use crate::types::Matrix;

/// Hard cap on Type 3 glyph-procedure recursion depth (ISO 32000-1:2008
/// 9.6.5 "Type 3 Fonts": a `CharProc` is itself a content stream that
/// could, in principle, `Tf` a *different* Type 3 font and show more
/// text, or -- crafted adversarially -- reference itself). Without this
/// bound, a self-referential or mutually-recursive set of Type 3 fonts
/// would recurse until the process's call stack overflows (an
/// uncatchable abort, not a recoverable `Result`), which this crate's
/// mandatory untrusted-input rules require bounding against.
pub(super) const MAX_TYPE3_DEPTH: usize = 6;

/// A resolved `/Resources /Font` entry, ready for the `Tf`/`Tj`-family
/// operators to use. See the [module docs](self) for exactly what each
/// variant does and does not implement.
pub(super) enum ResolvedFont {
    /// A simple (single-byte code) font (ISO 32000-1 9.6): `/TrueType`,
    /// `/Type1`, `/MMType1`, ... -- the `/Subtype` name itself doesn't
    /// determine renderability here, only whether an embedded,
    /// `ttf-parser`-loadable program is present (see [`FontProgram`]).
    Simple(SimpleFont),
    /// A composite (Type 0) font (9.7): CID-keyed, 2-byte codes assumed
    /// (see [module docs](self)).
    Composite(CompositeFontRt),
    /// A Type 3 font (9.6.5): glyphs are content-stream procedures, run
    /// recursively through this same interpreter.
    Type3(Type3Font),
}

impl ResolvedFont {
    /// The number of content-stream bytes that make up one character code
    /// for this font: 1 for simple and Type 3 fonts, 2 for composite
    /// fonts (see [module docs](self) for the 2-byte-only simplification).
    pub(super) fn code_width_bytes(&self) -> usize {
        match self {
            ResolvedFont::Simple(_) | ResolvedFont::Type3(_) => 1,
            ResolvedFont::Composite(_) => 2,
        }
    }

    /// The glyph displacement `w0` for `code`, in thousandths of a text
    /// space unit (ISO 32000-1 9.2.4) regardless of font kind -- Type 3's
    /// own glyph-space `/Widths` are pre-converted through `/FontMatrix`
    /// at resolution time (see [`Type3Font`]) so callers never need a
    /// separate code path.
    pub(super) fn width_1000(&self, code: u32) -> f64 {
        match self {
            ResolvedFont::Simple(f) => f.widths.width_1000(code),
            ResolvedFont::Composite(f) => f.widths.width_1000(code),
            ResolvedFont::Type3(f) => f.widths.width_1000(code),
        }
    }

    /// If this font resolved to an embedded program this phase cannot
    /// use (see [`UnsupportedFontReason`]), the reason -- so the caller
    /// can record one [`super::error::RenderWarning::UnsupportedFontProgram`]
    /// at `Tf` time rather than once per glyph. Type 3 fonts have no
    /// separate "program" to fail to load (their glyphs are content
    /// streams), so they never return one here.
    pub(super) fn unsupported_reason(&self) -> Option<&UnsupportedFontReason> {
        let program = match self {
            ResolvedFont::Simple(f) => &f.program,
            ResolvedFont::Composite(f) => &f.program,
            ResolvedFont::Type3(_) => return None,
        };
        match program {
            FontProgram::Unsupported(reason) => Some(reason),
            FontProgram::Loaded(_) => None,
        }
    }

    /// A placeholder used when `Tf` names a resource that couldn't be
    /// resolved at all (missing from `/Resources /Font`, or not a
    /// dereferenced dictionary) -- behaves exactly like a simple font
    /// with no embedded program (renders nothing, advances the pen by 0
    /// per glyph since it has no `/Widths` to consult either).
    pub(super) fn missing_placeholder() -> ResolvedFont {
        ResolvedFont::Simple(SimpleFont {
            program: FontProgram::Unsupported(UnsupportedFontReason::NotEmbedded),
            widths: SimpleWidths {
                first_char: 0,
                widths_1000: Vec::new(),
                missing_width_1000: 0.0,
            },
        })
    }
}

/// Why this phase cannot rasterize a font's embedded program (or the fact
/// that it has none) -- see the [module docs](self) for which of these is
/// the *expected, structural* gap (Type1/CFF) versus an unexpected/
/// adversarial one.
#[derive(Debug, Clone)]
pub(super) enum UnsupportedFontReason {
    /// No `FontFile`/`FontFile2`/`FontFile3` at all: this phase does not
    /// implement standard/system-font substitution.
    NotEmbedded,
    /// A `FontFile` (classic Type1) or bare/non-OpenType-wrapped CFF
    /// (`FontFile3` `/Subtype` `/Type1C` or `/CIDFontType0C`) program:
    /// the documented, structural "no pure-Rust Type1/CFF interpreter"
    /// gap.
    Type1OrBareCff,
    /// An embedded program that even `ttf-parser` rejected as malformed
    /// for a reason other than simply "not an `sfnt` container" (a
    /// corrupt/adversarial `FontFile2`, or an exotic flavor -- e.g.
    /// bitmap-only -- this phase can't shape).
    Unparseable,
}

impl std::fmt::Display for UnsupportedFontReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnsupportedFontReason::NotEmbedded => write!(f, "no embedded font program (font substitution not implemented)"),
            UnsupportedFontReason::Type1OrBareCff => {
                write!(f, "Type1/bare-CFF font program (no pure-Rust charstring interpreter available; known gap)")
            }
            UnsupportedFontReason::Unparseable => write!(f, "embedded font program could not be parsed"),
        }
    }
}

/// Either a loaded, outline-capable font program, or the reason this
/// phase couldn't get one.
pub(super) enum FontProgram {
    Loaded(Rc<TrueTypeFont>),
    Unsupported(UnsupportedFontReason),
}

/// A resolved simple (single-byte) font.
pub(super) struct SimpleFont {
    pub(super) program: FontProgram,
    widths: SimpleWidths,
}

/// A resolved composite (Type 0) font.
pub(super) struct CompositeFontRt {
    pub(super) program: FontProgram,
    cid_to_gid: CidToGidMap,
    widths: CompositeWidths,
}

/// `/CIDToGIDMap` (ISO 32000-1 9.7.4.2): either the identity (CID == GID,
/// the `/Identity` name -- also what an unsubset embed from this crate's
/// own [`crate::font::cid::CompositeFont`] always uses) or an explicit
/// big-endian `u16`-per-CID lookup stream.
enum CidToGidMap {
    Identity,
    Mapped(Vec<u16>),
}

impl CidToGidMap {
    fn gid_for(&self, cid: u32) -> u16 {
        match self {
            CidToGidMap::Identity => u16::try_from(cid).unwrap_or(0),
            CidToGidMap::Mapped(map) => usize::try_from(cid).ok().and_then(|i| map.get(i).copied()).unwrap_or(0),
        }
    }
}

/// A resolved Type 3 font (ISO 32000-1 9.6.5).
pub(super) struct Type3Font {
    /// Glyph space -> text space (Table 111's `/FontMatrix`; falls back
    /// to the conventional `[0.001 0 0 0.001 0 0]` -- same as a
    /// 1000-unit-per-em simple font -- if the font dictionary omits the
    /// technically-required entry, rather than refusing to render the
    /// font at all over one missing key).
    pub(super) font_matrix: Matrix,
    /// Code -> glyph name (`/Encoding` `/Differences`, Table 114; Type 3
    /// has no predefined base encodings, so only `/Differences` is
    /// meaningful).
    encoding: BTreeMap<u8, String>,
    /// Glyph name -> content-stream bytes (`/CharProcs`, 9.6.5.2).
    pub(super) char_procs: BTreeMap<String, Vec<u8>>,
    /// The font's own `/Resources` (9.6.5.3), if it declares one; a
    /// `CharProc` without its own falls back to the page's resources
    /// (handled by the caller, not this struct).
    pub(super) resources: Option<PdfDictionary>,
    widths: SimpleWidths,
}

impl Type3Font {
    /// The glyph name for `code`, if `/Encoding /Differences` assigns
    /// one.
    pub(super) fn glyph_name(&self, code: u32) -> Option<&str> {
        u8::try_from(code).ok().and_then(|c| self.encoding.get(&c)).map(String::as_str)
    }
}

/// `/Widths` (ISO 32000-1 9.6.3 Table 111, first form: `/FirstChar`
/// .. `/LastChar` `/Widths`), used by simple and Type 3 fonts.
struct SimpleWidths {
    first_char: u32,
    /// Already expressed as *thousandths of a text-space unit* --
    /// Type 3's glyph-space values are pre-multiplied by `/FontMatrix`'s
    /// horizontal scale (and by 1000) at resolution time so
    /// [`ResolvedFont::width_1000`] never needs a separate code path; see
    /// [`resolve_simple_widths`].
    widths_1000: Vec<f64>,
    missing_width_1000: f64,
}

impl SimpleWidths {
    fn width_1000(&self, code: u32) -> f64 {
        if code < self.first_char {
            return self.missing_width_1000;
        }
        let idx = (code - self.first_char) as usize;
        self.widths_1000.get(idx).copied().unwrap_or(self.missing_width_1000)
    }
}

/// `/W` (ISO 32000-1 9.7.4.3 Table 118), used by composite fonts.
struct CompositeWidths {
    map: BTreeMap<u32, f64>,
    default_width_1000: f64,
}

impl CompositeWidths {
    fn width_1000(&self, cid: u32) -> f64 {
        self.map.get(&cid).copied().unwrap_or(self.default_width_1000)
    }
}

fn as_f64(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) if r.is_finite() => Some(*r),
        _ => None,
    }
}

fn dict_of<'a>(dict: &'a PdfDictionary, key: &str) -> Option<&'a PdfDictionary> {
    match dict.get(key) {
        Some(Object::Dictionary(d)) => Some(d),
        _ => None,
    }
}

/// Extracts and fully decodes an embedded font program stream (ISO
/// 32000-1 Table 122's `FontFile`/`FontFile2`/`FontFile3`).
///
/// Font program streams are commonly `/Filter /FlateDecode`-compressed
/// (every mainstream PDF producer compresses large embedded font data),
/// so this must apply the stream's declared filter chain via
/// [`PdfStream::decode_all`] rather than reading [`PdfStream::data`]'s
/// raw, possibly-still-compressed bytes directly -- `ttf-parser` (or any
/// TrueType/OpenType parser) cannot parse zlib-compressed bytes as a font
/// program; passing them in unconditionally fails every compressed
/// `FontFile*`, which in practice is most of them. A stream whose filter
/// chain fails to decode is treated as absent (`None`) here, which the
/// caller already classifies as [`UnsupportedFontReason::Unparseable`] --
/// consistent with how any other unparseable font program degrades.
fn stream_bytes(dict: &PdfDictionary, key: &str) -> Option<Vec<u8>> {
    match dict.get(key) {
        Some(Object::Stream(s)) => s.decode_all().ok(),
        _ => None,
    }
}

/// Attempts to load *some* embedded font program out of `descriptor`
/// (ISO 32000-1 9.8, Table 122's `FontFile`/`FontFile2`/`FontFile3`),
/// classifying failure per the [module docs](self): tries every embedded
/// program key regardless of its conventional format, since `ttf-parser`
/// genuinely can load an OpenType-wrapped CFF program (whichever key it's
/// under) -- only a load that actually fails is classified as the
/// Type1/bare-CFF gap (and only when the source key makes that
/// classification accurate).
fn load_font_program(descriptor: &PdfDictionary) -> FontProgram {
    // Priority mirrors how a conforming reader would pick among these if
    // more than one were (invalidly) present: FontFile2 (TrueType) is
    // the least ambiguous, then FontFile3 (OpenType/CFF), then the
    // classic Type1 FontFile.
    let candidates: [(&str, bool); 3] = [
        ("FontFile2", false),
        ("FontFile3", is_bare_cff_subtype(descriptor)),
        ("FontFile", true),
    ];

    for (key, likely_gap) in candidates {
        let Some(bytes) = stream_bytes(descriptor, key) else {
            continue;
        };
        return match TrueTypeFont::load(bytes, 0) {
            Ok(ttf) => FontProgram::Loaded(Rc::new(ttf)),
            Err(_) if likely_gap => FontProgram::Unsupported(UnsupportedFontReason::Type1OrBareCff),
            Err(_) => FontProgram::Unsupported(UnsupportedFontReason::Unparseable),
        };
    }

    FontProgram::Unsupported(UnsupportedFontReason::NotEmbedded)
}

/// Whether a `FontFile3` stream's `/Subtype` (Table 126) names a bare,
/// non-`sfnt`-wrapped CFF program (`/Type1C` for a simple font,
/// `/CIDFontType0C` for a composite one) rather than `/OpenType` (which
/// *is* an `sfnt` container `ttf-parser` can read).
fn is_bare_cff_subtype(descriptor: &PdfDictionary) -> bool {
    match descriptor.get("FontFile3") {
        Some(Object::Stream(s)) => match s.dictionary.get("Subtype") {
            Some(Object::Name(n)) => n.as_str() != "OpenType",
            _ => true,
        },
        _ => false,
    }
}

/// Resolves a `/Resources /Font /<name>` dictionary (already dereferenced
/// -- see this module's [caller](super::interpreter) for the
/// pre-resolved-`Resources` assumption this whole `native-render` phase
/// makes) into a [`ResolvedFont`].
pub(super) fn resolve_font(font_dict: &PdfDictionary) -> ResolvedFont {
    let subtype = match font_dict.get("Subtype") {
        Some(Object::Name(n)) => n.as_str(),
        _ => "",
    };

    if subtype == "Type3" {
        return ResolvedFont::Type3(resolve_type3(font_dict));
    }

    if subtype == "Type0" {
        return ResolvedFont::Composite(resolve_composite(font_dict));
    }

    ResolvedFont::Simple(resolve_simple(font_dict))
}

fn resolve_simple(font_dict: &PdfDictionary) -> SimpleFont {
    let descriptor = dict_of(font_dict, "FontDescriptor");
    let program = descriptor.map(load_font_program).unwrap_or(FontProgram::Unsupported(UnsupportedFontReason::NotEmbedded));
    let missing_width_1000 = descriptor
        .and_then(|d| d.get("MissingWidth"))
        .and_then(as_f64)
        .unwrap_or(0.0);
    let widths = resolve_simple_widths(font_dict, 1.0, missing_width_1000);
    SimpleFont { program, widths }
}

fn resolve_composite(font_dict: &PdfDictionary) -> CompositeFontRt {
    let descendant = match font_dict.get("DescendantFonts") {
        Some(Object::Array(arr)) => arr.iter().find_map(|o| match o {
            Object::Dictionary(d) => Some(d),
            _ => None,
        }),
        _ => None,
    };
    let Some(descendant) = descendant else {
        return CompositeFontRt {
            program: FontProgram::Unsupported(UnsupportedFontReason::Unparseable),
            cid_to_gid: CidToGidMap::Identity,
            widths: CompositeWidths {
                map: BTreeMap::new(),
                default_width_1000: 1000.0,
            },
        };
    };

    let descriptor = dict_of(descendant, "FontDescriptor");
    let program = descriptor.map(load_font_program).unwrap_or(FontProgram::Unsupported(UnsupportedFontReason::NotEmbedded));

    let cid_to_gid = match descendant.get("CIDToGIDMap") {
        Some(Object::Stream(s)) => {
            let bytes = s.data();
            let map = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            CidToGidMap::Mapped(map)
        }
        _ => CidToGidMap::Identity,
    };

    let default_width_1000 = descendant.get("DW").and_then(as_f64).unwrap_or(1000.0);
    let mut map = BTreeMap::new();
    if let Some(Object::Array(w)) = descendant.get("W") {
        parse_w_array(w, &mut map);
    }

    CompositeFontRt {
        program,
        cid_to_gid,
        widths: CompositeWidths {
            map,
            default_width_1000,
        },
    }
}

/// Parses the `/W` array (ISO 32000-1 9.7.4.3 Table 118), both forms:
/// `c [w1 w2 ... wn]` (consecutive CIDs starting at `c`) and
/// `cFirst cLast w` (a whole range sharing one width). Malformed entries
/// (wrong operand types/truncated tail) stop parsing at that point rather
/// than panicking -- `/W` comes straight from an untrusted PDF.
fn parse_w_array(arr: &crate::object::PdfArray, out: &mut BTreeMap<u32, f64>) {
    let items: Vec<&Object> = arr.iter().collect();
    let mut i = 0;
    while i < items.len() {
        let Some(c_first) = as_f64(items[i]) else { break };
        let Some(next) = items.get(i + 1) else { break };
        match next {
            Object::Array(widths) => {
                let mut cid = c_first as u32;
                for w in widths.iter() {
                    if let Some(w) = as_f64(w) {
                        out.insert(cid, w);
                    }
                    cid = cid.saturating_add(1);
                }
                i += 2;
            }
            _ => {
                let (Some(c_last), Some(w)) = (as_f64(next), items.get(i + 2).and_then(|o| as_f64(o))) else {
                    break;
                };
                let c_last = c_last as u32;
                let w_val = w;
                let mut cid = c_first as u32;
                // Bound the range width itself: a crafted `/W` entry like
                // `0 4294967295 500` must not be expanded into billions of
                // map entries (untrusted input).
                const MAX_RANGE: u32 = 65536;
                while cid <= c_last && (cid - c_first as u32) < MAX_RANGE {
                    out.insert(cid, w_val);
                    cid += 1;
                }
                i += 3;
            }
        }
    }
}

fn resolve_type3(font_dict: &PdfDictionary) -> Type3Font {
    let font_matrix = match font_dict.get("FontMatrix") {
        Some(Object::Array(arr)) if arr.len() == 6 => {
            let v: Vec<f64> = arr.iter().filter_map(as_f64).collect();
            if v.len() == 6 {
                Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5])
            } else {
                Matrix::new(0.001, 0.0, 0.0, 0.001, 0.0, 0.0)
            }
        }
        // Technically required by ISO 32000-1 9.6.5.3, but absent isn't
        // worth refusing the whole font over -- fall back to the
        // conventional 1000-unit-em scale.
        _ => Matrix::new(0.001, 0.0, 0.0, 0.001, 0.0, 0.0),
    };

    let mut encoding = BTreeMap::new();
    if let Some(Object::Dictionary(enc)) = font_dict.get("Encoding") {
        if let Some(Object::Array(diffs)) = enc.get("Differences") {
            let mut code: u32 = 0;
            for item in diffs.iter() {
                match item {
                    Object::Integer(n) if *n >= 0 => code = *n as u32,
                    Object::Real(r) if r.is_finite() && *r >= 0.0 => code = *r as u32,
                    Object::Name(name) => {
                        if let Ok(c) = u8::try_from(code) {
                            encoding.insert(c, name.as_str().to_string());
                        }
                        code = code.saturating_add(1);
                    }
                    _ => {}
                }
            }
        }
    }

    let mut char_procs = BTreeMap::new();
    if let Some(Object::Dictionary(procs)) = font_dict.get("CharProcs") {
        for (name, obj) in procs.iter() {
            if let Object::Stream(s) = obj {
                char_procs.insert(name.clone(), s.data().to_vec());
            }
        }
    }

    let resources = dict_of(font_dict, "Resources").cloned();

    // Type 3 /Widths are in *glyph* space (9.6.5.3), scaled here by the
    // FontMatrix's horizontal component so `ResolvedFont::width_1000` can
    // stay agnostic to font kind.
    let widths = resolve_simple_widths(font_dict, font_matrix.a * 1000.0, 0.0);

    Type3Font {
        font_matrix,
        encoding,
        char_procs,
        resources,
        widths,
    }
}

/// Shared `/FirstChar`/`/LastChar`/`/Widths` parsing (Table 111), used by
/// both simple and Type 3 fonts. `scale_to_1000` converts each raw
/// `/Widths` entry to thousandths of a text-space unit: `1.0` for a
/// simple font (values are already in that unit) or
/// `FontMatrix.a * 1000.0` for Type 3 (values are in glyph space).
fn resolve_simple_widths(font_dict: &PdfDictionary, scale_to_1000: f64, missing_width_1000: f64) -> SimpleWidths {
    let first_char = font_dict.get("FirstChar").and_then(as_f64).unwrap_or(0.0) as u32;
    let widths_1000 = match font_dict.get("Widths") {
        Some(Object::Array(arr)) => arr.iter().map(|o| as_f64(o).unwrap_or(0.0) * scale_to_1000).collect(),
        _ => Vec::new(),
    };
    SimpleWidths {
        first_char,
        widths_1000,
        missing_width_1000,
    }
}

/// Looks up the glyph ID to paint for `code` against a loaded simple-font
/// program, via [`crate::font::encoding::win_ansi_to_unicode`] then the
/// font's own `cmap` (see [module docs](self) for why `/Differences`/
/// symbolic cmaps aren't specially handled).
pub(super) fn simple_glyph_id(ttf: &TrueTypeFont, code: u32) -> u16 {
    let Ok(byte) = u8::try_from(code) else {
        return 0;
    };
    let ch = win_ansi_to_unicode(byte);
    ttf.glyph_id(ch).unwrap_or(0)
}

/// Looks up the glyph ID to paint for CID `cid` against a loaded
/// composite-font program, honoring `/CIDToGIDMap` (see [`CidToGidMap`]).
pub(super) fn composite_glyph_id(font: &CompositeFontRt, cid: u32) -> u16 {
    font.cid_to_gid.gid_for(cid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::truetype::test_support::build_test_font;
    use crate::object::{PdfArray, PdfName, PdfStream};

    fn simple_truetype_dict(font_bytes: Vec<u8>) -> PdfDictionary {
        let mut descriptor = PdfDictionary::new();
        descriptor.set("FontFile2", Object::Stream(PdfStream::new(font_bytes)));
        let mut dict = PdfDictionary::new();
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("TrueType")));
        dict.set("FirstChar", Object::Integer(65));
        let mut widths = PdfArray::new();
        widths.push(Object::Integer(600));
        widths.push(Object::Integer(700));
        dict.set("Widths", Object::Array(widths));
        dict.set("FontDescriptor", Object::Dictionary(descriptor));
        dict
    }

    #[test]
    fn resolves_simple_truetype_with_loaded_program() {
        let font_bytes = build_test_font(&[('A', 1), ('B', 2)]);
        let dict = simple_truetype_dict(font_bytes);
        let resolved = resolve_font(&dict);
        match resolved {
            ResolvedFont::Simple(f) => {
                assert!(matches!(f.program, FontProgram::Loaded(_)));
                assert_eq!(f.widths.width_1000(65), 600.0);
                assert_eq!(f.widths.width_1000(66), 700.0);
                // Below FirstChar: falls back to MissingWidth (0 here).
                assert_eq!(f.widths.width_1000(64), 0.0);
            }
            _ => panic!("expected a Simple font"),
        }
    }

    /// Regression test for a real-world bug: `stream_bytes` used to read
    /// [`PdfStream::data`] (raw, still-filtered bytes) instead of applying
    /// the stream's `/Filter` chain via [`PdfStream::decode_all`]. Every
    /// mainstream PDF producer Flate-compresses embedded font programs
    /// (they are large), so this silently broke text rendering for the
    /// overwhelming majority of real-world PDFs with embedded TrueType
    /// fonts -- caught via a document with a genuinely `/FlateDecode`d
    /// `FontFile2` (`DejaVuSans`, as commonly embedded by e.g. WeasyPrint/
    /// ReportLab-family PDF producers), not one of this crate's own
    /// (uncompressed-by-default) test fixtures.
    #[test]
    fn resolves_flate_compressed_truetype_font_file2() {
        let font_bytes = build_test_font(&[('A', 1), ('B', 2)]);
        let compressed = PdfStream::new(font_bytes)
            .with_compression()
            .expect("compress");
        assert!(
            compressed.dictionary.get("Filter").is_some(),
            "test setup: with_compression() should have set /Filter"
        );

        let mut descriptor = PdfDictionary::new();
        descriptor.set("FontFile2", Object::Stream(compressed));
        let mut dict = PdfDictionary::new();
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("TrueType")));
        dict.set("FontDescriptor", Object::Dictionary(descriptor));

        let resolved = resolve_font(&dict);
        match resolved {
            ResolvedFont::Simple(f) => match f.program {
                FontProgram::Loaded(_) => {}
                FontProgram::Unsupported(reason) => {
                    panic!("expected a Flate-compressed FontFile2 to load, got: {reason}")
                }
            },
            _ => panic!("expected a Simple font"),
        }
    }

    #[test]
    fn simple_font_with_no_font_descriptor_is_not_embedded() {
        let mut dict = PdfDictionary::new();
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("TrueType")));
        let resolved = resolve_font(&dict);
        match resolved {
            ResolvedFont::Simple(f) => {
                assert!(matches!(
                    f.program,
                    FontProgram::Unsupported(UnsupportedFontReason::NotEmbedded)
                ));
            }
            _ => panic!("expected a Simple font"),
        }
    }

    #[test]
    fn bare_type1c_fontfile3_is_the_documented_type1_cff_gap() {
        let mut descriptor = PdfDictionary::new();
        let mut stream = PdfStream::new(b"garbage-not-an-sfnt-cff-program".to_vec());
        stream.dictionary.set("Subtype", Object::Name(PdfName::new_unchecked("Type1C")));
        descriptor.set("FontFile3", Object::Stream(stream));
        let mut dict = PdfDictionary::new();
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Type1")));
        dict.set("FontDescriptor", Object::Dictionary(descriptor));

        let resolved = resolve_font(&dict);
        match resolved {
            ResolvedFont::Simple(f) => {
                assert!(matches!(
                    f.program,
                    FontProgram::Unsupported(UnsupportedFontReason::Type1OrBareCff)
                ));
            }
            _ => panic!("expected a Simple font"),
        }
    }

    #[test]
    fn classic_fontfile_type1_is_the_documented_type1_cff_gap() {
        let mut descriptor = PdfDictionary::new();
        descriptor.set("FontFile", Object::Stream(PdfStream::new(b"%!PS-AdobeFont-1.0\ngarbage".to_vec())));
        let mut dict = PdfDictionary::new();
        dict.set("Subtype", Object::Name(PdfName::new_unchecked("Type1")));
        dict.set("FontDescriptor", Object::Dictionary(descriptor));

        let resolved = resolve_font(&dict);
        match resolved {
            ResolvedFont::Simple(f) => {
                assert!(matches!(
                    f.program,
                    FontProgram::Unsupported(UnsupportedFontReason::Type1OrBareCff)
                ));
            }
            _ => panic!("expected a Simple font"),
        }
    }

    #[test]
    fn resolves_composite_identity_cid_to_gid() {
        let font_bytes = build_test_font(&[('中', 3), ('日', 4)]);
        let mut descriptor = PdfDictionary::new();
        descriptor.set("FontFile2", Object::Stream(PdfStream::new(font_bytes)));
        let mut descendant = PdfDictionary::new();
        descendant.set("FontDescriptor", Object::Dictionary(descriptor));
        descendant.set("DW", Object::Integer(500));
        let mut w = PdfArray::new();
        w.push(Object::Integer(3));
        let mut ws = PdfArray::new();
        ws.push(Object::Integer(1000));
        ws.push(Object::Integer(999));
        w.push(Object::Array(ws));
        descendant.set("W", Object::Array(w));

        let mut font_dict = PdfDictionary::new();
        font_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Type0")));
        let mut descendants = PdfArray::new();
        descendants.push(Object::Dictionary(descendant));
        font_dict.set("DescendantFonts", Object::Array(descendants));

        let resolved = resolve_font(&font_dict);
        match resolved {
            ResolvedFont::Composite(f) => {
                assert!(matches!(f.program, FontProgram::Loaded(_)));
                assert_eq!(composite_glyph_id(&f, 3), 3); // Identity map
                assert_eq!(f.widths.width_1000(3), 1000.0);
                assert_eq!(f.widths.width_1000(4), 999.0);
                assert_eq!(f.widths.width_1000(999), 500.0); // falls back to DW
            }
            _ => panic!("expected a Composite font"),
        }
    }

    #[test]
    fn resolves_composite_with_explicit_cid_to_gid_map() {
        let font_bytes = build_test_font(&[('A', 1)]);
        let mut descriptor = PdfDictionary::new();
        descriptor.set("FontFile2", Object::Stream(PdfStream::new(font_bytes)));
        let mut descendant = PdfDictionary::new();
        descendant.set("FontDescriptor", Object::Dictionary(descriptor));
        // CID 5 -> GID 1.
        let mut map_bytes = vec![0u8; 12];
        map_bytes[10..12].copy_from_slice(&1u16.to_be_bytes());
        descendant.set("CIDToGIDMap", Object::Stream(PdfStream::new(map_bytes)));

        let mut font_dict = PdfDictionary::new();
        font_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Type0")));
        let mut descendants = PdfArray::new();
        descendants.push(Object::Dictionary(descendant));
        font_dict.set("DescendantFonts", Object::Array(descendants));

        let resolved = resolve_font(&font_dict);
        match resolved {
            ResolvedFont::Composite(f) => {
                assert_eq!(composite_glyph_id(&f, 5), 1);
                assert_eq!(composite_glyph_id(&f, 0), 0);
            }
            _ => panic!("expected a Composite font"),
        }
    }

    #[test]
    fn w_array_range_form_is_bounded_against_a_huge_range() {
        let mut out = BTreeMap::new();
        let mut arr = PdfArray::new();
        arr.push(Object::Integer(0));
        arr.push(Object::Integer(i64::from(u32::MAX)));
        arr.push(Object::Integer(500));
        parse_w_array(&arr, &mut out);
        assert!(out.len() <= 65536, "range expansion must be bounded, got {} entries", out.len());
    }

    #[test]
    fn resolves_type3_font_with_differences_and_char_procs() {
        let mut font_dict = PdfDictionary::new();
        font_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Type3")));
        let mut matrix = PdfArray::new();
        for v in [0.001, 0.0, 0.0, 0.001, 0.0, 0.0] {
            matrix.push(Object::Real(v));
        }
        font_dict.set("FontMatrix", Object::Array(matrix));

        let mut encoding = PdfDictionary::new();
        let mut diffs = PdfArray::new();
        diffs.push(Object::Integer(65));
        diffs.push(Object::Name(PdfName::new_unchecked("square")));
        encoding.set("Differences", Object::Array(diffs));
        font_dict.set("Encoding", Object::Dictionary(encoding));

        let mut char_procs = PdfDictionary::new();
        char_procs.set(
            "square",
            Object::Stream(PdfStream::new(b"0 0 500 500 re f".to_vec())),
        );
        font_dict.set("CharProcs", Object::Dictionary(char_procs));

        font_dict.set("FirstChar", Object::Integer(65));
        let mut widths = PdfArray::new();
        widths.push(Object::Integer(750)); // glyph-space units
        font_dict.set("Widths", Object::Array(widths));

        let resolved = resolve_font(&font_dict);
        match resolved {
            ResolvedFont::Type3(f) => {
                assert_eq!(f.glyph_name(65), Some("square"));
                assert!(f.char_procs.contains_key("square"));
                // 750 glyph-space units * FontMatrix.a(0.001) * 1000 == 750.
                assert_eq!(f.widths.width_1000(65), 750.0);
            }
            _ => panic!("expected a Type3 font"),
        }
    }

    #[test]
    fn missing_font_matrix_falls_back_to_1000_unit_convention() {
        let mut font_dict = PdfDictionary::new();
        font_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Type3")));
        let resolved = resolve_font(&font_dict);
        match resolved {
            ResolvedFont::Type3(f) => {
                assert_eq!(f.font_matrix.a, 0.001);
            }
            _ => panic!("expected a Type3 font"),
        }
    }
}
