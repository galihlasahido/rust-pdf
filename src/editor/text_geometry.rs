//! Shared text-run geometry helpers: approximate glyph-advance
//! measurement and text/CTM matrix bookkeeping for a content-stream
//! interpreter.
//!
//! Originally lived entirely inside [`super::redact`] (which needs a
//! text-showing operator's rendered bounding box to decide whether it
//! falls inside a redaction area); [`super::text_layout`] needs the exact
//! same measurement to place a selectable text-layer overlay over a
//! rendered page, so the pure, state-free pieces of that logic were
//! pulled out here (`pub(crate)`, not `pub` - this is glue between two
//! sibling modules in this crate, not part of the crate's public API) so
//! neither caller re-implements font-width lookup/matrix math
//! independently and the two can't silently drift apart.
//!
//! See [`super::redact`]'s module docs ("Known limitations") for the
//! caveats this approximation carries: fixed ascent/descent rather than
//! real `/FontDescriptor` metrics, no kerning-pair/ligature-aware
//! shaping, `/Widths`/`/W` advance widths when present and a generous
//! fixed fallback otherwise.

use super::graph::EditableDocument;
use crate::object::{Object, PdfArray, PdfDictionary};
use crate::types::Matrix;
use std::collections::BTreeMap;

/// Fallback glyph advance width (1000-unit glyph space, ISO 32000-1
/// 9.2.4) used when a simple font has no `/Widths` entry for a code (or
/// no `/Widths` array at all - common for a bare `/BaseFont` reference to
/// one of the 14 standard fonts with no embedded metrics) and for the
/// generic "no resolvable font resource at all" fallback. Deliberately
/// on the generous side of the standard-14 average-width range (~480-600
/// depending on font; see [`crate::font::Standard14Font::average_width`])
/// so an unmeasurable run is more likely to be (over-)estimated than
/// under, which is the safe direction both current callers want
/// ([`super::redact`]: over-redact rather than miss;
/// [`super::text_layout`]: an overlay slightly too wide is a harmless
/// cosmetic mismatch, one too narrow clips selectable text).
pub(crate) const DEFAULT_GLYPH_WIDTH_1000: f64 = 600.0;

/// Fallback vertical extent (1000-unit glyph space) above/below the
/// baseline used for every text run's bounding box, regardless of the
/// font's actual `/FontDescriptor` `/Ascent`/`/Descent`. Vertical extent
/// only affects how generously a run's box is drawn, never *which*
/// glyphs get measured, so a fixed, generous constant is a safe
/// simplification (see [`super::redact`]'s "Known limitations").
pub(crate) const DEFAULT_ASCENT_1000: f64 = 750.0;
pub(crate) const DEFAULT_DESCENT_1000: f64 = -250.0;

/// Hard cap on a single `/W` (Type 0 CID width array, ISO 32000-1
/// 9.7.4.3) range-form entry's span (`cFirst cLast w`), so a hostile
/// `/W` declaring e.g. `[0 4000000000 500]` cannot force allocation of
/// billions of map entries.
const MAX_CID_WIDTH_RANGE: i64 = 70_000;

/// Hard cap on the total number of CID->width entries accumulated from a
/// single font's `/W` array, independent of any one range's span.
const MAX_CID_WIDTH_ENTRIES: usize = 500_000;

/// Per-font glyph-advance lookup used to measure a text run's width. See
/// the [module docs](self) for why this is an approximation, not full
/// text shaping.
pub(crate) struct FontWidths {
    /// `1` for simple fonts, `2` for the `Identity-H`/`-V` composite-font
    /// assumption this crate uses elsewhere (see
    /// [`super::text_extract`]).
    code_width_bytes: usize,
    /// Advance width (1000-unit glyph space) for a code not present in
    /// `widths_1000` (`/W`'s `/DW`, ISO 32000-1 9.7.4.3, for composite
    /// fonts; [`DEFAULT_GLYPH_WIDTH_1000`] for simple fonts, since simple
    /// fonts have no per-font default-width entry in the spec).
    default_width_1000: f64,
    widths_1000: BTreeMap<u32, f64>,
}

impl FontWidths {
    pub(crate) fn from_font_dict(doc: &EditableDocument, dict: &PdfDictionary) -> Self {
        let is_composite = matches!(dict.get("Subtype"), Some(Object::Name(n)) if n.as_str() == "Type0");
        if is_composite {
            let mut widths_1000 = BTreeMap::new();
            let mut default_width_1000 = 1000.0; // ISO 32000-1 9.7.4.3: DW defaults to 1000.
            if let Some(descendant) = resolve_first_descendant(doc, dict) {
                if let Some(dw) = descendant.get("DW").and_then(|o| o.as_real()) {
                    default_width_1000 = dw;
                }
                let w_obj = match descendant.get("W") {
                    Some(Object::Reference(id)) => doc.get_object(*id),
                    other => other.cloned(),
                };
                if let Some(Object::Array(w)) = w_obj {
                    widths_1000 = parse_cid_widths(&w);
                }
            }
            FontWidths { code_width_bytes: 2, default_width_1000, widths_1000 }
        } else {
            let first_char = dict.get("FirstChar").and_then(|o| o.as_integer()).unwrap_or(0).max(0) as u32;
            let mut widths_1000 = BTreeMap::new();
            let widths_obj = match dict.get("Widths") {
                Some(Object::Reference(id)) => doc.get_object(*id),
                other => other.cloned(),
            };
            if let Some(Object::Array(arr)) = widths_obj {
                for (i, w) in arr.iter().enumerate() {
                    if let Some(w) = w.as_real() {
                        widths_1000.insert(first_char.saturating_add(i as u32), w);
                    }
                }
            }
            FontWidths {
                code_width_bytes: 1,
                default_width_1000: DEFAULT_GLYPH_WIDTH_1000,
                widths_1000,
            }
        }
    }

    /// Text-space horizontal displacement (ISO 32000-1 9.4.3) for showing
    /// `bytes` under the current text state.
    pub(crate) fn measure_advance(&self, bytes: &[u8], font_size: f64, char_spacing: f64, word_spacing: f64, h_scale: f64) -> f64 {
        let cw = self.code_width_bytes.max(1);
        let mut total = 0.0;
        let mut i = 0;
        while i + cw <= bytes.len() {
            let mut code: u32 = 0;
            for &b in &bytes[i..i + cw] {
                code = (code << 8) | u32::from(b);
            }
            let w0 = self.widths_1000.get(&code).copied().unwrap_or(self.default_width_1000) / 1000.0;
            let is_space = cw == 1 && code == 32;
            total += (w0 * font_size + char_spacing + if is_space { word_spacing } else { 0.0 }) * h_scale;
            i += cw;
        }
        total
    }
}

fn resolve_first_descendant(doc: &EditableDocument, dict: &PdfDictionary) -> Option<PdfDictionary> {
    let arr = match dict.get("DescendantFonts") {
        Some(Object::Array(arr)) => Some(arr.clone()),
        Some(Object::Reference(id)) => match doc.get_object(*id) {
            Some(Object::Array(arr)) => Some(arr),
            _ => None,
        },
        _ => None,
    }?;
    match arr.get(0)? {
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        Object::Dictionary(d) => Some(d.clone()),
        _ => None,
    }
}

/// Parses a `/W` array (ISO 32000-1 9.7.4.3, Table 117): a sequence of
/// either `c [w1 w2 ... wn]` (individual widths starting at CID `c`) or
/// `cFirst cLast w` (one width for the whole inclusive range) entries.
/// Bounded against a hostile/corrupt array (untrusted-input rule): see
/// [`MAX_CID_WIDTH_RANGE`]/[`MAX_CID_WIDTH_ENTRIES`].
fn parse_cid_widths(arr: &PdfArray) -> BTreeMap<u32, f64> {
    let mut out = BTreeMap::new();
    let items: Vec<&Object> = arr.iter().collect();
    let mut i = 0usize;
    while i < items.len() && out.len() < MAX_CID_WIDTH_ENTRIES {
        let Some(c_first) = items[i].as_integer() else { break };
        i += 1;
        if i >= items.len() {
            break;
        }
        match items[i] {
            Object::Array(list) => {
                for (k, w) in list.iter().enumerate() {
                    if out.len() >= MAX_CID_WIDTH_ENTRIES {
                        break;
                    }
                    if let Some(w) = w.as_real() {
                        out.insert((c_first.max(0) as u32).saturating_add(k as u32), w);
                    }
                }
                i += 1;
            }
            other => {
                let Some(c_last) = other.as_integer() else { break };
                i += 1;
                let Some(w) = items.get(i).and_then(|o| o.as_real()) else { break };
                i += 1;
                let c_last_bounded = c_last.max(c_first).min(c_first.saturating_add(MAX_CID_WIDTH_RANGE));
                let mut cid = c_first;
                while cid <= c_last_bounded && out.len() < MAX_CID_WIDTH_ENTRIES {
                    out.insert(cid.max(0) as u32, w);
                    cid += 1;
                }
            }
        }
    }
    out
}

/// One element of a `TJ` array (ISO 32000-1 9.4.3): either a string to
/// show, or a numeric position adjustment.
pub(crate) enum OwnedPiece {
    Str(Vec<u8>),
    Adj(f64),
}

pub(crate) fn owned_pieces_from_tj_array(arr: &PdfArray) -> Vec<OwnedPiece> {
    arr.iter()
        .filter_map(|e| match e {
            Object::String(s) => Some(OwnedPiece::Str(s.as_bytes().to_vec())),
            Object::Integer(n) => Some(OwnedPiece::Adj(*n as f64)),
            Object::Real(n) => Some(OwnedPiece::Adj(*n)),
            _ => None,
        })
        .collect()
}

/// Text-space advance for showing `bytes` when no font resource could be
/// resolved: assumes 1-byte codes and [`DEFAULT_GLYPH_WIDTH_1000`] per
/// code (matching [`super::text_extract`]'s WinAnsi-fallback convention
/// of treating an unresolvable font as single-byte).
pub(crate) fn measure_bytes_no_font(bytes: &[u8], font_size: f64, char_spacing: f64, word_spacing: f64, h_scale: f64) -> f64 {
    let w0 = DEFAULT_GLYPH_WIDTH_1000 / 1000.0;
    bytes
        .iter()
        .map(|&b| {
            let is_space = b == 32;
            (w0 * font_size + char_spacing + if is_space { word_spacing } else { 0.0 }) * h_scale
        })
        .sum()
}

/// Computes the total text-space displacement of showing `pieces` (ISO
/// 32000-1 9.4.3) and the resulting device-space bounding box
/// (transforming through `tm` then `ctm`). Returns `(new_tm, bbox)`;
/// `new_tm` is always the position-after-showing (ISO 32000-1 9.4.4) so
/// callers can unconditionally use it to keep tracking position, and
/// `bbox` is `(minx, miny, maxx, maxy)` in the same space `ctm` maps
/// into (page default user space when `ctm` is accumulated from the
/// page's own content stream, as both current callers do).
///
/// Named for what it computes rather than what either caller does with
/// it - [`super::redact`] additionally intersection-tests `bbox` against
/// a redaction target; [`super::text_layout`] uses it directly as the
/// text run's on-page box. (Previously `evaluate_show_text`, which also
/// took the redaction target and returned a `hit: bool`; hit-testing
/// moved to the caller so this function has no redaction-specific
/// knowledge.)
#[allow(clippy::too_many_arguments)]
pub(crate) fn compute_show_text_advance_and_bbox(
    pieces: &[OwnedPiece],
    font: Option<&FontWidths>,
    font_size: f64,
    char_spacing: f64,
    word_spacing: f64,
    h_scale: f64,
    rise: f64,
    tm: Matrix,
    ctm: Matrix,
) -> (Matrix, (f64, f64, f64, f64)) {
    let mut total_advance = 0.0;
    for p in pieces {
        total_advance += match p {
            OwnedPiece::Str(bytes) => match font {
                Some(fw) => fw.measure_advance(bytes, font_size, char_spacing, word_spacing, h_scale),
                None => measure_bytes_no_font(bytes, font_size, char_spacing, word_spacing, h_scale),
            },
            OwnedPiece::Adj(n) => -(n / 1000.0) * font_size * h_scale,
        };
    }
    let y0 = rise + DEFAULT_DESCENT_1000 / 1000.0 * font_size;
    let y1 = rise + DEFAULT_ASCENT_1000 / 1000.0 * font_size;
    let (x0, x1) = (total_advance.min(0.0), total_advance.max(0.0));
    let m = tm.multiply(&ctm);
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| m.transform_point(x, y));
    let bbox = aabb(&corners);
    let new_tm = Matrix::translate(total_advance, 0.0).multiply(&tm);
    (new_tm, bbox)
}

/// Axis-aligned bounding box of four (already device/user-space)
/// corners.
pub(crate) fn aabb(pts: &[(f64, f64); 4]) -> (f64, f64, f64, f64) {
    let minx = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let maxx = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let miny = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let maxy = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    (minx, miny, maxx, maxy)
}

/// Standard axis-aligned bounding box overlap test; `a`/`b` are
/// `(minx, miny, maxx, maxy)`.
pub(crate) fn rects_intersect(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// Parses a `cm`/`Tm`-style 6-operand matrix (the last 6 operands, so a
/// stray leading operand from a malformed operator doesn't shift the
/// parse).
pub(crate) fn matrix_from_operands(operands: &[Object]) -> Option<Matrix> {
    if operands.len() < 6 {
        return None;
    }
    let n = operands.len();
    let v: Vec<f64> = (n - 6..n).map(|i| operands[i].as_real()).collect::<Option<Vec<_>>>()?;
    Some(Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aabb_of_axis_aligned_square() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert_eq!(aabb(&pts), (0.0, 0.0, 10.0, 10.0));
    }

    #[test]
    fn rects_intersect_overlapping_and_disjoint() {
        assert!(rects_intersect((0.0, 0.0, 10.0, 10.0), (5.0, 5.0, 15.0, 15.0)));
        assert!(!rects_intersect((0.0, 0.0, 10.0, 10.0), (10.0, 10.0, 20.0, 20.0))); // touching edge, not overlapping
        assert!(!rects_intersect((0.0, 0.0, 10.0, 10.0), (20.0, 20.0, 30.0, 30.0)));
    }
}
